//! The encrypted session pair-verify ends in (HAP spec §6.5.2 for IP,
//! §7.4.6 for BLE).
//!
//! Two keys, one per direction, both HKDF'd from the Curve25519 shared
//! secret with `Control-Salt`. Two counters, one per direction, starting at
//! zero and never reused: the nonce IS the counter, so a lost or replayed
//! frame desynchronises the session and every later frame fails to
//! authenticate. That is the correct outcome — the fix is a fresh
//! pair-verify, never "try the next counter".
//!
//! BLE encrypts the whole PDU with no AAD. IP frames prefix a 2-byte
//! little-endian length that is the AAD; [`SecureSession::seal_ip`] exists
//! so the key/counter logic is not duplicated when that transport arrives,
//! but nothing in switcheroo uses it yet.

use crate::keys::{counter_nonce, hkdf_32, open, seal, AeadError};

pub struct SecureSession {
    /// Controller → accessory.
    write_key: [u8; 32],
    /// Accessory → controller.
    read_key: [u8; 32],
    write_counter: u64,
    read_counter: u64,
}

impl std::fmt::Debug for SecureSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecureSession").field("write_counter", &self.write_counter).field("read_counter", &self.read_counter).finish()
    }
}

impl SecureSession {
    /// The controller's view: writes with the Write key, reads with Read.
    pub fn from_shared_secret(shared: &[u8; 32]) -> Self {
        SecureSession {
            write_key: hkdf_32(b"Control-Salt", b"Control-Write-Encryption-Key", shared),
            read_key: hkdf_32(b"Control-Salt", b"Control-Read-Encryption-Key", shared),
            write_counter: 0,
            read_counter: 0,
        }
    }

    /// The accessory's view: the same two keys, swapped. For the simulator.
    pub fn accessory_side(shared: &[u8; 32]) -> Self {
        let c = Self::from_shared_secret(shared);
        SecureSession { write_key: c.read_key, read_key: c.write_key, write_counter: 0, read_counter: 0 }
    }

    /// Encrypt one outbound frame (BLE: the whole PDU). Advances the write
    /// counter.
    pub fn seal_ble(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let n = counter_nonce(self.write_counter);
        self.write_counter += 1;
        seal(&self.write_key, &n, b"", plaintext)
    }

    /// Decrypt one inbound frame. Advances the read counter only on
    /// success, so a corrupted frame does not also skip a nonce.
    pub fn open_ble(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, AeadError> {
        let n = counter_nonce(self.read_counter);
        let p = open(&self.read_key, &n, b"", ciphertext)?;
        self.read_counter += 1;
        Ok(p)
    }

    /// IP framing: `len(2, LE) | ciphertext | tag`, with the length as AAD.
    /// Unused by switcheroo; kept so the IP transport, when it comes, does
    /// not reimplement the counters.
    pub fn seal_ip(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let len = (plaintext.len() as u16).to_le_bytes();
        let n = counter_nonce(self.write_counter);
        self.write_counter += 1;
        let mut out = len.to_vec();
        out.extend(seal(&self.write_key, &n, &len, plaintext));
        out
    }

    pub fn counters(&self) -> (u64, u64) { (self.write_counter, self.read_counter) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sides_agree_and_counters_advance_per_direction() {
        let shared = [42u8; 32];
        let mut c = SecureSession::from_shared_secret(&shared);
        let mut a = SecureSession::accessory_side(&shared);
        let f1 = c.seal_ble(b"one");
        let f2 = c.seal_ble(b"two");
        assert_eq!(a.open_ble(&f1).unwrap(), b"one");
        assert_eq!(a.open_ble(&f2).unwrap(), b"two");
        let r = a.seal_ble(b"reply");
        assert_eq!(c.open_ble(&r).unwrap(), b"reply");
        assert_eq!(c.counters(), (2, 1));
        assert_eq!(a.counters(), (1, 2));
    }

    #[test]
    fn a_replayed_frame_fails_and_does_not_burn_the_nonce() {
        let shared = [1u8; 32];
        let mut c = SecureSession::from_shared_secret(&shared);
        let mut a = SecureSession::accessory_side(&shared);
        let f1 = c.seal_ble(b"x");
        a.open_ble(&f1).unwrap();
        assert!(a.open_ble(&f1).is_err(), "replay");
        assert_eq!(a.counters().1, 1, "failed open did not advance");
        let f2 = c.seal_ble(b"y");
        assert_eq!(a.open_ble(&f2).unwrap(), b"y", "the next real frame still works");
    }

    #[test]
    fn a_different_shared_secret_cannot_read() {
        let mut c = SecureSession::from_shared_secret(&[1; 32]);
        let mut a = SecureSession::accessory_side(&[2; 32]);
        assert!(a.open_ble(&c.seal_ble(b"x")).is_err());
    }

    #[test]
    fn ip_frame_carries_its_length_as_aad() {
        let mut c = SecureSession::from_shared_secret(&[5; 32]);
        let f = c.seal_ip(b"hello");
        assert_eq!(&f[..2], &5u16.to_le_bytes());
        assert_eq!(f.len(), 2 + 5 + 16);
    }
}
