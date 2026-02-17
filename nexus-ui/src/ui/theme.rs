//! Color palette for the Nexus UI theme.

use strata::primitives::Color;

// Backgrounds (darker charcoal matching Claude Code)
pub const BG_APP: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
pub const BG_BLOCK: Color = Color { r: 0.09, g: 0.09, b: 0.1, a: 1.0 };
pub const BG_INPUT: Color = Color { r: 0.08, g: 0.08, b: 0.09, a: 1.0 };

// Status (softer tones matching Claude Code)
pub const SUCCESS: Color = Color { r: 0.4, g: 0.75, b: 0.45, a: 1.0 };
pub const ERROR: Color = Color { r: 0.9, g: 0.4, b: 0.4, a: 1.0 };
pub const WARNING: Color = Color { r: 0.9, g: 0.7, b: 0.3, a: 1.0 };
pub const RUNNING: Color = Color { r: 0.5, g: 0.6, b: 0.7, a: 1.0 };
pub const THINKING: Color = Color { r: 0.5, g: 0.5, b: 0.55, a: 1.0 };

// Text (softer grays matching Claude Code)
pub const TEXT_PRIMARY: Color = Color { r: 0.85, g: 0.85, b: 0.85, a: 1.0 };
pub const TEXT_SECONDARY: Color = Color { r: 0.55, g: 0.55, b: 0.55, a: 1.0 };
pub const TEXT_MUTED: Color = Color { r: 0.4, g: 0.4, b: 0.42, a: 1.0 };
pub const TEXT_PATH: Color = Color { r: 0.4, g: 0.6, b: 0.9, a: 1.0 };
pub const TEXT_PURPLE: Color = Color { r: 0.6, g: 0.5, b: 0.85, a: 1.0 };

// Tool colors (cyan accent matching Claude Code)
pub const TOOL_PENDING: Color = Color { r: 0.7, g: 0.65, b: 0.3, a: 1.0 };
pub const TOOL_OUTPUT: Color = Color { r: 0.8, g: 0.8, b: 0.8, a: 1.0 };
pub const TOOL_ACTION: Color = Color { r: 0.35, g: 0.7, b: 0.9, a: 1.0 };
pub const TOOL_RESULT: Color = Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 };
pub const TOOL_PATH: Color = Color { r: 0.65, g: 0.65, b: 0.85, a: 1.0 };
pub const TOOL_ARTIFACT_BG: Color = Color { r: 0.06, g: 0.06, b: 0.07, a: 1.0 };
pub const TOOL_BORDER: Color = Color { r: 0.2, g: 0.2, b: 0.22, a: 1.0 };

// Diff colors (subtle alpha-blended tints for native GUI feel)
pub const DIFF_ADD: Color = Color { r: 0.4, g: 0.85, b: 0.5, a: 1.0 };
pub const DIFF_REMOVE: Color = Color { r: 0.9, g: 0.45, b: 0.45, a: 1.0 };
pub const DIFF_BG_ADD: Color = Color { r: 0.2, g: 0.8, b: 0.2, a: 0.08 };
pub const DIFF_BG_REMOVE: Color = Color { r: 0.8, g: 0.2, b: 0.2, a: 0.08 };

// Code blocks
pub const CODE_BG: Color = Color { r: 0.05, g: 0.05, b: 0.06, a: 1.0 };
pub const CODE_TEXT: Color = Color { r: 0.85, g: 0.85, b: 0.85, a: 1.0 };

// Buttons
pub const BTN_DENY: Color = Color { r: 0.6, g: 0.15, b: 0.15, a: 1.0 };
pub const BTN_ALLOW: Color = Color { r: 0.15, g: 0.5, b: 0.25, a: 1.0 };
pub const BTN_ALWAYS: Color = Color { r: 0.1, g: 0.35, b: 0.18, a: 1.0 };
pub const BTN_KILL: Color = Color { r: 0.7, g: 0.2, b: 0.2, a: 1.0 };

// Borders
pub const BORDER_INPUT: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.08 };

// Welcome screen
pub const WELCOME_TITLE: Color = Color { r: 0.6, g: 0.8, b: 0.6, a: 1.0 };
pub const WELCOME_HEADING: Color = Color { r: 0.8, g: 0.7, b: 0.5, a: 1.0 };
pub const CARD_BG: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.03 };
pub const CARD_BORDER: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.06 };
