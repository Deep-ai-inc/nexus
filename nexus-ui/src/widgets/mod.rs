//! Widget modules for Nexus UI.
//!
//! Each widget handles its own view and (where applicable) update logic,
//! keeping rendering code co-located with the data it operates on.

mod view_context;
mod shell_block;
mod tool;

pub(crate) use view_context::ViewContext;
pub use shell_block::{ShellBlockWidget, ShellBlockMessage};
pub use tool::{ToolWidget, ToolMessage};
