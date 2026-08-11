//! snapshot_policy — what a filesystem snapshot captures, from
//! `snapshot_store.rs`.
//!
//! GITLOG-2: the working directory is snapshotted at command-submit time into a
//! content-addressed store, and [`crate::audit_chain`]'s log entry references
//! the resulting tree hash. That is what makes rollback possible — the chain
//! records *that* a command ran, and this records *what the disk looked like*.
//!
//! ## The store layout is git's trick, and the reason is stated
//!
//! `blobs/<sha[0..2]>/<sha[2..]>`. *"Two-character prefix dirs prevent any
//! single directory from growing too large; git uses the same trick."* On a
//! store that accrues one tree per command, a flat directory is a filesystem
//! that gets slower every day you use the feature.
//!
//! ## Bounds, and each is a different failure
//!
//! Snapshotting is `O(files × file_size)` and it runs **per command**, so an
//! unbounded walk turns every `ls` into a disk scan. Four separate limits, and
//! collapsing them into one would let a pathological directory through:
//!
//! * **Skip list** — dependency directories by name. The walker *"bails out
//!   when it would have to recurse into an ignored dir, which makes
//!   node_modules effectively free"*: the cost is skipping the entry, not
//!   walking it and discarding.
//! * **Depth** — a deep tree with few files per level passes a file count and
//!   still costs thousands of `stat`s.
//! * **File count** — a wide flat directory passes a depth limit.
//! * **File size** — one 4 GB core dump passes all three.
//!
//! ## Oversize files are recorded, not omitted
//!
//! A file over the limit gets a `TOOBIG:<size>` placeholder rather than being
//! dropped. That distinction is the whole point: rollback that silently skips a
//! file leaves the user believing the directory was restored. A placeholder
//! lets it say which files it *cannot* restore.

/// Largest file stored in full. Ten megabytes, chosen as *"much bigger than
/// source code, small enough that we don't thrash the disk per command"*.
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// Ceiling on files in one snapshot.
pub const MAX_FILES: usize = 2_000;

/// Ceiling on recursion depth.
pub const MAX_DEPTH: usize = 20;

/// Directory basenames never snapshotted. A conservative subset of a typical
/// `.gitignore` — expanded as real cases appear rather than guessed at.
pub const SKIP_DIRS: &[&str] = &[
    "node_modules", "target", "build", "dist", "__pycache__",
    "venv", ".venv", "env", ".env", ".git",
];

/// Why a path was not captured. Carried rather than discarded, so a rollback
/// can say what it will not restore instead of quietly restoring less.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Skip {
    /// Any path segment begins with a dot.
    Hidden,
    /// A dependency directory by name.
    DependencyDir,
    /// Not followed and not stored — a symlink's target may be outside the
    /// snapshot root entirely, and restoring through one would write somewhere
    /// the user never asked about.
    Symlink,
    /// Deeper than the walker goes.
    TooDeep,
    /// The snapshot already holds its maximum.
    TooMany,
    /// The name is not valid UTF-8; storing it would make the tree's own
    /// encoding ambiguous.
    NonUtf8Name,
}

/// What a snapshot decided about one path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Capture {
    /// Store the contents.
    Full,
    /// Record the path with a `TOOBIG:<size>` placeholder.
    Placeholder(u64),
    Skipped(Skip),
}

impl Capture {
    /// Is the path in the tree at all? Placeholders are — that is their job.
    pub fn in_tree(&self) -> bool { !matches!(self, Capture::Skipped(_)) }

    /// Can rollback restore this path?
    pub fn restorable(&self) -> bool { matches!(self, Capture::Full) }
}

/// Is this a dependency directory?
pub fn is_skip_dir(basename: &str) -> bool {
    SKIP_DIRS.iter().any(|s| s.eq_ignore_ascii_case(basename))
}

/// Does any segment of a relative path start with a dot?
///
/// Any segment, not just the last: `src/.cache/x` is inside a hidden directory
/// even though `x` is not itself hidden, and a walker checking only the leaf
/// would descend into every dotdir it meets.
pub fn is_hidden(rel: &str) -> bool {
    rel.split('/').any(|seg| seg.starts_with('.') && seg != "." && seg != "..")
}

/// A file the walker is considering.
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub rel: &'a str,
    pub size: u64,
    pub depth: usize,
    pub is_symlink: bool,
    pub name_is_utf8: bool,
}

/// Decide what to do with one candidate.
///
/// Order matters and is not arbitrary: cheap structural rejections come before
/// anything that would need the file's metadata, because the point of the skip
/// list is that an ignored directory costs the price of its *name*.
pub fn decide(c: &Candidate, taken_so_far: usize) -> Capture {
    if !c.name_is_utf8 { return Capture::Skipped(Skip::NonUtf8Name); }
    if c.depth > MAX_DEPTH { return Capture::Skipped(Skip::TooDeep); }
    if is_hidden(c.rel) { return Capture::Skipped(Skip::Hidden); }
    if c.rel.split('/').any(is_skip_dir) { return Capture::Skipped(Skip::DependencyDir); }
    if c.is_symlink { return Capture::Skipped(Skip::Symlink); }
    if taken_so_far >= MAX_FILES { return Capture::Skipped(Skip::TooMany); }
    if c.size > MAX_FILE_BYTES { return Capture::Placeholder(c.size); }
    Capture::Full
}

/// Should the walker descend into this directory at all?
///
/// Asked BEFORE recursing, which is what makes an ignored directory cost the
/// price of its name rather than the price of its contents. Checking after
/// would still be correct and would still walk node_modules.
pub fn should_descend(basename: &str, depth: usize) -> bool {
    depth <= MAX_DEPTH && !is_skip_dir(basename) && !basename.starts_with('.')
}

/// The two-character shard a hash lands in.
pub fn shard(sha_hex: &str) -> Option<(&str, &str)> {
    if sha_hex.len() < 3 || !sha_hex.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
    Some((&sha_hex[..2], &sha_hex[2..]))
}

/// The placeholder written for an oversize file.
pub fn placeholder(size: u64) -> String { format!("TOOBIG:{size}") }

/// Read a placeholder back, if that is what a blob is.
pub fn parse_placeholder(blob: &str) -> Option<u64> {
    blob.strip_prefix("TOOBIG:").and_then(|s| s.parse().ok())
}

/// What a snapshot ended up holding.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SnapshotStats {
    pub stored: usize,
    pub placeholders: usize,
    pub skipped: usize,
    pub bytes: u64,
    /// True when a limit stopped the walk — the snapshot is partial and a
    /// rollback from it will not restore everything.
    pub truncated: bool,
}

impl SnapshotStats {
    pub fn add(&mut self, c: &Capture) {
        match c {
            Capture::Full => self.stored += 1,
            Capture::Placeholder(n) => { self.placeholders += 1; self.bytes += n; }
            Capture::Skipped(s) => {
                self.skipped += 1;
                if matches!(s, Skip::TooMany | Skip::TooDeep) { self.truncated = true; }
            }
        }
    }

    /// Is this snapshot a complete picture of the directory?
    ///
    /// Placeholders count against it: a tree that records a path it cannot
    /// restore is not a snapshot you can roll back to.
    pub fn is_complete(&self) -> bool { !self.truncated && self.placeholders == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(rel: &str, size: u64) -> Candidate<'_> {
        Candidate { rel, size, depth: 1, is_symlink: false, name_is_utf8: true }
    }

    #[test]
    fn an_ordinary_source_file_is_stored() {
        assert_eq!(decide(&cand("src/main.rs", 4_000), 0), Capture::Full);
    }

    #[test]
    fn a_dependency_directory_is_skipped_by_name() {
        // "Makes node_modules effectively free" — but only because the check
        // happens before descending.
        assert_eq!(decide(&cand("node_modules/react/index.js", 900), 0),
                   Capture::Skipped(Skip::DependencyDir));
        assert!(!should_descend("node_modules", 1));
        assert!(!should_descend("target", 1));
        assert!(should_descend("src", 1));
    }

    #[test]
    fn descending_is_decided_before_the_walk_not_after() {
        // Checking afterwards is still correct and still walks node_modules.
        // This is the difference between a bounded snapshot and a slow one.
        assert!(!should_descend(".git", 1));
        assert!(!should_descend("__pycache__", 1));
        assert!(!should_descend("anything", MAX_DEPTH + 1), "depth is part of the same question");
    }

    #[test]
    fn any_hidden_segment_hides_the_path_not_just_the_leaf() {
        // `src/.cache/x` is inside a hidden directory even though `x` is not
        // hidden; a walker checking only the leaf descends into every dotdir.
        assert!(is_hidden("src/.cache/data.bin"));
        assert!(is_hidden(".git/config"));
        assert!(!is_hidden("src/main.rs"));
        assert_eq!(decide(&cand("src/.cache/x", 10), 0), Capture::Skipped(Skip::Hidden));
    }

    #[test]
    fn a_symlink_is_not_followed_and_not_stored() {
        // Its target may be outside the snapshot root entirely, and restoring
        // through one would write somewhere the user never asked about.
        let c = Candidate { is_symlink: true, ..cand("src/link", 10) };
        assert_eq!(decide(&c, 0), Capture::Skipped(Skip::Symlink));
    }

    #[test]
    fn an_oversize_file_is_recorded_rather_than_omitted() {
        // The distinction that makes rollback honest: a silently skipped file
        // leaves the user believing the directory was restored.
        let big = MAX_FILE_BYTES + 1;
        let d = decide(&cand("core.dump", big), 0);
        assert_eq!(d, Capture::Placeholder(big));
        assert!(d.in_tree(), "it must appear in the tree");
        assert!(!d.restorable(), "and be known unrestorable");
    }

    #[test]
    fn a_file_exactly_at_the_limit_is_stored_in_full() {
        // The boundary belongs to the stored side, or the documented "10MB"
        // means something slightly smaller than 10MB.
        assert_eq!(decide(&cand("big.bin", MAX_FILE_BYTES), 0), Capture::Full);
    }

    #[test]
    fn the_placeholder_round_trips_its_size() {
        // Rollback needs the size to report what it is not restoring.
        let s = placeholder(12_345_678);
        assert_eq!(parse_placeholder(&s), Some(12_345_678));
        assert_eq!(parse_placeholder("deadbeef"), None, "a real blob is not a placeholder");
        assert_eq!(parse_placeholder("TOOBIG:notanumber"), None);
    }

    #[test]
    fn the_four_limits_catch_four_different_shapes() {
        // Collapsing them into one lets a pathological directory through: a
        // deep tree passes a file count, a wide one passes a depth limit, and
        // one huge file passes both.
        let deep = Candidate { depth: MAX_DEPTH + 1, ..cand("a/b/c", 10) };
        assert_eq!(decide(&deep, 0), Capture::Skipped(Skip::TooDeep));
        assert_eq!(decide(&cand("f.txt", 10), MAX_FILES), Capture::Skipped(Skip::TooMany));
        assert_eq!(decide(&cand("f.bin", MAX_FILE_BYTES + 1), 0),
                   Capture::Placeholder(MAX_FILE_BYTES + 1));
    }

    #[test]
    fn a_non_utf8_name_is_refused_before_anything_else() {
        // Storing it would make the tree's own encoding ambiguous, and the
        // check is first because every later rule reads the path as text.
        let c = Candidate { name_is_utf8: false, ..cand("weird", 10) };
        assert_eq!(decide(&c, 0), Capture::Skipped(Skip::NonUtf8Name));
    }

    #[test]
    fn hashes_shard_two_characters_deep_like_git() {
        // On a store accruing one tree per command, a flat directory is a
        // filesystem that gets slower every day the feature is used.
        assert_eq!(shard("abcdef123456"), Some(("ab", "cdef123456")));
        assert_eq!(shard("ab"), None, "too short to shard");
        assert_eq!(shard("zzz"), None, "not a hash");
    }

    #[test]
    fn a_truncated_snapshot_says_so() {
        // A partial snapshot that reports success is one somebody rolls back
        // to and finds half a directory.
        let mut s = SnapshotStats::default();
        s.add(&Capture::Full);
        assert!(s.is_complete());
        s.add(&Capture::Skipped(Skip::TooMany));
        assert!(s.truncated);
        assert!(!s.is_complete());
    }

    #[test]
    fn a_placeholder_makes_a_snapshot_incomplete_too() {
        // A tree recording a path it cannot restore is not a snapshot you can
        // roll back to, even though nothing was truncated.
        let mut s = SnapshotStats::default();
        s.add(&Capture::Full);
        s.add(&Capture::Placeholder(MAX_FILE_BYTES + 1));
        assert!(!s.truncated);
        assert!(!s.is_complete(), "reported complete while holding an unrestorable path");
    }

    #[test]
    fn an_ordinary_skip_does_not_mark_a_snapshot_truncated() {
        // node_modules being absent is the design working, not the snapshot
        // running out of room — conflating them would report every project as
        // partial.
        let mut s = SnapshotStats::default();
        s.add(&Capture::Skipped(Skip::DependencyDir));
        s.add(&Capture::Skipped(Skip::Hidden));
        s.add(&Capture::Full);
        assert!(!s.truncated);
        assert!(s.is_complete());
    }
}
