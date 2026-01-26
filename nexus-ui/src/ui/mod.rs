//! UI rendering modules for the Nexus application.
//!
//! This module contains the view layer, separated by concern:
//! - `value_view`: Rendering structured data (Values, Media, Tables)
//! - `shell_view`: Rendering shell command blocks
//! - `input`: Rendering the input area with completions and popups

pub mod input;
pub mod shell_view;
pub mod value_view;

pub use input::view_input;
pub use shell_view::view_block;
pub use value_view::{render_file_list, render_media, render_value};
