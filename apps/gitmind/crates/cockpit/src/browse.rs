//! Repo file-browser backing logic (pure `std`, unit-tested in isolation).
//!
//! The GUI had no way to list the working tree — you could only view a file by
//! peeking a *changed* one from the status panel, so a clean repo showed nothing
//! browsable. [`list_repo_dir`] lists one directory under the repo root for the
//! Files tab: it skips `.git` and heavy build dirs, sorts the result, and refuses
//! any path that escapes the repo.

use std::path::Path;

/// Directories the browser never descends into (git internals + build output).
const SKIP: &[&str] = &[".git", "target", "node_modules"];

/// List the directory at `root`/`subpath` (subpath is repo-relative; empty =
/// the root). Returns `(relative_path, dirs, files)`, each sorted, with the
/// noise dirs filtered out. Errors if the path can't be read or escapes `root`.
pub fn list_repo_dir(root: &Path, subpath: &str) -> Result<(String, Vec<String>, Vec<String>), String> {
    let sub = subpath.trim().trim_start_matches('/');
    let target = if sub.is_empty() { root.to_path_buf() } else { root.join(sub) };

    // Resolve symlinks and reject anything outside the repo root.
    let canon_root = root.canonicalize().map_err(|e| format!("repo root: {e}"))?;
    let canon_target = target
        .canonicalize()
        .map_err(|e| format!("cannot read {}: {e}", target.display()))?;
    if !canon_target.starts_with(&canon_root) {
        return Err("path escapes the repo".into());
    }

    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&canon_target).map_err(|e| e.to_string())?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            if SKIP.contains(&name.as_str()) {
                continue;
            }
            dirs.push(name);
        } else {
            files.push(name);
        }
    }
    dirs.sort();
    files.sort();

    let rel = canon_target
        .strip_prefix(&canon_root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    Ok((rel, dirs, files))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_sorted_skips_noise_and_blocks_escape() {
        let tmp = std::env::temp_dir().join(format!("gm_browse_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        for d in ["apps", "shared", "target", ".git"] {
            fs::create_dir_all(tmp.join(d)).unwrap();
        }
        fs::write(tmp.join("README.md"), "x").unwrap();
        fs::write(tmp.join("Cargo.toml"), "x").unwrap();

        let (rel, dirs, files) = list_repo_dir(&tmp, "").unwrap();
        assert_eq!(rel, "");
        assert_eq!(dirs, vec!["apps", "shared"], "target/.git skipped, sorted");
        assert_eq!(files, vec!["Cargo.toml", "README.md"], "files sorted");

        // descend into a subdir
        fs::write(tmp.join("apps/main.rs"), "x").unwrap();
        let (rel2, _, files2) = list_repo_dir(&tmp, "apps").unwrap();
        assert_eq!(rel2, "apps");
        assert_eq!(files2, vec!["main.rs"]);

        // a leading slash is tolerated (treated as repo-relative)
        assert!(list_repo_dir(&tmp, "/apps").is_ok());

        // escapes are refused
        assert!(list_repo_dir(&tmp, "../..").is_err(), "must reject path escape");

        let _ = fs::remove_dir_all(&tmp);
    }
}
