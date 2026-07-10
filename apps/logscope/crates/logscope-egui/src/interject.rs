//! interject.rs — the workspace explorer + interjection input path.
//!
//! Instead of only typing into the chat box, you pull up the files connected into the tree (each node's
//! workspace via [`crate::ws_registry`]), open one in a viewing prompt, tap a line region, and speak a
//! small change. Each tap becomes an [`Interjection`] — a targeted `(file, lines, note)` the system
//! turns into an AI command and works through as a queue. It's a side channel into the pipeline, not a
//! chat turn: the model gets a precise edit directive pointed at a span, not a fresh prose message.
//!
//! Explorer data, range normalisation, command generation, and the queue are pure and unit-tested; the
//! viewing-prompt UI (line taps, voice capture) is the on-device egui/voice layer that feeds these.

use crate::ws_registry::WsRegistry;
use std::collections::VecDeque;
use std::path::PathBuf;

/// A file connected into the tree — which node produced it, the dir it lives in, and its relpath.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePointer {
    pub node_uuid: u128,
    pub dir: PathBuf,
    pub rel: String,
}

impl FilePointer {
    /// The full on-disk path to open in the viewing prompt.
    pub fn path(&self) -> PathBuf {
        self.dir.join(&self.rel)
    }
    /// A short label for the explorer row: relpath + a short node id.
    pub fn label(&self) -> String {
        format!("{}  ·  {:04x}", self.rel, self.node_uuid as u16)
    }
}

/// Every file across every tracked node's workspace, sorted (relpath, then node) — the explorer's flat
/// list of "files connected into the tree, with pointers back to their node".
pub fn tree_files(reg: &WsRegistry) -> Vec<FilePointer> {
    let mut v: Vec<FilePointer> = reg
        .iter()
        .flat_map(|e| {
            let (uuid, dir) = (e.uuid, e.dir.clone());
            e.files.iter().map(move |f| FilePointer { node_uuid: uuid, dir: dir.clone(), rel: f.clone() })
        })
        .collect();
    v.sort_by(|a, b| a.rel.cmp(&b.rel).then(a.node_uuid.cmp(&b.node_uuid)));
    v
}

/// One tapped region in a file plus the spoken change for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interjection {
    /// File the tap was in (relpath or [`FilePointer::label`]).
    pub file: String,
    /// Inclusive 1-based line span tapped (`(a, a)` for a single line).
    pub lines: (usize, usize),
    /// The spoken change.
    pub note: String,
}

impl Interjection {
    pub fn new(file: impl Into<String>, lines: (usize, usize), note: impl Into<String>) -> Self {
        let (a, b) = lines;
        Self { file: file.into(), lines: (a.min(b), a.max(b)), note: note.into() }
    }

    /// Turn the interjection into an AI command — the pipe into the pipeline. A targeted edit directive
    /// naming the file and the line span, so the model edits precisely there rather than guessing.
    pub fn to_command(&self) -> String {
        let span = if self.lines.0 == self.lines.1 {
            format!("line {}", self.lines.0)
        } else {
            format!("lines {}–{}", self.lines.0, self.lines.1)
        };
        format!("Edit {} at {}: {}", self.file, span, self.note.trim())
    }

    /// A tap with no spoken change yet is not actionable (so the queue ignores it).
    pub fn is_actionable(&self) -> bool {
        !self.note.trim().is_empty()
    }
}

/// A queue of interjections the system works through in order — new taps append, the runner pops. This
/// is what makes interjection a *pipe*: several small spoken edits stack up and drain one at a time.
#[derive(Default)]
pub struct InterjectionQueue {
    items: VecDeque<Interjection>,
}

impl InterjectionQueue {
    pub fn new() -> Self {
        Self::default()
    }
    /// Append an interjection (ignored if it carries no spoken change).
    pub fn push(&mut self, it: Interjection) {
        if it.is_actionable() {
            self.items.push_back(it);
        }
    }
    /// Take the next interjection to run.
    pub fn pop(&mut self) -> Option<Interjection> {
        self.items.pop_front()
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// The pending commands in order — a preview of what the interjection pipe will run.
    pub fn preview(&self) -> Vec<String> {
        self.items.iter().map(|i| i.to_command()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_files_flattens_and_sorts_pointers() {
        let mut reg = WsRegistry::new();
        reg.register(0xAA, PathBuf::from("/tmp/a"), vec!["src/main.rs".into(), "Cargo.toml".into()], 100, 1);
        reg.register(0xBB, PathBuf::from("/tmp/b"), vec!["src/main.rs".into()], 50, 2);
        let files = tree_files(&reg);
        assert_eq!(files.len(), 3);
        // sorted by relpath: Cargo.toml first, then the two src/main.rs (by node)
        assert_eq!(files[0].rel, "Cargo.toml");
        assert_eq!(files[1].rel, "src/main.rs");
        assert_eq!(files[1].path(), PathBuf::from("/tmp/a/src/main.rs"));
        assert!(files[1].node_uuid != files[2].node_uuid); // both src/main.rs, different nodes
    }

    #[test]
    fn interjection_normalises_range_and_builds_a_targeted_command() {
        let one = Interjection::new("src/main.rs", (12, 12), "  rename this to run()  ");
        assert_eq!(one.to_command(), "Edit src/main.rs at line 12: rename this to run()");
        let span = Interjection::new("lib.rs", (40, 20), "extract into a helper"); // reversed range
        assert_eq!(span.lines, (20, 40)); // normalised
        assert_eq!(span.to_command(), "Edit lib.rs at lines 20–40: extract into a helper");
    }

    #[test]
    fn queue_drains_in_order_and_skips_empty_taps() {
        let mut q = InterjectionQueue::new();
        q.push(Interjection::new("a.rs", (1, 1), "add a doc comment"));
        q.push(Interjection::new("a.rs", (5, 5), "   ")); // empty note → ignored
        q.push(Interjection::new("b.rs", (9, 10), "guard against None"));
        assert_eq!(q.len(), 2);
        assert_eq!(q.preview()[0], "Edit a.rs at line 1: add a doc comment");
        assert_eq!(q.pop().unwrap().file, "a.rs");
        assert_eq!(q.pop().unwrap().file, "b.rs"); // FIFO
        assert!(q.is_empty());
    }
}
