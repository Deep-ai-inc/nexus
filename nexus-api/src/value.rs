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
    /// Tabular data with named columns and optional display format hints
    Table {
        columns: Vec<TableColumn>,
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
    // Domain-Specific Types (grouped behind a single variant)
    // =========================================================================
    /// Command-specific domain types: file operations, network events,
    /// diffs, trees, interactive viewers, etc.
    Domain(Box<DomainValue>),

    // =========================================================================
    // Control Flow & Errors
    // =========================================================================
    /// An error value
    Error { code: i32, message: String },
}

/// Command-specific domain types, grouped to keep the root `Value` enum lean.
///
/// Adding a new command output type (e.g. `Whois`, `Traceroute`) only requires
/// adding a variant here and implementing it in `DomainValue`'s methods — no
/// changes to `Value` itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DomainValue {
    /// File operation with progress tracking (cp, mv, rm).
    FileOp(FileOpInfo),
    /// Directory tree (flat arena representation).
    Tree(TreeInfo),
    /// Structured diff for a single file.
    DiffFile(DiffFileInfo),
    /// Network event (ping reply, timeout, error).
    NetEvent(NetEventInfo),
    /// DNS lookup answer.
    DnsAnswer(DnsAnswerInfo),
    /// HTTP response metadata + body preview.
    HttpResponse(HttpResponseInfo),
    /// Request to open an interactive viewer in the UI.
    Interactive(InteractiveRequest),
    /// Binary data chunk with metadata (non-renderable binaries from cat).
    BlobChunk(BlobChunk),
}

impl DomainValue {
    pub fn write_text(&self, buf: &mut String) {
        match self {
            DomainValue::FileOp(info) => {
                let phase = format!("{:?}", info.phase);
                let op = format!("{:?}", info.op_type);
                buf.push_str(&format!("{} {}: ", op, phase));
                if let Some(total) = info.files_total {
                    buf.push_str(&format!("{}/{} files", info.files_processed, total));
                } else {
                    buf.push_str(&format!("{} files", info.files_processed));
                }
                if let Some(total_bytes) = info.total_bytes {
                    buf.push_str(&format!(", {}/{} bytes", info.bytes_processed, total_bytes));
                }
                if !info.errors.is_empty() {
                    buf.push_str(&format!(", {} errors", info.errors.len()));
                }
            }
            DomainValue::Tree(tree) => {
                for node in &tree.nodes {
                    let indent: String = if node.depth == 0 {
                        String::new()
                    } else {
                        let prefix = "    ".repeat(node.depth.saturating_sub(1));
                        let is_last = tree.nodes.iter()
                            .filter(|n| n.parent == node.parent && n.depth == node.depth)
                            .last()
                            .map(|n| n.id == node.id)
                            .unwrap_or(true);
                        if is_last {
                            format!("{}\u{2514}\u{2500}\u{2500} ", prefix)
                        } else {
                            format!("{}\u{251C}\u{2500}\u{2500} ", prefix)
                        }
                    };
                    buf.push_str(&format!("{}{}\n", indent, node.name));
                }
            }
            DomainValue::DiffFile(diff) => {
                if let Some(ref old) = diff.old_path {
                    buf.push_str(&format!("--- {}\n", old));
                } else {
                    buf.push_str(&format!("--- a/{}\n", diff.file_path));
                }
                buf.push_str(&format!("+++ b/{}\n", diff.file_path));
                for hunk in &diff.hunks {
                    buf.push_str(&format!("@@ -{},{} +{},{} @@ {}\n",
                        hunk.old_start, hunk.old_count,
                        hunk.new_start, hunk.new_count,
                        hunk.header));
                    for line in &hunk.lines {
                        let prefix = match line.kind {
                            DiffLineKind::Context => " ",
                            DiffLineKind::Addition => "+",
                            DiffLineKind::Deletion => "-",
                        };
                        buf.push_str(&format!("{}{}\n", prefix, line.content));
                    }
                }
            }
            DomainValue::NetEvent(evt) => {
                match evt.event_type {
                    NetEventType::PingResponse => {
                        let ip = evt.ip.as_deref().unwrap_or(&evt.host);
                        let rtt = evt.rtt_ms.map(|r| format!(" time={:.1} ms", r)).unwrap_or_default();
                        let ttl = evt.ttl.map(|t| format!(" ttl={}", t)).unwrap_or_default();
                        let seq = evt.seq.map(|s| format!(" seq={}", s)).unwrap_or_default();
                        buf.push_str(&format!("64 bytes from {}:{}{}{}", ip, seq, ttl, rtt));
                    }
                    NetEventType::Timeout => {
                        let seq = evt.seq.map(|s| format!(" seq={}", s)).unwrap_or_default();
                        buf.push_str(&format!("Request timeout for {}{}", evt.host, seq));
                    }
                    NetEventType::Error => {
                        let msg = evt.message.as_deref().unwrap_or("unknown error");
                        buf.push_str(&format!("ping: {}: {}", evt.host, msg));
                    }
                }
            }
            DomainValue::DnsAnswer(dns) => {
                buf.push_str(&format!(";; QUESTION SECTION:\n;{}\t\tIN\t{}\n\n", dns.query, dns.record_type));
                buf.push_str(";; ANSWER SECTION:\n");
                for record in &dns.answers {
                    buf.push_str(&format!("{}\t{}\tIN\t{}\t{}\n",
                        record.name, record.ttl, record.record_type, record.data));
                }
                buf.push_str(&format!("\n;; Query time: {:.0} msec\n", dns.query_time_ms));
                buf.push_str(&format!(";; SERVER: {}\n", dns.server));
            }
            DomainValue::HttpResponse(resp) => {
                if let Some(ref preview) = resp.body_preview {
                    buf.push_str(preview);
                }
            }
            DomainValue::Interactive(req) => {
                req.content.write_text(buf);
            }
            DomainValue::BlobChunk(chunk) => {
                let size = chunk.total_size.unwrap_or(chunk.data.len() as u64);
                let src = chunk.source.as_deref().unwrap_or("binary");
                buf.push_str(&format!("[{}: {} {}]", src, chunk.content_type, format_size(size)));
            }
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            DomainValue::FileOp(_) => "file-op",
            DomainValue::Tree(_) => "tree",
            DomainValue::DiffFile(_) => "diff-file",
            DomainValue::NetEvent(_) => "net-event",
            DomainValue::DnsAnswer(_) => "dns-answer",
            DomainValue::HttpResponse(_) => "http-response",
            DomainValue::Interactive(_) => "interactive",
            DomainValue::BlobChunk(_) => "blob-chunk",
        }
    }

    pub fn get_field(&self, name: &str) -> Option<Value> {
        match self {
            DomainValue::FileOp(info) => info.get_field(name),
            DomainValue::NetEvent(evt) => evt.get_field(name),
            DomainValue::DnsAnswer(dns) => dns.get_field(name),
            DomainValue::HttpResponse(resp) => resp.get_field(name),
            DomainValue::BlobChunk(chunk) => match name {
                "content_type" => Some(Value::String(chunk.content_type.clone())),
                "offset" => Some(Value::Int(chunk.offset as i64)),
                "total_size" => chunk.total_size.map(|s| Value::Int(s as i64)),
                "source" => chunk.source.as_ref().map(|s| Value::String(s.clone())),
                "len" => Some(Value::Int(chunk.data.len() as i64)),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn is_typed(&self) -> bool {
        matches!(
            self,
            DomainValue::FileOp(_)
                | DomainValue::NetEvent(_)
                | DomainValue::DnsAnswer(_)
                | DomainValue::HttpResponse(_)
                | DomainValue::BlobChunk(_)
        )
    }
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
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub nlink: Option<u64>,
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
// Table Column Definition
// =============================================================================

/// A table column with name and optional display format hint.
///
/// The display format is a hint to the renderer - it doesn't change the
/// underlying data type. This allows sorting/filtering to work on the raw
/// value while displaying it in a human-friendly format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableColumn {
    /// Column name (displayed in header)
    pub name: String,
    /// Optional display format hint for the renderer
    pub format: Option<DisplayFormat>,
}

impl TableColumn {
    /// Create a simple column with no format hint.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            format: None,
        }
    }

    /// Create a column with a display format hint.
    pub fn with_format(name: impl Into<String>, format: DisplayFormat) -> Self {
        Self {
            name: name.into(),
            format: Some(format),
        }
    }
}

impl From<&str> for TableColumn {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for TableColumn {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

/// Display format hints for table columns.
///
/// These don't change the underlying data - a size column still stores
/// `Value::Int(207667)`, but the renderer shows "202.8K" when the hint
/// is `HumanBytes`. This enables correct sorting while showing friendly output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayFormat {
    /// Format bytes as human-readable (e.g., 1024 -> "1.0K")
    HumanBytes,
    /// Format as percentage with % suffix (e.g., 0.5 -> "50%")
    Percentage,
    /// Format Unix timestamp as relative time (e.g., "2 hours ago")
    RelativeTime,
    /// Format Unix timestamp as absolute datetime
    DateTime,
    /// Format duration in seconds as human-readable (e.g., "2h 30m")
    Duration,
    /// Format with octal representation (for permissions)
    Octal,
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
    /// Group who owns the process
    pub group: Option<String>,
    /// Executable name (e.g., "node", "python")
    pub command: String,
    /// Full command line arguments
    pub args: Vec<String>,
    /// CPU usage as percentage (0.0 - 100.0+)
    pub cpu_percent: f64,
    /// Memory usage in bytes (RSS - Resident Set Size)
    pub mem_bytes: u64,
    /// Memory usage as percentage of total system memory
    pub mem_percent: f64,
    /// Virtual memory size in bytes
    pub virtual_size: u64,
    /// Process status
    pub status: ProcessStatus,
    /// Start time as Unix timestamp (seconds)
    pub started: Option<u64>,
    /// CPU time consumed in seconds
    pub cpu_time: u64,
    /// Controlling terminal (e.g., "/dev/pts/0")
    pub tty: Option<String>,
    /// Nice value (-20 to 19, lower = higher priority)
    pub nice: Option<i8>,
    /// Process priority
    pub priority: i32,
    /// Process group ID
    pub pgid: Option<u32>,
    /// Session ID
    pub sid: Option<u32>,
    /// Foreground process group ID of the controlling terminal
    pub tpgid: Option<i32>,
    /// Number of threads
    pub threads: Option<u32>,
    /// Kernel wait channel (what the process is waiting on)
    pub wchan: Option<String>,
    /// Process flags
    pub flags: Option<u32>,
    /// Is this process the session leader?
    pub is_session_leader: Option<bool>,
    /// Is this process in the foreground process group?
    pub has_foreground: Option<bool>,
}

/// Process status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessStatus {
    /// Running or runnable (on run queue)
    Running,
    /// Interruptible sleep (waiting for an event)
    Sleeping,
    /// Uninterruptible sleep (usually IO)
    DiskSleep,
    /// Stopped (by signal or tracing)
    Stopped,
    /// Zombie (terminated but not reaped)
    Zombie,
    /// Idle kernel thread
    Idle,
    /// Dead (should never be seen)
    Dead,
    /// Stopped by debugger during tracing
    TracingStop,
    /// Unknown state
    Unknown,
}

impl ProcessInfo {
    /// Get a field by name for filtering/selection.
    pub fn get_field(&self, name: &str) -> Option<Value> {
        match name {
            "pid" => Some(Value::Int(self.pid as i64)),
            "ppid" => Some(Value::Int(self.ppid as i64)),
            "user" | "euser" => Some(Value::String(self.user.clone())),
            "group" => self.group.as_ref().map(|g| Value::String(g.clone())),
            "command" | "cmd" | "comm" => Some(Value::String(self.command.clone())),
            "args" => Some(Value::List(self.args.iter().map(|s| Value::String(s.clone())).collect())),
            "cpu" | "cpu_percent" | "%cpu" | "pcpu" => Some(Value::Float(self.cpu_percent)),
            "mem" | "mem_bytes" | "rss" => Some(Value::Int(self.mem_bytes as i64)),
            "mem_percent" | "%mem" | "pmem" => Some(Value::Float(self.mem_percent)),
            "vsz" | "vsize" | "virtual_size" => Some(Value::Int(self.virtual_size as i64)),
            "status" | "state" | "stat" => Some(Value::String(format!("{:?}", self.status))),
            "started" | "start" | "stime" => self.started.map(|t| Value::Int(t as i64)),
            "time" | "cputime" | "cpu_time" => Some(Value::Int(self.cpu_time as i64)),
            "tty" | "tt" => self.tty.as_ref().map(|t| Value::String(t.clone())),
            "nice" | "ni" => self.nice.map(|n| Value::Int(n as i64)),
            "priority" | "pri" => Some(Value::Int(self.priority as i64)),
            "pgid" | "pgrp" => self.pgid.map(|p| Value::Int(p as i64)),
            "sid" | "sess" => self.sid.map(|s| Value::Int(s as i64)),
            "tpgid" => self.tpgid.map(|t| Value::Int(t as i64)),
            "threads" | "nlwp" => self.threads.map(|t| Value::Int(t as i64)),
            "wchan" => self.wchan.as_ref().map(|w| Value::String(w.clone())),
            "flags" | "f" => self.flags.map(|f| Value::Int(f as i64)),
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

// =============================================================================
// File Operation (cp, mv, rm with progress)
// =============================================================================

/// Progress information for a file operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileOpInfo {
    pub op_type: FileOpKind,
    pub phase: FileOpPhase,
    pub sources: Vec<PathBuf>,
    pub dest: Option<PathBuf>,
    /// Total bytes to process. `None` = still scanning.
    pub total_bytes: Option<u64>,
    pub bytes_processed: u64,
    /// Total files to process. `None` = still scanning.
    pub files_total: Option<usize>,
    pub files_processed: usize,
    pub current_file: Option<PathBuf>,
    /// Milliseconds since Unix epoch when the operation started.
    pub start_time_ms: u64,
    pub errors: Vec<FileOpError>,
}

/// An error encountered during a file operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileOpError {
    pub path: PathBuf,
    pub message: String,
}

/// A chunk of binary data with metadata. Used for non-renderable binaries (archives,
/// executables, etc.) where only a prefix is kept in memory. `data` holds at most
/// 64 KiB; `total_size` reflects the actual file size from metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlobChunk {
    pub data: Vec<u8>,
    pub content_type: String,
    pub offset: u64,
    pub total_size: Option<u64>,
    pub source: Option<String>,
}

/// Kind of file operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileOpKind {
    Copy,
    Move,
    Remove,
    Chmod,
    Chown,
}

/// Phase of a file operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileOpPhase {
    Planning,
    Executing,
    Completed,
    Failed,
}

impl FileOpInfo {
    pub fn get_field(&self, name: &str) -> Option<Value> {
        match name {
            "op_type" => Some(Value::String(format!("{:?}", self.op_type))),
            "phase" => Some(Value::String(format!("{:?}", self.phase))),
            "total_bytes" => self.total_bytes.map(|b| Value::Int(b as i64)),
            "bytes_processed" => Some(Value::Int(self.bytes_processed as i64)),
            "files_total" => self.files_total.map(|n| Value::Int(n as i64)),
            "files_processed" => Some(Value::Int(self.files_processed as i64)),
            "errors" => Some(Value::Int(self.errors.len() as i64)),
            _ => None,
        }
    }
}

// =============================================================================
// Directory Tree (flat arena)
// =============================================================================

/// Flat arena tree representation. Scales to large directories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreeInfo {
    /// Index of the root node in `nodes`.
    pub root: usize,
    pub nodes: Vec<TreeNodeFlat>,
}

/// A single node in the flat tree arena.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreeNodeFlat {
    pub id: usize,
    pub parent: Option<usize>,
    pub name: String,
    pub path: PathBuf,
    pub node_type: FileType,
    pub size: u64,
    pub depth: usize,
    pub child_count: usize,
}

// =============================================================================
// Diff (structured)
// =============================================================================

/// Structured diff information for a single file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffFileInfo {
    pub file_path: String,
    pub old_path: Option<String>,
    pub change_type: GitChangeType,
    pub hunks: Vec<DiffHunk>,
    pub additions: usize,
    pub deletions: usize,
}

/// A single hunk in a diff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffHunk {
    pub header: String,
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub lines: Vec<DiffLine>,
}

/// A single line in a diff hunk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
    pub old_lineno: Option<usize>,
    pub new_lineno: Option<usize>,
}

/// Kind of diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
}

// =============================================================================
// Network Event (ping)
// =============================================================================

/// A network event from ping or similar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetEventInfo {
    pub event_type: NetEventType,
    pub host: String,
    pub ip: Option<String>,
    pub rtt_ms: Option<f64>,
    pub ttl: Option<u32>,
    pub seq: Option<u32>,
    pub success: bool,
    pub message: Option<String>,
}

/// Type of network event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetEventType {
    PingResponse,
    Timeout,
    Error,
}

impl NetEventInfo {
    pub fn get_field(&self, name: &str) -> Option<Value> {
        match name {
            "host" => Some(Value::String(self.host.clone())),
            "ip" => self.ip.as_ref().map(|s| Value::String(s.clone())),
            "rtt_ms" | "rtt" => self.rtt_ms.map(Value::Float),
            "ttl" => self.ttl.map(|t| Value::Int(t as i64)),
            "seq" => self.seq.map(|s| Value::Int(s as i64)),
            "success" => Some(Value::Bool(self.success)),
            _ => None,
        }
    }
}

// =============================================================================
// DNS Answer (dig)
// =============================================================================

/// DNS lookup result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DnsAnswerInfo {
    pub query: String,
    pub record_type: String,
    pub answers: Vec<DnsRecord>,
    pub query_time_ms: f64,
    pub server: String,
    pub from_cache: bool,
}

/// A single DNS record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DnsRecord {
    pub name: String,
    pub record_type: String,
    pub ttl: u32,
    pub data: String,
}

impl DnsAnswerInfo {
    pub fn get_field(&self, name: &str) -> Option<Value> {
        match name {
            "query" => Some(Value::String(self.query.clone())),
            "record_type" => Some(Value::String(self.record_type.clone())),
            "query_time_ms" => Some(Value::Float(self.query_time_ms)),
            "server" => Some(Value::String(self.server.clone())),
            "from_cache" => Some(Value::Bool(self.from_cache)),
            "answer_count" => Some(Value::Int(self.answers.len() as i64)),
            _ => None,
        }
    }
}

// =============================================================================
// HTTP Response (curl)
// =============================================================================

/// HTTP response metadata with optional body preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpResponseInfo {
    pub url: String,
    pub method: String,
    pub status_code: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    /// First 4KB of text bodies.
    pub body_preview: Option<String>,
    pub body_len: u64,
    pub body_truncated: bool,
    pub content_type: Option<String>,
    pub timing: HttpTiming,
}

/// HTTP timing information.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpTiming {
    pub total_ms: f64,
    pub dns_ms: Option<f64>,
    pub connect_ms: Option<f64>,
    pub tls_ms: Option<f64>,
    pub ttfb_ms: Option<f64>,
    pub transfer_ms: Option<f64>,
}

impl HttpResponseInfo {
    pub fn get_field(&self, name: &str) -> Option<Value> {
        match name {
            "url" => Some(Value::String(self.url.clone())),
            "method" => Some(Value::String(self.method.clone())),
            "status_code" | "status" => Some(Value::Int(self.status_code as i64)),
            "status_text" => Some(Value::String(self.status_text.clone())),
            "body_len" | "content_length" => Some(Value::Int(self.body_len as i64)),
            "content_type" => self.content_type.as_ref().map(|s| Value::String(s.clone())),
            "total_ms" => Some(Value::Float(self.timing.total_ms)),
            "dns_ms" => self.timing.dns_ms.map(Value::Float),
            "connect_ms" => self.timing.connect_ms.map(Value::Float),
            "tls_ms" => self.timing.tls_ms.map(Value::Float),
            "ttfb_ms" => self.timing.ttfb_ms.map(Value::Float),
            "transfer_ms" => self.timing.transfer_ms.map(Value::Float),
            _ => None,
        }
    }
}

// =============================================================================
// Interactive Request (less, top, man, tree viewer)
// =============================================================================

/// A request to open an interactive viewer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractiveRequest {
    pub viewer: ViewerKind,
    pub content: Value,
}

/// The kind of interactive viewer to open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ViewerKind {
    Pager,
    ProcessMonitor { interval_ms: u64 },
    TreeBrowser,
    ManPage,
    DiffViewer,
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
        let (permissions, uid, gid, nlink) = {
            use std::os::unix::fs::MetadataExt;
            (
                metadata.mode(),
                Some(metadata.uid()),
                Some(metadata.gid()),
                Some(metadata.nlink()),
            )
        };
        #[cfg(not(unix))]
        let (permissions, uid, gid, nlink) = {
            let p = if metadata.permissions().readonly() {
                0o444
            } else {
                0o644
            };
            (p, None, None, None)
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
            uid,
            gid,
            owner: None, // Resolved at kernel level with libc
            group: None,
            nlink,
        })
    }
}

/// Format a byte size into human-readable form.
pub fn format_size(bytes: u64) -> String {
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

/// Format a value according to a display format hint.
/// This is used by table rendering to show human-friendly output
/// while keeping the underlying value intact for sorting/filtering.
pub fn format_value_for_display(value: &Value, format: DisplayFormat) -> String {
    match format {
        DisplayFormat::HumanBytes => {
            let bytes = match value {
                Value::Int(n) => *n as u64,
                Value::Float(f) => *f as u64,
                _ => return value.to_text(),
            };
            format_size(bytes)
        }
        DisplayFormat::Percentage => {
            match value {
                Value::Float(f) => format!("{:.1}%", f),
                Value::Int(n) => format!("{}%", n),
                _ => value.to_text(),
            }
        }
        DisplayFormat::RelativeTime => {
            let timestamp = match value {
                Value::Int(n) => *n as u64,
                _ => return value.to_text(),
            };
            format_relative_time(timestamp)
        }
        DisplayFormat::DateTime => {
            let timestamp = match value {
                Value::Int(n) => *n as u64,
                _ => return value.to_text(),
            };
            format_datetime(timestamp)
        }
        DisplayFormat::Duration => {
            let secs = match value {
                Value::Int(n) => *n as u64,
                Value::Float(f) => *f as u64,
                _ => return value.to_text(),
            };
            format_duration(secs)
        }
        DisplayFormat::Octal => {
            match value {
                Value::Int(n) => format!("{:o}", n),
                _ => value.to_text(),
            }
        }
    }
}

/// Format a Unix timestamp as relative time (e.g., "2 hours ago").
fn format_relative_time(timestamp: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if timestamp > now {
        return "in the future".to_string();
    }

    let diff = now - timestamp;

    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        let mins = diff / 60;
        format!("{} min{} ago", mins, if mins == 1 { "" } else { "s" })
    } else if diff < 86400 {
        let hours = diff / 3600;
        format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
    } else if diff < 604800 {
        let days = diff / 86400;
        format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
    } else if diff < 2592000 {
        let weeks = diff / 604800;
        format!("{} week{} ago", weeks, if weeks == 1 { "" } else { "s" })
    } else if diff < 31536000 {
        let months = diff / 2592000;
        format!("{} month{} ago", months, if months == 1 { "" } else { "s" })
    } else {
        let years = diff / 31536000;
        format!("{} year{} ago", years, if years == 1 { "" } else { "s" })
    }
}

/// Format a Unix timestamp as datetime string.
fn format_datetime(timestamp: u64) -> String {
    // Simple ISO-like format without external dependencies
    use std::time::{Duration, UNIX_EPOCH};

    let _datetime = UNIX_EPOCH + Duration::from_secs(timestamp);
    // For now, just return the timestamp - a proper implementation would
    // use chrono or similar. This is a placeholder that works.
    format!("{}", timestamp)
}

/// Format duration in seconds as human-readable.
fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        let mins = secs / 60;
        let s = secs % 60;
        if s == 0 {
            format!("{}m", mins)
        } else {
            format!("{}m {}s", mins, s)
        }
    } else if secs < 86400 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        if mins == 0 {
            format!("{}h", hours)
        } else {
            format!("{}h {}m", hours, mins)
        }
    } else {
        let days = secs / 86400;
        let hours = (secs % 86400) / 3600;
        if hours == 0 {
            format!("{}d", days)
        } else {
            format!("{}d {}h", days, hours)
        }
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
            Value::Domain(d) => match d.as_ref() {
                DomainValue::BlobChunk(chunk) => chunk.data.clone(),
                _ => self.to_text().into_bytes(),
            },
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

    // ── Domain value constructors ──────────────────────────────────────

    pub fn file_op(info: FileOpInfo) -> Self {
        Value::Domain(Box::new(DomainValue::FileOp(info)))
    }
    pub fn tree(info: TreeInfo) -> Self {
        Value::Domain(Box::new(DomainValue::Tree(info)))
    }
    pub fn diff_file(info: DiffFileInfo) -> Self {
        Value::Domain(Box::new(DomainValue::DiffFile(info)))
    }
    pub fn net_event(info: NetEventInfo) -> Self {
        Value::Domain(Box::new(DomainValue::NetEvent(info)))
    }
    pub fn dns_answer(info: DnsAnswerInfo) -> Self {
        Value::Domain(Box::new(DomainValue::DnsAnswer(info)))
    }
    pub fn http_response(info: HttpResponseInfo) -> Self {
        Value::Domain(Box::new(DomainValue::HttpResponse(info)))
    }
    pub fn interactive(req: InteractiveRequest) -> Self {
        Value::Domain(Box::new(DomainValue::Interactive(req)))
    }
    pub fn blob_chunk(chunk: BlobChunk) -> Self {
        Value::Domain(Box::new(DomainValue::BlobChunk(chunk)))
    }

    /// Access the inner `DomainValue` if this is a `Value::Domain`.
    pub fn as_domain(&self) -> Option<&DomainValue> {
        match self {
            Value::Domain(d) => Some(d),
            _ => None,
        }
    }

    /// Mutably access the inner `DomainValue` if this is a `Value::Domain`.
    pub fn as_domain_mut(&mut self) -> Option<&mut DomainValue> {
        match self {
            Value::Domain(d) => Some(d),
            _ => None,
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
                let col_names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
                buf.push_str(&col_names.join("\t"));
                buf.push('\n');
                for row in rows {
                    for (i, cell) in row.iter().enumerate() {
                        if i > 0 {
                            buf.push('\t');
                        }
                        // Apply display format if specified
                        if let Some(col) = columns.get(i) {
                            if let Some(format) = &col.format {
                                buf.push_str(&format_value_for_display(cell, *format));
                                continue;
                            }
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
            Value::Domain(d) => d.write_text(buf),
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
            Value::Domain(d) => d.get_field(name),
            Value::Structured { data, .. } => data.get(name).cloned(),
            _ => None,
        }
    }

    /// Check if this value is a domain-specific type that has typed fields.
    pub fn is_typed(&self) -> bool {
        match self {
            Value::Process(_)
            | Value::GitStatus(_)
            | Value::GitCommit(_)
            | Value::FileEntry(_)
            | Value::Structured { .. } => true,
            Value::Domain(d) => d.is_typed(),
            _ => false,
        }
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
            Value::Domain(d) => d.type_name(),
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

// =============================================================================
// Table Helpers
// =============================================================================

impl Value {
    /// Create a table with simple string column names (no format hints).
    /// This is a convenience method for backwards compatibility.
    pub fn table(columns: Vec<impl Into<String>>, rows: Vec<Vec<Value>>) -> Self {
        Value::Table {
            columns: columns.into_iter().map(|c| TableColumn::new(c)).collect(),
            rows,
        }
    }

    /// Create a table with full column definitions (including format hints).
    pub fn table_with_columns(columns: Vec<TableColumn>, rows: Vec<Vec<Value>>) -> Self {
        Value::Table { columns, rows }
    }
}

/// Convert a slice of strings to TableColumn vector.
/// Useful for quick migrations from old code.
pub fn columns_from_strings(names: &[&str]) -> Vec<TableColumn> {
    names.iter().map(|&s| TableColumn::new(s)).collect()
}
