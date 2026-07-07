//! pickups.rs — collectible items that bob, spin, and glow on the floor: health, armor, and ammo. The
//! Z.ai demo had none of these; they round out the combat loop (you push forward to grab the green cross
//! before the next wave). Each pickup renders as a glowing floor pad plus a spinning emissive icon, all
//! ordinary `Mesh3` through the same renderer — using the logscope `disc`, `rotate_y_about`, and the
//! per-mesh emissive term.

use crate::level::{Level, Rng};
use crate::math::{Vec2, Vec3};
use crate::mesh::{boxx, disc, rgb, rotate_y_about, Mesh3, FACE_ALL};
use crate::player::Player;
use egui::Color32;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PickupKind {
    Health,
    Armor,
    Ammo,
}

impl PickupKind {
    pub fn color(self) -> Color32 {
        match self {
            PickupKind::Health => rgb(70, 230, 90),
            PickupKind::Armor => rgb(96, 156, 255),
            PickupKind::Ammo => rgb(255, 206, 96),
        }
    }
}

#[derive(Clone)]
pub struct Pickup {
    pub kind: PickupKind,
    /// Floor point the pickup sits on.
    pub pos: Vec3,
    /// Animation clock (bob + spin).
    pub t: f32,
    pub alive: bool,
}

impl Pickup {
    pub fn new(kind: PickupKind, pos: Vec3, phase: f32) -> Self {
        Self { kind, pos, t: phase, alive: true }
    }

    /// Advance the bob/spin animation.
    pub fn update(&mut self, dt: f32) {
        self.t += dt;
    }

    fn bob(&self) -> f32 {
        (self.t * 2.2).sin() * 0.12
    }

    /// If `player` is standing on it and can actually use the item, apply the effect and consume it.
    /// Returns true if it was just collected (so the caller can spawn a sparkle).
    pub fn try_collect(&mut self, player: &mut Player) -> bool {
        if !self.alive || !player.alive {
            return false;
        }
        let feet = Vec2::new(player.pos.x, player.pos.z);
        let here = Vec2::new(self.pos.x, self.pos.z);
        if feet.dist(here) > player.radius + 0.7 {
            return false;
        }
        if (player.pos.y - self.pos.y).abs() > 2.2 {
            return false;
        }
        let used = match self.kind {
            PickupKind::Health => player.heal(25.0),
            PickupKind::Armor => player.add_armor(35.0),
            PickupKind::Ammo => player.add_ammo(),
        };
        if used {
            self.alive = false;
        }
        used
    }

    /// The glowing pad + the spinning icon, ready to drop into the frame's mesh list.
    pub fn build(&self) -> Vec<Mesh3> {
        let col = self.kind.color();
        let center = Vec3::new(self.pos.x, self.pos.y + 0.62 + self.bob(), self.pos.z);
        let spin = self.t * 1.7;

        // floor pad — a soft glow ring under the item
        let mut pad = disc(Vec3::new(self.pos.x, self.pos.y + 0.04, self.pos.z), 0.5, 18, col);
        pad.emissive = 0.45;

        // icon
        let mut icon = match self.kind {
            PickupKind::Health => {
                let mut m = boxx(center, Vec3::new(0.10, 0.30, 0.10), col, FACE_ALL);
                m.merge(&boxx(center, Vec3::new(0.30, 0.10, 0.10), col, FACE_ALL));
                m
            }
            PickupKind::Armor => boxx(center, Vec3::new(0.24, 0.24, 0.24), col, FACE_ALL),
            PickupKind::Ammo => boxx(center, Vec3::new(0.28, 0.20, 0.18), col, FACE_ALL),
        };
        rotate_y_about(&mut icon, center, spin);
        icon.emissive = 0.7;

        vec![pad, icon]
    }
}

/// Scatter a few ammo crates across rooms (skipping the spawn room) at level start. Health regenerates,
/// so the only thing worth grabbing is resupply.
pub fn populate(level: &Level, seed: u64) -> Vec<Pickup> {
    let mut rng = Rng::new(seed ^ 0x91C4_5EED);
    let mut out = Vec::new();
    for room in level.rooms.iter().skip(1) {
        if !rng.chance(0.5) {
            continue;
        }
        let pos = room.center.with_y(level.floor_y);
        out.push(Pickup::new(PickupKind::Ammo, pos, rng.f32() * 6.28));
    }
    out
}

/// Roll a possible ammo drop when an enemy dies. `None` = nothing.
pub fn roll_drop(rng: &mut Rng) -> Option<PickupKind> {
    if rng.f32() < 0.22 {
        Some(PickupKind::Ammo)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::MapConfig;

    fn level() -> Level {
        Level::generate(MapConfig::default())
    }

    #[test]
    fn health_pickup_heals_and_is_consumed_when_hurt() {
        let l = level();
        let mut p = Player::new(&l);
        p.health = 40.0;
        let mut pk = Pickup::new(PickupKind::Health, p.pos.with_y(l.floor_y), 0.0);
        assert!(pk.try_collect(&mut p));
        assert!(p.health > 40.0);
        assert!(!pk.alive);
    }

    #[test]
    fn full_health_player_leaves_the_pickup() {
        let l = level();
        let mut p = Player::new(&l); // spawns at 100 health
        let mut pk = Pickup::new(PickupKind::Health, p.pos.with_y(l.floor_y), 0.0);
        assert!(!pk.try_collect(&mut p));
        assert!(pk.alive); // still there for later
    }

    #[test]
    fn distant_pickup_is_not_collected() {
        let l = level();
        let mut p = Player::new(&l);
        p.health = 10.0;
        let far = p.pos.with_y(l.floor_y) + Vec3::new(20.0, 0.0, 0.0);
        let mut pk = Pickup::new(PickupKind::Health, far, 0.0);
        assert!(!pk.try_collect(&mut p));
        assert!(pk.alive);
    }

    #[test]
    fn pickup_builds_pad_and_icon() {
        let pk = Pickup::new(PickupKind::Ammo, Vec3::ZERO, 0.0);
        let meshes = pk.build();
        assert_eq!(meshes.len(), 2);
        assert!(meshes.iter().all(|m| m.tri_count() > 0));
    }
}
