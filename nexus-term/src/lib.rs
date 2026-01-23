//! Nexus Term - Headless terminal state management.
//!
//! This crate wraps alacritty_terminal to parse ANSI escape sequences
//! and maintain a terminal grid state, without any rendering.

mod grid;
mod parser;
mod cell;

pub use grid::TerminalGrid;
pub use parser::TerminalParser;
pub use cell::{Cell, CellFlags, Color};

/// Default terminal dimensions.
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;
