//! patch — "Quiltstone": frames that re-send only the 16x16 blocks that changed.
//!
//! ## Why
//!
//! Keyframe-only output was 4.8x the H.264 source (bench_1080p). Game footage
//! is mostly static between frames -- HUDs, menus, backgrounds -- and every one
//! of those pixels was re-encoded every frame. A patch frame sends a bitmap of
//! which blocks changed plus those blocks alone; the decoder copies the rest
//! from the previous frame.
//!
//! ## The design choices that matter
//!
//! * THE DCT CODEC IS REUSED UNCHANGED. Changed blocks are packed into a small
//!   "patch" image and compressed with `compress_frame_dct` -- the proven
//!   Keystone path -- so the only new bitstream is a header and a bitmap.
//!   Blocks are 16x16 at even origins, so 4:2:0 chroma (8x8 per block) packs
//!   without straddling.
//! * NO DRIFT. The library's whole-frame temporal codec predicts from the
//!   previous frame, and an encoder that does that from SOURCE while the
//!   decoder has only RECONSTRUCTIONS accumulates coding error over a run.
//!   Here a block is judged against a per-block REFERENCE: the source pixels it
//!   had the last time it was sent. A skipped block's error is therefore at
//!   most the skip tolerance plus that one coding -- it cannot accumulate.
//! * PARALLEL. Deciding WHICH blocks changed is cheap and sequential (it
//!   updates the reference). Compressing a frame's patch -- the expensive part
//!   -- needs only that frame and its block list, not the previous frame's
//!   reconstruction, so the transcoder's worker pool still applies.
//!
//! ## Payload
//!
//! ```text
//! b'M' | u32 LE block count k | bitmap, ceil(cols*rows / 8) bytes, row-major, LSB first
//!      | if k > 0: a Keystone DCT payload of the patch image
//! ```
//!
//! The patch is `min(k, PATCH_COLS)` blocks wide, blocks in bitmap order.
//! `k` is redundant with the bitmap's popcount and is checked against it.

use crate::decode::{rgb_to_yuv420p, yuv_to_rgb};
use fao_codec_lib as fao_v1;

pub const MARKER: u8 = b'M';
/// Block edge, in pixels.
pub const MB: usize = 16;
/// Patch image width, in blocks, once a frame has at least this many changed.
pub const PATCH_COLS: usize = 32;

/// Block grid for a frame: `(cols, rows)`, edge blocks partial.
pub fn grid(w: usize, h: usize) -> (usize, usize) {
    (w.div_ceil(MB), h.div_ceil(MB))
}

/// The in-frame rectangle of block `i`: `(x, y, bw, bh)`.
pub(crate) fn block_rect(i: usize, w: usize, h: usize) -> (usize, usize, usize, usize) {
    let cols = w.div_ceil(MB);
    let (x, y) = ((i % cols) * MB, (i / cols) * MB);
    (x, y, MB.min(w - x), MB.min(h - y))
}

/// Blocks whose pixels differ from `reference` by more than `tol` in any
/// channel of any pixel. Row-major block indices.
pub fn changed_blocks(frame: &[u8], reference: &[u8], w: usize, h: usize, tol: u8) -> Vec<usize> {
    let (cols, rows) = grid(w, h);
    (0..cols * rows)
        .filter(|&i| {
            let (x, y, bw, bh) = block_rect(i, w, h);
            (y..y + bh).any(|py| {
                let a = (py * w + x) * 3;
                let b = a + bw * 3;
                frame[a..b].iter().zip(&reference[a..b]).any(|(p, q)| p.abs_diff(*q) > tol)
            })
        })
        .collect()
}

/// Record that `blocks` were sent with `frame`'s pixels.
pub fn update_reference(reference: &mut [u8], frame: &[u8], w: usize, h: usize, blocks: &[usize]) {
    for &i in blocks {
        let (x, y, bw, _bh) = block_rect(i, w, h);
        for py in y..y + block_rect(i, w, h).3 {
            let a = (py * w + x) * 3;
            reference[a..a + bw * 3].copy_from_slice(&frame[a..a + bw * 3]);
        }
    }
}

/// Patch image size for `k` blocks, in pixels. Always multiples of 16.
pub fn patch_dims(k: usize) -> (usize, usize) {
    let cols = k.clamp(1, PATCH_COLS);
    (cols * MB, k.div_ceil(cols).max(1) * MB)
}

/// Pack `blocks` of `frame` into a patch image (RGB). A partial edge block is
/// padded by repeating its last row/column, so the DCT sees no invented edge.
pub fn build_patch(frame: &[u8], w: usize, h: usize, blocks: &[usize]) -> (Vec<u8>, usize, usize) {
    let (pw, ph) = patch_dims(blocks.len());
    let mut patch = vec![0u8; pw * ph * 3];
    let pcols = pw / MB;
    for (slot, &i) in blocks.iter().enumerate() {
        let (x, y, bw, bh) = block_rect(i, w, h);
        let (ox, oy) = ((slot % pcols) * MB, (slot / pcols) * MB);
        for py in 0..MB {
            let sy = y + py.min(bh - 1);
            for px in 0..MB {
                let sx = x + px.min(bw - 1);
                let s = (sy * w + sx) * 3;
                let d = ((oy + py) * pw + ox + px) * 3;
                patch[d..d + 3].copy_from_slice(&frame[s..s + 3]);
            }
        }
    }
    (patch, pw, ph)
}

/// Copy decoded patch blocks into `target` -- the in-frame part only.
pub fn apply_patch(target: &mut [u8], w: usize, h: usize, blocks: &[usize], patch: &[u8], pw: usize) {
    let pcols = pw / MB;
    for (slot, &i) in blocks.iter().enumerate() {
        let (x, y, bw, bh) = block_rect(i, w, h);
        let (ox, oy) = ((slot % pcols) * MB, (slot / pcols) * MB);
        for py in 0..bh {
            let s = ((oy + py) * pw + ox) * 3;
            let d = ((y + py) * w + x) * 3;
            target[d..d + bw * 3].copy_from_slice(&patch[s..s + bw * 3]);
        }
    }
}

/// Encode a patch frame: the header, the bitmap, and the changed blocks'
/// patch through the Keystone DCT codec.
pub fn encode(frame: &[u8], w: usize, h: usize, blocks: &[usize], quality: u8) -> Vec<u8> {
    let (cols, rows) = grid(w, h);
    let mut out = vec![MARKER];
    out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    let mut bitmap = vec![0u8; (cols * rows).div_ceil(8)];
    for &i in blocks {
        bitmap[i / 8] |= 1 << (i % 8);
    }
    out.extend_from_slice(&bitmap);
    if !blocks.is_empty() {
        let (patch, pw, ph) = build_patch(frame, w, h, blocks);
        out.extend_from_slice(&fao_v1::compress_frame_dct(&rgb_to_yuv420p(&patch, pw, ph), pw, ph, quality));
    }
    out
}

/// Decode a patch frame on top of the previous frame's picture. `None` for a
/// malformed payload -- wrong marker, short bitmap, a count that disagrees
/// with the bitmap, or a patch that will not decode.
pub fn decode(payload: &[u8], prev: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    let (cols, rows) = grid(w, h);
    let nbytes = (cols * rows).div_ceil(8);
    if payload.first() != Some(&MARKER) || payload.len() < 5 + nbytes || prev.len() != w * h * 3 {
        return None;
    }
    let k = u32::from_le_bytes(payload[1..5].try_into().ok()?) as usize;
    let bitmap = &payload[5..5 + nbytes];
    let blocks: Vec<usize> = (0..cols * rows).filter(|&i| bitmap[i / 8] & (1 << (i % 8)) != 0).collect();
    if blocks.len() != k {
        return None;
    }
    let mut out = prev.to_vec();
    if k > 0 {
        let (pw, ph) = patch_dims(k);
        let yuv = fao_v1::decompress_frame_dct(&payload[5 + nbytes..], pw, ph)?;
        apply_patch(&mut out, w, h, &blocks, &yuv_to_rgb(&yuv, pw, ph), pw);
    }
    Some(out)
}

/// Skip tolerance for a quality setting: exact at 95 and above, a few levels
/// below -- invisible, and enough to absorb a lossy source's decode noise in
/// static areas.
pub fn tolerance(quality: u8) -> u8 {
    if quality >= 95 { 0 } else { ((100 - quality.min(100) as u32) / 10) as u8 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: usize, h: usize, seed: u32) -> Vec<u8> {
        (0..w * h * 3).map(|i| ((i as u32 * 7 + seed * 31 + (i as u32 / w as u32) * 5) % 256) as u8).collect()
    }

    #[test]
    fn the_grid_covers_partial_edge_blocks() {
        assert_eq!(grid(64, 48), (4, 3));
        assert_eq!(grid(1920, 1080), (120, 68), "1080 = 67 whole rows + a half");
        assert_eq!(block_rect(4 * 2 + 3, 60, 40), (48, 32, 12, 8), "bottom-right partial block");
    }

    #[test]
    fn changed_blocks_find_exactly_the_touched_blocks() {
        let (w, h) = (64, 48);
        let a = img(w, h, 1);
        let mut b = a.clone();
        b[((20 * w) + 40) * 3] ^= 0x80; // pixel (40,20): block col 2, row 1 -> index 6
        b[((47 * w) + 63) * 3 + 2] ^= 0x80; // last pixel: block 11
        assert_eq!(changed_blocks(&b, &a, w, h, 0), vec![6, 11]);
        assert_eq!(changed_blocks(&a, &a, w, h, 0), Vec::<usize>::new());
    }

    #[test]
    fn a_difference_within_tolerance_is_not_a_change() {
        let (w, h) = (32, 32);
        let a = img(w, h, 2);
        let mut b = a.clone();
        b[0] = b[0].wrapping_add(if b[0] < 250 { 2 } else { 0 });
        assert!(changed_blocks(&b, &a, w, h, 2).is_empty(), "2 levels, tolerance 2");
        assert_eq!(changed_blocks(&b, &a, w, h, 1).len(), usize::from(a[0] < 250));
    }

    /// Pack then unpack with no codec in between is EXACT, including partial
    /// edge blocks -- the padding never leaks into the frame.
    #[test]
    fn build_then_apply_restores_the_blocks_exactly() {
        let (w, h) = (60, 40); // partial blocks on both edges
        let src = img(w, h, 3);
        let blocks = vec![0, 3, 4, 11];
        let (patch, pw, _) = build_patch(&src, w, h, &blocks);
        let mut target = vec![0u8; w * h * 3];
        apply_patch(&mut target, w, h, &blocks, &patch, pw);
        for &i in &blocks {
            let (x, y, bw, bh) = block_rect(i, w, h);
            for py in y..y + bh {
                let a = (py * w + x) * 3;
                assert_eq!(&target[a..a + bw * 3], &src[a..a + bw * 3], "block {i} row {py}");
            }
        }
        assert!(target[((20 * w) + 20) * 3..][..3].iter().all(|&v| v == 0), "an unsent block is untouched");
    }

    #[test]
    fn a_frame_with_no_changes_is_a_few_bytes_and_decodes_to_the_previous() {
        let (w, h) = (64, 48);
        let prev = img(w, h, 4);
        let p = encode(&prev, w, h, &[], 80);
        assert_eq!(p.len(), 1 + 4 + (4 * 3usize).div_ceil(8), "marker + count + bitmap, no patch");
        assert_eq!(decode(&p, &prev, w, h).unwrap(), prev);
    }

    /// Through the real DCT codec: changed blocks come back close to the new
    /// frame, and every other block is BIT-IDENTICAL to the previous picture.
    #[test]
    fn a_patch_frame_round_trips_through_the_dct_codec() {
        let (w, h) = (96, 64);
        let prev = img(w, h, 5);
        let mut next = prev.clone();
        for py in 16..32 { for px in 32..48 { let i = (py * w + px) * 3; next[i..i + 3].copy_from_slice(&[200, 40, 90]); } }
        let blocks = changed_blocks(&next, &prev, w, h, 0);
        assert_eq!(blocks, vec![grid(w, h).0 + 2], "exactly the one block painted");
        let dec = decode(&encode(&next, w, h, &blocks, 90), &prev, w, h).expect("decodes");
        for (i, (&d, &p)) in dec.iter().zip(&prev).enumerate() {
            let (px, py) = ((i / 3) % w, (i / 3) / w);
            let in_block = (32..48).contains(&px) && (16..32).contains(&py);
            if in_block {
                assert!(d.abs_diff(next[i]) <= 24, "changed pixel ({px},{py}) near its new value");
            } else {
                assert_eq!(d, p, "unchanged pixel ({px},{py}) copied exactly");
            }
        }
    }

    #[test]
    fn malformed_payloads_are_rejected_not_misread() {
        let (w, h) = (64, 48);
        let prev = img(w, h, 6);
        let good = encode(&prev, w, h, &[1, 2], 80);
        assert!(decode(&good, &prev, w, h).is_some());
        let mut bad_count = good.clone();
        bad_count[1] = 3; // says 3 blocks, bitmap has 2
        assert!(decode(&bad_count, &prev, w, h).is_none());
        assert!(decode(&good[..6], &prev, w, h).is_none(), "truncated bitmap");
        assert!(decode(&good, &prev[..10], w, h).is_none(), "wrong-size previous frame");
        let mut wrong = good.clone();
        wrong[0] = b'D';
        assert!(decode(&wrong, &prev, w, h).is_none());
    }

    #[test]
    fn tolerance_is_exact_at_high_quality() {
        assert_eq!(tolerance(100), 0);
        assert_eq!(tolerance(95), 0);
        assert_eq!(tolerance(80), 2);
        assert_eq!(tolerance(50), 5);
    }
}
