//! motion — "Shiftquilt": patch frames whose changed blocks may be PREDICTED
//! from the previous decoded frame (a motion vector plus a small residual)
//! instead of coded from scratch.
//!
//! ## Why
//!
//! Patch frames (marker M) stopped re-sending static scenery, but at 1080p a
//! static scene with one moving window was still 4.67x its H.264 source: the
//! moving region is intra-coded every frame, where H.264 predicts it from the
//! frame before. This is that prediction.
//!
//! ## Closed loop, by construction
//!
//! Prediction reads the PREVIOUS DECODED frame -- the only picture the decoder
//! has. The transcoder obtains it by running THIS module's `decode` on the
//! payload it just wrote, so encoder and decoder predict from byte-identical
//! pictures and there is nothing to drift. The cost: frame n cannot be encoded
//! before frame n-1 is decoded, so frames encode SEQUENTIALLY.
//!
//! ## KNOWN LIMITATION: quality declines across a run
//!
//! Measured on a camera pan: mean PSNR ~30 dB over the first frames after a
//! keyframe, ~26.6 dB over the last -- keyframes-only stays flat. So "closed
//! loop, therefore no drift" was incomplete. Best hypothesis: each predicted
//! frame round-trips RGB -> YUV 4:2:0 -> RGB, and chroma subsampling discards
//! high-frequency colour error the residual can never correct at full
//! resolution, so it accumulates. The fix is to predict and code in YUV end to
//! end, which means a YUV decode cache -- not done here. The degradation is
//! bounded by the keyframe interval (a keyframe resets it), and it is why
//! motion is opt-in (`transcode::MOTION_DEFAULT` is false).
//!
//! ## Payload (marker Q)
//!
//! ```text
//! b'Q' | u32 LE k | bitmap (as M) | per changed block, bitmap order:
//!        u8 mode (0 intra, 1 inter) [+ i8 dx, i8 dy if inter]
//!      | u32 LE intra length | intra DCT patch (Keystone) | residual DCT patch
//! ```
//!
//! Residuals are `src - pred + 128`, clamped to a byte, packed like intra
//! blocks and DCT-coded -- a flat 128 where the prediction is good, which the
//! DCT codes almost for free. A vector points into the previous frame:
//! `cur(x, y) ~ prev(x + dx, y + dy)`, coordinates clamped at the frame edge.

use crate::decode::{rgb_to_yuv420p, yuv_to_rgb};
use crate::patch::{block_rect, grid, patch_dims, MB};
use fao_codec_lib as fao_v1;

pub const MARKER: u8 = b'Q';
/// Search range, pixels, each axis. Fits an i8 with room to spare.
pub const RANGE: i32 = 24;

/// Residuals are coded this much below the frame's quality. A residual is
/// mostly the previous frame's coding noise, and re-encoding noise faithfully
/// buys little. Measured on a 45-frame pan (160x96), size / mean PSNR:
///
///   residual at quality     bytes    mean dB
///   80 (no drop)           95,437    28.11   worse than keyframes-only q50
///   60 (drop 20)           63,791    28.07   better than keyframes-only q20
///   40 (drop 40)           51,098    27.87   (65,282 / 27.45)
///
/// Nearly free in quality, large in size; 20 is the measured sweet spot.
pub const RESIDUAL_Q_DROP: u8 = 20;

/// Luma (BT.601-ish integer) of an RGB image, for motion search only.
pub fn luma(rgb: &[u8]) -> Vec<u8> {
    rgb.chunks_exact(3).map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8).collect()
}

#[inline]
fn clampi(v: i32, hi: usize) -> usize {
    v.clamp(0, hi as i32 - 1) as usize
}

/// Sum of absolute luma differences between block `i` of `cur` and the
/// region of `prev` displaced by `(dx, dy)`. Stops early past `cap`.
fn sad(cur: &[u8], prev: &[u8], w: usize, h: usize, i: usize, dx: i32, dy: i32, cap: u32) -> u32 {
    let (x, y, bw, bh) = block_rect(i, w, h);
    let mut s = 0u32;
    for py in 0..bh {
        let sy = clampi((y + py) as i32 + dy, h);
        for px in 0..bw {
            let sx = clampi((x + px) as i32 + dx, w);
            s += cur[(y + py) * w + x + px].abs_diff(prev[sy * w + sx]) as u32;
        }
        if s >= cap {
            return s;
        }
    }
    s
}

/// What coding block `i` from scratch roughly costs: its luma's total
/// deviation from its own mean. A prediction must beat this to be worth it.
fn intra_cost(cur: &[u8], w: usize, h: usize, i: usize) -> u32 {
    let (x, y, bw, bh) = block_rect(i, w, h);
    let n = (bw * bh) as u32;
    let sum: u32 = (y..y + bh).flat_map(|py| (x..x + bw).map(move |px| (py, px))).map(|(py, px)| cur[py * w + px] as u32).sum();
    let mean = sum / n;
    (y..y + bh).flat_map(|py| (x..x + bw).map(move |px| (py, px)))
        .map(|(py, px)| (cur[py * w + px] as i32 - mean as i32).unsigned_abs()).sum()
}

/// Best vector for block `i`: the zero vector and `hint` first, then a
/// three-step search (steps 8, 4, 2, 1) around the best so far. About 25
/// probes instead of ~2,400 for exhaustive search over +/-24.
pub fn search(cur: &[u8], prev: &[u8], w: usize, h: usize, i: usize, hint: (i32, i32)) -> ((i32, i32), u32) {
    let mut best = (0, 0);
    let mut best_s = sad(cur, prev, w, h, i, 0, 0, u32::MAX);
    if hint != (0, 0) {
        let s = sad(cur, prev, w, h, i, hint.0, hint.1, best_s);
        if s < best_s { best = hint; best_s = s; }
    }
    const RING: [(i32, i32); 8] = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];
    let mut step = 8;
    while step >= 1 {
        let c = best;
        for (ox, oy) in RING {
            let (dx, dy) = (c.0 + ox * step, c.1 + oy * step);
            if dx.abs() > RANGE || dy.abs() > RANGE { continue; }
            let s = sad(cur, prev, w, h, i, dx, dy, best_s);
            if s < best_s { best = (dx, dy); best_s = s; }
        }
        step /= 2;
    }
    // REFINE AROUND THE BEST, not the step's starting centre. The last step
    // searches the ring around where it STARTED; if it moves mid-ring, the
    // true answer can be a neighbour of the new best that the ring never
    // covered. Measured: a planted (-5, 0) shift (SAD 0) was reported as
    // (-5, -1) with SAD 286 -- reached from (-4, -2), whose ring does not
    // include (-5, 0). So keep stepping by one around the best until nothing
    // improves (capped, though convergence takes a few rounds at most).
    for _ in 0..8 {
        let c = best;
        for (ox, oy) in RING {
            let (dx, dy) = (c.0 + ox, c.1 + oy);
            if dx.abs() > RANGE || dy.abs() > RANGE { continue; }
            let s = sad(cur, prev, w, h, i, dx, dy, best_s);
            if s < best_s { best = (dx, dy); best_s = s; }
        }
        if best == c { break; }
    }
    (best, best_s)
}

/// Per changed block: `Some(vector)` to predict it, `None` to code it from
/// scratch. Predict only when the prediction is clearly cheaper than intra
/// AND close enough that the clamped residual cannot clip (worst luma error
/// small) -- a residual that clips reconstructs wrongly.
pub fn plan(cur: &[u8], prev: &[u8], w: usize, h: usize, blocks: &[usize]) -> Vec<Option<(i8, i8)>> {
    let (cl, pl) = (luma(cur), luma(prev));
    let mut hint = (0, 0);
    blocks.iter().map(|&i| {
        let (mv, s) = search(&cl, &pl, w, h, i, hint);
        let (x, y, bw, bh) = block_rect(i, w, h);
        let worst = (0..bh).flat_map(|py| (0..bw).map(move |px| (py, px))).map(|(py, px)| {
            let sy = clampi((y + py) as i32 + mv.1, h);
            let sx = clampi((x + px) as i32 + mv.0, w);
            cl[(y + py) * w + x + px].abs_diff(pl[sy * w + sx])
        }).max().unwrap_or(0);
        if s * 10 < intra_cost(&cl, w, h, i) * 7 && worst <= 100 {
            hint = mv;
            Some((mv.0 as i8, mv.1 as i8))
        } else {
            None
        }
    }).collect()
}

/// Pixel `c` of the prediction for block `i` at in-block `(px, py)`.
#[inline]
fn pred(prev: &[u8], w: usize, h: usize, x: usize, y: usize, mv: (i8, i8), px: usize, py: usize) -> [u8; 3] {
    let sy = clampi((y + py) as i32 + mv.1 as i32, h);
    let sx = clampi((x + px) as i32 + mv.0 as i32, w);
    let o = (sy * w + sx) * 3;
    [prev[o], prev[o + 1], prev[o + 2]]
}

/// The residual patch for inter blocks: `src - pred + 128`, clamped. Partial
/// edge blocks padded by repeating their last row/column, as intra patches are.
fn residual_patch(cur: &[u8], prev: &[u8], w: usize, h: usize, inter: &[(usize, (i8, i8))]) -> (Vec<u8>, usize, usize) {
    let (pw, ph) = patch_dims(inter.len());
    let pcols = pw / MB;
    let mut out = vec![128u8; pw * ph * 3];
    for (slot, &(i, mv)) in inter.iter().enumerate() {
        let (x, y, bw, bh) = block_rect(i, w, h);
        let (ox, oy) = ((slot % pcols) * MB, (slot / pcols) * MB);
        for py in 0..MB {
            let qy = py.min(bh - 1);
            for px in 0..MB {
                let qx = px.min(bw - 1);
                let s = ((y + qy) * w + x + qx) * 3;
                let p = pred(prev, w, h, x, y, mv, qx, qy);
                let d = ((oy + py) * pw + ox + px) * 3;
                for c in 0..3 {
                    out[d + c] = (cur[s + c] as i32 - p[c] as i32 + 128).clamp(0, 255) as u8;
                }
            }
        }
    }
    (out, pw, ph)
}

/// Encode a motion patch frame.
pub fn encode(cur: &[u8], prev: &[u8], w: usize, h: usize, blocks: &[usize], modes: &[Option<(i8, i8)>], quality: u8) -> Vec<u8> {
    let (cols, rows) = grid(w, h);
    let mut out = vec![MARKER];
    out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    let mut bitmap = vec![0u8; (cols * rows).div_ceil(8)];
    for &i in blocks { bitmap[i / 8] |= 1 << (i % 8); }
    out.extend_from_slice(&bitmap);
    let (mut intra, mut inter) = (Vec::new(), Vec::new());
    for (&i, m) in blocks.iter().zip(modes) {
        match m {
            Some(mv) => { out.extend_from_slice(&[1, mv.0 as u8, mv.1 as u8]); inter.push((i, *mv)); }
            None => { out.push(0); intra.push(i); }
        }
    }
    let intra_bytes = if intra.is_empty() { Vec::new() } else {
        let (p, pw, ph) = crate::patch::build_patch(cur, w, h, &intra);
        fao_v1::compress_frame_dct(&rgb_to_yuv420p(&p, pw, ph), pw, ph, quality)
    };
    out.extend_from_slice(&(intra_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&intra_bytes);
    if !inter.is_empty() {
        let (r, pw, ph) = residual_patch(cur, prev, w, h, &inter);
        out.extend_from_slice(&fao_v1::compress_frame_dct(&rgb_to_yuv420p(&r, pw, ph), pw, ph,
                                                          quality.saturating_sub(RESIDUAL_Q_DROP)));
    }
    out
}

/// Decode a motion patch frame over the previous frame's picture. `None` for
/// anything malformed. Prediction reads `prev` (never the frame being built),
/// so block order cannot change the result.
pub fn decode(payload: &[u8], prev: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    let (cols, rows) = grid(w, h);
    let nbytes = (cols * rows).div_ceil(8);
    if payload.first() != Some(&MARKER) || payload.len() < 5 + nbytes || prev.len() != w * h * 3 {
        return None;
    }
    let k = u32::from_le_bytes(payload[1..5].try_into().ok()?) as usize;
    let bitmap = &payload[5..5 + nbytes];
    let blocks: Vec<usize> = (0..cols * rows).filter(|&i| bitmap[i / 8] & (1 << (i % 8)) != 0).collect();
    if blocks.len() != k { return None; }
    let mut at = 5 + nbytes;
    let (mut intra, mut inter) = (Vec::new(), Vec::new());
    for &i in &blocks {
        match *payload.get(at)? {
            0 => { intra.push(i); at += 1; }
            1 => { inter.push((i, (*payload.get(at + 1)? as i8, *payload.get(at + 2)? as i8))); at += 3; }
            _ => return None,
        }
    }
    let ilen = u32::from_le_bytes(payload.get(at..at + 4)?.try_into().ok()?) as usize;
    at += 4;
    let intra_data = payload.get(at..at + ilen)?;
    at += ilen;
    let mut out = prev.to_vec();
    if !intra.is_empty() {
        let (pw, ph) = patch_dims(intra.len());
        let yuv = fao_v1::decompress_frame_dct(intra_data, pw, ph)?;
        crate::patch::apply_patch(&mut out, w, h, &intra, &yuv_to_rgb(&yuv, pw, ph), pw);
    }
    if !inter.is_empty() {
        let (pw, ph) = patch_dims(inter.len());
        let res = yuv_to_rgb(&fao_v1::decompress_frame_dct(&payload[at..], pw, ph)?, pw, ph);
        let pcols = pw / MB;
        for (slot, &(i, mv)) in inter.iter().enumerate() {
            let (x, y, bw, bh) = block_rect(i, w, h);
            let (ox, oy) = ((slot % pcols) * MB, (slot / pcols) * MB);
            for py in 0..bh {
                for px in 0..bw {
                    let p = pred(prev, w, h, x, y, mv, px, py);
                    let r = ((oy + py) * pw + ox + px) * 3;
                    let d = ((y + py) * w + x + px) * 3;
                    for c in 0..3 {
                        out[d + c] = (p[c] as i32 + res[r + c] as i32 - 128).clamp(0, 255) as u8;
                    }
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A smooth, textured picture: predictable, the way real scenery is.
    fn scene(w: usize, h: usize, t: i32) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 3);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let (u, q) = (x - t, y);
                v.push((128.0 + 90.0 * ((u as f32) * 0.11).sin() * ((q as f32) * 0.07).cos()) as u8);
                v.push((128.0 + 70.0 * ((u as f32 + q as f32) * 0.05).sin()) as u8);
                v.push((100 + ((u * 3 + q * 2).rem_euclid(80))) as u8);
            }
        }
        v
    }

    /// A known pan: the scene shifted right by 5 is found as vector (-5, 0).
    #[test]
    fn search_finds_a_known_shift() {
        let (w, h) = (96, 64);
        let (prev, cur) = (luma(&scene(w, h, 0)), luma(&scene(w, h, 5)));
        let i = grid(w, h).0 + 2; // an interior block
        let (mv, s) = search(&cur, &prev, w, h, i, (0, 0));
        assert_eq!(mv, (-5, 0));
        assert_eq!(s, 0, "an exact shift predicts exactly");
    }

    /// A panned scene: interior blocks are predicted, and the frame decodes
    /// close to the true next picture -- far closer than the previous frame.
    #[test]
    fn a_panned_frame_is_predicted_and_decodes_close() {
        let (w, h) = (96, 64);
        let (prev, cur) = (scene(w, h, 0), scene(w, h, 5));
        let blocks: Vec<usize> = (0..grid(w, h).0 * grid(w, h).1).collect();
        let modes = plan(&cur, &prev, w, h, &blocks);
        let inter = modes.iter().filter(|m| m.is_some()).count();
        assert!(inter >= blocks.len() * 2 / 3, "most blocks predicted: {inter}/{}", blocks.len());
        let dec = decode(&encode(&cur, &prev, w, h, &blocks, &modes, 85), &prev, w, h).expect("decodes");
        let err = |a: &[u8]| a.iter().zip(&cur).map(|(p, q)| p.abs_diff(*q) as u64).sum::<u64>() / cur.len() as u64;
        assert!(err(&dec) <= 4, "decoded close to the new picture: mean error {}", err(&dec));
        assert!(err(&prev) > 10 * err(&dec).max(1), "and much closer than just the old one");
    }

    /// Prediction beats coding from scratch on size, for content that moved.
    #[test]
    fn predicted_blocks_cost_far_less_than_intra() {
        let (w, h) = (96, 64);
        let (prev, cur) = (scene(w, h, 0), scene(w, h, 5));
        let blocks: Vec<usize> = (0..grid(w, h).0 * grid(w, h).1).collect();
        let modes = plan(&cur, &prev, w, h, &blocks);
        let with_motion = encode(&cur, &prev, w, h, &blocks, &modes, 85).len();
        let intra_only = encode(&cur, &prev, w, h, &blocks, &vec![None; blocks.len()], 85).len();
        assert!(with_motion * 2 < intra_only, "motion {with_motion} bytes vs intra {intra_only}");
    }

    /// Unrelated content is NOT predicted -- a bad prediction would clip the
    /// residual and reconstruct wrongly.
    #[test]
    fn unrelated_content_falls_back_to_intra() {
        let (w, h) = (64, 48);
        let prev = vec![0u8; w * h * 3];
        let cur = scene(w, h, 0);
        let blocks: Vec<usize> = (0..grid(w, h).0 * grid(w, h).1).collect();
        assert!(plan(&cur, &prev, w, h, &blocks).iter().all(|m| m.is_none()));
    }

    #[test]
    fn malformed_payloads_are_rejected() {
        let (w, h) = (64, 48);
        let (prev, cur) = (scene(w, h, 0), scene(w, h, 3));
        let blocks = vec![1, 5];
        let good = encode(&cur, &prev, w, h, &blocks, &plan(&cur, &prev, w, h, &blocks), 85);
        assert!(decode(&good, &prev, w, h).is_some());
        assert!(decode(&good[..good.len() / 2], &prev, w, h).is_none(), "truncated");
        let mut bad = good.clone();
        bad[5 + 2] = 7; // first block's mode byte
        assert!(decode(&bad, &prev, w, h).is_none(), "unknown mode");
    }
}
