//! `sim::net::lag_compensation` — server-side hit rewind ("lag compensation").
//!
//! When a client fires at their local render time, they are aiming at where they
//! *saw* the targets — which is `INTERP_DELAY + attacker_rtt/2` behind the host's
//! current authoritative state (the client renders remote entities interpolated
//! that far in the past; see `snapshot_interp`). To resolve the shot fairly, the
//! host **rewinds** each candidate target's position back to the attacker's
//! view-time, tests the shot against those rewound positions, and applies the
//! result there. This is the Valve/Source model; it's the server-side mirror of
//! the client-side interpolation — the rewind reconstructs exactly what the
//! attacker's screen showed.
//!
//! Design:
//! * The host records authoritative positions into a per-entity [`EntityTrack`]
//!   ring (keyed in a `BTreeMap` for deterministic iteration).
//! * [`RewindLog::rewind`] interpolates a target's position to any past time,
//!   clamped to the samples it has.
//! * [`view_time`] converts an attacker's latency into the rewind time, clamped
//!   so no shot can rewind further than [`MAX_REWIND`] (anti-abuse) or past the
//!   retained [`HISTORY_WINDOW`].
//! * A small hitscan helper ([`ray_sphere`] / [`resolve_shot`]) tests a shot ray
//!   against the rewound target spheres and returns the nearest hit.
//!
//! Pure/std-only: `f32`/`f64` math + `BTreeMap`/`VecDeque`. Times are seconds
//! (`f64`) on the host clock; positions are `f32` world units (the caller
//! converts `entity_replication`'s `Fixed` → `f32`).

use std::collections::{BTreeMap, VecDeque};

/// How far back the host keeps position history. Samples older than this behind
/// the newest recorded time are pruned. Must exceed the largest expected
/// `view_time` rewind (RTT/2 + interp delay).
pub const HISTORY_WINDOW: f64 = 1.0;

/// Hard cap on how far a shot may rewind, regardless of reported latency. Caps
/// the fairness window and blocks a client from claiming an absurd RTT to hit
/// long-gone positions.
pub const MAX_REWIND: f64 = 0.5;

/// The interpolation delay the client renders remote entities at (mirrors
/// `snapshot_interp::INTERP_DELAY`). The host rewinds by this much *plus* the
/// one-way latency so the rewound state matches the attacker's screen.
pub const INTERP_DELAY: f64 = 0.10;

/// Per-entity ring of timestamped positions is capped at this many samples so a
/// high tick-rate can't grow the history unbounded within [`HISTORY_WINDOW`].
pub const TRACK_CAP: usize = 64;

/// One recorded authoritative position at a host time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PosSample {
    pub time: f64,
    pub pos: [f32; 3],
}

/// A single entity's recent position history (newest at the back).
#[derive(Clone, Debug, Default)]
pub struct EntityTrack {
    samples: VecDeque<PosSample>,
}

impl EntityTrack {
    pub fn new() -> Self {
        EntityTrack { samples: VecDeque::new() }
    }

    /// Record a position at `time`. Samples must arrive in non-decreasing time
    /// order (as the host tick loop produces them); an out-of-order or duplicate
    /// timestamp is dropped rather than corrupting the interpolation order.
    pub fn record(&mut self, time: f64, pos: [f32; 3]) {
        if let Some(last) = self.samples.back() {
            if time <= last.time {
                return;
            }
        }
        self.samples.push_back(PosSample { time, pos });
        while self.samples.len() > TRACK_CAP {
            self.samples.pop_front();
        }
    }

    /// Drop samples strictly older than `oldest_keep`.
    pub fn prune(&mut self, oldest_keep: f64) {
        while self.samples.len() > 1 {
            // keep at least one so a stale-but-present entity can still be hit
            if self.samples.front().map_or(false, |s| s.time < oldest_keep) {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Newest recorded time, if any.
    pub fn latest_time(&self) -> Option<f64> {
        self.samples.back().map(|s| s.time)
    }

    /// The position at `time`, interpolating between the two bracketing samples.
    /// Before the oldest sample it clamps to the oldest; after the newest it
    /// clamps to the newest (the host never extrapolates the authority forward —
    /// it *has* the newest state, it's only reconstructing the past). `None` only
    /// when there is no history at all.
    pub fn position_at(&self, time: f64) -> Option<[f32; 3]> {
        let n = self.samples.len();
        if n == 0 {
            return None;
        }
        let front = self.samples.front().unwrap();
        let back = self.samples.back().unwrap();
        if time <= front.time {
            return Some(front.pos);
        }
        if time >= back.time {
            return Some(back.pos);
        }
        // find the bracketing pair [a, b] with a.time <= time <= b.time
        // (linear scan from the back — the query is usually near the newest end)
        let mut b = self.samples.back().unwrap();
        for pair in self.samples.iter().rev() {
            if pair.time <= time {
                let a = pair;
                let span = b.time - a.time;
                let t = if span > 0.0 { ((time - a.time) / span) as f32 } else { 0.0 };
                return Some(lerp3(a.pos, b.pos, t));
            }
            b = pair;
        }
        Some(front.pos)
    }
}

/// The host's rewind log: every tracked entity's recent position history.
#[derive(Clone, Debug, Default)]
pub struct RewindLog {
    tracks: BTreeMap<u64, EntityTrack>,
    newest: f64,
}

impl RewindLog {
    pub fn new() -> Self {
        RewindLog { tracks: BTreeMap::new(), newest: f64::NEG_INFINITY }
    }

    /// Record `id`'s authoritative position at host `time`, then prune globally
    /// so nothing older than [`HISTORY_WINDOW`] behind the newest survives.
    pub fn record(&mut self, id: u64, time: f64, pos: [f32; 3]) {
        self.tracks.entry(id).or_default().record(time, pos);
        if time > self.newest {
            self.newest = time;
        }
        let cutoff = self.newest - HISTORY_WINDOW;
        for t in self.tracks.values_mut() {
            t.prune(cutoff);
        }
    }

    /// Forget an entity entirely (despawn / disconnect).
    pub fn remove(&mut self, id: u64) {
        self.tracks.remove(&id);
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Rewind one entity to `time`. `None` if it isn't tracked / has no history.
    pub fn rewind(&self, id: u64, time: f64) -> Option<[f32; 3]> {
        self.tracks.get(&id).and_then(|t| t.position_at(time))
    }

    /// Rewind every tracked entity to `time`, excluding `exclude` (the attacker
    /// shouldn't be a target of their own shot). Deterministic order (sorted by
    /// id via `BTreeMap`).
    pub fn rewind_all(&self, time: f64, exclude: u64) -> Vec<(u64, [f32; 3])> {
        self.tracks
            .iter()
            .filter(|(id, _)| **id != exclude)
            .filter_map(|(id, t)| t.position_at(time).map(|p| (*id, p)))
            .collect()
    }
}

/// The host time a shot should be resolved against, given the attacker's
/// round-trip time. Rewinds by one-way latency (`rtt/2`) plus [`INTERP_DELAY`],
/// clamped to `[server_now - MAX_REWIND, server_now]`. Reported `rtt` is clamped
/// non-negative first.
pub fn view_time(server_now: f64, attacker_rtt: f64, interp_delay: f64) -> f64 {
    let rtt = attacker_rtt.max(0.0);
    let rewind = (rtt * 0.5 + interp_delay).min(MAX_REWIND);
    server_now - rewind
}

/// A resolved hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub id: u64,
    /// Distance along the ray to the entry point.
    pub dist: f32,
}

/// Ray/sphere intersection. `dir` need not be normalized; the returned `t` is in
/// units of `dir` length. Returns the nearest non-negative entry distance, or
/// `None` if the ray misses or the sphere is entirely behind the origin.
pub fn ray_sphere(origin: [f32; 3], dir: [f32; 3], center: [f32; 3], radius: f32) -> Option<f32> {
    let oc = sub3(origin, center);
    let a = dot3(dir, dir);
    if a <= 0.0 {
        return None;
    }
    let b = 2.0 * dot3(oc, dir);
    let c = dot3(oc, oc) - radius * radius;
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None;
    }
    let sq = disc.sqrt();
    let t0 = (-b - sq) / (2.0 * a);
    let t1 = (-b + sq) / (2.0 * a);
    // nearest non-negative root (origin inside sphere → t0 < 0 <= t1 → hit at 0)
    if t0 >= 0.0 {
        Some(t0)
    } else if t1 >= 0.0 {
        Some(0.0)
    } else {
        None
    }
}

/// Resolve a hitscan shot against a set of rewound target spheres, returning the
/// nearest hit. `targets` is `(id, center, radius)` — typically the output of
/// [`RewindLog::rewind_all`] paired with each entity's hit radius. Ties broken by
/// smaller id for determinism.
pub fn resolve_shot(
    origin: [f32; 3],
    dir: [f32; 3],
    targets: &[(u64, [f32; 3], f32)],
) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    for &(id, center, radius) in targets {
        if let Some(t) = ray_sphere(origin, dir, center, radius) {
            let take = match best {
                None => true,
                Some(h) => t < h.dist || (t == h.dist && id < h.id),
            };
            if take {
                best = Some(Hit { id, dist: t });
            }
        }
    }
    best
}

// ---- small vector helpers (kept local so the module stays std-only) ----

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_interpolates_midpoint() {
        let mut t = EntityTrack::new();
        t.record(0.0, [0.0, 0.0, 0.0]);
        t.record(1.0, [10.0, 0.0, 0.0]);
        let p = t.position_at(0.5).unwrap();
        assert!((p[0] - 5.0).abs() < 1e-4, "midpoint lerp");
    }

    #[test]
    fn track_clamps_to_ends() {
        let mut t = EntityTrack::new();
        t.record(1.0, [1.0, 0.0, 0.0]);
        t.record(2.0, [2.0, 0.0, 0.0]);
        assert_eq!(t.position_at(0.0).unwrap(), [1.0, 0.0, 0.0], "before oldest → oldest");
        assert_eq!(t.position_at(9.0).unwrap(), [2.0, 0.0, 0.0], "after newest → newest (no fwd extrapolation)");
    }

    #[test]
    fn out_of_order_and_dupes_dropped() {
        let mut t = EntityTrack::new();
        t.record(1.0, [1.0, 0.0, 0.0]);
        t.record(1.0, [9.0, 9.0, 9.0]); // dup time
        t.record(0.5, [8.0, 8.0, 8.0]); // older
        assert_eq!(t.len(), 1);
        assert_eq!(t.latest_time(), Some(1.0));
    }

    #[test]
    fn rewind_reconstructs_a_moving_target() {
        // target walks +x at 10 u/s; a shot rewound to t=1.5 should see x≈15
        let mut log = RewindLog::new();
        for i in 0..=20 {
            let t = i as f64 * 0.1;
            log.record(7, t, [(t as f32) * 10.0, 0.0, 0.0]);
        }
        let p = log.rewind(7, 1.5).unwrap();
        assert!((p[0] - 15.0).abs() < 0.1, "rewound to past position, got {:?}", p);
    }

    #[test]
    fn view_time_subtracts_latency_and_interp_and_clamps() {
        // rtt 0.2 → one-way 0.1, + interp 0.1 → rewind 0.2
        assert!((view_time(10.0, 0.2, INTERP_DELAY) - 9.8).abs() < 1e-9);
        // absurd rtt is clamped to MAX_REWIND
        assert!((view_time(10.0, 100.0, INTERP_DELAY) - (10.0 - MAX_REWIND)).abs() < 1e-9);
        // negative rtt treated as 0
        assert!((view_time(10.0, -5.0, 0.0) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn pruning_drops_stale_but_keeps_one() {
        let mut log = RewindLog::new();
        log.record(1, 0.0, [0.0, 0.0, 0.0]);
        log.record(1, 0.1, [1.0, 0.0, 0.0]);
        // advance well past HISTORY_WINDOW
        log.record(1, 5.0, [50.0, 0.0, 0.0]);
        // old samples pruned; newest still present and hittable
        assert!(log.rewind(1, 5.0).is_some());
        let track_len = log.tracks.get(&1).unwrap().len();
        assert!(track_len >= 1 && track_len < 3, "stale pruned, len={track_len}");
    }

    #[test]
    fn ray_sphere_hits_and_misses() {
        // ray down +x from origin; sphere at (5,0,0) r=1 → hit at t≈4
        let hit = ray_sphere([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [5.0, 0.0, 0.0], 1.0).unwrap();
        assert!((hit - 4.0).abs() < 1e-4);
        // parallel miss (offset in y beyond radius)
        assert!(ray_sphere([0.0, 5.0, 0.0], [1.0, 0.0, 0.0], [5.0, 0.0, 0.0], 1.0).is_none());
        // sphere behind the origin → miss
        assert!(ray_sphere([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [-5.0, 0.0, 0.0], 1.0).is_none());
    }

    #[test]
    fn resolve_shot_picks_nearest() {
        let targets = [
            (2u64, [8.0, 0.0, 0.0], 1.0),
            (1u64, [4.0, 0.0, 0.0], 1.0),
            (3u64, [0.0, 9.0, 0.0], 1.0), // off-axis miss
        ];
        let hit = resolve_shot([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], &targets).unwrap();
        assert_eq!(hit.id, 1, "nearest along the ray");
    }

    #[test]
    fn rewind_all_excludes_attacker_and_is_deterministic() {
        let mut log = RewindLog::new();
        log.record(3, 1.0, [3.0, 0.0, 0.0]);
        log.record(1, 1.0, [1.0, 0.0, 0.0]);
        log.record(2, 1.0, [2.0, 0.0, 0.0]);
        let out = log.rewind_all(1.0, /*exclude*/ 2);
        let ids: Vec<u64> = out.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![1, 3], "attacker excluded, sorted by id");
    }

    #[test]
    fn end_to_end_rewound_hit_beats_current_miss() {
        // Target moving +x fast. Attacker (id 99) with 200ms rtt fired at a
        // spot the target has since left. Rewinding to the attacker's view-time
        // should land the hit where "current" authority would miss.
        let mut log = RewindLog::new();
        for i in 0..=30 {
            let t = i as f64 * 0.05;
            log.record(5, t, [(t as f32) * 20.0, 0.0, 0.0]);
        }
        let server_now = 1.5; // target currently at x=30
        let vt = view_time(server_now, 0.2, INTERP_DELAY); // rewind 0.2s → t=1.3 → x≈26
        let targets: Vec<(u64, [f32; 3], f32)> =
            log.rewind_all(vt, 99).into_iter().map(|(id, p)| (id, p, 0.5)).collect();
        // attacker aimed at x≈26 (where they saw the target)
        let hit = resolve_shot([26.0, -10.0, 0.0], [0.0, 1.0, 0.0], &targets);
        assert!(hit.is_some(), "rewound shot connects");
        assert_eq!(hit.unwrap().id, 5);
        // the same shot against CURRENT authority (x=30) would miss
        let now_targets = [(5u64, [30.0, 0.0, 0.0], 0.5)];
        assert!(resolve_shot([26.0, -10.0, 0.0], [0.0, 1.0, 0.0], &now_targets).is_none());
    }
}
