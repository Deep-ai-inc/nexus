//! Row - Horizontal layout container.
//!
//! Children flow left to right. Supports flex sizing, spacing, padding,
//! and alignment on both axes.

use crate::content_address::SourceId;
use crate::layout_snapshot::{CursorIcon, LayoutSnapshot};
use crate::primitives::{Color, Point, Rect, Size};

use super::child::LayoutChild;
use super::column::Column;
use super::constraints::LayoutConstraints;
use super::context::LayoutContext;
use super::text_input::{TextInputElement, render_text_input, render_text_input_multiline};
use super::table::{TableElement, VirtualTableElement, render_table, render_virtual_table};
use super::elements::{TextElement, TerminalElement, ImageElement, ButtonElement};
use super::length::{Length, Padding, Alignment, CrossAxisAlignment, CHAR_WIDTH, LINE_HEIGHT, BASE_FONT_SIZE};
use super::scroll_column::ScrollColumn;

// =========================================================================
// Helper Functions
// =========================================================================

/// Hash a Length value for cache keys.
#[inline]
fn hash_length(len: &Length) -> u64 {
    match len {
        Length::Shrink => 0,
        Length::Fill => 1,
        Length::FillPortion(n) => 2u64.wrapping_add(*n as u64),
        Length::Fixed(f) => 3u64.wrapping_add(f.to_bits() as u64),
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
        self.children.push(LayoutChild::Column(Box::new(column)));
        self
    }

    /// Add a nested row.
    pub fn row(mut self, row: Row) -> Self {
        self.children.push(LayoutChild::Row(Box::new(row)));
        self
    }

    /// Add a scroll column.
    pub fn scroll_column(mut self, scroll: ScrollColumn) -> Self {
        self.children.push(LayoutChild::ScrollColumn(Box::new(scroll)));
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

    pub fn virtual_table(mut self, element: VirtualTableElement) -> Self {
        self.children.push(LayoutChild::VirtualTable(element));
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

    /// Compute a content hash for cache key generation.
    ///
    /// This hash captures all properties that affect layout size:
    /// - Width/height Length values
    /// - Spacing and padding
    /// - Number of children and their content hashes (recursive)
    ///
    /// Note: background, border, shadow are NOT included since
    /// they don't affect the measured size.
    pub fn content_hash(&self) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis

        // Mix in width/height Length settings
        hash = hash.wrapping_mul(0x100000001b3);
        hash ^= hash_length(&self.width);

        hash = hash.wrapping_mul(0x100000001b3);
        hash ^= hash_length(&self.height);

        // Mix in spacing and padding
        hash = hash.wrapping_mul(0x100000001b3);
        hash ^= self.spacing.to_bits() as u64;

        hash = hash.wrapping_mul(0x100000001b3);
        hash ^= self.padding.horizontal().to_bits() as u64;

        hash = hash.wrapping_mul(0x100000001b3);
        hash ^= self.padding.vertical().to_bits() as u64;

        // Mix in child count
        hash = hash.wrapping_mul(0x100000001b3);
        hash ^= self.children.len() as u64;

        // Mix in each child's content hash (recursive)
        for child in &self.children {
            hash = hash.wrapping_mul(0x100000001b3);
            hash ^= child.content_hash();
        }

        hash
    }

    /// Calculate the height of this Row for a given available width.
    /// This is needed because FlowContainer children have width-dependent heights.
    pub fn height_for_width(&self, available_width: f32) -> f32 {
        if let Length::Fixed(px) = self.height {
            return px;
        }

        // Calculate fixed widths and flex factor (mirrors measurement pass logic)
        let mut total_fixed_width = 0.0f32;
        let mut total_flex = 0.0f32;

        for child in &self.children {
            match child {
                LayoutChild::Flow(flow) => {
                    match flow.width {
                        Length::Fill | Length::FillPortion(_) => {
                            total_flex += flow.width.flex();
                        }
                        Length::Fixed(px) => total_fixed_width += px,
                        Length::Shrink => total_fixed_width += flow.measure().width,
                    }
                }
                LayoutChild::Column(c) => {
                    match c.width {
                        Length::Fill | Length::FillPortion(_) => total_flex += c.width.flex(),
                        Length::Fixed(px) => total_fixed_width += px,
                        Length::Shrink => total_fixed_width += c.measure().width,
                    }
                }
                LayoutChild::Row(r) => {
                    match r.width {
                        Length::Fill | Length::FillPortion(_) => total_flex += r.width.flex(),
                        Length::Fixed(px) => total_fixed_width += px,
                        Length::Shrink => total_fixed_width += r.measure().width,
                    }
                }
                LayoutChild::Spacer { flex } => total_flex += flex,
                LayoutChild::FixedSpacer { size } => total_fixed_width += size,
                _ => total_fixed_width += child.measure_main(false),
            }
        }

        // Add spacing
        if self.children.len() > 1 {
            total_fixed_width += self.spacing * (self.children.len() - 1) as f32;
        }

        let content_width = available_width - self.padding.horizontal();
        let available_flex = (content_width - total_fixed_width).max(0.0);

        // Calculate max child height, using height_for_width for width-dependent containers
        let mut max_height = 0.0f32;
        for child in &self.children {
            let h = match child {
                LayoutChild::Flow(flow) => {
                    let flow_width = if flow.width.is_flex() && total_flex > 0.0 {
                        (flow.width.flex() / total_flex) * available_flex
                    } else {
                        match flow.width {
                            Length::Fixed(px) => px,
                            Length::Shrink => flow.measure().width,
                            _ => available_flex,
                        }
                    };
                    flow.height_for_width(flow_width)
                }
                LayoutChild::Column(col) => {
                    let col_width = if col.width.is_flex() && total_flex > 0.0 {
                        (col.width.flex() / total_flex) * available_flex
                    } else {
                        match col.width {
                            Length::Fixed(px) => px,
                            Length::Shrink => col.measure().width,
                            _ => content_width, // Fill takes remaining width
                        }
                    };
                    col.height_for_width(col_width)
                }
                LayoutChild::Row(row) => {
                    let row_width = if row.width.is_flex() && total_flex > 0.0 {
                        (row.width.flex() / total_flex) * available_flex
                    } else {
                        match row.width {
                            Length::Fixed(px) => px,
                            Length::Shrink => row.measure().width,
                            _ => content_width,
                        }
                    };
                    row.height_for_width(row_width)
                }
                _ => child.measure_cross(false),
            };
            max_height = max_height.max(h);
        }

        max_height + self.padding.vertical()
    }

    /// Compute layout and flush to snapshot.
    pub fn layout(self, snapshot: &mut LayoutSnapshot, bounds: Rect) {
        // Debug tracking for layout visualization
        snapshot.debug_enter("Row", bounds);

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
                LayoutChild::VirtualTable(table) => {
                    let w = table.estimate_size().width;
                    child_widths.push(w);
                    total_fixed_width += w;
                }
                LayoutChild::Flow(flow) => {
                    match flow.width {
                        Length::Fill | Length::FillPortion(_) => {
                            child_widths.push(0.0);
                            total_flex += flow.width.flex();
                        }
                        Length::Fixed(px) => {
                            child_widths.push(px);
                            total_fixed_width += px;
                        }
                        Length::Shrink => {
                            let w = flow.measure().width;
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

        let available_flex = (bounds.width - self.padding.horizontal() - total_fixed_width).max(0.0);

        // Recalculate max_child_cross for width-dependent children now that we know their widths.
        // FlowContainer/Column/Row heights can depend on available width for wrapping.
        for (i, child) in self.children.iter().enumerate() {
            let child_height = match child {
                LayoutChild::Flow(flow) => {
                    let flow_width = if flow.width.is_flex() && total_flex > 0.0 {
                        (flow.width.flex() / total_flex) * available_flex
                    } else {
                        child_widths[i]
                    };
                    Some(flow.height_for_width(flow_width))
                }
                LayoutChild::Column(col) => {
                    let col_width = if col.width.is_flex() && total_flex > 0.0 {
                        (col.width.flex() / total_flex) * available_flex
                    } else {
                        child_widths[i]
                    };
                    Some(col.height_for_width(col_width))
                }
                LayoutChild::Row(row) => {
                    let row_width = if row.width.is_flex() && total_flex > 0.0 {
                        (row.width.flex() / total_flex) * available_flex
                    } else {
                        child_widths[i]
                    };
                    Some(row.height_for_width(row_width))
                }
                _ => None,
            };
            if let Some(h) = child_height {
                max_child_cross = max_child_cross.max(h);
            }
        }

        // Overflow detection (replaces previous self.measure() call)
        let content_w = total_fixed_width + self.padding.horizontal();
        let content_h = max_child_cross + self.padding.vertical();
        let content_overflows = bounds.width < content_w || bounds.height < content_h;
        let clips = has_chrome || content_overflows;
        if clips {
            snapshot.primitives_mut().push_clip(bounds);
        }

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

            // Right edge of Row content area, for expanding hit-boxes.
            let content_right = content_x + bounds.width - self.padding.horizontal();

            match child {
                LayoutChild::Text(t) => {
                    let fs = t.font_size();
                    let size = t.estimate_size(CHAR_WIDTH, LINE_HEIGHT);
                    let y = cross_y(size.height);

                    use crate::layout_snapshot::{SourceLayout, TextLayout};
                    if let Some(source_id) = t.source_id {
                        let scale = fs / BASE_FONT_SIZE;
                        let mut text_layout = TextLayout::simple(
                            t.text.clone(),
                            t.color.pack(),
                            x, y,
                            CHAR_WIDTH * scale, LINE_HEIGHT * scale,
                        );
                        // Expand hit-box to fill remaining Row width so empty
                        // space to the right of text is clickable.
                        text_layout.bounds.width = text_layout.bounds.width.max(content_right - x);
                        snapshot.register_source(source_id, SourceLayout::text(text_layout));
                    }

                    // Register widget if this text is clickable
                    if let Some(widget_id) = t.widget_id {
                        let text_rect = Rect::new(x, y, size.width, size.height);
                        snapshot.register_widget(widget_id, text_rect);
                        if let Some(cursor) = t.cursor_hint {
                            snapshot.set_cursor_hint(widget_id, cursor);
                        }
                    }

                    snapshot.primitives_mut().add_text_cached_styled(
                        t.text,
                        crate::primitives::Point::new(x, y),
                        t.color,
                        fs,
                        t.cache_key,
                        t.bold,
                        t.italic,
                    );

                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Terminal(t) => {
                    let size = t.size();
                    let y = cross_y(size.height);

                    use crate::layout_snapshot::{GridLayout, GridRow, SourceLayout};
                    let rows_content: Vec<GridRow> = t.row_content.into_iter()
                        .map(|runs| GridRow { runs })
                        .collect();
                    let mut grid_layout = GridLayout::with_rows(
                        Rect::new(x, y, size.width.max(content_right - x), size.height),
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
                LayoutChild::VirtualTable(table) => {
                    let size = table.estimate_size();
                    let ty = cross_y(size.height);
                    render_virtual_table(snapshot, table, x, ty, size.width, size.height);
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Flow(flow) => {
                    // Resolve flex width for Fill children
                    if flow.width.is_flex() && total_flex > 0.0 {
                        width = (flow.width.flex() / total_flex) * available_flex;
                    }
                    let h = flow.height_for_width(width);
                    let fy = cross_y(h);
                    flow.layout(snapshot, x, fy, width);
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

        snapshot.debug_exit();
    }

    // =========================================================================
    // Constraint-based Layout API (Phase 4)
    // =========================================================================

    /// Layout with constraints - the new constraint-based API.
    ///
    /// Takes constraints (min/max bounds) and returns the actual size used.
    /// Row's main axis is width (horizontal), cross axis is height.
    ///
    /// # Arguments
    /// * `ctx` - Layout context with scratch buffers and snapshot
    /// * `constraints` - Min/max bounds for this row
    /// * `origin` - Top-left position to place this row
    ///
    /// # Returns
    /// The actual size consumed by this row.
    pub fn layout_with_constraints(
        self,
        ctx: &mut LayoutContext,
        constraints: LayoutConstraints,
        origin: Point,
    ) -> Size {
        ctx.enter("Row");

        // Determine our bounds from constraints
        let width = match self.width {
            Length::Fixed(px) => px,
            Length::Fill | Length::FillPortion(_) => {
                if constraints.has_bounded_width() {
                    constraints.max_width
                } else {
                    self.measure().width
                }
            }
            Length::Shrink => {
                let intrinsic = self.measure().width;
                if constraints.has_bounded_width() {
                    intrinsic.min(constraints.max_width)
                } else {
                    intrinsic
                }
            }
        };

        let height = match self.height {
            Length::Fixed(px) => px,
            Length::Fill | Length::FillPortion(_) => {
                if constraints.has_bounded_height() {
                    constraints.max_height
                } else {
                    self.height_for_width(width)
                }
            }
            Length::Shrink => {
                let intrinsic = self.height_for_width(width);
                if constraints.has_bounded_height() {
                    intrinsic.min(constraints.max_height)
                } else {
                    intrinsic
                }
            }
        };

        let size = constraints.constrain(Size::new(width, height));
        ctx.log_layout(constraints, size);

        // Perform actual layout using legacy method
        // (debug rects are pushed inside layout() via snapshot.debug_enter())
        let bounds = Rect::new(origin.x, origin.y, size.width, size.height);
        self.layout(ctx.snapshot, bounds);

        ctx.exit();
        size
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::elements::TextElement;

    #[test]
    fn test_row_new() {
        let row = Row::new();
        assert_eq!(row.spacing, 0.0);
        assert_eq!(row.padding.horizontal(), 0.0);
        assert_eq!(row.padding.vertical(), 0.0);
        assert!(row.background.is_none());
    }

    #[test]
    fn test_row_builder_pattern() {
        let row = Row::new()
            .spacing(10.0)
            .padding(5.0)
            .background(Color::WHITE)
            .corner_radius(4.0)
            .width(Length::Fill)
            .height(Length::Fixed(50.0));

        assert_eq!(row.spacing, 10.0);
        assert_eq!(row.padding.horizontal(), 10.0);
        assert!(row.background.is_some());
        assert_eq!(row.corner_radius, 4.0);
        assert!(row.width.is_flex());
        assert!(!row.height.is_flex());
    }

    #[test]
    fn test_row_measure_empty() {
        let row = Row::new();
        let size = row.measure();
        assert_eq!(size.width, 0.0);
        assert_eq!(size.height, 0.0);
    }

    #[test]
    fn test_row_measure_with_children() {
        let row = Row::new()
            .spacing(5.0)
            .push(TextElement::new("Hello"))
            .push(TextElement::new("World"));

        let size = row.measure();
        // Width should be sum of children + spacing
        assert!(size.width > 0.0);
        // Height should be max child height
        assert!(size.height > 0.0);
    }

    #[test]
    fn test_row_measure_with_padding() {
        let row = Row::new()
            .padding(10.0)
            .push(TextElement::new("Test"));

        let size = row.measure();
        // Should include padding
        assert!(size.width >= 20.0); // At least horizontal padding
        assert!(size.height >= 20.0); // At least vertical padding
    }

    #[test]
    fn test_row_fixed_spacer() {
        let row = Row::new()
            .fixed_spacer(50.0);

        let size = row.measure();
        assert_eq!(size.width, 50.0);
    }

    #[test]
    fn test_row_default() {
        let row = Row::default();
        assert_eq!(row.spacing, 0.0);
    }

    #[test]
    fn test_row_custom_padding() {
        let row = Row::new()
            .padding_custom(Padding::new(1.0, 2.0, 3.0, 4.0));

        assert_eq!(row.padding.top, 1.0);
        assert_eq!(row.padding.right, 2.0);
        assert_eq!(row.padding.bottom, 3.0);
        assert_eq!(row.padding.left, 4.0);
    }

    #[test]
    fn test_row_border() {
        let row = Row::new()
            .border(Color::RED, 2.0);

        assert!(row.border_color.is_some());
        assert_eq!(row.border_width, 2.0);
    }

    #[test]
    fn test_row_shadow() {
        let row = Row::new()
            .shadow(8.0, Color::BLACK);

        assert!(row.shadow.is_some());
        let (blur, _color) = row.shadow.unwrap();
        assert_eq!(blur, 8.0);
    }

    #[test]
    fn test_row_alignment() {
        let row = Row::new()
            .align(Alignment::Center)
            .cross_align(CrossAxisAlignment::Center);

        assert_eq!(row.alignment, Alignment::Center);
        assert_eq!(row.cross_alignment, CrossAxisAlignment::Center);
    }

    #[test]
    fn test_row_spacer() {
        let row = Row::new()
            .push(TextElement::new("Left"))
            .spacer(1.0)
            .push(TextElement::new("Right"));

        // Should have 3 children: text, spacer, text
        let size = row.measure();
        assert!(size.width > 0.0);
    }

    #[test]
    fn test_row_layout_with_constraints() {
        use crate::layout_snapshot::LayoutSnapshot;
        use crate::layout::context::LayoutContext;
        use crate::layout::constraints::LayoutConstraints;
        use crate::primitives::Point;

        let row = Row::new()
            .spacing(5.0)
            .push(TextElement::new("Hello"))
            .push(TextElement::new("World"));

        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        let constraints = LayoutConstraints::loose(500.0, 100.0);
        let size = row.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);

        assert!(size.width > 0.0);
        assert!(size.width <= 500.0);
        assert!(size.height > 0.0);
        assert!(size.height <= 100.0);
    }

    #[test]
    fn test_row_layout_with_constraints_fill() {
        use crate::layout_snapshot::LayoutSnapshot;
        use crate::layout::context::LayoutContext;
        use crate::layout::constraints::LayoutConstraints;
        use crate::primitives::Point;

        let row = Row::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(TextElement::new("Hello"));

        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        let constraints = LayoutConstraints::tight(300.0, 50.0);
        let size = row.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);

        // Fill should take all available space
        assert_eq!(size.width, 300.0);
        assert_eq!(size.height, 50.0);
    }
}
