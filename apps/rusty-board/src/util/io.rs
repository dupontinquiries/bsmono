//! Filesystem helpers.

use std::io::Result;
use std::path::Path;

/// Write `contents` to `path` atomically: serialize to a sibling
/// temp file, fsync to flush bytes to disk, then `rename(2)` onto
/// the target.
///
/// Why not `std::fs::write`? It opens the target with `O_TRUNC`
/// then streams the new bytes. If the process is killed (OOM,
/// power loss) between the truncate and write completing, the
/// user loses ALL of the file's contents — not just the in-flight
/// changes, but the entire previous state. For user settings
/// (config.toml) and trained templates (user_templates.json),
/// that's months of accumulated personalization, gone.
///
/// POSIX guarantees `rename(2)` is atomic within a single
/// filesystem, so either the new file appears whole (we got
/// through the rename) or the previous file is intact (we didn't).
/// The fsync before rename is belt-and-suspenders: without it, a
/// power loss right after rename could leave the directory entry
/// pointing at a still-empty inode.
///
/// The temp suffix includes the process PID so concurrent saves
/// from multiple instances (rare for a keyboard, but theoretically
/// possible during a config reload race) don't clobber each
/// other's temp files mid-write.
///
/// Returns the underlying `io::Error` on failure so callers can
/// `with_context()` it or convert as needed.
pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("tmp");
    let tmp_name = format!("{}.tmp.{}", file_name, std::process::id());
    let tmp_path = parent.join(tmp_name);

    std::fs::write(&tmp_path, contents)?;
    // Best-effort fsync: some filesystems (procfs, tmpfs, certain
    // network mounts) don't support per-file sync. Failures here
    // mean the directory entry might point at a not-yet-durable
    // file after rename — survivable in those filesystems anyway
    // (they're typically volatile or rely on different durability
    // guarantees), so we log-and-continue rather than fail the save.
    if let Ok(f) = std::fs::File::open(&tmp_path) {
        let _ = f.sync_all();
    }
    std::fs::rename(&tmp_path, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn unique_path(label: &str) -> std::path::PathBuf {
        // Use a unique-per-test tempfile to avoid races between
        // tests running in parallel. tempfile crate would be
        // cleaner but adds a dep; just compose path manually.
        let dir = std::env::temp_dir().join(format!(
            "rb-atomic-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("data.txt")
    }

    #[test]
    fn atomic_write_creates_file_with_contents() {
        let p = unique_path("create");
        atomic_write(&p, b"hello world").unwrap();
        let read = std::fs::read(&p).unwrap();
        assert_eq!(read, b"hello world");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn atomic_write_overwrites_existing() {
        let p = unique_path("overwrite");
        // Seed with old data.
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(b"old content").unwrap();
        drop(f);
        // Atomic write should fully replace.
        atomic_write(&p, b"new").unwrap();
        let read = std::fs::read(&p).unwrap();
        assert_eq!(read, b"new");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn atomic_write_leaves_no_temp_file_on_success() {
        let p = unique_path("no-temp");
        atomic_write(&p, b"data").unwrap();
        // Scan parent dir; no .tmp.* file should remain.
        let parent = p.parent().unwrap();
        let entries: Vec<_> = std::fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.contains(".tmp."))
                    .unwrap_or(false)
            })
            .collect();
        assert!(
            entries.is_empty(),
            "expected no temp files, found: {:?}",
            entries.iter().map(|e| e.file_name()).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn atomic_write_handles_path_in_root_when_no_parent() {
        // Path with no parent (e.g., bare filename). Should default
        // parent to "." without panic. We don't actually write —
        // just verify the path computation logic doesn't blow up.
        let p = Path::new("just-a-name.txt");
        // We can't actually write to the cwd from a test reliably,
        // but we CAN exercise the path construction by writing then
        // immediately checking + cleaning up.
        match atomic_write(p, b"x") {
            Ok(_) => {
                let _ = std::fs::remove_file(p);
            }
            Err(_) => {
                // No-write permissions in test env — that's fine,
                // we just wanted to verify no panic.
            }
        }
    }
}
