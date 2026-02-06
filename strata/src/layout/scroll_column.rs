//! Scroll Column - Virtualized vertical scroll container.
//!
//! Scroll state lives in app state. The container receives the current scroll
//! offset as a parameter. Only children intersecting the viewport are rendered.

use crate::content_address::SourceId;
use crate::layout_snapshot::{CursorIcon, LayoutSnapshot};
use crate::primitives::{Color, Point, Rect, Size};
use crate::scroll_state::ScrollState;

use super::child::LayoutChild;
use super::column::Column;
use super::constraints::LayoutConstraints;
use super::context::LayoutContext;
use super::row::Row;
use super::text_input::{TextInputElement, render_text_input, render_text_input_multiline};
use super::table::{TableElement, VirtualTableElement, render_table, render_virtual_table};
use super::elements::{TextElement, TerminalElement, ImageElement, ButtonElement};
use super::length::{Length, Padding, CHAR_WIDTH, LINE_HEIGHT, BASE_FONT_SIZE};

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
        self.children.push(LayoutChild::Column(Box::new(column)));
        self
    }

    /// Add a nested row.
    pub fn row(mut self, row: Row) -> Self {
        self.children.push(LayoutChild::Row(Box::new(row)));
        self
    }

    /// Add a nested scroll column.
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
    /// - Number of children and their content hashes
    ///
    /// Note: scroll_offset, background, border are NOT included since
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

        // Mix in each child's content hash
        for child in &self.children {
            hash = hash.wrapping_mul(0x100000001b3);
            hash ^= child.content_hash();
        }

        hash
    }

    /// Compute layout and flush to snapshot.
    ///
    /// Implements virtualization: only children intersecting the viewport
    /// are laid out. A scrollbar thumb is drawn when content overflows.
    pub fn layout(self, snapshot: &mut LayoutSnapshot, bounds: Rect) {
        // Debug tracking for layout visualization
        snapshot.debug_enter("ScrollColumn", bounds);

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

        // Reserve space for scrollbar (we'll check if we need it after measuring).
        const SCROLLBAR_GUTTER: f32 = 24.0;

        // First pass: measure heights assuming no scrollbar
        let mut child_heights: Vec<f32> = Vec::with_capacity(self.children.len());
        let mut total_content_height = self.padding.vertical();
        for child in &self.children {
            let h = match child {
                LayoutChild::Flow(f) => f.height_for_width(full_content_width),
                LayoutChild::Row(r) => r.height_for_width(full_content_width),
                LayoutChild::Column(c) => c.height_for_width(full_content_width),
                _ => child.measure_main(true),
            };
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
                let h = match child {
                    LayoutChild::Flow(f) => f.height_for_width(content_width),
                    LayoutChild::Row(r) => r.height_for_width(content_width),
                    LayoutChild::Column(c) => c.height_for_width(content_width),
                    _ => child.measure_main(true),
                };
                child_heights.push(h);
                total_content_height += h;
            }
            if self.children.len() > 1 {
                total_content_height += self.spacing * (self.children.len() - 1) as f32;
            }
        }

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
                        let size = t.estimate_size(CHAR_WIDTH, LINE_HEIGHT);
                        use crate::layout_snapshot::{SourceLayout, TextLayout};
                        if let Some(source_id) = t.source_id {
                            let scale = fs / BASE_FONT_SIZE;
                            let mut text_layout = TextLayout::simple(
                                t.text.clone(),
                                t.color.pack(),
                                content_x, screen_y,
                                CHAR_WIDTH * scale, LINE_HEIGHT * scale,
                            );
                            // Expand hit-box to full content width — in ScrollColumn,
                            // text owns the entire line so this is safe.
                            text_layout.bounds.width = text_layout.bounds.width.max(content_width);
                            snapshot.register_source(source_id, SourceLayout::text(text_layout));
                        }

                        // Register widget if this text is clickable
                        if let Some(widget_id) = t.widget_id {
                            let text_rect = Rect::new(content_x, screen_y, size.width, size.height);
                            snapshot.register_widget(widget_id, text_rect);
                            if let Some(cursor) = t.cursor_hint {
                                snapshot.set_cursor_hint(widget_id, cursor);
                            }
                        }

                        snapshot.primitives_mut().add_text_cached_styled(
                            t.text,
                            crate::primitives::Point::new(content_x, screen_y),
                            t.color,
                            fs,
                            t.cache_key,
                            t.bold,
                            t.italic,
                        );
                    }
                    LayoutChild::Terminal(t) => {
                        let size = t.size();

                        use crate::layout_snapshot::{GridLayout, GridRow, SourceLayout};
                        let rows_content: Vec<GridRow> = t.row_content.into_iter()
                            .map(|runs| GridRow { runs })
                            .collect();
                        let mut grid_layout = GridLayout::with_rows(
                            Rect::new(content_x, screen_y, size.width.max(content_width), size.height),
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
                        let input_h = if input.multiline {
                            input.estimate_size().height
                        } else {
                            LINE_HEIGHT + input.padding.vertical()
                        };
                        if input.multiline {
                            render_text_input_multiline(snapshot, input, content_x, screen_y, w, input_h);
                        } else {
                            render_text_input(snapshot, input, content_x, screen_y, w, input_h);
                        }
                    }
                    LayoutChild::Table(table) => {
                        let size = table.estimate_size();
                        let w = size.width.min(content_width);
                        render_table(snapshot, table, content_x, screen_y, w, size.height);
                    }
                    LayoutChild::VirtualTable(table) => {
                        let size = table.estimate_size();
                        let w = size.width.min(content_width);
                        render_virtual_table(snapshot, table, content_x, screen_y, w, size.height);
                    }
                    LayoutChild::Flow(flow) => {
                        let w = match flow.width {
                            Length::Fixed(px) => px,
                            Length::Fill | Length::FillPortion(_) | Length::Shrink => content_width,
                        };
                        flow.layout(snapshot, content_x, screen_y, w);
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
                        // Give Rows the full content width so their children's
                        // hit-boxes can expand to fill the line.
                        let w = match nested.width {
                            Length::Fixed(px) => px,
                            Length::Fill | Length::FillPortion(_) | Length::Shrink => content_width,
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

        snapshot.debug_exit();
    }

    // =========================================================================
    // Constraint-based Layout API (Phase 4)
    // =========================================================================

    /// Layout with constraints - the new constraint-based API.
    ///
    /// Takes constraints (min/max bounds) and returns the actual size used.
    /// ScrollColumn is a virtualized vertical container - it clips content
    /// and only renders visible children.
    ///
    /// ## Caching
    ///
    /// When a cache is provided via `LayoutContext::with_cache()`, this method
    /// will memoize the size calculation based on content hash and constraints.
    /// The rendering step (`layout`) always runs since the snapshot is
    /// cleared each frame - only the size calculation is cached.
    ///
    /// # Arguments
    /// * `ctx` - Layout context with scratch buffers and snapshot
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
            // Cache hit - use cached size
            cached_size
        } else {
            // Cache miss - compute size
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
                        // ScrollColumn typically needs a bounded height
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

            // Store in cache for next frame
            ctx.cache_insert(content_hash, &constraints, computed_size);

            computed_size
        };

        ctx.log_layout(constraints, size);

        // Always render children (snapshot is cleared each frame)
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
