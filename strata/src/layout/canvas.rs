//! Canvas Widget - Custom primitive drawing within the widget tree.
//!
//! Provides a widget that accepts a closure for drawing custom primitives.
//! This keeps drawing logic localized within the widget tree rather than
//! requiring post-layout rendering.
//!
//! # Example
//!
//! ```ignore
//! Column::new()
//!     .push(
//!         Canvas::new(|bounds, primitives| {
//!             primitives.add_rounded_rect(bounds, 8.0, Color::rgb(0.1, 0.1, 0.15));
//!             primitives.add_text("Custom content", bounds.top_left(), Color::WHITE, 14.0);
//!         })
//!         .width(Length::Fill)
//!         .height(Length::Fixed(100.0))
//!     )
//! ```

use crate::content_address::SourceId;
use crate::layout_snapshot::LayoutSnapshot;
use crate::primitives::{Rect, Size};

use super::length::Length;
use super::primitives::PrimitiveBatch;

/// Type alias for the draw function stored in Canvas.
pub type DrawFn<'a> = Box<dyn FnOnce(Rect, &mut PrimitiveBatch) + 'a>;

/// A widget that draws custom primitives via a closure.
///
/// The draw closure receives the computed bounds and a mutable reference
/// to the primitive batch. This allows arbitrary drawing while keeping
/// the logic localized to the widget's position in the tree.
///
/// ## Performance
///
/// The closure is boxed (one small heap allocation per Canvas), but this
/// happens during tree construction, not during layout. The layout/render
/// pass is just a function call with zero overhead.
pub struct Canvas<'a> {
    draw: DrawFn<'a>,
    width: Length,
    height: Length,
    id: Option<SourceId>,
}

impl<'a> Canvas<'a> {
    /// Create a new canvas with the given draw closure.
    ///
    /// The closure receives:
    /// - `bounds`: The computed layout bounds for this widget
    /// - `primitives`: Mutable reference to the primitive batch for drawing
    pub fn new<F>(draw: F) -> Self
    where
        F: FnOnce(Rect, &mut PrimitiveBatch) + 'a,
    {
        Canvas {
            draw: Box::new(draw),
            width: Length::Shrink,
            height: Length::Shrink,
            id: None,
        }
    }

    /// Set the width of this canvas.
    pub fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    /// Set the height of this canvas.
    pub fn height(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    /// Set an ID for this canvas (for hit-testing).
    pub fn id(mut self, id: SourceId) -> Self {
        self.id = Some(id);
        self
    }

    /// Measure the intrinsic size of this canvas.
    pub(crate) fn measure(&self) -> Size {
        let w = match self.width {
            Length::Fixed(px) => px,
            _ => 0.0,
        };
        let h = match self.height {
            Length::Fixed(px) => px,
            _ => 0.0,
        };
        Size::new(w, h)
    }

    /// Render this canvas at the given position with the given size.
    pub(crate) fn render(self, snapshot: &mut LayoutSnapshot, x: f32, y: f32, w: f32, h: f32) {
        let bounds = Rect::new(x, y, w, h);

        // Register widget for hit-testing if ID provided
        if let Some(id) = self.id {
            snapshot.register_widget(id, bounds);
        }

        // Call the draw closure
        (self.draw)(bounds, snapshot.primitives_mut());
    }

    /// Get the width length.
    pub(crate) fn width_length(&self) -> Length {
        self.width
    }

    /// Get the height length.
    pub(crate) fn height_length(&self) -> Length {
        self.height
    }
}
