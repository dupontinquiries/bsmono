//! Elemental manipulation verbs — the *active* effects of manipulation items.
//!
//! Per `ELEMENTS.md` / `MATERIALS.md`, an elemental item perturbs the world;
//! the world's rich response (flow, slump, erosion, ignition) emerges later
//! from the physics/field systems. This module is the **crude-but-functional
//! first form**: pure functions that turn a verb ("raise a wall here") into a
//! list of world edits. The block-placing verbs are upgraded to real physics
//! down the road (a water sphere that flows via the fluid sim, an earth wall
//! that erodes, sand that slumps) without changing this interface — the same
//! "CPU-reference-then-rich" pattern as the rest of the engine.
//!
//! Composition: it builds on `field_brush` (ship 22) for the field verbs and
//! shares the `radius + 0.5` reach with `area_mining::sphere_offsets` (ship
//! 17) for the solid shapes. The geometry verbs are **generic over the block
//! type**, so this module needs no knowledge of the block registry's variants;
//! the item-facing layer ([`run_verb`]) is concrete over [`BlockId`].
//!
//! Nothing here writes to the world directly — verbs return edits, applied via
//! the [`WorldEdit`] trait by the interaction layer (decoupled exactly like
//! `field_brush`'s `ScalarField` traits). Inert until the integration gate
//! (`blocks.rs` / `bin/` / `entity.rs`) wires item use into the game loop.

use crate::world::blocks::BlockId;
use crate::world::field_brush::{brush_cells, FieldBrush, Falloff};

/// A block edit: set the cell `(x, y, z)` to `block`.
pub type BlockEdit = (i32, i32, i32, BlockId);

/// A field edit: add the signed `delta` to a scalar field at `(x, y, z)`.
pub type FieldEdit = (i32, i32, i32, f32);

/// A horizontal facing (the four cardinals).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facing {
    North,
    South,
    East,
    West,
}

impl Facing {
    /// Step into the facing direction on the XZ plane, as `(dx, dz)`.
    pub fn forward(self) -> (i32, i32) {
        match self {
            Facing::North => (0, -1),
            Facing::South => (0, 1),
            Facing::East => (1, 0),
            Facing::West => (-1, 0),
        }
    }

    /// 90° clockwise from `forward` — the wall-width axis, as `(dx, dz)`.
    pub fn right(self) -> (i32, i32) {
        match self {
            Facing::North => (1, 0),
            Facing::South => (-1, 0),
            Facing::East => (0, 1),
            Facing::West => (0, -1),
        }
    }
}

/// Raise a wall: a slab `width` along the facing's right axis × `height` up ×
/// `thickness` into the facing, of `fill`. `base` is the centered front-bottom
/// cell; width is centered on it. The crude block form of an earth wall.
pub fn raise_wall<B: Copy>(
    base: (i32, i32, i32),
    facing: Facing,
    width: i32,
    height: i32,
    thickness: i32,
    fill: B,
) -> Vec<(i32, i32, i32, B)> {
    let (bx, by, bz) = base;
    let (fx, fz) = facing.forward();
    let (rx, rz) = facing.right();
    let w = width.max(1);
    let h = height.max(1);
    let t = thickness.max(1);
    let w0 = -(w / 2);
    let mut out = Vec::with_capacity((w * h * t) as usize);
    for iw in w0..w0 + w {
        for ih in 0..h {
            for it in 0..t {
                let x = bx + rx * iw + fx * it;
                let z = bz + rz * iw + fz * it;
                out.push((x, by + ih, z, fill));
            }
        }
    }
    out
}

/// Raise a pillar: a vertical stack `height` tall of horizontal disks of
/// `radius` (the `radius + 0.5` reach), rising from `base`, of `fill`.
pub fn raise_pillar<B: Copy>(
    base: (i32, i32, i32),
    height: i32,
    radius: i32,
    fill: B,
) -> Vec<(i32, i32, i32, B)> {
    let (bx, by, bz) = base;
    let h = height.max(1);
    let disk = disk_offsets(radius);
    let mut out = Vec::with_capacity(disk.len() * h as usize);
    for ih in 0..h {
        for &(dx, dz) in &disk {
            out.push((bx + dx, by + ih, bz + dz, fill));
        }
    }
    out
}

/// Smash: clear a downward hemisphere of `radius` at `center` to `air` — a
/// crater. (Outward displacement/throw is physics; this is the crude form.)
pub fn smash_crater<B: Copy>(center: (i32, i32, i32), radius: i32, air: B) -> Vec<(i32, i32, i32, B)> {
    let (cx, cy, cz) = center;
    let r = radius.max(0);
    let reach = r as f64 + 0.5;
    let r_sq = reach * reach;
    let lim = reach.ceil() as i32;
    let mut out = Vec::new();
    for dx in -lim..=lim {
        for dy in -lim..=0 {
            for dz in -lim..=lim {
                if (dx * dx + dy * dy + dz * dz) as f64 <= r_sq {
                    out.push((cx + dx, cy + dy, cz + dz, air));
                }
            }
        }
    }
    out
}

/// Fill a solid sphere of `radius` at `center` with `block` — a water sphere,
/// an earth ball, etc. Cell set matches `area_mining::sphere_offsets`.
pub fn fill_sphere<B: Copy>(center: (i32, i32, i32), radius: i32, block: B) -> Vec<(i32, i32, i32, B)> {
    let (cx, cy, cz) = center;
    crate::world::area_mining::sphere_offsets(radius)
        .into_iter()
        .map(|(dx, dy, dz)| (cx + dx, cy + dy, cz + dz, block))
        .collect()
}

/// Heat burst: temperature-field deltas over a sphere via a `field_brush`. The
/// fire verb places no blocks; melt / evaporate / ignite emerge from field
/// coupling (`ELEMENTS.md`). The same brush negated is a chill (ice) burst.
pub fn heat_burst(center: (i32, i32, i32), brush: &FieldBrush) -> Vec<FieldEdit> {
    let (cx, cy, cz) = center;
    brush_cells(brush)
        .into_iter()
        .map(|(dx, dy, dz, delta)| (cx + dx, cy + dy, cz + dz, delta))
        .collect()
}

/// 2D disk offsets at the `radius + 0.5` reach (shared by [`raise_pillar`]).
fn disk_offsets(radius: i32) -> Vec<(i32, i32)> {
    let r = radius.max(0);
    let reach = r as f64 + 0.5;
    let r_sq = reach * reach;
    let lim = reach.ceil() as i32;
    let mut out = Vec::new();
    for dx in -lim..=lim {
        for dz in -lim..=lim {
            if (dx * dx + dz * dz) as f64 <= r_sq {
                out.push((dx, dz));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Item-facing layer (concrete over BlockId)
// ---------------------------------------------------------------------------

/// The active effect of a manipulation item, with its parameters.
#[derive(Debug, Clone, Copy)]
pub enum ElementalVerb {
    /// Earth: a wall in front of the target.
    RaiseWall { width: i32, height: i32, thickness: i32 },
    /// Earth: a column rising from the target.
    RaisePillar { height: i32, radius: i32 },
    /// Earth/impact: clear a downward crater.
    Smash { radius: i32 },
    /// Water/earth: fill a solid sphere (a water sphere, an earth ball).
    Sphere { radius: i32 },
    /// Fire: a temperature burst (negative `peak` is a chill burst).
    HeatBurst { radius: f32, peak: f32 },
}

/// Which blocks a verb places, supplied by the caller so this module stays
/// independent of the block registry's variants.
#[derive(Debug, Clone, Copy)]
pub struct VerbBlocks {
    pub fill: BlockId,
    pub fluid: BlockId,
    pub air: BlockId,
}

/// The edits a verb produces: block placements and/or field deltas.
#[derive(Debug, Default)]
pub struct VerbEdits {
    pub blocks: Vec<BlockEdit>,
    pub fields: Vec<FieldEdit>,
}

/// Resolve a verb at `target` (with `facing` for directional verbs) into the
/// edits to apply, using `blocks` for any placement.
pub fn run_verb(
    verb: ElementalVerb,
    target: (i32, i32, i32),
    facing: Facing,
    blocks: VerbBlocks,
) -> VerbEdits {
    let mut e = VerbEdits::default();
    match verb {
        ElementalVerb::RaiseWall { width, height, thickness } => {
            e.blocks = raise_wall(target, facing, width, height, thickness, blocks.fill);
        }
        ElementalVerb::RaisePillar { height, radius } => {
            e.blocks = raise_pillar(target, height, radius, blocks.fill);
        }
        ElementalVerb::Smash { radius } => {
            e.blocks = smash_crater(target, radius, blocks.air);
        }
        ElementalVerb::Sphere { radius } => {
            e.blocks = fill_sphere(target, radius, blocks.fluid);
        }
        ElementalVerb::HeatBurst { radius, peak } => {
            e.fields = heat_burst(target, &FieldBrush::new(radius, peak, Falloff::Smooth));
        }
    }
    e
}

/// The world a verb writes into. The interaction layer implements this for the
/// `ChunkManager` (`set_block`) and the temperature field (`add_temperature`);
/// the verbs stay decoupled. Block writes route to `set_block`; the heat verb's
/// field deltas route to `add_temperature`.
pub trait WorldEdit {
    fn set_block(&mut self, x: i32, y: i32, z: i32, block: BlockId);
    fn add_temperature(&mut self, x: i32, y: i32, z: i32, delta: f32);
}

/// Apply a verb's edits to a world.
pub fn apply_edits<W: WorldEdit>(world: &mut W, edits: &VerbEdits) {
    for &(x, y, z, b) in &edits.blocks {
        world.set_block(x, y, z, b);
    }
    for &(x, y, z, d) in &edits.fields {
        world.add_temperature(x, y, z, d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::area_mining::sphere_offsets;
    use std::collections::HashMap;

    #[test]
    fn wall_dimensions() {
        assert_eq!(raise_wall((0, 0, 0), Facing::North, 5, 3, 1, 1u8).len(), 15);
        assert_eq!(raise_wall((0, 0, 0), Facing::South, 3, 4, 2, 1u8).len(), 24);
    }

    #[test]
    fn wall_orientation_north_runs_along_x() {
        let cells: Vec<(i32, i32, i32)> = raise_wall((10, 64, 20), Facing::North, 3, 2, 1, 7u8)
            .into_iter()
            .map(|(x, y, z, _)| (x, y, z))
            .collect();
        let mut got = cells.clone();
        got.sort();
        let mut want = vec![
            (9, 64, 20), (9, 65, 20),
            (10, 64, 20), (10, 65, 20),
            (11, 64, 20), (11, 65, 20),
        ];
        want.sort();
        assert_eq!(got, want);
        // Block carried through.
        assert!(raise_wall((10, 64, 20), Facing::North, 3, 2, 1, 7u8)
            .iter()
            .all(|&(_, _, _, b)| b == 7u8));
    }

    #[test]
    fn wall_facing_east_runs_along_z() {
        let cells: Vec<(i32, i32, i32)> = raise_wall((0, 0, 0), Facing::East, 3, 1, 1, 1u8)
            .into_iter()
            .map(|(x, y, z, _)| (x, y, z))
            .collect();
        let mut got = cells.clone();
        got.sort();
        // right(East) = (0,1) -> width runs along z, centered on 0.
        assert_eq!(got, vec![(0, 0, -1), (0, 0, 0), (0, 0, 1)]);
    }

    #[test]
    fn pillar_counts() {
        assert_eq!(raise_pillar((0, 0, 0), 5, 1, 1u8).len(), 45); // disk(1)=9 * 5
        assert_eq!(raise_pillar((0, 0, 0), 8, 0, 1u8).len(), 8); // disk(0)=1 * 8
    }

    #[test]
    fn crater_is_downward_hemisphere() {
        let edits = smash_crater((0, 10, 0), 2, 0u8);
        assert_eq!(edits.len(), 51);
        assert!(edits.iter().all(|&(_, y, _, _)| y <= 10), "crater carves downward");
        assert!(edits.iter().all(|&(_, _, _, b)| b == 0u8));
        assert_eq!(smash_crater((0, 0, 0), 3, 0u8).len(), 108);
    }

    #[test]
    fn fill_sphere_matches_sphere_offsets() {
        let got: Vec<(i32, i32, i32)> = fill_sphere((0, 0, 0), 2, 9u8)
            .into_iter()
            .map(|(x, y, z, _)| (x, y, z))
            .collect();
        let mut a = got.clone();
        let mut b = sphere_offsets(2);
        a.sort();
        b.sort();
        assert_eq!(a.len(), 81);
        assert_eq!(a, b);
    }

    #[test]
    fn heat_burst_center_is_peak() {
        let edits = heat_burst((5, 5, 5), &FieldBrush::new(2.0, 4.0, Falloff::Smooth));
        assert_eq!(edits.len(), 81);
        let center = edits.iter().find(|&&(x, y, z, _)| (x, y, z) == (5, 5, 5)).unwrap().3;
        assert!((center - 4.0).abs() < 1e-6);
    }

    // A recording world for the apply path (field side, no BlockId needed).
    #[derive(Default)]
    struct RecWorld {
        temps: HashMap<(i32, i32, i32), f32>,
    }
    impl WorldEdit for RecWorld {
        fn set_block(&mut self, _x: i32, _y: i32, _z: i32, _b: BlockId) {}
        fn add_temperature(&mut self, x: i32, y: i32, z: i32, delta: f32) {
            *self.temps.entry((x, y, z)).or_insert(0.0) += delta;
        }
    }

    #[test]
    fn apply_routes_field_edits_to_temperature() {
        let mut w = RecWorld::default();
        let edits = VerbEdits {
            blocks: Vec::new(),
            fields: heat_burst((0, 0, 0), &FieldBrush::new(1.0, 2.0, Falloff::Constant)),
        };
        apply_edits(&mut w, &edits);
        assert_eq!(w.temps.len(), 19); // sphere(1)
        assert!((w.temps[&(0, 0, 0)] - 2.0).abs() < 1e-6);
    }
}
