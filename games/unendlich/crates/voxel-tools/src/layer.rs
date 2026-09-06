//! Layers: saying what a region IS, and baking it.
//!
//! A tree op places one tree. A forest is four hundred, and asking a
//! model for four hundred lines is asking for four hundred chances to
//! get a coordinate wrong. A LAYER is the WorldPainter answer: paint a
//! region with a rule — "forest, density 0.4, pines" — and bake it. The
//! model says what the region is; the bake decides where each tree
//! goes.
//!
//! A layer has three parts:
//!
//! * a **footprint** — a box, or a box with a coverage function, so a
//!   forest can thin toward its edge or follow a slope
//! * a **rule** — what to put there: trees at a density, ground cover,
//!   rock outcrops, a surface material, caves, a detail packet
//! * a **packet** — a named bundle of detail settings the rule
//!   references, so "alpine" means the same thing everywhere it is used
//!
//! Baking is deterministic from the layer's seed, so a preview and its
//! commit agree, and so re-baking after a terrain brush gives the same
//! forest shifted to the new ground rather than a different forest.
//!
//! Layers stack. Later layers win where they overlap, which is the
//! painting model everyone already knows: the last brush stroke is on
//! top.

use crate::VoxelWorld;

/// A named bundle of detail settings. A rule names a packet rather
/// than carrying twelve numbers, so a phrase like "alpine" is one word
/// in the IR and the same twelve numbers everywhere.
#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub name: &'static str,
    pub tree: &'static str,
    /// Trees per 100 columns at density 1.0.
    pub tree_rate: f32,
    pub tree_h: (i32, i32),
    /// Ground cover blocks and their relative weights.
    pub cover: &'static [(u16, f32)],
    /// Rock outcrop block, and how often (per 100 columns).
    pub rock: Option<(u16, f32)>,
    /// Surface block to repaint the ground with, if any.
    pub surface: Option<u16>,
    /// Breeding words for the packet's trees: what a "boreal" tree IS,
    /// genetically. Each tree in the bake is bred toward these from
    /// stock seeded by its own position, so a forest varies the way a
    /// real one does rather than repeating one shape.
    pub breed: &'static str,
}

pub const PACKETS: &[Packet] = &[
    Packet { name: "temperate", breed: "oak", tree: "oak", tree_rate: 3.0, tree_h: (6, 10),
             cover: &[(25, 1.0), (27, 0.5)], rock: Some((1, 0.3)), surface: None },
    Packet { name: "alpine", breed: "pine hardy alpine narrow", tree: "pine", tree_rate: 2.0, tree_h: (7, 12),
             cover: &[(32, 0.4)], rock: Some((1, 1.5)), surface: Some(1) },
    Packet { name: "boreal", breed: "pine tall dense evergreen", tree: "pine", tree_rate: 4.5, tree_h: (9, 14),
             cover: &[(33, 0.3), (23, 0.4)], rock: Some((1, 0.4)), surface: None },
    Packet { name: "jungle", breed: "jungle tall wide dense fast", tree: "jungle", tree_rate: 5.0, tree_h: (10, 18),
             cover: &[(27, 1.5), (25, 0.6), (23, 1.0)], rock: None, surface: Some(23) },
    Packet { name: "desert", breed: "cactus hardy drought", tree: "cactus", tree_rate: 0.6, tree_h: (2, 4),
             cover: &[(28, 0.4)], rock: Some((48, 0.8)), surface: Some(12) },
    Packet { name: "darkwood", breed: "oak old gnarled sparse", tree: "oak", tree_rate: 4.0, tree_h: (8, 13),
             cover: &[(33, 0.8), (34, 0.3), (28, 0.3)], rock: None, surface: Some(23) },
    Packet { name: "meadow", breed: "birch small", tree: "birch", tree_rate: 0.4, tree_h: (5, 8),
             cover: &[(25, 3.0), (27, 0.5)], rock: None, surface: None },
    Packet { name: "barren", breed: "oak dead sparse hardy", tree: "oak", tree_rate: 0.0, tree_h: (4, 6),
             cover: &[(28, 0.5)], rock: Some((1, 2.0)), surface: Some(13) },
];

pub fn packet(name: &str) -> Option<&'static Packet> {
    PACKETS.iter().find(|p| p.name.eq_ignore_ascii_case(name.trim()))
}

/// How coverage varies across the footprint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Coverage {
    /// Everywhere inside the box.
    Full,
    /// 1.0 at the centre falling to 0.0 at the edge (an ellipse).
    Radial,
    /// Only where the slope is below `max_rise_per_block`: forests
    /// avoid cliffs.
    Gentle(f32),
    /// Only above a height: the tree line, in reverse.
    Above(i32),
    /// Only below a height.
    Below(i32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub name: String,
    pub min: (i32, i32),
    pub max: (i32, i32),
    pub packet: &'static Packet,
    /// 0..1, scales the packet's rates.
    pub density: f32,
    pub coverage: Coverage,
    pub seed: u64,
}

/// What a bake produced: IR the rest of the pipeline already knows how
/// to run, so a layer previews, commits and undoes like any plan.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bake {
    pub ir: String,
    pub trees: usize,
    pub cover: usize,
    pub rocks: usize,
    pub repainted: usize,
}

fn hash2(x: i32, z: i32, seed: u64, salt: u64) -> f32 {
    let mut h = (seed ^ salt)
        ^ (x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (z as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 31;
    h = h.wrapping_mul(0x7FB5_D329_728E_A185);
    ((h ^ (h >> 27)) % 10_000) as f32 / 10_000.0
}

/// Bake a layer against the world. Reads terrain for coverage; emits IR.
///
/// Trees are placed on a jittered grid rather than by pure chance, so
/// a forest at density 0.4 has no bald patches and no clumps of five
/// trunks touching — the two things random placement always produces
/// and no forest ever does.
pub fn bake<W: VoxelWorld>(w: &W, layer: &Layer, from_y: i32) -> Bake {
    let (x0, x1) = (layer.min.0.min(layer.max.0), layer.min.0.max(layer.max.0));
    let (z0, z1) = (layer.min.1.min(layer.max.1), layer.min.1.max(layer.max.1));
    let p = layer.packet;
    let d = layer.density.clamp(0.0, 1.0);
    let mut out = Bake { ir: format!("# layer {} ({}, density {:.2})\n", layer.name, p.name, d),
                         ..Default::default() };

    let surface = |x: i32, z: i32| -> Option<i32> {
        let mut y = from_y;
        while y > from_y - 512 {
            let b = w.get(x, y, z);
            if b != 0 { return if b == 9 { None } else { Some(y) }; }
            y -= 1;
        }
        None
    };
    let cx = (x0 + x1) as f32 / 2.0;
    let cz = (z0 + z1) as f32 / 2.0;
    let rx = ((x1 - x0) as f32 / 2.0).max(1.0);
    let rz = ((z1 - z0) as f32 / 2.0).max(1.0);
    // Coverage at a column, 0..1. Water is never covered.
    let cover_at = |x: i32, z: i32| -> f32 {
        let Some(sy) = surface(x, z) else { return 0.0 };
        match layer.coverage {
            Coverage::Full => 1.0,
            Coverage::Radial => {
                let t = (((x as f32 - cx) / rx).powi(2) + ((z as f32 - cz) / rz).powi(2)).sqrt();
                (1.0 - t).clamp(0.0, 1.0)
            }
            Coverage::Gentle(max_rise) => {
                let rise = [(2, 0), (-2, 0), (0, 2), (0, -2)].iter()
                    .filter_map(|(dx, dz)| surface(x + dx, z + dz).map(|n| (n - sy).abs()))
                    .max().unwrap_or(0) as f32 / 2.0;
                if rise <= max_rise { 1.0 } else { 0.0 }
            }
            Coverage::Above(y) => if sy >= y { 1.0 } else { 0.0 },
            Coverage::Below(y) => if sy <= y { 1.0 } else { 0.0 },
        }
    };

    // Surface repaint, if the packet has one: per column, weighted.
    if let Some(sb) = p.surface {
        for x in x0..=x1 { for z in z0..=z1 {
            let c = cover_at(x, z) * d;
            if c <= 0.0 || hash2(x, z, layer.seed, 1) > c { continue; }
            if let Some(sy) = surface(x, z) {
                if w.get(x, sy, z) != sb {
                    out.ir.push_str(&format!("fill {x},{sy},{z} {x},{sy},{z} block={}\n",
                                             name_of(sb)));
                    out.repainted += 1;
                }
            }
        }}
    }

    // Trees on a jittered grid. Cell size from the rate: at rate R per
    // 100 columns and density d, one tree per 100/(R*d) columns, so a
    // cell of side sqrt(100/(R*d)).
    let rate = p.tree_rate * d;
    if rate > 0.0 {
        let cell = (100.0 / rate).sqrt().max(2.0);
        let mut gx = x0 as f32;
        while gx <= x1 as f32 {
            let mut gz = z0 as f32;
            while gz <= z1 as f32 {
                let jx = (hash2(gx as i32, gz as i32, layer.seed, 2) - 0.5) * cell * 0.8;
                let jz = (hash2(gx as i32, gz as i32, layer.seed, 3) - 0.5) * cell * 0.8;
                let (x, z) = ((gx + jx).round() as i32, (gz + jz).round() as i32);
                if x >= x0 && x <= x1 && z >= z0 && z <= z1 {
                    let c = cover_at(x, z);
                    if c > 0.0 && hash2(x, z, layer.seed, 4) <= c {
                        // A LIVING tree, bred toward the packet's words from
                        // stock seeded by its position: the engine registers
                        // it and it keeps growing. Height comes from the
                        // genome now, not from a range.
                        out.ir.push_str(&format!("plant {x},{z} {} seed={}\n", p.breed,
                                                 (hash2(x, z, layer.seed, 5) * 1_000_000.0) as i32));
                        out.trees += 1;
                    }
                }
                gz += cell;
            }
            gx += cell;
        }
    }

    // Ground cover and rock, by chance per column, scaled by coverage.
    let total_w: f32 = p.cover.iter().map(|(_, w)| w).sum();
    for x in x0..=x1 { for z in z0..=z1 {
        let c = cover_at(x, z) * d;
        if c <= 0.0 { continue; }
        let Some(sy) = surface(x, z) else { continue };
        // cover: one in ~12 columns at full density
        if total_w > 0.0 && hash2(x, z, layer.seed, 6) < c * 0.08 * total_w.min(3.0) {
            let mut pick = hash2(x, z, layer.seed, 7) * total_w;
            let mut block = p.cover[0].0;
            for (b, wgt) in p.cover { if pick <= *wgt { block = *b; break; } pick -= wgt; }
            out.ir.push_str(&format!("fill {x},{},{z} {x},{},{z} block={}\n",
                                     sy + 1, sy + 1, name_of(block)));
            out.cover += 1;
        }
        if let Some((rb, rr)) = p.rock {
            if hash2(x, z, layer.seed, 8) < c * rr / 100.0 {
                let r = 1 + (hash2(x, z, layer.seed, 9) * 2.0) as i32;
                out.ir.push_str(&format!("sphere {x},{z} r={r} y={} block={}\n",
                                         sy, name_of(rb)));
                out.rocks += 1;
            }
        }
    }}
    out
}

fn name_of(b: u16) -> &'static str {
    match b {
        1 => "stone", 2 => "grass", 12 => "sand", 13 => "gravel", 23 => "moss",
        25 => "flower", 27 => "fern", 28 => "dead_bush", 32 => "snow", 33 => "mushroom",
        34 => "cobweb", 48 => "sandstone", 4 => "cobblestone", _ => "stone",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{parse_plan, Severity};
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
    fn layer(name: &str, pk: &str, d: f32, cov: Coverage) -> Layer {
        Layer { name: name.into(), min: (-40, -40), max: (40, 40), packet: packet(pk).unwrap(),
                density: d, coverage: cov, seed: 3 }
    }

    #[test]
    fn a_forest_layer_is_hundreds_of_trees_from_one_line() {
        let w = flat();
        let b = bake(&w, &layer("wood", "boreal", 0.8, Coverage::Full), 200);
        assert!(b.trees > 150, "a real forest: {}", b.trees);
        let (plan, issues) = parse_plan(&b.ir);
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
        assert!(plan.ops.len() >= b.trees);
    }

    #[test]
    fn density_scales_the_count() {
        let w = flat();
        let full = bake(&w, &layer("a", "boreal", 1.0, Coverage::Full), 200).trees;
        let half = bake(&w, &layer("a", "boreal", 0.5, Coverage::Full), 200).trees;
        let none = bake(&w, &layer("a", "boreal", 0.0, Coverage::Full), 200).trees;
        assert!(full > half && half > none, "{full} > {half} > {none}");
        assert_eq!(none, 0);
    }

    #[test]
    fn trees_are_spread_not_clumped_and_not_bald() {
        // The jittered grid: no two trunks touching, no 20x20 hole.
        let w = flat();
        let b = bake(&w, &layer("a", "temperate", 1.0, Coverage::Full), 200);
        let (plan, _) = parse_plan(&b.ir);
        let pts: Vec<(i32, i32)> = plan.ops.iter().filter_map(|o| match o {
            crate::plan::Op::Plant { at, .. } => Some(*at), _ => None }).collect();
        for (i, a) in pts.iter().enumerate() {
            for b in &pts[i + 1..] {
                assert!((a.0 - b.0).abs() + (a.1 - b.1).abs() >= 2, "trunks touching at {a:?}/{b:?}");
            }
        }
        // no 20x20 window inside the box without a tree
        for wx in (-40..20).step_by(20) { for wz in (-40..20).step_by(20) {
            let any = pts.iter().any(|p| p.0 >= wx && p.0 < wx + 20 && p.1 >= wz && p.1 < wz + 20);
            assert!(any, "bald patch at {wx},{wz}");
        }}
    }

    #[test]
    fn radial_coverage_thins_toward_the_edge() {
        let w = flat();
        let b = bake(&w, &layer("a", "boreal", 1.0, Coverage::Radial), 200);
        let (plan, _) = parse_plan(&b.ir);
        let pts: Vec<(i32, i32)> = plan.ops.iter().filter_map(|o| match o {
            crate::plan::Op::Plant { at, .. } => Some(*at), _ => None }).collect();
        let inner = pts.iter().filter(|p| p.0.abs() < 15 && p.1.abs() < 15).count();
        let outer = pts.iter().filter(|p| p.0.abs() > 30 || p.1.abs() > 30).count();
        assert!(inner > outer * 2, "dense middle {inner}, sparse rim {outer}");
    }

    #[test]
    fn gentle_coverage_keeps_trees_off_cliffs() {
        let w = T { f: |x, _| if x > 0 { 64 + x * 3 } else { 64 }, cells: BTreeMap::new() };
        let b = bake(&w, &layer("a", "boreal", 1.0, Coverage::Gentle(1.0)), 200);
        let (plan, _) = parse_plan(&b.ir);
        let on_cliff = plan.ops.iter().filter(|o| matches!(o,
            crate::plan::Op::Plant { at, .. } if at.0 > 2)).count();
        assert_eq!(on_cliff, 0, "no trees on the 3:1 slope");
        assert!(b.trees > 20, "but plenty on the flat: {}", b.trees);
    }

    #[test]
    fn the_bake_is_deterministic_from_its_seed() {
        let w = flat();
        let l = layer("a", "jungle", 0.7, Coverage::Full);
        assert_eq!(bake(&w, &l, 200), bake(&w, &l, 200));
        let l2 = Layer { seed: 4, ..l.clone() };
        assert_ne!(bake(&w, &l, 200).ir, bake(&w, &l2, 200).ir);
    }

    #[test]
    fn a_packet_with_a_surface_repaints_the_ground() {
        let w = flat();
        let b = bake(&w, &layer("a", "desert", 1.0, Coverage::Full), 200);
        assert!(b.repainted > 1000, "sand over the grass: {}", b.repainted);
        assert!(b.ir.contains("block=sand"));
    }

    #[test]
    fn every_packet_bakes_to_valid_ir() {
        let w = flat();
        for p in PACKETS {
            let b = bake(&w, &layer("t", p.name, 0.6, Coverage::Full), 200);
            let (_, issues) = parse_plan(&b.ir);
            let errs: Vec<_> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
            assert!(errs.is_empty(), "packet '{}': {errs:?}", p.name);
        }
    }
}
