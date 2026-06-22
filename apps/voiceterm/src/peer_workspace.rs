//! v0.1.711: peer workspace sharing. The use case: share a notes workspace
//! WITH author metadata preserved; the receiver drops the other person's repo
//! into a subfolder (`peers/<author>/`) and uses git for diffing/syncing once
//! the copy lands. This module owns the decision-shaped pieces:
//!
//!   - AUTHOR stamping: a small frontmatter block recording who/when, added to
//!     shared notes without disturbing the body.
//!   - the SHARE MANIFEST: a list of (relative path, content hash) + author, so
//!     both sides can confirm they have the same snapshot (its hash is what the
//!     handshake carries).
//!   - the PEER LAYOUT + GIT PLAN: where a received workspace lands and the git
//!     commands to init/commit/diff it (returned as a plan, not executed here).
//!
//! PURE + tested. Actual fs copy + `git` invocation live in main.rs.

// ── author stamping ──

/// A `web2qml-author` frontmatter block. Idempotent: stamping an already-
/// stamped note replaces the block rather than nesting.
pub fn stamp_author(note: &str, author: &str, unix_ts: i64) -> String {
    let body = strip_author(note);
    let block = format!(
        "<!-- web2qml-author\nby: {}\nat: {}\n-->\n",
        sanitize_line(author), unix_ts
    );
    format!("{}{}", block, body)
}

/// Remove any leading web2qml-author block, returning the bare body.
pub fn strip_author(note: &str) -> String {
    let marker = "<!-- web2qml-author";
    if let Some(rest) = note.strip_prefix(marker) {
        if let Some(end) = rest.find("-->") {
            // skip past "-->" and a single trailing newline
            let after = &rest[end + 3..];
            return after.strip_prefix('\n').unwrap_or(after).to_string();
        }
    }
    note.to_string()
}

/// Read the author handle from a stamped note, if present.
pub fn author_of(note: &str) -> Option<String> {
    let rest = note.strip_prefix("<!-- web2qml-author")?;
    let end = rest.find("-->")?;
    for line in rest[..end].lines() {
        if let Some(v) = line.trim().strip_prefix("by:") {
            return Some(v.trim().to_string());
        }
    }
    None
}

fn sanitize_line(s: &str) -> String {
    s.chars().filter(|c| *c != '\n' && *c != '\r').collect()
}

// ── content hashing + share manifest ──

/// FNV-1a 64-bit content hash (pure, deterministic, no deps). Good enough to
/// detect "same snapshot?" — not a security primitive.
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManifestEntry {
    pub path: String,   // relative to workspace root
    pub hash: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShareManifest {
    pub author: String,
    pub entries: Vec<ManifestEntry>,
}

impl ShareManifest {
    /// Build a manifest from (relative-path, contents) pairs. Sorted by path so
    /// the overall hash is order-independent.
    pub fn build(author: &str, files: &[(String, Vec<u8>)]) -> ShareManifest {
        let mut entries: Vec<ManifestEntry> = files.iter()
            .map(|(p, c)| ManifestEntry { path: p.clone(), hash: content_hash(c) })
            .collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        ShareManifest { author: author.to_string(), entries }
    }

    /// A single hex digest over the whole manifest — what the handshake's
    /// `workspace_hash` carries so both sides confirm the same snapshot.
    pub fn digest(&self) -> String {
        let mut h: u64 = 0xcbf29ce484222325;
        for e in &self.entries {
            for b in e.path.bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); }
            h ^= e.hash; h = h.wrapping_mul(0x100000001b3);
        }
        format!("{:016x}", h)
    }

    /// Paths that differ (added/changed/removed) between two manifests.
    pub fn diff(&self, other: &ShareManifest) -> ManifestDiff {
        use std::collections::BTreeMap;
        let mine: BTreeMap<&str, u64> = self.entries.iter()
            .map(|e| (e.path.as_str(), e.hash)).collect();
        let theirs: BTreeMap<&str, u64> = other.entries.iter()
            .map(|e| (e.path.as_str(), e.hash)).collect();
        let mut added = Vec::new(); let mut changed = Vec::new(); let mut removed = Vec::new();
        for (p, h) in &theirs {
            match mine.get(p) {
                None => added.push(p.to_string()),
                Some(mh) if mh != h => changed.push(p.to_string()),
                _ => {}
            }
        }
        for p in mine.keys() {
            if !theirs.contains_key(p) { removed.push(p.to_string()); }
        }
        ManifestDiff { added, changed, removed }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ManifestDiff {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

impl ManifestDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

// ── peer layout + git plan ──

/// Where a received peer's workspace lands under the local notes root.
/// `peers/<author-slug>/` — keeps it walled off from your own notes, as its
/// own git repo for diffing/syncing.
pub fn peer_subdir(author: &str) -> String {
    let slug: String = author.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    format!("peers/{}", if slug.is_empty() { "unknown".into() } else { slug })
}

/// The git commands to run after a peer's copy is written into `rel_dir`
/// (relative to notes root). Returned as argv vectors, NOT executed — the
/// caller runs them with the notes root as cwd. Idempotent-ish: init is safe to
/// re-run; commit records the received snapshot for later diffing.
pub fn git_receive_plan(rel_dir: &str, author: &str, snapshot_label: &str) -> Vec<Vec<String>> {
    let s = |parts: &[&str]| parts.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    vec![
        s(&["git", "-C", rel_dir, "init", "-q"]),
        s(&["git", "-C", rel_dir, "add", "-A"]),
        s(&["git", "-C", rel_dir, "commit", "-q",
            "-m", &format!("snapshot from {}: {}", author, snapshot_label),
            "--author", &format!("{} <peer@web2qml>", author),
            "--allow-empty"]),
    ]
}

/// The git command to diff the working tree against the last received snapshot
/// (after the user has edited / a newer copy was dropped in).
pub fn git_diff_plan(rel_dir: &str) -> Vec<String> {
    vec!["git".into(), "-C".into(), rel_dir.into(), "diff".into(), "--stat".into()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_stamp_roundtrip() {
        let note = "# My idea\n\nbody text\n";
        let stamped = stamp_author(note, "alice", 1700000000);
        assert_eq!(author_of(&stamped), Some("alice".into()));
        assert_eq!(strip_author(&stamped), note);
    }

    #[test]
    fn stamp_is_idempotent() {
        let note = "body\n";
        let once = stamp_author(note, "bob", 100);
        let twice = stamp_author(&once, "bob", 200); // re-stamp
        // only one block; body intact
        assert_eq!(strip_author(&twice), note);
        assert_eq!(author_of(&twice), Some("bob".into()));
        assert_eq!(twice.matches("web2qml-author").count(), 1);
    }

    #[test]
    fn unstamped_note_has_no_author() {
        assert_eq!(author_of("plain note"), None);
        assert_eq!(strip_author("plain note"), "plain note");
    }

    #[test]
    fn manifest_digest_order_independent() {
        let a = ShareManifest::build("me", &[
            ("b.md".into(), b"two".to_vec()),
            ("a.md".into(), b"one".to_vec()),
        ]);
        let b = ShareManifest::build("me", &[
            ("a.md".into(), b"one".to_vec()),
            ("b.md".into(), b"two".to_vec()),
        ]);
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn manifest_diff_detects_changes() {
        let mine = ShareManifest::build("me", &[
            ("a.md".into(), b"one".to_vec()),
            ("b.md".into(), b"two".to_vec()),
        ]);
        let theirs = ShareManifest::build("you", &[
            ("a.md".into(), b"one".to_vec()),       // same
            ("b.md".into(), b"TWO-edited".to_vec()),// changed
            ("c.md".into(), b"three".to_vec()),     // added
        ]);
        let d = mine.diff(&theirs);
        assert_eq!(d.added, vec!["c.md"]);
        assert_eq!(d.changed, vec!["b.md"]);
        assert!(d.removed.is_empty());
        assert!(!d.is_empty());
    }

    #[test]
    fn identical_manifests_have_empty_diff() {
        let m = ShareManifest::build("me", &[("a.md".into(), b"x".to_vec())]);
        assert!(m.diff(&m).is_empty());
    }

    #[test]
    fn peer_subdir_slugs_author() {
        assert_eq!(peer_subdir("Alice Cooper"), "peers/alice-cooper");
        assert_eq!(peer_subdir("bob"), "peers/bob");
        assert_eq!(peer_subdir("!!!"), "peers/unknown");
    }

    #[test]
    fn git_plan_shape() {
        let plan = git_receive_plan("peers/alice", "alice", "2026-06-07");
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0], vec!["git", "-C", "peers/alice", "init", "-q"]);
        assert!(plan[2].iter().any(|a| a.contains("snapshot from alice")));
        assert!(plan[2].iter().any(|a| a.contains("alice <peer@web2qml>")));
        let dp = git_diff_plan("peers/alice");
        assert_eq!(dp, vec!["git", "-C", "peers/alice", "diff", "--stat"]);
    }

    #[test]
    fn content_hash_differs_on_change() {
        assert_ne!(content_hash(b"abc"), content_hash(b"abd"));
        assert_eq!(content_hash(b"abc"), content_hash(b"abc"));
    }
}
