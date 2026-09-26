//! Budgeted damage rendering for the software (CPU) presenter.
//!
//! Converting NV12 to RGBA and re-uploading the whole frame every frame is
//! the CPU presenter's cost, and most of it repeats work: a lecture's board,
//! a game's HUD and minimap, letterbox bars -- unchanged pixels, converted
//! and uploaded again. This keeps a tile tree over the frame:
//!
//! - **Leaves** are 16x16 tiles, each with an exact hash of its luma and
//!   chroma and a **temporal link**: how many frames it has been unchanged,
//!   and at what level of detail it is on screen now.
//! - **Damage** is the set of leaves whose hash changed. Only they are
//!   converted and uploaded.
//! - **Budget.** A frame may spend a fixed number of pixel conversions. When
//!   the damage does not fit at full detail, damaged tiles drop to a coarser
//!   LOD -- 1/4 or 1/16 of the samples, each converted once and replicated
//!   into a block (the "pixel compression"). Damage is never skipped:
//!   stale pixels behind motion are worse than temporary blur.
//! - **Refinement.** Whatever budget is left refines static tiles still
//!   showing a coarse LOD, oldest first, so a scene that stops moving
//!   converges back to full quality, and a static scene costs nothing.
//! - **Rects.** Tiles are merged into horizontal runs, and identical runs on
//!   consecutive rows into rectangles, so uploads are few and large.
//!
//! Pure: hashing, planning and region conversion take slices and return
//! data. The presenter owns the texture.

use crate::frame::{ColorInfo, CpuPlanes, PlaneLayout};

pub const TILE: u32 = 16;
/// Coarsest level: 1/2^3 per axis -- one sample per 8x8 block.
pub const LOD_MAX: u8 = 3;
const NEVER: u8 = u8::MAX;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Leaf {
    pub hash: u64,
    /// Frames since the tile last changed.
    pub age: u32,
    /// Detail on screen: 0 full, n = 1/2^n per axis; NEVER = not drawn yet.
    pub shown: u8,
}

/// One region to convert and upload.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Op {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub lod: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub ops: Vec<Op>,
    /// Leaves whose content changed this frame.
    pub damaged: usize,
    /// Static leaves brought up to full detail this frame.
    pub refined: usize,
    /// The LOD damaged tiles were drawn at.
    pub damage_lod: u8,
    /// Pixel conversions this plan costs.
    pub cost: u64,
    pub leaves: usize,
}

#[derive(Clone, Debug)]
pub struct TileTree {
    pub width: u32,
    pub height: u32,
    cols: u32,
    rows: u32,
    leaves: Vec<Leaf>,
}

/// A 64-bit hash over bytes, eight at a time. Not cryptographic: a
/// collision shows as one stale tile until it next changes, and at 2^-64 per
/// comparison that is not a failure mode worth paying a slower hash for.
fn hash_bytes(mut h: u64, b: &[u8]) -> u64 {
    let mut chunks = b.chunks_exact(8);
    for c in &mut chunks {
        let v = u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
        h = (h ^ v).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(29);
    }
    for &x in chunks.remainder() {
        h = (h ^ x as u64).wrapping_mul(0x0100_0000_01B3);
    }
    h
}

impl TileTree {
    pub fn new(width: u32, height: u32) -> Self {
        let (cols, rows) = (width.div_ceil(TILE), height.div_ceil(TILE));
        Self { width, height, cols, rows, leaves: vec![Leaf { hash: 0, age: 0, shown: NEVER }; (cols * rows) as usize] }
    }

    pub fn leaf(&self, col: u32, row: u32) -> Leaf {
        self.leaves[(row * self.cols + col) as usize]
    }

    fn tile_rect(&self, i: usize) -> (u32, u32, u32, u32) {
        let (c, r) = (i as u32 % self.cols, i as u32 / self.cols);
        let (x, y) = (c * TILE, r * TILE);
        (x, y, TILE.min(self.width - x), TILE.min(self.height - y))
    }

    /// Hash every leaf of a 2-plane frame (NV12 or P010: `bps` bytes a
    /// sample, chroma interleaved at half resolution).
    pub fn hash_frame(&self, planes: &CpuPlanes, bps: u32) -> Vec<u64> {
        let (ys, cs) = (planes.strides[0] as usize, planes.strides.get(1).copied().unwrap_or(planes.strides[0]) as usize);
        (0..self.leaves.len())
            .map(|i| {
                let (x, y, w, h) = self.tile_rect(i);
                let mut hsh = 0xcbf2_9ce4_8422_2325u64;
                for row in y..y + h {
                    let o = row as usize * ys + (x * bps) as usize;
                    hsh = hash_bytes(hsh, planes.data[0].get(o..o + (w * bps) as usize).unwrap_or(&[]));
                }
                // Chroma: half resolution, two samples a texel.
                for row in (y / 2)..(y + h).div_ceil(2) {
                    let o = row as usize * cs + (x / 2 * 2 * bps) as usize;
                    let n = (w.div_ceil(2) * 2 * bps) as usize;
                    hsh = hash_bytes(hsh, planes.data.get(1).and_then(|p| p.get(o..o + n)).unwrap_or(&[]));
                }
                hsh
            })
            .collect()
    }

    /// Conversions a tile of `w x h` costs at `lod`.
    fn cost(w: u32, h: u32, lod: u8) -> u64 {
        (w.div_ceil(1 << lod) as u64) * (h.div_ceil(1 << lod) as u64)
    }

    /// Plan this frame's work within `budget` pixel conversions. Updates the
    /// temporal links.
    pub fn plan(&mut self, hashes: &[u64], budget: u64) -> Plan {
        assert_eq!(hashes.len(), self.leaves.len(), "hashes for a different tree");
        let mut damaged = Vec::new();
        for (i, (leaf, &h)) in self.leaves.iter_mut().zip(hashes).enumerate() {
            if leaf.hash != h || leaf.shown == NEVER {
                leaf.hash = h;
                leaf.age = 0;
                damaged.push(i);
            } else {
                leaf.age = leaf.age.saturating_add(1);
            }
        }
        // One LOD for all damage: mixed detail inside moving content reads
        // as flicker. The finest that fits; the coarsest if nothing does.
        let cost_at = |lod: u8| -> u64 {
            damaged.iter().map(|&i| {
                let (_, _, w, h) = self.tile_rect(i);
                Self::cost(w, h, lod)
            }).sum()
        };
        let lod = (0..=LOD_MAX).find(|&l| cost_at(l) <= budget).unwrap_or(LOD_MAX);
        let mut cost = cost_at(lod);
        let mut chosen: Vec<(usize, u8)> = damaged.iter().map(|&i| (i, lod)).collect();

        // Refinement: static tiles not at full detail, oldest first.
        let mut stale: Vec<usize> = (0..self.leaves.len())
            .filter(|i| self.leaves[*i].age > 0 && self.leaves[*i].shown != 0 && self.leaves[*i].shown != NEVER)
            .collect();
        stale.sort_by(|a, b| self.leaves[*b].age.cmp(&self.leaves[*a].age));
        let mut refined = 0;
        for i in stale {
            let (_, _, w, h) = self.tile_rect(i);
            let c = Self::cost(w, h, 0);
            if cost + c > budget {
                break;
            }
            cost += c;
            chosen.push((i, 0));
            refined += 1;
        }
        for &(i, l) in &chosen {
            self.leaves[i].shown = l;
        }
        Plan { ops: self.merge(&chosen), damaged: damaged.len(), refined, damage_lod: lod, cost, leaves: self.leaves.len() }
    }

    /// Tiles into rectangles: horizontal runs of the same LOD, then
    /// identical runs on consecutive rows stacked.
    fn merge(&self, chosen: &[(usize, u8)]) -> Vec<Op> {
        let mut grid: Vec<Option<u8>> = vec![None; self.leaves.len()];
        for &(i, l) in chosen {
            grid[i] = Some(l);
        }
        let mut runs: Vec<(u32, u32, u32, u8)> = Vec::new(); // (row, c0, c1 excl, lod)
        for r in 0..self.rows {
            let mut c = 0;
            while c < self.cols {
                if let Some(l) = grid[(r * self.cols + c) as usize] {
                    let start = c;
                    while c < self.cols && grid[(r * self.cols + c) as usize] == Some(l) {
                        c += 1;
                    }
                    runs.push((r, start, c, l));
                } else {
                    c += 1;
                }
            }
        }
        let mut ops: Vec<(u32, u32, u32, u32, u8)> = Vec::new(); // (r0, r1 excl, c0, c1, lod)
        for (r, c0, c1, l) in runs {
            match ops.iter_mut().rev().find(|o| o.1 == r && o.2 == c0 && o.3 == c1 && o.4 == l) {
                Some(o) => o.1 = r + 1,
                None => ops.push((r, r + 1, c0, c1, l)),
            }
        }
        ops.into_iter()
            .map(|(r0, r1, c0, c1, l)| {
                let (x, y) = (c0 * TILE, r0 * TILE);
                Op { x, y, w: (c1 * TILE).min(self.width) - x, h: (r1 * TILE).min(self.height) - y, lod: l }
            })
            .collect()
    }
}

/// Convert one region of a 2-plane frame into `out` (RGBA8, the whole
/// frame, `width * 4` bytes a row) at `lod`: one sample per 2^lod block,
/// replicated. Reuses `planes_to_rgba8` on a sampled copy of the region, so
/// the colour maths (range, matrix, HDR) has exactly one definition.
pub fn convert_region(
    planes: &CpuPlanes,
    layout: PlaneLayout,
    color: ColorInfo,
    width: u32,
    op: Op,
    out: &mut [u8],
) -> Result<(), crate::convert_cpu::ConvertError> {
    let bps: usize = if layout == PlaneLayout::P010 { 2 } else { 1 };
    let step = 1u32 << op.lod;
    // Sampled region dims, even so chroma pairs line up.
    let (sw, sh) = (op.w.div_ceil(step).max(2).next_multiple_of(2), op.h.div_ceil(step).max(2).next_multiple_of(2));
    let (ys, cs) = (planes.strides[0] as usize, planes.strides[1] as usize);
    let fw = width as usize;
    let mut luma = Vec::with_capacity(sw as usize * sh as usize * bps);
    for j in 0..sh {
        let y = (op.y + (j * step).min(op.h - 1)) as usize;
        for i in 0..sw {
            let x = (op.x + (i * step).min(op.w - 1)) as usize;
            let o = y * ys + x.min(fw - 1) * bps;
            luma.extend_from_slice(planes.data[0].get(o..o + bps).unwrap_or(&[0, 0][..bps]));
        }
    }
    let (cw, ch) = (sw / 2, sh / 2);
    let mut chroma = Vec::with_capacity(cw as usize * ch as usize * 2 * bps);
    for j in 0..ch {
        let y = ((op.y + (j * 2 * step).min(op.h - 1)) / 2) as usize;
        for i in 0..cw {
            let x = ((op.x + (i * 2 * step).min(op.w - 1)) / 2) as usize;
            let o = y * cs + x * 2 * bps;
            chroma.extend_from_slice(planes.data[1].get(o..o + 2 * bps).unwrap_or(&[128, 128, 128, 128][..2 * bps]));
        }
    }
    let sub = CpuPlanes { data: vec![luma, chroma], strides: vec![sw * bps as u32, sw * bps as u32] };
    let rgba = crate::convert_cpu::planes_to_rgba8(&sub, layout, color, sw as usize, sh as usize)?;
    for yy in 0..op.h {
        let sj = (yy / step) as usize;
        let row = ((op.y + yy) * width * 4) as usize;
        for xx in 0..op.w {
            let si = (xx / step) as usize;
            let s = (sj * sw as usize + si) * 4;
            let d = row + ((op.x + xx) * 4) as usize;
            if let (Some(dst), Some(src)) = (out.get_mut(d..d + 4), rgba.get(s..s + 4)) {
                dst.copy_from_slice(src);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> CpuPlanes {
        let mut y = Vec::new();
        for j in 0..h {
            for i in 0..w {
                y.push(f(i, j));
            }
        }
        let c = vec![128u8; (w.div_ceil(2) * 2 * h.div_ceil(2)) as usize];
        CpuPlanes { data: vec![y, c], strides: vec![w, w] }
    }

    const W: u32 = 128;
    const H: u32 = 64; // 8 x 4 = 32 leaves

    #[test]
    fn the_first_frame_is_all_damage_and_a_repeat_is_none() {
        let mut t = TileTree::new(W, H);
        let f = frame(W, H, |x, y| (x * 3 + y) as u8);
        let p = t.plan(&t.hash_frame(&f, 1), u64::MAX);
        assert_eq!((p.damaged, p.damage_lod, p.cost), (32, 0, (W * H) as u64));
        assert_eq!(p.ops, vec![Op { x: 0, y: 0, w: W, h: H, lod: 0 }], "one merged rect");
        let q = t.plan(&t.hash_frame(&f, 1), u64::MAX);
        assert_eq!((q.damaged, q.cost), (0, 0), "a static frame costs nothing");
        assert!(q.ops.is_empty());
        assert_eq!(t.leaf(3, 2).age, 1);
    }

    #[test]
    fn only_the_changed_tile_is_redrawn() {
        let mut t = TileTree::new(W, H);
        let a = frame(W, H, |x, y| (x ^ y) as u8);
        t.plan(&t.hash_frame(&a, 1), u64::MAX);
        let b = frame(W, H, |x, y| if (40..44).contains(&x) && (20..22).contains(&y) { 255 } else { (x ^ y) as u8 });
        let p = t.plan(&t.hash_frame(&b, 1), u64::MAX);
        assert_eq!(p.damaged, 1);
        assert_eq!(p.ops, vec![Op { x: 32, y: 16, w: 16, h: 16, lod: 0 }]);
    }

    /// Chroma alone changing is damage too (a colour flash on flat luma).
    #[test]
    fn a_chroma_only_change_is_damage() {
        let mut t = TileTree::new(W, H);
        let a = frame(W, H, |_, _| 100);
        t.plan(&t.hash_frame(&a, 1), u64::MAX);
        let mut b = CpuPlanes { data: a.data.clone(), strides: a.strides.clone() };
        b.data[1][(10 * W + 20) as usize] = 200; // chroma row 10, texel 10 -> luma tile (1, 1)
        let p = t.plan(&t.hash_frame(&b, 1), u64::MAX);
        assert_eq!(p.damaged, 1);
        assert_eq!((p.ops[0].x, p.ops[0].y), (16, 16));
    }

    /// Full motion over budget: all damage drawn, at the finest LOD that
    /// fits; when it stops, refinement restores full detail over frames.
    #[test]
    fn over_budget_motion_degrades_then_refines_back_to_full() {
        let mut t = TileTree::new(W, H);
        let budget = (W * H / 4) as u64; // a quarter of a full frame
        for k in 0..3u32 {
            let f = frame(W, H, |x, y| (x + y + k * 7) as u8);
            let p = t.plan(&t.hash_frame(&f, 1), budget);
            assert_eq!(p.damaged, 32, "all damage drawn, never skipped");
            assert_eq!(p.damage_lod, 1, "half resolution fits a quarter budget exactly");
            assert!(p.cost <= budget);
        }
        // Motion stops: refine oldest first within the budget each frame.
        let still = frame(W, H, |x, y| (x + y + 2 * 7) as u8);
        let mut frames = 0;
        loop {
            let p = t.plan(&t.hash_frame(&still, 1), budget);
            assert!(p.cost <= budget);
            frames += 1;
            if (0..8).all(|c| (0..4).all(|r| t.leaf(c, r).shown == 0)) {
                break;
            }
            assert!(frames < 10, "never converged");
        }
        assert_eq!(frames, 4, "a quarter of the tiles per frame: 4 frames to full detail");
        assert_eq!(t.plan(&t.hash_frame(&still, 1), budget).cost, 0, "then free");
    }

    #[test]
    fn a_budget_nothing_fits_uses_the_coarsest_lod_rather_than_skipping() {
        let mut t = TileTree::new(W, H);
        let p = t.plan(&t.hash_frame(&frame(W, H, |x, _| x as u8), 1), 1);
        assert_eq!((p.damaged, p.damage_lod), (32, LOD_MAX));
    }

    #[test]
    fn edge_tiles_are_clipped_to_the_frame() {
        let (w, h) = (40, 20);
        let mut t = TileTree::new(w, h);
        let p = t.plan(&t.hash_frame(&frame(w, h, |x, y| (x * y) as u8), 1), u64::MAX);
        assert_eq!(p.ops, vec![Op { x: 0, y: 0, w, h, lod: 0 }]);
        assert_eq!(p.cost, (w * h) as u64);
    }

    /// Region conversion at full detail matches the whole-frame converter
    /// exactly; at a coarse LOD every pixel of a block is one converted sample.
    #[test]
    fn region_conversion_matches_the_whole_frame_and_replicates_blocks() {
        let f = frame(W, H, |x, y| (16 + (x * 5 + y * 3) % 200) as u8);
        let c = ColorInfo::default();
        let whole = crate::convert_cpu::planes_to_rgba8(&f, PlaneLayout::Nv12, c, W as usize, H as usize).unwrap();
        let mut out = vec![0u8; (W * H * 4) as usize];
        convert_region(&f, PlaneLayout::Nv12, c, W, Op { x: 32, y: 16, w: 32, h: 16, lod: 0 }, &mut out).unwrap();
        for y in 16..32 {
            for x in 32..64 {
                let i = ((y * W + x) * 4) as usize;
                assert_eq!(&out[i..i + 3], &whole[i..i + 3], "at {x},{y}");
            }
        }
        assert_eq!(&out[0..4], &[0, 0, 0, 0], "outside the region untouched");
        let mut coarse = vec![0u8; (W * H * 4) as usize];
        convert_region(&f, PlaneLayout::Nv12, c, W, Op { x: 0, y: 0, w: 16, h: 16, lod: 2 }, &mut coarse).unwrap();
        let px = |x: u32, y: u32| coarse[((y * W + x) * 4) as usize];
        assert!((0..4).all(|y| (0..4).all(|x| px(x, y) == px(0, 0))), "a 4x4 block is one sample");
        assert_eq!(px(0, 0), whole[0], "the sample is the block's corner pixel, converted");
    }

    /// `cargo test --release -p vidkit-core --lib damage_bench -- --ignored --nocapture`
    /// Where the damage path spends its time, per stage, at three sizes.
    #[test]
    #[ignore]
    fn damage_bench() {
        use std::time::Instant;
        for (w, h) in [(640u32, 360u32), (1920, 1080), (3840, 2160)] {
            let mk = |k: u32| frame(w, h, |x, y| ((x.wrapping_mul(7) ^ y.wrapping_mul(13)).wrapping_add(k)) as u8);
            let (a, b) = (mk(0), mk(1));
            let mut t = TileTree::new(w, h);
            let reps = (40_000_000 / (w * h)).max(3);
            let time = |f: &mut dyn FnMut()| {
                f();
                let s = Instant::now();
                for _ in 0..reps { f(); }
                s.elapsed().as_secs_f64() / reps as f64 * 1e3
            };
            let hash_ms = time(&mut || { std::hint::black_box(t.hash_frame(&a, 1)); });
            let hb = t.hash_frame(&b, 1);
            let plan_ms = time(&mut || { t.leaves.iter_mut().for_each(|l| l.shown = NEVER); std::hint::black_box(t.plan(&hb, u64::MAX)); });
            let mut out = vec![0u8; (w * h * 4) as usize];
            let full = Op { x: 0, y: 0, w, h, lod: 0 };
            let conv0 = time(&mut || { convert_region(&a, PlaneLayout::Nv12, ColorInfo::default(), w, full, &mut out).unwrap(); });
            let conv1 = time(&mut || { convert_region(&a, PlaneLayout::Nv12, ColorInfo::default(), w, Op { lod: 1, ..full }, &mut out).unwrap(); });
            let whole = time(&mut || { std::hint::black_box(crate::convert_cpu::planes_to_rgba8(&a, PlaneLayout::Nv12, ColorInfo::default(), w as usize, h as usize).unwrap()); });
            // Tiled conversion: the same frame as one op per 16x16 tile.
            let tiles: Vec<Op> = (0..h.div_ceil(TILE)).flat_map(|r| (0..w.div_ceil(TILE)).map(move |c| (c, r)))
                .map(|(c, r)| Op { x: c * TILE, y: r * TILE, w: TILE.min(w - c * TILE), h: TILE.min(h - r * TILE), lod: 0 }).collect();
            let conv_tiles = time(&mut || { for op in &tiles { convert_region(&a, PlaneLayout::Nv12, ColorInfo::default(), w, *op, &mut out).unwrap(); } });
            let mpx = (w * h) as f64 / 1e6;
            eprintln!("DIAG {w}x{h}: hash {hash_ms:.2} ms ({:.2} ns/px) | plan {plan_ms:.2} | whole-frame convert {whole:.2} | region LOD0 {conv0:.2} | LOD1 {conv1:.2} | per-tile ops {conv_tiles:.2} ({} ops)", hash_ms * 1e6 / (mpx * 1e6), tiles.len());
        }
    }
}
