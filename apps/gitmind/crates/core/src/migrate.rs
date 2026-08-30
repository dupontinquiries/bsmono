// gitmind-core/src/migrate.rs
//
// Migration of delta-hostile files OUT of git's object store and INTO the
// chunk store — the answer to "this repo versions videos/models/archives
// and git stores every version near-whole".
//
// Why git loses on these files: git's packfile deltas are byte-level
// diffs between similar blobs, then zlib. Already-compressed content
// (media containers, archives, encrypted or high-entropy data) defeats
// both stages — small source edits produce avalanche changes in the
// compressed bytes, so deltas find nothing, and zlib can't shrink what's
// already at entropy. Result: N versions ≈ N × size, forever, in every
// clone. The chunk store attacks the same content BEFORE its compressed
// form matters: FastCDC finds unchanged regions, dedups them, and (since
// the 'R' encoding) doesn't even waste zstd on the rest.
//
// The migration contract, in order of what a user cares about:
//
//   * NEVER LOSSY. Originals are ingested through the standard chunk
//     path; `migrated.jsonl` records path → blob oid; `restore_file`
//     writes the exact bytes back. Migration is reversible by design.
//   * HISTORY UNTOUCHED. `apply` moves files forward (ingest + optional
//     `git rm --cached` + .gitignore append). Past commits still carry
//     the blobs; shrinking history is a separate, explicitly destructive
//     operation (filter-repo) that this module deliberately does not do.
//   * PLAN FIRST. `scan` is read-only and returns everything `apply`
//     would touch, with sizes and reasons, so frontends can show the
//     plan (and drivemind's `dvm gitscan` can produce it on another
//     machine) before anything changes.
//
// Detection mirrors drivemind-core's deltahostile module (extension class
// OR sampled entropy ≥ threshold) — two small independent copies rather
// than a cross-app dependency; the lists live in one screenful each.

use crate::backend::{GitCli, RepoBackend};
use crate::oid::Oid;
use crate::store::{chunk_entropy, ChunkStore};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Compressed-by-format extensions (lowercase, no dot).
pub const COMPRESSED_EXTS: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "webm", "m4v", "mpg", "mpeg", "ts", "mts", "m2ts", "wmv", "flv",
    "ogv", "3gp", "vob", "ivf", "mp3", "aac", "m4a", "ogg", "opus", "flac", "jpg", "jpeg", "png",
    "gif", "webp", "heic", "avif", "zip", "gz", "xz", "zst", "bz2", "7z", "rar", "jar", "apk",
    "flatpak", "deb", "rpm", "gguf", "safetensors", "onnx", "npz",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Reason {
    CompressedFormat(String),
    HighEntropy(f64),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    /// Repo-relative path as git reports it.
    pub path: String,
    pub size: u64,
    pub reason: Reason,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigratePlan {
    pub candidates: Vec<Candidate>,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, Default)]
pub struct MigrateConfig {
    pub min_size: u64,
    pub entropy_threshold: f64,
    /// Also `git rm --cached` each migrated path so git stops tracking it.
    pub untrack: bool,
}

impl MigrateConfig {
    pub fn defaults() -> Self {
        Self { min_size: 1 << 20, entropy_threshold: 7.5, untrack: false }
    }
}

/// One line of `.gitmind/migrated.jsonl` — the reversibility record.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigratedEntry {
    pub path: String,
    pub blob_oid: String,
    pub size: u64,
    pub migrated_unix: u64,
}

#[derive(Clone, Debug, Default)]
pub struct ApplyReport {
    pub files: usize,
    pub bytes_ingested: u64,
    /// New bytes the store actually grew by — dedup + the raw/zstd encode
    /// decide this, and the gap to `bytes_ingested` is the pitch.
    pub bytes_stored: u64,
    pub untracked: usize,
}

/// Average entropy over up to three 64 KiB windows (start/middle/end) —
/// windows, not the head alone, because real containers have low-entropy
/// index/header regions that would mask an otherwise incompressible file.
fn sampled_entropy(path: &Path) -> std::io::Result<f64> {
    const W: usize = 64 << 10;
    let mut f = fs::File::open(path)?;
    let len = f.metadata()?.len();
    let mut offs: Vec<u64> = vec![0];
    if len > (2 * W as u64) {
        offs.push(len / 2 - (W as u64) / 2);
    }
    if len > (3 * W as u64) {
        offs.push(len - W as u64);
    }
    let mut buf = vec![0u8; W];
    let (mut sum, mut n) = (0.0, 0usize);
    for off in offs {
        f.seek(SeekFrom::Start(off))?;
        let got = f.read(&mut buf)?;
        if got > 0 {
            sum += chunk_entropy(&buf[..got]);
            n += 1;
        }
    }
    Ok(if n == 0 { 0.0 } else { sum / n as f64 })
}

fn ext_of(path: &str) -> Option<String> {
    Path::new(path).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase())
}

/// Read-only scan of the repo's TRACKED files for migration candidates.
pub fn scan(repo: &GitCli, cfg: &MigrateConfig) -> Result<MigratePlan, String> {
    let listing = repo.git(&["ls-files", "-z"])?;
    let mut candidates = Vec::new();
    let mut total_bytes = 0u64;
    for rel in listing.split('\0').filter(|s| !s.is_empty()) {
        let abs = repo.root().join(rel);
        let size = match fs::metadata(&abs) {
            Ok(m) if m.is_file() => m.len(),
            _ => continue,
        };
        if size < cfg.min_size {
            continue;
        }
        let reason = match ext_of(rel) {
            Some(ext) if COMPRESSED_EXTS.contains(&ext.as_str()) => {
                Some(Reason::CompressedFormat(ext))
            }
            _ => match sampled_entropy(&abs) {
                Ok(e) if e >= cfg.entropy_threshold => Some(Reason::HighEntropy(e)),
                _ => None,
            },
        };
        if let Some(reason) = reason {
            total_bytes += size;
            candidates.push(Candidate { path: rel.to_string(), size, reason });
        }
    }
    candidates.sort_by(|a, b| b.size.cmp(&a.size));
    Ok(MigratePlan { candidates, total_bytes })
}

fn migrated_manifest_path(repo_root: &Path) -> PathBuf {
    crate::portage::native_store_dir(repo_root).join("migrated.jsonl")
}

/// Load the migration record (empty vec if none yet).
pub fn migrated(repo_root: &Path) -> Result<Vec<MigratedEntry>, String> {
    let p = migrated_manifest_path(repo_root);
    let Ok(body) = fs::read_to_string(&p) else { return Ok(Vec::new()) };
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| format!("migrated.jsonl: {e}")))
        .collect()
}

/// Execute a plan: ingest every candidate into the store, append the
/// migration record, and (with cfg.untrack) `git rm --cached` + add a
/// `.gitignore` line per file so git stops re-adding them.
pub fn apply(
    repo: &GitCli,
    store: &ChunkStore,
    plan: &MigratePlan,
    cfg: &MigrateConfig,
) -> Result<ApplyReport, String> {
    let mut rep = ApplyReport::default();
    let manifest = migrated_manifest_path(repo.root());
    if let Some(dir) = manifest.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut mf = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&manifest)
        .map_err(|e| format!("open {}: {e}", manifest.display()))?;

    let mut ignore_lines = Vec::new();
    for cand in &plan.candidates {
        let abs = repo.root().join(&cand.path);
        let f = fs::File::open(&abs).map_err(|e| format!("open {}: {e}", cand.path))?;
        let ing = store.ingest(&cand.path, f)?;
        rep.files += 1;
        rep.bytes_ingested += ing.total_len;
        rep.bytes_stored += ing.new_bytes_stored;
        let entry = MigratedEntry {
            path: cand.path.clone(),
            blob_oid: ing.blob_oid.to_hex(),
            size: cand.size,
            migrated_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        writeln!(mf, "{}", serde_json::to_string(&entry).unwrap()).map_err(|e| e.to_string())?;

        if cfg.untrack {
            repo.git(&["rm", "--cached", "--quiet", "--", &cand.path])?;
            rep.untracked += 1;
            ignore_lines.push(format!("/{}", cand.path));
        }
    }

    if !ignore_lines.is_empty() {
        let gi = repo.root().join(".gitignore");
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gi)
            .map_err(|e| format!("open .gitignore: {e}"))?;
        writeln!(f, "\n# migrated to gitmind chunk store (gin migrate)").map_err(|e| e.to_string())?;
        for l in &ignore_lines {
            writeln!(f, "{l}").map_err(|e| e.to_string())?;
        }
    }
    Ok(rep)
}

/// Restore one migrated file's exact original bytes to its path (or to
/// `out` when given). The reversibility half of the contract.
pub fn restore_file(
    repo_root: &Path,
    store: &ChunkStore,
    path: &str,
    out: Option<&Path>,
) -> Result<u64, String> {
    let entries = migrated(repo_root)?;
    // Last entry wins: a re-migrated file's newest oid is the live one.
    let entry = entries
        .iter()
        .rev()
        .find(|e| e.path == path)
        .ok_or_else(|| format!("{path} is not in the migration record"))?;
    let oid = Oid::from_hex(&entry.blob_oid).ok_or("bad oid in migrated.jsonl")?;
    let dest = out.map(Path::to_path_buf).unwrap_or_else(|| repo_root.join(path));
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let f = fs::File::create(&dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    store.restore(oid, f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git {args:?} failed in {}", dir.display());
    }

    fn noise(n: usize, mut seed: u64) -> Vec<u8> {
        let mut v = Vec::with_capacity(n);
        while v.len() < n {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            v.extend_from_slice(&seed.to_le_bytes());
        }
        v.truncate(n);
        v
    }

    #[test]
    fn scan_apply_untrack_restore_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        git(root, &["config", "user.email", "t@t"]);
        git(root, &["config", "user.name", "t"]);

        // 2 MiB fake "video" (extension), 2 MiB noise (entropy),
        // 2 MiB of text (compressible → not a candidate), small png (skip).
        let clip = noise(2 << 20, 1);
        fs::write(root.join("clip.mp4"), &clip).unwrap();
        fs::write(root.join("blob.dat"), noise(2 << 20, 2)).unwrap();
        fs::write(root.join("log.txt"), "line\n".repeat(400_000)).unwrap();
        fs::write(root.join("icon.png"), vec![0u8; 100]).unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "seed"]);

        let repo = GitCli::discover(root).unwrap();
        let cfg = MigrateConfig { untrack: true, ..MigrateConfig::defaults() };
        let plan = scan(&repo, &cfg).unwrap();
        let names: Vec<&str> = plan.candidates.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.contains(&"clip.mp4") && names.contains(&"blob.dat"));

        let store =
            ChunkStore::open(root.join(".gitmind-test.redb")).unwrap();
        let rep = apply(&repo, &store, &plan, &cfg).unwrap();
        assert_eq!(rep.files, 2);
        assert_eq!(rep.untracked, 2);
        assert_eq!(rep.bytes_ingested, 2 * (2 << 20));
        // High-entropy content: the 'R' encoding keeps stored ≈ ingested
        // (within chunk tag overhead), never inflated.
        assert!(rep.bytes_stored <= rep.bytes_ingested + 1024);

        // git no longer tracks them; .gitignore shields re-adds.
        let tracked = repo.git(&["ls-files"]).unwrap();
        assert!(!tracked.contains("clip.mp4"));
        assert!(tracked.contains("log.txt"));
        let gi = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gi.contains("/clip.mp4"));

        // Reversibility: delete the worktree copy, restore exact bytes.
        fs::remove_file(root.join("clip.mp4")).unwrap();
        let n = restore_file(root, &store, "clip.mp4", None).unwrap();
        assert_eq!(n, clip.len() as u64);
        assert_eq!(fs::read(root.join("clip.mp4")).unwrap(), clip);

        // The record survives and lists both.
        assert_eq!(migrated(root).unwrap().len(), 2);
    }
}
