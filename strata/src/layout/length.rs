//! Layout sizing types and constants.
//!
//! Core types for specifying container dimensions and alignment.

// Layout metrics derived from cosmic-text for JetBrains Mono at 14px base size.
pub const CHAR_WIDTH: f32 = 8.4;
pub const LINE_HEIGHT: f32 = 18.0;
pub const BASE_FONT_SIZE: f32 = 14.0;

/// Sizing mode for a container axis.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Length {
    /// Shrink to fit content (intrinsic size).
    #[default]
    Shrink,
    /// Expand to fill available space (flex: 1).
    Fill,
    /// Expand proportionally (flex: n). `FillPortion(1)` == `Fill`.
    FillPortion(u16),
    /// Fixed pixel size.
    Fixed(f32),
}

impl Length {
    /// Get the flex factor for this length, or 0 if not flexible.
    pub fn flex(&self) -> f32 {
        match self {
            Length::Fill => 1.0,
            Length::FillPortion(n) => *n as f32,
            _ => 0.0,
        }
    }

    /// Whether this length participates in flex distribution.
    pub fn is_flex(&self) -> bool {
        matches!(self, Length::Fill | Length::FillPortion(_))
    }
}

/// Alignment on the main axis (direction of flow).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Alignment {
    /// Pack children at the start.
    #[default]
    Start,
    /// Pack children at the end.
    End,
    /// Center children.
    Center,
    /// Distribute space evenly between children.
    SpaceBetween,
    /// Distribute space evenly around children.
    SpaceAround,
}

/// Alignment on the cross axis (perpendicular to flow).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CrossAxisAlignment {
    /// Align to start of cross axis.
    #[default]
    Start,
    /// Align to end of cross axis.
    End,
    /// Center on cross axis.
    Center,
    /// Stretch to fill cross axis.
    Stretch,
}

/// Padding around content.
#[derive(Debug, Clone, Copy, Default)]
pub struct Padding {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Padding {
    /// Create padding with explicit values for each side.
    pub fn new(top: f32, right: f32, bottom: f32, left: f32) -> Self {
        Self { top, right, bottom, left }
    }

    /// Uniform padding on all sides.
    pub fn all(value: f32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    /// Symmetric padding (horizontal, vertical).
    pub fn symmetric(horizontal: f32, vertical: f32) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }

    /// Total horizontal padding.
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    /// Total vertical padding.
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Length tests
    #[test]
    fn test_length_flex_factor() {
        assert_eq!(Length::Fill.flex(), 1.0);
        assert_eq!(Length::FillPortion(2).flex(), 2.0);
        assert_eq!(Length::FillPortion(3).flex(), 3.0);
        assert_eq!(Length::Shrink.flex(), 0.0);
        assert_eq!(Length::Fixed(100.0).flex(), 0.0);
    }

    #[test]
    fn test_length_is_flex() {
        assert!(Length::Fill.is_flex());
        assert!(Length::FillPortion(2).is_flex());
        assert!(!Length::Shrink.is_flex());
        assert!(!Length::Fixed(100.0).is_flex());
    }

    #[test]
    fn test_length_default_is_shrink() {
        assert_eq!(Length::default(), Length::Shrink);
    }

    // Padding tests
    #[test]
    fn test_padding_new() {
        let p = Padding::new(1.0, 2.0, 3.0, 4.0);
        assert_eq!(p.top, 1.0);
        assert_eq!(p.right, 2.0);
        assert_eq!(p.bottom, 3.0);
        assert_eq!(p.left, 4.0);
    }

    #[test]
    fn test_padding_all() {
        let p = Padding::all(10.0);
        assert_eq!(p.top, 10.0);
        assert_eq!(p.right, 10.0);
        assert_eq!(p.bottom, 10.0);
        assert_eq!(p.left, 10.0);
    }

    #[test]
    fn test_padding_symmetric() {
        let p = Padding::symmetric(10.0, 5.0);
        assert_eq!(p.top, 5.0);
        assert_eq!(p.right, 10.0);
        assert_eq!(p.bottom, 5.0);
        assert_eq!(p.left, 10.0);
    }

    #[test]
    fn test_padding_horizontal() {
        let p = Padding::new(0.0, 10.0, 0.0, 5.0);
        assert_eq!(p.horizontal(), 15.0);
    }

    #[test]
    fn test_padding_vertical() {
        let p = Padding::new(10.0, 0.0, 5.0, 0.0);
        assert_eq!(p.vertical(), 15.0);
    }

    #[test]
    fn test_padding_default_is_zero() {
        let p = Padding::default();
        assert_eq!(p.top, 0.0);
        assert_eq!(p.right, 0.0);
        assert_eq!(p.bottom, 0.0);
        assert_eq!(p.left, 0.0);
    }

    // Alignment tests
    #[test]
    fn test_alignment_default_is_start() {
        assert_eq!(Alignment::default(), Alignment::Start);
    }

    #[test]
    fn test_cross_axis_alignment_default_is_start() {
        assert_eq!(CrossAxisAlignment::default(), CrossAxisAlignment::Start);
    }

    // Constants tests
    #[test]
    fn test_layout_constants() {
        assert!(CHAR_WIDTH > 0.0);
        assert!(LINE_HEIGHT > 0.0);
        assert!(BASE_FONT_SIZE > 0.0);
        // JetBrains Mono 14px metrics
        assert_eq!(CHAR_WIDTH, 8.4);
        assert_eq!(LINE_HEIGHT, 18.0);
        assert_eq!(BASE_FONT_SIZE, 14.0);
    }
}
