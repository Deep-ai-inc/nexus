//! Color conversion and value-to-color mapping.

use nexus_api::{FileEntry, FileType, Value};
use strata::primitives::Color;

use crate::ui::theme;

/// Convert nexus-term color to Strata color.
pub(crate) fn term_color_to_strata(c: nexus_term::Color) -> Color {
    // ANSI palette matched from theme.rs ANSI_* constants
    fn ansi_color(n: u8) -> Color {
        match n {
            0  => Color::rgb(0.0, 0.0, 0.0),       // Black
            1  => Color::rgb(0.8, 0.2, 0.2),        // Red
            2  => Color::rgb(0.05, 0.74, 0.47),     // Green
            3  => Color::rgb(0.9, 0.9, 0.06),       // Yellow
            4  => Color::rgb(0.14, 0.45, 0.78),     // Blue
            5  => Color::rgb(0.74, 0.25, 0.74),     // Magenta
            6  => Color::rgb(0.07, 0.66, 0.8),      // Cyan
            7  => Color::rgb(0.9, 0.9, 0.9),        // White
            8  => Color::rgb(0.4, 0.4, 0.4),        // Bright Black
            9  => Color::rgb(0.95, 0.3, 0.3),       // Bright Red
            10 => Color::rgb(0.14, 0.82, 0.55),     // Bright Green
            11 => Color::rgb(0.96, 0.96, 0.26),     // Bright Yellow
            12 => Color::rgb(0.23, 0.56, 0.92),     // Bright Blue
            13 => Color::rgb(0.84, 0.44, 0.84),     // Bright Magenta
            14 => Color::rgb(0.16, 0.72, 0.86),     // Bright Cyan
            15 => Color::rgb(1.0, 1.0, 1.0),        // Bright White
            // 216-color cube (indices 16-231)
            16..=231 => {
                let idx = n - 16;
                let r = (idx / 36) % 6;
                let g = (idx / 6) % 6;
                let b = idx % 6;
                let to_val = |v: u8| if v == 0 { 0.0 } else { (55.0 + v as f32 * 40.0) / 255.0 };
                Color::rgb(to_val(r), to_val(g), to_val(b))
            }
            // Grayscale (indices 232-255)
            232..=255 => {
                let gray = (8.0 + (n - 232) as f32 * 10.0) / 255.0;
                Color::rgb(gray, gray, gray)
            }
        }
    }

    match c {
        nexus_term::Color::Default => Color::rgb(0.9, 0.9, 0.9),
        nexus_term::Color::Named(n) => ansi_color(n),
        nexus_term::Color::Rgb(r, g, b) => Color::rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0),
        nexus_term::Color::Indexed(n) => ansi_color(n),
    }
}

/// Map a Value to its display color.
pub(super) fn value_text_color(value: &Value) -> Color {
    match value {
        Value::Int(_) | Value::Float(_) => Color::rgb(0.6, 0.8, 1.0),
        Value::Bool(true) => theme::SUCCESS,
        Value::Bool(false) => theme::ERROR,
        Value::Path(_) => theme::TEXT_PATH,
        Value::FileEntry(e) => file_entry_color(e),
        Value::Error { .. } => theme::ERROR,
        _ => theme::TEXT_PRIMARY,
    }
}

/// Badge dot color for a file type indicator.
///
/// The dot communicates *what kind of thing* this is — each type gets a
/// unique, maximally distinct hue so users can scan a listing at a glance.
pub(super) fn file_type_dot_color(ft: &FileType) -> Color {
    match ft {
        FileType::Directory  => Color::rgb(0.35, 0.55, 1.0),  // blue
        FileType::Symlink    => Color::rgb(0.0, 0.85, 0.85),  // cyan
        FileType::Fifo       => Color::rgb(0.95, 0.75, 0.1),  // amber
        FileType::Socket     => Color::rgb(0.85, 0.35, 0.85), // magenta
        FileType::BlockDevice => Color::rgb(0.95, 0.55, 0.15),// orange
        FileType::CharDevice => Color::rgb(0.75, 0.45, 0.15), // brown-orange
        FileType::File       => Color::rgb(0.55, 0.55, 0.6),  // gray
        FileType::Unknown    => Color::rgb(0.4, 0.4, 0.42),   // dim gray
    }
}

/// Text color for a file entry.
///
/// The text color communicates *attributes* — executable, hidden, symlink —
/// complementing the dot which already encodes the type.
pub(super) fn file_entry_color(entry: &FileEntry) -> Color {
    match entry.file_type {
        FileType::Directory => Color::rgb(0.45, 0.65, 1.0),            // bold blue
        FileType::Symlink   => Color::rgb(0.4, 0.88, 0.88),           // cyan
        FileType::Fifo | FileType::Socket
            | FileType::BlockDevice | FileType::CharDevice
                            => Color::rgb(0.9, 0.75, 0.3),            // warm yellow
        _ if entry.is_hidden => Color::rgb(0.5, 0.5, 0.52),           // dimmed
        _ if entry.permissions & 0o111 != 0 => Color::rgb(0.4, 0.9, 0.4), // green = executable
        _ => Color::rgb(0.82, 0.82, 0.82),                            // neutral light
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    fn color_eq(a: Color, b: Color) -> bool {
        approx_eq(a.r, b.r) && approx_eq(a.g, b.g) && approx_eq(a.b, b.b)
    }

    #[test]
    fn test_color_default() {
        let c = term_color_to_strata(nexus_term::Color::Default);
        assert!(color_eq(c, Color::rgb(0.9, 0.9, 0.9)));
    }

    #[test]
    fn test_color_named_black() {
        let c = term_color_to_strata(nexus_term::Color::Named(0));
        assert!(color_eq(c, Color::rgb(0.0, 0.0, 0.0)));
    }

    #[test]
    fn test_color_named_red() {
        let c = term_color_to_strata(nexus_term::Color::Named(1));
        assert!(color_eq(c, Color::rgb(0.8, 0.2, 0.2)));
    }

    #[test]
    fn test_color_named_bright_white() {
        let c = term_color_to_strata(nexus_term::Color::Named(15));
        assert!(color_eq(c, Color::rgb(1.0, 1.0, 1.0)));
    }

    #[test]
    fn test_color_indexed_maps_like_named() {
        for i in 0..=15u8 {
            let named = term_color_to_strata(nexus_term::Color::Named(i));
            let indexed = term_color_to_strata(nexus_term::Color::Indexed(i));
            assert!(color_eq(named, indexed), "mismatch at index {i}");
        }
    }

    #[test]
    fn test_color_216_cube_black_corner() {
        let c = term_color_to_strata(nexus_term::Color::Indexed(16));
        assert!(color_eq(c, Color::rgb(0.0, 0.0, 0.0)));
    }

    #[test]
    fn test_color_216_cube_white_corner() {
        let c = term_color_to_strata(nexus_term::Color::Indexed(231));
        let expected = (55.0 + 5.0 * 40.0) / 255.0;
        assert!(color_eq(c, Color::rgb(expected, expected, expected)));
    }

    #[test]
    fn test_color_216_cube_pure_red() {
        let c = term_color_to_strata(nexus_term::Color::Indexed(196));
        let r_val = (55.0 + 5.0 * 40.0) / 255.0;
        assert!(color_eq(c, Color::rgb(r_val, 0.0, 0.0)));
    }

    #[test]
    fn test_color_grayscale_darkest() {
        let c = term_color_to_strata(nexus_term::Color::Indexed(232));
        let gray = 8.0 / 255.0;
        assert!(color_eq(c, Color::rgb(gray, gray, gray)));
    }

    #[test]
    fn test_color_grayscale_lightest() {
        let c = term_color_to_strata(nexus_term::Color::Indexed(255));
        let gray = (8.0 + 23.0 * 10.0) / 255.0;
        assert!(color_eq(c, Color::rgb(gray, gray, gray)));
    }

    #[test]
    fn test_color_rgb() {
        let c = term_color_to_strata(nexus_term::Color::Rgb(128, 64, 255));
        assert!(approx_eq(c.r, 128.0 / 255.0));
        assert!(approx_eq(c.g, 64.0 / 255.0));
        assert!(approx_eq(c.b, 255.0 / 255.0));
    }
}
