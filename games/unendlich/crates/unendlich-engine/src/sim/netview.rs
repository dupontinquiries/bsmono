//! netview — expose the rollback simulation as a web snapshot for the shared **webhost-kit**.
//!
//! This is unendlich's game-specific half of the socket-adapter integration: map the deterministic
//! [`Rollback`] state to a JSON view the Python browser/LAN host can spectate. The transport + protocol
//! live in the shared `netview` crate (same one soldier uses); we only supply the mapping.
//!
//! Because this is a *rollback* game, the snapshot surfaces the netcode state too — the confirmed vs
//! current tick (how far prediction has outrun confirmed input) and the desync `checksum` — which makes
//! the browser view a genuinely useful rollback inspector, not just a spectator camera.

use serde::Serialize;

use super::rollback::Rollback;

/// A character/mob as the browser needs it: position (world units), heading, and hp where it applies.
#[derive(Serialize, Clone, Debug)]
pub struct Actor {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hp: Option<i32>,
}

/// The live rollback snapshot browsers render.
#[derive(Serialize, Clone, Debug)]
pub struct SnapshotView {
    pub ok: bool,
    pub title: &'static str,
    pub tick: u64,
    /// First tick whose inputs aren't all confirmed — everything before is final.
    pub confirmed_tick: u64,
    /// How many ticks the visible state is predicted ahead of the confirmed baseline.
    pub predicted_ticks: u64,
    /// Desync checksum of the current state (matches across peers when in sync).
    pub checksum: u64,
    pub players: Vec<Actor>,
    pub mobs: Vec<Actor>,
}

/// Build a browser snapshot from the rollback engine — the whole game-specific part of the integration.
pub fn snapshot(rb: &Rollback) -> SnapshotView {
    let st = rb.state();
    let players = st
        .players
        .iter()
        .map(|c| Actor {
            x: c.pos.x.to_f32(),
            y: c.pos.y.to_f32(),
            z: c.pos.z.to_f32(),
            yaw: c.yaw as f32,
            hp: None,
        })
        .collect();
    let mobs = st
        .mobs
        .iter()
        .map(|m| Actor {
            x: m.body.pos.x.to_f32(),
            y: m.body.pos.y.to_f32(),
            z: m.body.pos.z.to_f32(),
            yaw: m.body.yaw as f32,
            hp: Some(m.health as i32),
        })
        .collect();
    SnapshotView {
        ok: true,
        title: "unendlich",
        tick: st.tick.0,
        confirmed_tick: rb.confirmed_tick(),
        predicted_ticks: rb.current_tick().saturating_sub(rb.confirmed_tick()),
        checksum: rb.checksum(),
        players,
        mobs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::physics::{BlockPhysics, WorldQuery};
    use crate::sim::state::{Input, SimState};

    struct Flat;
    impl WorldQuery for Flat {
        fn physics_at(&self, _x: i32, y: i32, _z: i32) -> BlockPhysics {
            if y < 0 {
                BlockPhysics::solid(crate::sim::fixed::Fixed::from_frac(6, 10))
            } else {
                BlockPhysics::AIR
            }
        }
    }

    #[test]
    fn snapshot_reflects_rollback_state_and_advances() {
        let world = Flat;
        let mut rb = Rollback::new(2, SimState::new(2), 8);
        let s0 = snapshot(&rb);
        assert!(s0.ok && s0.title == "unendlich" && s0.players.len() == 2 && s0.tick == 0);
        // advance a few confirmed ticks; tick + confirmed_tick move, serialization is stable JSON
        for _ in 0..5 {
            let t = rb.current_tick();
            for p in 0..2 {
                rb.set_input(t, p, Input::default(), &world);
            }
            rb.advance(&world);
        }
        let s1 = snapshot(&rb);
        assert!(s1.tick >= 5, "tick advanced: {}", s1.tick);
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&s1).unwrap()).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["players"].as_array().unwrap().len(), 2);
        assert!(v.get("checksum").is_some() && v.get("confirmed_tick").is_some());
    }
}
