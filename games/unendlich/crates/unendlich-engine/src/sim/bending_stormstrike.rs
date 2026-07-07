//! Stormstrike — **lightning routing** discipline (r58-06, pure sim).
//!
//! Per `docs/specs/resonance_bending_upgrade.docx`: a Stormstrike bender
//! "doesn't throw more lightning, he decides WHERE it goes — and being wrong is
//! costly." He establishes a circuit in which a target becomes the most
//! conductive node and lets physics do the rest. This core owns that decision:
//! given an arc origin, the committed aim, and the conductive nodes in range,
//! it routes the bolt as a forward conductance-seeking chain and reports the
//! **misprediction penalty** when the route is wrong (the bolt grounds on the
//! bender / finds nothing in the aim cone).
//!
//! Composes with `sim::gesture_resolve` (the commit) and `sim::element_reaction`
//! (Storm element). Pure: geometry in → a [`StrikePlan`] out. No RNG, I/O, GPU,
//! wall-clock; no `unwrap`/`expect`/`panic!`; degenerate inputs ground safely
//! with no NaN. Deterministic.

/// A conductive node the bolt can route through. `is_self` marks the bender
/// (grounding on it is the failure case).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Conductor {
    pub pos: [f32; 3],
    pub conductance01: f32,
    pub is_self: bool,
}

/// The routed strike. `path` is the ordered list of conductor indices the bolt
/// chains through (no repeats). `grounded_on_self` is set when the bolt found no
/// valid forward target (aimed into empty space, or only the bender was in
/// range). `penalty01 ∈ [0,1]` is the misprediction cost.
#[derive(Clone, Debug, PartialEq)]
pub struct StrikePlan {
    pub path: Vec<usize>,
    pub grounded_on_self: bool,
    pub penalty01: f32,
}

// --- small vec helpers (unconditional; pure) ---

#[inline]
fn fin3(v: [f32; 3]) -> [f32; 3] {
    [
        if v[0].is_finite() { v[0] } else { 0.0 },
        if v[1].is_finite() { v[1] } else { 0.0 },
        if v[2].is_finite() { v[2] } else { 0.0 },
    ]
}
#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn len(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}
/// Unit vector, or `None` for a (near-)zero / non-finite vector.
#[inline]
fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let a = fin3(a);
    let l = len(a);
    if l > 1e-9 && l.is_finite() {
        Some([a[0] / l, a[1] / l, a[2] / l])
    } else {
        None
    }
}

/// Route the bolt from `origin` along `aim`, chaining each hop to the
/// best-conducting conductor that is within `max_hop` of the current node and
/// inside the aim cone (`dot(dir, toCandidate) >= aim_cos`), up to `max_links`
/// hops, never repeating an index. The cone direction advances along each
/// chosen segment. If the first hop finds no non-self target (empty space or
/// only the bender in range), the bolt grounds on the bender.
pub fn route(
    origin: [f32; 3],
    aim: [f32; 3],
    conductors: &[Conductor],
    max_links: u8,
    max_hop: f32,
    aim_cos: f32,
) -> StrikePlan {
    let origin = fin3(origin);
    let hop = if max_hop.is_finite() { max_hop.max(0.0) } else { 0.0 };
    let cone = aim_cos.clamp(-1.0, 1.0);

    // Degenerate aim → can't route → ground safely (no NaN).
    let mut dir = match normalize(aim) {
        Some(d) => d,
        None => {
            return StrikePlan { path: Vec::new(), grounded_on_self: true, penalty01: 1.0 };
        }
    };

    let mut path: Vec<usize> = Vec::new();
    let mut cur = origin;

    for _ in 0..max_links {
        let mut best: Option<(usize, f32, f32, [f32; 3])> = None; // (idx, conductance, dist, unit_to)
        for (i, c) in conductors.iter().enumerate() {
            if c.is_self || path.contains(&i) {
                continue;
            }
            let to = sub(fin3(c.pos), cur);
            let d = len(to);
            if d <= 1e-9 || d > hop {
                continue; // coincident or out of reach
            }
            let unit = match normalize(to) {
                Some(u) => u,
                None => continue,
            };
            if dot(dir, unit) < cone {
                continue; // outside the aim cone
            }
            let cond = fin3([c.conductance01, 0.0, 0.0])[0].clamp(0.0, 1.0);
            // Prefer highest conductance, then nearer, then lower index.
            let better = match best {
                None => true,
                Some((bi, bc, bd, _)) => {
                    cond > bc || (cond == bc && d < bd) || (cond == bc && d == bd && i < bi)
                }
            };
            if better {
                best = Some((i, cond, d, unit));
            }
        }

        match best {
            Some((idx, _cond, _d, unit)) => {
                path.push(idx);
                cur = fin3(conductors[idx].pos);
                dir = unit; // chain advances along the chosen segment
            }
            None => break, // no forward target this hop
        }
    }

    let grounded_on_self = path.is_empty();
    let penalty01 = if grounded_on_self {
        1.0
    } else {
        // A perfectly conductive first node is a clean route (≈0 risk); a
        // marginal one leaves residual misprediction risk.
        let first_cond = conductors
            .get(path[0])
            .map(|c| c.conductance01.clamp(0.0, 1.0))
            .unwrap_or(0.0);
        (1.0 - first_cond).clamp(0.0, 1.0)
    };

    StrikePlan { path, grounded_on_self, penalty01 }
}

/// How wrong the committed aim was relative to where the bolt actually needed
/// to go: `0.0` dead-on, `0.5` perpendicular, `1.0` opposite. Both arguments
/// are treated as direction vectors. A degenerate (zero/non-finite) input
/// returns the maximum cost `1.0` (you committed to nothing), never NaN.
/// Clamped to `[0,1]` and monotonic in angular error.
pub fn redirect_cost(committed_aim: [f32; 3], actual_target: [f32; 3]) -> f32 {
    match (normalize(committed_aim), normalize(actual_target)) {
        (Some(a), Some(b)) => (((1.0 - dot(a, b)) * 0.5) as f32).clamp(0.0, 1.0),
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(pos: [f32; 3], cond: f32) -> Conductor {
        Conductor { pos, conductance01: cond, is_self: false }
    }
    fn me(pos: [f32; 3]) -> Conductor {
        Conductor { pos, conductance01: 1.0, is_self: true }
    }

    #[test]
    fn chains_to_highest_conductance_in_cone_first() {
        // origin at 0, aiming +x. Two in-cone candidates; the higher-conductance
        // one (idx 1) must be chosen first.
        let cs = [
            c([5.0, 0.0, 0.0], 0.3),
            c([6.0, 0.5, 0.0], 0.9),
            c([0.0, 9.0, 0.0], 1.0), // off to +y, outside a tight +x cone
        ];
        let plan = route([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], &cs, 4, 100.0, 0.7);
        assert!(!plan.grounded_on_self);
        assert_eq!(plan.path.first(), Some(&1), "highest-conductance in-cone first");
        assert!(!plan.path.contains(&2), "out-of-cone node excluded");
        // no repeats
        let mut seen = std::collections::BTreeSet::new();
        assert!(plan.path.iter().all(|i| seen.insert(*i)));
    }

    #[test]
    fn respects_max_links_and_max_hop() {
        // a line of nodes along +x at x=1,2,3,4,5
        let cs: Vec<Conductor> = (1..=5).map(|x| c([x as f32, 0.0, 0.0], 0.5)).collect();
        // max_hop 1.5 → each hop can only reach the next node; max_links 2 → 2 hops.
        let plan = route([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], &cs, 2, 1.5, 0.9);
        assert_eq!(plan.path.len(), 2, "max_links honored");
        assert_eq!(plan.path, vec![0, 1], "chained nearest reachable forward");
        // with max_hop too short to reach anything → grounds
        let plan2 = route([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], &cs, 4, 0.5, 0.9);
        assert!(plan2.grounded_on_self && plan2.path.is_empty());
    }

    #[test]
    fn empty_space_grounds_on_self_with_high_penalty() {
        let plan = route([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], &[], 4, 100.0, 0.7);
        assert!(plan.grounded_on_self);
        assert!(plan.path.is_empty());
        assert!(plan.penalty01 >= 0.99, "grounding is costly");
    }

    #[test]
    fn only_self_in_range_grounds_on_self() {
        let cs = [me([2.0, 0.0, 0.0])];
        let plan = route([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], &cs, 4, 100.0, 0.7);
        assert!(plan.grounded_on_self, "only the bender in range → grounds");
        assert!(plan.penalty01 >= 0.99);
    }

    #[test]
    fn good_route_has_low_penalty() {
        let cs = [c([3.0, 0.0, 0.0], 1.0)];
        let plan = route([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], &cs, 4, 100.0, 0.7);
        assert!(!plan.grounded_on_self);
        assert!(plan.penalty01 <= 0.01, "a perfectly conductive target is a clean route");
    }

    #[test]
    fn degenerate_aim_grounds_without_nan() {
        let cs = [c([3.0, 0.0, 0.0], 1.0)];
        let plan = route([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], &cs, 4, 100.0, 0.7);
        assert!(plan.grounded_on_self);
        assert!(plan.penalty01.is_finite());
    }

    #[test]
    fn redirect_cost_endpoints_and_monotonicity() {
        let fwd = [1.0, 0.0, 0.0];
        assert!((redirect_cost(fwd, fwd) - 0.0).abs() < 1e-6, "dead-on → 0");
        assert!((redirect_cost(fwd, [-1.0, 0.0, 0.0]) - 1.0).abs() < 1e-6, "opposite → 1");
        assert!((redirect_cost(fwd, [0.0, 1.0, 0.0]) - 0.5).abs() < 1e-6, "perpendicular → 0.5");
        // monotonic rising with angle from 0..pi
        let mut prev = -1.0;
        for i in 0..=180 {
            let th = (i as f32).to_radians();
            let cost = redirect_cost(fwd, [th.cos(), th.sin(), 0.0]);
            assert!(cost.is_finite() && (0.0..=1.0).contains(&cost));
            assert!(cost + 1e-5 >= prev, "redirect_cost must rise with angular error");
            prev = cost;
        }
        // degenerate → max cost, no NaN
        assert_eq!(redirect_cost([0.0, 0.0, 0.0], fwd), 1.0);
        assert!(redirect_cost(fwd, [f32::NAN, 0.0, 0.0]).is_finite());
    }

    #[test]
    fn deterministic() {
        let cs = [
            c([5.0, 0.0, 0.0], 0.3),
            c([6.0, 0.5, 0.0], 0.9),
            c([12.0, 1.0, 0.0], 0.8),
            me([1.0, 0.0, 0.0]),
        ];
        let run = || route([0.0, 0.0, 0.0], [1.0, 0.05, 0.0], &cs, 4, 100.0, 0.6);
        let first = run();
        for _ in 0..100 {
            assert_eq!(run(), first);
        }
    }
}
