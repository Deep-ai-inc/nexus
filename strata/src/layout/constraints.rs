//! Layout constraints for constraint-based layout.
//!
//! Constraints flow down the widget tree, specifying the min/max bounds
//! that a widget can occupy. This replaces the two-phase measure() + layout()
//! pattern with a single layout(constraints) -> Size pattern.

use crate::primitives::Size;
use super::length::Padding;

/// Constraints passed down to children during layout.
///
/// This replaces the two-phase measure() + layout(bounds) with a single
/// layout(constraints) -> Size pattern, similar to Flutter's BoxConstraints.
#[derive(Debug, Clone, Copy)]
pub struct LayoutConstraints {
    pub min_width: f32,
    pub max_width: f32,
    pub min_height: f32,
    pub max_height: f32,
}

impl LayoutConstraints {
    /// Unbounded constraints (infinite max, zero min).
    pub const UNBOUNDED: Self = Self {
        min_width: 0.0,
        max_width: f32::INFINITY,
        min_height: 0.0,
        max_height: f32::INFINITY,
    };

    /// Create tight constraints (exact size required).
    #[inline]
    pub fn tight(width: f32, height: f32) -> Self {
        Self {
            min_width: width,
            max_width: width,
            min_height: height,
            max_height: height,
        }
    }

    /// Create loose constraints with maximum bounds.
    #[inline]
    pub fn loose(max_width: f32, max_height: f32) -> Self {
        Self {
            min_width: 0.0,
            max_width,
            min_height: 0.0,
            max_height,
        }
    }

    /// For width-first layout (e.g., text wrapping).
    /// Constrained width, unbounded height.
    #[inline]
    pub fn with_max_width(max_width: f32) -> Self {
        Self {
            min_width: 0.0,
            max_width,
            min_height: 0.0,
            max_height: f32::INFINITY,
        }
    }

    /// For height-first layout.
    /// Constrained height, unbounded width.
    #[inline]
    pub fn with_max_height(max_height: f32) -> Self {
        Self {
            min_width: 0.0,
            max_width: f32::INFINITY,
            min_height: 0.0,
            max_height,
        }
    }

    /// Constrain a size to these bounds.
    #[inline(always)]
    pub fn constrain(&self, size: Size) -> Size {
        debug_assert!(!size.width.is_nan(), "NaN width in layout");
        debug_assert!(!size.height.is_nan(), "NaN height in layout");
        Size {
            width: size.width.clamp(self.min_width, self.max_width),
            height: size.height.clamp(self.min_height, self.max_height),
        }
    }

    /// Check if max_width is finite (bounded).
    #[inline]
    pub fn has_bounded_width(&self) -> bool {
        self.max_width.is_finite()
    }

    /// Check if max_height is finite (bounded).
    #[inline]
    pub fn has_bounded_height(&self) -> bool {
        self.max_height.is_finite()
    }

    /// Whether these are tight constraints (min == max).
    #[inline]
    pub fn is_tight(&self) -> bool {
        self.min_width == self.max_width && self.min_height == self.max_height
    }

    /// Shrink constraints by padding.
    #[inline]
    pub fn deflate(&self, padding: &Padding) -> Self {
        Self {
            min_width: (self.min_width - padding.horizontal()).max(0.0),
            max_width: (self.max_width - padding.horizontal()).max(0.0),
            min_height: (self.min_height - padding.vertical()).max(0.0),
            max_height: (self.max_height - padding.vertical()).max(0.0),
        }
    }

    /// Expand constraints by padding (inverse of deflate).
    #[inline]
    pub fn inflate(&self, padding: &Padding) -> Self {
        Self {
            min_width: self.min_width + padding.horizontal(),
            max_width: if self.max_width.is_finite() {
                self.max_width + padding.horizontal()
            } else {
                f32::INFINITY
            },
            min_height: self.min_height + padding.vertical(),
            max_height: if self.max_height.is_finite() {
                self.max_height + padding.vertical()
            } else {
                f32::INFINITY
            },
        }
    }

    /// Get the biggest size that satisfies these constraints.
    #[inline]
    pub fn biggest(&self) -> Size {
        Size {
            width: if self.max_width.is_finite() { self.max_width } else { 0.0 },
            height: if self.max_height.is_finite() { self.max_height } else { 0.0 },
        }
    }

    /// Get the smallest size that satisfies these constraints.
    #[inline]
    pub fn smallest(&self) -> Size {
        Size {
            width: self.min_width,
            height: self.min_height,
        }
    }
}

impl Default for LayoutConstraints {
    fn default() -> Self {
        Self::UNBOUNDED
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tight_constraints() {
        let c = LayoutConstraints::tight(100.0, 50.0);
        assert!(c.is_tight());
        assert_eq!(c.min_width, 100.0);
        assert_eq!(c.max_width, 100.0);
        assert_eq!(c.min_height, 50.0);
        assert_eq!(c.max_height, 50.0);
    }

    #[test]
    fn test_loose_constraints() {
        let c = LayoutConstraints::loose(100.0, 50.0);
        assert!(!c.is_tight());
        assert_eq!(c.min_width, 0.0);
        assert_eq!(c.max_width, 100.0);
    }

    #[test]
    fn test_constrain() {
        let c = LayoutConstraints::loose(100.0, 50.0);

        // Size within bounds
        let s1 = c.constrain(Size::new(50.0, 25.0));
        assert_eq!(s1.width, 50.0);
        assert_eq!(s1.height, 25.0);

        // Size exceeding bounds
        let s2 = c.constrain(Size::new(200.0, 100.0));
        assert_eq!(s2.width, 100.0);
        assert_eq!(s2.height, 50.0);
    }

    #[test]
    fn test_deflate() {
        let c = LayoutConstraints::tight(100.0, 50.0);
        let padding = Padding::all(10.0);
        let deflated = c.deflate(&padding);

        assert_eq!(deflated.max_width, 80.0);
        assert_eq!(deflated.max_height, 30.0);
    }

    #[test]
    fn test_bounded_checks() {
        let unbounded = LayoutConstraints::UNBOUNDED;
        assert!(!unbounded.has_bounded_width());
        assert!(!unbounded.has_bounded_height());

        let bounded = LayoutConstraints::loose(100.0, 50.0);
        assert!(bounded.has_bounded_width());
        assert!(bounded.has_bounded_height());
    }
}
