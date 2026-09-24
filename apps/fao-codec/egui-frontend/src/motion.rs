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
//! ## History: the quality slide, and its fix
//!
//! The first build of Q coded residuals as one 4:2:0 RGB image. On a camera
//! pan, quality slid ~30 dB -> ~26.6 dB across each run and compact lost to
//! keyframes at high quality. By elimination: not a rounding bias (flat 128
//! and 40 chained perfect predictions are exact); luma error levels off while
//! COLOUR error kept climbing; odd shifts were a quarter of it. Coding each of
//! R, G, B as its own full-resolution plane removed it -- colour error then
//! FALLS through a run, since full-resolution residuals repair what every
//! keyframe loses to 4:2:0. A size-based keyframe rule then cascaded (every
//! frame a keyframe), so compact mode keyframes on scene cuts instead.
//!
//! ## Payload (marker Q)
//!
//! ```text
//! b'Q' | u32 LE k | bitmap (as M) | per changed block, bitmap order:
//!        u8 mode (0 intra, 1 inter) [+ i8 dx, i8 dy if inter]
//!      | u32 LE intra length | intra DCT patch (Keystone)
//!      | u32 LE R length | R plane | u32 LE G length | G plane | B plane
//! ```
//!
//! (The first build of Q, 2eb4a5ca8, coded one 4:2:0 RGB residual instead.
//! Compact mode was opt-in and experimental; files from that build will not
//! decode.)
//!
//! Residuals are `src - pred + 128`, clamped to a byte, packed like intra
//! blocks -- a flat 128 where the prediction is good, which the DCT codes
//! almost for free. Each of R, G, B is coded as its OWN full-resolution plane
//! (the luma of a greyscale 4:2:0 image), never through a colour transform --
//! see "Why three planes" below. A vector points into the previous frame:
//! `cur(x, y) ~ prev(x + dx, y + dy)`, coordinates clamped at the frame edge.

use crate::decode::{rgb_to_yuv420p, yuv_to_rgb};
use crate::patch::{block_rect, grid, patch_dims, MB};
use fao_codec_lib as fao_v1;

/// Current layout: the three residual channels stacked into ONE plane.
pub const MARKER: u8 = b'U';
/// The first layout (2eb4a5ca8 .. a3434bbd7): each residual channel wrapped in
/// its own 4:2:0 image with neutral chroma. Decoded, never written -- files
/// encoded with compact mode as the default exist.
pub const MARKER_Q: u8 = b'Q';

/// Can `k` predicted blocks' stacked residual (3 x patch height) be stored?
/// The plane header holds dimensions as u16; this covers 4K (32,400 blocks).
pub fn fits(k: usize) -> bool {
    patch_dims(k).1 * 3 <= u16::MAX as usize
}
/// Search range, pixels, each axis. Fits an i8 with room to spare.
pub const RANGE: i32 = 24;

/// The quality residuals are coded at, for a frame quality `q`: 30% of it,
/// never below 5.
///
/// KEYFRAMES STAY AT `q`; only residuals are coarser. A residual is mostly
/// the previous frame's coding noise plus what moved, and predicted frames
/// inherit their detail from the keyframe -- so a coarse residual HOLDS the
/// keyframe's quality instead of exceeding it. At a fixed "q - 20" residual,
/// compact mode overshot: at q80 it spent its bytes pushing predicted frames
/// ~4 dB past the keyframe, and came out LARGER than keyframes-only.
///
/// Measured on a 1080p60 camera pan at q80 (120 frames):
///
///   residual quality     size      mean dB   worst dB
///   keyframes only     11.50 MB    31.83     31.67
///   q - 20 (60)        12.97 MB    36.18     31.94
///   q - 40 (40)        10.26 MB    34.82     31.94
///   q - 55 (25)         7.19 MB    33.85     31.94
///
/// The worst frame is the keyframe (31.94) in every row: predicted frames
/// never fall below it. At 30% (24 at q80) compact is ~37% smaller than
/// keyframes-only with a higher mean AND a higher worst frame. A fraction,
/// not a subtraction, so low settings do not hit quality 0.
pub fn residual_quality(q: u8) -> u8 {
    ((q as u32 * 30 / 100) as u8).max(5)
}

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
    // EARLY EXIT: if the zero or neighbour vector already matches to within
    // about 2 levels per pixel on average, the multi-step search cannot beat
    // it by anything a coarse residual would notice. In a pan every block
    // moves like its neighbour, so this skips nearly every search.
    let (_, _, bw, bh) = block_rect(i, w, h);
    if best_s <= (bw * bh * 2) as u32 {
        return (best, best_s);
    }
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

/// Channel `c` of an interleaved RGB image as the LUMA of a 4:2:0 image with
/// neutral chroma -- how the LEGACY Q layout coded each residual channel.
/// Test-only now: it builds real Q frames for the legacy-decode test. (Those
/// neutral chroma planes were a third of the residual DCT work, which is why
/// the U layout stacks the channels into one plane instead.)
#[cfg(test)]
fn plane_as_yuv(rgb: &[u8], c: usize, w: usize, h: usize) -> Vec<u8> {
    let mut yuv: Vec<u8> = rgb.chunks_exact(3).map(|p| p[c]).collect();
    yuv.resize(w * h + 2 * (w / 2) * (h / 2), 128);
    yuv
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
        // R, G, B stacked into one plane (pw x 3ph), coded once: no neutral
        // chroma planes, one header instead of three. See compress_plane_dct.
        let mut stacked = Vec::with_capacity(pw * ph * 3);
        for c in 0..3 {
            stacked.extend(r.chunks_exact(3).map(|p| p[c]));
        }
        out.extend_from_slice(&fao_v1::compress_plane_dct(&stacked, pw, ph * 3, residual_quality(quality)));
    }
    out
}

/// Decode a motion patch frame over the previous frame's picture. `None` for
/// anything malformed. Prediction reads `prev` (never the frame being built),
/// so block order cannot change the result.
pub fn decode(payload: &[u8], prev: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    let (cols, rows) = grid(w, h);
    let nbytes = (cols * rows).div_ceil(8);
    let marker = *payload.first()?;
    if (marker != MARKER && marker != MARKER_Q) || payload.len() < 5 + nbytes || prev.len() != w * h * 3 {
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
        let mut planes: Vec<Vec<u8>> = Vec::with_capacity(3);
        if marker == MARKER {
            let st = fao_v1::decompress_plane_dct(payload.get(at..)?, pw, ph * 3)?;
            for c in 0..3 {
                planes.push(st[c * pw * ph..(c + 1) * pw * ph].to_vec());
            }
        } else {
        for c in 0..3 {
            let data = if c < 2 {
                let n = u32::from_le_bytes(payload.get(at..at + 4)?.try_into().ok()?) as usize;
                at += 4;
                let d = payload.get(at..at + n)?;
                at += n;
                d
            } else {
                payload.get(at..)?
            };
            let yuv = fao_v1::decompress_frame_dct(data, pw, ph)?;
            planes.push(yuv.get(..pw * ph)?.to_vec());
        }
        }
        let pcols = pw / MB;
        for (slot, &(i, mv)) in inter.iter().enumerate() {
            let (x, y, bw, bh) = block_rect(i, w, h);
            let (ox, oy) = ((slot % pcols) * MB, (slot / pcols) * MB);
            for py in 0..bh {
                for px in 0..bw {
                    let p = pred(prev, w, h, x, y, mv, px, py);
                    let r = (oy + py) * pw + ox + px;
                    let d = ((y + py) * w + x + px) * 3;
                    for c in 0..3 {
                        out[d + c] = (p[c] as i32 + planes[c][r] as i32 - 128).clamp(0, 255) as u8;
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

    /// The LEGACY Q encoder, exactly as it shipped (each channel wrapped in a
    /// 4:2:0 image with neutral chroma, three payloads). Test-only: it exists
    /// to produce real Q frames, so the legacy decoder is checked against what
    /// actually wrote those files.
    fn encode_q_legacy(cur: &[u8], prev: &[u8], w: usize, h: usize, blocks: &[usize], modes: &[Option<(i8, i8)>], quality: u8) -> Vec<u8> {
        let mut out = encode(cur, prev, w, h, blocks, modes, quality);
        out[0] = MARKER_Q;
        let inter: Vec<(usize, (i8, i8))> = blocks.iter().zip(modes).filter_map(|(&i, m)| m.map(|mv| (i, mv))).collect();
        if inter.is_empty() { return out; }
        // Everything up to the residual is shared; replace the residual section.
        let (cols, rows) = grid(w, h);
        let mut at = 5 + (cols * rows).div_ceil(8);
        at += modes.iter().map(|m| if m.is_some() { 3 } else { 1 }).sum::<usize>();
        let ilen = u32::from_le_bytes(out[at..at + 4].try_into().unwrap()) as usize;
        out.truncate(at + 4 + ilen);
        let (r, pw, ph) = residual_patch(cur, prev, w, h, &inter);
        for c in 0..3 {
            let plane = fao_v1::compress_frame_dct(&plane_as_yuv(&r, c, pw, ph), pw, ph, residual_quality(quality));
            if c < 2 { out.extend_from_slice(&(plane.len() as u32).to_le_bytes()); }
            out.extend_from_slice(&plane);
        }
        out
    }

    /// OLD FILES STILL DECODE: a real legacy Q frame decodes, to the same
    /// picture as the new U layout within coding noise (the residual is coded
    /// the same way; only the packaging differs).
    #[test]
    fn legacy_q_frames_still_decode() {
        let (w, h) = (96, 64);
        let (prev, cur) = (scene(w, h, 0), scene(w, h, 5));
        let blocks: Vec<usize> = (0..grid(w, h).0 * grid(w, h).1).collect();
        let modes = plan(&cur, &prev, w, h, &blocks);
        assert!(modes.iter().any(|m| m.is_some()), "the fixture must exercise the residual path");
        let q = encode_q_legacy(&cur, &prev, w, h, &blocks, &modes, 80);
        let u = encode(&cur, &prev, w, h, &blocks, &modes, 80);
        assert_eq!((q[0], u[0]), (MARKER_Q, MARKER));
        let (dq, du) = (decode(&q, &prev, w, h).expect("legacy Q decodes"), decode(&u, &prev, w, h).expect("U decodes"));
        assert_eq!(dq, du, "same residual coding, so the same picture");
        assert!(u.len() < q.len(), "one header, no neutral chroma: U {} vs Q {} bytes", u.len(), q.len());
    }

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
