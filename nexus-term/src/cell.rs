//! Terminal cell representation.

use alacritty_terminal::term::cell::Flags as AlacrittyFlags;
use alacritty_terminal::vte::ansi::Color as AnsiColor;

/// A single cell in the terminal grid.
#[derive(Debug, Clone, Default)]
pub struct Cell {
    /// The character in this cell.
    pub c: char,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// Cell attributes (bold, italic, etc.).
    pub flags: CellFlags,
}

/// Cell attribute flags.
#[derive(Debug, Clone, Copy, Default)]
pub struct CellFlags {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub dim: bool,
    pub inverse: bool,
    pub hidden: bool,
}

impl From<AlacrittyFlags> for CellFlags {
    fn from(flags: AlacrittyFlags) -> Self {
        Self {
            bold: flags.contains(AlacrittyFlags::BOLD),
            italic: flags.contains(AlacrittyFlags::ITALIC),
            underline: flags.contains(AlacrittyFlags::ALL_UNDERLINES),
            strikethrough: flags.contains(AlacrittyFlags::STRIKEOUT),
            dim: flags.contains(AlacrittyFlags::DIM),
            inverse: flags.contains(AlacrittyFlags::INVERSE),
            hidden: flags.contains(AlacrittyFlags::HIDDEN),
        }
    }
}

/// Terminal color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// Default foreground/background.
    Default,
    /// Named ANSI color (0-15).
    Named(u8),
    /// 256-color palette index.
    Indexed(u8),
    /// True color RGB.
    Rgb(u8, u8, u8),
}

impl Default for Color {
    fn default() -> Self {
        Color::Default
    }
}

impl Color {
    /// Convert to RGBA values (0.0-1.0).
    pub fn to_rgba(&self, is_foreground: bool) -> [f32; 4] {
        match self {
            Color::Default => {
                if is_foreground {
                    [0.9, 0.9, 0.9, 1.0] // Light gray for text
                } else {
                    [0.1, 0.1, 0.1, 1.0] // Dark gray for background
                }
            }
            Color::Named(n) | Color::Indexed(n) => ansi_to_rgba(*n),
            Color::Rgb(r, g, b) => [*r as f32 / 255.0, *g as f32 / 255.0, *b as f32 / 255.0, 1.0],
        }
    }
}

/// Convert ANSI color index to RGBA.
fn ansi_to_rgba(index: u8) -> [f32; 4] {
    // Standard ANSI colors
    let (r, g, b) = match index {
        0 => (0, 0, 0),       // Black
        1 => (205, 49, 49),   // Red
        2 => (13, 188, 121),  // Green
        3 => (229, 229, 16),  // Yellow
        4 => (36, 114, 200),  // Blue
        5 => (188, 63, 188),  // Magenta
        6 => (17, 168, 205),  // Cyan
        7 => (229, 229, 229), // White
        // Bright colors
        8 => (102, 102, 102),  // Bright Black
        9 => (241, 76, 76),    // Bright Red
        10 => (35, 209, 139),  // Bright Green
        11 => (245, 245, 67),  // Bright Yellow
        12 => (59, 142, 234),  // Bright Blue
        13 => (214, 112, 214), // Bright Magenta
        14 => (41, 184, 219),  // Bright Cyan
        15 => (255, 255, 255), // Bright White
        // 216 color cube (16-231)
        16..=231 => {
            let n = index - 16;
            let r = (n / 36) % 6;
            let g = (n / 6) % 6;
            let b = n % 6;
            let to_val = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (to_val(r), to_val(g), to_val(b))
        }
        // Grayscale (232-255)
        232..=255 => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
    };
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

impl From<AnsiColor> for Color {
    fn from(color: AnsiColor) -> Self {
        match color {
            AnsiColor::Named(named) => Color::Named(named as u8),
            AnsiColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
            AnsiColor::Indexed(idx) => Color::Indexed(idx),
        }
    }
}
