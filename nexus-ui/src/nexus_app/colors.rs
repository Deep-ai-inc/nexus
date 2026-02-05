//! Color palette for the Nexus UI theme.

use strata::primitives::Color;

// Backgrounds
pub const BG_APP: Color = Color { r: 0.07, g: 0.07, b: 0.09, a: 1.0 };
pub const BG_BLOCK: Color = Color { r: 0.12, g: 0.12, b: 0.14, a: 1.0 };
pub const BG_INPUT: Color = Color { r: 0.1, g: 0.1, b: 0.12, a: 1.0 };

// Status
pub const SUCCESS: Color = Color { r: 0.3, g: 0.8, b: 0.5, a: 1.0 };
pub const ERROR: Color = Color { r: 0.9, g: 0.3, b: 0.3, a: 1.0 };
pub const WARNING: Color = Color { r: 0.9, g: 0.7, b: 0.2, a: 1.0 };
pub const RUNNING: Color = Color { r: 0.3, g: 0.7, b: 1.0, a: 1.0 };
pub const THINKING: Color = Color { r: 0.6, g: 0.6, b: 0.7, a: 1.0 };

// Text
pub const TEXT_PRIMARY: Color = Color { r: 0.9, g: 0.9, b: 0.9, a: 1.0 };
pub const TEXT_SECONDARY: Color = Color { r: 0.6, g: 0.6, b: 0.6, a: 1.0 };
pub const TEXT_MUTED: Color = Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 };
pub const TEXT_PATH: Color = Color { r: 0.392, g: 0.584, b: 0.929, a: 1.0 };
pub const TEXT_PURPLE: Color = Color { r: 0.6, g: 0.5, b: 0.9, a: 1.0 };

// Tool colors
pub const TOOL_PENDING: Color = Color { r: 0.6, g: 0.6, b: 0.3, a: 1.0 };
pub const TOOL_OUTPUT: Color = Color { r: 0.8, g: 0.8, b: 0.8, a: 1.0 };
pub const TOOL_ACTION: Color = Color { r: 0.3, g: 0.7, b: 1.0, a: 1.0 };
pub const TOOL_RESULT: Color = Color { r: 0.5, g: 0.5, b: 0.55, a: 1.0 };
pub const TOOL_PATH: Color = Color { r: 0.7, g: 0.7, b: 0.9, a: 1.0 };
pub const TOOL_ARTIFACT_BG: Color = Color { r: 0.09, g: 0.09, b: 0.11, a: 1.0 };
pub const TOOL_BORDER: Color = Color { r: 0.25, g: 0.25, b: 0.35, a: 1.0 };

// Diff colors (subtle alpha-blended tints for native GUI feel)
pub const DIFF_ADD: Color = Color { r: 0.4, g: 0.85, b: 0.5, a: 1.0 };
pub const DIFF_REMOVE: Color = Color { r: 0.9, g: 0.45, b: 0.45, a: 1.0 };
pub const DIFF_BG_ADD: Color = Color { r: 0.2, g: 0.8, b: 0.2, a: 0.08 };
pub const DIFF_BG_REMOVE: Color = Color { r: 0.8, g: 0.2, b: 0.2, a: 0.08 };

// Code blocks
pub const CODE_BG: Color = Color { r: 0.06, g: 0.06, b: 0.08, a: 1.0 };
pub const CODE_TEXT: Color = Color { r: 0.9, g: 0.9, b: 0.9, a: 1.0 };

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
