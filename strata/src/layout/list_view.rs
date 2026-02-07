//! ListView Widget - Virtualized list with automatic spacers.
//!
//! Handles virtualization internally: only visible items are laid out,
//! with spacers automatically inserted for off-screen items.
//!
//! # Example
//!
//! ```ignore
//! ListView::new(&state.items)
//!     .item_height(|item| estimate_height(item))
//!     .builder(|item, index| {
//!         Row::new()
//!             .push(TextElement::new(&item.name))
//!             .into()
//!     })
//!     .spacing(8.0)
//!     .scroll_offset(state.scroll.offset)
//! ```

use std::marker::PhantomData;

use crate::content_address::SourceId;
use crate::primitives::{Point, Size};

use super::child::LayoutChild;
use super::column::Column;
use super::constraints::LayoutConstraints;
use super::context::LayoutContext;
use super::length::Length;

/// Number of extra items to render above/below viewport.
const OVERSCAN: usize = 2;

/// A virtualized list view that only lays out visible items.
///
/// Items outside the viewport are replaced with fixed spacers to
/// preserve correct scrollbar proportions and total content height.
pub struct ListView<'a, T, H, B>
where
    H: Fn(&T) -> f32,
    B: Fn(&T, usize) -> LayoutChild<'a>,
{
    items: &'a [T],
    item_height: H,
    builder: B,
    spacing: f32,
    scroll_offset: f32,
    viewport_height: f32,
    width: Length,
    id: Option<SourceId>,
    _marker: PhantomData<&'a T>,
}

impl<'a, T, H, B> ListView<'a, T, H, B>
where
    H: Fn(&T) -> f32,
    B: Fn(&T, usize) -> LayoutChild<'a>,
{
    /// Create a new ListView with items, height estimator, and builder.
    ///
    /// - `items`: The slice of data items
    /// - `item_height`: Function that estimates the height of each item
    /// - `builder`: Function that builds the layout element for each visible item
    pub fn new(items: &'a [T], item_height: H, builder: B) -> Self {
        ListView {
            items,
            item_height,
            builder,
            spacing: 0.0,
            scroll_offset: 0.0,
            viewport_height: 0.0,
            width: Length::Fill,
            id: None,
            _marker: PhantomData,
        }
    }

    /// Set spacing between items.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set the current scroll offset.
    pub fn scroll_offset(mut self, offset: f32) -> Self {
        self.scroll_offset = offset;
        self
    }

    /// Set the viewport height (for virtualization calculations).
    ///
    /// If not set, uses the constrained height during layout.
    pub fn viewport_height(mut self, height: f32) -> Self {
        self.viewport_height = height;
        self
    }

    /// Set the width of the list.
    pub fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    /// Set an ID for this list (for widget registration).
    pub fn id(mut self, id: SourceId) -> Self {
        self.id = Some(id);
        self
    }

    /// Compute the visible range and spacer heights.
    ///
    /// Returns `(first_index, last_index, top_spacer, bottom_spacer, total_height)`.
    fn compute_visible_range(&self, viewport_h: f32) -> (usize, usize, f32, f32, f32) {
        if self.items.is_empty() || viewport_h <= 0.0 {
            return (0, 0, 0.0, 0.0, 0.0);
        }

        // Compute all item heights and cumulative positions
        let mut heights: Vec<f32> = Vec::with_capacity(self.items.len());
        let mut total_height = 0.0f32;

        for item in self.items {
            let h = (self.item_height)(item);
            heights.push(h);
            total_height += h;
        }
        // Add spacing
        if self.items.len() > 1 {
            total_height += (self.items.len() - 1) as f32 * self.spacing;
        }

        // Find visible range
        let mut y = 0.0f32;
        let mut first = self.items.len();
        let mut last = self.items.len();
        let scroll_bottom = self.scroll_offset + viewport_h;

        for (i, &h) in heights.iter().enumerate() {
            let item_bottom = y + h;

            // First visible item: bottom edge past scroll top
            if first == self.items.len() && item_bottom > self.scroll_offset {
                first = i.saturating_sub(OVERSCAN);
            }

            // Last visible item: top edge past scroll bottom
            if first != self.items.len() && y > scroll_bottom {
                last = (i + OVERSCAN).min(self.items.len());
                break;
            }

            y += h + self.spacing;
        }

        let first = first.min(self.items.len());

        // Calculate spacer heights
        let top_spacer: f32 = heights[..first].iter().sum::<f32>()
            + if first > 0 { first as f32 * self.spacing } else { 0.0 };
        let bottom_spacer: f32 = heights[last..].iter().sum::<f32>()
            + if last < self.items.len() {
                (self.items.len() - last) as f32 * self.spacing
            } else {
                0.0
            };

        (first, last, top_spacer, bottom_spacer, total_height)
    }

    /// Build the list into a Column with spacers and visible items.
    pub fn build(self) -> Column<'a> {
        let viewport_h = if self.viewport_height > 0.0 {
            self.viewport_height
        } else {
            // Default to a reasonable viewport if not specified
            600.0
        };

        let (first, last, top_spacer, bottom_spacer, _total) =
            self.compute_visible_range(viewport_h);

        let mut col = Column::new().spacing(self.spacing).width(self.width);

        if let Some(id) = self.id {
            col = col.id(id);
        }

        // Top spacer for items above viewport
        if top_spacer > 0.0 {
            col = col.fixed_spacer(top_spacer);
        }

        // Only build visible items
        for (i, item) in self.items[first..last].iter().enumerate() {
            let child = (self.builder)(item, first + i);
            col = col.push(child);
        }

        // Bottom spacer for items below viewport
        if bottom_spacer > 0.0 {
            col = col.fixed_spacer(bottom_spacer);
        }

        col
    }

    /// Measure the total content height (for scroll container sizing).
    pub fn total_height(&self) -> f32 {
        if self.items.is_empty() {
            return 0.0;
        }

        let mut total = 0.0f32;
        for item in self.items {
            total += (self.item_height)(item);
        }
        total += (self.items.len() - 1) as f32 * self.spacing;
        total
    }

    /// Layout this list view using the constraint-based API.
    pub fn layout_with_constraints(
        self,
        ctx: &mut LayoutContext,
        constraints: LayoutConstraints,
        origin: Point,
    ) -> Size {
        // Use constraint height as viewport if not specified
        let viewport_h = if self.viewport_height > 0.0 {
            self.viewport_height
        } else {
            constraints.max_height
        };

        let (first, last, top_spacer, bottom_spacer, _total) =
            self.compute_visible_range(viewport_h);

        let mut col = Column::new().spacing(self.spacing).width(self.width);

        if let Some(id) = self.id {
            col = col.id(id);
        }

        if top_spacer > 0.0 {
            col = col.fixed_spacer(top_spacer);
        }

        for (i, item) in self.items[first..last].iter().enumerate() {
            let child = (self.builder)(item, first + i);
            col = col.push(child);
        }

        if bottom_spacer > 0.0 {
            col = col.fixed_spacer(bottom_spacer);
        }

        col.layout_with_constraints(ctx, constraints, origin)
    }
}
