//! Context Providers - modular error parsing and context enrichment.
//!
//! Each provider handles a specific domain (Node, Rust, Python, System).
//! This replaces the monolithic error_parser.rs with a trait-based system.

use super::{ProjectContext, ProjectKind};

// =============================================================================
// Core Types
// =============================================================================

/// Parsed error with optional suggestion.
#[derive(Debug, Clone)]
pub struct ParsedError {
    /// What kind of error this is.
    pub kind: ErrorKind,
    /// The raw error message (most relevant line).
    pub message: String,
    /// Actionable suggestion (if we can figure one out).
    pub suggestion: Option<Suggestion>,
}

/// Known error categories.
#[derive(Debug, Clone, PartialEq)]
pub enum ErrorKind {
    /// Permission denied (EACCES).
    PermissionDenied,
    /// Command not found.
    CommandNotFound { command: String },
    /// File or directory not found (ENOENT).
    FileNotFound { path: String },
    /// Module/package not found.
    ModuleNotFound { module: String },
    /// Port already in use.
    PortInUse { port: u16 },
    /// Compilation error.
    CompileError,
    /// Generic error (we parsed something but don't have a specific fix).
    Other,
}

/// An actionable suggestion.
#[derive(Debug, Clone)]
pub struct Suggestion {
    /// Human-readable label for the button.
    pub label: String,
    /// Command to run to fix the issue.
    pub command: String,
}

// =============================================================================
// Provider Trait
// =============================================================================

/// A context provider handles error parsing for a specific domain.
///
/// Providers are modular and can be extended without touching other code.
pub trait ContextProvider: Send + Sync {
    /// Provider name for debugging.
    fn name(&self) -> &'static str;

    /// Check if this provider applies to the current project.
    fn applies_to(&self, project: Option<&ProjectContext>) -> bool;

    /// Attempt to parse an error from command output.
    /// Returns None if this provider doesn't recognize the error.
    fn parse_error(
        &self,
        command: &str,
        output: &str,
        project: Option<&ProjectContext>,
    ) -> Option<ParsedError>;

    /// Generate context snippet for AI prompts.
    /// This is used to enrich the system prompt when the user engages the AI.
    fn context_prompt(&self, project: Option<&ProjectContext>) -> Option<String> {
        let _ = project;
        None
    }
}

// =============================================================================
// Provider Registry
// =============================================================================

/// Registry of all context providers.
pub struct ProviderRegistry {
    providers: Vec<Box<dyn ContextProvider>>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistry {
    /// Create a new registry with all built-in providers.
    pub fn new() -> Self {
        Self {
            providers: vec![
                Box::new(SystemProvider),
                Box::new(NodeProvider),
                Box::new(PythonProvider),
                Box::new(RustProvider),
            ],
        }
    }

    /// Analyze command output using all applicable providers.
    /// Returns the first match (providers are ordered by priority).
    pub fn analyze(
        &self,
        command: &str,
        output: &str,
        project: Option<&ProjectContext>,
    ) -> Option<ParsedError> {
        for provider in &self.providers {
            if let Some(error) = provider.parse_error(command, output, project) {
                return Some(error);
            }
        }
        None
    }

    /// Build context prompt from all applicable providers.
    pub fn build_context_prompt(&self, project: Option<&ProjectContext>) -> String {
        let mut parts = Vec::new();
        for provider in &self.providers {
            if let Some(prompt) = provider.context_prompt(project) {
                parts.push(prompt);
            }
        }
        parts.join("\n")
    }
}

// =============================================================================
// System Provider (permissions, ports, command not found)
// =============================================================================

/// Handles system-level errors that apply regardless of project type.
pub struct SystemProvider;

impl ContextProvider for SystemProvider {
    fn name(&self) -> &'static str {
        "system"
    }

    fn applies_to(&self, _project: Option<&ProjectContext>) -> bool {
        true // Always applies
    }

    fn parse_error(
        &self,
        _command: &str,
        output: &str,
        project: Option<&ProjectContext>,
    ) -> Option<ParsedError> {
        let output_lower = output.to_lowercase();

        // NOTE: Permission denied is handled by the legacy system in terminal.rs
        // which provides a better UX with Ctrl+S shortcut hint. Skip it here.

        // Port in use
        if output_lower.contains("eaddrinuse") || output_lower.contains("address already in use") {
            let port = extract_port(output).unwrap_or(0);
            return Some(ParsedError {
                kind: ErrorKind::PortInUse { port },
                message: extract_error_line(output),
                suggestion: if port > 0 {
                    Some(Suggestion {
                        label: format!("Kill port {}", port),
                        command: format!("lsof -ti:{} | xargs kill -9", port),
                    })
                } else {
                    None
                },
            });
        }

        // Command not found
        if output_lower.contains("command not found") {
            if let Some(cmd) = extract_command_name(output) {
                let suggestion = suggest_install(&cmd, project);
                return Some(ParsedError {
                    kind: ErrorKind::CommandNotFound { command: cmd },
                    message: extract_error_line(output),
                    suggestion,
                });
            }
        }

        // File not found (generic)
        if output_lower.contains("enoent") || output_lower.contains("no such file or directory") {
            let path = extract_quoted_string(output).unwrap_or_default();
            return Some(ParsedError {
                kind: ErrorKind::FileNotFound { path },
                message: extract_error_line(output),
                suggestion: None,
            });
        }

        None
    }
}

// =============================================================================
// Node.js Provider
// =============================================================================

/// Handles Node.js and npm specific errors.
pub struct NodeProvider;

impl ContextProvider for NodeProvider {
    fn name(&self) -> &'static str {
        "node"
    }

    fn applies_to(&self, project: Option<&ProjectContext>) -> bool {
        matches!(project, Some(p) if p.kind == ProjectKind::Node)
    }

    fn parse_error(
        &self,
        _command: &str,
        output: &str,
        project: Option<&ProjectContext>,
    ) -> Option<ParsedError> {
        let output_lower = output.to_lowercase();

        // Module not found
        if output_lower.contains("cannot find module")
            || output_lower.contains("module_not_found")
            || output_lower.contains("err_module_not_found")
        {
            let module = extract_quoted_string(output).unwrap_or_default();

            // Only suggest npm install for npm packages, not local files
            let suggestion = if !module.starts_with('.') && !module.starts_with('/') {
                if self.applies_to(project) {
                    Some(Suggestion {
                        label: "Run npm install".into(),
                        command: "npm install".into(),
                    })
                } else {
                    None
                }
            } else {
                None
            };

            return Some(ParsedError {
                kind: ErrorKind::ModuleNotFound { module },
                message: extract_error_line(output),
                suggestion,
            });
        }

        None
    }

    fn context_prompt(&self, project: Option<&ProjectContext>) -> Option<String> {
        if let Some(p) = project {
            if p.kind == ProjectKind::Node && !p.scripts.is_empty() {
                return Some(format!(
                    "Project: Node.js\nAvailable scripts: {}",
                    p.scripts.join(", ")
                ));
            }
        }
        None
    }
}

// =============================================================================
// Python Provider
// =============================================================================

/// Handles Python specific errors.
pub struct PythonProvider;

impl ContextProvider for PythonProvider {
    fn name(&self) -> &'static str {
        "python"
    }

    fn applies_to(&self, project: Option<&ProjectContext>) -> bool {
        matches!(project, Some(p) if p.kind == ProjectKind::Python)
    }

    fn parse_error(
        &self,
        _command: &str,
        output: &str,
        project: Option<&ProjectContext>,
    ) -> Option<ParsedError> {
        let output_lower = output.to_lowercase();

        // Module not found
        if output_lower.contains("modulenotfounderror") || output_lower.contains("no module named")
        {
            let module = extract_quoted_string(output).unwrap_or_default();

            let suggestion = if self.applies_to(project) && !module.is_empty() {
                Some(Suggestion {
                    label: format!("pip install {}", module),
                    command: format!("pip install {}", module),
                })
            } else {
                None
            };

            return Some(ParsedError {
                kind: ErrorKind::ModuleNotFound { module },
                message: extract_error_line(output),
                suggestion,
            });
        }

        None
    }

    fn context_prompt(&self, project: Option<&ProjectContext>) -> Option<String> {
        if let Some(p) = project {
            if p.kind == ProjectKind::Python {
                return Some("Project: Python".into());
            }
        }
        None
    }
}

// =============================================================================
// Rust Provider
// =============================================================================

/// Handles Rust and Cargo specific errors.
pub struct RustProvider;

impl ContextProvider for RustProvider {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn applies_to(&self, project: Option<&ProjectContext>) -> bool {
        matches!(project, Some(p) if p.kind == ProjectKind::Rust)
    }

    fn parse_error(
        &self,
        _command: &str,
        output: &str,
        _project: Option<&ProjectContext>,
    ) -> Option<ParsedError> {
        let output_lower = output.to_lowercase();

        // Compilation error
        if output_lower.contains("could not compile") || output.contains("error[E") {
            // Rust errors are complex - don't try to auto-fix, but do recognize them
            return Some(ParsedError {
                kind: ErrorKind::CompileError,
                message: extract_error_line(output),
                suggestion: None,
            });
        }

        None
    }

    fn context_prompt(&self, project: Option<&ProjectContext>) -> Option<String> {
        if let Some(p) = project {
            if p.kind == ProjectKind::Rust {
                return Some("Project: Rust (Cargo)".into());
            }
        }
        None
    }
}

// =============================================================================
// Extraction Helpers
// =============================================================================

/// Extract the most relevant error line from output.
/// Searches from the end since errors typically appear last.
fn extract_error_line(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();

    // First pass: lines starting with error prefixes
    for line in lines.iter().rev() {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();
        if lower.starts_with("error:")
            || lower.starts_with("fatal:")
            || lower.starts_with("error[")
        {
            return trimmed.to_string();
        }
    }

    // Second pass: lines containing error keywords
    for line in lines.iter().rev() {
        let lower = line.to_lowercase();
        if lower.contains("error")
            || lower.contains("failed")
            || lower.contains("denied")
            || lower.contains("not found")
        {
            return line.trim().to_string();
        }
    }

    // Fallback to last non-empty line
    lines
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(&"")
        .trim()
        .to_string()
}

/// Extract a quoted string (single or double quotes) from text.
fn extract_quoted_string(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(start) = line.find('\'').or_else(|| line.find('"')) {
            let quote_char = line.chars().nth(start)?;
            let rest = &line[start + 1..];
            if let Some(end) = rest.find(quote_char) {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

/// Extract command name from "command not found" error.
fn extract_command_name(output: &str) -> Option<String> {
    for line in output.lines() {
        if line.contains("command not found") {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 2 {
                let cmd = parts[parts.len() - 2].trim();
                if !cmd.is_empty() && !cmd.contains(' ') {
                    return Some(cmd.to_string());
                }
            }
        }
    }
    None
}

/// Extract port number from error output.
fn extract_port(output: &str) -> Option<u16> {
    for line in output.lines() {
        // Pattern: ":::3000" or ":3000" - find any colon followed by digits
        let chars: Vec<char> = line.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            if c == ':' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
                let port_str: String = chars[i + 1..]
                    .iter()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(port) = port_str.parse::<u16>() {
                    if port > 0 {
                        return Some(port);
                    }
                }
            }
        }

        // Pattern: "port 3000"
        if let Some(idx) = line.to_lowercase().find("port ") {
            let rest = &line[idx + 5..];
            let port_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(port) = port_str.parse::<u16>() {
                if port > 0 {
                    return Some(port);
                }
            }
        }
    }
    None
}

/// Suggest installation command for missing command.
fn suggest_install(cmd: &str, project: Option<&ProjectContext>) -> Option<Suggestion> {
    #[cfg(target_os = "macos")]
    let suggestions: &[(&str, &str, &str)] = &[
        ("node", "Install Node.js", "brew install node"),
        ("npm", "Install Node.js", "brew install node"),
        ("npx", "Install Node.js", "brew install node"),
        ("python", "Install Python", "brew install python"),
        ("python3", "Install Python", "brew install python"),
        ("pip", "Install Python", "brew install python"),
        (
            "cargo",
            "Install Rust",
            "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh",
        ),
        (
            "rustc",
            "Install Rust",
            "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh",
        ),
        ("go", "Install Go", "brew install go"),
        ("docker", "Install Docker", "brew install --cask docker"),
        ("git", "Install Git", "brew install git"),
        ("make", "Install make", "xcode-select --install"),
        ("gcc", "Install GCC", "xcode-select --install"),
    ];

    #[cfg(target_os = "linux")]
    let suggestions: &[(&str, &str, &str)] = &[
        (
            "cargo",
            "Install Rust",
            "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh",
        ),
        (
            "rustc",
            "Install Rust",
            "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh",
        ),
    ];

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let suggestions: &[(&str, &str, &str)] = &[];

    for (name, label, install_cmd) in suggestions {
        if cmd == *name {
            return Some(Suggestion {
                label: label.to_string(),
                command: install_cmd.to_string(),
            });
        }
    }

    // Script suggestion for Node projects
    if let Some(p) = project {
        if p.kind == ProjectKind::Node && p.scripts.contains(&cmd.to_string()) {
            return Some(Suggestion {
                label: format!("npm run {}", cmd),
                command: format!("npm run {}", cmd),
            });
        }
    }

    None
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn node_project() -> ProjectContext {
        ProjectContext {
            kind: ProjectKind::Node,
            root: PathBuf::from("/tmp"),
            scripts: vec!["start".into(), "test".into()],
        }
    }

    // NOTE: Permission denied is handled by legacy system in terminal.rs,
    // not by the Context System providers.

    #[test]
    fn test_command_not_found() {
        let registry = ProviderRegistry::new();
        let result = registry.analyze("foo", "bash: foo: command not found", None);
        assert!(result.is_some());
        assert!(matches!(
            result.unwrap().kind,
            ErrorKind::CommandNotFound { .. }
        ));
    }

    #[test]
    fn test_node_module_not_found() {
        let registry = ProviderRegistry::new();
        let project = node_project();
        let result = registry.analyze(
            "node index.js",
            "Error: Cannot find module 'express'",
            Some(&project),
        );
        assert!(result.is_some());
        let err = result.unwrap();
        assert!(matches!(err.kind, ErrorKind::ModuleNotFound { .. }));
        assert_eq!(err.suggestion.unwrap().command, "npm install");
    }

    #[test]
    fn test_port_in_use() {
        let registry = ProviderRegistry::new();
        let result = registry.analyze(
            "npm start",
            "Error: listen EADDRINUSE: address already in use :::3000",
            None,
        );
        assert!(result.is_some());
        assert!(matches!(
            result.unwrap().kind,
            ErrorKind::PortInUse { port: 3000 }
        ));
    }

    #[test]
    fn test_context_prompt() {
        let registry = ProviderRegistry::new();
        let project = node_project();
        let prompt = registry.build_context_prompt(Some(&project));
        assert!(prompt.contains("Node.js"));
        assert!(prompt.contains("start"));
    }
}
