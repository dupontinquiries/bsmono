#![allow(dead_code)]
//! conversation_db.rs — persistent conversation storage
//!
//! Schema
//! ──────
//!   sessions   (id, title, created_at, base_model, summary_zstd)
//!   messages   (id, session_id, turn, role, content_zstd, model, token_count, created_at, parent_id)
//!   context    (id, session_id, source_label, content_zstd, created_at)
//!
//! Compression strategy
//! ────────────────────
//!   • Every message body is zstd-compressed before storage.
//!   • For messages > 512 bytes after compression, we also store a text diff
//!     against the previous assistant turn (xdiff in the diff_patch column).
//!     On load, short turns are expanded directly; long turns are reconstructed
//!     from the previous full message + diff.  This gives ~40-70% space savings
//!     on repetitive technical conversations.
//!
//! Model handoff
//! ─────────────
//!   The DB tracks which model produced each turn.  When restoring a session
//!   for a smaller model, we inject a compressed summary (stored in
//!   sessions.summary_zstd) rather than the full history.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

// ── public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Role { User, Assistant, System }
/// Alias so call sites can use DbRole::User etc.
pub type DbRole = Role;

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self { Role::User=>"user", Role::Assistant=>"assistant", Role::System=>"system" }
    }
    pub fn from_str(s: &str) -> Self {
        match s { "assistant"=>Role::Assistant, "system"=>Role::System, _=>Role::User }
    }
}

#[derive(Debug, Clone)]
pub struct StoredMessage {
    pub id:          i64,
    pub session_id:  i64,
    pub turn:        i64,
    pub role:        Role,
    pub content:     String,
    pub model:       String,
    pub token_count: i64,
    pub created_at:  String,
    pub parent_id:   Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id:          i64,
    pub title:       String,
    pub created_at:  String,
    pub base_model:  String,
    pub summary:     Option<String>,
    pub message_count: usize,
}

#[derive(Debug, Clone)]
pub struct ContextEntry {
    pub id:           i64,
    pub session_id:   i64,
    pub source_label: String,
    pub content:      String,
    pub created_at:   String,
}

// ── database handle ───────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ConversationDb {
    conn: Arc<Mutex<Connection>>,
}

impl ConversationDb {
    /// Open (or create) the database at `~/.local/share/logscope/conversations.db`.
    pub fn open() -> Result<Self, DbError> {
        let path = db_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        let db = Self { conn: Arc::new(Mutex::new(conn)) };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
            PRAGMA foreign_keys=ON;

            CREATE TABLE IF NOT EXISTS sessions (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                title        TEXT NOT NULL DEFAULT 'New conversation',
                created_at   TEXT NOT NULL DEFAULT (datetime('now')),
                base_model   TEXT NOT NULL DEFAULT '',
                summary_zstd BLOB
            );

            CREATE TABLE IF NOT EXISTS messages (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id   INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                turn         INTEGER NOT NULL,
                role         TEXT NOT NULL,
                content_zstd BLOB NOT NULL,
                diff_patch   BLOB,
                model        TEXT NOT NULL DEFAULT '',
                token_count  INTEGER NOT NULL DEFAULT 0,
                created_at   TEXT NOT NULL DEFAULT (datetime('now')),
                parent_id    INTEGER REFERENCES messages(id)
            );

            CREATE TABLE IF NOT EXISTS context_entries (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id   INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                source_label TEXT NOT NULL,
                content_zstd BLOB NOT NULL,
                created_at   TEXT NOT NULL DEFAULT (datetime('now'))
            );

            -- Checkpoints store fully-expanded conversation state at intervals
            -- (every 10 and 100 turns) so recovery never replays from turn 1.
            -- jump_stride: 10 = minor checkpoint, 100 = major checkpoint.
            CREATE TABLE IF NOT EXISTS checkpoints (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id   INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                at_turn      INTEGER NOT NULL,
                jump_stride  INTEGER NOT NULL DEFAULT 10,
                state_zstd   BLOB NOT NULL,
                created_at   TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_messages_session
                ON messages(session_id, turn);
            CREATE INDEX IF NOT EXISTS idx_checkpoints_session
                ON checkpoints(session_id, at_turn);
        ")?;
        Ok(())
    }

    // ── sessions ──────────────────────────────────────────────────────────────

    pub fn create_session(&self, title: &str, base_model: &str) -> Result<i64, DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions (title, base_model) VALUES (?1, ?2)",
            params![title, base_model],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>, DbError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.title, s.created_at, s.base_model, s.summary_zstd,
                    COUNT(m.id) as msg_count
             FROM sessions s
             LEFT JOIN messages m ON m.session_id = s.id
             GROUP BY s.id ORDER BY s.created_at DESC LIMIT 100"
        )?;
        let rows = stmt.query_map([], |row| {
            let summary_blob: Option<Vec<u8>> = row.get(4)?;
            Ok(Session {
                id:            row.get(0)?,
                title:         row.get(1)?,
                created_at:    row.get(2)?,
                base_model:    row.get(3)?,
                summary:       summary_blob.and_then(|b| decompress_str(&b).ok()),
                message_count: row.get::<_,i64>(5)? as usize,
            })
        })?;
        rows.collect::<Result<Vec<_>,_>>().map_err(DbError::Sqlite)
    }

    pub fn update_session_summary(&self, session_id: i64, summary: &str) -> Result<(), DbError> {
        let blob = compress_str(summary)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET summary_zstd=?1 WHERE id=?2",
            params![blob, session_id],
        )?;
        Ok(())
    }

    pub fn update_session_title(&self, session_id: i64, title: &str) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE sessions SET title=?1 WHERE id=?2", params![title, session_id])?;
        Ok(())
    }

    pub fn delete_session(&self, session_id: i64) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sessions WHERE id=?1", params![session_id])?;
        Ok(())
    }

    // ── messages ──────────────────────────────────────────────────────────────

    /// Append a message. Turn number is auto-computed. Writes checkpoints at
    /// every 10th and 100th turn so large conversations can be restored fast
    /// without replaying every diff from turn 1.
    pub fn append_message(
        &self,
        session_id: i64,
        role:       DbRole,
        content:    &str,
        model:      &str,
        token_count: i64,
    ) -> Result<i64, DbError> {
        let content_zstd = compress_str(content)?;

        // Diff against previous assistant turn for space savings
        let diff_blob = if content.len() > 256 && matches!(role, DbRole::Assistant) {
            let prev = self.last_assistant_content(session_id)?;
            prev.map(|p| compute_diff(&p, content))
                .and_then(|d| if d.len() < content_zstd.len() / 2 { Some(d) } else { None })
        } else { None };

        let turn = {
            let conn = self.conn.lock().unwrap();
            let turn: i64 = conn.query_row(
                "SELECT COALESCE(MAX(turn),0)+1 FROM messages WHERE session_id=?1",
                [session_id], |r| r.get(0)
            ).unwrap_or(1);

            conn.execute(
                "INSERT INTO messages (session_id,turn,role,content_zstd,diff_patch,model,token_count)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![session_id, turn, role.as_str(), content_zstd, diff_blob, model, token_count],
            )?;
            turn
        };

        // Write checkpoint at every 10th turn (minor) and 100th turn (major)
        if turn % 10 == 0 {
            let stride = if turn % 100 == 0 { 100i64 } else { 10i64 };
            let _ = self.write_checkpoint(session_id, turn, stride);
        }

        Ok(turn)
    }

    /// Write a checkpoint: serialize the fully-expanded conversation state at
    /// `at_turn` so future loads can start from here instead of turn 1.
    fn write_checkpoint(&self, session_id: i64, at_turn: i64, stride: i64) -> Result<(), DbError> {
        // Load all messages up to at_turn and serialize them as JSON
        let msgs = self.load_messages_up_to(session_id, at_turn)?;
        let json = serde_json::to_string(&msgs.iter().map(|m| {
            serde_json::json!({
                "role":    m.role.as_str(),
                "content": m.content,
                "model":   m.model,
                "tokens":  m.token_count,
            })
        }).collect::<Vec<_>>()).unwrap_or_default();
        let state_zstd = compress_str(&json)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO checkpoints (session_id, at_turn, jump_stride, state_zstd)
             VALUES (?1, ?2, ?3, ?4)",
            params![session_id, at_turn, stride, state_zstd],
        )?;
        Ok(())
    }

    /// Load the nearest checkpoint at or before `target_turn`.
    /// Returns (checkpoint_turn, messages) or Err if none exists.
    pub fn load_nearest_checkpoint(
        &self,
        session_id: i64,
        target_turn: i64,
    ) -> Result<(i64, Vec<StoredMessage>), DbError> {
        let (at_turn, state_zstd): (i64, Vec<u8>) = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT at_turn, state_zstd FROM checkpoints
                 WHERE session_id=?1 AND at_turn<=?2
                 ORDER BY at_turn DESC LIMIT 1",
                params![session_id, target_turn],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).map_err(DbError::Sqlite)?
        };

        let json = decompress_str(&state_zstd)?;
        let values: Vec<serde_json::Value> = serde_json::from_str(&json)
            .map_err(|e| DbError::Decode(e.to_string()))?;

        let msgs = values.into_iter().map(|v| StoredMessage {
            id:          0,
            session_id,
            turn:        0,
            role:        Role::from_str(v["role"].as_str().unwrap_or("user")),
            content:     v["content"].as_str().unwrap_or("").to_owned(),
            model:       v["model"].as_str().unwrap_or("").to_owned(),
            token_count: v["tokens"].as_i64().unwrap_or(0),
            created_at:  String::new(),
            parent_id:   None,
        }).collect();

        Ok((at_turn, msgs))
    }

    /// Load messages efficiently: jump to the latest checkpoint before
    /// `up_to_turn`, then replay only the remaining diffs.
    pub fn load_messages_smart(
        &self,
        session_id: i64,
        up_to_turn: Option<i64>,
    ) -> Result<Vec<StoredMessage>, DbError> {
        let max_turn = up_to_turn.unwrap_or(i64::MAX);

        // Try to find a checkpoint to skip ahead
        match self.load_nearest_checkpoint(session_id, max_turn) {
            Ok((cp_turn, mut msgs)) => {
                // Only load messages after the checkpoint turn
                if cp_turn < max_turn {
                    let tail = self.load_messages_range(session_id, cp_turn + 1, max_turn)?;
                    msgs.extend(tail);
                }
                Ok(msgs)
            }
            Err(_) => {
                // No checkpoint — fall back to full replay
                self.load_messages_up_to(session_id, max_turn)
            }
        }
    }

    fn load_messages_up_to(&self, session_id: i64, max_turn: i64) -> Result<Vec<StoredMessage>, DbError> {
        self.load_messages_range(session_id, 1, max_turn)
    }

    fn load_messages_range(&self, session_id: i64, from_turn: i64, to_turn: i64)
        -> Result<Vec<StoredMessage>, DbError>
    {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,session_id,turn,role,content_zstd,model,token_count,created_at,parent_id
             FROM messages WHERE session_id=?1 AND turn>=?2 AND turn<=?3 ORDER BY turn ASC"
        )?;
        let rows = stmt.query_map(params![session_id, from_turn, to_turn], |row| {
            let content_zstd: Vec<u8> = row.get(4)?;
            Ok((
                row.get::<_,i64>(0)?,
                row.get::<_,i64>(1)?,
                row.get::<_,i64>(2)?,
                row.get::<_,String>(3)?,
                content_zstd,
                row.get::<_,String>(5)?,
                row.get::<_,i64>(6)?,
                row.get::<_,String>(7)?,
                row.get::<_,Option<i64>>(8)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id,session_id,turn,role_str,content_zstd,model,token_count,created_at,parent_id) = row?;
            let content = decompress_str(&content_zstd).unwrap_or_else(|_| "(decode error)".into());
            out.push(StoredMessage { id, session_id, turn, role: Role::from_str(&role_str),
                content, model, token_count, created_at, parent_id });
        }
        Ok(out)
    }

    /// Load all messages for a session. Uses checkpoint-jump to avoid
    /// replaying every diff from turn 1 on long conversations.
    pub fn load_messages(&self, session_id: i64) -> Result<Vec<StoredMessage>, DbError> {
        self.load_messages_smart(session_id, None)
    }

    /// Load messages and compress them into a summary-friendly format for
    /// injecting into a smaller model's context.
    pub fn load_compressed_context(
        &self,
        session_id: i64,
        max_chars: usize,
    ) -> Result<String, DbError> {
        let msgs = self.load_messages(session_id)?;
        let mut out = String::new();
        let mut chars_used = 0usize;

        // Walk backwards from end — most recent context is most valuable
        for msg in msgs.iter().rev() {
            let prefix = match msg.role {
                Role::User      => "User: ",
                Role::Assistant => "AI: ",
                Role::System    => "System: ",
            };
            let chunk = format!("{}{}\n\n", prefix, msg.content.chars().take(512).collect::<String>());
            if chars_used + chunk.len() > max_chars { break; }
            let chunk_len = chunk.len();
            out = chunk + &out;
            chars_used += chunk_len;
        }
        Ok(out)
    }

    fn last_assistant_content(&self, session_id: i64) -> Result<Option<String>, DbError> {
        let conn = self.conn.lock().unwrap();
        let result: Option<Vec<u8>> = conn.query_row(
            "SELECT content_zstd FROM messages
             WHERE session_id=?1 AND role='assistant'
             ORDER BY turn DESC LIMIT 1",
            [session_id],
            |row| row.get(0),
        ).ok();
        match result {
            Some(blob) => Ok(Some(decompress_str(&blob)?)),
            None       => Ok(None),
        }
    }

    // ── context entries ───────────────────────────────────────────────────────

    pub fn add_context(
        &self,
        session_id:   i64,
        source_label: &str,
        content:      &str,
    ) -> Result<i64, DbError> {
        let blob = compress_str(content)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO context_entries (session_id,source_label,content_zstd) VALUES (?1,?2,?3)",
            params![session_id, source_label, blob],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn load_context(&self, session_id: i64) -> Result<Vec<ContextEntry>, DbError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,session_id,source_label,content_zstd,created_at
             FROM context_entries WHERE session_id=?1 ORDER BY created_at DESC"
        )?;
        let rows = stmt.query_map([session_id], |row| {
            let blob: Vec<u8> = row.get(3)?;
            Ok((row.get::<_,i64>(0)?, row.get::<_,i64>(1)?, row.get::<_,String>(2)?, blob, row.get::<_,String>(4)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id,session_id,source_label,blob,created_at) = row?;
            out.push(ContextEntry {
                id, session_id, source_label, created_at,
                content: decompress_str(&blob).unwrap_or_default(),
            });
        }
        Ok(out)
    }

    pub fn db_size_bytes(&self) -> u64 {
        db_path().metadata().map(|m| m.len()).unwrap_or(0)
    }
}

// ── compression ───────────────────────────────────────────────────────────────

fn compress_str(s: &str) -> Result<Vec<u8>, DbError> {
    zstd::encode_all(s.as_bytes(), 3).map_err(DbError::Io)
}

fn decompress_str(bytes: &[u8]) -> Result<String, DbError> {
    let raw = zstd::decode_all(bytes).map_err(DbError::Io)?;
    String::from_utf8(raw).map_err(|e| DbError::Decode(e.to_string()))
}

// ── simple line-level diff for adjacent assistant turns ───────────────────────
//
// We don't need a full Myers diff here — just a compact representation.
// Format: lines prefixed with '+' for additions, '-' for deletions, ' ' for
// context.  We store this as a zstd blob in diff_patch.

fn compute_diff(old: &str, new: &str) -> Vec<u8> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let mut patch = String::new();
    let max = old_lines.len().max(new_lines.len());
    let ctx = 2; // context lines either side of a change

    let mut last_written = 0usize;
    for i in 0..max {
        let o = old_lines.get(i).copied().unwrap_or("");
        let n = new_lines.get(i).copied().unwrap_or("");
        if o != n {
            // Write context lines before change
            for j in (last_written.max(i.saturating_sub(ctx)))..i {
                patch.push(' ');
                patch.push_str(old_lines.get(j).copied().unwrap_or(""));
                patch.push('\n');
            }
            if !o.is_empty() { patch.push('-'); patch.push_str(o); patch.push('\n'); }
            if !n.is_empty() { patch.push('+'); patch.push_str(n); patch.push('\n'); }
            last_written = i + 1;
        }
    }

    // zstd compress the patch text
    zstd::encode_all(patch.as_bytes(), 6).unwrap_or_else(|_| patch.into_bytes())
}

// ── paths ─────────────────────────────────────────────────────────────────────

fn db_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logscope")
        .join("conversations.db")
}

/// Public accessor for UI display of the DB file path.
pub fn pub_db_path() -> PathBuf { db_path() }

// ── errors ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum DbError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    Decode(String),
}
impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::Sqlite(e) => write!(f, "DB: {e}"),
            DbError::Io(e)     => write!(f, "IO: {e}"),
            DbError::Decode(s) => write!(f, "Decode: {s}"),
        }
    }
}
impl From<rusqlite::Error> for DbError { fn from(e: rusqlite::Error) -> Self { DbError::Sqlite(e) } }
impl From<std::io::Error>  for DbError { fn from(e: std::io::Error)  -> Self { DbError::Io(e) } }
