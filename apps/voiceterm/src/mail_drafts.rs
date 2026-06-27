// src/mail_drafts.rs
//
// v0.1.878: local draft store for the mail bridge.
//
// Drafts are LOCAL working copies of unsent messages — composing,
// saving, and re-editing all happen on-device with no account, no
// network, and no crypto. (Sending is a separate, still-unwired step
// that needs the SRP login + SMTP seams.) That makes drafts the one
// part of the compose flow that's fully functional today, so it's
// worth shipping on its own.
//
// Storage: one JSON file per draft at
// `~/.config/web2qml/mail/drafts/<id>.json`, alongside the account
// store. Plaintext — a draft is an unsent local note in the user's own
// config dir; we don't have a SecretBox available without the crypto
// seam, and the threat model for an unsent local draft is low. If/when
// encrypted-at-rest drafts are wanted, swap the read/write helpers for
// the SecretBox path the account store uses.
//
// Serialization goes through `mail_json` (no serde in the tree).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::mail_json::{Json, Obj};

/// A locally-stored draft message.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Draft {
    pub id: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    /// Unix milliseconds of the last save. Newest-first ordering.
    pub updated_at: i64,
}

impl Draft {
    fn to_json(&self) -> String {
        Obj::new()
            .str("id", &self.id)
            .str("to", &self.to)
            .str("subject", &self.subject)
            .str("body", &self.body)
            .int("updated_at", self.updated_at)
            .build()
    }

    fn from_json(s: &str) -> Option<Draft> {
        let j: Json = Json::parse(s).ok()?;
        Some(Draft {
            id: j.str_field("id")?,
            to: j.str_field("to").unwrap_or_default(),
            subject: j.str_field("subject").unwrap_or_default(),
            body: j.str_field("body").unwrap_or_default(),
            updated_at: j.int_field("updated_at").unwrap_or(0),
        })
    }

    /// Map to the renderer row (mail_panes::DraftRow), deriving a short
    /// snippet from the body.
    pub fn to_row(&self) -> crate::mail_panes::DraftRow {
        crate::mail_panes::DraftRow {
            id: self.id.clone(),
            to: self.to.clone(),
            subject: self.subject.clone(),
            snippet: snippet(&self.body, 80),
        }
    }
}

/// First `max` chars of the body, single-lined, ellipsized.
fn snippet(body: &str, max: usize) -> String {
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max).collect();
        format!("{}\u{2026}", cut.trim_end())
    }
}

static DRAFT_SEQ: AtomicU64 = AtomicU64::new(0);

/// Unix milliseconds now (0 if the clock is before the epoch).
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A fresh draft id: millis + a process-local sequence so two saves in
/// the same millisecond don't collide. Slug-safe (filename-safe).
fn new_id() -> String {
    let seq = DRAFT_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("d{}-{}", now_millis(), seq)
}

/// Reject ids that aren't our own slug shape, so a hostile/garbled id
/// can't escape the drafts dir via path traversal.
fn id_is_safe(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub struct DraftStore {
    dir: PathBuf,
}

impl DraftStore {
    /// Default location: `~/.config/web2qml/mail/drafts/`.
    pub fn open_default() -> std::io::Result<Self> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let dir = PathBuf::from(home)
            .join(".config/web2qml")
            .join("mail")
            .join("drafts");
        Self::open_at(dir)
    }

    /// Open (creating if needed) at an explicit dir. Used by tests.
    pub fn open_at(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(DraftStore { dir })
    }

    fn path_for(&self, id: &str) -> Option<PathBuf> {
        if !id_is_safe(id) { return None; }
        Some(self.dir.join(format!("{}.json", id)))
    }

    /// Save a draft. With `id = None` a new draft is created and its id
    /// returned; with `id = Some(existing)` that draft is overwritten
    /// (and re-stamped). Returns the draft id.
    pub fn save(
        &self,
        id: Option<&str>,
        to: &str,
        subject: &str,
        body: &str,
    ) -> std::io::Result<String> {
        let id = match id {
            Some(existing) if id_is_safe(existing) => existing.to_string(),
            _ => new_id(),
        };
        let draft = Draft {
            id: id.clone(),
            to: to.to_string(),
            subject: subject.to_string(),
            body: body.to_string(),
            updated_at: now_millis(),
        };
        let path = self.path_for(&id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "unsafe draft id")
        })?;
        std::fs::write(path, draft.to_json())?;
        Ok(id)
    }

    /// Load one draft by id.
    pub fn get(&self, id: &str) -> Option<Draft> {
        let path = self.path_for(id)?;
        let s = std::fs::read_to_string(path).ok()?;
        Draft::from_json(&s)
    }

    /// All drafts, newest first.
    pub fn list(&self) -> Vec<Draft> {
        let mut out = read_all(&self.dir);
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
        out
    }

    /// Delete a draft. Missing id is not an error (idempotent).
    pub fn delete(&self, id: &str) -> std::io::Result<()> {
        if let Some(path) = self.path_for(id) {
            match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            }
        } else {
            Ok(())
        }
    }
}

/// Read+parse every `*.json` in `dir`, skipping unreadable/garbled
/// files rather than failing the whole listing.
fn read_all(dir: &Path) -> Vec<Draft> {
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return out,
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(s) = std::fs::read_to_string(&path) {
            if let Some(d) = Draft::from_json(&s) {
                out.push(d);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("w2q_drafts_test_{}_{}", tag, now_millis()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn save_list_get_delete_roundtrip() {
        let dir = tmpdir("rt");
        let store = DraftStore::open_at(&dir).unwrap();
        assert!(store.list().is_empty());

        let id = store.save(None, "a@b.com", "Hello", "Body text here").unwrap();
        let got = store.get(&id).expect("draft present");
        assert_eq!(got.to, "a@b.com");
        assert_eq!(got.subject, "Hello");
        assert_eq!(got.body, "Body text here");

        let all = store.list();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, id);

        store.delete(&id).unwrap();
        assert!(store.get(&id).is_none());
        assert!(store.list().is_empty());
        // delete is idempotent
        store.delete(&id).unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn overwrite_existing_id_keeps_one_file() {
        let dir = tmpdir("ow");
        let store = DraftStore::open_at(&dir).unwrap();
        let id = store.save(None, "x@y.com", "v1", "first").unwrap();
        let id2 = store.save(Some(&id), "x@y.com", "v2", "second").unwrap();
        assert_eq!(id, id2, "saving with an explicit id reuses it");
        let all = store.list();
        assert_eq!(all.len(), 1, "overwrite must not create a second file");
        assert_eq!(all[0].subject, "v2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_orders_newest_first() {
        let dir = tmpdir("ord");
        let store = DraftStore::open_at(&dir).unwrap();
        // Force distinct updated_at by writing directly.
        let older = Draft { id: "d1".into(), to: "a".into(), subject: "old".into(), body: "".into(), updated_at: 1000 };
        let newer = Draft { id: "d2".into(), to: "b".into(), subject: "new".into(), body: "".into(), updated_at: 2000 };
        std::fs::write(dir.join("d1.json"), older.to_json()).unwrap();
        std::fs::write(dir.join("d2.json"), newer.to_json()).unwrap();
        let all = store.list();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "d2", "newest (higher updated_at) first");
        assert_eq!(all[1].id, "d1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_roundtrip_escapes_specials() {
        let d = Draft {
            id: "d-esc".into(),
            to: "q@e.com".into(),
            subject: "quote \" and \\ slash".into(),
            body: "line1\nline2\ttab".into(),
            updated_at: 42,
        };
        let back = Draft::from_json(&d.to_json()).expect("parse");
        assert_eq!(back, d);
    }

    #[test]
    fn unsafe_ids_rejected() {
        assert!(!id_is_safe(""));
        assert!(!id_is_safe("../escape"));
        assert!(!id_is_safe("a/b"));
        assert!(!id_is_safe("with space"));
        assert!(id_is_safe("d1700000000000-3"));
        // a path-traversal id can't produce a path
        let store = DraftStore::open_at(tmpdir("safe")).unwrap();
        assert!(store.path_for("../../etc/passwd").is_none());
    }

    #[test]
    fn to_row_makes_snippet() {
        let d = Draft {
            id: "d1".into(), to: "a@b.com".into(), subject: "Re: lunch".into(),
            body: "  hey   there    friend  ".into(), updated_at: 1,
        };
        let row = d.to_row();
        assert_eq!(row.subject, "Re: lunch");
        assert_eq!(row.snippet, "hey there friend");
    }

    #[test]
    fn garbled_files_skipped_not_fatal() {
        let dir = tmpdir("garble");
        let store = DraftStore::open_at(&dir).unwrap();
        store.save(None, "ok@x.com", "good", "body").unwrap();
        std::fs::write(dir.join("junk.json"), "this is not json {{{").unwrap();
        std::fs::write(dir.join("notjson.txt"), "ignored").unwrap();
        let all = store.list();
        assert_eq!(all.len(), 1, "garbled/non-json files skipped");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
