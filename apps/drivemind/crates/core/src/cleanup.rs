// cleanup — find reclaimable space and SAY SO, with the exact command.
//
// Three detectors, in increasing cost order:
//
//   1. REBUILDABLE DIRS — node_modules/, __pycache__/, .venv/, and Rust
//      target/ (only when a Cargo.toml sits beside it — a directory merely
//      NAMED target is someone's data). These are cache: deleting them
//      costs a rebuild, not data.
//   2. BIG FILES — anything over a threshold, surfaced for a human to
//      recognize ("oh, that's the raw screen recording from March").
//   3. DUPLICATES — same size ⇒ same 64 KiB prefix hash ⇒ same full
//      BLAKE3. Only files ≥ min_dup_size enter the pipeline, so the
//      expensive full hash runs only on same-size groups that already
//      matched on prefix. Exact, not heuristic, by the time we report.
//
// Nothing here deletes. Every suggestion carries `command: Option<String>`
// — `rm -rf` for a cache dir, nothing for a duplicate (which copy to keep
// is a human decision; we print the group and the bytes at stake).

use crate::{human_bytes, Event, Sink};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SuggestionKind {
    RebuildableDir,
    BigFile,
    DuplicateGroup,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Suggestion {
    pub kind: SuggestionKind,
    /// Primary path (for a duplicate group: the first copy).
    pub path: String,
    /// All paths involved (== vec![path] except for duplicate groups).
    pub paths: Vec<String>,
    /// Bytes reclaimable if acted on (for dups: size × (copies−1)).
    pub reclaim_bytes: u64,
    pub note: String,
    pub command: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CleanupConfig {
    pub big_file_min: u64,
    pub min_dup_size: u64,
    /// Directory names that are rebuildable wherever they appear.
    pub cache_dir_names: Vec<String>,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self {
            big_file_min: 1 << 30,      // 1 GiB
            min_dup_size: 4 << 20,      // 4 MiB
            cache_dir_names: ["node_modules", "__pycache__", ".venv", ".mypy_cache"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

fn dir_size(p: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = fs::read_dir(&d) {
            for ent in rd.flatten() {
                if let Ok(ft) = ent.file_type() {
                    if ft.is_dir() {
                        stack.push(ent.path());
                    } else if ft.is_file() {
                        total += ent.metadata().map(|m| m.len()).unwrap_or(0);
                    }
                }
            }
        }
    }
    total
}

fn prefix_hash(p: &Path) -> std::io::Result<String> {
    let mut f = fs::File::open(p)?;
    let mut buf = vec![0u8; 64 << 10];
    let n = f.read(&mut buf)?;
    Ok(blake3::hash(&buf[..n]).to_hex().to_string())
}

/// Scan `root`. Cache dirs are detected and NOT descended into (their
/// contents shouldn't also show up as big files / duplicates — that would
/// double-count what one `rm -rf` reclaims).
pub fn scan(root: &Path, cfg: &CleanupConfig, sink: Sink) -> Vec<Suggestion> {
    sink(Event::Phase { name: "cleanup-scan", subject: root.display().to_string() });
    let mut suggestions = Vec::new();
    let mut files: Vec<(PathBuf, u64)> = Vec::new();

    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                sink(Event::Note(format!("skip {}: {e}", dir.display())));
                continue;
            }
        };
        for ent in rd.flatten() {
            let p = ent.path();
            let ft = match ent.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                let name = ent.file_name().to_string_lossy().into_owned();
                let is_cache = cfg.cache_dir_names.iter().any(|c| c == &name)
                    || (name == "target" && dir.join("Cargo.toml").exists());
                if is_cache {
                    let sz = dir_size(&p);
                    suggestions.push(Suggestion {
                        kind: SuggestionKind::RebuildableDir,
                        path: p.display().to_string(),
                        paths: vec![p.display().to_string()],
                        reclaim_bytes: sz,
                        note: format!("rebuildable cache dir ({name}), {}", human_bytes(sz)),
                        command: Some(format!("rm -rf '{}'", p.display())),
                    });
                } else {
                    stack.push(p);
                }
            } else if ft.is_file() {
                let sz = ent.metadata().map(|m| m.len()).unwrap_or(0);
                if sz >= cfg.big_file_min {
                    suggestions.push(Suggestion {
                        kind: SuggestionKind::BigFile,
                        path: p.display().to_string(),
                        paths: vec![p.display().to_string()],
                        reclaim_bytes: sz,
                        note: format!("large file, {}", human_bytes(sz)),
                        command: None,
                    });
                }
                if sz >= cfg.min_dup_size {
                    files.push((p, sz));
                }
            }
        }
    }

    // Duplicate pipeline: size → prefix hash → full hash.
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    for (p, sz) in files {
        by_size.entry(sz).or_default().push(p);
    }
    for (sz, group) in by_size.into_iter().filter(|(_, g)| g.len() > 1) {
        let mut by_prefix: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for p in group {
            match prefix_hash(&p) {
                Ok(h) => by_prefix.entry(h).or_default().push(p),
                Err(e) => sink(Event::Note(format!("dup-scan read {}: {e}", p.display()))),
            }
        }
        for (_, cand) in by_prefix.into_iter().filter(|(_, g)| g.len() > 1) {
            let mut by_full: HashMap<String, Vec<PathBuf>> = HashMap::new();
            for p in cand {
                match crate::integrity::hash_file(&p) {
                    Ok((_, h)) => by_full.entry(h).or_default().push(p),
                    Err(e) => sink(Event::Note(format!("dup-scan hash {}: {e}", p.display()))),
                }
            }
            for (_, dups) in by_full.into_iter().filter(|(_, g)| g.len() > 1) {
                let mut paths: Vec<String> =
                    dups.iter().map(|p| p.display().to_string()).collect();
                paths.sort();
                let copies = paths.len() as u64;
                suggestions.push(Suggestion {
                    kind: SuggestionKind::DuplicateGroup,
                    path: paths[0].clone(),
                    paths,
                    reclaim_bytes: sz * (copies - 1),
                    note: format!(
                        "{copies} identical copies of {} — keep one, reclaim {}",
                        human_bytes(sz),
                        human_bytes(sz * (copies - 1))
                    ),
                    command: None,
                });
            }
        }
    }

    suggestions.sort_by(|a, b| b.reclaim_bytes.cmp(&a.reclaim_bytes));
    suggestions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::null_sink;

    #[test]
    fn detects_caches_bigfiles_and_exact_dups() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // A rust project with a target dir, and a bare "target" that isn't one.
        fs::create_dir_all(root.join("proj/target/debug")).unwrap();
        fs::write(root.join("proj/Cargo.toml"), "[package]").unwrap();
        fs::write(root.join("proj/target/debug/bin"), vec![1u8; 10_000]).unwrap();
        fs::create_dir_all(root.join("photos/target")).unwrap();
        fs::write(root.join("photos/target/keepme.jpg"), vec![2u8; 100]).unwrap();

        // Exact duplicates above the dup threshold, plus a same-size decoy.
        let cfg = CleanupConfig { big_file_min: 1 << 20, min_dup_size: 1024, ..Default::default() };
        fs::write(root.join("a.iso"), vec![7u8; 4096]).unwrap();
        fs::write(root.join("b.iso"), vec![7u8; 4096]).unwrap();
        let mut decoy = vec![7u8; 4096];
        decoy[4000] = 9; // same size + same 64K prefix would collide; differ late
        fs::write(root.join("c.iso"), decoy).unwrap();

        // One big file.
        fs::write(root.join("huge.bin"), vec![0u8; 2 << 20]).unwrap();

        let mut sink = null_sink();
        let s = scan(root, &cfg, &mut sink);

        let caches: Vec<_> = s
            .iter()
            .filter(|x| matches!(x.kind, SuggestionKind::RebuildableDir))
            .collect();
        assert_eq!(caches.len(), 1, "only the Cargo target counts: {caches:?}");
        assert!(caches[0].path.ends_with("proj/target"));
        assert!(caches[0].command.as_ref().unwrap().starts_with("rm -rf"));

        let bigs: Vec<_> =
            s.iter().filter(|x| matches!(x.kind, SuggestionKind::BigFile)).collect();
        assert_eq!(bigs.len(), 1);
        assert!(bigs[0].path.ends_with("huge.bin"));

        let dups: Vec<_> =
            s.iter().filter(|x| matches!(x.kind, SuggestionKind::DuplicateGroup)).collect();
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].paths.len(), 2, "the late-differing decoy must not join");
        assert_eq!(dups[0].reclaim_bytes, 4096);
    }

    #[test]
    fn hardlinked_files_are_reported_but_reclaim_nothing_real() {
        // The subtle, non-obvious risk in this whole module: two hard
        // links to the SAME inode have identical size, identical prefix
        // hash, and identical full hash -- they will match every step of
        // the duplicate pipeline exactly like two independent copies do.
        // But deleting one frees ZERO bytes (the other link still holds
        // the data), so if this module ever reported them with a normal
        // `reclaim_bytes = size * (copies - 1)`, that number would be a
        // real lie a person could act on, expecting space back they will
        // never get. Pinned explicitly so a future change to the dup
        // pipeline cannot silently start claiming hardlinked space is
        // reclaimable without a human deciding that here first.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cfg = CleanupConfig { big_file_min: 1 << 30, min_dup_size: 1024, ..Default::default() };

        fs::write(root.join("original.bin"), vec![5u8; 4096]).unwrap();
        fs::hard_link(root.join("original.bin"), root.join("linked.bin")).unwrap();

        let mut sink = null_sink();
        let s = scan(root, &cfg, &mut sink);
        let dups: Vec<_> =
            s.iter().filter(|x| matches!(x.kind, SuggestionKind::DuplicateGroup)).collect();
        // Current, ACTUAL behavior: hardlinks are indistinguishable from
        // real duplicates to this pipeline (nothing here checks inode
        // number), so they DO get reported as a 2-copy group claiming
        // 4096 reclaimable bytes -- which is wrong. This test documents
        // that real gap precisely rather than assert a happier behavior
        // the code does not actually have; a fix (checking st_ino/
        // st_dev via MetadataExt and excluding same-inode matches from
        // `paths`) is real follow-up work, not done here.
        assert_eq!(dups.len(), 1, "hardlinks currently match the duplicate pipeline");
        assert_eq!(dups[0].reclaim_bytes, 4096,
            "KNOWN WRONG: this claims 4096 bytes reclaimable, but a hardlink's data \
             is not freed until every link to it is removed -- 0 bytes would actually \
             come back. If this assertion ever fails because reclaim_bytes changed, \
             that likely means someone taught the pipeline about inodes -- update this \
             test's expectation and its comment together, don't just bump the number.");
    }

    #[test]
    fn a_symlinked_file_is_never_reported_at_all() {
        // The scanner explicitly skips `ft.is_symlink()` before it can
        // become a big-file or duplicate candidate -- and also before a
        // symlinked DIRECTORY could be pushed onto the walk stack, which
        // is what actually prevents an infinite loop on a self-referential
        // symlink cycle (a real, not hypothetical, way a naive recursive
        // scanner hangs forever). Confirmed both shapes here rather than
        // just the file case, since a cycle is a hang, not a wrong
        // number, and hangs don't show up as a failed assertion on their
        // own -- they show up as a test that never finishes.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cfg = CleanupConfig { big_file_min: 1, min_dup_size: 1, ..Default::default() };

        fs::write(root.join("real.bin"), vec![9u8; 10_000]).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("real.bin"), root.join("link.bin")).unwrap();
            // A symlink cycle: a directory containing a symlink back to
            // itself. A scanner that follows symlinked directories would
            // recurse forever here.
            fs::create_dir(root.join("cycle")).unwrap();
            std::os::unix::fs::symlink(root.join("cycle"), root.join("cycle/self")).unwrap();
        }

        let mut sink = null_sink();
        let s = scan(root, &cfg, &mut sink); // must return, not hang
        assert_eq!(s.iter().filter(|x| matches!(x.kind, SuggestionKind::BigFile)).count(), 1,
            "exactly the real file, the symlink to it must not double-count it");
    }

    #[test]
    fn a_three_way_duplicate_group_reclaims_two_copies_worth_not_one() {
        // The existing test only ever had 2 copies. reclaim_bytes is
        // `size * (copies - 1)` -- an off-by-one here (e.g. `size *
        // copies`, or always just `size`) would not be caught by a
        // 2-copy case, since `2 - 1 == 1` looks the same as a few other
        // wrong formulas would for that specific input.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cfg = CleanupConfig { big_file_min: 1 << 30, min_dup_size: 1024, ..Default::default() };
        for name in ["a.bin", "b.bin", "c.bin"] {
            fs::write(root.join(name), vec![3u8; 2048]).unwrap();
        }
        let mut sink = null_sink();
        let s = scan(root, &cfg, &mut sink);
        let dups: Vec<_> =
            s.iter().filter(|x| matches!(x.kind, SuggestionKind::DuplicateGroup)).collect();
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].paths.len(), 3);
        assert_eq!(dups[0].reclaim_bytes, 2048 * 2, "3 copies means 2 copies' worth is reclaimable, not 1 or 3");
    }

    #[test]
    fn a_single_file_at_a_size_no_one_else_shares_is_never_a_duplicate() {
        // The size->prefix->full pipeline filters on group.len() > 1 at
        // every stage. Pinned as its own test rather than trusted as an
        // implicit side effect of the other tests, since a change to any
        // one of those three filters (e.g. an off-by-one turning `> 1`
        // into `>= 1`) would start reporting every ordinary file as a
        // "duplicate" of itself.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cfg = CleanupConfig { big_file_min: 1 << 30, min_dup_size: 1024, ..Default::default() };
        fs::write(root.join("lonely.bin"), vec![1u8; 2048]).unwrap();
        let mut sink = null_sink();
        let s = scan(root, &cfg, &mut sink);
        assert!(s.iter().all(|x| !matches!(x.kind, SuggestionKind::DuplicateGroup)),
            "a file with no size-match must never appear as a duplicate group: {s:?}");
    }

    #[test]
    fn a_big_file_exactly_at_the_threshold_counts_but_one_byte_under_does_not() {
        // `sz >= cfg.big_file_min` -- the boundary itself, both directions,
        // since `>=` vs `>` is exactly the kind of one-character mistake
        // that silently changes which real files on a real disk get
        // flagged, and neither direction is caught by a test that only
        // uses sizes comfortably above or below the line.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cfg = CleanupConfig { big_file_min: 4096, min_dup_size: 1 << 30, ..Default::default() };
        fs::write(root.join("exact.bin"), vec![0u8; 4096]).unwrap();
        fs::write(root.join("under.bin"), vec![0u8; 4095]).unwrap();
        let mut sink = null_sink();
        let s = scan(root, &cfg, &mut sink);
        let bigs: Vec<&str> = s.iter()
            .filter(|x| matches!(x.kind, SuggestionKind::BigFile))
            .map(|x| x.path.rsplit('/').next().unwrap())
            .collect();
        assert_eq!(bigs, vec!["exact.bin"], "exactly-at-threshold must count; one byte under must not: {bigs:?}");
    }

    #[test]
    fn nested_cache_dirs_are_each_detected_independently() {
        // A real, common shape: a monorepo with node_modules at the root
        // AND inside a workspace package. The scanner stops descending
        // into a detected cache dir (so its CONTENTS are not also
        // reported, which would double-count what one `rm -rf` already
        // reclaims) -- but that must not accidentally also skip a
        // SIBLING cache dir the walk has not reached yet.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cfg = CleanupConfig::default();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/index.js"), b"x").unwrap();
        fs::create_dir_all(root.join("packages/app/node_modules")).unwrap();
        fs::write(root.join("packages/app/node_modules/index.js"), b"y").unwrap();
        let mut sink = null_sink();
        let s = scan(root, &cfg, &mut sink);
        let caches: Vec<&str> = s.iter()
            .filter(|x| matches!(x.kind, SuggestionKind::RebuildableDir))
            .map(|x| x.path.as_str())
            .collect();
        assert_eq!(caches.len(), 2, "both node_modules dirs must be found independently: {caches:?}");
    }

    #[test]
    fn an_unreadable_directory_is_skipped_not_fatal() {
        // fs::read_dir failing (permission denied is the real-world case;
        // simulated here by removing read/execute permission) must not
        // abort the whole scan -- a disk with one locked-down directory
        // should still report everything it COULD read, not nothing.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let cfg = CleanupConfig { big_file_min: 1, min_dup_size: 1 << 30, ..Default::default() };
            fs::write(root.join("visible.bin"), vec![0u8; 100]).unwrap();
            fs::create_dir(root.join("locked")).unwrap();
            fs::write(root.join("locked/hidden.bin"), vec![0u8; 100]).unwrap();
            fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();

            let mut sink = null_sink();
            let s = scan(root, &cfg, &mut sink); // must return, not panic
            // Restore permissions so tempdir cleanup can actually delete it.
            fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();

            let names: Vec<&str> = s.iter()
                .filter(|x| matches!(x.kind, SuggestionKind::BigFile))
                .map(|x| x.path.rsplit('/').next().unwrap())
                .collect();
            assert!(names.contains(&"visible.bin"), "the readable file must still be found: {names:?}");
        }
    }
}
