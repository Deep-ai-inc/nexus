//! Layout Containers
//!
//! Flexbox-inspired layout containers that compute child positions.
//! The layout computation happens ONCE when `layout()` is called,
//! not during widget construction.

use crate::content_address::SourceId;
use crate::gpu::ImageHandle;
use crate::layout_snapshot::{CursorIcon, LayoutSnapshot};
use crate::primitives::{Color, Rect, Size};
use crate::scroll_state::ScrollState;
use crate::text_input_state::TextInputState;

// Layout metrics derived from fontdue for JetBrains Mono at 14px base size.
const CHAR_WIDTH: f32 = 8.4;
const LINE_HEIGHT: f32 = 18.0;
const BASE_FONT_SIZE: f32 = 14.0;

/// Sizing mode for a container axis.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Length {
    /// Shrink to fit content (intrinsic size).
    #[default]
    Shrink,
    /// Expand to fill available space (flex: 1).
    Fill,
    /// Expand proportionally (flex: n). `FillPortion(1)` == `Fill`.
    FillPortion(u16),
    /// Fixed pixel size.
    Fixed(f32),
}

impl Length {
    /// Get the flex factor for this length, or 0 if not flexible.
    fn flex(&self) -> f32 {
        match self {
            Length::Fill => 1.0,
            Length::FillPortion(n) => *n as f32,
            _ => 0.0,
        }
    }

    /// Whether this length participates in flex distribution.
    fn is_flex(&self) -> bool {
        matches!(self, Length::Fill | Length::FillPortion(_))
    }
}

/// Alignment on the main axis (direction of flow).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Alignment {
    /// Pack children at the start.
    #[default]
    Start,
    /// Pack children at the end.
    End,
    /// Center children.
    Center,
    /// Distribute space evenly between children.
    SpaceBetween,
    /// Distribute space evenly around children.
    SpaceAround,
}

/// Alignment on the cross axis (perpendicular to flow).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CrossAxisAlignment {
    /// Align to start of cross axis.
    #[default]
    Start,
    /// Align to end of cross axis.
    End,
    /// Center on cross axis.
    Center,
    /// Stretch to fill cross axis.
    Stretch,
}

/// Padding around content.
#[derive(Debug, Clone, Copy, Default)]
pub struct Padding {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Padding {
    /// Create padding with explicit values for each side.
    pub fn new(top: f32, right: f32, bottom: f32, left: f32) -> Self {
        Self { top, right, bottom, left }
    }

    /// Uniform padding on all sides.
    pub fn all(value: f32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    /// Symmetric padding (horizontal, vertical).
    pub fn symmetric(horizontal: f32, vertical: f32) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }

    /// Total horizontal padding.
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    /// Total vertical padding.
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

/// A child element in a layout container.
pub enum LayoutChild {
    /// A text element.
    Text(TextElement),

    /// A terminal/grid element.
    Terminal(TerminalElement),

    /// An image element.
    Image(ImageElement),

    /// A nested column.
    Column(Column),

    /// A nested row.
    Row(Row),

    /// A scroll column (virtualized vertical scroll container).
    ScrollColumn(ScrollColumn),

    /// A spacer that expands to fill available space.
    Spacer { flex: f32 },

    /// A button element (text label with background, registers as widget hit target).
    Button(ButtonElement),

    /// A text input element (editable text field, registers as widget hit target).
    TextInput(TextInputElement),

    /// A table element (headers + rows with sortable columns).
    Table(TableElement),

    /// A fixed-size spacer.
    FixedSpacer { size: f32 },
}

impl LayoutChild {
    /// Measure this child's main axis size (height for Column parent, width for Row parent).
    fn measure_main(&self, is_column: bool) -> f32 {
        let size = match self {
            LayoutChild::Text(t) => t.estimate_size(CHAR_WIDTH, LINE_HEIGHT),
            LayoutChild::Terminal(t) => t.size(),
            LayoutChild::Image(img) => img.size(),
            LayoutChild::Button(b) => b.estimate_size(),
            LayoutChild::TextInput(t) => t.estimate_size(),
            LayoutChild::Table(t) => t.estimate_size(),
            LayoutChild::Column(c) => c.measure(),
            LayoutChild::Row(r) => r.measure(),
            LayoutChild::ScrollColumn(s) => s.measure(),
            LayoutChild::Spacer { .. } => return 0.0,
            LayoutChild::FixedSpacer { size } => return *size,
        };
        if is_column { size.height } else { size.width }
    }

    /// Measure this child's cross axis size (width for Column parent, height for Row parent).
    fn measure_cross(&self, is_column: bool) -> f32 {
        let size = match self {
            LayoutChild::Text(t) => t.estimate_size(CHAR_WIDTH, LINE_HEIGHT),
            LayoutChild::Terminal(t) => t.size(),
            LayoutChild::Image(img) => img.size(),
            LayoutChild::Button(b) => b.estimate_size(),
            LayoutChild::TextInput(t) => t.estimate_size(),
            LayoutChild::Table(t) => t.estimate_size(),
            LayoutChild::Column(c) => c.measure(),
            LayoutChild::Row(r) => r.measure(),
            LayoutChild::ScrollColumn(s) => s.measure(),
            LayoutChild::Spacer { .. } => return 0.0,
            LayoutChild::FixedSpacer { .. } => return 0.0,
        };
        if is_column { size.width } else { size.height }
    }

    /// Get the flex factor on the parent's main axis.
    ///
    /// `is_column`: true if the parent is a Column (main axis = height),
    ///              false if the parent is a Row (main axis = width).
    fn flex_factor(&self, is_column: bool) -> f32 {
        match self {
            LayoutChild::Spacer { flex } => *flex,
            LayoutChild::Column(c) => {
                if is_column { c.height.flex() } else { c.width.flex() }
            }
            LayoutChild::Row(r) => {
                if is_column { r.height.flex() } else { r.width.flex() }
            }
            LayoutChild::ScrollColumn(s) => {
                if is_column { s.height.flex() } else { s.width.flex() }
            }
            LayoutChild::TextInput(t) => {
                if is_column { 0.0 } else { t.width.flex() }
            }
            _ => 0.0,
        }
    }

}

// =========================================================================
// From impls for LayoutChild — enables generic `.push()` on containers
// =========================================================================

impl From<TextElement> for LayoutChild {
    fn from(v: TextElement) -> Self { Self::Text(v) }
}
impl From<TerminalElement> for LayoutChild {
    fn from(v: TerminalElement) -> Self { Self::Terminal(v) }
}
impl From<ImageElement> for LayoutChild {
    fn from(v: ImageElement) -> Self { Self::Image(v) }
}
impl From<Column> for LayoutChild {
    fn from(v: Column) -> Self { Self::Column(v) }
}
impl From<Row> for LayoutChild {
    fn from(v: Row) -> Self { Self::Row(v) }
}
impl From<ScrollColumn> for LayoutChild {
    fn from(v: ScrollColumn) -> Self { Self::ScrollColumn(v) }
}
impl From<ButtonElement> for LayoutChild {
    fn from(v: ButtonElement) -> Self { Self::Button(v) }
}
impl From<TextInputElement> for LayoutChild {
    fn from(v: TextInputElement) -> Self { Self::TextInput(v) }
}
impl From<TableElement> for LayoutChild {
    fn from(v: TableElement) -> Self { Self::Table(v) }
}

// =========================================================================
// Widget trait — zero-cost reusable components
// =========================================================================

/// Trait for reusable, composable UI components.
///
/// Implementors produce a `LayoutChild` from existing primitives (Column, Row, etc.).
/// The blanket `From` impl means any `Widget` works with `.push()` on all containers.
///
/// # Zero-cost
///
/// `build()` consumes `self` by value. The widget struct lives on the stack,
/// and the returned `LayoutChild` is an enum variant — no heap allocation.
///
/// # Example
///
/// ```ignore
/// struct Card { inner: Column }
///
/// impl Card {
///     fn new(title: &str) -> Self {
///         Card {
///             inner: Column::new()
///                 .padding(10.0)
///                 .background(Color::rgb(0.1, 0.1, 0.1))
///                 .push(TextElement::new(title))
///         }
///     }
///
///     fn push(mut self, child: impl Into<LayoutChild>) -> Self {
///         self.inner = self.inner.push(child);
///         self
///     }
/// }
///
/// impl Widget for Card {
///     fn build(self) -> LayoutChild { self.inner.into() }
/// }
///
/// // Works with .push() on any container:
/// scroll_col.push(Card::new("Settings").push(some_input))
/// ```
pub trait Widget {
    /// Consume this widget and produce a layout node.
    fn build(self) -> LayoutChild;
}

/// Blanket impl: any `Widget` can be used with `.push()` on containers.
///
/// This does NOT conflict with the explicit `From` impls above because
/// built-in types (Column, Row, TextElement, etc.) do not implement `Widget`.
impl<W: Widget> From<W> for LayoutChild {
    #[inline(always)]
    fn from(w: W) -> LayoutChild {
        w.build()
    }
}

/// A text element descriptor.
///
/// This is declarative - it doesn't compute layout until the container does.
/// The cache key is auto-computed from the text content by default, enabling
/// the text engine to skip reshaping when content hasn't changed.
pub struct TextElement {
    /// Source ID for hit-testing and selection.
    pub source_id: Option<SourceId>,
    /// Text content.
    pub text: String,
    /// Text color.
    pub color: Color,
    /// Font size (if different from default).
    pub size: Option<f32>,
    /// Cache key for text shaping. Auto-computed from content by default.
    /// Override with `key()` for pre-computed keys on large strings.
    pub cache_key: u64,
    /// Measured size (filled during layout).
    measured_size: Option<Size>,
}

/// Render a TextInputElement at the given position and size.
fn render_text_input(
    snapshot: &mut LayoutSnapshot,
    input: TextInputElement,
    x: f32, y: f32, w: f32, h: f32,
) {
    use crate::primitives::Point;

    let input_rect = Rect::new(x, y, w, h);

    // Background
    snapshot.primitives_mut().add_rounded_rect(input_rect, input.corner_radius, input.background);

    // Border
    let border_color = if input.focused { input.focus_border_color } else { input.border_color };
    snapshot.primitives_mut().add_border(input_rect, input.corner_radius, input.border_width, border_color);

    // Clip content
    snapshot.primitives_mut().push_clip(input_rect);

    let text_x = x + input.padding.left;
    let text_y = y + input.padding.top;

    // Selection highlight
    if let Some((sel_start, sel_end)) = input.selection {
        let s = sel_start.min(sel_end);
        let e = sel_start.max(sel_end);
        let sel_x = text_x + s as f32 * CHAR_WIDTH;
        let sel_w = (e - s) as f32 * CHAR_WIDTH;
        snapshot.primitives_mut().add_solid_rect(
            Rect::new(sel_x, text_y, sel_w, LINE_HEIGHT),
            Color::rgba(0.3, 0.5, 0.8, 0.4),
        );
    }

    // Text or placeholder
    if input.text.is_empty() && !input.focused {
        snapshot.primitives_mut().add_text_cached(
            input.placeholder.clone(),
            Point::new(text_x, text_y),
            input.placeholder_color,
            BASE_FONT_SIZE,
            hash_text(&input.placeholder),
        );
    } else {
        snapshot.primitives_mut().add_text_cached(
            input.text,
            Point::new(text_x, text_y),
            input.text_color,
            BASE_FONT_SIZE,
            input.cache_key,
        );
    }

    // Cursor (blinking)
    if input.focused && input.cursor_visible {
        let cursor_x = text_x + input.cursor as f32 * CHAR_WIDTH;
        snapshot.primitives_mut().add_solid_rect(
            Rect::new(cursor_x, text_y, 2.0, LINE_HEIGHT),
            Color::rgba(0.85, 0.85, 0.88, 0.8),
        );
    }

    snapshot.primitives_mut().pop_clip();

    // Register for hit-testing
    snapshot.register_widget(input.id, input_rect);
    snapshot.set_cursor_hint(input.id, CursorIcon::Text);
}

/// Render a multiline text input element (code editor style).
///
/// Supports vertical scrolling, per-line cursor positioning, and
/// per-line selection highlights. Only visible lines are rendered
/// (virtualized).
fn render_text_input_multiline(
    snapshot: &mut LayoutSnapshot,
    input: TextInputElement,
    x: f32, y: f32, w: f32, h: f32,
) {
    use crate::primitives::Point;

    let input_rect = Rect::new(x, y, w, h);

    // Background
    snapshot.primitives_mut().add_rounded_rect(input_rect, input.corner_radius, input.background);

    // Border
    let border_color = if input.focused { input.focus_border_color } else { input.border_color };
    snapshot.primitives_mut().add_border(input_rect, input.corner_radius, input.border_width, border_color);

    // Clip content area
    snapshot.primitives_mut().push_clip(input_rect);

    let text_x = x + input.padding.left;
    let text_y = y + input.padding.top;
    let visible_h = h - input.padding.vertical();

    // Split text into lines
    let lines: Vec<&str> = if input.text.is_empty() {
        vec![""]
    } else {
        input.text.split('\n').collect()
    };

    // Compute visible line range (virtualized rendering)
    let first_visible = (input.scroll_offset / LINE_HEIGHT).floor().max(0.0) as usize;
    let visible_count = (visible_h / LINE_HEIGHT).ceil() as usize + 1;
    let last_visible = (first_visible + visible_count).min(lines.len());

    // Compute cursor (line, col) from byte offset
    let (cursor_line, cursor_col) = offset_to_line_col(&input.text, input.cursor);

    // Selection highlight (per-line)
    if let Some((sel_start, sel_end)) = input.selection {
        let s = sel_start.min(sel_end);
        let e = sel_start.max(sel_end);
        let (s_line, s_col) = offset_to_line_col(&input.text, s);
        let (e_line, e_col) = offset_to_line_col(&input.text, e);

        for line_idx in s_line..=e_line {
            if line_idx < first_visible || line_idx >= last_visible { continue; }
            let line_len = lines.get(line_idx).map(|l| l.chars().count()).unwrap_or(0);
            let col_start = if line_idx == s_line { s_col } else { 0 };
            let col_end = if line_idx == e_line { e_col } else { line_len };
            if col_start == col_end && s_line != e_line && line_idx != e_line {
                // Full-line selection indicator for empty-col lines in middle
            }
            let sel_x = text_x + col_start as f32 * CHAR_WIDTH;
            let sel_w = ((col_end - col_start).max(1)) as f32 * CHAR_WIDTH;
            let sel_y = text_y + line_idx as f32 * LINE_HEIGHT - input.scroll_offset;
            snapshot.primitives_mut().add_solid_rect(
                Rect::new(sel_x, sel_y, sel_w, LINE_HEIGHT),
                Color::rgba(0.3, 0.5, 0.8, 0.4),
            );
        }
    }

    // Render visible lines
    if input.text.is_empty() && !input.focused {
        snapshot.primitives_mut().add_text_cached(
            input.placeholder.clone(),
            Point::new(text_x, text_y),
            input.placeholder_color,
            BASE_FONT_SIZE,
            hash_text(&input.placeholder),
        );
    } else {
        for line_idx in first_visible..last_visible {
            let line = lines[line_idx];
            let ly = text_y + line_idx as f32 * LINE_HEIGHT - input.scroll_offset;
            if !line.is_empty() {
                snapshot.primitives_mut().add_text_cached(
                    line.to_string(),
                    Point::new(text_x, ly),
                    input.text_color,
                    BASE_FONT_SIZE,
                    hash_text(line).wrapping_add(line_idx as u64),
                );
            }
        }
    }

    // Cursor (blinking)
    if input.focused && input.cursor_visible {
        let cursor_x = text_x + cursor_col as f32 * CHAR_WIDTH;
        let cursor_y = text_y + cursor_line as f32 * LINE_HEIGHT - input.scroll_offset;
        snapshot.primitives_mut().add_solid_rect(
            Rect::new(cursor_x, cursor_y, 2.0, LINE_HEIGHT),
            Color::rgba(0.85, 0.85, 0.88, 0.8),
        );
    }

    snapshot.primitives_mut().pop_clip();

    // Register for hit-testing
    snapshot.register_widget(input.id, input_rect);
    snapshot.set_cursor_hint(input.id, CursorIcon::Text);
}

/// Convert a char offset to (line, col) within newline-delimited text.
fn offset_to_line_col(text: &str, char_offset: usize) -> (usize, usize) {
    let mut line = 0;
    let mut col = 0;
    for (i, ch) in text.chars().enumerate() {
        if i == char_offset {
            return (line, col);
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Fast non-cryptographic hash for cache keys.
#[inline]
fn hash_text(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl TextElement {
    /// Create a new text element.
    ///
    /// The cache key is automatically derived from the text content.
    /// For static strings, this means reshaping is skipped every frame.
    /// For dynamic strings, the key changes when content changes.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cache_key = hash_text(&text);
        Self {
            source_id: None,
            text,
            color: Color::WHITE,
            size: None,
            cache_key,
            measured_size: None,
        }
    }

    /// Set the source ID for hit-testing.
    pub fn source(mut self, source_id: SourceId) -> Self {
        self.source_id = Some(source_id);
        self
    }

    /// Set the text color.
    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Set the font size.
    pub fn size(mut self, size: f32) -> Self {
        self.size = Some(size);
        self
    }

    /// Override the cache key with an explicit value.
    ///
    /// Use this for performance-critical cases where hashing the text
    /// content is too expensive (e.g., very large strings), or when you
    /// have a stable external identifier (e.g., a row ID).
    pub fn key(mut self, key: u64) -> Self {
        self.cache_key = key;
        self
    }

    /// Estimate size for layout (uses character count heuristic).
    ///
    /// Scales metrics proportionally when a non-default font size is set.
    /// JetBrains Mono scales linearly, so this is a good approximation.
    fn estimate_size(&self, default_char_width: f32, default_line_height: f32) -> Size {
        if let Some(size) = self.measured_size {
            return size;
        }
        let (cw, lh) = if let Some(fs) = self.size {
            let scale = fs / BASE_FONT_SIZE;
            (default_char_width * scale, default_line_height * scale)
        } else {
            (default_char_width, default_line_height)
        };
        let char_count = self.text.chars().count() as f32;
        Size::new(char_count * cw, lh)
    }

    /// Get the effective font size for this element.
    fn font_size(&self) -> f32 {
        self.size.unwrap_or(BASE_FONT_SIZE)
    }
}

/// A terminal/grid element descriptor.
pub struct TerminalElement {
    /// Source ID for hit-testing.
    pub source_id: SourceId,
    /// Grid dimensions.
    pub cols: u16,
    pub rows: u16,
    /// Cell metrics.
    pub cell_width: f32,
    pub cell_height: f32,
    /// Content buffer reference (zero-copy binding).
    /// The actual content is in the app state, we just point to it.
    content_hash: u64,
    /// Row content for rendering.
    row_content: Vec<(String, u32)>,
}

impl TerminalElement {
    /// Create a new terminal element.
    pub fn new(source_id: SourceId, cols: u16, rows: u16) -> Self {
        Self {
            source_id,
            cols,
            rows,
            cell_width: 8.4,
            cell_height: 18.0,
            content_hash: 0,
            row_content: Vec::new(),
        }
    }

    /// Add a row of text content with a packed color.
    pub fn row(mut self, text: impl Into<String>, color: Color) -> Self {
        self.row_content.push((text.into(), color.pack()));
        self
    }

    /// Set cell metrics.
    pub fn cell_size(mut self, width: f32, height: f32) -> Self {
        self.cell_width = width;
        self.cell_height = height;
        self
    }

    /// Set content hash for cache invalidation.
    pub fn content_hash(mut self, hash: u64) -> Self {
        self.content_hash = hash;
        self
    }

    /// Calculate size based on grid dimensions.
    fn size(&self) -> Size {
        Size::new(
            self.cols as f32 * self.cell_width,
            self.rows as f32 * self.cell_height,
        )
    }
}

// =========================================================================
// Image Element
// =========================================================================

/// An image element descriptor.
pub struct ImageElement {
    /// Image handle from the pipeline.
    pub handle: ImageHandle,
    /// Display width in logical pixels.
    pub width: f32,
    /// Display height in logical pixels.
    pub height: f32,
    /// Corner radius for rounded clipping.
    pub corner_radius: f32,
    /// Tint color (Color::WHITE = no tint).
    pub tint: Color,
    /// Optional widget ID for hit testing (makes image clickable/draggable).
    pub widget_id: Option<SourceId>,
    /// Cursor hint shown when hovering over the image.
    pub cursor_hint: Option<CursorIcon>,
}

impl ImageElement {
    /// Create a new image element with explicit size.
    pub fn new(handle: ImageHandle, width: f32, height: f32) -> Self {
        Self {
            handle,
            width,
            height,
            corner_radius: 0.0,
            tint: Color::WHITE,
            widget_id: None,
            cursor_hint: None,
        }
    }

    /// Set corner radius for rounded clipping.
    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Set tint color (multiplied with image color).
    pub fn tint(mut self, tint: Color) -> Self {
        self.tint = tint;
        self
    }

    /// Set a widget ID for hit testing (makes the image clickable/draggable).
    pub fn widget_id(mut self, id: SourceId) -> Self {
        self.widget_id = Some(id);
        self
    }

    /// Set the cursor icon shown when hovering over this image.
    pub fn cursor(mut self, cursor: CursorIcon) -> Self {
        self.cursor_hint = Some(cursor);
        self
    }

    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

/// A button element descriptor.
///
/// Renders a padded text label with background and corner radius.
/// Auto-registers as a widget hit target for click detection via `on_mouse`.
pub struct ButtonElement {
    /// Widget ID for hit-testing (required).
    pub id: SourceId,
    /// Button label text.
    pub label: String,
    /// Text color.
    pub text_color: Color,
    /// Background color.
    pub background: Color,
    /// Corner radius.
    pub corner_radius: f32,
    /// Padding around the label.
    pub padding: Padding,
    /// Cache key for text rendering.
    cache_key: u64,
}

impl ButtonElement {
    pub fn new(id: SourceId, label: impl Into<String>) -> Self {
        let label = label.into();
        let cache_key = hash_text(&label);
        Self {
            id,
            label,
            text_color: Color::WHITE,
            background: Color::rgba(0.3, 0.3, 0.4, 1.0),
            corner_radius: 4.0,
            padding: Padding::new(3.0, 14.0, 3.0, 14.0),
            cache_key,
        }
    }

    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    pub fn background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.corner_radius = radius;
        self
    }

    pub fn padding(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }

    fn estimate_size(&self) -> Size {
        let char_count = self.label.chars().count() as f32;
        Size::new(
            char_count * CHAR_WIDTH + self.padding.horizontal(),
            LINE_HEIGHT + self.padding.vertical(),
        )
    }
}

/// A text input element descriptor.
///
/// Renders an editable text field with cursor and optional selection highlight.
/// All state is external — the app passes text, cursor, selection, and focus.
pub struct TextInputElement {
    pub id: SourceId,
    pub text: String,
    pub cursor: usize,
    pub selection: Option<(usize, usize)>,
    pub focused: bool,
    pub placeholder: String,
    pub text_color: Color,
    pub placeholder_color: Color,
    pub background: Color,
    pub border_color: Color,
    pub focus_border_color: Color,
    pub border_width: f32,
    pub corner_radius: f32,
    pub padding: Padding,
    pub width: Length,
    pub multiline: bool,
    pub height: Length,
    pub scroll_offset: f32,
    pub cursor_visible: bool,
    cache_key: u64,
}

impl TextInputElement {
    pub fn new(id: SourceId, text: impl Into<String>) -> Self {
        let text = text.into();
        let cache_key = hash_text(&text);
        Self {
            id,
            text,
            cursor: 0,
            selection: None,
            focused: false,
            placeholder: String::new(),
            text_color: Color::WHITE,
            placeholder_color: Color::rgba(0.4, 0.4, 0.45, 1.0),
            background: Color::rgba(0.10, 0.10, 0.13, 1.0),
            border_color: Color::rgba(1.0, 1.0, 1.0, 0.12),
            focus_border_color: Color::rgba(0.3, 0.5, 0.8, 0.6),
            border_width: 1.0,
            corner_radius: 6.0,
            padding: Padding::new(8.0, 12.0, 8.0, 12.0),
            width: Length::Fill,
            multiline: false,
            height: Length::Shrink,
            scroll_offset: 0.0,
            cursor_visible: true,
            cache_key,
        }
    }

    /// Create from a `TextInputState`, copying all state-driven fields.
    ///
    /// This pulls id, text, cursor, selection, focused, scroll_offset, and
    /// multiline from the state, so you only need to chain visual overrides.
    pub fn from_state(state: &TextInputState) -> Self {
        let mut el = Self::new(state.id(), &state.text);
        el.cursor = state.cursor;
        el.selection = state.selection;
        el.focused = state.focused;
        el.scroll_offset = state.scroll_offset;
        el.multiline = state.is_multiline();
        el
    }

    pub fn cursor(mut self, pos: usize) -> Self { self.cursor = pos; self }
    pub fn selection(mut self, range: Option<(usize, usize)>) -> Self { self.selection = range; self }
    pub fn focused(mut self, focused: bool) -> Self { self.focused = focused; self }
    pub fn placeholder(mut self, text: impl Into<String>) -> Self { self.placeholder = text.into(); self }
    pub fn text_color(mut self, color: Color) -> Self { self.text_color = color; self }
    pub fn background(mut self, color: Color) -> Self { self.background = color; self }
    pub fn border_color(mut self, color: Color) -> Self { self.border_color = color; self }
    pub fn focus_border_color(mut self, color: Color) -> Self { self.focus_border_color = color; self }
    pub fn corner_radius(mut self, radius: f32) -> Self { self.corner_radius = radius; self }
    pub fn padding(mut self, padding: Padding) -> Self { self.padding = padding; self }
    pub fn width(mut self, width: Length) -> Self { self.width = width; self }
    pub fn multiline(mut self, multiline: bool) -> Self { self.multiline = multiline; self }
    pub fn height(mut self, height: Length) -> Self { self.height = height; self }
    pub fn scroll_offset(mut self, offset: f32) -> Self { self.scroll_offset = offset; self }
    pub fn cursor_visible(mut self, visible: bool) -> Self { self.cursor_visible = visible; self }

    fn estimate_size(&self) -> Size {
        let text_w = self.text.chars().count().max(20) as f32 * CHAR_WIDTH;
        if self.multiline {
            let line_count = self.text.lines().count().max(1) as f32;
            let content_h = line_count * LINE_HEIGHT + self.padding.vertical();
            let h = match self.height {
                Length::Fixed(px) => px,
                _ => content_h,
            };
            Size::new(text_w + self.padding.horizontal(), h)
        } else {
            Size::new(
                text_w + self.padding.horizontal(),
                LINE_HEIGHT + self.padding.vertical(),
            )
        }
    }
}

// =========================================================================
// Table Element
// =========================================================================

/// A column definition for a table.
pub struct TableColumn {
    pub name: String,
    pub width: f32,
    pub sort_id: Option<SourceId>,
}

/// A cell in a table row.
pub struct TableCell {
    pub text: String,
    /// Pre-wrapped lines. If empty, `text` is rendered as a single line.
    pub lines: Vec<String>,
    pub color: Color,
    /// Optional widget ID for clickable cells (anchors).
    /// When set, the cell registers as a clickable widget with `CursorIcon::Pointer`.
    pub widget_id: Option<SourceId>,
}

/// A table element with column headers and data rows.
///
/// Headers register as clickable widgets (via `sort_id`) for sort interaction.
/// Data rows render as text primitives. All state is external.
pub struct TableElement {
    pub source_id: SourceId,
    pub columns: Vec<TableColumn>,
    pub rows: Vec<Vec<TableCell>>,
    pub header_bg: Color,
    pub header_text_color: Color,
    pub row_height: f32,
    pub line_height: f32,
    pub row_padding: f32,
    pub header_height: f32,
    pub stripe_color: Option<Color>,
    pub separator_color: Color,
}

impl TableElement {
    pub fn new(source_id: SourceId) -> Self {
        Self {
            source_id,
            columns: Vec::new(),
            rows: Vec::new(),
            header_bg: Color::rgba(0.15, 0.15, 0.2, 1.0),
            header_text_color: Color::rgba(0.6, 0.6, 0.65, 1.0),
            row_height: 22.0,
            line_height: 18.0,
            row_padding: 4.0,
            header_height: 26.0,
            stripe_color: Some(Color::rgba(1.0, 1.0, 1.0, 0.02)),
            separator_color: Color::rgba(1.0, 1.0, 1.0, 0.12),
        }
    }

    pub fn column(mut self, name: impl Into<String>, width: f32) -> Self {
        self.columns.push(TableColumn { name: name.into(), width, sort_id: None });
        self
    }

    pub fn column_sortable(mut self, name: impl Into<String>, width: f32, sort_id: SourceId) -> Self {
        self.columns.push(TableColumn { name: name.into(), width, sort_id: Some(sort_id) });
        self
    }

    pub fn row(mut self, cells: Vec<TableCell>) -> Self {
        self.rows.push(cells);
        self
    }

    pub fn header_bg(mut self, color: Color) -> Self { self.header_bg = color; self }
    pub fn header_text_color(mut self, color: Color) -> Self { self.header_text_color = color; self }
    pub fn row_height(mut self, height: f32) -> Self { self.row_height = height; self }
    pub fn header_height(mut self, height: f32) -> Self { self.header_height = height; self }
    pub fn stripe_color(mut self, color: Option<Color>) -> Self { self.stripe_color = color; self }
    pub fn separator_color(mut self, color: Color) -> Self { self.separator_color = color; self }

    fn estimate_size(&self) -> Size {
        let w: f32 = self.columns.iter().map(|c| c.width).sum();
        let rows_h: f32 = self.rows.iter().map(|row| self.row_height_for(row)).sum();
        let h = self.header_height + 1.0 + rows_h;
        Size::new(w, h)
    }

    /// Compute the height for a single row based on the tallest cell.
    fn row_height_for(&self, row: &[TableCell]) -> f32 {
        let max_lines = row.iter()
            .map(|cell| if cell.lines.is_empty() { 1 } else { cell.lines.len() })
            .max()
            .unwrap_or(1);
        if max_lines <= 1 {
            self.row_height // fast path: single-line rows use the fixed height
        } else {
            max_lines as f32 * self.line_height + self.row_padding
        }
    }
}

/// Render a table element into the snapshot.
fn render_table(
    snapshot: &mut LayoutSnapshot,
    table: TableElement,
    x: f32, y: f32, w: f32, _h: f32,
) {
    use crate::primitives::Point;

    let cell_pad = 8.0;

    // Header background
    snapshot.primitives_mut().add_solid_rect(
        Rect::new(x, y, w, table.header_height),
        table.header_bg,
    );

    // Header text + register sortable headers as widgets
    let mut col_x = x;
    let char_width = 8.4_f32;
    for col in &table.columns {
        let tx = col_x + cell_pad;
        let ty = y + 4.0;
        snapshot.primitives_mut().add_text_cached(
            col.name.clone(),
            Point::new(tx, ty),
            table.header_text_color,
            BASE_FONT_SIZE,
            hash_text(&col.name),
        );
        // Register header text for selection
        {
            use crate::layout_snapshot::{SourceLayout, TextLayout};
            let text_layout = TextLayout::simple(
                col.name.clone(), table.header_text_color.pack(),
                tx, ty, char_width, table.line_height,
            );
            snapshot.register_source(table.source_id, SourceLayout::text(text_layout));
        }
        if let Some(sort_id) = col.sort_id {
            snapshot.register_widget(sort_id, Rect::new(col_x, y, col.width, table.header_height));
            snapshot.set_cursor_hint(sort_id, CursorIcon::Pointer);
        }
        col_x += col.width;
    }

    // Separator line
    let sep_y = y + table.header_height;
    snapshot.primitives_mut().add_line(
        Point::new(x, sep_y),
        Point::new(x + w, sep_y),
        1.0,
        table.separator_color,
    );

    // Data rows — variable height based on wrapped line count
    let data_y = sep_y + 1.0;
    let mut ry = data_y;
    let char_width = 8.4_f32;
    for (row_idx, row) in table.rows.iter().enumerate() {
        let rh = table.row_height_for(row);

        // Stripe background for odd rows
        if row_idx % 2 == 1 {
            if let Some(stripe) = table.stripe_color {
                snapshot.primitives_mut().add_solid_rect(
                    Rect::new(x, ry, w, rh),
                    stripe,
                );
            }
        }

        let mut col_x = x;
        for (col_idx, cell) in row.iter().enumerate() {
            if col_idx < table.columns.len() {
                if cell.lines.len() <= 1 {
                    // Single line (fast path)
                    let text = if cell.lines.len() == 1 { &cell.lines[0] } else { &cell.text };
                    let tx = col_x + cell_pad;
                    let ty = ry + 2.0;
                    snapshot.primitives_mut().add_text_cached(
                        text.clone(),
                        Point::new(tx, ty),
                        cell.color,
                        BASE_FONT_SIZE,
                        hash_text(text) ^ (row_idx as u64),
                    );
                    // Register for selection
                    use crate::layout_snapshot::{SourceLayout, TextLayout};
                    let text_layout = TextLayout::simple(
                        text.clone(), cell.color.pack(),
                        tx, ty, char_width, table.line_height,
                    );
                    snapshot.register_source(table.source_id, SourceLayout::text(text_layout));
                } else {
                    // Multi-line wrapped cell
                    for (line_idx, line) in cell.lines.iter().enumerate() {
                        let tx = col_x + cell_pad;
                        let ly = ry + 2.0 + line_idx as f32 * table.line_height;
                        snapshot.primitives_mut().add_text_cached(
                            line.clone(),
                            Point::new(tx, ly),
                            cell.color,
                            BASE_FONT_SIZE,
                            hash_text(line) ^ (row_idx as u64) ^ ((line_idx as u64) << 32),
                        );
                        // Register for selection
                        use crate::layout_snapshot::{SourceLayout, TextLayout};
                        let text_layout = TextLayout::simple(
                            line.clone(), cell.color.pack(),
                            tx, ly, char_width, table.line_height,
                        );
                        snapshot.register_source(table.source_id, SourceLayout::text(text_layout));
                    }
                }
                // Register clickable cell as widget
                if let Some(wid) = cell.widget_id {
                    let cell_rect = Rect::new(col_x, ry, table.columns[col_idx].width, rh);
                    snapshot.register_widget(wid, cell_rect);
                    snapshot.set_cursor_hint(wid, CursorIcon::Pointer);
                }
                col_x += table.columns[col_idx].width;
            }
        }

        ry += rh;
    }

}

// =========================================================================
// Column
// =========================================================================

/// A vertical layout container (children flow top to bottom).
pub struct Column {
    /// Widget ID for hit-testing and overlay anchoring.
    id: Option<SourceId>,
    /// Child elements.
    children: Vec<LayoutChild>,
    /// Spacing between children.
    spacing: f32,
    /// Padding around all children.
    padding: Padding,
    /// Main axis alignment.
    alignment: Alignment,
    /// Cross axis alignment.
    cross_alignment: CrossAxisAlignment,
    /// Background color (optional).
    background: Option<Color>,
    /// Corner radius for background.
    corner_radius: f32,
    /// Width sizing mode.
    pub(crate) width: Length,
    /// Height sizing mode.
    pub(crate) height: Length,
    /// Border color (optional).
    border_color: Option<Color>,
    /// Border width.
    border_width: f32,
    /// Shadow: (blur_radius, color).
    shadow: Option<(f32, Color)>,
    /// Cursor hint when hovering (requires `id` to take effect).
    cursor_hint: Option<CursorIcon>,
}

impl Default for Column {
    fn default() -> Self {
        Self::new()
    }
}

impl Column {
    /// Create a new column.
    pub fn new() -> Self {
        Self {
            id: None,
            children: Vec::new(),
            spacing: 0.0,
            padding: Padding::default(),
            alignment: Alignment::Start,
            cross_alignment: CrossAxisAlignment::Start,
            background: None,
            corner_radius: 0.0,
            width: Length::Shrink,
            height: Length::Shrink,
            border_color: None,
            border_width: 0.0,
            shadow: None,
            cursor_hint: None,
        }
    }

    /// Set widget ID for hit-testing and overlay anchoring.
    pub fn id(mut self, id: SourceId) -> Self {
        self.id = Some(id);
        self
    }

    /// Set spacing between children.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set padding (uniform on all sides).
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = Padding::all(padding);
        self
    }

    /// Set custom padding.
    pub fn padding_custom(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }

    /// Set main axis alignment.
    pub fn align(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Set cross axis alignment.
    pub fn cross_align(mut self, alignment: CrossAxisAlignment) -> Self {
        self.cross_alignment = alignment;
        self
    }

    /// Set background color.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Set corner radius for background.
    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Set width sizing mode.
    pub fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    /// Set height sizing mode.
    pub fn height(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    /// Set border (color + width).
    pub fn border(mut self, color: Color, width: f32) -> Self {
        self.border_color = Some(color);
        self.border_width = width;
        self
    }

    /// Set drop shadow (blur_radius, color).
    pub fn shadow(mut self, blur: f32, color: Color) -> Self {
        self.shadow = Some((blur, color));
        self
    }

    /// Add a text element.
    pub fn text(mut self, element: TextElement) -> Self {
        self.children.push(LayoutChild::Text(element));
        self
    }

    /// Add a terminal element.
    pub fn terminal(mut self, element: TerminalElement) -> Self {
        self.children.push(LayoutChild::Terminal(element));
        self
    }

    /// Add a nested column.
    pub fn column(mut self, column: Column) -> Self {
        self.children.push(LayoutChild::Column(column));
        self
    }

    /// Add a nested row.
    pub fn row(mut self, row: Row) -> Self {
        self.children.push(LayoutChild::Row(row));
        self
    }

    /// Add a scroll column.
    pub fn scroll_column(mut self, scroll: ScrollColumn) -> Self {
        self.children.push(LayoutChild::ScrollColumn(scroll));
        self
    }

    /// Add a flexible spacer.
    pub fn spacer(mut self, flex: f32) -> Self {
        self.children.push(LayoutChild::Spacer { flex });
        self
    }

    /// Add a fixed-size spacer.
    pub fn fixed_spacer(mut self, size: f32) -> Self {
        self.children.push(LayoutChild::FixedSpacer { size });
        self
    }

    /// Add an image element.
    pub fn image(mut self, element: ImageElement) -> Self {
        self.children.push(LayoutChild::Image(element));
        self
    }

    /// Add a button element.
    pub fn button(mut self, element: ButtonElement) -> Self {
        self.children.push(LayoutChild::Button(element));
        self
    }

    /// Add a text input element.
    pub fn text_input(mut self, element: TextInputElement) -> Self {
        self.children.push(LayoutChild::TextInput(element));
        self
    }

    pub fn table(mut self, element: TableElement) -> Self {
        self.children.push(LayoutChild::Table(element));
        self
    }

    /// Add any child element using `From<T> for LayoutChild`.
    ///
    /// This is a generic alternative to the type-specific methods above.
    /// The compiler resolves the `Into` conversion at compile time, so this
    /// generates identical code to calling `.text()`, `.button()`, etc. directly.
    #[inline(always)]
    pub fn push(mut self, child: impl Into<LayoutChild>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Compute intrinsic size (content size + padding).
    ///
    /// Short-circuits on Fixed axes — does not recurse into children
    /// for dimensions that are already determined.
    pub fn measure(&self) -> Size {
        let intrinsic_width = match self.width {
            Length::Fixed(px) => px,
            _ => {
                let mut max_child_width: f32 = 0.0;
                for child in &self.children {
                    max_child_width = max_child_width.max(child.measure_cross(true));
                }
                max_child_width + self.padding.horizontal()
            }
        };

        let intrinsic_height = match self.height {
            Length::Fixed(px) => px,
            _ => {
                let mut total_height: f32 = 0.0;
                for child in &self.children {
                    // Skip flex children in measurement (they fill remaining space)
                    if child.flex_factor(true) > 0.0 {
                        continue;
                    }
                    total_height += child.measure_main(true);
                }
                // Spacing between all children (flex children still occupy a slot)
                if self.children.len() > 1 {
                    total_height += self.spacing * (self.children.len() - 1) as f32;
                }
                total_height + self.padding.vertical()
            }
        };

        Size::new(intrinsic_width, intrinsic_height)
    }

    /// Compute layout and flush to snapshot.
    ///
    /// This is where the actual layout math happens - ONCE per frame.
    pub fn layout(self, snapshot: &mut LayoutSnapshot, bounds: Rect) {
        // Available space after padding
        let content_x = bounds.x + self.padding.left;
        let content_y = bounds.y + self.padding.top;
        let content_width = bounds.width - self.padding.horizontal();

        // Draw shadow → background → border (correct z-order)
        // These are drawn OUTSIDE the clip rect (they ARE the container chrome).
        if let Some((blur, color)) = self.shadow {
            snapshot.primitives_mut().add_shadow(
                Rect::new(bounds.x + 4.0, bounds.y + 4.0, bounds.width, bounds.height),
                self.corner_radius,
                blur,
                color,
            );
        }
        if let Some(bg) = self.background {
            if self.corner_radius > 0.0 {
                snapshot.primitives_mut().add_rounded_rect(bounds, self.corner_radius, bg);
            } else {
                snapshot.primitives_mut().add_solid_rect(bounds, bg);
            }
        }
        if let Some(border_color) = self.border_color {
            snapshot.primitives_mut().add_border(
                bounds,
                self.corner_radius,
                self.border_width,
                border_color,
            );
        }

        let has_chrome = self.background.is_some() || self.border_color.is_some();

        // =====================================================================
        // Measurement pass: compute child heights and flex factors
        // Also tracks max cross-axis width for overflow detection.
        // =====================================================================
        let mut total_fixed_height = 0.0;
        let mut total_flex = 0.0;
        let mut max_child_cross: f32 = 0.0;
        let mut child_heights: Vec<f32> = Vec::with_capacity(self.children.len());

        for child in &self.children {
            max_child_cross = max_child_cross.max(child.measure_cross(true));
            match child {
                LayoutChild::Text(t) => {
                    let h = t.estimate_size(CHAR_WIDTH, LINE_HEIGHT).height;
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::Terminal(t) => {
                    let h = t.size().height;
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::Column(c) => {
                    match c.height {
                        Length::Fixed(px) => {
                            child_heights.push(px);
                            total_fixed_height += px;
                        }
                        Length::Fill | Length::FillPortion(_) => {
                            child_heights.push(0.0);
                            total_flex += c.height.flex();
                        }
                        Length::Shrink => {
                            let h = c.measure().height;
                            child_heights.push(h);
                            total_fixed_height += h;
                        }
                    }
                }
                LayoutChild::Row(r) => {
                    match r.height {
                        Length::Fixed(px) => {
                            child_heights.push(px);
                            total_fixed_height += px;
                        }
                        Length::Fill | Length::FillPortion(_) => {
                            child_heights.push(0.0);
                            total_flex += r.height.flex();
                        }
                        Length::Shrink => {
                            let h = r.measure().height;
                            child_heights.push(h);
                            total_fixed_height += h;
                        }
                    }
                }
                LayoutChild::ScrollColumn(s) => {
                    match s.height {
                        Length::Fixed(px) => {
                            child_heights.push(px);
                            total_fixed_height += px;
                        }
                        Length::Fill | Length::FillPortion(_) => {
                            child_heights.push(0.0);
                            total_flex += s.height.flex();
                        }
                        Length::Shrink => {
                            let h = s.measure().height;
                            child_heights.push(h);
                            total_fixed_height += h;
                        }
                    }
                }
                LayoutChild::Image(img) => {
                    let h = img.height;
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::Button(btn) => {
                    let h = btn.estimate_size().height;
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::TextInput(input) => {
                    let h = LINE_HEIGHT + input.padding.vertical();
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::Table(table) => {
                    let h = table.estimate_size().height;
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::Spacer { flex } => {
                    child_heights.push(0.0);
                    total_flex += flex;
                }
                LayoutChild::FixedSpacer { size } => {
                    child_heights.push(*size);
                    total_fixed_height += size;
                }
            }
        }

        // Add spacing to fixed height
        if !self.children.is_empty() {
            total_fixed_height += self.spacing * (self.children.len() - 1) as f32;
        }

        // Overflow detection (replaces previous self.measure() call)
        let content_w = max_child_cross + self.padding.horizontal();
        let content_h = total_fixed_height + self.padding.vertical();
        let content_overflows = bounds.width < content_w || bounds.height < content_h;
        let clips = has_chrome || content_overflows;
        if clips {
            snapshot.primitives_mut().push_clip(bounds);
        }

        let available_flex = (bounds.height - self.padding.vertical() - total_fixed_height).max(0.0);

        // Compute total consumed height (flex children consume available_flex)
        let total_flex_consumed = if total_flex > 0.0 { available_flex } else { 0.0 };
        let used_height = total_fixed_height + total_flex_consumed;
        let free_space = (bounds.height - self.padding.vertical() - used_height).max(0.0);

        // =====================================================================
        // Main axis alignment: compute starting y and extra per-gap spacing
        // =====================================================================
        let n = self.children.len();
        let (mut y, alignment_gap) = match self.alignment {
            Alignment::Start => (content_y, 0.0),
            Alignment::End => (content_y + free_space, 0.0),
            Alignment::Center => (content_y + free_space / 2.0, 0.0),
            Alignment::SpaceBetween => {
                if n > 1 {
                    (content_y, free_space / (n - 1) as f32)
                } else {
                    (content_y, 0.0)
                }
            }
            Alignment::SpaceAround => {
                if n > 0 {
                    let space = free_space / n as f32;
                    (content_y + space / 2.0, space)
                } else {
                    (content_y, 0.0)
                }
            }
        };

        // =====================================================================
        // Position pass: place children and flush to snapshot
        // =====================================================================
        for (i, child) in self.children.into_iter().enumerate() {
            let mut height = child_heights[i];

            // Helper: resolve cross-axis x position for a child of given width
            let cross_x = |child_width: f32| -> f32 {
                match self.cross_alignment {
                    CrossAxisAlignment::Start | CrossAxisAlignment::Stretch => content_x,
                    CrossAxisAlignment::End => content_x + content_width - child_width,
                    CrossAxisAlignment::Center => content_x + (content_width - child_width) / 2.0,
                }
            };

            match child {
                LayoutChild::Text(t) => {
                    let fs = t.font_size();
                    let size = t.estimate_size(CHAR_WIDTH, LINE_HEIGHT);
                    let x = cross_x(size.width);

                    use crate::layout_snapshot::{SourceLayout, TextLayout};
                    if let Some(source_id) = t.source_id {
                        let scale = fs / BASE_FONT_SIZE;
                        let text_layout = TextLayout::simple(
                            t.text.clone(),
                            t.color.pack(),
                            x, y,
                            CHAR_WIDTH * scale, LINE_HEIGHT * scale,
                        );
                        snapshot.register_source(source_id, SourceLayout::text(text_layout));
                    }

                    snapshot.primitives_mut().add_text_cached(
                        t.text,
                        crate::primitives::Point::new(x, y),
                        t.color,
                        fs,
                        t.cache_key,
                    );

                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::Terminal(t) => {
                    let size = t.size();
                    let x = cross_x(size.width);

                    use crate::layout_snapshot::{GridLayout, GridRow, SourceLayout};
                    let rows_content: Vec<GridRow> = t.row_content.into_iter()
                        .map(|(text, color)| GridRow { text, color })
                        .collect();
                    let mut grid_layout = GridLayout::with_rows(
                        Rect::new(x, y, size.width, size.height),
                        t.cell_width, t.cell_height,
                        t.cols, t.rows,
                        rows_content,
                    );
                    grid_layout.clip_rect = snapshot.current_clip();
                    snapshot.register_source(t.source_id, SourceLayout::grid(grid_layout));

                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::Image(img) => {
                    let x = cross_x(img.width);
                    let img_rect = Rect::new(x, y, img.width, img.height);
                    snapshot.primitives_mut().add_image(
                        img_rect,
                        img.handle,
                        img.corner_radius,
                        img.tint,
                    );
                    if let Some(id) = img.widget_id {
                        snapshot.register_widget(id, img_rect);
                        if let Some(cursor) = img.cursor_hint {
                            snapshot.set_cursor_hint(id, cursor);
                        }
                    }
                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::Button(btn) => {
                    let size = btn.estimate_size();
                    let bx = cross_x(size.width);
                    let btn_rect = Rect::new(bx, y, size.width, size.height);
                    snapshot.primitives_mut().add_rounded_rect(btn_rect, btn.corner_radius, btn.background);
                    snapshot.primitives_mut().add_text_cached(
                        btn.label,
                        crate::primitives::Point::new(bx + btn.padding.left, y + btn.padding.top),
                        btn.text_color,
                        BASE_FONT_SIZE,
                        btn.cache_key,
                    );
                    snapshot.register_widget(btn.id, btn_rect);
                    snapshot.set_cursor_hint(btn.id, CursorIcon::Pointer);
                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::TextInput(input) => {
                    let h = if input.multiline {
                        input.estimate_size().height
                    } else {
                        LINE_HEIGHT + input.padding.vertical()
                    };
                    let w = match input.width {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) => content_width,
                        Length::Shrink => input.estimate_size().width.min(content_width),
                    };
                    let ix = cross_x(w);
                    if input.multiline {
                        render_text_input_multiline(snapshot, input, ix, y, w, h);
                    } else {
                        render_text_input(snapshot, input, ix, y, w, h);
                    }
                    y += h + self.spacing + alignment_gap;
                }
                LayoutChild::Table(table) => {
                    let size = table.estimate_size();
                    let w = size.width.min(content_width);
                    let tx = cross_x(w);
                    render_table(snapshot, table, tx, y, w, size.height);
                    y += size.height + self.spacing + alignment_gap;
                }
                LayoutChild::Column(nested) => {
                    // Resolve flex height for Fill children
                    if nested.height.is_flex() && total_flex > 0.0 {
                        height = (nested.height.flex() / total_flex) * available_flex;
                    }
                    // Resolve width
                    let w = match nested.width {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) => content_width,
                        Length::Shrink => nested.measure().width.min(content_width),
                    };
                    let x = cross_x(w);
                    let nested_bounds = Rect::new(x, y, w, height);
                    nested.layout(snapshot, nested_bounds);
                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::Row(nested) => {
                    // Resolve flex height for Fill children
                    if nested.height.is_flex() && total_flex > 0.0 {
                        height = (nested.height.flex() / total_flex) * available_flex;
                    }
                    // Resolve width
                    let w = match nested.width {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) => content_width,
                        Length::Shrink => nested.measure().width.min(content_width),
                    };
                    let x = cross_x(w);
                    let nested_bounds = Rect::new(x, y, w, height);
                    nested.layout(snapshot, nested_bounds);
                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::ScrollColumn(nested) => {
                    // Resolve flex height for Fill children
                    if nested.height.is_flex() && total_flex > 0.0 {
                        height = (nested.height.flex() / total_flex) * available_flex;
                    }
                    // Resolve width
                    let w = match nested.width {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) => content_width,
                        Length::Shrink => nested.measure().width.min(content_width),
                    };
                    let x = cross_x(w);
                    let nested_bounds = Rect::new(x, y, w, height);
                    nested.layout(snapshot, nested_bounds);
                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::Spacer { flex } => {
                    if total_flex > 0.0 {
                        let space = (flex / total_flex) * available_flex;
                        y += space;
                    }
                    y += alignment_gap;
                }
                LayoutChild::FixedSpacer { size } => {
                    y += size + alignment_gap;
                }
            }
        }

        // Register widget ID for hit-testing and overlay anchoring
        if let Some(id) = self.id {
            snapshot.register_widget(id, bounds);
            if let Some(cursor) = self.cursor_hint {
                snapshot.set_cursor_hint(id, cursor);
            }
        }

        if clips {
            snapshot.primitives_mut().pop_clip();
        }
    }
}

// =========================================================================
// Row
// =========================================================================

/// A horizontal layout container (children flow left to right).
pub struct Row {
    /// Widget ID for hit-testing and overlay anchoring.
    id: Option<SourceId>,
    /// Child elements.
    children: Vec<LayoutChild>,
    /// Spacing between children.
    spacing: f32,
    /// Padding around all children.
    padding: Padding,
    /// Main axis alignment.
    alignment: Alignment,
    /// Cross axis alignment.
    cross_alignment: CrossAxisAlignment,
    /// Background color (optional).
    background: Option<Color>,
    /// Corner radius for background.
    corner_radius: f32,
    /// Width sizing mode.
    pub(crate) width: Length,
    /// Height sizing mode.
    pub(crate) height: Length,
    /// Border color (optional).
    border_color: Option<Color>,
    /// Border width.
    border_width: f32,
    /// Shadow: (blur_radius, color).
    shadow: Option<(f32, Color)>,
    /// Cursor hint when hovering (requires `id` to take effect).
    cursor_hint: Option<CursorIcon>,
}

impl Default for Row {
    fn default() -> Self {
        Self::new()
    }
}

impl Row {
    /// Create a new row.
    pub fn new() -> Self {
        Self {
            id: None,
            children: Vec::new(),
            spacing: 0.0,
            padding: Padding::default(),
            alignment: Alignment::Start,
            cross_alignment: CrossAxisAlignment::Start,
            background: None,
            corner_radius: 0.0,
            width: Length::Shrink,
            height: Length::Shrink,
            border_color: None,
            border_width: 0.0,
            shadow: None,
            cursor_hint: None,
        }
    }

    /// Set widget ID for hit-testing and overlay anchoring.
    pub fn id(mut self, id: SourceId) -> Self {
        self.id = Some(id);
        self
    }

    /// Set cursor hint for hover feedback (requires `id` to take effect).
    pub fn cursor_hint(mut self, cursor: CursorIcon) -> Self {
        self.cursor_hint = Some(cursor);
        self
    }

    /// Set spacing between children.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set padding (uniform on all sides).
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = Padding::all(padding);
        self
    }

    /// Set custom padding.
    pub fn padding_custom(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }

    /// Set main axis alignment.
    pub fn align(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Set cross axis alignment.
    pub fn cross_align(mut self, alignment: CrossAxisAlignment) -> Self {
        self.cross_alignment = alignment;
        self
    }

    /// Set background color.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Set corner radius for background.
    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Set width sizing mode.
    pub fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    /// Set height sizing mode.
    pub fn height(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    /// Set border (color + width).
    pub fn border(mut self, color: Color, width: f32) -> Self {
        self.border_color = Some(color);
        self.border_width = width;
        self
    }

    /// Set drop shadow (blur_radius, color).
    pub fn shadow(mut self, blur: f32, color: Color) -> Self {
        self.shadow = Some((blur, color));
        self
    }

    /// Add a text element.
    pub fn text(mut self, element: TextElement) -> Self {
        self.children.push(LayoutChild::Text(element));
        self
    }

    /// Add a terminal element.
    pub fn terminal(mut self, element: TerminalElement) -> Self {
        self.children.push(LayoutChild::Terminal(element));
        self
    }

    /// Add a nested column.
    pub fn column(mut self, column: Column) -> Self {
        self.children.push(LayoutChild::Column(column));
        self
    }

    /// Add a nested row.
    pub fn row(mut self, row: Row) -> Self {
        self.children.push(LayoutChild::Row(row));
        self
    }

    /// Add a scroll column.
    pub fn scroll_column(mut self, scroll: ScrollColumn) -> Self {
        self.children.push(LayoutChild::ScrollColumn(scroll));
        self
    }

    /// Add a flexible spacer.
    pub fn spacer(mut self, flex: f32) -> Self {
        self.children.push(LayoutChild::Spacer { flex });
        self
    }

    /// Add a fixed-size spacer.
    pub fn fixed_spacer(mut self, size: f32) -> Self {
        self.children.push(LayoutChild::FixedSpacer { size });
        self
    }

    /// Add an image element.
    pub fn image(mut self, element: ImageElement) -> Self {
        self.children.push(LayoutChild::Image(element));
        self
    }

    /// Add a button element.
    pub fn button(mut self, element: ButtonElement) -> Self {
        self.children.push(LayoutChild::Button(element));
        self
    }

    /// Add a text input element.
    pub fn text_input(mut self, element: TextInputElement) -> Self {
        self.children.push(LayoutChild::TextInput(element));
        self
    }

    /// Add a table element.
    pub fn table(mut self, element: TableElement) -> Self {
        self.children.push(LayoutChild::Table(element));
        self
    }

    /// Add any child element using `From<T> for LayoutChild`.
    #[inline(always)]
    pub fn push(mut self, child: impl Into<LayoutChild>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Compute intrinsic size (content size + padding).
    ///
    /// Short-circuits on Fixed axes.
    pub fn measure(&self) -> Size {
        let intrinsic_width = match self.width {
            Length::Fixed(px) => px,
            _ => {
                let mut total_width: f32 = 0.0;
                for child in &self.children {
                    if child.flex_factor(false) > 0.0 {
                        continue;
                    }
                    total_width += child.measure_main(false);
                }
                // Spacing between all children (flex children still occupy a slot)
                if self.children.len() > 1 {
                    total_width += self.spacing * (self.children.len() - 1) as f32;
                }
                total_width + self.padding.horizontal()
            }
        };

        let intrinsic_height = match self.height {
            Length::Fixed(px) => px,
            _ => {
                let mut max_child_height: f32 = 0.0;
                for child in &self.children {
                    max_child_height = max_child_height.max(child.measure_cross(false));
                }
                max_child_height + self.padding.vertical()
            }
        };

        Size::new(intrinsic_width, intrinsic_height)
    }

    /// Compute layout and flush to snapshot.
    pub fn layout(self, snapshot: &mut LayoutSnapshot, bounds: Rect) {
        // Available space after padding
        let content_x = bounds.x + self.padding.left;
        let content_y = bounds.y + self.padding.top;
        let content_height = bounds.height - self.padding.vertical();

        // Draw shadow → background → border (outside clip)
        if let Some((blur, color)) = self.shadow {
            snapshot.primitives_mut().add_shadow(
                Rect::new(bounds.x + 4.0, bounds.y + 4.0, bounds.width, bounds.height),
                self.corner_radius,
                blur,
                color,
            );
        }
        if let Some(bg) = self.background {
            if self.corner_radius > 0.0 {
                snapshot.primitives_mut().add_rounded_rect(bounds, self.corner_radius, bg);
            } else {
                snapshot.primitives_mut().add_solid_rect(bounds, bg);
            }
        }
        if let Some(border_color) = self.border_color {
            snapshot.primitives_mut().add_border(
                bounds,
                self.corner_radius,
                self.border_width,
                border_color,
            );
        }

        let has_chrome = self.background.is_some() || self.border_color.is_some();

        // =====================================================================
        // Measurement pass: compute child widths and flex factors.
        // Also tracks max cross-axis height for overflow detection.
        // =====================================================================
        let mut total_fixed_width = 0.0;
        let mut total_flex = 0.0;
        let mut max_child_cross: f32 = 0.0;
        let mut child_widths: Vec<f32> = Vec::with_capacity(self.children.len());

        for child in &self.children {
            max_child_cross = max_child_cross.max(child.measure_cross(false));
            match child {
                LayoutChild::Text(t) => {
                    let w = t.estimate_size(CHAR_WIDTH, LINE_HEIGHT).width;
                    child_widths.push(w);
                    total_fixed_width += w;
                }
                LayoutChild::Terminal(t) => {
                    let w = t.size().width;
                    child_widths.push(w);
                    total_fixed_width += w;
                }
                LayoutChild::Column(c) => {
                    match c.width {
                        Length::Fixed(px) => {
                            child_widths.push(px);
                            total_fixed_width += px;
                        }
                        Length::Fill | Length::FillPortion(_) => {
                            child_widths.push(0.0);
                            total_flex += c.width.flex();
                        }
                        Length::Shrink => {
                            let w = c.measure().width;
                            child_widths.push(w);
                            total_fixed_width += w;
                        }
                    }
                }
                LayoutChild::Row(r) => {
                    match r.width {
                        Length::Fixed(px) => {
                            child_widths.push(px);
                            total_fixed_width += px;
                        }
                        Length::Fill | Length::FillPortion(_) => {
                            child_widths.push(0.0);
                            total_flex += r.width.flex();
                        }
                        Length::Shrink => {
                            let w = r.measure().width;
                            child_widths.push(w);
                            total_fixed_width += w;
                        }
                    }
                }
                LayoutChild::ScrollColumn(s) => {
                    match s.width {
                        Length::Fixed(px) => {
                            child_widths.push(px);
                            total_fixed_width += px;
                        }
                        Length::Fill | Length::FillPortion(_) => {
                            child_widths.push(0.0);
                            total_flex += s.width.flex();
                        }
                        Length::Shrink => {
                            let w = s.measure().width;
                            child_widths.push(w);
                            total_fixed_width += w;
                        }
                    }
                }
                LayoutChild::Image(img) => {
                    let w = img.width;
                    child_widths.push(w);
                    total_fixed_width += w;
                }
                LayoutChild::Button(btn) => {
                    let w = btn.estimate_size().width;
                    child_widths.push(w);
                    total_fixed_width += w;
                }
                LayoutChild::TextInput(input) => {
                    match input.width {
                        Length::Fill | Length::FillPortion(_) => {
                            child_widths.push(0.0);
                            total_flex += input.width.flex();
                        }
                        Length::Fixed(px) => {
                            child_widths.push(px);
                            total_fixed_width += px;
                        }
                        Length::Shrink => {
                            let w = input.estimate_size().width;
                            child_widths.push(w);
                            total_fixed_width += w;
                        }
                    }
                }
                LayoutChild::Table(table) => {
                    let w = table.estimate_size().width;
                    child_widths.push(w);
                    total_fixed_width += w;
                }
                LayoutChild::Spacer { flex } => {
                    child_widths.push(0.0);
                    total_flex += flex;
                }
                LayoutChild::FixedSpacer { size } => {
                    child_widths.push(*size);
                    total_fixed_width += size;
                }
            }
        }

        // Add spacing to fixed width
        if !self.children.is_empty() {
            total_fixed_width += self.spacing * (self.children.len() - 1) as f32;
        }

        // Overflow detection (replaces previous self.measure() call)
        let content_w = total_fixed_width + self.padding.horizontal();
        let content_h = max_child_cross + self.padding.vertical();
        let content_overflows = bounds.width < content_w || bounds.height < content_h;
        let clips = has_chrome || content_overflows;
        if clips {
            snapshot.primitives_mut().push_clip(bounds);
        }

        let available_flex = (bounds.width - self.padding.horizontal() - total_fixed_width).max(0.0);

        // Compute total consumed width (flex children consume available_flex)
        let total_flex_consumed = if total_flex > 0.0 { available_flex } else { 0.0 };
        let used_width = total_fixed_width + total_flex_consumed;
        let free_space = (bounds.width - self.padding.horizontal() - used_width).max(0.0);

        // =====================================================================
        // Main axis alignment: compute starting x and extra per-gap spacing
        // =====================================================================
        let n = self.children.len();
        let (mut x, alignment_gap) = match self.alignment {
            Alignment::Start => (content_x, 0.0),
            Alignment::End => (content_x + free_space, 0.0),
            Alignment::Center => (content_x + free_space / 2.0, 0.0),
            Alignment::SpaceBetween => {
                if n > 1 {
                    (content_x, free_space / (n - 1) as f32)
                } else {
                    (content_x, 0.0)
                }
            }
            Alignment::SpaceAround => {
                if n > 0 {
                    let space = free_space / n as f32;
                    (content_x + space / 2.0, space)
                } else {
                    (content_x, 0.0)
                }
            }
        };

        // =====================================================================
        // Position pass: place children and flush to snapshot
        // =====================================================================
        for (i, child) in self.children.into_iter().enumerate() {
            let mut width = child_widths[i];

            // Helper: resolve cross-axis y position for a child of given height
            let cross_y = |child_height: f32| -> f32 {
                match self.cross_alignment {
                    CrossAxisAlignment::Start | CrossAxisAlignment::Stretch => content_y,
                    CrossAxisAlignment::End => content_y + content_height - child_height,
                    CrossAxisAlignment::Center => {
                        content_y + (content_height - child_height) / 2.0
                    }
                }
            };

            match child {
                LayoutChild::Text(t) => {
                    let fs = t.font_size();
                    let size = t.estimate_size(CHAR_WIDTH, LINE_HEIGHT);
                    let y = cross_y(size.height);

                    use crate::layout_snapshot::{SourceLayout, TextLayout};
                    if let Some(source_id) = t.source_id {
                        let scale = fs / BASE_FONT_SIZE;
                        let text_layout = TextLayout::simple(
                            t.text.clone(),
                            t.color.pack(),
                            x, y,
                            CHAR_WIDTH * scale, LINE_HEIGHT * scale,
                        );
                        snapshot.register_source(source_id, SourceLayout::text(text_layout));
                    }

                    snapshot.primitives_mut().add_text_cached(
                        t.text,
                        crate::primitives::Point::new(x, y),
                        t.color,
                        fs,
                        t.cache_key,
                    );

                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Terminal(t) => {
                    let size = t.size();
                    let y = cross_y(size.height);

                    use crate::layout_snapshot::{GridLayout, GridRow, SourceLayout};
                    let rows_content: Vec<GridRow> = t.row_content.into_iter()
                        .map(|(text, color)| GridRow { text, color })
                        .collect();
                    let mut grid_layout = GridLayout::with_rows(
                        Rect::new(x, y, size.width, size.height),
                        t.cell_width, t.cell_height,
                        t.cols, t.rows,
                        rows_content,
                    );
                    grid_layout.clip_rect = snapshot.current_clip();
                    snapshot.register_source(t.source_id, SourceLayout::grid(grid_layout));

                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Image(img) => {
                    let y = cross_y(img.height);
                    let img_rect = Rect::new(x, y, img.width, img.height);
                    snapshot.primitives_mut().add_image(
                        img_rect,
                        img.handle,
                        img.corner_radius,
                        img.tint,
                    );
                    if let Some(id) = img.widget_id {
                        snapshot.register_widget(id, img_rect);
                        if let Some(cursor) = img.cursor_hint {
                            snapshot.set_cursor_hint(id, cursor);
                        }
                    }
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Button(btn) => {
                    let size = btn.estimate_size();
                    let by = cross_y(size.height);
                    let btn_rect = Rect::new(x, by, size.width, size.height);
                    snapshot.primitives_mut().add_rounded_rect(btn_rect, btn.corner_radius, btn.background);
                    snapshot.primitives_mut().add_text_cached(
                        btn.label,
                        crate::primitives::Point::new(x + btn.padding.left, by + btn.padding.top),
                        btn.text_color,
                        BASE_FONT_SIZE,
                        btn.cache_key,
                    );
                    snapshot.register_widget(btn.id, btn_rect);
                    snapshot.set_cursor_hint(btn.id, CursorIcon::Pointer);
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::TextInput(input) => {
                    let w = if input.width.is_flex() && total_flex > 0.0 {
                        (input.width.flex() / total_flex) * available_flex
                    } else {
                        width
                    };
                    let h = if input.multiline {
                        input.estimate_size().height
                    } else {
                        LINE_HEIGHT + input.padding.vertical()
                    };
                    let iy = cross_y(h);
                    if input.multiline {
                        render_text_input_multiline(snapshot, input, x, iy, w, h);
                    } else {
                        render_text_input(snapshot, input, x, iy, w, h);
                    }
                    x += w + self.spacing + alignment_gap;
                }
                LayoutChild::Table(table) => {
                    let size = table.estimate_size();
                    let ty = cross_y(size.height);
                    render_table(snapshot, table, x, ty, size.width, size.height);
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Column(nested) => {
                    // Resolve flex width for Fill children
                    if nested.width.is_flex() && total_flex > 0.0 {
                        width = (nested.width.flex() / total_flex) * available_flex;
                    }
                    // Resolve height
                    let h = match nested.height {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) => content_height,
                        Length::Shrink => nested.measure().height.min(content_height),
                    };
                    let y = cross_y(h);
                    let nested_bounds = Rect::new(x, y, width, h);
                    nested.layout(snapshot, nested_bounds);
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Row(nested) => {
                    // Resolve flex width for Fill children
                    if nested.width.is_flex() && total_flex > 0.0 {
                        width = (nested.width.flex() / total_flex) * available_flex;
                    }
                    // Resolve height
                    let h = match nested.height {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) => content_height,
                        Length::Shrink => nested.measure().height.min(content_height),
                    };
                    let y = cross_y(h);
                    let nested_bounds = Rect::new(x, y, width, h);
                    nested.layout(snapshot, nested_bounds);
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::ScrollColumn(nested) => {
                    // Resolve flex width for Fill children
                    if nested.width.is_flex() && total_flex > 0.0 {
                        width = (nested.width.flex() / total_flex) * available_flex;
                    }
                    // Resolve height
                    let h = match nested.height {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) => content_height,
                        Length::Shrink => nested.measure().height.min(content_height),
                    };
                    let y = cross_y(h);
                    let nested_bounds = Rect::new(x, y, width, h);
                    nested.layout(snapshot, nested_bounds);
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Spacer { flex } => {
                    if total_flex > 0.0 {
                        let space = (flex / total_flex) * available_flex;
                        x += space;
                    }
                    x += alignment_gap;
                }
                LayoutChild::FixedSpacer { size } => {
                    x += size + alignment_gap;
                }
            }
        }

        // Register widget ID for hit-testing and overlay anchoring
        if let Some(id) = self.id {
            snapshot.register_widget(id, bounds);
            if let Some(cursor) = self.cursor_hint {
                snapshot.set_cursor_hint(id, cursor);
            }
        }

        if clips {
            snapshot.primitives_mut().pop_clip();
        }
    }
}

// =========================================================================
// ScrollColumn
// =========================================================================

/// A virtualized vertical scroll container.
///
/// Scroll state lives in app state. The container receives the current scroll
/// offset as a parameter. Wheel events flow through `on_mouse` → message →
/// `update()` modifies offset.
///
/// The ID is required (for event routing and hit-testing the scroll area).
pub struct ScrollColumn {
    /// Widget ID (required for hit-testing and scroll event routing).
    id: SourceId,
    /// Scrollbar thumb widget ID (for drag interaction).
    thumb_id: SourceId,
    /// Child elements.
    children: Vec<LayoutChild>,
    /// Current scroll offset (from app state).
    scroll_offset: f32,
    /// Spacing between children.
    spacing: f32,
    /// Padding around all children.
    padding: Padding,
    /// Background color (optional).
    background: Option<Color>,
    /// Corner radius for background.
    corner_radius: f32,
    /// Width sizing mode.
    pub(crate) width: Length,
    /// Height sizing mode.
    pub(crate) height: Length,
    /// Border color (optional).
    border_color: Option<Color>,
    /// Border width.
    border_width: f32,
}

impl ScrollColumn {
    /// Create a new scroll column with a required ID.
    pub fn new(id: SourceId, thumb_id: SourceId) -> Self {
        Self {
            id,
            thumb_id,
            children: Vec::new(),
            scroll_offset: 0.0,
            spacing: 0.0,
            padding: Padding::default(),
            background: None,
            corner_radius: 0.0,
            width: Length::Shrink,
            height: Length::Shrink,
            border_color: None,
            border_width: 0.0,
        }
    }

    /// Create from a `ScrollState`, copying id, thumb_id, and offset.
    ///
    /// This pulls all state-driven fields so you only chain layout props.
    pub fn from_state(state: &ScrollState) -> Self {
        let mut sc = Self::new(state.id(), state.thumb_id());
        sc.scroll_offset = state.offset;
        sc
    }

    /// Set the scroll offset (from app state).
    pub fn scroll_offset(mut self, offset: f32) -> Self {
        self.scroll_offset = offset;
        self
    }

    /// Set spacing between children.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set padding (uniform on all sides).
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = Padding::all(padding);
        self
    }

    /// Set custom padding.
    pub fn padding_custom(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }

    /// Set background color.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Set corner radius for background.
    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Set width sizing mode.
    pub fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    /// Set height sizing mode.
    pub fn height(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    /// Set border (color + width).
    pub fn border(mut self, color: Color, width: f32) -> Self {
        self.border_color = Some(color);
        self.border_width = width;
        self
    }

    /// Add a text element.
    pub fn text(mut self, element: TextElement) -> Self {
        self.children.push(LayoutChild::Text(element));
        self
    }

    /// Add a terminal element.
    pub fn terminal(mut self, element: TerminalElement) -> Self {
        self.children.push(LayoutChild::Terminal(element));
        self
    }

    /// Add a nested column.
    pub fn column(mut self, column: Column) -> Self {
        self.children.push(LayoutChild::Column(column));
        self
    }

    /// Add a nested row.
    pub fn row(mut self, row: Row) -> Self {
        self.children.push(LayoutChild::Row(row));
        self
    }

    /// Add a flexible spacer.
    pub fn spacer(mut self, flex: f32) -> Self {
        self.children.push(LayoutChild::Spacer { flex });
        self
    }

    /// Add a fixed-size spacer.
    pub fn fixed_spacer(mut self, size: f32) -> Self {
        self.children.push(LayoutChild::FixedSpacer { size });
        self
    }

    /// Add an image element.
    pub fn image(mut self, element: ImageElement) -> Self {
        self.children.push(LayoutChild::Image(element));
        self
    }

    /// Add a button element.
    pub fn button(mut self, element: ButtonElement) -> Self {
        self.children.push(LayoutChild::Button(element));
        self
    }

    /// Add a text input element.
    pub fn text_input(mut self, element: TextInputElement) -> Self {
        self.children.push(LayoutChild::TextInput(element));
        self
    }

    /// Add a table element.
    pub fn table(mut self, element: TableElement) -> Self {
        self.children.push(LayoutChild::Table(element));
        self
    }

    /// Add any child element using `From<T> for LayoutChild`.
    #[inline(always)]
    pub fn push(mut self, child: impl Into<LayoutChild>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Compute intrinsic size (content size + padding).
    pub fn measure(&self) -> Size {
        let intrinsic_width = match self.width {
            Length::Fixed(px) => px,
            _ => {
                let mut max_child_width: f32 = 0.0;
                for child in &self.children {
                    max_child_width = max_child_width.max(child.measure_cross(true));
                }
                max_child_width + self.padding.horizontal()
            }
        };

        let intrinsic_height = match self.height {
            Length::Fixed(px) => px,
            _ => {
                let mut total_height: f32 = 0.0;
                for child in &self.children {
                    if child.flex_factor(true) > 0.0 {
                        continue;
                    }
                    total_height += child.measure_main(true);
                }
                if self.children.len() > 1 {
                    total_height += self.spacing * (self.children.len() - 1) as f32;
                }
                total_height + self.padding.vertical()
            }
        };

        Size::new(intrinsic_width, intrinsic_height)
    }

    /// Compute layout and flush to snapshot.
    ///
    /// Implements virtualization: only children intersecting the viewport
    /// are laid out. A scrollbar thumb is drawn when content overflows.
    pub fn layout(self, snapshot: &mut LayoutSnapshot, bounds: Rect) {
        let content_x = bounds.x + self.padding.left;
        let full_content_width = bounds.width - self.padding.horizontal();
        let viewport_h = bounds.height;

        // Draw chrome outside clip
        if let Some(bg) = self.background {
            if self.corner_radius > 0.0 {
                snapshot.primitives_mut().add_rounded_rect(bounds, self.corner_radius, bg);
            } else {
                snapshot.primitives_mut().add_solid_rect(bounds, bg);
            }
        }
        if let Some(border_color) = self.border_color {
            snapshot.primitives_mut().add_border(
                bounds,
                self.corner_radius,
                self.border_width,
                border_color,
            );
        }

        // Push clip to viewport bounds
        snapshot.primitives_mut().push_clip(bounds);

        // Measure all children to compute total content height
        let mut child_heights: Vec<f32> = Vec::with_capacity(self.children.len());
        let mut total_content_height = self.padding.vertical();
        for child in &self.children {
            let h = child.measure_main(true);
            child_heights.push(h);
            total_content_height += h;
        }
        if self.children.len() > 1 {
            total_content_height += self.spacing * (self.children.len() - 1) as f32;
        }

        // Reserve space for scrollbar when content overflows, so child content
        // doesn't extend into the scrollbar hit region (which would block clicks).
        const SCROLLBAR_GUTTER: f32 = 24.0;
        let overflows = total_content_height > viewport_h;
        let content_width = if overflows { full_content_width - SCROLLBAR_GUTTER } else { full_content_width };

        // Register container widget for hit-testing (wheel events route here).
        // When overflowing, exclude the gutter so this doesn't compete with the
        // scrollbar thumb track widget in the HashMap-based hit test.
        let container_hit_width = if overflows { bounds.width - SCROLLBAR_GUTTER } else { bounds.width };
        snapshot.register_widget(self.id, Rect::new(bounds.x, bounds.y, container_hit_width, bounds.height));

        // Clamp scroll offset and record max for app-side clamping
        let max_scroll = (total_content_height - viewport_h).max(0.0);
        snapshot.set_scroll_limit(self.id, max_scroll);
        let offset = self.scroll_offset.clamp(0.0, max_scroll);

        // Position pass with virtualization
        let mut virtual_y = self.padding.top; // position in content space
        let viewport_top = offset;
        let viewport_bottom = offset + viewport_h;

        for (i, child) in self.children.into_iter().enumerate() {
            let h = child_heights[i];
            let child_top = virtual_y;
            let child_bottom = virtual_y + h;

            // Check if child intersects the viewport
            if child_bottom > viewport_top && child_top < viewport_bottom {
                // Compute screen-space Y
                let screen_y = bounds.y + child_top - offset;

                match child {
                    LayoutChild::Text(t) => {
                        let fs = t.font_size();
                        use crate::layout_snapshot::{SourceLayout, TextLayout};
                        if let Some(source_id) = t.source_id {
                            let scale = fs / BASE_FONT_SIZE;
                            let text_layout = TextLayout::simple(
                                t.text.clone(),
                                t.color.pack(),
                                content_x, screen_y,
                                CHAR_WIDTH * scale, LINE_HEIGHT * scale,
                            );
                            snapshot.register_source(source_id, SourceLayout::text(text_layout));
                        }

                        snapshot.primitives_mut().add_text_cached(
                            t.text,
                            crate::primitives::Point::new(content_x, screen_y),
                            t.color,
                            fs,
                            t.cache_key,
                        );
                    }
                    LayoutChild::Terminal(t) => {
                        let size = t.size();

                        use crate::layout_snapshot::{GridLayout, GridRow, SourceLayout};
                        let rows_content: Vec<GridRow> = t.row_content.into_iter()
                            .map(|(text, color)| GridRow { text, color })
                            .collect();
                        let mut grid_layout = GridLayout::with_rows(
                            Rect::new(content_x, screen_y, size.width, size.height),
                            t.cell_width, t.cell_height,
                            t.cols, t.rows,
                            rows_content,
                        );
                        grid_layout.clip_rect = snapshot.current_clip();
                        snapshot.register_source(t.source_id, SourceLayout::grid(grid_layout));
                    }
                    LayoutChild::Image(img) => {
                        let img_rect = Rect::new(content_x, screen_y, img.width, img.height);
                        snapshot.primitives_mut().add_image(
                            img_rect,
                            img.handle,
                            img.corner_radius,
                            img.tint,
                        );
                        if let Some(id) = img.widget_id {
                            snapshot.register_widget(id, img_rect);
                        }
                    }
                    LayoutChild::Button(btn) => {
                        let size = btn.estimate_size();
                        let btn_rect = Rect::new(content_x, screen_y, size.width, size.height);
                        snapshot.primitives_mut().add_rounded_rect(btn_rect, btn.corner_radius, btn.background);
                        snapshot.primitives_mut().add_text_cached(
                            btn.label,
                            crate::primitives::Point::new(content_x + btn.padding.left, screen_y + btn.padding.top),
                            btn.text_color,
                            BASE_FONT_SIZE,
                            btn.cache_key,
                        );
                        snapshot.register_widget(btn.id, btn_rect);
                        snapshot.set_cursor_hint(btn.id, CursorIcon::Pointer);
                    }
                    LayoutChild::TextInput(input) => {
                        let w = match input.width {
                            Length::Fixed(px) => px,
                            Length::Fill | Length::FillPortion(_) => content_width,
                            Length::Shrink => input.estimate_size().width.min(content_width),
                        };
                        let h = if input.multiline {
                            input.estimate_size().height
                        } else {
                            LINE_HEIGHT + input.padding.vertical()
                        };
                        if input.multiline {
                            render_text_input_multiline(snapshot, input, content_x, screen_y, w, h);
                        } else {
                            render_text_input(snapshot, input, content_x, screen_y, w, h);
                        }
                    }
                    LayoutChild::Table(table) => {
                        let size = table.estimate_size();
                        let w = size.width.min(content_width);
                        render_table(snapshot, table, content_x, screen_y, w, size.height);
                    }
                    LayoutChild::Column(nested) => {
                        let w = match nested.width {
                            Length::Fixed(px) => px,
                            Length::Fill | Length::FillPortion(_) => content_width,
                            Length::Shrink => nested.measure().width.min(content_width),
                        };
                        nested.layout(snapshot, Rect::new(content_x, screen_y, w, h));
                    }
                    LayoutChild::Row(nested) => {
                        let w = match nested.width {
                            Length::Fixed(px) => px,
                            Length::Fill | Length::FillPortion(_) => content_width,
                            Length::Shrink => nested.measure().width.min(content_width),
                        };
                        nested.layout(snapshot, Rect::new(content_x, screen_y, w, h));
                    }
                    LayoutChild::ScrollColumn(nested) => {
                        let w = match nested.width {
                            Length::Fixed(px) => px,
                            Length::Fill | Length::FillPortion(_) => content_width,
                            Length::Shrink => nested.measure().width.min(content_width),
                        };
                        nested.layout(snapshot, Rect::new(content_x, screen_y, w, h));
                    }
                    LayoutChild::Spacer { .. } | LayoutChild::FixedSpacer { .. } => {
                        // Spacers have no visual representation
                    }
                }
            }

            virtual_y += h + self.spacing;
        }

        // Draw scrollbar thumb if content overflows
        if total_content_height > viewport_h {
            let thumb_h = ((viewport_h / total_content_height) * viewport_h).max(20.0);
            let scroll_pct = if max_scroll > 0.0 { offset / max_scroll } else { 0.0 };
            let scroll_available = viewport_h - thumb_h;
            let thumb_y = bounds.y + scroll_pct * scroll_available;
            let thumb_visual = Rect::new(bounds.x + bounds.width - 8.0, thumb_y, 6.0, thumb_h);

            snapshot.primitives_mut().add_rounded_rect(
                thumb_visual,
                3.0,
                Color::rgba(1.0, 1.0, 1.0, 0.25),
            );

            // Register the full-height track as the hit region so clicking
            // anywhere in the scrollbar gutter initiates a drag.
            let track_hit = Rect::new(bounds.x + bounds.width - SCROLLBAR_GUTTER, bounds.y, SCROLLBAR_GUTTER, viewport_h);
            snapshot.register_widget(self.thumb_id, track_hit);
            snapshot.set_cursor_hint(self.thumb_id, CursorIcon::Grab);

            // Store track info so the app can convert mouse Y → scroll offset
            use crate::layout_snapshot::ScrollTrackInfo;
            snapshot.set_scroll_track(self.id, ScrollTrackInfo {
                track_y: bounds.y,
                track_height: viewport_h,
                thumb_height: thumb_h,
                max_scroll,
            });
        }

        // Pop clip
        snapshot.primitives_mut().pop_clip();
    }
}
