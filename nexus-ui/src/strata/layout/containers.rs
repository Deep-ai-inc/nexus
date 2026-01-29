//! Layout Containers
//!
//! Flexbox-inspired layout containers that compute child positions.
//! The layout computation happens ONCE when `layout()` is called,
//! not during widget construction.

use crate::strata::content_address::SourceId;
use crate::strata::layout_snapshot::LayoutSnapshot;
use crate::strata::primitives::{Color, Rect, Size};

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

    /// A spacer that expands to fill available space.
    Spacer { flex: f32 },

    /// A fixed-size spacer.
    FixedSpacer { size: f32 },
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

/// A vertical layout container (children flow top to bottom).
pub struct Column {
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
            children: Vec::new(),
            spacing: 0.0,
            padding: Padding::default(),
            alignment: Alignment::Start,
            cross_alignment: CrossAxisAlignment::Start,
            background: None,
            corner_radius: 0.0,
        }
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

    /// Compute layout and flush to snapshot.
    ///
    /// This is where the actual layout math happens - ONCE per frame.
    pub fn layout(self, snapshot: &mut LayoutSnapshot, bounds: Rect) {
        // Layout metrics (would come from font system in production)
        let char_width = 8.4;
        let line_height = 18.0;

        // Available space after padding
        let content_x = bounds.x + self.padding.left;
        let content_y = bounds.y + self.padding.top;
        let content_width = bounds.width - self.padding.horizontal();
        let _content_height = bounds.height - self.padding.vertical();

        // Draw background if set
        if let Some(bg) = self.background {
            if self.corner_radius > 0.0 {
                snapshot.add_rounded_rect(bounds, self.corner_radius, bg);
            } else {
                snapshot.add_solid_rect(bounds, bg);
            }
        }

        // Calculate total fixed height and flex factor
        let mut total_fixed_height = 0.0;
        let mut total_flex = 0.0;
        let mut child_heights: Vec<f32> = Vec::with_capacity(self.children.len());

        for child in &self.children {
            match child {
                LayoutChild::Text(t) => {
                    let size = t.estimate_size(char_width, line_height);
                    child_heights.push(size.height);
                    total_fixed_height += size.height;
                }
                LayoutChild::Terminal(t) => {
                    let size = t.size();
                    child_heights.push(size.height);
                    total_fixed_height += size.height;
                }
                LayoutChild::Column(_) | LayoutChild::Row(_) => {
                    // Nested containers need recursive measurement
                    // For now, use a default estimate
                    child_heights.push(100.0);
                    total_fixed_height += 100.0;
                }
                LayoutChild::Spacer { flex } => {
                    child_heights.push(0.0); // Will be computed
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

        // Position children
        let mut y = content_y;

        for (i, child) in self.children.into_iter().enumerate() {
            let height = child_heights[i];

            match child {
                LayoutChild::Text(t) => {
                    let size = t.estimate_size(char_width, line_height);
                    let x = match self.cross_alignment {
                        CrossAxisAlignment::Start => content_x,
                        CrossAxisAlignment::End => content_x + content_width - size.width,
                        CrossAxisAlignment::Center => content_x + (content_width - size.width) / 2.0,
                        CrossAxisAlignment::Stretch => content_x,
                    };

                    // Register source for hit-testing if source_id is provided
                    use crate::strata::layout_snapshot::{SourceLayout, TextLayout};
                    if let Some(source_id) = t.source_id {
                        let text_layout = TextLayout::simple(
                            t.text.clone(),
                            t.color.pack(),
                            x,
                            y,
                            char_width,
                            line_height,
                        );
                        snapshot.register_source(source_id, SourceLayout::text(text_layout));
                    }

                    // Always add to primitives for rendering
                    // (sources are for hit-testing only, primitives are for rendering)
                    snapshot.primitives_mut().add_text_cached(
                        t.text,
                        crate::strata::primitives::Point::new(x, y),
                        t.color,
                        t.cache_key,
                    );

                    y += height + self.spacing;
                }
                LayoutChild::Terminal(t) => {
                    let size = t.size();
                    let x = match self.cross_alignment {
                        CrossAxisAlignment::Start => content_x,
                        CrossAxisAlignment::End => content_x + content_width - size.width,
                        CrossAxisAlignment::Center => content_x + (content_width - size.width) / 2.0,
                        CrossAxisAlignment::Stretch => content_x,
                    };

                    // Register grid with snapshot for hit-testing and rendering
                    use crate::strata::layout_snapshot::{GridLayout, GridRow, SourceLayout};
                    let rows_content: Vec<GridRow> = t.row_content.into_iter()
                        .map(|(text, color)| GridRow { text, color })
                        .collect();
                    let grid_layout = GridLayout::with_rows(
                        Rect::new(x, y, size.width, size.height),
                        t.cell_width,
                        t.cell_height,
                        t.cols,
                        t.rows,
                        rows_content,
                    );
                    snapshot.register_source(t.source_id, SourceLayout::grid(grid_layout));

                    y += height + self.spacing;
                }
                LayoutChild::Column(nested) => {
                    let nested_bounds = Rect::new(content_x, y, content_width, height);
                    nested.layout(snapshot, nested_bounds);
                    y += height + self.spacing;
                }
                LayoutChild::Row(nested) => {
                    let nested_bounds = Rect::new(content_x, y, content_width, height);
                    nested.layout(snapshot, nested_bounds);
                    y += height + self.spacing;
                }
                LayoutChild::Spacer { flex } => {
                    // Calculate flex space
                    if total_flex > 0.0 {
                        let available = bounds.height
                            - self.padding.vertical()
                            - total_fixed_height;
                        let space = (flex / total_flex) * available.max(0.0);
                        y += space;
                    }
                }
                LayoutChild::FixedSpacer { size } => {
                    y += size;
                }
            }
        }
    }
}

/// A horizontal layout container (children flow left to right).
pub struct Row {
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
            children: Vec::new(),
            spacing: 0.0,
            padding: Padding::default(),
            alignment: Alignment::Start,
            cross_alignment: CrossAxisAlignment::Start,
            background: None,
            corner_radius: 0.0,
        }
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

    /// Compute layout and flush to snapshot.
    pub fn layout(self, snapshot: &mut LayoutSnapshot, bounds: Rect) {
        // Layout metrics
        let char_width = 8.4;
        let line_height = 18.0;

        // Available space after padding
        let content_x = bounds.x + self.padding.left;
        let content_y = bounds.y + self.padding.top;
        let _content_width = bounds.width - self.padding.horizontal();
        let content_height = bounds.height - self.padding.vertical();

        // Draw background if set
        if let Some(bg) = self.background {
            if self.corner_radius > 0.0 {
                snapshot.add_rounded_rect(bounds, self.corner_radius, bg);
            } else {
                snapshot.add_solid_rect(bounds, bg);
            }
        }

        // Calculate total fixed width and flex factor
        let mut total_fixed_width = 0.0;
        let mut total_flex = 0.0;
        let mut child_widths: Vec<f32> = Vec::with_capacity(self.children.len());

        for child in &self.children {
            match child {
                LayoutChild::Text(t) => {
                    let size = t.estimate_size(char_width, line_height);
                    child_widths.push(size.width);
                    total_fixed_width += size.width;
                }
                LayoutChild::Terminal(t) => {
                    let size = t.size();
                    child_widths.push(size.width);
                    total_fixed_width += size.width;
                }
                LayoutChild::Column(_) | LayoutChild::Row(_) => {
                    child_widths.push(100.0);
                    total_fixed_width += 100.0;
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

        // Position children
        let mut x = content_x;

        for (i, child) in self.children.into_iter().enumerate() {
            let width = child_widths[i];

            match child {
                LayoutChild::Text(t) => {
                    let size = t.estimate_size(char_width, line_height);
                    let y = match self.cross_alignment {
                        CrossAxisAlignment::Start => content_y,
                        CrossAxisAlignment::End => content_y + content_height - size.height,
                        CrossAxisAlignment::Center => {
                            content_y + (content_height - size.height) / 2.0
                        }
                        CrossAxisAlignment::Stretch => content_y,
                    };

                    // Register source for hit-testing if source_id is provided
                    use crate::strata::layout_snapshot::{SourceLayout, TextLayout};
                    if let Some(source_id) = t.source_id {
                        let text_layout = TextLayout::simple(
                            t.text.clone(),
                            t.color.pack(),
                            x,
                            y,
                            char_width,
                            line_height,
                        );
                        snapshot.register_source(source_id, SourceLayout::text(text_layout));
                    }

                    // Always add to primitives for rendering
                    snapshot.primitives_mut().add_text_cached(
                        t.text,
                        crate::strata::primitives::Point::new(x, y),
                        t.color,
                        t.cache_key,
                    );

                    x += width + self.spacing;
                }
                LayoutChild::Terminal(t) => {
                    let size = t.size();
                    let y = match self.cross_alignment {
                        CrossAxisAlignment::Start => content_y,
                        CrossAxisAlignment::End => content_y + content_height - size.height,
                        CrossAxisAlignment::Center => {
                            content_y + (content_height - size.height) / 2.0
                        }
                        CrossAxisAlignment::Stretch => content_y,
                    };

                    use crate::strata::layout_snapshot::{GridLayout, SourceLayout};
                    let grid_layout = GridLayout::new(
                        Rect::new(x, y, size.width, size.height),
                        t.cell_width,
                        t.cell_height,
                        t.cols,
                        t.rows,
                    );
                    snapshot.register_source(t.source_id, SourceLayout::grid(grid_layout));

                    x += width + self.spacing;
                }
                LayoutChild::Column(nested) => {
                    let nested_bounds = Rect::new(x, content_y, width, content_height);
                    nested.layout(snapshot, nested_bounds);
                    x += width + self.spacing;
                }
                LayoutChild::Row(nested) => {
                    let nested_bounds = Rect::new(x, content_y, width, content_height);
                    nested.layout(snapshot, nested_bounds);
                    x += width + self.spacing;
                }
                LayoutChild::Spacer { flex } => {
                    if total_flex > 0.0 {
                        let available =
                            bounds.width - self.padding.horizontal() - total_fixed_width;
                        let space = (flex / total_flex) * available.max(0.0);
                        x += space;
                    }
                }
                LayoutChild::FixedSpacer { size } => {
                    x += size;
                }
            }
        }
    }
}
