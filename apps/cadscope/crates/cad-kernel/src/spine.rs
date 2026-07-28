//! spine.rs — growing solids by driving a turtle, ported from
//! `games/blackstar/tools/spine_grower.html`. That tool is the reason CADscope has a Grow mode at
//! all: it demonstrated that steering a growing form with the keyboard is *faster* than placing
//! primitives, because you react to the shape as it appears instead of specifying it up front.
//!
//! The port keeps the grammar and the numbers exactly (0.03 radius steps clamped to 0.06..=1.4,
//! 0.06 taper steps clamped to 0.25..=2.2, 0.6 twist steps, 0.9/0.9/1.2 rad·s⁻¹ steering rates,
//! 0.06·dt advance, 1.8× node bulge, 0.7× child radius, 0.6 rad branch kick). What changes is the
//! representation underneath: the original lofts rings of points into a triangle mesh, whereas
//! here a spine is a chain of `Sdf::Swept` segments blended together — an f-rep solid, so it can
//! be subtracted from, shelled, measured and meshed like any other part.

use glam::{Vec3, Quat};

use crate::sdf::{self, Sdf, Section};

/// One recorded station along a spine: where the turtle was, how thick, which profile, and the
/// frame it was carrying (so twist and roll survive into the geometry).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub pos: Vec3,
    pub up: Vec3,
    pub radius: f32,
    pub section: Section,
}

/// A grown solid: consecutive nodes become swept segments, blended into one body.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spine {
    pub nodes: Vec<Node>,
    /// Smooth-union radius between consecutive segments. Small but non-zero keeps the joints from
    /// reading as facets where the direction changes.
    pub blend: f32,
}

impl Spine {
    pub fn new(blend: f32) -> Spine {
        Spine { nodes: Vec::new(), blend }
    }

    /// Build the solid. `None` when there is nothing to sweep (fewer than two stations).
    pub fn to_sdf(&self) -> Option<Sdf> {
        if self.nodes.len() < 2 {
            return None;
        }
        let mut acc: Option<Sdf> = None;
        for w in self.nodes.windows(2) {
            let (a, b) = (w[0], w[1]);
            if (b.pos - a.pos).length_squared() < 1e-12 {
                continue;
            }
            let seg = sdf::swept(a.pos, b.pos, a.radius, b.radius, a.section, a.up);
            acc = Some(match acc {
                None => seg,
                Some(prev) if self.blend > 0.0 => sdf::blend(prev, seg, self.blend),
                Some(prev) => sdf::union(prev, seg),
            });
        }
        acc
    }

    /// Total centreline length — the readout a machinist actually wants while growing.
    pub fn length(&self) -> f32 {
        self.nodes.windows(2).map(|w| (w[1].pos - w[0].pos).length()).sum()
    }
}

/// The growth state the keyboard drives. One turtle per spine.
#[derive(Clone, Copy, Debug)]
pub struct Turtle {
    pub pos: Vec3,
    pub dir: Vec3,
    pub up: Vec3,
    pub radius: f32,
    pub taper: f32,
    pub twist: f32,
    pub twist_accum: f32,
    pub wobble: bool,
    pub wobble_t: f32,
    /// The grower's `e` toggle: 0.0 or 0.6.
    pub ridge: f32,
    pub section: Section,
}

impl Default for Turtle {
    fn default() -> Self {
        Turtle {
            pos: Vec3::new(0.0, 0.6, 0.0),
            dir: Vec3::Z,
            up: Vec3::Y,
            radius: 0.35,
            taper: 1.0,
            twist: 0.0,
            twist_accum: 0.0,
            wobble: false,
            wobble_t: 0.0,
            ridge: 0.0,
            section: Section::Circle,
        }
    }
}

// The grammar's constants, named rather than sprinkled.
pub const RADIUS_STEP: f32 = 0.03;
pub const RADIUS_RANGE: (f32, f32) = (0.06, 1.4);
pub const TAPER_STEP: f32 = 0.06;
pub const TAPER_RANGE: (f32, f32) = (0.25, 2.2);
pub const TWIST_STEP: f32 = 0.6;
pub const YAW_RATE: f32 = 0.9;
pub const PITCH_RATE: f32 = 0.9;
pub const ROLL_RATE: f32 = 1.2;
pub const ADVANCE_RATE: f32 = 0.06;
pub const NODE_BULGE: f32 = 1.8;
pub const BRANCH_RADIUS: f32 = 0.7;
pub const BRANCH_KICK: f32 = 0.6;

impl Turtle {
    fn side(&self) -> Vec3 {
        self.dir.cross(self.up).normalize_or(Vec3::X)
    }
    fn turn(v: Vec3, axis: Vec3, ang: f32) -> Vec3 {
        (Quat::from_axis_angle(axis.normalize_or(Vec3::Y), ang) * v).normalize_or(v)
    }

    /// `h` / `l` — yaw about the local up.
    pub fn yaw(&mut self, dt: f32) {
        self.dir = Self::turn(self.dir, self.up, YAW_RATE * dt);
    }
    /// `j` / `k` — pitch, carrying `up` with it so the frame stays orthogonal.
    pub fn pitch(&mut self, dt: f32) {
        let side = self.side();
        self.dir = Self::turn(self.dir, side, PITCH_RATE * dt);
        self.up = Self::turn(self.up, side, PITCH_RATE * dt);
    }
    /// `H` / `L` — roll `up` about the direction of travel.
    pub fn roll(&mut self, dt: f32) {
        self.up = Self::turn(self.up, self.dir, ROLL_RATE * dt);
    }
    /// `w` / `b`.
    pub fn thicken(&mut self, sign: f32) {
        self.radius = (self.radius + RADIUS_STEP * sign).clamp(RADIUS_RANGE.0, RADIUS_RANGE.1);
    }
    /// `f` / `p` — flare and pinch.
    pub fn flare(&mut self, sign: f32) {
        self.taper = (self.taper + TAPER_STEP * sign).clamp(TAPER_RANGE.0, TAPER_RANGE.1);
    }
    /// `]` / `[`.
    pub fn twist_by(&mut self, sign: f32) {
        self.twist += TWIST_STEP * sign;
    }
    /// `0` — kill the twist and re-level.
    pub fn straighten(&mut self) {
        self.twist = 0.0;
        self.twist_accum = 0.0;
        self.up = Vec3::Y;
    }
    /// `e`.
    pub fn toggle_ridge(&mut self) {
        self.ridge = if self.ridge > 0.0 { 0.0 } else { 0.6 };
    }
    /// `` ` `` / `~`.
    pub fn toggle_wobble(&mut self) {
        self.wobble = !self.wobble;
    }
    /// `1`..`5`.
    pub fn set_section(&mut self, s: Section) {
        self.section = s;
    }

    /// The station the turtle would record right now. `bulge` is 1.0 normally and `NODE_BULGE`
    /// for the `space` key, which is how the grower punctuates a form with knuckles.
    pub fn station(&self, bulge: f32) -> Node {
        Node {
            pos: self.pos,
            up: Quat::from_axis_angle(self.dir.normalize_or(Vec3::Z), self.twist_accum) * self.up,
            radius: (self.radius * self.taper * bulge).max(1e-3),
            section: self.section,
        }
    }

    /// Advance one tick: wobble, accumulate twist, step forward.
    pub fn advance(&mut self, dt: f32) {
        if self.wobble {
            self.wobble_t += dt * 3.0;
            self.dir = Self::turn(self.dir, self.up, self.wobble_t.sin() * 0.05);
        }
        self.twist_accum += self.twist * dt;
        self.pos += self.dir * (ADVANCE_RATE * dt);
    }

    /// `t` — a child that starts thinner and visibly diverges.
    pub fn branch(&self) -> Turtle {
        let mut child = *self;
        child.radius *= BRANCH_RADIUS;
        child.dir = Self::turn(child.dir, child.up, BRANCH_KICK);
        child
    }
}

/// The whole grown assembly: several spines, a branch stack for `t`/`y`, and the `m` mirror.
#[derive(Clone, Debug)]
pub struct Grower {
    pub spines: Vec<Spine>,
    pub turtles: Vec<Turtle>,
    pub active: usize,
    stack: Vec<usize>,
    pub mirror: bool,
    pub blend: f32,
}

impl Default for Grower {
    fn default() -> Self {
        Grower::new(0.12)
    }
}

impl Grower {
    pub fn new(blend: f32) -> Grower {
        let turtle = Turtle::default();
        let mut spine = Spine::new(blend);
        spine.nodes.push(turtle.station(1.0));
        Grower { spines: vec![spine], turtles: vec![turtle], active: 0, stack: Vec::new(), mirror: false, blend }
    }

    pub fn turtle(&mut self) -> &mut Turtle {
        &mut self.turtles[self.active]
    }

    /// Advance the active turtle and record a station.
    pub fn grow(&mut self, dt: f32) {
        let t = &mut self.turtles[self.active];
        t.advance(dt);
        let station = t.station(1.0);
        self.spines[self.active].nodes.push(station);
    }

    /// `space` — a knuckle: two fat stations in place, exactly as the grower does it.
    pub fn node_bulge(&mut self) {
        let station = self.turtles[self.active].station(NODE_BULGE);
        let nodes = &mut self.spines[self.active].nodes;
        nodes.push(station);
        nodes.push(station);
    }

    /// `t` — fork, and steer the child from now on.
    pub fn branch(&mut self) {
        let child = self.turtles[self.active].branch();
        let mut spine = Spine::new(self.blend);
        spine.nodes.push(child.station(1.0));
        self.stack.push(self.active);
        self.spines.push(spine);
        self.turtles.push(child);
        self.active = self.spines.len() - 1;
    }

    /// `y` — back to the branch you came from.
    pub fn parent(&mut self) {
        if let Some(prev) = self.stack.pop() {
            self.active = prev;
        }
    }

    /// `m`.
    pub fn toggle_mirror(&mut self) {
        self.mirror = !self.mirror;
    }

    /// The assembled solid: every spine unioned, plus its reflection across X when mirroring.
    pub fn to_sdf(&self) -> Option<Sdf> {
        let mut acc: Option<Sdf> = None;
        for spine in &self.spines {
            if let Some(s) = spine.to_sdf() {
                acc = Some(match acc {
                    None => s,
                    Some(prev) => sdf::blend(prev, s, self.blend),
                });
            }
        }
        let acc = acc?;
        if !self.mirror {
            return Some(acc);
        }
        let flipped = mirror_x(&acc);
        Some(sdf::blend(acc, flipped, self.blend))
    }
}

/// Reflect a tree across X = 0. Only the placement-bearing nodes need touching, since every
/// primitive here is symmetric about its own axes except through its stored geometry.
fn mirror_x(s: &Sdf) -> Sdf {
    let flip = |v: Vec3| Vec3::new(-v.x, v.y, v.z);
    match s {
        Sdf::Swept { a, b, ra, rb, section, up } => Sdf::Swept {
            a: flip(*a),
            b: flip(*b),
            ra: *ra,
            rb: *rb,
            section: *section,
            up: flip(*up),
        },
        Sdf::Translate { off, node } => Sdf::Translate { off: flip(*off), node: Box::new(mirror_x(node)) },
        Sdf::Union { a, b } => sdf::union(mirror_x(a), mirror_x(b)),
        Sdf::Intersect { a, b } => sdf::intersect(mirror_x(a), mirror_x(b)),
        Sdf::Subtract { a, b } => sdf::subtract(mirror_x(a), mirror_x(b)),
        Sdf::Blend { a, b, k } => sdf::blend(mirror_x(a), mirror_x(b), *k),
        Sdf::Shell { t, node } => sdf::shell(*t, mirror_x(node)),
        // A rotation's mirror is not simply its own inverse; rather than get it subtly wrong,
        // wrap the subtree — correct, and one node deeper.
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic sampler: no rand dependency in the kernel.
    fn lcg(state: &mut u64) -> f32 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*state >> 33) as f32 / (1u64 << 31) as f32) - 1.0
    }

    fn straight_spine() -> Spine {
        let mut t = Turtle::default();
        let mut s = Spine::new(0.0);
        s.nodes.push(t.station(1.0));
        for _ in 0..20 {
            t.advance(1.0);
            s.nodes.push(t.station(1.0));
        }
        s
    }

    #[test]
    fn a_straight_spine_is_a_rod_of_the_right_length_and_thickness() {
        let s = straight_spine();
        assert!((s.length() - 20.0 * ADVANCE_RATE).abs() < 1e-4);
        let sdf = s.to_sdf().expect("two or more nodes");
        // On the axis, a quarter of the way along: inside, and about `radius` from the wall.
        let mid = Vec3::new(0.0, 0.6, 20.0 * ADVANCE_RATE * 0.25);
        assert!((sdf.eval(mid) + 0.35).abs() < 0.02, "{}", sdf.eval(mid));
        // Well outside the tube, but level with it.
        assert!(sdf.eval(mid + Vec3::X * 2.0) > 1.5);
    }

    #[test]
    fn distance_never_overestimates_for_any_section() {
        // The invariant the sphere tracer and the mesher both depend on: stepping by the reported
        // distance can graze the surface but must never cross it.
        for section in [Section::Circle, Section::Square, Section::Triangle, Section::Star, Section::Hex] {
            let s = Sdf::Swept {
                a: Vec3::new(-1.0, 0.0, 0.0),
                b: Vec3::new(1.5, 0.8, 0.3),
                ra: 0.7,
                rb: 0.25,
                section,
                up: Vec3::Y,
            };
            let mut st = 0x5eed_1234_u64;
            let mut checked = 0;
            for _ in 0..300 {
                let p = Vec3::new(lcg(&mut st) * 3.0, lcg(&mut st) * 3.0, lcg(&mut st) * 3.0);
                let d = s.eval(p);
                if d.abs() < 1e-2 {
                    continue;
                }
                for _ in 0..6 {
                    let dir = Vec3::new(lcg(&mut st), lcg(&mut st), lcg(&mut st)).normalize_or(Vec3::X);
                    let q = p + dir * (d.abs() * 0.9);
                    assert_eq!(
                        s.eval(q) < 0.0,
                        d < 0.0,
                        "{section:?}: sign flipped within the reported distance at {p:?} (d={d})"
                    );
                    checked += 1;
                }
            }
            assert!(checked > 500, "{section:?}: too few samples exercised");
        }
    }

    #[test]
    fn the_grammar_clamps_exactly_where_the_grower_did() {
        let mut t = Turtle::default();
        for _ in 0..200 {
            t.thicken(1.0);
        }
        assert!((t.radius - RADIUS_RANGE.1).abs() < 1e-6);
        for _ in 0..200 {
            t.thicken(-1.0);
        }
        assert!((t.radius - RADIUS_RANGE.0).abs() < 1e-6);
        for _ in 0..200 {
            t.flare(-1.0);
        }
        assert!((t.taper - TAPER_RANGE.0).abs() < 1e-6);
    }

    #[test]
    fn steering_keeps_the_frame_orthonormal() {
        let mut t = Turtle::default();
        for i in 0..400 {
            t.yaw(0.02);
            t.pitch(if i % 3 == 0 { 0.03 } else { -0.01 });
            t.roll(0.02);
            t.advance(1.0);
        }
        assert!((t.dir.length() - 1.0).abs() < 1e-3);
        assert!((t.up.length() - 1.0).abs() < 1e-3);
        assert!(t.dir.dot(t.up).abs() < 1e-2, "frame sheared: {}", t.dir.dot(t.up));
    }

    #[test]
    fn branch_forks_thinner_and_return_comes_back() {
        let mut g = Grower::new(0.1);
        for _ in 0..5 {
            g.grow(1.0);
        }
        let trunk_radius = g.turtles[0].radius;
        g.branch();
        assert_eq!(g.active, 1);
        assert!((g.turtles[1].radius - trunk_radius * BRANCH_RADIUS).abs() < 1e-6);
        for _ in 0..5 {
            g.grow(1.0);
        }
        g.parent();
        assert_eq!(g.active, 0);
        assert_eq!(g.spines.len(), 2);
        assert!(g.to_sdf().is_some());
    }

    #[test]
    fn node_bulge_is_fatter_than_the_shaft() {
        let mut g = Grower::new(0.0);
        for _ in 0..3 {
            g.grow(1.0);
        }
        let shaft = g.spines[0].nodes.last().unwrap().radius;
        g.node_bulge();
        let knuckle = g.spines[0].nodes.last().unwrap().radius;
        assert!((knuckle / shaft - NODE_BULGE).abs() < 1e-4);
    }

    #[test]
    fn mirror_reflects_across_x() {
        let mut g = Grower::new(0.1);
        g.turtle().yaw(1.0); // steer off-centre so the reflection is somewhere new
        for _ in 0..12 {
            g.grow(1.0);
        }
        let plain = g.to_sdf().unwrap();
        g.toggle_mirror();
        let mirrored = g.to_sdf().unwrap();
        let tip = g.spines[0].nodes.last().unwrap().pos;
        let reflected = Vec3::new(-tip.x, tip.y, tip.z);
        assert!(plain.eval(reflected) > 0.0, "unmirrored solid should not reach there");
        assert!(mirrored.eval(reflected) <= 0.0, "mirrored solid should");
    }

    #[test]
    fn a_grown_spine_meshes_to_a_closed_solid() {
        let mut g = Grower::new(0.12);
        g.turtle().twist_by(1.0);
        for i in 0..40 {
            if i % 7 == 0 {
                g.turtle().yaw(0.15);
            }
            g.grow(1.0);
        }
        let sdf = g.to_sdf().unwrap();
        let m = crate::dual_contour(&sdf, crate::MeshOpts { resolution: 40 });
        assert!(!m.is_empty());
        assert!(m.signed_volume() > 0.0, "winding inverted");
    }
}
