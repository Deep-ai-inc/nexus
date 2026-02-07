//! Scroll Column - Virtualized vertical scroll container.
//!
//! Scroll state lives in app state. The container receives the current scroll
//! offset as a parameter. Only children intersecting the viewport are rendered.
//!
//! ## Zero-Cost State Sync
//!
//! When created via `from_state(&scroll_state)`, the ScrollColumn holds a
//! reference to the ScrollState and updates it directly during layout via
//! interior mutability (`Cell`). This eliminates the need for manual
//! `sync_from_snapshot` calls.

use std::marker::PhantomData;

use crate::content_address::SourceId;
use crate::layout_snapshot::{CursorIcon, ScrollTrackInfo};
use crate::primitives::{Color, Point, Rect, Size};
use crate::scroll_state::ScrollState;

use super::base::{Chrome, render_chrome};
use super::child::LayoutChild;
use super::column::Column;
use super::constraints::LayoutConstraints;
use super::context::LayoutContext;
use super::row::Row;
use super::elements::{TextElement, TerminalElement, ImageElement, ButtonElement};
use super::text_input::TextInputElement;
use super::table::{TableElement, VirtualTableElement};
use super::length::{Length, Padding, CHAR_WIDTH, LINE_HEIGHT};

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
// ScrollColumn
// =========================================================================

/// A virtualized vertical scroll container.
///
/// Scroll state lives in app state. The container receives the current scroll
/// offset as a parameter. Wheel events flow through `on_mouse` → message →
/// `update()` modifies offset.
///
/// ## Zero-Cost State Sync
///
/// When created via `from_state(&scroll_state)`, the ScrollColumn holds a
/// reference to the ScrollState and updates it directly during layout:
/// - `max` (maximum scroll offset)
/// - `track` (scrollbar track geometry)
/// - `bounds` (container bounds for hit-testing)
///
/// This eliminates the need for manual `sync_from_snapshot` calls. The sync
/// happens during the layout pass using interior mutability (`Cell`), which
/// compiles to a single memory write — zero runtime overhead.
///
/// The ID is required (for event routing and hit-testing the scroll area).
pub struct ScrollColumn<'a> {
    /// Reference to ScrollState for zero-cost sync during layout.
    /// When set, layout updates the state's Cells directly.
    state_ref: Option<&'a ScrollState>,
    /// Widget ID (required for hit-testing and scroll event routing).
    id: SourceId,
    /// Scrollbar thumb widget ID (for drag interaction).
    thumb_id: SourceId,
    /// Child elements.
    children: Vec<LayoutChild<'a>>,
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
    /// Accumulated hash of all children, updated incrementally.
    /// This avoids O(N) iteration in content_hash().
    children_hash: u64,
    /// Phantom data to hold the lifetime when state_ref is None.
    _marker: PhantomData<&'a ()>,
}

/// FNV-1a prime for hash mixing.
const FNV_PRIME: u64 = 0x100000001b3;
/// FNV-1a offset basis.
const FNV_OFFSET: u64 = 0xcbf29ce484222325;

impl<'a> ScrollColumn<'a> {
    /// Create a new scroll column with a required ID.
    pub fn new(id: SourceId, thumb_id: SourceId) -> Self {
        Self {
            state_ref: None,
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
            children_hash: FNV_OFFSET,
            _marker: PhantomData,
        }
    }

    /// Create from a `ScrollState`, storing a reference for zero-cost sync.
    ///
    /// During layout, this ScrollColumn will update the state's `max`, `track`,
    /// and `bounds` fields directly via interior mutability. No manual
    /// `sync_from_snapshot` call is needed.
    pub fn from_state(state: &'a ScrollState) -> Self {
        Self {
            state_ref: Some(state),
            id: state.id(),
            thumb_id: state.thumb_id(),
            children: Vec::new(),
            scroll_offset: state.offset,
            spacing: 0.0,
            padding: Padding::default(),
            background: None,
            corner_radius: 0.0,
            width: Length::Shrink,
            height: Length::Shrink,
            border_color: None,
            border_width: 0.0,
            children_hash: FNV_OFFSET,
            _marker: PhantomData,
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
    pub fn text(self, element: TextElement) -> Self {
        self.push(element)
    }

    /// Add a terminal element.
    pub fn terminal(self, element: TerminalElement) -> Self {
        self.push(element)
    }

    /// Add a nested column.
    pub fn column(self, column: Column<'a>) -> Self {
        self.push(column)
    }

    /// Add a nested row.
    pub fn row(self, row: Row<'a>) -> Self {
        self.push(row)
    }

    /// Add a nested scroll column.
    pub fn scroll_column(self, scroll: ScrollColumn<'a>) -> Self {
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
    pub fn text_input(self, element: TextInputElement<'a>) -> Self {
        self.push(element)
    }

    /// Add a table element.
    pub fn table(self, element: TableElement) -> Self {
        self.push(element)
    }

    pub fn virtual_table(self, element: VirtualTableElement) -> Self {
        self.push(element)
    }

    /// Add any child element using `From<T> for LayoutChild`.
    ///
    /// The child's content hash is accumulated incrementally, making
    /// `content_hash()` O(1) instead of O(N).
    #[inline(always)]
    pub fn push(mut self, child: impl Into<LayoutChild<'a>>) -> Self {
        let child = child.into();
        // Accumulate child hash incrementally
        self.children_hash = self.children_hash
            .wrapping_mul(FNV_PRIME)
            .wrapping_add(child.content_hash());
        self.children.push(child);
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

    /// Compute a content hash for cache key generation.
    ///
    /// This hash captures all properties that affect layout size:
    /// - Width/height Length values
    /// - Spacing and padding
    /// - Number of children and their content hashes (pre-computed)
    ///
    /// Note: scroll_offset, background, border are NOT included since
    /// they don't affect the measured size.
    ///
    /// # Performance
    ///
    /// This is O(1) because child hashes are accumulated incrementally
    /// during `push()`, not computed here.
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

    /// Extract chrome (visual decorations) for this container.
    /// Note: ScrollColumn does not support shadow.
    #[inline]
    fn chrome(&self) -> Chrome {
        Chrome {
            background: self.background,
            corner_radius: self.corner_radius,
            border_color: self.border_color,
            border_width: self.border_width,
            shadow: None, // ScrollColumn does not support shadow
        }
    }

    // =========================================================================
    // Constraint-based Layout API
    // =========================================================================

    /// Layout with constraints - the native constraint-based API.
    ///
    /// This is the primary layout method. It computes child positions,
    /// performs virtualization (only visible children are rendered),
    /// and delegates rendering to `LayoutChild::perform_layout`.
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
    /// * `constraints` - Min/max bounds for this scroll column
    /// * `origin` - Top-left position to place this scroll column
    ///
    /// # Returns
    /// The actual size consumed by this scroll column.
    pub fn layout_with_constraints(
        self,
        ctx: &mut LayoutContext,
        constraints: LayoutConstraints,
        origin: Point,
    ) -> Size {
        ctx.enter("ScrollColumn");

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
                        self.measure().height
                    }
                }
                Length::Shrink => {
                    let intrinsic = self.measure().height;
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
        ctx.snapshot.debug_enter("ScrollColumn", bounds);
        ctx.log_layout(constraints, size);

        let content_x = bounds.x + self.padding.left;
        let full_content_width = bounds.width - self.padding.horizontal();
        let viewport_h = bounds.height;

        // Draw chrome (background → border, no shadow for ScrollColumn)
        render_chrome(ctx.snapshot, bounds, &self.chrome());

        // Push clip to viewport bounds
        ctx.snapshot.primitives_mut().push_clip(bounds);

        // Reserve space for scrollbar (we'll check if we need it after measuring).
        const SCROLLBAR_GUTTER: f32 = 24.0;

        // First pass: measure heights assuming no scrollbar
        let mut child_heights: Vec<f32> = Vec::with_capacity(self.children.len());
        let mut total_content_height = self.padding.vertical();
        for child in &self.children {
            let h = measure_child_height_for_scroll(child, full_content_width);
            child_heights.push(h);
            total_content_height += h;
        }
        if self.children.len() > 1 {
            total_content_height += self.spacing * (self.children.len() - 1) as f32;
        }

        let overflows = total_content_height > viewport_h;
        let content_width = if overflows { full_content_width - SCROLLBAR_GUTTER } else { full_content_width };

        // If we overflow, re-measure width-dependent children with the reduced width
        if overflows {
            child_heights.clear();
            total_content_height = self.padding.vertical();
            for child in &self.children {
                let h = measure_child_height_for_scroll(child, content_width);
                child_heights.push(h);
                total_content_height += h;
            }
            if self.children.len() > 1 {
                total_content_height += self.spacing * (self.children.len() - 1) as f32;
            }
        }

        // Register container widget for hit-testing (wheel events route here).
        let container_hit_width = if overflows { bounds.width - SCROLLBAR_GUTTER } else { bounds.width };
        ctx.snapshot.register_widget(self.id, Rect::new(bounds.x, bounds.y, container_hit_width, bounds.height));

        // Clamp scroll offset and record max for app-side clamping
        let max_scroll = (total_content_height - viewport_h).max(0.0);
        ctx.snapshot.set_scroll_limit(self.id, max_scroll);
        let offset = self.scroll_offset.clamp(0.0, max_scroll);

        // Position pass with virtualization
        let mut virtual_y = self.padding.top;
        let viewport_top = offset;
        let viewport_bottom = offset + viewport_h;

        for (i, child) in self.children.into_iter().enumerate() {
            let h = child_heights[i];
            let child_top = virtual_y;
            let child_bottom = virtual_y + h;

            // Only render children that intersect the viewport
            if child_bottom > viewport_top && child_top < viewport_bottom {
                let screen_y = bounds.y + child_top - offset;

                // Skip spacers (no rendering needed)
                match &child {
                    LayoutChild::Spacer { .. } | LayoutChild::FixedSpacer { .. } => {
                        virtual_y += h + self.spacing;
                        continue;
                    }
                    _ => {}
                }

                // Compute child width based on its sizing mode
                let child_width = compute_child_width_for_scroll(&child, content_width);
                let child_constraints = LayoutConstraints::tight(child_width, h);
                let child_origin = Point::new(content_x, screen_y);

                // Perform layout
                child.perform_layout(ctx, child_constraints, child_origin);
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

            ctx.snapshot.primitives_mut().add_rounded_rect(
                thumb_visual,
                3.0,
                Color::rgba(1.0, 1.0, 1.0, 0.25),
            );

            // Register the full-height track as the hit region
            let track_hit = Rect::new(bounds.x + bounds.width - SCROLLBAR_GUTTER, bounds.y, SCROLLBAR_GUTTER, viewport_h);
            ctx.snapshot.register_widget(self.thumb_id, track_hit);
            ctx.snapshot.set_cursor_hint(self.thumb_id, CursorIcon::Grab);

            // Store track info so the app can convert mouse Y → scroll offset
            let track_info = ScrollTrackInfo {
                track_y: bounds.y,
                track_height: viewport_h,
                thumb_height: thumb_h,
                max_scroll,
            };
            ctx.snapshot.set_scroll_track(self.id, track_info);

            // Sync to state_ref if present (zero-cost via Cell)
            if let Some(state) = self.state_ref {
                state.max.set(max_scroll);
                state.track.set(Some(track_info));
                state.bounds.set(bounds);
            }
        }

        // Sync bounds even when no scrollbar is visible
        if let Some(state) = self.state_ref {
            state.bounds.set(bounds);
            // Also sync max_scroll when content doesn't overflow
            if total_content_height <= viewport_h {
                state.max.set(0.0);
                state.track.set(None);
            }
        }

        // Pop clip
        ctx.snapshot.primitives_mut().pop_clip();

        ctx.snapshot.debug_exit();
        ctx.exit();
        size
    }
}

// =========================================================================
// Layout Helpers
// =========================================================================

/// Measure child height for scroll column measurement pass.
fn measure_child_height_for_scroll(child: &LayoutChild<'_>, content_width: f32) -> f32 {
    match child {
        LayoutChild::Flow(f) => f.height_for_width(content_width),
        LayoutChild::Row(r) => r.height_for_width(content_width),
        LayoutChild::Column(c) => c.height_for_width(content_width),
        _ => child.measure_main(true),
    }
}

/// Compute child width based on its sizing mode for scroll column.
fn compute_child_width_for_scroll(child: &LayoutChild<'_>, content_width: f32) -> f32 {
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
            // Give Rows the full content width so hit-boxes expand
            Length::Fill | Length::FillPortion(_) | Length::Shrink => content_width,
        },
        LayoutChild::ScrollColumn(s) => match s.width {
            Length::Fixed(px) => px,
            Length::Fill | Length::FillPortion(_) => content_width,
            Length::Shrink => s.measure().width.min(content_width),
        },
        LayoutChild::Canvas(c) => match c.width_length() {
            Length::Fixed(px) => px,
            Length::Fill | Length::FillPortion(_) => content_width,
            Length::Shrink => c.measure().width.min(content_width),
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
    fn test_scroll_column_new() {
        let id = SourceId::named("test");
        let thumb_id = SourceId::named("thumb");
        let sc = ScrollColumn::new(id, thumb_id);
        assert_eq!(sc.spacing, 0.0);
    }

    #[test]
    fn test_scroll_column_measure() {
        let id = SourceId::named("test");
        let thumb_id = SourceId::named("thumb");
        let sc = ScrollColumn::new(id, thumb_id)
            .push(TextElement::new("Hello"))
            .push(TextElement::new("World"));

        let size = sc.measure();
        assert!(size.width > 0.0);
        assert!(size.height > 0.0);
    }

    #[test]
    fn test_scroll_column_layout_with_constraints() {
        use crate::layout_snapshot::LayoutSnapshot;
        use crate::layout::context::LayoutContext;
        use crate::layout::constraints::LayoutConstraints;

        let id = SourceId::named("test");
        let thumb_id = SourceId::named("thumb");
        let sc = ScrollColumn::new(id, thumb_id)
            .push(TextElement::new("Hello"))
            .push(TextElement::new("World"));

        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        let constraints = LayoutConstraints::loose(500.0, 300.0);
        let size = sc.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);

        assert!(size.width > 0.0);
        assert!(size.width <= 500.0);
        assert!(size.height > 0.0);
        assert!(size.height <= 300.0);
    }

    #[test]
    fn test_scroll_column_content_hash() {
        let id = SourceId::named("test");
        let thumb_id = SourceId::named("thumb");

        let sc1 = ScrollColumn::new(id, thumb_id)
            .push(TextElement::new("Hello"))
            .push(TextElement::new("World"));

        let sc2 = ScrollColumn::new(id, thumb_id)
            .push(TextElement::new("Hello"))
            .push(TextElement::new("World"));

        let sc3 = ScrollColumn::new(id, thumb_id)
            .push(TextElement::new("Different"));

        // Same content = same hash
        assert_eq!(sc1.content_hash(), sc2.content_hash());

        // Different content = different hash
        assert_ne!(sc1.content_hash(), sc3.content_hash());
    }

    #[test]
    fn test_scroll_column_content_hash_spacing_matters() {
        let id = SourceId::named("test");
        let thumb_id = SourceId::named("thumb");

        let sc1 = ScrollColumn::new(id, thumb_id)
            .spacing(4.0)
            .push(TextElement::new("Hello"));

        let sc2 = ScrollColumn::new(id, thumb_id)
            .spacing(8.0)
            .push(TextElement::new("Hello"));

        // Different spacing = different hash (affects layout)
        assert_ne!(sc1.content_hash(), sc2.content_hash());
    }

    #[test]
    fn test_scroll_column_caching_enabled() {
        use crate::layout_snapshot::LayoutSnapshot;
        use crate::layout::context::LayoutContext;
        use crate::layout::cache::LayoutCache;
        use crate::layout::constraints::LayoutConstraints;

        let id = SourceId::named("test");
        let thumb_id = SourceId::named("thumb");

        let mut snapshot = LayoutSnapshot::new();
        let mut cache = LayoutCache::new();

        let constraints = LayoutConstraints::loose(500.0, 300.0);

        // First layout - cache miss
        {
            let sc = ScrollColumn::new(id, thumb_id)
                .push(TextElement::new("Hello"))
                .push(TextElement::new("World"));
            let mut ctx = LayoutContext::with_cache(&mut snapshot, &mut cache);
            let _size1 = sc.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);
        }

        // Second layout with same content - should be cache hit
        {
            let sc = ScrollColumn::new(id, thumb_id)
                .push(TextElement::new("Hello"))
                .push(TextElement::new("World"));
            let mut ctx = LayoutContext::with_cache(&mut snapshot, &mut cache);
            let _size2 = sc.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);
        }

        assert_eq!(cache.len(), 1, "Should have one cached entry");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_scroll_column_cache_stats() {
        use crate::layout_snapshot::LayoutSnapshot;
        use crate::layout::context::LayoutContext;
        use crate::layout::cache::LayoutCache;
        use crate::layout::constraints::LayoutConstraints;

        let id = SourceId::named("test");
        let thumb_id = SourceId::named("thumb");

        let mut snapshot = LayoutSnapshot::new();
        let mut cache = LayoutCache::new();

        let constraints = LayoutConstraints::loose(500.0, 300.0);

        // First pass - cache miss
        {
            let sc = ScrollColumn::new(id, thumb_id)
                .push(TextElement::new("Test"));
            let mut ctx = LayoutContext::with_cache(&mut snapshot, &mut cache);
            sc.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);
        }

        // Second pass - cache hit
        {
            let sc = ScrollColumn::new(id, thumb_id)
                .push(TextElement::new("Test"));
            let mut ctx = LayoutContext::with_cache(&mut snapshot, &mut cache);
            sc.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);
        }

        let (hits, misses) = cache.stats();
        assert_eq!(misses, 1, "Should have one miss (first pass)");
        assert_eq!(hits, 1, "Should have one hit (second pass)");
    }
}
