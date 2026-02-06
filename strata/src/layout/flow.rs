//! Flow Container - CSS flex-wrap style wrapping layout.
//!
//! Children are laid out horizontally until they exceed the container width,
//! then wrap to the next line. Supports text, images, and buttons.
//! Reflows automatically on container resize.
//!
//! ## Lifetime Parameter
//!
//! The `'a` lifetime enables child elements to hold references to application
//! state. This is used by `ScrollColumn` and `TextInputElement` for zero-cost
//! interior mutability during layout.

use std::marker::PhantomData;

use crate::content_address::SourceId;
use crate::layout_snapshot::LayoutSnapshot;
use crate::primitives::{Point, Rect, Size};

use super::child::LayoutChild;
use super::constraints::LayoutConstraints;
use super::context::LayoutContext;
use super::elements::{TextElement, unicode_display_width};
use super::length::{Length, Padding, CHAR_WIDTH, LINE_HEIGHT, BASE_FONT_SIZE};

// =========================================================================
// FlowContainer
// =========================================================================

/// A flow container that wraps children like CSS `flex-wrap: wrap`.
///
/// Children are laid out horizontally until they exceed the container width,
/// then wrap to the next line. Supports any element type (text, images, etc.).
/// Reflows automatically on container resize.
///
/// ## Lifetime Parameter
///
/// The `'a` lifetime allows children to hold references to application state,
/// enabling zero-cost interior mutability for types like `ScrollColumn`.
pub struct FlowContainer<'a> {
    /// Child elements.
    children: Vec<LayoutChild<'a>>,
    /// Source ID for hit-testing.
    source_id: Option<SourceId>,
    /// Horizontal spacing between items.
    spacing: f32,
    /// Vertical spacing between lines.
    line_spacing: f32,
    /// Padding around content.
    padding: Padding,
    /// Width sizing mode.
    pub(crate) width: Length,
    /// Phantom data to hold the lifetime.
    _marker: PhantomData<&'a ()>,
}

impl Default for FlowContainer<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> FlowContainer<'a> {
    /// Create a new flow container.
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            source_id: None,
            spacing: 0.0,
            line_spacing: 2.0,
            padding: Padding::default(),
            width: Length::Fill,
            _marker: PhantomData,
        }
    }

    /// Set the source ID for hit-testing.
    pub fn source(mut self, source_id: SourceId) -> Self {
        self.source_id = Some(source_id);
        self
    }

    /// Set horizontal spacing between items.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set vertical spacing between wrapped lines.
    pub fn line_spacing(mut self, spacing: f32) -> Self {
        self.line_spacing = spacing;
        self
    }

    /// Set padding around content.
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = Padding::all(padding);
        self
    }

    /// Set custom padding.
    pub fn padding_custom(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }

    /// Set the width sizing mode.
    pub fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    /// Add a text element.
    pub fn text(mut self, element: TextElement) -> Self {
        self.children.push(LayoutChild::Text(element));
        self
    }

    /// Add any child element.
    pub fn push(mut self, child: impl Into<LayoutChild<'a>>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Measure intrinsic size (assumes single line for estimation).
    pub fn measure(&self) -> Size {
        let mut width = 0.0f32;
        let mut max_height = 0.0f32;

        for child in &self.children {
            let size = child.size();
            width += size.width + self.spacing;
            max_height = max_height.max(size.height);
        }

        Size::new(
            width + self.padding.horizontal(),
            max_height + self.padding.vertical(),
        )
    }

    /// Compute a content hash for cache key generation.
    ///
    /// This hash captures all properties that affect layout size:
    /// - Number of children and their content hashes
    /// - Spacing, line_spacing, and padding
    pub fn content_hash(&self) -> u64 {
        // Start with container properties that affect layout
        let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis

        // Mix in spacing values
        hash = hash.wrapping_mul(0x100000001b3);
        hash ^= self.spacing.to_bits() as u64;

        hash = hash.wrapping_mul(0x100000001b3);
        hash ^= self.line_spacing.to_bits() as u64;

        hash = hash.wrapping_mul(0x100000001b3);
        hash ^= self.padding.horizontal().to_bits() as u64;

        hash = hash.wrapping_mul(0x100000001b3);
        hash ^= self.padding.vertical().to_bits() as u64;

        // Mix in child count
        hash = hash.wrapping_mul(0x100000001b3);
        hash ^= self.children.len() as u64;

        // Mix in each child's content hash
        for child in &self.children {
            hash = hash.wrapping_mul(0x100000001b3);
            hash ^= child.content_hash();
        }

        hash
    }

    /// Layout and render into the snapshot (legacy API).
    ///
    /// This is the original API. For new code, prefer `layout_with_constraints()`
    /// which provides better introspection and constraint propagation.
    #[deprecated(since = "0.2.0", note = "use layout_with_constraints() instead")]
    pub fn layout(&self, snapshot: &mut LayoutSnapshot, x: f32, y: f32, available_width: f32) {
        self.layout_impl(snapshot, x, y, available_width);
    }

    /// Calculate the total height needed for a given width.
    pub fn height_for_width(&self, available_width: f32) -> f32 {
        let max_width = available_width - self.padding.horizontal();

        let mut line_x = 0.0f32;
        let mut line_y = 0.0f32;
        let mut line_height = 0.0f32;

        for child in &self.children {
            let size = child.size();

            if line_x > 0.0 && line_x + size.width > max_width {
                line_y += line_height + self.line_spacing;
                line_x = 0.0;
                line_height = 0.0;
            }

            line_x += size.width + self.spacing;
            line_height = line_height.max(size.height);
        }

        line_y + line_height + self.padding.vertical()
    }

    // =========================================================================
    // Constraint-based Layout API (Phase 3)
    // =========================================================================

    /// Layout with constraints - the new constraint-based API.
    ///
    /// Takes constraints (min/max bounds) and returns the actual size used.
    /// This is a bridge method that enables gradual migration to constraint-based layout.
    ///
    /// Flow containers use width-first layout: they take the available width
    /// (from constraints), wrap children, and return their computed height.
    ///
    /// ## Caching
    ///
    /// When a cache is provided via `LayoutContext::with_cache()`, this method
    /// will memoize the size calculation based on content hash and max_width.
    /// The rendering step (`layout_impl`) always runs since the snapshot is
    /// cleared each frame - only the size calculation is cached.
    pub fn layout_with_constraints(
        &self,
        ctx: &mut LayoutContext,
        constraints: LayoutConstraints,
        origin: Point,
    ) -> Size {
        ctx.enter("FlowContainer");

        // Determine width: use max_width if bounded, otherwise use intrinsic
        let available_width = if constraints.has_bounded_width() {
            constraints.max_width
        } else {
            self.measure().width
        };

        // Try to get cached size if caching is enabled
        let content_hash = self.content_hash();
        let size = if let Some(cached_size) = ctx.cache_get_flow(content_hash, available_width) {
            // Cache hit - use cached size
            cached_size
        } else {
            // Cache miss - compute size
            let computed_height = self.height_for_width(available_width);
            let computed_size = constraints.constrain(Size::new(available_width, computed_height));

            // Store in cache for next frame
            ctx.cache_insert_flow(content_hash, available_width, computed_size);

            computed_size
        };

        ctx.log_layout(constraints, size);

        // Always render children (snapshot is cleared each frame)
        // (debug rects are pushed inside layout_impl() via snapshot.debug_enter())
        self.layout_impl(ctx.snapshot, origin.x, origin.y, size.width);

        ctx.exit();
        size
    }

    /// Internal implementation shared by both old and new APIs.
    fn layout_impl(&self, snapshot: &mut LayoutSnapshot, x: f32, y: f32, available_width: f32) {
        // Debug tracking for layout visualization
        let height = self.height_for_width(available_width);
        snapshot.debug_enter("FlowContainer", crate::primitives::Rect::new(x, y, available_width, height));

        let content_x = x + self.padding.left;
        let content_y = y + self.padding.top;
        let max_width = available_width - self.padding.horizontal();

        let mut line_x = 0.0f32;
        let mut line_y = 0.0f32;
        let mut line_height = 0.0f32;

        for child in &self.children {
            let size = child.size();

            // Check if we need to wrap to next line
            if line_x > 0.0 && line_x + size.width > max_width {
                line_y += line_height + self.line_spacing;
                line_x = 0.0;
                line_height = 0.0;
            }

            // Render the child at current position
            let child_x = content_x + line_x;
            let child_y = content_y + line_y;

            render_flow_child(snapshot, child, child_x, child_y, size.width, size.height, self.source_id);

            // Advance position
            line_x += size.width + self.spacing;
            line_height = line_height.max(size.height);
        }

        snapshot.debug_exit();
    }
}

// =========================================================================
// Flow Child Rendering
// =========================================================================

/// Render a single child in a flow container.
#[allow(deprecated)] // Nested Flow containers use deprecated layout()
fn render_flow_child(
    snapshot: &mut LayoutSnapshot,
    child: &LayoutChild,
    x: f32, y: f32, w: f32, h: f32,
    source_id: Option<SourceId>,
) {
    match child {
        LayoutChild::Text(t) => {
            let fs = t.font_size();
            snapshot.primitives_mut().add_text_cached_styled(
                &t.text,
                crate::primitives::Point::new(x, y),
                t.color,
                fs,
                t.cache_key,
                t.bold,
                t.italic,
            );
            if let Some(sid) = t.source_id.or(source_id) {
                use crate::layout_snapshot::{SourceLayout, TextLayout};
                let scale = fs / BASE_FONT_SIZE;
                let text_layout = TextLayout::simple(
                    t.text.clone(),
                    t.color.pack(),
                    x, y,
                    CHAR_WIDTH * scale, LINE_HEIGHT * scale,
                );
                snapshot.register_source(sid, SourceLayout::text(text_layout));
            }
        }
        LayoutChild::Image(img) => {
            let img_rect = Rect::new(x, y, img.width, img.height);
            snapshot.primitives_mut().add_image(img_rect, img.handle.clone(), img.corner_radius, img.tint);
        }
        LayoutChild::Button(btn) => {
            let btn_rect = Rect::new(x, y, w, h);
            snapshot.primitives_mut().add_rounded_rect(btn_rect, btn.corner_radius, btn.background);
            let text_x = x + (w - unicode_display_width(&btn.label) * CHAR_WIDTH) / 2.0;
            let text_y = y + (h - LINE_HEIGHT) / 2.0;
            snapshot.primitives_mut().add_text_cached(
                btn.label.clone(),
                crate::primitives::Point::new(text_x, text_y),
                btn.text_color,
                BASE_FONT_SIZE,
                btn.cache_key,
            );
            snapshot.register_widget(btn.id, btn_rect);
        }
        // Note: Column/Row/ScrollColumn are not supported inside FlowContainer
        // because their layout methods consume self. FlowContainer is designed
        // for inline elements (text, images, buttons) that can be reflowed.
        LayoutChild::Flow(nested) => {
            nested.layout(snapshot, x, y, w);
        }
        _ => {}
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
    fn test_flow_container_basic() {
        let flow = FlowContainer::new()
            .spacing(4.0)
            .text(TextElement::new("Hello"))
            .text(TextElement::new("World"));

        let size = flow.measure();
        assert!(size.width > 0.0);
        assert!(size.height > 0.0);
    }

    #[test]
    fn test_flow_container_height_for_width() {
        let flow = FlowContainer::new()
            .spacing(4.0)
            .text(TextElement::new("Hello"))
            .text(TextElement::new("World"));

        // Wide enough for one line
        let h1 = flow.height_for_width(500.0);
        // Narrow - should wrap to multiple lines
        let h2 = flow.height_for_width(50.0);

        assert!(h2 > h1, "Wrapped height should be greater than single-line height");
    }

    #[test]
    fn test_flow_child_size() {
        let child = LayoutChild::Text(TextElement::new("test"));
        let size = child.size();
        assert!(size.width > 0.0);
        assert!(size.height > 0.0);
    }

    #[test]
    fn test_flow_layout_with_constraints() {
        use crate::layout_snapshot::LayoutSnapshot;
        use crate::layout::context::LayoutContext;
        use crate::layout::constraints::LayoutConstraints;
        use crate::primitives::Point;

        let flow = FlowContainer::new()
            .spacing(4.0)
            .text(TextElement::new("Hello"))
            .text(TextElement::new("World"));

        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        // Constrain to 500px width
        let constraints = LayoutConstraints::with_max_width(500.0);
        let size = flow.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);

        assert!(size.width > 0.0);
        assert!(size.width <= 500.0);
        assert!(size.height > 0.0);
    }

    #[test]
    fn test_flow_layout_with_constraints_wrapping() {
        use crate::layout_snapshot::LayoutSnapshot;
        use crate::layout::context::LayoutContext;
        use crate::layout::constraints::LayoutConstraints;
        use crate::primitives::Point;

        let flow = FlowContainer::new()
            .spacing(4.0)
            .text(TextElement::new("Hello"))
            .text(TextElement::new("World"));

        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        // Wide constraints - single line
        let wide = LayoutConstraints::with_max_width(500.0);
        let size_wide = flow.layout_with_constraints(&mut ctx, wide, Point::ORIGIN);

        // Narrow constraints - should wrap
        let narrow = LayoutConstraints::with_max_width(50.0);
        let size_narrow = flow.layout_with_constraints(&mut ctx, narrow, Point::ORIGIN);

        assert!(size_narrow.height > size_wide.height,
            "Narrow layout should be taller due to wrapping");
    }

    #[test]
    fn test_flow_content_hash() {
        let flow1 = FlowContainer::new()
            .spacing(4.0)
            .text(TextElement::new("Hello"))
            .text(TextElement::new("World"));

        let flow2 = FlowContainer::new()
            .spacing(4.0)
            .text(TextElement::new("Hello"))
            .text(TextElement::new("World"));

        let flow3 = FlowContainer::new()
            .spacing(4.0)
            .text(TextElement::new("Different"));

        // Same content = same hash
        assert_eq!(flow1.content_hash(), flow2.content_hash());

        // Different content = different hash
        assert_ne!(flow1.content_hash(), flow3.content_hash());
    }

    #[test]
    fn test_flow_content_hash_spacing_matters() {
        let flow1 = FlowContainer::new()
            .spacing(4.0)
            .text(TextElement::new("Hello"));

        let flow2 = FlowContainer::new()
            .spacing(8.0)
            .text(TextElement::new("Hello"));

        // Different spacing = different hash (affects layout)
        assert_ne!(flow1.content_hash(), flow2.content_hash());
    }

    #[test]
    fn test_flow_caching_enabled() {
        use crate::layout_snapshot::LayoutSnapshot;
        use crate::layout::context::LayoutContext;
        use crate::layout::cache::LayoutCache;
        use crate::layout::constraints::LayoutConstraints;
        use crate::primitives::Point;

        let flow = FlowContainer::new()
            .spacing(4.0)
            .text(TextElement::new("Hello"))
            .text(TextElement::new("World"));

        let mut snapshot = LayoutSnapshot::new();
        let mut cache = LayoutCache::new();
        let mut ctx = LayoutContext::with_cache(&mut snapshot, &mut cache);

        let constraints = LayoutConstraints::with_max_width(500.0);

        // First layout - cache miss
        let size1 = flow.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);

        // Second layout - should be cache hit with same result
        let size2 = flow.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);

        assert_eq!(size1, size2, "Cached size should match computed size");
        assert_eq!(cache.len(), 1, "Should have one cached entry");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_flow_cache_stats() {
        use crate::layout_snapshot::LayoutSnapshot;
        use crate::layout::context::LayoutContext;
        use crate::layout::cache::LayoutCache;
        use crate::layout::constraints::LayoutConstraints;
        use crate::primitives::Point;

        let flow = FlowContainer::new()
            .spacing(4.0)
            .text(TextElement::new("Test"));

        let mut snapshot = LayoutSnapshot::new();
        let mut cache = LayoutCache::new();

        // First pass - cache miss
        {
            let mut ctx = LayoutContext::with_cache(&mut snapshot, &mut cache);
            let constraints = LayoutConstraints::with_max_width(500.0);
            flow.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);
        }

        // Second pass - cache hit
        {
            let mut ctx = LayoutContext::with_cache(&mut snapshot, &mut cache);
            let constraints = LayoutConstraints::with_max_width(500.0);
            flow.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);
        }

        let (hits, misses) = cache.stats();
        assert_eq!(misses, 1, "Should have one miss (first pass)");
        assert_eq!(hits, 1, "Should have one hit (second pass)");
    }
}
