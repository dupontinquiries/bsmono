// gitmind-core/src/ingest_zip.rs
//
// The dry-run behind "batch unzip -o into a summary". Before extracting an
// archive (an AI coding-session export, a downloaded library), we compute
// exactly what an overwriting extract WOULD do — without touching disk —
// and recognize the metadata that matters for a wizard/review:
//
//   ADDED      path not present in the target → safe new file
//   MODIFIED   path present AND content differs → a real overwrite
//   UNCHANGED  path present AND content identical → a no-op overwrite
//
// "Content differs" is decided by BLAKE3, not size/mtime, so an `unzip -o`
// that would rewrite a file with identical bytes is correctly reported as a
// no-op (the common case that makes naive overwrite summaries noisy). apply
// then performs the extraction, and the plan IS the summary the GUI shows.

use crate::oid::Oid;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Overwrite {
    pub path: String,
    pub old_size: u64,
    pub new_size: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnzipPlan {
    pub added: Vec<String>,
    pub modified: Vec<Overwrite>,
    pub unchanged: Vec<String>,
    pub total_entries: usize,
    pub total_uncompressed: u64,
}

impl UnzipPlan {
    /// One-line summary for a wizard header.
    pub fn summary(&self) -> String {
        format!(
            "{} entries: {} new, {} overwrite, {} unchanged ({:.1} MB)",
            self.total_entries,
            self.added.len(),
            self.modified.len(),
            self.unchanged.len(),
            self.total_uncompressed as f64 / 1.0e6
        )
    }

    /// True when nothing on disk would actually change (every overwrite is
    /// a no-op). Lets the wizard skip a pointless extract.
    pub fn is_noop(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty()
    }
}

/// Compute the plan for extracting `zip_path` over `target_dir`. Reads the
/// archive and compares each file entry against the target by content hash.
pub fn plan_unzip(zip_path: &Path, target_dir: &Path) -> Result<UnzipPlan, String> {
    let f = std::fs::File::open(zip_path).map_err(|e| format!("open zip: {e}"))?;
    let mut ar = zip::ZipArchive::new(f).map_err(|e| format!("read zip: {e}"))?;
    let mut plan = UnzipPlan::default();

    for i in 0..ar.len() {
        let mut entry = ar.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        // Use the sanitized path the zip crate derives, guarding against
        // zip-slip (../ escapes) — entries without a safe name are skipped.
        let rel = match entry.enclosed_name() {
            Some(p) => p,
            None => continue,
        };
        let name = rel.to_string_lossy().to_string();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        plan.total_entries += 1;
        plan.total_uncompressed += buf.len() as u64;

        let target: PathBuf = target_dir.join(&rel);
        if !target.exists() {
            plan.added.push(name);
            continue;
        }
        // Exists → compare content by hash to tell overwrite from no-op.
        let cur = std::fs::read(&target).map_err(|e| e.to_string())?;
        if Oid::hash(&cur) == Oid::hash(&buf) {
            plan.unchanged.push(name);
        } else {
            plan.modified.push(Overwrite {
                path: name,
                old_size: cur.len() as u64,
                new_size: buf.len() as u64,
            });
        }
    }
    Ok(plan)
}

/// Extract the archive over `target_dir` (the `unzip -o` the plan described).
/// Returns how many files were written. zip-slip-safe via enclosed_name.
pub fn apply_unzip(zip_path: &Path, target_dir: &Path) -> Result<usize, String> {
    let f = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut ar = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    let mut written = 0;
    for i in 0..ar.len() {
        let mut entry = ar.by_index(i).map_err(|e| e.to_string())?;
        let rel = match entry.enclosed_name() {
            Some(p) => p,
            None => continue,
        };
        let out = target_dir.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        std::fs::write(&out, &buf).map_err(|e| e.to_string())?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        for (name, data) in files {
            zw.start_file(*name, SimpleFileOptions::default()).unwrap();
            zw.write_all(data).unwrap();
        }
        zw.finish().unwrap();
    }

    #[test]
    fn plan_recognizes_added_modified_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("work");
        std::fs::create_dir_all(&target).unwrap();
        // Pre-existing files: one identical to the zip, one that differs.
        std::fs::write(target.join("same.txt"), b"identical").unwrap();
        std::fs::write(target.join("diff.txt"), b"old version").unwrap();

        let zip_path = dir.path().join("a.zip");
        make_zip(&zip_path, &[
            ("same.txt", b"identical"),      // unchanged
            ("diff.txt", b"new version!!!"), // modified
            ("fresh.txt", b"brand new"),     // added
        ]);

        let plan = plan_unzip(&zip_path, &target).unwrap();
        assert_eq!(plan.total_entries, 3);
        assert_eq!(plan.added, vec!["fresh.txt"]);
        assert_eq!(plan.unchanged, vec!["same.txt"]);
        assert_eq!(plan.modified.len(), 1);
        assert_eq!(plan.modified[0].path, "diff.txt");
        assert!(!plan.is_noop());
        assert!(plan.summary().contains("1 new"));
    }

    #[test]
    fn identical_archive_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("w");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("x"), b"hello").unwrap();
        let zip_path = dir.path().join("b.zip");
        make_zip(&zip_path, &[("x", b"hello")]);
        let plan = plan_unzip(&zip_path, &target).unwrap();
        assert!(plan.is_noop(), "re-extracting identical content changes nothing");
    }

    #[test]
    fn apply_writes_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out");
        let zip_path = dir.path().join("c.zip");
        make_zip(&zip_path, &[("sub/file.txt", b"data")]);
        let n = apply_unzip(&zip_path, &target).unwrap();
        assert_eq!(n, 1);
        assert_eq!(std::fs::read(target.join("sub/file.txt")).unwrap(), b"data");
    }
}
