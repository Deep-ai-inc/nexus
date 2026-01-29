//! Primitive Batch - Direct GPU Instance Access
//!
//! The fastest path for rendering. Primitives added here map 1:1 to GPU instances
//! with zero abstraction overhead. Use for backgrounds, decorations, canvas drawing.

use crate::strata::primitives::{Color, Point, Rect};

/// A batch of primitives ready for GPU rendering.
///
/// This is the "escape hatch" for when you need maximum performance.
/// Primitives added here bypass the widget system entirely.
#[derive(Debug, Default, Clone)]
pub struct PrimitiveBatch {
    /// Solid rectangles (rendered via white pixel trick).
    pub(crate) solid_rects: Vec<SolidRect>,

    /// Rounded rectangles (rendered via SDF).
    pub(crate) rounded_rects: Vec<RoundedRect>,

    /// Circles (rendered via SDF).
    pub(crate) circles: Vec<Circle>,

    /// Raw text runs (pre-positioned, for canvas-like drawing).
    pub(crate) text_runs: Vec<TextRun>,
}

/// A solid rectangle primitive.
#[derive(Debug, Clone, Copy)]
pub struct SolidRect {
    pub rect: Rect,
    pub color: Color,
}

/// A rounded rectangle primitive.
#[derive(Debug, Clone, Copy)]
pub struct RoundedRect {
    pub rect: Rect,
    pub corner_radius: f32,
    pub color: Color,
}

/// A circle primitive.
#[derive(Debug, Clone, Copy)]
pub struct Circle {
    pub center: Point,
    pub radius: f32,
    pub color: Color,
}

/// A pre-positioned text run (bypasses layout).
#[derive(Debug, Clone)]
pub struct TextRun {
    pub text: String,
    pub position: Point,
    pub color: Color,
    pub cache_key: Option<u64>,
}

impl PrimitiveBatch {
    /// Create an empty primitive batch.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all primitives.
    pub fn clear(&mut self) {
        self.solid_rects.clear();
        self.rounded_rects.clear();
        self.circles.clear();
        self.text_runs.clear();
    }

    /// Add a solid rectangle.
    #[inline]
    pub fn add_solid_rect(&mut self, rect: Rect, color: Color) -> &mut Self {
        self.solid_rects.push(SolidRect { rect, color });
        self
    }

    /// Add a rounded rectangle.
    #[inline]
    pub fn add_rounded_rect(&mut self, rect: Rect, corner_radius: f32, color: Color) -> &mut Self {
        self.rounded_rects.push(RoundedRect {
            rect,
            corner_radius,
            color,
        });
        self
    }

    /// Add a circle.
    #[inline]
    pub fn add_circle(&mut self, center: Point, radius: f32, color: Color) -> &mut Self {
        self.circles.push(Circle {
            center,
            radius,
            color,
        });
        self
    }

    /// Add a pre-positioned text run.
    ///
    /// Use `cache_key` if the text content is stable (e.g., hash of the string).
    /// This enables the text engine to skip reshaping if nothing changed.
    #[inline]
    pub fn add_text(&mut self, text: impl Into<String>, position: Point, color: Color) -> &mut Self {
        self.text_runs.push(TextRun {
            text: text.into(),
            position,
            color,
            cache_key: None,
        });
        self
    }

    /// Add text with an explicit cache key.
    ///
    /// The cache key should be stable across frames for unchanged content.
    /// Typically: `hash(source_id, content)` or a row/line ID.
    #[inline]
    pub fn add_text_cached(
        &mut self,
        text: impl Into<String>,
        position: Point,
        color: Color,
        cache_key: u64,
    ) -> &mut Self {
        self.text_runs.push(TextRun {
            text: text.into(),
            position,
            color,
            cache_key: Some(cache_key),
        });
        self
    }

    /// Check if the batch is empty.
    pub fn is_empty(&self) -> bool {
        self.solid_rects.is_empty()
            && self.rounded_rects.is_empty()
            && self.circles.is_empty()
            && self.text_runs.is_empty()
    }

    /// Total number of primitives.
    pub fn len(&self) -> usize {
        self.solid_rects.len()
            + self.rounded_rects.len()
            + self.circles.len()
            + self.text_runs.len()
    }
}
