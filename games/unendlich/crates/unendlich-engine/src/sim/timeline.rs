//! `timeline` — branching timelines for **timebending**, layered on the
//! rollback model.
//!
//! The linear rollback engine ([`super::rollback`]) keeps one trusted
//! timeline and re-simulates it forward when a *present* input corrects a
//! *recent* predicted tick. Timebending is the same idea pushed to its
//! extreme: a timebender reaches *far* into the past, **edits** the world
//! there (e.g. raises a structure 1000 years ago), and the world re-runs
//! forward from that point. But unlike netcode rollback — where the
//! corrected resim *replaces* the old one because it's strictly more
//! correct — a deliberate past edit is a *divergence*: the pre-edit future
//! still "happened", so we keep it. The edit therefore **forks a new
//! timeline branch** at the edit tick; the parent timeline downstream of the
//! fork becomes **stale** (it's the world that no longer leads anywhere), and
//! the branch becomes the live continuation.
//!
//! ```text
//!   parent:  o──o──o──o──o──o──o     (head)        ← edit at tick 3
//!                  │
//!                  └──X──o──o──o──o   (branch head, LIVE)
//!                  ^edit
//!   parent ticks >= 3 are now STALE (kept, but no longer the live world).
//! ```
//!
//! **Looping** falls out of the same structure: a branch can itself be
//! timebent (fork a fork), and a fork can target a tick on *any* existing
//! branch — including an ancestor — so histories form a DAG of branch
//! points, not a single line. Staleness propagates down-tree: forking a
//! branch staleness-marks that branch below the fork, and any branches that
//! were rooted on the now-stale segment are transitively stale too.
//!
//! **Decay / erosion.** A structure placed in the deep past doesn't arrive
//! in the present pristine — it weathers for every year between the edit and
//! the branch head. We model the structure as a small heightfield and run
//! [`crate::world::thermal_erosion::erode_step`] once per elapsed *era* when
//! projecting it forward, so the thing the timebender built 1000 years ago
//! shows up eroded (and the longer the branch runs, the more worn it is).
//!
//! Pure + deterministic: no clocks, no RNG, no I/O. A given tree of edits
//! always yields the same branches, staleness, and eroded structures — so it
//! inherits the simulation's reproducibility and is fully unit-testable
//! headless (see the tests at the bottom).

use crate::world::thermal_erosion::erode_step;

/// A tick on the world clock. One "year" in the lore is just a fixed number
/// of ticks; the module is unit-agnostic — callers pass whatever cadence
/// they use (the test treats 1 era = 1 year = some ticks).
pub type Tick = u64;

/// Stable id for a timeline branch within a [`Timeline`] tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BranchId(pub u32);

/// The placed-structure edit a timebender makes in the past: a small
/// heightfield (`w * h` row-major samples) dropped at `origin` in the world.
/// It erodes as the branch ages.
#[derive(Clone, Debug, PartialEq)]
pub struct Structure {
    /// World-space anchor (block coords) of the heightfield's corner.
    pub origin: (i64, i64),
    /// Heightfield width (samples). `cells.len()` must be a multiple of `w`.
    pub w: usize,
    /// Row-major height samples. Mutated in place as it erodes forward.
    pub cells: Vec<f32>,
    /// Erosion talus (slope threshold) and rate, per era. Tunable per build:
    /// soft mud erodes fast, dressed stone slow.
    pub talus: f32,
    pub rate: f32,
}

impl Structure {
    /// A flat-topped block `w×h` at `height`, anchored at `origin`. The
    /// canonical "raise a wall/tower" edit. (Note a perfectly flat top has no
    /// relief, so it barely erodes — use [`Structure::tower`] for something
    /// that visibly weathers, or build relief into `cells` directly.)
    pub fn raised(origin: (i64, i64), w: usize, h: usize, height: f32, talus: f32, rate: f32) -> Structure {
        Structure { origin, w, cells: vec![height; w * h], talus, rate }
    }

    /// A stepped pyramid/tower `w×h`: a central peak of `height` falling off
    /// toward the edges, so it has real relief and visibly erodes (the peak
    /// slumps, the silhouette rounds). The everyday timebender build.
    pub fn tower(origin: (i64, i64), w: usize, h: usize, height: f32, talus: f32, rate: f32) -> Structure {
        let mut cells = vec![0.0f32; w * h];
        let (cx, cy) = ((w as f32 - 1.0) * 0.5, (h as f32 - 1.0) * 0.5);
        let max_r = cx.max(cy).max(1.0);
        for y in 0..h {
            for x in 0..w {
                // height tapers linearly from the centre to the rim
                let d = (((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / max_r).min(1.0);
                cells[y * w + x] = height * (1.0 - d);
            }
        }
        Structure { origin, w, cells, talus, rate }
    }

    /// Run `eras` erosion steps over the heightfield (in place). One step per
    /// era of branch age — the deep-past structure weathers forward.
    pub fn erode_eras(&mut self, eras: u64) {
        for _ in 0..eras {
            erode_step(&mut self.cells, self.w, self.talus, self.rate);
        }
    }

    /// Total "mass" (sum of heights). The thermal-erosion model *conserves*
    /// mass (it slumps material to lower neighbours, never sheds it), so this
    /// is roughly constant across erosion — weathering shows up as lost
    /// *relief*, not lost volume. See [`relief`](Structure::relief).
    pub fn mass(&self) -> f32 {
        self.cells.iter().copied().sum()
    }

    /// Peak height — the tallest surviving point. Falls as spires erode and
    /// material slumps outward.
    pub fn peak(&self) -> f32 {
        self.cells.iter().copied().fold(f32::MIN, f32::max)
    }

    /// **Relief**: peak minus mean height — how much the structure still
    /// stands proud of its own average. Erosion smooths the field (peaks
    /// drop, hollows fill), so relief is the honest "how worn" metric and is
    /// non-increasing across [`erode_eras`](Structure::erode_eras). A fresh
    /// tower has high relief; an ancient ruin trends toward flat (relief → 0).
    pub fn relief(&self) -> f32 {
        if self.cells.is_empty() {
            return 0.0;
        }
        let mean = self.mass() / self.cells.len() as f32;
        self.peak() - mean
    }
}

/// One timeline branch: where it forked from, when, the edit that created it,
/// and how far it has run.
#[derive(Clone, Debug)]
pub struct Branch {
    pub id: BranchId,
    /// Parent branch + the tick on it where this branch forked. `None` for
    /// the root (the original, un-bent timeline).
    pub parent: Option<(BranchId, Tick)>,
    /// The timebender's edit that spawned this branch (`None` for the root).
    pub edit: Option<Structure>,
    /// First tick that belongs to this branch (== the fork tick).
    pub start: Tick,
    /// How far this branch has been advanced (its head tick).
    pub head: Tick,
    /// A branch is stale once a *later* timebend forked away from its own
    /// past: the live world moved to the child, this segment is kept history.
    pub stale: bool,
}

impl Branch {
    /// Age of the branch in eras/ticks from fork to head.
    pub fn age(&self) -> u64 {
        self.head.saturating_sub(self.start)
    }
}

/// A tree (really a DAG of fork points) of timeline branches produced by
/// timebending. Branch 0 is the root timeline.
#[derive(Clone, Debug)]
pub struct Timeline {
    branches: Vec<Branch>,
    next_id: u32,
    /// The branch the player currently lives on (the live continuation).
    live: BranchId,
}

impl Timeline {
    /// Start a fresh history: a single root branch from `start` (usually 0),
    /// advanced to `head`.
    pub fn new(start: Tick, head: Tick) -> Timeline {
        let root = Branch {
            id: BranchId(0),
            parent: None,
            edit: None,
            start,
            head: head.max(start),
            stale: false,
        };
        Timeline { branches: vec![root], next_id: 1, live: BranchId(0) }
    }

    /// The branch the world is currently living on.
    pub fn live(&self) -> BranchId {
        self.live
    }

    /// Number of branches (including stale ones — they're kept history).
    pub fn branch_count(&self) -> usize {
        self.branches.len()
    }

    /// Borrow a branch by id.
    pub fn branch(&self, id: BranchId) -> Option<&Branch> {
        self.branches.iter().find(|b| b.id == id)
    }

    fn branch_mut(&mut self, id: BranchId) -> Option<&mut Branch> {
        self.branches.iter_mut().find(|b| b.id == id)
    }

    /// All branches, live and stale (in creation order).
    pub fn branches(&self) -> &[Branch] {
        &self.branches
    }

    /// **Timebend**: go back to `at_tick` on branch `from` and place `edit`
    /// there, forking a new live branch. The new branch starts at `at_tick`;
    /// `from` (and anything rooted on its now-superseded future) goes stale.
    /// Returns the new branch id, or `None` if `at_tick` isn't within `from`.
    ///
    /// This is the core timebending op. "Build a structure 1000 years ago"
    /// = `timebend(live, head - 1000_years, Structure::raised(...))`.
    pub fn timebend(&mut self, from: BranchId, at_tick: Tick, edit: Structure) -> Option<BranchId> {
        let parent = self.branch(from)?;
        // the edit must land within the parent's lived span
        if at_tick < parent.start || at_tick > parent.head {
            return None;
        }
        let id = BranchId(self.next_id);
        self.next_id += 1;
        let new = Branch {
            id,
            parent: Some((from, at_tick)),
            edit: Some(edit),
            start: at_tick,
            head: at_tick, // not yet advanced
            stale: false,
        };
        self.branches.push(new);
        // the parent's future (and transitively anything depending on it past
        // the fork) is now a dead branch — the live world is the child.
        self.mark_stale_below(from, at_tick);
        self.live = id;
        Some(id)
    }

    /// Advance a branch's head by `ticks` (simulate it forward). Only the live
    /// branch is normally advanced, but stale branches can be inspected at
    /// their frozen head.
    pub fn advance(&mut self, id: BranchId, ticks: u64) {
        if let Some(b) = self.branch_mut(id) {
            b.head = b.head.saturating_add(ticks);
        }
    }

    /// Mark `branch` stale for the region at/after `from_tick`, and cascade to
    /// any branches that forked off that now-stale segment. The fork point
    /// itself stays valid (the child lives there); it's the *parent's own
    /// continuation past the fork* that dies, taking its dependents with it.
    fn mark_stale_below(&mut self, branch: BranchId, from_tick: Tick) {
        // The parent timeline only becomes stale if it actually *had* a future
        // past the fork (head strictly beyond the edit). A fork at the head is
        // a clean continuation — nothing to invalidate.
        let had_future = self.branch(branch).is_some_and(|b| b.head > from_tick);
        if had_future {
            if let Some(b) = self.branch_mut(branch) {
                b.stale = true;
            }
        }
        // Cascade: any *other* branch that forked from `branch` at a tick in
        // the now-dead segment (> from_tick) is rooted on history that no
        // longer leads to the live world, so it's stale too — recursively.
        let dependents: Vec<(BranchId, Tick)> = self
            .branches
            .iter()
            .filter(|b| matches!(b.parent, Some((p, t)) if p == branch && t > from_tick))
            .map(|b| (b.id, b.parent.unwrap().1))
            .collect();
        for (dep, t) in dependents {
            // mark the dependent itself + recurse into its subtree
            if let Some(b) = self.branch_mut(dep) {
                b.stale = true;
            }
            self.mark_stale_below(dep, t);
        }
    }

    /// Project the live branch's structure into the present: take the edit's
    /// heightfield and erode it once per era of the branch's age, returning
    /// the weathered structure as it appears *now* at the branch head. `None`
    /// if the live branch is the root (no edit). `ticks_per_era` converts the
    /// branch age into erosion steps (e.g. ticks per in-lore year).
    pub fn project_live_structure(&self, ticks_per_era: u64) -> Option<Structure> {
        self.project_structure(self.live, ticks_per_era)
    }

    /// As [`project_live_structure`] but for any branch.
    pub fn project_structure(&self, id: BranchId, ticks_per_era: u64) -> Option<Structure> {
        let b = self.branch(id)?;
        let mut s = b.edit.clone()?;
        let eras = if ticks_per_era == 0 { 0 } else { b.age() / ticks_per_era };
        s.erode_eras(eras);
        Some(s)
    }

    /// The chain of fork points from the root down to `id` (root-first), each
    /// as `(branch, fork_tick)`. Useful for visualising the bent history.
    pub fn lineage(&self, id: BranchId) -> Vec<(BranchId, Tick)> {
        let mut out = Vec::new();
        let mut cur = self.branch(id);
        while let Some(b) = cur {
            match b.parent {
                Some((p, t)) => {
                    out.push((b.id, t));
                    cur = self.branch(p);
                }
                None => {
                    out.push((b.id, b.start));
                    break;
                }
            }
        }
        out.reverse();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The headline scenario: live on the root at year 1000, timebend back to
    /// year 0, raise a structure, run the new branch forward 1000 years — the
    /// structure arrives eroded, the old timeline is stale, and the live world
    /// is the new branch.
    #[test]
    fn timebend_builds_branch_that_erodes_and_staleifies_the_old() {
        const ERA: u64 = 1; // 1 tick == 1 year here, for clarity
        // root timeline lived from year 0 to year 1000
        let mut tl = Timeline::new(0, 1000);
        assert_eq!(tl.branch_count(), 1);
        assert_eq!(tl.live(), BranchId(0));
        assert!(!tl.branch(BranchId(0)).unwrap().stale);

        // go back to year 0 and raise a 5x5 stone tower (peaked, height 10)
        let tower = Structure::tower((0, 0), 5, 5, 10.0, /*talus*/ 0.6, /*rate*/ 0.5);
        let pristine_relief = tower.relief();
        let pristine_peak = tower.peak();
        let pristine_mass = tower.mass();
        let branch = tl.timebend(BranchId(0), 0, tower).expect("edit lands in root span");

        // a new branch exists and is now the live one
        assert_eq!(tl.branch_count(), 2);
        assert_eq!(tl.live(), branch);
        // the OLD timeline (root) had a future past year 0 -> it's stale now
        assert!(tl.branch(BranchId(0)).unwrap().stale, "old timeline went stale");
        assert!(!tl.branch(branch).unwrap().stale, "new branch is live, not stale");

        // run the branch forward 1000 years
        tl.advance(branch, 1000);
        assert_eq!(tl.branch(branch).unwrap().age(), 1000);

        // project the tower into the present: it has eroded for 1000 eras.
        // Thermal erosion CONSERVES mass (slumps, doesn't shed), so the
        // weathering shows as lost RELIEF + a lower peak, not lost volume.
        let now = tl.project_live_structure(ERA).expect("live branch has a structure");
        assert!(
            now.relief() < pristine_relief,
            "1000 years of erosion smoothed the structure ({} -> {} relief)",
            pristine_relief,
            now.relief()
        );
        assert!(now.peak() < pristine_peak, "the tower's peak weathered down");
        // mass is ~conserved (material slumped outward, not removed)
        assert!(
            (now.mass() - pristine_mass).abs() < 1e-2,
            "erosion conserves mass ({} -> {})",
            pristine_mass,
            now.mass()
        );
        // the structure still exists (didn't vanish) — it's worn, not gone
        assert!(now.mass() > 0.0, "the ruin persists");
    }

    /// Looping: a fork of a fork, and staleness cascades to dependents rooted
    /// on a segment that a later timebend kills.
    #[test]
    fn forking_a_fork_cascades_staleness() {
        let mut tl = Timeline::new(0, 100);
        // branch A forks root at 50, runs to 100
        let a = tl.timebend(BranchId(0), 50, Structure::raised((0, 0), 3, 3, 5.0, 0.5, 0.5)).unwrap();
        tl.advance(a, 50); // A: 50 -> 100
        // branch B forks A at 80, runs to 120
        let b = tl.timebend(a, 80, Structure::raised((0, 0), 3, 3, 5.0, 0.5, 0.5)).unwrap();
        tl.advance(b, 40); // B: 80 -> 120
        assert_eq!(tl.live(), b);
        assert!(tl.branch(a).unwrap().stale, "A's future past 80 died when B forked");

        // now timebend the ROOT again, back at 20 — this kills root's future
        // past 20, which is where A was rooted (50 > 20), so A (and thus B,
        // rooted on A) cascade to stale.
        let c = tl.timebend(BranchId(0), 20, Structure::raised((0, 0), 3, 3, 5.0, 0.5, 0.5)).unwrap();
        assert_eq!(tl.live(), c);
        assert!(tl.branch(BranchId(0)).unwrap().stale);
        assert!(tl.branch(a).unwrap().stale, "A cascaded stale (rooted on dead root segment)");
        assert!(tl.branch(b).unwrap().stale, "B cascaded stale (rooted on now-stale A)");
        assert!(!tl.branch(c).unwrap().stale, "C is the live branch");
    }

    /// Lineage walks the fork chain root-first.
    #[test]
    fn lineage_traces_fork_points() {
        let mut tl = Timeline::new(0, 100);
        let a = tl.timebend(BranchId(0), 40, Structure::raised((0, 0), 2, 2, 3.0, 0.5, 0.5)).unwrap();
        tl.advance(a, 30);
        let b = tl.timebend(a, 60, Structure::raised((0, 0), 2, 2, 3.0, 0.5, 0.5)).unwrap();
        let lin = tl.lineage(b);
        // root (0 @ start 0) -> A forked at 40 -> B forked at 60
        assert_eq!(lin.first().unwrap().0, BranchId(0));
        assert_eq!(lin.last().unwrap(), &(b, 60));
        assert_eq!(lin.len(), 3);
    }

    /// An edit outside the parent's lived span is rejected (can't bend to a
    /// time that branch never saw).
    #[test]
    fn timebend_outside_span_is_rejected() {
        let mut tl = Timeline::new(0, 100);
        assert!(tl.timebend(BranchId(0), 200, Structure::raised((0, 0), 2, 2, 1.0, 0.5, 0.5)).is_none());
        assert_eq!(tl.branch_count(), 1, "no branch created for an out-of-span edit");
    }

    /// A fork exactly at the parent's head is a clean continuation — the
    /// parent had no future to invalidate, so it does NOT go stale.
    #[test]
    fn fork_at_head_does_not_staleify_parent() {
        let mut tl = Timeline::new(0, 100);
        let a = tl.timebend(BranchId(0), 100, Structure::raised((0, 0), 2, 2, 1.0, 0.5, 0.5)).unwrap();
        assert!(!tl.branch(BranchId(0)).unwrap().stale, "fork at head = clean continuation");
        assert_eq!(tl.live(), a);
    }
}
