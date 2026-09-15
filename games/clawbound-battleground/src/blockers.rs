//! blockers — static world collision for props.
//!
//! Before this, every prop in the game was scenery you walked straight
//! through: trees, boulders, ruin walls, the ziggurats, the colossus heads.
//! `systems::move_gods` applied `pos += dir * speed * dt`, clamped to
//! `ARENA_RADIUS`, then pushed units apart from each other — and that was the
//! whole of collision. Nothing consulted the props at all, because `props.rs`
//! only ever produced meshes and `scene.rs` (the render side) was the only
//! thing that knew where they had been placed.
//!
//! ## Why cylinders
//!
//! The props are voxel meshes, so the exact shape is available and tempting.
//! It is also the wrong thing to collide against: a per-voxel test on a
//! `mega_tree` is hundreds of cells for a trunk a player experiences as a
//! post, and the frame cost buys a difference nobody can feel through a
//! twin-stick camera. A vertical cylinder per prop matches how these read in
//! play (trunk, boulder, pillar) and makes the resolve step a distance
//! compare.
//!
//! The exceptions are the props that are genuinely long rather than round —
//! `ruin_wall`, `fallen_log`, `bridge`, `aqueduct_arch`. Those get a
//! [`Shape::Segment`]: a capsule around a line, which is the same cheap test
//! with an extra clamp and does not turn a wall into a bollard.
//!
//! ## Passable props are not blockers
//!
//! Ferns, bushes, vine curtains and lane pads are deliberately absent from
//! the blocker set. They are ankle-high or hanging, the jungle is already
//! visually dense, and a bush that stops a chase reads as a bug rather than
//! as terrain. [`Footprint::passable`] records that as a decision rather than
//! an omission, so a later reader can tell "we chose not to" from "we forgot".
//!
//! ## Layer
//!
//! Pure geometry: no renderer, no `Game`, no ECS. It takes positions in and
//! gives corrected positions out, which is what lets the sim use it without
//! depending on `scene.rs` and lets the tests below run without a GPU.

use glam::Vec3;

/// How a prop occupies the ground plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// A vertical cylinder at `(x, z)`.
    Circle { x: f32, z: f32, r: f32 },
    /// A capsule around the segment `(x0,z0) -> (x1,z1)`, radius `r`.
    /// Walls, logs, bridges — things longer than they are round.
    Segment { x0: f32, z0: f32, x1: f32, z1: f32, r: f32 },
}

impl Shape {
    /// Closest point on the shape's axis to `(px, pz)`, and the shape radius.
    ///
    /// Collapsing both variants to (axis point, radius) is what keeps
    /// [`Blockers::resolve`] from branching per shape: a circle is just a
    /// segment whose endpoints coincide.
    fn nearest_axis(&self, px: f32, pz: f32) -> (f32, f32, f32) {
        match *self {
            Shape::Circle { x, z, r } => (x, z, r),
            Shape::Segment { x0, z0, x1, z1, r } => {
                let (dx, dz) = (x1 - x0, z1 - z0);
                let len2 = dx * dx + dz * dz;
                if len2 <= 1e-6 {
                    return (x0, z0, r);
                }
                let t = (((px - x0) * dx + (pz - z0) * dz) / len2).clamp(0.0, 1.0);
                (x0 + dx * t, z0 + dz * t, r)
            }
        }
    }

    /// Bounding half-extent around the shape's centre, for grid insertion.
    fn bounds(&self) -> (f32, f32, f32) {
        match *self {
            Shape::Circle { x, z, r } => (x, z, r),
            Shape::Segment { x0, z0, x1, z1, r } => {
                let (cx, cz) = ((x0 + x1) * 0.5, (z0 + z1) * 0.5);
                let half = (((x1 - x0).powi(2) + (z1 - z0).powi(2)).sqrt()) * 0.5;
                (cx, cz, half + r)
            }
        }
    }
}

/// One solid piece of world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blocker {
    pub shape: Shape,
    /// Ground height the top of the obstacle reaches, in world units. A dash
    /// or a knock-up above this clears it; see [`Blockers::resolve_at`].
    pub top: f32,
}

/// What a prop does to movement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Footprint {
    /// Collision radius in world units. Ignored when `passable`.
    pub radius: f32,
    /// Height the obstacle blocks up to.
    pub top: f32,
    /// Deliberately walk-through. See the module header — this is a recorded
    /// decision, not a missing entry.
    pub passable: bool,
}

impl Footprint {
    pub const fn solid(radius: f32, top: f32) -> Footprint {
        Footprint { radius, top, passable: false }
    }
    pub const fn passable() -> Footprint {
        Footprint { radius: 0.0, top: 0.0, passable: true }
    }
}

/// Every prop the scene can place, with the footprint it presents.
///
/// This exists so the blocker radius lives next to the prop identity rather
/// than at the ~40 call sites in `scene.rs` that place them. Adding a prop
/// means adding a variant, and the compiler then requires a footprint
/// decision for it — including the decision that it is passable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropKind {
    Pine,
    Oak,
    Rock,
    BoulderCluster,
    VineCurtain,
    Crystal,
    Brazier,
    BaseGate,
    Fountain,
    TitanPedestal,
    LanePad,
    JungleTree,
    Palm,
    StranglerFig,
    Bamboo,
    Fern,
    Bush,
    FallenLog,
    RuinPillar,
    RuinArch,
    RuinWall,
    StoneHead,
    Bridge,
    MegaTree,
    Ziggurat,
    ColossusHead,
    AqueductArch,
}

impl PropKind {
    /// Radii are the mesh's visual trunk/base, not its canopy: a jungle tree
    /// reads as a 0.9-unit trunk with leaves you can stand under, and
    /// blocking the canopy would make the jungle impassable rather than
    /// dense. Scaled props multiply this by their own `s` at placement.
    pub const fn footprint(self) -> Footprint {
        use PropKind::*;
        match self {
            // Trunks — block the post, not the leaves.
            Pine => Footprint::solid(0.75, 3.0),
            Oak => Footprint::solid(0.85, 3.0),
            JungleTree => Footprint::solid(0.90, 4.0),
            Palm => Footprint::solid(0.55, 4.0),
            StranglerFig => Footprint::solid(1.10, 4.0),
            Bamboo => Footprint::solid(0.45, 4.0),
            MegaTree => Footprint::solid(2.60, 8.0),

            // Stone.
            Rock => Footprint::solid(0.90, 1.4),
            BoulderCluster => Footprint::solid(1.50, 2.0),
            RuinPillar => Footprint::solid(0.80, 4.0),
            StoneHead => Footprint::solid(1.60, 2.6),
            Crystal => Footprint::solid(0.70, 2.0),

            // Structures. A gate is a wall you path around, not through.
            BaseGate => Footprint::solid(3.20, 5.0),
            Fountain => Footprint::solid(2.00, 1.6),
            TitanPedestal => Footprint::solid(2.40, 1.2),
            Ziggurat => Footprint::solid(7.00, 9.0),
            ColossusHead => Footprint::solid(4.00, 6.0),
            Brazier => Footprint::solid(0.55, 1.6),

            // Long things: radius here is the capsule's thickness; the length
            // comes from the placement, which is why these are Segments.
            RuinWall => Footprint::solid(0.70, 3.0),
            FallenLog => Footprint::solid(0.60, 0.9),
            Bridge => Footprint::passable(),
            AqueductArch => Footprint::solid(1.20, 7.0),
            // An arch is a doorway. Blocking its centre would wall off the
            // thing it exists to let you walk through; the piers are placed
            // as two separate blockers at build time instead.
            RuinArch => Footprint::passable(),

            // Deliberately walk-through — see the module header.
            Fern | Bush | VineCurtain | LanePad => Footprint::passable(),
        }
    }
}

/// A uniform grid of blockers.
///
/// Built once at world construction and queried every movement tick for every
/// god and minion. A linear scan over ~400 props per unit per tick is the
/// naive version and is the thing this avoids: the grid makes a query touch
/// the handful of cells the unit's own radius overlaps.
#[derive(Debug, Clone, Default)]
pub struct Blockers {
    all: Vec<Blocker>,
    /// cell index -> indices into `all`.
    cells: Vec<Vec<u32>>,
    cell: f32,
    dim: i32,
    half_extent: f32,
}

impl Blockers {
    /// `half_extent` should cover the arena; `cell` is the grid pitch (a few
    /// world units — big enough that most props land in one cell, small
    /// enough that a query touches few).
    pub fn new(half_extent: f32, cell: f32) -> Blockers {
        let cell = cell.max(1.0);
        let dim = ((half_extent * 2.0 / cell).ceil() as i32).max(1);
        Blockers {
            all: Vec::new(),
            cells: vec![Vec::new(); (dim * dim) as usize],
            cell,
            dim,
            half_extent,
        }
    }

    pub fn len(&self) -> usize {
        self.all.len()
    }
    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }
    pub fn all(&self) -> &[Blocker] {
        &self.all
    }

    fn cell_of(&self, x: f32, z: f32) -> (i32, i32) {
        let gx = ((x + self.half_extent) / self.cell).floor() as i32;
        let gz = ((z + self.half_extent) / self.cell).floor() as i32;
        (gx.clamp(0, self.dim - 1), gz.clamp(0, self.dim - 1))
    }

    /// Add a blocker, registering it in every cell its bounds touch.
    pub fn push(&mut self, b: Blocker) {
        let idx = self.all.len() as u32;
        let (cx, cz, half) = b.shape.bounds();
        let (x0, z0) = self.cell_of(cx - half, cz - half);
        let (x1, z1) = self.cell_of(cx + half, cz + half);
        for gz in z0..=z1 {
            for gx in x0..=x1 {
                self.cells[(gz * self.dim + gx) as usize].push(idx);
            }
        }
        self.all.push(b);
    }

    /// Add a prop's blocker, if it has one.
    ///
    /// `scale` multiplies the footprint radius the way the mesh builders
    /// take an `s` — a 1.3-scale jungle tree presents a 1.3x trunk. Returns
    /// whether anything was added, so a caller can assert coverage.
    pub fn push_prop(&mut self, kind: PropKind, x: f32, z: f32, ground_y: f32, scale: f32) -> bool {
        let f = kind.footprint();
        if f.passable || f.radius <= 0.0 {
            return false;
        }
        self.push(Blocker {
            shape: Shape::Circle { x, z, r: f.radius * scale.max(0.01) },
            top: ground_y + f.top,
        });
        true
    }

    /// Add a long prop as a capsule between two points.
    pub fn push_segment(
        &mut self,
        kind: PropKind,
        x0: f32,
        z0: f32,
        x1: f32,
        z1: f32,
        ground_y: f32,
        scale: f32,
    ) -> bool {
        let f = kind.footprint();
        if f.passable || f.radius <= 0.0 {
            return false;
        }
        self.push(Blocker {
            shape: Shape::Segment { x0, z0, x1, z1, r: f.radius * scale.max(0.01) },
            top: ground_y + f.top,
        });
        true
    }

    /// Candidate blocker indices near `(x, z)` within `pad`.
    fn near(&self, x: f32, z: f32, pad: f32, out: &mut Vec<u32>) {
        out.clear();
        let (x0, z0) = self.cell_of(x - pad, z - pad);
        let (x1, z1) = self.cell_of(x + pad, z + pad);
        for gz in z0..=z1 {
            for gx in x0..=x1 {
                for &i in &self.cells[(gz * self.dim + gx) as usize] {
                    if !out.contains(&i) {
                        out.push(i);
                    }
                }
            }
        }
    }

    /// Push `pos` out of any blocker it overlaps, returning the corrected
    /// position. `radius` is the moving unit's own radius.
    ///
    /// Runs a few relaxation passes rather than one. A single pass resolves
    /// one blocker and can shove the unit into its neighbour — which is
    /// exactly what happens in a boulder cluster or along a wall, and reads
    /// as popping through geometry. Three passes settles the common corner
    /// case; it is capped rather than looped to convergence because a unit
    /// wedged in a crevice must not cost an unbounded number of iterations.
    pub fn resolve(&self, pos: Vec3, radius: f32) -> Vec3 {
        self.resolve_at(pos, radius, f32::NEG_INFINITY)
    }

    /// As [`Self::resolve`], but ignores blockers whose `top` is below
    /// `feet_y` — a unit dashing or knocked above a boulder passes over it.
    ///
    /// Pass `f32::NEG_INFINITY` for ground movement (nothing is skipped).
    pub fn resolve_at(&self, pos: Vec3, radius: f32, feet_y: f32) -> Vec3 {
        if self.all.is_empty() {
            return pos;
        }
        let mut p = pos;
        let mut scratch = Vec::new();
        for _ in 0..3 {
            let mut moved = false;
            self.near(p.x, p.z, radius + self.cell, &mut scratch);
            for &i in &scratch {
                let b = self.all[i as usize];
                if b.top <= feet_y {
                    continue;
                }
                let (ax, az, br) = b.shape.nearest_axis(p.x, p.z);
                let (dx, dz) = (p.x - ax, p.z - az);
                let d2 = dx * dx + dz * dz;
                let need = br + radius;
                if d2 >= need * need {
                    continue;
                }
                let d = d2.sqrt();
                if d > 1e-4 {
                    let s = (need - d) / d;
                    p.x += dx * s;
                    p.z += dz * s;
                } else {
                    // Dead centre: no direction to push along. Pick one
                    // deterministically rather than leaving the unit stuck
                    // inside the prop — an arbitrary but stable axis beats a
                    // NaN or a freeze.
                    p.x += need;
                }
                moved = true;
            }
            if !moved {
                break;
            }
        }
        p
    }

    /// Is `(x, z)` inside any blocker? Used by map generation and spawn
    /// placement to avoid dropping something inside a rock.
    pub fn blocked(&self, x: f32, z: f32, radius: f32) -> bool {
        let mut scratch = Vec::new();
        self.near(x, z, radius + self.cell, &mut scratch);
        scratch.iter().any(|&i| {
            let b = self.all[i as usize];
            let (ax, az, br) = b.shape.nearest_axis(x, z);
            let need = br + radius;
            (x - ax).powi(2) + (z - az).powi(2) < need * need
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(shape: Shape) -> Blockers {
        let mut b = Blockers::new(100.0, 4.0);
        b.push(Blocker { shape, top: 5.0 });
        b
    }

    #[test]
    fn a_unit_outside_a_blocker_is_untouched() {
        let b = one(Shape::Circle { x: 0.0, z: 0.0, r: 1.0 });
        let p = Vec3::new(9.0, 0.0, 0.0);
        assert_eq!(b.resolve(p, 0.5), p);
    }

    #[test]
    fn a_unit_inside_is_pushed_to_the_surface() {
        let b = one(Shape::Circle { x: 0.0, z: 0.0, r: 2.0 });
        let out = b.resolve(Vec3::new(0.5, 0.0, 0.0), 0.5);
        // Pushed out along +x to exactly radius + unit radius.
        assert!((out.x - 2.5).abs() < 1e-3, "got {out:?}");
        assert!(out.z.abs() < 1e-3);
    }

    /// The degenerate case that would otherwise divide by zero and strand a
    /// unit inside the prop forever.
    #[test]
    fn dead_centre_still_escapes() {
        let b = one(Shape::Circle { x: 0.0, z: 0.0, r: 2.0 });
        let out = b.resolve(Vec3::ZERO, 0.5);
        assert!(out.is_finite(), "no NaN");
        assert!(!b.blocked(out.x, out.z, 0.5), "must end outside, got {out:?}");
    }

    /// A wall is a wall along its whole length, not a post at its midpoint.
    #[test]
    fn a_segment_blocks_along_its_length() {
        let b = one(Shape::Segment { x0: -10.0, z0: 0.0, x1: 10.0, z1: 0.0, r: 0.7 });
        for x in [-9.0, -4.0, 0.0, 4.0, 9.0] {
            assert!(b.blocked(x, 0.0, 0.5), "wall should block at x={x}");
        }
        // Past the end it does not.
        assert!(!b.blocked(14.0, 0.0, 0.5));
        // And the push is sideways, off the wall, not along it.
        let out = b.resolve(Vec3::new(3.0, 0.0, 0.2), 0.5);
        assert!(out.z > 1.1, "pushed clear in z, got {out:?}");
        assert!((out.x - 3.0).abs() < 1e-3, "not slid along the wall");
    }

    /// One pass can shove a unit out of one rock and into the next. This is
    /// the boulder-cluster case and the reason resolve relaxes.
    #[test]
    fn overlapping_blockers_resolve_to_a_free_spot() {
        let mut b = Blockers::new(100.0, 4.0);
        b.push(Blocker { shape: Shape::Circle { x: -1.0, z: 0.0, r: 1.5 }, top: 5.0 });
        b.push(Blocker { shape: Shape::Circle { x: 1.0, z: 0.0, r: 1.5 }, top: 5.0 });
        let out = b.resolve(Vec3::new(0.0, 0.0, 0.1), 0.4);
        assert!(!b.blocked(out.x, out.z, 0.4), "ended inside a blocker: {out:?}");
    }

    /// A dash that carries a god over a boulder should not be shoved sideways
    /// by it mid-flight.
    #[test]
    fn airborne_units_clear_low_blockers() {
        let mut b = Blockers::new(100.0, 4.0);
        b.push(Blocker { shape: Shape::Circle { x: 0.0, z: 0.0, r: 2.0 }, top: 1.4 });
        let p = Vec3::new(0.5, 0.0, 0.0);
        // On the ground: blocked.
        assert_ne!(b.resolve_at(p, 0.5, f32::NEG_INFINITY), p);
        // Feet above the rock's top: passes over.
        assert_eq!(b.resolve_at(p, 0.5, 2.0), p);
    }

    /// Passable props must contribute nothing, and the enum must say so
    /// rather than the caller having to remember.
    #[test]
    fn passable_props_are_not_added() {
        let mut b = Blockers::new(100.0, 4.0);
        for k in [PropKind::Fern, PropKind::Bush, PropKind::VineCurtain, PropKind::LanePad, PropKind::Bridge] {
            assert!(!b.push_prop(k, 0.0, 0.0, 0.0, 1.0), "{k:?} must be passable");
        }
        assert!(b.is_empty());
        // And a solid one does get added.
        assert!(b.push_prop(PropKind::RuinPillar, 5.0, 5.0, 0.0, 1.0));
        assert_eq!(b.len(), 1);
    }

    /// An arch is a doorway; walling its centre defeats the prop.
    #[test]
    fn an_arch_does_not_block_its_own_opening() {
        assert!(PropKind::RuinArch.footprint().passable);
    }

    #[test]
    fn scale_widens_the_footprint() {
        let mut b = Blockers::new(100.0, 4.0);
        b.push_prop(PropKind::JungleTree, 0.0, 0.0, 0.0, 2.0);
        let r = PropKind::JungleTree.footprint().radius * 2.0;
        assert!(b.blocked(r - 0.1, 0.0, 0.0));
        assert!(!b.blocked(r + 0.1, 0.0, 0.0));
    }

    /// The grid must not change the answer, only the speed — a blocker far
    /// from the origin behaves the same as one at it.
    #[test]
    fn the_grid_is_position_independent() {
        for (x, z) in [(0.0, 0.0), (60.0, -70.0), (-95.0, 95.0)] {
            let mut b = Blockers::new(100.0, 4.0);
            b.push(Blocker { shape: Shape::Circle { x, z, r: 2.0 }, top: 5.0 });
            let probe = Vec3::new(x + 0.5, 0.0, z);
            let out = b.resolve(probe, 0.5);
            assert!(!b.blocked(out.x, out.z, 0.5), "failed at ({x},{z}): {out:?}");
        }
    }

    /// Anything beyond the grid's extent must still be handled rather than
    /// indexing out of bounds — the arena clamp runs after this, and a prop
    /// placed at the rim is ordinary.
    #[test]
    fn out_of_extent_queries_are_clamped_not_panics() {
        let b = one(Shape::Circle { x: 0.0, z: 0.0, r: 1.0 });
        let far = Vec3::new(5000.0, 0.0, -5000.0);
        assert_eq!(b.resolve(far, 0.5), far);
        assert!(!b.blocked(5000.0, -5000.0, 0.5));
    }

    #[test]
    fn an_empty_set_is_a_no_op() {
        let b = Blockers::new(100.0, 4.0);
        let p = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(b.resolve(p, 0.5), p);
        assert!(!b.blocked(1.0, 3.0, 0.5));
    }
}
