//! `sim::poi` — Points Of Interest as the creature's near-term decision substrate.
//!
//! A creature balances the salient things it can perceive — **food, shelter,
//! friends (kin/flock), foes (predator/player)** — each a POI with a position, a
//! kind, and a salience. It scores them by **type-weight × distance-falloff** and
//! the result "underfoots" the near-term decision: foe-POIs raise the THREAT drive,
//! and the strongest attractor (food/shelter/friend) is the navigation TARGET the
//! behaviour arbiter steers toward once it picks a non-threat behaviour. This is
//! the unifying substrate the witnessed-attack reaction (a foe POI), the predator
//! hunt (a food POI), and flocking (a friend POI) all reduce to.
//!
//! Cost is local: POIs are the FEW salient things a creature perceives (via sight +
//! `sim::acoustics` hearing), so scoring is cheap per creature — unlike an
//! omniscient herd scan. The type-weights are intended GENETIC (a lineage that
//! values shelter over food), shaped over deep time; off-screen, a microsim batches
//! POI dynamics into a sim-vector rather than scoring per-frame (see
//! `docs/design/acoustic_perception_and_poi.md`).
//!
//! Pure & deterministic: positions/weights in, drives + target out. No clock / RNG
//! / IO, no panic.

use crate::sim::behavior_arbiter::Drives;

/// The four POI categories a creature balances.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoiKind {
    /// Something to eat (forage block, prey).
    Food,
    /// Somewhere safe (cover, den, dense flora).
    Shelter,
    /// Kin / flockmate to be near.
    Friend,
    /// Predator / player / threat to avoid or confront.
    Foe,
}

/// A single perceived point of interest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Poi {
    /// World position (x, y, z).
    pub pos: [f32; 3],
    pub kind: PoiKind,
    /// How strong/important this POI is in `[0,1]` (a big predator vs a small one,
    /// a rich food patch vs a sparse one). Perception sets this.
    pub salience: f32,
}

/// Heritable per-lineage weighting of how much each POI category pulls. Foe weight
/// scales how much a foe raises threat; the others scale attraction. Intended to be
/// a genetic disposition (a skittish species weights Foe high + Shelter high; a
/// bold one weights Foe low).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PoiWeights {
    pub food: f32,
    pub shelter: f32,
    pub friend: f32,
    pub foe: f32,
}

impl Default for PoiWeights {
    fn default() -> Self {
        PoiWeights { food: 1.0, shelter: 1.0, friend: 1.0, foe: 1.0 }
    }
}

/// The creature's own internal body state (NOT spatial) — passed through to the
/// drives the arbiter reads. Hunger/fatigue/health are bodily; aggression is the
/// current aggro/habit level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub hunger: f32,
    pub fatigue: f32,
    pub aggression: f32,
    pub health_frac: f32,
}

/// Horizontal (xz) distance between two points — creatures live on a surface, so
/// vertical separation matters less for POI pull.
#[inline]
fn dist_xz(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Smooth distance falloff in `[0,1]`: 1 at the POI, 0 at/beyond `radius`. Linear
/// in `1 - d/radius` (cheap, monotone). `radius <= 0` ⇒ 0.
#[inline]
pub fn distance_falloff(d: f32, radius: f32) -> f32 {
    if radius <= 0.0 {
        return 0.0;
    }
    (1.0 - d / radius).clamp(0.0, 1.0)
}

/// The pull this POI exerts on a creature at `my_pos`: `salience × type_weight ×
/// distance_falloff`, in `[0,1]` (type_weight assumed ≤ 1 for the normalized range;
/// larger weights just scale up). Always non-negative — direction (approach foe vs
/// flee foe) is the arbiter's call, not the pull's sign.
pub fn poi_pull(poi: &Poi, my_pos: [f32; 3], weights: &PoiWeights, radius: f32) -> f32 {
    let w = match poi.kind {
        PoiKind::Food => weights.food,
        PoiKind::Shelter => weights.shelter,
        PoiKind::Friend => weights.friend,
        PoiKind::Foe => weights.foe,
    };
    let f = distance_falloff(dist_xz(poi.pos, my_pos), radius);
    (poi.salience.clamp(0.0, 1.0) * w.max(0.0) * f).max(0.0)
}

/// The strongest ATTRACTOR POI (Food/Shelter/Friend — not Foe) within radius: the
/// near-term navigation target. Returns `(index, pull)` or `None` if no attractor
/// is in range. Foe POIs are excluded (they drive threat, not attraction).
pub fn dominant_attractor(
    pois: &[Poi],
    my_pos: [f32; 3],
    weights: &PoiWeights,
    radius: f32,
) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f32)> = None;
    for (i, p) in pois.iter().enumerate() {
        if matches!(p.kind, PoiKind::Foe) {
            continue;
        }
        let pull = poi_pull(p, my_pos, weights, radius);
        if pull > 0.0 && best.map_or(true, |(_, b)| pull > b) {
            best = Some((i, pull));
        }
    }
    best
}

/// The threat level from the strongest foe POI within radius, in `[0,1]` — feeds the
/// arbiter's `threat` drive. `0` if no foe is in range.
pub fn threat_from_foes(pois: &[Poi], my_pos: [f32; 3], weights: &PoiWeights, radius: f32) -> f32 {
    pois.iter()
        .filter(|p| matches!(p.kind, PoiKind::Foe))
        .map(|p| poi_pull(p, my_pos, weights, radius))
        .fold(0.0f32, f32::max)
        .clamp(0.0, 1.0)
}

/// Assemble the arbiter's [`Drives`] from perceived POIs + the creature's body.
/// `threat` is the strongest foe-POI pull (combined with any externally-supplied
/// `extra_threat`, e.g. a directly-seen player, via max); the rest pass through
/// from `body`. The arbiter then scores Flee/Fight/Forage/Rest/Patrol; the
/// [`dominant_attractor`] gives the target to move toward for the non-threat picks.
pub fn to_drives(
    pois: &[Poi],
    my_pos: [f32; 3],
    weights: &PoiWeights,
    radius: f32,
    body: Body,
    extra_threat: f32,
) -> Drives {
    let foe_threat = threat_from_foes(pois, my_pos, weights, radius);
    Drives {
        threat: foe_threat.max(extra_threat.clamp(0.0, 1.0)),
        hunger: body.hunger.clamp(0.0, 1.0),
        fatigue: body.fatigue.clamp(0.0, 1.0),
        aggression: body.aggression.clamp(0.0, 1.0),
        health_frac: body.health_frac.clamp(0.0, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poi(kind: PoiKind, x: f32, z: f32, sal: f32) -> Poi {
        Poi { pos: [x, 0.0, z], kind, salience: sal }
    }
    const W: PoiWeights = PoiWeights { food: 1.0, shelter: 1.0, friend: 1.0, foe: 1.0 };
    const ORIGIN: [f32; 3] = [0.0, 0.0, 0.0];

    #[test]
    fn falloff_is_one_at_zero_zero_at_radius() {
        assert!((distance_falloff(0.0, 10.0) - 1.0).abs() < 1e-6);
        assert_eq!(distance_falloff(10.0, 10.0), 0.0);
        assert_eq!(distance_falloff(20.0, 10.0), 0.0);
        assert!((distance_falloff(5.0, 10.0) - 0.5).abs() < 1e-6);
        assert_eq!(distance_falloff(1.0, 0.0), 0.0); // zero radius
    }

    #[test]
    fn nearer_poi_pulls_harder() {
        let near = poi(PoiKind::Food, 2.0, 0.0, 1.0);
        let far = poi(PoiKind::Food, 8.0, 0.0, 1.0);
        assert!(poi_pull(&near, ORIGIN, &W, 10.0) > poi_pull(&far, ORIGIN, &W, 10.0));
    }

    #[test]
    fn salience_and_weight_scale_pull() {
        let big = poi(PoiKind::Food, 2.0, 0.0, 1.0);
        let small = poi(PoiKind::Food, 2.0, 0.0, 0.3);
        assert!(poi_pull(&big, ORIGIN, &W, 10.0) > poi_pull(&small, ORIGIN, &W, 10.0));
        // heritable weight: a shelter-loving lineage pulls harder to shelter
        let shelter = poi(PoiKind::Shelter, 2.0, 0.0, 1.0);
        let skittish = PoiWeights { shelter: 2.0, ..Default::default() };
        assert!(poi_pull(&shelter, ORIGIN, &skittish, 10.0) > poi_pull(&shelter, ORIGIN, &W, 10.0));
    }

    #[test]
    fn dominant_attractor_ignores_foes_and_picks_strongest() {
        let pois = [
            poi(PoiKind::Foe, 1.0, 0.0, 1.0),     // closest but a foe — excluded
            poi(PoiKind::Food, 3.0, 0.0, 1.0),    // strong attractor
            poi(PoiKind::Friend, 9.0, 0.0, 0.5),  // weak/far attractor
        ];
        let (idx, _) = dominant_attractor(&pois, ORIGIN, &W, 20.0).unwrap();
        assert_eq!(idx, 1, "picks the food, not the foe or the far friend");
    }

    #[test]
    fn no_attractor_in_range_is_none() {
        let pois = [poi(PoiKind::Food, 50.0, 0.0, 1.0)]; // out of radius
        assert_eq!(dominant_attractor(&pois, ORIGIN, &W, 10.0), None);
        let only_foe = [poi(PoiKind::Foe, 2.0, 0.0, 1.0)];
        assert_eq!(dominant_attractor(&only_foe, ORIGIN, &W, 10.0), None);
    }

    #[test]
    fn threat_comes_from_nearest_strongest_foe() {
        let pois = [
            poi(PoiKind::Foe, 9.0, 0.0, 1.0),  // far big predator
            poi(PoiKind::Foe, 2.0, 0.0, 1.0),  // near big predator → dominates threat
            poi(PoiKind::Food, 1.0, 0.0, 1.0), // not a foe
        ];
        let t = threat_from_foes(&pois, ORIGIN, &W, 10.0);
        let near_only = threat_from_foes(&[poi(PoiKind::Foe, 2.0, 0.0, 1.0)], ORIGIN, &W, 10.0);
        assert!((t - near_only).abs() < 1e-6, "the near foe sets the threat");
        assert!(t > 0.5);
    }

    #[test]
    fn to_drives_fills_threat_from_foes_and_passes_body() {
        let pois = [poi(PoiKind::Foe, 2.0, 0.0, 1.0)];
        let body = Body { hunger: 0.7, fatigue: 0.2, aggression: 0.4, health_frac: 0.9 };
        let d = to_drives(&pois, ORIGIN, &W, 10.0, body, 0.0);
        assert!(d.threat > 0.5, "foe POI drove threat");
        assert_eq!(d.hunger, 0.7);
        assert_eq!(d.aggression, 0.4);
        assert_eq!(d.health_frac, 0.9);
    }

    #[test]
    fn extra_threat_combines_by_max() {
        // a directly-seen player (extra_threat) with no foe POI still raises threat.
        let body = Body { hunger: 0.0, fatigue: 0.0, aggression: 0.0, health_frac: 1.0 };
        let d = to_drives(&[], ORIGIN, &W, 10.0, body, 0.8);
        assert_eq!(d.threat, 0.8);
    }

    #[test]
    fn determinism() {
        let pois = [poi(PoiKind::Food, 3.0, 1.0, 0.8), poi(PoiKind::Foe, 5.0, 2.0, 0.6)];
        let body = Body { hunger: 0.5, fatigue: 0.1, aggression: 0.2, health_frac: 0.8 };
        let a = to_drives(&pois, ORIGIN, &W, 12.0, body, 0.1);
        let b = to_drives(&pois, ORIGIN, &W, 12.0, body, 0.1);
        assert_eq!(a, b);
    }
}
