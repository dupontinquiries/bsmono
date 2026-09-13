//! `ledger` — a personal, offline-first receipt chain.
//!
//! Every person is their own issuer and their own root of trust. There is no
//! shared chain, no consensus, no global clock. Each device keeps an
//! append-only, hash-linked, signed log of *its own* entries; giving
//! something to someone is an entry in your log that names them. Because
//! each chain is single-author and totally ordered (a `seq` counter plus a
//! `prev` hash), there is nothing to agree on: to check a claim you verify
//! one signature and one hash link. Two people who never met can each hold a
//! verifiable copy of the same receipt, carried between them by the `drop`
//! courier layer or (later) a DHT — the ledger does not know or care which.
//!
//! What an entry can say ([`Body`]):
//! * `Gift { to, amount, memo }` — hand someone units of *your own* scrip.
//!   Alice-credit is just "receipts Alice signed naming you"; Alice is the
//!   only one who can mint it, so there is no double-spend to prevent, only
//!   equivocation to detect. `balance(alice, bob)` sums it.
//! * `Attest { subject, tag, obj }` — a signed statement: "I authored
//!   artifact `obj`", "I grant `subject` access to `obj`", "`subject` is my
//!   friend". This is the "who wrote the script / who gave me logscope
//!   access / add me on snapclone" layer. `tag` namespaces the meaning;
//!   [`tag`] has the well-known ones, the rest are yours.
//! * `Revoke { target }` — retract one of your earlier entries by its id.
//!
//! What makes it safe without a server:
//! * **Single-author chains.** Entry `n` carries `prev = id(entry n-1)`. A
//!   [`Store`] only counts entries in the longest gap-free verified prefix of
//!   each author, so a receipt you can't trace back to that author's genesis
//!   earns nothing.
//! * **Equivocation is evidence, not a race.** If an author ever signs two
//!   different entries at the same `seq`, [`Store::insert`] keeps both as a
//!   [`Fork`] — cryptographic proof they cheated — rather than picking a
//!   winner.
//! * **Merge is a CRDT.** A `Store` is a grow-only set keyed by
//!   `(author, seq)`. Merging two stores is order-independent and idempotent,
//!   so tapping cards in any order converges. This is exactly what `drop`
//!   needs to reconcile two phones' views.
//!
//! Crypto is the same seam as `drop`: signing/verification go through
//! [`crate::drop::Sealer`] (Ed25519 in `crypto-kit`; `MacSealer` in tests),
//! and content hashing goes through [`Hasher`] (BLAKE3 in `crypto-kit`; a
//! deliberately-labelled non-cryptographic mixer in tests). The hash and the
//! signature scheme are deployment-wide constants, like the text vocabulary —
//! everyone in a community must use the same ones for ids to agree.
//!
//! This module is transport-agnostic on purpose. [`pack_batch`] /
//! [`unpack_batch`] frame a set of entries into one blob small enough to ride
//! in a single `drop` pack; the `drop`/DHT wiring lives in the app, not here.

use crate::drop::Sealer;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
#[cfg(not(feature = "std"))]
#[allow(unused_imports)] // needed on a bare no_std target; host no_std sees it via the sysroot prelude
use alloc::vec;

/// 256-bit content id. Printed big-endian hex.
pub type Id = [u8; 32];

pub const ZERO_ID: Id = [0u8; 32];

/// Content hasher. A deployment picks one (BLAKE3 in `crypto-kit`); every
/// participant must use the same, since ids are part of the wire contract.
pub trait Hasher {
    fn hash(&self, bytes: &[u8]) -> Id;
}

/// Well-known [`Body::Attest`] tags. Values above `USER_BASE` are yours to
/// define; these are the ones the apps in this monorepo lean on.
pub mod tag {
    /// "I authored `obj`", where `obj` is the content id of the artifact
    /// (a voiceterm script, a gitmind note). Answers "who wrote this".
    pub const AUTHOR: u16 = 1;
    /// "I grant `subject` access to `obj`" (a logscope-egui session, a repo).
    pub const GRANT: u16 = 2;
    /// "`subject` is my friend" (snapclone). Mutual when both sign it.
    pub const FRIEND: u16 = 3;
    /// "I vouch that `subject` is who they claim" — a light web-of-trust edge.
    pub const VOUCH: u16 = 4;
    /// First tag value applications may define freely.
    pub const USER_BASE: u16 = 1024;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Units of the author's own scrip, given to `to`. `memo` is opaque
    /// (an app may put `text::encode` bytes or a content id there).
    Gift { to: u32, amount: u64, memo: Vec<u8> },
    /// A signed statement about `subject` regarding `obj`.
    Attest { subject: u32, tag: u16, obj: Vec<u8> },
    /// Retract one of the author's own earlier entries.
    Revoke { target: Id },
}

const B_GIFT: u8 = 0;
const B_ATTEST: u8 = 1;
const B_REVOKE: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub author: u32,
    pub seq: u32,
    /// id of the author's entry at `seq - 1`; [`ZERO_ID`] at `seq == 0`.
    pub prev: Id,
    /// Author's local clock in minutes (advisory; ordering is by `seq`).
    pub time: u32,
    pub body: Body,
    /// Signature over [`Entry::canon`], produced by [`Sealer::sign`].
    pub sig: Vec<u8>,
}

fn put_u32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn put_u64(v: &mut Vec<u8>, x: u64) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn put_bytes(v: &mut Vec<u8>, b: &[u8]) {
    put_u32(v, b.len() as u32);
    v.extend_from_slice(b);
}
fn get_u32(b: &[u8], p: &mut usize) -> Option<u32> {
    let x = b.get(*p..*p + 4)?;
    *p += 4;
    Some(u32::from_le_bytes(x.try_into().unwrap()))
}
fn get_u64(b: &[u8], p: &mut usize) -> Option<u64> {
    let x = b.get(*p..*p + 8)?;
    *p += 8;
    Some(u64::from_le_bytes(x.try_into().unwrap()))
}
fn get_bytes(b: &[u8], p: &mut usize) -> Option<Vec<u8>> {
    let n = get_u32(b, p)? as usize;
    let x = b.get(*p..*p + n)?;
    *p += n;
    Some(x.to_vec())
}
fn get_id(b: &[u8], p: &mut usize) -> Option<Id> {
    let x = b.get(*p..*p + 32)?;
    *p += 32;
    Some(x.try_into().unwrap())
}

impl Entry {
    /// Canonical bytes signed and hashed. Excludes `sig` so the id is stable
    /// regardless of signature representation.
    pub fn canon(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(64);
        put_u32(&mut v, self.author);
        put_u32(&mut v, self.seq);
        v.extend_from_slice(&self.prev);
        put_u32(&mut v, self.time);
        match &self.body {
            Body::Gift { to, amount, memo } => {
                v.push(B_GIFT);
                put_u32(&mut v, *to);
                put_u64(&mut v, *amount);
                put_bytes(&mut v, memo);
            }
            Body::Attest { subject, tag, obj } => {
                v.push(B_ATTEST);
                put_u32(&mut v, *subject);
                v.extend_from_slice(&tag.to_le_bytes());
                put_bytes(&mut v, obj);
            }
            Body::Revoke { target } => {
                v.push(B_REVOKE);
                v.extend_from_slice(target);
            }
        }
        v
    }

    /// Content id = `H(canon)`.
    pub fn id(&self, h: &dyn Hasher) -> Id {
        h.hash(&self.canon())
    }

    /// Full wire form (canon + length-prefixed signature).
    pub fn write(&self, out: &mut Vec<u8>) {
        let c = self.canon();
        put_bytes(out, &c);
        put_bytes(out, &self.sig);
    }

    /// Parse one entry from the front of `b`, returning it and the bytes used.
    pub fn read(b: &[u8]) -> Option<(Entry, usize)> {
        let mut p = 0usize;
        let canon = get_bytes(b, &mut p)?;
        let sig = get_bytes(b, &mut p)?;
        let mut e = Self::from_canon(&canon)?;
        e.sig = sig;
        Some((e, p))
    }

    fn from_canon(c: &[u8]) -> Option<Entry> {
        let mut p = 0usize;
        let author = get_u32(c, &mut p)?;
        let seq = get_u32(c, &mut p)?;
        let prev = get_id(c, &mut p)?;
        let time = get_u32(c, &mut p)?;
        let kind = *c.get(p)?;
        p += 1;
        let body = match kind {
            B_GIFT => Body::Gift { to: get_u32(c, &mut p)?, amount: get_u64(c, &mut p)?, memo: get_bytes(c, &mut p)? },
            B_ATTEST => {
                let subject = get_u32(c, &mut p)?;
                let tag = u16::from_le_bytes(c.get(p..p + 2)?.try_into().unwrap());
                p += 2;
                Body::Attest { subject, tag, obj: get_bytes(c, &mut p)? }
            }
            B_REVOKE => Body::Revoke { target: get_id(c, &mut p)? },
            _ => return None,
        };
        if p != c.len() {
            return None; // trailing garbage inside canon => reject
        }
        Some(Entry { author, seq, prev, time, body, sig: Vec::new() })
    }

    /// Verify the signature only (not the chain link).
    pub fn verify_sig(&self, s: &dyn Sealer) -> bool {
        s.verify(self.author, &self.canon(), &self.sig)
    }
}

/// A person's local view of the community's receipts.
#[derive(Debug, Clone, Default)]
pub struct Store {
    /// Canonical chain: `(author, seq) -> entry`.
    entries: BTreeMap<(u32, u32), Entry>,
    /// Author -> id already accepted at each seq (for O(1) equivocation check).
    ids: BTreeMap<(u32, u32), Id>,
    /// Author -> conflicting entries seen at an already-occupied seq.
    forks: BTreeMap<u32, Vec<Entry>>,
}

/// Outcome of offering one entry to a [`Store`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Insert {
    /// New, valid, accepted.
    Added,
    /// Byte-identical to one already held.
    Duplicate,
    /// A *different* entry already occupies `(author, seq)` — proof of
    /// equivocation. Both are retained; this names the author.
    Fork { author: u32 },
    /// Signature did not verify.
    BadSig,
    /// Body could not be parsed / other structural problem.
    Malformed,
}

impl Store {
    pub fn new() -> Store {
        Store::default()
    }

    /// Offer an entry. Verifies the signature; detects equivocation. Chain
    /// continuity is *not* required here (entries may arrive out of order) —
    /// it is enforced later by [`Store::verified_len`], which balances and
    /// queries respect.
    pub fn insert(&mut self, e: Entry, h: &dyn Hasher, s: &dyn Sealer) -> Insert {
        if !e.verify_sig(s) {
            return Insert::BadSig;
        }
        let id = e.id(h);
        let key = (e.author, e.seq);
        if let Some(prev_id) = self.ids.get(&key) {
            if *prev_id == id {
                return Insert::Duplicate;
            }
            self.forks.entry(e.author).or_default().push(e);
            return Insert::Fork { author: key.0 };
        }
        self.ids.insert(key, id);
        self.entries.insert(key, e);
        Insert::Added
    }

    /// Merge another store into this one (union of raw entries). Commutative
    /// and idempotent; forks in either survive. Returns the authors newly
    /// found to have equivocated.
    pub fn merge(&mut self, other: &Store, h: &dyn Hasher, s: &dyn Sealer) -> Vec<u32> {
        let mut forked = Vec::new();
        for e in other.entries.values().chain(other.forks.values().flatten()) {
            if let Insert::Fork { author } = self.insert(e.clone(), h, s) {
                if !forked.contains(&author) {
                    forked.push(author);
                }
            }
        }
        forked
    }

    /// Length of the longest gap-free, hash-linked prefix of `author`'s chain
    /// starting at `seq 0`. Entries at or beyond a break are untrusted.
    pub fn verified_len(&self, author: u32, h: &dyn Hasher) -> u32 {
        let mut n = 0u32;
        let mut prev = ZERO_ID;
        loop {
            let Some(e) = self.entries.get(&(author, n)) else { break };
            if e.prev != prev {
                break;
            }
            prev = e.id(h);
            n += 1;
        }
        n
    }

    /// Whether `author` has been caught equivocating.
    pub fn is_forked(&self, author: u32) -> bool {
        self.forks.contains_key(&author)
    }

    /// The retained fork evidence for `author`, if any.
    pub fn fork_evidence(&self, author: u32) -> Option<&[Entry]> {
        self.forks.get(&author).map(|v| v.as_slice())
    }

    /// Iterate an author's verified entries in order.
    pub fn chain<'a>(&'a self, author: u32, h: &dyn Hasher) -> impl Iterator<Item = &'a Entry> {
        let len = self.verified_len(author, h);
        (0..len).filter_map(move |seq| self.entries.get(&(author, seq)))
    }

    /// Every verified entry across all known authors.
    pub fn all_verified<'a>(&'a self, h: &'a dyn Hasher) -> impl Iterator<Item = &'a Entry> {
        let authors: Vec<u32> = self.entries.keys().map(|&(a, _)| a).collect::<alloc::collections::BTreeSet<_>>().into_iter().collect();
        authors.into_iter().flat_map(move |a| {
            let len = self.verified_len(a, h);
            (0..len).filter_map(move |seq| self.entries.get(&(a, seq)))
        })
    }

    /// Ids of `issuer`'s own entries they have revoked (within the verified
    /// prefix).
    fn revoked_by(&self, issuer: u32, h: &dyn Hasher) -> alloc::collections::BTreeSet<Id> {
        self.chain(issuer, h)
            .filter_map(|e| match &e.body {
                Body::Revoke { target } => Some(*target),
                _ => None,
            })
            .collect()
    }

    /// Units of `issuer`'s scrip currently held by `holder`: the sum of
    /// `issuer`'s un-revoked `Gift`s naming `holder`. Zero if `issuer` has
    /// equivocated (their chain is not trustworthy).
    pub fn balance(&self, issuer: u32, holder: u32, h: &dyn Hasher) -> u64 {
        if self.is_forked(issuer) {
            return 0;
        }
        let revoked = self.revoked_by(issuer, h);
        self.chain(issuer, h)
            .filter(|e| !revoked.contains(&e.id(h)))
            .filter_map(|e| match &e.body {
                Body::Gift { to, amount, .. } if *to == holder => Some(*amount),
                _ => None,
            })
            .sum()
    }

    /// Total of `issuer`'s scrip they have ever handed out (un-revoked). A
    /// personal currency's money supply is whatever its owner has minted.
    pub fn issued(&self, issuer: u32, h: &dyn Hasher) -> u64 {
        if self.is_forked(issuer) {
            return 0;
        }
        let revoked = self.revoked_by(issuer, h);
        self.chain(issuer, h)
            .filter(|e| !revoked.contains(&e.id(h)))
            .filter_map(|e| match &e.body {
                Body::Gift { amount, .. } => Some(*amount),
                _ => None,
            })
            .sum()
    }

    /// Un-revoked attestations with a given `tag` about `subject`, as
    /// `(author, obj)`. Answers "who granted me access to X", "who says we're
    /// friends". Excludes forked authors.
    pub fn attestations_about(&self, subject: u32, tag: u16, h: &dyn Hasher) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        let authors: Vec<u32> = self.entries.keys().map(|&(a, _)| a).collect::<alloc::collections::BTreeSet<_>>().into_iter().collect();
        for a in authors {
            if self.is_forked(a) {
                continue;
            }
            let revoked = self.revoked_by(a, h);
            for e in self.chain(a, h) {
                if revoked.contains(&e.id(h)) {
                    continue;
                }
                if let Body::Attest { subject: sub, tag: t, obj } = &e.body {
                    if *sub == subject && *t == tag {
                        out.push((a, obj.clone()));
                    }
                }
            }
        }
        out
    }

    /// Authors who attested [`tag::AUTHOR`] over content id `obj`. "Who wrote
    /// this?"
    pub fn authors_of(&self, obj: &[u8], h: &dyn Hasher) -> Vec<u32> {
        let mut out = Vec::new();
        let authors: Vec<u32> = self.entries.keys().map(|&(a, _)| a).collect::<alloc::collections::BTreeSet<_>>().into_iter().collect();
        for a in authors {
            if self.is_forked(a) {
                continue;
            }
            let revoked = self.revoked_by(a, h);
            if self.chain(a, h).any(|e| !revoked.contains(&e.id(h)) && matches!(&e.body, Body::Attest { tag, obj: o, .. } if *tag == tag::AUTHOR && o == obj)) {
                out.push(a);
            }
        }
        out
    }

    /// Whether `a` and `b` mutually attest [`tag::FRIEND`] about each other.
    pub fn mutual_friends(&self, a: u32, b: u32, h: &dyn Hasher) -> bool {
        let a_says = self.attestations_about(b, tag::FRIEND, h).iter().any(|(who, _)| *who == a);
        let b_says = self.attestations_about(a, tag::FRIEND, h).iter().any(|(who, _)| *who == b);
        a_says && b_says
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A person's own chain, for authoring new entries. Wraps a [`Store`] with
/// this device's identity and next sequence number.
pub struct Author<'a> {
    pub id: u32,
    seq: u32,
    prev: Id,
    h: &'a dyn Hasher,
    s: &'a dyn Sealer,
}

impl<'a> Author<'a> {
    /// Resume authoring on top of `store`'s view of `id`'s chain.
    pub fn resume(id: u32, store: &Store, h: &'a dyn Hasher, s: &'a dyn Sealer) -> Author<'a> {
        let seq = store.verified_len(id, h);
        let prev = if seq == 0 { ZERO_ID } else { store.entries[&(id, seq - 1)].id(h) };
        Author { id, seq, prev, h, s }
    }

    /// Sign and append `body`, inserting into `store` and returning the entry.
    pub fn append(&mut self, store: &mut Store, time: u32, body: Body) -> Option<Entry> {
        let mut e = Entry { author: self.id, seq: self.seq, prev: self.prev, time, body, sig: Vec::new() };
        e.sig = self.s.sign(self.id, &e.canon())?;
        let id = e.id(self.h);
        match store.insert(e.clone(), self.h, self.s) {
            Insert::Added => {
                self.seq += 1;
                self.prev = id;
                Some(e)
            }
            _ => None,
        }
    }

    pub fn gift(&mut self, store: &mut Store, time: u32, to: u32, amount: u64, memo: &[u8]) -> Option<Entry> {
        self.append(store, time, Body::Gift { to, amount, memo: memo.to_vec() })
    }
    pub fn attest(&mut self, store: &mut Store, time: u32, subject: u32, tag: u16, obj: &[u8]) -> Option<Entry> {
        self.append(store, time, Body::Attest { subject, tag, obj: obj.to_vec() })
    }
    pub fn revoke(&mut self, store: &mut Store, time: u32, target: Id) -> Option<Entry> {
        self.append(store, time, Body::Revoke { target })
    }
}

// ── transport framing (rides a drop pack) ────────────────────────────────────

/// Frame entries into one self-delimiting blob for a single `drop` pack.
/// Amortises the pack header over many tiny receipts.
pub fn pack_batch(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::new();
    put_u32(&mut out, entries.len() as u32);
    for e in entries {
        e.write(&mut out);
    }
    out
}

/// Parse a blob from [`pack_batch`]. Stops at the first malformed entry and
/// returns whatever parsed cleanly before it (a torn tail loses only the
/// tail, like a `drop` card log).
pub fn unpack_batch(blob: &[u8]) -> Vec<Entry> {
    let mut p = 0usize;
    let Some(n) = get_u32(blob, &mut p) else { return Vec::new() };
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        match Entry::read(&blob[p..]) {
            Some((e, used)) => {
                p += used;
                out.push(e);
            }
            None => break,
        }
    }
    out
}

/// Greedily choose entries (in the given order) whose framed batch fits in
/// `budget` bytes. Returns the blob and how many entries it holds — the
/// ledger analogue of `text::fit`, for deciding what receipts to leave on a
/// tag.
pub fn fit_batch(entries: &[Entry], budget: usize) -> (Vec<u8>, usize) {
    let mut chosen: Vec<Entry> = Vec::new();
    for e in entries {
        chosen.push(e.clone());
        if pack_batch(&chosen).len() > budget {
            chosen.pop();
            break;
        }
    }
    (pack_batch(&chosen), chosen.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drop::MacSealer;
    use alloc::collections::BTreeSet;

    /// Test hasher: a 256-bit mixer over four 64-bit lanes. Well-distributed
    /// enough for tests; NOT collision-resistant — `crypto-kit` supplies
    /// BLAKE3 for real deployments.
    struct TestHasher;
    impl Hasher for TestHasher {
        fn hash(&self, bytes: &[u8]) -> Id {
            let mut lanes = [0x9E3779B97F4A7C15u64, 0xBF58476D1CE4E5B9, 0x94D049BB133111EB, 0x2545F4914F6CDD1D];
            for (i, &b) in bytes.iter().enumerate() {
                let l = &mut lanes[i & 3];
                *l = (*l ^ b as u64).wrapping_mul(0x100000001B3);
                *l ^= l.rotate_left(27);
            }
            for _ in 0..3 {
                for i in 0..4 {
                    lanes[i] = lanes[i].wrapping_add(lanes[(i + 1) & 3].rotate_left(17));
                    lanes[i] ^= lanes[i] >> 31;
                }
            }
            let mut out = [0u8; 32];
            for i in 0..4 {
                out[i * 8..i * 8 + 8].copy_from_slice(&lanes[i].to_le_bytes());
            }
            out
        }
    }

    fn world(ids: &[u32]) -> (TestHasher, MacSealer) {
        let mut s = MacSealer::default();
        s.pub_ok.extend(ids.iter().copied());
        (TestHasher, s)
    }

    const A: u32 = 0xA11CE;
    const B: u32 = 0xB0B;
    const C: u32 = 0xC4A0;

    #[test]
    fn entry_wire_roundtrip_and_sig_tamper() {
        let (h, s) = world(&[A]);
        let mut st = Store::new();
        let mut al = Author::resume(A, &st, &h, &s);
        let e = al.gift(&mut st, 100, B, 5, b"thanks for the ride").unwrap();
        let mut buf = Vec::new();
        e.write(&mut buf);
        let (back, used) = Entry::read(&buf).unwrap();
        assert_eq!(used, buf.len());
        assert_eq!(back, e);
        assert!(back.verify_sig(&s));
        // flip a byte in the memo region -> signature no longer matches
        let mut e2 = e.clone();
        if let Body::Gift { memo, .. } = &mut e2.body {
            memo[0] ^= 1;
        }
        assert!(!e2.verify_sig(&s), "tampered body must fail its original sig");
    }

    #[test]
    fn chain_links_and_gap_detection() {
        let (h, s) = world(&[A]);
        let mut st = Store::new();
        let mut al = Author::resume(A, &st, &h, &s);
        let e0 = al.attest(&mut st, 1, B, tag::FRIEND, b"").unwrap();
        let _e1 = al.gift(&mut st, 2, B, 3, b"").unwrap();
        let e2 = al.attest(&mut st, 3, C, tag::VOUCH, b"legit").unwrap();
        assert_eq!(st.verified_len(A, &h), 3);
        assert_eq!(e0.seq, 0);
        assert_eq!(e2.prev, st.entries[&(A, 1)].id(&h));
        // a fresh store that receives seq 0 and 2 but not 1 trusts only seq 0
        let mut gap = Store::new();
        assert_eq!(gap.insert(st.entries[&(A, 0)].clone(), &h, &s), Insert::Added);
        assert_eq!(gap.insert(st.entries[&(A, 2)].clone(), &h, &s), Insert::Added);
        assert_eq!(gap.verified_len(A, &h), 1, "seq 2 is unreachable without seq 1");
        // filling the gap heals it
        assert_eq!(gap.insert(st.entries[&(A, 1)].clone(), &h, &s), Insert::Added);
        assert_eq!(gap.verified_len(A, &h), 3);
    }

    #[test]
    fn a_forged_prev_link_breaks_the_chain_there() {
        // seq contiguity alone is not enough: an attacker who splices an entry
        // with the right seq but a wrong `prev` must not extend the trusted
        // prefix. This is what makes the log tamper-evident rather than just
        // ordered.
        let (h, s) = world(&[A]);
        let mut st = Store::new();
        let mut al = Author::resume(A, &st, &h, &s);
        al.gift(&mut st, 1, B, 1, b"").unwrap(); // seq 0
        al.gift(&mut st, 2, B, 1, b"").unwrap(); // seq 1
        assert_eq!(st.verified_len(A, &h), 2);
        // hand-build a seq-2 entry whose prev points at the WRONG ancestor
        // (seq 0 instead of seq 1), sign it so the signature is genuine.
        let wrong_prev = st.entries[&(A, 0)].id(&h);
        let mut spliced = Entry { author: A, seq: 2, prev: wrong_prev, time: 9, body: Body::Gift { to: B, amount: 500, memo: b"spliced".to_vec() }, sig: vec![] };
        spliced.sig = s.sign(A, &spliced.canon()).unwrap();
        assert!(spliced.verify_sig(&s), "signature itself is valid");
        assert_eq!(st.insert(spliced, &h, &s), Insert::Added, "structurally accepted...");
        assert_eq!(st.verified_len(A, &h), 2, "...but not trusted: the bad prev caps the chain at 2");
        assert_eq!(st.balance(A, B, &h), 2, "the 500 splice earns nothing");
    }

    #[test]
    fn equivocation_is_caught_and_kills_balance() {
        let (h, s) = world(&[A, B]);
        let mut st = Store::new();
        let mut al = Author::resume(A, &st, &h, &s);
        al.gift(&mut st, 1, B, 10, b"").unwrap();
        assert_eq!(st.balance(A, B, &h), 10);
        // A forges a *different* entry at seq 0 (double life). Build it by hand
        // at the same seq with different content, sign it, offer it.
        let mut evil = Entry { author: A, seq: 0, prev: ZERO_ID, time: 99, body: Body::Gift { to: C, amount: 999, memo: b"evil".to_vec() }, sig: vec![] };
        evil.sig = s.sign(A, &evil.canon()).unwrap();
        assert_eq!(st.insert(evil, &h, &s), Insert::Fork { author: A });
        assert!(st.is_forked(A));
        assert!(st.fork_evidence(A).is_some());
        assert_eq!(st.balance(A, B, &h), 0, "a caught equivocator's scrip is worthless");
    }

    #[test]
    fn merge_is_commutative_and_idempotent() {
        let (h, s) = world(&[A, B, C]);
        // three independent chains authored in three separate stores
        let mk = |who: u32, n: u32| {
            let mut st = Store::new();
            let mut au = Author::resume(who, &st, &h, &s);
            for i in 0..n {
                au.gift(&mut st, i, if who == A { B } else { A }, (i + 1) as u64, b"").unwrap();
            }
            st
        };
        let sa = mk(A, 3);
        let sb = mk(B, 2);
        let sc = mk(C, 4);

        let mut ab = sa.clone();
        ab.merge(&sb, &h, &s);
        ab.merge(&sc, &h, &s);
        let mut cba = sc.clone();
        cba.merge(&sb, &h, &s);
        cba.merge(&sa, &h, &s);
        // same set of (author, seq) keys regardless of merge order
        let keys = |st: &Store| st.entries.keys().copied().collect::<BTreeSet<_>>();
        assert_eq!(keys(&ab), keys(&cba));
        assert_eq!(ab.len(), 3 + 2 + 4);
        // idempotent: merging again changes nothing
        let before = ab.len();
        ab.merge(&cba, &h, &s);
        assert_eq!(ab.len(), before);
        // balances agree across orders
        assert_eq!(ab.balance(A, B, &h), cba.balance(A, B, &h));
        assert_eq!(ab.balance(A, B, &h), 1 + 2 + 3);
    }

    #[test]
    fn attestation_queries_answer_the_use_cases() {
        let (h, s) = world(&[A, B, C]);
        let mut st = Store::new();
        let script = h.hash(b"the movie script bytes");
        let mut al = Author::resume(A, &st, &h, &s);
        al.attest(&mut st, 1, A, tag::AUTHOR, &script).unwrap(); // A wrote the script
        al.attest(&mut st, 2, B, tag::GRANT, b"logscope-session-42").unwrap(); // A grants B access
        al.attest(&mut st, 3, B, tag::FRIEND, b"").unwrap(); // A -> B friend
        let mut bl = Author::resume(B, &st, &h, &s);
        bl.attest(&mut st, 4, A, tag::FRIEND, b"").unwrap(); // B -> A friend (mutual)

        assert_eq!(st.authors_of(&script, &h), vec![A]);
        let grants = st.attestations_about(B, tag::GRANT, &h);
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].0, A);
        assert_eq!(grants[0].1, b"logscope-session-42");
        assert!(st.mutual_friends(A, B, &h));
        assert!(!st.mutual_friends(A, C, &h));
    }

    #[test]
    fn revoke_removes_a_grant_and_a_gift() {
        let (h, s) = world(&[A, B]);
        let mut st = Store::new();
        let mut al = Author::resume(A, &st, &h, &s);
        let grant = al.attest(&mut st, 1, B, tag::GRANT, b"repo").unwrap();
        let gift = al.gift(&mut st, 2, B, 7, b"").unwrap();
        assert_eq!(st.attestations_about(B, tag::GRANT, &h).len(), 1);
        assert_eq!(st.balance(A, B, &h), 7);
        al.revoke(&mut st, 3, grant.id(&h)).unwrap();
        al.revoke(&mut st, 4, gift.id(&h)).unwrap();
        assert_eq!(st.attestations_about(B, tag::GRANT, &h).len(), 0, "grant revoked");
        assert_eq!(st.balance(A, B, &h), 0, "gift revoked");
    }

    #[test]
    fn batch_framing_roundtrips_and_fits_a_budget() {
        let (h, s) = world(&[A]);
        let mut st = Store::new();
        let mut al = Author::resume(A, &st, &h, &s);
        let mut entries = Vec::new();
        for i in 0..30 {
            entries.push(al.gift(&mut st, i, B, i as u64, b"x").unwrap());
        }
        let blob = pack_batch(&entries);
        assert_eq!(unpack_batch(&blob), entries);
        // fit into a Classic-1K-ish pack budget
        let (fitted, n) = fit_batch(&entries, 752);
        assert!(fitted.len() <= 752);
        assert!(n > 0 && n <= entries.len());
        let got = unpack_batch(&fitted);
        assert_eq!(got.len(), n);
        assert_eq!(got[..], entries[..n]);
        // one more entry would overflow (or we took them all)
        if n < entries.len() {
            assert!(pack_batch(&entries[..n + 1]).len() > 752);
        }
        // a torn tail loses only the tail
        let mut torn = fitted.clone();
        torn.truncate(torn.len() - 3);
        assert_eq!(unpack_batch(&torn).len(), n - 1);
    }

    /// The whole stack: A leaves receipts on a card as a drop pack, B taps the
    /// same card, unpacks and verifies them into its own store.
    #[test]
    fn receipts_ride_a_drop_pack_between_two_devices() {
        use crate::drop::{Card, Device, Kind};
        let (h, s) = world(&[A, B]);
        // A authors two receipts
        let mut a_store = Store::new();
        let mut al = Author::resume(A, &a_store, &h, &s);
        let e0 = al.attest(&mut a_store, 1, A, tag::AUTHOR, &h.hash(b"script")).unwrap();
        let e1 = al.gift(&mut a_store, 2, B, 5, b"ty").unwrap();
        let blob = pack_batch(&[e0, e1]);

        // A device puts the blob in a broadcast opaque pack and leaves it on a card
        let mut a_dev = Device::new(A);
        a_dev.compose(&s, Kind::Opaque, 0, &blob, 100, 6).unwrap();
        let card = a_dev.tap(&s, &vec![0u8; 752], 752, 100).image;

        // B taps: reads the broadcast, feeds the payload to its ledger store
        let mut b_dev = Device::new(B);
        let t = b_dev.tap(&s, &card, 752, 200);
        let recv = t.received.iter().find(|r| r.kind == Kind::Opaque).expect("B got the pack");
        let mut b_store = Store::new();
        for e in unpack_batch(&recv.plain) {
            assert_eq!(b_store.insert(e, &h, &s), Insert::Added);
        }
        assert_eq!(b_store.verified_len(A, &h), 2);
        assert_eq!(b_store.balance(A, B, &h), 5);
        assert_eq!(b_store.authors_of(&h.hash(b"script"), &h), vec![A]);
        // sanity: the card really carried it (not delivered straight from memory)
        assert!(Card::read(&card).packs.iter().any(|p| p.kind == Kind::Opaque));
    }
}
