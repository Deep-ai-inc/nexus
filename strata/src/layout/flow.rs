//! Flow Container - CSS flex-wrap style wrapping layout.
//!
//! Children are laid out horizontally until they exceed the container width,
//! then wrap to the next line. Supports text, images, and buttons.
//! Reflows automatically on container resize.

use crate::content_address::SourceId;
use crate::layout_snapshot::LayoutSnapshot;
use crate::primitives::{Rect, Size};

use super::child::LayoutChild;
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
pub struct FlowContainer {
    /// Child elements.
    children: Vec<LayoutChild>,
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
}

impl FlowContainer {
    /// Create a new flow container.
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            source_id: None,
            spacing: 0.0,
            line_spacing: 2.0,
            padding: Padding::default(),
            width: Length::Fill,
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
    pub fn push(mut self, child: impl Into<LayoutChild>) -> Self {
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

    /// Layout and render into the snapshot.
    pub fn layout(&self, snapshot: &mut LayoutSnapshot, x: f32, y: f32, available_width: f32) {
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
}

impl Default for FlowContainer {
    fn default() -> Self {
        Self::new()
    }
}

// =========================================================================
// Flow Child Rendering
// =========================================================================

/// Render a single child in a flow container.
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
}
