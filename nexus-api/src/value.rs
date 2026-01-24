//! Structured value types for the Nexus command system.
//!
//! Commands return `Value` instead of raw bytes, enabling rich rendering
//! in the GUI while falling back to text for legacy interop.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

/// A structured value that can be passed between commands and rendered by the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Value {
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
    /// Raw bytes
    Bytes(Vec<u8>),
    /// Ordered list of values
    List(Vec<Value>),
    /// Ordered key-value pairs (like a JSON object but ordered)
    Record(Vec<(String, Value)>),
    /// Tabular data with named columns
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
    },

    // Domain-specific rich types
    /// A filesystem path
    Path(PathBuf),
    /// A file/directory entry with metadata
    FileEntry(Box<FileEntry>),
    /// An error value
    Error { code: i32, message: String },
}

/// Metadata about a file or directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Value {
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
            Value::Path(p) => buf.push_str(&p.to_string_lossy()),
            Value::FileEntry(entry) => {
                // Default: just the name (like simple `ls`)
                buf.push_str(&entry.name);
            }
            Value::Error { message, .. } => {
                buf.push_str("error: ");
                buf.push_str(message);
            }
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
