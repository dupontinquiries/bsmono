// gitmind-core/src/remote_folder.rs
//
// The first `RemoteTarget`: a plain directory.
//
// ## Why this one
//
// `remote.rs` defines the backup seam — `missing`, `put_chunk`,
// `get_chunk`, `put_ref`, `list_refs` — and has NO implementations. Not
// one, anywhere in the tree. `BACKUP_SYNC.md` describes the whole design
// as if it works, and it cannot, because nothing can hold bytes.
//
// A folder is the right first target for three reasons:
//
// * `Protection::Plain` already names it as the intended cheapest case:
//   "local backup folder on hardware you control ... fine for
//   same-machine rollback".
// * It is the one target testable without a network, a peer, or a cloud
//   account, so its error paths can actually be exercised here rather
//   than shipped unrun.
// * It covers the requested-backup-folder feature the doc leads with.
//
// ## Layout
//
// ```text
//   <root>/chunks/ab/cdef…      one file per chunk, sharded by first byte
//   <root>/refs/<name>          one file per ref, contents = target oid
// ```
//
// Sharded because a flat directory of a hundred thousand chunk files is
// slow to stat on every filesystem that matters and unusable in a file
// manager. Two hex chars gives 256 buckets, which keeps the common case
// (a few thousand chunks) at tens of files per bucket.
//
// Refs are separate files rather than one index, because a ref update is
// then atomic per ref and two concurrent backups cannot lose each
// other's — an index would need locking to get the same property.

use crate::oid::Oid;
use crate::remote::RemoteTarget;
use std::path::{Path, PathBuf};

pub struct FolderTarget {
    id: String,
    root: PathBuf,
}

impl FolderTarget {
    /// Open (and create) a backup folder.
    ///
    /// Creating rather than requiring it to exist: "point at an empty
    /// disk" is the ordinary way to start a backup, and failing on a
    /// missing directory would make the first run the awkward one.
    pub fn new(id: impl Into<String>, root: impl Into<PathBuf>) -> Result<Self, String> {
        let root = root.into();
        std::fs::create_dir_all(root.join("chunks"))
            .map_err(|e| format!("backup folder {}: {e}", root.display()))?;
        std::fs::create_dir_all(root.join("refs"))
            .map_err(|e| format!("backup folder {}: {e}", root.display()))?;
        Ok(FolderTarget { id: id.into(), root })
    }

    fn chunk_path(&self, oid: Oid) -> PathBuf {
        let hex = hex_of(oid);
        self.root.join("chunks").join(&hex[..2]).join(&hex[2..])
    }

    fn ref_path(&self, name: &str) -> Result<PathBuf, String> {
        // A ref name is used as a FILENAME, so `../` in it would write
        // outside the backup folder. Rejected rather than sanitised:
        // silently rewriting a name means the ref a caller stored is not
        // the ref it can read back.
        if name.is_empty()
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            || name.starts_with('.')
        {
            return Err(format!("ref name {name:?} must be [A-Za-z0-9._-] and not start with '.'"));
        }
        Ok(self.root.join("refs").join(name))
    }
}

fn hex_of(oid: Oid) -> String {
    oid.0.iter().map(|b| format!("{b:02x}")).collect()
}

/// Write a file so a reader never sees a partial one.
///
/// Temp file in the SAME directory then rename: rename is atomic within
/// a filesystem, and a temp file elsewhere (`/tmp`) could be on a
/// different one, where the rename degrades to a copy and loses the
/// property this exists for.
///
/// It matters here because a backup is exactly what gets read after a
/// crash, and a half-written chunk that hashes wrong is worse than an
/// absent one — the absent one is re-pushed, the corrupt one is trusted.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

impl RemoteTarget for FolderTarget {
    fn id(&self) -> &str {
        &self.id
    }

    fn missing(&self, want: &[Oid]) -> Result<Vec<Oid>, String> {
        // A stat per chunk. Not batched, because the alternative is
        // listing the whole store to build a set — which is slower once
        // the backup is larger than the push, and that is the normal
        // case for every backup after the first.
        Ok(want.iter().copied().filter(|o| !self.chunk_path(*o).exists()).collect())
    }

    fn put_chunk(&self, oid: Oid, bytes: &[u8]) -> Result<(), String> {
        let p = self.chunk_path(oid);
        // Idempotent, as the trait requires: an existing chunk is a
        // no-op so retries are safe. Content-addressed, so a present
        // file with this name IS this chunk — rewriting it would spend
        // IO to produce the same bytes.
        if p.exists() {
            return Ok(());
        }
        write_atomic(&p, bytes)
    }

    fn get_chunk(&self, oid: Oid) -> Result<Vec<u8>, String> {
        let p = self.chunk_path(oid);
        std::fs::read(&p).map_err(|e| format!("chunk {}: {e}", hex_of(oid)))
            .map_err(|e| format!("{e} ({})", p.display()))
    }

    fn put_ref(&self, name: &str, target: &str) -> Result<(), String> {
        let p = self.ref_path(name)?;
        write_atomic(&p, target.as_bytes())
    }

    fn list_refs(&self) -> Result<Vec<(String, String)>, String> {
        let dir = self.root.join("refs");
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            // No refs directory is an empty list, not an error: a
            // freshly created target legitimately has none, and a
            // restore should say "nothing to restore" rather than fail.
            Err(_) => return Ok(Vec::new()),
        };
        let mut out = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // Skip our own temp files: a concurrent push would otherwise
            // list a half-written ref as a real one.
            if name.contains(".tmp") {
                continue;
            }
            if let Ok(t) = std::fs::read_to_string(e.path()) {
                out.push((name, t.trim().to_string()));
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(n: u8) -> Oid {
        Oid([n; 32])
    }

    fn target(tag: &str) -> (FolderTarget, PathBuf) {
        let dir = std::env::temp_dir()
            .join(format!("gm-folder-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (FolderTarget::new("test", &dir).expect("created"), dir)
    }

    #[test]
    fn a_chunk_round_trips() {
        let (t, dir) = target("round");
        t.put_chunk(oid(1), b"hello").unwrap();
        assert_eq!(t.get_chunk(oid(1)).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_reports_only_what_is_absent() {
        // This is the dedup win the trait doc names: a backup pushes the
        // delta, so a wrong answer here either re-pushes everything or
        // silently skips a chunk that was never stored.
        let (t, dir) = target("missing");
        t.put_chunk(oid(1), b"a").unwrap();
        let m = t.missing(&[oid(1), oid(2), oid(3)]).unwrap();
        assert_eq!(m, vec![oid(2), oid(3)]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pushing_an_existing_chunk_is_a_no_op_not_an_error() {
        // "Idempotent: pushing an existing chunk is a no-op so retries
        // are safe" — the trait says so, and a retry after a network
        // wobble is the normal way this gets exercised.
        let (t, dir) = target("idem");
        t.put_chunk(oid(7), b"once").unwrap();
        t.put_chunk(oid(7), b"once").unwrap();
        assert_eq!(t.get_chunk(oid(7)).unwrap(), b"once");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_ref_name_cannot_escape_the_backup_folder() {
        // The name becomes a FILENAME, so `../` would write outside the
        // folder — into whatever the backup root's parent happens to be.
        let (t, dir) = target("escape");
        for bad in ["../evil", "a/b", "", ".hidden", "with space"] {
            assert!(t.put_ref(bad, "deadbeef").is_err(), "accepted {bad:?}");
        }
        // And a legitimate name still works: a guard that blocks real
        // input gets removed rather than fixed.
        t.put_ref("refs-checkpoints-main", "deadbeef").unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refs_round_trip_and_list_sorted() {
        let (t, dir) = target("refs");
        t.put_ref("main", "aaa").unwrap();
        t.put_ref("backup", "bbb").unwrap();
        assert_eq!(
            t.list_refs().unwrap(),
            vec![("backup".to_string(), "bbb".to_string()),
                 ("main".to_string(), "aaa".to_string())]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_target_with_no_refs_lists_empty_rather_than_failing() {
        // A freshly created backup legitimately has none, and a restore
        // should say "nothing to restore" rather than error.
        let (t, dir) = target("empty");
        assert!(t.list_refs().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn chunks_are_sharded_so_one_directory_never_holds_them_all() {
        // A flat directory of a hundred thousand chunk files is slow to
        // stat everywhere that matters and unusable in a file manager.
        let (t, dir) = target("shard");
        t.put_chunk(oid(0xab), b"x").unwrap();
        assert!(dir.join("chunks").join("ab").exists(), "not sharded by first byte");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_chunk_says_which_one() {
        // A restore that fails with "No such file" and no oid cannot be
        // acted on: the whole point is knowing WHICH chunk to re-fetch.
        let (t, dir) = target("absent");
        let e = t.get_chunk(oid(9)).unwrap_err();
        assert!(e.contains(&hex_of(oid(9))[..8]), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
