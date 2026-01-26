//! Shared constants for the Nexus UI.
//!
//! Single source of truth for terminal rendering metrics and other constants.

/// Default font size for terminal text.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

/// Line height multiplier.
pub const LINE_HEIGHT_FACTOR: f32 = 1.4;

/// Character width ratio relative to font size.
pub const CHAR_WIDTH_RATIO: f32 = 0.607; // ~8.5/14.0, conservative for anti-aliasing

/// Scrollable ID for auto-scrolling the history.
pub const HISTORY_SCROLLABLE: &str = "history";
