//! What the ground gives you is what you build with.
//!
//! Two things the strata layer made obvious the moment it existed.
//!
//! A BUILDING ON BANDED ROCK still used a palette chosen from the
//! SURFACE block. Stand on a mesa and the surface is sand, so the
//! house is sandstone -- which is right by luck. Stand on grass with
//! granite two blocks down and the house is oak, when every real
//! building on that spot is granite, because that is what came out of
//! the hole they dug for the cellar. Vernacular architecture is
//! whatever was underfoot; the profile knows what is underfoot.
//!
//! A CARVE LEFT NO EVIDENCE. Dig a canyon and the rock vanishes. A
//! quarry has a spoil heap, a mine has tailings, a collapsed cavern
//! has a cone of debris on its floor. That material is the same rock
//! that came out, which the profile also knows.
//!
//! So: `quarry_palette` reads the profile and returns what a builder
//! here would actually use; `spoil` turns a carve's removals into a
//! heap beside it; `rubble` scatters the same material over a floor.
//! All three are pure functions of the profile and the change list.

use crate::strata::Profile;
use crate::weather::Change;
use crate::Palette;
use std::collections::BTreeMap;

/// The palette a builder standing here would use: the most workable
/// rock within reach, its softer neighbour for trim, and whatever the
/// surface offers for accents.
///
/// "Workable" is a ranking, not a lookup: a builder prefers a rock
/// that cuts into blocks over one that does not, and prefers what is
/// near the surface over what is deep, because the hole they dug was
/// only so deep.
pub fn quarry_palette(profile: &Profile, x: i32, z: i32, seed: u64, reach: i32) -> Palette {
    // How good a block is to build with. Stone-family first, then the
    // softer sedimentaries, then earth. Air and liquids score nothing.
    fn workable(b: u16) -> i32 {
        match b {
            1 | 4 | 98 => 10,           // stone, cobble, bricks
            24 | 179 | 172 => 8,        // sandstone, terracotta
            87 | 49 => 7,               // netherrack, obsidian: hard but usable
            13 => 4,                    // gravel
            12 => 3,                    // sand
            3 | 2 => 1,                 // dirt, grass: not really
            _ => 0,
        }
    }
    // trim is the SECOND-BEST distinct material, so the two contrast
    let mut seen: Vec<(u16, i32, i32)> = Vec::new();     // block, score, depth
    for d in 0..reach.max(1) {
        let b = profile.at(x, z, d, seed);
        let s = workable(b);
        if s == 0 { continue; }
        if !seen.iter().any(|(sb, _, _)| *sb == b) { seen.push((b, s, d)); }
    }
    // prefer high score, then shallow: a rock ten blocks down is a
    // quarry, a rock thirty down is a mine nobody dug
    seen.sort_by_key(|(_, s, d)| (-s, *d));
    let primary = seen.first().map(|(b, _, _)| *b).unwrap_or(1);
    let secondary = seen.get(1).map(|(b, _, _)| *b).unwrap_or(primary);
    // the accent comes from the deepest thing in reach: the rare
    // material, used sparingly, because there was less of it
    let accent = seen.last().filter(|(b, _, _)| *b != primary).map(|(b, _, _)| *b).unwrap_or(secondary);
    let surface = profile.at(x, z, 0, seed);
    let foliage = match surface {
        12 | 24 => 81,      // desert: cactus
        80 | 32 => 18,      // snow: spruce leaves
        _ => 18,
    };
    Palette { name: format!("quarried:{}", profile.name), primary, secondary, accent, foliage }
}

/// Where a spoil heap goes and how big.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spoil {
    /// Fraction of the removed material that ends up in the heap. The
    /// rest is "carted away" -- a heap holding a whole canyon would
    /// bury the canyon.
    pub fraction: f32,
    /// How far from the cut the heap sits.
    pub offset: i32,
    /// Cap on the heap's radius.
    pub max_radius: i32,
    pub seed: u64,
}

impl Default for Spoil {
    fn default() -> Self { Self { fraction: 0.04, offset: 8, max_radius: 24, seed: 1 } }
}

/// Turn a carve's removals into a heap beside it, made of the rock
/// that came out. The heap's composition follows what was actually
/// removed -- dig through sandstone and the heap is sandstone -- which
/// is the point: it is evidence of the hole.
pub fn spoil<W: crate::VoxelWorld>(w: &W, removed: &[Change], profile: &Profile, s: &Spoil,
                                   from_y: i32) -> Vec<Change> {
    let cells: Vec<(i32, i32, i32)> = removed.iter().filter(|c| c.to == 0).map(|c| c.at).collect();
    if cells.len() < 20 { return Vec::new(); }
    // the cut's centre and its long axis
    let n = cells.len() as f32;
    let cx = cells.iter().map(|c| c.0 as f32).sum::<f32>() / n;
    let cz = cells.iter().map(|c| c.2 as f32).sum::<f32>() / n;
    let (mut vxx, mut vzz, mut vxz) = (0.0f32, 0.0f32, 0.0f32);
    for c in &cells {
        let (dx, dz) = (c.0 as f32 - cx, c.2 as f32 - cz);
        vxx += dx * dx; vzz += dz * dz; vxz += dx * dz;
    }
    // the perpendicular to the long axis: the heap goes to one SIDE,
    // not on top of the thing that was dug
    let angle = 0.5 * (2.0 * vxz).atan2(vxx - vzz);
    let (px, pz) = (-angle.sin(), angle.cos());
    let volume = (cells.len() as f32 * s.fraction).max(1.0);
    // a cone: radius from the volume, height from the radius
    let radius = ((volume / 1.6).cbrt() * 1.4).min(s.max_radius as f32).max(1.0);
    let hx = (cx + px * (s.offset as f32 + radius)) as i32;
    let hz = (cz + pz * (s.offset as f32 + radius)) as i32;
    // what came out, by count: the heap is made of the same stuff
    let mut mix: BTreeMap<u16, usize> = BTreeMap::new();
    for c in cells.iter().take(4000) {
        let surf = (0..=from_y + 80).rev().find(|&y| w.get(c.0, y, c.2) != 0).unwrap_or(c.1);
        *mix.entry(profile.at(c.0, c.2, (surf - c.1).max(0), s.seed)).or_insert(0) += 1;
    }
    let mut palette: Vec<(u16, usize)> = mix.into_iter().collect();
    palette.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    if palette.is_empty() { palette.push((1, 1)); }
    let total: usize = palette.iter().map(|(_, n)| *n).sum();
    let mut out = Vec::new();
    let r = radius as i32;
    let mut k = 0u64;
    for dx in -r..=r { for dz in -r..=r {
        let d = ((dx * dx + dz * dz) as f32).sqrt();
        if d > radius { continue; }
        // the cone, with a rough edge: a spoil heap is not a dome
        let mut h = ((1.0 - d / radius) * radius * 0.7) as i32;
        k = k.wrapping_mul(6364136223846793005).wrapping_add(s.seed | 1);
        if (k >> 33) % 5 == 0 { h -= 1; }
        if h < 1 { continue; }
        let (x, z) = (hx + dx, hz + dz);
        let ground = (0..=from_y + 80).rev().find(|&y| w.get(x, y, z) != 0).unwrap_or(from_y);
        for i in 0..h {
            // pick a material weighted by what came out
            k = k.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mut pickn = ((k >> 33) as usize) % total.max(1);
            let mut block = palette[0].0;
            for (b, cnt) in &palette { if pickn < *cnt { block = *b; break; } pickn -= cnt; }
            out.push(Change { at: (x, ground + 1 + i, z), to: block });
        }
    }}
    out
}

/// Scatter debris over the floor of a carve: the material that fell
/// when the roof came down. Unlike a heap this is thin and inside the
/// hole, and it is what makes a cave floor read as a cave floor rather
/// than as a corridor.
pub fn rubble<W: crate::VoxelWorld>(w: &W, removed: &[Change], profile: &Profile,
                                    density: f32, seed: u64, from_y: i32) -> Vec<Change> {
    let cells: Vec<(i32, i32, i32)> = removed.iter().filter(|c| c.to == 0).map(|c| c.at).collect();
    if cells.is_empty() { return Vec::new(); }
    // the floor of each column: the lowest removed cell that has
    // something solid beneath it
    let mut floors: BTreeMap<(i32, i32), i32> = BTreeMap::new();
    let removed_set: std::collections::BTreeSet<(i32, i32, i32)> = cells.iter().copied().collect();
    for c in &cells {
        let below = (c.0, c.1 - 1, c.2);
        if removed_set.contains(&below) { continue; }
        if w.get(below.0, below.1, below.2) == 0 { continue; }
        let e = floors.entry((c.0, c.2)).or_insert(c.1);
        *e = (*e).min(c.1);
    }
    let mut out = Vec::new();
    let mut k = seed | 1;
    for ((x, z), y) in floors {
        k = k.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        if ((k >> 33) % 1000) as f32 / 1000.0 > density { continue; }
        // the debris is the rock from ABOVE: what fell
        let surf = (0..=from_y + 80).rev().find(|&yy| w.get(x, yy, z) != 0).unwrap_or(y);
        let block = profile.at(x, z, (surf - y - 2).max(0), seed);
        out.push(Change { at: (x, y, z), to: block });
        // occasionally two deep
        if (k >> 41) % 7 == 0 { out.push(Change { at: (x, y + 1, z), to: block }); }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strata::lookup;
    use crate::VoxelWorld;

    struct Flat(i32);
    impl VoxelWorld for Flat {
        fn get(&self, _: i32, y: i32, _: i32) -> u16 { if y <= self.0 { 1 } else { 0 } }
        fn set(&mut self, _: i32, _: i32, _: i32, _: u16) -> bool { true }
    }

    #[test]
    fn a_builder_uses_the_rock_under_their_feet_not_the_grass_on_top() {
        // temperate: grass, dirt, then stone. The surface says grass;
        // the builder says stone, because that is what the cellar gave.
        let p = lookup("temperate").unwrap();
        let q = quarry_palette(p, 0, 0, 1, 12);
        assert_eq!(q.primary, 1, "stone, not grass: {q:?}");
        assert_ne!(q.secondary, q.primary, "trim contrasts");
        // sedimentary: sandstone is the workable one near the surface
        let s = lookup("sedimentary").unwrap();
        let qs = quarry_palette(s, 0, 0, 1, 14);
        assert!(matches!(qs.primary, 24 | 172 | 179), "a banded rock: {qs:?}");
        assert!(qs.name.starts_with("quarried:sedimentary"));
        // desert: sandstone under the sand
        let d = quarry_palette(lookup("desert").unwrap(), 0, 0, 1, 12);
        assert_eq!(d.primary, 24);
    }

    #[test]
    fn reach_matters_a_shallow_hole_finds_less() {
        let p = lookup("temperate").unwrap();
        // one block down is grass only: nothing workable but the grass
        let shallow = quarry_palette(p, 0, 0, 1, 1);
        let deep = quarry_palette(p, 0, 0, 1, 20);
        assert_ne!(shallow.primary, deep.primary, "a deeper hole finds better rock");
        assert_eq!(deep.primary, 1);
    }

    #[test]
    fn spoil_is_a_heap_beside_the_cut_made_of_what_came_out() {
        let w = Flat(90);
        let p = lookup("sedimentary").unwrap();
        // a long east-west trench
        let removed: Vec<Change> = (0..120).flat_map(|x| (85..=90).map(move |y| Change { at: (x, y, 0), to: 0 })).collect();
        let heap = spoil(&w, &removed, p, &Spoil { fraction: 0.08, seed: 3, ..Default::default() }, 90);
        assert!(!heap.is_empty(), "a heap");
        // it is BESIDE the trench, not on it: the trench runs along x,
        // so the heap is offset in z
        let hz: f32 = heap.iter().map(|c| c.at.2 as f32).sum::<f32>() / heap.len() as f32;
        assert!(hz.abs() > 6.0, "beside the cut, not on it: mean z {hz}");
        // and made of the rock that came out
        let blocks: std::collections::BTreeSet<u16> = heap.iter().map(|c| c.to).collect();
        assert!(blocks.iter().all(|b| [12, 24, 172, 179, 1].contains(b)), "sedimentary spoil: {blocks:?}");
        assert!(blocks.len() >= 2, "a mix, as the cut was: {blocks:?}");
        // it is a cone: taller in the middle
        let by_col: BTreeMap<(i32, i32), usize> = heap.iter().fold(BTreeMap::new(), |mut m, c| {
            *m.entry((c.at.0, c.at.2)).or_insert(0) += 1; m });
        let tallest = by_col.values().max().unwrap();
        assert!(*tallest >= 3, "a heap has height: {tallest}");
    }

    #[test]
    fn a_small_cut_makes_no_heap_and_a_huge_one_is_capped() {
        let w = Flat(90);
        let p = lookup("temperate").unwrap();
        let tiny: Vec<Change> = (0..5).map(|x| Change { at: (x, 90, 0), to: 0 }).collect();
        assert!(spoil(&w, &tiny, p, &Spoil::default(), 90).is_empty(), "a posthole leaves no spoil heap");
        let huge: Vec<Change> = (0..400).flat_map(|x| (0..60).flat_map(move |z| (60..90).map(move |y| Change { at: (x, y, z), to: 0 }))).collect();
        let heap = spoil(&w, &huge, p, &Spoil { max_radius: 12, ..Default::default() }, 90);
        let xs: Vec<i32> = heap.iter().map(|c| c.at.0).collect();
        let span = xs.iter().max().unwrap() - xs.iter().min().unwrap();
        assert!(span <= 26, "capped at the max radius: {span} across");
    }

    #[test]
    fn rubble_lies_on_the_floor_and_is_the_rock_from_above() {
        let w = Flat(90);
        let p = lookup("sedimentary").unwrap();
        // a chamber: a block of removals with a floor under it
        let removed: Vec<Change> = (0..20).flat_map(|x| (0..20).flat_map(move |z| (80..88).map(move |y| Change { at: (x, y, z), to: 0 })))
            .collect();
        let r = rubble(&w, &removed, p, 0.4, 5, 90);
        assert!(!r.is_empty());
        // every piece sits at the floor, not in the air
        assert!(r.iter().all(|c| c.at.1 <= 81), "on the floor: {:?}", r.iter().map(|c| c.at.1).max());
        // roughly the density asked for
        let cols: std::collections::BTreeSet<(i32, i32)> = r.iter().map(|c| (c.at.0, c.at.2)).collect();
        assert!((100..=250).contains(&cols.len()), "about 40% of 400 columns: {}", cols.len());
        // and no rubble at all where there is no floor
        let floating: Vec<Change> = (0..10).map(|x| Change { at: (x, 200, 0), to: 0 }).collect();
        assert!(rubble(&w, &floating, p, 1.0, 5, 90).is_empty(), "nothing to land on");
    }
}
