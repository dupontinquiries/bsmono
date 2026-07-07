//! `sim::net::entity_snapshot` — host-authoritative entity-transform replication
//! (WID r330-coordinator).
//!
//! Roadmap **item 7** of `docs/sections/SECTION_multiplayer.md` (§C "Entity
//! replication + authority"); rides the **`ChannelId::Snapshots`** channel defined
//! in `sim::net_channels` — **UnreliableSequenced, newest-wins** (MULTIPLAYER_WORLD_JOIN
//! §6). The host owns truth: it stamps each replicated entity with a monotonically
//! rising **per-entity sequence** ([`AuthorityStamper`]) and broadcasts transform
//! snapshots. Clients apply **newest-wins** via [`SnapshotGate`]: a transform is
//! applied only if its seq is newer than the last one applied for that entity;
//! stale / duplicate / reordered packets are dropped, never blocking on a lost one
//! (the unreliable-sequenced contract — a dropped transform is simply superseded by
//! the next snapshot).
//!
//! This is the *transform* replication path. It is deliberately separate from
//! `sim::world_authority` (command-sourced **world edits**: block place/remove) and
//! from `sim::snapshot` (the rollback capture/restore trait). Entity transforms are
//! lossy/newest-wins; world edits are reliable/ordered commands.
//!
//! Pure: fixed-layout little-endian wire, no IO/clock/RNG/`unwrap`/`expect`/`panic!`,
//! [`decode_snapshot`] returns `None` on any malformed/truncated/hostile input, and
//! the entity count is capped so a bad length field can't trigger a huge allocation.
//! Mirrors the `sim::net::wire` codec idiom (magic / version / kind / capped count /
//! bounded `Reader`). Positions are [`Fixed`] (the deterministic 48.16 sim type), so
//! the wire is byte-exact across peers.

use crate::sim::fixed::Fixed;

/// Identifies our snapshot datagrams ("UN") — same family as `sim::net::wire`.
pub const SNAPSHOT_MAGIC: u16 = 0x554E;
/// Bumped on any wire-format change; mismatched versions are dropped.
pub const SNAPSHOT_VERSION: u8 = 1;
/// Kind discriminant. `wire.rs` owns 0 (input) and 1 (sync); snapshots are 2.
const KIND_SNAPSHOT: u8 = 2;

/// Cap on entities per snapshot packet, so a malformed/hostile `count` can't blow
/// up allocation (same defense as `wire::MAX_INPUTS_PER_PACKET`).
pub const MAX_ENTITIES_PER_SNAPSHOT: usize = 1024;

/// Wire size of one entity: id(8)+kind(1)+x(8)+y(8)+z(8)+yaw(2)+pitch(2)+seq(2).
const ENTITY_WIRE_LEN: usize = 39;

/// What kind of entity a replicated transform belongs to (for display + client
/// interest management; not authority-bearing). Decodes unknown bytes to `Other`
/// so adding kinds later is forward-compatible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    Player,
    Mob,
    Dragon,
    Other,
}

impl EntityKind {
    fn to_u8(self) -> u8 {
        match self {
            EntityKind::Player => 0,
            EntityKind::Mob => 1,
            EntityKind::Dragon => 2,
            EntityKind::Other => 255,
        }
    }
    fn from_u8(b: u8) -> EntityKind {
        match b {
            0 => EntityKind::Player,
            1 => EntityKind::Mob,
            2 => EntityKind::Dragon,
            _ => EntityKind::Other, // forward-compatible
        }
    }
}

/// One replicated entity transform plus its authority sequence number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntityState {
    pub id: u64,
    pub kind: EntityKind,
    pub x: Fixed,
    pub y: Fixed,
    pub z: Fixed,
    pub yaw: i16,
    pub pitch: i16,
    /// Per-entity authority sequence. Wraps at `u16`; newest-wins via serial
    /// arithmetic ([`SnapshotGate::seq_newer`]).
    pub seq: u16,
}

/// A decoded snapshot: the host tick it was sampled at + its entities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub host_tick: u64,
    pub entities: Vec<EntityState>,
}

// ── Encode ──────────────────────────────────────────────────────────────────

/// Encode a host snapshot. Entities beyond [`MAX_ENTITIES_PER_SNAPSHOT`] are
/// dropped (the host should split a larger world across packets / interest sets).
pub fn encode_snapshot(host_tick: u64, entities: &[EntityState]) -> Vec<u8> {
    let n = entities.len().min(MAX_ENTITIES_PER_SNAPSHOT);
    let mut b = Vec::with_capacity(4 + 8 + 2 + n * ENTITY_WIRE_LEN);
    b.extend_from_slice(&SNAPSHOT_MAGIC.to_le_bytes());
    b.push(SNAPSHOT_VERSION);
    b.push(KIND_SNAPSHOT);
    b.extend_from_slice(&host_tick.to_le_bytes());
    b.extend_from_slice(&(n as u16).to_le_bytes());
    for e in entities.iter().take(n) {
        write_entity(&mut b, e);
    }
    b
}

fn write_entity(b: &mut Vec<u8>, e: &EntityState) {
    b.extend_from_slice(&e.id.to_le_bytes());
    b.push(e.kind.to_u8());
    b.extend_from_slice(&e.x.0.to_le_bytes());
    b.extend_from_slice(&e.y.0.to_le_bytes());
    b.extend_from_slice(&e.z.0.to_le_bytes());
    b.extend_from_slice(&e.yaw.to_le_bytes());
    b.extend_from_slice(&e.pitch.to_le_bytes());
    b.extend_from_slice(&e.seq.to_le_bytes());
}

// ── Decode (never panics; `None` on any bad input) ──────────────────────────

/// Decode a snapshot datagram. Returns `None` on bad magic/version/kind, on
/// truncation, or if the declared count exceeds [`MAX_ENTITIES_PER_SNAPSHOT`].
pub fn decode_snapshot(buf: &[u8]) -> Option<Snapshot> {
    let mut r = Reader::new(buf);
    if r.u16()? != SNAPSHOT_MAGIC {
        return None;
    }
    if r.u8()? != SNAPSHOT_VERSION {
        return None;
    }
    if r.u8()? != KIND_SNAPSHOT {
        return None;
    }
    let host_tick = r.u64()?;
    let count = r.u16()? as usize;
    if count > MAX_ENTITIES_PER_SNAPSHOT {
        return None; // hostile/garbage length — refuse before allocating
    }
    let mut entities = Vec::with_capacity(count);
    for _ in 0..count {
        entities.push(read_entity(&mut r)?);
    }
    Some(Snapshot {
        host_tick,
        entities,
    })
}

fn read_entity(r: &mut Reader) -> Option<EntityState> {
    let id = r.u64()?;
    let kind = EntityKind::from_u8(r.u8()?);
    let x = Fixed(r.i64()?);
    let y = Fixed(r.i64()?);
    let z = Fixed(r.i64()?);
    let yaw = r.i16()?;
    let pitch = r.i16()?;
    let seq = r.u16()?;
    Some(EntityState {
        id,
        kind,
        x,
        y,
        z,
        yaw,
        pitch,
        seq,
    })
}

// ── Client gate: newest-wins, wraparound-safe ───────────────────────────────

/// Client-side newest-wins gate. Tracks the last-applied seq per entity; applying
/// a snapshot returns only the entities whose seq advanced (in input order),
/// updating the high-water marks. Stale / duplicate / reordered transforms are
/// dropped. Linear scan over a small tracked set — fine for the entity counts a
/// snapshot carries; `forget` keeps it bounded.
#[derive(Clone, Debug, Default)]
pub struct SnapshotGate {
    last: Vec<(u64, u16)>, // (entity id, last-applied seq)
}

impl SnapshotGate {
    pub fn new() -> SnapshotGate {
        SnapshotGate { last: Vec::new() }
    }

    /// `true` iff `incoming` is strictly newer than `prev` under RFC1982 serial
    /// arithmetic over `u16` (so it survives wraparound): newer when the forward
    /// distance is in `(0, 2^15)`.
    pub fn seq_newer(incoming: u16, prev: u16) -> bool {
        incoming != prev && incoming.wrapping_sub(prev) < 0x8000
    }

    /// Apply a snapshot; return the entities that advanced (first-seen always
    /// advances), updating per-entity high-water marks.
    pub fn apply(&mut self, snap: &Snapshot) -> Vec<EntityState> {
        let mut out: Vec<EntityState> = Vec::new();
        for e in &snap.entities {
            match self.last.iter_mut().find(|(id, _)| *id == e.id) {
                Some(slot) => {
                    if SnapshotGate::seq_newer(e.seq, slot.1) {
                        slot.1 = e.seq;
                        out.push(*e);
                    }
                }
                None => {
                    self.last.push((e.id, e.seq));
                    out.push(*e);
                }
            }
        }
        out
    }

    /// Drop tracking for an entity that despawned / left the client's interest
    /// set, keeping the tracked set bounded.
    pub fn forget(&mut self, id: u64) {
        self.last.retain(|(eid, _)| *eid != id);
    }

    /// Number of entities currently tracked.
    pub fn tracked(&self) -> usize {
        self.last.len()
    }
}

// ── Host: per-entity authority sequence source ──────────────────────────────

/// Host-side monotonic per-entity sequence source. The host stamps each entity
/// with `next(id)` per broadcast so clients can order/newest-win. Host owns truth
/// (§C). Wraps at `u16`, paired with [`SnapshotGate::seq_newer`].
#[derive(Clone, Debug, Default)]
pub struct AuthorityStamper {
    seqs: Vec<(u64, u16)>,
}

impl AuthorityStamper {
    pub fn new() -> AuthorityStamper {
        AuthorityStamper { seqs: Vec::new() }
    }

    /// Next sequence for `id` (first call yields 0, then 1, 2, …, wrapping).
    pub fn next(&mut self, id: u64) -> u16 {
        match self.seqs.iter_mut().find(|(eid, _)| *eid == id) {
            Some(slot) => {
                slot.1 = slot.1.wrapping_add(1);
                slot.1
            }
            None => {
                self.seqs.push((id, 0));
                0
            }
        }
    }
}

// ── Bounded reader (mirrors `wire::Reader`; never panics) ────────────────────

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Reader<'a> {
        Reader { b, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.b.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        let s = self.take(2)?;
        Some(u16::from_le_bytes([s[0], s[1]]))
    }
    fn i16(&mut self) -> Option<i16> {
        let s = self.take(2)?;
        Some(i16::from_le_bytes([s[0], s[1]]))
    }
    fn u64(&mut self) -> Option<u64> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Some(u64::from_le_bytes(a))
    }
    fn i64(&mut self) -> Option<i64> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Some(i64::from_le_bytes(a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(id: u64, kind: EntityKind, seq: u16) -> EntityState {
        EntityState {
            id,
            kind,
            x: Fixed::from_int(id as i64),
            y: Fixed::from_int(64),
            z: Fixed::from_frac(-3, 2),
            yaw: -120,
            pitch: 30,
            seq,
        }
    }

    #[test]
    fn roundtrips_every_kind() {
        let ents = vec![
            ent(1, EntityKind::Player, 7),
            ent(2, EntityKind::Mob, 0),
            ent(3, EntityKind::Dragon, 65535),
            ent(4, EntityKind::Other, 42),
        ];
        let buf = encode_snapshot(900, &ents);
        let got = decode_snapshot(&buf).expect("decodes");
        assert_eq!(got.host_tick, 900);
        assert_eq!(got.entities, ents);
    }

    #[test]
    fn empty_snapshot_roundtrips() {
        let buf = encode_snapshot(5, &[]);
        let got = decode_snapshot(&buf).expect("decodes");
        assert_eq!(got.host_tick, 5);
        assert!(got.entities.is_empty());
    }

    #[test]
    fn decode_rejects_bad_input() {
        let good = encode_snapshot(1, &[ent(1, EntityKind::Player, 1)]);
        // bad magic
        let mut bad = good.clone();
        bad[0] ^= 0xFF;
        assert!(decode_snapshot(&bad).is_none());
        // bad version
        let mut bad = good.clone();
        bad[2] = 99;
        assert!(decode_snapshot(&bad).is_none());
        // truncated mid-entity
        assert!(decode_snapshot(&good[..good.len() - 3]).is_none());
        // empty
        assert!(decode_snapshot(&[]).is_none());
    }

    #[test]
    fn decode_refuses_hostile_count() {
        // magic, version, kind=2, tick=0, count=0xFFFF (> MAX), no bodies.
        let mut b = Vec::new();
        b.extend_from_slice(&SNAPSHOT_MAGIC.to_le_bytes());
        b.push(SNAPSHOT_VERSION);
        b.push(KIND_SNAPSHOT);
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&0xFFFFu16.to_le_bytes());
        assert!(decode_snapshot(&b).is_none(), "huge count refused, no giant alloc");
    }

    #[test]
    fn encode_caps_entity_count() {
        let ents: Vec<EntityState> = (0..(MAX_ENTITIES_PER_SNAPSHOT as u64 + 50))
            .map(|i| ent(i, EntityKind::Mob, 1))
            .collect();
        let buf = encode_snapshot(0, &ents);
        let got = decode_snapshot(&buf).expect("decodes");
        assert_eq!(got.entities.len(), MAX_ENTITIES_PER_SNAPSHOT);
    }

    #[test]
    fn seq_newer_handles_wraparound() {
        assert!(SnapshotGate::seq_newer(1, 0));
        assert!(!SnapshotGate::seq_newer(0, 0)); // equal is not newer
        assert!(!SnapshotGate::seq_newer(0, 1)); // older
        // wrap: 0 is newer than 65535 (forward distance 1)
        assert!(SnapshotGate::seq_newer(0, 65535));
        assert!(!SnapshotGate::seq_newer(65535, 0));
        // half-range boundary is treated as "old" (ambiguous → not newer)
        assert!(!SnapshotGate::seq_newer(0x8000, 0));
        assert!(SnapshotGate::seq_newer(0x7FFF, 0));
    }

    #[test]
    fn gate_applies_first_drops_stale_accepts_advance() {
        let mut g = SnapshotGate::new();
        // first sight applies
        let a = g.apply(&Snapshot { host_tick: 1, entities: vec![ent(7, EntityKind::Mob, 5)] });
        assert_eq!(a.len(), 1);
        // duplicate / stale dropped
        let b = g.apply(&Snapshot { host_tick: 2, entities: vec![ent(7, EntityKind::Mob, 5)] });
        assert!(b.is_empty(), "duplicate seq dropped");
        let c = g.apply(&Snapshot { host_tick: 3, entities: vec![ent(7, EntityKind::Mob, 4)] });
        assert!(c.is_empty(), "older seq dropped (reordered packet)");
        // advance applies
        let d = g.apply(&Snapshot { host_tick: 4, entities: vec![ent(7, EntityKind::Mob, 6)] });
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].seq, 6);
    }

    #[test]
    fn gate_is_independent_per_entity() {
        let mut g = SnapshotGate::new();
        g.apply(&Snapshot { host_tick: 1, entities: vec![ent(1, EntityKind::Mob, 10)] });
        // entity 2 first-seen at low seq still applies; entity 1 stale dropped
        let out = g.apply(&Snapshot {
            host_tick: 2,
            entities: vec![ent(1, EntityKind::Mob, 9), ent(2, EntityKind::Player, 1)],
        });
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, 2);
        assert_eq!(g.tracked(), 2);
    }

    #[test]
    fn gate_forget_bounds_tracking() {
        let mut g = SnapshotGate::new();
        g.apply(&Snapshot { host_tick: 1, entities: vec![ent(1, EntityKind::Mob, 1)] });
        assert_eq!(g.tracked(), 1);
        g.forget(1);
        assert_eq!(g.tracked(), 0);
        // after forget, the entity is "first seen" again even at an old seq
        let out = g.apply(&Snapshot { host_tick: 2, entities: vec![ent(1, EntityKind::Mob, 0)] });
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn authority_stamper_is_monotonic_per_entity_and_independent() {
        let mut s = AuthorityStamper::new();
        assert_eq!(s.next(1), 0);
        assert_eq!(s.next(1), 1);
        assert_eq!(s.next(1), 2);
        assert_eq!(s.next(2), 0, "independent per entity");
        assert_eq!(s.next(1), 3);
    }

    #[test]
    fn stamper_and_gate_compose_end_to_end() {
        // Host stamps a stream; client gate accepts strictly increasing seqs.
        let mut s = AuthorityStamper::new();
        let mut g = SnapshotGate::new();
        let mut accepted = 0;
        for tick in 0..5u64 {
            let seq = s.next(99);
            let snap = Snapshot { host_tick: tick, entities: vec![ent(99, EntityKind::Dragon, seq)] };
            // round-trip through the wire too
            let wire = encode_snapshot(tick, &snap.entities);
            let decoded = decode_snapshot(&wire).expect("decodes");
            accepted += g.apply(&decoded).len();
        }
        assert_eq!(accepted, 5, "every fresh stamped snapshot is accepted once");
    }

    #[test]
    fn encode_is_deterministic() {
        let ents = vec![ent(1, EntityKind::Player, 3), ent(2, EntityKind::Dragon, 9)];
        assert_eq!(encode_snapshot(7, &ents), encode_snapshot(7, &ents));
    }
}
