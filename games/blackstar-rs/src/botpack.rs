//! botpack — read `bots.pack`, the quantised fold strip.
//!
//! Written by `games/blackstar/tools/pack_bots.py`. The measurement is in
//! `docs/FLEET_COMPRESSION.md`: 770 MB of float32 vertex buffers to 130 MB, at
//! a worst round-trip error of 0.00076% of a model's span.
//!
//! ## One box per chassis, and that is the correctness property
//!
//! `xform::Morph` lerps vertex i of pose k toward vertex i of pose k+1. The
//! pack stores ONE quantisation box for the whole strip, so a given world
//! position decodes to the same value in every pose it appears in.
//!
//! A per-pose box would compress slightly better (18% vs 22% measured) and
//! would warp the fold: the vehicle and the robot have different extents —
//! quickfire's first pose spans [-1.00, 1.08] against the strip's
//! [-1.30, 1.08] — so the same position would land on different integers and
//! the lerp would interpolate between mismatched values. It would look correct
//! at both ends and wrong in the middle, which is the hardest kind of wrong to
//! notice.
//!
//! ## Layout
//!
//! ```text
//! "BSPK"  u16 version  u16 poses  f32 lo  f32 hi
//! poses x u32 original byte length
//! poses x (u32 compressed length, zlib(u16 stream))
//! ```

use std::path::Path;

pub const MAGIC: &[u8; 4] = b"BSPK";

/// Decode the pose that `dir` names, where `dir` is `<chassis>/fNN`.
///
/// Returns the float32 bytes the gltf loader would have read from scene.bin, so
/// the caller cannot tell the difference — which is what makes the pack a
/// storage change rather than a format change.
pub fn pose_buffer(dir: &Path) -> Option<Vec<u8>> {
    let chassis = dir.parent()?;
    let idx = pose_index(dir)?;
    let blob = std::fs::read(chassis.join("bots.pack")).ok()?;
    decode_pose(&blob, idx)
}

/// `f03` -> 3. The directory name IS the pose order — the packer sorts by it,
/// so a mismatch here would silently return the wrong pose rather than fail.
fn pose_index(dir: &Path) -> Option<usize> {
    let name = dir.file_name()?.to_str()?;
    name.strip_prefix('f')?.parse::<usize>().ok()
}

/// Pull one pose out of a pack blob.
pub fn decode_pose(blob: &[u8], idx: usize) -> Option<Vec<u8>> {
    if blob.len() < 16 || &blob[..4] != MAGIC {
        return None;
    }
    let version = u16::from_le_bytes(blob[4..6].try_into().ok()?);
    if version != 1 {
        // Refused rather than guessed: a future layout read as v1 would produce
        // plausible-looking garbage geometry.
        return None;
    }
    let poses = u16::from_le_bytes(blob[6..8].try_into().ok()?) as usize;
    if idx >= poses {
        return None;
    }
    let lo = f32::from_le_bytes(blob[8..12].try_into().ok()?);
    let hi = f32::from_le_bytes(blob[12..16].try_into().ok()?);

    let mut off = 16usize;
    let mut lens = Vec::with_capacity(poses);
    for _ in 0..poses {
        lens.push(u32::from_le_bytes(blob.get(off..off + 4)?.try_into().ok()?) as usize);
        off += 4;
    }
    // Walk to the pose we want. Sequential because the chunks are
    // variable-length; a chassis has five of them, so an index would cost more
    // bytes than it saves time.
    for i in 0..poses {
        let clen = u32::from_le_bytes(blob.get(off..off + 4)?.try_into().ok()?) as usize;
        off += 4;
        let chunk = blob.get(off..off + clen)?;
        off += clen;
        if i == idx {
            let raw = inflate(chunk)?;
            return Some(dequantise(&raw, lo, hi, lens[i]));
        }
    }
    None
}

/// u16 stream back to float32 bytes.
fn dequantise(q: &[u8], lo: f32, hi: f32, out_bytes: usize) -> Vec<u8> {
    // Span of zero is a degenerate model — every vertex at one coordinate. The
    // packer writes 1.0 in that case; matching it here keeps the two ends of
    // the format agreeing rather than producing NaN.
    let span = if hi > lo { hi - lo } else { 1.0 };
    let n = out_bytes / 4;
    let mut out = Vec::with_capacity(out_bytes);
    for i in 0..n {
        let u = u16::from_le_bytes([q[i * 2], q[i * 2 + 1]]) as f32;
        let v = lo + u / 65535.0 * span;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Minimal zlib inflate.
///
/// The crate has no flate2 dependency and this reads a file the repo produces,
/// so rather than add one for a decoder, the packer writes stored-and-fixed
/// blocks that this handles. See `pack_bots.py`.
fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_zlib(data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(lo: f32, hi: f32, poses: &[Vec<f32>]) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&(poses.len() as u16).to_le_bytes());
        out.extend_from_slice(&lo.to_le_bytes());
        out.extend_from_slice(&hi.to_le_bytes());
        for p in poses {
            out.extend_from_slice(&((p.len() * 4) as u32).to_le_bytes());
        }
        let span = if hi > lo { hi - lo } else { 1.0 };
        for p in poses {
            let mut q = Vec::new();
            for v in p {
                let u = (((v - lo) / span) * 65535.0).round() as u16;
                q.extend_from_slice(&u.to_le_bytes());
            }
            let c = miniz_oxide::deflate::compress_to_vec_zlib(&q, 6);
            out.extend_from_slice(&(c.len() as u32).to_le_bytes());
            out.extend_from_slice(&c);
        }
        out
    }

    fn as_floats(b: &[u8]) -> Vec<f32> {
        b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
    }

    #[test]
    fn a_pose_round_trips_within_a_quantisation_step() {
        let a = vec![-1.0f32, 0.0, 0.5, 1.0];
        let blob = pack(-1.0, 1.0, &[a.clone(), vec![0.1, 0.2, 0.3, 0.4]]);
        let got = as_floats(&decode_pose(&blob, 0).expect("decode"));
        let step = 2.0 / 65535.0;
        for (g, o) in got.iter().zip(a.iter()) {
            assert!((g - o).abs() <= step, "{g} vs {o}");
        }
    }

    #[test]
    fn the_same_value_decodes_identically_in_every_pose() {
        // THE FOLD-SAFETY PROPERTY. One box for the strip means one mapping, so
        // a vertex that has not moved between poses must not appear to move.
        // A per-pose box breaks exactly this, and only mid-fold.
        let shared = 0.25f32;
        let blob = pack(-1.3, 1.08, &[vec![shared, -1.0], vec![shared, 1.08]]);
        let p0 = as_floats(&decode_pose(&blob, 0).unwrap())[0];
        let p1 = as_floats(&decode_pose(&blob, 1).unwrap())[0];
        assert_eq!(p0, p1, "a stationary vertex moved between poses");
    }

    #[test]
    fn every_pose_is_reachable() {
        let blob = pack(0.0, 1.0, &[vec![0.0], vec![0.5], vec![1.0]]);
        for i in 0..3 {
            assert!(decode_pose(&blob, i).is_some(), "pose {i}");
        }
        assert!(decode_pose(&blob, 3).is_none(), "read past the end");
    }

    #[test]
    fn the_output_length_matches_the_original_exactly() {
        // The loader hands these bytes to gltf, which indexes by byte offset —
        // a buffer one float short is a mesh that reads into the next
        // accessor's data.
        let a = vec![0.0f32; 37];
        let blob = pack(0.0, 1.0, &[a.clone(), a.clone()]);
        assert_eq!(decode_pose(&blob, 0).unwrap().len(), 37 * 4);
    }

    #[test]
    fn a_wrong_magic_is_refused() {
        let mut blob = pack(0.0, 1.0, &[vec![0.0], vec![1.0]]);
        blob[0] = b'X';
        assert!(decode_pose(&blob, 0).is_none());
    }

    #[test]
    fn a_future_version_is_refused_rather_than_guessed() {
        // Read as v1, a new layout produces plausible-looking garbage geometry
        // rather than an error — the worst outcome available.
        let mut blob = pack(0.0, 1.0, &[vec![0.0], vec![1.0]]);
        blob[4] = 2;
        assert!(decode_pose(&blob, 0).is_none());
    }

    #[test]
    fn a_truncated_blob_returns_none_rather_than_panicking() {
        // Half-written packs happen: a full disk during packing is exactly how
        // this session has been going.
        let blob = pack(0.0, 1.0, &[vec![0.0, 1.0], vec![0.5, 0.5]]);
        for cut in [4, 10, 20, blob.len() - 3] {
            assert!(decode_pose(&blob[..cut], 0).is_none(), "cut at {cut}");
        }
    }

    #[test]
    fn a_degenerate_span_does_not_produce_nan() {
        // Every vertex at one coordinate. The packer writes span 1.0; matching
        // that here keeps both ends of the format agreeing.
        let blob = pack(0.5, 0.5, &[vec![0.5, 0.5], vec![0.5, 0.5]]);
        let got = as_floats(&decode_pose(&blob, 0).unwrap());
        assert!(got.iter().all(|v| v.is_finite()), "{got:?}");
    }

    #[test]
    fn the_pose_index_comes_from_the_directory_name() {
        // The packer sorts pose directories by name, so f03 must decode chunk
        // 3 — an off-by-one here returns the WRONG POSE, which renders fine and
        // folds wrong.
        assert_eq!(pose_index(Path::new("/x/quickfire/f03")), Some(3));
        assert_eq!(pose_index(Path::new("/x/quickfire/f00")), Some(0));
        assert_eq!(pose_index(Path::new("/x/quickfire/notapose")), None);
    }
}
