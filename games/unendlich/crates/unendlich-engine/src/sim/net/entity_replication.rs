//! `sim::net::entity_replication` — the host→client transform round trip.
//!
//! [`entity_snapshot`](crate::sim::net::entity_snapshot) ships the three pieces of
//! host-authoritative transform replication separately: the [`AuthorityStamper`]
//! (host's per-entity sequence source), the snapshot wire codec, and the
//! [`SnapshotGate`] (client newest-wins). This composes them into the two calls a
//! host and a client actually make: [`seal`] stamps a set of transforms and frames
//! a snapshot for the Snapshots channel; [`receive`] decodes one and applies it
//! newest-wins, dropping stale/duplicate/reordered packets.
//!
//! Pure: stamping + framing + gating only; the socket send/recv stays bin glue.

use crate::sim::net::entity_snapshot::{
    decode_snapshot, encode_snapshot, AuthorityStamper, EntityState, SnapshotGate,
};

/// Host: stamp every entity with its next authority sequence, then frame a
/// snapshot at `host_tick`. Any `seq` already on the inputs is overwritten — the
/// stamper is the single source of truth (§C, host owns authority).
pub fn seal(stamper: &mut AuthorityStamper, host_tick: u64, entities: &[EntityState]) -> Vec<u8> {
    let stamped: Vec<EntityState> =
        entities.iter().map(|e| EntityState { seq: stamper.next(e.id), ..*e }).collect();
    encode_snapshot(host_tick, &stamped)
}

/// Client: decode a snapshot and apply it newest-wins, returning the transforms
/// actually accepted (newer than what's tracked). `None` on malformed/truncated
/// bytes; an empty vec means every entity in the packet was stale or a duplicate
/// (the unreliable-sequenced contract — a dropped packet is simply superseded).
pub fn receive(gate: &mut SnapshotGate, bytes: &[u8]) -> Option<Vec<EntityState>> {
    let snap = decode_snapshot(bytes)?;
    Some(gate.apply(&snap))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixed::Fixed;
    use crate::sim::net::entity_snapshot::EntityKind;

    fn ent(id: u64, x: i64) -> EntityState {
        EntityState {
            id,
            kind: EntityKind::Player,
            x: Fixed(x),
            y: Fixed(0),
            z: Fixed(0),
            yaw: 0,
            pitch: 0,
            seq: 0, // ignored by seal
        }
    }

    #[test]
    fn round_trip_applies_all_first_time() {
        let mut stamper = AuthorityStamper::new();
        let mut gate = SnapshotGate::new();
        let bytes = seal(&mut stamper, 10, &[ent(1, 100), ent(2, 200), ent(3, 300)]);
        let applied = receive(&mut gate, &bytes).expect("decodes");
        assert_eq!(applied.len(), 3, "first broadcast — all entities are new");
        assert_eq!(gate.tracked(), 3);
    }

    #[test]
    fn second_broadcast_supersedes() {
        let mut stamper = AuthorityStamper::new();
        let mut gate = SnapshotGate::new();
        let _ = receive(&mut gate, &seal(&mut stamper, 1, &[ent(1, 1), ent(2, 2)])).unwrap();
        // host moves them and re-broadcasts (seq advances) → both accepted again.
        let applied = receive(&mut gate, &seal(&mut stamper, 2, &[ent(1, 9), ent(2, 9)])).unwrap();
        assert_eq!(applied.len(), 2);
        assert_eq!(applied.iter().find(|e| e.id == 1).unwrap().x, Fixed(9));
    }

    #[test]
    fn reordered_stale_snapshot_is_dropped() {
        let mut stamper = AuthorityStamper::new();
        let mut gate = SnapshotGate::new();
        let older = seal(&mut stamper, 1, &[ent(1, 10)]); // seq 0
        let newer = seal(&mut stamper, 2, &[ent(1, 20)]); // seq 1
        // Deliver newest first, then the stale one arrives late.
        assert_eq!(receive(&mut gate, &newer).unwrap().len(), 1);
        assert!(receive(&mut gate, &older).unwrap().is_empty(), "stale packet dropped");
    }

    #[test]
    fn duplicate_is_dropped() {
        let mut stamper = AuthorityStamper::new();
        let mut gate = SnapshotGate::new();
        let bytes = seal(&mut stamper, 1, &[ent(7, 5)]);
        assert_eq!(receive(&mut gate, &bytes).unwrap().len(), 1);
        assert!(receive(&mut gate, &bytes).unwrap().is_empty(), "exact duplicate dropped");
    }

    #[test]
    fn per_entity_independence() {
        let mut stamper = AuthorityStamper::new();
        let mut gate = SnapshotGate::new();
        let _ = receive(&mut gate, &seal(&mut stamper, 1, &[ent(1, 1), ent(2, 2)])).unwrap();
        // Only entity 1 moves and is re-sent; 2 stays tracked, untouched.
        let applied = receive(&mut gate, &seal(&mut stamper, 2, &[ent(1, 99)])).unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].id, 1);
        assert_eq!(gate.tracked(), 2);
    }

    #[test]
    fn malformed_is_none() {
        let mut gate = SnapshotGate::new();
        assert!(receive(&mut gate, b"not a snapshot").is_none());
        assert!(receive(&mut gate, &[]).is_none());
    }

    #[test]
    fn host_sequence_increments_across_seals() {
        let mut stamper = AuthorityStamper::new();
        let s0 = decode_snapshot(&seal(&mut stamper, 1, &[ent(1, 0)])).unwrap();
        let s1 = decode_snapshot(&seal(&mut stamper, 2, &[ent(1, 0)])).unwrap();
        assert_eq!(s0.entities[0].seq, 0);
        assert_eq!(s1.entities[0].seq, 1);
    }
}
