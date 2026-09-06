//! Brushes: shaping terrain at landscape scale.
//!
//! `terraform` moves a box by a delta. That is a shovel. A landscape
//! editor works with BRUSHES: a centre, a radius, a falloff, and an
//! operation on the heightfield under it — raise, lower, smooth,
//! flatten, roughen. The falloff is what makes a hill look like a hill
//! rather than a plinth, and a radius of 150 is what makes "a mountain
//! range" a single command rather than forty.
//!
//! THE HEIGHTFIELD IS SAMPLED, NOT STORED
//!
//! Nothing here keeps a terrain grid. A brush reads the surface height
//! under its footprint from the world, computes the new height per
//! column, and emits the block changes to get there: fill up to the new
//! height with the column's own surface block, or cut down to it. That
//! keeps the engine's world the only source of truth and means a brush
//! works on terrain it did not generate — a build, a previous brush,
//! anything.
//!
//! FALLOFF
//!
//! Every brush weights its effect by distance from the centre, 1.0 at
//! the middle falling to 0.0 at the radius. `Falloff::Smooth` is the
//! default (smoothstep); `Sharp` is a plateau with a steep edge;
//! `Linear` is a cone. The choice IS the landform: a smooth raise is a
//! hill, a sharp one is a mesa.
//!
//! BUDGET
//!
//! A radius of 150 is 70,000 columns. Each column changes at most a few
//! blocks for a smooth or a small raise, but a 40-block raise over that
//! radius is millions. `plan` reports the count before anything is
//! written, and the caller decides.

use crate::weather::Change;
use crate::VoxelWorld;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Falloff {
    /// Smoothstep: a hill.
    Smooth,
    /// Plateau with a steep rim: a mesa.
    Sharp,
    /// A cone.
    Linear,
    /// The whole footprint equally: a plinth, or a flatten.
    Flat,
}

impl Falloff {
    /// Weight at normalised distance `t` in 0..=1.
    pub fn weight(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Falloff::Smooth => { let u = 1.0 - t; u * u * (3.0 - 2.0 * u) }
            Falloff::Sharp => if t < 0.8 { 1.0 } else { ((1.0 - t) / 0.2).clamp(0.0, 1.0) },
            Falloff::Linear => 1.0 - t,
            Falloff::Flat => 1.0,
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_lowercase().as_str() {
            "smooth" | "hill" => Falloff::Smooth,
            "sharp" | "mesa" | "plateau" => Falloff::Sharp,
            "linear" | "cone" => Falloff::Linear,
            "flat" => Falloff::Flat,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BrushOp {
    /// Add up to `amount` blocks of height at the centre.
    Raise(i32),
    /// Remove up to `amount`.
    Lower(i32),
    /// Pull every column toward the local mean, `strength` 0..1.
    Smooth(f32),
    /// Pull every column toward `y`, weighted by falloff.
    Flatten(i32),
    /// Add seeded noise of up to `amplitude`.
    Roughen(i32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Brush {
    pub centre: (i32, i32),
    pub radius: i32,
    pub falloff: Falloff,
    pub op: BrushOp,
    pub seed: u64,
}

/// A brush's planned effect, before anything is written.
#[derive(Debug, Clone, Default)]
pub struct BrushPlan {
    pub changes: Vec<Change>,
    pub columns: usize,
    pub raised: usize,
    pub lowered: usize,
    /// The largest single-column change, for the preview.
    pub max_delta: i32,
    /// Planning stopped early because the stroke exceeded the brush
    /// ceiling. The apply would refuse it anyway; say so instead.
    pub truncated: bool,
}

impl BrushPlan {
    pub fn summary(&self) -> String {
        format!("{} columns: {} raised, {} lowered, {} blocks, max {} per column{}",
                self.columns, self.raised, self.lowered, self.changes.len(), self.max_delta,
                if self.truncated { " -- TOO BIG, reduce r= or by=" } else { "" })
    }
}

fn hash2(x: i32, z: i32, seed: u64) -> f32 {
    let mut h = seed
        ^ (x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (z as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 31;
    h = h.wrapping_mul(0x7FB5_D329_728E_A185);
    ((h ^ (h >> 27)) % 10_000) as f32 / 10_000.0
}

/// Value noise at a coarse scale, so roughen makes hummocks rather than
/// single-block spikes.
fn value_noise(x: i32, z: i32, scale: i32, seed: u64) -> f32 {
    let s = scale.max(1);
    let (cx, cz) = (x.div_euclid(s), z.div_euclid(s));
    let (fx, fz) = ((x.rem_euclid(s)) as f32 / s as f32, (z.rem_euclid(s)) as f32 / s as f32);
    let (fx, fz) = (fx * fx * (3.0 - 2.0 * fx), fz * fz * (3.0 - 2.0 * fz));
    let a = hash2(cx, cz, seed);
    let b = hash2(cx + 1, cz, seed);
    let c = hash2(cx, cz + 1, seed);
    let d = hash2(cx + 1, cz + 1, seed);
    let top = a + (b - a) * fx;
    let bot = c + (d - c) * fx;
    (top + (bot - top) * fz) * 2.0 - 1.0
}

/// Plan a brush stroke. Reads the world; writes nothing.
///
/// `from_y` is where the surface search starts; `air`, `fill` are what
/// to cut to and what to build up with when the column's own surface
/// block is unusable (water, air).
pub fn plan<W: VoxelWorld>(w: &W, b: &Brush, from_y: i32, fill: u16) -> BrushPlan {
    let r = b.radius.clamp(1, 400);
    let (cx, cz) = b.centre;
    let mut out = BrushPlan::default();
    // A brush shapes LAND. `by=2147483647` is not a request for a
    // two-billion-block spike; it is a fumbled number, and without a
    // clamp it asked for billions of pushes per column. Sixty-four is
    // a mountain.
    let clamp_amt = |a: i32| a.clamp(-64, 64);
    let b = Brush { op: match b.op {
        BrushOp::Raise(a) => BrushOp::Raise(clamp_amt(a)),
        BrushOp::Lower(a) => BrushOp::Lower(clamp_amt(a)),
        BrushOp::Flatten(y) => BrushOp::Flatten(y),
        BrushOp::Roughen(a) => BrushOp::Roughen(clamp_amt(a).abs().min(16)),
        o => o,
    }, ..*b };
    let b = &b;

    // Two passes for smooth: heights first, then the mean, then the
    // pull. Everything else is one pass.
    let mut cols: Vec<((i32, i32), i32, u16)> = Vec::new();
    for x in (cx - r)..=(cx + r) {
        for z in (cz - r)..=(cz + r) {
            let d2 = ((x - cx) as i64).pow(2) + ((z - cz) as i64).pow(2);
            if d2 > (r as i64).pow(2) { continue; }
            // Surface: the first solid block scanning down. Water is
            // not a surface a brush should stack onto.
            let mut y = from_y;
            let mut surf = None;
            while y > from_y - 512 {
                let bl = w.get(x, y, z);
                if bl != 0 && bl != 9 { surf = Some((y, bl)); break; }
                y -= 1;
            }
            if let Some((sy, sb)) = surf { cols.push(((x, z), sy, sb)); }
        }
    }
    out.columns = cols.len();
    if cols.is_empty() { return out; }

    let mean = if let BrushOp::Smooth(_) = b.op {
        // Local mean over a 5-column neighbourhood, so smoothing
        // flattens ridges rather than pulling the whole region to one
        // height. Looked up by (x,z).
        let map: std::collections::HashMap<(i32, i32), i32> =
            cols.iter().map(|(k, y, _)| (*k, *y)).collect();
        let mut m = std::collections::HashMap::new();
        for (k, _, _) in &cols {
            let mut sum = 0i64; let mut n = 0i64;
            for dx in -2..=2 { for dz in -2..=2 {
                if let Some(v) = map.get(&(k.0 + dx, k.1 + dz)) { sum += *v as i64; n += 1; }
            }}
            m.insert(*k, if n > 0 { (sum / n) as i32 } else { map[k] });
        }
        Some(m)
    } else { None };

    for ((x, z), sy, sb) in cols {
        let d = (((x - cx) as f32).powi(2) + ((z - cz) as f32).powi(2)).sqrt();
        let wgt = b.falloff.weight(d / r as f32);
        if wgt <= 0.0 { continue; }
        let target = match b.op {
            BrushOp::Raise(a) => sy + (a as f32 * wgt).round() as i32,
            BrushOp::Lower(a) => sy - (a as f32 * wgt).round() as i32,
            BrushOp::Flatten(y) => sy + (((y - sy).clamp(-64, 64)) as f32 * wgt).round() as i32,
            BrushOp::Smooth(s) => {
                let m = mean.as_ref().and_then(|m| m.get(&(x, z))).copied().unwrap_or(sy);
                sy + ((m - sy) as f32 * s.clamp(0.0, 1.0) * wgt).round() as i32
            }
            BrushOp::Roughen(a) => {
                let n = value_noise(x, z, (r / 6).max(3), b.seed);
                sy + (n * a as f32 * wgt).round() as i32
            }
        };
        let delta = target - sy;
        if delta == 0 { continue; }
        // Bail once the plan is past what any apply will accept: a
        // flatten toward a bad `to=` can be 64 blocks in every one of
        // 500,000 columns, and planning 32 million changes that will be
        // refused is a hang with extra steps.
        if out.changes.len() > crate::MAX_BLOCKS_PER_BRUSH { out.truncated = true; break; }
        out.max_delta = out.max_delta.max(delta.abs());
        // Build up with the column's own surface block, so a raise on
        // grass is grass and a raise on sand is sand. Fall back on the
        // fill block if the surface was something odd.
        let block = if matches!(sb, 0 | 9) { fill } else { sb };
        if delta > 0 {
            out.raised += 1;
            for y in (sy + 1)..=target { out.changes.push(Change { at: (x, y, z), to: block }); }
        } else {
            out.lowered += 1;
            for y in (target + 1)..=sy { out.changes.push(Change { at: (x, y, z), to: 0 }); }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct T { f: fn(i32, i32) -> i32, cells: BTreeMap<(i32, i32, i32), u16> }
    impl VoxelWorld for T {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.cells.get(&(x, y, z)) { return b; }
            if y <= (self.f)(x, z) { 2 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.cells.insert((x, y, z), b); true }
    }
    fn flat() -> T { T { f: |_, _| 64, cells: BTreeMap::new() } }
    fn apply(w: &mut T, p: &BrushPlan) { for c in &p.changes { w.set(c.at.0, c.at.1, c.at.2, c.to); } }
    fn height(w: &T, x: i32, z: i32) -> i32 {
        (0..200).rev().find(|&y| w.get(x, y, z) != 0).unwrap()
    }

    #[test]
    fn a_smooth_raise_is_a_hill_not_a_plinth() {
        let mut w = flat();
        let b = Brush { centre: (0, 0), radius: 20, falloff: Falloff::Smooth,
                        op: BrushOp::Raise(10), seed: 1 };
        let p = plan(&w, &b, 200, 2);
        apply(&mut w, &p);
        let (c, mid, edge, out) = (height(&w, 0, 0), height(&w, 10, 0), height(&w, 19, 0), height(&w, 25, 0));
        assert_eq!(c, 74, "full height at the centre");
        assert!(c > mid && mid > edge, "falls off: {c} > {mid} > {edge}");
        assert_eq!(out, 64, "untouched beyond the radius");
        assert!(edge - 64 <= 1, "nearly nothing at the rim: {edge}");
    }

    #[test]
    fn a_sharp_raise_is_a_mesa() {
        let mut w = flat();
        let b = Brush { centre: (0, 0), radius: 20, falloff: Falloff::Sharp,
                        op: BrushOp::Raise(10), seed: 1 };
        let p = plan(&w, &b, 200, 2); apply(&mut w, &p);
        assert_eq!(height(&w, 0, 0), 74);
        assert_eq!(height(&w, 14, 0), 74, "flat top out to 80% of the radius");
        assert!(height(&w, 19, 0) < 70, "then a steep drop");
    }

    #[test]
    fn raising_uses_the_columns_own_surface_block() {
        // A raise on sand is sand; on grass, grass. Never a grey lump.
        let mut w = T { f: |x, _| if x < 0 { 64 } else { 64 }, cells: BTreeMap::new() };
        for z in -30..=30 { for x in 0..=30 { for y in 60..=64 { w.set(x, y, z, 12); } } }  // sand east
        let b = Brush { centre: (0, 0), radius: 10, falloff: Falloff::Flat,
                        op: BrushOp::Raise(3), seed: 1 };
        let p = plan(&w, &b, 200, 1);
        let east: Vec<u16> = p.changes.iter().filter(|c| c.at.0 == 5).map(|c| c.to).collect();
        let west: Vec<u16> = p.changes.iter().filter(|c| c.at.0 == -5).map(|c| c.to).collect();
        assert!(east.iter().all(|b| *b == 12), "east raised in sand: {east:?}");
        assert!(west.iter().all(|b| *b == 2), "west raised in grass: {west:?}");
    }

    #[test]
    fn lower_cuts_and_flatten_converges() {
        let mut w = T { f: |x, _| 64 + x / 4, cells: BTreeMap::new() };
        let b = Brush { centre: (0, 0), radius: 24, falloff: Falloff::Flat,
                        op: BrushOp::Flatten(64), seed: 1 };
        let p = plan(&w, &b, 200, 2);
        assert!(p.raised > 0 && p.lowered > 0, "a slope flattened both ways: {}", p.summary());
        apply(&mut w, &p);
        for x in [-20, -10, 0, 10, 20] {
            assert_eq!(height(&w, x, 0), 64, "flat at x={x}");
        }
    }

    #[test]
    fn smooth_flattens_a_ridge_toward_its_neighbours() {
        let mut w = flat();
        for z in -30..=30 { for y in 65..=72 { w.set(0, y, z, 2); } }   // a one-block-wide ridge
        let b = Brush { centre: (0, 0), radius: 12, falloff: Falloff::Flat,
                        op: BrushOp::Smooth(1.0), seed: 1 };
        let before = height(&w, 0, 0);
        let p = plan(&w, &b, 200, 2); apply(&mut w, &p);
        let after = height(&w, 0, 0);
        assert!(after < before, "the ridge came down: {before} -> {after}");
        assert!(height(&w, 1, 0) >= 64, "and the ground beside it did not sink below base");
    }

    #[test]
    fn roughen_makes_hummocks_not_spikes() {
        let mut w = flat();
        let b = Brush { centre: (0, 0), radius: 40, falloff: Falloff::Flat,
                        op: BrushOp::Roughen(4), seed: 7 };
        let p = plan(&w, &b, 200, 2); apply(&mut w, &p);
        // adjacent columns must differ by at most 1 most of the time:
        // coarse noise, not per-column hash
        let mut steep = 0;
        for x in -30..30 {
            if (height(&w, x, 0) - height(&w, x + 1, 0)).abs() > 1 { steep += 1; }
        }
        assert!(steep < 6, "{steep} steep steps in 60 columns is spikes, not hummocks");
        let hs: std::collections::BTreeSet<i32> = (-30..30).map(|x| height(&w, x, 0)).collect();
        assert!(hs.len() >= 3, "and it does vary: {hs:?}");
    }

    #[test]
    fn the_plan_reports_before_anything_is_written() {
        let w = flat();
        let b = Brush { centre: (0, 0), radius: 60, falloff: Falloff::Smooth,
                        op: BrushOp::Raise(30), seed: 1 };
        let p = plan(&w, &b, 200, 2);
        assert!(p.changes.len() > 10_000, "a real landform: {}", p.summary());
        assert_eq!(p.max_delta, 30);
        assert_eq!(w.cells.len(), 0, "nothing written by planning");
    }

    #[test]
    fn a_brush_does_not_stack_onto_water() {
        let mut w = flat();
        for x in -5..=5 { for z in -5..=5 { w.set(x, 64, z, 9); } }   // a pond
        let b = Brush { centre: (0, 0), radius: 3, falloff: Falloff::Flat,
                        op: BrushOp::Raise(2), seed: 1 };
        let p = plan(&w, &b, 200, 2);
        // the surface under the pond is y=63 stone-ish (2); the raise
        // builds from there, and the fill block replaces the water
        assert!(p.changes.iter().all(|c| c.at.1 <= 65), "{:?}", p.changes.iter().map(|c| c.at.1).max());
    }

    #[test]
    fn falloff_names_parse() {
        assert_eq!(Falloff::parse("hill"), Some(Falloff::Smooth));
        assert_eq!(Falloff::parse("MESA"), Some(Falloff::Sharp));
        assert_eq!(Falloff::parse("cone"), Some(Falloff::Linear));
        assert_eq!(Falloff::parse("nope"), None);
    }
}
