//! Pack signing. One ed25519 key per depot that PRODUCES packs; the depot that
//! CONSUMES them holds the producer's public key under `trust/`.
//!
//! Why this and not GPG: the artifacts inside a pack already carry the
//! signatures their consumers check (RPM GPG signatures, cargo checksums,
//! narinfo `Sig:`). What a pack signature protects is the CATALOG — "these
//! versions, these hashes, this repo membership" — against a tampered USB
//! stick between the online and offline sides. A single 32-byte key that
//! `crypto-kit` already implements covers that without a keyring, an agent,
//! or a trust-level UI. (It is the same construction nix uses for narinfo
//! `Sig:` lines: ed25519 over a canonical string, key named by a label.)
//!
//! Files:
//!
//! ```text
//! keys/depot.key     hex seed, mode 0600 — the producer's secret; never in a pack
//! keys/depot.pub     hex verifying key
//! trust/<name>.pub   verifying keys this depot accepts on unpack (any count)
//! ```

use crate::{hex, invalid, unhex, Error, Result};
use std::fs;
use std::path::Path;

pub const KEY_FILE: &str = "keys/depot.key";
pub const PUB_FILE: &str = "keys/depot.pub";
pub const TRUST_DIR: &str = "trust";

/// Generate a keypair under `<root>/keys/`. Refuses to overwrite: losing the
/// key that signed every pack in the field is not something to do by accident.
pub fn keygen(root: &Path) -> Result<String> {
    let key_path = root.join(KEY_FILE);
    if key_path.exists() {
        return Err(invalid(format!("{} exists; remove it deliberately first", key_path.display())));
    }
    let (sk, pk) = crypto_kit::ed25519_keypair();
    fs::create_dir_all(key_path.parent().unwrap())?;
    write_private(&key_path, &hex(&sk[..]))?;
    let pub_hex = hex(&pk);
    fs::write(root.join(PUB_FILE), format!("{pub_hex}\n"))?;
    Ok(pub_hex)
}

/// Create-once, 0600 (crate::create_private): the signing key is never overwritten.
fn write_private(path: &Path, contents: &str) -> Result<()> {
    Ok(crate::create_private(path, contents)?)
}

fn read_hex_key(path: &Path, len: usize) -> Result<Vec<u8>> {
    let s = fs::read_to_string(path)?;
    let line = s.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).ok_or_else(|| invalid(format!("{}: empty", path.display())))?;
    let b = unhex(line).ok_or_else(|| invalid(format!("{}: not hex", path.display())))?;
    if b.len() != len {
        return Err(invalid(format!("{}: expected {len} bytes, got {}", path.display(), b.len())));
    }
    Ok(b)
}

pub struct SigningKey {
    seed: zeroize::Zeroizing<[u8; 32]>,
    pub public_hex: String,
}

impl SigningKey {
    pub fn load(root: &Path) -> Result<SigningKey> {
        let seed = read_hex_key(&root.join(KEY_FILE), 32)?;
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&seed);
        let public_hex = fs::read_to_string(root.join(PUB_FILE)).ok().map(|s| s.trim().to_string()).unwrap_or_default();
        Ok(SigningKey { seed: zeroize::Zeroizing::new(arr), public_hex })
    }

    pub fn sign(&self, msg: &[u8]) -> String {
        hex(&crypto_kit::sign(&self.seed, msg))
    }
}

/// All `trust/*.pub` keys, as (file stem, key bytes).
pub fn trusted_keys(root: &Path) -> Result<Vec<(String, [u8; 32])>> {
    let dir = root.join(TRUST_DIR);
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut names: Vec<_> = fs::read_dir(&dir)?.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "pub").unwrap_or(false)).collect();
    names.sort();
    for p in names {
        let b = read_hex_key(&p, 32)?;
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&b);
        out.push((p.file_stem().unwrap().to_string_lossy().into_owned(), arr));
    }
    Ok(out)
}

/// Verify `sig_hex` over `msg` against any trusted key; returns the name of
/// the key that matched. An empty trust dir is an error, not a pass.
pub fn verify_trusted(root: &Path, msg: &[u8], sig_hex: &str) -> Result<String> {
    let keys = trusted_keys(root)?;
    if keys.is_empty() {
        return Err(Error::Untrusted(format!("no keys in {}/ — copy the producer's keys/depot.pub there", root.join(TRUST_DIR).display())));
    }
    let sig = unhex(sig_hex).filter(|s| s.len() == 64).ok_or_else(|| Error::Untrusted("signature is not 64 hex bytes".into()))?;
    let mut arr = [0u8; 64];
    arr.copy_from_slice(&sig);
    for (name, pk) in &keys {
        if crypto_kit::verify(pk, msg, &arr) {
            return Ok(name.clone());
        }
    }
    Err(Error::Untrusted(format!("signature matches none of the {} trusted key(s)", keys.len())))
}

/// Add a public key to `trust/` under `name`.
pub fn trust(root: &Path, name: &str, pub_hex: &str) -> Result<()> {
    if unhex(pub_hex.trim()).map(|b| b.len()) != Some(32) {
        return Err(invalid("public key must be 64 hex chars"));
    }
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return Err(invalid("key name must be a plain filename"));
    }
    fs::create_dir_all(root.join(TRUST_DIR))?;
    fs::write(root.join(TRUST_DIR).join(format!("{name}.pub")), format!("{}\n", pub_hex.trim()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmpdir;

    #[test]
    fn keygen_sign_trust_verify_and_refusals() {
        let a = tmpdir("sig-a");
        let b = tmpdir("sig-b");
        let pk = keygen(&a).unwrap();
        assert_eq!(pk.len(), 64);
        assert!(keygen(&a).is_err(), "refuses to overwrite");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(a.join(KEY_FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let sk = SigningKey::load(&a).unwrap();
        assert_eq!(sk.public_hex, pk);
        let sig = sk.sign(b"manifest bytes");
        assert!(matches!(verify_trusted(&b, b"manifest bytes", &sig), Err(Error::Untrusted(_))), "empty trust dir is a failure");
        trust(&b, "producer", &pk).unwrap();
        assert_eq!(verify_trusted(&b, b"manifest bytes", &sig).unwrap(), "producer");
        assert!(verify_trusted(&b, b"manifest bytez", &sig).is_err(), "tampered message");
        let mut bad = sig.clone();
        bad.replace_range(0..2, if &sig[0..2] == "00" { "01" } else { "00" });
        assert!(verify_trusted(&b, b"manifest bytes", &bad).is_err(), "tampered signature");
        assert!(trust(&b, "../x", &pk).is_err());
        assert!(trust(&b, "x", "zz").is_err());
        fs::remove_dir_all(a).unwrap();
        fs::remove_dir_all(b).unwrap();
    }
}
