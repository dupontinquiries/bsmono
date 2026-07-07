//! **Rollback sync check** — the determinism "proof of work" for the rollback
//! netcode: every peer hashes its *confirmed* simulation state each frame and
//! exchanges the digest. Matching digests are the proof that all peers ran
//! identical deterministic work; a mismatch is the desync signal, and
//! [`first_desync`] pinpoints the frame to roll back to and resimulate.
//!
//! This is **not** cryptographic proof-of-work (hashcash). Hashcash buys
//! spam/sybil resistance, not computational-integrity — it cannot prove a peer
//! simulated correctly. The checksum below proves *agreement*: in deterministic
//! lockstep everyone recomputes, so divergence means someone is wrong and you
//! roll back. (For a *malicious* peer you still need an authoritative host or
//! fraud proofs; this detects the common case — accidental desync from a
//! nondeterminism bug or cross-platform float drift.)
//!
//! ## The determinism boundary (why entities have two field sets)
//! Per-player, non-deterministic work — `f32` perception, foveation, prediction,
//! animation — is **speculative** and must never enter the hash, or local
//! divergence poisons consensus. Only **confirmed** fixed-point state (the engine
//! `Fixed` i64s, the integer `stable_noise` detection) is hashed. So an entity
//! "gets these calls" speculatively for presentation, then **re-enters
//! deterministic behaviour** via [`reconcile`]: its authoritative confirmed state
//! (recomputed identically on every peer, or adopted from the agreed rollback)
//! replaces the local guess and its mode flips to [`Mode::Confirmed`]. The proof
//! only ever covers the confirmed track.
//!
//! Determinism rules the confirmed track must obey for the digest to agree:
//! fixed-point only (no `f32` in confirmed fields), canonical iteration (this
//! module sorts by id), seeded RNG only, no wall-clock.
//!
//! ## Analytics
//! Time the digest as a cheap per-frame step and emit a `desync` event to
//! `events.jsonl` when [`first_desync`] fires, so the dashboard can surface
//! desyncs alongside frame pacing.

use crate::sim::gpu_bench::{bench, BenchResult};

const SEED: u64 = 0xCBF2_9CE4_8422_2325;

/// splitmix64-style mix — strong avalanche so any single-field change flips ~half
/// the digest bits.
#[inline]
fn mix(h: u64, x: u64) -> u64 {
    let mut z = h ^ x.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Whether an entity's confirmed state is locally guessed or agreed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Local/predicted; not yet agreed across peers.
    Speculative,
    /// Agreed deterministic state — part of the hashed proof.
    Confirmed,
}

/// A simulated entity split into the deterministic state that the proof covers
/// and the per-player speculative state that it must never touch.
#[derive(Clone, Debug)]
pub struct Entity {
    pub id: u64,
    /// Deterministic fixed-point state (positions, velocities as `Fixed` i64,
    /// hp, integer flags). The **only** thing hashed.
    pub confirmed: Vec<i64>,
    /// Per-player presentation/prediction state (quantized perception, animation
    /// phase, …). Never hashed.
    pub speculative: Vec<i64>,
    pub mode: Mode,
}

impl Entity {
    /// A confirmed entity with no speculative state.
    pub fn confirmed(id: u64, fields: Vec<i64>) -> Self {
        Entity { id, confirmed: fields, speculative: Vec::new(), mode: Mode::Confirmed }
    }
}

/// Compute the confirmed-state digest for a frame: the "proof" each peer
/// exchanges. Iterates entities in canonical id order (so map/iteration order
/// can't matter) and folds only their confirmed fields — speculative state is
/// excluded by construction.
pub fn frame_digest(frame: u64, entities: &[Entity]) -> u64 {
    let mut order: Vec<usize> = (0..entities.len()).collect();
    order.sort_by_key(|&i| entities[i].id);
    let mut h = mix(SEED, frame);
    for &i in &order {
        let e = &entities[i];
        h = mix(h, e.id);
        h = mix(h, e.confirmed.len() as u64);
        for &f in &e.confirmed {
            h = mix(h, f as u64);
        }
    }
    h
}

/// Earliest frame index where two peers' confirmed digest streams diverge — the
/// rollback target. `None` if they agree over the compared range.
pub fn first_desync(local: &[u64], remote: &[u64]) -> Option<usize> {
    let n = local.len().min(remote.len());
    (0..n).find(|&i| local[i] != remote[i])
}

/// Re-enter deterministic behaviour: replace the entity's confirmed state with
/// the authoritative (agreed/recomputed) values and mark it [`Mode::Confirmed`].
/// Speculative state is left for presentation but remains outside the proof.
pub fn reconcile(entity: &mut Entity, authoritative: Vec<i64>) {
    entity.confirmed = authoritative;
    entity.mode = Mode::Confirmed;
}

/// Benchmark the per-frame digest over `iters` runs.
pub fn bench_digest(frame: u64, entities: &[Entity], iters: usize) -> BenchResult {
    bench(iters, || {
        let d = frame_digest(frame, entities);
        std::hint::black_box(d);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ents() -> Vec<Entity> {
        vec![
            Entity::confirmed(42, vec![100, -200, 3]),
            Entity::confirmed(7, vec![1, 2]),
            Entity::confirmed(1000, vec![-5]),
        ]
    }

    #[test]
    fn digest_is_deterministic() {
        assert_eq!(frame_digest(5, &ents()), frame_digest(5, &ents()));
    }

    #[test]
    fn digest_is_order_independent() {
        let a = ents();
        let mut b = ents();
        b.reverse();
        b.swap(0, 1);
        assert_eq!(frame_digest(5, &a), frame_digest(5, &b), "canonical sort should erase input order");
    }

    #[test]
    fn digest_changes_on_any_confirmed_field() {
        let base = frame_digest(5, &ents());
        for ei in 0..3 {
            let mut e = ents();
            let fi = 0;
            e[ei].confirmed[fi] += 1;
            assert_ne!(frame_digest(5, &e), base, "entity {ei} field {fi} change must alter digest");
        }
        // also length-sensitive
        let mut e = ents();
        e[0].confirmed.push(0);
        assert_ne!(frame_digest(5, &e), base);
    }

    #[test]
    fn digest_is_frame_sensitive() {
        assert_ne!(frame_digest(3, &ents()), frame_digest(4, &ents()));
    }

    #[test]
    fn speculative_state_never_affects_proof() {
        let mut a = ents();
        let mut b = ents();
        // identical confirmed, wildly different speculative + mode
        a[0].speculative = vec![1, 2, 3];
        a[0].mode = Mode::Speculative;
        b[0].speculative = vec![999, -999];
        b[0].mode = Mode::Confirmed;
        assert_eq!(frame_digest(9, &a), frame_digest(9, &b));
    }

    #[test]
    fn reconcile_adopts_authoritative_and_confirms() {
        let mut e = Entity { id: 1, confirmed: vec![0], speculative: vec![123], mode: Mode::Speculative };
        let before = frame_digest(0, std::slice::from_ref(&e));
        reconcile(&mut e, vec![7, 8, 9]);
        assert_eq!(e.mode, Mode::Confirmed);
        assert_eq!(e.confirmed, vec![7, 8, 9]);
        assert_eq!(e.speculative, vec![123], "speculative left for presentation");
        assert_ne!(frame_digest(0, std::slice::from_ref(&e)), before, "confirmed state changed → proof changed");
    }

    #[test]
    fn first_desync_localizes() {
        let local: Vec<u64> = (0..10).map(|f| frame_digest(f, &ents())).collect();
        let mut remote = local.clone();
        assert_eq!(first_desync(&local, &remote), None);
        remote[6] ^= 0xDEAD; // peer diverged at frame 6
        assert_eq!(first_desync(&local, &remote), Some(6));
    }

    #[test]
    fn bench_runs() {
        let br = bench_digest(1, &ents(), 3);
        assert_eq!(br.samples.len(), 3);
        assert!(br.metric_line("sync_digest", None).contains("sync_digest n=3"));
    }
}
