//! WORLDHOST — what a game needs from a world, and nothing about how the
//! world is made.
//!
//! blackstar-rs's game systems were written renderer-agnostic on purpose:
//! `field` takes a ground closure and a line-of-sight closure and hands
//! back events; `sortie` owns a field and never draws; `district` is a
//! lattice of pieces with no `World` in hand. What they share is a small
//! set of questions about the world — how high is the ground here, is
//! this point clear, can A see B, where do people stand, what do I steer
//! around, where is this window on the plane. This trait is those
//! questions and no others.
//!
//! Two implementors are intended, and the reason for the crate is that
//! they are different engines:
//!
//! * **skydribble's `world::World`** (blackstar-rs today): a fixed tile,
//!   an SDF the shader ray-marches, a height grid. `clearance` is the
//!   SDF; `ground_h` is the grid; the district's landmarks are fields.
//! * **unendlich's `ChunkManager`** (next): streamed voxel chunks with
//!   GPU worldgen, CSM shadows, volumetrics — the world loop this game
//!   needs at car speed. `clearance` is a step function off `block_at`;
//!   `ground_h` is `surface_y_at`; `line_of_sight` is its octree
//!   raycast. Landmarks come from a structure placer the district lattice
//!   feeds.
//!
//! The game is the backend; the engine is the frontend, the way any
//! well-built app splits, and the way an Unreal game is the game and not
//! the renderer. Two games on one host is what pays: the tooling unifies
//! and the engine gets fixed for both at once.
//!
//! Kept deliberately small. Physics is NOT here — skydribble's physics
//! is coupled to its own shape list and unendlich has its own voxel
//! physics; the body on the ground is the host's business until one of
//! them wins. Rendering is not here. Content is not here.

use glam::Vec3;

pub trait WorldHost {
    /// Height of the ground under (x, z), window-local engine metres.
    fn ground_h(&self, x: f32, z: f32) -> f32;

    /// Signed clearance at `p`: positive is free space, zero or less is
    /// inside something solid. An SDF host returns the true distance; a
    /// voxel host may return a step (a large positive for air, a
    /// negative for solid) — callers only ever ask whether a point is
    /// clear and how far they may step, never for the exact distance.
    fn clearance(&self, p: Vec3) -> f32;

    /// Can `a` see `b`. The default marches `clearance` in `LOS_STEP_M`
    /// steps; a host with a real raycast should override it.
    fn line_of_sight(&self, a: Vec3, b: Vec3) -> bool {
        let d = b - a;
        let len = d.length();
        if len < 1e-3 {
            return true;
        }
        let dir = d / len;
        let mut t = LOS_STEP_M;
        while t < len - 1.0 {
            if self.clearance(a + dir * t) < LOS_CLEAR_M {
                return false;
            }
            t += LOS_STEP_M;
        }
        true
    }

    /// Velocity of whatever solid is within `reach` of `p`, if that
    /// solid is under way. `None` means nothing near, or nothing near is
    /// moving — a body standing here inherits no motion.
    ///
    /// A world question, not a physics one: the host knows which of its
    /// solids move because it moves them. A static world takes the
    /// default and never thinks about it.
    fn platform_vel(&self, _p: Vec3, _reach: f32) -> Option<Vec3> {
        None
    }

    /// Which way is DOWN at `p`. A world with a plane answers `-Y`
    /// everywhere; a world made of bodies answers "toward whichever body
    /// is pulling here". The default is the plane answer, so a host that
    /// has never thought about it stays correct.
    ///
    /// Not normalised by contract — callers normalise, because a host
    /// summing several attractors would otherwise pay for it twice.
    fn gravity_dir(&self, _p: Vec3) -> Vec3 {
        Vec3::NEG_Y
    }

    /// Does `ground_h` mean anything on this world? False on a field
    /// world — an asteroid belt, open space — where the terrain grid
    /// exists only because every map gets one, and consulting it would
    /// invent a floor that is not there.
    ///
    /// This is the one bit `footing` needs that `ground_h`'s signature
    /// cannot carry: it returns an `f32`, so it can only say HOW HIGH,
    /// never WHETHER.
    fn ground_plane(&self) -> bool {
        true
    }

    /// Where this window sits on the world plane. A host with one fixed
    /// world returns zero.
    fn origin(&self) -> Vec3 {
        Vec3::ZERO
    }

    /// Landmarks: where a wave stands (a district's plazas). Empty on a
    /// world that has none.
    fn plazas(&self) -> &[Vec3] {
        &[]
    }
    /// Cover a body can stand behind.
    fn cover(&self) -> &[Vec3] {
        &[]
    }
    /// Solids a kinematic body must steer around, as ground discs.
    fn obstacles(&self) -> &[(Vec3, f32)] {
        &[]
    }
}

/// March step for the default line of sight, metres.
pub const LOS_STEP_M: f32 = 1.5;
/// Below this clearance a point counts as blocked.
pub const LOS_CLEAR_M: f32 = 0.15;

/// THE CONTRACT, EXECUTABLE — what every host must agree on.
///
/// Two engines answering the same trait is only worth something if their
/// answers mean the same thing. The trait's signatures cannot say that
/// `clearance` is negative inside a solid, or that looking at yourself is
/// always clear; those are prose in the doc comments, and prose is not
/// checked. This module is that prose as functions, so both hosts can run
/// the same suite and a third host has a definition of "done".
///
/// Not `#[cfg(test)]`: it is public API precisely so OTHER crates' tests can
/// call it. It costs a few hundred bytes in a release build and buys the only
/// evidence that a swap is safe.
pub mod conformance {
    use super::*;

    /// A point the host should consider open sky, and one it should consider
    /// buried, so the suite can test a host without knowing its terrain.
    pub struct Probe {
        /// Somewhere with nothing solid at it.
        pub open: Vec3,
        /// Somewhere inside solid matter. `None` if the world has none.
        pub buried: Option<Vec3>,
        /// A ground column to sample, as (x, z).
        pub column: (f32, f32),
    }

    /// Run every invariant. Panics with the failing rule named, so a caller
    /// writes one `#[test]` per host and gets a specific failure.
    pub fn check(host: &dyn WorldHost, p: &Probe) {
        ground_is_deterministic_and_finite(host, p);
        clearance_agrees_with_its_own_sign(host, p);
        looking_at_yourself_is_clear(host, p);
        sight_is_symmetric(host, p);
        down_is_a_direction(host, p);
        landmark_slices_are_readable(host);
    }

    /// Asking twice must answer twice the same, and never NaN. A streamed
    /// host can fail this by letting a query mutate a cache.
    pub fn ground_is_deterministic_and_finite(host: &dyn WorldHost, p: &Probe) {
        let (x, z) = p.column;
        let a = host.ground_h(x, z);
        let b = host.ground_h(x, z);
        assert!(a.is_finite(), "ground_h returned {a}");
        assert_eq!(a.to_bits(), b.to_bits(), "ground_h is not deterministic at ({x}, {z})");
        assert!(host.clearance(p.open).is_finite(), "clearance returned a non-finite value");
    }

    /// Positive is free, zero or less is inside something. The trait allows a
    /// step instead of a true distance, but not a sign that lies.
    pub fn clearance_agrees_with_its_own_sign(host: &dyn WorldHost, p: &Probe) {
        assert!(host.clearance(p.open) > 0.0, "an open point reported as solid: {:?}", p.open);
        if let Some(b) = p.buried {
            assert!(host.clearance(b) <= 0.0, "a buried point reported as free: {b:?}");
        }
        if host.ground_plane() {
            let (x, z) = p.column;
            let above = Vec3::new(x, host.ground_h(x, z) + 4.0, z);
            assert!(
                host.clearance(above) > 0.0,
                "four metres above this world's own ground is not free space: {above:?}"
            );
        }
    }

    /// Zero-length sight must not consult the world at all.
    pub fn looking_at_yourself_is_clear(host: &dyn WorldHost, p: &Probe) {
        assert!(host.line_of_sight(p.open, p.open), "a point cannot see itself");
    }

    /// Sight is a property of the segment, not of which end asked. A host
    /// that overrides `line_of_sight` with a one-directional march can fail
    /// this, which is exactly why it is checked rather than assumed.
    pub fn sight_is_symmetric(host: &dyn WorldHost, p: &Probe) {
        let Some(b) = p.buried else { return };
        let there = host.line_of_sight(p.open, b);
        let back = host.line_of_sight(b, p.open);
        assert_eq!(there, back, "sight between {:?} and {b:?} depends on who is asking", p.open);
    }

    /// Down must be usable as a direction: non-zero and finite. Not
    /// normalised — the trait says callers normalise.
    pub fn down_is_a_direction(host: &dyn WorldHost, p: &Probe) {
        let g = host.gravity_dir(p.open);
        assert!(g.is_finite(), "gravity_dir returned {g:?}");
        assert!(g.length_squared() > 1e-6, "gravity_dir has no direction at {:?}", p.open);
    }

    /// The landmark slices must be callable and internally consistent —
    /// an obstacle with a negative radius is a host bug that only shows up
    /// as a steering failure much later.
    pub fn landmark_slices_are_readable(host: &dyn WorldHost) {
        for (c, r) in host.obstacles() {
            assert!(r.is_finite() && *r >= 0.0, "obstacle at {c:?} has radius {r}");
            assert!(c.is_finite(), "obstacle centre is not finite: {c:?}");
        }
        for v in host.plazas().iter().chain(host.cover()) {
            assert!(v.is_finite(), "landmark is not finite: {v:?}");
        }
    }
}

/// The empty world: flat at `y`, nothing solid, sees everything. For
/// tests, and for a game system that wants to run with no world at all.
#[derive(Clone, Copy, Debug, Default)]
pub struct Flat {
    pub y: f32,
}

impl WorldHost for Flat {
    fn ground_h(&self, _x: f32, _z: f32) -> f32 {
        self.y
    }
    fn clearance(&self, p: Vec3) -> f32 {
        p.y - self.y
    }
}

/// A flat world with a few solid balls in it, for testing anything that
/// asks about clearance or sight.
#[derive(Clone, Debug, Default)]
pub struct Balls {
    pub y: f32,
    pub balls: Vec<(Vec3, f32)>,
}

impl WorldHost for Balls {
    fn ground_h(&self, _x: f32, _z: f32) -> f32 {
        self.y
    }
    fn clearance(&self, p: Vec3) -> f32 {
        let mut d = p.y - self.y;
        for (c, r) in &self.balls {
            d = d.min((p - *c).length() - r);
        }
        d
    }
    fn obstacles(&self) -> &[(Vec3, f32)] {
        &self.balls
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_world_sees_everything_and_stands_at_its_height() {
        let f = Flat { y: 2.0 };
        assert_eq!(f.ground_h(10.0, -3.0), 2.0);
        assert!(f.line_of_sight(Vec3::new(0.0, 3.0, 0.0), Vec3::new(100.0, 3.0, 50.0)));
        assert!(f.clearance(Vec3::new(0.0, 1.0, 0.0)) < 0.0);
        assert!(f.plazas().is_empty());
    }

    #[test]
    fn a_ball_in_the_way_blocks_sight_and_beside_it_does_not() {
        let b = Balls { y: 0.0, balls: vec![(Vec3::new(20.0, 1.0, 0.0), 3.0)] };
        let a = Vec3::new(0.0, 1.0, 0.0);
        assert!(!b.line_of_sight(a, Vec3::new(40.0, 1.0, 0.0)));
        assert!(b.line_of_sight(a, Vec3::new(40.0, 1.0, 10.0)));
        assert_eq!(b.obstacles().len(), 1);
    }
}

#[cfg(test)]
mod conformance_tests {
    use super::conformance::{check, Probe};
    use super::*;

    #[test]
    fn the_built_in_hosts_satisfy_their_own_contract() {
        let flat = Flat { y: 0.0 };
        check(&flat, &Probe { open: Vec3::new(0.0, 10.0, 0.0), buried: None, column: (3.0, -4.0) });

        let balls = Balls { y: 0.0, balls: vec![(Vec3::new(20.0, 5.0, 0.0), 3.0)] };
        check(
            &balls,
            &Probe {
                open: Vec3::new(0.0, 10.0, 0.0),
                buried: Some(Vec3::new(20.0, 5.0, 0.0)),
                column: (1.0, 1.0),
            },
        );
    }
}
