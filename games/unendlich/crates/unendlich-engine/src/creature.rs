//! Multi-AABB physics bodies for big creatures with complex shapes.
//!
//! `PhysicsBody` (in `physics.rs`) is a single-AABB Copy struct used
//! for the player. That's perfect for player physics (cheap, simple),
//! but inadequate for a giraffe, dragon, or wagon — those have body
//! parts that are spatially separated and need independent collision.
//!
//! `Creature` is the multi-AABB analog: a primary body position +
//! orientation (yaw), plus N attached `Limb` AABBs each with their
//! own offset relative to the body.
//!
//! ## Design
//!
//! - **Primary body** has position, velocity, mass, on_ground state.
//!   Behaves like `PhysicsBody` for the body's anchor point.
//! - **Limbs** are AABBs attached to the body via a relative offset
//!   that's rotated by the body's yaw. The limb's world AABB is
//!   `(body_pos + rotate_yaw(limb.offset)) ± limb.half_extents`.
//! - **Collision** is checked against ALL AABBs (body + limbs).
//!   Movement is blocked if ANY of them overlaps a solid block.
//! - **Step height** auto-walk applies to the primary body — limbs
//!   follow.
//!
//! ## Limitations / future
//!
//! - Limbs don't ROTATE relative to the body — they're offset only.
//!   A real giraffe's neck would bend at multiple joints. This would
//!   need an articulation/skeleton system (future).
//! - No per-limb mass or inertia. Whole body has one mass. No
//!   articulated dynamics.
//! - No IK ground contact for limbs (legs don't auto-bend to terrain).
//! - Animation system (limb position cycling for walk gait) is a
//!   separate concern, not handled here.
//!
//! ## What this DOES enable
//!
//! - Collision against multi-cell creatures (a 2-block-tall, 3-block-
//!   long creature can't fit through a 1-block tunnel).
//! - Per-limb spatial queries (which limb is touching what).
//! - Distinct visual representation per limb (mesh attached to each
//!   limb's AABB). When entity rendering lands.
//!
//! ## Why not refactor PhysicsBody to support this directly
//!
//! PhysicsBody is `Copy`, used in many places (snapshot, Arc-based
//! state, etc.) and changing it would ripple through the codebase.
//! Creature is a separate type for now; if creatures become common
//! enough that the duplication hurts, refactor later.

use crate::physics::PhysicsBody;

/// An anatomical part. A creature's [`Capabilities`] are derived procedurally
/// from the parts it has, so behavior follows body plan rather than a hardcoded
/// per-species table: add jaws and it can eat; add sticky feet and it can climb.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    /// Jaws / a mouth → can eat.
    Jaw,
    /// Plain feet/legs → can walk on the ground.
    Foot,
    /// Adhesive (bug/gecko) feet → can walk AND climb sheer surfaces.
    StickyFoot,
    /// A hand with an opposable thumb → can grasp/hold/carry.
    Hand,
    /// A wing → can fly.
    Wing,
    /// An eye → can see (drives perception/vision availability).
    Eye,
    /// An ear → can hear (drives hearing range/acuity; a sentinel's other sense).
    Ear,
    /// A fin → can swim efficiently.
    Fin,
    /// A tail → balance/expression; grants no hard capability yet (reserved
    /// for future gait/turn-rate effects).
    Tail,
}

/// What a creature can do, derived from its [`Part`] set. A simple boolean set
/// rather than bitflags so call sites read naturally (`caps.climb`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub eat: bool,
    pub walk: bool,
    pub climb: bool,
    pub grasp: bool,
    pub fly: bool,
    pub see: bool,
    pub swim: bool,
    pub hear: bool,
}

impl Capabilities {
    /// Union — a creature has a capability if ANY of its parts grants it.
    pub fn or(self, o: Capabilities) -> Capabilities {
        Capabilities {
            eat: self.eat || o.eat,
            walk: self.walk || o.walk,
            climb: self.climb || o.climb,
            grasp: self.grasp || o.grasp,
            fly: self.fly || o.fly,
            see: self.see || o.see,
            swim: self.swim || o.swim,
            hear: self.hear || o.hear,
        }
    }
}

impl Part {
    /// The capabilities this single part grants.
    pub fn grants(self) -> Capabilities {
        let mut c = Capabilities::default();
        match self {
            Part::Jaw => c.eat = true,
            Part::Foot => c.walk = true,
            Part::StickyFoot => {
                c.walk = true;
                c.climb = true;
            }
            Part::Hand => c.grasp = true,
            Part::Wing => c.fly = true,
            Part::Eye => c.see = true,
            Part::Ear => c.hear = true,
            Part::Fin => c.swim = true,
            Part::Tail => {}
        }
        c
    }
}

/// Flight needs balanced lift: the WEAKEST wing must be at least this fraction
/// of its max health, or the creature is grounded (a torn wing = no flight).
pub const FLY_MIN_WING_FRACTION: f32 = 0.5;

/// A single anatomical part as a *damageable hitbox*. Where [`Part`] is a binary
/// "do I have jaws?", a `PartState` carries health and a body-local AABB, so a
/// strike that lands in this box damages exactly this part — and the part's
/// remaining health scales the capability it contributes (see
/// [`Anatomy::graded`]). This is what makes a dragon limp on a hurt leg, go
/// blind as its eyes are destroyed, or lose flight when one wing is shredded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartState {
    pub part: Part,
    pub hp: u16,
    pub max_hp: u16,
    /// Body-local AABB center offset (X forward, Y up, Z right).
    pub offset: [f32; 3],
    /// Body-local AABB half-extents.
    pub half_extents: [f32; 3],
}

impl PartState {
    pub fn new(part: Part, max_hp: u16, offset: [f32; 3], half_extents: [f32; 3]) -> Self {
        Self { part, hp: max_hp, max_hp, offset, half_extents }
    }
    /// Health in `[0, 1]`. A destroyed (hp 0) or zero-max part is `0.0`.
    pub fn fraction(self) -> f32 {
        if self.max_hp == 0 { 0.0 } else { self.hp as f32 / self.max_hp as f32 }
    }
    pub fn is_destroyed(self) -> bool { self.hp == 0 }
    /// True if `local_point` (body-local coords) is inside this part's AABB.
    pub fn contains_local(self, p: [f32; 3]) -> bool {
        (0..3).all(|i| (p[i] - self.offset[i]).abs() <= self.half_extents[i])
    }
}

/// Graded capabilities: capabilities scaled by the health of the parts that
/// grant them, rather than the on/off booleans of [`Capabilities`]. Movement,
/// pathing, perception, and attack code reads these so damage has continuous
/// consequences (a limp, dimmed sight, a weak bite) instead of a cliff.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradedCapabilities {
    /// Eye acuity in `[0,1]` (overall eye health). 0 = blind. Multiply your
    /// sight range / foveation budget by this to plug into perception.
    pub vision: f32,
    /// Hearing acuity in `[0,1]` (overall ear health). 0 = deaf. Scale a
    /// sentinel's hearing range / threshold by this so a torn ear dulls it.
    pub hearing: f32,
    /// Ground-move speed multiplier in `[0,1]` (average foot health). 0 = the
    /// creature can't walk (no functioning feet); <1 = a limp.
    pub move_speed: f32,
    /// Whether the creature can currently fly (has wings AND the weakest is
    /// above [`FLY_MIN_WING_FRACTION`]).
    pub can_fly: bool,
    /// Lift in `[0,1]` when `can_fly` (the weakest wing's fraction); 0 otherwise.
    pub fly_power: f32,
    /// Climb effectiveness in `[0,1]` (sticky-foot health).
    pub climb: f32,
    /// Swim effectiveness in `[0,1]` (fin health).
    pub swim: f32,
    /// Bite/eat strength in `[0,1]` (jaw health). 0 = can't bite or eat.
    pub bite: f32,
    /// Grasp strength in `[0,1]` (hand health).
    pub grasp: f32,
}

/// A damageable body plan: parts with health + hitboxes. Supersedes the binary
/// [`Creature::capabilities`] when continuous, damage-driven behavior is wanted.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Anatomy {
    pub parts: Vec<PartState>,
}

impl Anatomy {
    pub fn new() -> Self { Self { parts: Vec::new() } }

    pub fn with(mut self, p: PartState) -> Self { self.parts.push(p); self }

    /// Mean health fraction across all parts of `kind`. `None` if the creature
    /// has no such part at all (anatomically absent, distinct from destroyed).
    fn mean_fraction(&self, kind: Part) -> Option<f32> {
        let mut sum = 0.0;
        let mut n = 0;
        for ps in self.parts.iter().filter(|p| p.part == kind) {
            sum += ps.fraction();
            n += 1;
        }
        if n == 0 { None } else { Some(sum / n as f32) }
    }

    /// Minimum health fraction across all parts of `kind`. `None` if absent.
    fn min_fraction(&self, kind: Part) -> Option<f32> {
        self.parts.iter()
            .filter(|p| p.part == kind)
            .map(|p| p.fraction())
            .fold(None, |acc: Option<f32>, f| Some(acc.map_or(f, |a| a.min(f))))
    }

    /// Horizontal facing (unit `(x, z)`) of each living eye, inferred from where
    /// the eye sits on the body: an eye offset toward the front (`+X`) looks
    /// forward, eyes offset to the sides look sideways. This is what lets the
    /// vision API work for creatures whose eyes aren't in front — run
    /// `vision::can_see` once per eye with that eye's facing, instead of assuming
    /// a single forward cone. Eyes with no horizontal offset (top of head) are
    /// skipped (they'd be looking straight up/down).
    pub fn eye_facings(&self) -> Vec<(f32, f32)> {
        self.parts
            .iter()
            .filter(|p| p.part == Part::Eye && p.hp > 0)
            .filter_map(|p| {
                let (x, z) = (p.offset[0], p.offset[2]);
                let len = (x * x + z * z).sqrt();
                if len < 1e-4 {
                    None
                } else {
                    Some((x / len, z / len))
                }
            })
            .collect()
    }

    /// Fraction of the full horizontal circle the creature can see, in `[0,1]`
    /// (1.0 = panoramic 360°). Each living eye covers a ±`EYE_HALF_FOV_DEG` cone
    /// around its facing; overlapping cones (two forward eyes) give narrow
    /// binocular coverage, opposed cones (side or ringed eyes) give wide or full
    /// coverage. Sampled per degree so circular wraparound is exact. No eyes → 0.
    pub fn vision_coverage(&self) -> f32 {
        const EYE_HALF_FOV_DEG: i32 = 60;
        let facings = self.eye_facings();
        if facings.is_empty() {
            return 0.0;
        }
        let mut covered = [false; 360];
        for (fx, fz) in facings {
            let center = fz.atan2(fx).to_degrees().round() as i32;
            for d in -EYE_HALF_FOV_DEG..=EYE_HALF_FOV_DEG {
                let bucket = (((center + d) % 360) + 360) % 360;
                covered[bucket as usize] = true;
            }
        }
        covered.iter().filter(|c| **c).count() as f32 / 360.0
    }

    /// Derive continuous capabilities from current part health.
    pub fn graded(&self) -> GradedCapabilities {
        // Vision: overall eye health (sum hp / sum max), so losing one of two
        // eyes halves acuity and losing both blinds.
        let (eye_hp, eye_max) = self.parts.iter()
            .filter(|p| p.part == Part::Eye)
            .fold((0u32, 0u32), |(h, m), p| (h + p.hp as u32, m + p.max_hp as u32));
        let vision = if eye_max == 0 { 0.0 } else { eye_hp as f32 / eye_max as f32 };

        // Hearing: overall ear health, same shape as vision.
        let (ear_hp, ear_max) = self.parts.iter()
            .filter(|p| p.part == Part::Ear)
            .fold((0u32, 0u32), |(h, m), p| (h + p.hp as u32, m + p.max_hp as u32));
        let hearing = if ear_max == 0 { 0.0 } else { ear_hp as f32 / ear_max as f32 };

        // Feet (plain or sticky) drive walk speed; mean health = limp factor.
        let foot_fracs: Vec<f32> = self.parts.iter()
            .filter(|p| matches!(p.part, Part::Foot | Part::StickyFoot))
            .map(|p| p.fraction())
            .collect();
        let move_speed = if foot_fracs.is_empty() {
            0.0
        } else {
            foot_fracs.iter().sum::<f32>() / foot_fracs.len() as f32
        };

        // Flight: weakest wing limits lift; below the threshold (or no wings)
        // the creature is grounded.
        let (can_fly, fly_power) = match self.min_fraction(Part::Wing) {
            Some(min) if min >= FLY_MIN_WING_FRACTION => (true, min),
            _ => (false, 0.0),
        };

        GradedCapabilities {
            vision,
            hearing,
            move_speed,
            can_fly,
            fly_power,
            climb: self.mean_fraction(Part::StickyFoot).unwrap_or(0.0),
            swim: self.mean_fraction(Part::Fin).unwrap_or(0.0),
            bite: self.mean_fraction(Part::Jaw).unwrap_or(0.0),
            grasp: self.mean_fraction(Part::Hand).unwrap_or(0.0),
        }
    }

    /// Which part's hitbox contains `local_point` (body-local), nearest-center
    /// first if several overlap. `None` if the strike missed every part.
    pub fn hit_test(&self, local_point: [f32; 3]) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (i, ps) in self.parts.iter().enumerate() {
            if ps.contains_local(local_point) {
                let d2 = (0..3).map(|k| {
                    let d = local_point[k] - ps.offset[k];
                    d * d
                }).sum::<f32>();
                if best.map_or(true, |(_, bd)| d2 < bd) {
                    best = Some((i, d2));
                }
            }
        }
        best.map(|(i, _)| i)
    }

    /// Apply `amount` damage to part `index` (saturating at 0). Returns the
    /// part's remaining health fraction.
    pub fn damage(&mut self, index: usize, amount: u16) -> f32 {
        if let Some(ps) = self.parts.get_mut(index) {
            ps.hp = ps.hp.saturating_sub(amount);
            ps.fraction()
        } else {
            0.0
        }
    }

    /// Resolve a strike at a body-local point: find the part hit and damage it.
    /// Returns the hit part index, or `None` if nothing was struck.
    pub fn apply_hit(&mut self, local_point: [f32; 3], amount: u16) -> Option<usize> {
        let idx = self.hit_test(local_point)?;
        self.damage(idx, amount);
        Some(idx)
    }
}

/// One AABB attached to a creature body. Offset is in BODY-LOCAL
/// coordinates; world position is computed via the body's yaw.
#[derive(Debug, Clone, Copy)]
pub struct Limb {
    /// Identifier for diagnostic/debug purposes. Not load-bearing.
    pub name: &'static str,
    /// Offset from body position to limb center, in BODY-LOCAL coords
    /// (X = forward, Y = up, Z = right).
    pub offset: [f32; 3],
    /// Half-extents of this limb's AABB.
    pub half_extents: [f32; 3],
}

/// Multi-part creature with a primary body and N attached limbs.
#[derive(Debug, Clone)]
pub struct Creature {
    /// Primary AABB and motion state. The body's `position` is the
    /// anchor; limb world positions are derived from it.
    pub body: PhysicsBody,
    /// Yaw in radians. 0 = facing +X. limb offsets rotate around Y
    /// axis by this amount when projected to world space.
    pub yaw: f32,
    /// Walk-cycle phase in radians, advanced by distance travelled. Drives the
    /// leg swing in the renderer so a moving creature steps instead of sliding.
    /// 0 = legs at rest (so a standing creature is unchanged).
    pub gait_phase: f32,
    /// Terrain lean (radians). Pitch = nose up/down on slopes, roll = bank across
    /// side-slopes. 0/0 = upright (so existing creatures are unchanged). Set from
    /// the posture model; applied in the renderer alongside yaw.
    pub pitch: f32,
    pub roll: f32,
    /// Attached AABBs.
    pub limbs: Vec<Limb>,
    /// Body mass — eventually drives wind effects, fall damage, etc.
    pub mass: f32,
    /// The creature's body plan: the set of anatomical parts it has. Drives
    /// capabilities procedurally (see [`Creature::capabilities`]) — "if it has
    /// jaws it can eat, if it has feet it can walk, if it has sticky feet it
    /// can climb." Distinct from `limbs` (collision AABBs); a creature can have
    /// a part without a dedicated collision box and vice-versa.
    pub parts: Vec<Part>,
    /// Optional damageable body plan (parts with health + hitboxes). When
    /// present, [`Creature::graded_capabilities`] reports continuous,
    /// damage-driven capabilities (limp, dimmed vision, lost flight) that
    /// movement / pathing / perception code can read. `None` = the creature
    /// uses only the binary [`Creature::capabilities`].
    pub anatomy: Option<Anatomy>,
    /// Per-individual colour multiplier applied to the body and every part by
    /// the mesher. `[1,1,1]` = no change (so creatures without a set tint look
    /// exactly as before). Genome-driven dragons set this from their element.
    pub tint: [f32; 3],
    /// Lateral torso warp (radians) applied as an extra bank on the torso box —
    /// the body bends into turns and across slopes. `0` = straight (so existing
    /// creatures are unchanged). Driven by [`Creature::settle_posture`].
    pub torso_bend: f32,
    /// Cyclic animation clock in seconds, advanced by the bin each frame
    /// (`anim_time += dt`). Drives the forge dragon's idle motion (wing-beat, jaw,
    /// sway) independent of locomotion. `0` = rest (so existing creatures and a
    /// freshly spawned dragon are unchanged).
    pub anim_time: f32,
    /// Head gaze relative to the body (rad), set by `dragon_observe`. Drives the
    /// forge dragon's head/neck so it visibly looks toward what it's attending to.
    /// `0,0` = look straight ahead / idle-scan (so existing creatures are unchanged).
    pub gaze_yaw: f32,
    pub gaze_pitch: f32,
    /// The canonical voxel dragon model (`dragon_forge`), when this creature is a
    /// genome dragon. When present the mesher renders these cells instead of the
    /// box anatomy. `None` for every other creature (unchanged rendering).
    pub forge: Option<std::sync::Arc<crate::sim::dragon_forge::ForgeModel>>,
    /// Accumulated weathering (dirt on the belly, wetness washing it off), fed into
    /// the forge dragon's body material at mesh time. `clean()` for fresh/other
    /// creatures (unchanged look until something accumulates grime).
    pub grime: crate::sim::creature_grime::GrimeState,
}

impl Creature {
    pub fn new(body: PhysicsBody) -> Self {
        Self { body, yaw: 0.0, gait_phase: 0.0, pitch: 0.0, roll: 0.0, limbs: Vec::new(), mass: 1.0, parts: Vec::new(), anatomy: None, tint: [1.0, 1.0, 1.0], torso_bend: 0.0, anim_time: 0.0, gaze_yaw: 0.0, gaze_pitch: 0.0, forge: None, grime: crate::sim::creature_grime::GrimeState::clean() }
    }

    /// The creature's lowest point relative to its body centre (negative = below
    /// it), spanning the body box and every anatomy part. Rest the feet on the
    /// ground by placing the body at `surface - lowest_point()`, so the legs are
    /// what the creature stands on rather than the body centre sinking to ground.
    pub fn lowest_point(&self) -> f32 {
        let mut lo = -self.body.half_extents[1];
        if let Some(anat) = &self.anatomy {
            for p in &anat.parts {
                lo = lo.min(p.offset[1] - p.half_extents[1]);
            }
        }
        lo
    }

    /// The world Y for `body.position` that rests this creature's lowest point on
    /// a surface whose **top face** is at `ground_top_y`. Note the convention:
    /// `WorldGenerator::surface_y_at` returns the *top solid block index*, so the
    /// walkable top face is `surface_y + 1` — pass that, not the raw surface_y, or
    /// the creature sinks a block into the ground.
    pub fn grounded_y(&self, ground_top_y: f32) -> f32 {
        ground_top_y - self.lowest_point()
    }

    /// Attach a damageable body plan (builder form). Also mirrors the part
    /// kinds into `parts` so the binary [`Creature::capabilities`] stays
    /// consistent with the anatomy's kinds.
    pub fn with_anatomy(mut self, anatomy: Anatomy) -> Self {
        for ps in &anatomy.parts {
            if !self.parts.contains(&ps.part) {
                self.parts.push(ps.part);
            }
        }
        self.anatomy = Some(anatomy);
        self
    }

    /// Continuous, damage-driven capabilities if this creature has an
    /// [`Anatomy`]; otherwise `None` (fall back to [`Creature::capabilities`]).
    pub fn graded_capabilities(&self) -> Option<GradedCapabilities> {
        self.anatomy.as_ref().map(|a| a.graded())
    }

    pub fn with_yaw(mut self, yaw: f32) -> Self { self.yaw = yaw; self }
    pub fn with_mass(mut self, mass: f32) -> Self { self.mass = mass; self }
    /// Set the per-individual colour multiplier (see [`Creature::tint`]).
    pub fn with_tint(mut self, tint: [f32; 3]) -> Self { self.tint = tint; self }

    /// Attach the canonical voxel dragon model so the mesher renders it instead
    /// of the box anatomy.
    pub fn with_forge(mut self, model: std::sync::Arc<crate::sim::dragon_forge::ForgeModel>) -> Self {
        self.forge = Some(model);
        self
    }

    /// Ease the body's tilt and torso warp toward a target posture (computed
    /// from the stance via [`crate::sim::body_posture::target_from_feet`]),
    /// frame-rate-independently. Updates `pitch`, `roll`, and `torso_bend`.
    pub fn settle_posture(
        &mut self,
        target: crate::sim::body_posture::Posture,
        rate: f32,
        dt: f32,
    ) {
        let cur = crate::sim::body_posture::Posture {
            pitch: self.pitch,
            roll: self.roll,
            torso_bend: self.torso_bend,
        };
        let next = crate::sim::body_posture::settle(cur, target, rate, dt);
        self.pitch = next.pitch;
        self.roll = next.roll;
        self.torso_bend = next.torso_bend;
    }

    /// A four-legged gait solved from the body box: front/rear feet at
    /// ±wheelbase, left/right at ±track, each placed independently on the
    /// ground height under it (cliff stepping) and length-bounded to the leg's
    /// reach. `ground_h` is `[FL, FR, RL, RR]`. Returns the four foot
    /// placements for the renderer to drive a quadruped rig.
    pub fn quadruped_feet(
        &self,
        ground_h: [f32; 4],
        gait_phase01: f32,
        speed01: f32,
        step_len: f32,
    ) -> [crate::sim::quadruped_gait::FootPlacement; 4] {
        use crate::sim::quadruped_gait::{place_foot, Leg};
        let h = self.body.half_extents;
        let wb = h[0] * 1.2; // half-wheelbase (X forward)
        let tr = h[2] * 0.9; // half-track (Z right)
        let down = -h[1];
        let reach = (h[1] * 2.0).max(1.0);
        // FL, FR, RL, RR  (front = +X, right = +Z)
        let rests = [
            [wb, down, -tr],
            [wb, down, tr],
            [-wb, down, -tr],
            [-wb, down, tr],
        ];
        let mut out = [crate::sim::quadruped_gait::FootPlacement {
            pos: [0.0; 3],
            lift: 0.0,
            planted: true,
        }; 4];
        for i in 0..4 {
            let leg = Leg { rest_offset: rests[i], max_reach: reach };
            out[i] = place_foot(
                self.body.position,
                self.yaw,
                i,
                &leg,
                gait_phase01,
                speed01,
                ground_h[i],
                step_len,
            );
        }
        out
    }

    /// The tallest vertical step (in blocks) this creature can negotiate,
    /// derived from its size: roughly its body half-height. A cliff shorter than
    /// this it can step up or down; a taller one blocks it. Bigger dragons
    /// (more `bulk`) clear bigger steps.
    pub fn max_step_height(&self) -> f32 {
        self.body.half_extents[1]
    }

    /// Build a footfall (or body-slam) impact event from this creature's mass.
    /// Feed the result to [`crate::sim::impact_response::respond`] with the
    /// camera position to drive shake / sound / erosion.
    pub fn footfall(
        &self,
        foot_pos: [f32; 3],
        speed: f32,
        footprint: f32,
    ) -> crate::sim::impact_response::Impact {
        crate::sim::impact_response::Impact {
            mass: self.mass,
            speed,
            pos: foot_pos,
            footprint,
        }
    }

    /// Add an anatomical part to the body plan (builder form).
    pub fn with_part(mut self, part: Part) -> Self { self.parts.push(part); self }

    /// Add an anatomical part to the body plan.
    pub fn add_part(&mut self, part: Part) { self.parts.push(part); }

    /// Derive what this creature can do from its parts. Procedural: no
    /// per-species table — capabilities are the union of what each part grants.
    /// A wingless, footed, jawed, eyed creature walks/eats/sees but can't fly;
    /// swap plain feet for sticky feet and it gains climbing.
    pub fn capabilities(&self) -> Capabilities {
        self.parts.iter().fold(Capabilities::default(), |acc, p| acc.or(p.grants()))
    }

    pub fn add_limb(&mut self, limb: Limb) {
        self.limbs.push(limb);
    }

    /// Number of AABBs total (body + limbs).
    pub fn aabb_count(&self) -> usize { 1 + self.limbs.len() }

    /// Iterator over (min, max) world-space AABBs of body and all limbs.
    /// First yields the body, then each limb in registration order.
    pub fn world_aabbs(&self) -> impl Iterator<Item = ([f32; 3], [f32; 3])> + '_ {
        let body_min = self.body.aabb_min();
        let body_max = self.body.aabb_max();
        let body_iter = std::iter::once((body_min, body_max));

        let yaw_cos = self.yaw.cos();
        let yaw_sin = self.yaw.sin();
        let body_pos = self.body.position;

        let limb_iter = self.limbs.iter().map(move |limb| {
            // Rotate limb offset by yaw around Y axis.
            // local: (X=forward, Y=up, Z=right)
            // After yaw rotation:
            //   world_x = local_x * cos(yaw) - local_z * sin(yaw)
            //   world_z = local_x * sin(yaw) + local_z * cos(yaw)
            //   world_y = local_y
            let lx = limb.offset[0];
            let ly = limb.offset[1];
            let lz = limb.offset[2];
            let world_offset = [
                lx * yaw_cos - lz * yaw_sin,
                ly,
                lx * yaw_sin + lz * yaw_cos,
            ];
            let center = [
                body_pos[0] + world_offset[0],
                body_pos[1] + world_offset[1],
                body_pos[2] + world_offset[2],
            ];
            let min = [
                center[0] - limb.half_extents[0],
                center[1] - limb.half_extents[1],
                center[2] - limb.half_extents[2],
            ];
            let max = [
                center[0] + limb.half_extents[0],
                center[1] + limb.half_extents[1],
                center[2] + limb.half_extents[2],
            ];
            (min, max)
        });

        body_iter.chain(limb_iter)
    }

    /// Combined bounding box covering body + all limbs. Useful for
    /// frustum culling, broad-phase collision, network sync radius.
    pub fn combined_aabb(&self) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::INFINITY, f32::INFINITY, f32::INFINITY];
        let mut max = [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
        for (lo, hi) in self.world_aabbs() {
            for i in 0..3 {
                if lo[i] < min[i] { min[i] = lo[i]; }
                if hi[i] > max[i] { max[i] = hi[i]; }
            }
        }
        (min, max)
    }

    /// Test if any of the body's AABBs overlap a solid block in the
    /// world. Returns true on first collision found (short-circuits).
    /// Used by the multi-AABB sweep to determine if a candidate
    /// position is blocked.
    ///
    /// Sampling rule: a block AT integer (bx, by, bz) occupies the
    /// half-open volume [bx, bx+1) × [by, by+1) × [bz, bz+1). An
    /// AABB overlap is detected by checking each integer cell whose
    /// extent intersects the AABB.
    pub fn collides_with<W: crate::physics::BlockSampler>(&self, world: &W) -> bool {
        for (lo, hi) in self.world_aabbs() {
            // Integer cell range covering this AABB.
            let bx0 = lo[0].floor() as i32;
            let by0 = lo[1].floor() as i32;
            let bz0 = lo[2].floor() as i32;
            let bx1 = (hi[0] - 1e-6).floor() as i32;
            let by1 = (hi[1] - 1e-6).floor() as i32;
            let bz1 = (hi[2] - 1e-6).floor() as i32;
            for bx in bx0..=bx1 {
                for by in by0..=by1 {
                    for bz in bz0..=bz1 {
                        // BlockSampler::block_at returns Option<BlockId>.
                        // None = unloaded chunk → treat as solid for safety
                        // (don't let creatures fall through unloaded
                        // territory). True solid = collision.
                        let block = world.block_at(bx, by, bz);
                        let solid = match block {
                            Some(b) => b.blocks_collision(),
                            None => true,
                        };
                        if solid { return true; }
                    }
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_at(pos: [f32; 3]) -> PhysicsBody {
        PhysicsBody::new_player(pos)
    }

    fn eye(offset: [f32; 3]) -> PartState {
        PartState::new(Part::Eye, 20, offset, [0.2, 0.2, 0.2])
    }

    #[test]
    fn grounded_y_rests_the_lowest_point_on_the_surface_top() {
        let body = PhysicsBody::new_player([0.0, 0.0, 0.0]);
        let c = Creature::new(body);
        // Putting the body at grounded_y(g) must place the lowest point exactly at g.
        let ground_top = 92.0;
        let y = c.grounded_y(ground_top);
        assert!(((y + c.lowest_point()) - ground_top).abs() < 1e-4, "lowest point rests on the surface top");
        // And it sits a block above the surface-block index (which is ground_top-1).
        assert!(y > ground_top - 1.0);
    }

    #[test]
    fn forward_eyes_give_narrow_coverage_side_eyes_wide() {
        // Two forward-facing eyes (offset +X, slight ±Z): overlapping cones.
        let front = Anatomy::new().with(eye([1.0, 0.5, -0.3])).with(eye([1.0, 0.5, 0.3]));
        // Two side-facing eyes (offset ±Z): opposed cones, much wider field.
        let sides = Anatomy::new().with(eye([0.0, 0.5, 1.0])).with(eye([0.0, 0.5, -1.0]));
        let fc = front.vision_coverage();
        let sc = sides.vision_coverage();
        assert!(fc < sc, "side eyes see more of the circle ({fc} < {sc})");
        assert!(fc < 0.5, "two forward eyes are a narrow forward field: {fc}");
        // Both forward eyes look roughly +X.
        for (x, _z) in front.eye_facings() {
            assert!(x > 0.0, "forward eye faces +X");
        }
    }

    #[test]
    fn eyes_ringing_the_head_approach_panoramic() {
        let ringed = Anatomy::new()
            .with(eye([1.0, 0.5, 0.0]))
            .with(eye([-1.0, 0.5, 0.0]))
            .with(eye([0.0, 0.5, 1.0]))
            .with(eye([0.0, 0.5, -1.0]));
        assert!(ringed.vision_coverage() > 0.95, "four cardinal eyes ≈ full circle");
        // No eyes → no field; a blinded (hp 0) eye doesn't count.
        assert_eq!(Anatomy::new().vision_coverage(), 0.0);
        let blind = Anatomy::new().with(PartState::new(Part::Eye, 0, [1.0, 0.5, 0.0], [0.2, 0.2, 0.2]));
        assert!(blind.eye_facings().is_empty(), "dead eye doesn't see");
    }

    #[test]
    fn no_parts_means_no_capabilities() {
        let c = Creature::new(body_at([0.0, 0.0, 0.0]));
        assert_eq!(c.capabilities(), Capabilities::default());
    }

    #[test]
    fn jaws_grant_eating() {
        let c = Creature::new(body_at([0.0; 3])).with_part(Part::Jaw);
        assert!(c.capabilities().eat);
        assert!(!c.capabilities().walk);
    }

    #[test]
    fn plain_feet_walk_but_do_not_climb() {
        let c = Creature::new(body_at([0.0; 3])).with_part(Part::Foot);
        let caps = c.capabilities();
        assert!(caps.walk);
        assert!(!caps.climb);
    }

    #[test]
    fn sticky_feet_walk_and_climb() {
        let c = Creature::new(body_at([0.0; 3])).with_part(Part::StickyFoot);
        let caps = c.capabilities();
        assert!(caps.walk, "sticky feet should still walk");
        assert!(caps.climb, "sticky feet should climb");
    }

    #[test]
    fn opposable_thumb_grants_grasp() {
        let c = Creature::new(body_at([0.0; 3])).with_part(Part::Hand);
        assert!(c.capabilities().grasp);
    }

    #[test]
    fn a_dragon_eats_flies_walks_sees_but_cannot_climb_or_grasp() {
        // Body plan: jaws + wings + feet + eyes.
        let dragon = Creature::new(body_at([0.0; 3]))
            .with_part(Part::Jaw)
            .with_part(Part::Wing)
            .with_part(Part::Foot)
            .with_part(Part::Eye);
        let c = dragon.capabilities();
        assert!(c.eat && c.fly && c.walk && c.see);
        assert!(!c.climb && !c.grasp && !c.swim);
    }

    #[test]
    fn a_climbing_bug_walks_climbs_eats_and_sees() {
        let bug = Creature::new(body_at([0.0; 3]))
            .with_part(Part::StickyFoot)
            .with_part(Part::Jaw)
            .with_part(Part::Eye);
        let c = bug.capabilities();
        assert!(c.walk && c.climb && c.eat && c.see);
        assert!(!c.fly && !c.grasp);
    }

    #[test]
    fn creature_with_no_limbs_has_one_aabb() {
        let c = Creature::new(body_at([0.0, 0.0, 0.0]));
        assert_eq!(c.aabb_count(), 1);
        assert_eq!(c.world_aabbs().count(), 1);
    }

    #[test]
    fn creature_aabb_count_includes_limbs() {
        let mut c = Creature::new(body_at([0.0, 0.0, 0.0]));
        c.add_limb(Limb { name: "head", offset: [1.0, 0.5, 0.0], half_extents: [0.3, 0.3, 0.3] });
        c.add_limb(Limb { name: "tail", offset: [-1.0, 0.0, 0.0], half_extents: [0.2, 0.2, 0.2] });
        assert_eq!(c.aabb_count(), 3);
        assert_eq!(c.world_aabbs().count(), 3);
    }

    #[test]
    fn limb_offset_no_yaw_is_simple_translation() {
        let mut c = Creature::new(body_at([10.0, 5.0, 7.0]));
        c.add_limb(Limb { name: "head", offset: [2.0, 1.0, 0.0], half_extents: [0.5, 0.5, 0.5] });
        // Yaw is 0 (facing +X). Head should be at body_pos + offset.
        let aabbs: Vec<_> = c.world_aabbs().collect();
        let (min, max) = aabbs[1];
        assert!((min[0] - (10.0 + 2.0 - 0.5)).abs() < 1e-4, "head min[0] wrong: {}", min[0]);
        assert!((min[1] - (5.0 + 1.0 - 0.5)).abs() < 1e-4);
        assert!((max[0] - (10.0 + 2.0 + 0.5)).abs() < 1e-4);
    }

    #[test]
    fn limb_offset_yaw_90_rotates_to_z_axis() {
        // Body at origin, yaw = π/2 (facing +Z). A head 2 blocks "forward"
        // in body-local should appear 2 blocks +Z in world.
        let mut c = Creature::new(body_at([0.0, 0.0, 0.0]));
        c.yaw = std::f32::consts::FRAC_PI_2;
        c.add_limb(Limb { name: "head", offset: [2.0, 0.0, 0.0], half_extents: [0.1, 0.1, 0.1] });
        let aabbs: Vec<_> = c.world_aabbs().collect();
        let (min, max) = aabbs[1];
        let cx = (min[0] + max[0]) * 0.5;
        let cz = (min[2] + max[2]) * 0.5;
        // Head should now be at world (0, 0, 2).
        assert!(cx.abs() < 1e-4, "head should be at x=0 when yaw=90°, got {}", cx);
        assert!((cz - 2.0).abs() < 1e-4, "head should be at z=2 when yaw=90°, got {}", cz);
    }

    #[test]
    fn combined_aabb_encloses_all_limbs() {
        let mut c = Creature::new(body_at([0.0, 0.0, 0.0]));
        c.add_limb(Limb { name: "head", offset: [3.0, 0.0, 0.0], half_extents: [0.5, 0.5, 0.5] });
        c.add_limb(Limb { name: "tail", offset: [-2.0, 0.0, 0.0], half_extents: [0.3, 0.3, 0.3] });
        let (min, max) = c.combined_aabb();
        // Head at +3 contributes max[0] = 3.5; tail at -2 contributes min[0] = -2.3.
        assert!(max[0] >= 3.5 - 1e-4, "combined max[0]={}", max[0]);
        assert!(min[0] <= -2.3 + 1e-4, "combined min[0]={}", min[0]);
    }

    #[test]
    fn long_creature_blocks_at_limb_obstacles() {
        // A creature with a long head extending 3 blocks forward of the
        // body. Combined AABB should be 3 blocks longer in X than the
        // body alone.
        let body = body_at([0.0, 0.0, 0.0]); // half_extents [0.3, 0.9, 0.3]
        let mut c = Creature::new(body);
        c.add_limb(Limb { name: "head", offset: [3.0, 0.0, 0.0], half_extents: [0.4, 0.4, 0.4] });
        let (min, max) = c.combined_aabb();
        // Body alone extends from -0.3 to +0.3 in X. With head at +3,
        // max[0] should be 3.4. Body min is -0.3.
        assert!(max[0] >= 3.4 - 1e-4);
        assert!(min[0] <= -0.3 + 1e-4);
        // Combined Y range covers body (1.8 tall).
        assert!(max[1] - min[1] >= 1.8 - 1e-4);
    }

    /// Tiny stub world for collision tests: a single solid block at
    /// fixed (x, y, z); everything else is air.
    struct OneBlockWorld { block_pos: [i32; 3] }
    impl crate::physics::BlockSampler for OneBlockWorld {
        fn block_at(&self, x: i32, y: i32, z: i32) -> Option<crate::world::blocks::BlockId> {
            if [x, y, z] == self.block_pos {
                Some(crate::world::blocks::BlockId::Stone)
            } else {
                Some(crate::world::blocks::BlockId::Air)
            }
        }
    }

    #[test]
    fn body_only_collides_when_overlapping_solid() {
        let world = OneBlockWorld { block_pos: [5, 5, 5] };
        // Body AABB at (0, 0, 0) — far from the obstacle.
        let mut c = Creature::new(body_at([0.0, 0.0, 0.0]));
        assert!(!c.collides_with(&world), "body should not collide far from block");

        // Move body so AABB overlaps the block.
        c.body.position = [5.5, 5.5, 5.5];
        assert!(c.collides_with(&world), "body should collide when overlapping block");
    }

    #[test]
    fn limb_can_collide_when_body_clear() {
        // A long-necked creature: body at origin, head 4 blocks ahead.
        // Place a solid block in the head's path but NOT the body's.
        let world = OneBlockWorld { block_pos: [4, 0, 0] };
        let mut c = Creature::new(body_at([0.0, 0.0, 0.0]));
        c.add_limb(Limb {
            name: "head",
            offset: [4.0, 0.0, 0.0],
            half_extents: [0.5, 0.5, 0.5],
        });
        // Body alone wouldn't collide; the limb does.
        assert!(c.collides_with(&world),
            "long-neck creature should collide via limb even when body is clear");
    }

    #[test]
    fn yaw_rotation_redirects_collision_axis() {
        // Block at +X 4 blocks. Creature with head extending forward.
        // At yaw=0 (facing +X) head reaches the block → collides.
        // At yaw=π (facing -X) head points away → no collision.
        let world = OneBlockWorld { block_pos: [4, 0, 0] };

        let mut c = Creature::new(body_at([0.0, 0.0, 0.0]));
        c.add_limb(Limb {
            name: "head",
            offset: [4.0, 0.0, 0.0],
            half_extents: [0.5, 0.5, 0.5],
        });
        c.yaw = 0.0;
        assert!(c.collides_with(&world), "head should hit block when yaw=0");

        c.yaw = std::f32::consts::PI;
        assert!(!c.collides_with(&world),
            "head should not hit block when yaw=π (head points away from block)");
    }

    // ── Anatomy: damageable parts → graded capabilities ─────────────────────

    /// A canonical winged predator: two eyes, two wings, four legs, a jaw.
    fn dragon() -> Anatomy {
        Anatomy::new()
            .with(PartState::new(Part::Eye, 20, [1.0, 0.5, -0.3], [0.2, 0.2, 0.2]))
            .with(PartState::new(Part::Eye, 20, [1.0, 0.5, 0.3], [0.2, 0.2, 0.2]))
            .with(PartState::new(Part::Wing, 60, [0.0, 0.6, -1.5], [1.2, 0.2, 0.6]))
            .with(PartState::new(Part::Wing, 60, [0.0, 0.6, 1.5], [1.2, 0.2, 0.6]))
            .with(PartState::new(Part::Foot, 30, [0.6, -1.0, -0.5], [0.3, 0.5, 0.3]))
            .with(PartState::new(Part::Foot, 30, [0.6, -1.0, 0.5], [0.3, 0.5, 0.3]))
            .with(PartState::new(Part::Foot, 30, [-0.6, -1.0, -0.5], [0.3, 0.5, 0.3]))
            .with(PartState::new(Part::Foot, 30, [-0.6, -1.0, 0.5], [0.3, 0.5, 0.3]))
            .with(PartState::new(Part::Jaw, 25, [1.4, 0.3, 0.0], [0.4, 0.3, 0.3]))
    }

    fn approx(a: f32, b: f32) -> bool { (a - b).abs() < 1e-4 }

    #[test]
    fn full_health_dragon_has_all_capabilities() {
        let g = dragon().graded();
        assert!(g.can_fly, "intact wings → can fly");
        assert!(approx(g.fly_power, 1.0));
        assert!(approx(g.vision, 1.0), "two intact eyes → full vision");
        assert!(approx(g.move_speed, 1.0), "four intact legs → full speed");
        assert!(approx(g.bite, 1.0));
    }

    #[test]
    fn a_torn_wing_grounds_the_dragon_but_it_still_walks_and_sees() {
        let mut a = dragon();
        // Find a wing and shred it below the flight threshold.
        let wing = a.parts.iter().position(|p| p.part == Part::Wing).unwrap();
        a.damage(wing, 40); // 60 → 20 hp = 0.33 fraction, below FLY_MIN (0.5)
        let g = a.graded();
        assert!(!g.can_fly, "a wing below threshold must ground the creature");
        assert_eq!(g.fly_power, 0.0);
        assert!(approx(g.move_speed, 1.0), "legs unaffected → still walks normally");
        assert!(approx(g.vision, 1.0), "eyes unaffected → still sees");
    }

    #[test]
    fn losing_eyes_dims_then_blinds() {
        let mut a = dragon();
        let eye = a.parts.iter().position(|p| p.part == Part::Eye).unwrap();
        a.damage(eye, 20); // destroy one of two eyes
        assert!(approx(a.graded().vision, 0.5), "one of two eyes → half vision");
        // Destroy the other eye too.
        let eye2 = a.parts.iter().enumerate()
            .filter(|(_, p)| p.part == Part::Eye && p.hp > 0)
            .map(|(i, _)| i).next().unwrap();
        a.damage(eye2, 20);
        assert_eq!(a.graded().vision, 0.0, "both eyes destroyed → blind");
    }

    #[test]
    fn ears_grant_hearing_and_damage_dulls_it() {
        // The binary capability comes straight off the part.
        assert!(Part::Ear.grants().hear, "an ear grants hearing");
        assert!(!Part::Eye.grants().hear, "an eye does not");

        let ear = |o: [f32; 3]| PartState::new(Part::Ear, 10, o, [0.15, 0.15, 0.15]);
        // No ears → deaf.
        let deaf = Anatomy::new().with(eye([1.0, 0.5, 0.0]));
        assert_eq!(deaf.graded().hearing, 0.0);

        // Two intact ears → full hearing.
        let mut a = Anatomy::new().with(ear([0.0, 0.6, 0.7])).with(ear([0.0, 0.6, -0.7]));
        assert!(approx(a.graded().hearing, 1.0));

        // Destroy one ear → half acuity.
        let one = a.parts.iter().position(|p| p.part == Part::Ear).unwrap();
        a.damage(one, 10);
        assert!(approx(a.graded().hearing, 0.5), "one of two ears → half hearing");
    }

    #[test]
    fn a_hurt_leg_makes_it_limp_losing_all_legs_stops_it() {
        let mut a = dragon();
        let leg = a.parts.iter().position(|p| p.part == Part::Foot).unwrap();
        a.damage(leg, 15); // one leg to 50%
        let g = a.graded();
        assert!(g.move_speed < 1.0 && g.move_speed > 0.0, "one hurt leg → a limp: {}", g.move_speed);
        // 3 legs at 1.0 + 1 leg at 0.5 → mean 0.875.
        assert!(approx(g.move_speed, 0.875));
        // Destroy every leg.
        for i in 0..a.parts.len() {
            if a.parts[i].part == Part::Foot { a.parts[i].hp = 0; }
        }
        assert_eq!(a.graded().move_speed, 0.0, "no functioning legs → cannot walk");
    }

    #[test]
    fn a_destroyed_jaw_cannot_bite() {
        let mut a = dragon();
        let jaw = a.parts.iter().position(|p| p.part == Part::Jaw).unwrap();
        a.damage(jaw, 999); // saturating to 0
        assert_eq!(a.graded().bite, 0.0, "no jaw → no bite/eat");
    }

    #[test]
    fn hit_test_routes_a_strike_to_the_correct_part() {
        let mut a = dragon();
        // A point out on the +Z wing (offset [0,0.6,1.5]).
        let strike = [0.0, 0.6, 1.6];
        let idx = a.hit_test(strike).expect("should hit the +Z wing");
        assert_eq!(a.parts[idx].part, Part::Wing);
        assert!(a.parts[idx].offset[2] > 0.0, "should be the +Z wing specifically");
        // Striking it enough grounds the dragon.
        a.damage(idx, 40);
        assert!(!a.graded().can_fly, "shredding the struck wing should ground it");
        // A strike into empty air hits nothing.
        assert!(a.hit_test([100.0, 100.0, 100.0]).is_none());
    }

    #[test]
    fn absent_part_differs_from_destroyed_for_flight() {
        // A wingless creature can't fly because it has no wings (absent)...
        let wingless = Anatomy::new()
            .with(PartState::new(Part::Foot, 10, [0.0, -1.0, 0.0], [0.3, 0.5, 0.3]))
            .with(PartState::new(Part::Eye, 10, [0.5, 0.3, 0.0], [0.2, 0.2, 0.2]));
        assert!(!wingless.graded().can_fly);
        assert!(approx(wingless.graded().move_speed, 1.0), "intact leg → walks fine");
        // ...which reads the same as a winged creature whose wings are destroyed,
        // but the body plan is genuinely different (no Wing parts present).
        assert!(!wingless.parts.iter().any(|p| p.part == Part::Wing));
    }

    #[test]
    fn creature_exposes_graded_caps_only_with_anatomy() {
        let plain = Creature::new(body_at([0.0, 0.0, 0.0]));
        assert!(plain.graded_capabilities().is_none(), "no anatomy → no graded caps");
        let winged = Creature::new(body_at([0.0, 0.0, 0.0])).with_anatomy(dragon());
        assert!(winged.graded_capabilities().unwrap().can_fly);
        // with_anatomy mirrors kinds into `parts` so the binary view agrees.
        assert!(winged.capabilities().fly, "binary caps should also see the wing");
    }
}

#[cfg(test)]
mod motion_tests {
    use super::*;
    use crate::physics::PhysicsBody;

    fn body() -> Creature {
        let mut c = Creature::new(PhysicsBody::new_player([0.0, 10.0, 0.0]));
        c.body.half_extents = [2.0, 1.5, 4.0];
        c.mass = 40.0;
        c
    }

    #[test]
    fn default_torso_bend_is_zero() {
        assert_eq!(Creature::new(PhysicsBody::new_player([0.0; 3])).torso_bend, 0.0);
    }

    #[test]
    fn settle_posture_eases_tilt() {
        let mut c = body();
        let target = crate::sim::body_posture::Posture { pitch: 0.4, roll: -0.2, torso_bend: 0.1 };
        for _ in 0..200 {
            c.settle_posture(target, 8.0, 0.05);
        }
        assert!((c.pitch - 0.4).abs() < 1e-2 && (c.roll + 0.2).abs() < 1e-2);
        assert!((c.torso_bend - 0.1).abs() < 1e-2);
    }

    #[test]
    fn quadruped_gives_four_feet_with_cliff_stepping() {
        let c = body();
        // Right side on higher ground than the left.
        let feet = c.quadruped_feet([4.0, 6.0, 4.0, 6.0], 0.0, 0.0, 2.0);
        assert_eq!(feet.len(), 4);
        // Standing → all planted.
        assert!(feet.iter().all(|f| f.planted));
        // FR (index 1, right) plants higher than FL (index 0, left).
        assert!(feet[1].pos[1] > feet[0].pos[1], "independent ground per foot");
    }

    #[test]
    fn footfall_carries_mass() {
        let c = body();
        let im = c.footfall([0.0, 10.0, 0.0], 5.0, 0.5);
        assert_eq!(im.mass, 40.0);
        let r = crate::sim::impact_response::respond(&im, [1.0, 10.0, 0.0]);
        assert!(r.shake > 0.0 && r.erosion > 0.0);
    }
}

#[cfg(test)]
mod cliff_tests {
    use super::*;
    use crate::physics::PhysicsBody;

    fn dragon_like(half_h: f32) -> Creature {
        let mut c = Creature::new(PhysicsBody::new_player([0.0, 0.0, 0.0]));
        c.body.half_extents = [2.0, half_h, 4.0];
        c
    }
    // Flat ground at 0 until x >= edge, then a cliff of height h.
    fn ground(x: f32, edge: f32, h: f32) -> f32 {
        if x >= edge { h } else { 0.0 }
    }

    #[test]
    fn max_step_scales_with_size() {
        assert!(dragon_like(2.0).max_step_height() > dragon_like(1.0).max_step_height());
    }

    #[test]
    fn steps_up_a_small_cliff_and_advances_over_time() {
        let mut c = dragon_like(1.5);
        let max_step = c.max_step_height(); // 1.5 blocks
        let cliff = max_step * 0.6; // climbable
        let (edge, v, dt) = (10.0, 2.0, 1.0);
        let mut cur_ground = 0.0;
        let mut t = 0.0;
        for _ in 0..30 {
            let nx = c.body.position[0] + v * dt;
            let g = ground(nx, edge, cliff);
            if (g - cur_ground).abs() <= max_step {
                c.body.position[0] = nx;
                cur_ground = g;
                c.body.position[1] = c.grounded_y(g);
                t += dt;
            } else {
                break;
            }
        }
        assert!(c.body.position[0] > edge, "advanced past the cliff edge");
        assert!((c.body.position[1] - (cliff + max_step)).abs() < 1e-3, "climbed onto the ledge");
        // Position after time moving with no block: x = v * t.
        assert!((c.body.position[0] - v * t).abs() < 1e-3);
    }

    #[test]
    fn blocked_by_a_cliff_taller_than_its_step() {
        let mut c = dragon_like(1.0);
        let max_step = c.max_step_height(); // 1.0 block
        let cliff = max_step + 3.0; // far over the step → blocked
        let (edge, v, dt) = (10.0, 2.0, 1.0);
        let mut cur_ground = 0.0;
        let mut blocked = false;
        for _ in 0..30 {
            let nx = c.body.position[0] + v * dt;
            let g = ground(nx, edge, cliff);
            if (g - cur_ground).abs() <= max_step {
                c.body.position[0] = nx;
                cur_ground = g;
                c.body.position[1] = c.grounded_y(g);
            } else {
                blocked = true;
                break;
            }
        }
        assert!(blocked, "a cliff taller than its step blocks the dragon");
        assert!(c.body.position[0] <= edge, "stopped at the wall");
        assert!((c.body.position[1] - c.grounded_y(0.0)).abs() < 1e-3, "stayed on the low ground");
    }

    #[test]
    fn feet_step_independently_at_the_edge() {
        let c = dragon_like(1.5);
        // Front feet up on the ledge (1.0), rear feet still low (0.0).
        let feet = c.quadruped_feet([1.0, 1.0, 0.0, 0.0], 0.0, 0.0, 2.0);
        assert!(feet[0].pos[1] > feet[2].pos[1], "front foot steps onto the ledge while the rear stays low");
        assert!(feet[1].pos[1] > feet[3].pos[1]);
    }
}

#[cfg(test)]
mod predation_tests {
    use super::*;
    use crate::physics::PhysicsBody;
    use crate::sim::predation::{hunt, Predator};

    fn predator_from(c: &Creature) -> Predator {
        Predator::from_capabilities(&c.capabilities(), 60.0, 40.0, 3.0, 2.0)
    }

    #[test]
    fn eyed_dragon_eats_prey() {
        let d = crate::spawn::spawn(crate::item::CreatureKind::Dragon, [0.0; 3]);
        let caps = d.capabilities();
        assert!(caps.see && caps.eat, "a spawned dragon has eyes and jaws");
        assert!(hunt(&predator_from(&d), 30.0, false, 500).ate, "a seeing, biting dragon eats prey");
    }

    #[test]
    fn eyeless_creature_eats_only_when_sound_is_enabled() {
        // Jawed + eared, but no eyes.
        let c = Creature::new(PhysicsBody::new_player([0.0; 3])).with_anatomy(
            Anatomy::new()
                .with(PartState::new(Part::Jaw, 10, [1.0, 0.0, 0.0], [0.3, 0.3, 0.3]))
                .with(PartState::new(Part::Ear, 10, [0.5, 0.5, 0.3], [0.1, 0.1, 0.1])),
        );
        let caps = c.capabilities();
        assert!(!caps.see && caps.hear && caps.eat, "eyeless, eared, jawed");
        let p = predator_from(&c);
        assert!(hunt(&p, 30.0, true, 500).ate, "eyeless but hears with sound → eats");
        assert!(!hunt(&p, 30.0, false, 500).ate, "eyeless and no sound → cannot find prey");
    }
}
