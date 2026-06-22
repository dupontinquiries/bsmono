// gitmind-core/src/compose.rs
//
// Subrepo composition. Two capabilities that work together:
//
//   1. BUBBLE-UP HASHING (`tree_oid`). A directory's identity is the
//      BLAKE3 of its sorted child entries (name + kind + child Oid).
//      Because each level folds its children's Oids in, any change deep
//      in the tree bubbles up to a new root Oid — and, crucially, two
//      subtrees with identical content *anywhere, in any repo* get the
//      SAME Oid. That shared identity is what lets the global chunk store
//      dedup a library that's vendored into many projects, and what gives
//      an auto-created subrepo a stable cross-repo name.
//
//   2. GRAFT (`graft`). Insert another repo's code under a `prefix/` on a
//      NEW branch, as one commit, for fast field-testing — drop an
//      experiment into a mature crate, or vendor a library into a fresh
//      project, then test it in that branch. It uses only core git
//      plumbing (`read-tree --prefix` inside a throwaway worktree), so it
//      NEVER touches the user's current checkout, index, or HEAD.
//
// Autocreation of a subrepo from a subdirectory builds on (1): hash the
// subtree, register its root Oid, and split it out — the history-
// preserving split is a later phase; `snapshot_subrepo` below does the
// content-only extract now.

use crate::backend::{GitCli, RepoBackend};
use crate::oid::Oid;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::Command;

/// Content-address a directory tree (bubble-up Merkle hash). Skips `.git`.
/// Files are streamed so a large file never lands fully in RAM.
pub fn tree_oid(path: &Path) -> Result<Oid, String> {
    let mut entries: Vec<(String, u8, Oid)> = Vec::new();
    let rd = fs::read_dir(path).map_err(|e| format!("read_dir {}: {e}", path.display()))?;
    for ent in rd {
        let ent = ent.map_err(|e| e.to_string())?;
        let name = ent.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }
        let p = ent.path();
        let meta = fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
        let (kind, child) = if meta.file_type().is_symlink() {
            // Hash the link target, not what it points at — keeps the
            // tree hash stable and avoids following loops.
            let target = fs::read_link(&p).map_err(|e| e.to_string())?;
            (b'l', Oid::hash(target.to_string_lossy().as_bytes()))
        } else if meta.is_dir() {
            (b'd', tree_oid(&p)?)
        } else {
            (b'f', hash_file(&p)?)
        };
        entries.push((name, kind, child));
    }
    // Sort for determinism — the hash must not depend on readdir order.
    entries.sort();

    let mut buf = Vec::with_capacity(entries.len() * 40);
    for (name, kind, child) in entries {
        buf.push(kind);
        buf.extend_from_slice(name.as_bytes());
        buf.push(0);
        buf.extend_from_slice(&child.0);
    }
    Ok(Oid::hash(&buf))
}

fn hash_file(p: &Path) -> Result<Oid, String> {
    let mut f = fs::File::open(p).map_err(|e| format!("open {}: {e}", p.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Oid(*hasher.finalize().as_bytes()))
}

/// Insert `source`'s tree (at `source_ref`, e.g. "HEAD" or a branch) into
/// `target` under `prefix/` on a fresh branch `branch`. Returns the new
/// branch's commit oid. The user's current worktree/HEAD are untouched.
///
/// `prefix` must not already exist on the target's HEAD (we're adding a
/// new subtree, which is the field-test / vendor case).
pub fn graft(
    target: &GitCli,
    source: &Path,
    source_ref: &str,
    prefix: &str,
    branch: &str,
) -> Result<String, String> {
    let remote = "__gitmind_graft_src";
    let wt = std::env::temp_dir().join(format!(
        "gitmind-graft-{}-{}",
        std::process::id(),
        branch.replace('/', "_")
    ));
    let wt_s = wt.to_string_lossy().to_string();

    // Best-effort cleanup of leftovers from a previous failed run.
    let _ = target.git(&["worktree", "remove", "--force", &wt_s]);
    let _ = target.git(&["remote", "remove", remote]);

    // Branch at current HEAD (does not switch the user's checkout).
    target.git(&["branch", branch])?;

    // Pull the source objects into the target's shared object DB.
    let src_s = source.to_string_lossy().to_string();
    target.git(&["remote", "add", remote, &src_s])?;
    if let Err(e) = target.git(&["fetch", "-q", remote, source_ref]) {
        let _ = target.git(&["remote", "remove", remote]);
        let _ = target.git(&["branch", "-D", branch]);
        return Err(format!("fetch source: {e}"));
    }
    // FETCH_HEAD is per-worktree (lives in the main gitdir), so it isn't
    // visible inside the linked worktree below. Resolve it to a concrete
    // object id here — objects are shared across all worktrees.
    let fetched = match target.git(&["rev-parse", "FETCH_HEAD"]) {
        Ok(o) => o,
        Err(e) => {
            let _ = target.git(&["remote", "remove", remote]);
            let _ = target.git(&["branch", "-D", branch]);
            return Err(format!("resolve fetched commit: {e}"));
        }
    };

    // Do the graft inside a throwaway worktree checked out to `branch`, so
    // the user's working tree is never written.
    let result = (|| -> Result<String, String> {
        target.git(&["worktree", "add", "-q", &wt_s, branch])?;
        // read-tree --prefix merges the fetched tree under prefix/ into the
        // worktree's index (which matches HEAD); -u writes the files.
        run_in(&wt, &["read-tree", &format!("--prefix={}/", prefix.trim_end_matches('/')), "-u", &fetched])?;
        run_in(&wt, &["commit", "-q", "-m", &format!("graft {prefix} from {source_ref}")])?;
        let oid = run_in(&wt, &["rev-parse", "HEAD"])?;
        Ok(oid)
    })();

    // Always clean up the worktree + temp remote.
    let _ = target.git(&["worktree", "remove", "--force", &wt_s]);
    let _ = target.git(&["remote", "remove", remote]);

    match result {
        Ok(oid) => Ok(oid),
        Err(e) => {
            let _ = target.git(&["branch", "-D", branch]);
            Err(e)
        }
    }
}

/// Content-only extract of a subdirectory into a brand-new standalone repo
/// (subrepo autocreation, v0 — no history yet). Returns the new repo's
/// initial commit oid. History-preserving split is a later phase.
pub fn snapshot_subrepo(target: &GitCli, prefix: &str, dest: &Path) -> Result<String, String> {
    let src = target.root().join(prefix);
    if !src.is_dir() {
        return Err(format!("{prefix} is not a directory in the repo"));
    }
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    copy_tree(&src, dest)?;
    let dest_git = GitCli::discover(dest); // likely None until init
    let _ = dest_git;
    run_in(dest, &["init", "-q"])?;
    run_in(dest, &["add", "-A"])?;
    run_in(dest, &["commit", "-q", "-m", &format!("subrepo from {prefix}")])?;
    run_in(dest, &["rev-parse", "HEAD"])
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    for ent in fs::read_dir(src).map_err(|e| e.to_string())? {
        let ent = ent.map_err(|e| e.to_string())?;
        let name = ent.file_name();
        if name == ".git" {
            continue;
        }
        let from = ent.path();
        let to = dst.join(&name);
        let meta = fs::symlink_metadata(&from).map_err(|e| e.to_string())?;
        if meta.is_dir() {
            fs::create_dir_all(&to).map_err(|e| e.to_string())?;
            copy_tree(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn run_in(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_oid_is_stable_and_change_sensitive() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(root.join("a/x.rs"), "fn main() {}").unwrap();
        fs::write(root.join("top.txt"), "hello").unwrap();

        let h1 = tree_oid(root).unwrap();
        let h2 = tree_oid(root).unwrap();
        assert_eq!(h1, h2, "same content must hash the same");

        // A deep change must bubble up to the root.
        fs::write(root.join("a/x.rs"), "fn main() { /* edit */ }").unwrap();
        let h3 = tree_oid(root).unwrap();
        assert_ne!(h1, h3, "deep change must change the root hash");
    }

    #[test]
    fn identical_subtrees_in_different_dirs_share_oid() {
        let d = tempfile::tempdir().unwrap();
        for base in ["repoA/lib", "repoB/lib"] {
            let p = d.path().join(base);
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("mod.rs"), "pub fn f() {}").unwrap();
        }
        let a = tree_oid(&d.path().join("repoA/lib")).unwrap();
        let b = tree_oid(&d.path().join("repoB/lib")).unwrap();
        // Same content under different parents → same Oid → shared storage.
        assert_eq!(a, b);
    }
}
