//! Call history — append-only JSONL at `$XDG_DATA_HOME/dialer/history.jsonl`.
//!
//! JSONL rather than SQLite (plasma-dialer's choice) on purpose: the write
//! pattern is "append one record when a call ends", the read pattern is "show
//! the most recent N", and a text file keeps the store greppable on-device
//! and dependency-free. If contacts/search ever land, that is the moment to
//! revisit — the store is behind [`HistoryStore`] so only this file changes.
//!
//! A malformed line (partial write on power loss — this is a phone) is
//! skipped, not fatal: losing one record beats losing the list.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::session::Outcome;
use crate::types::CallDirection;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub number: String,
    pub direction: CallDirection,
    /// Unix seconds when the call appeared.
    pub started: u64,
    /// Talk time (active→end); 0 for calls that never connected.
    pub duration_secs: u64,
    pub outcome: Outcome,
}

pub struct HistoryStore {
    path: PathBuf,
    entries: Vec<HistoryEntry>,
}

impl HistoryStore {
    /// Default on-device location.
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("dialer").join("history.jsonl")
    }

    /// Open (creating parents lazily on first append). Load errors other than
    /// "not there yet" are reported so a corrupt store is visible, but an
    /// empty store is returned either way — the dialer must still dial.
    pub fn open(path: &Path) -> (HistoryStore, Option<String>) {
        let mut entries = Vec::new();
        let mut warn = None;
        match fs::read_to_string(path) {
            Ok(text) => {
                for line in text.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<HistoryEntry>(line) {
                        Ok(e) => entries.push(e),
                        Err(_) => warn = Some("history had unreadable lines (skipped)".into()),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn = Some(format!("history unreadable: {e}")),
        }
        (HistoryStore { path: path.to_path_buf(), entries }, warn)
    }

    /// Append one entry to memory and disk. Disk failure is returned as a
    /// message (the shell toasts it) — the in-memory list stays consistent so
    /// the UI still shows the call.
    pub fn append(&mut self, entry: HistoryEntry) -> Result<(), String> {
        self.entries.push(entry.clone());
        let res = (|| -> std::io::Result<()> {
            if let Some(dir) = self.path.parent() {
                fs::create_dir_all(dir)?;
            }
            let mut f = fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
            let line = serde_json::to_string(&entry).expect("HistoryEntry always serializes");
            writeln!(f, "{line}")
        })();
        res.map_err(|e| format!("could not write history: {e}"))
    }

    /// Newest first.
    pub fn recent(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.entries.iter().rev()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dialer-test-{}-{name}.jsonl", std::process::id()))
    }

    #[test]
    fn round_trips_and_survives_a_garbage_line() {
        let path = tmp("roundtrip");
        let _ = fs::remove_file(&path);
        let (mut store, warn) = HistoryStore::open(&path);
        assert!(warn.is_none());
        store
            .append(HistoryEntry {
                number: "+15550001".into(),
                direction: CallDirection::Outgoing,
                started: 1000,
                duration_secs: 42,
                outcome: Outcome::Completed,
            })
            .unwrap();
        // Simulate a torn write.
        {
            let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(f, "{{\"number\": \"+1555").unwrap();
        }
        store
            .append(HistoryEntry {
                number: "+15550002".into(),
                direction: CallDirection::Incoming,
                started: 2000,
                duration_secs: 0,
                outcome: Outcome::Missed,
            })
            .unwrap();

        let (reloaded, warn) = HistoryStore::open(&path);
        assert!(warn.is_some(), "the torn line should be reported");
        let nums: Vec<&str> = reloaded.recent().map(|e| e.number.as_str()).collect();
        assert_eq!(nums, ["+15550002", "+15550001"], "newest first, garbage skipped");
        let _ = fs::remove_file(&path);
    }
}
