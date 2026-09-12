//! `drop` — store-carry-forward over passive tags.
//!
//! A tag cannot run code, so it cannot be a node. It can be a *mailbox*, and
//! the phones that tap it can be the *couriers*: read what is on the card,
//! keep what is for you, carry what is for others, leave what you have. Do
//! that at every card and messages hop card → device → card → device across
//! people who are never in the same place at the same time. This module is
//! that transaction, headless — it operates on a card *image* (`&[u8]`) and
//! a device's own state, so the whole fleet can be simulated in a test
//! (see `simulated_fleet_delivers_across_two_couriers`).
//!
//! ## Card image
//!
//! ```text
//! [ CardHeader 8 B ][ Pack ][ Pack ] ... [ 0x0000 or end of memory ]
//!
//! CardHeader:  magic u16 'DP' | gen u16 | owner u32  (owner 0 = public board)
//! Pack:        len u16 | kind u8 | hops u8 | born u32 | src u32 | dst u32 |
//!              seq u16 | crc u16 | payload[len-20-auth] | auth[16 or 64]
//! ```
//!
//! No directory. A reader walks the log and stops at the first pack whose
//! CRC fails or whose length is zero, so a tap interrupted mid-write loses
//! the tail and nothing else. The writer orders packs by priority (owner's
//! mail first, transit last) so a torn write costs the least important ones.
//!
//! `crc` is transport integrity (CRC-16/CCITT). Authenticity is the `auth`
//! trailer, produced by a [`Sealer`]: 16 bytes for an AEAD tag when the
//! pack is for one known recipient (the courier carries an opaque blob), 64
//! for an Ed25519 signature when it is a broadcast anyone may verify. The
//! sealer is a trait so this module has no crypto dependency; `crypto-kit`
//! has both primitives. `hops` is outside the auth on purpose: couriers
//! increment it and it is diagnostics, not a security claim.
//!
//! ## What makes cards *routing* infrastructure
//!
//! Every tap also leaves a 20-byte control pack: "device X was here at T".
//! Cards therefore carry a rolling log of who visits them and when; devices
//! that read it learn the visit graph (Alice taps the café card most
//! mornings) and can place Alice's mail where she will find it, instead of
//! flooding. The prediction lives on the devices, which have memory; the
//! cards only ever hold the observations. That is the stream of new packs:
//! the cards' contents evolve on every tap even when nobody sent anything.

use std::collections::{BTreeMap, BTreeSet};

pub const MAGIC: u16 = 0x4450; // "DP"
pub const CARD_HEADER_LEN: usize = 8;
pub const PACK_HEADER_LEN: usize = 20;
pub const MAX_TTL: u8 = 15;

/// Payload kinds (low two bits of `kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// `text::encode` output.
    Text = 0,
    /// `image::encode` output.
    Image = 1,
    /// Caller-defined bytes.
    Opaque = 2,
    /// Presence beacon; payload is empty, `born` is the visit time.
    Beacon = 3,
}

const KIND_MASK: u8 = 0b11;
const FLAG_SEALED: u8 = 1 << 2; // auth is a 16-byte AEAD tag, payload encrypted
const FLAG_BROADCAST: u8 = 1 << 3; // auth is a 64-byte signature
const TTL_SHIFT: u8 = 4;

/// Identity of a pack: (sender, sequence, born). Enough to dedup for the
/// lifetime of a sequence counter; devices keep these in a set.
pub type PackId = (u32, u16, u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    pub kind: Kind,
    pub sealed: bool,
    pub broadcast: bool,
    pub ttl: u8,
    pub hops: u8,
    pub born: u32,
    pub src: u32,
    pub dst: u32,
    pub seq: u16,
    /// Payload as it sits on the card (ciphertext if `sealed`).
    pub payload: Vec<u8>,
    pub auth: Vec<u8>,
}

impl Pack {
    pub fn id(&self) -> PackId {
        (self.src, self.seq, self.born)
    }
    pub fn wire_len(&self) -> usize {
        PACK_HEADER_LEN + self.payload.len() + self.auth.len()
    }
    fn kind_byte(&self) -> u8 {
        (self.kind as u8)
            | if self.sealed { FLAG_SEALED } else { 0 }
            | if self.broadcast { FLAG_BROADCAST } else { 0 }
            | (self.ttl.min(MAX_TTL) << TTL_SHIFT)
    }
    /// Header fields the auth covers: everything except `len`, `hops`, `ttl`
    /// and `crc` (the courier-mutable and transport-only parts).
    fn header_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(15);
        v.push(self.kind_byte() & !(0xF << TTL_SHIFT));
        v.extend_from_slice(&self.born.to_le_bytes());
        v.extend_from_slice(&self.src.to_le_bytes());
        v.extend_from_slice(&self.dst.to_le_bytes());
        v.extend_from_slice(&self.seq.to_le_bytes());
        v
    }
    /// Header + payload: what a broadcast signature is over.
    fn authed_bytes(&self) -> Vec<u8> {
        let mut v = self.header_bytes();
        v.extend_from_slice(&self.payload);
        v
    }
    pub fn write(&self, out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(&(self.wire_len() as u16).to_le_bytes());
        out.push(self.kind_byte());
        out.push(self.hops);
        out.extend_from_slice(&self.born.to_le_bytes());
        out.extend_from_slice(&self.src.to_le_bytes());
        out.extend_from_slice(&self.dst.to_le_bytes());
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&[0, 0]); // crc placeholder
        out.extend_from_slice(&self.payload);
        out.extend_from_slice(&self.auth);
        let crc = crc16(&out[start..start + 18]) ^ crc16(&out[start + 20..]);
        out[start + 18..start + 20].copy_from_slice(&crc.to_le_bytes());
    }
    /// Parse one pack at the start of `b`. `None` on any inconsistency.
    pub fn read(b: &[u8]) -> Option<Pack> {
        if b.len() < PACK_HEADER_LEN {
            return None;
        }
        let len = u16::from_le_bytes([b[0], b[1]]) as usize;
        if len < PACK_HEADER_LEN || len > b.len() {
            return None;
        }
        let kb = b[2];
        let sealed = kb & FLAG_SEALED != 0;
        let broadcast = kb & FLAG_BROADCAST != 0;
        let auth_len = if sealed { 16 } else if broadcast { 64 } else { 0 };
        if len < PACK_HEADER_LEN + auth_len {
            return None;
        }
        let crc = u16::from_le_bytes([b[18], b[19]]);
        if crc16(&b[..18]) ^ crc16(&b[20..len]) != crc {
            return None;
        }
        let kind = match kb & KIND_MASK {
            0 => Kind::Text,
            1 => Kind::Image,
            2 => Kind::Opaque,
            _ => Kind::Beacon,
        };
        Some(Pack {
            kind,
            sealed,
            broadcast,
            ttl: kb >> TTL_SHIFT,
            hops: b[3],
            born: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            src: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            dst: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
            seq: u16::from_le_bytes([b[16], b[17]]),
            payload: b[PACK_HEADER_LEN..len - auth_len].to_vec(),
            auth: b[len - auth_len..len].to_vec(),
        })
    }
}

/// CRC-16/CCITT-FALSE.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

// ── card ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub gen: u16,
    pub owner: u32,
    pub packs: Vec<Pack>,
}

impl Card {
    /// Parse a card image. An unformatted card (bad magic) parses as an empty
    /// public board at generation 0 — the next tap formats it.
    pub fn read(image: &[u8]) -> Card {
        if image.len() < CARD_HEADER_LEN || u16::from_le_bytes([image[0], image[1]]) != MAGIC {
            return Card { gen: 0, owner: 0, packs: vec![] };
        }
        let gen = u16::from_le_bytes([image[2], image[3]]);
        let owner = u32::from_le_bytes([image[4], image[5], image[6], image[7]]);
        let mut packs = Vec::new();
        let mut pos = CARD_HEADER_LEN;
        while let Some(p) = Pack::read(&image[pos..]) {
            pos += p.wire_len();
            packs.push(p);
        }
        Card { gen, owner, packs }
    }
    /// Serialise into exactly `capacity` bytes (zero-filled tail = end mark).
    /// Packs that do not fit are dropped, in order, so callers sort first.
    pub fn write(&self, capacity: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(capacity);
        out.extend_from_slice(&MAGIC.to_le_bytes());
        out.extend_from_slice(&self.gen.to_le_bytes());
        out.extend_from_slice(&self.owner.to_le_bytes());
        for p in &self.packs {
            if out.len() + p.wire_len() > capacity {
                continue;
            }
            p.write(&mut out);
        }
        out.resize(capacity, 0);
        out
    }
}

// ── sealing (the crypto seam) ────────────────────────────────────────────────

/// Produces and checks the `auth` trailer. `crypto-kit` supplies real
/// implementations (AES-256-GCM for [`Pack::sealed`], Ed25519 for
/// [`Pack::broadcast`]); tests use [`MacSealer`].
pub trait Sealer {
    /// Encrypt `plain` for `dst` from `src` with `aad` (the pack header)
    /// authenticated; returns (ciphertext, 16-byte tag).
    fn seal(&self, src: u32, dst: u32, aad: &[u8], plain: &[u8]) -> Option<(Vec<u8>, Vec<u8>)>;
    /// Verify-and-decrypt; `None` if the tag fails or we hold no key for the pair.
    fn open(&self, src: u32, dst: u32, aad: &[u8], cipher: &[u8], tag: &[u8]) -> Option<Vec<u8>>;
    /// Sign `authed` as `src` (64 bytes).
    fn sign(&self, src: u32, authed: &[u8]) -> Option<Vec<u8>>;
    fn verify(&self, src: u32, authed: &[u8], sig: &[u8]) -> bool;
}

/// A stand-in sealer: a keyed CRC "tag" and XOR "cipher". **Not security** —
/// it exists so the transaction logic can be exercised without pulling a
/// crypto dependency into a codec kit. Keys: `pair[(min, max)]` shared
/// secrets and `pub_ok` = set of senders whose signatures we accept.
#[derive(Default, Clone)]
pub struct MacSealer {
    pub pair: BTreeMap<(u32, u32), u32>,
    pub pub_ok: BTreeSet<u32>,
}

impl MacSealer {
    fn key(&self, a: u32, b: u32) -> Option<u32> {
        self.pair.get(&(a.min(b), a.max(b))).copied()
    }
    fn tag(key: u32, authed: &[u8], body: &[u8]) -> Vec<u8> {
        let k = key.to_le_bytes();
        let mut t = vec![0u8; 16];
        let c = crc16(authed) as u32 | ((crc16(body) as u32) << 16);
        let mix = c ^ key.rotate_left(7) ^ (authed.len() as u32).wrapping_mul(0x9E37_79B9);
        t[..4].copy_from_slice(&mix.to_le_bytes());
        t[4..8].copy_from_slice(&k);
        t[8..12].copy_from_slice(&(mix.rotate_left(13) ^ key).to_le_bytes());
        t[12..16].copy_from_slice(&(body.len() as u32).to_le_bytes());
        t
    }
}

impl Sealer for MacSealer {
    fn seal(&self, src: u32, dst: u32, authed: &[u8], plain: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let key = self.key(src, dst)?;
        let cipher: Vec<u8> = plain.iter().enumerate().map(|(i, &b)| b ^ key.to_le_bytes()[i % 4]).collect();
        let tag = Self::tag(key, authed, &cipher);
        Some((cipher, tag))
    }
    fn open(&self, src: u32, dst: u32, authed: &[u8], cipher: &[u8], tag: &[u8]) -> Option<Vec<u8>> {
        let key = self.key(src, dst)?;
        if Self::tag(key, authed, cipher) != tag {
            return None;
        }
        Some(cipher.iter().enumerate().map(|(i, &b)| b ^ key.to_le_bytes()[i % 4]).collect())
    }
    fn sign(&self, src: u32, authed: &[u8]) -> Option<Vec<u8>> {
        let mut s = Self::tag(src, authed, b"sig");
        s.extend_from_slice(&vec![0u8; 48]);
        Some(s)
    }
    fn verify(&self, src: u32, authed: &[u8], sig: &[u8]) -> bool {
        self.pub_ok.contains(&src) && sig.len() == 64 && sig[..16] == Self::tag(src, authed, b"sig")[..]
    }
}

// ── device ───────────────────────────────────────────────────────────────────

/// What a courier will carry for others.
#[derive(Debug, Clone)]
pub struct Policy {
    /// Carry transit packs at all (false = only deliver/collect own mail).
    pub relay: bool,
    /// Only carry packs whose src or dst is a contact.
    pub known_parties_only: bool,
    /// Cap on the device's relay buffer.
    pub max_carry: usize,
    /// Cap on beacons kept per card image (older ones scroll off).
    pub beacons_per_card: usize,
    /// Drop transit packs older than this many minutes.
    pub max_age_min: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Policy { relay: true, known_parties_only: false, max_carry: 64, beacons_per_card: 4, max_age_min: 7 * 24 * 60 }
    }
}

/// A received, verified pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    pub from: u32,
    pub kind: Kind,
    pub born: u32,
    pub hops: u8,
    pub plain: Vec<u8>,
}

#[derive(Debug, Default, Clone)]
pub struct Device {
    pub id: u32,
    pub seq: u16,
    pub contacts: BTreeSet<u32>,
    pub policy: PolicyBox,
    pub seen: BTreeSet<PackId>,
    pub inbox: Vec<Received>,
    /// Packs this device authored, waiting to be placed on cards.
    pub outbox: Vec<Pack>,
    /// Transit packs picked up for others.
    pub carry: Vec<Pack>,
    /// Visit observations: (card owner, device, minute). What a router learns.
    pub visits: Vec<(u32, u32, u32)>,
}

/// `Policy` behind a newtype so `Device: Default` stays derivable.
#[derive(Debug, Clone)]
pub struct PolicyBox(pub Policy);
impl Default for PolicyBox {
    fn default() -> Self {
        PolicyBox(Policy::default())
    }
}

/// Outcome of a tap.
#[derive(Debug, Clone)]
pub struct Tap {
    /// The image to write back; equal to the input if nothing changed.
    pub image: Vec<u8>,
    /// Packs delivered to this device by this tap.
    pub received: Vec<Received>,
    /// Packs the courier picked up to carry.
    pub picked_up: usize,
    /// Packs the courier left on the card (own + transit + beacon).
    pub left: usize,
}

impl Device {
    pub fn new(id: u32) -> Device {
        Device { id, ..Default::default() }
    }

    /// Author a pack for `dst` (0 = broadcast, signed; else sealed).
    pub fn compose(&mut self, s: &dyn Sealer, kind: Kind, dst: u32, plain: &[u8], now_min: u32, ttl: u8) -> Option<()> {
        self.seq = self.seq.wrapping_add(1);
        let mut p = Pack {
            kind,
            sealed: dst != 0,
            broadcast: dst == 0,
            ttl: ttl.min(MAX_TTL),
            hops: 0,
            born: now_min,
            src: self.id,
            dst,
            seq: self.seq,
            payload: plain.to_vec(),
            auth: vec![],
        };
        if dst == 0 {
            p.auth = s.sign(self.id, &p.authed_bytes())?;
        } else {
            let (cipher, tag) = s.seal(self.id, dst, &p.header_bytes(), plain)?;
            p.payload = cipher;
            p.auth = tag;
        }
        self.seen.insert(p.id());
        self.outbox.push(p);
        Some(())
    }

    fn open(&self, s: &dyn Sealer, p: &Pack) -> Option<Received> {
        let plain = if p.sealed {
            s.open(p.src, p.dst, &p.header_bytes(), &p.payload, &p.auth)?
        } else if p.broadcast {
            if !s.verify(p.src, &p.authed_bytes(), &p.auth) {
                return None;
            }
            p.payload.clone()
        } else {
            return None; // unauthenticated packs are not delivered
        };
        Some(Received { from: p.src, kind: p.kind, born: p.born, hops: p.hops, plain })
    }

    fn will_carry(&self, p: &Pack) -> bool {
        let pol = &self.policy.0;
        if !pol.relay || p.ttl == 0 || p.kind == Kind::Beacon {
            return false;
        }
        if pol.known_parties_only && !(self.contacts.contains(&p.src) || self.contacts.contains(&p.dst)) {
            return false;
        }
        true
    }

    /// The tap transaction. Pure over `image` and `self`; the transport
    /// writes back `Tap::image` (only the blocks that differ).
    pub fn tap(&mut self, s: &dyn Sealer, image: &[u8], capacity: usize, now_min: u32) -> Tap {
        let pol = self.policy.0.clone();
        let card = Card::read(image);
        let mut received = Vec::new();
        let mut picked_up = 0;

        // 1. read: deliver, learn, pick up
        for p in &card.packs {
            if p.kind == Kind::Beacon {
                self.visits.push((card.owner, p.src, p.born));
                continue;
            }
            if self.seen.contains(&p.id()) {
                continue;
            }
            let for_me = p.dst == self.id || (p.broadcast && p.dst == 0);
            if for_me {
                if let Some(r) = self.open(s, p) {
                    self.seen.insert(p.id());
                    received.push(r);
                }
            }
            // a broadcast is both mine to read and everyone's to carry; only
            // a pack addressed to me personally ends its journey here
            if p.dst != self.id && self.will_carry(p) && self.carry.len() < pol.max_carry {
                let mut c = p.clone();
                c.hops = c.hops.saturating_add(1);
                c.ttl -= 1;
                self.seen.insert(p.id());
                self.carry.push(c);
                picked_up += 1;
            }
        }

        // 2. compose the new card. Tiers: mail for the card's owner first (that
        //    is what a card is FOR), then my own packs, then transit; newest
        //    first inside a tier; the copy already on the card wins a tie so a
        //    courier's hop count never overwrites the card's own copy.
        let fresh = |p: &Pack| now_min.saturating_sub(p.born) <= pol.max_age_min;
        let tier = |p: &Pack| -> u8 {
            if card.owner != 0 && p.dst == card.owner {
                0
            } else if p.src == self.id {
                1
            } else {
                2
            }
        };
        let mut cands: Vec<(u8, u32, usize, Pack)> = Vec::new(); // (tier, born, arrival, pack)
        let mut n = 0usize;
        for p in card.packs.iter().filter(|p| p.kind != Kind::Beacon) {
            let read_by_owner = card.owner == self.id && self.seen.contains(&p.id());
            if fresh(p) && !read_by_owner {
                cands.push((tier(p), p.born, n, p.clone()));
                n += 1;
            }
        }
        let own = std::mem::take(&mut self.outbox);
        for p in &own {
            cands.push((tier(p), p.born, n, p.clone()));
            n += 1;
        }
        for p in self.carry.iter().filter(|p| fresh(p)) {
            cands.push((tier(p), p.born, n, p.clone()));
            n += 1;
        }
        cands.sort_by_key(|(t, born, arrival, _)| (*t, std::cmp::Reverse(*born), *arrival));
        let mut next = Card { gen: card.gen.wrapping_add(1), owner: card.owner, packs: vec![] };
        let mut on_card: BTreeSet<PackId> = BTreeSet::new();
        for (_, _, _, p) in cands {
            if on_card.insert(p.id()) {
                next.packs.push(p);
            }
        }
        // beacons: mine, then the newest existing ones up to the cap
        let mut beacons: Vec<Pack> =
            card.packs.iter().filter(|p| p.kind == Kind::Beacon && p.src != self.id).cloned().collect();
        beacons.sort_by_key(|p| std::cmp::Reverse(p.born));
        beacons.truncate(pol.beacons_per_card.saturating_sub(1));
        self.seq = self.seq.wrapping_add(1);
        next.packs.push(Pack {
            kind: Kind::Beacon,
            sealed: false,
            broadcast: false,
            ttl: 0,
            hops: 0,
            born: now_min,
            src: self.id,
            dst: 0,
            seq: self.seq,
            payload: vec![],
            auth: vec![],
        });
        next.packs.extend(beacons);

        // 3. fit to capacity. Own packs that landed move to `carry` so later
        //    cards get copies too (the author's copies spend no hops); the rest
        //    return to the outbox. Carried packs stay carried either way.
        let image = next.write(capacity);
        let landed: BTreeSet<PackId> = Card::read(&image).packs.iter().map(|p| p.id()).collect();
        let left = landed.len();
        for p in own {
            if landed.contains(&p.id()) {
                self.carry.push(p);
            } else {
                self.outbox.push(p);
            }
        }
        self.inbox.extend(received.iter().cloned());
        Tap { image, received, picked_up, left }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASSIC_1K: usize = 752;

    fn fleet() -> (MacSealer, Device, Device, Device) {
        let (a, b, c) = (0xA11CE, 0xB0B, 0xC4A01);
        let mut s = MacSealer::default();
        s.pair.insert((a.min(c), a.max(c)), 0xDEADBEEF);
        s.pair.insert((a.min(b), a.max(b)), 0x0BADF00D);
        s.pub_ok.extend([a, b, c]);
        let mut da = Device::new(a);
        let mut db = Device::new(b);
        let mut dc = Device::new(c);
        da.contacts.extend([b, c]);
        db.contacts.extend([a, c]);
        dc.contacts.extend([a, b]);
        (s, da, db, dc)
    }

    #[test]
    fn pack_roundtrips_and_crc_catches_a_torn_write() {
        let p = Pack {
            kind: Kind::Text,
            sealed: true,
            broadcast: false,
            ttl: 5,
            hops: 2,
            born: 123456,
            src: 1,
            dst: 2,
            seq: 7,
            payload: b"ciphertext".to_vec(),
            auth: vec![9; 16],
        };
        let mut w = Vec::new();
        p.write(&mut w);
        assert_eq!(w.len(), p.wire_len());
        assert_eq!(Pack::read(&w).unwrap(), p);
        assert!(Pack::read(&w[..w.len() - 1]).is_none(), "truncated");
        let mut torn = w.clone();
        torn[25] ^= 0x40;
        assert!(Pack::read(&torn).is_none(), "flipped payload bit");
        // a card whose second pack is torn keeps the first
        let mut img = Card { gen: 1, owner: 0, packs: vec![p.clone(), p.clone()] }.write(200);
        img[8 + p.wire_len() + 30] ^= 1;
        assert_eq!(Card::read(&img).packs.len(), 1);
    }

    #[test]
    fn simulated_fleet_delivers_across_two_couriers() {
        // Cards: home (A's), cafe (public), office (C's). A never meets C.
        let (s, mut a, mut b, mut c) = fleet();
        let mut cafe = vec![0u8; CLASSIC_1K];
        // owned cards are formatted with their owner; the public one is blank
        let home = Card { gen: 0, owner: a.id, packs: vec![] }.write(CLASSIC_1K);
        let mut office = Card { gen: 0, owner: c.id, packs: vec![] }.write(CLASSIC_1K);

        let msg = crate::text::encode(b"hey, are you still coming tonight?", crate::text::Parse::Optimal);
        a.compose(&s, Kind::Text, c.id, &msg, 1000, 6).unwrap();

        // day 1, morning: A taps home, then cafe
        let t = a.tap(&s, &home, CLASSIC_1K, 1000);
        let home = t.image;
        assert!(Card::read(&home).packs.iter().any(|p| p.dst == c.id), "A's own card carries the copy too");
        let t = a.tap(&s, &cafe, CLASSIC_1K, 1010);
        cafe = t.image;
        assert!(Card::read(&cafe).packs.iter().any(|p| p.dst == c.id), "A's mail is on the cafe card");
        assert!(a.outbox.is_empty(), "delivered to a card, nothing pending");

        // B taps cafe (picks it up), then office (drops it)
        let t = b.tap(&s, &cafe, CLASSIC_1K, 1100);
        cafe = t.image;
        assert_eq!(t.picked_up, 1);
        assert!(t.received.is_empty(), "B cannot open A→C mail");
        let t = b.tap(&s, &office, CLASSIC_1K, 1130);
        office = t.image;
        assert!(Card::read(&office).packs.iter().any(|p| p.dst == c.id && p.hops == 1));

        // C taps office and gets it
        let t = c.tap(&s, &office, CLASSIC_1K, 1200);
        assert_eq!(t.received.len(), 1);
        assert_eq!(t.received[0].from, a.id);
        assert_eq!(t.received[0].hops, 1);
        assert_eq!(crate::text::decode(&t.received[0].plain).unwrap(), b"hey, are you still coming tonight?");
        office = t.image;
        // C is the owner and has read it: it is off the card in the same tap
        assert!(!Card::read(&office).packs.iter().any(|p| p.dst == c.id));
        let t = c.tap(&s, &office, CLASSIC_1K, 1201);
        assert!(t.received.is_empty());

        // the visit log: C learned that B was at the office card
        assert!(c.visits.iter().any(|&(owner, who, _)| owner == c.id && who == b.id));
        // and the cafe card carries beacons from A and B, newest first
        let cafe_card = Card::read(&cafe);
        let beacons: Vec<u32> = cafe_card.packs.iter().filter(|p| p.kind == Kind::Beacon).map(|p| p.src).collect();
        assert_eq!(beacons, vec![b.id, a.id]);
    }

    #[test]
    fn a_photo_pack_and_chat_packs_share_a_card_by_priority() {
        let (s, mut a, mut b, mut c) = fleet();
        let mut cafe = vec![0u8; CLASSIC_1K];
        // A leaves a 600-byte photo for C, plus two chat lines
        a.compose(&s, Kind::Image, c.id, &vec![7u8; 600], 500, 4).unwrap();
        a.compose(&s, Kind::Text, c.id, b"one", 501, 4).unwrap();
        a.compose(&s, Kind::Text, c.id, b"two", 502, 4).unwrap();
        let t = a.tap(&s, &cafe, CLASSIC_1K, 503);
        cafe = t.image;
        let card = Card::read(&cafe);
        let kinds: Vec<Kind> = card.packs.iter().map(|p| p.kind).collect();
        // newest first: two, one, photo (636 B) — photo fits: 8+ (20+3+16)*2 + 636 + 20 = 742
        assert_eq!(kinds, vec![Kind::Text, Kind::Text, Kind::Image, Kind::Beacon]);
        assert!(a.outbox.is_empty());
        // B's own 100-byte sealed note to A now competes on the public card: B's
        // own tier beats transit, the 636-byte photo no longer fits, B carries it
        b.compose(&s, Kind::Opaque, a.id, &vec![1u8; 100], 600, 3).unwrap(); // 20+100+16 = 136
        let t = b.tap(&s, &cafe, CLASSIC_1K, 600);
        assert_eq!(t.picked_up, 3, "B carries all three transit packs");
        let card = Card::read(&t.image);
        assert!(card.packs.iter().any(|p| p.src == b.id && p.kind == Kind::Opaque));
        assert!(!card.packs.iter().any(|p| p.kind == Kind::Image), "photo did not fit; B carries it");
        assert!(b.carry.iter().any(|p| p.kind == Kind::Image));
        // On C's OWN card, mail for C outranks B's note to A: the photo lands
        // (8 + 39 + 39 + 636 + beacon 20 = 742) and B's 136-byte note waits.
        let mut office = Card { gen: 0, owner: c.id, packs: vec![] }.write(CLASSIC_1K);
        office = b.tap(&s, &office, CLASSIC_1K, 700).image;
        let office_card = Card::read(&office);
        assert!(office_card.packs.iter().any(|p| p.kind == Kind::Image));
        assert!(!office_card.packs.iter().any(|p| p.dst == a.id), "no room left for B's note to A");
        let t = c.tap(&s, &office, CLASSIC_1K, 800);
        let got: Vec<Vec<u8>> = t.received.iter().map(|r| r.plain.clone()).collect();
        assert_eq!(got.len(), 3);
        assert!(got.contains(&b"one".to_vec()));
        assert!(got.contains(&b"two".to_vec()));
        assert!(got.iter().any(|g| g.len() == 600 && g.iter().all(|&b| b == 7)));
    }

    #[test]
    fn ttl_and_age_stop_the_flood() {
        let (s, mut a, mut b, _c) = fleet();
        let mut card = vec![0u8; 144]; // NTAG213
        a.compose(&s, Kind::Text, 0, b"hi all", 10, 1).unwrap(); // ttl 1
        card = a.tap(&s, &card, 144, 10).image;
        let t = b.tap(&s, &card, 144, 11);
        assert_eq!(t.received.len(), 1, "broadcast read by B");
        assert_eq!(t.picked_up, 1);
        card = t.image;
        assert_eq!(b.carry[0].ttl, 0, "one hop used up");
        // B drops it on a third card as its last hop (ttl 0): readable there,
        // but no one picks it up again
        let t = b.tap(&s, &vec![0u8; 144], 144, 12);
        let third = t.image;
        assert!(Card::read(&third).packs.iter().any(|p| p.kind == Kind::Text && p.ttl == 0 && p.hops == 1));
        let mut d = Device::new(0xD);
        let t = d.tap(&s, &third, 144, 13);
        assert_eq!(t.received.len(), 1, "D reads the broadcast");
        assert_eq!(t.picked_up, 0, "D does not carry a ttl-0 pack");
        // age: a fresh device tapping the first card a week later leaves it off
        let mut z = Device::new(0x2);
        let t = z.tap(&s, &card, 144, 10 + 8 * 24 * 60);
        assert!(!Card::read(&t.image).packs.iter().any(|p| p.kind == Kind::Text));
    }

    #[test]
    fn known_parties_policy_refuses_stranger_traffic() {
        let (s, mut a, mut b, c) = fleet();
        let mut x = Device::new(0x777); // stranger, not in anyone's contacts
        let mut s2 = s.clone();
        s2.pub_ok.insert(x.id);
        b.policy.0.known_parties_only = true;
        let mut card = vec![0u8; CLASSIC_1K];
        x.compose(&s2, Kind::Text, 0, b"spam", 1, 5).unwrap();
        card = x.tap(&s2, &card, CLASSIC_1K, 1).image;
        a.compose(&s, Kind::Text, c.id, b"real", 2, 5).unwrap();
        card = a.tap(&s, &card, CLASSIC_1K, 2).image;
        let t = b.tap(&s2, &card, CLASSIC_1K, 3);
        assert_eq!(t.picked_up, 1);
        assert_eq!(b.carry[0].src, a.id);
        assert_eq!(t.received.len(), 1, "B still READS the stranger's broadcast (it verifies), it just won't carry it");
        let _ = c;
    }
}
