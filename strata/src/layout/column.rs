//! Column - Vertical layout container.
//!
//! Children flow top to bottom. Supports flex sizing, spacing, padding,
//! and alignment on both axes.

use crate::content_address::SourceId;
use crate::layout_snapshot::{CursorIcon, LayoutSnapshot};
use crate::primitives::{Color, Point, Rect, Size};

use super::child::LayoutChild;
use super::constraints::LayoutConstraints;
use super::context::LayoutContext;
use super::text_input::{TextInputElement, render_text_input, render_text_input_multiline};
use super::table::{TableElement, VirtualTableElement, render_table, render_virtual_table};
use super::elements::{TextElement, TerminalElement, ImageElement, ButtonElement};
use super::length::{Length, Padding, Alignment, CrossAxisAlignment, CHAR_WIDTH, LINE_HEIGHT, BASE_FONT_SIZE};
use super::row::Row;
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

    pub fn table(mut self, element: TableElement) -> Self {
        self.children.push(LayoutChild::Table(element));
        self
    }

    pub fn virtual_table(mut self, element: VirtualTableElement) -> Self {
        self.children.push(LayoutChild::VirtualTable(element));
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

    /// Calculate the height of this Column for a given available width.
    /// This is needed because FlowContainer and Row children have width-dependent heights.
    pub fn height_for_width(&self, available_width: f32) -> f32 {
        if let Length::Fixed(px) = self.height {
            return px;
        }

        let content_width = available_width - self.padding.horizontal();
        let mut total_height = 0.0f32;

        for child in &self.children {
            // Skip flex children in measurement (they fill remaining space)
            if child.flex_factor(true) > 0.0 {
                continue;
            }

            let h = match child {
                LayoutChild::Flow(f) => f.height_for_width(content_width),
                LayoutChild::Row(r) => r.height_for_width(content_width),
                LayoutChild::Column(c) => c.height_for_width(content_width),
                _ => child.measure_main(true),
            };
            total_height += h;
        }

        // Spacing between all children (flex children still occupy a slot)
        if self.children.len() > 1 {
            total_height += self.spacing * (self.children.len() - 1) as f32;
        }

        total_height + self.padding.vertical()
    }

    /// Compute layout and flush to snapshot.
    ///
    /// This is where the actual layout math happens - ONCE per frame.
    pub fn layout(self, snapshot: &mut LayoutSnapshot, bounds: Rect) {
        // Debug tracking for layout visualization
        snapshot.debug_enter("Column", bounds);

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
                            // Use height_for_width to account for FlowContainer wrapping
                            let h = r.height_for_width(content_width);
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
                LayoutChild::VirtualTable(table) => {
                    let h = table.estimate_size().height;
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::Flow(flow) => {
                    // FlowContainer height depends on available width
                    let h = flow.height_for_width(content_width);
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
                        let mut text_layout = TextLayout::simple(
                            t.text.clone(),
                            t.color.pack(),
                            x, y,
                            CHAR_WIDTH * scale, LINE_HEIGHT * scale,
                        );
                        // Expand hit-box to full content width — in Column, text
                        // owns the entire line so this is safe (no sibling conflicts).
                        text_layout.bounds.width = text_layout.bounds.width.max(content_width);
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

                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::Terminal(t) => {
                    let size = t.size();
                    let x = cross_x(size.width);

                    use crate::layout_snapshot::{GridLayout, GridRow, SourceLayout};
                    let rows_content: Vec<GridRow> = t.row_content.into_iter()
                        .map(|runs| GridRow { runs })
                        .collect();
                    let mut grid_layout = GridLayout::with_rows(
                        Rect::new(x, y, size.width.max(content_width), size.height),
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
                LayoutChild::VirtualTable(table) => {
                    let size = table.estimate_size();
                    let w = size.width.min(content_width);
                    let tx = cross_x(w);
                    render_virtual_table(snapshot, table, tx, y, w, size.height);
                    y += size.height + self.spacing + alignment_gap;
                }
                LayoutChild::Flow(flow) => {
                    let w = match flow.width {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) | Length::Shrink => content_width,
                    };
                    let h = flow.height_for_width(w);
                    let fx = cross_x(w);
                    flow.layout(snapshot, fx, y, w);
                    y += h + self.spacing + alignment_gap;
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
                    // Give Rows the full content width so their children's
                    // hit-boxes can expand to fill the line (same as Column text).
                    let w = match nested.width {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) | Length::Shrink => content_width,
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

        snapshot.debug_exit();
    }

    // =========================================================================
    // Constraint-based Layout API (Phase 4)
    // =========================================================================

    /// Layout with constraints - the new constraint-based API.
    ///
    /// Takes constraints (min/max bounds) and returns the actual size used.
    /// This uses the shared `distribute_flex` function for consistent flex math.
    ///
    /// # Arguments
    /// * `ctx` - Layout context with scratch buffers and snapshot
    /// * `constraints` - Min/max bounds for this column
    /// * `origin` - Top-left position to place this column
    ///
    /// # Returns
    /// The actual size consumed by this column.
    pub fn layout_with_constraints(
        self,
        ctx: &mut LayoutContext,
        constraints: LayoutConstraints,
        origin: Point,
    ) -> Size {
        ctx.enter("Column");

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
    fn test_column_new() {
        let col = Column::new();
        assert_eq!(col.spacing, 0.0);
        assert_eq!(col.padding.horizontal(), 0.0);
        assert_eq!(col.padding.vertical(), 0.0);
        assert!(col.background.is_none());
    }

    #[test]
    fn test_column_builder_pattern() {
        let col = Column::new()
            .spacing(10.0)
            .padding(5.0)
            .background(Color::WHITE)
            .corner_radius(4.0)
            .width(Length::Fill)
            .height(Length::Fixed(100.0));

        assert_eq!(col.spacing, 10.0);
        assert_eq!(col.padding.horizontal(), 10.0);
        assert!(col.background.is_some());
        assert_eq!(col.corner_radius, 4.0);
        assert!(col.width.is_flex());
        assert!(!col.height.is_flex());
    }

    #[test]
    fn test_column_measure_empty() {
        let col = Column::new();
        let size = col.measure();
        assert_eq!(size.width, 0.0);
        assert_eq!(size.height, 0.0);
    }

    #[test]
    fn test_column_measure_with_children() {
        let col = Column::new()
            .spacing(5.0)
            .push(TextElement::new("Hello"))
            .push(TextElement::new("World"));

        let size = col.measure();
        // Width should be max child width
        assert!(size.width > 0.0);
        // Height should be sum of children + spacing
        assert!(size.height > 0.0);
    }

    #[test]
    fn test_column_measure_with_padding() {
        let col = Column::new()
            .padding(10.0)
            .push(TextElement::new("Test"));

        let size = col.measure();
        // Should include padding
        assert!(size.width >= 20.0); // At least horizontal padding
        assert!(size.height >= 20.0); // At least vertical padding
    }

    #[test]
    fn test_column_fixed_spacer() {
        let col = Column::new()
            .fixed_spacer(50.0);

        let size = col.measure();
        assert_eq!(size.height, 50.0);
    }

    #[test]
    fn test_column_default() {
        let col = Column::default();
        assert_eq!(col.spacing, 0.0);
    }

    #[test]
    fn test_column_custom_padding() {
        let col = Column::new()
            .padding_custom(Padding::new(1.0, 2.0, 3.0, 4.0));

        assert_eq!(col.padding.top, 1.0);
        assert_eq!(col.padding.right, 2.0);
        assert_eq!(col.padding.bottom, 3.0);
        assert_eq!(col.padding.left, 4.0);
    }

    #[test]
    fn test_column_border() {
        let col = Column::new()
            .border(Color::RED, 2.0);

        assert!(col.border_color.is_some());
        assert_eq!(col.border_width, 2.0);
    }

    #[test]
    fn test_column_shadow() {
        let col = Column::new()
            .shadow(8.0, Color::BLACK);

        assert!(col.shadow.is_some());
        let (blur, _color) = col.shadow.unwrap();
        assert_eq!(blur, 8.0);
    }

    #[test]
    fn test_column_alignment() {
        let col = Column::new()
            .align(Alignment::Center)
            .cross_align(CrossAxisAlignment::Center);

        assert_eq!(col.alignment, Alignment::Center);
        assert_eq!(col.cross_alignment, CrossAxisAlignment::Center);
    }

    #[test]
    fn test_column_layout_with_constraints() {
        use crate::layout_snapshot::LayoutSnapshot;
        use crate::layout::context::LayoutContext;
        use crate::layout::constraints::LayoutConstraints;
        use crate::primitives::Point;

        let col = Column::new()
            .spacing(5.0)
            .push(TextElement::new("Hello"))
            .push(TextElement::new("World"));

        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        let constraints = LayoutConstraints::loose(500.0, 300.0);
        let size = col.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);

        assert!(size.width > 0.0);
        assert!(size.width <= 500.0);
        assert!(size.height > 0.0);
        assert!(size.height <= 300.0);
    }

    #[test]
    fn test_column_layout_with_constraints_fill() {
        use crate::layout_snapshot::LayoutSnapshot;
        use crate::layout::context::LayoutContext;
        use crate::layout::constraints::LayoutConstraints;
        use crate::primitives::Point;

        let col = Column::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(TextElement::new("Hello"));

        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        let constraints = LayoutConstraints::tight(200.0, 100.0);
        let size = col.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);

        // Fill should take all available space
        assert_eq!(size.width, 200.0);
        assert_eq!(size.height, 100.0);
    }
}
