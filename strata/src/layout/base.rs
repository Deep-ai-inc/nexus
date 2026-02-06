//! Base container infrastructure.
//!
//! Provides shared types and functions for container chrome rendering,
//! eliminating duplication between Column, Row, and ScrollColumn.

use crate::layout_snapshot::LayoutSnapshot;
use crate::primitives::{Color, Rect};

// =========================================================================
// Container Chrome
// =========================================================================

/// Visual chrome properties shared by all containers.
///
/// Chrome is the "decoration" of a container: shadow, background, and border.
/// These are rendered in a specific z-order (shadow → background → border)
/// to ensure correct visual stacking.
#[derive(Debug, Clone, Default)]
pub struct Chrome {
    /// Background color (optional).
    pub background: Option<Color>,
    /// Corner radius for rounded rectangles.
    pub corner_radius: f32,
    /// Border color (optional).
    pub border_color: Option<Color>,
    /// Border width in pixels.
    pub border_width: f32,
    /// Shadow: (blur_radius, color). Offset is fixed at (4, 4).
    pub shadow: Option<(f32, Color)>,
}

impl Chrome {
    /// Create chrome with no decorations.
    #[inline]
    pub fn none() -> Self {
        Self::default()
    }

    /// Check if this chrome has any visible decorations.
    #[inline]
    pub fn has_visible_chrome(&self) -> bool {
        self.background.is_some() || self.border_color.is_some()
    }
}

/// Render container chrome (shadow → background → border).
///
/// This must be called BEFORE pushing any clip rects, as chrome is
/// drawn outside the content area. The z-order ensures:
/// 1. Shadow appears behind everything
/// 2. Background fills the container
/// 3. Border draws on top of background
#[inline]
pub fn render_chrome(snapshot: &mut LayoutSnapshot, bounds: Rect, chrome: &Chrome) {
    // 1. Shadow (offset by 4px)
    if let Some((blur, color)) = chrome.shadow {
        snapshot.primitives_mut().add_shadow(
            Rect::new(bounds.x + 4.0, bounds.y + 4.0, bounds.width, bounds.height),
            chrome.corner_radius,
            blur,
            color,
        );
    }

    // 2. Background
    if let Some(bg) = chrome.background {
        if chrome.corner_radius > 0.0 {
            snapshot.primitives_mut().add_rounded_rect(bounds, chrome.corner_radius, bg);
        } else {
            snapshot.primitives_mut().add_solid_rect(bounds, bg);
        }
    }

    // 3. Border
    if let Some(border_color) = chrome.border_color {
        snapshot.primitives_mut().add_border(
            bounds,
            chrome.corner_radius,
            chrome.border_width,
            border_color,
        );
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Color;

    #[test]
    fn test_chrome_default_is_invisible() {
        let chrome = Chrome::none();
        assert!(!chrome.has_visible_chrome());
    }

    #[test]
    fn test_chrome_with_background_is_visible() {
        let chrome = Chrome {
            background: Some(Color::WHITE),
            ..Chrome::default()
        };
        assert!(chrome.has_visible_chrome());
    }

    #[test]
    fn test_chrome_with_border_is_visible() {
        let chrome = Chrome {
            border_color: Some(Color::BLACK),
            border_width: 1.0,
            ..Chrome::default()
        };
        assert!(chrome.has_visible_chrome());
    }
}
