//! # cleanup — remove bulky encode intermediates after success
//!
//! After a clip's final output is produced and verified, the per-chunk
//! intermediate files (.ivf/.266 chunk outputs, chunk-state JSON, the
//! combined.266 temp, concat lists) are no longer needed for THAT clip.
//! Removing them keeps the workspace tidy.
//!
//! This is toggleable: ON by default (tidy), but the batch runner should
//! turn it OFF for large batches so a crash leaves completed chunks on
//! disk for fast resume. The cache manifest (in the config dir) is
//! NEVER touched by cleanup — completed clips still get cache hits even
//! after their intermediates are gone, because the manifest points at
//! the FINAL output, not the intermediates.

use std::path::Path;

/// What got cleaned, for logging.
#[derive(Debug, Default)]
pub struct CleanupReport {
    pub files_removed: usize,
    pub bytes_freed: u64,
}

/// Remove intermediate files from a job's state directory, keeping the
/// directory itself (and anything we don't recognize as an intermediate).
/// Recognized intermediates: chunk_*.ivf, chunk_*.266, chunk_*.json,
/// combined.266, concat_list.txt, merge_list.txt, chapters.txt.
///
/// Deliberately conservative: only deletes files matching known
/// intermediate patterns, never the final output (which lives outside
/// the state dir) and never unknown files.
pub fn clean_state_dir(state_dir: &Path) -> std::io::Result<CleanupReport> {
    let mut report = CleanupReport::default();
    if !state_dir.is_dir() {
        return Ok(report);
    }
    for entry in std::fs::read_dir(state_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if is_intermediate(name) {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if std::fs::remove_file(&path).is_ok() {
                report.files_removed += 1;
                report.bytes_freed += size;
            }
        }
    }
    Ok(report)
}

/// Whether a filename is a recognized encode intermediate.
fn is_intermediate(name: &str) -> bool {
    // Chunk outputs + state.
    if name.starts_with("chunk_") {
        return name.ends_with(".ivf")
            || name.ends_with(".266")
            || name.ends_with(".json");
    }
    // Concat/merge scratch.
    matches!(name,
        "combined.266" | "concat_list.txt" | "merge_list.txt" 
        | "chapters.txt" | "audio.opus" | "combined_timed.mp4")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn touch(dir: &Path, name: &str, bytes: usize) {
        let mut f = std::fs::File::create(dir.join(name)).unwrap();
        f.write_all(&vec![0u8; bytes]).unwrap();
    }

    #[test]
    fn recognizes_intermediates() {
        assert!(is_intermediate("chunk_0001.ivf"));
        assert!(is_intermediate("chunk_0042.266"));
        assert!(is_intermediate("chunk_0007.json"));
        assert!(is_intermediate("combined.266"));
        assert!(is_intermediate("concat_list.txt"));
        assert!(is_intermediate("chapters.txt"));
        assert!(is_intermediate("audio.opus"));
        // NOT intermediates:
        assert!(!is_intermediate("myvideo.av1.mkv"));
        assert!(!is_intermediate("job.json"));
        assert!(!is_intermediate("notes.txt"));
    }

    #[test]
    fn cleans_only_intermediates() {
        let dir = std::env::temp_dir().join(format!("cleanup_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        touch(&dir, "chunk_0001.ivf", 1000);
        touch(&dir, "chunk_0002.ivf", 1000);
        touch(&dir, "combined.266", 500);
        touch(&dir, "job.json", 100);          // should survive
        touch(&dir, "important.mkv", 9999);     // should survive

        let report = clean_state_dir(&dir).unwrap();
        assert_eq!(report.files_removed, 3);
        assert_eq!(report.bytes_freed, 2500);
        // Survivors still present.
        assert!(dir.join("job.json").exists());
        assert!(dir.join("important.mkv").exists());
        // Intermediates gone.
        assert!(!dir.join("chunk_0001.ivf").exists());
        assert!(!dir.join("combined.266").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
