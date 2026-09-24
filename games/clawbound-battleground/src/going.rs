//! going — how hard the ground is to cross, as a speed multiplier.
//!
//! The word is the racecourse one: the going is good, the going is heavy.
//!
//! ## Why this exists
//!
//! The jungle had two answers to "can I walk here": a wall (`Blockers`) or
//! nothing at all. Ferns and bushes were pure scenery for movement, bamboo
//! and small rocks were posts you slid round, and the only ground that cost
//! anything was a river. A chase through a thicket and a chase down the lane
//! were the same chase. Hard blockers also make bad terrain: a unit that
//! paths by push-out stalls on them (TODO.md, the ruin wall's concave
//! corner), while ground that is merely SLOW never traps anyone, and turns
//! "which way round" into a real choice rather than a collision.
//!
//! So most of the jungle's small stuff moves here: it does not stop you, it
//! costs you. Trunks, walls, pillars and structures stay in `Blockers` —
//! there is no pushing through a tree.
//!
//! ## Layer
//!
//! Pure geometry, like `blockers`: discs and capsules in, a number out. It
//! lives INSIDE `Blockers` (one field) so the one `Arc` the scene hands the
//! sim carries it, and no caller had to learn a second world handle.
//!
//! ## Overlap takes the worst, not the product
//!
//! The same rule `concealment_at` uses, for the same reason: scatter plants
//! ferns in clumps, and a product would turn a clump density into a near
//! stop. Three bushes are as slow as the slowest of them.

use glam::Vec3;

use crate::blockers::{PropKind, Shape};

/// What kind of rough ground it is. It matters for WHO it slows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ground {
    /// Ferns, bushes, hanging vines. A stalking cat slips through these —
    /// that is most of what stalking is — so they cost it far less.
    Undergrowth,
    /// Bamboo stands and bramble: stems too close to slip between.
    Thicket,
    /// Loose stone and low rocks: a scramble, whoever you are.
    Scree,
    /// Buttress roots: a stride over each one.
    Roots,
}

/// One patch of rough ground.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rough {
    pub shape: Shape,
    /// Speed multiplier at the core, `MIN_GOING..=1`.
    pub mul: f32,
    /// Width of the soft edge OUTSIDE the shape over which it fades to 1.
    /// A hard edge is a speed step a player feels as a hitch.
    pub edge: f32,
    pub ground: Ground,
}

/// Nothing is slower than this. A floor, so no combination of patches and a
/// future tuning mistake can pin a cat in place.
pub const MIN_GOING: f32 = 0.35;

/// How much of an undergrowth patch's cost a stalking cat still pays.
pub const STALK_UNDERGROWTH: f32 = 0.25;

/// Who is crossing: the parts of the mover that change what ground costs.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Carriage {
    /// Low and deliberate. See [`Ground::Undergrowth`].
    pub stalking: bool,
}

impl PropKind {
    /// The rough ground a prop lays down around itself, as `(mul, radius,
    /// ground)`. `radius` is in world units at scale 1.
    ///
    /// Each value is what crossing it costs at a run, and each is a choice:
    /// a fern costs little (you run through ferns), a bush more, bamboo a lot
    /// (you shoulder through the stems -- bamboo was a hard post before this,
    /// and is now only this).
    ///
    /// Rocks and boulder clusters KEEP their hard core: they are the leap
    /// perches the Hollow's chains are built from (`midjungle`, `leap`), and
    /// a perch has to be solid to be stood on. What they gain is a skirt of
    /// scree wider than the core, so the ground round a boulder is a
    /// scramble rather than a clean wall to slide along.
    pub const fn going(self) -> Option<(f32, f32, Ground)> {
        use PropKind::*;
        match self {
            Fern => Some((0.85, 1.3, Ground::Undergrowth)),
            Bush => Some((0.72, 1.6, Ground::Undergrowth)),
            VineCurtain => Some((0.85, 1.1, Ground::Undergrowth)),
            Bamboo => Some((0.55, 1.5, Ground::Thicket)),
            Rock => Some((0.70, 1.7, Ground::Scree)),
            BoulderCluster => Some((0.70, 2.5, Ground::Scree)),
            _ => None,
        }
    }
}

/// A uniform grid of rough patches. Same shape as the blockers grid, kept
/// separate so neither set's queries scan the other's entries.
#[derive(Debug, Clone, Default)]
pub struct Going {
    all: Vec<Rough>,
    cells: Vec<Vec<u32>>,
    cell: f32,
    dim: i32,
    half_extent: f32,
}

impl Going {
    pub fn new(half_extent: f32, cell: f32) -> Going {
        let cell = cell.max(1.0);
        let dim = ((half_extent * 2.0 / cell).ceil() as i32).max(1);
        Going { all: Vec::new(), cells: vec![Vec::new(); (dim * dim) as usize], cell, dim, half_extent }
    }

    pub fn len(&self) -> usize {
        self.all.len()
    }
    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }
    pub fn all(&self) -> &[Rough] {
        &self.all
    }

    fn cell_of(&self, x: f32, z: f32) -> (i32, i32) {
        let hi = (self.dim - 1).max(0);
        let gx = ((x + self.half_extent) / self.cell).floor() as i32;
        let gz = ((z + self.half_extent) / self.cell).floor() as i32;
        (gx.clamp(0, hi), gz.clamp(0, hi))
    }

    /// Add a patch. Refuses nonsense rather than storing it: a `mul` outside
    /// `(0, 1)` or a non-finite shape is a bug at the call site, and a patch
    /// that speeds you up is a different feature.
    pub fn push(&mut self, r: Rough) -> bool {
        if self.cells.is_empty() || !(r.mul.is_finite() && r.mul > 0.0 && r.mul < 1.0) || !(r.edge.is_finite() && r.edge >= 0.0) {
            return false;
        }
        let (cx, cz, half) = bounds(&r.shape);
        if !(cx.is_finite() && cz.is_finite() && half.is_finite()) {
            return false;
        }
        let idx = self.all.len() as u32;
        let reach = half + r.edge;
        let (x0, z0) = self.cell_of(cx - reach, cz - reach);
        let (x1, z1) = self.cell_of(cx + reach, cz + reach);
        for gz in z0..=z1 {
            for gx in x0..=x1 {
                self.cells[(gz * self.dim + gx) as usize].push(idx);
            }
        }
        self.all.push(Rough { mul: r.mul.max(MIN_GOING), ..r });
        true
    }

    /// The patch a prop brings with it, if any.
    pub fn push_prop(&mut self, kind: PropKind, x: f32, z: f32, scale: f32) -> bool {
        match kind.going() {
            Some((mul, r, ground)) => self.push(Rough { shape: Shape::Circle { x, z, r: r * scale.max(0.01) }, mul, edge: 0.6, ground }),
            None => false,
        }
    }

    /// Speed multiplier at `(x, z)` for this carriage, `MIN_GOING..=1`.
    pub fn at(&self, x: f32, z: f32, who: Carriage) -> f32 {
        if self.cells.is_empty() || !(x.is_finite() && z.is_finite()) {
            return 1.0;
        }
        let (gx, gz) = self.cell_of(x, z);
        let mut worst = 1.0f32;
        for &i in &self.cells[(gz * self.dim + gx) as usize] {
            let r = &self.all[i as usize];
            let (ax, az, br) = r.shape.nearest_axis(x, z);
            let d = (x - ax).hypot(z - az) - br;
            let w = if d <= 0.0 {
                1.0
            } else if d >= r.edge {
                continue;
            } else {
                let t = 1.0 - d / r.edge;
                t * t * (3.0 - 2.0 * t)
            };
            let share = if who.stalking && r.ground == Ground::Undergrowth { STALK_UNDERGROWTH } else { 1.0 };
            worst = worst.min(1.0 - (1.0 - r.mul) * w * share);
        }
        worst.max(MIN_GOING)
    }

    /// The average going along a straight walk from `a` to `b`, sampled. What
    /// a steering bot asks: not "is it slow here" but "is that way slow".
    pub fn along(&self, a: Vec3, b: Vec3, who: Carriage) -> f32 {
        let d = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
        let len = d.length();
        let n = ((len / 0.75).ceil() as usize).clamp(1, 64);
        // Harmonic mean: time is what adds up along a path, and time is
        // distance over speed. An arithmetic mean would let one fast stretch
        // buy back a thicket.
        let mut inv = 0.0;
        for k in 1..=n {
            let p = a + d * (k as f32 / n as f32);
            inv += 1.0 / self.at(p.x, p.z, who);
        }
        n as f32 / inv
    }
}

fn bounds(s: &Shape) -> (f32, f32, f32) {
    match *s {
        Shape::Circle { x, z, r } => (x, z, r),
        Shape::Segment { x0, z0, x1, z1, r } => {
            let (cx, cz) = ((x0 + x1) * 0.5, (z0 + z1) * 0.5);
            (cx, cz, (x1 - x0).hypot(z1 - z0) * 0.5 + r)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(mul: f32, ground: Ground) -> Going {
        let mut g = Going::new(50.0, 4.0);
        assert!(g.push(Rough { shape: Shape::Circle { x: 0.0, z: 0.0, r: 2.0 }, mul, edge: 1.0, ground }));
        g
    }
    const RUN: Carriage = Carriage { stalking: false };
    const STALK: Carriage = Carriage { stalking: true };

    #[test]
    fn open_ground_is_full_speed_and_the_core_is_the_patch() {
        let g = patch(0.6, Ground::Thicket);
        assert_eq!(g.at(10.0, 0.0, RUN), 1.0);
        assert!((g.at(0.5, 0.0, RUN) - 0.6).abs() < 1e-5);
        assert!((g.at(-1.9, 0.0, RUN) - 0.6).abs() < 1e-5, "the whole disc, not its centre");
    }

    /// The soft edge: monotone from the patch's value out to 1, no step.
    #[test]
    fn the_edge_fades_monotonically() {
        let g = patch(0.5, Ground::Scree);
        let mut last = 0.0;
        let mut x = 1.5;
        while x < 3.5 {
            let v = g.at(x, 0.0, RUN);
            assert!(v >= last - 1e-6, "not monotone at {x}: {v} after {last}");
            last = v;
            x += 0.05;
        }
        assert_eq!(g.at(3.01, 0.0, RUN), 1.0, "edge ends at r + edge");
        let mid = g.at(2.5, 0.0, RUN);
        assert!(mid > 0.55 && mid < 0.95, "half way out is half way: {mid}");
    }

    #[test]
    fn overlap_takes_the_worst_not_the_product() {
        let mut g = patch(0.7, Ground::Undergrowth);
        g.push(Rough { shape: Shape::Circle { x: 0.5, z: 0.0, r: 2.0 }, mul: 0.7, edge: 1.0, ground: Ground::Undergrowth });
        g.push(Rough { shape: Shape::Circle { x: -0.5, z: 0.0, r: 2.0 }, mul: 0.6, edge: 1.0, ground: Ground::Thicket });
        assert!((g.at(0.0, 0.0, RUN) - 0.6).abs() < 1e-5, "{}", g.at(0.0, 0.0, RUN));
    }

    #[test]
    fn a_stalking_cat_slips_through_undergrowth_but_not_a_thicket() {
        let fern = patch(0.6, Ground::Undergrowth);
        let run = fern.at(0.0, 0.0, RUN);
        let stalk = fern.at(0.0, 0.0, STALK);
        assert!((stalk - (1.0 - 0.4 * STALK_UNDERGROWTH)).abs() < 1e-5, "{stalk}");
        assert!(stalk > run);
        let bamboo = patch(0.6, Ground::Thicket);
        assert_eq!(bamboo.at(0.0, 0.0, STALK), bamboo.at(0.0, 0.0, RUN), "stems too close to slip between");
    }

    #[test]
    fn a_capsule_is_rough_along_its_length() {
        let mut g = Going::new(50.0, 4.0);
        g.push(Rough { shape: Shape::Segment { x0: -12.0, z0: 0.0, x1: 12.0, z1: 0.0, r: 0.8 }, mul: 0.8, edge: 0.5, ground: Ground::Roots });
        for x in [-11.0, -5.0, 0.0, 6.0, 11.5] {
            assert!((g.at(x, 0.2, RUN) - 0.8).abs() < 1e-5, "at x = {x}");
        }
        assert_eq!(g.at(0.0, 2.0, RUN), 1.0);
    }

    #[test]
    fn nonsense_is_refused_and_the_floor_holds() {
        let mut g = Going::new(50.0, 4.0);
        let c = Shape::Circle { x: 0.0, z: 0.0, r: 1.0 };
        for mul in [0.0, 1.0, 1.5, -0.2, f32::NAN] {
            assert!(!g.push(Rough { shape: c, mul, edge: 0.5, ground: Ground::Scree }), "mul {mul} accepted");
        }
        assert!(!g.push(Rough { shape: Shape::Circle { x: f32::NAN, z: 0.0, r: 1.0 }, mul: 0.5, edge: 0.5, ground: Ground::Scree }));
        assert!(g.push(Rough { shape: c, mul: 0.05, edge: 0.5, ground: Ground::Scree }));
        assert!((g.at(0.0, 0.0, RUN) - MIN_GOING).abs() < 1e-5, "{}", g.at(0.0, 0.0, RUN));
        assert_eq!(g.at(f32::NAN, 0.0, RUN), 1.0, "a NaN position is not slowed into a NaN speed");
    }

    #[test]
    fn a_default_set_is_open_ground() {
        let g = Going::default();
        assert_eq!(g.at(3.0, 4.0, RUN), 1.0);
        assert!(!Going::default().push(Rough { shape: Shape::Circle { x: 0.0, z: 0.0, r: 1.0 }, mul: 0.5, edge: 0.5, ground: Ground::Scree }));
    }

    /// Time, not speed, is what a path adds up: a thicket half way along is
    /// not bought back by the open half.
    #[test]
    fn along_is_a_harmonic_mean() {
        let mut g = Going::new(50.0, 2.0);
        g.push(Rough { shape: Shape::Segment { x0: 5.0, z0: -3.0, x1: 5.0, z1: 3.0, r: 5.0 }, mul: 0.5, edge: 0.0, ground: Ground::Thicket });
        let v = g.along(Vec3::new(-10.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), RUN);
        // half the samples at 1.0, half at 0.5 -> 2/(1+2) = 0.667, not 0.75
        assert!((v - 2.0 / 3.0).abs() < 0.04, "{v}");
    }

    #[test]
    fn undergrowth_and_stone_are_rough_and_trunks_are_not() {
        for k in [PropKind::Fern, PropKind::Bush, PropKind::VineCurtain, PropKind::Bamboo, PropKind::Rock, PropKind::BoulderCluster] {
            let (mul, r, _) = k.going().unwrap_or_else(|| panic!("{k:?} lays no rough ground"));
            assert!(mul > MIN_GOING && mul < 1.0 && r > 0.5, "{k:?}");
        }
        for k in [PropKind::JungleTree, PropKind::MegaTree, PropKind::RuinWall, PropKind::FallenLog, PropKind::BaseGate] {
            assert!(k.going().is_none(), "{k:?} is collision, not going");
        }
    }
}
