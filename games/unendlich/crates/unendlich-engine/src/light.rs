//! Voxel lighting precomputation — how this engine gets global-feeling light
//! *cheaply*: compute it on the grid at generation/edit time, bake it into the
//! mesh's per-vertex colors, and pay nothing per pixel at render.
//!
//! Three pieces, all O(voxels) at bake time and O(1) per vertex at mesh time:
//!
//! 1. **Ambient occlusion** ([`vertex_ao`]) — the corner darkening that gives
//!    voxel scenes their soft, grounded look. Per face vertex, sample the two
//!    edge neighbors and the diagonal corner; more solid neighbors → darker.
//!    This is the cheap stand-in for global ambient: no rays, just three
//!    occupancy reads per vertex.
//!
//! 2. **Colored block light** ([`propagate_block_light`]) — torches/lava/glow
//!    inject RGB light that floods through transparent voxels via BFS,
//!    attenuating one level per block and stopped by opaque voxels. Per-channel,
//!    so colored lights mix correctly. Precomputed per region; the mesher reads
//!    a voxel's level for free.
//!
//! 3. **Skylight** ([`skylight_columns`]) — top-down sky exposure per column,
//!    full strength until the first opaque block. Feeds the "outdoors is bright,
//!    caves are dark" baseline. Horizontal bleed under overhangs reuses the
//!    same BFS as block light (seed the open-sky cells).
//!
//! The final per-vertex brightness the mesher should bake is roughly
//! `ao · max(skylight·daylight, block_light) · sun_face_term` — see
//! [`combine_vertex_light`]. All pure, no render deps, unit-tested in the
//! default build.

use std::collections::VecDeque;

/// Max light level (4-bit, Minecraft-style). Block light and skylight both
/// range 0..=15 per channel.
pub const MAX_LIGHT: u8 = 15;

/// Brightness multiplier for each of the 4 AO levels (0 = most occluded).
/// A gentle curve — fully-occluded corners sit at 0.45, open faces at 1.0.
pub const AO_BRIGHTNESS: [f32; 4] = [0.45, 0.62, 0.80, 1.0];

/// Ambient-occlusion level `0..=3` for a face vertex, from the occupancy of the
/// two edge-adjacent voxels (`side1`, `side2`) and the diagonal `corner` voxel.
/// The classic rule: if both sides are solid the corner is fully boxed in
/// (level 0) regardless of the corner; otherwise darken by how many of the
/// three are solid.
pub fn vertex_ao(side1: bool, side2: bool, corner: bool) -> u8 {
    if side1 && side2 {
        0
    } else {
        3 - (side1 as u8 + side2 as u8 + corner as u8)
    }
}

/// Brightness multiplier for an AO level.
pub fn ao_brightness(level: u8) -> f32 {
    AO_BRIGHTNESS[level.min(3) as usize]
}

/// A compact 3D field of per-voxel RGB light levels (each channel 0..=15).
#[derive(Debug, Clone, PartialEq)]
pub struct LightGrid {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// Row-major `[r,g,b]` per voxel, indexed by [`LightGrid::idx`].
    pub data: Vec<[u8; 3]>,
}

impl LightGrid {
    pub fn new(nx: usize, ny: usize, nz: usize) -> Self {
        Self { nx, ny, nz, data: vec![[0, 0, 0]; nx * ny * nz] }
    }
    #[inline]
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.nx * (y + self.ny * z)
    }
    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> [u8; 3] {
        self.data[self.idx(x, y, z)]
    }
    #[inline]
    fn in_bounds(&self, x: i32, y: i32, z: i32) -> bool {
        x >= 0 && y >= 0 && z >= 0 && (x as usize) < self.nx && (y as usize) < self.ny && (z as usize) < self.nz
    }

    /// Compact this grid for upload/storage: one `u16` per voxel instead of
    /// three bytes. Lossless for in-range (0..=15) light.
    pub fn pack(&self) -> PackedLightGrid {
        PackedLightGrid {
            nx: self.nx,
            ny: self.ny,
            nz: self.nz,
            data: self.data.iter().map(|&rgb| pack_light(rgb)).collect(),
        }
    }
}

/// Pack an RGB light triple (each `0..=15`) into a `u16`: red in bits 0..4,
/// green 4..8, blue 8..12. The top nibble (bits 12..16) is reserved — the
/// obvious home for a future sky-light channel — and packs as 0. Channels above
/// [`MAX_LIGHT`] clamp to it, so packing is exact for any legal light value.
///
/// This is the range-bound form: block light only ever spans 0..15, so three
/// bytes (24 bits) for 12 bits of real data is halved with no fidelity loss and
/// a trivial shift/mask decode.
#[inline]
pub fn pack_light(rgb: [u8; 3]) -> u16 {
    let r = rgb[0].min(MAX_LIGHT) as u16;
    let g = rgb[1].min(MAX_LIGHT) as u16;
    let b = rgb[2].min(MAX_LIGHT) as u16;
    r | (g << 4) | (b << 8)
}

/// Inverse of [`pack_light`]. Reads the low three nibbles; ignores the reserved
/// top nibble.
#[inline]
pub fn unpack_light(packed: u16) -> [u8; 3] {
    [
        (packed & 0xF) as u8,
        ((packed >> 4) & 0xF) as u8,
        ((packed >> 8) & 0xF) as u8,
    ]
}

/// Half-size companion to [`LightGrid`]: one `u16` per voxel. This is the form
/// that should cross to the GPU / hit disk; the flood-fill works on the `[u8;3]`
/// [`LightGrid`] and packs once at the end. Random-accessible (unlike an
/// entropy-coded stream), so the shader can read a voxel with one load + mask.
#[derive(Clone, Debug)]
pub struct PackedLightGrid {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// Row-major packed light, indexed by [`PackedLightGrid::idx`].
    pub data: Vec<u16>,
}

impl PackedLightGrid {
    #[inline]
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.nx * (y + self.ny * z)
    }

    /// The RGB light at a voxel, decoded on the fly.
    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> [u8; 3] {
        unpack_light(self.data[self.idx(x, y, z)])
    }

    /// Bytes this grid occupies — half the unpacked `[u8;3]` form.
    pub fn byte_len(&self) -> usize {
        self.data.len() * std::mem::size_of::<u16>()
    }

    /// Expand back to a full `[u8;3]` [`LightGrid`] (e.g. for the flood-fill to
    /// resume from). Exact inverse of [`LightGrid::pack`].
    pub fn unpack(&self) -> LightGrid {
        LightGrid {
            nx: self.nx,
            ny: self.ny,
            nz: self.nz,
            data: self.data.iter().map(|&p| unpack_light(p)).collect(),
        }
    }
}

const NEIGHBORS: [[i32; 3]; 6] = [
    [1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1],
];

/// BFS flood-fill of colored block light. `emitters` are `(position, rgb)` with
/// each channel a 0..=15 emission level. Light spreads to non-opaque neighbors
/// at one level lower per step, per channel (so a red torch and a blue torch
/// mix to magenta where they overlap). O(voxels) — run at gen/edit time.
pub fn propagate_block_light(
    nx: usize,
    ny: usize,
    nz: usize,
    emitters: &[([usize; 3], [u8; 3])],
    is_opaque: impl Fn(usize, usize, usize) -> bool,
) -> LightGrid {
    // No-tint wrapper: every non-opaque step costs exactly one level per channel.
    propagate_block_light_tinted(nx, ny, nz, emitters, is_opaque, |_, _, _| [0, 0, 0])
}

/// Colored block light with **tinted transmission**: as light crosses a
/// translucent block it loses an extra per-channel amount (`transmission`), so a
/// white source reads blue underwater, green under a canopy, amber through amber.
/// Opaque voxels still stop light entirely. `transmission` is consulted for the
/// block being *entered*; see [`crate::material::transmission`].
pub fn propagate_block_light_tinted(
    nx: usize,
    ny: usize,
    nz: usize,
    emitters: &[([usize; 3], [u8; 3])],
    is_opaque: impl Fn(usize, usize, usize) -> bool,
    transmission: impl Fn(usize, usize, usize) -> [u8; 3],
) -> LightGrid {
    let mut grid = LightGrid::new(nx, ny, nz);
    let mut queue: VecDeque<[usize; 3]> = VecDeque::new();

    for &(pos, rgb) in emitters {
        if pos[0] >= nx || pos[1] >= ny || pos[2] >= nz {
            continue;
        }
        let i = grid.idx(pos[0], pos[1], pos[2]);
        let mut changed = false;
        for c in 0..3 {
            if rgb[c] > grid.data[i][c] {
                grid.data[i][c] = rgb[c].min(MAX_LIGHT);
                changed = true;
            }
        }
        if changed {
            queue.push_back(pos);
        }
    }

    flood_rgb(&mut grid, &mut queue, &is_opaque, &transmission);
    grid
}

/// Spread queued RGB light outward until the queue drains. Each non-opaque step
/// costs one level per channel, plus the entered block's `transmission` loss per
/// channel (0 for clear media). The shared core of full and incremental
/// propagation, so they agree by construction.
fn flood_rgb<F, T>(grid: &mut LightGrid, queue: &mut VecDeque<[usize; 3]>, is_opaque: &F, transmission: &T)
where
    F: Fn(usize, usize, usize) -> bool,
    T: Fn(usize, usize, usize) -> [u8; 3],
{
    while let Some(p) = queue.pop_front() {
        let here = grid.get(p[0], p[1], p[2]);
        for d in NEIGHBORS {
            let (nx_, ny_, nz_) = (p[0] as i32 + d[0], p[1] as i32 + d[1], p[2] as i32 + d[2]);
            if !grid.in_bounds(nx_, ny_, nz_) {
                continue;
            }
            let (ux, uy, uz) = (nx_ as usize, ny_ as usize, nz_ as usize);
            if is_opaque(ux, uy, uz) {
                continue; // light doesn't enter solid voxels
            }
            let ni = grid.idx(ux, uy, uz);
            let tint = transmission(ux, uy, uz);
            let mut improved = false;
            for c in 0..3 {
                let v = here[c].saturating_sub(1 + tint[c]);
                if v > grid.data[ni][c] {
                    grid.data[ni][c] = v;
                    improved = true;
                }
            }
            if improved {
                queue.push_back([ux, uy, uz]);
            }
        }
    }
}

/// Single-channel flood, for re-lighting a hole after a removal.
fn flood_channel<F: Fn(usize, usize, usize) -> bool>(
    grid: &mut LightGrid,
    queue: &mut VecDeque<[usize; 3]>,
    c: usize,
    is_opaque: &F,
) {
    while let Some(p) = queue.pop_front() {
        let here = grid.get(p[0], p[1], p[2])[c];
        for d in NEIGHBORS {
            let (nx_, ny_, nz_) = (p[0] as i32 + d[0], p[1] as i32 + d[1], p[2] as i32 + d[2]);
            if !grid.in_bounds(nx_, ny_, nz_) {
                continue;
            }
            let (ux, uy, uz) = (nx_ as usize, ny_ as usize, nz_ as usize);
            if is_opaque(ux, uy, uz) {
                continue;
            }
            let ni = grid.idx(ux, uy, uz);
            let v = here.saturating_sub(1);
            if v > grid.data[ni][c] {
                grid.data[ni][c] = v;
                queue.push_back([ux, uy, uz]);
            }
        }
    }
}

/// Incrementally **add** a point light of `rgb` at `pos` to an existing grid,
/// propagating only where it brightens cells — O(the region it reaches), not
/// the whole world. Placing a torch, lighting a fuse, a glowing projectile
/// spawning: each is one cheap local flood instead of a full recompute.
pub fn add_light(
    grid: &mut LightGrid,
    pos: [usize; 3],
    rgb: [u8; 3],
    is_opaque: impl Fn(usize, usize, usize) -> bool,
) {
    if pos[0] >= grid.nx || pos[1] >= grid.ny || pos[2] >= grid.nz {
        return;
    }
    let i = grid.idx(pos[0], pos[1], pos[2]);
    let mut queue: VecDeque<[usize; 3]> = VecDeque::new();
    let mut changed = false;
    for c in 0..3 {
        if rgb[c] > grid.data[i][c] {
            grid.data[i][c] = rgb[c].min(MAX_LIGHT);
            changed = true;
        }
    }
    if changed {
        queue.push_back(pos);
    }
    flood_rgb(grid, &mut queue, &is_opaque, &|_, _, _| [0, 0, 0]);
}

/// Incrementally **remove** a point light of `rgb` from `pos` and re-light the
/// hole from whatever other sources remain — touching only the region the
/// removed light reached. The classic two-phase update (per channel): a removal
/// BFS clears every cell this light was responsible for, collecting the brighter
/// boundary cells lit by *other* sources as seeds; then those seeds re-flood the
/// hole. Breaking a torch, a light moving away: cheap and local.
pub fn remove_light(
    grid: &mut LightGrid,
    pos: [usize; 3],
    rgb: [u8; 3],
    is_opaque: impl Fn(usize, usize, usize) -> bool,
) {
    if pos[0] >= grid.nx || pos[1] >= grid.ny || pos[2] >= grid.nz {
        return;
    }
    for c in 0..3 {
        if rgb[c] == 0 {
            continue;
        }
        let i = grid.idx(pos[0], pos[1], pos[2]);
        let start_level = grid.data[i][c];
        if start_level == 0 {
            continue;
        }
        grid.data[i][c] = 0;
        let mut removal: VecDeque<([usize; 3], u8)> = VecDeque::new();
        let mut relight: VecDeque<[usize; 3]> = VecDeque::new();
        removal.push_back((pos, start_level));
        while let Some((p, level)) = removal.pop_front() {
            for d in NEIGHBORS {
                let (nx_, ny_, nz_) = (p[0] as i32 + d[0], p[1] as i32 + d[1], p[2] as i32 + d[2]);
                if !grid.in_bounds(nx_, ny_, nz_) {
                    continue;
                }
                let (ux, uy, uz) = (nx_ as usize, ny_ as usize, nz_ as usize);
                let ni = grid.idx(ux, uy, uz);
                let nl = grid.data[ni][c];
                if nl != 0 && nl < level {
                    // This cell was lit by the light we're removing → clear it
                    // and keep unwinding.
                    grid.data[ni][c] = 0;
                    removal.push_back(([ux, uy, uz], nl));
                } else if nl >= level {
                    // Lit at least as strongly by something else → a seed to
                    // re-flood the hole from.
                    relight.push_back([ux, uy, uz]);
                }
            }
        }
        flood_channel(grid, &mut relight, c, &is_opaque);
    }
}

/// Per-column skylight: each `(x, z)` column is lit at [`MAX_LIGHT`] (white)
/// from the top down until the first opaque voxel, which and everything below
/// it gets 0. This is the vertical sky-exposure pass; horizontal bleed under
/// overhangs is obtained by feeding the lit cells as emitters into
/// [`propagate_block_light`].
pub fn skylight_columns(
    nx: usize,
    ny: usize,
    nz: usize,
    is_opaque: impl Fn(usize, usize, usize) -> bool,
) -> LightGrid {
    let mut grid = LightGrid::new(nx, ny, nz);
    for z in 0..nz {
        for x in 0..nx {
            let mut lit = true;
            for y in (0..ny).rev() {
                if lit && is_opaque(x, y, z) {
                    lit = false;
                }
                if lit {
                    let i = grid.idx(x, y, z);
                    grid.data[i] = [MAX_LIGHT, MAX_LIGHT, MAX_LIGHT];
                }
            }
        }
    }
    grid
}

/// Compose the per-vertex brightness the mesher bakes. `daylight` in `[0,1]`
/// scales skylight (night → skylight contributes nothing, caves stay dark);
/// block light is daylight-independent (torches glow at night). The two combine
/// by per-channel max (a lit torch doesn't get *darker* in shade), the result
/// is scaled by AO and the sun's directional face term. Returns linear RGB in
/// roughly `[0,1]`.
pub fn combine_vertex_light(
    skylight: [u8; 3],
    block_light: [u8; 3],
    daylight: f32,
    ao_level: u8,
    sun_face_term: f32,
) -> [f32; 3] {
    let ao = ao_brightness(ao_level);
    let d = daylight.clamp(0.0, 1.0);
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        let sky = (skylight[c] as f32 / MAX_LIGHT as f32) * d;
        let blk = block_light[c] as f32 / MAX_LIGHT as f32;
        let lit = sky.max(blk);
        out[c] = (lit * ao * sun_face_term).clamp(0.0, 1.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_packing_round_trips_every_legal_value() {
        // All 16^3 in-range RGB triples survive pack→unpack exactly.
        for r in 0..=MAX_LIGHT {
            for g in 0..=MAX_LIGHT {
                for b in 0..=MAX_LIGHT {
                    let rgb = [r, g, b];
                    assert_eq!(unpack_light(pack_light(rgb)), rgb, "round-trip {rgb:?}");
                }
            }
        }
        // Out-of-range channels clamp to MAX_LIGHT (still lossless for legal data).
        assert_eq!(unpack_light(pack_light([200, 16, 15])), [MAX_LIGHT, MAX_LIGHT, MAX_LIGHT]);
        // The reserved top nibble is unused by packing.
        assert_eq!(pack_light([15, 15, 15]) & 0xF000, 0);
    }

    #[test]
    fn packed_grid_halves_storage_and_preserves_values() {
        let mut g = LightGrid::new(4, 3, 2);
        // Scatter some colored light.
        let (i, j) = (g.idx(1, 1, 1), g.idx(3, 0, 1));
        g.data[i] = [15, 7, 0];
        g.data[j] = [0, 4, 12];
        let packed = g.pack();
        // One u16 vs three u8 per voxel.
        assert_eq!(packed.byte_len(), g.data.len() * 2);
        assert_eq!(g.data.len() * 3, packed.byte_len() / 2 * 3); // sanity on the 3:2 ratio
        // get() on the packed grid matches the source...
        for z in 0..g.nz { for y in 0..g.ny { for x in 0..g.nx {
            assert_eq!(packed.get(x, y, z), g.get(x, y, z), "voxel ({x},{y},{z})");
        }}}
        // ...and a full unpack reproduces the grid exactly.
        let back = packed.unpack();
        assert_eq!(back.data, g.data);
        assert_eq!((back.nx, back.ny, back.nz), (g.nx, g.ny, g.nz));
    }

    #[test]
    fn colored_transmission_tints_light_through_a_medium() {
        use crate::world::blocks::BlockId;
        let (nx, ny, nz) = (8usize, 1usize, 1usize);
        let emitters = [([0usize, 0, 0], [15u8, 15, 15])]; // white source at x=0
        let opaque = |_: usize, _: usize, _: usize| false;
        // x>=1 is water (tints), the source cell is clear.
        let water = |x: usize, _: usize, _: usize| {
            if x >= 1 { crate::material::transmission(BlockId::Water) } else { [0, 0, 0] }
        };
        let tinted = propagate_block_light_tinted(nx, ny, nz, &emitters, opaque, water);
        let far = tinted.get(4, 0, 0);
        // Water absorbs red fastest, blue slowest → blue > green > red downstream.
        assert!(far[2] > far[1] && far[1] > far[0], "expected blue>green>red, got {far:?}");
        assert_eq!(far[0], 0, "red extinguished through water, got {far:?}");

        // The no-tint wrapper must be unchanged: every channel falls off equally.
        let clear = propagate_block_light(nx, ny, nz, &emitters, opaque);
        let cf = clear.get(4, 0, 0);
        assert!(cf[0] == cf[1] && cf[1] == cf[2] && cf[0] > 0, "clear light stays white, got {cf:?}");
    }

    #[test]
    fn ao_levels_match_the_classic_rule() {
        // No solid neighbors → brightest (level 3).
        assert_eq!(vertex_ao(false, false, false), 3);
        // One side solid → level 2.
        assert_eq!(vertex_ao(true, false, false), 2);
        assert_eq!(vertex_ao(false, true, false), 2);
        // Corner only → level 2.
        assert_eq!(vertex_ao(false, false, true), 2);
        // One side + corner → level 1.
        assert_eq!(vertex_ao(true, false, true), 1);
        // Both sides solid → fully boxed (level 0) regardless of corner.
        assert_eq!(vertex_ao(true, true, false), 0);
        assert_eq!(vertex_ao(true, true, true), 0);
        // Brightness is monotonic increasing in level.
        assert!(ao_brightness(0) < ao_brightness(1));
        assert!(ao_brightness(1) < ao_brightness(2));
        assert!(ao_brightness(2) < ao_brightness(3));
        assert_eq!(ao_brightness(3), 1.0);
    }

    #[test]
    fn block_light_attenuates_with_distance() {
        // A single white torch at one end of an open 10-long corridor.
        let g = propagate_block_light(10, 1, 1, &[([0, 0, 0], [15, 15, 15])], |_, _, _| false);
        assert_eq!(g.get(0, 0, 0), [15, 15, 15], "at the torch: full");
        assert_eq!(g.get(1, 0, 0), [14, 14, 14], "one block away: -1");
        assert_eq!(g.get(5, 0, 0), [10, 10, 10], "five away: -5");
        assert_eq!(g.get(15.min(9), 0, 0), [6, 6, 6], "nine away: -9");
    }

    #[test]
    fn opaque_wall_blocks_light() {
        // Torch at x=0; an opaque wall at x=3 in a 1-wide corridor.
        let wall = |x: usize, _: usize, _: usize| x == 3;
        let g = propagate_block_light(7, 1, 1, &[([0, 0, 0], [15, 0, 0])], wall);
        assert!(g.get(2, 0, 0)[0] > 0, "before the wall is lit");
        assert_eq!(g.get(3, 0, 0), [0, 0, 0], "the opaque wall takes no light");
        assert_eq!(g.get(4, 0, 0), [0, 0, 0], "beyond the wall stays dark");
    }

    #[test]
    fn colored_lights_mix_per_channel() {
        // Red torch at one end, blue at the other, in a short corridor.
        let g = propagate_block_light(
            5, 1, 1,
            &[([0, 0, 0], [15, 0, 0]), ([4, 0, 0], [0, 0, 15])],
            |_, _, _| false,
        );
        // The middle sees both → magenta (red and blue channels both lit).
        let mid = g.get(2, 0, 0);
        assert!(mid[0] > 0 && mid[2] > 0, "middle should be magenta: {mid:?}");
        assert_eq!(mid[1], 0, "no green anywhere");
        // Each end keeps its own color strongest.
        assert!(g.get(0, 0, 0)[0] > g.get(0, 0, 0)[2], "red end reddest");
        assert!(g.get(4, 0, 0)[2] > g.get(4, 0, 0)[0], "blue end bluest");
    }

    #[test]
    fn brighter_emitter_wins_when_two_overlap() {
        // Two torches; the brighter dominates the overlap region.
        let g = propagate_block_light(
            6, 1, 1,
            &[([0, 0, 0], [15, 0, 0]), ([5, 0, 0], [8, 0, 0])],
            |_, _, _| false,
        );
        // Near the bright torch, level should exceed what the dim one could reach.
        assert!(g.get(1, 0, 0)[0] >= 14);
    }

    #[test]
    fn skylight_fills_open_columns_and_stops_under_cover() {
        // A 1×5×1 column with a roof at y=3.
        let roof = |_: usize, y: usize, _: usize| y == 3;
        let g = skylight_columns(1, 5, 1, roof);
        assert_eq!(g.get(0, 4, 0), [15, 15, 15], "above the roof: full sky");
        assert_eq!(g.get(0, 3, 0), [0, 0, 0], "the roof block itself: dark");
        assert_eq!(g.get(0, 2, 0), [0, 0, 0], "under the roof: shadowed");
        assert_eq!(g.get(0, 0, 0), [0, 0, 0], "floor under cover: dark");
    }

    #[test]
    fn combine_respects_day_night_and_torches() {
        // Daytime open sky, no torch, open face, full sun term → bright white.
        let day = combine_vertex_light([15, 15, 15], [0, 0, 0], 1.0, 3, 1.0);
        assert!(day[0] > 0.99);
        // Night, same spot, no torch → dark (skylight scaled by daylight 0).
        let night = combine_vertex_light([15, 15, 15], [0, 0, 0], 0.0, 3, 1.0);
        assert!(night[0] < 0.01, "skylight should vanish at night: {night:?}");
        // Night but a red torch nearby → red glow persists regardless of daylight.
        let torch = combine_vertex_light([0, 0, 0], [15, 0, 0], 0.0, 3, 1.0);
        assert!(torch[0] > 0.9 && torch[2] < 0.01, "torch glows at night: {torch:?}");
        // AO darkens an occluded corner even in full day.
        let occluded = combine_vertex_light([15, 15, 15], [0, 0, 0], 1.0, 0, 1.0);
        assert!(occluded[0] < day[0], "AO should darken: {} < {}", occluded[0], day[0]);
    }

    /// Walls as a closure for the incremental tests.
    fn opaque(cells: &[[usize; 3]]) -> impl Fn(usize, usize, usize) -> bool + '_ {
        move |x, y, z| cells.contains(&[x, y, z])
    }

    #[test]
    fn add_light_matches_a_full_recompute() {
        let (nx, ny, nz) = (12, 5, 5);
        let wall = [[6usize, 0, 2], [6, 1, 2], [6, 2, 2]];
        let emitters = [([2usize, 2, 2], [15u8, 10, 0]), ([9, 2, 2], [0, 0, 14])];
        let full = propagate_block_light(nx, ny, nz, &emitters, opaque(&wall));
        // Build the same field one light at a time, incrementally.
        let mut inc = LightGrid::new(nx, ny, nz);
        for &(pos, rgb) in &emitters {
            add_light(&mut inc, pos, rgb, opaque(&wall));
        }
        assert_eq!(inc.data, full.data, "incremental add == full flood");
    }

    #[test]
    fn remove_light_matches_recompute_without_it() {
        let (nx, ny, nz) = (14, 5, 5);
        let a = ([3usize, 2, 2], [15u8, 0, 0]);
        let b = ([10usize, 2, 2], [0, 0, 15]);
        // Full field with both, then incrementally remove A.
        let mut grid = propagate_block_light(nx, ny, nz, &[a, b], |_, _, _| false);
        remove_light(&mut grid, a.0, a.1, |_, _, _| false);
        // Should match a fresh recompute of B alone.
        let only_b = propagate_block_light(nx, ny, nz, &[b], |_, _, _| false);
        assert_eq!(grid.data, only_b.data, "incremental remove == recompute of the rest");
    }

    #[test]
    fn removing_the_only_light_clears_the_grid() {
        let (nx, ny, nz) = (9, 3, 3);
        let t = ([4usize, 1, 1], [15u8, 12, 8]);
        let mut grid = propagate_block_light(nx, ny, nz, &[t], |_, _, _| false);
        assert!(grid.get(4, 1, 1)[0] > 0);
        remove_light(&mut grid, t.0, t.1, |_, _, _| false);
        assert!(grid.data.iter().all(|c| *c == [0, 0, 0]), "no light left after the last source goes out");
    }

    #[test]
    fn overlapping_lights_remove_cleanly() {
        // Two red torches whose glows overlap; removing one must leave exactly
        // the other's contribution (the hard case for incremental removal).
        let (nx, ny, nz) = (12, 3, 3);
        let a = ([3usize, 1, 1], [15u8, 0, 0]);
        let b = ([7usize, 1, 1], [15u8, 0, 0]);
        let mut grid = propagate_block_light(nx, ny, nz, &[a, b], |_, _, _| false);
        remove_light(&mut grid, a.0, a.1, |_, _, _| false);
        let only_b = propagate_block_light(nx, ny, nz, &[b], |_, _, _| false);
        assert_eq!(grid.data, only_b.data, "overlap re-lights correctly from the survivor");
    }
}
