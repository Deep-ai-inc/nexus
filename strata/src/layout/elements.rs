//! Leaf layout elements - TextElement, TerminalElement, ImageElement, ButtonElement.
//!
//! These are the "atoms" of the layout system - they don't contain other elements.

use unicode_width::UnicodeWidthChar;

use crate::content_address::SourceId;
use crate::gpu::ImageHandle;
use crate::layout_snapshot::CursorIcon;
use crate::primitives::{Color, Size};

use super::length::{Padding, CHAR_WIDTH, LINE_HEIGHT, BASE_FONT_SIZE};

// =========================================================================
// Helper Functions
// =========================================================================

/// Estimate display width in cell units (1 for Latin, 2 for CJK, 0 for combining marks).
pub(crate) fn unicode_display_width(text: &str) -> f32 {
    text.chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0) as f32)
        .sum()
}

/// Fast non-cryptographic hash for cache keys.
#[inline]
pub(crate) fn hash_text(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

// =========================================================================
// TextElement
// =========================================================================

/// A text element descriptor.
///
/// This is declarative - it doesn't compute layout until the container does.
/// The cache key is auto-computed from the text content by default, enabling
/// the text engine to skip reshaping when content hasn't changed.
pub struct TextElement {
    /// Source ID for hit-testing and selection.
    pub source_id: Option<SourceId>,
    /// Widget ID for click detection (makes text clickable as a widget).
    pub widget_id: Option<SourceId>,
    /// Cursor hint shown when hovering (requires widget_id).
    pub cursor_hint: Option<CursorIcon>,
    /// Text content.
    pub text: String,
    /// Text color.
    pub color: Color,
    /// Font size (if different from default).
    pub size: Option<f32>,
    /// Bold text style.
    pub bold: bool,
    /// Italic text style.
    pub italic: bool,
    /// Cache key for text shaping. Auto-computed from content by default.
    /// Override with `key()` for pre-computed keys on large strings.
    pub cache_key: u64,
    /// Measured size (filled during layout).
    measured_size: Option<Size>,
}

impl TextElement {
    /// Create a new text element.
    ///
    /// The cache key is automatically derived from the text content.
    /// For static strings, this means reshaping is skipped every frame.
    /// For dynamic strings, the key changes when content changes.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cache_key = hash_text(&text);
        Self {
            source_id: None,
            widget_id: None,
            cursor_hint: None,
            text,
            color: Color::WHITE,
            size: None,
            bold: false,
            italic: false,
            cache_key,
            measured_size: None,
        }
    }

    /// Set the source ID for hit-testing (makes text selectable content).
    pub fn source(mut self, source_id: SourceId) -> Self {
        self.source_id = Some(source_id);
        self
    }

    /// Set widget ID for click detection (makes text clickable as a widget).
    /// Can be combined with `source()` for text that is both selectable and clickable.
    pub fn widget_id(mut self, id: SourceId) -> Self {
        self.widget_id = Some(id);
        self
    }

    /// Set cursor hint shown when hovering (requires widget_id).
    pub fn cursor_hint(mut self, cursor: CursorIcon) -> Self {
        self.cursor_hint = Some(cursor);
        self
    }

    /// Set the text color.
    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Set the font size.
    pub fn size(mut self, size: f32) -> Self {
        self.size = Some(size);
        self
    }

    /// Set bold text style.
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    /// Set italic text style.
    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    /// Override the cache key with an explicit value.
    ///
    /// Use this for performance-critical cases where hashing the text
    /// content is too expensive (e.g., very large strings), or when you
    /// have a stable external identifier (e.g., a row ID).
    pub fn key(mut self, key: u64) -> Self {
        self.cache_key = key;
        self
    }

    /// Estimate size for layout (uses character count heuristic).
    ///
    /// Scales metrics proportionally when a non-default font size is set.
    /// JetBrains Mono scales linearly, so this is a good approximation.
    pub(crate) fn estimate_size(&self, default_char_width: f32, default_line_height: f32) -> Size {
        if let Some(size) = self.measured_size {
            return size;
        }
        let (cw, lh) = if let Some(fs) = self.size {
            let scale = fs / BASE_FONT_SIZE;
            (default_char_width * scale, default_line_height * scale)
        } else {
            (default_char_width, default_line_height)
        };
        let char_count = unicode_display_width(&self.text);
        Size::new(char_count * cw, lh)
    }

    /// Get the effective font size for this element.
    pub(crate) fn font_size(&self) -> f32 {
        self.size.unwrap_or(BASE_FONT_SIZE)
    }
}

// =========================================================================
// TerminalElement
// =========================================================================

/// A terminal/grid element descriptor.
pub struct TerminalElement {
    /// Source ID for hit-testing.
    pub source_id: SourceId,
    /// Grid dimensions.
    pub cols: u16,
    pub rows: u16,
    /// Cell metrics.
    pub cell_width: f32,
    pub cell_height: f32,
    /// Content buffer reference (zero-copy binding).
    /// The actual content is in the app state, we just point to it.
    pub(crate) content_hash: u64,
    /// Row content for rendering — each row is a list of styled runs.
    pub(crate) row_content: Vec<Vec<crate::layout_snapshot::TextRun>>,
}

impl TerminalElement {
    /// Create a new terminal element.
    pub fn new(source_id: SourceId, cols: u16, rows: u16) -> Self {
        Self {
            source_id,
            cols,
            rows,
            cell_width: 8.4,
            cell_height: 18.0,
            content_hash: 0,
            row_content: Vec::new(),
        }
    }

    /// Add a row of styled text runs.
    pub fn row(mut self, runs: Vec<crate::layout_snapshot::TextRun>) -> Self {
        self.row_content.push(runs);
        self
    }

    /// Set cell metrics.
    pub fn cell_size(mut self, width: f32, height: f32) -> Self {
        self.cell_width = width;
        self.cell_height = height;
        self
    }

    /// Set content hash for cache invalidation.
    pub fn content_hash(mut self, hash: u64) -> Self {
        self.content_hash = hash;
        self
    }

    /// Calculate size based on grid dimensions.
    pub(crate) fn size(&self) -> Size {
        Size::new(
            self.cols as f32 * self.cell_width,
            self.rows as f32 * self.cell_height,
        )
    }
}

// =========================================================================
// ImageElement
// =========================================================================

/// An image element descriptor.
pub struct ImageElement {
    /// Image handle from the pipeline.
    pub handle: ImageHandle,
    /// Display width in logical pixels.
    pub width: f32,
    /// Display height in logical pixels.
    pub height: f32,
    /// Corner radius for rounded clipping.
    pub corner_radius: f32,
    /// Tint color (Color::WHITE = no tint).
    pub tint: Color,
    /// Optional widget ID for hit testing (makes image clickable/draggable).
    pub widget_id: Option<SourceId>,
    /// Cursor hint shown when hovering over the image.
    pub cursor_hint: Option<CursorIcon>,
}

impl ImageElement {
    /// Create a new image element with explicit size.
    pub fn new(handle: ImageHandle, width: f32, height: f32) -> Self {
        Self {
            handle,
            width,
            height,
            corner_radius: 0.0,
            tint: Color::WHITE,
            widget_id: None,
            cursor_hint: None,
        }
    }

    /// Set corner radius for rounded clipping.
    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Set tint color (multiplied with image color).
    pub fn tint(mut self, tint: Color) -> Self {
        self.tint = tint;
        self
    }

    /// Set a widget ID for hit testing (makes the image clickable/draggable).
    pub fn widget_id(mut self, id: SourceId) -> Self {
        self.widget_id = Some(id);
        self
    }

    /// Set the cursor icon shown when hovering over this image.
    pub fn cursor(mut self, cursor: CursorIcon) -> Self {
        self.cursor_hint = Some(cursor);
        self
    }

    pub(crate) fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

// =========================================================================
// ButtonElement
// =========================================================================

/// A button element descriptor.
///
/// Renders a padded text label with background and corner radius.
/// Auto-registers as a widget hit target for click detection via `on_mouse`.
pub struct ButtonElement {
    /// Widget ID for hit-testing (required).
    pub id: SourceId,
    /// Button label text.
    pub label: String,
    /// Text color.
    pub text_color: Color,
    /// Background color.
    pub background: Color,
    /// Corner radius.
    pub corner_radius: f32,
    /// Padding around the label.
    pub padding: Padding,
    /// Cache key for text rendering.
    pub(crate) cache_key: u64,
}

impl ButtonElement {
    pub fn new(id: SourceId, label: impl Into<String>) -> Self {
        let label = label.into();
        let cache_key = hash_text(&label);
        Self {
            id,
            label,
            text_color: Color::WHITE,
            background: Color::rgba(0.3, 0.3, 0.4, 1.0),
            corner_radius: 4.0,
            padding: Padding::new(3.0, 14.0, 3.0, 14.0),
            cache_key,
        }
    }

    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    pub fn background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.corner_radius = radius;
        self
    }

    pub fn padding(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }

    pub(crate) fn estimate_size(&self) -> Size {
        let char_count = self.label.chars().count() as f32;
        Size::new(
            char_count * CHAR_WIDTH + self.padding.horizontal(),
            LINE_HEIGHT + self.padding.vertical(),
        )
    }
}
