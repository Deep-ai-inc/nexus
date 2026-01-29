//! Text Widget
//!
//! A widget for rendering text with cosmic-text shaping.
//! Supports accurate hit-testing for text selection.

use std::cell::RefCell;

use crate::strata::content_address::SourceId;
use crate::strata::event_context::{Event, EventContext, MouseButton, MouseEvent};
use crate::strata::gpu::StrataPipeline;
use crate::strata::layout_snapshot::{LayoutSnapshot, SourceLayout, TextLayout};
use crate::strata::primitives::{Color, Constraints, Rect, Size};
use crate::strata::text_engine::{ShapedText, TextAttrs, TextEngine};
use crate::strata::widget::{EventResult, StrataWidget};

/// Messages that a TextWidget can produce.
#[derive(Debug, Clone)]
pub enum TextMessage {
    /// User clicked at a cursor position within the text.
    Clicked { cursor_position: usize },
}

/// A widget that renders text with accurate character positioning.
///
/// Uses cosmic-text for text shaping, which provides accurate character
/// positions for hit-testing and selection.
pub struct TextWidget {
    /// Unique source ID for this widget instance.
    source_id: SourceId,

    /// The text content (owned for cache compatibility).
    text: String,

    /// Text attributes (font, size, color).
    attrs: TextAttrs,

    /// Cached shaped text (populated during layout).
    shaped: RefCell<Option<ShapedText>>,

    /// Reference to the shared text engine.
    engine: RefCell<TextEngine>,
}

impl TextWidget {
    /// Create a new text widget with default attributes.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            source_id: SourceId::new(),
            text: text.into(),
            attrs: TextAttrs::default(),
            shaped: RefCell::new(None),
            engine: RefCell::new(TextEngine::new()),
        }
    }

    /// Create a new text widget with a specific source ID.
    pub fn with_source_id(source_id: SourceId, text: impl Into<String>) -> Self {
        Self {
            source_id,
            text: text.into(),
            attrs: TextAttrs::default(),
            shaped: RefCell::new(None),
            engine: RefCell::new(TextEngine::new()),
        }
    }

    /// Set the text color.
    pub fn color(mut self, color: Color) -> Self {
        self.attrs.color = color;
        self
    }

    /// Set the font size.
    pub fn font_size(mut self, size: f32) -> Self {
        self.attrs.font_size = size;
        self
    }

    /// Set the line height.
    pub fn line_height(mut self, height: f32) -> Self {
        self.attrs.line_height = height;
        self
    }

    /// Set the text attributes.
    pub fn attrs(mut self, attrs: TextAttrs) -> Self {
        self.attrs = attrs;
        self
    }

    /// Get the text content.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Set new text content.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        // Invalidate cached shape
        *self.shaped.borrow_mut() = None;
    }

    /// Ensure text is shaped and return the shaped result.
    fn ensure_shaped(&self) -> ShapedText {
        let mut shaped_ref = self.shaped.borrow_mut();
        if shaped_ref.is_none() {
            let mut engine = self.engine.borrow_mut();
            let shaped = engine.shape(self.text.clone(), &self.attrs);
            *shaped_ref = Some(shaped);
        }
        shaped_ref.clone().unwrap()
    }
}

impl StrataWidget<TextMessage> for TextWidget {
    fn source_id(&self) -> SourceId {
        self.source_id
    }

    fn measure(&self, constraints: Constraints) -> Size {
        // Shape text to get accurate dimensions
        let shaped = self.ensure_shaped();
        constraints.constrain(Size::new(shaped.width, shaped.height))
    }

    fn layout(&mut self, snapshot: &mut LayoutSnapshot, bounds: Rect) {
        // Shape text and register with snapshot
        let shaped = self.ensure_shaped();
        let text_layout = TextLayout::from_shaped(&shaped, bounds.x, bounds.y);
        snapshot.register_source(self.source_id, SourceLayout::text(text_layout));
    }

    fn event(&mut self, ctx: &EventContext, event: &Event) -> EventResult<TextMessage> {
        match event {
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                position,
            }) => {
                // Hit-test to find cursor position
                if let Some(crate::strata::layout_snapshot::HitResult::Content(addr)) = ctx.layout.hit_test(*position) {
                    if addr.source_id == self.source_id {
                        return EventResult::Message(TextMessage::Clicked {
                            cursor_position: addr.content_offset,
                        });
                    }
                }
                EventResult::Ignored
            }
            _ => EventResult::Ignored,
        }
    }

    fn render(&self, pipeline: &mut StrataPipeline, bounds: Rect) {
        pipeline.add_text(&self.text, bounds.x, bounds.y, self.attrs.color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_widget_creation() {
        let widget = TextWidget::new("Hello, World!")
            .color(Color::WHITE)
            .font_size(16.0);

        assert_eq!(widget.text(), "Hello, World!");
        assert_eq!(widget.attrs.font_size, 16.0);
    }

    #[test]
    fn test_text_widget_measure() {
        let widget = TextWidget::new("Hello");
        let size = widget.measure(Constraints::UNBOUNDED);

        // Should have non-zero dimensions
        assert!(size.width > 0.0);
        assert!(size.height > 0.0);
    }
}
