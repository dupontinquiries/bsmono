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
}
