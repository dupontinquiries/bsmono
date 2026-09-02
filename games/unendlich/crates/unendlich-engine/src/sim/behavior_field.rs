//! BEHAVIOR FIELD — the creatures' senses as a grid, built once per tick,
//! sampled per creature. The design behind "advanced GPU fields for their
//! behaviours": not dumber, not fewer — the same decisions read off a
//! field instead of re-derived by every brain.
//!
//! What the roamer loop does per creature per tick is scan the world:
//! `block_at` rings for food, `surface_y_at` columns for footing, every
//! other creature for predators and herd. Each scan is O(radius) and
//! O(creatures), and they are the unattributed 0.6 ms a creature costs
//! (perfbisect, 2026-09-01). The same information for every creature in
//! the loaded area can be computed ONCE, as a coarse grid over the ground
//! plane with a few channels — food, threat, herd, height, water — and
//! then a creature's whole perception is a handful of array reads and a
//! gradient. Grazing is ascent on `food`; fleeing is descent on `threat`;
//! herding is ascent on `herd` down-weighted by crowding; footing is
//! `height`. Cost moves from per-creature × per-scan to per-cell × per-
//! channel, which is what makes a thousand creatures the same price as
//! twenty.
//!
//! This is the CPU build of the field and the sampling API; both are
//! pure and tested. The GPU build is the same layout written by a compute
//! pass from the surface data the chunk manager already holds, read back
//! once per tick through the same ring `gpu_generator` now has — the
//! field is a few hundred KB, the readback is one fence. The API here is
//! what the roamer loop should be rewritten against; the build function
//! is the thing the compute shader replaces.
//!
//! Units: cells are `cell` blocks square; the field covers `w × h` cells
//! from `origin` on the XZ plane. Values are dimensionless 0..1-ish
//! densities (food, herd, threat) or blocks (height, water).

/// A creature the field should know about.
#[derive(Clone, Copy, Debug)]
pub struct Presence {
    pub x: f32,
    pub z: f32,
    /// predators splat threat; grazers splat herd
    pub predator: bool,
}

#[derive(Clone, Debug)]
pub struct BehaviorField {
    pub origin: [f32; 2],
    pub cell: f32,
    pub w: usize,
    pub h: usize,
    pub food: Vec<f32>,
    pub threat: Vec<f32>,
    pub herd: Vec<f32>,
    pub height: Vec<f32>,
    pub water: Vec<f32>,
}

/// How far a predator's threat reaches, blocks. GUESS: a herbivore starts
/// moving at about this range in the current per-creature code.
pub const THREAT_RADIUS: f32 = 24.0;
/// How far a grazer counts toward its neighbours' herd sense, blocks.
pub const HERD_RADIUS: f32 = 16.0;

impl BehaviorField {
    /// An empty field of `w × h` cells of `cell` blocks, cornered at `origin`.
    pub fn new(origin: [f32; 2], cell: f32, w: usize, h: usize) -> BehaviorField {
        let n = w * h;
        BehaviorField { origin, cell, w, h, food: vec![0.0; n], threat: vec![0.0; n], herd: vec![0.0; n], height: vec![0.0; n], water: vec![0.0; n] }
    }

    /// The field around a viewer: `radius` blocks each way.
    pub fn around(x: f32, z: f32, radius: f32, cell: f32) -> BehaviorField {
        let n = ((radius * 2.0) / cell).ceil().max(1.0) as usize;
        BehaviorField::new([x - radius, z - radius], cell, n, n)
    }

    #[inline]
    fn idx(&self, cx: usize, cz: usize) -> usize {
        cz * self.w + cx
    }

    /// Cell under a world position, clamped to the field.
    pub fn cell_of(&self, x: f32, z: f32) -> (usize, usize) {
        let cx = ((x - self.origin[0]) / self.cell).floor().clamp(0.0, (self.w - 1) as f32) as usize;
        let cz = ((z - self.origin[1]) / self.cell).floor().clamp(0.0, (self.h - 1) as f32) as usize;
        (cx, cz)
    }

    /// World centre of a cell.
    pub fn center_of(&self, cx: usize, cz: usize) -> (f32, f32) {
        (self.origin[0] + (cx as f32 + 0.5) * self.cell, self.origin[1] + (cz as f32 + 0.5) * self.cell)
    }

    /// BUILD, CPU. `surface(x, z)` answers (height, food_here, is_water)
    /// for a world column — the chunk manager's surface data. `creatures`
    /// splat threat and herd. One pass over cells, one over creatures.
    pub fn build<F>(&mut self, surface: F, creatures: &[Presence])
    where
        F: Fn(f32, f32) -> (f32, f32, bool),
    {
        for cz in 0..self.h {
            for cx in 0..self.w {
                let (x, z) = self.center_of(cx, cz);
                let (hgt, food, water) = surface(x, z);
                let i = self.idx(cx, cz);
                self.height[i] = hgt;
                self.food[i] = food;
                self.water[i] = if water { 1.0 } else { 0.0 };
                self.threat[i] = 0.0;
                self.herd[i] = 0.0;
            }
        }
        for c in creatures {
            let (r, ch) = if c.predator { (THREAT_RADIUS, 0) } else { (HERD_RADIUS, 1) };
            self.splat(c.x, c.z, r, ch);
        }
    }

    /// A radial falloff (1 at the centre, 0 at `radius`) added to channel
    /// 0 (threat) or 1 (herd). Bounded to the cells the disc touches.
    fn splat(&mut self, x: f32, z: f32, radius: f32, channel: u8) {
        let (cx0, cz0) = self.cell_of(x - radius, z - radius);
        let (cx1, cz1) = self.cell_of(x + radius, z + radius);
        for cz in cz0..=cz1 {
            for cx in cx0..=cx1 {
                let (px, pz) = self.center_of(cx, cz);
                let d = ((px - x).powi(2) + (pz - z).powi(2)).sqrt();
                if d >= radius {
                    continue;
                }
                let v = 1.0 - d / radius;
                let i = self.idx(cx, cz);
                if channel == 0 {
                    self.threat[i] += v;
                } else {
                    self.herd[i] += v;
                }
            }
        }
    }

    /// Bilinear sample of a channel at a world position.
    pub fn sample(&self, ch: &[f32], x: f32, z: f32) -> f32 {
        let fx = ((x - self.origin[0]) / self.cell - 0.5).clamp(0.0, (self.w - 1) as f32);
        let fz = ((z - self.origin[1]) / self.cell - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, z0) = (fx.floor() as usize, fz.floor() as usize);
        let (x1, z1) = ((x0 + 1).min(self.w - 1), (z0 + 1).min(self.h - 1));
        let (tx, tz) = (fx - x0 as f32, fz - z0 as f32);
        let a = ch[z0 * self.w + x0] * (1.0 - tx) + ch[z0 * self.w + x1] * tx;
        let b = ch[z1 * self.w + x0] * (1.0 - tx) + ch[z1 * self.w + x1] * tx;
        a * (1.0 - tz) + b * tz
    }

    /// Gradient of a channel at a world position, per block, by central
    /// difference one cell each way. Points UPHILL.
    pub fn gradient(&self, ch: &[f32], x: f32, z: f32) -> (f32, f32) {
        let d = self.cell;
        let gx = (self.sample(ch, x + d, z) - self.sample(ch, x - d, z)) / (2.0 * d);
        let gz = (self.sample(ch, x, z + d) - self.sample(ch, x, z - d)) / (2.0 * d);
        (gx, gz)
    }

    /// THE DECISION, for a grazer: where to go, as a unit-ish XZ direction
    /// and a reason. Flee threat first (down its gradient), else climb
    /// food, else drift toward the herd; stay put when nothing pulls.
    /// `crowd` is how much herd is too much — a grazer standing in a
    /// dense herd stops climbing it.
    pub fn grazer_intent(&self, x: f32, z: f32, crowd: f32) -> (f32, f32, Intent) {
        let t = self.sample(&self.threat, x, z);
        if t > 0.05 {
            let (gx, gz) = self.gradient(&self.threat, x, z);
            let n = (gx * gx + gz * gz).sqrt();
            if n > 1e-4 {
                return (-gx / n, -gz / n, Intent::Flee);
            }
        }
        let (fx, fz) = self.gradient(&self.food, x, z);
        let fn_ = (fx * fx + fz * fz).sqrt();
        if fn_ > 1e-4 {
            return (fx / fn_, fz / fn_, Intent::Graze);
        }
        let here = self.sample(&self.herd, x, z);
        if here < crowd {
            let (hx, hz) = self.gradient(&self.herd, x, z);
            let n = (hx * hx + hz * hz).sqrt();
            if n > 1e-4 {
                return (hx / n, hz / n, Intent::Herd);
            }
        }
        (0.0, 0.0, Intent::Rest)
    }

    /// Bytes of the field, for the GPU readback budget.
    pub fn bytes(&self) -> usize {
        self.w * self.h * 5 * std::mem::size_of::<f32>()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Intent {
    Flee,
    Graze,
    Herd,
    Rest,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_with_grass_at(gx: f32, gz: f32) -> impl Fn(f32, f32) -> (f32, f32, bool) {
        move |x, z| {
            let d = ((x - gx).powi(2) + (z - gz).powi(2)).sqrt();
            (64.0, (1.0 - d / 30.0).max(0.0), false)
        }
    }

    #[test]
    fn the_field_covers_the_viewer_and_reads_the_surface() {
        let mut f = BehaviorField::around(100.0, -50.0, 64.0, 4.0);
        assert_eq!(f.w, 32);
        f.build(|x, z| (x * 0.1, 0.0, z < -60.0), &[]);
        let (cx, cz) = f.cell_of(100.0, -50.0);
        assert!(cx < f.w && cz < f.h);
        let h = f.sample(&f.height, 100.0, -50.0);
        assert!((h - 10.0).abs() < 0.5, "height {h}");
        assert!(f.sample(&f.water, 100.0, -100.0) > 0.5);
        assert!(f.sample(&f.water, 100.0, -20.0) < 0.5);
        assert_eq!(f.bytes(), 32 * 32 * 20);
    }

    #[test]
    fn a_grazer_climbs_toward_the_grass() {
        let mut f = BehaviorField::around(0.0, 0.0, 64.0, 4.0);
        f.build(flat_with_grass_at(30.0, 0.0), &[]);
        let (dx, dz, why) = f.grazer_intent(0.0, 0.0, 3.0);
        assert_eq!(why, Intent::Graze);
        assert!(dx > 0.9 && dz.abs() < 0.2, "went ({dx:.2},{dz:.2}) instead of +x");
    }

    #[test]
    fn a_predator_nearby_beats_the_grass_and_is_fled_away_from() {
        let mut f = BehaviorField::around(0.0, 0.0, 64.0, 4.0);
        f.build(flat_with_grass_at(30.0, 0.0), &[Presence { x: 10.0, z: 0.0, predator: true }]);
        let (dx, _dz, why) = f.grazer_intent(0.0, 0.0, 3.0);
        assert_eq!(why, Intent::Flee);
        assert!(dx < -0.9, "fled toward the predator: dx {dx:.2}");
        // past its reach (24 blocks) the grass wins again: at x=45 the
        // predator at 10 is 35 away and the grass at 30 is 15 away
        let (dx, _, why) = f.grazer_intent(45.0, 0.0, 3.0);
        assert_eq!(why, Intent::Graze);
        assert!(dx < 0.0, "did not turn back toward the grass");
        // and beyond both, nothing pulls: the first version of this test
        // expected grass sensed from 80 blocks, which is not a sense
        assert_eq!(f.grazer_intent(-50.0, 0.0, 3.0).2, Intent::Rest);
    }

    #[test]
    fn the_herd_pulls_and_a_crowd_does_not() {
        let mut f = BehaviorField::around(0.0, 0.0, 64.0, 4.0);
        let herd: Vec<Presence> = (0..6).map(|i| Presence { x: 20.0 + i as f32, z: 0.0, predator: false }).collect();
        f.build(|_, _| (64.0, 0.0, false), &herd);
        let (dx, _, why) = f.grazer_intent(0.0, 0.0, 3.0);
        assert_eq!(why, Intent::Herd);
        assert!(dx > 0.9);
        // standing in it, with a low crowd threshold: rest
        let (_, _, why) = f.grazer_intent(22.0, 0.0, 0.5);
        assert_eq!(why, Intent::Rest);
    }

    #[test]
    fn the_build_is_per_cell_not_per_creature() {
        // a thousand creatures cost the splats, not a thousand world scans:
        // the surface closure is called once per cell regardless
        use std::cell::Cell;
        let calls = Cell::new(0usize);
        let mut f = BehaviorField::around(0.0, 0.0, 128.0, 4.0);
        let herd: Vec<Presence> = (0..1000).map(|i| Presence { x: (i % 100) as f32 - 50.0, z: (i / 100) as f32 * 3.0, predator: i % 7 == 0 }).collect();
        f.build(|_, _| { calls.set(calls.get() + 1); (64.0, 0.2, false) }, &herd);
        assert_eq!(calls.get(), f.w * f.h);
    }
}
