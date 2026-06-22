// gitmind-core/src/native.rs
//
// Native write operations — create history directly in `.gitmind`, with no
// `git` process. Files become blobs, directories become trees, and a commit
// ties a root tree to its parents, all hashed to git-identical oids via
// gitobj. Combined with portage::export_to_git, a commit made here can be
// materialized into a real git repo bit-for-bit. This is what turns
// `.gitmind` from a mirror into a source of truth.
//
// v1 scope: regular files, executables, and subdirectories (recursively).
// It does not yet honor .gitignore (it skips only .git/.gitmind), and emits
// no entry for empty directories — matching git's "no empty trees" rule.

use crate::gitobj::{self, TreeEntry};
use crate::store::{ChunkStore, KIND_BLOB, KIND_COMMIT, KIND_TREE};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// A commit just written to the native store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeCommit {
    pub oid: String,
    pub tree: String,
}

/// Store a git object: ingest its content (deduped) and record git oid →
/// (kind, blob oid) in the OBJECTS table.
fn store_obj(store: &ChunkStore, kind: u8, oid: &str, content: &[u8]) -> Result<(), String> {
    let rep = store.ingest(oid, content)?;
    store.put_object(oid, kind, rep.blob_oid)
}

/// Write a blob from raw content; returns its git oid.
pub fn store_blob(store: &ChunkStore, content: &[u8]) -> Result<String, String> {
    let oid = gitobj::hash_object("blob", content);
    store_obj(store, KIND_BLOB, &oid, content)?;
    Ok(oid)
}

/// Recursively snapshot `dir` into native blobs/trees; returns the root tree
/// oid. `root` is the repo root (for computing ignore-relative paths) and
/// `ignore` filters entries the way git's .gitignore would. Empty
/// subdirectories are omitted (git stores no empty trees).
pub fn build_tree_from_dir(
    store: &ChunkStore,
    root: &Path,
    dir: &Path,
    ignore: &crate::gmignore::GmIgnore,
) -> Result<String, String> {
    let mut entries: Vec<TreeEntry> = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name == ".git" || name == ".gitmind" {
            continue;
        }
        let path = e.path();
        // Repo-relative path with forward slashes, for ignore matching.
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if ignore.ignored(&rel) {
            continue; // matches git: ignored files and dirs are not committed
        }
        let ft = e.file_type().map_err(|e| e.to_string())?;
        if ft.is_dir() {
            let sub = build_tree_from_dir(store, root, &path, ignore)?;
            if sub == EMPTY_TREE {
                continue; // git emits no entry for an empty directory
            }
            entries.push(TreeEntry { mode: "40000".into(), name, oid: sub });
        } else if ft.is_file() {
            let content = std::fs::read(&path).map_err(|e| e.to_string())?;
            let oid = store_blob(store, &content)?;
            let mode = if is_exec(&path) { "100755" } else { "100644" };
            entries.push(TreeEntry { mode: mode.into(), name, oid });
        }
        // symlinks/other special files are skipped in v1.
    }
    let (oid, bytes) = gitobj::build_tree(&mut entries);
    store_obj(store, KIND_TREE, &oid, &bytes)?;
    Ok(oid)
}

/// Commit a directory natively: snapshot it to a tree, write a commit with
/// the given parents/identity/message, and return the commit. Does NOT move
/// any ref — the caller decides what branch to advance.
pub fn commit_dir(
    store: &ChunkStore,
    dir: &Path,
    parents: &[String],
    name: &str,
    email: &str,
    message: &str,
) -> Result<NativeCommit, String> {
    // `dir` is the repo root here; honor its .gitignore + .gmignore so the
    // committed file set matches what git would track.
    let ignore = crate::gmignore::GmIgnore::load_all(dir);
    let tree = build_tree_from_dir(store, dir, dir, &ignore)?;
    let ts = now_secs();
    let line = gitobj::ident_line(name, email, ts, "+0000");
    let (oid, bytes) = gitobj::build_commit(&tree, parents, &line, &line, message);
    store_obj(store, KIND_COMMIT, &oid, &bytes)?;
    Ok(NativeCommit { oid, tree })
}

/// Resolve the native HEAD to a commit oid (following the symbolic ref).
pub fn resolve_head(store: &ChunkStore) -> Result<Option<String>, String> {
    let refs = store.refs()?;
    let head = refs.iter().find(|(n, _)| n == "HEAD").map(|(_, v)| v.clone());
    match head {
        Some(target) if target.starts_with("refs/") => {
            Ok(refs.iter().find(|(n, _)| *n == target).map(|(_, v)| v.clone()))
        }
        Some(direct) => Ok(Some(direct)), // detached HEAD stored as an oid
        None => Ok(None),
    }
}

/// Commit the working tree natively onto a branch, advancing HEAD. Returns
/// the new commit. Parent is the branch's current tip (if any).
pub fn commit_worktree(
    store: &ChunkStore,
    dir: &Path,
    branch: &str,
    name: &str,
    email: &str,
    message: &str,
) -> Result<NativeCommit, String> {
    let refname = format!("refs/heads/{branch}");
    let parents: Vec<String> = store
        .refs()?
        .into_iter()
        .find(|(n, _)| *n == refname)
        .map(|(_, v)| vec![v])
        .unwrap_or_default();
    let commit = commit_dir(store, dir, &parents, name, email, message)?;
    store.put_ref(&refname, &commit.oid)?;
    store.put_ref("HEAD", &refname)?;
    Ok(commit)
}

/// What changed between the native HEAD and the working tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

impl ChangeKind {
    pub fn sigil(&self) -> char {
        match self {
            ChangeKind::Added => 'A',
            ChangeKind::Modified => 'M',
            ChangeKind::Deleted => 'D',
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub kind: ChangeKind,
    pub path: String,
}

/// Flatten a native tree into path → blob oid, recursing into subtrees.
pub fn flatten_tree(
    store: &ChunkStore,
    tree_oid: &str,
    prefix: &str,
    out: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let (kind, blob) = store
        .get_object(tree_oid)?
        .ok_or_else(|| format!("missing tree {tree_oid}"))?;
    if kind != KIND_TREE {
        return Err(format!("{tree_oid} is not a tree"));
    }
    let bytes = store.restore_bytes(blob)?;
    for e in gitobj::parse_tree(&bytes) {
        if e.mode == "40000" {
            flatten_tree(store, &e.oid, &format!("{prefix}{}/", e.name), out)?;
        } else {
            out.insert(format!("{prefix}{}", e.name), e.oid);
        }
    }
    Ok(())
}

/// Hash the working tree into path → git blob oid WITHOUT storing anything,
/// honoring the same ignore rules a commit would.
fn hash_worktree(
    root: &Path,
    dir: &Path,
    ignore: &crate::gmignore::GmIgnore,
    out: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    for e in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name == ".git" || name == ".gitmind" {
            continue;
        }
        let path = e.path();
        let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        if ignore.ignored(&rel) {
            continue;
        }
        let ft = e.file_type().map_err(|e| e.to_string())?;
        if ft.is_dir() {
            hash_worktree(root, &path, ignore, out)?;
        } else if ft.is_file() {
            let content = std::fs::read(&path).map_err(|e| e.to_string())?;
            out.insert(rel, gitobj::hash_object("blob", &content));
        }
    }
    Ok(())
}

/// Compare the working tree to the native HEAD commit — a `git status` with
/// no git. Returns sorted Added/Modified/Deleted changes; empty means clean.
pub fn status(store: &ChunkStore, root: &Path) -> Result<Vec<Change>, String> {
    let mut head: BTreeMap<String, String> = BTreeMap::new();
    if let Some(commit_oid) = resolve_head(store)? {
        let meta = crate::portage::native_commit(store, &commit_oid)?;
        flatten_tree(store, &meta.tree, "", &mut head)?;
    }
    let ignore = crate::gmignore::GmIgnore::load_all(root);
    let mut work: BTreeMap<String, String> = BTreeMap::new();
    hash_worktree(root, root, &ignore, &mut work)?;

    let mut changes = Vec::new();
    for (path, oid) in &work {
        match head.get(path) {
            None => changes.push(Change { kind: ChangeKind::Added, path: path.clone() }),
            Some(h) if h != oid => changes.push(Change { kind: ChangeKind::Modified, path: path.clone() }),
            _ => {}
        }
    }
    for path in head.keys() {
        if !work.contains_key(path) {
            changes.push(Change { kind: ChangeKind::Deleted, path: path.clone() });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

/// Resolve a commit reference that is an oid — exact full oid, or a unique
/// short prefix (as printed by `native-log`). Errors if absent or ambiguous.
pub fn resolve_commit_oid(store: &ChunkStore, target: &str) -> Result<String, String> {
    if let Some((kind, _)) = store.get_object(target)? {
        return if kind == KIND_COMMIT {
            Ok(target.to_string())
        } else {
            Err(format!("{target} is not a commit"))
        };
    }
    let matches: Vec<String> = store
        .list_objects()?
        .into_iter()
        .filter(|(oid, k, _)| *k == KIND_COMMIT && oid.starts_with(target))
        .map(|(oid, _, _)| oid)
        .collect();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => Err(format!("not a branch or commit: {target}")),
        _ => Err(format!("ambiguous commit prefix '{target}' ({} matches)", matches.len())),
    }
}

/// Result of a native checkout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckoutReport {
    pub commit: String,
    pub written: usize,
    pub removed: usize,
}

/// Write a native tree into `dir` recursively, restoring blob content and
/// applying the executable bit. Counts files written.
fn materialize_tree(
    store: &ChunkStore,
    tree_oid: &str,
    dir: &Path,
    written: &mut usize,
) -> Result<(), String> {
    let (kind, blob) = store
        .get_object(tree_oid)?
        .ok_or_else(|| format!("missing tree {tree_oid}"))?;
    if kind != KIND_TREE {
        return Err(format!("{tree_oid} is not a tree"));
    }
    let bytes = store.restore_bytes(blob)?;
    for e in gitobj::parse_tree(&bytes) {
        let path = dir.join(&e.name);
        if e.mode == "40000" {
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            materialize_tree(store, &e.oid, &path, written)?;
        } else {
            let (_, b) = store
                .get_object(&e.oid)?
                .ok_or_else(|| format!("missing blob {}", e.oid))?;
            let content = store.restore_bytes(b)?;
            if let Some(p) = path.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, &content).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            if e.mode == "100755" {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
            }
            *written += 1;
        }
    }
    Ok(())
}

/// Remove a file and any now-empty parent directories, stopping at `root`.
fn remove_and_prune(root: &Path, rel: &str) {
    let full = root.join(rel);
    let _ = std::fs::remove_file(&full);
    let mut cur = full.parent().map(|p| p.to_path_buf());
    while let Some(d) = cur {
        if d == root {
            break;
        }
        let empty = std::fs::read_dir(&d).map(|mut it| it.next().is_none()).unwrap_or(false);
        if empty {
            let _ = std::fs::remove_dir(&d);
            cur = d.parent().map(|p| p.to_path_buf());
        } else {
            break;
        }
    }
}

/// Check out a native commit (by branch name or oid) into `dir`: write its
/// tree, remove tracked files that the previous HEAD had but the target
/// doesn't (untracked files are preserved), and move HEAD. This is a force
/// checkout — local edits to tracked files are overwritten.
pub fn checkout(store: &ChunkStore, target: &str, dir: &Path) -> Result<CheckoutReport, String> {
    let refs = store.refs()?;
    let branch_ref = format!("refs/heads/{target}");
    let (commit_oid, head_ref): (String, Option<String>) = if let Some((name, oid)) =
        refs.iter().find(|(n, _)| n == target || *n == branch_ref)
    {
        (oid.clone(), Some(name.clone()))
    } else {
        (resolve_commit_oid(store, target)?, None)
    };

    // Files tracked by the current HEAD (for safe removal).
    let mut old: BTreeMap<String, String> = BTreeMap::new();
    if let Some(h) = resolve_head(store)? {
        let m = crate::portage::native_commit(store, &h)?;
        flatten_tree(store, &m.tree, "", &mut old)?;
    }
    // Target tree.
    let meta = crate::portage::native_commit(store, &commit_oid)?;
    let mut new: BTreeMap<String, String> = BTreeMap::new();
    flatten_tree(store, &meta.tree, "", &mut new)?;

    // Write the target tree.
    let mut written = 0;
    materialize_tree(store, &meta.tree, dir, &mut written)?;

    // Remove tracked files that no longer exist in the target.
    let mut removed = 0;
    for path in old.keys() {
        if !new.contains_key(path) {
            remove_and_prune(dir, path);
            removed += 1;
        }
    }

    // Move HEAD: branch checkout keeps it symbolic; oid checkout detaches.
    match head_ref {
        Some(refname) => store.put_ref("HEAD", &refname)?,
        None => store.put_ref("HEAD", &commit_oid)?,
    }
    Ok(CheckoutReport { commit: commit_oid, written, removed })
}

fn is_exec(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Branching
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: String,
    pub oid: String,
    pub is_head: bool,
}

/// Resolve a branch name or commit oid/prefix to a commit oid.
fn resolve_ref_or_commit(store: &ChunkStore, s: &str) -> Result<String, String> {
    let refname = format!("refs/heads/{s}");
    if let Some((_, oid)) = store.refs()?.into_iter().find(|(n, _)| *n == refname || n == s) {
        return Ok(oid);
    }
    resolve_commit_oid(store, s)
}

/// List native branches; marks the one HEAD currently points at.
pub fn branch_list(store: &ChunkStore) -> Result<Vec<BranchInfo>, String> {
    let refs = store.refs()?;
    let head = refs.iter().find(|(n, _)| n == "HEAD").map(|(_, v)| v.clone());
    let mut out = Vec::new();
    for (n, oid) in &refs {
        if let Some(b) = n.strip_prefix("refs/heads/") {
            out.push(BranchInfo {
                name: b.to_string(),
                oid: oid.clone(),
                is_head: head.as_deref() == Some(n.as_str()),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Create a branch at `start` (a branch/oid) or at HEAD when None.
pub fn branch_create(store: &ChunkStore, name: &str, start: Option<&str>) -> Result<String, String> {
    let refname = format!("refs/heads/{name}");
    if store.refs()?.iter().any(|(n, _)| *n == refname) {
        return Err(format!("branch already exists: {name}"));
    }
    let oid = match start {
        Some(s) => resolve_ref_or_commit(store, s)?,
        None => resolve_head(store)?.ok_or("no HEAD to branch from")?,
    };
    store.put_ref(&refname, &oid)?;
    Ok(oid)
}

/// Delete a branch. Refuses to delete the branch HEAD points at.
pub fn branch_delete(store: &ChunkStore, name: &str) -> Result<(), String> {
    let refname = format!("refs/heads/{name}");
    let refs = store.refs()?;
    if refs.iter().find(|(n, _)| n == "HEAD").map(|(_, v)| v.as_str()) == Some(refname.as_str()) {
        return Err(format!("cannot delete the current branch: {name}"));
    }
    if !store.delete_ref(&refname)? {
        return Err(format!("no such branch: {name}"));
    }
    Ok(())
}

/// Advance whatever HEAD points at (symbolic branch, or detached) to `oid`.
fn advance_head(store: &ChunkStore, oid: &str) -> Result<(), String> {
    let head = store.refs()?.into_iter().find(|(n, _)| n == "HEAD").map(|(_, v)| v);
    match head {
        Some(t) if t.starts_with("refs/") => store.put_ref(&t, oid),
        _ => store.put_ref("HEAD", oid),
    }
}

// ---------------------------------------------------------------------------
// Tree helpers (mode-aware) for synthesizing commits
// ---------------------------------------------------------------------------

/// Flatten a tree into path → (mode, blob oid).
pub fn flatten_tree_full(
    store: &ChunkStore,
    tree_oid: &str,
    prefix: &str,
    out: &mut BTreeMap<String, (String, String)>,
) -> Result<(), String> {
    let (kind, blob) = store
        .get_object(tree_oid)?
        .ok_or_else(|| format!("missing tree {tree_oid}"))?;
    if kind != KIND_TREE {
        return Err(format!("{tree_oid} is not a tree"));
    }
    let bytes = store.restore_bytes(blob)?;
    for e in gitobj::parse_tree(&bytes) {
        if e.mode == "40000" {
            flatten_tree_full(store, &e.oid, &format!("{prefix}{}/", e.name), out)?;
        } else {
            out.insert(format!("{prefix}{}", e.name), (e.mode, e.oid));
        }
    }
    Ok(())
}

/// Build nested tree objects from a flat path → (mode, oid) map; returns the
/// root tree oid. Inverse of flatten_tree_full.
pub fn build_tree_from_map(
    store: &ChunkStore,
    entries: &BTreeMap<String, (String, String)>,
) -> Result<String, String> {
    build_subtree(store, entries, "")
}

fn build_subtree(
    store: &ChunkStore,
    entries: &BTreeMap<String, (String, String)>,
    prefix: &str,
) -> Result<String, String> {
    let mut tes: Vec<TreeEntry> = Vec::new();
    let mut subdirs: BTreeSet<String> = BTreeSet::new();
    for (path, (mode, oid)) in entries {
        if !path.starts_with(prefix) {
            continue;
        }
        let rest = &path[prefix.len()..];
        if rest.is_empty() {
            continue;
        }
        match rest.find('/') {
            Some(i) => {
                subdirs.insert(rest[..i].to_string());
            }
            None => tes.push(TreeEntry { mode: mode.clone(), name: rest.to_string(), oid: oid.clone() }),
        }
    }
    for d in &subdirs {
        let sub = build_subtree(store, entries, &format!("{prefix}{d}/"))?;
        tes.push(TreeEntry { mode: "40000".into(), name: d.clone(), oid: sub });
    }
    let (oid, bytes) = gitobj::build_tree(&mut tes);
    store_obj(store, KIND_TREE, &oid, &bytes)?;
    Ok(oid)
}

// ---------------------------------------------------------------------------
// Cherry-pick (file-level 3-way; honest conflict detection)
// ---------------------------------------------------------------------------

/// Outcome of a cherry-pick / rebase step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickOutcome {
    Committed(NativeCommit),
    /// Paths where the change couldn't be applied cleanly (both sides differ).
    Conflicts(Vec<String>),
    /// Nothing to do (the change is already present on HEAD).
    Empty,
}

/// Apply the change a commit introduced (commit vs its first parent) onto the
/// current HEAD as a new commit. File-level granularity: a file changed by
/// the pick is taken wholesale when HEAD still matches the pick's base for
/// that file; if HEAD changed the same file differently, it's a conflict and
/// nothing is committed. Returns the new commit, a conflict list, or Empty.
pub fn cherry_pick(
    store: &ChunkStore,
    commit_ref: &str,
    name: &str,
    email: &str,
) -> Result<PickOutcome, String> {
    let c = resolve_commit_oid(store, commit_ref)?;
    let head_oid = resolve_head(store)?.ok_or("no HEAD to cherry-pick onto")?;
    let hmeta = crate::portage::native_commit(store, &head_oid)?;
    let mut base: BTreeMap<String, (String, String)> = BTreeMap::new();
    flatten_tree_full(store, &hmeta.tree, "", &mut base)?;

    match apply_commit_delta(store, &c, &base)? {
        ApplyResult::Conflicts(paths) => Ok(PickOutcome::Conflicts(paths)),
        ApplyResult::Merged(_, false) => Ok(PickOutcome::Empty),
        ApplyResult::Merged(map, true) => {
            let cmeta = crate::portage::native_commit(store, &c)?;
            let tree = build_tree_from_map(store, &map)?;
            let ts = now_secs();
            let line = gitobj::ident_line(name, email, ts, "+0000");
            let (oid, bytes) = gitobj::build_commit(&tree, &[head_oid], &line, &line, &cmeta.message);
            store_obj(store, KIND_COMMIT, &oid, &bytes)?;
            advance_head(store, &oid)?;
            Ok(PickOutcome::Committed(NativeCommit { oid, tree }))
        }
    }
}

/// Result of applying one commit's change onto a base tree map.
enum ApplyResult {
    /// (resulting tree map, whether anything actually changed)
    Merged(BTreeMap<String, (String, String)>, bool),
    Conflicts(Vec<String>),
}

/// Apply the change a commit made (vs its first parent) onto `base`
/// (path → (mode, oid)) at file granularity. Clean when `base` still matches
/// the commit's parent for each touched file; conflict when both sides differ.
fn apply_commit_delta(
    store: &ChunkStore,
    commit_oid: &str,
    base: &BTreeMap<String, (String, String)>,
) -> Result<ApplyResult, String> {
    let cmeta = crate::portage::native_commit(store, commit_oid)?;
    let mut p_map: BTreeMap<String, (String, String)> = BTreeMap::new();
    if let Some(par) = cmeta.parents.first() {
        let pm = crate::portage::native_commit(store, par)?;
        flatten_tree_full(store, &pm.tree, "", &mut p_map)?;
    }
    let mut c_map: BTreeMap<String, (String, String)> = BTreeMap::new();
    flatten_tree_full(store, &cmeta.tree, "", &mut c_map)?;

    let mut out = base.clone();
    let mut conflicts: Vec<String> = Vec::new();
    let mut applied = false;
    let paths: BTreeSet<String> = p_map.keys().chain(c_map.keys()).cloned().collect();
    for path in paths {
        let pv = p_map.get(&path);
        let cv = c_map.get(&path);
        let bv = out.get(&path);
        match (pv, cv) {
            (None, Some(cval)) => match bv {
                None => { out.insert(path, cval.clone()); applied = true; }
                Some(b) if b == cval => {}
                Some(_) => conflicts.push(path),
            },
            (Some(_), None) => match bv {
                Some(b) if Some(b) == pv => { out.remove(&path); applied = true; }
                None => {}
                Some(_) => conflicts.push(path),
            },
            (Some(pval), Some(cval)) if pval != cval => match bv {
                Some(b) if b == pval => { out.insert(path, cval.clone()); applied = true; }
                Some(b) if b == cval => {}
                _ => conflicts.push(path),
            },
            _ => {}
        }
    }
    if !conflicts.is_empty() {
        conflicts.sort();
        return Ok(ApplyResult::Conflicts(conflicts));
    }
    Ok(ApplyResult::Merged(out, applied))
}

/// First-parent chain from `oid` back to the root (inclusive, tip-first).
fn first_parent_chain(store: &ChunkStore, oid: &str) -> Result<Vec<String>, String> {
    let mut chain = Vec::new();
    let mut cur = Some(oid.to_string());
    while let Some(c) = cur {
        chain.push(c.clone());
        let meta = crate::portage::native_commit(store, &c)?;
        cur = meta.parents.first().cloned();
        if chain.len() > 100_000 {
            break; // safety
        }
    }
    Ok(chain)
}

/// Outcome of a rebase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RebaseOutcome {
    Rebased { new_tip: String, commits: usize },
    Conflicts { commit: String, paths: Vec<String> },
    UpToDate,
}

/// Rebase `branch` onto `onto` (a branch name or commit). Replays the commits
/// unique to `branch` (those after the common ancestor) onto `onto`'s tip via
/// the same file-level 3-way logic as cherry-pick, building a fresh commit
/// chain and moving the branch ref. Aborts (moving nothing) on the first
/// conflicting commit. Commits that become empty are dropped (git's default).
pub fn rebase(
    store: &ChunkStore,
    branch: &str,
    onto: &str,
    name: &str,
    email: &str,
) -> Result<RebaseOutcome, String> {
    let branch_ref = format!("refs/heads/{branch}");
    if !store.refs()?.iter().any(|(n, _)| *n == branch_ref) {
        return Err(format!("no such branch: {branch}"));
    }
    let b_tip = resolve_ref_or_commit(store, branch)?;
    let o_tip = resolve_ref_or_commit(store, onto)?;

    let branch_chain = first_parent_chain(store, &b_tip)?;
    let onto_set: BTreeSet<String> = first_parent_chain(store, &o_tip)?.into_iter().collect();
    // Commits on branch not already in onto's history, oldest-first.
    let mut replay: Vec<String> =
        branch_chain.iter().filter(|c| !onto_set.contains(*c)).cloned().collect();
    replay.reverse();

    if replay.is_empty() {
        // Nothing unique to replay → fast-forward the branch to onto.
        store.put_ref(&branch_ref, &o_tip)?;
        return Ok(RebaseOutcome::UpToDate);
    }

    let ometa = crate::portage::native_commit(store, &o_tip)?;
    let mut base_map: BTreeMap<String, (String, String)> = BTreeMap::new();
    flatten_tree_full(store, &ometa.tree, "", &mut base_map)?;

    let mut parent = o_tip.clone();
    let mut count = 0;
    for c in &replay {
        match apply_commit_delta(store, c, &base_map)? {
            ApplyResult::Conflicts(paths) => {
                return Ok(RebaseOutcome::Conflicts { commit: c.clone(), paths });
            }
            ApplyResult::Merged(_, false) => { /* empty commit, drop it */ }
            ApplyResult::Merged(map, true) => {
                let tree = build_tree_from_map(store, &map)?;
                let cmeta = crate::portage::native_commit(store, c)?;
                let ts = now_secs();
                let line = gitobj::ident_line(name, email, ts, "+0000");
                let (oid, bytes) =
                    gitobj::build_commit(&tree, &[parent.clone()], &line, &line, &cmeta.message);
                store_obj(store, KIND_COMMIT, &oid, &bytes)?;
                base_map = map;
                parent = oid;
                count += 1;
            }
        }
    }
    store.put_ref(&branch_ref, &parent)?;
    // If HEAD is this branch, keep HEAD symbolic (already is); worktree sync is
    // the caller's responsibility.
    Ok(RebaseOutcome::Rebased { new_tip: parent, commits: count })
}

// ---------------------------------------------------------------------------
// Patch export / apply (unified diff, git-apply compatible, no git in path)
// ---------------------------------------------------------------------------

/// Read a native blob's bytes by its git oid.
fn read_blob(store: &ChunkStore, git_oid: &str) -> Result<Vec<u8>, String> {
    let (_, blob) = store.get_object(git_oid)?.ok_or_else(|| format!("missing blob {git_oid}"))?;
    store.restore_bytes(blob)
}

/// Blob as UTF-8 text, or None if it isn't valid UTF-8 (treated as binary).
fn blob_text(store: &ChunkStore, git_oid: &str) -> Result<Option<String>, String> {
    Ok(String::from_utf8(read_blob(store, git_oid)?).ok())
}

/// Format what a commit changed (vs its first parent) as a unified diff that
/// `git apply` accepts. Binary changes are noted, not diffed.
pub fn format_commit_patch(store: &ChunkStore, commit_ref: &str) -> Result<String, String> {
    let c = resolve_commit_oid(store, commit_ref)?;
    let cmeta = crate::portage::native_commit(store, &c)?;
    let mut p_map: BTreeMap<String, (String, String)> = BTreeMap::new();
    if let Some(par) = cmeta.parents.first() {
        let pm = crate::portage::native_commit(store, par)?;
        flatten_tree_full(store, &pm.tree, "", &mut p_map)?;
    }
    let mut c_map: BTreeMap<String, (String, String)> = BTreeMap::new();
    flatten_tree_full(store, &cmeta.tree, "", &mut c_map)?;

    let paths: BTreeSet<String> = p_map.keys().chain(c_map.keys()).cloned().collect();
    let mut out = String::new();
    for path in paths {
        let old = match p_map.get(&path) { Some((_, oid)) => blob_text(store, oid)?, None => None };
        let new = match c_map.get(&path) { Some((_, oid)) => blob_text(store, oid)?, None => None };
        let present_old = p_map.contains_key(&path);
        let present_new = c_map.contains_key(&path);
        // Binary on either side (and actually changed) → marker only.
        if (present_old && old.is_none()) || (present_new && new.is_none()) {
            let old_oid = p_map.get(&path).map(|(_, o)| o.clone());
            let new_oid = c_map.get(&path).map(|(_, o)| o.clone());
            if old_oid != new_oid {
                out.push_str(&format!("diff --git a/{path} b/{path}\n"));
                out.push_str(&format!("Binary files a/{path} and b/{path} differ\n"));
            }
            continue;
        }
        let section = crate::diff::file_patch(
            &path,
            if present_old { old.as_deref() } else { None },
            if present_new { new.as_deref() } else { None },
            3,
        );
        if let Some(s) = section {
            out.push_str(&s);
        }
    }
    Ok(out)
}

/// Apply a unified diff to a working directory (create/modify/delete files).
/// Strict context matching with a small offset search; binary sections error.
pub fn apply_patch_to_dir(patch: &str, dir: &Path) -> Result<usize, String> {
    let mut applied = 0;
    for section in split_sections(patch) {
        let mut a_path: Option<String> = None;
        let mut b_path: Option<String> = None;
        let mut hunk_start: Option<usize> = None;
        let lines: Vec<&str> = section.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            if let Some(p) = l.strip_prefix("--- ") {
                a_path = patch_path(p);
            } else if let Some(p) = l.strip_prefix("+++ ") {
                b_path = patch_path(p);
            } else if l.starts_with("@@") {
                hunk_start = Some(i);
                break;
            } else if l.starts_with("Binary files") {
                return Err("cannot apply binary patch section natively".into());
            }
        }
        let hs = match hunk_start { Some(h) => h, None => continue };
        let hunks = lines[hs..].join("\n");

        if b_path.is_none() {
            // deletion (+++ /dev/null)
            if let Some(p) = &a_path {
                let _ = std::fs::remove_file(dir.join(p));
                applied += 1;
            }
        } else if a_path.is_none() {
            // creation (--- /dev/null)
            let p = b_path.unwrap();
            let content = crate::diff::apply_hunks("", &hunks)?;
            let full = dir.join(&p);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&full, content).map_err(|e| e.to_string())?;
            applied += 1;
        } else {
            // modification
            let p = b_path.unwrap();
            let full = dir.join(&p);
            let current = std::fs::read_to_string(&full).map_err(|e| e.to_string())?;
            let patched = crate::diff::apply_hunks(&current, &hunks)?;
            std::fs::write(&full, patched).map_err(|e| e.to_string())?;
            applied += 1;
        }
    }
    Ok(applied)
}

/// Split a multi-file patch into `diff --git` sections.
fn split_sections(patch: &str) -> Vec<String> {
    let mut sections = Vec::new();
    let mut cur = String::new();
    for line in patch.lines() {
        if line.starts_with("diff --git ") && !cur.is_empty() {
            sections.push(std::mem::take(&mut cur));
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.trim().is_empty() {
        sections.push(cur);
    }
    sections
}

/// Strip an `a/`,`b/` prefix; map `/dev/null` to None.
fn patch_path(s: &str) -> Option<String> {
    let s = s.trim();
    if s == "/dev/null" {
        return None;
    }
    let s = s.strip_prefix("a/").or_else(|| s.strip_prefix("b/")).unwrap_or(s);
    Some(s.to_string())
}

// ---------------------------------------------------------------------------
// Merge (file-level 3-way against a merge-base)
// ---------------------------------------------------------------------------

/// Outcome of merging another ref into the current HEAD.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    /// HEAD already contained the other ref.
    UpToDate,
    /// HEAD was an ancestor; the branch was fast-forwarded to the new tip.
    FastForward { new_tip: String },
    /// A real merge commit (two parents) was created.
    Merged { commit: String },
    /// Files both sides changed differently; nothing committed.
    Conflicts(Vec<String>),
}

/// Merge `theirs_ref` into the current branch (HEAD), file-level 3-way:
/// per file, if both sides match take it; if only one side changed take that
/// side; if both changed differently it's a conflict. Fast-forwards when HEAD
/// is an ancestor. Creates a two-parent merge commit otherwise.
pub fn merge(
    store: &ChunkStore,
    theirs_ref: &str,
    name: &str,
    email: &str,
) -> Result<MergeOutcome, String> {
    let ours = resolve_head(store)?.ok_or("no HEAD to merge into")?;
    let theirs = resolve_ref_or_commit(store, theirs_ref)?;
    if ours == theirs {
        return Ok(MergeOutcome::UpToDate);
    }

    let ours_chain = first_parent_chain(store, &ours)?;
    let theirs_chain = first_parent_chain(store, &theirs)?;
    let theirs_set: BTreeSet<String> = theirs_chain.iter().cloned().collect();

    // Most recent common ancestor along first-parent chains.
    let base = ours_chain.iter().find(|c| theirs_set.contains(*c)).cloned();

    // theirs is already an ancestor of ours → nothing to do.
    if base.as_deref() == Some(theirs.as_str()) {
        return Ok(MergeOutcome::UpToDate);
    }
    // ours is an ancestor of theirs → fast-forward.
    if theirs_chain.contains(&ours) {
        advance_head(store, &theirs)?;
        return Ok(MergeOutcome::FastForward { new_tip: theirs });
    }

    let base_oid = base.ok_or("no common ancestor between the branches")?;
    let mut base_map = BTreeMap::new();
    flatten_tree_full(store, &crate::portage::native_commit(store, &base_oid)?.tree, "", &mut base_map)?;
    let mut our_map = BTreeMap::new();
    flatten_tree_full(store, &crate::portage::native_commit(store, &ours)?.tree, "", &mut our_map)?;
    let mut their_map = BTreeMap::new();
    flatten_tree_full(store, &crate::portage::native_commit(store, &theirs)?.tree, "", &mut their_map)?;

    let all: BTreeSet<String> =
        base_map.keys().chain(our_map.keys()).chain(their_map.keys()).cloned().collect();
    let mut merged: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut conflicts: Vec<String> = Vec::new();
    for path in all {
        let b = base_map.get(&path);
        let o = our_map.get(&path);
        let t = their_map.get(&path);
        let chosen: Option<&(String, String)> = if o == t {
            o // same on both sides (including both deleted)
        } else if b == o {
            t // only theirs changed
        } else if b == t {
            o // only ours changed
        } else {
            conflicts.push(path);
            continue;
        };
        if let Some(v) = chosen {
            merged.insert(path, v.clone());
        }
    }
    if !conflicts.is_empty() {
        conflicts.sort();
        return Ok(MergeOutcome::Conflicts(conflicts));
    }

    let into = store.refs()?.into_iter().find(|(n, _)| n == "HEAD")
        .and_then(|(_, t)| t.strip_prefix("refs/heads/").map(|s| s.to_string()))
        .unwrap_or_else(|| "HEAD".to_string());
    let tree = build_tree_from_map(store, &merged)?;
    let ts = now_secs();
    let line = gitobj::ident_line(name, email, ts, "+0000");
    let msg = format!("Merge {theirs_ref} into {into}");
    let (oid, bytes) = gitobj::build_commit(&tree, &[ours, theirs], &line, &line, &msg);
    store_obj(store, KIND_COMMIT, &oid, &bytes)?;
    advance_head(store, &oid)?;
    Ok(MergeOutcome::Merged { commit: oid })
}
