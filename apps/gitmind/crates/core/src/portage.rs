// gitmind-core/src/portage.rs
//
// Interop between a native gitmind store and git. Two directions:
//
//   IMPORT  (`.git` → `.gitmind`): read every reachable blob out of an
//     existing git repo and stream it into the chunk store, producing the
//     content-addressed, deduped equivalent. This is the "ingest a .git and
//     create an equivalent" path — the object data lands in `.gitmind`
//     without disturbing `.git`.
//
//   EXPORT  (`.gitmind`/repo → git): write a `git bundle` of all refs, so
//     anyone with stock git can clone/fetch from a single portable file.
//     This keeps us compatible with the entire git ecosystem.
//
// Full history/graph import (commits + trees, not just blob bytes) is a
// later phase; blobs are the bulk of the data and the hard storage problem,
// so they come first.

use crate::backend::{GitCli, RepoBackend};
use crate::store::ChunkStore;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportReport {
    pub blobs: usize,
    pub total_bytes: u64,
    pub new_chunks: usize,
    pub new_bytes: u64,
}

/// Stream every reachable blob from `repo`'s git object store into `store`.
/// Bounded memory: each blob is piped straight from `git cat-file` into the
/// chunker, so even multi-GB blobs never fully land in RAM.
pub fn import_git_objects(repo: &GitCli, store: &ChunkStore) -> Result<ImportReport, String> {
    // List all objects with their type; keep the blobs.
    let listing = repo.git(&[
        "cat-file",
        "--batch-all-objects",
        "--batch-check=%(objectname) %(objecttype)",
    ])?;
    let mut report = ImportReport::default();
    for line in listing.lines() {
        let mut it = line.split_whitespace();
        let oid = match it.next() {
            Some(o) => o,
            None => continue,
        };
        if it.next() != Some("blob") {
            continue;
        }
        // Pipe `git cat-file blob <oid>` → chunk store, streaming.
        let mut child = Command::new("git")
            .arg("-C")
            .arg(repo.root())
            .args(["cat-file", "blob", oid])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn cat-file: {e}"))?;
        let stdout = child.stdout.take().ok_or("no stdout from cat-file")?;
        let rep = store.ingest(oid, stdout)?;
        let status = child.wait().map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("cat-file blob {oid} failed"));
        }
        report.blobs += 1;
        report.total_bytes += rep.total_len;
        report.new_chunks += rep.new_chunks;
        report.new_bytes += rep.new_bytes_stored;
    }
    Ok(report)
}

/// `git bundle create <out> --all` — a single portable file holding every
/// ref, clonable by stock git.
pub fn export_bundle(repo: &GitCli, out: &Path) -> Result<(), String> {
    let out_s = out.to_string_lossy().to_string();
    repo.git(&["bundle", "create", &out_s, "--all"]).map(|_| ())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphReport {
    pub commits: usize,
    pub trees: usize,
    pub blobs: usize,
    pub tags: usize,
    pub refs: usize,
    pub new_chunks: usize,
    pub total_bytes: u64,
}

impl GraphReport {
    pub fn objects(&self) -> usize {
        self.commits + self.trees + self.blobs + self.tags
    }
    pub fn summary(&self) -> String {
        format!(
            "{} objects ({} commits, {} trees, {} blobs, {} tags), {} refs, {:.1} MB",
            self.objects(), self.commits, self.trees, self.blobs, self.tags, self.refs,
            self.total_bytes as f64 / 1.0e6
        )
    }
}

/// Import the git object graph into the native store. When `reachable` is
/// true, only objects reachable from refs are imported (a clean mirror of
/// live history); when false, every object in the store is imported
/// (including dangling/unreachable ones). Either way `.gitmind` becomes a
/// complete, walkable, reconstructable mirror.
pub fn import_graph(repo: &GitCli, store: &ChunkStore, reachable: bool) -> Result<GraphReport, String> {
    use crate::store::kind_from_str;
    use std::io::{BufRead, BufReader, Read};
    let (mut child, writer) = spawn_cat_file(repo, reachable)?;
    let mut rdr = BufReader::new(child.stdout.take().ok_or("no cat-file stdout")?);
    let mut report = GraphReport::default();
    loop {
        let mut header = String::new();
        if rdr.read_line(&mut header).map_err(|e| e.to_string())? == 0 {
            break; // EOF
        }
        let header = header.trim_end();
        if header.is_empty() {
            continue;
        }
        let mut p = header.split(' ');
        let oid = match p.next() {
            Some(o) => o.to_string(),
            None => continue,
        };
        let typ = p.next().unwrap_or("");
        // "<oid> missing" or malformed → no content body to read; skip.
        let size: usize = match p.next().and_then(|s| s.parse().ok()) {
            Some(s) => s,
            None => continue,
        };
        let mut content = vec![0u8; size];
        rdr.read_exact(&mut content).map_err(|e| e.to_string())?;
        let mut lf = [0u8; 1];
        rdr.read_exact(&mut lf).map_err(|e| e.to_string())?; // trailing newline

        let kind = match kind_from_str(typ) {
            Some(k) => k,
            None => continue,
        };
        report.total_bytes += content.len() as u64;
        let rep = store.ingest(&oid, &content[..])?;
        report.new_chunks += rep.new_chunks;
        store.put_object(&oid, kind, rep.blob_oid)?;
        match kind {
            crate::store::KIND_COMMIT => report.commits += 1,
            crate::store::KIND_TREE => report.trees += 1,
            crate::store::KIND_BLOB => report.blobs += 1,
            crate::store::KIND_TAG => report.tags += 1,
            _ => {}
        }
    }
    if let Some(w) = writer {
        let _ = w.join();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("git cat-file failed".into());
    }
    // Refs (branches, tags) + symbolic HEAD.
    if let Ok(refs) = repo.git(&["show-ref"]) {
        for line in refs.lines() {
            let mut p = line.split_whitespace();
            if let (Some(oid), Some(name)) = (p.next(), p.next()) {
                store.put_ref(name, oid)?;
                report.refs += 1;
            }
        }
    }
    if let Ok(head) = repo.git(&["symbolic-ref", "HEAD"]) {
        if !head.is_empty() {
            store.put_ref("HEAD", &head)?;
        }
    }
    Ok(report)
}

/// Spawn the `git cat-file --batch` stream. For `reachable`, we enumerate
/// objects with `rev-list --objects --all` and feed their oids to cat-file
/// on stdin (from a writer thread so a large list can't deadlock the pipe);
/// otherwise we use `--batch-all-objects` to dump every object in the store.
fn spawn_cat_file(
    repo: &GitCli,
    reachable: bool,
) -> Result<(std::process::Child, Option<std::thread::JoinHandle<()>>), String> {
    if reachable {
        let oids_text = repo.git(&["rev-list", "--objects", "--all"])?;
        let mut child = Command::new("git")
            .arg("-C")
            .arg(repo.root())
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("git cat-file --batch: {e}"))?;
        let mut sin = child.stdin.take().ok_or("no cat-file stdin")?;
        let handle = std::thread::spawn(move || {
            use std::io::Write;
            for line in oids_text.lines() {
                if let Some(oid) = line.split(' ').next() {
                    if !oid.is_empty() {
                        let _ = writeln!(sin, "{oid}");
                    }
                }
            }
            // sin drops here, closing stdin so cat-file finishes.
        });
        Ok((child, Some(handle)))
    } else {
        let child = Command::new("git")
            .arg("-C")
            .arg(repo.root())
            .args(["cat-file", "--batch-all-objects", "--batch"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("git cat-file --batch: {e}"))?;
        Ok((child, None))
    }
}

/// Parsed fields of a commit object (for walking the graph without git).
#[derive(Clone, Debug, PartialEq)]
pub struct CommitMeta {
    pub tree: String,
    pub parents: Vec<String>,
    pub message: String,
}

/// Parse a raw commit object (the bytes `git cat-file commit` returns).
pub fn parse_commit(content: &[u8]) -> CommitMeta {
    let text = String::from_utf8_lossy(content);
    let mut tree = String::new();
    let mut parents = Vec::new();
    let mut lines = text.lines();
    for line in lines.by_ref() {
        if line.is_empty() {
            break; // headers end at the blank line
        }
        if let Some(t) = line.strip_prefix("tree ") {
            tree = t.to_string();
        } else if let Some(p) = line.strip_prefix("parent ") {
            parents.push(p.to_string());
        }
    }
    let message: String = lines.collect::<Vec<_>>().join("\n");
    CommitMeta { tree, parents, message }
}

/// Read a commit straight from the native store (no `.git`) — proof the
/// graph is walkable standalone.
pub fn native_commit(store: &ChunkStore, git_oid: &str) -> Result<CommitMeta, String> {
    let (kind, blob_oid) = store
        .get_object(git_oid)?
        .ok_or_else(|| format!("no such object {git_oid}"))?;
    if kind != crate::store::KIND_COMMIT {
        return Err(format!("{git_oid} is not a commit"));
    }
    let bytes = store.restore_bytes(blob_oid)?;
    Ok(parse_commit(&bytes))
}

/// Reconstruct a full git repository from the native store alone — proof
/// `.gitmind` can stand without `.git`. Writes every object back with
/// `git hash-object -w` and verifies the recomputed oid matches what we
/// stored (so reconstruction is bit-exact), then restores refs + HEAD.
pub fn export_to_git(store: &ChunkStore, into: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    run_git(into, &["init", "-q"])?;
    let mut written = 0;
    for (git_oid, kind, blob_oid) in store.list_objects()? {
        let content = store.restore_bytes(blob_oid)?;
        let got = git_write_object(into, crate::store::kind_to_str(kind), &content)?;
        if got != git_oid {
            return Err(format!("reconstruction mismatch: stored {git_oid}, rehashed {got}"));
        }
        written += 1;
    }
    let mut head_target: Option<String> = None;
    for (name, oid) in store.refs()? {
        if name == "HEAD" {
            head_target = Some(oid);
            continue;
        }
        run_git(into, &["update-ref", &name, &oid])?;
    }
    if let Some(target) = head_target {
        let _ = run_git(into, &["symbolic-ref", "HEAD", &target]);
    }
    Ok(written)
}

fn git_write_object(dir: &Path, kind: &str, content: &[u8]) -> Result<String, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["hash-object", "-w", "-t", kind, "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("hash-object: {e}"))?;
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .ok_or("no stdin")?
            .write_all(content)
            .map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("hash-object failed".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn run_git(dir: &Path, args: &[&str]) -> Result<(), String> {
    let st = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("git {args:?} failed"))
    }
}

/// True if `path` is a valid git bundle (delegates to git).
pub fn verify_bundle(repo: &GitCli, path: &Path) -> bool {
    let p = path.to_string_lossy().to_string();
    repo.git(&["bundle", "verify", &p]).is_ok()
}

/// Default location for the native store inside a repo: `.gitmind/`. Kept
/// separate from `.git` so the two systems coexist.
pub fn native_store_dir(repo_root: &Path) -> std::path::PathBuf {
    repo_root.join(".gitmind")
}
