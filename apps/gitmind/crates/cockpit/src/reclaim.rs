//! Reclaiming disk space. Once gitmind builds go to the shared cargo cache, the old
//! per-crate `target/` trees inside repos are dead weight (they're gitignored build
//! output). This finds them, sizes them, and — only after re-validating each one — can
//! remove them. The find/size/validate logic is unit-tested against a temp tree; the
//! actual removal is the caller's (a guarded remove_dir_all).

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

/// A `target` directory is reclaimable when it sits next to a Cargo.toml/CMakeLists.txt
/// (i.e. it's real build output), lives under `root`, and isn't the shared cache `keep`.
fn looks_like_build_target(target: &Path, keep: &Path) -> bool {
    if target.file_name().map(|n| n != "target").unwrap_or(true) {
        return false;
    }
    if target == keep || target.starts_with(keep) {
        return false;
    }
    target.parent().map(|d| d.join("Cargo.toml").is_file() || d.join("CMakeLists.txt").is_file()).unwrap_or(false)
}

/// Find reclaimable `target` dirs under `root`, skipping hidden dirs (.git, .gitmind)
/// and not descending into a target once found.
pub fn find_reclaimable(root: &Path, keep: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, keep, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, keep: &Path, out: &mut Vec<PathBuf>) {
    let rd = match std::fs::read_dir(dir) { Ok(r) => r, Err(_) => return };
    for e in rd.flatten() {
        if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = e.file_name();
        let p = e.path();
        if name == "target" && looks_like_build_target(&p, keep) {
            out.push(p);
            continue; // don't descend into a build-output dir
        }
        if name.to_string_lossy().starts_with('.') {
            continue; // skip .git / .gitmind / etc.
        }
        walk(&p, keep, out);
    }
}

/// Re-validate (before deleting) that a path is safe to remove: an existing dir named
/// `target`, under `root`, with a sibling manifest, and not the shared cache.
pub fn is_safe_target(path: &Path, root: &Path, keep: &Path) -> bool {
    path.is_dir() && path.starts_with(root) && looks_like_build_target(path, keep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_only_build_targets_and_sizes_them() {
        let base = std::env::temp_dir().join(format!("gm-reclaim-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        // a crate with a target dir (reclaimable)
        fs::create_dir_all(base.join("apps/foo/target/release")).unwrap();
        fs::write(base.join("apps/foo/Cargo.toml"), "[package]\nname=\"foo\"\n").unwrap();
        fs::write(base.join("apps/foo/target/release/foo"), vec![0u8; 1000]).unwrap();
        // a "target" dir with NO sibling manifest (not a build target → ignored)
        fs::create_dir_all(base.join("data/target")).unwrap();
        fs::write(base.join("data/target/notes.txt"), "hi").unwrap();
        // hidden dir is skipped
        fs::create_dir_all(base.join(".git/target")).unwrap();

        let keep = base.join(".cache/cargo-target");
        let found = find_reclaimable(&base, &keep);
        assert_eq!(found.len(), 1, "only the real build target: {found:?}");
        assert!(found[0].ends_with("apps/foo/target"));
        assert_eq!(dir_size(&found[0]), 1000);
        // validation gate
        assert!(is_safe_target(&found[0], &base, &keep));
        assert!(!is_safe_target(&base.join("data/target"), &base, &keep), "no manifest → unsafe");
        assert!(!is_safe_target(&base.join("apps/foo/src"), &base, &keep), "not named target → unsafe");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn never_reclaims_the_shared_cache() {
        let base = std::env::temp_dir().join(format!("gm-reclaim-keep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let keep = base.join("target"); // pretend the cache is here, next to a manifest
        fs::create_dir_all(&keep).unwrap();
        fs::write(base.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        assert!(!is_safe_target(&keep, &base, &keep), "the shared cache is never reclaimed");
        assert!(find_reclaimable(&base, &keep).is_empty());
        let _ = fs::remove_dir_all(&base);
    }
}
