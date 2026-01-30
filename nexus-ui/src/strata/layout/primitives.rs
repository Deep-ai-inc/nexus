//! Primitive Batch - Direct GPU Instance Access
//!
//! The fastest path for rendering. Primitives added here map 1:1 to GPU instances
//! with zero abstraction overhead. Use for backgrounds, decorations, canvas drawing.

use crate::strata::gpu::ImageHandle;
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

    /// Borders (hollow rounded rects via SDF ring).
    pub(crate) borders: Vec<Border>,

    /// Drop shadows (soft SDF blur).
    pub(crate) shadows: Vec<Shadow>,

    /// Images (rendered via image atlas, Mode 4).
    pub(crate) images: Vec<ImagePrimitive>,

    /// Clip stack for nested container clipping.
    /// Each entry is a clip rect; the effective clip is the intersection of all.
    clip_stack: Vec<Rect>,
}

/// A solid rectangle primitive.
#[derive(Debug, Clone, Copy)]
pub struct SolidRect {
    pub rect: Rect,
    pub color: Color,
    pub clip_rect: Option<Rect>,
}

/// A rounded rectangle primitive.
#[derive(Debug, Clone, Copy)]
pub struct RoundedRect {
    pub rect: Rect,
    pub corner_radius: f32,
    pub color: Color,
    pub clip_rect: Option<Rect>,
}

/// A circle primitive.
#[derive(Debug, Clone, Copy)]
pub struct Circle {
    pub center: Point,
    pub radius: f32,
    pub color: Color,
    pub clip_rect: Option<Rect>,
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
    pub clip_rect: Option<Rect>,
}

/// A polyline primitive (series of connected line segments).
#[derive(Debug, Clone)]
pub struct Polyline {
    pub points: Vec<Point>,
    pub thickness: f32,
    pub color: Color,
    pub style: LineStyle,
    pub clip_rect: Option<Rect>,
}

/// A pre-positioned text run (bypasses layout).
#[derive(Debug, Clone)]
pub struct TextRun {
    pub text: String,
    pub position: Point,
    pub color: Color,
    pub cache_key: Option<u64>,
    pub clip_rect: Option<Rect>,
}

/// A border/outline primitive (hollow rounded rect via SDF ring).
#[derive(Debug, Clone, Copy)]
pub struct Border {
    pub rect: Rect,
    pub corner_radius: f32,
    pub border_width: f32,
    pub color: Color,
    pub clip_rect: Option<Rect>,
}

/// A drop shadow primitive (soft SDF-based blur).
#[derive(Debug, Clone, Copy)]
pub struct Shadow {
    pub rect: Rect,
    pub corner_radius: f32,
    pub blur_radius: f32,
    pub color: Color,
    pub clip_rect: Option<Rect>,
}

/// An image primitive (rendered via Mode 4 from the image atlas).
#[derive(Debug, Clone, Copy)]
pub struct ImagePrimitive {
    pub rect: Rect,
    pub handle: ImageHandle,
    pub corner_radius: f32,
    pub tint: Color,
    pub clip_rect: Option<Rect>,
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
        self.borders.clear();
        self.shadows.clear();
        self.images.clear();
        self.clip_stack.clear();
    }

    // =========================================================================
    // Clip stack
    // =========================================================================

    /// Push a clip rectangle. All subsequently added primitives will be clipped
    /// to the intersection of all active clip rects.
    pub fn push_clip(&mut self, rect: Rect) {
        self.clip_stack.push(rect);
    }

    /// Pop the most recent clip rectangle.
    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
    }

    /// Get the current effective clip rect (public, for non-primitive paths).
    #[inline]
    pub fn current_clip_public(&self) -> Option<Rect> {
        self.current_clip()
    }

    /// Sentinel clip rect that activates the shader's clip check (z > 0)
    /// but clips everything (off-screen, tiny).
    const CLIP_EVERYTHING: Rect = Rect { x: -1.0, y: -1.0, width: 0.001, height: 0.001 };

    /// Get the current effective clip rect (intersection of all stack entries).
    /// Returns `None` if no clip is active.
    #[inline]
    fn current_clip(&self) -> Option<Rect> {
        if self.clip_stack.is_empty() {
            return None;
        }
        let mut clip = self.clip_stack[0];
        for r in &self.clip_stack[1..] {
            match clip.intersection(r) {
                Some(c) => clip = c,
                // Empty intersection: clip everything.
                None => return Some(Self::CLIP_EVERYTHING),
            }
        }
        // Zero/negative dimensions would bypass the shader's `z > 0` check,
        // so treat them as "clip everything".
        if clip.width <= 0.0 || clip.height <= 0.0 {
            return Some(Self::CLIP_EVERYTHING);
        }
        Some(clip)
    }

    // =========================================================================
    // Primitive add methods
    // =========================================================================

    /// Add a solid rectangle.
    #[inline]
    pub fn add_solid_rect(&mut self, rect: Rect, color: Color) -> &mut Self {
        let clip_rect = self.current_clip();
        self.solid_rects.push(SolidRect { rect, color, clip_rect });
        self
    }

    /// Add a rounded rectangle.
    #[inline]
    pub fn add_rounded_rect(&mut self, rect: Rect, corner_radius: f32, color: Color) -> &mut Self {
        let clip_rect = self.current_clip();
        self.rounded_rects.push(RoundedRect {
            rect,
            corner_radius,
            color,
            clip_rect,
        });
        self
    }

    /// Add a circle.
    #[inline]
    pub fn add_circle(&mut self, center: Point, radius: f32, color: Color) -> &mut Self {
        let clip_rect = self.current_clip();
        self.circles.push(Circle {
            center,
            radius,
            color,
            clip_rect,
        });
        self
    }

    /// Add a solid line segment.
    #[inline]
    pub fn add_line(&mut self, p1: Point, p2: Point, thickness: f32, color: Color) -> &mut Self {
        let clip_rect = self.current_clip();
        self.lines.push(LineSegment { p1, p2, thickness, color, style: LineStyle::Solid, clip_rect });
        self
    }

    /// Add a styled line segment (solid, dashed, or dotted).
    #[inline]
    pub fn add_line_styled(&mut self, p1: Point, p2: Point, thickness: f32, color: Color, style: LineStyle) -> &mut Self {
        let clip_rect = self.current_clip();
        self.lines.push(LineSegment { p1, p2, thickness, color, style, clip_rect });
        self
    }

    /// Add a solid polyline (series of connected line segments).
    ///
    /// For N points, renders N-1 line segments. Efficient for charts and graphs.
    #[inline]
    pub fn add_polyline(&mut self, points: Vec<Point>, thickness: f32, color: Color) -> &mut Self {
        if points.len() >= 2 {
            let clip_rect = self.current_clip();
            self.polylines.push(Polyline { points, thickness, color, style: LineStyle::Solid, clip_rect });
        }
        self
    }

    /// Add a styled polyline (solid, dashed, or dotted).
    #[inline]
    pub fn add_polyline_styled(&mut self, points: Vec<Point>, thickness: f32, color: Color, style: LineStyle) -> &mut Self {
        if points.len() >= 2 {
            let clip_rect = self.current_clip();
            self.polylines.push(Polyline { points, thickness, color, style, clip_rect });
        }
        self
    }

    /// Add a pre-positioned text run.
    ///
    /// Use `cache_key` if the text content is stable (e.g., hash of the string).
    /// This enables the text engine to skip reshaping if nothing changed.
    #[inline]
    pub fn add_text(&mut self, text: impl Into<String>, position: Point, color: Color) -> &mut Self {
        let clip_rect = self.current_clip();
        self.text_runs.push(TextRun {
            text: text.into(),
            position,
            color,
            cache_key: None,
            clip_rect,
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
        let clip_rect = self.current_clip();
        self.text_runs.push(TextRun {
            text: text.into(),
            position,
            color,
            cache_key: Some(cache_key),
            clip_rect,
        });
        self
    }

    /// Add a border/outline (hollow rounded rect).
    #[inline]
    pub fn add_border(
        &mut self,
        rect: Rect,
        corner_radius: f32,
        border_width: f32,
        color: Color,
    ) -> &mut Self {
        let clip_rect = self.current_clip();
        self.borders.push(Border {
            rect,
            corner_radius,
            border_width,
            color,
            clip_rect,
        });
        self
    }

    /// Add a drop shadow.
    ///
    /// Draw shadows BEFORE the content they shadow for correct layering.
    #[inline]
    pub fn add_shadow(
        &mut self,
        rect: Rect,
        corner_radius: f32,
        blur_radius: f32,
        color: Color,
    ) -> &mut Self {
        let clip_rect = self.current_clip();
        self.shadows.push(Shadow {
            rect,
            corner_radius,
            blur_radius,
            color,
            clip_rect,
        });
        self
    }

    /// Add an image.
    #[inline]
    pub fn add_image(
        &mut self,
        rect: Rect,
        handle: ImageHandle,
        corner_radius: f32,
        tint: Color,
    ) -> &mut Self {
        let clip_rect = self.current_clip();
        self.images.push(ImagePrimitive {
            rect,
            handle,
            corner_radius,
            tint,
            clip_rect,
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
            && self.borders.is_empty()
            && self.shadows.is_empty()
            && self.images.is_empty()
    }

    /// Total number of primitives.
    pub fn len(&self) -> usize {
        self.solid_rects.len()
            + self.rounded_rects.len()
            + self.circles.len()
            + self.lines.len()
            + self.polylines.len()
            + self.text_runs.len()
            + self.borders.len()
            + self.shadows.len()
            + self.images.len()
    }
}
