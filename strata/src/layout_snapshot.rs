//! Layout Snapshot
//!
//! The `LayoutSnapshot` is the single source of truth for both rendering AND queries.
//! It captures all layout information during the layout pass and exposes it for:
//! - Hit-testing (screen point → content address)
//! - Character bounds (content address → screen rect)
//! - Selection rendering
//!
//! This solves iced's broken text APIs by storing character positions computed
//! during layout rather than re-querying them.

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
        }
    }

    /// Create a simple single-line text layout with uniform character spacing.
    ///
    /// This is a convenience method for simple text where each character
    /// has the same width. For proper text shaping, use the full constructor.
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
        let char_positions: Vec<f32> = (0..char_count)
            .map(|i| i as f32 * char_width)
            .collect();
        let width = char_count as f32 * char_width;

        Self {
            text,
            color,
            bounds: Rect::new(x, y, width, line_height),
            char_positions,
            char_widths: Vec::new(),
            line_breaks: Vec::new(),
            line_height,
            char_count,
        }
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

/// A row of text in a grid layout.
#[derive(Debug, Clone)]
pub struct GridRow {
    /// The text content for this row.
    pub text: String,
    /// Foreground color (packed RGBA).
    pub color: u32,
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
}

impl SourceLayout {
    /// Create a new source layout with no items.
    pub fn new(bounds: Rect) -> Self {
        Self {
            bounds,
            items: Vec::new(),
        }
    }

    /// Create a source layout with a single text item.
    pub fn text(text_layout: TextLayout) -> Self {
        let bounds = text_layout.bounds;
        Self {
            bounds,
            items: vec![ItemLayout::Text(text_layout)],
        }
    }

    /// Create a source layout with a single grid item.
    pub fn grid(grid_layout: GridLayout) -> Self {
        let bounds = grid_layout.bounds;
        Self {
            bounds,
            items: vec![ItemLayout::Grid(grid_layout)],
        }
    }
}

/// The layout snapshot captures all layout information for a frame.
///
/// Built once during layout, used by both rendering and queries.
/// This is the core type that solves iced's broken hit-testing.
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

    /// Register a source with its layout.
    ///
    /// Sources should be registered in document order (top to bottom).
    /// The order of registration determines the document order for selection.
    ///
    /// If the source is already registered, new items are appended and bounds
    /// are expanded. This allows multiple widgets (e.g. per-line TextElements)
    /// to share a single source for cross-line selection.
    pub fn register_source(&mut self, source_id: SourceId, layout: SourceLayout) {
        self.source_ordering.register(source_id);
        if let Some(existing) = self.sources.get_mut(&source_id) {
            existing.bounds = existing.bounds.union(&layout.bounds);
            existing.items.extend(layout.items);
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
    /// This snaps to the nearest character boundary based on click position.
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

        // Get character positions for this line (left edge of each character)
        let line_chars = &layout.char_positions[line_start..line_end];
        if line_chars.is_empty() {
            return line_start;
        }

        // Find cursor position by snapping to nearest character boundary.
        // char_positions[i] = left edge of character i = cursor position i.
        // We also need the right edge of the last character for cursor position N.
        let idx = line_chars.partition_point(|&pos| pos < rel_x);

        let final_idx = if idx == 0 {
            // Before first character
            0
        } else if idx >= line_chars.len() {
            // Past last character - cursor goes at the end
            line_chars.len()
        } else {
            // Between two characters - snap to nearest boundary
            let left_edge = line_chars[idx - 1];
            let right_edge = line_chars[idx];
            let midpoint = (left_edge + right_edge) / 2.0;
            if rel_x <= midpoint {
                idx - 1
            } else {
                idx
            }
        };

        line_start + final_idx
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
            layout.char_widths.get(offset).copied().unwrap_or(8.0)
        } else {
            layout
                .char_positions
                .get(offset + 1)
                .map(|next| next - x)
                .unwrap_or(8.0) // Default char width for last char
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
    pub fn selection_bounds(&self, selection: &Selection) -> Vec<Rect> {
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

            // Fast path: entire source is selected — use combined bounds
            let fully_before_start = current_order > start_order
                || (current_order == start_order
                    && start.item_index == 0
                    && start.content_offset == 0);
            let fully_after_end = current_order < end_order
                || (current_order == end_order
                    && end.item_index >= layout.items.len());
            if fully_before_start && fully_after_end {
                rects.push(layout.bounds);
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
                    rects.extend(item_rects);
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

                    let x_start = text_layout
                        .char_positions
                        .get(range_start)
                        .copied()
                        .unwrap_or(0.0);
                    let x_end = text_layout
                        .char_positions
                        .get(range_end)
                        .copied()
                        .unwrap_or_else(|| {
                            text_layout
                                .char_positions
                                .last()
                                .copied()
                                .unwrap_or(0.0)
                                + 8.0
                        });

                    rects.push(Rect {
                        x: text_layout.bounds.x + x_start,
                        y: text_layout.bounds.y + text_layout.line_y(line),
                        width: x_end - x_start,
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

    let line_chars = &layout.char_positions[line_start..line_end];
    if line_chars.is_empty() {
        return line_start;
    }

    // Same partition_point + midpoint-snap logic as hit_test_text
    let idx = line_chars.partition_point(|&pos| pos < rel_x);

    let final_idx = if idx == 0 {
        0
    } else if idx >= line_chars.len() {
        line_chars.len()
    } else {
        let left_edge = line_chars[idx - 1];
        let right_edge = line_chars[idx];
        let midpoint = (left_edge + right_edge) / 2.0;
        if rel_x <= midpoint { idx - 1 } else { idx }
    };

    line_start + final_idx
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
