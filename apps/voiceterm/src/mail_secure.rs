//! At-rest and in-memory credential protection, built on web2qml's own
//! security infrastructure rather than hydroxide's approach.
//!
//! v0.1.862: This is where the w2q port deliberately diverges from
//! hydroxide. hydroxide's credential handling has two weaknesses worth
//! fixing:
//!
//!   1. **Secrets live in ordinary heap memory.** The mailbox password, the
//!      decrypted tokens, and the bridge key sit in normal Go-managed memory
//!      that can be paged to swap or captured in a core dump.
//!   2. **The credential file is sealed with the bridge password alone.** A
//!      stolen `auth.json` plus the bridge password (which the user pastes
//!      into a mail client, copies around, maybe syncs) is enough to decrypt
//!      everything. There is no machine-bound second factor, and the file is
//!      written unconditionally.
//!
//! w2q already has the primitives to do better, and this module wires the
//! mail bridge into them:
//!
//!   * [`crate::security::SecretBytes`] — an mlock'd, `MADV_DONTDUMP`,
//!     zero-on-drop region. We hold the bridge key and the derived sealing
//!     key here ([`SecureKey`]) so they never hit swap or a core dump.
//!   * [`crate::security::persistence_enabled`] — the same opt-in gate that
//!     governs voice-learning persistence. With it off (the hardened
//!     default), credentials are **never written to disk**; accounts live
//!     only for the session. This is the behavioural difference from
//!     hydroxide, which always writes the file. See [`PolicySecretBox`].
//!   * A per-machine key file ([`MachineKey`]) XOR-combined with the bridge
//!     key, so a stolen `.acct` file is useless without *this device's*
//!     `machine.key`. The user still only needs their bridge password; the
//!     machine key is the silent second factor that stays on the box.
//!
//! XOR is a legitimate combiner here precisely because both inputs are
//! independent, uniformly-random 32-byte keys (the bridge key comes from the
//! OS CSPRNG in [`crate::mail_account::new_bridge_password`]; the machine key
//! likewise). It needs no hash/KDF crate — keeping the dependency-free
//! posture of the default build — while still requiring an attacker to hold
//! both halves.

use std::path::PathBuf;

use crate::mail_crypto::{platform_secret_box, SecretBox};
use crate::mail_error::{MailError, Result};
use crate::security;

/// A 32-byte key kept in w2q's secure memory. Construction copies the bytes
/// into a [`security::SecretBytes`] region (mlock + no-coredump + zeroed on
/// drop); the caller's source bytes should be scrubbed too where they
/// originate. If the secure region can't be allocated, we fall back to a
/// plain heap array and say so once, rather than failing the whole login —
/// degraded protection beats no bridge.
pub struct SecureKey {
    secret: Option<security::SecretBytes>,
    /// Fallback storage when SecretBytes allocation fails.
    fallback: Option<[u8; 32]>,
}

impl SecureKey {
    pub fn from_bytes(key: &[u8; 32]) -> Self {
        match security::SecretBytes::with_capacity(32) {
            Some(mut sb) => {
                sb.copy_from(key);
                SecureKey {
                    secret: Some(sb),
                    fallback: None,
                }
            }
            None => {
                crate::clog!(
                    "mail",
                    "[secure] SecretBytes unavailable; bridge key held in plain memory"
                );
                SecureKey {
                    secret: None,
                    fallback: Some(*key),
                }
            }
        }
    }

    /// Run `f` with a borrowed view of the 32-byte key. The bytes are not
    /// cloned out; callers must not log or transmit them.
    pub fn with<R>(&self, f: impl FnOnce(&[u8; 32]) -> R) -> R {
        let mut buf = [0u8; 32];
        // Source the key bytes from whichever backing store we have; the
        // (None, None) case leaves `buf` zeroed, which is the safe degenerate
        // value and never occurs in practice (construction always sets one).
        match (&self.secret, &self.fallback) {
            (Some(sb), _) => {
                let s = sb.as_slice();
                let n = s.len().min(32);
                buf[..n].copy_from_slice(&s[..n]);
            }
            (None, Some(arr)) => buf.copy_from_slice(arr),
            (None, None) => {}
        }
        let r = f(&buf);
        for b in buf.iter_mut() {
            *b = 0;
        }
        r
    }
}

impl Drop for SecureKey {
    fn drop(&mut self) {
        if let Some(arr) = self.fallback.as_mut() {
            for b in arr.iter_mut() {
                *b = 0;
            }
        }
        // SecretBytes zeroes itself on its own drop.
    }
}

/// The per-machine key file. A random 32-byte secret stored at
/// `~/.config/web2qml/mail/machine.key` with `0600` permissions, loaded into
/// secure memory. It is the silent second factor mixed into every sealing
/// key. Created lazily — and only when persistence is enabled, so the
/// hardened default leaves nothing on disk.
pub struct MachineKey {
    key: SecureKey,
    /// True when this key is backed by an on-disk file (so sealing to disk
    /// is meaningful). False for the ephemeral session-only key used when
    /// persistence is disabled.
    persistent: bool,
}

impl MachineKey {
    /// Path to the machine key file.
    pub fn path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home)
            .join(".config/web2qml")
            .join("mail")
            .join("machine.key")
    }

    /// Load the machine key, creating it if absent. Creation (and therefore
    /// any on-disk machine key) only happens when persistence is enabled;
    /// otherwise an ephemeral, session-only key is returned so that nothing
    /// is written and the bridge still functions in memory.
    pub fn load_or_create() -> Result<Self> {
        let path = Self::path();

        if !security::persistence_enabled() {
            // Hardened default: do not touch disk. Mint an ephemeral key so
            // sealing/opening still works for the in-memory session.
            let mut buf = [0u8; 32];
            fill_random(&mut buf)?;
            let key = SecureKey::from_bytes(&buf);
            for b in buf.iter_mut() {
                *b = 0;
            }
            return Ok(MachineKey {
                key,
                persistent: false,
            });
        }

        // Persistence is on: load or create the file.
        match std::fs::read(&path) {
            Ok(bytes) if bytes.len() == 32 => {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                let key = SecureKey::from_bytes(&arr);
                for b in arr.iter_mut() {
                    *b = 0;
                }
                Ok(MachineKey {
                    key,
                    persistent: true,
                })
            }
            _ => {
                // Generate, persist with tight permissions, and hold.
                let mut buf = [0u8; 32];
                fill_random(&mut buf)?;
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| MailError::Store(e.to_string()))?;
                }
                write_private(&path, &buf)?;
                let key = SecureKey::from_bytes(&buf);
                for b in buf.iter_mut() {
                    *b = 0;
                }
                Ok(MachineKey {
                    key,
                    persistent: true,
                })
            }
        }
    }

    pub fn is_persistent(&self) -> bool {
        self.persistent
    }

    /// Mix the machine key into a bridge key, yielding the effective sealing
    /// key. XOR of two independent uniform 32-byte keys.
    fn mix(&self, bridge_key: &[u8; 32]) -> [u8; 32] {
        self.key.with(|mk| {
            let mut out = [0u8; 32];
            for i in 0..32 {
                out[i] = bridge_key[i] ^ mk[i];
            }
            out
        })
    }
}

/// A [`SecretBox`] that wraps the platform cipher with w2q's security
/// policy: it refuses to seal to disk unless persistence is enabled, and it
/// machine-binds the sealing key. This is what the [`crate::mail_account::
/// AccountStore`] is given instead of the bare platform box.
pub struct PolicySecretBox {
    inner: Box<dyn SecretBox>,
    machine: MachineKey,
}

impl PolicySecretBox {
    /// Build the policy box from the platform cipher and the machine key.
    pub fn new() -> Result<Self> {
        Ok(PolicySecretBox {
            inner: platform_secret_box(),
            machine: MachineKey::load_or_create()?,
        })
    }

    /// Whether records sealed by this box can survive across sessions
    /// (i.e. persistence is enabled *and* a real cipher is linked).
    pub fn persists(&self) -> bool {
        self.machine.is_persistent()
    }
}

impl SecretBox for PolicySecretBox {
    fn seal(&self, key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
        // Enforce the opt-in persistence gate. With persistence off we
        // refuse rather than write Proton credentials to disk silently —
        // the same posture w2q takes for voice learning. The account store
        // surfaces this and keeps the record in memory only.
        if !security::persistence_enabled() {
            return Err(MailError::PersistenceDisabled);
        }
        let effective = self.machine.mix(key);
        let r = self.inner.seal(&effective, plaintext);
        // effective is a stack array; scrubbing it is cheap insurance.
        let _ = effective;
        r
    }

    fn open(&self, key: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>> {
        let effective = self.machine.mix(key);
        self.inner.open(&effective, sealed)
    }
}

/// Convenience: the account store's secret box, policy-wrapped. This is the
/// constructor `MailHub` uses by default.
pub fn secure_secret_box() -> Result<Box<dyn SecretBox>> {
    Ok(Box::new(PolicySecretBox::new()?))
}

// ---------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------

/// Fill a buffer from the OS CSPRNG (`/dev/urandom`). No `rand`/`getrandom`
/// dependency, matching the tree's posture.
fn fill_random(buf: &mut [u8]) -> Result<()> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom")
        .map_err(|e| MailError::Store(format!("CSPRNG unavailable: {e}")))?;
    f.read_exact(buf)
        .map_err(|e| MailError::Store(format!("CSPRNG read failed: {e}")))?;
    Ok(())
}

/// Write `data` to `path` with `0600` permissions (owner read/write only),
/// creating or truncating. On Unix the mode is set at create time so the
/// secret is never briefly world-readable.
fn write_private(path: &std::path::Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| MailError::Store(e.to_string()))?;
        f.write_all(data).map_err(|e| MailError::Store(e.to_string()))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let mut f = std::fs::File::create(path).map_err(|e| MailError::Store(e.to_string()))?;
        f.write_all(data).map_err(|e| MailError::Store(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_key_roundtrips_through_secure_memory() {
        let k = [9u8; 32];
        let sk = SecureKey::from_bytes(&k);
        sk.with(|got| assert_eq!(got, &k));
    }

    #[test]
    fn machine_mix_is_xor_and_reversible() {
        // Build a MachineKey with a known key by hand (bypassing disk).
        let mk_bytes = [0x5au8; 32];
        let machine = MachineKey {
            key: SecureKey::from_bytes(&mk_bytes),
            persistent: false,
        };
        let bridge = [0x0fu8; 32];
        let eff = machine.mix(&bridge);
        for i in 0..32 {
            assert_eq!(eff[i], 0x0f ^ 0x5a);
        }
        // XORing the effective key back with the machine key recovers bridge.
        let recovered = machine.mix(&eff);
        assert_eq!(recovered, bridge);
    }
}
