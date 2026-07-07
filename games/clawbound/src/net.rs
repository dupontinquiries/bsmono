//! Netcode core — server-authoritative, up to 6 players. Design adapted from
//! cybertron's `netcode.c`: reliable control messages (JOIN / EVENT) vs
//! unreliable per-tick SNAPSHOT / INPUT, each with pack/unpack. Adds snapshot
//! **delta** encoding (send only what changed against a baseline) which is the
//! core bandwidth win for a fast 4-cats-vs-1-hunter game.
//!
//! This module is pure and transport-agnostic (no sockets) so it runs and tests
//! headless; a real build wraps it in WebRTC/UDP and feeds bytes in/out.

pub const MAX_PLAYERS: usize = 6;
pub const PROTOCOL: u16 = 1;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MsgType { CJoin, CInput, SJoinAccept, SSnapshot, SDelta, SEvent }

impl MsgType {
    pub fn tag(self) -> u8 {
        match self { MsgType::CJoin => 1, MsgType::CInput => 2, MsgType::SJoinAccept => 3, MsgType::SSnapshot => 4, MsgType::SDelta => 5, MsgType::SEvent => 6 }
    }
    pub fn from_tag(t: u8) -> Option<MsgType> {
        Some(match t { 1 => MsgType::CJoin, 2 => MsgType::CInput, 3 => MsgType::SJoinAccept, 4 => MsgType::SSnapshot, 5 => MsgType::SDelta, 6 => MsgType::SEvent, _ => return None })
    }
    /// Reliability tier (cf. `ex_net_reliability_for`): control msgs are reliable,
    /// high-frequency state is unreliable (superseded by the next tick).
    pub fn reliable(self) -> bool {
        matches!(self, MsgType::CJoin | MsgType::SJoinAccept | MsgType::SEvent)
    }
}

/// Networked entity. kind: 0 = cat, 1..=5 = hunter archetype (fox/dog/badger/hawk/snake).
/// flags bit0 crouched, bit1 climbing, bit2 hidden, bit3 sprint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityState { pub id: u8, pub kind: u8, pub x: f32, pub z: f32, pub heading: f32, pub flags: u8 }

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot { pub tick: u32, pub ents: Vec<EntityState> }

#[derive(Clone, Debug, PartialEq)]
pub struct Delta { pub tick: u32, pub base_tick: u32, pub changed: Vec<EntityState>, pub removed: Vec<u8> }

// ── little-endian byte helpers ──────────────────────────────────────────────
struct Buf { d: Vec<u8> }
impl Buf {
    fn new(tag: u8) -> Self { Buf { d: vec![tag] } }
    fn u8(&mut self, v: u8) { self.d.push(v); }
    fn u32(&mut self, v: u32) { self.d.extend_from_slice(&v.to_le_bytes()); }
    fn f32(&mut self, v: f32) { self.d.extend_from_slice(&v.to_le_bytes()); }
    fn ent(&mut self, e: &EntityState) { self.u8(e.id); self.u8(e.kind); self.f32(e.x); self.f32(e.z); self.f32(e.heading); self.u8(e.flags); }
}
struct Cur<'a> { d: &'a [u8], p: usize }
impl<'a> Cur<'a> {
    fn new(d: &'a [u8]) -> Self { Cur { d, p: 0 } }
    // bounds-checked read: a truncated/malformed packet yields None instead of panicking
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.d.get(self.p..self.p + n)?;
        self.p += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> { Some(self.take(1)?[0]) }
    fn u32(&mut self) -> Option<u32> { let b = self.take(4)?; Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]])) }
    fn f32(&mut self) -> Option<f32> { let b = self.take(4)?; Some(f32::from_le_bytes([b[0], b[1], b[2], b[3]])) }
    fn ent(&mut self) -> Option<EntityState> {
        Some(EntityState { id: self.u8()?, kind: self.u8()?, x: self.f32()?, z: self.f32()?, heading: self.f32()?, flags: self.u8()? })
    }
}

// ── wire (de)serialization ──────────────────────────────────────────────────
pub fn pack_snapshot(s: &Snapshot) -> Vec<u8> {
    let mut b = Buf::new(MsgType::SSnapshot.tag());
    b.u32(s.tick); b.u8(s.ents.len() as u8);
    for e in &s.ents { b.ent(e); }
    b.d
}
pub fn unpack_snapshot(bytes: &[u8]) -> Option<Snapshot> {
    if bytes.first().copied() != Some(MsgType::SSnapshot.tag()) { return None; }
    let mut c = Cur::new(&bytes[1..]);
    let tick = c.u32()?; let n = c.u8()? as usize;
    let mut ents = Vec::with_capacity(n);
    for _ in 0..n { ents.push(c.ent()?); }
    Some(Snapshot { tick, ents })
}

pub fn pack_delta(d: &Delta) -> Vec<u8> {
    let mut b = Buf::new(MsgType::SDelta.tag());
    b.u32(d.tick); b.u32(d.base_tick);
    b.u8(d.changed.len() as u8);
    for e in &d.changed { b.ent(e); }
    b.u8(d.removed.len() as u8);
    for id in &d.removed { b.u8(*id); }
    b.d
}
pub fn unpack_delta(bytes: &[u8]) -> Option<Delta> {
    if bytes.first().copied() != Some(MsgType::SDelta.tag()) { return None; }
    let mut c = Cur::new(&bytes[1..]);
    let tick = c.u32()?; let base_tick = c.u32()?;
    let nc = c.u8()? as usize; let mut changed = Vec::with_capacity(nc);
    for _ in 0..nc { changed.push(c.ent()?); }
    let nr = c.u8()? as usize; let mut removed = Vec::with_capacity(nr);
    for _ in 0..nr { removed.push(c.u8()?); }
    Some(Delta { tick, base_tick, changed, removed })
}

// ── delta encode / apply ────────────────────────────────────────────────────
pub fn make_delta(base: &Snapshot, cur: &Snapshot) -> Delta {
    let mut changed = Vec::new();
    for e in &cur.ents {
        match base.ents.iter().find(|b| b.id == e.id) {
            Some(b) if b == e => {}          // unchanged → omit
            _ => changed.push(*e),           // new or moved
        }
    }
    let removed = base.ents.iter().filter(|b| !cur.ents.iter().any(|e| e.id == b.id)).map(|b| b.id).collect();
    Delta { tick: cur.tick, base_tick: base.tick, changed, removed }
}
pub fn apply_delta(base: &Snapshot, d: &Delta) -> Snapshot {
    let mut ents: Vec<EntityState> = base.ents.iter().filter(|b| !d.removed.contains(&b.id)).cloned().collect();
    for e in &d.changed {
        if let Some(slot) = ents.iter_mut().find(|x| x.id == e.id) { *slot = *e; } else { ents.push(*e); }
    }
    ents.sort_by_key(|e| e.id);
    Snapshot { tick: d.tick, ents }
}

/// Render-side interpolation between two snapshots (matched by id).
pub fn interpolate(a: &Snapshot, b: &Snapshot, t: f32) -> Vec<EntityState> {
    let t = t.clamp(0.0, 1.0);
    b.ents.iter().map(|eb| {
        if let Some(ea) = a.ents.iter().find(|x| x.id == eb.id) {
            let mut dh = eb.heading - ea.heading;
            while dh > std::f32::consts::PI { dh -= std::f32::consts::TAU; }
            while dh < -std::f32::consts::PI { dh += std::f32::consts::TAU; }
            EntityState { id: eb.id, kind: eb.kind, flags: eb.flags,
                x: ea.x + (eb.x - ea.x) * t, z: ea.z + (eb.z - ea.z) * t, heading: ea.heading + dh * t }
        } else { *eb }
    }).collect()
}

// ── authoritative server ────────────────────────────────────────────────────
#[derive(Clone, Copy)]
struct Input { id: u8, mx: f32, mz: f32, speed: f32 }

pub struct Server { pub tick: u32, pub ents: Vec<EntityState>, next_id: u8, queue: Vec<Input> }

impl Server {
    pub fn new() -> Self { Server { tick: 0, ents: Vec::new(), next_id: 0, queue: Vec::new() } }

    /// Seat a player; returns its entity id, or None if the lobby is full.
    pub fn join(&mut self, kind: u8, x: f32, z: f32) -> Option<u8> {
        if self.ents.len() >= MAX_PLAYERS { return None; }
        let id = self.next_id; self.next_id += 1;
        self.ents.push(EntityState { id, kind, x, z, heading: 0.0, flags: 0 });
        Some(id)
    }

    pub fn queue_input(&mut self, id: u8, mx: f32, mz: f32, speed: f32) {
        self.queue.push(Input { id, mx, mz, speed });
    }

    /// Advance one authoritative tick: apply queued inputs, integrate, bump tick.
    pub fn step(&mut self, dt: f32) {
        for inp in self.queue.drain(..) {
            if let Some(e) = self.ents.iter_mut().find(|e| e.id == inp.id) {
                let len = (inp.mx * inp.mx + inp.mz * inp.mz).sqrt();
                if len > 1e-4 {
                    let (dx, dz) = (inp.mx / len, inp.mz / len);
                    e.x += dx * inp.speed * dt;
                    e.z += dz * inp.speed * dt;
                    e.heading = dx.atan2(dz);
                }
            }
        }
        self.tick += 1;
    }

    pub fn snapshot(&self) -> Snapshot { Snapshot { tick: self.tick, ents: self.ents.clone() } }
}

impl Default for Server { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(tick: u32, ents: &[EntityState]) -> Snapshot { Snapshot { tick, ents: ents.to_vec() } }
    fn e(id: u8, x: f32, z: f32) -> EntityState { EntityState { id, kind: 0, x, z, heading: 0.0, flags: 0 } }

    #[test]
    fn reliability_tiers() {
        assert!(MsgType::CJoin.reliable() && MsgType::SEvent.reliable());
        assert!(!MsgType::SSnapshot.reliable() && !MsgType::CInput.reliable());
    }

    #[test]
    fn snapshot_roundtrip() {
        let s = snap(42, &[e(0, 1.5, -2.0), e(1, 9.0, 3.25)]);
        let back = unpack_snapshot(&pack_snapshot(&s)).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn unpack_survives_truncation_and_bad_counts() {
        // every prefix of a valid packet must parse-or-reject, never panic
        let s = snap(7, &[e(0, 1.0, 2.0), e(1, -4.0, 5.0)]);
        let bytes = pack_snapshot(&s);
        assert_eq!(unpack_snapshot(&bytes), Some(s));
        for k in 0..=bytes.len() { let _ = unpack_snapshot(&bytes[..k]); }

        // a header that claims 200 entities but carries none → None, not a panic
        let mut lying = vec![MsgType::SSnapshot.tag()];
        lying.extend_from_slice(&9u32.to_le_bytes());
        lying.push(200);
        assert_eq!(unpack_snapshot(&lying), None);

        // and the delta path
        let d = Delta { tick: 3, base_tick: 1, changed: vec![e(5, 0.0, 0.0)], removed: vec![9, 4] };
        let db = pack_delta(&d);
        assert_eq!(unpack_delta(&db), Some(d));
        for k in 0..=db.len() { let _ = unpack_delta(&db[..k]); }
    }

    #[test]
    fn delta_roundtrip_and_apply() {
        let base = snap(10, &[e(0, 0.0, 0.0), e(1, 5.0, 5.0), e(2, 1.0, 1.0)]);
        let mut moved = e(1, 5.0, 6.0); moved.heading = 0.3;      // id 1 moved
        let cur = snap(11, &[e(0, 0.0, 0.0), moved]);            // id 2 removed
        let d = make_delta(&base, &cur);
        assert_eq!(d.changed.len(), 1);                         // only id 1
        assert_eq!(d.removed, vec![2]);
        let wire = pack_delta(&d);
        let d2 = unpack_delta(&wire).unwrap();
        assert_eq!(d, d2);
        let rebuilt = apply_delta(&base, &d2);
        assert_eq!(rebuilt, cur);                               // base + delta == current
    }

    #[test]
    fn lobby_caps_at_six() {
        let mut s = Server::new();
        for i in 0..MAX_PLAYERS { assert_eq!(s.join(if i == 0 { 3 } else { 0 }, 0.0, 0.0), Some(i as u8)); }
        assert_eq!(s.join(0, 0.0, 0.0), None);                  // 7th is rejected
    }

    #[test]
    fn input_moves_authoritative_entity() {
        let mut s = Server::new();
        let id = s.join(0, 0.0, 0.0).unwrap();
        s.queue_input(id, 0.0, 1.0, 10.0);                      // north at 10 u/s
        s.step(0.1);
        let e = &s.snapshot().ents[0];
        assert!((e.z - 1.0).abs() < 1e-4 && e.x.abs() < 1e-4);
        assert_eq!(s.tick, 1);
    }

    #[test]
    fn interpolation_halfway() {
        let a = snap(1, &[e(0, 0.0, 0.0)]);
        let b = snap(2, &[e(0, 2.0, 4.0)]);
        let mid = interpolate(&a, &b, 0.5);
        assert!((mid[0].x - 1.0).abs() < 1e-4 && (mid[0].z - 2.0).abs() < 1e-4);
    }
}
