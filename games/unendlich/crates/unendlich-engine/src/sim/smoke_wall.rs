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
    /// Half-thickness when it was made: as it spreads it thins by
    /// `across0 / half[1]` (a fixed "3" here diluted big puffs from birth).
    pub across0: f32,
    /// The dragon whose smoke this is (0 = nobody's), so its wisps can wear
    /// that dragon's feelings (`smoke_wisps`).
    pub owner: u64,
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
            across0: 3.0,
            owner: 0,
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
        self.density * shape * fade * (self.across0 / self.half[1].max(self.across0).max(1e-3))
    }
}

/// A dragon that **smokes**: while active, it breathes out a puff every
/// `spacing` metres (or `interval` seconds) wherever it is, so its cover moves
/// with it. Nobody can camp in a dragon's smoke -- it takes the cover along,
/// and the puffs it leaves behind thin out in `puff_life` seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mantle {
    /// Seconds of smoking left.
    pub left: f32,
    /// Puff size: horizontal radius and height (m) -- a large dragon smokes big.
    pub radius: f32,
    pub height: f32,
    pub puff_life: f32,
    last: Option<([f32; 3], f32)>,
    t: f32,
    n: u64,
}

impl Mantle {
    pub fn new(duration: f32, body_radius: f32) -> Mantle {
        Mantle {
            left: duration.max(0.0),
            radius: (body_radius * 2.0).clamp(4.0, 14.0),
            height: (body_radius * 4.0).clamp(10.0, 30.0),
            puff_life: 10.0,
            last: None,
            t: 0.0,
            n: 0,
        }
    }

    pub fn active(&self) -> bool {
        self.left > 0.0
    }

    /// Advance; returns a new puff to add when one is due.
    pub fn emit(&mut self, dragon_ground: [f32; 3], dt: f32) -> Option<SmokeVolume> {
        let dt = if dt.is_finite() { dt.clamp(0.0, 1.0) } else { 0.0 };
        self.t += dt;
        if !self.active() || !dragon_ground.iter().all(|v| v.is_finite()) {
            self.left = (self.left - dt).max(0.0);
            return None;
        }
        self.left = (self.left - dt).max(0.0);
        let due = match self.last {
            None => true,
            Some((p, t)) => {
                let moved = ((p[0] - dragon_ground[0]).powi(2) + (p[2] - dragon_ground[2]).powi(2)).sqrt();
                moved >= self.radius * 0.6 || self.t - t >= 0.8
            }
        };
        if !due {
            return None;
        }
        self.last = Some((dragon_ground, self.t));
        // Puffs land scattered around it, not centred on it: the cloud's shape
        // must not point at where the dragon is.
        self.n += 1;
        let j = |i: u64| {
            let mut z = self.n.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ i.wrapping_mul(0xD1B5_4A32_D192_ED03);
            z = (z ^ (z >> 29)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            ((z ^ (z >> 32)) >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
        };
        let off = [j(1) * self.radius * 1.3, j(2) * self.radius * 1.3];
        Some(SmokeVolume {
            center: [dragon_ground[0] + off[0], dragon_ground[1] + self.height * 0.5, dragon_ground[2] + off[1]],
            along: [1.0, 0.0],
            half: [self.radius, self.radius, self.height * 0.5],
            density: 0.8,
            age: 0.0,
            life: self.puff_life,
            across0: self.radius,
            owner: 0,
        })
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

    /// The cover moves with the dragon: where it is now is smoked; where it
    /// was ten seconds ago is clear again -- no camping in its smoke.
    #[test]
    fn a_smoking_dragon_carries_its_cover() {
        let mut m = Mantle::new(20.0, 4.0);
        let mut vols: Vec<SmokeVolume> = Vec::new();
        let mut pos = [0.0f32, 64.0, 0.0];
        for _ in 0..(15.0 / 0.1) as usize {
            pos[0] += 4.0 * 0.1; // it moves at 4 m/s
            for v in vols.iter_mut() {
                v.step([0.0, 0.0], 0.1);
            }
            vols.retain(|v| v.alive());
            if let Some(p) = m.emit(pos, 0.1) {
                vols.push(p);
            }
        }
        let here = density_at(&vols, [pos[0], 66.0, 0.0]);
        let start = density_at(&vols, [0.0, 66.0, 0.0]);
        assert!(here > 0.5, "the dragon stands in its smoke: {here}");
        assert!(start < 0.05, "where it began is clear again: {start}");
        // An eye 30 m off cannot see it through its own cloud…
        let t = transmittance(&vols, [pos[0] + 30.0, 66.0, 0.0], [pos[0], 66.0, 0.0], 1.0);
        assert!(t < SIGHT_FLOOR, "t {t}");
        // …and it stops smoking when the mantle runs out.
        let mut spent = Mantle::new(0.5, 4.0);
        let n = (0..50).filter_map(|_| spent.emit([0.0, 64.0, 0.0], 0.1)).count();
        assert!(n <= 1);
    }

    #[test]
    fn puffs_scatter_around_it_rather_than_marking_its_spot() {
        let mut m = Mantle::new(30.0, 4.0);
        let offs: Vec<f32> = (0..200)
            .filter_map(|i| m.emit([i as f32 * 0.5, 64.0, 0.0], 0.1).map(|p| (p.center[2]).abs() + (p.center[0] - i as f32 * 0.5).abs()))
            .collect();
        assert!(offs.len() > 10);
        let mean = offs.iter().sum::<f32>() / offs.len() as f32;
        assert!(mean > m.radius * 0.5, "puffs sit {mean:.1} m from it on average");
    }

    #[test]
    fn a_fast_dragon_does_not_outrun_its_smoke() {
        let mut m = Mantle::new(10.0, 4.0);
        let mut last: Option<f32> = None;
        let mut x = 0.0f32;
        for _ in 0..100 {
            x += 15.0 * 0.05; // 15 m/s
            if let Some(p) = m.emit([x, 64.0, 0.0], 0.05) {
                if let Some(l) = last {
                    assert!(p.center[0] - l <= m.radius * 0.6 + 0.8 + 2.0 * 1.3 * m.radius, "gap {}", p.center[0] - l);
                }
                last = Some(p.center[0]);
            }
        }
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
