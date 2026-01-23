//! Nexus theme - semantic colors and styling.

#![allow(dead_code)]

use iced::Color;

/// Nexus color palette.
pub struct NexusTheme;

impl NexusTheme {
    // Backgrounds
    pub const BG_PRIMARY: Color = Color::from_rgb(0.08, 0.08, 0.10);
    pub const BG_SECONDARY: Color = Color::from_rgb(0.12, 0.12, 0.14);
    pub const BG_TERTIARY: Color = Color::from_rgb(0.16, 0.16, 0.18);

    // Foregrounds
    pub const FG_PRIMARY: Color = Color::from_rgb(0.9, 0.9, 0.9);
    pub const FG_SECONDARY: Color = Color::from_rgb(0.6, 0.6, 0.6);
    pub const FG_MUTED: Color = Color::from_rgb(0.4, 0.4, 0.4);

    // Accents
    pub const ACCENT_PRIMARY: Color = Color::from_rgb(0.2, 0.6, 1.0);
    pub const ACCENT_SECONDARY: Color = Color::from_rgb(0.4, 0.7, 1.0);

    // Status colors
    pub const SUCCESS: Color = Color::from_rgb(0.3, 0.8, 0.5);
    pub const ERROR: Color = Color::from_rgb(0.9, 0.3, 0.3);
    pub const WARNING: Color = Color::from_rgb(0.9, 0.7, 0.2);
    pub const INFO: Color = Color::from_rgb(0.2, 0.6, 1.0);

    // Borders
    pub const BORDER_DEFAULT: Color = Color::from_rgb(0.2, 0.2, 0.22);
    pub const BORDER_FOCUSED: Color = Color::from_rgb(0.3, 0.5, 0.8);

    // ANSI colors
    pub const ANSI_BLACK: Color = Color::from_rgb(0.0, 0.0, 0.0);
    pub const ANSI_RED: Color = Color::from_rgb(0.8, 0.2, 0.2);
    pub const ANSI_GREEN: Color = Color::from_rgb(0.05, 0.74, 0.47);
    pub const ANSI_YELLOW: Color = Color::from_rgb(0.9, 0.9, 0.06);
    pub const ANSI_BLUE: Color = Color::from_rgb(0.14, 0.45, 0.78);
    pub const ANSI_MAGENTA: Color = Color::from_rgb(0.74, 0.25, 0.74);
    pub const ANSI_CYAN: Color = Color::from_rgb(0.07, 0.66, 0.8);
    pub const ANSI_WHITE: Color = Color::from_rgb(0.9, 0.9, 0.9);

    // Bright ANSI colors
    pub const ANSI_BRIGHT_BLACK: Color = Color::from_rgb(0.4, 0.4, 0.4);
    pub const ANSI_BRIGHT_RED: Color = Color::from_rgb(0.95, 0.3, 0.3);
    pub const ANSI_BRIGHT_GREEN: Color = Color::from_rgb(0.14, 0.82, 0.55);
    pub const ANSI_BRIGHT_YELLOW: Color = Color::from_rgb(0.96, 0.96, 0.26);
    pub const ANSI_BRIGHT_BLUE: Color = Color::from_rgb(0.23, 0.56, 0.92);
    pub const ANSI_BRIGHT_MAGENTA: Color = Color::from_rgb(0.84, 0.44, 0.84);
    pub const ANSI_BRIGHT_CYAN: Color = Color::from_rgb(0.16, 0.72, 0.86);
    pub const ANSI_BRIGHT_WHITE: Color = Color::from_rgb(1.0, 1.0, 1.0);
}

impl NexusTheme {
    /// Get ANSI color by index (0-15).
    pub fn ansi_color(index: u8) -> Color {
        match index {
            0 => Self::ANSI_BLACK,
            1 => Self::ANSI_RED,
            2 => Self::ANSI_GREEN,
            3 => Self::ANSI_YELLOW,
            4 => Self::ANSI_BLUE,
            5 => Self::ANSI_MAGENTA,
            6 => Self::ANSI_CYAN,
            7 => Self::ANSI_WHITE,
            8 => Self::ANSI_BRIGHT_BLACK,
            9 => Self::ANSI_BRIGHT_RED,
            10 => Self::ANSI_BRIGHT_GREEN,
            11 => Self::ANSI_BRIGHT_YELLOW,
            12 => Self::ANSI_BRIGHT_BLUE,
            13 => Self::ANSI_BRIGHT_MAGENTA,
            14 => Self::ANSI_BRIGHT_CYAN,
            15 => Self::ANSI_BRIGHT_WHITE,
            _ => Self::FG_PRIMARY,
        }
    }
}
