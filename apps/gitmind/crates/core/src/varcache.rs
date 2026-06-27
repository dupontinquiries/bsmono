//! `.var` — a system-wide, content-addressed cache for build artifacts.
//!
//! The problem: a `target/` (cargo), `node_modules/`, or render cache is too
//! big to commit but too valuable to `.gitignore` and lose. The insight: most
//! of it is *shared* — the same crate version compiles to the same artifacts
//! across all your projects. So we stash it into ONE system-wide chunk store,
//! where FastCDC dedup (plus the delta repack) collapses everything identical
//! or near-identical across every project to a single copy. The repo only
//! commits a tiny pointer file (`.var/<name>` = a tree oid); the bytes live in
//! the shared store and are reconstructed on demand.
//!
//! A stash is just a native tree (content-addressed), so two projects stashing
//! the same dependency build produce the *same* tree oid and share storage for
//! free. "Pinning" records a stash under `refs/pins/<name>` so a future gc
//! keeps it while evicting unpinned scratch.

use crate::gmignore::GmIgnore;
use crate::native;
use crate::store::ChunkStore;
use std::path::{Path, PathBuf};

/// Where the shared store lives. Overridable with `GITMIND_VAR`; otherwise
/// `~/.cache/gitmind/var.redb` (system-wide, shared across all repos).
pub fn var_store_path() -> PathBuf {
    if let Ok(p) = std::env::var("GITMIND_VAR") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".cache").join("gitmind").join("var.redb")
}

/// Open (creating parent dirs) the shared var store.
pub fn open_var_store() -> Result<ChunkStore, String> {
    let p = var_store_path();
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    ChunkStore::open(p)
}

/// Stash a directory (e.g. `target/`) into the shared store. Returns the tree
/// oid — identical content across projects yields the identical oid, so this
/// is where the cross-project dedup happens. Honors `.gitignore`/`.gmignore`
/// inside `dir`.
pub fn stash(store: &ChunkStore, dir: &Path) -> Result<String, String> {
    let ignore = GmIgnore::load_all(dir);
    native::build_tree_from_dir(store, dir, dir, &ignore)
}

/// Restore a stashed tree back into `dir`.
pub fn restore(store: &ChunkStore, tree_oid: &str, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut n = 0;
    native::materialize_tree(store, tree_oid, dir, &mut n)
}

/// Pin a stash so gc keeps it: `refs/pins/<name>` → tree oid.
pub fn pin(store: &ChunkStore, name: &str, tree_oid: &str) -> Result<(), String> {
    store.put_ref(&format!("refs/pins/{name}"), tree_oid)
}

pub fn unpin(store: &ChunkStore, name: &str) -> Result<(), String> {
    store.delete_ref(&format!("refs/pins/{name}")).map(|_| ())
}

/// All pins as (name, tree_oid).
pub fn pins(store: &ChunkStore) -> Result<Vec<(String, String)>, String> {
    Ok(store
        .refs()?
        .into_iter()
        .filter_map(|(n, v)| n.strip_prefix("refs/pins/").map(|s| (s.to_string(), v)))
        .collect())
}
