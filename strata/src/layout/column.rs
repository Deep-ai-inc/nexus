//! Column - Vertical layout container.
//!
//! Children flow top to bottom. Supports flex sizing, spacing, padding,
//! and alignment on both axes.

use crate::content_address::SourceId;
use crate::layout_snapshot::CursorIcon;
use crate::primitives::{Color, Point, Rect, Size};

use super::base::{Chrome, render_chrome};
use super::child::LayoutChild;
use super::constraints::LayoutConstraints;
use super::context::LayoutContext;
use super::elements::{TextElement, TerminalElement, ImageElement, ButtonElement};
use super::text_input::TextInputElement;
use super::table::{TableElement, VirtualTableElement};
use super::length::{Length, Padding, Alignment, CrossAxisAlignment, CHAR_WIDTH, LINE_HEIGHT};
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
    /// Accumulated hash of all children, updated incrementally.
    /// This avoids O(N) iteration in content_hash().
    children_hash: u64,
}

impl Default for Column {
    fn default() -> Self {
        Self::new()
    }
}

/// FNV-1a prime for hash mixing.
const FNV_PRIME: u64 = 0x100000001b3;
/// FNV-1a offset basis.
const FNV_OFFSET: u64 = 0xcbf29ce484222325;

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
            children_hash: FNV_OFFSET,
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
    pub fn text(self, element: TextElement) -> Self {
        self.push(element)
    }

    /// Add a terminal element.
    pub fn terminal(self, element: TerminalElement) -> Self {
        self.push(element)
    }

    /// Add a nested column.
    pub fn column(self, column: Column) -> Self {
        self.push(column)
    }

    /// Add a nested row.
    pub fn row(self, row: Row) -> Self {
        self.push(row)
    }

    /// Add a scroll column.
    pub fn scroll_column(self, scroll: ScrollColumn) -> Self {
        self.push(scroll)
    }

    /// Add a flexible spacer.
    pub fn spacer(self, flex: f32) -> Self {
        self.push(LayoutChild::Spacer { flex })
    }

    /// Add a fixed-size spacer.
    pub fn fixed_spacer(self, size: f32) -> Self {
        self.push(LayoutChild::FixedSpacer { size })
    }

    /// Add an image element.
    pub fn image(self, element: ImageElement) -> Self {
        self.push(element)
    }

    /// Add a button element.
    pub fn button(self, element: ButtonElement) -> Self {
        self.push(element)
    }

    /// Add a text input element.
    pub fn text_input(self, element: TextInputElement) -> Self {
        self.push(element)
    }

    pub fn table(self, element: TableElement) -> Self {
        self.push(element)
    }

    pub fn virtual_table(self, element: VirtualTableElement) -> Self {
        self.push(element)
    }

    /// Add any child element using `From<T> for LayoutChild`.
    ///
    /// This is a generic alternative to the type-specific methods above.
    /// The compiler resolves the `Into` conversion at compile time, so this
    /// generates identical code to calling `.text()`, `.button()`, etc. directly.
    ///
    /// The child's content hash is accumulated incrementally, making
    /// `content_hash()` O(1) instead of O(N).
    #[inline(always)]
    pub fn push(mut self, child: impl Into<LayoutChild>) -> Self {
        let child = child.into();
        // Accumulate child hash incrementally
        self.children_hash = self.children_hash
            .wrapping_mul(FNV_PRIME)
            .wrapping_add(child.content_hash());
        self.children.push(child);
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
    /// - Number of children and their content hashes (pre-computed)
    ///
    /// Note: background, border, shadow are NOT included since
    /// they don't affect the measured size.
    ///
    /// This method is O(1) because child hashes are accumulated
    /// incrementally during `push()`.
    #[inline]
    pub fn content_hash(&self) -> u64 {
        let mut hash: u64 = FNV_OFFSET;

        // Mix in width/height Length settings
        hash = hash.wrapping_mul(FNV_PRIME) ^ hash_length(&self.width);
        hash = hash.wrapping_mul(FNV_PRIME) ^ hash_length(&self.height);

        // Mix in spacing and padding
        hash = hash.wrapping_mul(FNV_PRIME) ^ self.spacing.to_bits() as u64;
        hash = hash.wrapping_mul(FNV_PRIME) ^ self.padding.horizontal().to_bits() as u64;
        hash = hash.wrapping_mul(FNV_PRIME) ^ self.padding.vertical().to_bits() as u64;

        // Mix in child count and pre-computed children hash
        hash = hash.wrapping_mul(FNV_PRIME) ^ self.children.len() as u64;
        hash = hash.wrapping_mul(FNV_PRIME) ^ self.children_hash;

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

    /// Extract chrome (visual decorations) for this container.
    #[inline]
    fn chrome(&self) -> Chrome {
        Chrome {
            background: self.background,
            corner_radius: self.corner_radius,
            border_color: self.border_color,
            border_width: self.border_width,
            shadow: self.shadow,
        }
    }

    // =========================================================================
    // Constraint-based Layout API
    // =========================================================================

    /// Layout with constraints - the native constraint-based API.
    ///
    /// This is the primary layout method. It computes child positions and
    /// delegates rendering to `LayoutChild::perform_layout`.
    ///
    /// ## Caching
    ///
    /// When a cache is provided via `LayoutContext::with_cache()`, this method
    /// will memoize the size calculation based on content hash and constraints.
    /// The rendering step always runs since the snapshot is cleared each frame -
    /// only the size calculation is cached.
    ///
    /// # Arguments
    /// * `ctx` - Layout context with snapshot and debug state
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

        // Try to get cached size if caching is enabled
        let content_hash = self.content_hash();
        let size = if let Some(cached_size) = ctx.cache_get(content_hash, &constraints) {
            cached_size
        } else {
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

            let computed_size = constraints.constrain(Size::new(width, height));
            ctx.cache_insert(content_hash, &constraints, computed_size);
            computed_size
        };

        let bounds = Rect::new(origin.x, origin.y, size.width, size.height);

        // Debug tracking
        ctx.snapshot.debug_enter("Column", bounds);
        ctx.log_layout(constraints, size);

        // Content area after padding
        let content_x = bounds.x + self.padding.left;
        let content_y = bounds.y + self.padding.top;
        let content_width = bounds.width - self.padding.horizontal();
        let content_height = bounds.height - self.padding.vertical();

        // Draw chrome (shadow → background → border)
        let chrome = self.chrome();
        let has_chrome = chrome.has_visible_chrome();
        render_chrome(ctx.snapshot, bounds, &chrome);

        // =====================================================================
        // Measurement pass: compute child heights and flex factors
        // =====================================================================
        let mut total_fixed_height = 0.0;
        let mut total_flex = 0.0;
        let mut max_child_cross: f32 = 0.0;
        let mut child_heights: Vec<f32> = Vec::with_capacity(self.children.len());

        for child in &self.children {
            max_child_cross = max_child_cross.max(child.measure_cross(true));
            let (h, flex) = measure_child_height(child, content_width);
            child_heights.push(h);
            if flex > 0.0 {
                total_flex += flex;
            } else {
                total_fixed_height += h;
            }
        }

        // Add spacing
        if !self.children.is_empty() {
            total_fixed_height += self.spacing * (self.children.len() - 1) as f32;
        }

        // Overflow detection
        let content_overflows = bounds.width < max_child_cross + self.padding.horizontal()
            || bounds.height < total_fixed_height + self.padding.vertical();
        let clips = has_chrome || content_overflows;
        if clips {
            ctx.snapshot.primitives_mut().push_clip(bounds);
        }

        let available_flex = (content_height - total_fixed_height).max(0.0);
        let total_flex_consumed = if total_flex > 0.0 { available_flex } else { 0.0 };
        let free_space = (content_height - total_fixed_height - total_flex_consumed).max(0.0);

        // =====================================================================
        // Main axis alignment
        // =====================================================================
        let n = self.children.len();
        let (mut y, alignment_gap) = match self.alignment {
            Alignment::Start => (content_y, 0.0),
            Alignment::End => (content_y + free_space, 0.0),
            Alignment::Center => (content_y + free_space / 2.0, 0.0),
            Alignment::SpaceBetween => {
                if n > 1 { (content_y, free_space / (n - 1) as f32) } else { (content_y, 0.0) }
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

        // Helper: resolve cross-axis x position
        let cross_x = |child_width: f32| -> f32 {
            match self.cross_alignment {
                CrossAxisAlignment::Start | CrossAxisAlignment::Stretch => content_x,
                CrossAxisAlignment::End => content_x + content_width - child_width,
                CrossAxisAlignment::Center => content_x + (content_width - child_width) / 2.0,
            }
        };

        // =====================================================================
        // Position pass: place children using perform_layout
        // =====================================================================
        for (i, child) in self.children.into_iter().enumerate() {
            let mut child_height = child_heights[i];

            // Resolve flex height
            if child.flex_factor(true) > 0.0 && total_flex > 0.0 {
                child_height = (child.flex_factor(true) / total_flex) * available_flex;
            }

            // Compute child width based on its sizing mode
            let child_width = compute_child_width(&child, content_width);
            let x = cross_x(child_width);

            // Skip spacers (no rendering needed)
            match &child {
                LayoutChild::Spacer { .. } => {
                    y += child_height + alignment_gap;
                    continue;
                }
                LayoutChild::FixedSpacer { size } => {
                    y += size + alignment_gap;
                    continue;
                }
                _ => {}
            }

            // Create constraints for child
            let child_constraints = LayoutConstraints::tight(child_width, child_height);
            let child_origin = Point::new(x, y);

            // Perform layout
            child.perform_layout(ctx, child_constraints, child_origin);

            y += child_height + self.spacing + alignment_gap;
        }

        // Register widget ID
        if let Some(id) = self.id {
            ctx.snapshot.register_widget(id, bounds);
            if let Some(cursor) = self.cursor_hint {
                ctx.snapshot.set_cursor_hint(id, cursor);
            }
        }

        if clips {
            ctx.snapshot.primitives_mut().pop_clip();
        }

        ctx.snapshot.debug_exit();
        ctx.exit();
        size
    }
}

// =========================================================================
// Layout Helpers
// =========================================================================

/// Measure child height and flex factor for the measurement pass.
fn measure_child_height(child: &LayoutChild, content_width: f32) -> (f32, f32) {
    match child {
        LayoutChild::Text(t) => (t.estimate_size(CHAR_WIDTH, LINE_HEIGHT).height, 0.0),
        LayoutChild::Terminal(t) => (t.size().height, 0.0),
        LayoutChild::Image(img) => (img.height, 0.0),
        LayoutChild::Button(btn) => (btn.estimate_size().height, 0.0),
        LayoutChild::TextInput(input) => (LINE_HEIGHT + input.padding.vertical(), 0.0),
        LayoutChild::Table(table) => (table.estimate_size().height, 0.0),
        LayoutChild::VirtualTable(table) => (table.estimate_size().height, 0.0),
        LayoutChild::Flow(flow) => (flow.height_for_width(content_width), 0.0),
        LayoutChild::Column(c) => match c.height {
            Length::Fixed(px) => (px, 0.0),
            Length::Fill | Length::FillPortion(_) => (0.0, c.height.flex()),
            Length::Shrink => (c.measure().height, 0.0),
        },
        LayoutChild::Row(r) => match r.height {
            Length::Fixed(px) => (px, 0.0),
            Length::Fill | Length::FillPortion(_) => (0.0, r.height.flex()),
            Length::Shrink => (r.height_for_width(content_width), 0.0),
        },
        LayoutChild::ScrollColumn(s) => match s.height {
            Length::Fixed(px) => (px, 0.0),
            Length::Fill | Length::FillPortion(_) => (0.0, s.height.flex()),
            Length::Shrink => (s.measure().height, 0.0),
        },
        LayoutChild::Spacer { flex } => (0.0, *flex),
        LayoutChild::FixedSpacer { size } => (*size, 0.0),
    }
}

/// Compute child width based on its sizing mode.
fn compute_child_width(child: &LayoutChild, content_width: f32) -> f32 {
    match child {
        LayoutChild::Text(t) => t.estimate_size(CHAR_WIDTH, LINE_HEIGHT).width,
        LayoutChild::Terminal(t) => t.size().width,
        LayoutChild::Image(img) => img.width,
        LayoutChild::Button(btn) => btn.estimate_size().width,
        LayoutChild::TextInput(input) => match input.width {
            Length::Fixed(px) => px,
            Length::Fill | Length::FillPortion(_) => content_width,
            Length::Shrink => input.estimate_size().width.min(content_width),
        },
        LayoutChild::Table(table) => table.estimate_size().width.min(content_width),
        LayoutChild::VirtualTable(table) => table.estimate_size().width.min(content_width),
        LayoutChild::Flow(flow) => match flow.width {
            Length::Fixed(px) => px,
            _ => content_width,
        },
        LayoutChild::Column(c) => match c.width {
            Length::Fixed(px) => px,
            Length::Fill | Length::FillPortion(_) => content_width,
            Length::Shrink => c.measure().width.min(content_width),
        },
        LayoutChild::Row(r) => match r.width {
            Length::Fixed(px) => px,
            _ => content_width,
        },
        LayoutChild::ScrollColumn(s) => match s.width {
            Length::Fixed(px) => px,
            Length::Fill | Length::FillPortion(_) => content_width,
            Length::Shrink => s.measure().width.min(content_width),
        },
        LayoutChild::Spacer { .. } | LayoutChild::FixedSpacer { .. } => 0.0,
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

    #[test]
    fn test_column_caching_enabled() {
        use crate::layout_snapshot::LayoutSnapshot;
        use crate::layout::context::LayoutContext;
        use crate::layout::cache::LayoutCache;
        use crate::layout::constraints::LayoutConstraints;
        use crate::primitives::Point;

        let mut snapshot = LayoutSnapshot::new();
        let mut cache = LayoutCache::new();

        let constraints = LayoutConstraints::loose(500.0, 300.0);

        // First layout - cache miss
        {
            let col = Column::new()
                .push(TextElement::new("Hello"))
                .push(TextElement::new("World"));
            let mut ctx = LayoutContext::with_cache(&mut snapshot, &mut cache);
            let _size1 = col.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);
        }

        // Second layout with same content - should be cache hit
        {
            let col = Column::new()
                .push(TextElement::new("Hello"))
                .push(TextElement::new("World"));
            let mut ctx = LayoutContext::with_cache(&mut snapshot, &mut cache);
            let _size2 = col.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);
        }

        assert_eq!(cache.len(), 1, "Should have one cached entry");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_column_cache_stats() {
        use crate::layout_snapshot::LayoutSnapshot;
        use crate::layout::context::LayoutContext;
        use crate::layout::cache::LayoutCache;
        use crate::layout::constraints::LayoutConstraints;
        use crate::primitives::Point;

        let mut snapshot = LayoutSnapshot::new();
        let mut cache = LayoutCache::new();

        let constraints = LayoutConstraints::loose(500.0, 300.0);

        // First pass - cache miss
        {
            let col = Column::new()
                .push(TextElement::new("Test"));
            let mut ctx = LayoutContext::with_cache(&mut snapshot, &mut cache);
            col.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);
        }

        // Second pass - cache hit
        {
            let col = Column::new()
                .push(TextElement::new("Test"));
            let mut ctx = LayoutContext::with_cache(&mut snapshot, &mut cache);
            col.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);
        }

        let (hits, misses) = cache.stats();
        assert_eq!(misses, 1, "Should have one miss (first pass)");
        assert_eq!(hits, 1, "Should have one hit (second pass)");
    }
}
