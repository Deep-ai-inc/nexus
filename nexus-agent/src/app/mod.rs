pub mod acp;
pub mod server;

// NOTE: gpui and terminal modules removed - using Iced for UI instead

use crate::types::ToolSyntax;
use nexus_sandbox::SandboxPolicy;

use std::path::PathBuf;

/// Configuration for running the agent in either terminal or GPUI mode
#[derive(Debug, Clone)]
pub struct AgentRunConfig {
    pub path: PathBuf,
    pub task: Option<String>,
    pub continue_task: bool,
    pub model: String,
    pub tool_syntax: ToolSyntax,
    pub use_diff_format: bool,
    pub record: Option<PathBuf>,
    pub playback: Option<PathBuf>,
    pub fast_playback: bool,
    pub sandbox_policy: SandboxPolicy,
}
