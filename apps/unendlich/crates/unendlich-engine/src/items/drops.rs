//! Item drops — items that exist in the world as fallen entities until a
//! player walks over them. A broken block spawns one; it falls under gravity,
//! settles on solid terrain, ages out after a while, and is collected by
//! proximity.
//!
//! Decoupling: physics needs to know what's solid, so [`DropField::update`]
//! takes a `solid(x, y, z)` closure rather than depending on the chunk store —
//! the same seam the sim uses with `WorldQuery`. The game wires it to the real
//! terrain.
//!
//! These are **presentation** entities and use float math, so (like the
//! renderer) they are not rolled back. If drops ever need to be networked,
//! they'd move into the fixed-point sim as entities; the spawn/settle/pickup
//! shape here carries over directly.

use super::ItemStack;

/// One fallen item in the world.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Drop {
    pub stack: ItemStack,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub age_ticks: u32,
}

/// All active drops plus collection/despawn tuning.
#[derive(Clone)]
pub struct DropField {
    drops: Vec<Drop>,
    /// A player within this distance collects a drop.
    pub pickup_radius: f32,
    /// Drops older than this many ticks vanish.
    pub despawn_ticks: u32,
    /// Horizontal velocity retained per tick (0..1); the rest is drag.
    pub horizontal_drag: f32,
}

impl DropField {
    pub fn new() -> DropField {
        DropField {
            drops: Vec::new(),
            pickup_radius: 1.5,
            despawn_ticks: 60 * 60, // ~60s at 60 Hz
            horizontal_drag: 0.85,
        }
    }

    /// Spawn a drop, e.g. at a broken block's centre with a little pop.
    pub fn spawn(&mut self, stack: ItemStack, pos: [f32; 3], vel: [f32; 3]) {
        self.drops.push(Drop { stack, pos, vel, age_ticks: 0 });
    }

    pub fn drops(&self) -> &[Drop] {
        &self.drops
    }
    pub fn len(&self) -> usize {
        self.drops.len()
    }
    pub fn is_empty(&self) -> bool {
        self.drops.is_empty()
    }

    /// Advance every drop one tick: gravity, horizontal drag, settle on top of
    /// solid terrain, and age-out. `solid(x, y, z)` reports voxel solidity;
    /// `dt` is the timestep in seconds, `gravity` blocks/s² (negative = down).
    pub fn update<F: Fn(i32, i32, i32) -> bool>(&mut self, dt: f32, gravity: f32, solid: F) {
        let despawn = self.despawn_ticks;
        let drag = self.horizontal_drag;
        self.drops.retain_mut(|d| {
            d.vel[1] += gravity * dt;
            d.vel[0] *= drag;
            d.vel[2] *= drag;

            let nx = d.pos[0] + d.vel[0] * dt;
            let nz = d.pos[2] + d.vel[2] * dt;
            let ny = d.pos[1] + d.vel[1] * dt;
            d.pos[0] = nx;
            d.pos[2] = nz;

            // Settle: if falling into a solid voxel, rest on its top face.
            let bx = nx.floor() as i32;
            let bz = nz.floor() as i32;
            let by = ny.floor() as i32;
            if d.vel[1] < 0.0 && solid(bx, by, bz) {
                d.pos[1] = (by + 1) as f32;
                d.vel[1] = 0.0;
            } else {
                d.pos[1] = ny;
            }

            d.age_ticks = d.age_ticks.saturating_add(1);
            d.age_ticks < despawn
        });
    }

    /// r448: items in water — buoyancy floats a submerged drop gently toward the surface and the
    /// current carries it along. Call after `update`. `in_water(x,y,z)` reports water voxels;
    /// `current(x,z)` gives the local flow vector. `buoyancy` (blocks/s² up) should exceed the
    /// gravity passed to `update` so a submerged item rises; vertical speed is damped + capped so it
    /// bobs at the surface rather than launching. `drift` couples the current into horizontal motion.
    pub fn apply_water<W, C>(&mut self, dt: f32, buoyancy: f32, drift: f32, in_water: W, current: C)
    where
        W: Fn(i32, i32, i32) -> bool,
        C: Fn(i32, i32) -> [f32; 2],
    {
        for d in &mut self.drops {
            let bx = d.pos[0].floor() as i32;
            let by = d.pos[1].floor() as i32;
            let bz = d.pos[2].floor() as i32;
            if !in_water(bx, by, bz) {
                continue;
            }
            // buoyancy up, damped + capped so it eases to the surface and bobs
            d.vel[1] += buoyancy * dt;
            d.vel[1] *= 0.80;
            if d.vel[1] > 1.5 {
                d.vel[1] = 1.5;
            }
            // current drift into horizontal velocity, with extra in-water drag
            let f = current(bx, bz);
            d.vel[0] += f[0] * drift * dt;
            d.vel[2] += f[1] * drift * dt;
            d.vel[0] *= 0.92;
            d.vel[2] *= 0.92;
        }
    }

    /// Remove and return the stacks of all drops within `pickup_radius` of
    /// `player`. The caller adds them to an inventory.
    pub fn collect_near(&mut self, player: [f32; 3]) -> Vec<ItemStack> {
        let r2 = self.pickup_radius * self.pickup_radius;
        let mut collected = Vec::new();
        self.drops.retain(|d| {
            let dx = d.pos[0] - player[0];
            let dy = d.pos[1] - player[1];
            let dz = d.pos[2] - player[2];
            if dx * dx + dy * dy + dz * dz <= r2 {
                collected.push(d.stack);
                false
            } else {
                true
            }
        });
        collected
    }

    /// Drop everything (e.g. on world unload).
    pub fn clear(&mut self) {
        self.drops.clear();
    }
}

impl Default for DropField {
    fn default() -> DropField {
        DropField::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::ItemId;

    fn stack() -> ItemStack {
        ItemStack::new(ItemId(1), 1)
    }

    // Solid floor: voxels at y < 1 are solid, so the surface top is y = 1.
    fn floor(_x: i32, y: i32, _z: i32) -> bool {
        y < 1
    }

    #[test]
    fn a_drop_falls_and_settles_on_the_surface() {
        let mut f = DropField::new();
        f.spawn(stack(), [0.5, 8.0, 0.5], [0.0, 0.0, 0.0]);
        for _ in 0..600 {
            f.update(1.0 / 60.0, -20.0, floor);
        }
        let d = f.drops()[0];
        assert!((d.pos[1] - 1.0).abs() < 1e-3, "rests on the surface (y=1), got {}", d.pos[1]);
        assert_eq!(d.vel[1], 0.0, "vertical velocity zeroed on landing");
    }

    #[test]
    fn collect_takes_only_nearby_drops() {
        let mut f = DropField::new();
        f.spawn(stack(), [0.0, 1.0, 0.0], [0.0; 3]); // near
        f.spawn(stack(), [50.0, 1.0, 50.0], [0.0; 3]); // far
        let got = f.collect_near([0.3, 1.0, 0.3]);
        assert_eq!(got.len(), 1, "only the near drop is collected");
        assert_eq!(f.len(), 1, "the far drop remains");
    }

    #[test]
    fn drops_despawn_after_their_lifetime() {
        let mut f = DropField::new();
        f.despawn_ticks = 10;
        f.spawn(stack(), [0.5, 1.0, 0.5], [0.0; 3]);
        for _ in 0..9 {
            f.update(1.0 / 60.0, -20.0, floor);
        }
        assert_eq!(f.len(), 1, "still alive at tick 9");
        for _ in 0..2 {
            f.update(1.0 / 60.0, -20.0, floor);
        }
        assert!(f.is_empty(), "despawned past its lifetime");
    }

    #[test]
    fn horizontal_velocity_drags_to_rest() {
        let mut f = DropField::new();
        f.spawn(stack(), [0.5, 1.0, 0.5], [5.0, 0.0, 0.0]);
        for _ in 0..120 {
            f.update(1.0 / 60.0, -20.0, floor);
        }
        let d = f.drops()[0];
        assert!(d.vel[0].abs() < 0.1, "horizontal motion dragged to near zero, got {}", d.vel[0]);
    }

    #[test]
    fn submerged_drop_floats_up() {
        let mut f = DropField::new();
        f.spawn(stack(), [0.5, 1.0, 0.5], [0.0, 0.0, 0.0]);
        let in_water = |_x: i32, _y: i32, _z: i32| true; // everywhere is water
        let no_flow = |_x: i32, _z: i32| [0.0f32, 0.0];
        let y0 = f.drops()[0].pos[1];
        for _ in 0..30 {
            f.update(1.0 / 60.0, -18.0, |_x, _y, _z| false); // no solid floor
            f.apply_water(1.0 / 60.0, 24.0, 4.0, in_water, no_flow);
        }
        assert!(f.drops()[0].pos[1] > y0, "buoyancy raised the drop: {} > {}", f.drops()[0].pos[1], y0);
    }

    #[test]
    fn current_carries_a_floating_drop() {
        let mut f = DropField::new();
        f.spawn(stack(), [0.5, 1.0, 0.5], [0.0, 0.0, 0.0]);
        let in_water = |_x: i32, _y: i32, _z: i32| true;
        let flow_px = |_x: i32, _z: i32| [2.0f32, 0.0]; // current toward +x
        let x0 = f.drops()[0].pos[0];
        for _ in 0..40 {
            f.update(1.0 / 60.0, -18.0, |_x, _y, _z| false);
            f.apply_water(1.0 / 60.0, 24.0, 4.0, in_water, flow_px);
        }
        assert!(f.drops()[0].pos[0] > x0 + 0.2, "current pushed the drop +x: {}", f.drops()[0].pos[0]);
    }

    #[test]
    fn dry_drop_unaffected_by_water_pass() {
        let mut f = DropField::new();
        f.spawn(stack(), [0.5, 5.0, 0.5], [0.0, 0.0, 0.0]);
        let no_water = |_x: i32, _y: i32, _z: i32| false;
        let flow = |_x: i32, _z: i32| [9.0f32, 9.0];
        f.apply_water(1.0 / 60.0, 24.0, 4.0, no_water, flow);
        let d = f.drops()[0];
        assert_eq!(d.vel, [0.0, 0.0, 0.0], "drop not in water is untouched");
    }
}
