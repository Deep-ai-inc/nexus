//! Structured value types for the Nexus command system.
//!
//! Commands return `Value` instead of raw bytes, enabling rich rendering
//! in the GUI while falling back to text for legacy interop.
//!
//! # Type System Philosophy: "Progressive Enhancement"
//!
//! The type system is designed around three tiers:
//!
//! 1. **High-Frequency Domain Types** - Explicit types for common operations
//!    (FileEntry, Process, GitStatus). These enable rich GUI rendering and
//!    type-safe pipelines.
//!
//! 2. **Generic Structured Type** - The `Structured { kind, data }` escape hatch
//!    for parsed JSON/YAML/CSV that doesn't have a dedicated type. The `kind`
//!    field helps the GUI pick a renderer (e.g., "k8s/pod", "docker/container").
//!
//! 3. **Legacy Fallback** - All types implement `to_text()` for piping to
//!    external commands that expect plain text.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

/// A structured value that can be passed between commands and rendered by the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    // =========================================================================
    // Primitives
    // =========================================================================
    /// No value (like void/unit)
    Unit,
    /// Boolean
    Bool(bool),
    /// Signed integer
    Int(i64),
    /// Floating point
    Float(f64),
    /// Text string
    String(String),
    /// Raw bytes (legacy blob, compressed data, etc.)
    Bytes(Vec<u8>),

    // =========================================================================
    // Collections
    // =========================================================================
    /// Ordered list of values
    List(Vec<Value>),
    /// Ordered key-value pairs (like a JSON object but preserves insertion order)
    Record(Vec<(String, Value)>),
    /// Tabular data with named columns
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
    },

    // =========================================================================
    // High-Frequency Domain Types
    // These appear in 90% of interactive sessions and warrant explicit types.
    // =========================================================================
    /// A filesystem path
    Path(PathBuf),
    /// A file/directory entry with metadata (ls, find, fd)
    FileEntry(Box<FileEntry>),
    /// A running process (ps, top, kill)
    Process(Box<ProcessInfo>),
    /// Git repository status (git status)
    GitStatus(Box<GitStatusInfo>),
    /// A git commit (git log)
    GitCommit(Box<GitCommitInfo>),
    /// Rich media content - images, audio, video, documents
    Media {
        /// Raw file data
        data: Vec<u8>,
        /// MIME type (e.g., "image/png", "audio/mp3", "application/pdf")
        content_type: String,
        /// Optional metadata (dimensions, duration, etc.)
        metadata: MediaMetadata,
    },

    // =========================================================================
    // Generic Structured Type (The Escape Hatch)
    // For parsed data that doesn't have a dedicated type.
    // =========================================================================
    /// Generic structured data with an optional kind hint for rendering.
    ///
    /// Use this for:
    /// - Parsed JSON/YAML/CSV that doesn't fit a domain type
    /// - External API responses (k8s, docker, cloud providers)
    /// - Any structured data where you want the GUI to pick a renderer
    ///
    /// The `kind` field is a hint like "k8s/pod", "docker/container", "aws/ec2".
    Structured {
        /// Optional type hint for GUI rendering (e.g., "k8s/pod", "docker/container")
        kind: Option<String>,
        /// The actual data, preserving field order
        data: IndexMap<String, Value>,
    },

    // =========================================================================
    // Control Flow & Errors
    // =========================================================================
    /// An error value
    Error { code: i32, message: String },
}

/// Metadata about a file or directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub file_type: FileType,
    pub size: u64,
    pub modified: Option<u64>, // Unix timestamp (seconds)
    pub accessed: Option<u64>,
    pub created: Option<u64>,
    pub permissions: u32,
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub symlink_target: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileType {
    File,
    Directory,
    Symlink,
    BlockDevice,
    CharDevice,
    Fifo,
    Socket,
    Unknown,
}

// =============================================================================
// Process Information (ps, top, kill)
// =============================================================================

/// Information about a running process.
///
/// Returned by `ps`, `top`, and used as input to `kill`.
/// The GUI can render this with CPU sparklines, memory bars, and kill buttons.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// Process ID
    pub pid: u32,
    /// Parent process ID
    pub ppid: u32,
    /// User who owns the process
    pub user: String,
    /// Executable name (e.g., "node", "python")
    pub command: String,
    /// Full command line arguments
    pub args: Vec<String>,
    /// CPU usage as percentage (0.0 - 100.0+)
    pub cpu_percent: f64,
    /// Memory usage in bytes
    pub mem_bytes: u64,
    /// Memory usage as percentage of total system memory
    pub mem_percent: f64,
    /// Process status
    pub status: ProcessStatus,
    /// Start time as Unix timestamp (seconds)
    pub started: Option<u64>,
}

/// Process status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessStatus {
    Running,
    Sleeping,
    Stopped,
    Zombie,
    Idle,
    Unknown,
}

impl ProcessInfo {
    /// Get a field by name for filtering/selection.
    pub fn get_field(&self, name: &str) -> Option<Value> {
        match name {
            "pid" => Some(Value::Int(self.pid as i64)),
            "ppid" => Some(Value::Int(self.ppid as i64)),
            "user" => Some(Value::String(self.user.clone())),
            "command" | "cmd" => Some(Value::String(self.command.clone())),
            "args" => Some(Value::List(self.args.iter().map(|s| Value::String(s.clone())).collect())),
            "cpu" | "cpu_percent" => Some(Value::Float(self.cpu_percent)),
            "mem" | "mem_bytes" => Some(Value::Int(self.mem_bytes as i64)),
            "mem_percent" => Some(Value::Float(self.mem_percent)),
            "status" => Some(Value::String(format!("{:?}", self.status))),
            "started" => self.started.map(|t| Value::Int(t as i64)),
            _ => None,
        }
    }
}

// =============================================================================
// Git Status (git status)
// =============================================================================

/// Git repository status information.
///
/// Returned by `git status`. The GUI can render this with branch badges,
/// file status icons, and staging checkboxes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitStatusInfo {
    /// Current branch name (or "HEAD (detached)" if detached)
    pub branch: String,
    /// Upstream branch if tracking one
    pub upstream: Option<String>,
    /// Commits ahead of upstream
    pub ahead: u32,
    /// Commits behind upstream
    pub behind: u32,
    /// Files staged for commit
    pub staged: Vec<GitFileStatus>,
    /// Files modified but not staged
    pub unstaged: Vec<GitFileStatus>,
    /// Untracked files
    pub untracked: Vec<String>,
    /// Whether there are conflicts
    pub has_conflicts: bool,
}

/// Status of a file in git.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitFileStatus {
    /// File path relative to repo root
    pub path: String,
    /// Type of change
    pub status: GitChangeType,
    /// Original path if renamed
    pub orig_path: Option<String>,
}

/// Type of git file change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitChangeType {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
}

impl GitStatusInfo {
    /// Get a field by name for filtering/selection.
    pub fn get_field(&self, name: &str) -> Option<Value> {
        match name {
            "branch" => Some(Value::String(self.branch.clone())),
            "upstream" => self.upstream.clone().map(Value::String),
            "ahead" => Some(Value::Int(self.ahead as i64)),
            "behind" => Some(Value::Int(self.behind as i64)),
            "has_conflicts" => Some(Value::Bool(self.has_conflicts)),
            "staged_count" => Some(Value::Int(self.staged.len() as i64)),
            "unstaged_count" => Some(Value::Int(self.unstaged.len() as i64)),
            "untracked_count" => Some(Value::Int(self.untracked.len() as i64)),
            _ => None,
        }
    }
}

// =============================================================================
// Git Commit (git log)
// =============================================================================

/// Information about a git commit.
///
/// Returned by `git log`. The GUI can render this with clickable hashes
/// that open diffs, author avatars, and relative timestamps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitCommitInfo {
    /// Full commit hash
    pub hash: String,
    /// Short hash (first 7 chars)
    pub short_hash: String,
    /// Author name
    pub author: String,
    /// Author email
    pub author_email: String,
    /// Commit timestamp (Unix seconds)
    pub date: u64,
    /// Commit message (first line / subject)
    pub message: String,
    /// Full commit body (if available)
    pub body: Option<String>,
    /// Number of files changed (if available)
    pub files_changed: Option<u32>,
    /// Lines added (if available)
    pub insertions: Option<u32>,
    /// Lines deleted (if available)
    pub deletions: Option<u32>,
}

impl GitCommitInfo {
    /// Get a field by name for filtering/selection.
    pub fn get_field(&self, name: &str) -> Option<Value> {
        match name {
            "hash" => Some(Value::String(self.hash.clone())),
            "short_hash" => Some(Value::String(self.short_hash.clone())),
            "author" => Some(Value::String(self.author.clone())),
            "author_email" | "email" => Some(Value::String(self.author_email.clone())),
            "date" | "time" | "timestamp" => Some(Value::Int(self.date as i64)),
            "message" | "subject" => Some(Value::String(self.message.clone())),
            "body" => self.body.clone().map(Value::String),
            "files_changed" => self.files_changed.map(|n| Value::Int(n as i64)),
            "insertions" | "additions" => self.insertions.map(|n| Value::Int(n as i64)),
            "deletions" => self.deletions.map(|n| Value::Int(n as i64)),
            _ => None,
        }
    }
}

// =============================================================================
// Media Metadata
// =============================================================================

/// Metadata for media content - dimensions, duration, etc.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaMetadata {
    /// For images/video: width in pixels
    pub width: Option<u32>,
    /// For images/video: height in pixels
    pub height: Option<u32>,
    /// For audio/video: duration in seconds
    pub duration_secs: Option<f64>,
    /// Original filename if known
    pub filename: Option<String>,
    /// File size in bytes (redundant with data.len() but useful for display)
    pub size: Option<u64>,
}

impl MediaMetadata {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_dimensions(mut self, width: u32, height: u32) -> Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }

    pub fn with_duration(mut self, secs: f64) -> Self {
        self.duration_secs = Some(secs);
        self
    }

    pub fn with_filename(mut self, name: impl Into<String>) -> Self {
        self.filename = Some(name.into());
        self
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }
}

/// Detect MIME type from magic bytes.
pub fn detect_mime_type(data: &[u8]) -> &'static str {
    if data.len() < 12 {
        return "application/octet-stream";
    }

    // Images
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return "image/png";
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "image/jpeg";
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return "image/gif";
    }
    if data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        return "image/webp";
    }
    if data.starts_with(b"BM") {
        return "image/bmp";
    }

    // Documents
    if data.starts_with(b"%PDF") {
        return "application/pdf";
    }
    if data.starts_with(&[0x50, 0x4B, 0x03, 0x04]) {
        // ZIP-based formats - could be docx, xlsx, epub, etc.
        // Would need deeper inspection, default to zip
        return "application/zip";
    }

    // Audio
    if data.starts_with(b"ID3") || data.starts_with(&[0xFF, 0xFB]) {
        return "audio/mpeg";
    }
    if data.starts_with(b"OggS") {
        return "audio/ogg";
    }
    if data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"WAVE" {
        return "audio/wav";
    }
    if data.starts_with(b"fLaC") {
        return "audio/flac";
    }

    // Video
    if data.len() >= 12 && &data[4..12] == b"ftypisom" {
        return "video/mp4";
    }
    if data.len() >= 12 && &data[4..8] == b"ftyp" {
        return "video/mp4"; // Various MP4 variants
    }
    if data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return "video/webm";
    }

    // Text/code detection (simple heuristic)
    if data.starts_with(b"<?xml") || data.starts_with(b"<svg") {
        return "image/svg+xml";
    }
    if data.starts_with(b"<!DOCTYPE html") || data.starts_with(b"<html") {
        return "text/html";
    }
    if data.starts_with(b"{") || data.starts_with(b"[") {
        // Could be JSON
        if std::str::from_utf8(data).is_ok() {
            return "application/json";
        }
    }

    // Check if it's valid UTF-8 text
    if std::str::from_utf8(data).is_ok() {
        return "text/plain";
    }

    "application/octet-stream"
}

/// Get MIME type from file extension.
pub fn mime_from_extension(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "tiff" | "tif" => "image/tiff",

        // Audio
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" | "aac" => "audio/aac",

        // Video
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",

        // Documents
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",

        // Text/code
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "rs" => "text/x-rust",
        "py" => "text/x-python",
        "sh" => "text/x-shellscript",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",

        // Archives
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",

        _ => "application/octet-stream",
    }
}

impl FileEntry {
    /// Create a FileEntry from filesystem metadata.
    pub fn from_path(path: PathBuf) -> std::io::Result<Self> {
        let metadata = path.symlink_metadata()?;
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        let file_type = if metadata.is_dir() {
            FileType::Directory
        } else if metadata.is_symlink() {
            FileType::Symlink
        } else if metadata.is_file() {
            FileType::File
        } else {
            FileType::Unknown
        };

        let is_hidden = name.starts_with('.');

        let symlink_target = if metadata.is_symlink() {
            std::fs::read_link(&path).ok()
        } else {
            None
        };

        fn to_unix_ts(time: std::io::Result<SystemTime>) -> Option<u64> {
            time.ok()?
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        }

        #[cfg(unix)]
        let permissions = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode()
        };
        #[cfg(not(unix))]
        let permissions = if metadata.permissions().readonly() {
            0o444
        } else {
            0o644
        };

        Ok(FileEntry {
            name,
            path,
            file_type,
            size: metadata.len(),
            modified: to_unix_ts(metadata.modified()),
            accessed: to_unix_ts(metadata.accessed()),
            created: to_unix_ts(metadata.created()),
            permissions,
            is_hidden,
            is_symlink: metadata.is_symlink(),
            symlink_target,
        })
    }
}

/// Format a byte size into human-readable form.
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1}GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}KB", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

impl Value {
    /// Create a media value from raw bytes, auto-detecting MIME type.
    pub fn media(data: Vec<u8>) -> Self {
        let content_type = detect_mime_type(&data).to_string();
        Value::Media {
            data,
            content_type,
            metadata: MediaMetadata::default(),
        }
    }

    /// Create a media value with explicit MIME type.
    pub fn media_with_type(data: Vec<u8>, content_type: impl Into<String>) -> Self {
        Value::Media {
            data,
            content_type: content_type.into(),
            metadata: MediaMetadata::default(),
        }
    }

    /// Create a media value with full metadata.
    pub fn media_with_metadata(
        data: Vec<u8>,
        content_type: impl Into<String>,
        metadata: MediaMetadata,
    ) -> Self {
        Value::Media {
            data,
            content_type: content_type.into(),
            metadata,
        }
    }

    /// Check if this value is media content.
    pub fn is_media(&self) -> bool {
        matches!(self, Value::Media { .. })
    }

    /// Check if this is an image (based on content type).
    pub fn is_image(&self) -> bool {
        matches!(self, Value::Media { content_type, .. } if content_type.starts_with("image/"))
    }

    /// Check if this is audio.
    pub fn is_audio(&self) -> bool {
        matches!(self, Value::Media { content_type, .. } if content_type.starts_with("audio/"))
    }

    /// Check if this is video.
    pub fn is_video(&self) -> bool {
        matches!(self, Value::Media { content_type, .. } if content_type.starts_with("video/"))
    }

    /// Get media data and content type if this is media.
    pub fn as_media(&self) -> Option<(&[u8], &str, &MediaMetadata)> {
        match self {
            Value::Media { data, content_type, metadata } => Some((data, content_type, metadata)),
            _ => None,
        }
    }

    /// Extract raw bytes from the value.
    /// For Media/Bytes, returns the raw data.
    /// For String/other types, returns UTF-8 encoded text representation.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Value::Bytes(b) => b.clone(),
            Value::Media { data, .. } => data.clone(),
            _ => self.to_text().into_bytes(),
        }
    }

    /// Get the byte length of the value's data.
    pub fn byte_len(&self) -> usize {
        match self {
            Value::Bytes(b) => b.len(),
            Value::Media { data, .. } => data.len(),
            Value::String(s) => s.len(),
            _ => self.to_text().len(),
        }
    }

    /// Convert value to text for legacy interop (piping to external commands).
    pub fn to_text(&self) -> String {
        let mut buf = String::new();
        self.write_text(&mut buf);
        buf
    }

    fn write_text(&self, buf: &mut String) {
        match self {
            Value::Unit => {}
            Value::Bool(b) => buf.push_str(if *b { "true" } else { "false" }),
            Value::Int(n) => buf.push_str(&n.to_string()),
            Value::Float(f) => buf.push_str(&f.to_string()),
            Value::String(s) => buf.push_str(s),
            Value::Bytes(bytes) => {
                // Lossy UTF-8 conversion for display
                buf.push_str(&String::from_utf8_lossy(bytes));
            }
            Value::List(items) => {
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        buf.push('\n');
                    }
                    item.write_text(buf);
                }
            }
            Value::Record(fields) => {
                for (key, value) in fields {
                    buf.push_str(key);
                    buf.push_str(": ");
                    value.write_text(buf);
                    buf.push('\n');
                }
            }
            Value::Table { columns, rows } => {
                // Simple tab-separated output
                buf.push_str(&columns.join("\t"));
                buf.push('\n');
                for row in rows {
                    for (i, cell) in row.iter().enumerate() {
                        if i > 0 {
                            buf.push('\t');
                        }
                        cell.write_text(buf);
                    }
                    buf.push('\n');
                }
            }
            Value::Media { content_type, metadata, data } => {
                // Text representation of media
                let size = metadata.size.unwrap_or(data.len() as u64);
                let size_str = format_size(size);

                let extra = if let (Some(w), Some(h)) = (metadata.width, metadata.height) {
                    format!(" {}x{}", w, h)
                } else if let Some(dur) = metadata.duration_secs {
                    format!(" {:.1}s", dur)
                } else {
                    String::new()
                };

                let name = metadata.filename.as_deref().unwrap_or("media");
                buf.push_str(&format!("[{}: {} {}{}]", name, content_type, size_str, extra));
            }
            Value::Path(p) => buf.push_str(&p.to_string_lossy()),
            Value::FileEntry(entry) => {
                // Default: just the name (like simple `ls`)
                buf.push_str(&entry.name);
            }
            Value::Process(proc) => {
                // ps-like output: PID USER CPU% MEM COMMAND
                buf.push_str(&format!(
                    "{}\t{}\t{:.1}%\t{}\t{}",
                    proc.pid,
                    proc.user,
                    proc.cpu_percent,
                    format_size(proc.mem_bytes),
                    proc.command
                ));
            }
            Value::GitStatus(status) => {
                // git status -s like output
                buf.push_str(&format!("On branch {}\n", status.branch));
                if !status.staged.is_empty() {
                    buf.push_str("Changes to be committed:\n");
                    for f in &status.staged {
                        buf.push_str(&format!("  {:?}: {}\n", f.status, f.path));
                    }
                }
                if !status.unstaged.is_empty() {
                    buf.push_str("Changes not staged for commit:\n");
                    for f in &status.unstaged {
                        buf.push_str(&format!("  {:?}: {}\n", f.status, f.path));
                    }
                }
                if !status.untracked.is_empty() {
                    buf.push_str("Untracked files:\n");
                    for f in &status.untracked {
                        buf.push_str(&format!("  {}\n", f));
                    }
                }
            }
            Value::GitCommit(commit) => {
                // git log --oneline like output
                buf.push_str(&format!(
                    "{} {} <{}> {}",
                    commit.short_hash, commit.author, commit.author_email, commit.message
                ));
            }
            Value::Structured { kind, data } => {
                // JSON-like output with optional kind prefix
                if let Some(k) = kind {
                    buf.push_str(&format!("[{}] ", k));
                }
                buf.push_str("{\n");
                for (key, value) in data {
                    buf.push_str(&format!("  {}: ", key));
                    value.write_text(buf);
                    buf.push('\n');
                }
                buf.push('}');
            }
            Value::Error { message, .. } => {
                buf.push_str("error: ");
                buf.push_str(message);
            }
        }
    }

    /// Get a field by name for filtering/selection operations.
    ///
    /// This enables typed filtering like `ps | where cpu > 80` by allowing
    /// access to fields on domain-specific types.
    pub fn get_field(&self, name: &str) -> Option<Value> {
        match self {
            Value::Record(fields) => fields
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone()),
            Value::Process(p) => p.get_field(name),
            Value::GitStatus(g) => g.get_field(name),
            Value::GitCommit(c) => c.get_field(name),
            Value::FileEntry(f) => match name {
                "name" => Some(Value::String(f.name.clone())),
                "path" => Some(Value::Path(f.path.clone())),
                "size" => Some(Value::Int(f.size as i64)),
                "type" | "file_type" => Some(Value::String(format!("{:?}", f.file_type))),
                "modified" => f.modified.map(|t| Value::Int(t as i64)),
                "permissions" | "perms" => Some(Value::Int(f.permissions as i64)),
                "is_hidden" | "hidden" => Some(Value::Bool(f.is_hidden)),
                "is_symlink" | "symlink" => Some(Value::Bool(f.is_symlink)),
                _ => None,
            },
            Value::Structured { data, .. } => data.get(name).cloned(),
            _ => None,
        }
    }

    /// Check if this value is a domain-specific type that has typed fields.
    pub fn is_typed(&self) -> bool {
        matches!(
            self,
            Value::Process(_)
                | Value::GitStatus(_)
                | Value::GitCommit(_)
                | Value::FileEntry(_)
                | Value::Structured { .. }
        )
    }

    /// Get the type name of this value (useful for debugging/display).
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "unit",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::String(_) => "string",
            Value::Bytes(_) => "bytes",
            Value::List(_) => "list",
            Value::Record(_) => "record",
            Value::Table { .. } => "table",
            Value::Path(_) => "path",
            Value::FileEntry(_) => "file",
            Value::Process(_) => "process",
            Value::GitStatus(_) => "git-status",
            Value::GitCommit(_) => "git-commit",
            Value::Media { .. } => "media",
            Value::Structured { .. } => "structured",
            Value::Error { .. } => "error",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_text())
    }
}

// Convenient conversions
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Int(n)
    }
}

impl From<i32> for Value {
    fn from(n: i32) -> Self {
        Value::Int(n as i64)
    }
}

impl From<f64> for Value {
    fn from(f: f64) -> Self {
        Value::Float(f)
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_owned())
    }
}

impl From<PathBuf> for Value {
    fn from(p: PathBuf) -> Self {
        Value::Path(p)
    }
}

impl From<FileEntry> for Value {
    fn from(entry: FileEntry) -> Self {
        Value::FileEntry(Box::new(entry))
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(items: Vec<T>) -> Self {
        Value::List(items.into_iter().map(Into::into).collect())
    }
}

impl From<ProcessInfo> for Value {
    fn from(proc: ProcessInfo) -> Self {
        Value::Process(Box::new(proc))
    }
}

impl From<GitStatusInfo> for Value {
    fn from(status: GitStatusInfo) -> Self {
        Value::GitStatus(Box::new(status))
    }
}

impl From<GitCommitInfo> for Value {
    fn from(commit: GitCommitInfo) -> Self {
        Value::GitCommit(Box::new(commit))
    }
}

impl From<IndexMap<String, Value>> for Value {
    fn from(data: IndexMap<String, Value>) -> Self {
        Value::Structured { kind: None, data }
    }
}
