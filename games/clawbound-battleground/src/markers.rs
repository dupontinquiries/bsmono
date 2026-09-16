//! markers — the smell that says whose ground this is.
//!
//! > *No challenging yowls came from the patrol, and the scents gradually died
//! > away, leaving only the smell of the border markers.*
//!
//! That sentence is the whole territorial system, and it turns on a
//! distinction `wildkit`'s scent modules do not make. A creature's trail is
//! laid by moving and fades in minutes — `scent_decay::remaining` with a short
//! half-life, which is exactly right for "who walked here". A border marker is
//! **placed**, owned by a clan rather than an individual, refreshed on patrol,
//! and measured in days. When the patrol's own scent has gone, the markers are
//! still there. That is why they are a separate type and not a long-lived
//! trail.
//!
//! ## What a marker answers
//!
//! Not "who walked here" but **"whose ground am I on"**. A cat crossing a
//! border does not see a line; it smells one clan's markers weaken and
//! another's strengthen. [`Markers::owner_at`] is that question, and the place
//! where the two overlap is the border — a place with width, not a line.
//!
//! ## Refreshing, not stacking
//!
//! Patrolling a border re-marks it. A marker laid twice is not twice as
//! strong; it is *new again*. Stacking would let a clan that patrols
//! obsessively own ground by volume, and would make a long-neglected border
//! indistinguishable from a fresh one after a single visit.
//!
//! ## Decay is borrowed, not rewritten
//!
//! `scent_decay::remaining` is the same exponential used for trails. Only the
//! half-life differs, and it is the parameter, so a marker and a pawprint
//! cannot drift into two different ideas of what fading means.

use wildkit::sim::scent_decay;

/// Which clan a marker belongs to. An id rather than an enum: clans are
/// content, and a rig that needed a code change to add one would make the
/// map editor a compiler.
pub type ClanId = u16;

/// How long a marker takes to lose half its strength, in seconds.
///
/// Three days. The books' rhythm is that borders are re-marked on a patrol
/// that happens once or twice a day, so the half-life has to be long enough
/// that a missed patrol is a *worry* rather than an immediate loss of
/// territory, and short enough that a clan which stops patrolling loses the
/// ground within a few days.
pub const MARKER_HALF_LIFE_S: f32 = 3.0 * 24.0 * 3600.0;

/// Below this, a marker no longer claims anything.
///
/// Not zero: an exponential never reaches zero, so without a floor every
/// marker ever placed would claim the ground forever at a vanishing strength,
/// and `owner_at` would be decided by rounding.
pub const CLAIM_FLOOR: f32 = 0.08;

/// How far a marker can be smelled at full strength, in world units.
pub const MARKER_RANGE: f32 = 12.0;

/// One placed scent mark.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marker {
    pub x: f32,
    pub z: f32,
    pub clan: ClanId,
    /// When it was last laid or refreshed, in seconds.
    pub laid_at_s: f32,
}

impl Marker {
    /// Strength here and now, 0..1, ignoring distance.
    pub fn freshness(&self, now_s: f32) -> f32 {
        let age = (now_s - self.laid_at_s).max(0.0);
        scent_decay::remaining(1.0, age, MARKER_HALF_LIFE_S)
    }

    /// Strength as smelled from `(x, z)`.
    ///
    /// Linear falloff over `MARKER_RANGE` on top of the time decay. Linear
    /// rather than inverse-square because this is a gameplay signal a player
    /// reads by walking, and an inverse-square marker is imperceptible until
    /// you are standing on it.
    pub fn strength_at(&self, x: f32, z: f32, now_s: f32) -> f32 {
        let d = ((x - self.x).powi(2) + (z - self.z).powi(2)).sqrt();
        if d >= MARKER_RANGE {
            return 0.0;
        }
        self.freshness(now_s) * (1.0 - d / MARKER_RANGE)
    }
}

/// Every marker on the map.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Markers {
    marks: Vec<Marker>,
}

impl Markers {
    pub fn new() -> Markers {
        Markers::default()
    }

    pub fn len(&self) -> usize {
        self.marks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }
    pub fn all(&self) -> &[Marker] {
        &self.marks
    }

    /// Lay a mark, or refresh the one already there.
    ///
    /// "Already there" means the same clan within [`Self::MERGE_DIST`].
    /// Refreshing rather than appending is the rule the module header names:
    /// a marker laid twice is new again, not twice as strong. Without this a
    /// patrol walking its own border would pile up marks and own ground by
    /// sheer volume of visits.
    pub fn mark(&mut self, x: f32, z: f32, clan: ClanId, now_s: f32) {
        if let Some(m) = self.marks.iter_mut().find(|m| {
            m.clan == clan && (m.x - x).powi(2) + (m.z - z).powi(2) <= Self::MERGE_DIST.powi(2)
        }) {
            m.laid_at_s = now_s;
            return;
        }
        self.marks.push(Marker { x, z, clan, laid_at_s: now_s });
    }

    /// Two marks of the same clan closer than this are the same mark.
    pub const MERGE_DIST: f32 = 4.0;

    /// Total strength of each clan's claim at a point.
    ///
    /// Summed across that clan's marks, because a border is marked repeatedly
    /// along its length and standing between two of a clan's marks should feel
    /// more like their ground than standing beside one.
    pub fn claim_at(&self, x: f32, z: f32, clan: ClanId, now_s: f32) -> f32 {
        self.marks
            .iter()
            .filter(|m| m.clan == clan)
            .map(|m| m.strength_at(x, z, now_s))
            .sum::<f32>()
            .min(1.0)
    }

    /// Whose ground this is, and how strongly.
    ///
    /// `None` when nobody's claim clears [`CLAIM_FLOOR`] — unclaimed land is a
    /// real state, and the forest between territories is most of the map.
    ///
    /// A tie is `None` rather than a coin flip: contested ground is contested,
    /// and picking a winner would make a border flicker as a cat walked along
    /// it.
    pub fn owner_at(&self, x: f32, z: f32, now_s: f32) -> Option<(ClanId, f32)> {
        let mut best: Option<(ClanId, f32)> = None;
        let mut tied = false;
        for clan in self.clans() {
            let s = self.claim_at(x, z, clan, now_s);
            if s < CLAIM_FLOOR {
                continue;
            }
            match best {
                Some((_, b)) if (s - b).abs() < 1e-4 => tied = true,
                Some((_, b)) if s > b => {
                    best = Some((clan, s));
                    tied = false;
                }
                Some(_) => {}
                None => best = Some((clan, s)),
            }
        }
        if tied {
            None
        } else {
            best
        }
    }

    /// Every clan with at least one mark, ascending.
    pub fn clans(&self) -> Vec<ClanId> {
        let mut v: Vec<ClanId> = self.marks.iter().map(|m| m.clan).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// A clan's marks, weakest first — the patrol's to-do list.
    ///
    /// This is what makes a border patrol a *task* rather than a walk: the
    /// stalest marks are where the territory is actually slipping, and they
    /// are not necessarily where the last patrol went.
    pub fn stalest(&self, clan: ClanId, now_s: f32, n: usize) -> Vec<Marker> {
        let mut v: Vec<Marker> = self.marks.iter().copied().filter(|m| m.clan == clan).collect();
        v.sort_by(|a, b| {
            a.freshness(now_s)
                .total_cmp(&b.freshness(now_s))
                // Ties on position so a patrol list is stable between frames.
                .then(a.x.total_cmp(&b.x))
                .then(a.z.total_cmp(&b.z))
        });
        v.truncate(n);
        v
    }

    /// Drop marks that no longer claim anything.
    ///
    /// Explicit rather than automatic in `owner_at`: a query should not mutate,
    /// and a caller may want to see a lapsed border in an editor.
    pub fn prune(&mut self, now_s: f32) {
        self.marks.retain(|m| m.freshness(now_s) >= CLAIM_FLOOR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: f32 = 24.0 * 3600.0;
    const A: ClanId = 1;
    const B: ClanId = 2;

    /// THE SENTENCE. A patrol's own trail fades in minutes; its markers are
    /// still there days later. That gap is why this type exists.
    #[test]
    fn markers_outlast_the_patrol_that_laid_them() {
        let m = Marker { x: 0.0, z: 0.0, clan: A, laid_at_s: 0.0 };
        // A creature trail's half-life is seconds-to-minutes; after an hour it
        // is long gone. The marker has barely moved.
        assert!(m.freshness(3600.0) > 0.98, "an hour later: {}", m.freshness(3600.0));
        assert!(m.freshness(3.0 * DAY) > 0.45, "three days is one half-life");
        assert!(m.freshness(14.0 * DAY) < CLAIM_FLOOR, "a fortnight unpatrolled and it is gone");
    }

    /// REFRESH, DO NOT STACK. Otherwise a clan that patrols obsessively owns
    /// ground by volume of visits.
    #[test]
    fn re_marking_refreshes_rather_than_accumulating() {
        let mut m = Markers::new();
        m.mark(0.0, 0.0, A, 0.0);
        for t in 1..10 {
            m.mark(0.5, 0.5, A, t as f32 * DAY);
        }
        assert_eq!(m.len(), 1, "one mark, re-laid: {:?}", m.all());
        assert!(m.all()[0].freshness(9.0 * DAY) > 0.99, "and it is fresh again");
    }

    /// A different clan marking the same spot is a SECOND mark -- that is the
    /// contested border, not a takeover.
    #[test]
    fn a_rival_marking_the_same_spot_adds_a_mark() {
        let mut m = Markers::new();
        m.mark(0.0, 0.0, A, 0.0);
        m.mark(0.0, 0.0, B, 0.0);
        assert_eq!(m.len(), 2);
        assert_eq!(m.clans(), vec![A, B]);
    }

    /// Contested ground is contested. Picking a winner on a tie would make the
    /// border flicker as a cat walked along it.
    #[test]
    fn evenly_matched_ground_has_no_owner() {
        let mut m = Markers::new();
        m.mark(0.0, 0.0, A, 0.0);
        m.mark(0.0, 0.0, B, 0.0);
        assert_eq!(m.owner_at(0.0, 0.0, 0.0), None);
        // Let A's mark be fresher and it wins.
        m.mark(0.0, 0.0, A, 2.0 * DAY);
        assert_eq!(m.owner_at(0.0, 0.0, 2.0 * DAY).map(|(c, _)| c), Some(A));
    }

    /// Crossing a border is a change of owner, with unclaimed forest between.
    #[test]
    fn walking_across_a_border_changes_whose_ground_it_is() {
        let mut m = Markers::new();
        m.mark(0.0, 0.0, A, 0.0);
        m.mark(60.0, 0.0, B, 0.0);
        assert_eq!(m.owner_at(0.0, 0.0, 0.0).map(|(c, _)| c), Some(A));
        assert_eq!(m.owner_at(60.0, 0.0, 0.0).map(|(c, _)| c), Some(B));
        assert_eq!(m.owner_at(30.0, 0.0, 0.0), None, "the forest between belongs to nobody");
    }

    /// A border is a PLACE, not a line: the claim weakens with distance rather
    /// than switching at an edge.
    #[test]
    fn a_claim_weakens_with_distance_rather_than_ending_at_an_edge() {
        let mut m = Markers::new();
        m.mark(0.0, 0.0, A, 0.0);
        let near = m.claim_at(1.0, 0.0, A, 0.0);
        let mid = m.claim_at(6.0, 0.0, A, 0.0);
        let far = m.claim_at(MARKER_RANGE + 1.0, 0.0, A, 0.0);
        assert!(near > mid && mid > 0.0);
        assert_eq!(far, 0.0, "out of range is out of range");
    }

    /// THE PATROL'S TO-DO LIST. The stalest marks are where the territory is
    /// slipping, and they are not where the last patrol went.
    #[test]
    fn the_stalest_marks_are_the_ones_to_re_mark() {
        let mut m = Markers::new();
        m.mark(0.0, 0.0, A, 5.0 * DAY);   // freshest
        m.mark(20.0, 0.0, A, 1.0 * DAY);  // stalest
        m.mark(40.0, 0.0, A, 3.0 * DAY);
        let now = 5.0 * DAY;
        let worst: Vec<f32> = m.stalest(A, now, 2).iter().map(|k| k.x).collect();
        assert_eq!(worst, vec![20.0, 40.0], "weakest first");
        assert_eq!(m.stalest(B, now, 5).len(), 0, "another clan's marks are not our problem");
    }

    /// An exponential never reaches zero, so without a floor every mark ever
    /// laid would claim ground forever at a vanishing strength.
    #[test]
    fn lapsed_marks_stop_claiming_and_can_be_pruned() {
        let mut m = Markers::new();
        m.mark(0.0, 0.0, A, 0.0);
        let long_after = 30.0 * DAY;
        assert_eq!(m.owner_at(0.0, 0.0, long_after), None, "it no longer claims");
        assert_eq!(m.len(), 1, "but the query did not mutate");
        m.prune(long_after);
        assert!(m.is_empty());
    }

    /// Standing between two of a clan's marks should feel more like their
    /// ground than standing beside one.
    #[test]
    fn claims_from_several_marks_add_up() {
        let mut m = Markers::new();
        m.mark(-5.0, 0.0, A, 0.0);
        m.mark(5.0, 0.0, A, 0.0);
        let between = m.claim_at(0.0, 0.0, A, 0.0);
        let single = Marker { x: -5.0, z: 0.0, clan: A, laid_at_s: 0.0 }
            .strength_at(0.0, 0.0, 0.0);
        assert!(between > single, "{between} should beat {single}");
        assert!(between <= 1.0, "and still saturate");
    }

    /// An empty map has no owners and does not panic.
    #[test]
    fn an_unmarked_map_belongs_to_nobody() {
        let m = Markers::new();
        assert_eq!(m.owner_at(0.0, 0.0, 0.0), None);
        assert!(m.clans().is_empty());
        assert!(m.stalest(A, 0.0, 5).is_empty());
    }
}
