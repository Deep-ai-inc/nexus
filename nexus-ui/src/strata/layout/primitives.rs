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

    /// Line segments (rendered as rotated quads).
    pub(crate) lines: Vec<LineSegment>,

    /// Polylines (series of connected line segments).
    pub(crate) polylines: Vec<Polyline>,

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

/// Line rendering style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineStyle {
    /// Solid line (default).
    #[default]
    Solid,
    /// Dashed line (repeating dash-gap pattern).
    Dashed,
    /// Dotted line (repeating dot-gap pattern).
    Dotted,
}

/// A line segment primitive (rendered as a rotated quad in the shader).
#[derive(Debug, Clone, Copy)]
pub struct LineSegment {
    pub p1: Point,
    pub p2: Point,
    pub thickness: f32,
    pub color: Color,
    pub style: LineStyle,
}

/// A polyline primitive (series of connected line segments).
#[derive(Debug, Clone)]
pub struct Polyline {
    pub points: Vec<Point>,
    pub thickness: f32,
    pub color: Color,
    pub style: LineStyle,
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
        self.lines.clear();
        self.polylines.clear();
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

    /// Add a solid line segment.
    #[inline]
    pub fn add_line(&mut self, p1: Point, p2: Point, thickness: f32, color: Color) -> &mut Self {
        self.lines.push(LineSegment { p1, p2, thickness, color, style: LineStyle::Solid });
        self
    }

    /// Add a styled line segment (solid, dashed, or dotted).
    #[inline]
    pub fn add_line_styled(&mut self, p1: Point, p2: Point, thickness: f32, color: Color, style: LineStyle) -> &mut Self {
        self.lines.push(LineSegment { p1, p2, thickness, color, style });
        self
    }

    /// Add a solid polyline (series of connected line segments).
    ///
    /// For N points, renders N-1 line segments. Efficient for charts and graphs.
    #[inline]
    pub fn add_polyline(&mut self, points: Vec<Point>, thickness: f32, color: Color) -> &mut Self {
        if points.len() >= 2 {
            self.polylines.push(Polyline { points, thickness, color, style: LineStyle::Solid });
        }
        self
    }

    /// Add a styled polyline (solid, dashed, or dotted).
    #[inline]
    pub fn add_polyline_styled(&mut self, points: Vec<Point>, thickness: f32, color: Color, style: LineStyle) -> &mut Self {
        if points.len() >= 2 {
            self.polylines.push(Polyline { points, thickness, color, style });
        }
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
            && self.lines.is_empty()
            && self.polylines.is_empty()
            && self.text_runs.is_empty()
    }

    /// Total number of primitives.
    pub fn len(&self) -> usize {
        self.solid_rects.len()
            + self.rounded_rects.len()
            + self.circles.len()
            + self.lines.len()
            + self.polylines.len()
            + self.text_runs.len()
    }
}
