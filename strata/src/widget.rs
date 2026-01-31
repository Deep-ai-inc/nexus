//! Widget System
//!
//! The `StrataWidget` trait provides a two-phase layout system optimized for
//! virtualized content (e.g., scrolling through millions of terminal lines).
//!
//! # Two-Phase Layout
//!
//! 1. **Measure phase** (`measure`): Returns the total size of the widget.
//!    For virtualized content, this should be O(1) - just `count * item_height`.
//!    No text shaping or expensive computation should happen here.
//!
//! 2. **Layout phase** (`layout`): Populates the `LayoutSnapshot` with character
//!    positions and bounds. Only called for visible content, so this is O(visible).
//!    Text shaping happens here.
//!
//! # Event Handling
//!
//! Widgets receive events through `event()` and can:
//! - Capture the pointer for drag operations (e.g., text selection)
//! - Emit messages to update application state
//!
//! # Rendering
//!
//! Widgets render to the GPU pipeline through `render()`. The pipeline handles
//! batching and instanced rendering for efficiency.

use crate::content_address::SourceId;
use crate::event_context::{Event, EventContext};
use crate::gpu::StrataPipeline;
use crate::layout_snapshot::LayoutSnapshot;
use crate::primitives::{Constraints, Rect, Size};

/// Result of handling an event.
#[derive(Debug, Clone)]
pub enum EventResult<M> {
    /// Event was ignored, propagate to parent.
    Ignored,

    /// Event was captured, don't propagate.
    Captured,

    /// Event produced a message for the application.
    Message(M),
}

impl<M> EventResult<M> {
    /// Check if the event was handled (captured or produced a message).
    pub fn is_handled(&self) -> bool {
        !matches!(self, EventResult::Ignored)
    }

    /// Convert to an Option<M>, discarding Ignored and Captured.
    pub fn into_message(self) -> Option<M> {
        match self {
            EventResult::Message(m) => Some(m),
            _ => None,
        }
    }
}

impl<M> From<Option<M>> for EventResult<M> {
    fn from(opt: Option<M>) -> Self {
        match opt {
            Some(m) => EventResult::Message(m),
            None => EventResult::Captured,
        }
    }
}

/// A Strata widget that can measure, layout, handle events, and render.
///
/// Widgets are the building blocks of Strata applications. They follow a
/// two-phase layout protocol optimized for virtualized scrolling content.
///
/// # Type Parameter
///
/// - `M`: The message type that events can produce.
pub trait StrataWidget<M> {
    /// Get the source ID for this widget (used for hit-testing and selection).
    fn source_id(&self) -> SourceId;

    /// Measure the widget to determine its size.
    ///
    /// This should be **O(1)** - for virtualized content, return the total
    /// scrollable size without iterating all items. For example:
    /// `Size::new(width, item_count * item_height)`
    ///
    /// No text shaping or expensive computation should happen here.
    fn measure(&self, constraints: Constraints) -> Size;

    /// Layout the visible portion of the widget.
    ///
    /// This populates the `LayoutSnapshot` with character positions and bounds
    /// for hit-testing and selection rendering. Only content within `bounds`
    /// needs to be laid out.
    ///
    /// This is **O(visible)** - text shaping and position calculation happens here.
    ///
    /// # Arguments
    ///
    /// - `snapshot`: The layout snapshot to populate with content addresses.
    /// - `bounds`: The screen-space bounds where this widget is rendered.
    fn layout(&mut self, snapshot: &mut LayoutSnapshot, bounds: Rect);

    /// Handle an event.
    ///
    /// The `EventContext` provides:
    /// - Access to the layout snapshot for hit-testing
    /// - Pointer capture for drag operations
    ///
    /// # Returns
    ///
    /// - `EventResult::Ignored`: Event not handled, propagate to parent
    /// - `EventResult::Captured`: Event handled, don't propagate
    /// - `EventResult::Message(m)`: Event produced a message
    fn event(&mut self, ctx: &EventContext, event: &Event) -> EventResult<M>;

    /// Render the widget to the GPU pipeline.
    ///
    /// The widget should add its glyphs/primitives to the pipeline.
    /// The pipeline handles batching for efficient rendering.
    ///
    /// # Arguments
    ///
    /// - `pipeline`: The GPU pipeline to render to.
    /// - `bounds`: The screen-space bounds for rendering.
    fn render(&self, pipeline: &mut StrataPipeline, bounds: Rect);
}

/// A boxed widget for dynamic dispatch.
pub type BoxedWidget<M> = Box<dyn StrataWidget<M>>;

/// Extension trait for convenient widget operations.
pub trait StrataWidgetExt<M>: StrataWidget<M> {
    /// Box this widget for dynamic dispatch.
    fn boxed(self) -> BoxedWidget<M>
    where
        Self: Sized + 'static,
    {
        Box::new(self)
    }
}

impl<M, W: StrataWidget<M>> StrataWidgetExt<M> for W {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_address::SourceId;
    use crate::primitives::Color;

    /// A simple test widget that renders a single line of text.
    struct TestWidget {
        source_id: SourceId,
        text: String,
        size: Size,
    }

    impl TestWidget {
        fn new(text: impl Into<String>) -> Self {
            let text = text.into();
            Self {
                source_id: SourceId::new(),
                text,
                size: Size::new(100.0, 20.0),
            }
        }
    }

    impl StrataWidget<()> for TestWidget {
        fn source_id(&self) -> SourceId {
            self.source_id
        }

        fn measure(&self, constraints: Constraints) -> Size {
            constraints.constrain(self.size)
        }

        fn layout(&mut self, _snapshot: &mut LayoutSnapshot, _bounds: Rect) {
            // Would register with snapshot here
        }

        fn event(&mut self, _ctx: &EventContext, _event: &Event) -> EventResult<()> {
            EventResult::Ignored
        }

        fn render(&self, pipeline: &mut StrataPipeline, bounds: Rect) {
            pipeline.add_text(&self.text, bounds.x, bounds.y, Color::WHITE, 14.0);
        }
    }

    #[test]
    fn test_widget_measure() {
        let widget = TestWidget::new("Hello");
        let size = widget.measure(Constraints::UNBOUNDED);
        assert_eq!(size, Size::new(100.0, 20.0));

        let constrained = widget.measure(Constraints::tight(Size::new(50.0, 10.0)));
        assert_eq!(constrained, Size::new(50.0, 10.0));
    }

    #[test]
    fn test_event_result() {
        let ignored: EventResult<i32> = EventResult::Ignored;
        assert!(!ignored.is_handled());
        assert!(ignored.into_message().is_none());

        let captured: EventResult<i32> = EventResult::Captured;
        assert!(captured.is_handled());
        assert!(captured.into_message().is_none());

        let message: EventResult<i32> = EventResult::Message(42);
        assert!(message.is_handled());
        assert_eq!(message.into_message(), Some(42));
    }
}
