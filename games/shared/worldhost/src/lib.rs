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
