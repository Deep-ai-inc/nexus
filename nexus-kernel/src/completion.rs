//! Tab completion engine for Nexus shell.
//!
//! Provides intelligent completions for:
//! - Paths (files and directories)
//! - Commands (PATH, builtins, native commands)
//! - Git branches and files (when in git context)
//! - Command-specific flags

use std::path::{Path, PathBuf};
use std::collections::HashSet;

use crate::commands::CommandRegistry;
use crate::ShellState;

/// A completion suggestion.
#[derive(Debug, Clone)]
pub struct Completion {
    /// The text to insert.
    pub text: String,
    /// Display text (may include icons/formatting).
    pub display: String,
    /// Type of completion for styling.
    pub kind: CompletionKind,
    /// Score for ranking (higher = better match).
    pub score: i32,
}

/// Type of completion item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    /// A file path.
    File,
    /// A directory path.
    Directory,
    /// An executable in PATH.
    Executable,
    /// A shell builtin.
    Builtin,
    /// A native (in-process) command.
    NativeCommand,
    /// A shell function.
    Function,
    /// An alias.
    Alias,
    /// A variable name.
    Variable,
    /// A git branch.
    GitBranch,
    /// A command flag/option.
    Flag,
}

impl CompletionKind {
    /// Get an icon for this completion kind.
    pub fn icon(&self) -> &'static str {
        match self {
            CompletionKind::File => "📄",
            CompletionKind::Directory => "📁",
            CompletionKind::Executable => "⚙️",
            CompletionKind::Builtin => "🔧",
            CompletionKind::NativeCommand => "🚀",
            CompletionKind::Function => "ƒ",
            CompletionKind::Alias => "→",
            CompletionKind::Variable => "$",
            CompletionKind::GitBranch => "",
            CompletionKind::Flag => "-",
        }
    }
}

/// Completion engine.
pub struct CompletionEngine<'a> {
    state: &'a ShellState,
    commands: &'a CommandRegistry,
}

impl<'a> CompletionEngine<'a> {
    /// Create a new completion engine.
    pub fn new(state: &'a ShellState, commands: &'a CommandRegistry) -> Self {
        Self { state, commands }
    }

    /// Get completions for the current input.
    ///
    /// # Arguments
    /// * `input` - The full input line
    /// * `cursor` - Cursor position in the input
    ///
    /// # Returns
    /// A tuple of (completions, start_offset) where start_offset is the position
    /// in the input where the completion word starts.
    pub fn complete(&self, input: &str, cursor: usize) -> (Vec<Completion>, usize) {
        let input = &input[..cursor.min(input.len())];

        // Find the word being completed
        let (word, word_start) = self.find_current_word(input);

        // Determine completion context
        let context = self.determine_context(input, word_start);

        let completions = match context {
            CompletionContext::Command => self.complete_command(&word),
            CompletionContext::Path => self.complete_path(&word),
            CompletionContext::Variable => self.complete_variable(&word),
            CompletionContext::GitBranch(cmd) => self.complete_git(&cmd, &word),
            CompletionContext::Flag(cmd) => self.complete_flags(&cmd, &word),
        };

        (completions, word_start)
    }

    /// Find the word at the cursor position.
    fn find_current_word(&self, input: &str) -> (String, usize) {
        // Work backwards from cursor to find word start
        let bytes = input.as_bytes();
        let mut start = input.len();

        for i in (0..input.len()).rev() {
            let c = bytes[i] as char;
            if c.is_whitespace() || c == '|' || c == ';' || c == '&' || c == '>' || c == '<' {
                start = i + 1;
                break;
            }
            if i == 0 {
                start = 0;
            }
        }

        let word = input[start..].to_string();
        (word, start)
    }

    /// Determine what kind of completion to provide.
    fn determine_context(&self, input: &str, word_start: usize) -> CompletionContext {
        let before_word = &input[..word_start].trim_end();
        let current_word = &input[word_start..];

        // Variable completion if word starts with $
        if current_word.starts_with('$') {
            return CompletionContext::Variable;
        }

        // Flag completion if word starts with -
        if current_word.starts_with('-') {
            // Find the command name
            if let Some(cmd) = self.find_command_name(before_word) {
                return CompletionContext::Flag(cmd);
            }
        }

        // If we're at the start or after a command separator, complete commands
        if before_word.is_empty()
            || before_word.ends_with('|')
            || before_word.ends_with(';')
            || before_word.ends_with("&&")
            || before_word.ends_with("||")
        {
            // But if word contains /, it's a path
            if current_word.contains('/') || current_word.starts_with('.') || current_word.starts_with('~') {
                return CompletionContext::Path;
            }
            return CompletionContext::Command;
        }

        // Git-specific completion
        if let Some(cmd) = self.find_command_name(before_word) {
            if cmd == "git" {
                // Check for git subcommands that need branch completion
                let parts: Vec<&str> = before_word.split_whitespace().collect();
                if parts.len() >= 2 {
                    let subcmd = parts.get(1).unwrap_or(&"");
                    if matches!(*subcmd, "checkout" | "switch" | "merge" | "rebase" | "branch" | "push" | "pull") {
                        return CompletionContext::GitBranch(cmd);
                    }
                }
            }
        }

        // Default to path completion
        CompletionContext::Path
    }

    /// Find the command name from the input before the current word.
    fn find_command_name(&self, before_word: &str) -> Option<String> {
        // Split by command separators and get the last command
        let parts: Vec<&str> = before_word
            .split(|c| c == '|' || c == ';' || c == '&')
            .collect();

        let last_cmd = parts.last()?.trim();
        let words: Vec<&str> = last_cmd.split_whitespace().collect();
        words.first().map(|s| s.to_string())
    }

    /// Complete command names.
    fn complete_command(&self, prefix: &str) -> Vec<Completion> {
        let mut completions = Vec::new();
        let prefix_lower = prefix.to_lowercase();

        // Builtins
        let builtins = [
            "cd", "exit", "export", "unset", "set", "alias", "unalias",
            "source", "eval", "read", "shift", "return", "break", "continue",
            "readonly", "command", "getopts", "trap", "exec", "local",
            "test", "[",
        ];

        for name in builtins {
            if name.starts_with(&prefix_lower) {
                completions.push(Completion {
                    text: name.to_string(),
                    display: format!("{} {}", CompletionKind::Builtin.icon(), name),
                    kind: CompletionKind::Builtin,
                    score: 100 + (prefix.len() as i32 * 10),
                });
            }
        }

        // Native commands
        for name in self.commands.names() {
            if name.to_lowercase().starts_with(&prefix_lower) {
                completions.push(Completion {
                    text: name.to_string(),
                    display: format!("{} {}", CompletionKind::NativeCommand.icon(), name),
                    kind: CompletionKind::NativeCommand,
                    score: 90 + (prefix.len() as i32 * 10),
                });
            }
        }

        // Functions
        for name in self.state.functions.keys() {
            if name.to_lowercase().starts_with(&prefix_lower) {
                completions.push(Completion {
                    text: name.clone(),
                    display: format!("{} {}", CompletionKind::Function.icon(), name),
                    kind: CompletionKind::Function,
                    score: 85 + (prefix.len() as i32 * 10),
                });
            }
        }

        // Aliases
        for name in self.state.aliases.keys() {
            if name.to_lowercase().starts_with(&prefix_lower) {
                completions.push(Completion {
                    text: name.clone(),
                    display: format!("{} {}", CompletionKind::Alias.icon(), name),
                    kind: CompletionKind::Alias,
                    score: 80 + (prefix.len() as i32 * 10),
                });
            }
        }

        // Executables from PATH
        if let Some(path_var) = self.state.get_env("PATH") {
            let mut seen = HashSet::new();

            for dir in path_var.split(':') {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.to_lowercase().starts_with(&prefix_lower) && !seen.contains(&name) {
                            // Check if executable
                            if is_executable(&entry.path()) {
                                seen.insert(name.clone());
                                completions.push(Completion {
                                    text: name.clone(),
                                    display: format!("{} {}", CompletionKind::Executable.icon(), name),
                                    kind: CompletionKind::Executable,
                                    score: 70 + (prefix.len() as i32 * 10),
                                });
                            }
                        }
                    }
                }
            }
        }

        // Sort by score (descending)
        completions.sort_by(|a, b| b.score.cmp(&a.score));
        completions
    }

    /// Complete file/directory paths.
    fn complete_path(&self, prefix: &str) -> Vec<Completion> {
        let mut completions = Vec::new();

        // Expand tilde
        let expanded = if prefix.starts_with('~') {
            if let Some(home) = self.state.get_env("HOME") {
                if prefix == "~" {
                    home.to_string()
                } else {
                    format!("{}{}", home, &prefix[1..])
                }
            } else {
                prefix.to_string()
            }
        } else {
            prefix.to_string()
        };

        // Determine base directory and filename prefix
        let path = Path::new(&expanded);
        let (dir, file_prefix) = if expanded.ends_with('/') {
            (PathBuf::from(&expanded), String::new())
        } else if path.is_dir() && !expanded.contains('/') {
            // Just a directory name without slash - treat as prefix
            (self.state.cwd.clone(), expanded.clone())
        } else {
            let parent = path.parent().unwrap_or(Path::new("."));
            let file_name = path.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();

            let base = if parent.as_os_str().is_empty() || parent == Path::new("") {
                self.state.cwd.clone()
            } else if parent.is_absolute() {
                parent.to_path_buf()
            } else {
                self.state.cwd.join(parent)
            };

            (base, file_name)
        };

        // Read directory entries
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let prefix_lower = file_prefix.to_lowercase();

            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();

                // Skip hidden files unless prefix starts with .
                if name.starts_with('.') && !file_prefix.starts_with('.') {
                    continue;
                }

                if name.to_lowercase().starts_with(&prefix_lower) {
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    let is_exec = !is_dir && is_executable(&entry.path());

                    // Build the completion text
                    let completion_text = if prefix.contains('/') {
                        // Preserve the path prefix
                        let path_prefix = prefix.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
                        if is_dir {
                            format!("{}/{}/", path_prefix, name)
                        } else {
                            format!("{}/{}", path_prefix, name)
                        }
                    } else if prefix.starts_with('~') {
                        // Preserve tilde
                        if is_dir {
                            format!("~/{}/", name)
                        } else {
                            format!("~/{}", name)
                        }
                    } else {
                        if is_dir {
                            format!("{}/", name)
                        } else {
                            name.clone()
                        }
                    };

                    let kind = if is_dir {
                        CompletionKind::Directory
                    } else if is_exec {
                        CompletionKind::Executable
                    } else {
                        CompletionKind::File
                    };

                    // Directories rank higher
                    let score = if is_dir { 80 } else { 70 } + (file_prefix.len() as i32 * 10);

                    completions.push(Completion {
                        text: completion_text,
                        display: format!("{} {}", kind.icon(), name),
                        kind,
                        score,
                    });
                }
            }
        }

        // Sort: directories first, then by name
        completions.sort_by(|a, b| {
            match (a.kind, b.kind) {
                (CompletionKind::Directory, CompletionKind::Directory) => a.text.cmp(&b.text),
                (CompletionKind::Directory, _) => std::cmp::Ordering::Less,
                (_, CompletionKind::Directory) => std::cmp::Ordering::Greater,
                _ => a.text.cmp(&b.text),
            }
        });

        completions
    }

    /// Complete variable names.
    fn complete_variable(&self, prefix: &str) -> Vec<Completion> {
        let mut completions = Vec::new();
        let var_prefix = prefix.trim_start_matches('$').trim_start_matches('{');
        let needs_brace = prefix.contains('{');

        // Shell variables
        for name in self.state.vars.keys() {
            if name.starts_with(var_prefix) {
                let text = if needs_brace {
                    format!("${{{}}}", name)
                } else {
                    format!("${}", name)
                };

                completions.push(Completion {
                    text,
                    display: format!("$ {} (shell)", name),
                    kind: CompletionKind::Variable,
                    score: 90,
                });
            }
        }

        // Environment variables
        for name in self.state.env.keys() {
            if name.starts_with(var_prefix) {
                let text = if needs_brace {
                    format!("${{{}}}", name)
                } else {
                    format!("${}", name)
                };

                completions.push(Completion {
                    text,
                    display: format!("$ {} (env)", name),
                    kind: CompletionKind::Variable,
                    score: 80,
                });
            }
        }

        // Special variables
        let special = ["?", "$", "!", "#", "@", "*", "0", "_"];
        for name in special {
            if name.starts_with(var_prefix) {
                completions.push(Completion {
                    text: format!("${}", name),
                    display: format!("$ {} (special)", name),
                    kind: CompletionKind::Variable,
                    score: 70,
                });
            }
        }

        completions
    }

    /// Complete git branches/refs.
    fn complete_git(&self, _cmd: &str, prefix: &str) -> Vec<Completion> {
        let mut completions = Vec::new();

        // Try to get git branches
        if let Ok(output) = std::process::Command::new("git")
            .args(["branch", "-a", "--format=%(refname:short)"])
            .current_dir(&self.state.cwd)
            .output()
        {
            if output.status.success() {
                let branches = String::from_utf8_lossy(&output.stdout);
                for branch in branches.lines() {
                    let branch = branch.trim();
                    if branch.to_lowercase().starts_with(&prefix.to_lowercase()) {
                        completions.push(Completion {
                            text: branch.to_string(),
                            display: format!("{} {}", CompletionKind::GitBranch.icon(), branch),
                            kind: CompletionKind::GitBranch,
                            score: 90,
                        });
                    }
                }
            }
        }

        // Also offer path completion for git add/rm/etc
        completions.extend(self.complete_path(prefix));

        completions
    }

    /// Complete command flags.
    fn complete_flags(&self, cmd: &str, prefix: &str) -> Vec<Completion> {
        let mut completions = Vec::new();

        // Common flags for well-known commands
        let flags: &[&str] = match cmd {
            "ls" => &["-l", "-a", "-la", "-lh", "-R", "--color", "--help"],
            "grep" => &["-i", "-v", "-n", "-r", "-l", "-c", "-E", "-F", "--help"],
            "find" => &["-name", "-type", "-mtime", "-size", "-exec", "--help"],
            "git" => &["--help", "--version", "-C"],
            "rm" => &["-r", "-f", "-rf", "-i", "--help"],
            "cp" => &["-r", "-f", "-i", "-v", "--help"],
            "mv" => &["-f", "-i", "-v", "--help"],
            "mkdir" => &["-p", "-m", "--help"],
            "chmod" => &["-R", "-v", "--help"],
            "cat" => &["-n", "-b", "-s", "--help"],
            "head" => &["-n", "-c", "--help"],
            "tail" => &["-n", "-f", "-F", "--help"],
            "sort" => &["-r", "-n", "-k", "-t", "-u", "--help"],
            "wc" => &["-l", "-w", "-c", "-m", "--help"],
            "curl" => &["-X", "-H", "-d", "-o", "-O", "-L", "-s", "-v", "--help"],
            _ => &["--help", "--version"],
        };

        for flag in flags {
            if flag.starts_with(prefix) {
                completions.push(Completion {
                    text: flag.to_string(),
                    display: format!("{} {}", CompletionKind::Flag.icon(), flag),
                    kind: CompletionKind::Flag,
                    score: 80,
                });
            }
        }

        completions
    }
}

/// Completion context.
#[derive(Debug)]
enum CompletionContext {
    /// Completing a command name.
    Command,
    /// Completing a file/directory path.
    Path,
    /// Completing a variable name.
    Variable,
    /// Completing a git branch/ref.
    GitBranch(String),
    /// Completing a command flag.
    Flag(String),
}

/// Check if a path is executable.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    if let Ok(metadata) = path.metadata() {
        let permissions = metadata.permissions();
        permissions.mode() & 0o111 != 0
    } else {
        false
    }
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    // On non-Unix, check for common executable extensions
    path.extension()
        .map(|ext| {
            let ext = ext.to_string_lossy().to_lowercase();
            matches!(ext.as_str(), "exe" | "bat" | "cmd" | "com")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_current_word() {
        let state = ShellState::from_cwd(std::env::current_dir().unwrap());
        let commands = CommandRegistry::new();
        let engine = CompletionEngine::new(&state, &commands);

        let (word, start) = engine.find_current_word("ls -la");
        assert_eq!(word, "-la");
        assert_eq!(start, 3);

        let (word, start) = engine.find_current_word("git ");
        assert_eq!(word, "");
        assert_eq!(start, 4);

        let (word, start) = engine.find_current_word("echo hello | grep he");
        assert_eq!(word, "he");
        assert_eq!(start, 18);
    }

    #[test]
    fn test_complete_command() {
        let state = ShellState::from_cwd(std::env::current_dir().unwrap());
        let commands = CommandRegistry::new();
        let engine = CompletionEngine::new(&state, &commands);

        let completions = engine.complete_command("ec");
        assert!(completions.iter().any(|c| c.text == "echo"));
    }
}
