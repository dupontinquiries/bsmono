//! Database handle wrapping rusqlite::Connection.
//! Implements the full CRUD surface of StorageManager.cpp in safe Rust.

use crate::contact::{Contact, ContactState};
use crate::migrations;
use crate::snap::{Snap, SnapId};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use tracing::{debug, warn};

pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open (or create) the database at `path`.
    /// Equivalent to StorageManager::open().
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Cannot open DB at {}", path))?;

        // DELETE journal + FULL sync — guaranteed persistence on mobile SIGKILL.
        // Matches the C++ fix that replaced WAL mode to eliminate the
        // "data in WAL not checkpointed before kill" data loss pattern.
        conn.execute_batch("
            PRAGMA journal_mode = DELETE;
            PRAGMA synchronous  = FULL;
            PRAGMA foreign_keys = ON;
            PRAGMA cache_size   = -8192;
            PRAGMA temp_store   = MEMORY;
            PRAGMA busy_timeout = 5000;
        ")?;

        migrations::apply(&conn)?;

        Ok(Self { conn })
    }

    // ── Contacts ─────────────────────────────────────────────────────────────

    /// Save (insert or replace) a contact. Preserves existing nickname/notes
    /// if the new values are empty (reconnects don't wipe user edits).
    pub fn save_contact(&self, c: &Contact) -> Result<()> {
        if c.id.is_empty() { anyhow::bail!("save_contact: empty id"); }

        // Read existing fields to preserve user edits
        let existing = self.load_contact(&c.id).ok().flatten();
        let nickname = if c.nickname.is_empty() {
            existing.as_ref().map(|e| e.nickname.as_str()).unwrap_or("").to_owned()
        } else { c.nickname.clone() };
        let notes = if c.notes.is_empty() {
            existing.as_ref().map(|e| e.notes.as_str()).unwrap_or("").to_owned()
        } else { c.notes.clone() };
        let vcard_uid = if c.vcard_uid.is_empty() {
            existing.as_ref().map(|e| e.vcard_uid.as_str()).unwrap_or("").to_owned()
        } else { c.vcard_uid.clone() };

        let display = if c.display_name.is_empty() { &c.id } else { &c.display_name };
        let last_seen = c.last_seen.map(|t| t.to_rfc3339()).unwrap_or_default();

        self.conn.execute(
            "INSERT OR REPLACE INTO users
             (id, display_name, avatar_path, online, nickname, notes,
              sc_id, vcard_uid, contact_state, last_seen_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                c.id, display, c.avatar_path, c.online as i64,
                nickname, notes,
                c.id,      // sc_id = same as id
                vcard_uid, c.state.to_int(), last_seen,
            ],
        )?;

        debug!("save_contact: {} {} state={}", c.id, display, c.state.to_int());
        Ok(())
    }

    /// Load all contacts except `self_id`. Sorted nickname-or-displayname ASC.
    pub fn load_contacts(&self, self_id: &str) -> Result<Vec<Contact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, display_name, avatar_path, online, nickname, notes,
                    vcard_uid, contact_state, last_seen_at
             FROM users WHERE id != ?1
             ORDER BY CASE WHEN nickname != '' THEN nickname ELSE display_name END COLLATE NOCASE"
        )?;

        let rows = stmt.query_map(params![self_id], |r| {
            Ok(Contact {
                id:           r.get(0)?,
                display_name: r.get(1)?,
                avatar_path:  r.get(2)?,
                online:       r.get::<_, i64>(3)? != 0,
                nickname:     r.get(4)?,
                notes:        r.get(5)?,
                vcard_uid:    r.get(6)?,
                state:        ContactState::from_int(r.get(7)?),
                last_seen:    r.get::<_, Option<String>>(8)?
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|t| t.with_timezone(&Utc)),
            })
        })?;

        rows.collect::<std::result::Result<_, _>>()
            .map_err(Into::into)
    }

    /// Load a single contact by sc-ID.
    pub fn load_contact(&self, id: &str) -> Result<Option<Contact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, display_name, avatar_path, online, nickname, notes,
                    vcard_uid, contact_state, last_seen_at
             FROM users WHERE id = ?1 LIMIT 1"
        )?;

        let mut rows = stmt.query_map(params![id], |r| {
            Ok(Contact {
                id:           r.get(0)?,
                display_name: r.get(1)?,
                avatar_path:  r.get(2)?,
                online:       r.get::<_, i64>(3)? != 0,
                nickname:     r.get(4)?,
                notes:        r.get(5)?,
                vcard_uid:    r.get(6)?,
                state:        ContactState::from_int(r.get(7)?),
                last_seen:    r.get::<_, Option<String>>(8)?
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|t| t.with_timezone(&Utc)),
            })
        })?;

        Ok(rows.next().transpose()?)
    }

    // ── Snaps (scaffold — full implementation mirrors loadSnap/saveSnap) ──────

    pub fn save_snap(&self, s: &Snap) -> Result<()> {
        let delivered = s.delivered_at.map(|t| t.to_rfc3339()).unwrap_or_default();
        self.conn.execute(
            "INSERT OR REPLACE INTO snaps
             (id, sender_id, recipient_id, content_hash, local_path,
              duration_secs, sent_at, delivered_at, viewed,
              encoded_duration_ms, thumbnail_path, has_audio, is_looping)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                s.id.0, s.sender_id, s.recipient_id, s.content_hash,
                s.local_path, s.duration_secs as i64,
                s.sent_at.to_rfc3339(), delivered, s.viewed as i64,
                s.encoded_duration_ms as i64, s.thumbnail_path,
                s.has_audio as i64, s.is_looping as i64,
            ],
        )?;
        Ok(())
    }

    pub fn load_snap(&self, id: &SnapId) -> Result<Option<Snap>> {
        // Scaffold — mirrors C++ loadSnap column mapping
        let _ = id;
        Ok(None) // TODO: implement
    }

    pub fn undelivered_snap_ids(&self, recipient_id: &str, since: &DateTime<Utc>) -> Result<Vec<SnapId>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM snaps WHERE recipient_id=?1 AND delivered_at='' AND sent_at>=?2"
        )?;
        let ids = stmt.query_map(params![recipient_id, since.to_rfc3339()], |r| {
            Ok(SnapId(r.get(0)?))
        })?.collect::<std::result::Result<_, _>>()?;
        Ok(ids)
    }

    pub fn mark_delivered(&self, id: &SnapId) -> Result<()> {
        self.conn.execute(
            "UPDATE snaps SET delivered_at=?1 WHERE id=?2",
            params![Utc::now().to_rfc3339(), id.0],
        )?;
        Ok(())
    }
}
