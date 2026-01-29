//! Layout Containers
//!
//! Flexbox-inspired layout containers that compute child positions.
//! The layout computation happens ONCE when `layout()` is called,
//! not during widget construction.

use crate::strata::content_address::SourceId;
use crate::strata::layout_snapshot::LayoutSnapshot;
use crate::strata::primitives::{Color, Rect, Size};

// Layout metrics (centralized; will come from font system in production)
const CHAR_WIDTH: f32 = 8.4;
const LINE_HEIGHT: f32 = 18.0;

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

    /// A nested column.
    Column(Column),

    /// A nested row.
    Row(Row),

    /// A scroll column (virtualized vertical scroll container).
    ScrollColumn(ScrollColumn),

    /// A spacer that expands to fill available space.
    Spacer { flex: f32 },

    /// A fixed-size spacer.
    FixedSpacer { size: f32 },
}

impl LayoutChild {
    /// Measure this child's main axis size (height for Column parent, width for Row parent).
    fn measure_main(&self, is_column: bool) -> f32 {
        let size = match self {
            LayoutChild::Text(t) => t.estimate_size(CHAR_WIDTH, LINE_HEIGHT),
            LayoutChild::Terminal(t) => t.size(),
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
            _ => 0.0,
        }
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
    fn estimate_size(&self, default_char_width: f32, default_line_height: f32) -> Size {
        if let Some(size) = self.measured_size {
            return size;
        }
        // Simple estimate: char_count * char_width
        let char_count = self.text.chars().count() as f32;
        Size::new(char_count * default_char_width, default_line_height)
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
                    let size = t.estimate_size(CHAR_WIDTH, LINE_HEIGHT);
                    let x = cross_x(size.width);

                    use crate::strata::layout_snapshot::{SourceLayout, TextLayout};
                    if let Some(source_id) = t.source_id {
                        let text_layout = TextLayout::simple(
                            t.text.clone(),
                            t.color.pack(),
                            x, y,
                            CHAR_WIDTH, LINE_HEIGHT,
                        );
                        snapshot.register_source(source_id, SourceLayout::text(text_layout));
                    }

                    snapshot.primitives_mut().add_text_cached(
                        t.text,
                        crate::strata::primitives::Point::new(x, y),
                        t.color,
                        t.cache_key,
                    );

                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::Terminal(t) => {
                    let size = t.size();
                    let x = cross_x(size.width);

                    use crate::strata::layout_snapshot::{GridLayout, GridRow, SourceLayout};
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
                    let size = t.estimate_size(CHAR_WIDTH, LINE_HEIGHT);
                    let y = cross_y(size.height);

                    use crate::strata::layout_snapshot::{SourceLayout, TextLayout};
                    if let Some(source_id) = t.source_id {
                        let text_layout = TextLayout::simple(
                            t.text.clone(),
                            t.color.pack(),
                            x, y,
                            CHAR_WIDTH, LINE_HEIGHT,
                        );
                        snapshot.register_source(source_id, SourceLayout::text(text_layout));
                    }

                    snapshot.primitives_mut().add_text_cached(
                        t.text,
                        crate::strata::primitives::Point::new(x, y),
                        t.color,
                        t.cache_key,
                    );

                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Terminal(t) => {
                    let size = t.size();
                    let y = cross_y(size.height);

                    use crate::strata::layout_snapshot::{GridLayout, GridRow, SourceLayout};
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
                        use crate::strata::layout_snapshot::{SourceLayout, TextLayout};
                        if let Some(source_id) = t.source_id {
                            let text_layout = TextLayout::simple(
                                t.text.clone(),
                                t.color.pack(),
                                content_x, screen_y,
                                CHAR_WIDTH, LINE_HEIGHT,
                            );
                            snapshot.register_source(source_id, SourceLayout::text(text_layout));
                        }

                        snapshot.primitives_mut().add_text_cached(
                            t.text,
                            crate::strata::primitives::Point::new(content_x, screen_y),
                            t.color,
                            t.cache_key,
                        );
                    }
                    LayoutChild::Terminal(t) => {
                        let size = t.size();

                        use crate::strata::layout_snapshot::{GridLayout, GridRow, SourceLayout};
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

            // Store track info so the app can convert mouse Y → scroll offset
            use crate::strata::layout_snapshot::ScrollTrackInfo;
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
