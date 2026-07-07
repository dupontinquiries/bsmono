//! `sim::bending_targeting` — player aim → affected targets, all shapes (WID r66-06).
//!
//! Generalises `bending_stormstrike::route` (chain along aim) to every targeting
//! shape so any discipline can ask: *given my aim, shape, and power, which
//! targets are hit and how hard?* The four geometric primitives — [`ray_hits`],
//! [`cone_hits`], [`area_hits`], [`chain_hits`] — are public; [`resolve_targets`]
//! composes a [`gesture_resolve::TechniqueShape`] and dispatches to the right one
//! (the shape only distinguishes `radial` = omnidirectional ⇒ Area from
//! directional ⇒ Cone; a Ray is a thin cone and Chain is the stormstrike-style
//! hop, both exposed for callers that want them). Vector math mirrors
//! `bending_stormstrike`.
//!
//! `strength01` is geometric attenuation in `[0,1]` (1 dead-centre, 0 at the
//! edge); `power01` scales the effective reach. Pure: logic in, data out — no
//! `crate::render`, no GPU, no I/O, no clock, no RNG, no `.unwrap()`/`.expect()`
//! in non-test code. Release-safe (no `#[cfg]`-gated helpers).

use crate::sim::gesture_resolve::TechniqueShape;

/// A candidate target (sphere).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub pos: [f32; 3],
    pub radius: f32,
}

/// A resolved hit: which target, and how strongly (`1.0` dead-centre … `0` at edge).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub index: usize,
    pub strength01: f32,
}

// world-space reach for a unit `TechniqueShape.range`.
const BASE_REACH: f32 = 16.0;
const RAY_CYL_RADIUS: f32 = 0.5; // thin cylinder half-width (added to target radius)
const CONE_HALF_ANGLE_DEFAULT: f32 = 0.5236; // 30°
const CHAIN_MAX_LINKS_DEFAULT: usize = 4;
const CHAIN_DECAY: f32 = 0.7;

fn fin(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}
fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let l = len(a);
    if l.is_finite() && l > 1e-9 {
        Some([a[0] / l, a[1] / l, a[2] / l])
    } else {
        None
    }
}
fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

/// Smooth distance falloff: `1` at `0`, `0` at/after `max`, smoothstep between.
pub fn falloff(distance: f32, max: f32) -> f32 {
    if !(max > 0.0) || !distance.is_finite() {
        return 0.0;
    }
    if distance <= 0.0 {
        return 1.0;
    }
    if distance >= max {
        return 0.0;
    }
    let t = distance / max; // 0..1
    let s = t * t * (3.0 - 2.0 * t); // smoothstep 0..1
    (1.0 - s).max(0.0)
}

/// Ray: the single nearest target along `aim` within a thin cylinder, out to
/// `reach`. Empty on degenerate aim or no hit.
pub fn ray_hits(origin: [f32; 3], aim: [f32; 3], reach: f32, targets: &[Target]) -> Vec<Hit> {
    let dir = match normalize(aim) {
        Some(d) => d,
        None => return Vec::new(),
    };
    let reach = fin(reach).max(0.0);
    let mut best: Option<(usize, f32, f32)> = None; // (index, t_along, perp)
    for (i, tg) in targets.iter().enumerate() {
        let to = sub(tg.pos, origin);
        let t = dot(to, dir); // distance along the ray
        if t < 0.0 || t > reach {
            continue; // behind, or out of reach
        }
        let closest = [dir[0] * t, dir[1] * t, dir[2] * t];
        let perp = len(sub(to, closest));
        let cyl = fin(tg.radius).max(0.0) + RAY_CYL_RADIUS;
        if perp <= cyl {
            if best.map_or(true, |(_, bt, _)| t < bt) {
                best = Some((i, t, perp / cyl));
            }
        }
    }
    match best {
        Some((i, t, perp_frac)) => {
            // dead-centre along axis = falloff(t); off-centre within the cylinder fades.
            let s = falloff(t, reach) * (1.0 - perp_frac).max(0.0);
            vec![Hit { index: i, strength01: clamp01(s) }]
        }
        None => Vec::new(),
    }
}

/// Cone: every target inside the half-angle and `reach`; strength falls with both
/// off-axis angle and distance. Empty on degenerate aim.
pub fn cone_hits(origin: [f32; 3], aim: [f32; 3], reach: f32, half_angle_rad: f32, targets: &[Target]) -> Vec<Hit> {
    let dir = match normalize(aim) {
        Some(d) => d,
        None => return Vec::new(),
    };
    let reach = fin(reach).max(0.0);
    let half = fin(half_angle_rad).max(1e-4);
    let cos_half = half.cos();
    let mut out = Vec::new();
    for (i, tg) in targets.iter().enumerate() {
        let to = sub(tg.pos, origin);
        let d = len(to);
        if d > reach {
            continue;
        }
        let unit = match normalize(to) {
            Some(u) => u,
            None => {
                // target on top of origin ⇒ dead-centre hit
                out.push(Hit { index: i, strength01: clamp01(falloff(d, reach)) });
                continue;
            }
        };
        let cosang = dot(dir, unit).clamp(-1.0, 1.0);
        if cosang < cos_half {
            continue; // outside the cone
        }
        let ang = cosang.acos();
        let angle_factor = (1.0 - ang / half).max(0.0); // 1 on-axis .. 0 at edge
        let s = falloff(d, reach) * angle_factor;
        out.push(Hit { index: i, strength01: clamp01(s) });
    }
    out
}

/// Area: every target within `radius` of `center`; strength falls with distance.
/// Omnidirectional — works without an aim direction.
pub fn area_hits(center: [f32; 3], radius: f32, targets: &[Target]) -> Vec<Hit> {
    let radius = fin(radius).max(0.0);
    let mut out = Vec::new();
    for (i, tg) in targets.iter().enumerate() {
        let d = len(sub(tg.pos, center));
        let reach = radius + fin(tg.radius).max(0.0);
        if d <= reach {
            out.push(Hit { index: i, strength01: clamp01(falloff(d, reach)) });
        }
    }
    out
}

/// Chain: nearest target along `aim`, then hop forward to the nearest unused
/// target within `hop_range`, up to `max_links`, never repeating. Strength
/// decays each hop (stormstrike-style). Empty on degenerate aim.
pub fn chain_hits(
    origin: [f32; 3],
    aim: [f32; 3],
    hop_range: f32,
    max_links: usize,
    targets: &[Target],
) -> Vec<Hit> {
    let mut dir = match normalize(aim) {
        Some(d) => d,
        None => return Vec::new(),
    };
    let hop_range = fin(hop_range).max(0.0);
    let mut out: Vec<Hit> = Vec::new();
    let mut used = vec![false; targets.len()];
    let mut from = origin;
    let mut strength = 1.0f32;

    for _ in 0..max_links {
        // nearest unused target in front (within the forward half-space) and in range
        let mut best: Option<(usize, f32)> = None;
        for (i, tg) in targets.iter().enumerate() {
            if used[i] {
                continue;
            }
            let to = sub(tg.pos, from);
            let d = len(to);
            if d > hop_range {
                continue;
            }
            let unit = match normalize(to) {
                Some(u) => u,
                None => {
                    best = Some((i, 0.0));
                    break;
                }
            };
            if dot(dir, unit) < 0.0 {
                continue; // behind the current travel direction
            }
            if best.map_or(true, |(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        match best {
            Some((i, d)) => {
                let s = strength * falloff(d, hop_range);
                out.push(Hit { index: i, strength01: clamp01(s) });
                // advance: travel direction points at the node we just hit
                if let Some(u) = normalize(sub(targets[i].pos, from)) {
                    dir = u;
                }
                from = targets[i].pos;
                used[i] = true;
                strength *= CHAIN_DECAY;
            }
            None => break,
        }
    }
    out
}

/// Resolve which targets a technique affects. Composes the [`TechniqueShape`]:
/// `radial` ⇒ Area centred on `origin`; otherwise ⇒ Cone along `aim`. `power01`
/// scales the effective reach (`0` ⇒ nothing). A degenerate (zero) `aim` yields
/// a centred Area result for radial shapes and an empty result otherwise.
pub fn resolve_targets(
    origin: [f32; 3],
    aim: [f32; 3],
    shape: &TechniqueShape,
    targets: &[Target],
    power01: f32,
) -> Vec<Hit> {
    let reach = BASE_REACH * fin(shape.range).max(0.0) * clamp01(power01);
    if shape.radial {
        area_hits(origin, reach, targets)
    } else {
        cone_hits(origin, aim, reach, CONE_HALF_ANGLE_DEFAULT, targets)
    }
}

/// Default chain parameters for callers that don't specify (`CHAIN_MAX_LINKS_DEFAULT`).
pub fn chain_default_links() -> usize {
    CHAIN_MAX_LINKS_DEFAULT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tgt(pos: [f32; 3]) -> Target {
        Target { pos, radius: 0.5 }
    }

    #[test]
    fn falloff_endpoints_and_monotone() {
        assert_eq!(falloff(0.0, 10.0), 1.0);
        assert_eq!(falloff(10.0, 10.0), 0.0);
        assert_eq!(falloff(20.0, 10.0), 0.0);
        assert_eq!(falloff(5.0, 0.0), 0.0, "zero max ⇒ 0");
        assert_eq!(falloff(f32::NAN, 10.0), 0.0);
        let (mut prev, mut d) = (2.0f32, 0.0f32);
        while d <= 10.0 {
            let f = falloff(d, 10.0);
            assert!((0.0..=1.0).contains(&f));
            assert!(f <= prev + 1e-6, "monotone non-increasing");
            prev = f;
            d += 0.5;
        }
    }

    #[test]
    fn ray_hits_nearest_in_front_only() {
        let origin = [0.0, 0.0, 0.0];
        let aim = [0.0, 0.0, 1.0];
        let targets = [
            tgt([0.0, 0.0, 5.0]),   // on axis, near
            tgt([0.0, 0.0, 9.0]),   // on axis, far
            tgt([0.0, 0.0, -5.0]),  // behind ⇒ never
            tgt([6.0, 0.0, 5.0]),   // far off-axis ⇒ outside cylinder
        ];
        let hits = ray_hits(origin, aim, 20.0, &targets);
        assert_eq!(hits.len(), 1, "ray returns the single nearest");
        assert_eq!(hits[0].index, 0, "nearest on-axis target");
        assert!(hits[0].strength01 > 0.0 && hits[0].strength01 <= 1.0);
        // out of reach ⇒ nothing
        assert!(ray_hits(origin, aim, 3.0, &targets).is_empty());
        // degenerate aim ⇒ empty
        assert!(ray_hits(origin, [0.0, 0.0, 0.0], 20.0, &targets).is_empty());
    }

    #[test]
    fn cone_strength_falls_with_angle_and_excludes_outside() {
        let origin = [0.0, 0.0, 0.0];
        let aim = [0.0, 0.0, 1.0];
        let on_axis = tgt([0.0, 0.0, 5.0]);
        let off_axis = tgt([2.0, 0.0, 5.0]); // within 30°? atan(2/5)=21.8° < 30 ⇒ in
        let wide = tgt([6.0, 0.0, 5.0]); // atan(6/5)=50° > 30 ⇒ out
        let targets = [on_axis, off_axis, wide];
        let hits = cone_hits(origin, aim, 20.0, CONE_HALF_ANGLE_DEFAULT, &targets);
        let s_on = hits.iter().find(|h| h.index == 0).map(|h| h.strength01).unwrap_or(0.0);
        let s_off = hits.iter().find(|h| h.index == 1).map(|h| h.strength01).unwrap_or(0.0);
        assert!(s_on > s_off, "on-axis stronger than off-axis ({s_on} vs {s_off})");
        assert!(!hits.iter().any(|h| h.index == 2), "wide target excluded");
        assert!(hits.iter().all(|h| (0.0..=1.0).contains(&h.strength01)));
    }

    #[test]
    fn area_hits_within_radius_only() {
        let center = [0.0, 0.0, 0.0];
        let targets = [tgt([1.0, 0.0, 0.0]), tgt([3.0, 0.0, 0.0]), tgt([9.0, 0.0, 0.0])];
        let hits = area_hits(center, 4.0, &targets);
        let idxs: Vec<usize> = hits.iter().map(|h| h.index).collect();
        assert!(idxs.contains(&0) && idxs.contains(&1), "within radius hit");
        assert!(!idxs.contains(&2), "outside radius excluded");
        // closer ⇒ stronger
        let s0 = hits.iter().find(|h| h.index == 0).map(|h| h.strength01).unwrap_or(0.0);
        let s1 = hits.iter().find(|h| h.index == 1).map(|h| h.strength01).unwrap_or(0.0);
        assert!(s0 > s1);
    }

    #[test]
    fn chain_hops_nearest_then_forward_decaying() {
        let origin = [0.0, 0.0, 0.0];
        let aim = [0.0, 0.0, 1.0];
        // a forward staircase
        let targets = [
            tgt([0.0, 0.0, 3.0]),
            tgt([1.0, 0.0, 6.0]),
            tgt([2.0, 0.0, 9.0]),
            tgt([0.0, 0.0, -3.0]), // behind ⇒ not first
        ];
        let hits = chain_hits(origin, aim, 8.0, 4, &targets);
        assert!(hits.len() >= 2, "chains multiple hops");
        assert_eq!(hits[0].index, 0, "first hop is nearest in front");
        // strength decays along the chain
        for w in hits.windows(2) {
            assert!(w[1].strength01 <= w[0].strength01 + 1e-6, "decays per hop");
        }
        assert!(hits.iter().all(|h| (0.0..=1.0).contains(&h.strength01)));
        // no repeats
        let mut seen = std::collections::BTreeSet::new();
        assert!(hits.iter().all(|h| seen.insert(h.index)), "never repeats a target");
        assert!(chain_hits(origin, [0.0, 0.0, 0.0], 8.0, 4, &targets).is_empty(), "degenerate aim ⇒ empty");
    }

    #[test]
    fn resolve_targets_dispatch_and_degenerate_aim() {
        let origin = [0.0, 0.0, 0.0];
        let aim = [0.0, 0.0, 1.0];
        let targets = [tgt([0.0, 0.0, 4.0]), tgt([2.0, 0.0, 4.0])];
        // directional shape ⇒ cone along aim
        let directional = TechniqueShape { kind: 0, scale: 1.0, range: 1.0, radial: false, signature: false };
        let dir_hits = resolve_targets(origin, aim, &directional, &targets, 1.0);
        assert!(!dir_hits.is_empty(), "directional shape hits along aim");
        // radial shape ⇒ area around origin, works even with zero aim
        let radial = TechniqueShape { kind: 0, scale: 1.0, range: 1.0, radial: true, signature: false };
        let rad_hits = resolve_targets(origin, [0.0, 0.0, 0.0], &radial, &targets, 1.0);
        assert!(!rad_hits.is_empty(), "radial shape is centred (no aim needed)");
        // directional + degenerate aim ⇒ empty
        assert!(resolve_targets(origin, [0.0, 0.0, 0.0], &directional, &targets, 1.0).is_empty());
        // power 0 ⇒ zero reach ⇒ nothing
        assert!(resolve_targets(origin, aim, &directional, &targets, 0.0).is_empty());
        assert!(dir_hits.iter().all(|h| (0.0..=1.0).contains(&h.strength01)));
    }

    #[test]
    fn strength_in_range_and_deterministic() {
        let origin = [0.0, 0.0, 0.0];
        let aim = [1.0, 0.5, 2.0];
        let targets = [tgt([1.0, 0.5, 4.0]), tgt([3.0, 0.0, 6.0]), tgt([-2.0, 1.0, 5.0])];
        let shape = TechniqueShape { kind: 2, scale: 1.2, range: 1.3, radial: false, signature: true };
        let a = resolve_targets(origin, aim, &shape, &targets, 0.8);
        let b = resolve_targets(origin, aim, &shape, &targets, 0.8);
        assert_eq!(a, b, "deterministic");
        for h in &a {
            assert!(h.strength01.is_finite() && (0.0..=1.0).contains(&h.strength01));
        }
    }
}
