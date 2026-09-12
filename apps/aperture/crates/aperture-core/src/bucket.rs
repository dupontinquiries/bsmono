//! bucket — an ordered, named pack of places in the codebase, with an
//! attention horizon.
//!
//! The ask arrived twice, from two directions, and they are the same thing:
//! a `// TODO root workspace concept has to adapt to learn to take file
//! packs` in `state.rs`, and a design conversation's "buckets" — temporary
//! ordered collections of files and AST items that you work out of instead
//! of navigating a directory tree. This module is that shared answer.
//!
//! ## What a bucket is, and what it deliberately is not
//!
//! It is a `Vec<Entry>` with a name and a depth limit. It is NOT a tab
//! group, NOT a saved search, and NOT a second filesystem: it holds
//! REFERENCES to code, it never owns text, and removing an entry cannot
//! lose work. That is what makes it safe to treat as scratch — the whole
//! point is that a person makes one for an afternoon's investigation and
//! throws it away.
//!
//! ## Why an [`Entry`] is a path plus an optional symbol, and not an id
//!
//! The natural thing is to reference an entity id from a symbol index.
//! There isn't one yet, and building buckets on top of a layer that does
//! not exist means neither ships. So an entry carries the two things that
//! are true today and stay true after an entity layer lands: WHERE the
//! file is, and WHAT it is called inside that file. When the index
//! arrives, `Entry` gains an `id` field and `path`/`symbol` become its
//! fallback resolution — additive, no format break (see [`parse`]'s
//! unknown-key rule).
//!
//! Keying on a NAME rather than a line number is the load-bearing part: a
//! bucket entry has to survive the function moving in its file, which is
//! the one thing a line number cannot do. [`locate`] does that resolution
//! against whatever outline the caller already has.
//!
//! **An entry that no longer resolves is shown as unresolved, never
//! silently dropped.** Same rule the room renderer already follows for
//! blame data it does not have: absence of signal is not evidence.
//!
//! ## The format
//!
//! `key<TAB>value`, one per line; an `entry` key opens a record and every
//! key after it attaches to that record. Unknown keys are ignored, which
//! is what makes adding a field later a non-event. Chosen over a real
//! serializer because this crate has exactly one dependency on purpose,
//! and over the `\u{1f}`-separated shape `gitspace` uses for git output
//! because this text is stored, read by people through `git cat-file`,
//! and diffed — one field per line diffs cleanly, a record per line does
//! not.
//!
//! Parsing is TOTAL: any input either yields a bucket or `None`, never a
//! panic. It is read back from a git object written by a possibly older
//! version of this program, so a malformed line is an ordinary case.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// One place in the codebase. `path` is repo-relative when there is a
/// repo (the same string `gitspace::discover` hands back), absolute
/// otherwise.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    /// The item's name inside the file — `None` means the entry is the
    /// whole file, which is a legitimate and common case ("this bucket is
    /// these six files").
    pub symbol: Option<String>,
    /// The outline kind (`fn`, `struct`, `impl`, …) when it is known.
    /// Used only to disambiguate a name that appears twice in a file; a
    /// missing kind matches anything.
    pub kind: Option<String>,
    /// Why this is here, in the person's own words. The one field the
    /// machine never writes.
    pub note: Option<String>,
    /// Pinned entries survive [`Bucket::sweep`]. Everything else in a
    /// bucket is assumed disposable.
    pub pinned: bool,
}

impl Entry {
    pub fn file(path: impl Into<String>) -> Entry {
        Entry { path: path.into(), ..Default::default() }
    }

    pub fn symbol(path: impl Into<String>, symbol: impl Into<String>) -> Entry {
        Entry { path: path.into(), symbol: Some(symbol.into()), ..Default::default() }
    }

    /// Two entries are the same PLACE when the path and symbol agree.
    /// Kind, note and pin are attributes of the entry, not part of its
    /// identity — re-adding a place you already have should update it,
    /// not duplicate it.
    pub fn same_place(&self, other: &Entry) -> bool {
        self.path == other.path && self.symbol == other.symbol
    }
}

/// Where an entry currently lives, if anywhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Located {
    /// Resolved to a line in the file (0-based, the outline's own convention).
    Line(usize),
    /// A whole-file entry: the file is the destination, line 0.
    FileOnly,
    /// The file is there; the symbol is not. Shown, not dropped — it may
    /// be a rename, and the person is the one who can tell.
    Unresolved,
}

/// Resolve an entry against an outline of its file, as
/// `(kind, name, line)` triples — deliberately not `syntax::OutlineItem`,
/// which only exists under the `tree-sitter` feature. Buckets work in a
/// lexer-only build; they just resolve fewer symbols.
///
/// A missing `kind` on the entry matches any kind. First match wins,
/// which matters for a name that appears as both a `struct` and its
/// `impl`: record the kind when adding and it will not be ambiguous.
pub fn locate(entry: &Entry, outline: &[(&str, &str, usize)]) -> Located {
    let Some(sym) = entry.symbol.as_deref() else { return Located::FileOnly };
    for (kind, name, line) in outline {
        if *name == sym && entry.kind.as_deref().is_none_or_eq(kind) {
            return Located::Line(*line);
        }
    }
    Located::Unresolved
}

/// `Option<&str>` == `&str`, with `None` meaning "matches anything".
/// A free function would read worse at the call site than this does.
trait NoneOrEq {
    fn is_none_or_eq(&self, other: &str) -> bool;
}
impl NoneOrEq for Option<&str> {
    fn is_none_or_eq(&self, other: &str) -> bool {
        match self {
            None => true,
            Some(s) => *s == other,
        }
    }
}

/// The depth horizon a bucket has when it does not say otherwise.
///
/// Two hops: from a focused item you see what it calls and what those
/// call. Three is where a call graph stops being a neighbourhood and
/// starts being the program.
pub const DEFAULT_DEPTH: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bucket {
    /// What the person called it. Free text; [`slug`] derives the storage
    /// key from it.
    pub name: String,
    /// How far a consumer may expand relationships from these entries.
    /// `None` = no limit, and it is a real choice a person can make, so
    /// it is an `Option` rather than a sentinel like `u32::MAX`.
    pub depth: Option<u32>,
    /// Order is the person's, and it is meaningful — a bucket is a
    /// reading order as much as a set. Nothing in this module sorts.
    pub entries: Vec<Entry>,
}

impl Bucket {
    pub fn new(name: impl Into<String>) -> Bucket {
        Bucket { name: name.into(), depth: Some(DEFAULT_DEPTH), entries: Vec::new() }
    }

    /// The storage key. See [`slug`].
    pub fn slug(&self) -> String {
        slug(&self.name)
    }

    /// Add, or update in place if the place is already here. Returns the
    /// index either way, so a caller can scroll to it.
    ///
    /// Update-in-place rather than reject: the second add usually carries
    /// something the first did not (a note, a kind), and a picker that
    /// silently does nothing on a re-add reads as broken.
    pub fn add(&mut self, e: Entry) -> usize {
        if let Some(i) = self.entries.iter().position(|x| x.same_place(&e)) {
            let pinned = self.entries[i].pinned || e.pinned;
            let note = e.note.clone().or_else(|| self.entries[i].note.clone());
            self.entries[i] = Entry { pinned, note, ..e };
            return i;
        }
        self.entries.push(e);
        self.entries.len() - 1
    }

    pub fn remove(&mut self, i: usize) -> Option<Entry> {
        (i < self.entries.len()).then(|| self.entries.remove(i))
    }

    /// Move an entry one step, clamped. Returns its new index.
    pub fn shift(&mut self, i: usize, delta: isize) -> usize {
        if self.entries.is_empty() || i >= self.entries.len() {
            return i;
        }
        let target = (i as isize + delta).clamp(0, self.entries.len() as isize - 1) as usize;
        let e = self.entries.remove(i);
        self.entries.insert(target, e);
        target
    }

    /// Drop everything unpinned — "I am done with this investigation but
    /// keep the three things that mattered."
    pub fn sweep(&mut self) {
        self.entries.retain(|e| e.pinned);
    }

    pub fn contains(&self, e: &Entry) -> bool {
        self.entries.iter().any(|x| x.same_place(e))
    }

    /// Serialize. Round-trips through [`parse`] for every value this
    /// module can construct, including notes containing tabs and
    /// newlines.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        s.push_str("bucket\t1\n");
        s.push_str(&format!("name\t{}\n", esc(&self.name)));
        if let Some(d) = self.depth {
            s.push_str(&format!("depth\t{d}\n"));
        }
        for e in &self.entries {
            s.push_str(&format!("entry\t{}\n", esc(&e.path)));
            if let Some(v) = &e.symbol {
                s.push_str(&format!("symbol\t{}\n", esc(v)));
            }
            if let Some(v) = &e.kind {
                s.push_str(&format!("kind\t{}\n", esc(v)));
            }
            if let Some(v) = &e.note {
                s.push_str(&format!("note\t{}\n", esc(v)));
            }
            if e.pinned {
                s.push_str("pin\t1\n");
            }
        }
        s
    }
}

/// Read a bucket back. `None` only for text that is not a bucket at all
/// (no `bucket` header); anything past that is best-effort, because the
/// alternative is losing a person's whole bucket to one bad line written
/// by a version of this program that is not this one.
///
/// Unknown keys are skipped. That is the forward-compatibility rule: a
/// newer aperture can add `id` or `created` and an older one still opens
/// the bucket, minus the field it does not know.
pub fn parse(text: &str) -> Option<Bucket> {
    let mut lines = text.lines();
    let header = lines.next()?;
    let (k, v) = split(header)?;
    if k != "bucket" || v != "1" {
        return None;
    }
    let mut b = Bucket { name: String::new(), depth: None, entries: Vec::new() };
    for line in lines {
        let Some((k, v)) = split(line) else { continue };
        match k {
            "name" => b.name = unesc(v),
            "depth" => b.depth = v.parse().ok(),
            "entry" => b.entries.push(Entry::file(unesc(v))),
            "symbol" | "kind" | "note" | "pin" => {
                let Some(e) = b.entries.last_mut() else { continue };
                match k {
                    "symbol" => e.symbol = Some(unesc(v)),
                    "kind" => e.kind = Some(unesc(v)),
                    "note" => e.note = Some(unesc(v)),
                    _ => e.pinned = v == "1",
                }
            }
            _ => {}
        }
    }
    Some(b)
}

fn split(line: &str) -> Option<(&str, &str)> {
    let mut it = line.splitn(2, '\t');
    let k = it.next()?;
    let v = it.next().unwrap_or("");
    (!k.is_empty()).then_some((k, v))
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            // A lone trailing backslash, or an escape this version does
            // not know: keep the backslash rather than eat the character
            // after it. Losing text is worse than showing an odd one.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// A storage key from a display name: lowercase alphanumerics and `-`,
/// no runs, no leading or trailing separator.
///
/// The output is a git ref component, which refuses more than it looks
/// like it would: a leading `-`, a trailing `.lock`, `..`, `~^:?*[`,
/// whitespace, control characters, and an empty component. Emitting only
/// alphanumerics and `-` avoids the whole class rather than enumerating
/// it.
///
/// **Alphanumeric here is Unicode's, not ASCII's**, and that is a
/// deliberate second pass over this function. The ASCII-only version
/// turned a Japanese or Greek bucket name into `unnamed` and a name like
/// `Ünïcode 42` into `n-code-42` — silently, and worst for exactly the
/// people least able to work around it. Git takes non-ASCII ref
/// components fine (checked against a real repo: written, listed by
/// `for-each-ref`, read back by name), and none of the characters git
/// actually refuses are alphanumeric in any script. Case folding uses
/// `to_lowercase`, so it can widen a character (`İ` → two chars) — which
/// is correct and harmless here, since the only requirements on a slug
/// are that it is stable and ref-legal.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let s = out.trim_matches('-').to_string();
    if s.is_empty() {
        "unnamed".to_string()
    } else {
        s
    }
}

/// Where buckets are kept. Two implementations exist: git refs
/// (`gitspace::GitStore`, the real one) and [`FileStore`] for a workspace
/// that is not a repo.
///
/// A trait rather than one function with a branch, because the git side
/// needs a `Runner` and the file side needs a directory, and threading
/// both through every call site to satisfy one `if` is how a seam that
/// should be two lines becomes an argument list.
pub trait Store {
    /// Slugs of every bucket present, in whatever order the backing store
    /// gives — callers sort by name for display.
    fn list(&self) -> Vec<String>;
    fn load(&self, slug: &str) -> Option<String>;
    fn save(&self, slug: &str, text: &str) -> Result<(), String>;
    fn delete(&self, slug: &str) -> Result<(), String>;
}

/// Buckets as files under a directory, for a workspace with no repo.
///
/// Deliberately the FALLBACK and not the default: a git ref costs no
/// gitignore entry, no backup story and no "what is this directory" from
/// the next person to clone, which is the same reasoning that put
/// `refs/aperture/visited/*` where it is.
pub struct FileStore {
    pub dir: PathBuf,
}

impl FileStore {
    pub fn new(dir: impl Into<PathBuf>) -> FileStore {
        FileStore { dir: dir.into() }
    }
    fn path(&self, slug: &str) -> PathBuf {
        self.dir.join(format!("{slug}.bucket"))
    }
}

impl Store for FileStore {
    fn list(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        rd.filter_map(|e| {
            let p = e.ok()?.path();
            (p.extension()? == "bucket").then(|| p.file_stem()?.to_str().map(str::to_string))?
        })
        .collect()
    }
    fn load(&self, slug: &str) -> Option<String> {
        std::fs::read_to_string(self.path(slug)).ok()
    }
    fn save(&self, slug: &str, text: &str) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        std::fs::write(self.path(slug), text).map_err(|e| e.to_string())
    }
    fn delete(&self, slug: &str) -> Result<(), String> {
        match std::fs::remove_file(self.path(slug)) {
            Ok(()) => Ok(()),
            // Deleting what is not there is the outcome the caller wanted.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// The workspace: a root, the buckets in it, and which one is active.
///
/// This is what `AppState::workspace_root()` grows into. The root does
/// not go away — a bucket is an ADDITIONAL way to reach code, never a
/// replacement for the tree, and the file picker still walks the root.
/// What changes is that the walk is one source of entries rather than the
/// definition of what you are working on.
#[derive(Clone, Debug)]
pub struct Workspace {
    pub root: PathBuf,
    pub buckets: Vec<Bucket>,
    pub active: Option<usize>,
}

impl Workspace {
    pub fn new(root: impl Into<PathBuf>) -> Workspace {
        Workspace { root: root.into(), buckets: Vec::new(), active: None }
    }

    /// Load every bucket the store has, sorted by name so the picker's
    /// order does not depend on the filesystem's or git's.
    ///
    /// A bucket whose text does not parse is SKIPPED, not fatal: one
    /// corrupt object should cost one bucket, not the workspace. The
    /// count of skipped ones is returned so a caller can say so instead
    /// of silently showing fewer buckets than exist.
    pub fn load(root: impl Into<PathBuf>, store: &dyn Store) -> (Workspace, usize) {
        let mut w = Workspace::new(root);
        let mut skipped = 0;
        let mut slugs = store.list();
        slugs.sort();
        for s in slugs {
            match store.load(&s).as_deref().and_then(parse) {
                Some(b) => w.buckets.push(b),
                None => skipped += 1,
            }
        }
        w.buckets.sort_by(|a, b| a.name.cmp(&b.name));
        (w, skipped)
    }

    pub fn active(&self) -> Option<&Bucket> {
        self.buckets.get(self.active?)
    }

    pub fn active_mut(&mut self) -> Option<&mut Bucket> {
        self.buckets.get_mut(self.active?)
    }

    /// Create (or find) a bucket by name and make it active.
    ///
    /// Finds by SLUG, not by display name: two names that differ only in
    /// punctuation share a storage key, and letting both exist in memory
    /// would mean the second silently overwrites the first on save.
    pub fn open(&mut self, name: &str) -> usize {
        let want = slug(name);
        let i = match self.buckets.iter().position(|b| b.slug() == want) {
            Some(i) => i,
            None => {
                self.buckets.push(Bucket::new(name));
                self.buckets.len() - 1
            }
        };
        self.active = Some(i);
        i
    }

    /// Remove a bucket from memory AND from the store.
    pub fn close(&mut self, i: usize, store: &dyn Store) -> Result<(), String> {
        if i >= self.buckets.len() {
            return Ok(());
        }
        let b = self.buckets.remove(i);
        self.active = match self.active {
            Some(a) if a == i => None,
            Some(a) if a > i => Some(a - 1),
            other => other,
        };
        store.delete(&b.slug())
    }

    pub fn save(&self, i: usize, store: &dyn Store) -> Result<(), String> {
        let b = self.buckets.get(i).ok_or("no such bucket")?;
        store.save(&b.slug(), &b.to_text())
    }

    pub fn save_all(&self, store: &dyn Store) -> Result<(), String> {
        for i in 0..self.buckets.len() {
            self.save(i, store)?;
        }
        Ok(())
    }

    /// Every distinct file any bucket mentions — what a "show me only the
    /// files I am working on" filter runs off.
    pub fn all_paths(&self) -> BTreeSet<&str> {
        self.buckets.iter().flat_map(|b| b.entries.iter().map(|e| e.path.as_str())).collect()
    }

    /// Make a path storable: repo-relative to `root` when it is inside,
    /// otherwise the absolute path unchanged. Buckets in a repo should
    /// not carry `/home/somebody/` in them.
    pub fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().replace('\\', "/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_including_a_note_with_a_tab_and_a_newline() {
        let mut b = Bucket::new("Collision investigation");
        b.depth = Some(3);
        b.add(Entry::symbol("src/physics.rs", "resolve_collision"));
        b.add(Entry {
            path: "src/player.rs".into(),
            symbol: Some("update".into()),
            kind: Some("fn".into()),
            note: Some("mutates velocity\there\nand here".into()),
            pinned: true,
        });
        b.add(Entry::file("src/lib.rs"));

        let back = parse(&b.to_text()).expect("parses");
        assert_eq!(back, b, "every field survives the round trip");
        // And the serialized form is one field per line, which is the
        // reason for this format rather than a packed one.
        assert!(b.to_text().lines().count() >= 8, "{}", b.to_text());
    }

    #[test]
    fn parse_is_total_and_forward_compatible() {
        assert!(parse("").is_none(), "empty is not a bucket");
        assert!(parse("name\tx\n").is_none(), "no header, not a bucket");
        assert!(parse("bucket\t2\nname\tx\n").is_none(), "a future version is refused, not guessed at");

        // A key this version does not know, a key with no value, and a
        // detail line before any entry: all skipped, nothing lost.
        let b = parse("bucket\t1\nname\tX\ncreated\t2026\nsymbol\torphan\nentry\ta.rs\nid\tQ7\nnote\thi\n")
            .expect("still parses");
        assert_eq!(b.name, "X");
        assert_eq!(b.entries.len(), 1);
        assert_eq!(b.entries[0].note.as_deref(), Some("hi"));
        assert_eq!(b.entries[0].symbol, None, "the orphan detail line attached to nothing");
    }

    #[test]
    fn adding_the_same_place_updates_rather_than_duplicates() {
        let mut b = Bucket::new("b");
        b.add(Entry::symbol("a.rs", "foo"));
        let i = b.add(Entry {
            note: Some("why".into()),
            pinned: true,
            ..Entry::symbol("a.rs", "foo")
        });
        assert_eq!(i, 0);
        assert_eq!(b.entries.len(), 1, "same path+symbol is the same place");
        assert_eq!(b.entries[0].note.as_deref(), Some("why"));

        // A DIFFERENT symbol in the same file is a different place, and
        // the whole-file entry is a third.
        b.add(Entry::symbol("a.rs", "bar"));
        b.add(Entry::file("a.rs"));
        assert_eq!(b.entries.len(), 3);
    }

    #[test]
    fn a_re_add_cannot_silently_unpin_or_lose_a_note() {
        let mut b = Bucket::new("b");
        b.add(Entry { pinned: true, note: Some("keep".into()), ..Entry::symbol("a.rs", "foo") });
        b.add(Entry::symbol("a.rs", "foo")); // a plain re-add from a picker
        assert!(b.entries[0].pinned, "pin is sticky");
        assert_eq!(b.entries[0].note.as_deref(), Some("keep"), "the note survives");
    }

    #[test]
    fn shift_moves_and_clamps_at_both_ends() {
        let mut b = Bucket::new("b");
        for n in ["a", "b", "c"] {
            b.add(Entry::file(n));
        }
        assert_eq!(b.shift(2, -1), 1);
        assert_eq!(paths(&b), ["a", "c", "b"]);
        assert_eq!(b.shift(0, -5), 0, "clamps at the top");
        assert_eq!(b.shift(0, 99), 2, "clamps at the bottom");
        assert_eq!(paths(&b), ["c", "b", "a"]);
        assert_eq!(b.shift(9, 1), 9, "an index past the end is a no-op, not a panic");
    }

    #[test]
    fn sweep_keeps_exactly_the_pinned_ones() {
        let mut b = Bucket::new("b");
        b.add(Entry::file("a"));
        b.add(Entry { pinned: true, ..Entry::file("b") });
        b.add(Entry::file("c"));
        b.sweep();
        assert_eq!(paths(&b), ["b"]);
    }

    #[test]
    fn locate_finds_by_name_survives_a_move_and_reports_unresolved() {
        let e = Entry::symbol("a.rs", "resolve_collision");
        let early = [("fn", "resolve_collision", 12usize), ("fn", "other", 40)];
        let moved = [("fn", "other", 3usize), ("fn", "resolve_collision", 300)];
        assert_eq!(locate(&e, &early), Located::Line(12));
        assert_eq!(locate(&e, &moved), Located::Line(300), "the point: a line number could not do this");
        assert_eq!(locate(&e, &[("fn", "other", 1)]), Located::Unresolved, "renamed away, not dropped");
        assert_eq!(locate(&Entry::file("a.rs"), &[]), Located::FileOnly);
    }

    #[test]
    fn locate_uses_kind_only_to_disambiguate() {
        let outline = [("struct", "Thing", 10usize), ("impl", "Thing", 30)];
        let any = Entry::symbol("a.rs", "Thing");
        assert_eq!(locate(&any, &outline), Located::Line(10), "no kind: first match");
        let imp = Entry { kind: Some("impl".into()), ..Entry::symbol("a.rs", "Thing") };
        assert_eq!(locate(&imp, &outline), Located::Line(30));
        let gone = Entry { kind: Some("trait".into()), ..Entry::symbol("a.rs", "Thing") };
        assert_eq!(locate(&gone, &outline), Located::Unresolved);
    }

    #[test]
    fn slugs_are_ref_safe_and_stable() {
        assert_eq!(slug("Collision investigation"), "collision-investigation");
        assert_eq!(slug("  ../weird//name.lock  "), "weird-name-lock");
        assert_eq!(slug("-leading and trailing-"), "leading-and-trailing");
        assert_eq!(slug("!!!"), "unnamed", "never empty — an empty ref component is refused by git");
        // Non-Latin names keep their letters instead of collapsing. The
        // first version of this function was ASCII-only and turned both
        // of these into rubbish; git accepts them, verified against a
        // real repo in `gitspace`'s hermetic test.
        assert_eq!(slug("Ünïcode 42"), "ünïcode-42");
        assert_eq!(slug("衝突 の調査"), "衝突-の調査");
        for s in [slug("../x"), slug("-x"), slug("x.lock"), slug(""), slug("a~b^c:d?e*f[g"), slug("tab\there")] {
            assert!(!s.is_empty(), "an empty ref component is refused by git");
            assert!(!s.starts_with('-') && !s.ends_with('-'));
            assert!(!s.contains("--") && !s.contains(".."));
            assert!(
                s.chars().all(|c| c.is_alphanumeric() || c == '-'),
                "only alphanumerics and '-' reach a ref name: {s:?}"
            );
        }
    }

    #[test]
    fn workspace_round_trips_through_a_file_store() {
        let dir = std::env::temp_dir().join(format!("aperture-bucket-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = FileStore::new(&dir);

        let mut w = Workspace::new("/tmp/root");
        w.open("Rendering");
        w.active_mut().unwrap().add(Entry::symbol("src/draw.rs", "draw"));
        let i = w.open("Collision");
        w.active_mut().unwrap().add(Entry::file("src/physics.rs"));
        w.save_all(&store).unwrap();
        assert_eq!(i, 1);

        let (back, skipped) = Workspace::load("/tmp/root", &store);
        assert_eq!(skipped, 0);
        let names: Vec<&str> = back.buckets.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["Collision", "Rendering"], "sorted by name, not by store order");
        assert_eq!(back.all_paths().len(), 2);

        // A corrupt object costs one bucket, not the workspace.
        store.save("junk", "this is not a bucket").unwrap();
        let (partial, skipped) = Workspace::load("/tmp/root", &store);
        assert_eq!(skipped, 1);
        assert_eq!(partial.buckets.len(), 2, "the good ones still load");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_is_idempotent_by_slug_not_by_display_name() {
        let mut w = Workspace::new("/tmp");
        let a = w.open("Collision investigation");
        let b = w.open("collision, investigation!");
        assert_eq!(a, b, "same storage key must be the same bucket in memory too");
        assert_eq!(w.buckets.len(), 1);
    }

    #[test]
    fn closing_fixes_up_the_active_index() {
        let dir = std::env::temp_dir().join(format!("aperture-bucket-close-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = FileStore::new(&dir);
        let mut w = Workspace::new("/tmp");
        w.open("a");
        w.open("b");
        w.open("c"); // active = 2
        w.close(0, &store).unwrap();
        assert_eq!(w.active, Some(1), "the active bucket shifted down with it");
        w.close(1, &store).unwrap();
        assert_eq!(w.active, None, "closing the active one clears it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rel_strips_the_root_and_leaves_outsiders_alone() {
        let w = Workspace::new("/home/k/proj");
        assert_eq!(w.rel(Path::new("/home/k/proj/src/a.rs")), "src/a.rs");
        assert_eq!(w.rel(Path::new("/etc/hosts")), "/etc/hosts");
    }

    fn paths(b: &Bucket) -> Vec<&str> {
        b.entries.iter().map(|e| e.path.as_str()).collect()
    }
}
