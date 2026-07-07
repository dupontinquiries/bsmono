//! `sim::prey_scan` — perception-bounded prey/threat selection.
//!
//! [`crate::sim::predator_prey::decide`] makes the right flee/hunt/ignore call,
//! but it takes a slice of neighbours the caller has already gathered — and the
//! old path gathered them by scanning the **whole herd** for every predator,
//! which is O(n²) at the mob cap (the `perception_and_population_behavior.md`
//! §2 "not yet built" item). This module closes that gap: it uses
//! [`crate::sim::neighbor_grid`] as a **perception index** so a predator only
//! considers creatures in the 3×3×3 cells around it, filters to those actually
//! within its sight radius, and hands *that* set to `decide`. A predator hunts
//! what it can see, so scanning only what it can see is both the perf fix and
//! the correct model — turning the n² tail into ≈O(n).
//!
//! Pure and deterministic: candidates are visited in the grid's fixed traversal
//! order, distance is computed from positions (so it can't disagree with the
//! grid), and the decision is exactly `predator_prey`'s. Build the grid with
//! `cell >= sight` (see [`gather`]) so no in-range creature is missed. Reuses a
//! caller-provided scratch `Vec` so the per-predator query is allocation-free
//! after warmup.

use crate::sim::neighbor_grid::NeighborGrid;
use crate::sim::predator_prey::{decide, Act, Me, Other};

/// The per-candidate data `decide` needs, resolved from a grid id. Distance is
/// *not* included — [`gather`] computes it from `pos` vs the querying creature,
/// so the grid bin and the distance can never disagree. The relative flags
/// (`edible_by_me`, `threat_to_me`) are evaluated by the resolver against the
/// querying creature.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    pub pos: [f32; 3],
    pub kind: u8,
    pub size: f32,
    pub edible_by_me: bool,
    pub threat_to_me: bool,
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Fill `out` with the creatures the querying predator can perceive: grid
/// candidates around `me_pos`, excluding itself (`me_id`), within `sight`
/// blocks. `resolve(id)` returns the candidate's data (with the relative flags
/// computed against the querying creature), or `None` to skip a stale/despawned
/// id. Deterministic; visits only the local cells.
///
/// The grid MUST have been built with `cell >= sight`, so every creature within
/// `sight` lands in the queried cell or one of its 26 neighbours; otherwise a
/// near-but-across-a-cell-boundary creature could be missed.
pub fn gather<F>(grid: &NeighborGrid, me_id: u32, me_pos: [f32; 3], sight: f32, mut resolve: F, out: &mut Vec<Other>)
where
    F: FnMut(u32) -> Option<Candidate>,
{
    out.clear();
    let (cx, cy, cz) = (
        me_pos[0].floor() as i32,
        me_pos[1].floor() as i32,
        me_pos[2].floor() as i32,
    );
    // Optimization (behaviour-preserving): filter on squared distance so the
    // sqrt is paid ONLY for candidates that are actually in range and get
    // stored. `d2 <= sight²` ⟺ `sqrt(d2) <= sight` (both sides ≥ 0), and the
    // stored `dist` is `d2.sqrt()`, identical to the old `dist(me_pos, c.pos)`.
    let sight2 = sight * sight;
    grid.for_candidates(cx, cy, cz, |id| {
        if id == me_id {
            return;
        }
        if let Some(c) = resolve(id) {
            let (dx, dy, dz) = (me_pos[0] - c.pos[0], me_pos[1] - c.pos[1], me_pos[2] - c.pos[2]);
            let d2 = dx * dx + dy * dy + dz * dz;
            if d2 <= sight2 {
                out.push(Other {
                    idx: id as usize,
                    kind: c.kind,
                    dist: d2.sqrt(),
                    size: c.size,
                    edible_by_me: c.edible_by_me,
                    threat_to_me: c.threat_to_me,
                });
            }
        }
    });
}

/// Perception-bounded selection: [`gather`] what `me` can see, then apply the
/// exact same flee → hunt → ignore rule as the global scan — but over only the
/// nearby, in-sight set. `scratch` is reused across calls (cleared each time).
pub fn select<F>(
    me: &Me,
    grid: &NeighborGrid,
    me_id: u32,
    me_pos: [f32; 3],
    sight: f32,
    resolve: F,
    scratch: &mut Vec<Other>,
) -> Act
where
    F: FnMut(u32) -> Option<Candidate>,
{
    gather(grid, me_id, me_pos, sight, resolve, scratch);
    decide(me, scratch)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A tiny id→candidate table for the resolver closures.
    struct World {
        ids: Vec<u32>,
        pos: Vec<[f32; 3]>,
        kind: Vec<u8>,
        size: Vec<f32>,
        edible: Vec<bool>,
        threat: Vec<bool>,
    }
    impl World {
        fn grid(&self, cell: i32) -> NeighborGrid {
            let mut g = NeighborGrid::new(cell);
            for (i, &id) in self.ids.iter().enumerate() {
                let p = self.pos[i];
                g.insert(id, p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32);
            }
            g
        }
        fn resolve(&self, id: u32) -> Option<Candidate> {
            let i = self.ids.iter().position(|&x| x == id)?;
            Some(Candidate {
                pos: self.pos[i],
                kind: self.kind[i],
                size: self.size[i],
                edible_by_me: self.edible[i],
                threat_to_me: self.threat[i],
            })
        }
    }

    fn hungry() -> Me {
        Me { kind: 1, size: 1.0, hunger: 0.9 }
    }

    #[test]
    fn empty_neighbourhood_is_ignore() {
        let w = World { ids: vec![], pos: vec![], kind: vec![], size: vec![], edible: vec![], threat: vec![] };
        let g = w.grid(8);
        let mut scratch = Vec::new();
        let act = select(&hungry(), &g, 0, [0.0, 0.0, 0.0], 6.0, |id| w.resolve(id), &mut scratch);
        assert_eq!(act, Act::Ignore);
    }

    #[test]
    fn a_threat_in_sight_is_fled() {
        let w = World {
            ids: vec![7],
            pos: vec![[3.0, 0.0, 0.0]],
            kind: vec![9],
            size: vec![2.0],
            edible: vec![false],
            threat: vec![true],
        };
        let g = w.grid(8);
        let mut scratch = Vec::new();
        let act = select(&hungry(), &g, 0, [0.0, 0.0, 0.0], 6.0, |id| w.resolve(id), &mut scratch);
        assert_eq!(act, Act::Flee(7));
    }

    #[test]
    fn a_creature_beyond_sight_is_not_considered() {
        // Big cell (16) so the far creature is still a *grid* candidate, but it's
        // outside the sight radius — the distance filter must drop it.
        let w = World {
            ids: vec![5],
            pos: vec![[12.0, 0.0, 0.0]], // 12 blocks away
            kind: vec![9],
            size: vec![2.0],
            edible: vec![false],
            threat: vec![true],
        };
        let g = w.grid(16);
        assert!(g.candidates(0, 0, 0).contains(&5), "should be a grid candidate at this cell size");
        let mut scratch = Vec::new();
        let act = select(&hungry(), &g, 0, [0.0, 0.0, 0.0], 6.0, |id| w.resolve(id), &mut scratch);
        assert_eq!(act, Act::Ignore, "a threat past sight range must not trigger flee");
    }

    #[test]
    fn a_hungry_predator_hunts_nearby_edible_smaller_prey() {
        let w = World {
            ids: vec![4],
            pos: vec![[2.0, 0.0, 1.0]],
            kind: vec![2],
            size: vec![0.5],
            edible: vec![true],
            threat: vec![false],
        };
        let g = w.grid(8);
        let mut scratch = Vec::new();
        let act = select(&hungry(), &g, 0, [0.0, 0.0, 0.0], 6.0, |id| w.resolve(id), &mut scratch);
        assert_eq!(act, Act::Hunt(4));
    }

    #[test]
    fn the_predator_never_selects_itself() {
        // Self is inserted in the grid; gather must skip me_id even though the
        // resolver would return data for it.
        let w = World {
            ids: vec![1],
            pos: vec![[0.0, 0.0, 0.0]],
            kind: vec![1],
            size: vec![1.0],
            edible: vec![true],
            threat: vec![true],
        };
        let g = w.grid(8);
        let mut scratch = Vec::new();
        gather(&g, 1, [0.0, 0.0, 0.0], 6.0, |id| w.resolve(id), &mut scratch);
        assert!(scratch.is_empty(), "self must be excluded");
    }

    #[test]
    fn bounded_gather_equals_a_manual_in_range_decide() {
        // Perception-bounding must not change the decision for the in-range set:
        // gather+decide == decide over the same neighbours built by hand.
        let w = World {
            ids: vec![10, 11, 12],
            pos: vec![[2.0, 0.0, 0.0], [4.0, 0.0, 0.0], [1.0, 0.0, 1.0]],
            kind: vec![2, 9, 2],
            size: vec![0.5, 2.0, 0.4],
            edible: vec![true, false, true],
            threat: vec![false, true, false],
        };
        let g = w.grid(8);
        let me = hungry();
        let mut scratch = Vec::new();
        let bounded = select(&me, &g, 0, [0.0, 0.0, 0.0], 6.0, |id| w.resolve(id), &mut scratch);

        // Build the same neighbour set manually and call decide directly.
        let mut manual: Vec<Other> = Vec::new();
        for (i, &id) in w.ids.iter().enumerate() {
            let d = dist([0.0, 0.0, 0.0], w.pos[i]);
            manual.push(Other { idx: id as usize, kind: w.kind[i], dist: d, size: w.size[i], edible_by_me: w.edible[i], threat_to_me: w.threat[i] });
        }
        let manual_act = crate::sim::predator_prey::decide(&me, &manual);
        assert_eq!(bounded, manual_act, "bounding by perception changed the decision");
    }

    #[test]
    fn gather_is_deterministic() {
        let w = World {
            ids: vec![20, 21, 22],
            pos: vec![[2.0, 0.0, 0.0], [1.0, 0.0, 2.0], [3.0, 0.0, 1.0]],
            kind: vec![2, 2, 2],
            size: vec![0.5, 0.5, 0.5],
            edible: vec![true, true, true],
            threat: vec![false, false, false],
        };
        let g = w.grid(8);
        let mut a = Vec::new();
        let mut b = Vec::new();
        gather(&g, 0, [0.0, 0.0, 0.0], 6.0, |id| w.resolve(id), &mut a);
        gather(&g, 0, [0.0, 0.0, 0.0], 6.0, |id| w.resolve(id), &mut b);
        assert_eq!(a, b, "same input ⇒ same order and contents");
    }
}
