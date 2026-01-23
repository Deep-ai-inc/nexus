//! Provider protocol - JSON-RPC communication with provider processes.

use serde::{Deserialize, Serialize};

/// A request from the shell to a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum ProviderRequest {
    /// Get completions for the current command line.
    GetCompletions {
        command_line: String,
        cursor_position: usize,
    },

    /// Get documentation for the argument at cursor.
    GetDocumentation {
        command_line: String,
        cursor_position: usize,
    },

    /// Get sidecar command to run alongside user command.
    GetSidecar { command: String },

    /// Parse sidecar output into structured data.
    ParseSidecarOutput { output: String },

    /// Get available actions for an anchor.
    GetActions { anchor: Anchor },
}

/// A response from a provider to the shell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ProviderResponse {
    Completions(Vec<Completion>),
    Documentation(Option<Documentation>),
    Sidecar(Option<SidecarSpec>),
    ParsedOutput(ParsedSidecarOutput),
    Actions(Vec<Action>),
    Error { message: String },
}

/// A completion candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Completion {
    pub text: String,
    pub display: String,
    pub description: Option<String>,
    pub kind: CompletionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompletionKind {
    Command,
    File,
    Directory,
    Flag,
    Argument,
    Variable,
    Branch,
    Remote,
    Container,
    Other,
}

/// Documentation for a command or argument.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Documentation {
    pub summary: String,
    pub details: Option<String>,
    pub url: Option<String>,
}

/// Specification for a sidecar command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarSpec {
    pub argv: Vec<String>,
    pub env: Option<Vec<(String, String)>>,
}

/// Result of parsing sidecar output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedSidecarOutput {
    pub anchors: Vec<Anchor>,
    pub structured_data: Option<serde_json::Value>,
}

/// An anchor - an interactive element detected in output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Anchor {
    pub id: String,
    pub kind: AnchorKind,
    pub text: String,
    pub range: (usize, usize), // byte range in output
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorKind {
    FilePath,
    Url,
    GitCommit,
    GitBranch,
    ContainerId,
    IpAddress,
    Other,
}

/// An action that can be performed on an anchor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub argv: Vec<String>,
}
