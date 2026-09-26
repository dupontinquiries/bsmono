//! AV1 film grain synthesis (spec 7.18.3), 8-bit: the CPU reference, and
//! the specification the GPU shader mirrors.
//!
//! Ported from rav1d (src/filmgrain.rs, src/fg_apply.rs at d3d1cd67), itself a
//! port of dav1d. Their licence applies to this file:
//!
//!   Copyright (c) 2018-2019, VideoLAN and dav1d authors
//!   Copyright (c) 2023-2024, VideoLAN, dav1d authors, and Internet Security
//!   Research Group. All rights reserved. Redistribution and use in source and
//!   binary forms, with or without modification, are permitted provided that
//!   (1) redistributions of source code retain the above copyright notice,
//!   this list of conditions and the following disclaimer, and (2)
//!   redistributions in binary form reproduce them in the documentation
//!   and/or other materials provided with the distribution. THIS SOFTWARE IS
//!   PROVIDED "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES ARE DISCLAIMED.
//!   (BSD-2-Clause; full text in rav1d's COPYING.)
//!
//! Why here: grain is the last stage of AV1 decode, applied per pixel from a
//! few KB of per-frame tables -- the one stage that splits off a CPU decoder
//! cleanly (dav1d can export the parameters instead of applying them;
//! measured ~5% of dav1d's time on SVT-AV1 1080p). Moving it to the GPU
//! takes that off the CPU.
//!
//! Shape: the templates (73x82 luma, 38x44 chroma: an auto-regressive
//! filter, serial but tiny) and the scaling curves are built on the CPU once
//! per frame by [`prepare`]; each 32x32 block's random offset is one byte in
//! a small table ([`block_offsets`]). Everything per pixel is then an
//! independent function of position -- [`luma_grain`], [`chroma_grain`] --
//! written per pixel here (rav1d loops over block rows) so the GPU shader can
//! mirror it line for line.

pub const GRAIN_W: usize = 82;
pub const GRAIN_H: usize = 73;
const SUB_W: usize = 44;
const SUB_H: usize = 38;
pub const BLOCK: usize = 32;
const AR_PAD: usize = 3;

/// Film grain parameters in their DERIVED form (spec 7.18.3.x semantics:
/// multipliers minus 128, offsets minus 256, shifts with their bias added).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct GrainParams {
    pub seed: u16,
    pub y_points: Vec<[u8; 2]>,
    pub cb_points: Vec<[u8; 2]>,
    pub cr_points: Vec<[u8; 2]>,
    pub chroma_scaling_from_luma: bool,
    /// 8..=11.
    pub scaling_shift: u8,
    /// 0..=3.
    pub ar_coeff_lag: u8,
    pub ar_coeffs_y: [i8; 24],
    pub ar_coeffs_uv: [[i8; 25]; 2],
    /// 6..=9.
    pub ar_coeff_shift: u8,
    pub grain_scale_shift: u8,
    pub uv_mult: [i32; 2],
    pub uv_luma_mult: [i32; 2],
    pub uv_offset: [i32; 2],
    pub overlap: bool,
    pub clip_to_restricted_range: bool,
}

/// Per-frame tables: grain templates (luma, cb, cr) and scaling curves.
pub struct Grain {
    pub lut: Box<[[[i8; GRAIN_W]; GRAIN_H]; 3]>,
    pub scaling: [[u8; 256]; 3],
}

fn rand(bits: u32, state: &mut u32) -> i32 {
    let r = *state;
    let bit = (r ^ (r >> 1) ^ (r >> 3) ^ (r >> 12)) & 1;
    *state = (r >> 1) | (bit << 15);
    ((*state >> (16 - bits)) & ((1 << bits) - 1)) as i32
}

fn round2(x: i32, shift: u32) -> i32 {
    if shift == 0 { x } else { (x + (1 << (shift - 1))) >> shift }
}

fn gaussian(seed: &mut u32, shift: u32) -> i8 {
    round2(GAUSSIAN[rand(11, seed) as usize] as i32, shift) as i8
}

/// Templates and scaling curves for one frame. `subx`/`suby`: chroma
/// subsampling (4:2:0 = both).
pub fn prepare(p: &GrainParams, subx: bool, suby: bool) -> Grain {
    let mut lut = Box::new([[[0i8; GRAIN_W]; GRAIN_H]; 3]);
    let shift = 4 + p.grain_scale_shift as u32;
    let lag = (p.ar_coeff_lag & 3) as usize;
    // Luma.
    let mut seed = p.seed as u32;
    for row in lut[0].iter_mut() {
        for v in row.iter_mut() {
            *v = gaussian(&mut seed, shift);
        }
    }
    for y in 0..GRAIN_H - AR_PAD {
        for x in 0..GRAIN_W - 2 * AR_PAD {
            let mut sum = 0i32;
            let mut k = 0;
            'taps: for dy in 0..=lag {
                for dx in 0..=2 * lag {
                    if dx == lag && dy == lag {
                        break 'taps;
                    }
                    let v = lut[0][y + AR_PAD - lag + dy][x + AR_PAD - lag + dx] as i32;
                    sum += p.ar_coeffs_y[k] as i32 * v;
                    k += 1;
                }
            }
            let g = lut[0][y + AR_PAD][x + AR_PAD] as i32 + round2(sum, p.ar_coeff_shift as u32);
            lut[0][y + AR_PAD][x + AR_PAD] = g.clamp(-128, 127) as i8;
        }
    }
    // Chroma: generated when the plane has points or scales from luma.
    let (ch, cw) = (if suby { SUB_H } else { GRAIN_H }, if subx { SUB_W } else { GRAIN_W });
    for uv in 0..2 {
        let has = if uv == 0 { !p.cb_points.is_empty() } else { !p.cr_points.is_empty() };
        if !has && !p.chroma_scaling_from_luma {
            continue;
        }
        let mut seed = p.seed as u32 ^ if uv == 1 { 0x49d8 } else { 0xb524 };
        let (luma, rest) = lut.split_at_mut(1);
        let buf = &mut rest[uv];
        let luma = &luma[0];
        for row in buf.iter_mut().take(ch) {
            for v in row.iter_mut().take(cw) {
                *v = gaussian(&mut seed, shift);
            }
        }
        for y in 0..ch - AR_PAD {
            for x in 0..cw - 2 * AR_PAD {
                let mut sum = 0i32;
                let mut k = 0;
                'taps: for dy in 0..=lag {
                    for dx in 0..=2 * lag {
                        if dx == lag && dy == lag {
                            // The luma contribution: the co-located luma grain,
                            // averaged over the subsampled area.
                            let (ly, lx) = ((y << suby as usize) + AR_PAD, (x << subx as usize) + AR_PAD);
                            let mut l = 0i32;
                            for i in 0..1 + suby as usize {
                                for j in 0..1 + subx as usize {
                                    l += luma[ly + i][lx + j] as i32;
                                }
                            }
                            l = round2(l, suby as u32 + subx as u32);
                            sum += l * p.ar_coeffs_uv[uv][k] as i32;
                            break 'taps;
                        }
                        let v = buf[y + AR_PAD - lag + dy][x + AR_PAD - lag + dx] as i32;
                        sum += p.ar_coeffs_uv[uv][k] as i32 * v;
                        k += 1;
                    }
                }
                let g = buf[y + AR_PAD][x + AR_PAD] as i32 + round2(sum, p.ar_coeff_shift as u32);
                buf[y + AR_PAD][x + AR_PAD] = g.clamp(-128, 127) as i8;
            }
        }
    }
    Grain { lut, scaling: [scaling_curve(&p.y_points), scaling_curve(&p.cb_points), scaling_curve(&p.cr_points)] }
}

/// The piecewise-linear scaling curve, 8-bit.
pub fn scaling_curve(points: &[[u8; 2]]) -> [u8; 256] {
    let mut s = [0u8; 256];
    let Some(first) = points.first() else { return s };
    s[..first[0] as usize].fill(first[1]);
    for w in points.windows(2) {
        let (bx, by, ex, ey) = (w[0][0] as i64, w[0][1] as i64, w[1][0] as i64, w[1][1] as i64);
        let dx = ex - bx;
        if dx <= 0 {
            continue;
        }
        let delta = (ey - by) * ((0x10000 + (dx >> 1)) / dx);
        let mut d = 0x8000i64;
        for x in 0..dx {
            s[(bx + x) as usize] = (by + (d >> 16)) as u8;
            d += delta;
        }
    }
    let last = points[points.len() - 1];
    s[last[0] as usize..].fill(last[1]);
    s
}

/// Each 32x32 luma block's random offset byte, row-major (rows of 32 luma
/// lines, columns of 32 luma pixels). Row r's column c is the c-th 8-bit draw
/// from row r's seed; the "previous row" offsets used for overlap are the
/// same draws row r-1 made, so one table serves every pixel.
pub fn block_offsets(p: &GrainParams, width: usize, height: usize) -> (usize, usize, Vec<u8>) {
    let (cols, rows) = (width.div_ceil(BLOCK), height.div_ceil(BLOCK));
    let mut t = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        let mut seed = p.seed as u32;
        seed ^= ((((r * 37 + 178) & 0xff) << 8) as u32) ^ ((r * 173 + 105) & 0xff) as u32;
        for _ in 0..cols {
            t.push(rand(8, &mut seed) as u8);
        }
    }
    (cols, rows, t)
}

/// Where a block's grain is read from in a template.
fn sample(lut: &[[i8; GRAIN_W]; GRAIN_H], rv: u8, subx: usize, suby: usize, bx: usize, by: usize, x: usize, y: usize) -> i32 {
    let offx = 3 + (2 >> subx) * (3 + (rv as usize >> 4));
    let offy = 3 + (2 >> suby) * (3 + (rv as usize & 15));
    lut[offy + y + (BLOCK >> suby) * by][offx + x + (BLOCK >> subx) * bx] as i32
}

/// The blended grain value at one pixel of a plane. `(px, py)` in that
/// plane's samples; `(pw, ph)` its size; `sub` its subsampling.
#[allow(clippy::too_many_arguments)]
fn grain_at(p: &GrainParams, lut: &[[i8; GRAIN_W]; GRAIN_H], offs: &(usize, usize, Vec<u8>), subx: usize, suby: usize, px: usize, py: usize, pw: usize, luma_h: usize) -> i32 {
    let (bsx, bsy) = (BLOCK >> subx, BLOCK >> suby);
    let (col, row) = (px / bsx, py / bsy);
    let (x, y) = (px % bsx, py % bsy);
    let (cols, _, t) = offs;
    let off = |r: usize, c: usize| t[r * cols + c];
    let bh = (luma_h - row * BLOCK).min(BLOCK).div_ceil(1 << suby);
    let bw = (pw - col * bsx).min(bsx);
    let ystart = if p.overlap && row > 0 { (2 >> suby).min(bh) } else { 0 };
    let xstart = if p.overlap && col > 0 { (2 >> subx).min(bw) } else { 0 };
    // offsets[bx][by]: [0][0] this block, [1][0] left, [0][1] top, [1][1] top-left.
    let o = |bxf: usize, byf: usize| off(row - byf, col - bxf);
    let s = |bxf: usize, byf: usize| sample(lut, o(bxf, byf), subx, suby, bxf, byf, x, y);
    const W: [[[i32; 2]; 2]; 2] = [[[27, 17], [17, 27]], [[23, 22], [0, 0]]];
    let wx = W[subx][x.min(1)];
    let wy = W[suby][y.min(1)];
    let mix = |old: i32, new: i32, w: [i32; 2]| round2(old * w[0] + new * w[1], 5).clamp(-128, 127);
    match (y < ystart, x < xstart) {
        (false, false) => s(0, 0),
        (false, true) => mix(s(1, 0), s(0, 0), wx),
        (true, false) => mix(s(0, 1), s(0, 0), wy),
        (true, true) => {
            let top = mix(s(1, 1), s(0, 1), wx);
            let cur = mix(s(1, 0), s(0, 0), wx);
            mix(top, cur, wy)
        }
    }
}

/// Luma: the grained value of one luma pixel.
pub fn luma_grain(p: &GrainParams, g: &Grain, offs: &(usize, usize, Vec<u8>), src: u8, x: usize, y: usize, w: usize, h: usize) -> u8 {
    if p.y_points.is_empty() {
        return src;
    }
    let grain = grain_at(p, &g.lut[0], offs, 0, 0, x, y, w, h);
    let noise = round2(g.scaling[0][src as usize] as i32 * grain, p.scaling_shift as u32);
    let (lo, hi) = if p.clip_to_restricted_range { (16, 235) } else { (0, 255) };
    (src as i32 + noise).clamp(lo, hi) as u8
}

/// Chroma: the grained value of one chroma sample. `luma_avg`: the
/// UNGRAINED co-located luma (averaged horizontally when subsampled).
#[allow(clippy::too_many_arguments)]
pub fn chroma_grain(p: &GrainParams, g: &Grain, offs: &(usize, usize, Vec<u8>), uv: usize, src: u8, luma_avg: u8, x: usize, y: usize, cw: usize, luma_h: usize, subx: bool, suby: bool, identity_matrix: bool) -> u8 {
    let has = if uv == 0 { !p.cb_points.is_empty() } else { !p.cr_points.is_empty() };
    if !has && !p.chroma_scaling_from_luma {
        return src;
    }
    let grain = grain_at(p, &g.lut[1 + uv], offs, subx as usize, suby as usize, x, y, cw, luma_h);
    let val = if p.chroma_scaling_from_luma {
        luma_avg as i32
    } else {
        let combined = luma_avg as i32 * p.uv_luma_mult[uv] + src as i32 * p.uv_mult[uv];
        ((combined >> 6) + p.uv_offset[uv]).clamp(0, 255)
    };
    let table = if p.chroma_scaling_from_luma { &g.scaling[0] } else { &g.scaling[1 + uv] };
    let noise = round2(table[val as usize] as i32 * grain, p.scaling_shift as u32);
    let (lo, hi) = if p.clip_to_restricted_range { (16, if identity_matrix { 235 } else { 240 }) } else { (0, 255) };
    (src as i32 + noise).clamp(lo, hi) as u8
}

/// Apply grain to an 8-bit I420 frame in place. The CPU reference.
pub fn apply_i420(p: &GrainParams, y: &mut [u8], u: &mut [u8], v: &mut [u8], w: usize, h: usize, identity_matrix: bool) {
    let g = prepare(p, true, true);
    let offs = block_offsets(p, w, h);
    let luma_src = y.to_vec();
    for yy in 0..h {
        for xx in 0..w {
            let i = yy * w + xx;
            y[i] = luma_grain(p, &g, &offs, luma_src[i], xx, yy, w, h);
        }
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    for (uv, plane) in [u, v].into_iter().enumerate() {
        for yy in 0..ch {
            for xx in 0..cw {
                let l0 = luma_src[(yy * 2) * w + (xx * 2).min(w - 1)] as i32;
                let l1 = luma_src[(yy * 2) * w + (xx * 2 + 1).min(w - 1)] as i32;
                let avg = ((l0 + l1 + 1) >> 1) as u8;
                let i = yy * cw + xx;
                plane[i] = chroma_grain(p, &g, &offs, uv, plane[i], avg, xx, yy, cw, h, true, true, identity_matrix);
            }
        }
    }
}

#[rustfmt::skip]
pub static GAUSSIAN: [i16; 2048] = [
    56, 568, -180, 172, 124, -84, 172, -64, -900, 24, 820, 224, 1248, 996, 272, -8,
    -916, -388, -732, -104, -188, 800, 112, -652, -320, -376, 140, -252, 492, -168, 44, -788,
    588, -584, 500, -228, 12, 680, 272, -476, 972, -100, 652, 368, 432, -196, -720, -192,
    1000, -332, 652, -136, -552, -604, -4, 192, -220, -136, 1000, -52, 372, -96, -624, 124,
    -24, 396, 540, -12, -104, 640, 464, 244, -208, -84, 368, -528, -740, 248, -968, -848,
    608, 376, -60, -292, -40, -156, 252, -292, 248, 224, -280, 400, -244, 244, -60, 76,
    -80, 212, 532, 340, 128, -36, 824, -352, -60, -264, -96, -612, 416, -704, 220, -204,
    640, -160, 1220, -408, 900, 336, 20, -336, -96, -792, 304, 48, -28, -1232, -1172, -448,
    104, -292, -520, 244, 60, -948, 0, -708, 268, 108, 356, -548, 488, -344, -136, 488,
    -196, -224, 656, -236, -1128, 60, 4, 140, 276, -676, -376, 168, -108, 464, 8, 564,
    64, 240, 308, -300, -400, -456, -136, 56, 120, -408, -116, 436, 504, -232, 328, 844,
    -164, -84, 784, -168, 232, -224, 348, -376, 128, 568, 96, -1244, -288, 276, 848, 832,
    -360, 656, 464, -384, -332, -356, 728, -388, 160, -192, 468, 296, 224, 140, -776, -100,
    280, 4, 196, 44, -36, -648, 932, 16, 1428, 28, 528, 808, 772, 20, 268, 88,
    -332, -284, 124, -384, -448, 208, -228, -1044, -328, 660, 380, -148, -300, 588, 240, 540,
    28, 136, -88, -436, 256, 296, -1000, 1400, 0, -48, 1056, -136, 264, -528, -1108, 632,
    -484, -592, -344, 796, 124, -668, -768, 388, 1296, -232, -188, -200, -288, -4, 308, 100,
    -168, 256, -500, 204, -508, 648, -136, 372, -272, -120, -1004, -552, -548, -384, 548, -296,
    428, -108, -8, -912, -324, -224, -88, -112, -220, -100, 996, -796, 548, 360, -216, 180,
    428, -200, -212, 148, 96, 148, 284, 216, -412, -320, 120, -300, -384, -604, -572, -332,
    -8, -180, -176, 696, 116, -88, 628, 76, 44, -516, 240, -208, -40, 100, -592, 344,
    -308, -452, -228, 20, 916, -1752, -136, -340, -804, 140, 40, 512, 340, 248, 184, -492,
    896, -156, 932, -628, 328, -688, -448, -616, -752, -100, 560, -1020, 180, -800, -64, 76,
    576, 1068, 396, 660, 552, -108, -28, 320, -628, 312, -92, -92, -472, 268, 16, 560,
    516, -672, -52, 492, -100, 260, 384, 284, 292, 304, -148, 88, -152, 1012, 1064, -228,
    164, -376, -684, 592, -392, 156, 196, -524, -64, -884, 160, -176, 636, 648, 404, -396,
    -436, 864, 424, -728, 988, -604, 904, -592, 296, -224, 536, -176, -920, 436, -48, 1176,
    -884, 416, -776, -824, -884, 524, -548, -564, -68, -164, -96, 692, 364, -692, -1012, -68,
    260, -480, 876, -1116, 452, -332, -352, 892, -1088, 1220, -676, 12, -292, 244, 496, 372,
    -32, 280, 200, 112, -440, -96, 24, -644, -184, 56, -432, 224, -980, 272, -260, 144,
    -436, 420, 356, 364, -528, 76, 172, -744, -368, 404, -752, -416, 684, -688, 72, 540,
    416, 92, 444, 480, -72, -1416, 164, -1172, -68, 24, 424, 264, 1040, 128, -912, -524,
    -356, 64, 876, -12, 4, -88, 532, 272, -524, 320, 276, -508, 940, 24, -400, -120,
    756, 60, 236, -412, 100, 376, -484, 400, -100, -740, -108, -260, 328, -268, 224, -200,
    -416, 184, -604, -564, -20, 296, 60, 892, -888, 60, 164, 68, -760, 216, -296, 904,
    -336, -28, 404, -356, -568, -208, -1480, -512, 296, 328, -360, -164, -1560, -776, 1156, -428,
    164, -504, -112, 120, -216, -148, -264, 308, 32, 64, -72, 72, 116, 176, -64, -272,
    460, -536, -784, -280, 348, 108, -752, -132, 524, -540, -776, 116, -296, -1196, -288, -560,
    1040, -472, 116, -848, -1116, 116, 636, 696, 284, -176, 1016, 204, -864, -648, -248, 356,
    972, -584, -204, 264, 880, 528, -24, -184, 116, 448, -144, 828, 524, 212, -212, 52,
    12, 200, 268, -488, -404, -880, 824, -672, -40, 908, -248, 500, 716, -576, 492, -576,
    16, 720, -108, 384, 124, 344, 280, 576, -500, 252, 104, -308, 196, -188, -8, 1268,
    296, 1032, -1196, 436, 316, 372, -432, -200, -660, 704, -224, 596, -132, 268, 32, -452,
    884, 104, -1008, 424, -1348, -280, 4, -1168, 368, 476, 696, 300, -8, 24, 180, -592,
    -196, 388, 304, 500, 724, -160, 244, -84, 272, -256, -420, 320, 208, -144, -156, 156,
    364, 452, 28, 540, 316, 220, -644, -248, 464, 72, 360, 32, -388, 496, -680, -48,
    208, -116, -408, 60, -604, -392, 548, -840, 784, -460, 656, -544, -388, -264, 908, -800,
    -628, -612, -568, 572, -220, 164, 288, -16, -308, 308, -112, -636, -760, 280, -668, 432,
    364, 240, -196, 604, 340, 384, 196, 592, -44, -500, 432, -580, -132, 636, -76, 392,
    4, -412, 540, 508, 328, -356, -36, 16, -220, -64, -248, -60, 24, -192, 368, 1040,
    92, -24, -1044, -32, 40, 104, 148, 192, -136, -520, 56, -816, -224, 732, 392, 356,
    212, -80, -424, -1008, -324, 588, -1496, 576, 460, -816, -848, 56, -580, -92, -1372, -112,
    -496, 200, 364, 52, -140, 48, -48, -60, 84, 72, 40, 132, -356, -268, -104, -284,
    -404, 732, -520, 164, -304, -540, 120, 328, -76, -460, 756, 388, 588, 236, -436, -72,
    -176, -404, -316, -148, 716, -604, 404, -72, -88, -888, -68, 944, 88, -220, -344, 960,
    472, 460, -232, 704, 120, 832, -228, 692, -508, 132, -476, 844, -748, -364, -44, 1116,
    -1104, -1056, 76, 428, 552, -692, 60, 356, 96, -384, -188, -612, -576, 736, 508, 892,
    352, -1132, 504, -24, -352, 324, 332, -600, -312, 292, 508, -144, -8, 484, 48, 284,
    -260, -240, 256, -100, -292, -204, -44, 472, -204, 908, -188, -1000, -256, 92, 1164, -392,
    564, 356, 652, -28, -884, 256, 484, -192, 760, -176, 376, -524, -452, -436, 860, -736,
    212, 124, 504, -476, 468, 76, -472, 552, -692, -944, -620, 740, -240, 400, 132, 20,
    192, -196, 264, -668, -1012, -60, 296, -316, -828, 76, -156, 284, -768, -448, -832, 148,
    248, 652, 616, 1236, 288, -328, -400, -124, 588, 220, 520, -696, 1032, 768, -740, -92,
    -272, 296, 448, -464, 412, -200, 392, 440, -200, 264, -152, -260, 320, 1032, 216, 320,
    -8, -64, 156, -1016, 1084, 1172, 536, 484, -432, 132, 372, -52, -256, 84, 116, -352,
    48, 116, 304, -384, 412, 924, -300, 528, 628, 180, 648, 44, -980, -220, 1320, 48,
    332, 748, 524, -268, -720, 540, -276, 564, -344, -208, -196, 436, 896, 88, -392, 132,
    80, -964, -288, 568, 56, -48, -456, 888, 8, 552, -156, -292, 948, 288, 128, -716,
    -292, 1192, -152, 876, 352, -600, -260, -812, -468, -28, -120, -32, -44, 1284, 496, 192,
    464, 312, -76, -516, -380, -456, -1012, -48, 308, -156, 36, 492, -156, -808, 188, 1652,
    68, -120, -116, 316, 160, -140, 352, 808, -416, 592, 316, -480, 56, 528, -204, -568,
    372, -232, 752, -344, 744, -4, 324, -416, -600, 768, 268, -248, -88, -132, -420, -432,
    80, -288, 404, -316, -1216, -588, 520, -108, 92, -320, 368, -480, -216, -92, 1688, -300,
    180, 1020, -176, 820, -68, -228, -260, 436, -904, 20, 40, -508, 440, -736, 312, 332,
    204, 760, -372, 728, 96, -20, -632, -520, -560, 336, 1076, -64, -532, 776, 584, 192,
    396, -728, -520, 276, -188, 80, -52, -612, -252, -48, 648, 212, -688, 228, -52, -260,
    428, -412, -272, -404, 180, 816, -796, 48, 152, 484, -88, -216, 988, 696, 188, -528,
    648, -116, -180, 316, 476, 12, -564, 96, 476, -252, -364, -376, -392, 556, -256, -576,
    260, -352, 120, -16, -136, -260, -492, 72, 556, 660, 580, 616, 772, 436, 424, -32,
    -324, -1268, 416, -324, -80, 920, 160, 228, 724, 32, -516, 64, 384, 68, -128, 136,
    240, 248, -204, -68, 252, -932, -120, -480, -628, -84, 192, 852, -404, -288, -132, 204,
    100, 168, -68, -196, -868, 460, 1080, 380, -80, 244, 0, 484, -888, 64, 184, 352,
    600, 460, 164, 604, -196, 320, -64, 588, -184, 228, 12, 372, 48, -848, -344, 224,
    208, -200, 484, 128, -20, 272, -468, -840, 384, 256, -720, -520, -464, -580, 112, -120,
    644, -356, -208, -608, -528, 704, 560, -424, 392, 828, 40, 84, 200, -152, 0, -144,
    584, 280, -120, 80, -556, -972, -196, -472, 724, 80, 168, -32, 88, 160, -688, 0,
    160, 356, 372, -776, 740, -128, 676, -248, -480, 4, -364, 96, 544, 232, -1032, 956,
    236, 356, 20, -40, 300, 24, -676, -596, 132, 1120, -104, 532, -1096, 568, 648, 444,
    508, 380, 188, -376, -604, 1488, 424, 24, 756, -220, -192, 716, 120, 920, 688, 168,
    44, -460, 568, 284, 1144, 1160, 600, 424, 888, 656, -356, -320, 220, 316, -176, -724,
    -188, -816, -628, -348, -228, -380, 1012, -452, -660, 736, 928, 404, -696, -72, -268, -892,
    128, 184, -344, -780, 360, 336, 400, 344, 428, 548, -112, 136, -228, -216, -820, -516,
    340, 92, -136, 116, -300, 376, -244, 100, -316, -520, -284, -12, 824, 164, -548, -180,
    -128, 116, -924, -828, 268, -368, -580, 620, 192, 160, 0, -1676, 1068, 424, -56, -360,
    468, -156, 720, 288, -528, 556, -364, 548, -148, 504, 316, 152, -648, -620, -684, -24,
    -376, -384, -108, -920, -1032, 768, 180, -264, -508, -1268, -260, -60, 300, -240, 988, 724,
    -376, -576, -212, -736, 556, 192, 1092, -620, -880, 376, -56, -4, -216, -32, 836, 268,
    396, 1332, 864, -600, 100, 56, -412, -92, 356, 180, 884, -468, -436, 292, -388, -804,
    -704, -840, 368, -348, 140, -724, 1536, 940, 372, 112, -372, 436, -480, 1136, 296, -32,
    -228, 132, -48, -220, 868, -1016, -60, -1044, -464, 328, 916, 244, 12, -736, -296, 360,
    468, -376, -108, -92, 788, 368, -56, 544, 400, -672, -420, 728, 16, 320, 44, -284,
    -380, -796, 488, 132, 204, -596, -372, 88, -152, -908, -636, -572, -624, -116, -692, -200,
    -56, 276, -88, 484, -324, 948, 864, 1000, -456, -184, -276, 292, -296, 156, 676, 320,
    160, 908, -84, -1236, -288, -116, 260, -372, -644, 732, -756, -96, 84, 344, -520, 348,
    -688, 240, -84, 216, -1044, -136, -676, -396, -1500, 960, -40, 176, 168, 1516, 420, -504,
    -344, -364, -360, 1216, -940, -380, -212, 252, -660, -708, 484, -444, -152, 928, -120, 1112,
    476, -260, 560, -148, -344, 108, -196, 228, -288, 504, 560, -328, -88, 288, -1008, 460,
    -228, 468, -836, -196, 76, 388, 232, 412, -1168, -716, -644, 756, -172, -356, -504, 116,
    432, 528, 48, 476, -168, -608, 448, 160, -532, -272, 28, -676, -12, 828, 980, 456,
    520, 104, -104, 256, -344, -4, -28, -368, -52, -524, -572, -556, -200, 768, 1124, -208,
    -512, 176, 232, 248, -148, -888, 604, -600, -304, 804, -156, -212, 488, -192, -804, -256,
    368, -360, -916, -328, 228, -240, -448, -472, 856, -556, -364, 572, -12, -156, -368, -340,
    432, 252, -752, -152, 288, 268, -580, -848, -592, 108, -76, 244, 312, -716, 592, -80,
    436, 360, 4, -248, 160, 516, 584, 732, 44, -468, -280, -292, -156, -588, 28, 308,
    912, 24, 124, 156, 180, -252, 944, -924, -772, -520, -428, -624, 300, -212, -1144, 32,
    -724, 800, -1128, -212, -1288, -848, 180, -416, 440, 192, -576, -792, -76, -1080, 80, -532,
    -352, -132, 380, -820, 148, 1112, 128, 164, 456, 700, -924, 144, -668, -384, 648, -832,
    508, 552, -52, -100, -656, 208, -568, 748, -88, 680, 232, 300, 192, -408, -1012, -152,
    -252, -268, 272, -876, -664, -648, -332, -136, 16, 12, 1152, -28, 332, -536, 320, -672,
    -460, -316, 532, -260, 228, -40, 1052, -816, 180, 88, -496, -556, -672, -368, 428, 92,
    356, 404, -408, 252, 196, -176, -556, 792, 268, 32, 372, 40, 96, -332, 328, 120,
    372, -900, -40, 472, -264, -592, 952, 128, 656, 112, 664, -232, 420, 4, -344, -464,
    556, 244, -416, -32, 252, 0, -412, 188, -696, 508, -476, 324, -1096, 656, -312, 560,
    264, -136, 304, 160, -64, -580, 248, 336, -720, 560, -348, -288, -276, -196, -500, 852,
    -544, -236, -1128, -992, -776, 116, 56, 52, 860, 884, 212, -12, 168, 1020, 512, -552,
    924, -148, 716, 188, 164, -340, -520, -184, 880, -152, -680, -208, -1156, -300, -528, -472,
    364, 100, -744, -1056, -32, 540, 280, 144, -676, -32, -232, -280, -224, 96, 568, -76,
    172, 148, 148, 104, 32, -296, -32, 788, -80, 32, -16, 280, 288, 944, 428, -484
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gaussian_table_came_across_whole() {
        assert_eq!(GAUSSIAN.len(), 2048);
        assert_eq!(&GAUSSIAN[..6], &[56, 568, -180, 172, 124, -84]);
        assert_eq!(&GAUSSIAN[2045..], &[944, 428, -484]);
    }

    #[test]
    fn scaling_curves_interpolate_and_hold_their_ends() {
        let s = scaling_curve(&[[16, 20], [128, 80], [235, 40]]);
        assert_eq!(s[0], 20);
        assert_eq!(s[16], 20);
        assert_eq!(s[128], 80);
        assert!(s[72] > 40 && s[72] < 60, "{}", s[72]);
        assert_eq!(s[255], 40);
        assert_eq!(scaling_curve(&[]), [0u8; 256]);
    }

    #[test]
    fn no_points_leaves_the_picture_untouched() {
        let p = GrainParams { seed: 1, scaling_shift: 8, ar_coeff_shift: 6, ..Default::default() };
        let (mut y, mut u, mut v) = (vec![100u8; 64 * 32], vec![128u8; 32 * 16], vec![128u8; 32 * 16]);
        apply_i420(&p, &mut y, &mut u, &mut v, 64, 32, false);
        assert!(y.iter().all(|&b| b == 100) && u.iter().all(|&b| b == 128));
    }
}
