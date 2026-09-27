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
            //
            // Radii are the trunk AS DRAWN, measured off the meshes
            // (`props::trunk_fit`, held there by
            // `trunk_collision_is_the_trunk_that_is_drawn`). They were
            // 0.55-0.9 against drawn trunks of 0.7-1.8, and every mesh sat
            // half a voxel off its placement: a cat's shoulder went a third
            // of a metre into the bark on one side and walked round air on
            // the other. Tops are the trunk's height, not a hip-high 3-4 u:
            // a leap now stops at a tree instead of sailing through it.
            Pine => Footprint::solid(1.20, 6.0),
            Oak => Footprint::solid(1.20, 7.0),
            JungleTree => Footprint::solid(1.75, 8.4),
            Palm => Footprint::solid(0.70, 6.0),
            StranglerFig => Footprint::solid(1.50, 9.6),
            // A stand of stems, not a trunk: you shoulder through it. Hides
            // well, costs a lot of speed (`going`), stops nobody.
            Bamboo => Footprint::cover(0.75, 1.5),
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
            // Measured from the meshes, which are 0.6 u/voxel: the wall's arms
            // are three voxels thick and five high, and the "log" is a fallen
            // jungle giant 4 u across and 3.3 u to the top of its moss. These
            // read 0.70/3.0 and 0.60/0.9 -- voxel counts taken for world units.
            RuinWall => Footprint::solid(0.90, 3.0),
            FallenLog => Footprint::solid(1.90, 3.3),
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

/// Can a cat STAND on top of this, and how much room is there?
///
/// A property of the prop, not of its height. The first leap used
/// `top <= 2.4` as the test, which was wrong twice: it called a ruin wall
/// unreachable when a cat clears five times its own shoulder height from a
/// standstill, and it would have called a young pine a platform, because a
/// tree's blocker `top` is how high it BLOCKS, not a surface. Whether there
/// is somewhere to put four paws is a fact about the thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Perch {
    /// Nowhere to stand: trunks, flames, crystal points, team structures.
    #[default]
    None,
    /// Room to stand and turn: a boulder, a carved head.
    Flat,
    /// A ridge a body wide: a wall top, a log, the cap of a pillar. A cat
    /// walks these without thinking about it; what it costs them is that a
    /// solid hit takes their footing (see `leap`).
    Narrow,
}

impl PropKind {
    pub const fn perch(self) -> Perch {
        use PropKind::*;
        match self {
            // The log is here and not below: it is a fallen jungle giant 4 u
            // across. The rig sheet asked the scene for a ridge, was handed a
            // log, and the picture made the mistake obvious.
            Rock | BoulderCluster | StoneHead | ColossusHead | FallenLog => Perch::Flat,
            RuinWall | RuinPillar => Perch::Narrow,
            // Trees block up to a height; they do not END there.
            Pine | Oak | JungleTree | Palm | StranglerFig | MegaTree => Perch::None,
            // Fire, points, and the things each team is there to defend: a
            // cat on the enemy gate is a balance problem, not a vista.
            Brazier | Crystal | BaseGate | Fountain | TitanPedestal | Ziggurat | AqueductArch => Perch::None,
            Bridge | RuinArch | Bush | Fern | VineCurtain | Bamboo | LanePad => Perch::None,
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
    /// Parallel to `all`. Kept beside it rather than inside `Blocker` so a
    /// hand-built blocker (the arena rim, every test double) is unstandable
    /// by default instead of having to say so.
    perches: Vec<Perch>,
    all: Vec<Blocker>,
    /// cell index -> indices into `all`.
    cells: Vec<Vec<u32>>,
    cell: f32,
    dim: i32,
    half_extent: f32,
    /// Rough ground: what the world COSTS to cross, where `all` is what it
    /// forbids. See `going`. Filled by `push_prop` from the same PropKind,
    /// so a prop cannot be moved and leave its thicket behind.
    going: crate::going::Going,
    /// On a map carved out of rock (`floor`), the edge of the floor is a
    /// wall too: a cliff. `None` on the open jungle.
    floor: Option<&'static crate::floor::FloorPlan>,
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
            perches: Vec::new(),
            all: Vec::new(),
            cells: vec![Vec::new(); (dim * dim) as usize],
            cell,
            dim,
            half_extent,
            going: crate::going::Going::new(half_extent, cell),
            floor: None,
        }
    }

    /// Make the edge of `floor` a wall: everything off it is cliff.
    pub fn set_floor(&mut self, floor: &'static crate::floor::FloorPlan) {
        self.floor = Some(floor);
    }
    pub fn floor(&self) -> Option<&'static crate::floor::FloorPlan> {
        self.floor
    }

    /// Speed multiplier for crossing `(x, z)`. See `going`.
    pub fn going_at(&self, x: f32, z: f32, who: crate::going::Carriage) -> f32 {
        self.going.at(x, z, who)
    }

    /// The rough-ground layer, for steering and plans.
    pub fn going(&self) -> &crate::going::Going {
        &self.going
    }

    /// Lay a patch of rough ground that belongs to no single prop: a bramble
    /// bed, a scree slope.
    pub fn push_rough(&mut self, r: crate::going::Rough) -> bool {
        self.going.push(r)
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
        // `dim` is 0 for a default-constructed set (a Game nobody has handed a
        // scene's blockers to: every headless match, every server). `clamp`
        // PANICS when min > max, so the first leap in such a game killed it.
        let hi = (self.dim - 1).max(0);
        (gx.clamp(0, hi), gz.clamp(0, hi))
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
        self.perches.push(Perch::None);
    }

    /// Add a prop's blocker, if it has one.
    ///
    /// `scale` multiplies the footprint radius the way the mesh builders
    /// take an `s` — a 1.3-scale jungle tree presents a 1.3x trunk. Returns
    /// whether anything was added, so a caller can assert coverage.
    pub fn push_prop(&mut self, kind: PropKind, x: f32, z: f32, ground_y: f32, scale: f32) -> bool {
        self.going.push_prop(kind, x, z, scale);
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
        if solid {
            *self.perches.last_mut().expect("just pushed") = kind.perch();
        }
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

    /// The buttress roots of a jungle tree at `(x, z)`.
    ///
    /// Mirrors `props::jungle_tree`: four roots at `variant * 0.9 + 0.4 +
    /// k * 90°`, boxes three voxels wide at 0.6 u/voxel. The outer two
    /// boxes of each root are low enough (1.8 u) to scramble over, and are
    /// steps. The inner three stand 2.4-4.8 u, and are walls a cat goes round
    /// or bounds over (their `top` is the lowest of them, 2.4 u, so a leap
    /// clears the run); they used to be ghost geometry a cat walked through.
    /// A placement's quarter-turn maps the four directions onto themselves,
    /// so it does not need to be known here. Returns the steps added.
    pub fn push_jungle_roots(&mut self, x: f32, z: f32, base_y: f32, variant: i32) -> usize {
        const VOX: f32 = 0.6;
        let mut n = 0;
        for k in 0..4 {
            let a = variant as f32 * 0.9 + k as f32 * std::f32::consts::FRAC_PI_2 + 0.4;
            let (dx, dz) = (a.cos(), a.sin());
            // boxes i = 0..2: from the trunk's edge out to the third box
            let (w0, w1) = (2.6 * VOX, 4.9 * VOX);
            self.push(Blocker {
                shape: Shape::Segment { x0: x + dx * w0, z0: z + dz * w0, x1: x + dx * w1, z1: z + dz * w1, r: 1.35 * VOX },
                top: base_y + 4.0 * VOX,
                cover: Cover::NONE,
            });
            // boxes i = 3, 4 of the mesh: centres at 6.65 and 8.0 voxels.
            let (r0, r1) = (6.65 * VOX, 8.0 * VOX);
            let shape = Shape::Segment { x0: x + dx * r0, z0: z + dz * r0, x1: x + dx * r1, z1: z + dz * r1, r: 1.5 * VOX };
            n += self.push_step(shape, base_y, 3.0 * VOX) as usize;
            // A stride over each root: not a wall, a small cost.
            self.going.push(crate::going::Rough { shape, mul: 0.82, edge: 0.4, ground: crate::going::Ground::Roots });
        }
        n
    }

    /// The aerial roots of a strangler fig at `(x, z)`, where they reach
    /// the ground: six posts round the host trunk, from `props::strangler_fig`
    /// (root k at `k * 60° + variant * 0.55`, `2.6 + (1 - t) * 5.6 * s`
    /// voxels out at height fraction t, two voxels wide in x). Between them
    /// there is room for a cat and not much more: the cage is somewhere to
    /// worm into, not a wall and not air. `quarter` is the placement's turn
    /// (six roots do not map onto themselves under 90°). Returns the posts.
    pub fn push_fig_roots(&mut self, x: f32, z: f32, base_y: f32, variant: i32, s: f32, quarter: u8) -> usize {
        const VOX: f32 = 0.6;
        // the posts at cat height: the root between y = 0 and 3 voxels
        let r = (2.6 + (1.0 - 1.5 / 17.0) * 5.6 * s) * VOX;
        for k in 0..6 {
            let a = k as f32 / 6.0 * std::f32::consts::TAU + variant as f32 * 0.55;
            let (mut px, mut pz) = (a.cos() * r + 0.5 * VOX, a.sin() * r);
            for _ in 0..(quarter % 4) {
                (px, pz) = (pz, -px);
            }
            self.push(Blocker { shape: Shape::Circle { x: x + px, z: z + pz, r: 0.75 }, top: base_y + 9.6, cover: Cover::NONE });
        }
        6
    }

    /// The two bark poles a vine curtain hangs from (`props::vine_curtain`:
    /// two voxels square at x = +-4.4 u, 0.55 u/voxel, 13 u tall). The
    /// curtain between them is cover you slip through; the poles are wood.
    pub fn push_curtain_poles(&mut self, x: f32, z: f32, base_y: f32, quarter: u8) -> usize {
        for side in [-1.0f32, 1.0] {
            let (mut px, mut pz) = (side * 4.4, 0.0f32);
            for _ in 0..(quarter % 4) {
                (px, pz) = (pz, -px);
            }
            self.push(Blocker { shape: Shape::Circle { x: x + px, z: z + pz, r: 0.62 }, top: base_y + 13.0, cover: Cover::NONE });
        }
        2
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
        *self.perches.last_mut().expect("just pushed") = kind.perch();
        true
    }

    /// The spine(s) of a long prop in its mesh's own frame, as
    /// `(x0, z0, x1, z1)`, before the placement's quarter turns.
    ///
    /// From `props::fallen_log` (an ellipsoid 11*s voxels either way along
    /// x) and `props::ruin_wall` (an L: one arm along x at the grid's z
    /// edge, one along z at its x edge). `long_props_cover_their_meshes`
    /// holds these against the real voxels.
    pub fn long_spines(kind: PropKind, s: f32) -> Vec<(f32, f32, f32, f32)> {
        let r = kind.footprint().radius;
        match kind {
            PropKind::FallenLog => vec![(-6.6 * s + r, 0.0, 6.6 * s - r, 0.0)],
            PropKind::RuinWall => vec![(-7.2 + r, -5.1, 4.8 - r, -5.1), (-6.3, -4.2 + r, -6.3, 3.6 - r)],
            _ => Vec::new(),
        }
    }

    /// Place a prop. Long props become capsules along their real spines;
    /// everything else is `push_prop`.
    ///
    /// This is what the scene's placement macros call. They used to call
    /// `push_prop` for everything, so `push_segment` had no callers and every
    /// wall and log in the arena collided as one small disc at its midpoint.
    pub fn push_placed(&mut self, kind: PropKind, x: f32, z: f32, ground_y: f32, quarter: u8, s: f32) -> bool {
        let spines = Self::long_spines(kind, s);
        if spines.is_empty() {
            return self.push_prop(kind, x, z, ground_y, 1.0);
        }
        let turn = |mut px: f32, mut pz: f32| {
            // the same quarter turn `voxel::merge_xform` gives the mesh
            for _ in 0..(quarter % 4) {
                (px, pz) = (pz, -px);
            }
            (x + px, z + pz)
        };
        for (x0, z0, x1, z1) in spines {
            let (a, b) = (turn(x0, z0), turn(x1, z1));
            self.push_segment(kind, a.0, a.1, b.0, b.1, ground_y, 1.0);
        }
        true
    }

    /// The nearest LIGHT stem within `within` of `(x, z)`: a prop that hides
    /// and does not block -- fern, bush, hanging vine. These are what a
    /// stalking cat goes round and a running one goes through
    /// (`puppet::Probe::stem`). Concealment is flat across a cover disc, so
    /// the stem cannot be recovered from `concealment_at`; it has to be asked
    /// for.
    pub fn nearest_stem(&self, x: f32, z: f32, within: f32) -> Option<(f32, f32)> {
        let mut scratch = Vec::new();
        self.near(x, z, within, &mut scratch);
        scratch
            .iter()
            .map(|&i| &self.all[i as usize])
            .filter(|b| b.top == f32::NEG_INFINITY && b.cover.amount > 0.0)
            .map(|b| (b.cover.x, b.cover.z, (b.cover.x - x).hypot(b.cover.z - z)))
            .filter(|s| s.2 < within)
            .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
            .map(|s| (s.0, s.1))
    }

    /// Every blocker with whether it can be stood on.
    pub fn with_perch(&self) -> impl Iterator<Item = (&Blocker, Perch)> {
        self.all.iter().zip(self.perches.iter().copied())
    }

    /// What is a cat at `(x, z)` with its paws at height `y` standing on?
    /// `None` when there is nothing under it at that height.
    pub fn standing_on(&self, x: f32, z: f32, y: f32) -> Option<Perch> {
        self.with_perch()
            .filter(|(b, p)| *p != Perch::None && (b.top - y).abs() < 0.08)
            .find(|(b, _)| {
                let (ax, az, r) = b.shape.nearest_axis(x, z);
                (x - ax).hypot(z - az) < r + 0.15
            })
            .map(|(_, p)| p)
    }

    /// The highest standable top under `(x, z)` that is at or below `y`.
    /// What a FALLING cat lands on: the rock under the wall, not the floor
    /// under the rock.
    pub fn landing_under(&self, x: f32, z: f32, y: f32) -> Option<(f32, Perch)> {
        self.with_perch()
            .filter(|(b, p)| *p != Perch::None && b.top <= y + 1e-3)
            .filter(|(b, _)| {
                let (ax, az, r) = b.shape.nearest_axis(x, z);
                (x - ax).hypot(z - az) < r
            })
            .map(|(b, p)| (b.top, p))
            .fold(None, |best: Option<(f32, Perch)>, c| if best.map_or(true, |b| c.0 > b.0) { Some(c) } else { best })
    }

    /// Candidate blocker indices near `(x, z)` within `pad`.
    fn near(&self, x: f32, z: f32, pad: f32, out: &mut Vec<u32>) {
        // An empty grid has no cells to look in. Guarded HERE, at the one door
        // every query comes through, rather than at each place it used to crash.
        if self.cells.is_empty() {
            out.clear();
            return;
        }
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
        let p = self.resolve_props(pos, radius, feet_y);
        // The cliffs last, so nothing a prop does can push a unit into the
        // rock. Whatever the feet: no dash clears a cliff.
        match self.floor {
            Some(f) if pos.is_finite() => {
                let (x, z) = f.push_in(p.x, p.z, radius);
                Vec3::new(x, p.y, z)
            }
            _ => p,
        }
    }

    /// As [`Self::resolve_at`] without the floor: props still push, the
    /// floor's edge does not. For a cat off the floor ON PURPOSE -- riding a
    /// sky-map cloud across a chasm, or in the air over one -- whom the
    /// floor would otherwise drag back to the nearest island.
    pub fn resolve_free(&self, pos: Vec3, radius: f32, feet_y: f32) -> Vec3 {
        self.resolve_props(pos, radius, feet_y)
    }

    fn resolve_props(&self, pos: Vec3, radius: f32, feet_y: f32) -> Vec3 {
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
            let b = &self.all[i as usize];
            // A solid thing hides whatever is inside its footprint, whatever
            // its SHAPE. Only the cover disc used to be consulted, and
            // `push_segment` leaves that empty on the stated theory that a
            // wall's own footprint does the hiding -- which nothing here ever
            // checked. So when walls and logs became capsules (3e4574e4e)
            // they also became transparent. The mid-jungle's screen test is
            // what noticed.
            if b.top > f32::NEG_INFINITY {
                let (ax, az, r) = b.shape.nearest_axis(x, z);
                if (x - ax).hypot(z - az) < r {
                    return 1.0;
                }
            }
            let c = b.cover;
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
        // A fixed fine step. This was `cell * 0.5` -- 3 u on the scene's 6 u
        // grid -- which is wider than most props: a ray straight across a
        // 1.8 u wall could land a sample either side of it and report a clear
        // view. The grid pitch is about finding candidates fast, and has
        // nothing to do with how finely a line must be walked.
        let step = 0.4;
        let n = ((len / step).ceil() as usize).clamp(1, 1024);
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
        if self.floor.is_some_and(|f| !f.fits(x, z, radius)) {
            return true;
        }
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

    /// Held against the REAL voxels, turned the way the scene turns them:
    /// nearly every column of the mesh is inside a capsule, and the capsules
    /// do not sprawl far past the mesh.
    #[test]
    fn long_props_cover_their_meshes() {
        let mut checked = 0;
        for (kind, mesh) in [(PropKind::FallenLog, crate::props::fallen_log(1.0)), (PropKind::RuinWall, crate::props::ruin_wall(1.0, 1))] {
            for quarter in 0..4u8 {
                let mut b = Blockers::new(100.0, 6.0);
                assert!(b.push_placed(kind, 20.0, -30.0, 0.0, quarter, 1.0));
                let (mut inside, mut total) = (0, 0);
                let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
                for v in &mesh.inst {
                    let (mut px, mut pz) = (v.off_ao[0], v.off_ao[2]);
                    for _ in 0..quarter {
                        (px, pz) = (pz, -px);
                    }
                    let (wx, wz) = (20.0 + px, -30.0 + pz);
                    lo = [lo[0].min(wx), lo[1].min(wz)];
                    hi = [hi[0].max(wx), hi[1].max(wz)];
                    total += 1;
                    inside += b.blocked(wx, wz, 0.0) as usize;
                }
                assert!(total > 200, "{kind:?}: mesh has {total} voxels");
                let cover = inside as f32 / total as f32;
                assert!(cover > 0.85, "{kind:?} q{quarter}: capsules cover {cover:.2} of the mesh");
                for (dx, dz) in [(-2.5, 0.0), (2.5, 0.0), (0.0, -2.5), (0.0, 2.5)] {
                    let (x, z) = (if dx < 0.0 { lo[0] } else { hi[0] } + dx, if dz < 0.0 { lo[1] } else { hi[1] } + dz);
                    let (x, z) = (if dx == 0.0 { (lo[0] + hi[0]) * 0.5 } else { x }, if dz == 0.0 { (lo[1] + hi[1]) * 0.5 } else { z });
                    assert!(!b.blocked(x, z, 0.0), "{kind:?} q{quarter}: blocks ({x},{z}), 2.5 u outside its mesh");
                }
                checked += 1;
            }
        }
        assert_eq!(checked, 8);
    }

    /// A wall hides what is behind it from EVERY angle, including square-on,
    /// where the old 3 u sampling step walked straight over it.
    #[test]
    fn a_thin_wall_blocks_sight_wherever_the_ray_crosses_it() {
        let mut b = Blockers::new(100.0, 6.0);
        b.push_segment(PropKind::RuinWall, 8.0, -20.0, 8.0, 20.0, 0.0, 1.0);
        let mut crossed = 0;
        for k in 0..40 {
            let z = -18.0 + k as f32 * 0.9;
            let start = (0.13 * k as f32, z); // vary the phase of the samples against the wall
            assert_eq!(b.sight_obstruction(start, (14.0, z + 1.0)), 1.0, "ray {k} saw through the wall");
            crossed += 1;
        }
        assert_eq!(crossed, 40);
        assert_eq!(b.sight_obstruction((0.0, 30.0), (14.0, 30.0)), 0.0, "and past its end the view is open");
    }

    /// An empty world must answer every question with "nothing", not a panic.
    #[test]
    fn a_default_set_answers_nothing_instead_of_panicking() {
        let b = Blockers::default();
        assert!(!b.blocked(3.0, -7.0, 0.6));
        assert_eq!(b.concealment_at(1.0, 1.0), 0.0);
        assert_eq!(b.nearest_stem(0.0, 0.0, 2.0), None);
        assert_eq!(b.sight_obstruction((0.0, 0.0), (30.0, 5.0)), 0.0);
        let p = Vec3::new(4.0, 0.0, 4.0);
        assert_eq!(b.resolve(p, 0.6), p);
        assert_eq!(b.standing_on(0.0, 0.0, 1.0), None);
    }

    /// Stems are the light things only: a rock beside the path is the sim's
    /// business (it already pushed the cat out), a fern is the body's.
    #[test]
    fn only_things_a_cat_could_push_through_count_as_stems() {
        let mut b = Blockers::new(100.0, 6.0);
        b.push_prop(PropKind::Rock, 1.0, 0.0, 0.0, 1.0);
        b.push_prop(PropKind::Fern, 0.0, 0.8, 0.0, 1.0);
        b.push_prop(PropKind::Bush, 0.0, -3.0, 0.0, 1.0);
        assert_eq!(b.nearest_stem(0.0, 0.0, 1.2), Some((0.0, 0.8)), "the fern, not the nearer rock");
        assert_eq!(b.nearest_stem(0.0, -2.6, 1.2), Some((0.0, -3.0)));
        assert_eq!(b.nearest_stem(0.0, 0.0, 0.5), None, "nothing within reach");
        assert_eq!(b.nearest_stem(40.0, 40.0, 1.2), None);
    }

    /// Standable is about the prop. A pine blocks to 3 u and is not a
    /// platform; a pillar is 4 u and is.
    #[test]
    fn standing_is_a_property_of_the_prop_not_its_height() {
        let mut b = Blockers::new(100.0, 6.0);
        b.push_prop(PropKind::Pine, 0.0, 0.0, 0.0, 1.0);
        b.push_prop(PropKind::RuinPillar, 10.0, 0.0, 0.0, 1.0);
        b.push_prop(PropKind::Rock, 20.0, 0.0, 0.0, 1.0);
        b.push(Blocker { shape: Shape::Circle { x: 30.0, z: 0.0, r: 2.0 }, top: 1.0, cover: Cover::NONE });
        assert_eq!(b.standing_on(0.0, 0.0, 3.0), None, "nobody stands on a pine");
        assert_eq!(b.standing_on(10.0, 0.0, 4.0), Some(Perch::Narrow));
        assert_eq!(b.standing_on(20.0, 0.0, 1.4), Some(Perch::Flat));
        assert_eq!(b.standing_on(20.0, 0.0, 0.0), None, "beside the rock's top, not on it");
        assert_eq!(b.standing_on(30.0, 0.0, 1.0), None, "a hand-built blocker is unstandable unless it says otherwise");
        assert_eq!(b.landing_under(20.0, 0.0, 9.0), Some((1.4, Perch::Flat)), "a falling cat lands on the rock");
        assert_eq!(b.landing_under(20.0, 0.0, 1.0), None, "not if it is already below the top");
    }

    /// The outer roots lift paws and move nobody; the inner, tall ones are
    /// walls. The first half is the feature, the second the promise that a
    /// cat on the low root is not shoved off it by the tall one beside it.
    #[test]
    fn low_roots_are_climbed_over_and_tall_ones_are_walls() {
        let mut b = Blockers::new(100.0, 6.0);
        assert_eq!(b.push_jungle_roots(10.0, 10.0, 0.0, 0), 4);
        assert_eq!(b.len(), 4, "one wall per root: its tall inner run");
        // the inner run of root 0, halfway along: a wall at cat height,
        // and one a leap clears
        let (ix, iz) = (10.0 + 0.4f32.cos() * 2.3, 10.0 + 0.4f32.sin() * 2.3);
        assert!(b.blocked(ix, iz, 0.6), "the tall root is solid");
        assert!(!b.blocked(10.0 + 0.4f32.cos() * 2.3 - 0.4f32.sin() * 1.6, 10.0 + 0.4f32.sin() * 2.3 + 0.4f32.cos() * 1.6, 0.6), "beside it is open");
        assert!(b.all().iter().all(|r| (2.3..=2.41).contains(&r.top)), "above a scramble, and a bound clears it");
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

    /// A fig's aerial roots are posts with a cat's width between them, and
    /// they turn with the tree; a curtain's poles are wood either side of
    /// the vines.
    #[test]
    fn fig_roots_are_a_cage_to_worm_into_and_curtain_poles_are_wood() {
        use std::f32::consts::TAU;
        for q in 0..4u8 {
            let mut b = Blockers::new(100.0, 6.0);
            assert_eq!(b.push_fig_roots(0.0, 0.0, 0.0, 0, 1.0, q), 6);
            let posts: Vec<(f32, f32)> = b.all().iter().map(|p| match p.shape { Shape::Circle { x, z, .. } => (x, z), _ => panic!("a post") }).collect();
            for &(x, z) in &posts {
                let d = x.hypot(z);
                assert!((4.3..5.2).contains(&d), "q{q}: a post {d:.2} u out (the mesh's roots reach the ground ~4.6-4.9 u out)");
                assert!(b.blocked(x, z, 0.6));
            }
            // between two neighbouring posts, at the posts' ring: room for a cat
            let a = posts[0].1.atan2(posts[0].0) + TAU / 12.0;
            let r = posts[0].0.hypot(posts[0].1);
            assert!(!b.blocked(a.cos() * r, a.sin() * r, 0.55), "q{q}: no gap to worm through");
            // inside the cage, beside the trunk: open (the trunk is its own blocker)
            assert!(!b.blocked(a.cos() * 2.9, a.sin() * 2.9, 0.55), "q{q}: the cage is not solid");
        }
        // the posts turn with the tree: quarter 1 is quarter 0 turned (x, z) -> (z, -x)
        let mut b0 = Blockers::new(100.0, 6.0);
        let mut b1 = Blockers::new(100.0, 6.0);
        b0.push_fig_roots(0.0, 0.0, 0.0, 3, 1.1, 0);
        b1.push_fig_roots(0.0, 0.0, 0.0, 3, 1.1, 1);
        for (p0, p1) in b0.all().iter().zip(b1.all()) {
            if let (Shape::Circle { x: x0, z: z0, .. }, Shape::Circle { x: x1, z: z1, .. }) = (p0.shape, p1.shape) {
                assert!((x1 - z0).abs() < 1e-4 && (z1 + x0).abs() < 1e-4);
            }
        }
        let mut c = Blockers::new(100.0, 6.0);
        assert_eq!(c.push_curtain_poles(0.0, 0.0, 0.0, 0), 2);
        assert!(c.blocked(4.4, 0.0, 0.5) && c.blocked(-4.4, 0.0, 0.5), "the poles");
        assert!(!c.blocked(0.0, 0.0, 0.5), "the vines between them are not");
        let mut c1 = Blockers::new(100.0, 6.0);
        c1.push_curtain_poles(0.0, 0.0, 0.0, 1);
        assert!(c1.blocked(0.0, -4.4, 0.5) && !c1.blocked(4.4, 0.0, 0.5), "turned with the curtain");
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
