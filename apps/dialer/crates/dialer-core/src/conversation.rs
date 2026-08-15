//! Conversations — the messaging model. Threads keyed by peer, each an
//! ordered message list; unread tracking; newest-thread-first ordering. Pure
//! and unit-tested. Persistence is the same append-only JSONL as call
//! history (one file, greppable on-device), reloaded on launch.
//!
//! A "peer" is a transport-qualified id so an SMS thread with +1555… and a
//! Matrix thread with @a:x never merge: `tel:+15551234`, `matrix:@a:x.org`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::message::Payload;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dir {
    In,
    Out,
}

/// One message. Flat so it JSONL-serialises one-per-line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub peer: String,
    pub dir: Dir,
    pub payload: Payload,
    /// Unix seconds.
    pub ts: u64,
    /// Whether the local user has seen it (outbound is always true).
    #[serde(default)]
    pub read: bool,
}

/// A thread: the peer and its messages, oldest-first.
#[derive(Debug, Clone, Default)]
pub struct Thread {
    pub peer: String,
    pub messages: Vec<Message>,
}

impl Thread {
    pub fn last(&self) -> Option<&Message> {
        self.messages.last()
    }
    pub fn unread(&self) -> usize {
        self.messages.iter().filter(|m| m.dir == Dir::In && !m.read).count()
    }
    pub fn last_ts(&self) -> u64 {
        self.last().map(|m| m.ts).unwrap_or(0)
    }
}

/// All threads plus persistence.
pub struct Conversations {
    path: PathBuf,
    threads: HashMap<String, Thread>,
}

impl Conversations {
    pub fn default_path() -> PathBuf {
        crate::history::HistoryStore::default_path()
            .parent()
            .map(|d| d.join("messages.jsonl"))
            .unwrap_or_else(|| PathBuf::from("messages.jsonl"))
    }

    /// Load, skipping torn lines (same power-loss tolerance as history).
    pub fn open(path: &Path) -> (Conversations, Option<String>) {
        let mut threads: HashMap<String, Thread> = HashMap::new();
        let mut warn = None;
        match fs::read_to_string(path) {
            Ok(text) => {
                for line in text.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Message>(line) {
                        Ok(m) => threads
                            .entry(m.peer.clone())
                            .or_insert_with(|| Thread { peer: m.peer.clone(), messages: Vec::new() })
                            .messages
                            .push(m),
                        Err(_) => warn = Some("messages had unreadable lines (skipped)".into()),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn = Some(format!("messages unreadable: {e}")),
        }
        // Keep each thread ordered even if the file wasn't.
        for t in threads.values_mut() {
            t.messages.sort_by_key(|m| m.ts);
        }
        (Conversations { path: path.to_path_buf(), threads }, warn)
    }

    /// Append a message to memory and disk. Disk failure returns a message;
    /// memory stays consistent so the UI still shows it.
    pub fn append(&mut self, m: Message) -> Result<(), String> {
        self.threads
            .entry(m.peer.clone())
            .or_insert_with(|| Thread { peer: m.peer.clone(), messages: Vec::new() })
            .messages
            .push(m.clone());
        let res = (|| -> std::io::Result<()> {
            if let Some(dir) = self.path.parent() {
                fs::create_dir_all(dir)?;
            }
            let mut f = fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
            writeln!(f, "{}", serde_json::to_string(&m).expect("Message serializes"))
        })();
        res.map_err(|e| format!("could not write message: {e}"))
    }

    /// Threads, most-recently-active first.
    pub fn threads_by_recency(&self) -> Vec<&Thread> {
        let mut v: Vec<&Thread> = self.threads.values().collect();
        v.sort_by(|a, b| b.last_ts().cmp(&a.last_ts()));
        v
    }

    pub fn thread(&self, peer: &str) -> Option<&Thread> {
        self.threads.get(peer)
    }

    pub fn total_unread(&self) -> usize {
        self.threads.values().map(|t| t.unread()).sum()
    }

    /// Mark a thread's inbound messages read (memory only — the read flag is
    /// re-derivable and not worth a rewrite of the append-only file; on next
    /// launch a thread reopens with its persisted flags, which is acceptable:
    /// unread-after-restart is a minor annoyance, a rewritten log is a risk).
    pub fn mark_read(&mut self, peer: &str) {
        if let Some(t) = self.threads.get_mut(peer) {
            for m in t.messages.iter_mut() {
                if m.dir == Dir::In {
                    m.read = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dialer-conv-{}-{name}.jsonl", std::process::id()))
    }

    fn msg(peer: &str, dir: Dir, body: &str, ts: u64) -> Message {
        Message { peer: peer.into(), dir, payload: Payload::text(body), ts, read: dir == Dir::Out }
    }

    #[test]
    fn threads_split_by_peer_and_sort_by_recency() {
        let path = tmp("recency");
        let _ = fs::remove_file(&path);
        let (mut c, _) = Conversations::open(&path);
        c.append(msg("tel:+1", Dir::In, "hi", 100)).unwrap();
        c.append(msg("matrix:@a:x", Dir::In, "yo", 200)).unwrap();
        c.append(msg("tel:+1", Dir::Out, "hello", 300)).unwrap();

        let order: Vec<&str> = c.threads_by_recency().iter().map(|t| t.peer.as_str()).collect();
        assert_eq!(order, ["tel:+1", "matrix:@a:x"], "most recent activity first");
        assert_eq!(c.thread("tel:+1").unwrap().messages.len(), 2);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn unread_counts_only_inbound_and_clears_on_read() {
        let path = tmp("unread");
        let _ = fs::remove_file(&path);
        let (mut c, _) = Conversations::open(&path);
        c.append(msg("tel:+1", Dir::In, "a", 1)).unwrap();
        c.append(msg("tel:+1", Dir::In, "b", 2)).unwrap();
        c.append(msg("tel:+1", Dir::Out, "c", 3)).unwrap(); // outbound never unread
        assert_eq!(c.total_unread(), 2);
        c.mark_read("tel:+1");
        assert_eq!(c.total_unread(), 0);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn reload_reconstructs_ordered_threads_and_skips_garbage() {
        let path = tmp("reload");
        let _ = fs::remove_file(&path);
        {
            let (mut c, _) = Conversations::open(&path);
            c.append(msg("tel:+9", Dir::In, "first", 10)).unwrap();
            c.append(msg("tel:+9", Dir::In, "second", 20)).unwrap();
        }
        {
            let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(f, "{{ broken").unwrap();
        }
        let (c, warn) = Conversations::open(&path);
        assert!(warn.is_some());
        let bodies: Vec<String> = c
            .thread("tel:+9")
            .unwrap()
            .messages
            .iter()
            .map(|m| m.payload.preview())
            .collect();
        assert_eq!(bodies, ["first", "second"]);
        let _ = fs::remove_file(&path);
    }
}
