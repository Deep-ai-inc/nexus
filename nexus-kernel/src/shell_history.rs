//! Native shell history integration.
//!
//! Reads from and writes to the user's native shell history file (~/.zsh_history,
//! ~/.bash_history) so Nexus shares history with regular shell sessions.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// =========================================================================
// Types
// =========================================================================

/// Which shell the user is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Zsh,
    Bash,
    Unknown,
}

/// Detected history file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryFormat {
    /// One command per line (bash, or zsh without EXTENDED_HISTORY).
    Plain,
    /// Zsh extended format: `: timestamp:duration;command`
    ZshExtended,
}

/// A single history entry.
#[derive(Debug, Clone)]
pub struct ShellHistoryEntry {
    pub command: String,
    pub timestamp: Option<u64>,
}

// =========================================================================
// ShellHistory
// =========================================================================

/// In-memory cache of the user's shell history, backed by the native history file.
pub struct ShellHistory {
    shell: ShellKind,
    path: PathBuf,
    format: HistoryFormat,
    entries: Vec<String>,
}

impl ShellHistory {
    /// Detect shell, locate history file, load tail into memory.
    pub fn open() -> Option<Self> {
        let (shell, path) = detect_shell_and_path()?;
        let format = detect_format(&path).unwrap_or(match shell {
            ShellKind::Zsh => HistoryFormat::Plain,
            _ => HistoryFormat::Plain,
        });

        let entries = read_tail(&path, 10_000, format).unwrap_or_default();

        Some(Self {
            shell,
            path,
            format,
            entries,
        })
    }

    /// Open with explicit path and format (for testing / the `history` command).
    pub fn open_path(path: PathBuf, format: HistoryFormat) -> Self {
        let entries = read_tail(&path, 10_000, format).unwrap_or_default();
        Self {
            shell: ShellKind::Unknown,
            path,
            format,
            entries,
        }
    }

    /// Recent entries (newest last, matching shell convention).
    pub fn recent(&self, limit: usize) -> Vec<ShellHistoryEntry> {
        let start = self.entries.len().saturating_sub(limit);
        self.entries[start..]
            .iter()
            .map(|cmd| ShellHistoryEntry {
                command: cmd.clone(),
                timestamp: None,
            })
            .collect()
    }

    /// Case-insensitive substring search, most recent first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<ShellHistoryEntry> {
        let query_lower = query.to_lowercase();
        self.entries
            .iter()
            .rev()
            .filter(|cmd| cmd.to_lowercase().contains(&query_lower))
            .take(limit)
            .map(|cmd| ShellHistoryEntry {
                command: cmd.clone(),
                timestamp: None,
            })
            .collect()
    }

    /// Append a command to the history file and in-memory cache.
    ///
    /// Uses `O_APPEND` for atomic writes — POSIX guarantees no interleaving
    /// for small writes (well under PIPE_BUF / 4KB).
    pub fn append(&mut self, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }

        // Deduplicate consecutive
        if self.entries.last().map(|s| s.as_str()) == Some(command) {
            return;
        }

        // Build the line to write
        let line = format_entry(command, self.format);

        // Atomic append to file
        if let Err(e) = append_to_file(&self.path, &line) {
            tracing::warn!("Failed to append to history file: {}", e);
        }

        self.entries.push(command.to_string());
    }

    /// Get the detected format.
    pub fn format(&self) -> HistoryFormat {
        self.format
    }

    /// Get the history file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get the detected shell kind.
    pub fn shell(&self) -> ShellKind {
        self.shell
    }

    /// Get all in-memory entries.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }
}

// =========================================================================
// Shell & path detection
// =========================================================================

fn detect_shell_and_path() -> Option<(ShellKind, PathBuf)> {
    let shell = detect_shell_kind();

    // $HISTFILE takes priority
    if let Ok(histfile) = std::env::var("HISTFILE") {
        let p = PathBuf::from(&histfile);
        if p.exists() {
            return Some((shell, p));
        }
    }

    // Fall back to conventional paths
    let home = std::env::var("HOME").ok()?;
    let home = PathBuf::from(home);

    match shell {
        ShellKind::Zsh => {
            let p = home.join(".zsh_history");
            if p.exists() {
                return Some((shell, p));
            }
        }
        ShellKind::Bash => {
            let p = home.join(".bash_history");
            if p.exists() {
                return Some((shell, p));
            }
        }
        ShellKind::Unknown => {}
    }

    // Last resort: try both
    for name in &[".zsh_history", ".bash_history"] {
        let p = home.join(name);
        if p.exists() {
            let kind = if *name == ".zsh_history" {
                ShellKind::Zsh
            } else {
                ShellKind::Bash
            };
            return Some((kind, p));
        }
    }

    None
}

fn detect_shell_kind() -> ShellKind {
    if let Ok(shell) = std::env::var("SHELL") {
        let basename = Path::new(&shell)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        match basename {
            "zsh" => ShellKind::Zsh,
            "bash" => ShellKind::Bash,
            _ => ShellKind::Unknown,
        }
    } else {
        ShellKind::Unknown
    }
}

// =========================================================================
// Format detection
// =========================================================================

/// Read the first few non-empty lines and check for zsh extended format.
fn detect_format(path: &Path) -> Option<HistoryFormat> {
    let file = File::open(path).ok()?;
    let reader = io::BufReader::new(file);

    let mut checked = 0;
    for line in reader.lines().take(50) {
        let line = line.ok()?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if is_zsh_extended_line(line) {
            return Some(HistoryFormat::ZshExtended);
        }
        checked += 1;
        if checked >= 5 {
            break;
        }
    }

    Some(HistoryFormat::Plain)
}

/// Check if a line matches `: timestamp:duration;command`.
fn is_zsh_extended_line(line: &str) -> bool {
    // Format: ": 1234567890:0;command"
    if !line.starts_with(": ") {
        return false;
    }
    let rest = &line[2..];
    // Find the colon after timestamp
    if let Some(colon_pos) = rest.find(':') {
        let timestamp_str = &rest[..colon_pos];
        // Timestamp should be all digits
        if timestamp_str.chars().all(|c| c.is_ascii_digit()) && !timestamp_str.is_empty() {
            // After the colon, should have digits then semicolon
            let after_colon = &rest[colon_pos + 1..];
            if let Some(semi_pos) = after_colon.find(';') {
                let duration_str = &after_colon[..semi_pos];
                return duration_str.chars().all(|c| c.is_ascii_digit())
                    && !duration_str.is_empty();
            }
        }
    }
    false
}

// =========================================================================
// Reading
// =========================================================================

/// Read the last ~`max_lines` logical lines from a history file.
///
/// For zsh extended format, handles multi-line commands (continuation with
/// backslash-newline).
fn read_tail(path: &Path, max_lines: usize, format: HistoryFormat) -> io::Result<Vec<String>> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();

    // For large files, seek to approximate tail position.
    // Average line ~60 bytes, read 2x for safety margin.
    let seek_bytes = (max_lines as u64) * 120;
    let raw_lines = if file_len > seek_bytes {
        file.seek(SeekFrom::Start(file_len - seek_bytes))?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        // Drop the first (potentially partial) line
        let lines: Vec<&str> = buf.lines().collect();
        if lines.len() > 1 {
            lines[1..].iter().map(|s| s.to_string()).collect()
        } else {
            lines.iter().map(|s| s.to_string()).collect()
        }
    } else {
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        buf.lines().map(|s| s.to_string()).collect::<Vec<_>>()
    };

    let commands = parse_lines(&raw_lines, format);

    // Deduplicate consecutive
    let mut deduped: Vec<String> = Vec::with_capacity(commands.len());
    for cmd in commands {
        if deduped.last().map(|s| s.as_str()) != Some(&cmd) {
            deduped.push(cmd);
        }
    }

    // Trim to max_lines
    let start = deduped.len().saturating_sub(max_lines);
    Ok(deduped[start..].to_vec())
}

/// Parse raw file lines into commands according to the format.
fn parse_lines(raw_lines: &[String], format: HistoryFormat) -> Vec<String> {
    match format {
        HistoryFormat::Plain => {
            raw_lines
                .iter()
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect()
        }
        HistoryFormat::ZshExtended => parse_zsh_extended(raw_lines),
    }
}

/// Parse zsh extended history format.
///
/// Format: `: timestamp:duration;command`
/// Multi-line commands use `\` + newline as continuation.
fn parse_zsh_extended(raw_lines: &[String]) -> Vec<String> {
    let mut commands = Vec::new();
    let mut current_command: Option<String> = None;

    for line in raw_lines {
        if let Some(ref mut cmd) = current_command {
            // We're in a multi-line continuation
            if line.ends_with('\\') {
                cmd.push('\n');
                cmd.push_str(&line[..line.len() - 1]);
            } else {
                cmd.push('\n');
                cmd.push_str(line);
                commands.push(std::mem::take(cmd));
                current_command = None;
            }
            continue;
        }

        // Try to parse as extended format line
        if let Some(cmd) = parse_extended_line(line) {
            if cmd.ends_with('\\') {
                current_command = Some(cmd[..cmd.len() - 1].to_string());
            } else {
                commands.push(cmd);
            }
        } else if !line.is_empty() {
            // Malformed line — keep as raw string
            commands.push(line.to_string());
        }
    }

    // Flush any trailing continuation
    if let Some(cmd) = current_command {
        commands.push(cmd);
    }

    commands
}

/// Extract the command from a `: timestamp:duration;command` line.
fn parse_extended_line(line: &str) -> Option<String> {
    let rest = line.strip_prefix(": ")?;
    let colon_pos = rest.find(':')?;
    let after_ts = &rest[colon_pos + 1..];
    let semi_pos = after_ts.find(';')?;
    Some(after_ts[semi_pos + 1..].to_string())
}

// =========================================================================
// Writing
// =========================================================================

/// Format a command as a history file entry.
fn format_entry(command: &str, format: HistoryFormat) -> String {
    // Replace internal newlines with backslash-newline (zsh multi-line convention)
    let encoded = if command.contains('\n') {
        command.replace('\n', "\\\n")
    } else {
        command.to_string()
    };

    match format {
        HistoryFormat::ZshExtended => {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!(": {}:0;{}\n", ts, encoded)
        }
        HistoryFormat::Plain => {
            format!("{}\n", encoded)
        }
    }
}

/// Append a pre-formatted entry to the history file using O_APPEND.
fn append_to_file(path: &Path, entry: &str) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)?;
    file.write_all(entry.as_bytes())
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp_file(content: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_history");
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    // -- Format detection --

    #[test]
    fn test_detect_zsh_extended() {
        let (_dir, path) = write_temp_file(
            ": 1700000000:0;ls -la\n\
             : 1700000001:0;echo hello\n\
             : 1700000002:5;git status\n",
        );
        assert_eq!(detect_format(&path), Some(HistoryFormat::ZshExtended));
    }

    #[test]
    fn test_detect_plain() {
        let (_dir, path) = write_temp_file(
            "ls -la\n\
             echo hello\n\
             git status\n",
        );
        assert_eq!(detect_format(&path), Some(HistoryFormat::Plain));
    }

    #[test]
    fn test_detect_empty_file() {
        let (_dir, path) = write_temp_file("");
        assert_eq!(detect_format(&path), Some(HistoryFormat::Plain));
    }

    // -- Reading plain format --

    #[test]
    fn test_read_bash_history() {
        let (_dir, path) = write_temp_file("ls\ncd /tmp\ngit status\n");
        let entries = read_tail(&path, 100, HistoryFormat::Plain).unwrap();
        assert_eq!(entries, vec!["ls", "cd /tmp", "git status"]);
    }

    #[test]
    fn test_read_plain_deduplicates_consecutive() {
        let (_dir, path) = write_temp_file("ls\nls\nls\ncd\ncd\ngit status\n");
        let entries = read_tail(&path, 100, HistoryFormat::Plain).unwrap();
        assert_eq!(entries, vec!["ls", "cd", "git status"]);
    }

    // -- Reading zsh extended format --

    #[test]
    fn test_read_zsh_extended() {
        let (_dir, path) = write_temp_file(
            ": 1700000000:0;ls -la\n\
             : 1700000001:0;echo hello\n\
             : 1700000002:5;git commit -m 'test'\n",
        );
        let entries = read_tail(&path, 100, HistoryFormat::ZshExtended).unwrap();
        assert_eq!(
            entries,
            vec!["ls -la", "echo hello", "git commit -m 'test'"]
        );
    }

    #[test]
    fn test_read_zsh_extended_multiline() {
        // Multi-line command: backslash at end of line means continuation
        let (_dir, path) = write_temp_file(
            ": 1700000000:0;echo foo\n\
             : 1700000001:0;if true; then\\\n\
             echo yes\\\n\
             fi\n\
             : 1700000002:0;ls\n",
        );
        let entries = read_tail(&path, 100, HistoryFormat::ZshExtended).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], "echo foo");
        assert_eq!(entries[1], "if true; then\necho yes\nfi");
        assert_eq!(entries[2], "ls");
    }

    #[test]
    fn test_read_zsh_extended_deduplicates() {
        let (_dir, path) = write_temp_file(
            ": 1700000000:0;ls\n\
             : 1700000001:0;ls\n\
             : 1700000002:0;cd\n",
        );
        let entries = read_tail(&path, 100, HistoryFormat::ZshExtended).unwrap();
        assert_eq!(entries, vec!["ls", "cd"]);
    }

    #[test]
    fn test_read_zsh_extended_malformed_lines_kept() {
        let (_dir, path) = write_temp_file(
            ": 1700000000:0;ls\n\
             this is garbage\n\
             : 1700000002:0;cd\n",
        );
        let entries = read_tail(&path, 100, HistoryFormat::ZshExtended).unwrap();
        assert_eq!(entries, vec!["ls", "this is garbage", "cd"]);
    }

    // -- Writing --

    #[test]
    fn test_write_plain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_history");
        std::fs::write(&path, "").unwrap();

        let mut hist = ShellHistory::open_path(path.clone(), HistoryFormat::Plain);
        hist.append("echo hello");
        hist.append("git status");

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "echo hello\ngit status\n");
        assert_eq!(hist.entries(), &["echo hello", "git status"]);
    }

    #[test]
    fn test_write_zsh_extended() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_history");
        std::fs::write(&path, "").unwrap();

        let mut hist = ShellHistory::open_path(path.clone(), HistoryFormat::ZshExtended);
        hist.append("echo hello");

        let content = std::fs::read_to_string(&path).unwrap();
        // Should match ": <timestamp>:0;echo hello\n"
        assert!(content.starts_with(": "));
        assert!(content.ends_with(":0;echo hello\n"));
        assert_eq!(hist.entries(), &["echo hello"]);
    }

    #[test]
    fn test_write_multiline_command() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_history");
        std::fs::write(&path, "").unwrap();

        let mut hist = ShellHistory::open_path(path.clone(), HistoryFormat::ZshExtended);
        hist.append("if true; then\necho yes\nfi");

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("if true; then\\\necho yes\\\nfi"));
    }

    #[test]
    fn test_write_deduplicates_consecutive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_history");
        std::fs::write(&path, "").unwrap();

        let mut hist = ShellHistory::open_path(path.clone(), HistoryFormat::Plain);
        hist.append("ls");
        hist.append("ls"); // duplicate — should be skipped
        hist.append("cd");

        assert_eq!(hist.entries(), &["ls", "cd"]);
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "ls\ncd\n");
    }

    #[test]
    fn test_write_empty_command_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_history");
        std::fs::write(&path, "").unwrap();

        let mut hist = ShellHistory::open_path(path.clone(), HistoryFormat::Plain);
        hist.append("");
        hist.append("  ");

        assert!(hist.entries().is_empty());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.is_empty());
    }

    // -- Search --

    #[test]
    fn test_search_case_insensitive() {
        let (_dir, path) = write_temp_file("ls -la\nGIT status\ngit commit\necho hello\n");
        let hist = ShellHistory::open_path(path, HistoryFormat::Plain);

        let results = hist.search("git", 10);
        assert_eq!(results.len(), 2);
        // Most recent first
        assert_eq!(results[0].command, "git commit");
        assert_eq!(results[1].command, "GIT status");
    }

    #[test]
    fn test_search_respects_limit() {
        let (_dir, path) = write_temp_file("git a\ngit b\ngit c\ngit d\ngit e\n");
        let hist = ShellHistory::open_path(path, HistoryFormat::Plain);

        let results = hist.search("git", 3);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].command, "git e");
    }

    #[test]
    fn test_search_no_results() {
        let (_dir, path) = write_temp_file("ls\ncd\npwd\n");
        let hist = ShellHistory::open_path(path, HistoryFormat::Plain);

        let results = hist.search("nonexistent", 10);
        assert!(results.is_empty());
    }

    // -- Recent --

    #[test]
    fn test_recent() {
        let (_dir, path) = write_temp_file("a\nb\nc\nd\ne\n");
        let hist = ShellHistory::open_path(path, HistoryFormat::Plain);

        let recent = hist.recent(3);
        assert_eq!(recent.len(), 3);
        // Newest last (chronological order)
        assert_eq!(recent[0].command, "c");
        assert_eq!(recent[1].command, "d");
        assert_eq!(recent[2].command, "e");
    }

    // -- Tail loading --

    #[test]
    fn test_tail_load_respects_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_history");
        // Write 100 lines
        let mut content = String::new();
        for i in 0..100 {
            content.push_str(&format!("cmd{}\n", i));
        }
        std::fs::write(&path, &content).unwrap();

        let entries = read_tail(&path, 10, HistoryFormat::Plain).unwrap();
        assert_eq!(entries.len(), 10);
        assert_eq!(entries[0], "cmd90");
        assert_eq!(entries[9], "cmd99");
    }

    // -- Format entry --

    #[test]
    fn test_format_entry_plain() {
        let line = format_entry("echo hello", HistoryFormat::Plain);
        assert_eq!(line, "echo hello\n");
    }

    #[test]
    fn test_format_entry_zsh_extended() {
        let line = format_entry("echo hello", HistoryFormat::ZshExtended);
        assert!(line.starts_with(": "));
        assert!(line.ends_with(":0;echo hello\n"));
    }

    #[test]
    fn test_format_entry_multiline() {
        let line = format_entry("if true; then\necho yes\nfi", HistoryFormat::ZshExtended);
        assert!(line.contains("if true; then\\\necho yes\\\nfi"));
    }

    // -- is_zsh_extended_line --

    #[test]
    fn test_is_zsh_extended_line() {
        assert!(is_zsh_extended_line(": 1700000000:0;ls"));
        assert!(is_zsh_extended_line(": 1700000000:123;echo hello"));
        assert!(!is_zsh_extended_line("ls -la"));
        assert!(!is_zsh_extended_line(": abc:0;ls"));
        assert!(!is_zsh_extended_line(": 123:;ls")); // empty duration
        assert!(!is_zsh_extended_line(""));
    }

    // -- Shell detection helpers --

    #[test]
    fn test_detect_shell_kind() {
        // This tests the function with whatever $SHELL is set — just ensure it doesn't panic.
        let _kind = detect_shell_kind();
    }

    // -- Round-trip: write then read --

    #[test]
    fn test_roundtrip_plain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_history");
        std::fs::write(&path, "").unwrap();

        let mut hist = ShellHistory::open_path(path.clone(), HistoryFormat::Plain);
        hist.append("ls -la");
        hist.append("git status");
        hist.append("echo 'hello world'");

        // Re-read
        let hist2 = ShellHistory::open_path(path, HistoryFormat::Plain);
        assert_eq!(
            hist2.entries(),
            &["ls -la", "git status", "echo 'hello world'"]
        );
    }

    #[test]
    fn test_roundtrip_zsh_extended() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_history");
        std::fs::write(&path, "").unwrap();

        let mut hist = ShellHistory::open_path(path.clone(), HistoryFormat::ZshExtended);
        hist.append("ls -la");
        hist.append("if true; then\necho yes\nfi");
        hist.append("git commit -m 'test'");

        // Re-read
        let hist2 = ShellHistory::open_path(path, HistoryFormat::ZshExtended);
        assert_eq!(hist2.entries().len(), 3);
        assert_eq!(hist2.entries()[0], "ls -la");
        assert_eq!(hist2.entries()[1], "if true; then\necho yes\nfi");
        assert_eq!(hist2.entries()[2], "git commit -m 'test'");
    }
}
