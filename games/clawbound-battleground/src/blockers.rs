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
    pub fn nearest_axis(&self, px: f32, pz: f32) -> (f32, f32, f32) {
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
    /// What this prop HIDES. Carried on the same entry as the collision so a
    /// prop cannot be moved in one and forgotten in the other — the same
    /// reasoning that put PropKind through the placement macros.
    pub cover: Cover,
}

/// A disc of concealment. Separate from [`Shape`] because a prop's hiding
/// span is not its collision span: a fern blocks nothing and hides 1.3 units,
/// a boulder does both over the same radius.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Cover {
    pub x: f32,
    pub z: f32,
    pub radius: f32,
    /// 0 = hides nothing, 1 = breaks the view entirely.
    pub amount: f32,
}

impl Cover {
    pub const NONE: Cover = Cover { x: 0.0, z: 0.0, radius: 0.0, amount: 0.0 };
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
    /// How much this prop HIDES, 0..1, independent of whether it blocks.
    ///
    /// A wall and a bramble thicket are opposites: the wall stops you and the
    /// thicket hides you, and a prop can do either, both, or neither. The
    /// blockers grid was answering only "can I walk through it", which made
    /// ferns and bushes pure decoration — the map had nowhere to dive.
    ///
    /// Concealment RADIUS is the collision radius when solid, and
    /// `conceal_radius` when passable, because a fern conceals across a
    /// wider span than the stem it can't be walked through at.
    pub conceal: f32,
    /// Span over which a passable prop conceals. Ignored when solid.
    pub conceal_radius: f32,
}

impl Footprint {
    pub const fn solid(radius: f32, top: f32) -> Footprint {
        // A solid prop conceals fully within its own footprint: you cannot
        // see through a boulder.
        Footprint { radius, top, passable: false, conceal: 1.0, conceal_radius: radius }
    }
    /// Passable AND hiding — a thicket, a fern bed, a hanging vine.
    ///
    /// This is the case the module was missing. `conceal` below 1.0 is
    /// deliberate for most flora: standing in a fern makes you hard to see,
    /// not invisible, and a binary would turn every bush into a cloak.
    pub const fn cover(conceal: f32, conceal_radius: f32) -> Footprint {
        Footprint { radius: 0.0, top: 0.0, passable: true, conceal, conceal_radius }
    }
    pub const fn passable() -> Footprint {
        Footprint { radius: 0.0, top: 0.0, passable: true, conceal: 0.0, conceal_radius: 0.0 }
    }

    /// Does this prop hide anything at all?
    pub const fn conceals(&self) -> bool {
        self.conceal > 0.0 && self.conceal_radius > 0.0
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

            // Walk-through, but NOT nothing: these are the props a cat dives
            // into. Values are what they hide you from at a glance, and none
            // is 1.0 — a fern makes you hard to see, not invisible, and a
            // binary would turn every bush into a cloak.
            //
            // Radii are wider than a collision radius would be: concealment
            // is about the spread of leaf, not the stem.
            Bush => Footprint::cover(0.80, 1.6),
            Fern => Footprint::cover(0.55, 1.3),
            // A hanging curtain hides a lot from one side and is the classic
            // ambush spot, hence the highest of the four.
            VineCurtain => Footprint::cover(0.90, 1.1),
            // A painted pad on the ground hides nothing; it is passable
            // because it is flat, which is a different reason.
            LanePad => Footprint::passable(),
        }
    }
}

/// A hump a cat climbs OVER rather than walks around: a buttress root, a low
/// slab. It never pushes anyone — `resolve` does not see it — it only answers
/// "how high is the thing under this paw", which is `puppet::Probe::step_top`.
///
/// Why this is not a `Blocker` with a flag: a blocker's job is to say no to
/// movement, and every consumer of `all` (collision, sight lines, the AI's
/// `blocked`) would have had to learn to skip the flag. Jungle-tree roots
/// reach 4.8 u from a trunk whose collision radius is 0.9 u; making them
/// solid turns every tree into a plus-shaped trap for a bot that paths by
/// push-out. Making them walkable changes nothing about where anyone can go.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub shape: Shape,
    /// Height of the crest above `base_y`.
    pub top: f32,
    pub base_y: f32,
}

/// Taller than this and a cat does not scramble over it, it leaps.
pub const SCRAMBLE_MAX: f32 = 2.0;

/// A uniform grid of blockers.
///
/// Built once at world construction and queried every movement tick for every
/// god and minion. A linear scan over ~400 props per unit per tick is the
/// naive version and is the thing this avoids: the grid makes a query touch
/// the handful of cells the unit's own radius overlaps.
#[derive(Debug, Clone, Default)]
pub struct Blockers {
    /// Walkable humps standing on the terrain. See [`Step`].
    steps: Vec<Step>,
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
            steps: Vec::new(),
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
        let solid = !f.passable && f.radius > 0.0;
        if !solid && !f.conceals() {
            // Neither blocks nor hides: a lane pad. Nothing to record.
            return false;
        }
        let r = if solid { f.radius * scale.max(0.01) } else { 0.0 };
        self.push(Blocker {
            // A cover-only prop gets a ZERO-radius shape rather than being
            // kept out of the set: it still needs a grid cell so
            // `concealment_at` can find it, and a zero radius can never push
            // anyone out (`resolve` compares against `br + radius` and the
            // unit's own radius is what remains).
            shape: Shape::Circle { x, z, r },
            top: if solid { ground_y + f.top } else { f32::NEG_INFINITY },
            cover: Cover {
                x,
                z,
                radius: f.conceal_radius * scale.max(0.01),
                amount: f.conceal,
            },
        });
        solid
    }

    /// Register a walkable hump. Anything taller than [`SCRAMBLE_MAX`] is
    /// refused: it would lift a cat clean off the ground by walking.
    pub fn push_step(&mut self, shape: Shape, base_y: f32, top: f32) -> bool {
        if !(top > 0.0 && top <= SCRAMBLE_MAX) {
            return false;
        }
        self.steps.push(Step { shape, top, base_y });
        true
    }

    /// The outer buttress roots of a jungle tree at `(x, z)`.
    ///
    /// Mirrors `props::jungle_tree`: four roots at `variant * 0.9 + 0.4 +
    /// k * 90°`, boxes three voxels wide at 0.6 u/voxel. Only the outer two
    /// boxes of each root are low enough (1.8 u) to scramble; the inner three
    /// are 2.4 u and up and are still ghost geometry — see TODO.md. A
    /// placement's quarter-turn maps the four directions onto themselves, so
    /// it does not need to be known here.
    pub fn push_jungle_roots(&mut self, x: f32, z: f32, base_y: f32, variant: i32) -> usize {
        const VOX: f32 = 0.6;
        let mut n = 0;
        for k in 0..4 {
            let a = variant as f32 * 0.9 + k as f32 * std::f32::consts::FRAC_PI_2 + 0.4;
            let (dx, dz) = (a.cos(), a.sin());
            // boxes i = 3, 4 of the mesh: centres at 6.65 and 8.0 voxels.
            let (r0, r1) = (6.65 * VOX, 8.0 * VOX);
            let shape = Shape::Segment { x0: x + dx * r0, z0: z + dz * r0, x1: x + dx * r1, z1: z + dz * r1, r: 1.5 * VOX };
            n += self.push_step(shape, base_y, 3.0 * VOX) as usize;
        }
        n
    }

    /// World Y of the highest hump under `(x, z)`, if any. Rounded off
    /// toward the rim so a paw rises onto it instead of snapping.
    pub fn step_top(&self, x: f32, z: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        for s in &self.steps {
            let (ax, az, br) = s.shape.nearest_axis(x, z);
            let d2 = (x - ax) * (x - ax) + (z - az) * (z - az);
            if br > 0.0 && d2 < br * br {
                let h = s.base_y + s.top * (1.0 - d2 / (br * br)).sqrt();
                best = Some(best.map_or(h, |b| b.max(h)));
            }
        }
        best
    }

    pub fn steps(&self) -> &[Step] {
        &self.steps
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
            // A wall's concealment is handled by its own solid footprint at
            // each sample point along it, so the disc is left empty rather
            // than guessing a midpoint.
            cover: Cover::NONE,
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

    /// How hidden is a unit standing at `(x, z)`, 0..1?
    ///
    /// The cover half of this module. `resolve` answers "can I walk there";
    /// this answers "can I be seen there", and they are genuinely different
    /// questions about the same props — a boulder does both, a fern only the
    /// second, a bridge neither.
    ///
    /// Concealment does NOT sum across overlapping props. Three ferns in a
    /// patch make you no more hidden than the best one, because the alternative
    /// is a scatter density accidentally becoming an invisibility field —
    /// and the jungle generator plants ferns in clumps of six.
    pub fn concealment_at(&self, x: f32, z: f32) -> f32 {
        let mut scratch = Vec::new();
        self.near(x, z, self.cell * 2.0, &mut scratch);
        let mut best = 0.0f32;
        for &i in &scratch {
            let c = self.all[i as usize].cover;
            if c.amount <= 0.0 {
                continue;
            }
            let d2 = (x - c.x).powi(2) + (z - c.z).powi(2);
            if d2 < c.radius * c.radius {
                best = best.max(c.amount);
            }
        }
        best.min(1.0)
    }

    /// Total sight obstruction along the segment from `a` to `b`.
    ///
    /// Sampled rather than analytically integrated: the props are circles and
    /// the exact chord lengths are computable, but a watcher only needs to
    /// know whether the view is broken, and sampling lets a solid blocker and
    /// a thicket contribute through one path. Step is half a cell, so nothing
    /// narrower than a cell can be missed entirely.
    ///
    /// Returns 0..1, saturating: once the line is fully broken, more foliage
    /// behind it changes nothing.
    pub fn sight_obstruction(&self, a: (f32, f32), b: (f32, f32)) -> f32 {
        let (dx, dz) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dz * dz).sqrt();
        if len < 1e-4 {
            return self.concealment_at(a.0, a.1);
        }
        let step = (self.cell * 0.5).max(0.25);
        let n = ((len / step).ceil() as usize).clamp(1, 512);
        let mut worst = 0.0f32;
        for k in 0..=n {
            let t = k as f32 / n as f32;
            worst = worst.max(self.concealment_at(a.0 + dx * t, a.1 + dz * t));
            if worst >= 1.0 {
                break;
            }
        }
        worst
    }

    /// Is `(x, z)` inside any blocker? Used by map generation and spawn
    /// placement to avoid dropping something inside a rock.
    pub fn blocked(&self, x: f32, z: f32, radius: f32) -> bool {
        let mut scratch = Vec::new();
        self.near(x, z, radius + self.cell, &mut scratch);
        scratch.iter().any(|&i| {
            let b = self.all[i as usize];
            // Cover-only entries live in the same set with a zero-radius
            // shape. Without this they would answer "blocked" for any query
            // whose own radius reached them, and spawn placement would refuse
            // to put anything near a fern.
            if b.top == f32::NEG_INFINITY {
                return false;
            }
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
        b.push(Blocker { shape, top: 5.0, cover: Cover::NONE });
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
        b.push(Blocker { shape: Shape::Circle { x: -1.0, z: 0.0, r: 1.5 }, top: 5.0, cover: Cover::NONE });
        b.push(Blocker { shape: Shape::Circle { x: 1.0, z: 0.0, r: 1.5 }, top: 5.0, cover: Cover::NONE });
        let out = b.resolve(Vec3::new(0.0, 0.0, 0.1), 0.4);
        assert!(!b.blocked(out.x, out.z, 0.4), "ended inside a blocker: {out:?}");
    }

    /// A dash that carries a god over a boulder should not be shoved sideways
    /// by it mid-flight.
    #[test]
    fn airborne_units_clear_low_blockers() {
        let mut b = Blockers::new(100.0, 4.0);
        b.push(Blocker { shape: Shape::Circle { x: 0.0, z: 0.0, r: 2.0 }, top: 1.4, cover: Cover::NONE });
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
        // Flat or spanning props record nothing at all.
        for k in [PropKind::LanePad, PropKind::Bridge] {
            assert!(!b.push_prop(k, 0.0, 0.0, 0.0, 1.0), "{k:?} must be passable");
        }
        assert!(b.is_empty());
        // Cover props DO record an entry (that is how `concealment_at` finds
        // them) but must never push: this test predates `Footprint::cover`
        // and asserted `is_empty()` here, which stopped being the design.
        for k in [PropKind::Fern, PropKind::Bush, PropKind::VineCurtain] {
            assert!(!b.push_prop(k, 0.0, 0.0, 0.0, 1.0), "{k:?} must be passable");
        }
        assert_eq!(b.len(), 3);
        let at = Vec3::new(0.05, 0.0, 0.0);
        assert_eq!(b.resolve(at, 0.6), at, "standing in a fern moves nobody");
        assert!(b.concealment_at(0.0, 0.0) > 0.5);
        let mut b = Blockers::new(100.0, 4.0);
        // And a solid one does get added.
        assert!(b.push_prop(PropKind::RuinPillar, 5.0, 5.0, 0.0, 1.0));
        assert_eq!(b.len(), 1);
    }

    /// Roots lift paws and move nobody. Both halves matter: the first is
    /// the feature, the second is the promise that bots path as before.
    #[test]
    fn roots_are_climbed_over_and_never_push() {
        let mut b = Blockers::new(100.0, 6.0);
        assert_eq!(b.push_jungle_roots(10.0, 10.0, 0.0, 0), 4);
        assert!(b.is_empty(), "a root is not a blocker");
        // variant 0, k 0: direction 0.4 rad, crest between 3.99 and 4.8 u out.
        let (dx, dz) = (0.4f32.cos(), 0.4f32.sin());
        let on = (10.0 + dx * 4.4, 10.0 + dz * 4.4);
        let crest = b.step_top(on.0, on.1).expect("the crest of the root");
        assert!((1.75..=1.8001).contains(&crest), "crest height {crest}");
        let rim = b.step_top(on.0 - dz * 0.8, on.1 + dx * 0.8).expect("still on it");
        assert!(rim < crest * 0.6, "the rim is rounded off, not a cliff: {rim}");
        assert_eq!(b.step_top(10.0, 10.0), None, "the trunk itself is not a step");
        assert_eq!(b.step_top(-50.0, -50.0), None);
        let p = Vec3::new(on.0, 0.0, on.1);
        assert_eq!(b.resolve(p, 0.6), p, "standing on a root moves nobody");
        assert!(!b.blocked(on.0, on.1, 0.6));
        assert!(!b.push_step(Shape::Circle { x: 0.0, z: 0.0, r: 1.0 }, 0.0, SCRAMBLE_MAX + 0.1), "too tall to scramble");
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
            b.push(Blocker { shape: Shape::Circle { x, z, r: 2.0 }, top: 5.0, cover: Cover::NONE });
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

    // ── cover ────────────────────────────────────────────────────────────

    /// THE THICKET. "All three cats dived for cover into a bramble thicket" —
    /// a prop that hides you and does NOT stop you. Before this the grid only
    /// answered "can I walk through it", so every fern was decoration and the
    /// map had nowhere to dive.
    #[test]
    fn a_bush_hides_without_blocking() {
        let mut b = Blockers::new(100.0, 4.0);
        // push_prop returns whether it made a SOLID blocker; a bush is cover
        // only, so false -- but it is still recorded.
        assert!(!b.push_prop(PropKind::Bush, 0.0, 0.0, 0.0, 1.0));
        assert_eq!(b.len(), 1, "cover-only props are still in the set");

        let p = Vec3::new(0.0, 0.0, 0.0);
        assert_eq!(b.resolve(p, 0.5), p, "a bush must never push a cat out");
        assert!(!b.blocked(0.0, 0.0, 0.5), "and must not refuse a spawn");
        assert!(b.concealment_at(0.0, 0.0) > 0.5, "but it hides");
    }

    /// A boulder does both; a lane pad does neither.
    #[test]
    fn solid_props_conceal_and_flat_ones_do_not() {
        let mut b = Blockers::new(100.0, 4.0);
        assert!(b.push_prop(PropKind::BoulderCluster, 0.0, 0.0, 0.0, 1.0));
        assert_eq!(b.concealment_at(0.0, 0.0), 1.0, "you cannot see through a boulder");
        assert!(b.blocked(0.0, 0.0, 0.3));

        let mut b2 = Blockers::new(100.0, 4.0);
        assert!(!b2.push_prop(PropKind::LanePad, 0.0, 0.0, 0.0, 1.0));
        assert!(b2.is_empty(), "a flat pad neither blocks nor hides");
    }

    /// Concealment must not SUM. The jungle generator plants ferns in clumps
    /// of six; summing would make a scatter density into an invisibility
    /// field by accident.
    #[test]
    fn overlapping_cover_takes_the_best_not_the_total() {
        let mut b = Blockers::new(100.0, 4.0);
        for i in 0..6 {
            b.push_prop(PropKind::Fern, i as f32 * 0.1, 0.0, 0.0, 1.0);
        }
        let one = PropKind::Fern.footprint().conceal;
        assert!((b.concealment_at(0.0, 0.0) - one).abs() < 1e-3,
            "six ferns hide as well as one, got {}", b.concealment_at(0.0, 0.0));
        assert!(b.concealment_at(0.0, 0.0) < 1.0, "and never fully");
    }

    /// No prop below 1.0 may make a cat invisible -- a binary would turn
    /// every bush into a cloak, which is the tuning mistake this guards.
    #[test]
    fn no_passable_cover_is_total() {
        for k in [PropKind::Bush, PropKind::Fern, PropKind::VineCurtain] {
            let f = k.footprint();
            assert!(f.passable, "{k:?} must stay walk-through");
            assert!(f.conceals(), "{k:?} must hide something");
            assert!(f.conceal < 1.0, "{k:?} must not be a cloak");
            assert!(f.conceal_radius > 0.0);
        }
    }

    /// A watcher's line of sight is broken by what lies ALONG it, not by what
    /// is near either end.
    #[test]
    fn sight_is_obstructed_by_cover_on_the_line() {
        let mut b = Blockers::new(100.0, 4.0);
        b.push_prop(PropKind::Bush, 5.0, 0.0, 0.0, 1.0);
        // Looking straight through the bush.
        assert!(b.sight_obstruction((0.0, 0.0), (10.0, 0.0)) > 0.5);
        // Looking parallel to it, well off to the side: clear.
        assert_eq!(b.sight_obstruction((0.0, 8.0), (10.0, 8.0)), 0.0);
    }

    /// Obstruction saturates: once the view is broken, more foliage behind it
    /// changes nothing.
    #[test]
    fn obstruction_saturates_at_one() {
        let mut b = Blockers::new(100.0, 4.0);
        for i in 0..5 {
            b.push_prop(PropKind::BoulderCluster, 2.0 + i as f32 * 2.0, 0.0, 0.0, 1.0);
        }
        assert_eq!(b.sight_obstruction((0.0, 0.0), (14.0, 0.0)), 1.0);
    }

    /// Scale widens what a prop hides, the same way it widens what it blocks.
    #[test]
    fn scale_widens_concealment_too() {
        let mut b = Blockers::new(100.0, 4.0);
        b.push_prop(PropKind::Bush, 0.0, 0.0, 0.0, 2.0);
        let r = PropKind::Bush.footprint().conceal_radius;
        assert!(b.concealment_at(r * 1.5, 0.0) > 0.0, "a 2x bush hides further out");
    }

    /// An empty set hides nothing and obstructs nothing, without panicking on
    /// a degenerate ray.
    #[test]
    fn an_empty_set_conceals_nothing() {
        let b = Blockers::new(100.0, 4.0);
        assert_eq!(b.concealment_at(1.0, 2.0), 0.0);
        assert_eq!(b.sight_obstruction((0.0, 0.0), (0.0, 0.0)), 0.0, "zero-length ray");
        assert_eq!(b.sight_obstruction((0.0, 0.0), (50.0, 50.0)), 0.0);
    }
}
