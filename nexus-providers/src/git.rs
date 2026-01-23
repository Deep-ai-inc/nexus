//! Git provider - intelligence for git commands.

use nexus_api::{
    Anchor, AnchorKind, Completion, CompletionKind, Documentation, ParsedSidecarOutput,
    SidecarSpec,
};

use crate::Provider;

/// Provider for git commands.
pub struct GitProvider {
    /// Cached branch names.
    cached_branches: Vec<String>,
}

impl GitProvider {
    pub fn new() -> Self {
        Self {
            cached_branches: Vec::new(),
        }
    }

    /// Get local branches.
    fn get_branches(&self) -> Vec<String> {
        // In a real implementation, this would call `git branch`
        // For now, return cached or empty
        self.cached_branches.clone()
    }

    /// Get git subcommands.
    fn subcommands() -> &'static [(&'static str, &'static str)] {
        &[
            ("add", "Add file contents to the index"),
            ("branch", "List, create, or delete branches"),
            ("checkout", "Switch branches or restore working tree files"),
            ("commit", "Record changes to the repository"),
            ("diff", "Show changes between commits, commit and working tree, etc"),
            ("fetch", "Download objects and refs from another repository"),
            ("init", "Create an empty Git repository"),
            ("log", "Show commit logs"),
            ("merge", "Join two or more development histories together"),
            ("pull", "Fetch from and integrate with another repository"),
            ("push", "Update remote refs along with associated objects"),
            ("rebase", "Reapply commits on top of another base tip"),
            ("reset", "Reset current HEAD to the specified state"),
            ("restore", "Restore working tree files"),
            ("stash", "Stash the changes in a dirty working directory"),
            ("status", "Show the working tree status"),
            ("switch", "Switch branches"),
        ]
    }
}

impl Default for GitProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for GitProvider {
    fn name(&self) -> &str {
        "git"
    }

    fn handles(&self) -> &[&str] {
        &["git"]
    }

    fn get_completions(&self, command_line: &str, cursor_pos: usize) -> Vec<Completion> {
        let parts: Vec<&str> = command_line[..cursor_pos].split_whitespace().collect();

        match parts.len() {
            0 | 1 => {
                // Complete "git" itself or show subcommands
                Self::subcommands()
                    .iter()
                    .map(|(cmd, desc)| Completion {
                        text: cmd.to_string(),
                        display: cmd.to_string(),
                        description: Some(desc.to_string()),
                        kind: CompletionKind::Command,
                    })
                    .collect()
            }
            2 => {
                // Completing subcommand
                let partial = parts[1];
                Self::subcommands()
                    .iter()
                    .filter(|(cmd, _)| cmd.starts_with(partial))
                    .map(|(cmd, desc)| Completion {
                        text: cmd.to_string(),
                        display: cmd.to_string(),
                        description: Some(desc.to_string()),
                        kind: CompletionKind::Command,
                    })
                    .collect()
            }
            _ => {
                // Subcommand-specific completions
                let subcommand = parts[1];
                match subcommand {
                    "checkout" | "switch" | "merge" | "rebase" => {
                        // Complete branch names
                        self.get_branches()
                            .into_iter()
                            .map(|branch| Completion {
                                text: branch.clone(),
                                display: branch,
                                description: None,
                                kind: CompletionKind::Branch,
                            })
                            .collect()
                    }
                    "add" | "restore" | "diff" => {
                        // Would complete file paths
                        vec![]
                    }
                    _ => vec![],
                }
            }
        }
    }

    fn get_documentation(&self, command_line: &str, _cursor_pos: usize) -> Option<Documentation> {
        let parts: Vec<&str> = command_line.split_whitespace().collect();

        if parts.len() >= 2 {
            let subcommand = parts[1];
            Self::subcommands()
                .iter()
                .find(|(cmd, _)| *cmd == subcommand)
                .map(|(cmd, desc)| Documentation {
                    summary: desc.to_string(),
                    details: Some(format!("Run `git {} --help` for full documentation.", cmd)),
                    url: Some(format!(
                        "https://git-scm.com/docs/git-{}",
                        cmd
                    )),
                })
        } else {
            Some(Documentation {
                summary: "Git - the stupid content tracker".to_string(),
                details: Some("Git is a fast, scalable, distributed revision control system.".to_string()),
                url: Some("https://git-scm.com/docs/git".to_string()),
            })
        }
    }

    fn get_sidecar(&self, command: &str) -> Option<SidecarSpec> {
        let parts: Vec<&str> = command.split_whitespace().collect();

        if parts.len() < 2 {
            return None;
        }

        match parts[1] {
            "status" => Some(SidecarSpec {
                argv: vec![
                    "git".to_string(),
                    "status".to_string(),
                    "--porcelain=v2".to_string(),
                    "--branch".to_string(),
                ],
                env: None,
            }),
            "log" => Some(SidecarSpec {
                argv: vec![
                    "git".to_string(),
                    "log".to_string(),
                    "--format=%H%x00%an%x00%ae%x00%at%x00%s".to_string(),
                    "-n".to_string(),
                    "50".to_string(),
                ],
                env: None,
            }),
            "branch" => Some(SidecarSpec {
                argv: vec![
                    "git".to_string(),
                    "branch".to_string(),
                    "--format=%(refname:short)%00%(upstream:short)%00%(HEAD)".to_string(),
                ],
                env: None,
            }),
            _ => None,
        }
    }

    fn parse_sidecar_output(&self, output: &str) -> ParsedSidecarOutput {
        let mut anchors = Vec::new();

        // Parse git status --porcelain=v2 output
        for line in output.lines() {
            if line.starts_with("1 ") || line.starts_with("2 ") {
                // Changed entry
                let parts: Vec<&str> = line.split(' ').collect();
                if parts.len() >= 9 {
                    let path = parts[8..].join(" ");
                    anchors.push(Anchor {
                        id: format!("file:{}", path),
                        kind: AnchorKind::FilePath,
                        text: path.clone(),
                        range: (0, 0), // Would need actual position
                        metadata: Some(serde_json::json!({
                            "status": parts[1],
                        })),
                    });
                }
            } else if line.starts_with("? ") {
                // Untracked file
                let path = &line[2..];
                anchors.push(Anchor {
                    id: format!("file:{}", path),
                    kind: AnchorKind::FilePath,
                    text: path.to_string(),
                    range: (0, 0),
                    metadata: Some(serde_json::json!({
                        "status": "untracked",
                    })),
                });
            }
        }

        ParsedSidecarOutput {
            anchors,
            structured_data: None,
        }
    }
}
