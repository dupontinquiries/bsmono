//! ratchet.rs — the sender-key ratchet.
//!
//! Each (member, epoch) owns a symmetric chain key `CK`. To send message `i`:
//!   * derive a per-message key `MK_i = HKDF(CK_i, MSG_KEY)`,
//!   * advance the chain `CK_{i+1} = HKDF(CK_i, RATCHET)` and DISCARD `CK_i`.
//!
//! Properties (proven in tests):
//!   * **Forward secrecy within an epoch** (§3(7)): the ratchet is one-way —
//!     `HKDF` is preimage-resistant, so an attacker who captures `CK_i` cannot
//!     compute `CK_{i-1}` or any earlier `MK`. Past messages stay secret even
//!     if the current chain key leaks.
//!   * A receiver who holds the sender's `CK` at index `j <= i` can ratchet
//!     forward to `i` to derive `MK_i` (message keys are deterministic from the
//!     chain), but never backward.
//!
//! The chain key is zeroized on drop and on each advance.

use crate::labels;
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroize;

/// A sender's symmetric chain for one epoch. `index` is the count of messages
/// already emitted (the NEXT message uses key at `index`).
pub struct SenderChain {
    ck: [u8; 32],
    index: u32,
}

impl Drop for SenderChain {
    fn drop(&mut self) {
        self.ck.zeroize();
    }
}

impl SenderChain {
    /// Start a chain from a 32-byte root sender key.
    pub fn from_root(root: [u8; 32]) -> Self {
        Self { ck: root, index: 0 }
    }

    /// Generate a fresh random root and start a chain (used when a member
    /// (re)keys for a new epoch).
    pub fn generate<R: rand_core::RngCore + rand_core::CryptoRng>(rng: &mut R) -> Self {
        let mut root = [0u8; 32];
        rng.fill_bytes(&mut root);
        let c = Self::from_root(root);
        root.zeroize();
        c
    }

    pub fn index(&self) -> u32 {
        self.index
    }

    /// HKDF helper: expand the current chain key under `info` into 32 bytes.
    fn expand(ck: &[u8; 32], info: &[u8]) -> [u8; 32] {
        let hk = Hkdf::<Sha256>::new(None, ck);
        let mut out = [0u8; 32];
        hk.expand(info, &mut out)
            .expect("HKDF expand of 32 bytes never fails");
        out
    }

    /// Derive the message key for the CURRENT index WITHOUT advancing. Used by
    /// a receiver that has ratcheted its chain copy exactly to the sender's
    /// index. Most senders should use `next_message_key` instead.
    pub fn message_key_at_current(&self) -> [u8; 32] {
        Self::expand(&self.ck, labels::MSG_KEY)
    }

    /// Derive the next message key and advance the chain. The previous chain
    /// key is overwritten (forward secrecy). Returns `(index_used, MK)`.
    pub fn next_message_key(&mut self) -> (u32, [u8; 32]) {
        let mk = Self::expand(&self.ck, labels::MSG_KEY);
        let used = self.index;
        // Advance: CK_{i+1} = HKDF(CK_i, RATCHET); discard CK_i.
        let next = Self::expand(&self.ck, labels::RATCHET);
        self.ck.zeroize();
        self.ck = next;
        self.index += 1;
        (used, mk)
    }

    /// Ratchet forward to `target` index (a receiver catching up to a sender's
    /// message index), returning the message key AT `target`. Returns None if
    /// `target` is behind the current index (we can't go backward — that's the
    /// forward-secrecy guarantee).
    pub fn message_key_for(&mut self, target: u32) -> Option<[u8; 32]> {
        if target < self.index {
            return None; // cannot ratchet backward
        }
        while self.index < target {
            let next = Self::expand(&self.ck, labels::RATCHET);
            self.ck.zeroize();
            self.ck = next;
            self.index += 1;
        }
        Some(self.message_key_at_current())
    }

    /// Snapshot the current root chain key (32 bytes) for sealing/distribution.
    /// Callers MUST treat the result as secret and zeroize it after use.
    pub fn export_root(&self) -> [u8; 32] {
        self.ck
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyRng(u64);
    impl rand_core::RngCore for DummyRng {
        fn next_u32(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
            (self.0 >> 33) as u32
        }
        fn next_u64(&mut self) -> u64 {
            ((self.next_u32() as u64) << 32) | self.next_u32() as u64
        }
        fn fill_bytes(&mut self, dest: &mut [u8]) {
            for chunk in dest.chunks_mut(4) {
                let v = self.next_u32().to_le_bytes();
                chunk.copy_from_slice(&v[..chunk.len()]);
            }
        }
        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
            self.fill_bytes(dest);
            Ok(())
        }
    }
    impl rand_core::CryptoRng for DummyRng {}

    #[test]
    fn sender_and_receiver_agree() {
        let root = [7u8; 32];
        let mut sender = SenderChain::from_root(root);
        let mut receiver = SenderChain::from_root(root);

        // Sender emits messages 0,1,2.
        let (i0, k0) = sender.next_message_key();
        let (i1, k1) = sender.next_message_key();
        let (i2, k2) = sender.next_message_key();
        assert_eq!((i0, i1, i2), (0, 1, 2));

        // Receiver, starting fresh, derives the same keys by ratcheting to each.
        assert_eq!(receiver.message_key_for(0).unwrap(), k0);
        assert_eq!(receiver.message_key_for(1).unwrap(), k1);
        assert_eq!(receiver.message_key_for(2).unwrap(), k2);
    }

    #[test]
    fn forward_secrecy_cannot_go_backward() {
        let mut receiver = SenderChain::from_root([3u8; 32]);
        // Catch up to index 5.
        let _ = receiver.message_key_for(5).unwrap();
        assert_eq!(receiver.index(), 5);
        // Asking for an earlier message must fail — the chain key for index 2
        // has been overwritten and is unrecoverable.
        assert!(receiver.message_key_for(2).is_none());
    }

    #[test]
    fn keys_are_distinct_per_index() {
        let mut s = SenderChain::from_root([1u8; 32]);
        let (_, a) = s.next_message_key();
        let (_, b) = s.next_message_key();
        let (_, c) = s.next_message_key();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn ratchet_and_message_labels_differ() {
        // MK and the next CK are derived from the same CK but DIFFERENT labels,
        // so they must not be equal — otherwise the message key would leak the
        // next chain key.
        let s = SenderChain::from_root([9u8; 32]);
        let mk = SenderChain::expand(&s.ck, labels::MSG_KEY);
        let next_ck = SenderChain::expand(&s.ck, labels::RATCHET);
        assert_ne!(mk, next_ck);
    }

    #[test]
    fn generate_is_random() {
        let mut rng = DummyRng(12345);
        let a = SenderChain::generate(&mut rng).export_root();
        let b = SenderChain::generate(&mut rng).export_root();
        assert_ne!(a, b);
    }
}
