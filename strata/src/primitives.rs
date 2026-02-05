//! Core primitive types for Strata.
//!
//! These types are used throughout the library for geometry, color, and constraints.

use std::ops::{Add, Sub};

/// A point in 2D space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ORIGIN: Self = Self { x: 0.0, y: 0.0 };

    #[inline]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl From<(f32, f32)> for Point {
    fn from((x, y): (f32, f32)) -> Self {
        Self { x, y }
    }
}

impl Add for Point {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl Sub for Point {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

/// A rectangle in screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    };

    #[inline]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }

    #[inline]
    pub fn from_origin_size(origin: Point, size: Size) -> Self {
        Self {
            x: origin.x,
            y: origin.y,
            width: size.width,
            height: size.height,
        }
    }

    /// Check if a point is inside this rectangle.
    #[inline]
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x
            && point.x < self.x + self.width
            && point.y >= self.y
            && point.y < self.y + self.height
    }

    /// Check if a point (as separate coordinates) is inside this rectangle.
    #[inline]
    pub fn contains_xy(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    /// Get the origin point of this rectangle.
    #[inline]
    pub fn origin(&self) -> Point {
        Point { x: self.x, y: self.y }
    }

    /// Get the size of this rectangle.
    #[inline]
    pub fn size(&self) -> Size {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    /// Get the right edge X coordinate.
    #[inline]
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    /// Get the bottom edge Y coordinate.
    #[inline]
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// Get the center point of this rectangle.
    #[inline]
    pub fn center(&self) -> Point {
        Point {
            x: self.x + self.width / 2.0,
            y: self.y + self.height / 2.0,
        }
    }

    /// Compute the smallest rectangle that contains both `self` and `other`.
    #[inline]
    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Rect { x, y, width: right - x, height: bottom - y }
    }

    /// Check if this rectangle intersects with another.
    #[inline]
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }

    /// Get the intersection of two rectangles, if any.
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        if !self.intersects(other) {
            return None;
        }

        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());

        Some(Rect {
            x,
            y,
            width: right - x,
            height: bottom - y,
        })
    }

    /// Translate this rectangle by an offset.
    #[inline]
    pub fn translate(&self, offset: Point) -> Self {
        Self {
            x: self.x + offset.x,
            y: self.y + offset.y,
            ..*self
        }
    }
}

/// A 2D size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const ZERO: Self = Self {
        width: 0.0,
        height: 0.0,
    };

    #[inline]
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

impl From<(f32, f32)> for Size {
    fn from((width, height): (f32, f32)) -> Self {
        Self { width, height }
    }
}

/// Layout constraints for widgets.
#[derive(Debug, Clone, Copy)]
pub struct Constraints {
    pub min_width: f32,
    pub max_width: f32,
    pub min_height: f32,
    pub max_height: f32,
}

impl Constraints {
    /// Unbounded constraints (no minimum, infinite maximum).
    pub const UNBOUNDED: Self = Self {
        min_width: 0.0,
        max_width: f32::INFINITY,
        min_height: 0.0,
        max_height: f32::INFINITY,
    };

    /// Create tight constraints that force a specific size.
    #[inline]
    pub fn tight(size: Size) -> Self {
        Self {
            min_width: size.width,
            max_width: size.width,
            min_height: size.height,
            max_height: size.height,
        }
    }

    /// Create loose constraints with a maximum size.
    #[inline]
    pub fn loose(size: Size) -> Self {
        Self {
            min_width: 0.0,
            max_width: size.width,
            min_height: 0.0,
            max_height: size.height,
        }
    }

    /// Create constraints with only a maximum width.
    #[inline]
    pub fn max_width(width: f32) -> Self {
        Self {
            min_width: 0.0,
            max_width: width,
            min_height: 0.0,
            max_height: f32::INFINITY,
        }
    }

    /// Constrain a size to fit within these constraints.
    #[inline]
    pub fn constrain(&self, size: Size) -> Size {
        Size {
            width: size.width.clamp(self.min_width, self.max_width),
            height: size.height.clamp(self.min_height, self.max_height),
        }
    }

    /// Check if a size satisfies these constraints.
    #[inline]
    pub fn is_satisfied_by(&self, size: Size) -> bool {
        size.width >= self.min_width
            && size.width <= self.max_width
            && size.height >= self.min_height
            && size.height <= self.max_height
    }
}

impl Default for Constraints {
    fn default() -> Self {
        Self::UNBOUNDED
    }
}

/// RGBA color with components in 0.0-1.0 range.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
    pub const BLACK: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const WHITE: Self = Self {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };
    pub const RED: Self = Self {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const GREEN: Self = Self {
        r: 0.0,
        g: 1.0,
        b: 0.0,
        a: 1.0,
    };
    pub const BLUE: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 1.0,
        a: 1.0,
    };

    /// Selection highlight color (blue tint with transparency).
    pub const SELECTION: Self = Self {
        r: 0.3,
        g: 0.5,
        b: 0.8,
        a: 0.4,
    };

    /// Create a color from RGB values (0.0-1.0).
    #[inline]
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Create a color from RGBA values (0.0-1.0).
    #[inline]
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Create a color from RGB values (0-255).
    #[inline]
    pub fn rgb8(r: u8, g: u8, b: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        }
    }

    /// Create a color from RGBA values (0-255).
    #[inline]
    pub fn rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
    }

    /// Pack this color into a u32 (RGBA8 format: R in lowest byte).
    #[inline]
    pub fn pack(&self) -> u32 {
        let r = (self.r.clamp(0.0, 1.0) * 255.0) as u32;
        let g = (self.g.clamp(0.0, 1.0) * 255.0) as u32;
        let b = (self.b.clamp(0.0, 1.0) * 255.0) as u32;
        let a = (self.a.clamp(0.0, 1.0) * 255.0) as u32;
        r | (g << 8) | (b << 16) | (a << 24)
    }

    /// Unpack a u32 (RGBA8 format) into a color.
    #[inline]
    pub fn unpack(packed: u32) -> Self {
        Self {
            r: (packed & 0xFF) as f32 / 255.0,
            g: ((packed >> 8) & 0xFF) as f32 / 255.0,
            b: ((packed >> 16) & 0xFF) as f32 / 255.0,
            a: ((packed >> 24) & 0xFF) as f32 / 255.0,
        }
    }

    /// Return this color with a different alpha value.
    #[inline]
    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Point tests
    // =========================================================================

    #[test]
    fn point_new() {
        let p = Point::new(10.0, 20.0);
        assert_eq!(p.x, 10.0);
        assert_eq!(p.y, 20.0);
    }

    #[test]
    fn point_origin() {
        assert_eq!(Point::ORIGIN, Point::new(0.0, 0.0));
    }

    #[test]
    fn point_from_tuple() {
        let p: Point = (5.0, 10.0).into();
        assert_eq!(p, Point::new(5.0, 10.0));
    }

    #[test]
    fn point_add() {
        let a = Point::new(10.0, 20.0);
        let b = Point::new(5.0, 15.0);
        let result = a + b;
        assert_eq!(result, Point::new(15.0, 35.0));
    }

    #[test]
    fn point_sub() {
        let a = Point::new(10.0, 20.0);
        let b = Point::new(5.0, 15.0);
        let result = a - b;
        assert_eq!(result, Point::new(5.0, 5.0));
    }

    #[test]
    fn point_default() {
        let p: Point = Default::default();
        assert_eq!(p, Point::ORIGIN);
    }

    // =========================================================================
    // Size tests
    // =========================================================================

    #[test]
    fn size_new() {
        let s = Size::new(100.0, 50.0);
        assert_eq!(s.width, 100.0);
        assert_eq!(s.height, 50.0);
    }

    #[test]
    fn size_zero() {
        assert_eq!(Size::ZERO, Size::new(0.0, 0.0));
    }

    #[test]
    fn size_from_tuple() {
        let s: Size = (200.0, 100.0).into();
        assert_eq!(s, Size::new(200.0, 100.0));
    }

    #[test]
    fn size_default() {
        let s: Size = Default::default();
        assert_eq!(s, Size::ZERO);
    }

    // =========================================================================
    // Rect tests
    // =========================================================================

    #[test]
    fn rect_contains() {
        let rect = Rect::new(10.0, 20.0, 100.0, 50.0);

        assert!(rect.contains(Point::new(10.0, 20.0))); // Top-left corner
        assert!(rect.contains(Point::new(50.0, 40.0))); // Center
        assert!(rect.contains(Point::new(109.9, 69.9))); // Just inside bottom-right

        assert!(!rect.contains(Point::new(110.0, 70.0))); // Bottom-right corner (exclusive)
        assert!(!rect.contains(Point::new(5.0, 40.0))); // Left of rect
        assert!(!rect.contains(Point::new(50.0, 80.0))); // Below rect
    }

    #[test]
    fn rect_contains_xy() {
        let rect = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert!(rect.contains_xy(50.0, 40.0));
        assert!(!rect.contains_xy(5.0, 40.0));
    }

    #[test]
    fn rect_zero() {
        let r = Rect::ZERO;
        assert_eq!(r.x, 0.0);
        assert_eq!(r.y, 0.0);
        assert_eq!(r.width, 0.0);
        assert_eq!(r.height, 0.0);
    }

    #[test]
    fn rect_from_origin_size() {
        let r = Rect::from_origin_size(Point::new(10.0, 20.0), Size::new(100.0, 50.0));
        assert_eq!(r, Rect::new(10.0, 20.0, 100.0, 50.0));
    }

    #[test]
    fn rect_origin_and_size() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(r.origin(), Point::new(10.0, 20.0));
        assert_eq!(r.size(), Size::new(100.0, 50.0));
    }

    #[test]
    fn rect_right_bottom() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(r.right(), 110.0);
        assert_eq!(r.bottom(), 70.0);
    }

    #[test]
    fn rect_center() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert_eq!(r.center(), Point::new(50.0, 25.0));
    }

    #[test]
    fn rect_union() {
        let a = Rect::new(0.0, 0.0, 50.0, 50.0);
        let b = Rect::new(25.0, 25.0, 50.0, 50.0);
        let union = a.union(&b);
        assert_eq!(union, Rect::new(0.0, 0.0, 75.0, 75.0));
    }

    #[test]
    fn rect_intersects() {
        let a = Rect::new(0.0, 0.0, 100.0, 100.0);
        let b = Rect::new(50.0, 50.0, 100.0, 100.0);
        let c = Rect::new(200.0, 200.0, 50.0, 50.0);

        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
    }

    #[test]
    fn rect_intersection() {
        let a = Rect::new(0.0, 0.0, 100.0, 100.0);
        let b = Rect::new(50.0, 50.0, 100.0, 100.0);

        let intersection = a.intersection(&b).unwrap();
        assert_eq!(intersection, Rect::new(50.0, 50.0, 50.0, 50.0));

        let c = Rect::new(200.0, 200.0, 50.0, 50.0);
        assert!(a.intersection(&c).is_none());
    }

    #[test]
    fn rect_translate() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        let translated = r.translate(Point::new(5.0, -10.0));
        assert_eq!(translated, Rect::new(15.0, 10.0, 100.0, 50.0));
    }

    #[test]
    fn rect_default() {
        let r: Rect = Default::default();
        assert_eq!(r, Rect::ZERO);
    }

    // =========================================================================
    // Constraints tests
    // =========================================================================

    #[test]
    fn constraints_constrain() {
        let constraints = Constraints {
            min_width: 50.0,
            max_width: 200.0,
            min_height: 30.0,
            max_height: 100.0,
        };

        assert_eq!(
            constraints.constrain(Size::new(100.0, 50.0)),
            Size::new(100.0, 50.0)
        );
        assert_eq!(
            constraints.constrain(Size::new(10.0, 10.0)),
            Size::new(50.0, 30.0)
        );
        assert_eq!(
            constraints.constrain(Size::new(500.0, 500.0)),
            Size::new(200.0, 100.0)
        );
    }

    #[test]
    fn constraints_unbounded() {
        let c = Constraints::UNBOUNDED;
        assert_eq!(c.min_width, 0.0);
        assert_eq!(c.min_height, 0.0);
        assert!(c.max_width.is_infinite());
        assert!(c.max_height.is_infinite());
    }

    #[test]
    fn constraints_tight() {
        let c = Constraints::tight(Size::new(100.0, 50.0));
        assert_eq!(c.min_width, 100.0);
        assert_eq!(c.max_width, 100.0);
        assert_eq!(c.min_height, 50.0);
        assert_eq!(c.max_height, 50.0);
    }

    #[test]
    fn constraints_loose() {
        let c = Constraints::loose(Size::new(100.0, 50.0));
        assert_eq!(c.min_width, 0.0);
        assert_eq!(c.max_width, 100.0);
        assert_eq!(c.min_height, 0.0);
        assert_eq!(c.max_height, 50.0);
    }

    #[test]
    fn constraints_max_width() {
        let c = Constraints::max_width(500.0);
        assert_eq!(c.min_width, 0.0);
        assert_eq!(c.max_width, 500.0);
        assert_eq!(c.min_height, 0.0);
        assert!(c.max_height.is_infinite());
    }

    #[test]
    fn constraints_is_satisfied_by() {
        let c = Constraints {
            min_width: 50.0,
            max_width: 200.0,
            min_height: 30.0,
            max_height: 100.0,
        };

        assert!(c.is_satisfied_by(Size::new(100.0, 50.0)));
        assert!(c.is_satisfied_by(Size::new(50.0, 30.0))); // Exactly at min
        assert!(c.is_satisfied_by(Size::new(200.0, 100.0))); // Exactly at max
        assert!(!c.is_satisfied_by(Size::new(49.0, 50.0))); // Below min width
        assert!(!c.is_satisfied_by(Size::new(100.0, 29.0))); // Below min height
        assert!(!c.is_satisfied_by(Size::new(201.0, 50.0))); // Above max width
    }

    #[test]
    fn constraints_default() {
        let c: Constraints = Default::default();
        assert_eq!(c.min_width, Constraints::UNBOUNDED.min_width);
        assert_eq!(c.max_width, Constraints::UNBOUNDED.max_width);
    }

    // =========================================================================
    // Color tests
    // =========================================================================

    #[test]
    fn color_constants() {
        assert_eq!(Color::BLACK, Color::rgba(0.0, 0.0, 0.0, 1.0));
        assert_eq!(Color::WHITE, Color::rgba(1.0, 1.0, 1.0, 1.0));
        assert_eq!(Color::RED, Color::rgba(1.0, 0.0, 0.0, 1.0));
        assert_eq!(Color::GREEN, Color::rgba(0.0, 1.0, 0.0, 1.0));
        assert_eq!(Color::BLUE, Color::rgba(0.0, 0.0, 1.0, 1.0));
        assert_eq!(Color::TRANSPARENT, Color::rgba(0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn color_rgb() {
        let c = Color::rgb(0.5, 0.25, 0.75);
        assert_eq!(c.r, 0.5);
        assert_eq!(c.g, 0.25);
        assert_eq!(c.b, 0.75);
        assert_eq!(c.a, 1.0);
    }

    #[test]
    fn color_rgb8() {
        let c = Color::rgb8(255, 128, 0);
        assert!((c.r - 1.0).abs() < 0.01);
        assert!((c.g - 0.5).abs() < 0.01);
        assert!((c.b - 0.0).abs() < 0.01);
        assert_eq!(c.a, 1.0);
    }

    #[test]
    fn color_rgba8() {
        let c = Color::rgba8(255, 128, 64, 128);
        assert!((c.r - 1.0).abs() < 0.01);
        assert!((c.g - 0.5).abs() < 0.01);
        assert!((c.b - 0.25).abs() < 0.01);
        assert!((c.a - 0.5).abs() < 0.01);
    }

    #[test]
    fn color_pack_unpack() {
        let color = Color::rgba(0.5, 0.25, 0.75, 1.0);
        let packed = color.pack();
        let unpacked = Color::unpack(packed);

        // Allow small floating point differences
        assert!((color.r - unpacked.r).abs() < 0.01);
        assert!((color.g - unpacked.g).abs() < 0.01);
        assert!((color.b - unpacked.b).abs() < 0.01);
        assert!((color.a - unpacked.a).abs() < 0.01);
    }

    #[test]
    fn color_with_alpha() {
        let c = Color::RED.with_alpha(0.5);
        assert_eq!(c.r, 1.0);
        assert_eq!(c.g, 0.0);
        assert_eq!(c.b, 0.0);
        assert_eq!(c.a, 0.5);
    }

    #[test]
    fn color_default() {
        let c: Color = Default::default();
        assert_eq!(c.r, 0.0);
        assert_eq!(c.g, 0.0);
        assert_eq!(c.b, 0.0);
        assert_eq!(c.a, 0.0);
    }
}
