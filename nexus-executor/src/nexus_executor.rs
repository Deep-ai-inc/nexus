//! NexusCommandExecutor - Bridges agent tools with Nexus Kernel
//!
//! This executor routes commands through the Nexus Kernel when possible,
//! falling back to the system shell for complex commands.

use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{CommandExecutor, CommandOutput, DefaultCommandExecutor, SandboxCommandRequest, StreamingCallback};

/// Command executor that routes through Nexus Kernel for native commands,
/// falling back to system shell for complex commands.
pub struct NexusCommandExecutor<K> {
    kernel: Arc<Mutex<K>>,
    fallback: DefaultCommandExecutor,
}

/// Trait to abstract over Kernel for testing
pub trait KernelLike: Send {
    /// Execute a command and return the exit code
    fn execute(&mut self, input: &str) -> Result<i32>;

    /// Check if a command is known to the kernel
    fn has_command(&self, name: &str) -> bool;

    /// Get the last output value as serializable text
    fn get_last_output_text(&self) -> Option<String>;
}

impl<K: KernelLike> NexusCommandExecutor<K> {
    /// Create a new NexusCommandExecutor with a kernel instance.
    pub fn new(kernel: Arc<Mutex<K>>) -> Self {
        Self {
            kernel,
            fallback: DefaultCommandExecutor,
        }
    }

    /// Parse the command line to extract the command name.
    fn parse_command_name(command_line: &str) -> Option<&str> {
        let trimmed = command_line.trim();

        // Skip variable assignments at the start
        let cmd_start = trimmed
            .split_whitespace()
            .find(|word| !word.contains('='))?;

        // Handle paths - extract the binary name
        cmd_start.rsplit('/').next()
    }

    /// Check if this command should be routed through the kernel.
    fn should_use_kernel(&self, command_line: &str) -> bool {
        // Don't use kernel for complex shell constructs
        if command_line.contains("&&")
            || command_line.contains("||")
            || command_line.contains(';')
            || command_line.contains('`')
            || command_line.contains("$(")
        {
            return false;
        }

        // Check if the command is known to the kernel
        if let Some(cmd_name) = Self::parse_command_name(command_line) {
            if let Ok(kernel) = self.kernel.lock() {
                return kernel.has_command(cmd_name);
            }
        }

        false
    }

    /// Execute through the Nexus Kernel.
    fn execute_via_kernel(&self, command_line: &str, working_dir: Option<&PathBuf>) -> Result<CommandOutput> {
        let mut kernel = self.kernel.lock().map_err(|e| anyhow::anyhow!("Kernel lock poisoned: {}", e))?;

        // Change to working directory if specified
        // (Note: This is a simplification - the kernel maintains its own cwd)
        let _old_cwd = if let Some(dir) = working_dir {
            let old = std::env::current_dir().ok();
            std::env::set_current_dir(dir).ok();
            old
        } else {
            None
        };

        let exit_code = kernel.execute(command_line)?;
        let output = kernel.get_last_output_text().unwrap_or_default();

        // Restore old cwd
        if let Some(old) = _old_cwd {
            std::env::set_current_dir(old).ok();
        }

        Ok(CommandOutput {
            success: exit_code == 0,
            output,
        })
    }
}

#[async_trait]
impl<K: KernelLike + 'static> CommandExecutor for NexusCommandExecutor<K> {
    async fn execute(
        &self,
        command_line: &str,
        working_dir: Option<&PathBuf>,
        sandbox_request: Option<&SandboxCommandRequest>,
    ) -> Result<CommandOutput> {
        if self.should_use_kernel(command_line) {
            self.execute_via_kernel(command_line, working_dir)
        } else {
            self.fallback.execute(command_line, working_dir, sandbox_request).await
        }
    }

    async fn execute_streaming(
        &self,
        command_line: &str,
        working_dir: Option<&PathBuf>,
        callback: Option<&dyn StreamingCallback>,
        sandbox_request: Option<&SandboxCommandRequest>,
    ) -> Result<CommandOutput> {
        // For kernel commands, we don't support streaming yet - just execute normally
        if self.should_use_kernel(command_line) {
            let result = self.execute_via_kernel(command_line, working_dir)?;

            // Send the complete output as one chunk if callback provided
            if let Some(cb) = callback {
                cb.on_output_chunk(&result.output)?;
            }

            Ok(result)
        } else {
            self.fallback.execute_streaming(command_line, working_dir, callback, sandbox_request).await
        }
    }
}

/// Serialize a Nexus Value to LLM-optimized markdown text.
///
/// This function converts structured Value types into readable text formats
/// that LLMs can easily parse and understand.
pub fn serialize_value_for_llm(value: &nexus_api::Value) -> String {
    use nexus_api::{FileType, Value};

    match value {
        Value::Unit => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) => s.clone(),
        Value::Bytes(b) => format!("<binary data: {} bytes>", b.len()),
        Value::Path(p) => p.display().to_string(),

        Value::List(items) => {
            items.iter()
                .map(|v| format!("- {}", serialize_value_for_llm(v)))
                .collect::<Vec<_>>()
                .join("\n")
        }

        Value::Record(pairs) => {
            pairs.iter()
                .map(|(k, v)| format!("{}: {}", k, serialize_value_for_llm(v)))
                .collect::<Vec<_>>()
                .join("\n")
        }

        Value::Table { columns, rows } => {
            if columns.is_empty() || rows.is_empty() {
                return String::new();
            }

            // Build markdown table
            let col_names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
            let header = format!("| {} |", col_names.join(" | "));
            let sep = format!("|{}|", columns.iter().map(|_| "---").collect::<Vec<_>>().join("|"));
            let rows_str = rows.iter()
                .map(|row| {
                    let cells: Vec<_> = row.iter()
                        .map(|v| serialize_value_for_llm(v))
                        .collect();
                    format!("| {} |", cells.join(" | "))
                })
                .collect::<Vec<_>>()
                .join("\n");

            format!("{}\n{}\n{}", header, sep, rows_str)
        }

        Value::Media { content_type, .. } => {
            format!("<media: {}>", content_type)
        }

        Value::FileEntry(entry) => {
            // Format like ls -l output
            let size_str = match entry.file_type {
                FileType::Directory => "-".to_string(),
                _ => format_size(entry.size),
            };

            let type_char = match entry.file_type {
                FileType::Directory => "d",
                FileType::Symlink => "l",
                FileType::BlockDevice => "b",
                FileType::CharDevice => "c",
                FileType::Fifo => "p",
                FileType::Socket => "s",
                _ => "-",
            };

            let perms = format_permissions(entry.permissions);
            format!("{}{} {:>8} {}", type_char, perms, size_str, entry.name)
        }

        Value::Error { code, message } => {
            format!("Error ({}): {}", code, message)
        }

        Value::Process(proc) => {
            format!(
                "PID: {}, User: {}, CPU: {:.1}%, Mem: {:.1}%, Status: {:?}, Command: {}",
                proc.pid, proc.user, proc.cpu_percent, proc.mem_percent, proc.status, proc.command
            )
        }

        Value::GitStatus(status) => {
            let mut parts = vec![format!("Branch: {}", status.branch)];
            if let Some(upstream) = &status.upstream {
                parts.push(format!("Upstream: {} (ahead: {}, behind: {})", upstream, status.ahead, status.behind));
            }
            if !status.staged.is_empty() {
                parts.push(format!("Staged: {} files", status.staged.len()));
            }
            if !status.unstaged.is_empty() {
                parts.push(format!("Unstaged: {} files", status.unstaged.len()));
            }
            if !status.untracked.is_empty() {
                parts.push(format!("Untracked: {} files", status.untracked.len()));
            }
            parts.join("\n")
        }

        Value::GitCommit(commit) => {
            format!(
                "{} {} <{}> {}",
                commit.short_hash, commit.author, commit.author_email, commit.message
            )
        }

        Value::Structured { kind, data } => {
            let prefix = kind.as_ref().map(|k| format!("[{}] ", k)).unwrap_or_default();
            let fields = data.iter()
                .map(|(k, v)| format!("{}: {}", k, serialize_value_for_llm(v)))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{}{}", prefix, fields)
        }
    }
}

/// Format Unix permissions as a string (e.g., "rwxr-xr-x").
fn format_permissions(mode: u32) -> String {
    let mut s = String::with_capacity(9);

    // Owner
    s.push(if mode & 0o400 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o200 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o100 != 0 { 'x' } else { '-' });

    // Group
    s.push(if mode & 0o040 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o020 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o010 != 0 { 'x' } else { '-' });

    // Other
    s.push(if mode & 0o004 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o002 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o001 != 0 { 'x' } else { '-' });

    s
}

/// Format a file size in human-readable form.
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1}G", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}M", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}K", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command_name() {
        assert_eq!(NexusCommandExecutor::<()>::parse_command_name("ls -la"), Some("ls"));
        assert_eq!(NexusCommandExecutor::<()>::parse_command_name("/bin/ls"), Some("ls"));
        assert_eq!(NexusCommandExecutor::<()>::parse_command_name("VAR=val cmd"), Some("cmd"));
        assert_eq!(NexusCommandExecutor::<()>::parse_command_name("  cat file.txt"), Some("cat"));
    }

    #[test]
    fn test_serialize_table() {
        use nexus_api::Value;

        let table = Value::table(
            vec!["Name", "Size"],
            vec![
                vec![Value::String("file.txt".to_string()), Value::Int(1024)],
                vec![Value::String("dir".to_string()), Value::Int(4096)],
            ],
        );

        let output = serialize_value_for_llm(&table);
        assert!(output.contains("| Name | Size |"));
        assert!(output.contains("| file.txt | 1024 |"));
    }

    #[test]
    fn test_format_permissions() {
        assert_eq!(format_permissions(0o755), "rwxr-xr-x");
        assert_eq!(format_permissions(0o644), "rw-r--r--");
        assert_eq!(format_permissions(0o777), "rwxrwxrwx");
        assert_eq!(format_permissions(0o000), "---------");
    }
}
