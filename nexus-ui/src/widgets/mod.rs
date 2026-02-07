//! Widget modules for Nexus UI.
//!
//! Each widget handles its own view and (where applicable) update logic,
//! keeping rendering code co-located with the data it operates on.

mod view_context;
mod shell_block;
mod tool;
mod value_renderer;
mod agent_block;
mod input;
mod job_bar;
mod welcome;

pub(crate) use view_context::ViewContext;
pub use shell_block::{ShellBlockWidget, ShellBlockMessage};
pub use tool::{ToolWidget, ToolMessage};
pub use agent_block::{AgentBlockWidget, AgentBlockMessage};
pub(crate) use value_renderer::{render_native_value, term_color_to_strata, is_anchor_value, format_eta};
pub use input::{NexusInputBar, CompletionPopup, HistorySearchBar};
pub use job_bar::JobBar;
pub use welcome::WelcomeScreen;
