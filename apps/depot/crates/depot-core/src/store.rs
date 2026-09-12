//! The object store: `objects/<first two hex>/<sha256>`.
//!
//! The same layout git and nix converged on, for the same reasons: a flat
//! directory of 100k files makes `ls` and some filesystems miserable, two hex
//! characters of fan-out (256 buckets) is enough, and the name IS the
//! integrity check — a file whose contents do not hash to its name is corrupt
//! by definition, so `verify` needs no side table.
//!
//! Objects are written to `tmp/` on the same filesystem and renamed into place
//! (atomic, see `write_atomic`), then chmod 0444: the served tree hardlinks to
//! them, and a hardlink shares the inode, so a writable object could be
//! modified through any of its names. Read-only is how the store says
//! "immutable" to every other process on the box.
//!
//! Why hardlinks rather than symlinks for `pub/`: dnf, cargo and nix fetch
//! over HTTP and never see either, but the *server* does — a symlink is a
//! second lookup plus a chance to escape the root (depot-httpd's `safe_join`
//! canonicalizes for exactly that reason), while a hardlink is just another
//! directory entry for the same bytes. The one cost, "must be the same
//! filesystem", is already required by the atomic rename.

use crate::{invalid, is_sha256_hex, sha256_file, sha256_reader, Result};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub struct Store {
    root: PathBuf,
}

/// What `put` learned about the object it stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub hash: String,
    pub size: u64,
    /// false if an identical object was already present (dedup hit).
    pub fresh: bool,
}

impl Store {
    pub fn open(root: &Path) -> Store {
        Store { root: root.to_path_buf() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Create the directory skeleton. Idempotent.
    pub fn init(&self) -> io::Result<()> {
        for d in ["objects", "tmp", "pub", "keys", "trust"] {
            fs::create_dir_all(self.root.join(d))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // The signing key lives here; nobody else on the box needs to list it.
            let _ = fs::set_permissions(self.root.join("keys"), fs::Permissions::from_mode(0o700));
        }
        Ok(())
    }

    pub fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }
    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }
    pub fn pub_dir(&self) -> PathBuf {
        self.root.join("pub")
    }

    pub fn object_path(&self, hash: &str) -> PathBuf {
        self.objects_dir().join(&hash[..2]).join(hash)
    }

    pub fn has(&self, hash: &str) -> bool {
        is_sha256_hex(hash) && self.object_path(hash).is_file()
    }

    pub fn size_of(&self, hash: &str) -> Option<u64> {
        fs::metadata(self.object_path(hash)).ok().map(|m| m.len())
    }

    /// Store a file that already exists on disk. If `expect` is given and does
    /// not match, nothing is written. The source is COPIED, not moved: the
    /// caller's file may be on another filesystem or one they want to keep.
    pub fn put_file(&self, src: &Path, expect: Option<&str>) -> Result<Stored> {
        let (hash, size) = sha256_file(src)?;
        if let Some(e) = expect {
            if e != hash {
                return Err(invalid(format!("{}: sha256 {hash} != expected {e}", src.display())));
            }
        }
        if self.has(&hash) {
            return Ok(Stored { hash, size, fresh: false });
        }
        let tmp = self.stage_name(&hash);
        fs::copy(src, &tmp)?;
        self.commit(&tmp, &hash)?;
        Ok(Stored { hash, size, fresh: true })
    }

    /// Store from a reader (an HTTP upload, a pack entry), streaming to a temp
    /// file while hashing. Memory use is one 64 KiB buffer regardless of size.
    pub fn put_reader(&self, r: &mut dyn Read, expect: Option<&str>) -> Result<Stored> {
        fs::create_dir_all(self.tmp_dir())?;
        let tmp = self.tmp_dir().join(format!("in-{}-{}", std::process::id(), crate::now()));
        let (hash, size) = {
            let mut f = fs::File::create(&tmp)?;
            let mut tee = Tee { r, w: &mut f };
            let res = sha256_reader(&mut tee);
            drop(tee);
            f.sync_all()?;
            res?
        };
        if let Some(e) = expect {
            if e != hash {
                let _ = fs::remove_file(&tmp);
                return Err(invalid(format!("upload: sha256 {hash} != expected {e}")));
            }
        }
        if self.has(&hash) {
            let _ = fs::remove_file(&tmp);
            return Ok(Stored { hash, size, fresh: false });
        }
        self.commit(&tmp, &hash)?;
        Ok(Stored { hash, size, fresh: true })
    }

    pub fn put_bytes(&self, data: &[u8]) -> Result<Stored> {
        let mut r = data;
        self.put_reader(&mut r, None)
    }

    fn stage_name(&self, hash: &str) -> PathBuf {
        let _ = fs::create_dir_all(self.tmp_dir());
        self.tmp_dir().join(format!("{hash}-{}", std::process::id()))
    }

    fn commit(&self, tmp: &Path, hash: &str) -> Result<()> {
        let dest = self.object_path(hash);
        fs::create_dir_all(dest.parent().unwrap())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(tmp, fs::Permissions::from_mode(0o444))?;
        }
        match fs::rename(tmp, &dest) {
            Ok(()) => Ok(()),
            // A concurrent writer won the race; ours is a byte-identical duplicate.
            Err(_) if dest.is_file() => {
                let _ = fs::remove_file(tmp);
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Make `dest` a hardlink to the object (falls back to copy across
    /// filesystems). Replaces an existing `dest`. Creates parents.
    pub fn link_into(&self, hash: &str, dest: &Path) -> Result<()> {
        let src = self.object_path(hash);
        if !src.is_file() {
            return Err(invalid(format!("object {hash} missing")));
        }
        if let Some(p) = dest.parent() {
            fs::create_dir_all(p)?;
        }
        // Already the same inode? Nothing to do — keeps rebuilds cheap.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(a), Ok(b)) = (fs::metadata(&src), fs::metadata(dest)) {
                if a.ino() == b.ino() && a.dev() == b.dev() {
                    return Ok(());
                }
            }
        }
        let _ = fs::remove_file(dest);
        if fs::hard_link(&src, dest).is_err() {
            fs::copy(&src, dest)?;
        }
        Ok(())
    }

    /// Every object hash the store holds, sorted. This is the `have` list a
    /// pack is deduplicated against.
    pub fn list(&self) -> io::Result<Vec<String>> {
        let mut out = Vec::new();
        let od = self.objects_dir();
        if !od.is_dir() {
            return Ok(out);
        }
        for bucket in fs::read_dir(&od)?.flatten() {
            if !bucket.file_type()?.is_dir() {
                continue;
            }
            for obj in fs::read_dir(bucket.path())?.flatten() {
                let name = obj.file_name().to_string_lossy().into_owned();
                if is_sha256_hex(&name) {
                    out.push(name);
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// Re-hash every object; returns the hashes whose contents do not match
    /// their names. Empty vec = clean. Slow on purpose — it reads everything.
    pub fn verify(&self) -> io::Result<Vec<String>> {
        let mut bad = Vec::new();
        for h in self.list()? {
            let (got, _) = sha256_file(&self.object_path(&h))?;
            if got != h {
                bad.push(h);
            }
        }
        Ok(bad)
    }

    /// Total bytes in objects/.
    pub fn disk_usage(&self) -> io::Result<u64> {
        let mut n = 0;
        for h in self.list()? {
            n += fs::metadata(self.object_path(&h))?.len();
        }
        Ok(n)
    }
}

struct Tee<'a> {
    r: &'a mut dyn Read,
    w: &'a mut fs::File,
}
impl Read for Tee<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.r.read(buf)?;
        if n > 0 {
            io::Write::write_all(self.w, &buf[..n])?;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{sha256_hex, tmpdir};

    #[test]
    fn put_dedups_verifies_and_links() {
        let d = tmpdir("store");
        let st = Store::open(&d);
        st.init().unwrap();
        let a = st.put_bytes(b"hello").unwrap();
        assert!(a.fresh);
        assert_eq!(a.hash, sha256_hex(b"hello"));
        assert_eq!(a.size, 5);
        let b = st.put_bytes(b"hello").unwrap();
        assert!(!b.fresh, "second put of identical bytes is a dedup hit");
        assert!(st.has(&a.hash));
        assert!(!st.has("not-a-hash"));

        let src = d.join("src.bin");
        fs::write(&src, b"world").unwrap();
        assert!(st.put_file(&src, Some(&sha256_hex(b"WRONG"))).is_err(), "expected-hash mismatch refuses");
        assert!(!st.has(&sha256_hex(b"world")), "nothing written on mismatch");
        let c = st.put_file(&src, Some(&sha256_hex(b"world"))).unwrap();
        assert!(c.fresh);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(st.object_path(&a.hash)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o444, "objects are immutable");
        }

        let dest = d.join("pub/x/y/hello.txt");
        st.link_into(&a.hash, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(fs::metadata(&dest).unwrap().ino(), fs::metadata(st.object_path(&a.hash)).unwrap().ino(), "hardlink, not copy");
        }
        st.link_into(&a.hash, &dest).unwrap(); // idempotent
        assert!(st.link_into(&sha256_hex(b"absent"), &d.join("pub/z")).is_err());

        let mut l = st.list().unwrap();
        l.sort();
        let mut want = vec![a.hash.clone(), c.hash.clone()];
        want.sort();
        assert_eq!(l, want);
        assert_eq!(st.verify().unwrap(), Vec::<String>::new());
        assert_eq!(st.disk_usage().unwrap(), 10);

        // Corrupt one in place (chmod back) and verify must name it.
        let p = st.object_path(&a.hash);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        }
        fs::write(&p, b"hellO").unwrap();
        assert_eq!(st.verify().unwrap(), vec![a.hash.clone()]);
        assert!(fs::read_dir(st.tmp_dir()).unwrap().count() == 0, "no staging leftovers");
        fs::remove_dir_all(d).unwrap();
    }
}
