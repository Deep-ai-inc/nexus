//! SQLite-backed persistence for sessions, commands, and outputs.
//!
//! This module provides the foundation for:
//! - Infinite command history with full-text search
//! - Session persistence (resume where you left off)
//! - Block/output storage (infinite scrollback)
//! - Cross-session sync
//! - Command frequency tracking for smart suggestions

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use nexus_api::{BlockId, Value};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;

/// Database version for migrations.
const SCHEMA_VERSION: i32 = 1;

/// The persistence store backed by SQLite.
pub struct Store {
    conn: Connection,
}

/// A stored command entry.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub id: i64,
    pub command: String,
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub timestamp: DateTime<Utc>,
    pub session_id: Option<i64>,
}

/// A stored session.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: i64,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub cwd: String,
}

/// A stored block (command + output).
#[derive(Debug, Clone)]
pub struct StoredBlock {
    pub id: i64,
    pub block_id: u64,
    pub session_id: i64,
    pub command: String,
    pub output_json: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub timestamp: DateTime<Utc>,
}

impl Store {
    /// Open or create the database at the default location (~/.nexus/nexus.db).
    pub fn open_default() -> Result<Self> {
        let path = default_db_path()?;
        Self::open(&path)
    }

    /// Open or create the database at a specific path.
    pub fn open(path: &PathBuf) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {:?}", parent))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database: {:?}", path))?;

        let mut store = Self { conn };
        store.initialize()?;
        Ok(store)
    }

    /// Open an in-memory database (for testing).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let mut store = Self { conn };
        store.initialize()?;
        Ok(store)
    }

    /// Initialize the database schema.
    fn initialize(&mut self) -> Result<()> {
        let version = self.get_schema_version()?;

        if version == 0 {
            self.create_schema()?;
        } else if version < SCHEMA_VERSION {
            self.migrate(version)?;
        }

        Ok(())
    }

    /// Get the current schema version.
    fn get_schema_version(&self) -> Result<i32> {
        // Check if the meta table exists
        let exists: bool = self.conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta'",
            [],
            |_| Ok(true),
        ).unwrap_or(false);

        if !exists {
            return Ok(0);
        }

        let version: i32 = self.conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |row| {
                let v: String = row.get(0)?;
                Ok(v.parse().unwrap_or(0))
            })
            .unwrap_or(0);

        Ok(version)
    }

    /// Create the initial schema.
    fn create_schema(&mut self) -> Result<()> {
        self.conn.execute_batch(r#"
            -- Metadata table for schema versioning
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Sessions table
            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                cwd TEXT NOT NULL
            );

            -- Command history with full-text search
            CREATE TABLE IF NOT EXISTS history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                command TEXT NOT NULL,
                cwd TEXT NOT NULL,
                exit_code INTEGER,
                duration_ms INTEGER,
                timestamp TEXT NOT NULL,
                session_id INTEGER,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            -- Full-text search index for history
            CREATE VIRTUAL TABLE IF NOT EXISTS history_fts USING fts5(
                command,
                content='history',
                content_rowid='id'
            );

            -- Triggers to keep FTS in sync
            CREATE TRIGGER IF NOT EXISTS history_ai AFTER INSERT ON history BEGIN
                INSERT INTO history_fts(rowid, command) VALUES (new.id, new.command);
            END;

            CREATE TRIGGER IF NOT EXISTS history_ad AFTER DELETE ON history BEGIN
                INSERT INTO history_fts(history_fts, rowid, command) VALUES('delete', old.id, old.command);
            END;

            CREATE TRIGGER IF NOT EXISTS history_au AFTER UPDATE ON history BEGIN
                INSERT INTO history_fts(history_fts, rowid, command) VALUES('delete', old.id, old.command);
                INSERT INTO history_fts(rowid, command) VALUES (new.id, new.command);
            END;

            -- Blocks table (command + structured output)
            CREATE TABLE IF NOT EXISTS blocks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                block_id INTEGER NOT NULL,
                session_id INTEGER NOT NULL,
                command TEXT NOT NULL,
                output_json TEXT,
                exit_code INTEGER,
                duration_ms INTEGER,
                timestamp TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            -- Index for fast session lookup
            CREATE INDEX IF NOT EXISTS idx_blocks_session ON blocks(session_id);
            CREATE INDEX IF NOT EXISTS idx_history_session ON history(session_id);
            CREATE INDEX IF NOT EXISTS idx_history_timestamp ON history(timestamp DESC);

            -- Set schema version
            INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', '1');
        "#)?;

        Ok(())
    }

    /// Migrate from an older schema version.
    fn migrate(&mut self, _from_version: i32) -> Result<()> {
        // Future migrations go here
        Ok(())
    }

    // =========================================================================
    // Session operations
    // =========================================================================

    /// Start a new session.
    pub fn start_session(&self, cwd: &str) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO sessions (started_at, cwd) VALUES (?1, ?2)",
            params![now, cwd],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// End a session.
    pub fn end_session(&self, session_id: i64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE sessions SET ended_at = ?1 WHERE id = ?2",
            params![now, session_id],
        )?;
        Ok(())
    }

    /// Get the most recent session.
    pub fn get_latest_session(&self) -> Result<Option<Session>> {
        self.conn
            .query_row(
                "SELECT id, started_at, ended_at, cwd FROM sessions ORDER BY id DESC LIMIT 1",
                [],
                |row| {
                    Ok(Session {
                        id: row.get(0)?,
                        started_at: parse_datetime(row.get::<_, String>(1)?),
                        ended_at: row.get::<_, Option<String>>(2)?.map(parse_datetime),
                        cwd: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    // =========================================================================
    // History operations
    // =========================================================================

    /// Add a command to history.
    pub fn add_history(
        &self,
        command: &str,
        cwd: &str,
        exit_code: Option<i32>,
        duration_ms: Option<u64>,
        session_id: Option<i64>,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO history (command, cwd, exit_code, duration_ms, timestamp, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![command, cwd, exit_code, duration_ms.map(|d| d as i64), now, session_id],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Search history using full-text search.
    pub fn search_history(&self, query: &str, limit: usize) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT h.id, h.command, h.cwd, h.exit_code, h.duration_ms, h.timestamp, h.session_id
             FROM history h
             JOIN history_fts fts ON h.id = fts.rowid
             WHERE history_fts MATCH ?1
             ORDER BY h.timestamp DESC
             LIMIT ?2"
        )?;

        let entries = stmt
            .query_map(params![query, limit as i64], |row| {
                Ok(HistoryEntry {
                    id: row.get(0)?,
                    command: row.get(1)?,
                    cwd: row.get(2)?,
                    exit_code: row.get(3)?,
                    duration_ms: row.get::<_, Option<i64>>(4)?.map(|d| d as u64),
                    timestamp: parse_datetime(row.get::<_, String>(5)?),
                    session_id: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    /// Get recent history entries.
    pub fn get_recent_history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, command, cwd, exit_code, duration_ms, timestamp, session_id
             FROM history
             ORDER BY timestamp DESC
             LIMIT ?1"
        )?;

        let entries = stmt
            .query_map(params![limit as i64], |row| {
                Ok(HistoryEntry {
                    id: row.get(0)?,
                    command: row.get(1)?,
                    cwd: row.get(2)?,
                    exit_code: row.get(3)?,
                    duration_ms: row.get::<_, Option<i64>>(4)?.map(|d| d as u64),
                    timestamp: parse_datetime(row.get::<_, String>(5)?),
                    session_id: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    /// Get command frequency (for suggestions).
    pub fn get_command_frequency(&self, prefix: &str, limit: usize) -> Result<Vec<(String, i64)>> {
        let pattern = format!("{}%", prefix);
        let mut stmt = self.conn.prepare(
            "SELECT command, COUNT(*) as freq
             FROM history
             WHERE command LIKE ?1
             GROUP BY command
             ORDER BY freq DESC
             LIMIT ?2"
        )?;

        let results = stmt
            .query_map(params![pattern, limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(results)
    }

    /// Get total history count.
    pub fn history_count(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM history", [], |row| row.get(0))
            .map_err(Into::into)
    }

    // =========================================================================
    // Block operations
    // =========================================================================

    /// Save a block (command + output).
    pub fn save_block(
        &self,
        block_id: BlockId,
        session_id: i64,
        command: &str,
        output: Option<&Value>,
        exit_code: Option<i32>,
        duration_ms: Option<u64>,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let output_json = output.map(|v| serde_json::to_string(v).unwrap_or_default());

        self.conn.execute(
            "INSERT INTO blocks (block_id, session_id, command, output_json, exit_code, duration_ms, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                block_id.0 as i64,
                session_id,
                command,
                output_json,
                exit_code,
                duration_ms.map(|d| d as i64),
                now
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Get blocks for a session.
    pub fn get_session_blocks(&self, session_id: i64) -> Result<Vec<StoredBlock>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, block_id, session_id, command, output_json, exit_code, duration_ms, timestamp
             FROM blocks
             WHERE session_id = ?1
             ORDER BY id ASC"
        )?;

        let blocks = stmt
            .query_map(params![session_id], |row| {
                Ok(StoredBlock {
                    id: row.get(0)?,
                    block_id: row.get::<_, i64>(1)? as u64,
                    session_id: row.get(2)?,
                    command: row.get(3)?,
                    output_json: row.get(4)?,
                    exit_code: row.get(5)?,
                    duration_ms: row.get::<_, Option<i64>>(6)?.map(|d| d as u64),
                    timestamp: parse_datetime(row.get::<_, String>(7)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(blocks)
    }

    /// Parse stored output JSON back to Value.
    pub fn parse_block_output(json: &str) -> Option<Value> {
        serde_json::from_str(json).ok()
    }
}

/// Get the default database path.
fn default_db_path() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .context("HOME environment variable not set")?;
    Ok(PathBuf::from(home).join(".nexus").join("nexus.db"))
}

/// Parse an RFC3339 datetime string.
fn parse_datetime(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_search_history() {
        let store = Store::open_in_memory().unwrap();

        // Start a session
        let session_id = store.start_session("/home/user").unwrap();

        // Add some history
        store.add_history("ls -la", "/home/user", Some(0), Some(50), Some(session_id)).unwrap();
        store.add_history("git status", "/home/user", Some(0), Some(100), Some(session_id)).unwrap();
        store.add_history("git commit -m 'test'", "/home/user", Some(0), Some(200), Some(session_id)).unwrap();

        // Search for git commands
        let results = store.search_history("git", 10).unwrap();
        assert_eq!(results.len(), 2);

        // Get recent history
        let recent = store.get_recent_history(10).unwrap();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].command, "git commit -m 'test'"); // Most recent first
    }

    #[test]
    fn test_command_frequency() {
        let store = Store::open_in_memory().unwrap();

        // Add repeated commands
        for _ in 0..5 {
            store.add_history("git status", "/", None, None, None).unwrap();
        }
        for _ in 0..3 {
            store.add_history("git commit", "/", None, None, None).unwrap();
        }
        store.add_history("ls", "/", None, None, None).unwrap();

        // Get frequency for git commands
        let freq = store.get_command_frequency("git", 10).unwrap();
        assert_eq!(freq.len(), 2);
        assert_eq!(freq[0].0, "git status");
        assert_eq!(freq[0].1, 5);
        assert_eq!(freq[1].0, "git commit");
        assert_eq!(freq[1].1, 3);
    }

    #[test]
    fn test_blocks() {
        let store = Store::open_in_memory().unwrap();
        let session_id = store.start_session("/home/user").unwrap();

        // Save a block with output
        let output = Value::List(vec![Value::String("file1.txt".into()), Value::String("file2.txt".into())]);
        store.save_block(
            BlockId(1),
            session_id,
            "ls",
            Some(&output),
            Some(0),
            Some(50),
        ).unwrap();

        // Get blocks
        let blocks = store.get_session_blocks(session_id).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].command, "ls");

        // Parse output
        let parsed = Store::parse_block_output(blocks[0].output_json.as_ref().unwrap());
        assert!(parsed.is_some());
    }
}
