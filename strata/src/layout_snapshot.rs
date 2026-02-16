//! Layout Snapshot
//!
//! The `LayoutSnapshot` is the single source of truth for both rendering AND queries.
//! It captures all layout information during the layout pass and exposes it for:
//! - Hit-testing (screen point → content address)
//! - Character bounds (content address → screen rect)
//! - Selection rendering
//!
//! Character positions are computed once during layout and stored for
//! efficient querying by both the renderer and event handlers.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;

use crate::content_address::{ContentAddress, Selection, SourceId, SourceOrdering};
use crate::layout::PrimitiveBatch;
use crate::primitives::{Color, Point, Rect, Size};
use crate::text_engine::ShapedText;

/// Result of a hit test.
///
/// Distinguishes between character-level content hits (text, terminal cells)
/// and opaque widget hits (buttons, cards, panels).
#[derive(Debug, Clone)]
pub enum HitResult {
    /// Character-level hit on text or terminal content.
    Content(ContentAddress),
    /// Hit on a widget container identified by its SourceId.
    Widget(SourceId),
}

/// Cursor icon hint for mouse interaction feedback.
///
/// Set by widgets during layout to indicate what cursor should display
/// when hovering over them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorIcon {
    /// Default arrow cursor (non-interactive areas).
    #[default]
    Arrow,
    /// Text selection cursor (I-beam) for content areas.
    Text,
    /// Pointer/hand cursor for clickable elements.
    Pointer,
    /// Grab cursor for draggable elements (scrollbar thumb).
    Grab,
    /// Grabbing cursor — drag in progress.
    Grabbing,
    /// Copy cursor — drop will copy/insert data.
    Copy,
}

/// Anchor position for overlays relative to a widget.
#[derive(Debug, Clone, Copy)]
pub enum Anchor {
    /// Position below the widget, left-aligned.
    Below,
    /// Position above the widget, left-aligned.
    Above,
    /// Position to the right of the widget, top-aligned.
    Right,
    /// Position to the left of the widget, top-aligned.
    Left,
}

/// Info about a scroll track, used to convert mouse position to scroll offset.
#[derive(Debug, Clone, Copy)]
pub struct ScrollTrackInfo {
    /// Y position of the scroll track (top of viewport).
    pub track_y: f32,
    /// Height of the scroll track (viewport height).
    pub track_height: f32,
    /// Height of the scrollbar thumb.
    pub thumb_height: f32,
    /// Maximum scroll offset.
    pub max_scroll: f32,
}

impl ScrollTrackInfo {
    /// Convert a mouse Y position to a scroll offset.
    ///
    /// `grab_offset` is the distance from the top of the thumb to where the
    /// user initially clicked. This keeps the thumb anchored to the cursor
    /// instead of jumping on first drag.
    pub fn offset_from_y(&self, mouse_y: f32, grab_offset: f32) -> f32 {
        let available = self.track_height - self.thumb_height;
        if available <= 0.0 {
            return 0.0;
        }
        let thumb_top = mouse_y - grab_offset;
        let relative = (thumb_top - self.track_y).clamp(0.0, available);
        (relative / available) * self.max_scroll
    }

    /// Compute the current thumb top Y from a scroll offset.
    pub fn thumb_y(&self, scroll_offset: f32) -> f32 {
        let available = self.track_height - self.thumb_height;
        if available <= 0.0 || self.max_scroll <= 0.0 {
            return self.track_y;
        }
        self.track_y + (scroll_offset / self.max_scroll) * available
    }
}

// =========================================================================
// Debug Visualization (compiled out in release)
// =========================================================================

/// A debug rectangle for layout visualization.
///
/// These are only populated when debug mode is enabled in the LayoutContext.
/// Used to visualize container bounds, constraint stress, and layout hierarchy.
#[cfg(debug_assertions)]
#[derive(Debug, Clone)]
pub struct DebugRect {
    /// The bounds of this layout element.
    pub rect: Rect,
    /// Human-readable path/name (e.g., "Column > Row > Text").
    pub label: String,
    /// Depth in the layout tree (for color coding).
    pub depth: u32,
    /// Whether this element's size exceeded its constraints (overflow).
    pub is_overflow: bool,
}

#[cfg(debug_assertions)]
impl DebugRect {
    /// Get a color based on depth (cycles through a palette).
    pub fn color(&self) -> Color {
        const PALETTE: &[(f32, f32, f32)] = &[
            (0.8, 0.2, 0.2), // Red
            (0.2, 0.8, 0.2), // Green
            (0.2, 0.2, 0.8), // Blue
            (0.8, 0.8, 0.2), // Yellow
            (0.8, 0.2, 0.8), // Magenta
            (0.2, 0.8, 0.8), // Cyan
            (0.9, 0.5, 0.2), // Orange
            (0.5, 0.2, 0.9), // Purple
        ];
        let (r, g, b) = PALETTE[self.depth as usize % PALETTE.len()];
        if self.is_overflow {
            // Bright red for overflow
            Color::rgba(1.0, 0.0, 0.0, 0.5)
        } else {
            Color::rgba(r, g, b, 0.3)
        }
    }
}

/// A decoration primitive for non-text rendering.
///
/// These are rendered via the ubershader along with glyphs.
#[derive(Debug, Clone)]
pub enum Decoration {
    /// A solid rectangle (sharp corners).
    SolidRect { rect: Rect, color: Color },

    /// A rounded rectangle with corner radius.
    RoundedRect {
        rect: Rect,
        corner_radius: f32,
        color: Color,
    },

    /// A circle (rendered as a rounded rect where radius = size/2).
    Circle {
        center: Point,
        radius: f32,
        color: Color,
    },
}

/// Layout information for text content.
///
/// Stores character positions computed during text shaping,
/// enabling accurate hit-testing and selection rendering.
#[derive(Debug, Clone)]
pub struct TextLayout {
    /// The actual text content (for rendering).
    /// Uses Cow to avoid allocation for string literals.
    pub text: Cow<'static, str>,

    /// Foreground color (packed RGBA).
    pub color: u32,

    /// Bounding rectangle of the text.
    pub bounds: Rect,

    /// X position of each character relative to bounds.x.
    /// char_positions[i] = x offset of character i's left edge.
    pub char_positions: Vec<f32>,

    /// Width of each character (for selection rendering).
    /// If empty, widths are computed from char_positions.
    pub char_widths: Vec<f32>,

    /// Indices where lines break (character index of first char on new line).
    /// Line 0 starts at index 0 (implicit).
    /// Line 1 starts at line_breaks[0], etc.
    pub line_breaks: Vec<usize>,

    /// Height of each line.
    pub line_height: f32,

    /// Total character count.
    pub char_count: usize,

    /// Fallback advance width for characters without shaped width data.
    /// Set to the monospace cell width the layout was built with.
    pub fallback_advance: f32,
}

impl TextLayout {
    /// Create a new text layout.
    pub fn new(
        text: impl Into<Cow<'static, str>>,
        color: u32,
        bounds: Rect,
        char_positions: Vec<f32>,
        line_breaks: Vec<usize>,
        line_height: f32,
    ) -> Self {
        let text = text.into();
        let char_count = char_positions.len();
        Self {
            text,
            color,
            bounds,
            char_positions,
            char_widths: Vec::new(), // Computed on demand
            line_breaks,
            line_height,
            char_count,
            fallback_advance: 8.0, // Sensible default for tests/manual construction
        }
    }

    /// Create a simple single-line text layout with accurate character positions.
    ///
    /// For ASCII-only text in a monospace font, uses a fast path with fixed
    /// `char_width` spacing. For text containing non-ASCII characters, shapes
    /// with cosmic-text for accurate positions (ligatures, CJK, emoji, etc.).
    ///
    /// Results are cached in a thread-local LRU to avoid re-shaping per frame.
    pub fn simple(
        text: impl Into<Cow<'static, str>>,
        color: u32,
        x: f32,
        y: f32,
        char_width: f32,
        line_height: f32,
    ) -> Self {
        let text = text.into();
        let char_count = text.chars().count();

        if char_count == 0 {
            return Self {
                text,
                color,
                bounds: Rect::new(x, y, 0.0, line_height),
                char_positions: Vec::new(),
                char_widths: Vec::new(),
                line_breaks: Vec::new(),
                line_height,
                char_count: 0,
                fallback_advance: char_width,
            };
        }

        // Fast path: pure ASCII in monospace — fixed-width positions
        if text.is_ascii() {
            let mut char_positions = Vec::with_capacity(char_count);
            let mut char_widths_vec = Vec::with_capacity(char_count);
            for i in 0..char_count {
                char_positions.push(i as f32 * char_width);
                char_widths_vec.push(char_width);
            }
            let total_width = char_count as f32 * char_width;
            return Self {
                text,
                color,
                bounds: Rect::new(x, y, total_width, line_height),
                char_positions,
                char_widths: char_widths_vec,
                line_breaks: Vec::new(),
                line_height,
                char_count,
                fallback_advance: char_width,
            };
        }

        // Slow path: shape with cosmic-text for non-ASCII text.
        // Use a thread-local cache to avoid reshaping identical text each frame.
        use std::cell::RefCell;
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        use std::num::NonZeroUsize;

        thread_local! {
            static SHAPE_CACHE: RefCell<lru::LruCache<u64, (Vec<f32>, Vec<f32>)>> =
                RefCell::new(lru::LruCache::new(NonZeroUsize::new(512).unwrap()));
        }

        let cache_key = {
            let mut hasher = DefaultHasher::new();
            text.hash(&mut hasher);
            char_width.to_bits().hash(&mut hasher);
            hasher.finish()
        };

        // Check cache
        let cached = SHAPE_CACHE.with(|cache| {
            cache.borrow_mut().get(&cache_key).cloned()
        });

        if let Some((char_positions, char_widths_vec)) = cached {
            let total_width = max_extent(&char_positions, &char_widths_vec);
            return Self {
                text,
                color,
                bounds: Rect::new(x, y, total_width, line_height),
                char_positions,
                char_widths: char_widths_vec,
                line_breaks: Vec::new(),
                line_height,
                char_count,
                fallback_advance: char_width,
            };
        }

        // Cache miss — shape with cosmic-text
        let (char_positions, char_widths_vec) = Self::shape_for_layout(&text, char_count, char_width);

        // Store in cache
        SHAPE_CACHE.with(|cache| {
            cache.borrow_mut().put(cache_key, (char_positions.clone(), char_widths_vec.clone()));
        });

        let total_width = max_extent(&char_positions, &char_widths_vec);

        Self {
            text,
            color,
            bounds: Rect::new(x, y, total_width, line_height),
            char_positions,
            char_widths: char_widths_vec,
            line_breaks: Vec::new(),
            line_height,
            char_count,
            fallback_advance: char_width,
        }
    }

    /// Shape text with cosmic-text and extract per-character positions/widths.
    fn shape_for_layout(text: &str, char_count: usize, char_width: f32) -> (Vec<f32>, Vec<f32>) {
        use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping};

        // Derive font_size from char_width using the known ratio:
        // CHAR_WIDTH (8.4) corresponds to BASE_FONT_SIZE (14.0)
        let font_size = char_width / 8.4 * 14.0;

        let fs_mutex = crate::text_engine::get_font_system();
        let mut font_system = fs_mutex.lock().unwrap();

        let metrics = Metrics::new(font_size, font_size * 1.2);
        let mut buffer = Buffer::new(&mut font_system, metrics);
        buffer.set_size(&mut font_system, Some(f32::MAX), Some(f32::MAX));
        let attrs = Attrs::new().family(Family::Monospace);
        buffer.set_text(&mut font_system, text, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut font_system, false);

        // Build byte_offset → char_index mapping.
        // Each byte maps to the char index whose byte range contains it.
        let byte_len = text.len();
        let mut byte_to_char = vec![0usize; byte_len + 1];
        for (char_idx, (byte_idx, ch)) in text.char_indices().enumerate() {
            let end = byte_idx + ch.len_utf8();
            for b in byte_idx..end {
                byte_to_char[b] = char_idx;
            }
        }
        // The entry at byte_len maps past the last char
        byte_to_char[byte_len] = char_count;

        // Extract per-character positions from shaped glyphs.
        // Track which chars were covered by a glyph cluster so we can
        // assign fallback widths to uncovered chars (e.g. trailing whitespace).
        let mut char_positions = vec![f32::NAN; char_count];
        let mut char_widths_vec = vec![0.0_f32; char_count];
        let mut covered = vec![false; char_count];

        for run in buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                let ci_start = if glyph.start < byte_to_char.len() {
                    byte_to_char[glyph.start]
                } else {
                    continue;
                };
                let ci_end = if glyph.end <= byte_len {
                    byte_to_char[glyph.end.min(byte_len)]
                } else {
                    char_count
                };

                // First char of this glyph cluster gets the position and width
                if ci_start < char_count && char_positions[ci_start].is_nan() {
                    char_positions[ci_start] = glyph.x;
                    char_widths_vec[ci_start] = glyph.w;
                    covered[ci_start] = true;
                }

                // Interior chars of multi-codepoint clusters get same position, 0 width.
                // Mark them as covered (they're part of a cluster, not orphaned).
                for interior in (ci_start + 1)..ci_end.min(char_count) {
                    if char_positions[interior].is_nan() {
                        char_positions[interior] = glyph.x;
                        covered[interior] = true;
                        // width stays 0 — they're interior to a cluster
                    }
                }
            }
        }

        // Fill uncovered chars (not part of any glyph cluster):
        // assign fallback advance width so they're selectable/hittable.
        let mut prev_end = 0.0_f32;
        for i in 0..char_count {
            if char_positions[i].is_nan() {
                char_positions[i] = prev_end;
            }
            if !covered[i] {
                char_widths_vec[i] = char_width;
            }
            prev_end = char_positions[i] + char_widths_vec[i];
        }

        (char_positions, char_widths_vec)
    }

    /// Create a text layout from shaped text.
    ///
    /// This uses cosmic-text shaping results for accurate character positions.
    /// The position (x, y) specifies the top-left corner of the text bounds.
    pub fn from_shaped(shaped: &ShapedText, x: f32, y: f32) -> Self {
        Self {
            text: shaped.text.clone(),
            color: shaped.color.pack(),
            bounds: Rect::new(x, y, shaped.width, shaped.height),
            char_positions: shaped.char_positions.clone(),
            char_widths: shaped.char_widths.clone(),
            line_breaks: shaped.line_breaks.clone(),
            line_height: shaped.line_height,
            char_count: shaped.char_positions.len(),
            fallback_advance: 8.0, // Default; shaped text should have accurate widths
        }
    }

    /// Get the line number for a character offset.
    pub fn line_for_offset(&self, offset: usize) -> usize {
        self.line_breaks
            .iter()
            .position(|&b| b > offset)
            .unwrap_or(self.line_breaks.len())
    }

    /// Get the character range for a line.
    pub fn line_range(&self, line: usize) -> (usize, usize) {
        let start = if line == 0 {
            0
        } else {
            self.line_breaks.get(line - 1).copied().unwrap_or(0)
        };
        let end = self
            .line_breaks
            .get(line)
            .copied()
            .unwrap_or(self.char_count);
        (start, end)
    }

    /// Get the Y position of a line relative to bounds.y.
    pub fn line_y(&self, line: usize) -> f32 {
        line as f32 * self.line_height
    }

    /// Get the number of lines.
    pub fn line_count(&self) -> usize {
        self.line_breaks.len() + 1
    }
}

/// Underline style for rendering.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// Style flags for a text run in a grid row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: UnderlineStyle,
    pub strikethrough: bool,
    pub dim: bool,
}

/// A styled text run within a grid row.
#[derive(Debug, Clone)]
pub struct TextRun {
    /// Text content for this run.
    pub text: String,
    /// Foreground color (packed RGBA).
    pub fg: u32,
    /// Background color (packed RGBA), 0 = default/transparent.
    pub bg: u32,
    /// Column offset from row start (in cell units).
    pub col_offset: u16,
    /// Width of this run in terminal cell units (accounts for wide chars, combining marks).
    pub cell_len: u16,
    /// Style flags.
    pub style: RunStyle,
}

/// A row of text in a grid layout.
#[derive(Debug, Clone)]
pub struct GridRow {
    /// Styled text runs for this row.
    pub runs: Vec<TextRun>,
}

/// Layout information for grid content (terminals).
#[derive(Debug, Clone)]
pub struct GridLayout {
    /// Bounding rectangle of the grid.
    pub bounds: Rect,

    /// Width of each cell.
    pub cell_width: f32,

    /// Height of each cell.
    pub cell_height: f32,

    /// Number of columns.
    pub cols: u16,

    /// Number of rows.
    pub rows: u16,

    /// Row content for rendering.
    pub rows_content: Vec<GridRow>,

    /// Clip rectangle for this grid (from container clipping).
    pub clip_rect: Option<Rect>,
}

impl GridLayout {
    /// Create a new grid layout.
    pub fn new(bounds: Rect, cell_width: f32, cell_height: f32, cols: u16, rows: u16) -> Self {
        Self {
            bounds,
            cell_width,
            cell_height,
            cols,
            rows,
            rows_content: Vec::new(),
            clip_rect: None,
        }
    }

    /// Create a grid layout with row content.
    pub fn with_rows(
        bounds: Rect,
        cell_width: f32,
        cell_height: f32,
        cols: u16,
        rows: u16,
        rows_content: Vec<GridRow>,
    ) -> Self {
        Self {
            bounds,
            cell_width,
            cell_height,
            cols,
            rows,
            rows_content,
            clip_rect: None,
        }
    }

    /// Convert a linear offset to (col, row).
    pub fn offset_to_grid(&self, offset: usize) -> (u16, u16) {
        let col = (offset % self.cols as usize) as u16;
        let row = (offset / self.cols as usize) as u16;
        (col, row)
    }

    /// Convert (col, row) to a linear offset.
    pub fn grid_to_offset(&self, col: u16, row: u16) -> usize {
        row as usize * self.cols as usize + col as usize
    }

    /// Get the bounds of a cell at (col, row).
    pub fn cell_bounds(&self, col: u16, row: u16) -> Rect {
        Rect {
            x: self.bounds.x + col as f32 * self.cell_width,
            y: self.bounds.y + row as f32 * self.cell_height,
            width: self.cell_width,
            height: self.cell_height,
        }
    }

    /// Total number of cells.
    pub fn cell_count(&self) -> usize {
        self.cols as usize * self.rows as usize
    }
}

/// Layout information for a single item within a source.
#[derive(Debug, Clone)]
pub enum ItemLayout {
    /// Text content with character-level positions.
    Text(TextLayout),

    /// Grid content (terminal).
    Grid(GridLayout),
}

impl ItemLayout {
    /// Get the bounding rectangle of this item.
    pub fn bounds(&self) -> Rect {
        match self {
            ItemLayout::Text(t) => t.bounds,
            ItemLayout::Grid(g) => g.bounds,
        }
    }
}

/// Layout information for a source (collection of items).
#[derive(Debug, Clone)]
pub struct SourceLayout {
    /// Overall bounds of the source.
    pub bounds: Rect,

    /// Layout of individual items within the source.
    /// For terminals, typically a single Grid item.
    /// For documents, multiple Text items (paragraphs).
    pub items: Vec<ItemLayout>,

    /// Clip rectangle inherited from the containing scroll container.
    /// Used to clip selection highlights so they don't overflow.
    pub clip_rect: Option<Rect>,
}

impl SourceLayout {
    /// Create a new source layout with no items.
    pub fn new(bounds: Rect) -> Self {
        Self {
            bounds,
            items: Vec::new(),
            clip_rect: None,
        }
    }

    /// Create a source layout with a single text item.
    pub fn text(text_layout: TextLayout) -> Self {
        let bounds = text_layout.bounds;
        Self {
            bounds,
            items: vec![ItemLayout::Text(text_layout)],
            clip_rect: None,
        }
    }

    /// Create a source layout with a single grid item.
    pub fn grid(grid_layout: GridLayout) -> Self {
        let bounds = grid_layout.bounds;
        Self {
            bounds,
            items: vec![ItemLayout::Grid(grid_layout)],
            clip_rect: None,
        }
    }
}

/// The layout snapshot captures all layout information for a frame.
///
/// Built once during layout, used by both rendering and queries.
/// This is the core type for layout-based hit-testing.
#[derive(Debug, Clone)]
pub struct LayoutSnapshot {
    /// Layout information for each registered source.
    sources: HashMap<SourceId, SourceLayout>,

    /// Document ordering of sources.
    source_ordering: SourceOrdering,

    /// Current viewport (for culling).
    viewport: Rect,

    /// Decoration primitives (solid rects, rounded rects, circles).
    /// Rendered BEFORE text (background layer).
    background_decorations: Vec<Decoration>,

    /// Decoration primitives rendered AFTER text (foreground layer).
    foreground_decorations: Vec<Decoration>,

    /// Direct primitive batch for high-performance rendering.
    /// This is the "escape hatch" for canvas-like drawing.
    primitives: PrimitiveBatch,

    /// Overlay primitive batch — rendered LAST, on top of everything.
    /// Use for popups, context menus, tooltips that must appear above all content.
    overlay_primitives: PrimitiveBatch,

    /// Bounds of widgets registered with an ID.
    /// Used for hit-testing non-content areas (buttons, panels) and overlay anchoring.
    widget_bounds: HashMap<SourceId, Rect>,

    /// Max scroll values for ScrollColumn containers.
    /// Written during layout, readable by the app to clamp scroll offsets.
    scroll_limits: HashMap<SourceId, f32>,

    /// Scroll track info for ScrollColumn containers.
    /// Maps scroll container ID → (track_y, track_height, thumb_height, max_scroll).
    /// Used to convert mouse Y position to scroll offset during thumb dragging.
    scroll_tracks: HashMap<SourceId, ScrollTrackInfo>,

    /// Cursor hints for widgets. Set during layout, queried by mouse_interaction().
    cursor_hints: HashMap<SourceId, CursorIcon>,

    /// Debug rectangles for layout visualization (debug builds only).
    /// Populated when LayoutContext has debug mode enabled.
    #[cfg(debug_assertions)]
    debug_rects: Vec<DebugRect>,

    /// Debug mode enabled flag (debug builds only).
    /// Set by LayoutContext when debug is enabled, read by legacy layout methods.
    #[cfg(debug_assertions)]
    debug_enabled: bool,

    /// Current debug depth for nested layouts (debug builds only).
    #[cfg(debug_assertions)]
    debug_depth: u32,

    /// Zoom level (1.0 = 100%). Used by the GPU renderer to scale all content
    /// and by the adapter to adjust mouse coordinates.
    zoom_level: f32,
}

impl Default for LayoutSnapshot {
    fn default() -> Self {
        Self::new()
    }
}

impl LayoutSnapshot {
    /// Create a new empty layout snapshot.
    pub fn new() -> Self {
        Self {
            sources: HashMap::new(),
            source_ordering: SourceOrdering::new(),
            viewport: Rect::ZERO,
            background_decorations: Vec::new(),
            foreground_decorations: Vec::new(),
            primitives: PrimitiveBatch::new(),
            overlay_primitives: PrimitiveBatch::new(),
            widget_bounds: HashMap::new(),
            scroll_limits: HashMap::new(),
            scroll_tracks: HashMap::new(),
            cursor_hints: HashMap::new(),
            #[cfg(debug_assertions)]
            debug_rects: Vec::new(),
            #[cfg(debug_assertions)]
            debug_enabled: false,
            #[cfg(debug_assertions)]
            debug_depth: 0,
            zoom_level: 1.0,
        }
    }

    /// Clear all sources (call at start of each frame's layout pass).
    pub fn clear(&mut self) {
        self.sources.clear();
        self.source_ordering.clear();
        self.background_decorations.clear();
        self.foreground_decorations.clear();
        self.primitives.clear();
        self.overlay_primitives.clear();
        self.widget_bounds.clear();
        self.scroll_limits.clear();
        self.scroll_tracks.clear();
        self.cursor_hints.clear();
        #[cfg(debug_assertions)]
        {
            self.debug_rects.clear();
            self.debug_enabled = false;
            self.debug_depth = 0;
        }
    }

    /// Get read-only access to the primitive batch.
    ///
    /// Use this for inspecting primitives added by the layout system.
    pub fn primitives(&self) -> &PrimitiveBatch {
        &self.primitives
    }

    /// Get read-only access to the overlay primitive batch.
    pub fn overlay_primitives(&self) -> &PrimitiveBatch {
        &self.overlay_primitives
    }

    /// Get mutable access to the overlay primitive batch.
    ///
    /// Overlay primitives render on top of ALL content (text, grids, etc.).
    /// Use for context menus, tooltips, popups.
    pub fn overlay_primitives_mut(&mut self) -> &mut PrimitiveBatch {
        &mut self.overlay_primitives
    }

    /// Get mutable access to the primitive batch.
    ///
    /// This is the fast path for direct GPU instance creation.
    /// Primitives added here bypass the widget system entirely.
    pub fn primitives_mut(&mut self) -> &mut PrimitiveBatch {
        &mut self.primitives
    }

    /// Get the current clip rect from the primitive batch's clip stack.
    ///
    /// Used by layout containers to propagate clip info to non-primitive
    /// render paths (e.g., GridLayout for terminal content).
    pub fn current_clip(&self) -> Option<Rect> {
        self.primitives.current_clip_public()
    }

    /// Register a widget with its bounds for hit-testing and overlay anchoring.
    pub fn register_widget(&mut self, id: SourceId, bounds: Rect) {
        self.widget_bounds.insert(id, bounds);
    }

    /// Get the bounds of a registered widget.
    pub fn widget_bounds(&self, id: &SourceId) -> Option<Rect> {
        self.widget_bounds.get(id).copied()
    }

    /// Set a cursor hint for a widget. Called during layout by framework containers.
    pub fn set_cursor_hint(&mut self, id: SourceId, cursor: CursorIcon) {
        self.cursor_hints.insert(id, cursor);
    }

    /// Resolve the cursor icon for a screen position.
    ///
    /// Resolution: Content → Text, Widget → hint (default Arrow), None → Arrow.
    pub fn cursor_at(&self, pos: Point) -> CursorIcon {
        match self.hit_test(pos) {
            Some(HitResult::Content(_)) => CursorIcon::Text,
            Some(HitResult::Widget(id)) => {
                self.cursor_hints.get(&id).copied().unwrap_or_default()
            }
            None => CursorIcon::Arrow,
        }
    }

    /// Resolve the cursor icon during a capture (drag).
    ///
    /// Looks up the hint for the captured source: Grab → Grabbing (active drag),
    /// no hint (content) → Text (text selection).
    pub fn cursor_for_capture(&self, source: SourceId) -> CursorIcon {
        match self.cursor_hints.get(&source) {
            Some(CursorIcon::Grab) => CursorIcon::Grabbing,
            Some(icon) => *icon,
            None => CursorIcon::Text, // Content capture = text selection
        }
    }

    /// Compute the position of an overlay anchored to a widget.
    ///
    /// Returns `None` if the widget ID is not registered.
    pub fn anchor_to(&self, id: &SourceId, anchor: Anchor, size: Size) -> Option<Rect> {
        let wb = self.widget_bounds.get(id)?;
        let (x, y) = match anchor {
            Anchor::Below => (wb.x, wb.bottom()),
            Anchor::Above => (wb.x, wb.y - size.height),
            Anchor::Right => (wb.right(), wb.y),
            Anchor::Left => (wb.x - size.width, wb.y),
        };
        Some(Rect::new(x, y, size.width, size.height))
    }

    /// Record the max scroll value for a ScrollColumn.
    pub fn set_scroll_limit(&mut self, id: SourceId, max_scroll: f32) {
        self.scroll_limits.insert(id, max_scroll);
    }

    /// Get the max scroll value for a ScrollColumn.
    pub fn scroll_limit(&self, id: &SourceId) -> Option<f32> {
        self.scroll_limits.get(id).copied()
    }

    /// Record scroll track info for a ScrollColumn.
    pub fn set_scroll_track(&mut self, id: SourceId, info: ScrollTrackInfo) {
        self.scroll_tracks.insert(id, info);
    }

    /// Get scroll track info for a ScrollColumn.
    pub fn scroll_track(&self, id: &SourceId) -> Option<&ScrollTrackInfo> {
        self.scroll_tracks.get(id)
    }

    /// Add a background decoration (rendered behind text).
    pub fn add_background(&mut self, decoration: Decoration) {
        self.background_decorations.push(decoration);
    }

    /// Add a foreground decoration (rendered in front of text).
    pub fn add_foreground(&mut self, decoration: Decoration) {
        self.foreground_decorations.push(decoration);
    }

    /// Add a solid rectangle background.
    pub fn add_solid_rect(&mut self, rect: Rect, color: Color) {
        self.background_decorations
            .push(Decoration::SolidRect { rect, color });
    }

    /// Add a rounded rectangle background.
    pub fn add_rounded_rect(&mut self, rect: Rect, corner_radius: f32, color: Color) {
        self.background_decorations.push(Decoration::RoundedRect {
            rect,
            corner_radius,
            color,
        });
    }

    /// Add a circle background.
    pub fn add_circle(&mut self, center: Point, radius: f32, color: Color) {
        self.background_decorations.push(Decoration::Circle {
            center,
            radius,
            color,
        });
    }

    /// Get background decorations.
    pub fn background_decorations(&self) -> &[Decoration] {
        &self.background_decorations
    }

    /// Get foreground decorations.
    pub fn foreground_decorations(&self) -> &[Decoration] {
        &self.foreground_decorations
    }

    /// Set the viewport rectangle.
    pub fn set_viewport(&mut self, viewport: Rect) {
        self.viewport = viewport;
    }

    /// Get the viewport rectangle.
    pub fn viewport(&self) -> Rect {
        self.viewport
    }

    /// Set the zoom level (1.0 = 100%).
    pub fn set_zoom_level(&mut self, zoom: f32) {
        self.zoom_level = zoom;
    }

    /// Get the current zoom level.
    pub fn zoom_level(&self) -> f32 {
        self.zoom_level
    }

    /// Register a source with its layout.
    ///
    /// Sources should be registered in document order (top to bottom).
    /// The order of registration determines the document order for selection.
    ///
    /// If the source is already registered, new items are appended and bounds
    /// are expanded. This allows multiple widgets (e.g. per-line TextElements)
    /// to share a single source for cross-line selection.
    pub fn register_source(&mut self, source_id: SourceId, mut layout: SourceLayout) {
        let clip = self.current_clip();
        layout.clip_rect = clip;
        self.source_ordering.register(source_id);
        if let Some(existing) = self.sources.get_mut(&source_id) {
            existing.bounds = existing.bounds.union(&layout.bounds);
            existing.items.extend(layout.items);
            // When merging, intersect clips (both are in the same container,
            // but be safe for nested cases).
            existing.clip_rect = match (existing.clip_rect, clip) {
                (Some(a), Some(b)) => a.intersection(&b),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };
        } else {
            self.sources.insert(source_id, layout);
        }
    }

    /// Get the layout for a source.
    pub fn get_source(&self, source_id: &SourceId) -> Option<&SourceLayout> {
        self.sources.get(source_id)
    }

    /// Get the source ordering.
    pub fn ordering(&self) -> &SourceOrdering {
        &self.source_ordering
    }

    /// Get all sources in document order.
    pub fn sources_in_order(&self) -> impl Iterator<Item = (SourceId, &SourceLayout)> {
        self.source_ordering
            .sources_in_order()
            .iter()
            .filter_map(|id| self.sources.get(id).map(|layout| (*id, layout)))
    }

    /// Hit test: screen point → hit result.
    ///
    /// Returns `HitResult::Content` for character-level hits (text, terminal),
    /// or `HitResult::Widget` for opaque container hits (buttons, panels).
    /// Content hits take priority over widget hits.
    pub fn hit_test(&self, point: Point) -> Option<HitResult> {
        self.hit_test_xy(point.x, point.y)
    }

    /// Hit test with separate x, y coordinates.
    pub fn hit_test_xy(&self, x: f32, y: f32) -> Option<HitResult> {
        // 1. Priority: Small interactive widgets (buttons, sort headers).
        //    These take precedence over content to ensure clickability even
        //    when overlapping with selectable text.  Large container widgets
        //    (scroll areas) are deferred to step 3.
        const INTERACTIVE_MAX_AREA: f32 = 40_000.0; // ~200x200
        let mut best_widget: Option<(SourceId, f32)> = None;
        for (id, rect) in &self.widget_bounds {
            if rect.contains_xy(x, y) {
                let area = rect.width * rect.height;
                if area <= INTERACTIVE_MAX_AREA {
                    if best_widget.is_none() || area < best_widget.unwrap().1 {
                        best_widget = Some((*id, area));
                    }
                }
            }
        }
        if let Some((id, _)) = best_widget {
            return Some(HitResult::Widget(id));
        }

        // 2. Content sources (text/terminal) in document order.
        for source_id in self.source_ordering.sources_in_order() {
            let Some(layout) = self.sources.get(source_id) else {
                continue;
            };

            // Quick bounds check
            if !layout.bounds.contains_xy(x, y) {
                continue;
            }

            // Check each item in the source
            for (item_index, item) in layout.items.iter().enumerate() {
                if !item.bounds().contains_xy(x, y) {
                    continue;
                }

                let content_offset = match item {
                    ItemLayout::Text(text_layout) => {
                        self.hit_test_text(text_layout, x, y)
                    }
                    ItemLayout::Grid(grid_layout) => {
                        self.hit_test_grid(grid_layout, x, y)
                    }
                };

                return Some(HitResult::Content(ContentAddress::new(*source_id, item_index, content_offset)));
            }
        }

        // 3. Fallback: Large container widgets (scroll areas, etc.)
        let mut best_container: Option<(SourceId, f32)> = None;
        for (id, rect) in &self.widget_bounds {
            if rect.contains_xy(x, y) {
                let area = rect.width * rect.height;
                if area > INTERACTIVE_MAX_AREA {
                    if best_container.is_none() || area < best_container.unwrap().1 {
                        best_container = Some((*id, area));
                    }
                }
            }
        }
        if let Some((id, _)) = best_container {
            return Some(HitResult::Widget(id));
        }

        None
    }

    /// Hit test within a text layout.
    ///
    /// Returns a cursor position (0 to char_count) suitable for text selection.
    /// Position N means "between character N-1 and character N" (or before first/after last).
    /// Uses a linear scan to handle both LTR and RTL text (positions may not be monotonic).
    fn hit_test_text(&self, layout: &TextLayout, x: f32, y: f32) -> usize {
        let rel_x = x - layout.bounds.x;
        let rel_y = y - layout.bounds.y;

        // Find which line
        let line = (rel_y / layout.line_height).floor() as usize;
        let line = line.min(layout.line_count().saturating_sub(1));

        // Get character range for this line
        let (line_start, line_end) = layout.line_range(line);
        if line_start >= line_end {
            return line_start;
        }

        line_start + nearest_char_in_range(layout, line_start, line_end, rel_x)
    }

    /// Hit test within a grid layout.
    fn hit_test_grid(&self, layout: &GridLayout, x: f32, y: f32) -> usize {
        let rel_x = x - layout.bounds.x;
        let rel_y = y - layout.bounds.y;

        let col = (rel_x / layout.cell_width).floor() as u16;
        let row = (rel_y / layout.cell_height).floor() as u16;

        // Clamp to valid range
        let col = col.min(layout.cols.saturating_sub(1));
        let row = row.min(layout.rows.saturating_sub(1));

        layout.grid_to_offset(col, row)
    }

    /// Find the nearest content address to a screen point.
    ///
    /// Unlike `hit_test`, this returns a content address even when the point
    /// is in a gap between elements. Used as a fallback during selection
    /// drags to bridge dead zones.
    pub fn nearest_content(&self, x: f32, y: f32) -> Option<HitResult> {
        let mut best_source: Option<(SourceId, &SourceLayout, (f32, f32))> = None;

        for source_id in self.source_ordering.sources_in_order() {
            let Some(layout) = self.sources.get(source_id) else { continue };
            if layout.items.is_empty() { continue; }

            let dist = rect_distance(&layout.bounds, x, y);
            if best_source.is_none() || dist < best_source.as_ref().unwrap().2 {
                best_source = Some((*source_id, layout, dist));
            }
        }

        let (source_id, source_layout, _) = best_source?;

        // Find nearest item within the source
        let mut best_item: Option<(usize, &ItemLayout, (f32, f32))> = None;
        for (i, item) in source_layout.items.iter().enumerate() {
            let dist = rect_distance(&item.bounds(), x, y);
            if best_item.is_none() || dist < best_item.as_ref().unwrap().2 {
                best_item = Some((i, item, dist));
            }
        }

        let (item_index, item, _) = best_item?;

        let content_offset = match item {
            ItemLayout::Text(text) => nearest_text_offset(text, x, y),
            ItemLayout::Grid(grid) => nearest_grid_offset(grid, x, y),
        };

        Some(HitResult::Content(ContentAddress::new(source_id, item_index, content_offset)))
    }

    /// Get the screen bounds for a content address.
    ///
    /// Returns the rectangle of the character or cell at the address.
    pub fn char_bounds(&self, addr: &ContentAddress) -> Option<Rect> {
        let layout = self.sources.get(&addr.source_id)?;
        let item = layout.items.get(addr.item_index)?;

        match item {
            ItemLayout::Text(text_layout) => {
                self.char_bounds_text(text_layout, addr.content_offset)
            }
            ItemLayout::Grid(grid_layout) => {
                self.char_bounds_grid(grid_layout, addr.content_offset)
            }
        }
    }

    /// Get character bounds in a text layout.
    fn char_bounds_text(&self, layout: &TextLayout, offset: usize) -> Option<Rect> {
        if offset >= layout.char_count {
            return None;
        }

        let x = *layout.char_positions.get(offset)?;

        // Get width (from char_widths or compute from next position)
        let width = if !layout.char_widths.is_empty() {
            layout.char_widths.get(offset).copied().unwrap_or(layout.fallback_advance)
        } else {
            layout
                .char_positions
                .get(offset + 1)
                .map(|next| next - x)
                .unwrap_or(layout.fallback_advance)
        };

        // Find which line this character is on
        let line = layout.line_for_offset(offset);
        let y = layout.line_y(line);

        Some(Rect {
            x: layout.bounds.x + x,
            y: layout.bounds.y + y,
            width,
            height: layout.line_height,
        })
    }

    /// Get cell bounds in a grid layout.
    fn char_bounds_grid(&self, layout: &GridLayout, offset: usize) -> Option<Rect> {
        if offset >= layout.cell_count() {
            return None;
        }

        let (col, row) = layout.offset_to_grid(offset);
        Some(layout.cell_bounds(col, row))
    }

    /// Compare two content addresses in document order.
    pub fn compare(&self, a: &ContentAddress, b: &ContentAddress) -> Ordering {
        self.source_ordering.compare(a, b)
    }

    /// Normalize a selection so start comes before end.
    pub fn normalize_selection(
        &self,
        selection: &Selection,
    ) -> (ContentAddress, ContentAddress) {
        selection.normalized(&self.source_ordering)
    }

    /// Get the bounds of a selection range.
    ///
    /// Returns a list of rectangles that cover the selection.
    /// This is used for rendering selection highlights.
    pub fn selection_bounds(&self, selection: &Selection) -> Vec<(Rect, Option<Rect>)> {
        let (start, end) = self.normalize_selection(selection);
        let mut rects = Vec::new();

        // Get sources that the selection spans
        let sources = selection.sources(&self.source_ordering);

        for source_id in sources {
            let Some(layout) = self.sources.get(&source_id) else {
                continue;
            };

            let start_order = self.source_ordering.position(&start.source_id);
            let end_order = self.source_ordering.position(&end.source_id);
            let current_order = self.source_ordering.position(&source_id);

            let (Some(start_order), Some(end_order), Some(current_order)) =
                (start_order, end_order, current_order)
            else {
                continue;
            };

            let clip = layout.clip_rect;

            // Fast path: entire source is selected — use combined bounds
            let fully_before_start = current_order > start_order
                || (current_order == start_order
                    && start.item_index == 0
                    && start.content_offset == 0);
            let fully_after_end = current_order < end_order
                || (current_order == end_order
                    && end.item_index >= layout.items.len());
            if fully_before_start && fully_after_end {
                rects.push((layout.bounds, clip));
            } else {
                // Partial selection - need to compute per-item bounds
                for (item_index, item) in layout.items.iter().enumerate() {
                    let item_rects = self.selection_bounds_for_item(
                        &source_id,
                        item_index,
                        item,
                        &start,
                        &end,
                        current_order,
                        start_order,
                        end_order,
                    );
                    rects.extend(item_rects.into_iter().map(|r| (r, clip)));
                }
            }
        }

        rects
    }

    /// Get selection bounds for a single item.
    #[allow(clippy::too_many_arguments)]
    fn selection_bounds_for_item(
        &self,
        source_id: &SourceId,
        item_index: usize,
        item: &ItemLayout,
        start: &ContentAddress,
        end: &ContentAddress,
        current_order: usize,
        start_order: usize,
        end_order: usize,
    ) -> Vec<Rect> {
        let mut rects = Vec::new();

        // Determine the offset range within this item
        let (item_start_offset, item_end_offset) = match item {
            ItemLayout::Text(t) => (0, t.char_count),
            ItemLayout::Grid(g) => (0, g.cell_count()),
        };

        // Adjust start offset if this is the starting source/item
        let sel_start = if *source_id == start.source_id && item_index == start.item_index {
            start.content_offset
        } else if current_order > start_order
            || (*source_id == start.source_id && item_index > start.item_index)
        {
            item_start_offset
        } else {
            return rects; // Before selection
        };

        // Adjust end offset if this is the ending source/item
        let sel_end = if *source_id == end.source_id && item_index == end.item_index {
            end.content_offset
        } else if current_order < end_order
            || (*source_id == end.source_id && item_index < end.item_index)
        {
            item_end_offset
        } else {
            return rects; // After selection
        };

        if sel_start >= sel_end {
            return rects;
        }

        // Generate rectangles based on item type
        match item {
            ItemLayout::Text(text_layout) => {
                // Generate per-line rectangles for text
                let start_line = text_layout.line_for_offset(sel_start);
                let end_line = text_layout.line_for_offset(sel_end.saturating_sub(1));

                for line in start_line..=end_line {
                    let (line_start, line_end) = text_layout.line_range(line);
                    let range_start = sel_start.max(line_start);
                    let range_end = sel_end.min(line_end);

                    if range_start >= range_end {
                        continue;
                    }

                    // Compute the visual extent of selected chars.
                    // Use min/max instead of assuming monotonic positions,
                    // so RTL and bidi text get correct highlight bounds.
                    let has_widths = !text_layout.char_widths.is_empty();
                    let mut min_x = f32::MAX;
                    let mut max_x = f32::MIN;

                    let fa = text_layout.fallback_advance;
                    for i in range_start..range_end {
                        let pos = text_layout.char_positions.get(i).copied().unwrap_or(0.0);
                        let w = if has_widths {
                            text_layout.char_widths.get(i).copied().unwrap_or(fa)
                        } else {
                            text_layout.char_positions.get(i + 1)
                                .map(|&next| (next - pos).abs())
                                .unwrap_or(fa)
                        };
                        let left = pos.min(pos + w);
                        let right = pos.max(pos + w);
                        min_x = min_x.min(left);
                        max_x = max_x.max(right);
                    }

                    if min_x >= max_x {
                        continue;
                    }

                    rects.push(Rect {
                        x: text_layout.bounds.x + min_x,
                        y: text_layout.bounds.y + text_layout.line_y(line),
                        width: max_x - min_x,
                        height: text_layout.line_height,
                    });
                }
            }
            ItemLayout::Grid(grid_layout) => {
                // For grids, generate per-row rectangles
                let (start_col, start_row) = grid_layout.offset_to_grid(sel_start);
                let end_offset = sel_end.saturating_sub(1);
                let (end_col, end_row) = grid_layout.offset_to_grid(end_offset);

                for row in start_row..=end_row {
                    let row_start_col = if row == start_row { start_col } else { 0 };
                    let row_end_col = if row == end_row {
                        end_col
                    } else {
                        grid_layout.cols - 1
                    };

                    let x_start = grid_layout.bounds.x + row_start_col as f32 * grid_layout.cell_width;
                    let x_end = grid_layout.bounds.x
                        + (row_end_col as f32 + 1.0) * grid_layout.cell_width;
                    let y = grid_layout.bounds.y + row as f32 * grid_layout.cell_height;

                    rects.push(Rect {
                        x: x_start,
                        y,
                        width: x_end - x_start,
                        height: grid_layout.cell_height,
                    });
                }
            }
        }

        rects
    }

    // =========================================================================
    // Debug Visualization (compiled out in release)
    // =========================================================================

    /// Push a debug rectangle for layout visualization.
    ///
    /// Only available in debug builds. Call this from containers during layout
    /// when `ctx.is_debug()` returns true.
    ///
    /// Applies a "staircase inset" based on depth: each nested container's
    /// debug rect is inset by 1 pixel per depth level, creating an onion-layer
    /// effect that makes hierarchy visible even when containers share exact bounds.
    #[cfg(debug_assertions)]
    pub fn push_debug_rect(&mut self, rect: Rect, label: impl Into<String>, depth: u32, is_overflow: bool) {
        // Staircase inset: 1 pixel per depth level
        let inset = depth as f32;
        let inset_rect = if rect.width > inset * 2.0 && rect.height > inset * 2.0 {
            Rect::new(
                rect.x + inset,
                rect.y + inset,
                rect.width - inset * 2.0,
                rect.height - inset * 2.0,
            )
        } else {
            // Container too small for inset, use original bounds
            rect
        };

        self.debug_rects.push(DebugRect {
            rect: inset_rect,
            label: label.into(),
            depth,
            is_overflow,
        });
    }

    /// Get debug rectangles for rendering the layout overlay.
    ///
    /// Returns an empty slice in release builds.
    #[cfg(debug_assertions)]
    pub fn debug_rects(&self) -> &[DebugRect] {
        &self.debug_rects
    }

    /// Get debug rectangles (release stub - always returns empty).
    #[cfg(not(debug_assertions))]
    pub fn debug_rects(&self) -> &[()] {
        &[]
    }

    /// Check if there are any debug rectangles to render.
    #[cfg(debug_assertions)]
    pub fn has_debug_rects(&self) -> bool {
        !self.debug_rects.is_empty()
    }

    /// Check if there are any debug rectangles (release stub - always false).
    #[cfg(not(debug_assertions))]
    pub fn has_debug_rects(&self) -> bool {
        false
    }

    /// Enable debug mode for legacy layout methods.
    ///
    /// Call this once before layout to enable debug rect collection
    /// in the legacy `layout()` methods.
    #[cfg(debug_assertions)]
    pub fn set_debug_enabled(&mut self, enabled: bool) {
        self.debug_enabled = enabled;
        self.debug_depth = 0;
    }

    #[cfg(not(debug_assertions))]
    pub fn set_debug_enabled(&mut self, _enabled: bool) {}

    /// Check if debug mode is enabled.
    #[cfg(debug_assertions)]
    pub fn is_debug_enabled(&self) -> bool {
        self.debug_enabled
    }

    #[cfg(not(debug_assertions))]
    pub fn is_debug_enabled(&self) -> bool {
        false
    }

    /// Enter a debug scope (for legacy layout methods).
    ///
    /// Call at the start of a container's layout method.
    /// Pushes a debug rect and increments depth.
    ///
    /// Applies a "staircase inset" based on depth: each nested container's
    /// debug rect is inset by 1 pixel per depth level, creating an onion-layer
    /// effect that makes hierarchy visible even when containers share exact bounds.
    #[cfg(debug_assertions)]
    pub fn debug_enter(&mut self, name: &str, rect: Rect) {
        if self.debug_enabled {
            // Staircase inset: 1 pixel per depth level
            let inset = self.debug_depth as f32;
            let inset_rect = if rect.width > inset * 2.0 && rect.height > inset * 2.0 {
                Rect::new(
                    rect.x + inset,
                    rect.y + inset,
                    rect.width - inset * 2.0,
                    rect.height - inset * 2.0,
                )
            } else {
                // Container too small for inset, use original bounds
                rect
            };

            self.debug_rects.push(DebugRect {
                rect: inset_rect,
                label: name.to_string(),
                depth: self.debug_depth,
                is_overflow: false,
            });
            self.debug_depth += 1;
        }
    }

    #[cfg(not(debug_assertions))]
    pub fn debug_enter(&mut self, _name: &str, _rect: Rect) {}

    /// Exit a debug scope (for legacy layout methods).
    #[cfg(debug_assertions)]
    pub fn debug_exit(&mut self) {
        if self.debug_enabled && self.debug_depth > 0 {
            self.debug_depth -= 1;
        }
    }

    #[cfg(not(debug_assertions))]
    pub fn debug_exit(&mut self) {}
}

/// Axis-aligned distance from a point to a rect. Returns `(dy, dx)`, both >= 0.
/// If the point is inside the rect on an axis, that component is 0.
fn rect_distance(bounds: &Rect, x: f32, y: f32) -> (f32, f32) {
    let dx = if x < bounds.x {
        bounds.x - x
    } else if x > bounds.x + bounds.width {
        x - (bounds.x + bounds.width)
    } else {
        0.0
    };
    let dy = if y < bounds.y {
        bounds.y - y
    } else if y > bounds.y + bounds.height {
        y - (bounds.y + bounds.height)
    } else {
        0.0
    };
    (dy, dx)
}

/// Resolve nearest content offset for a text item, clamping to edges.
///
/// Same logic as `hit_test_text` but with Y-clamping so positions above/below
/// the text bounds snap to the first/last line respectively.
fn nearest_text_offset(layout: &TextLayout, x: f32, y: f32) -> usize {
    let rel_x = x - layout.bounds.x;
    let rel_y = y - layout.bounds.y;

    // Clamp line: above → first line, below → last line
    let line = if rel_y < 0.0 {
        0
    } else {
        let l = (rel_y / layout.line_height).floor() as usize;
        l.min(layout.line_count().saturating_sub(1))
    };

    let (line_start, line_end) = layout.line_range(line);
    if line_start >= line_end {
        return line_start;
    }

    line_start + nearest_char_in_range(layout, line_start, line_end, rel_x)
}

/// Find the nearest character boundary in a range, handling both LTR and RTL.
///
/// Returns a local index (0..=range_len) where 0 = before first char in range,
/// range_len = after last char in range. Works by linear scan over character
/// midpoints, so it handles non-monotonic positions from RTL/bidi text.
fn nearest_char_in_range(layout: &TextLayout, line_start: usize, line_end: usize, rel_x: f32) -> usize {
    let count = line_end - line_start;
    if count == 0 {
        return 0;
    }

    let positions = &layout.char_positions[line_start..line_end];
    let widths = &layout.char_widths;
    let has_widths = !widths.is_empty();
    let fa = layout.fallback_advance;

    // Find which character the click is inside, by checking if rel_x falls
    // within [pos, pos + width) for each character.
    let mut best_idx = 0;
    let mut best_dist = f32::MAX;

    for i in 0..count {
        let pos = positions[i];
        let stored_w = if has_widths {
            widths.get(line_start + i).copied().unwrap_or(fa)
        } else {
            positions.get(i + 1).map(|&next| (next - pos).abs()).unwrap_or(fa)
        };

        // For zero-width chars (combining marks, ZWJ interior), derive
        // a width from neighbor positions so they're still hittable.
        let w = if stored_w < 0.01 {
            // Try to derive width from the next non-zero-width char's position
            let mut derived = fa;
            for j in (i + 1)..count {
                let next_pos = positions[j];
                let d = (next_pos - pos).abs();
                if d > 0.01 {
                    derived = d;
                    break;
                }
            }
            derived
        } else {
            stored_w
        };

        let left = pos.min(pos + w);
        let right = pos.max(pos + w);
        let mid = (left + right) / 2.0;

        // Distance from click to midpoint of this character
        let dist = (rel_x - mid).abs();
        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
            // Snap to left or right side of this character
            if rel_x > mid {
                best_idx = i + 1; // cursor after this char
            }
        }
    }

    best_idx.min(count)
}

/// Compute total visual extent as max(pos + width) across all characters.
fn max_extent(positions: &[f32], widths: &[f32]) -> f32 {
    positions.iter()
        .zip(widths)
        .map(|(p, w)| p + w)
        .fold(0.0_f32, f32::max)
}

/// Resolve nearest content offset for a grid item, clamping to edges.
fn nearest_grid_offset(layout: &GridLayout, x: f32, y: f32) -> usize {
    let rel_x = (x - layout.bounds.x).clamp(0.0, layout.bounds.width - 0.01);
    let rel_y = (y - layout.bounds.y).clamp(0.0, layout.bounds.height - 0.01);

    let col = (rel_x / layout.cell_width).floor() as u16;
    let row = (rel_y / layout.cell_height).floor() as u16;

    let col = col.min(layout.cols.saturating_sub(1));
    let row = row.min(layout.rows.saturating_sub(1));

    layout.grid_to_offset(col, row)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extract ContentAddress from HitResult::Content, panicking on Widget.
    fn unwrap_content(hit: Option<HitResult>) -> ContentAddress {
        match hit.expect("expected a hit") {
            HitResult::Content(addr) => addr,
            HitResult::Widget(id) => panic!("expected Content hit, got Widget({:?})", id),
        }
    }

    fn make_text_layout() -> TextLayout {
        // "Hello\nWorld" - 5 chars on line 0, 5 chars on line 1
        TextLayout::new(
            "Hello\nWorld",
            0xFFFFFFFF, // White
            Rect::new(0.0, 0.0, 50.0, 24.0),
            vec![
                0.0, 10.0, 20.0, 30.0, 40.0, // Hello
                0.0, 10.0, 20.0, 30.0, 40.0, // World
            ],
            vec![5], // Line break after index 5
            12.0,
        )
    }

    fn make_grid_layout() -> GridLayout {
        GridLayout::new(
            Rect::new(0.0, 0.0, 80.0, 24.0),
            8.0,  // cell_width
            12.0, // cell_height
            10,   // cols
            2,    // rows
        )
    }

    #[test]
    fn text_layout_line_for_offset() {
        let layout = make_text_layout();

        assert_eq!(layout.line_for_offset(0), 0);
        assert_eq!(layout.line_for_offset(4), 0);
        assert_eq!(layout.line_for_offset(5), 1);
        assert_eq!(layout.line_for_offset(9), 1);
    }

    #[test]
    fn text_layout_line_range() {
        let layout = make_text_layout();

        assert_eq!(layout.line_range(0), (0, 5));
        assert_eq!(layout.line_range(1), (5, 10));
    }

    #[test]
    fn grid_layout_offset_conversion() {
        let layout = make_grid_layout();

        assert_eq!(layout.offset_to_grid(0), (0, 0));
        assert_eq!(layout.offset_to_grid(5), (5, 0));
        assert_eq!(layout.offset_to_grid(10), (0, 1));
        assert_eq!(layout.offset_to_grid(15), (5, 1));

        assert_eq!(layout.grid_to_offset(0, 0), 0);
        assert_eq!(layout.grid_to_offset(5, 0), 5);
        assert_eq!(layout.grid_to_offset(0, 1), 10);
        assert_eq!(layout.grid_to_offset(5, 1), 15);
    }

    #[test]
    fn snapshot_hit_test_text() {
        let mut snapshot = LayoutSnapshot::new();
        let source = SourceId::new();

        let text_layout = make_text_layout();
        snapshot.register_source(source, SourceLayout::text(text_layout));

        // Hit first character
        let addr = unwrap_content(snapshot.hit_test_xy(5.0, 6.0));
        assert_eq!(addr.source_id, source);
        assert_eq!(addr.item_index, 0);
        assert_eq!(addr.content_offset, 0);

        // Hit character in middle of first line
        let addr = unwrap_content(snapshot.hit_test_xy(25.0, 6.0));
        assert_eq!(addr.content_offset, 2);

        // Hit second line
        let addr = unwrap_content(snapshot.hit_test_xy(15.0, 18.0));
        assert_eq!(addr.content_offset, 6); // Second line starts at offset 5
    }

    #[test]
    fn snapshot_hit_test_grid() {
        let mut snapshot = LayoutSnapshot::new();
        let source = SourceId::new();

        let grid_layout = make_grid_layout();
        snapshot.register_source(source, SourceLayout::grid(grid_layout));

        // Hit first cell
        let addr = unwrap_content(snapshot.hit_test_xy(4.0, 6.0));
        assert_eq!(addr.source_id, source);
        assert_eq!(addr.content_offset, 0);

        // Hit cell (5, 0)
        let addr = unwrap_content(snapshot.hit_test_xy(44.0, 6.0));
        assert_eq!(addr.content_offset, 5);

        // Hit cell (3, 1)
        let addr = unwrap_content(snapshot.hit_test_xy(28.0, 18.0));
        assert_eq!(addr.content_offset, 13); // row 1, col 3 = 10 + 3
    }

    #[test]
    fn snapshot_char_bounds() {
        let mut snapshot = LayoutSnapshot::new();
        let source = SourceId::new();

        let text_layout = make_text_layout();
        snapshot.register_source(source, SourceLayout::text(text_layout));

        let addr = ContentAddress::new(source, 0, 2);
        let bounds = snapshot.char_bounds(&addr).unwrap();

        assert_eq!(bounds.x, 20.0);
        assert_eq!(bounds.y, 0.0);
        assert_eq!(bounds.width, 10.0); // Next char at 30.0
        assert_eq!(bounds.height, 12.0);
    }

    #[test]
    fn snapshot_multiple_sources() {
        let mut snapshot = LayoutSnapshot::new();

        let source1 = SourceId::new();
        let source2 = SourceId::new();

        // Source 1 at top
        let text1 = TextLayout::new(
            "ABC",
            0xFFFFFFFF,
            Rect::new(0.0, 0.0, 50.0, 12.0),
            vec![0.0, 10.0, 20.0],
            vec![],
            12.0,
        );
        snapshot.register_source(source1, SourceLayout::text(text1));

        // Source 2 below source 1
        let text2 = TextLayout::new(
            "DEF",
            0xFFFFFFFF,
            Rect::new(0.0, 20.0, 50.0, 12.0),
            vec![0.0, 10.0, 20.0],
            vec![],
            12.0,
        );
        snapshot.register_source(source2, SourceLayout::text(text2));

        // Hit source 1
        let addr = unwrap_content(snapshot.hit_test_xy(5.0, 6.0));
        assert_eq!(addr.source_id, source1);

        // Hit source 2
        let addr = unwrap_content(snapshot.hit_test_xy(5.0, 26.0));
        assert_eq!(addr.source_id, source2);

        // Source 1 is before source 2 in document order
        let addr1 = ContentAddress::new(source1, 0, 0);
        let addr2 = ContentAddress::new(source2, 0, 0);
        assert_eq!(snapshot.compare(&addr1, &addr2), Ordering::Less);
    }
}
