//! Filesystem provider - intelligence for file-related commands.

use std::path::PathBuf;

use nexus_api::{
    Anchor, AnchorKind, Completion, CompletionKind, Documentation, ParsedSidecarOutput,
    SidecarSpec,
};

use crate::Provider;

/// Provider for filesystem commands.
pub struct FilesystemProvider;

impl FilesystemProvider {
    pub fn new() -> Self {
        Self
    }

    /// Get completions for a path.
    fn complete_path(&self, partial: &str) -> Vec<Completion> {
        let path = PathBuf::from(partial);
        let (dir, prefix) = if partial.ends_with('/') || partial.is_empty() {
            (
                if partial.is_empty() {
                    PathBuf::from(".")
                } else {
                    path
                },
                "",
            )
        } else {
            (
                path.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")),
                path.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(""),
            )
        };

        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => return vec![],
        };

        entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|s| s.starts_with(prefix))
                    .unwrap_or(false)
            })
            .map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let display = if is_dir {
                    format!("{}/", name)
                } else {
                    name.clone()
                };

                Completion {
                    text: if partial.contains('/') {
                        format!(
                            "{}/{}",
                            dir.display(),
                            if is_dir {
                                format!("{}/", name)
                            } else {
                                name
                            }
                        )
                    } else {
                        display.clone()
                    },
                    display,
                    description: e
                        .metadata()
                        .ok()
                        .map(|m| format!("{} bytes", m.len())),
                    kind: if is_dir {
                        CompletionKind::Directory
                    } else {
                        CompletionKind::File
                    },
                }
            })
            .collect()
    }
}

impl Default for FilesystemProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for FilesystemProvider {
    fn name(&self) -> &str {
        "filesystem"
    }

    fn handles(&self) -> &[&str] {
        &["ls", "cat", "head", "tail", "less", "more", "find", "tree"]
    }

    fn get_completions(&self, command_line: &str, cursor_pos: usize) -> Vec<Completion> {
        let text = &command_line[..cursor_pos];
        let parts: Vec<&str> = text.split_whitespace().collect();

        // Complete paths for the last argument
        let partial = parts.last().copied().unwrap_or("");

        // Skip flags
        if partial.starts_with('-') {
            return vec![];
        }

        self.complete_path(partial)
    }

    fn get_documentation(&self, command_line: &str, _cursor_pos: usize) -> Option<Documentation> {
        let cmd = command_line.split_whitespace().next()?;

        let (summary, details) = match cmd {
            "ls" => (
                "List directory contents",
                "List information about the FILEs (the current directory by default).",
            ),
            "cat" => (
                "Concatenate files and print on the standard output",
                "Concatenate FILE(s) to standard output.",
            ),
            "head" => (
                "Output the first part of files",
                "Print the first 10 lines of each FILE to standard output.",
            ),
            "tail" => (
                "Output the last part of files",
                "Print the last 10 lines of each FILE to standard output.",
            ),
            "find" => (
                "Search for files in a directory hierarchy",
                "Search for files in a directory hierarchy.",
            ),
            "tree" => (
                "List contents of directories in a tree-like format",
                "List contents of directories in a tree-like format.",
            ),
            _ => return None,
        };

        Some(Documentation {
            summary: summary.to_string(),
            details: Some(details.to_string()),
            url: Some(format!("https://man7.org/linux/man-pages/man1/{}.1.html", cmd)),
        })
    }

    fn get_sidecar(&self, _command: &str) -> Option<SidecarSpec> {
        // Filesystem commands don't need sidecars - we parse their output directly
        None
    }

    fn parse_sidecar_output(&self, output: &str) -> ParsedSidecarOutput {
        // Parse ls-style output to extract file paths
        let mut anchors = Vec::new();

        for (line_idx, line) in output.lines().enumerate() {
            // Simple heuristic: treat each line as a potential file path
            let path = line.trim();
            if !path.is_empty() && !path.starts_with("total ") {
                // Try to extract the filename from ls -l format
                let filename = if line.contains(' ') && line.len() > 40 {
                    // Likely ls -l format, filename is at the end
                    line.rsplit_once(' ').map(|(_, name)| name).unwrap_or(path)
                } else {
                    path
                };

                anchors.push(Anchor {
                    id: format!("file:{}:{}", line_idx, filename),
                    kind: AnchorKind::FilePath,
                    text: filename.to_string(),
                    range: (0, 0),
                    metadata: None,
                });
            }
        }

        ParsedSidecarOutput {
            anchors,
            structured_data: None,
        }
    }
}
