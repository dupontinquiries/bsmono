//! Build artifacts and junk, across every build system gitmind can import. Two jobs:
//! reclaiming disk (find + size + safely remove regeneratable output) and preparing a
//! project to ingest cleanly (so a venv, node_modules, or a target/ tree never ends up in
//! git or the gitmind store). Everything is validated before deletion — only known,
//! regeneratable build output is ever touched; source is never removed.

use std::path::{Path, PathBuf};

/// Recursively sum file sizes under `path` (bytes).
pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.flatten() {
            match e.file_type() {
                Ok(ft) if ft.is_dir() => total += dir_size(&e.path()),
                Ok(ft) if ft.is_file() => total += e.metadata().map(|m| m.len()).unwrap_or(0),
                _ => {}
            }
        }
    }
    total
}

/// A piece of reclaimable junk found under a root.
#[derive(Debug, Clone)]
pub struct Junk {
    pub path: PathBuf,
    pub bytes: u64,
    pub label: String,
}

/// Junk files (not dirs) that are always safe to delete.
const JUNK_FILES: &[&str] = &[".DS_Store", "Thumbs.db"];

/// If `dir` is a regeneratable build-output directory, return a human label. `keep` (the
/// shared build base) is never treated as junk. Validation is per-system so we never nuke
/// a source directory that merely shares a name.
pub fn dir_junk_label(dir: &Path, keep: &Path) -> Option<&'static str> {
    if dir == keep || dir.starts_with(keep) {
        return None;
    }
    let name = dir.file_name()?.to_string_lossy().into_owned();
    match name.as_str() {
        "target" => {
            let ok = dir.join("CACHEDIR.TAG").is_file()
                || dir.join(".rustc_info.json").is_file()
                || dir.parent().map(|d| d.join("Cargo.toml").is_file()).unwrap_or(false);
            ok.then_some("Rust target")
        }
        ".venv" | "venv" => (dir.join("pyvenv.cfg").is_file() || dir.join("bin/python").exists()).then_some("Python venv"),
        "__pycache__" => Some("Python cache"),
        ".pytest_cache" | ".mypy_cache" | ".ruff_cache" => Some("Python cache"),
        "node_modules" => Some("Node modules"),
        "build" => dir.join("CMakeCache.txt").is_file().then_some("CMake build"),
        ".gitmind-bin" => Some("gitmind build output"),
        _ => None,
    }
}

/// Find reclaimable build junk under `root`: regeneratable output dirs (target/, .venv,
/// node_modules, __pycache__, CMake build/, …) plus stray junk files. Skips .git/.gitmind
/// and never descends into a junk dir once found.
pub fn scan(root: &Path, keep: &Path) -> Vec<Junk> {
    let mut out = Vec::new();
    walk(root, keep, &mut out);
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes)); // biggest first
    out
}

fn walk(dir: &Path, keep: &Path, out: &mut Vec<Junk>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let ft = match e.file_type() { Ok(f) => f, Err(_) => continue };
        if ft.is_file() {
            if p.file_name().map(|n| JUNK_FILES.contains(&n.to_string_lossy().as_ref())).unwrap_or(false) {
                out.push(Junk { bytes: e.metadata().map(|m| m.len()).unwrap_or(0), label: "OS junk".into(), path: p });
            }
            continue;
        }
        if !ft.is_dir() { continue; }
        let name = e.file_name();
        if name == ".git" || name == ".gitmind" {
            continue;
        }
        if let Some(label) = dir_junk_label(&p, keep) {
            out.push(Junk { bytes: dir_size(&p), label: label.into(), path: p });
            continue; // don't descend into build output
        }
        walk(&p, keep, out);
    }
}

/// Remove the reclaimable junk under `root`, re-validating each entry immediately before
/// deletion. Returns (count removed, bytes freed).
pub fn clean(root: &Path, keep: &Path) -> (usize, u64) {
    let mut count = 0;
    let mut freed = 0u64;
    for j in scan(root, keep) {
        let still_junk = if j.path.is_dir() {
            dir_junk_label(&j.path, keep).is_some()
        } else {
            j.path.is_file() && j.path.file_name().map(|n| JUNK_FILES.contains(&n.to_string_lossy().as_ref())).unwrap_or(false)
        };
        if !still_junk || !j.path.starts_with(root) {
            continue;
        }
        let bytes = j.bytes;
        let removed = if j.path.is_dir() { std::fs::remove_dir_all(&j.path).is_ok() } else { std::fs::remove_file(&j.path).is_ok() };
        if removed { count += 1; freed += bytes; }
    }
    (count, freed)
}

/// .gitignore lines covering build output for every supported system (plus gitmind's own
/// launcher/output and the store, when `gitmind` is set), so an ingest stays clean.
pub fn gitignore_lines(gitmind: bool) -> Vec<&'static str> {
    let mut v = vec![
        "# build output (managed by gitmind)",
        "/target/",
        ".venv/",
        "venv/",
        "__pycache__/",
        "*.pyc",
        ".pytest_cache/",
        ".mypy_cache/",
        ".ruff_cache/",
        "node_modules/",
        "/build/",
        "CMakeCache.txt",
        "CMakeFiles/",
        ".gitmind-bin/",
        ".gitmind-run.sh",
        ".DS_Store",
        "Thumbs.db",
    ];
    if gitmind {
        v.push("/.gitmind/store.redb");
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_and_labels_junk_across_build_systems() {
        let base = std::env::temp_dir().join(format!("gm-art-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        // Rust target next to a manifest
        fs::create_dir_all(base.join("proj/target/release")).unwrap();
        fs::write(base.join("proj/Cargo.toml"), "[package]\nname=\"p\"\n").unwrap();
        fs::write(base.join("proj/target/release/p"), vec![0u8; 2000]).unwrap();
        // Python venv
        fs::create_dir_all(base.join("game/.venv/bin")).unwrap();
        fs::write(base.join("game/.venv/pyvenv.cfg"), "home = /usr").unwrap();
        fs::write(base.join("game/.venv/bin/python"), vec![0u8; 500]).unwrap();
        // __pycache__ + node_modules + CMake build
        fs::create_dir_all(base.join("game/__pycache__")).unwrap();
        fs::write(base.join("game/__pycache__/x.pyc"), vec![0u8; 100]).unwrap();
        fs::create_dir_all(base.join("web/node_modules/dep")).unwrap();
        fs::write(base.join("web/node_modules/dep/i.js"), vec![0u8; 300]).unwrap();
        fs::create_dir_all(base.join("c/build")).unwrap();
        fs::write(base.join("c/build/CMakeCache.txt"), "x").unwrap();
        // a source dir named "build" WITHOUT a CMakeCache → NOT junk
        fs::create_dir_all(base.join("docs/build")).unwrap();
        fs::write(base.join("docs/build/page.html"), "hi").unwrap();
        // OS junk file
        fs::write(base.join(".DS_Store"), vec![0u8; 10]).unwrap();
        // .git is skipped
        fs::create_dir_all(base.join(".git/target")).unwrap();

        let keep = base.join(".cache");
        let found = scan(&base, &keep);
        let labels: Vec<&str> = found.iter().map(|j| j.label.as_str()).collect();
        assert!(labels.contains(&"Rust target"));
        assert!(labels.contains(&"Python venv"));
        assert!(labels.contains(&"Python cache"));
        assert!(labels.contains(&"Node modules"));
        assert!(labels.contains(&"CMake build"));
        assert!(labels.contains(&"OS junk"));
        assert!(!found.iter().any(|j| j.path.ends_with("docs/build")), "source 'build' without CMakeCache is safe");
        assert!(!found.iter().any(|j| j.path.starts_with(base.join(".git"))), ".git is never touched");
        // biggest first
        assert!(found[0].bytes >= found[found.len() - 1].bytes);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn clean_removes_only_validated_junk() {
        let base = std::env::temp_dir().join(format!("gm-artclean-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("proj/target")).unwrap();
        fs::write(base.join("proj/Cargo.toml"), "[package]\nname=\"p\"\n").unwrap();
        fs::write(base.join("proj/target/big"), vec![0u8; 4096]).unwrap();
        fs::write(base.join("proj/src.rs"), "fn main(){}").unwrap();
        let keep = base.join(".cache");
        let (count, freed) = clean(&base, &keep);
        assert_eq!(count, 1);
        assert!(freed >= 4096);
        assert!(!base.join("proj/target").exists(), "target removed");
        assert!(base.join("proj/src.rs").exists(), "source untouched");
        assert!(base.join("proj/Cargo.toml").exists(), "manifest untouched");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn gitignore_covers_every_system() {
        let g = gitignore_lines(true);
        for pat in ["/target/", ".venv/", "node_modules/", "__pycache__/", "/build/", ".gitmind-bin/", "/.gitmind/store.redb"] {
            assert!(g.contains(&pat), "missing {pat}");
        }
        assert!(!gitignore_lines(false).contains(&"/.gitmind/store.redb"));
    }
}
