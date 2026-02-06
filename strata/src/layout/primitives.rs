//! Primitive Batch - Direct GPU Instance Access
//!
//! The fastest path for rendering. Primitives added here map 1:1 to GPU instances
//! with zero abstraction overhead. Use for backgrounds, decorations, canvas drawing.

use crate::gpu::ImageHandle;
use crate::primitives::{Color, Point, Rect};

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
    pub font_size: f32,
    pub cache_key: Option<u64>,
    pub clip_rect: Option<Rect>,
    pub bold: bool,
    pub italic: bool,
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

    /// Get the current clip bounds for viewport culling.
    /// Returns the effective clip rect if any clip is active.
    #[inline]
    pub fn current_clip_bounds(&self) -> Option<Rect> {
        self.current_clip()
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

    /// Add a pre-positioned text run at the given font size.
    ///
    /// Use `cache_key` if the text content is stable (e.g., hash of the string).
    /// This enables the text engine to skip reshaping if nothing changed.
    #[inline]
    pub fn add_text(&mut self, text: impl Into<String>, position: Point, color: Color, font_size: f32) -> &mut Self {
        let clip_rect = self.current_clip();
        self.text_runs.push(TextRun {
            text: text.into(),
            position,
            color,
            font_size,
            cache_key: None,
            clip_rect,
            bold: false,
            italic: false,
        });
        self
    }

    /// Add a styled text run with bold/italic support.
    #[inline]
    pub fn add_text_styled(
        &mut self,
        text: impl Into<String>,
        position: Point,
        color: Color,
        font_size: f32,
        bold: bool,
        italic: bool,
    ) -> &mut Self {
        let clip_rect = self.current_clip();
        self.text_runs.push(TextRun {
            text: text.into(),
            position,
            color,
            font_size,
            cache_key: None,
            clip_rect,
            bold,
            italic,
        });
        self
    }

    /// Add text with an explicit cache key at the given font size.
    ///
    /// The cache key should be stable across frames for unchanged content.
    /// Typically: `hash(source_id, content)` or a row/line ID.
    #[inline]
    pub fn add_text_cached(
        &mut self,
        text: impl Into<String>,
        position: Point,
        color: Color,
        font_size: f32,
        cache_key: u64,
    ) -> &mut Self {
        self.add_text_cached_styled(text, position, color, font_size, cache_key, false, false)
    }

    /// Add text with an explicit cache key and bold/italic styling.
    #[inline]
    pub fn add_text_cached_styled(
        &mut self,
        text: impl Into<String>,
        position: Point,
        color: Color,
        font_size: f32,
        cache_key: u64,
        bold: bool,
        italic: bool,
    ) -> &mut Self {
        let clip_rect = self.current_clip();
        self.text_runs.push(TextRun {
            text: text.into(),
            position,
            color,
            font_size,
            cache_key: Some(cache_key),
            clip_rect,
            bold,
            italic,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn white() -> Color {
        Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }
    }

    fn red() -> Color {
        Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }
    }

    // =========================================================================
    // Basic batch operations
    // =========================================================================

    #[test]
    fn test_new_creates_empty_batch() {
        let batch = PrimitiveBatch::new();
        assert!(batch.is_empty());
        assert_eq!(batch.len(), 0);
    }

    #[test]
    fn test_default_creates_empty_batch() {
        let batch = PrimitiveBatch::default();
        assert!(batch.is_empty());
        assert_eq!(batch.len(), 0);
    }

    #[test]
    fn test_clear_resets_batch() {
        let mut batch = PrimitiveBatch::new();
        batch.add_solid_rect(Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 }, white());
        batch.add_circle(Point { x: 5.0, y: 5.0 }, 3.0, red());
        batch.push_clip(Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 });

        assert!(!batch.is_empty());
        assert_eq!(batch.len(), 2);

        batch.clear();

        assert!(batch.is_empty());
        assert_eq!(batch.len(), 0);
        assert!(batch.current_clip_public().is_none());
    }

    // =========================================================================
    // Clip stack
    // =========================================================================

    #[test]
    fn test_clip_stack_empty_returns_none() {
        let batch = PrimitiveBatch::new();
        assert!(batch.current_clip_public().is_none());
        assert!(batch.current_clip_bounds().is_none());
    }

    #[test]
    fn test_push_clip_single() {
        let mut batch = PrimitiveBatch::new();
        let clip = Rect { x: 10.0, y: 20.0, width: 100.0, height: 50.0 };
        batch.push_clip(clip);

        let result = batch.current_clip_public().unwrap();
        assert_eq!(result.x, 10.0);
        assert_eq!(result.y, 20.0);
        assert_eq!(result.width, 100.0);
        assert_eq!(result.height, 50.0);
    }

    #[test]
    fn test_push_clip_intersection() {
        let mut batch = PrimitiveBatch::new();
        // First clip: 0,0 to 100,100
        batch.push_clip(Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 });
        // Second clip: 50,50 to 150,150 — intersection should be 50,50 to 100,100
        batch.push_clip(Rect { x: 50.0, y: 50.0, width: 100.0, height: 100.0 });

        let result = batch.current_clip_public().unwrap();
        assert_eq!(result.x, 50.0);
        assert_eq!(result.y, 50.0);
        assert_eq!(result.width, 50.0);
        assert_eq!(result.height, 50.0);
    }

    #[test]
    fn test_pop_clip_restores_previous() {
        let mut batch = PrimitiveBatch::new();
        let clip1 = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        let clip2 = Rect { x: 50.0, y: 50.0, width: 100.0, height: 100.0 };

        batch.push_clip(clip1);
        batch.push_clip(clip2);
        batch.pop_clip();

        let result = batch.current_clip_public().unwrap();
        assert_eq!(result.x, 0.0);
        assert_eq!(result.width, 100.0);
    }

    #[test]
    fn test_pop_clip_to_empty() {
        let mut batch = PrimitiveBatch::new();
        batch.push_clip(Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 });
        batch.pop_clip();

        assert!(batch.current_clip_public().is_none());
    }

    #[test]
    fn test_clip_non_intersecting_returns_sentinel() {
        let mut batch = PrimitiveBatch::new();
        // Two clips that don't overlap
        batch.push_clip(Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 });
        batch.push_clip(Rect { x: 100.0, y: 100.0, width: 10.0, height: 10.0 });

        // Should return the CLIP_EVERYTHING sentinel
        let result = batch.current_clip_public().unwrap();
        assert!(result.width < 1.0); // sentinel has tiny dimensions
    }

    // =========================================================================
    // Solid rects
    // =========================================================================

    #[test]
    fn test_add_solid_rect() {
        let mut batch = PrimitiveBatch::new();
        let rect = Rect { x: 10.0, y: 20.0, width: 30.0, height: 40.0 };

        batch.add_solid_rect(rect, white());

        assert_eq!(batch.len(), 1);
        assert!(!batch.is_empty());
        assert_eq!(batch.solid_rects.len(), 1);
        assert_eq!(batch.solid_rects[0].rect.x, 10.0);
        assert!(batch.solid_rects[0].clip_rect.is_none());
    }

    #[test]
    fn test_add_solid_rect_with_clip() {
        let mut batch = PrimitiveBatch::new();
        let clip = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        batch.push_clip(clip);

        batch.add_solid_rect(Rect { x: 10.0, y: 10.0, width: 20.0, height: 20.0 }, red());

        assert!(batch.solid_rects[0].clip_rect.is_some());
    }

    #[test]
    fn test_add_solid_rect_returns_self() {
        let mut batch = PrimitiveBatch::new();
        batch
            .add_solid_rect(Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 }, white())
            .add_solid_rect(Rect { x: 20.0, y: 0.0, width: 10.0, height: 10.0 }, red());

        assert_eq!(batch.solid_rects.len(), 2);
    }

    // =========================================================================
    // Rounded rects
    // =========================================================================

    #[test]
    fn test_add_rounded_rect() {
        let mut batch = PrimitiveBatch::new();
        batch.add_rounded_rect(Rect { x: 0.0, y: 0.0, width: 50.0, height: 30.0 }, 5.0, white());

        assert_eq!(batch.rounded_rects.len(), 1);
        assert_eq!(batch.rounded_rects[0].corner_radius, 5.0);
        assert_eq!(batch.len(), 1);
    }

    // =========================================================================
    // Circles
    // =========================================================================

    #[test]
    fn test_add_circle() {
        let mut batch = PrimitiveBatch::new();
        batch.add_circle(Point { x: 50.0, y: 50.0 }, 25.0, red());

        assert_eq!(batch.circles.len(), 1);
        assert_eq!(batch.circles[0].center.x, 50.0);
        assert_eq!(batch.circles[0].radius, 25.0);
    }

    // =========================================================================
    // Lines
    // =========================================================================

    #[test]
    fn test_add_line() {
        let mut batch = PrimitiveBatch::new();
        batch.add_line(
            Point { x: 0.0, y: 0.0 },
            Point { x: 100.0, y: 100.0 },
            2.0,
            white(),
        );

        assert_eq!(batch.lines.len(), 1);
        assert_eq!(batch.lines[0].thickness, 2.0);
        assert_eq!(batch.lines[0].style, LineStyle::Solid);
    }

    #[test]
    fn test_add_line_styled() {
        let mut batch = PrimitiveBatch::new();
        batch.add_line_styled(
            Point { x: 0.0, y: 0.0 },
            Point { x: 50.0, y: 50.0 },
            1.0,
            red(),
            LineStyle::Dashed,
        );

        assert_eq!(batch.lines[0].style, LineStyle::Dashed);
    }

    // =========================================================================
    // Polylines
    // =========================================================================

    #[test]
    fn test_add_polyline() {
        let mut batch = PrimitiveBatch::new();
        let points = vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 10.0, y: 20.0 },
            Point { x: 20.0, y: 10.0 },
        ];
        batch.add_polyline(points, 2.0, white());

        assert_eq!(batch.polylines.len(), 1);
        assert_eq!(batch.polylines[0].points.len(), 3);
    }

    #[test]
    fn test_add_polyline_requires_at_least_two_points() {
        let mut batch = PrimitiveBatch::new();

        // Single point should not add a polyline
        batch.add_polyline(vec![Point { x: 0.0, y: 0.0 }], 1.0, white());
        assert_eq!(batch.polylines.len(), 0);

        // Empty should not add a polyline
        batch.add_polyline(vec![], 1.0, white());
        assert_eq!(batch.polylines.len(), 0);

        // Two points should work
        batch.add_polyline(vec![Point { x: 0.0, y: 0.0 }, Point { x: 10.0, y: 10.0 }], 1.0, white());
        assert_eq!(batch.polylines.len(), 1);
    }

    #[test]
    fn test_add_polyline_styled() {
        let mut batch = PrimitiveBatch::new();
        let points = vec![Point { x: 0.0, y: 0.0 }, Point { x: 10.0, y: 10.0 }];
        batch.add_polyline_styled(points, 1.5, red(), LineStyle::Dotted);

        assert_eq!(batch.polylines[0].style, LineStyle::Dotted);
    }

    // =========================================================================
    // Text
    // =========================================================================

    #[test]
    fn test_add_text() {
        let mut batch = PrimitiveBatch::new();
        batch.add_text("Hello", Point { x: 10.0, y: 20.0 }, white(), 14.0);

        assert_eq!(batch.text_runs.len(), 1);
        assert_eq!(batch.text_runs[0].text, "Hello");
        assert_eq!(batch.text_runs[0].font_size, 14.0);
        assert!(batch.text_runs[0].cache_key.is_none());
    }

    #[test]
    fn test_add_text_cached() {
        let mut batch = PrimitiveBatch::new();
        batch.add_text_cached("Cached text", Point { x: 0.0, y: 0.0 }, white(), 16.0, 12345);

        assert_eq!(batch.text_runs[0].cache_key, Some(12345));
    }

    // =========================================================================
    // Borders
    // =========================================================================

    #[test]
    fn test_add_border() {
        let mut batch = PrimitiveBatch::new();
        batch.add_border(
            Rect { x: 0.0, y: 0.0, width: 100.0, height: 50.0 },
            8.0,  // corner_radius
            2.0,  // border_width
            red(),
        );

        assert_eq!(batch.borders.len(), 1);
        assert_eq!(batch.borders[0].corner_radius, 8.0);
        assert_eq!(batch.borders[0].border_width, 2.0);
    }

    // =========================================================================
    // Shadows
    // =========================================================================

    #[test]
    fn test_add_shadow() {
        let mut batch = PrimitiveBatch::new();
        batch.add_shadow(
            Rect { x: 5.0, y: 5.0, width: 100.0, height: 50.0 },
            4.0,  // corner_radius
            10.0, // blur_radius
            Color { r: 0.0, g: 0.0, b: 0.0, a: 0.5 },
        );

        assert_eq!(batch.shadows.len(), 1);
        assert_eq!(batch.shadows[0].blur_radius, 10.0);
    }

    // =========================================================================
    // Images
    // =========================================================================

    #[test]
    fn test_add_image() {
        let mut batch = PrimitiveBatch::new();
        let handle = ImageHandle(42);
        batch.add_image(
            Rect { x: 0.0, y: 0.0, width: 64.0, height: 64.0 },
            handle,
            4.0,  // corner_radius
            white(), // tint
        );

        assert_eq!(batch.images.len(), 1);
        assert_eq!(batch.images[0].handle.0, 42);
        assert_eq!(batch.images[0].corner_radius, 4.0);
    }

    // =========================================================================
    // Len counts all primitive types
    // =========================================================================

    #[test]
    fn test_len_counts_all_types() {
        let mut batch = PrimitiveBatch::new();

        batch.add_solid_rect(Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 }, white());
        batch.add_rounded_rect(Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 }, 2.0, white());
        batch.add_circle(Point { x: 5.0, y: 5.0 }, 5.0, white());
        batch.add_line(Point { x: 0.0, y: 0.0 }, Point { x: 10.0, y: 10.0 }, 1.0, white());
        batch.add_polyline(vec![Point { x: 0.0, y: 0.0 }, Point { x: 5.0, y: 5.0 }], 1.0, white());
        batch.add_text("test", Point { x: 0.0, y: 0.0 }, white(), 12.0);
        batch.add_border(Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 }, 2.0, 1.0, white());
        batch.add_shadow(Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 }, 2.0, 5.0, white());
        batch.add_image(Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 }, ImageHandle(1), 0.0, white());

        assert_eq!(batch.len(), 9);
        assert!(!batch.is_empty());
    }

    // =========================================================================
    // LineStyle
    // =========================================================================

    #[test]
    fn test_line_style_default() {
        let style = LineStyle::default();
        assert_eq!(style, LineStyle::Solid);
    }
}
