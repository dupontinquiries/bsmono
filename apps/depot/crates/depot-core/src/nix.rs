//! Nix binary cache. This is the cleanest of the four projections, because a
//! binary cache IS static files over HTTP — nix was designed for it:
//!
//! ```text
//! nix-cache-info                  StoreDir/WantMassQuery/Priority
//! <32-char hash>.narinfo          one store path: its NAR's URL, hashes, refs, Sig
//! nar/<hash>.nar.xz               the serialized store path, compressed
//! ```
//!
//! On the online side, `nix copy --to file:///some/dir <paths>` writes exactly
//! this layout; `depot ingest nix /some/dir` lifts it into the store. On the
//! offline side, `substituters = http://depot.lan:8000/nix` in `nix.conf` (and
//! `trusted-public-keys` for the cache's signing key) makes every `nix build`
//! substitute from the LAN before trying cache.nixos.org (which fails fast).
//!
//! depot does not create NARs or sign narinfos — that needs a nix store and
//! nix's own key — it verifies that the NAR it was given matches the narinfo
//! (FileHash/FileSize) and serves both byte-for-byte. Trust is the `Sig:` line
//! the nix client checks; depot's own pack signature covers the catalog.

use crate::catalog::NixEntry;
use crate::{invalid, sha256_file, Result, Store};
use std::collections::BTreeMap;
use std::path::Path;

pub const NIX_BASE32: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";

/// Decode nix's base32 (reversed bit order, custom alphabet) into bytes.
/// `nbytes` is the expected length (32 for sha256).
pub fn nix_base32_decode(s: &str, nbytes: usize) -> Option<Vec<u8>> {
    let len = (nbytes * 8 + 4) / 5;
    if s.len() != len {
        return None;
    }
    let mut hash = vec![0u8; nbytes];
    for (n, c) in s.bytes().rev().enumerate() {
        let digit = NIX_BASE32.iter().position(|&x| x == c)? as u16;
        let b = n * 5;
        let i = b / 8;
        let j = b % 8;
        hash[i] |= (digit << j) as u8;
        if i < nbytes - 1 {
            hash[i + 1] |= (digit >> (8 - j)) as u8;
        } else if (digit >> (8 - j)) != 0 {
            return None; // trailing bits must be zero
        }
    }
    Some(hash)
}

pub fn nix_base32_encode(hash: &[u8]) -> String {
    let len = (hash.len() * 8 + 4) / 5;
    let mut s = String::with_capacity(len);
    for n in (0..len).rev() {
        let b = n * 5;
        let i = b / 8;
        let j = b % 8;
        let mut c = (hash[i] as u16) >> j;
        if i + 1 < hash.len() {
            c |= (hash[i + 1] as u16) << (8 - j);
        }
        s.push(NIX_BASE32[(c & 0x1f) as usize] as char);
    }
    s
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NarInfo {
    pub fields: BTreeMap<String, String>,
}

impl NarInfo {
    pub fn parse(text: &str) -> Result<NarInfo> {
        let mut fields = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            // "References:" with nothing after it is legal (a leaf path).
            let (k, v) = line.split_once(':').ok_or_else(|| invalid(format!("narinfo: bad line {line:?}")))?;
            fields.insert(k.trim().to_string(), v.trim().to_string());
        }
        for req in ["StorePath", "URL", "NarHash", "NarSize"] {
            if !fields.contains_key(req) {
                return Err(invalid(format!("narinfo: missing {req}")));
            }
        }
        Ok(NarInfo { fields })
    }

    pub fn get(&self, k: &str) -> &str {
        self.fields.get(k).map(String::as_str).unwrap_or("")
    }

    pub fn store_path(&self) -> &str {
        self.get("StorePath")
    }

    /// The 32-character hash part of the store path.
    pub fn hash_part(&self) -> Result<String> {
        let base = self.store_path().rsplit('/').next().unwrap_or("");
        let h = base.split('-').next().unwrap_or("");
        if h.len() != 32 || !h.bytes().all(|b| NIX_BASE32.contains(&b)) {
            return Err(invalid(format!("narinfo: StorePath {:?} has no 32-char hash part", self.store_path())));
        }
        Ok(h.to_string())
    }

    /// `FileHash: sha256:<nix-base32 | hex>` as lower-case hex, if present.
    pub fn file_hash_hex(&self) -> Option<String> {
        let v = self.fields.get("FileHash")?;
        let rest = v.strip_prefix("sha256:")?;
        if rest.len() == 64 && rest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(rest.to_ascii_lowercase());
        }
        nix_base32_decode(rest, 32).map(|b| crate::hex(&b))
    }

    pub fn file_size(&self) -> Option<u64> {
        self.fields.get("FileSize")?.parse().ok()
    }
}

pub fn cache_info(store_dir: &str, priority: u32) -> String {
    format!("StoreDir: {store_dir}\nWantMassQuery: 1\nPriority: {priority}\n")
}

/// What `ingest_dir` did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct IngestReport {
    pub paths: usize,
    pub new_nars: usize,
    pub errors: Vec<String>,
}

/// Lift a `nix copy --to file://DIR` directory into the store: every
/// `*.narinfo` plus the NAR it points at. Verifies FileHash/FileSize when the
/// narinfo carries them. Returns the entries to upsert into the catalog.
pub fn ingest_dir(store: &Store, dir: &Path) -> Result<(Vec<NixEntry>, IngestReport)> {
    let mut entries = Vec::new();
    let mut rep = IngestReport::default();
    let mut files: Vec<_> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "narinfo").unwrap_or(false)).collect();
    files.sort();
    for p in files {
        match ingest_one(store, dir, &p) {
            Ok((e, fresh)) => {
                rep.paths += 1;
                if fresh {
                    rep.new_nars += 1;
                }
                entries.push(e);
            }
            Err(err) => rep.errors.push(format!("{}: {err}", p.display())),
        }
    }
    Ok((entries, rep))
}

fn ingest_one(store: &Store, dir: &Path, narinfo_path: &Path) -> Result<(NixEntry, bool)> {
    let text = std::fs::read_to_string(narinfo_path)?;
    let ni = NarInfo::parse(&text)?;
    let hash_part = ni.hash_part()?;
    let url = ni.get("URL").to_string();
    if url.starts_with('/') || url.contains("..") || url.is_empty() {
        return Err(invalid(format!("narinfo URL {url:?} must be a relative path")));
    }
    let nar_path = dir.join(&url);
    let (nar_sha, nar_size) = sha256_file(&nar_path).map_err(|e| invalid(format!("NAR {}: {e}", nar_path.display())))?;
    if let Some(fh) = ni.file_hash_hex() {
        if fh != nar_sha {
            return Err(invalid(format!("NAR {} sha256 {nar_sha} != narinfo FileHash {fh}", nar_path.display())));
        }
    }
    if let Some(fs) = ni.file_size() {
        if fs != nar_size {
            return Err(invalid(format!("NAR {} size {nar_size} != narinfo FileSize {fs}", nar_path.display())));
        }
    }
    let nar = store.put_file(&nar_path, Some(&nar_sha))?;
    let info = store.put_bytes(text.as_bytes())?;
    Ok((
        NixEntry { store_path: ni.store_path().to_string(), hash_part, narinfo: info.hash, nar_hash: nar.hash, nar_url: url, nar_size, added: crate::now() },
        nar.fresh,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmpdir;

    #[test]
    fn nix_base32_roundtrips_and_matches_known_vector() {
        // sha256("") in nix's base32 — the value `nix hash to-base32` prints.
        let empty = crate::unhex("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855").unwrap();
        let enc = nix_base32_encode(&empty);
        assert_eq!(enc.len(), 52);
        assert_eq!(enc, "0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73");
        assert_eq!(nix_base32_decode(&enc, 32).unwrap(), empty);
        for i in 0..64u8 {
            let bytes: Vec<u8> = (0..32u8).map(|k| k.wrapping_mul(i).wrapping_add(7)).collect();
            assert_eq!(nix_base32_decode(&nix_base32_encode(&bytes), 32).unwrap(), bytes);
        }
        assert!(nix_base32_decode("short", 32).is_none());
        assert!(nix_base32_decode(&"e".repeat(52), 32).is_none(), "'e' is not in the alphabet");
    }

    #[test]
    fn narinfo_parse_and_hash_part() {
        let ni = NarInfo::parse("StorePath: /nix/store/0mdqa9w1p6cmli6976v4wi0sw9r4p5pr-hello-2.12\nURL: nar/abc.nar.xz\nCompression: xz\nFileHash: sha256:0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73\nFileSize: 0\nNarHash: sha256:x\nNarSize: 1\nReferences: \nSig: k:v\n").unwrap();
        assert_eq!(ni.hash_part().unwrap(), "0mdqa9w1p6cmli6976v4wi0sw9r4p5pr");
        assert_eq!(ni.file_hash_hex().unwrap(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(ni.file_size(), Some(0));
        assert!(NarInfo::parse("URL: x\n").is_err());
        assert!(cache_info("/nix/store", 30).contains("Priority: 30"));
    }

    #[test]
    fn ingest_dir_verifies_nar_against_narinfo() {
        let root = tmpdir("nix-root");
        let src = tmpdir("nix-src");
        let st = Store::open(&root);
        st.init().unwrap();
        std::fs::create_dir_all(src.join("nar")).unwrap();
        let nar = b"fake nar bytes";
        std::fs::write(src.join("nar/x.nar.xz"), nar).unwrap();
        let sha = crate::sha256_hex(nar);
        let good = format!("StorePath: /nix/store/{}-hello-1\nURL: nar/x.nar.xz\nFileHash: sha256:{sha}\nFileSize: {}\nNarHash: sha256:q\nNarSize: 5\n", "a".repeat(32), nar.len());
        std::fs::write(src.join("aaaa.narinfo"), &good).unwrap();
        let bad = format!("StorePath: /nix/store/{}-evil-1\nURL: nar/x.nar.xz\nFileHash: sha256:{}\nNarHash: sha256:q\nNarSize: 5\n", "b".repeat(32), "0".repeat(64));
        std::fs::write(src.join("bbbb.narinfo"), &bad).unwrap();
        let (entries, rep) = ingest_dir(&st, &src).unwrap();
        assert_eq!(entries.len(), 1, "the mismatching one is refused, not stored under a lie");
        assert_eq!(rep.paths, 1);
        assert_eq!(rep.new_nars, 1);
        assert_eq!(rep.errors.len(), 1);
        assert!(rep.errors[0].contains("FileHash"));
        assert_eq!(entries[0].nar_hash, sha);
        assert_eq!(entries[0].hash_part, "a".repeat(32));
        assert!(st.has(&entries[0].narinfo));
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(src).unwrap();
    }
}
