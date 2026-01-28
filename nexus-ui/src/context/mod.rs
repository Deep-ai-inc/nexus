//! Context System - Nexus's awareness layer.
//!
//! This module provides rich context for:
//! - Smart completions (git branches, npm scripts, etc.)
//! - Error parsing and actionable suggestions
//! - Project-specific agent instructions (NEXUS.md)
//!
//! The Context System complements (not duplicates) Claude Code CLI's context.
//! CLI handles: conversation history, system prompt, context compaction.
//! Nexus handles: instant error parsing, completions, project detection.

mod error_parser;

pub use error_parser::{ErrorKind, ErrorParser, ParsedError, Suggestion};

use std::collections::HashMap;
use std::path::PathBuf;

// =============================================================================
// Core Context Types
// =============================================================================

/// Rich context that flows through Nexus.
#[derive(Debug, Clone, Default)]
pub struct NexusContext {
    /// Current working directory.
    pub cwd: PathBuf,
    /// Git repository context (if in a repo).
    pub git: Option<GitContext>,
    /// Project context (Node, Rust, Python, etc.).
    pub project: Option<ProjectContext>,
    /// Last command interaction (for error parsing).
    pub last_interaction: Option<InteractionContext>,
    /// Environment variables (cached).
    pub env_vars: HashMap<String, String>,
    /// Project-specific instructions from NEXUS.md.
    pub nexus_md: Option<String>,
}

/// Git repository context.
#[derive(Debug, Clone, PartialEq)]
pub struct GitContext {
    /// Repository root.
    pub root: PathBuf,
    /// Current branch name.
    pub branch: String,
    /// Dirty (modified) files.
    pub dirty_files: Vec<PathBuf>,
    /// Is this a bare repo?
    pub is_bare: bool,
}

/// Project type detection.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectKind {
    Node,   // package.json
    Rust,   // Cargo.toml
    Python, // pyproject.toml, setup.py, requirements.txt
    Go,     // go.mod
    Unknown,
}

/// Project context.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectContext {
    /// Detected project type.
    pub kind: ProjectKind,
    /// Project root directory.
    pub root: PathBuf,
    /// Available scripts/tasks (npm scripts, cargo commands, etc.).
    pub scripts: Vec<String>,
}

/// Last command interaction - for error parsing.
#[derive(Debug, Clone)]
pub struct InteractionContext {
    /// The command that was run.
    pub command: String,
    /// Truncated output.
    pub output: String,
    /// Exit code.
    pub exit_code: i32,
    /// Parsed error (if exit_code != 0).
    pub parsed_error: Option<ParsedError>,
}

// =============================================================================
// Context Scanning
// =============================================================================

impl NexusContext {
    /// Create a new context for the given directory.
    pub fn new(cwd: PathBuf) -> Self {
        let mut ctx = Self {
            cwd: cwd.clone(),
            ..Default::default()
        };
        ctx.refresh_sync();
        ctx
    }

    /// Synchronously refresh context (for initialization).
    /// For runtime updates, use the async worker.
    pub fn refresh_sync(&mut self) {
        self.git = scan_git_sync(&self.cwd);
        self.project = scan_project_sync(&self.cwd);
        self.nexus_md = read_nexus_md_sync(&self.cwd);
    }

    /// Update after a command finishes.
    pub fn on_command_finished(&mut self, command: String, output: String, exit_code: i32) {
        let parsed_error = if exit_code != 0 {
            ErrorParser::analyze(&command, &output, self.project.as_ref())
        } else {
            None
        };

        self.last_interaction = Some(InteractionContext {
            command,
            output,
            exit_code,
            parsed_error,
        });
    }

    /// Update CWD and refresh context.
    pub fn set_cwd(&mut self, cwd: PathBuf) {
        if self.cwd != cwd {
            self.cwd = cwd;
            self.refresh_sync();
        }
    }

    /// Get the current suggestion (if any).
    pub fn current_suggestion(&self) -> Option<&Suggestion> {
        self.last_interaction
            .as_ref()
            .and_then(|i| i.parsed_error.as_ref())
            .and_then(|e| e.suggestion.as_ref())
    }
}

// =============================================================================
// Sync Scanners (simple, blocking - for MVP)
// =============================================================================

/// Scan for git repository.
/// Optimized to use minimal git commands (2 instead of 4).
fn scan_git_sync(cwd: &PathBuf) -> Option<GitContext> {
    use std::process::Command;

    // Get root and branch in one command
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let out_str = String::from_utf8_lossy(&output.stdout);
    let mut lines = out_str.lines();
    let root = PathBuf::from(lines.next()?.trim());
    let branch = lines.next().unwrap_or("").trim().to_string();

    // Get dirty files with porcelain status
    let status_output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(cwd)
        .output()
        .ok()?;

    let dirty_files: Vec<PathBuf> = String::from_utf8_lossy(&status_output.stdout)
        .lines()
        .filter_map(|line| {
            if line.len() > 3 {
                Some(PathBuf::from(line[3..].trim()))
            } else {
                None
            }
        })
        .collect();

    Some(GitContext {
        root,
        branch,
        dirty_files,
        is_bare: false,
    })
}

/// Scan for project type.
fn scan_project_sync(cwd: &PathBuf) -> Option<ProjectContext> {
    // Walk up to find project root
    let mut current = cwd.clone();

    loop {
        // Node.js
        let package_json = current.join("package.json");
        if package_json.exists() {
            let scripts = parse_npm_scripts(&package_json).unwrap_or_default();
            return Some(ProjectContext {
                kind: ProjectKind::Node,
                root: current,
                scripts,
            });
        }

        // Rust
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists() {
            return Some(ProjectContext {
                kind: ProjectKind::Rust,
                root: current,
                scripts: vec![
                    "build".into(),
                    "run".into(),
                    "test".into(),
                    "check".into(),
                ],
            });
        }

        // Python
        let pyproject = current.join("pyproject.toml");
        let setup_py = current.join("setup.py");
        let requirements = current.join("requirements.txt");
        if pyproject.exists() || setup_py.exists() || requirements.exists() {
            return Some(ProjectContext {
                kind: ProjectKind::Python,
                root: current,
                scripts: vec![], // Could parse pyproject.toml scripts
            });
        }

        // Go
        let go_mod = current.join("go.mod");
        if go_mod.exists() {
            return Some(ProjectContext {
                kind: ProjectKind::Go,
                root: current,
                scripts: vec!["build".into(), "run".into(), "test".into()],
            });
        }

        // Walk up
        if !current.pop() {
            break;
        }
    }

    None
}

/// Parse npm scripts from package.json.
fn parse_npm_scripts(path: &PathBuf) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let scripts = json.get("scripts")?.as_object()?;
    Some(scripts.keys().cloned().collect())
}

/// Read NEXUS.md from project root.
fn read_nexus_md_sync(cwd: &PathBuf) -> Option<String> {
    // Walk up to find NEXUS.md
    let mut current = cwd.clone();

    loop {
        let nexus_md = current.join("NEXUS.md");
        if nexus_md.exists() {
            return std::fs::read_to_string(&nexus_md).ok();
        }

        // Also check .nexus/NEXUS.md
        let dot_nexus_md = current.join(".nexus").join("NEXUS.md");
        if dot_nexus_md.exists() {
            return std::fs::read_to_string(&dot_nexus_md).ok();
        }

        if !current.pop() {
            break;
        }
    }

    None
}
