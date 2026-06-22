//! Group mailbox slot — "share the slot, not the key".
//!
//! A group mailbox is a SINGLE shared DHT key, `H(group_id + "/groupmsg")`, that
//! every member reads and writes. The DHT already stores multiple distinct
//! values under one key side-by-side (content-derived value-ids), so N senders
//! coexist without clobbering — the same multi-writer mechanism the per-recipient
//! `connreq` slot already relies on. This batches group traffic into one lookup
//! instead of N per-recipient lookups, cutting DHT traversal.
//!
//! Crucially this does NOT use a shared symmetric group key. Each member seals
//! their own per-sender ratchet ([`crate::ratchet::SenderChain`] +
//! [`crate::seal`]), so forward secrecy and per-member revocation survive. This
//! module owns only the SLOT LAYOUT and READER logic — how a sealed blob is
//! addressed within the shared slot, and how a reader collects, dedups, and
//! orders the values it finds. The actual DHT PUT/GET is C++ (untestable here);
//! this is the pure decision logic that mirrors it, so the wire format and
//! collection rules are locked down by tests.
//!
//! ## ROLLBACK
//! Group-shared mailboxes are gated behind [`MailboxMode`]. The C++ side reads a
//! single flag (config `[network] groupMailboxShared`, default OFF) and chooses
//! [`MailboxMode::PerRecipient`] (the shipping behavior — one slot per peer) or
//! [`MailboxMode::GroupShared`]. To roll back after a hardware test, flip the
//! flag OFF: senders fan out to per-recipient slots exactly as today, and the
//! group slot is simply not written or read. No data migration, no format change
//! to the existing connreq slot. See `docs/group-mailbox.md`.

use crate::seal::{self, SealError};

/// Which mailbox strategy to use. The C++ layer maps a config flag to this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxMode {
    /// Shipping behavior: one DHT slot per recipient, sender fans out N writes.
    /// This is the rollback target — always safe.
    PerRecipient,
    /// New: one shared DHT slot per group, each member writes one per-sender
    /// sealed blob. One lookup amortized across the group.
    GroupShared,
}

impl MailboxMode {
    /// Map the rollback flag to a mode. `shared_enabled` comes from config
    /// (default false → PerRecipient).
    pub fn from_flag(shared_enabled: bool) -> Self {
        if shared_enabled { MailboxMode::GroupShared } else { MailboxMode::PerRecipient }
    }
    pub fn is_shared(self) -> bool { matches!(self, MailboxMode::GroupShared) }
}

/// The DHT key string for a group's shared mailbox slot. Mirrors the C++
/// `group_id + "/groupmsg"` convention (parallel to `id + "/connreq"`).
pub fn group_slot_key(group_id: &str) -> String {
    let mut s = String::with_capacity(group_id.len() + 9);
    s.push_str(group_id);
    s.push_str("/groupmsg");
    s
}

/// Wire layout of one value stored in the shared slot. The addressing header is
/// PLAINTEXT (it's public routing info, exactly like the sender-id prefix on the
/// existing connreq blob); only `sealed` is encrypted (per-sender ratchet).
///
///   sender_id "\n" group_id "\n" seq(ascii) "\n" sealed_bytes
///
/// `seq` is the sender's ratchet message index, so readers can order a sender's
/// messages and detect gaps without decrypting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxEntry {
    pub sender_id: String,
    pub group_id: String,
    pub seq: u32,
    pub sealed: Vec<u8>,
}

/// Errors building or parsing a mailbox entry.
#[derive(Debug, PartialEq, Eq)]
pub enum MailboxError {
    BadFormat,
    EmptyField,
    TooLarge,
    Seal(SealError),
}
impl From<SealError> for MailboxError {
    fn from(e: SealError) -> Self { MailboxError::Seal(e) }
}

/// Max serialized entry size — mirrors the C++ GET cap (2048) so we never build
/// a value the reader would reject.
pub const MAX_ENTRY_BYTES: usize = 2048;

impl MailboxEntry {
    /// Serialize to the DHT value bytes (the thing that becomes one `dht_value`).
    pub fn to_bytes(&self) -> Result<Vec<u8>, MailboxError> {
        if self.sender_id.is_empty() || self.group_id.is_empty() {
            return Err(MailboxError::EmptyField);
        }
        if self.sender_id.contains('\n') || self.group_id.contains('\n') {
            return Err(MailboxError::BadFormat);
        }
        let header = format!("{}\n{}\n{}\n", self.sender_id, self.group_id, self.seq);
        let mut out = Vec::with_capacity(header.len() + self.sealed.len());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(&self.sealed);
        if out.len() > MAX_ENTRY_BYTES {
            return Err(MailboxError::TooLarge);
        }
        Ok(out)
    }

    /// Parse a DHT value's bytes back into an entry. Tolerant of the sealed body
    /// containing arbitrary bytes (only the first three newlines are structural).
    pub fn from_bytes(raw: &[u8]) -> Result<MailboxEntry, MailboxError> {
        if raw.len() > MAX_ENTRY_BYTES {
            return Err(MailboxError::TooLarge);
        }
        // find the three structural newlines
        let nl1 = raw.iter().position(|&b| b == b'\n').ok_or(MailboxError::BadFormat)?;
        let nl2 = raw[nl1 + 1..].iter().position(|&b| b == b'\n')
            .map(|p| p + nl1 + 1).ok_or(MailboxError::BadFormat)?;
        let nl3 = raw[nl2 + 1..].iter().position(|&b| b == b'\n')
            .map(|p| p + nl2 + 1).ok_or(MailboxError::BadFormat)?;
        let sender_id = std::str::from_utf8(&raw[..nl1])
            .map_err(|_| MailboxError::BadFormat)?.to_string();
        let group_id = std::str::from_utf8(&raw[nl1 + 1..nl2])
            .map_err(|_| MailboxError::BadFormat)?.to_string();
        let seq_str = std::str::from_utf8(&raw[nl2 + 1..nl3])
            .map_err(|_| MailboxError::BadFormat)?;
        let seq: u32 = seq_str.parse().map_err(|_| MailboxError::BadFormat)?;
        if sender_id.is_empty() || group_id.is_empty() {
            return Err(MailboxError::EmptyField);
        }
        let sealed = raw[nl3 + 1..].to_vec();
        Ok(MailboxEntry { sender_id, group_id, seq, sealed })
    }
}

/// Collects the values a reader pulls from one shared slot, dedups byte-identical
/// replicas (OpenDHT returns each stored value several times), filters to the
/// expected group, and orders them deterministically (by sender, then seq) so
/// playback/processing order is stable across peers.
#[derive(Debug, Default)]
pub struct SlotReader {
    seen: std::collections::HashSet<Vec<u8>>,
    entries: Vec<MailboxEntry>,
    group_id: String,
}

impl SlotReader {
    pub fn new(group_id: &str) -> Self {
        SlotReader { seen: Default::default(), entries: Vec::new(), group_id: group_id.to_string() }
    }

    /// Offer one raw DHT value to the reader. Returns true if it was a NEW,
    /// well-formed entry for this group (false = duplicate, malformed, or wrong
    /// group). Mirrors the C++ GET callback's per-value handling.
    pub fn offer(&mut self, raw: &[u8]) -> bool {
        // dedup byte-identical replicas first (cheap, matches C++ `seen` set)
        if self.seen.contains(raw) {
            return false;
        }
        let entry = match MailboxEntry::from_bytes(raw) {
            Ok(e) => e,
            Err(_) => { return false; }
        };
        if entry.group_id != self.group_id {
            return false; // value under a shared key but for another group view
        }
        self.seen.insert(raw.to_vec());
        self.entries.push(entry);
        true
    }

    /// Finalize: returns entries ordered by (sender_id, seq). Stable ordering so
    /// every member processes the batch identically.
    pub fn into_ordered(mut self) -> Vec<MailboxEntry> {
        self.entries.sort_by(|a, b| {
            a.sender_id.cmp(&b.sender_id).then(a.seq.cmp(&b.seq))
        });
        self.entries
    }

    /// Per-sender sequence gaps in what we've collected so far — lets the reader
    /// log/diagnose missing messages (e.g. a value that expired before we read).
    /// Returns (sender_id, missing_seq) pairs. Only flags gaps BETWEEN observed
    /// messages, not a missing seq 0 (we may have joined late).
    pub fn gaps(&self) -> Vec<(String, u32)> {
        let mut by_sender: std::collections::BTreeMap<&str, Vec<u32>> = Default::default();
        for e in &self.entries {
            by_sender.entry(&e.sender_id).or_default().push(e.seq);
        }
        let mut out = Vec::new();
        for (sender, mut seqs) in by_sender {
            seqs.sort_unstable();
            seqs.dedup();
            for w in seqs.windows(2) {
                for missing in (w[0] + 1)..w[1] {
                    out.push((sender.to_string(), missing));
                }
            }
        }
        out
    }
}

/// Convenience: build the sealed body for a group message using the existing
/// per-sender ratchet + content encryption, then wrap it as a MailboxEntry.
/// `message_key` comes from the sender's `SenderChain::next_message_key()`.
pub fn build_entry<R: rand_core::RngCore + rand_core::CryptoRng>(
    sender_id: &str,
    group_id: &str,
    seq: u32,
    message_key: &[u8; 32],
    plaintext: &[u8],
    rng: &mut R,
) -> Result<MailboxEntry, MailboxError> {
    let sealed = seal::encrypt_content(message_key, plaintext, rng)?;
    Ok(MailboxEntry {
        sender_id: sender_id.to_string(),
        group_id: group_id.to_string(),
        seq,
        sealed,
    })
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
            self.fill_bytes(dest); Ok(())
        }
    }
    impl rand_core::CryptoRng for DummyRng {}

    #[test]
    fn mode_from_flag_defaults_safe() {
        assert_eq!(MailboxMode::from_flag(false), MailboxMode::PerRecipient);
        assert_eq!(MailboxMode::from_flag(true), MailboxMode::GroupShared);
        assert!(!MailboxMode::from_flag(false).is_shared());
    }

    #[test]
    fn slot_key_parallels_connreq() {
        assert_eq!(group_slot_key("grp-abc"), "grp-abc/groupmsg");
    }

    #[test]
    fn entry_roundtrips() {
        let e = MailboxEntry {
            sender_id: "sc-alice".into(),
            group_id: "grp-1".into(),
            seq: 7,
            sealed: vec![0, 1, 2, 255, b'\n', 3], // sealed may contain newlines
        };
        let bytes = e.to_bytes().unwrap();
        let back = MailboxEntry::from_bytes(&bytes).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn rejects_empty_and_malformed() {
        assert_eq!(MailboxEntry::from_bytes(b"no-newlines"), Err(MailboxError::BadFormat));
        assert_eq!(MailboxEntry::from_bytes(b"a\nb\nnotanumber\nx"), Err(MailboxError::BadFormat));
        let empty = MailboxEntry { sender_id: "".into(), group_id: "g".into(), seq: 0, sealed: vec![1] };
        assert_eq!(empty.to_bytes(), Err(MailboxError::EmptyField));
    }

    #[test]
    fn oversize_rejected_both_ways() {
        let big = MailboxEntry {
            sender_id: "sc-a".into(), group_id: "g".into(), seq: 0,
            sealed: vec![0u8; MAX_ENTRY_BYTES],
        };
        assert_eq!(big.to_bytes(), Err(MailboxError::TooLarge));
    }

    #[test]
    fn reader_dedups_replicas() {
        let e = MailboxEntry { sender_id: "sc-a".into(), group_id: "g".into(), seq: 0, sealed: vec![9] };
        let raw = e.to_bytes().unwrap();
        let mut r = SlotReader::new("g");
        assert!(r.offer(&raw));       // first: new
        assert!(!r.offer(&raw));      // replica: dropped
        assert!(!r.offer(&raw));
        assert_eq!(r.into_ordered().len(), 1);
    }

    #[test]
    fn reader_filters_wrong_group() {
        let mine = MailboxEntry { sender_id: "sc-a".into(), group_id: "g1".into(), seq: 0, sealed: vec![1] };
        let other = MailboxEntry { sender_id: "sc-b".into(), group_id: "g2".into(), seq: 0, sealed: vec![2] };
        let mut r = SlotReader::new("g1");
        assert!(r.offer(&mine.to_bytes().unwrap()));
        assert!(!r.offer(&other.to_bytes().unwrap())); // different group → ignored
        assert_eq!(r.into_ordered().len(), 1);
    }

    #[test]
    fn reader_orders_by_sender_then_seq() {
        let mk = |s: &str, q: u32| MailboxEntry {
            sender_id: s.into(), group_id: "g".into(), seq: q, sealed: vec![q as u8]
        }.to_bytes().unwrap();
        let mut r = SlotReader::new("g");
        // offer out of order
        r.offer(&mk("sc-bob", 1));
        r.offer(&mk("sc-alice", 2));
        r.offer(&mk("sc-bob", 0));
        r.offer(&mk("sc-alice", 0));
        let ordered = r.into_ordered();
        let got: Vec<(String, u32)> = ordered.iter().map(|e| (e.sender_id.clone(), e.seq)).collect();
        assert_eq!(got, vec![
            ("sc-alice".into(), 0), ("sc-alice".into(), 2),
            ("sc-bob".into(), 0), ("sc-bob".into(), 1),
        ]);
    }

    #[test]
    fn gaps_detected_between_observed_messages() {
        let mk = |s: &str, q: u32| MailboxEntry {
            sender_id: s.into(), group_id: "g".into(), seq: q, sealed: vec![0]
        }.to_bytes().unwrap();
        let mut r = SlotReader::new("g");
        r.offer(&mk("sc-a", 0));
        r.offer(&mk("sc-a", 3)); // 1 and 2 missing
        r.offer(&mk("sc-b", 5)); // lone message — no gap flagged (joined late)
        let gaps = r.gaps();
        assert!(gaps.contains(&("sc-a".to_string(), 1)));
        assert!(gaps.contains(&("sc-a".to_string(), 2)));
        assert_eq!(gaps.iter().filter(|(s, _)| s == "sc-b").count(), 0);
    }

    #[test]
    fn build_entry_seals_with_message_key() {
        let mut rng = DummyRng(42);
        let key = [7u8; 32];
        let e = build_entry("sc-a", "g", 0, &key, b"hello group", &mut rng).unwrap();
        assert_eq!(e.sender_id, "sc-a");
        assert_eq!(e.seq, 0);
        // sealed must decrypt back with the same message key
        let pt = seal::decrypt_content(&key, &e.sealed).unwrap();
        assert_eq!(pt, b"hello group");
        // and it survives a slot round-trip
        let raw = e.to_bytes().unwrap();
        let back = MailboxEntry::from_bytes(&raw).unwrap();
        assert_eq!(seal::decrypt_content(&key, &back.sealed).unwrap(), b"hello group");
    }
}
