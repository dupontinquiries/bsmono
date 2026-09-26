//! ferry — clouds that carry cats across the sky.
//!
//! On Starwell Glacier the chasms between islands are open sky, and a cloud
//! shuttles across each on a timetable: it waits at one pier, drifts over,
//! waits at the other, drifts back. A cat LEAPS onto it (walking cannot:
//! the floor ends at the pier), rides it -- free to move about the deck,
//! carried with it -- and leaps off, or walks off once it has docked. A
//! leap that meets neither cloud nor island drops the cat into the cloud
//! sea; the Starwell carries it home a few seconds later (`SKYFALL_SECS`).
//!
//! Pure and deterministic: a cloud's position is a function of the match
//! clock, so every peer and every replay agrees without syncing anything.
//! The sim (`systems::move_gods`) asks [`Ferry::at`] each tick; the leap
//! planner (`leap::plan`) is told where each cloud WILL be when the leap
//! lands ([`Platform`]).

use glam::Vec3;

/// Radius of a cloud's deck: room for three or four cats.
pub const CLOUD_R: f32 = 5.0;
/// Seconds a cloud waits at each pier.
pub const DWELL: f32 = 4.0;
/// Drift speed across the chasm, u/s (the middle of the trip; it eases in
/// and out).
pub const SPEED: f32 = 6.5;
/// A docked cloud's centre sits this far out from the pier, over the sky:
/// its deck overlaps the island's edge by `CLOUD_R - DOCK_OUT`.
pub const DOCK_OUT: f32 = 3.2;
/// Falling into the cloud sea: this long until the Starwell sets the cat
/// down at its fountain.
pub const SKYFALL_SECS: f32 = 3.0;
/// ...and it costs this fraction of max health (not a death: no gold, no
/// kill, no respawn timer).
pub const SKYFALL_COST: f32 = 0.15;

/// Where a cloud is: its deck's centre, how far it moved since `dt` ago, and
/// which pier it is docked at (0 = `a`, 1 = `b`), if any.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudAt {
    pub pos: Vec3,
    pub docked: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ferry {
    /// The two piers (points on the islands' edges).
    pub a: Vec3,
    pub b: Vec3,
    /// Offset into the timetable, seconds, so clouds do not move in step.
    pub phase: f32,
}

/// A cloud as the leap planner sees it: where it will be, and its radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Platform {
    pub at: Vec3,
    pub r: f32,
}

impl Ferry {
    pub fn new(a: Vec3, b: Vec3, phase: f32) -> Ferry {
        Ferry { a, b, phase }
    }

    /// The two dock positions (deck centres), just off each pier.
    pub fn docks(&self) -> (Vec3, Vec3) {
        let d = (self.b - self.a).normalize_or_zero();
        (self.a + d * DOCK_OUT, self.b - d * DOCK_OUT)
    }

    /// Seconds for one crossing.
    pub fn crossing(&self) -> f32 {
        let (p, q) = self.docks();
        // eased: the average speed of a smoothstep is 2/3 of its peak... the
        // other way round: to peak at SPEED the trip takes 1.5x dist/SPEED
        1.5 * (q - p).length() / SPEED
    }

    /// One full round trip.
    pub fn period(&self) -> f32 {
        2.0 * (DWELL + self.crossing())
    }

    /// Where the cloud is at match time `t`.
    pub fn at(&self, t: f32) -> CloudAt {
        let (p, q) = self.docks();
        let c = self.crossing();
        let u = (t + self.phase).rem_euclid(self.period());
        let ease = |x: f32| x * x * (3.0 - 2.0 * x);
        if u < DWELL {
            CloudAt { pos: p, docked: Some(0) }
        } else if u < DWELL + c {
            CloudAt { pos: p.lerp(q, ease((u - DWELL) / c)), docked: None }
        } else if u < 2.0 * DWELL + c {
            CloudAt { pos: q, docked: Some(1) }
        } else {
            CloudAt { pos: q.lerp(p, ease((u - 2.0 * DWELL - c) / c)), docked: None }
        }
    }

    /// Is (x, z) on this cloud's deck at time `t`, with `margin` to spare?
    pub fn on_deck(&self, t: f32, x: f32, z: f32, margin: f32) -> bool {
        let p = self.at(t).pos;
        (p.x - x).hypot(p.z - z) <= CLOUD_R - margin
    }
}

/// Walk from `from` toward `to` until the floor ends: the island's true
/// edge on that line (the authored piers are approximate).
fn edge(floor: &crate::floor::FloorPlan, from: Vec3, to: Vec3) -> Vec3 {
    let d = (to - from).normalize_or_zero();
    // back onto the island, then march out
    let mut p = from;
    for _ in 0..200 {
        if floor.sdf(p.x, p.z) < -0.5 {
            break;
        }
        p -= d * 0.1;
    }
    for _ in 0..400 {
        if floor.sdf(p.x, p.z) > 0.0 {
            return p;
        }
        p += d * 0.1;
    }
    from
}

/// The active map's ferries, their phases spread through the timetable.
/// Piers are moved onto the islands' true edges, so a docked deck overlaps
/// the floor by exactly `CLOUD_R - DOCK_OUT`.
pub fn for_map() -> Vec<Ferry> {
    match crate::map::active() {
        crate::map::MapId::Sky => crate::sky::ferry_routes()
            .into_iter()
            .map(|(a, b)| {
                let f = crate::sky::floor();
                (edge(f, a, b), edge(f, b, a))
            })
            .enumerate()
            .map(|(k, (a, b))| {
                // mirrored pairs are half a period apart, so the two halves
                // of the map are equally served (one leaves as the other
                // arrives)
                let f = Ferry::new(a, b, 0.0);
                Ferry { phase: (k % 3) as f32 * 3.1 + if k >= 3 { f.period() * 0.5 } else { 0.0 }, ..f }
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Keep a rider on the deck: its position clamped to the disc round the
/// cloud's centre.
pub fn clamp_to_deck(pos: Vec3, deck: Vec3, radius: f32) -> Vec3 {
    let off = Vec3::new(pos.x - deck.x, 0.0, pos.z - deck.z);
    let max = CLOUD_R - radius;
    if off.length() <= max {
        pos
    } else {
        let o = off.normalize_or_zero() * max;
        Vec3::new(deck.x + o.x, pos.y, deck.z + o.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ferry() -> Ferry {
        Ferry::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(40.0, 0.0, 0.0), 0.0)
    }

    #[test]
    fn a_cloud_waits_crosses_waits_and_comes_back() {
        let f = ferry();
        let (p, q) = f.docks();
        assert_eq!(f.at(0.5), CloudAt { pos: p, docked: Some(0) });
        let mid = f.at(DWELL + f.crossing() * 0.5);
        assert!(mid.docked.is_none() && (mid.pos - p.lerp(q, 0.5)).length() < 1e-3);
        assert_eq!(f.at(DWELL + f.crossing() + 0.5), CloudAt { pos: q, docked: Some(1) });
        assert_eq!(f.at(f.period() + 0.5).pos, p, "periodic");
        // continuous: no jump anywhere in two periods
        let mut last = f.at(0.0).pos;
        let dt = 1.0 / 60.0;
        let mut top = 0.0f32;
        for k in 1..(f.period() * 2.0 / dt) as usize {
            let now = f.at(k as f32 * dt).pos;
            top = top.max((now - last).length() / dt);
            last = now;
        }
        assert!(top <= SPEED * 1.02 && top > SPEED * 0.9, "peak drift {top} u/s, asked {SPEED}");
    }

    #[test]
    fn a_docked_deck_overlaps_its_pier() {
        let f = ferry();
        let (p, _) = f.docks();
        assert!((p - f.a).length() < CLOUD_R - 1.0, "the pier is on the deck: a cat can walk off");
    }

    #[test]
    fn a_rider_stays_on_the_deck() {
        let deck = Vec3::new(10.0, 0.0, 5.0);
        let inside = Vec3::new(11.0, 0.0, 6.0);
        assert_eq!(clamp_to_deck(inside, deck, 0.55), inside);
        let out = clamp_to_deck(Vec3::new(30.0, 0.0, 5.0), deck, 0.55);
        assert!(((out - deck).length() - (CLOUD_R - 0.55)).abs() < 1e-4);
    }

    #[test]
    fn the_sky_map_has_six_ferries_and_mirrors_are_out_of_step() {
        crate::map::with(crate::map::MapId::Sky, || {
            let v = for_map();
            assert_eq!(v.len(), 6);
            for k in 0..3 {
                let (f, g) = (v[k], v[k + 3]);
                assert!((f.a.z + g.a.z).abs() < 1e-3, "{k}: a mirrored pair");
                let t = 1.0;
                let (pf, pg) = (f.at(t).pos, g.at(t).pos);
                assert!((pf - Vec3::new(pg.x, pg.y, -pg.z)).length() > 1.0, "{k}: half a period apart, not in step");
            }
        });
        assert!(for_map().is_empty(), "no ferries on the jungle");
    }
}
