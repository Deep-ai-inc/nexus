//! Error Parser - instant analysis of command failures.
//!
//! This is Tier 1 support: fast, regex-based parsing that provides
//! actionable suggestions without calling the AI.

use super::ProjectContext;
use super::ProjectKind;

/// Parsed error with optional suggestion.
#[derive(Debug, Clone)]
pub struct ParsedError {
    /// What kind of error this is.
    pub kind: ErrorKind,
    /// The raw error message (first relevant line).
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

/// Error parser - stateless analysis.
pub struct ErrorParser;

impl ErrorParser {
    /// Analyze command output and return a parsed error if applicable.
    pub fn analyze(
        command: &str,
        output: &str,
        project: Option<&ProjectContext>,
    ) -> Option<ParsedError> {
        let output_lower = output.to_lowercase();

        // Permission denied
        if output_lower.contains("permission denied") || output_lower.contains("eacces") {
            return Some(ParsedError {
                kind: ErrorKind::PermissionDenied,
                message: extract_first_error_line(output),
                suggestion: Some(Suggestion {
                    label: "Retry with sudo".into(),
                    command: format!("sudo {}", command),
                }),
            });
        }

        // Command not found
        if output_lower.contains("command not found") || output_lower.contains("not found") {
            if let Some(cmd) = extract_command_name(output) {
                let suggestion = suggest_install(&cmd, project);
                return Some(ParsedError {
                    kind: ErrorKind::CommandNotFound {
                        command: cmd.clone(),
                    },
                    message: extract_first_error_line(output),
                    suggestion,
                });
            }
        }

        // Node.js: Cannot find module / MODULE_NOT_FOUND
        if output_lower.contains("cannot find module")
            || output_lower.contains("module_not_found")
            || output_lower.contains("err_module_not_found")
        {
            let module = extract_module_name(output).unwrap_or_default();
            let suggestion = if matches!(project, Some(p) if p.kind == ProjectKind::Node) {
                // Check if it's a local module vs npm package
                if module.starts_with('.') || module.starts_with('/') {
                    None // Local file, can't auto-fix
                } else {
                    Some(Suggestion {
                        label: "Run npm install".into(),
                        command: "npm install".into(),
                    })
                }
            } else {
                None
            };

            return Some(ParsedError {
                kind: ErrorKind::ModuleNotFound { module },
                message: extract_first_error_line(output),
                suggestion,
            });
        }

        // Python: ModuleNotFoundError / No module named
        if output_lower.contains("modulenotfounderror") || output_lower.contains("no module named")
        {
            let module = extract_python_module(output).unwrap_or_default();
            let suggestion = if matches!(project, Some(p) if p.kind == ProjectKind::Python) {
                Some(Suggestion {
                    label: format!("pip install {}", module),
                    command: format!("pip install {}", module),
                })
            } else {
                None
            };

            return Some(ParsedError {
                kind: ErrorKind::ModuleNotFound { module },
                message: extract_first_error_line(output),
                suggestion,
            });
        }

        // Rust: could not compile / error[E
        if output_lower.contains("could not compile") || output.contains("error[E") {
            // Rust errors are complex, don't try to auto-fix
            return Some(ParsedError {
                kind: ErrorKind::Other,
                message: extract_first_error_line(output),
                suggestion: None,
            });
        }

        // ENOENT / No such file or directory
        if output_lower.contains("enoent") || output_lower.contains("no such file or directory") {
            let path = extract_path_from_enoent(output).unwrap_or_default();
            return Some(ParsedError {
                kind: ErrorKind::FileNotFound { path },
                message: extract_first_error_line(output),
                suggestion: None, // Too generic to suggest
            });
        }

        // Port already in use (EADDRINUSE)
        if output_lower.contains("eaddrinuse") || output_lower.contains("address already in use") {
            let port = extract_port(output).unwrap_or(0);
            return Some(ParsedError {
                kind: ErrorKind::PortInUse { port },
                message: extract_first_error_line(output),
                suggestion: if port > 0 {
                    Some(Suggestion {
                        label: format!("Kill process on port {}", port),
                        command: format!("lsof -ti:{} | xargs kill -9", port),
                    })
                } else {
                    None
                },
            });
        }

        // No specific match
        None
    }
}

// =============================================================================
// Extraction Helpers
// =============================================================================

/// Extract the most relevant error line from output.
/// Searches from the end since errors typically appear last.
/// Prioritizes lines starting with "Error:" or "fatal:".
fn extract_first_error_line(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();

    // First pass: look for lines starting with common error prefixes (from end)
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

    // Second pass: look for lines containing error keywords (from end)
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

/// Extract command name from "command not found" error.
fn extract_command_name(output: &str) -> Option<String> {
    // Pattern: "foo: command not found" or "bash: foo: command not found"
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

/// Suggest installation command for missing command.
fn suggest_install(cmd: &str, project: Option<&ProjectContext>) -> Option<Suggestion> {
    // Platform-specific installation suggestions
    #[cfg(target_os = "macos")]
    let suggestions: &[(&str, &str, &str)] = &[
        ("node", "Install Node.js", "brew install node"),
        ("npm", "Install Node.js", "brew install node"),
        ("npx", "Install Node.js", "brew install node"),
        ("python", "Install Python", "brew install python"),
        ("python3", "Install Python", "brew install python"),
        ("pip", "Install Python", "brew install python"),
        ("cargo", "Install Rust", "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"),
        ("rustc", "Install Rust", "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"),
        ("go", "Install Go", "brew install go"),
        ("docker", "Install Docker", "brew install --cask docker"),
        ("git", "Install Git", "brew install git"),
        ("make", "Install make", "xcode-select --install"),
        ("gcc", "Install GCC", "xcode-select --install"),
    ];

    #[cfg(target_os = "linux")]
    let suggestions: &[(&str, &str, &str)] = &[
        // Cross-platform installers only - avoid distro-specific package managers
        ("cargo", "Install Rust", "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"),
        ("rustc", "Install Rust", "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"),
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

    // If in a Node project and command looks like a script, suggest npm install
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

/// Extract module name from Node.js error.
fn extract_module_name(output: &str) -> Option<String> {
    // Pattern: Cannot find module 'foo' or "foo"
    for line in output.lines() {
        if line.contains("Cannot find module") || line.contains("cannot find module") {
            if let Some(start) = line.find('\'').or_else(|| line.find('"')) {
                let rest = &line[start + 1..];
                if let Some(end) = rest.find('\'').or_else(|| rest.find('"')) {
                    return Some(rest[..end].to_string());
                }
            }
        }
    }
    None
}

/// Extract module name from Python error.
fn extract_python_module(output: &str) -> Option<String> {
    // Pattern: No module named 'foo' or ModuleNotFoundError: No module named 'foo'
    for line in output.lines() {
        if line.contains("No module named") || line.contains("no module named") {
            if let Some(start) = line.find('\'').or_else(|| line.find('"')) {
                let rest = &line[start + 1..];
                if let Some(end) = rest.find('\'').or_else(|| rest.find('"')) {
                    return Some(rest[..end].to_string());
                }
            }
        }
    }
    None
}

/// Extract path from ENOENT error.
fn extract_path_from_enoent(output: &str) -> Option<String> {
    // Pattern: ENOENT: no such file or directory, open '/path/to/file'
    for line in output.lines() {
        if line.contains("ENOENT") || line.contains("No such file") {
            if let Some(start) = line.find('\'').or_else(|| line.find('"')) {
                let rest = &line[start + 1..];
                if let Some(end) = rest.find('\'').or_else(|| rest.find('"')) {
                    return Some(rest[..end].to_string());
                }
            }
        }
    }
    None
}

/// Extract port number from EADDRINUSE error.
fn extract_port(output: &str) -> Option<u16> {
    // Pattern: "port 3000" or ":3000"
    for line in output.lines() {
        // Look for :NNNN pattern
        if let Some(idx) = line.find(':') {
            let rest = &line[idx + 1..];
            let port_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(port) = port_str.parse::<u16>() {
                if port > 0 {
                    return Some(port);
                }
            }
        }
        // Look for "port NNNN" pattern
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_denied() {
        let result = ErrorParser::analyze(
            "rm /etc/passwd",
            "rm: /etc/passwd: Permission denied",
            None,
        );
        assert!(result.is_some());
        let err = result.unwrap();
        assert!(matches!(err.kind, ErrorKind::PermissionDenied));
        assert!(err.suggestion.is_some());
        assert!(err.suggestion.unwrap().command.starts_with("sudo"));
    }

    #[test]
    fn test_command_not_found() {
        let result = ErrorParser::analyze("foo", "bash: foo: command not found", None);
        assert!(result.is_some());
        let err = result.unwrap();
        assert!(matches!(err.kind, ErrorKind::CommandNotFound { .. }));
    }

    #[test]
    fn test_node_module_not_found() {
        let project = ProjectContext {
            kind: ProjectKind::Node,
            root: "/tmp".into(),
            scripts: vec![],
        };
        let result = ErrorParser::analyze(
            "node index.js",
            "Error: Cannot find module 'express'",
            Some(&project),
        );
        assert!(result.is_some());
        let err = result.unwrap();
        assert!(matches!(err.kind, ErrorKind::ModuleNotFound { .. }));
        assert!(err.suggestion.is_some());
        assert_eq!(err.suggestion.unwrap().command, "npm install");
    }

    #[test]
    fn test_port_in_use() {
        let result = ErrorParser::analyze(
            "npm start",
            "Error: listen EADDRINUSE: address already in use :::3000",
            None,
        );
        assert!(result.is_some());
        let err = result.unwrap();
        assert!(matches!(err.kind, ErrorKind::PortInUse { port: 3000 }));
        assert!(err.suggestion.is_some());
    }
}
