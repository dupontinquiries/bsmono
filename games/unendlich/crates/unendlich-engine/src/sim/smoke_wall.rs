//! `sim::smoke_wall` — a dragon's giant wall of smoke, and what it does to
//! eyes on both sides of it.
//!
//! From the design library: `mob_brain_object_permanence` §8 (fog/smoke vs a
//! dragon's eyes: Beer-Lambert along the actual sight ray; *"anti-dragon
//! cover is volumetric — a smoke screen must be tall"*; per-creature fog
//! sensitivity, hatchling 1.5 … apex 0.5, Air dragons 0.3 because they read
//! the air) and `handoff/BIN_PATCHES_bending_ai.md` §3 (Fire's ash
//! smokescreen as an obscurant brush).
//!
//! A [`SmokeVolume`] is a soft-edged box: long, thick, and **tall** (the
//! Volume Rule), standing across a line. It drifts with the wind, rises, and
//! thins over its life. [`transmittance`] marches a sight ray through every
//! volume (Beer-Lambert, `exp(-σ·∫ρ ds)`); below [`SIGHT_FLOOR`] (the doc's
//! 0.30 stoop-commit threshold) the eyes cannot hold the target. That cuts
//! both ways: a player behind the wall is hidden from the dragon — and a
//! dragon inside it is hidden from the player, hunting by nose and ear
//! (`dragon_nose`, `dragon_ears`), which smoke does not stop.
//!
//! Pure.

/// Below this transmittance a target cannot be held by eye.
pub const SIGHT_FLOOR: f32 = 0.30;
/// Extinction per unit density per metre.
pub const SIGMA: f32 = 0.35;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmokeVolume {
    pub center: [f32; 3],
    /// Unit direction the wall runs ALONG (ground plane).
    pub along: [f32; 2],
    /// Half-length along, half-thickness across, half-height.
    pub half: [f32; 3],
    /// Peak density when fresh.
    pub density: f32,
    pub age: f32,
    pub life: f32,
}

impl SmokeVolume {
    /// A wall standing across the line from `from` toward `toward`, centred
    /// at `center` (on the ground), `length` long and `height` tall.
    pub fn wall(center: [f32; 3], from: [f32; 3], toward: [f32; 3], length: f32, height: f32, life: f32) -> SmokeVolume {
        let (dx, dz) = (toward[0] - from[0], toward[2] - from[2]);
        let l = (dx * dx + dz * dz).sqrt().max(1e-3);
        let across = [dx / l, dz / l];
        SmokeVolume {
            center: [center[0], center[1] + height * 0.5, center[2]],
            along: [-across[1], across[0]],
            half: [length * 0.5, 3.0, height * 0.5],
            density: 1.0,
            age: 0.0,
            life,
        }
    }

    pub fn alive(&self) -> bool {
        self.age < self.life
    }

    /// Advance: drift with the wind (m/s), rise slowly, age.
    pub fn step(&mut self, wind: [f32; 2], dt: f32) {
        let dt = if dt.is_finite() { dt.clamp(0.0, 1.0) } else { 0.0 };
        let w = |v: f32| if v.is_finite() { v.clamp(-30.0, 30.0) } else { 0.0 };
        self.center[0] += w(wind[0]) * 0.6 * dt;
        self.center[2] += w(wind[1]) * 0.6 * dt;
        self.center[1] += 0.4 * dt;
        // It spreads as it thins.
        self.half[1] += 0.25 * dt;
        self.age += dt;
    }

    /// Density at `p`: full inside, soft 2 m edges, fading over the last
    /// half of its life and thinning as it spreads.
    pub fn density_at(&self, p: [f32; 3]) -> f32 {
        if !self.alive() {
            return 0.0;
        }
        let (dx, dz) = (p[0] - self.center[0], p[2] - self.center[2]);
        let a = (dx * self.along[0] + dz * self.along[1]).abs();
        let c = (-dx * self.along[1] + dz * self.along[0]).abs();
        let y = (p[1] - self.center[1]).abs();
        let soft = |d: f32, h: f32| ((h + 2.0 - d) / 2.0).clamp(0.0, 1.0);
        let shape = soft(a, self.half[0]) * soft(c, self.half[1]) * soft(y, self.half[2]);
        let u = self.age / self.life.max(1e-3);
        let fade = if u < 0.5 { 1.0 } else { (1.0 - (u - 0.5) * 2.0).max(0.0) };
        self.density * shape * fade * (3.0 / self.half[1].max(3.0))
    }
}

/// Density of all volumes at `p`.
pub fn density_at(vols: &[SmokeVolume], p: [f32; 3]) -> f32 {
    vols.iter().map(|v| v.density_at(p)).sum()
}

/// Fraction of light that survives from `from` to `to` through the smoke,
/// for an eye of `sensitivity` (1 baseline; < 1 sees through more).
/// Marches 1 m steps, at most 160.
pub fn transmittance(vols: &[SmokeVolume], from: [f32; 3], to: [f32; 3], sensitivity: f32) -> f32 {
    if vols.is_empty() {
        return 1.0;
    }
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if !len.is_finite() || len < 1e-3 {
        return 1.0;
    }
    let n = (len.ceil() as usize).clamp(1, 160);
    let step = len / n as f32;
    let mut depth = 0.0f32;
    for i in 0..n {
        let t = (i as f32 + 0.5) / n as f32;
        let p = [from[0] + d[0] * t, from[1] + d[1] * t, from[2] + d[2] * t];
        depth += density_at(vols, p) * step;
    }
    (-SIGMA * sensitivity.max(0.0) * depth).exp()
}

/// Per-dragon smoke sensitivity (the doc's balance scalar): small dragons are
/// more hampered, the largest least; Air dragons read the air (× 0.3).
pub fn sensitivity(size_class: u8, size_class_max: u8, element: u8) -> f32 {
    let t = size_class as f32 / size_class_max.max(1) as f32;
    let base = 1.5 - 1.0 * t; // 1.5 hatchling-size .. 0.5 apex
    if element % 6 == 2 { base * 0.3 } else { base }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wall at x = 20 between a dragon at the origin and a player at x = 40.
    fn wall() -> SmokeVolume {
        SmokeVolume::wall([20.0, 64.0, 0.0], [0.0, 64.0, 0.0], [40.0, 64.0, 0.0], 30.0, 20.0, 30.0)
    }

    #[test]
    fn a_wall_blocks_sight_across_it_not_beside_it() {
        let v = [wall()];
        let across = transmittance(&v, [0.0, 66.0, 0.0], [40.0, 66.0, 0.0], 1.0);
        assert!(across < SIGHT_FLOOR * 0.25, "across {across}");
        // A sightline that goes around the end of the wall.
        let around = transmittance(&v, [0.0, 66.0, 25.0], [40.0, 66.0, 25.0], 1.0);
        assert!(around > 0.95, "around {around}");
        assert_eq!(transmittance(&[], [0.0; 3], [9.0; 3], 1.0), 1.0);
    }

    #[test]
    fn it_is_tall_enough_to_hide_you_from_above() {
        // The Volume Rule: a dragon hovering high and looking down across it.
        let v = [wall()];
        let from_above = transmittance(&v, [0.0, 110.0, 0.0], [22.0, 64.5, 0.0], 1.0);
        assert!(from_above < SIGHT_FLOOR, "from above {from_above}");
        // A low film of the same density does not.
        let mut film = wall();
        film.half[2] = 0.5;
        film.center[1] = 64.5;
        let through_film = transmittance(&[film], [0.0, 110.0, 0.0], [22.0, 64.5, 0.0], 1.0);
        assert!(through_film > SIGHT_FLOOR, "film {through_film}");
    }

    #[test]
    fn it_drifts_rises_and_thins_away() {
        let mut v = wall();
        let c0 = v.center;
        for _ in 0..100 {
            v.step([3.0, 0.0], 0.1);
        }
        assert!(v.center[0] > c0[0] + 10.0 && v.center[1] > c0[1]);
        let young = transmittance(&[wall()], [0.0, 66.0, 0.0], [40.0, 66.0, 0.0], 1.0);
        let mut old = wall();
        old.age = old.life * 0.9;
        let aged = transmittance(&[old], [0.0, 66.0, 0.0], [40.0, 66.0, 0.0], 1.0);
        assert!(aged > young * 5.0 && aged > SIGHT_FLOOR, "aged {aged} young {young}");
        old.age = old.life + 1.0;
        assert!(!old.alive() && old.density_at(old.center) == 0.0);
    }

    #[test]
    fn apex_and_air_dragons_see_through_more() {
        assert!((sensitivity(0, 4, 4) - 1.5).abs() < 1e-5);
        assert!((sensitivity(4, 4, 4) - 0.5).abs() < 1e-5);
        assert!(sensitivity(2, 4, 2) < sensitivity(2, 4, 4) * 0.5);
        // Thin edge of a wall: a hatchling loses you, an Air dragon does not.
        let mut v = wall();
        v.age = v.life * 0.8;
        let path = ([0.0, 66.0, 0.0], [40.0, 66.0, 0.0]);
        let hatchling = transmittance(&[v], path.0, path.1, sensitivity(0, 4, 4));
        let air = transmittance(&[v], path.0, path.1, sensitivity(0, 4, 2));
        assert!(hatchling < SIGHT_FLOOR && air > SIGHT_FLOOR, "hatchling {hatchling} air {air}");
    }

    #[test]
    fn garbage_is_safe() {
        let v = [wall()];
        assert!(transmittance(&v, [f32::NAN; 3], [1.0; 3], 1.0).is_finite());
        let mut w = wall();
        w.step([f32::NAN, f32::INFINITY], f32::NAN);
        assert!(w.center.iter().all(|x| x.is_finite()));
    }
}
