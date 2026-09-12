//! depot-core — the headless mirror.
//!
//! One object store, four projections, one sync format:
//!
//! ```text
//! <root>/
//!   depot.toml         config (see config.rs)
//!   catalog.json       everything the store knows, by kind (catalog.rs)
//!   objects/ab/ab…     content-addressed blobs, sha256, immutable (store.rs)
//!   keys/, trust/      ed25519 signing key / accepted public keys (sig.rs)
//!   pub/               the served tree — HARDLINKS into objects/ (views.rs)
//!     crates/          cargo sparse registry               (crates.rs)
//!     rpm/<repo>/      dnf repository with repodata/       (rpm.rs)
//!     nix/             nix binary cache                    (nix.rs)
//!     toolchains/      rust toolchain bundles              (toolchain.rs)
//!   tmp/               staging (same filesystem, so rename is atomic)
//! ```
//!
//! A pack (pack.rs) is a tar.gz of `manifest.json` + `manifest.sig` + the
//! objects the receiver does not already have. Every artifact is verified by
//! sha256 on the way in and on the way out of a pack; the manifest is signed.
//!
//! Nothing in here opens a socket or spawns a thread. The CLI and the web shell
//! (crates/depot) are the only callers, which is what keeps this testable with
//! a temp dir and a handful of reference files.

pub mod catalog;
pub mod config;
pub mod crates;
pub mod depot;
pub mod nix;
pub mod pack;
pub mod rpm;
pub mod sig;
pub mod store;
pub mod toolchain;
pub mod views;

use std::fmt;
use std::io::{self, Read, Write};
use std::path::Path;

pub use catalog::Catalog;
pub use depot::Depot;
pub use store::Store;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// Something about the artifact itself is wrong (bad magic, missing tag,
    /// checksum mismatch). The message names the file and the reason.
    Invalid(String),
    /// A signature or trust failure. Never downgraded to a warning.
    Untrusted(String),
    Json(serde_json::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Invalid(m) => write!(f, "invalid: {m}"),
            Error::Untrusted(m) => write!(f, "untrusted: {m}"),
            Error::Json(e) => write!(f, "json: {e}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}

// ─────────────────────────────────────────────────────────────────────────────
// Hashing
// ─────────────────────────────────────────────────────────────────────────────

/// Lower-case hex sha256. sha256 is not a choice: it is what cargo's index
/// `cksum`, dnf's `checksum type="sha256"` and nix's `FileHash: sha256:` all
/// carry, so one hash serves as the object id AND the artifact's own checksum.
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(data))
}

/// Hash a reader in 64 KiB chunks; returns (hex, bytes read).
pub fn sha256_reader(mut r: impl Read) -> io::Result<(String, u64)> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut n = 0u64;
    loop {
        let k = r.read(&mut buf)?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((hex(&h.finalize()), n))
}

pub fn sha256_file(path: &Path) -> io::Result<(String, u64)> {
    sha256_reader(std::fs::File::open(path)?)
}

pub fn hex(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

// ─────────────────────────────────────────────────────────────────────────────
// Atomic file writes
// ─────────────────────────────────────────────────────────────────────────────

/// Write `data` to `path` via a sibling temp file + `rename`. rename(2) is
/// atomic on POSIX within one filesystem, so a reader (or a crash) sees either
/// the old file or the new one, never a torn one. The temp file is fsync'd
/// first, because rename only orders directory metadata — without the fsync
/// the new name can point at zero bytes after a power cut (the classic ext4
/// data=ordered surprise). Used for catalog.json, index files, repomd.xml.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.tmp-{}", path.file_name().and_then(|n| n.to_str()).unwrap_or("f"), std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Current unix time in seconds; the one place the clock is read, so tests can
/// reason about it and an offline box with a wrong clock does not corrupt
/// anything (timestamps are informational only in depot).
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
pub(crate) fn tmpdir(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("depot-core-{tag}-{}-{}", std::process::id(), now()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector_and_hex_roundtrips() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        let (h, n) = sha256_reader(&b"abc"[..]).unwrap();
        assert_eq!((h.as_str(), n), ("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", 3));
        assert_eq!(unhex("00ff10").unwrap(), vec![0, 255, 16]);
        assert_eq!(unhex("0"), None);
        assert!(is_sha256_hex(&sha256_hex(b"x")));
        assert!(!is_sha256_hex("BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"));
    }

    #[test]
    fn write_atomic_replaces_and_leaves_no_temp() {
        let d = tmpdir("atomic");
        let p = d.join("sub/f.txt");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let leftovers: Vec<_> = std::fs::read_dir(d.join("sub")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(leftovers.len(), 1);
        std::fs::remove_dir_all(d).unwrap();
    }
}
