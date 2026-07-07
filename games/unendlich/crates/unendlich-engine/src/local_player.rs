//! Local-player bundle — the local-side handles needed to drive
//! one human player through one viewport.
//!
//! ## Why this struct exists
//!
//! Today there's exactly one local player in the game (the human at
//! the keyboard) and the bin's main loop directly mutates a body and
//! a `FlyCamera`. That code has implicit "there is one of these"
//! sprinkled through it.
//!
//! Multiplayer requires that we can have:
//!   - Zero local players (dedicated server hosting only remote
//!     players — the simulation runs without anyone driving it
//!     from this process).
//!   - Multiple local players (split-screen, 2-4 humans on the same
//!     machine sharing a simulation).
//!   - A mix of local and remote players (most common case — the
//!     local human plus several other clients).
//!
//! `LocalPlayer` wraps the per-local-human state so the bin's loop
//! can iterate over an array of them rather than coding for "the"
//! player. Today we have one entry; tomorrow we might have several
//! without rearchitecting the loop.
//!
//! ## What's HERE vs. what's deferred
//!
//! Here:
//!   - `body_id`: the BodyId of this player's avatar in the physics
//!     world. Indirection (rather than holding a body reference)
//!     means the body can move between data structures (e.g. ECS
//!     migration, persistence, server replication) without invali-
//!     dating LocalPlayer.
//!   - `view`: the View this player looks through. Mutated by the
//!     controller from input each frame; consumed by the renderer
//!     on draw.
//!   - `controller`: per-player input config (move_speed, mouse
//!     sensitivity). Different humans can have different settings.
//!
//! Deferred:
//!   - Input bindings. Right now the bin's input handling is
//!     hardcoded for one player using global keys. Split-screen
//!     wants per-player bindings (P1 uses WASD, P2 uses IJKL +
//!     gamepad, etc.). When that's needed, an `InputMap` field
//!     joins this struct.
//!   - Network identity. When this becomes a networked game,
//!     LocalPlayer will gain a session-token / auth handle. The
//!     server's view of the same human is a different struct.

use crate::physics::BodyId;
use crate::permissions::Permissions;
use crate::render::camera::{FlyController, View};

/// One local human's state. There can be 0..N of these per process.
#[derive(Debug, Clone)]
pub struct LocalPlayer {
    /// Stable identity of this player's avatar in the physics world.
    pub body_id: BodyId,
    /// The pose this player is looking through. Mutated by
    /// controller from input each frame; passed to the renderer
    /// for the corresponding draw pass.
    pub view: View,
    /// Per-player input config. Driven from a settings system once
    /// one exists; for now defaults are fine.
    pub controller: FlyController,
    /// What this player is allowed to do. Default is full-admin
    /// (single-player). Multiplayer servers push restricted values.
    /// Action sites in the bin gate through this BEFORE doing the
    /// action — even in single-player, where every check returns
    /// true, so the multiplayer migration is just changing the
    /// source of the value.
    pub permissions: Permissions,
}

impl LocalPlayer {
    /// Construct a local player attached to a specific body.
    /// The View starts at the origin; the bin should sync it
    /// from the body's render position before the first frame.
    pub fn new(body_id: BodyId) -> Self {
        Self {
            body_id,
            view: View::new(nalgebra_glm::vec3(0.0, 0.0, 0.0)),
            controller: FlyController::default(),
            permissions: Permissions::default(),
        }
    }

    /// Construct a local player at a specific spawn position. Convenience
    /// for the bin's setup path (spawn point known up front).
    pub fn at_position(body_id: BodyId, position: nalgebra_glm::Vec3) -> Self {
        Self {
            body_id,
            view: View::new(position),
            controller: FlyController::default(),
            permissions: Permissions::default(),
        }
    }

    /// Apply a mouse delta to this player's view via their controller.
    /// Convenience to keep call sites short.
    pub fn apply_mouse(&mut self, dx: f64, dy: f64) {
        self.controller.apply_mouse(&mut self.view, dx, dy);
    }

    /// Apply a movement delta to this player's view via their controller.
    pub fn apply_move(&mut self, local: nalgebra_glm::Vec3, dt: f32) {
        self.controller.apply_move(&mut self.view, local, dt);
    }

    /// Forward direction of this player's view.
    pub fn forward(&self) -> nalgebra_glm::Vec3 { self.view.forward() }

    /// Right direction of this player's view (horizontal).
    pub fn right(&self) -> nalgebra_glm::Vec3 { self.view.right() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LocalPlayers can coexist independently — split-screen prep.
    /// Mutating one's view must not affect another's.
    #[test]
    fn two_local_players_have_independent_views() {
        let mut p1 = LocalPlayer::new(BodyId(1));
        let mut p2 = LocalPlayer::new(BodyId(2));
        // P1 looks right; P2 looks up.
        p1.controller.apply_mouse(&mut p1.view, 200.0, 0.0);
        p2.controller.apply_mouse(&mut p2.view, 0.0, -200.0);
        assert!(p1.view.yaw.abs() > 0.1, "p1 yaw didn't change");
        assert!(p2.view.pitch.abs() > 0.1, "p2 pitch didn't change");
        assert!(p1.view.pitch.abs() < 1e-3, "p1 pitch should be unchanged: {}", p1.view.pitch);
        assert!(p2.view.yaw.abs() < 1e-3, "p2 yaw should be unchanged: {}", p2.view.yaw);
    }

    /// LocalPlayer carries its BodyId — that's the indirection that
    /// keeps body data in physics where it belongs.
    #[test]
    fn local_player_keeps_body_id() {
        let id = BodyId(42);
        let lp = LocalPlayer::new(id);
        assert_eq!(lp.body_id, id);
    }

    /// LocalPlayers can have different controller settings (per-player
    /// preferences). The default works but the field is mutable.
    #[test]
    fn controllers_can_differ_per_player() {
        let mut p1 = LocalPlayer::new(BodyId(1));
        let mut p2 = LocalPlayer::new(BodyId(2));
        p1.controller.move_speed = 8.0;  // slower
        p2.controller.move_speed = 32.0; // faster
        // After the same input, p2 should have moved further.
        p1.controller.apply_move(&mut p1.view,
            nalgebra_glm::vec3(1.0, 0.0, 0.0), 0.1);
        p2.controller.apply_move(&mut p2.view,
            nalgebra_glm::vec3(1.0, 0.0, 0.0), 0.1);
        assert!(p2.view.position.x > p1.view.position.x,
            "p2 should have moved further: p1={} p2={}",
            p1.view.position.x, p2.view.position.x);
    }

    /// at_position constructor places the view at the requested
    /// spawn point — used by the bin's setup path.
    #[test]
    fn at_position_seeds_view() {
        let p = LocalPlayer::at_position(BodyId::LOCAL_PLAYER,
            nalgebra_glm::vec3(100.0, 70.0, 200.0));
        assert!((p.view.position - nalgebra_glm::vec3(100.0, 70.0, 200.0)).norm() < 1e-5,
            "view should be at requested position, got {:?}", p.view.position);
        assert_eq!(p.body_id, BodyId::LOCAL_PLAYER);
    }

    /// The convenience methods on LocalPlayer match calling through
    /// the controller. Lets call sites pick the shorter API.
    #[test]
    fn convenience_methods_match_controller_calls() {
        let mut p1 = LocalPlayer::new(BodyId(1));
        let mut p2 = LocalPlayer::new(BodyId(2));
        // p1 uses the convenience method, p2 calls through the controller
        // — they should produce the same result.
        p1.apply_mouse(50.0, -25.0);
        p2.controller.apply_mouse(&mut p2.view, 50.0, -25.0);
        assert!((p1.view.yaw - p2.view.yaw).abs() < 1e-5,
            "yaw mismatch: p1={} p2={}", p1.view.yaw, p2.view.yaw);
        assert!((p1.view.pitch - p2.view.pitch).abs() < 1e-5,
            "pitch mismatch: p1={} p2={}", p1.view.pitch, p2.view.pitch);
    }

    /// LocalPlayer carries Permissions. Default is full-admin so
    /// single-player code paths just work.
    #[test]
    fn default_permissions_are_full_admin() {
        let p = LocalPlayer::new(BodyId(1));
        assert!(p.permissions.can_break_block);
        assert!(p.permissions.can_place_block);
        assert!(p.permissions.can_toggle_flight);
        assert!(p.permissions.can_light_fire);
    }

    /// Permissions can be replaced wholesale (e.g. switching to
    /// spectator mode). The body_id and view are unaffected.
    #[test]
    fn permissions_can_be_replaced() {
        use crate::permissions::Permissions;
        let mut p = LocalPlayer::new(BodyId(7));
        p.permissions = Permissions::spectator();
        assert!(!p.permissions.can_break_block);
        assert!(!p.permissions.can_place_block);
        // Other fields untouched.
        assert_eq!(p.body_id, BodyId(7));
    }
}
