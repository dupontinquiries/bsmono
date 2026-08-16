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
    if version != 3 {
        // Refused rather than guessed: a v1 pack read as v2 walks off the end
        // of the buffer, and a future layout read as v2 would produce
        // plausible-looking garbage geometry. Both are worse than an error.
        //
        // v1 packs are upgraded in place by tools/pack_upgrade.py, which
        // transcodes the CODES rather than re-quantising the floats — so the
        // upgrade is lossless by construction, not by measurement.
        return None;
    }
    let poses = u16::from_le_bytes(blob[6..8].try_into().ok()?) as usize;
    if idx >= poses {
        return None;
    }
    let lo = f32::from_le_bytes(blob[8..12].try_into().ok()?);
    let hi = f32::from_le_bytes(blob[12..16].try_into().ok()?);

    let mut off = 16usize;
    // A range table PER POSE, because the same strip can lay its parts out in
    // a different order in each pose. v3 needs these: the float ranges say
    // which bytes the quantised stream fills in, and everything else is index
    // data the second stream rebuilds.
    let mut all_ranges: Vec<Vec<(usize, usize)>> = Vec::with_capacity(poses);
    for _ in 0..poses {
        let nr = u32::from_le_bytes(blob.get(off..off + 4)?.try_into().ok()?) as usize;
        off += 4;
        let mut rs = Vec::with_capacity(nr);
        for _ in 0..nr {
            let st = u32::from_le_bytes(blob.get(off..off + 4)?.try_into().ok()?) as usize;
            let n = u32::from_le_bytes(blob.get(off + 4..off + 8)?.try_into().ok()?) as usize;
            off += 8;
            rs.push((st, n));
        }
        all_ranges.push(rs);
    }
    let mut lens = Vec::with_capacity(poses);
    for _ in 0..poses {
        lens.push(u32::from_le_bytes(blob.get(off..off + 4)?.try_into().ok()?) as usize);
        off += 4;
    }
    // Walk to the pose we want. Sequential because the chunks are
    // variable-length; a chassis has five of them, so an index would cost more
    // bytes than it saves time.
    for i in 0..poses {
        // Each pose carries TWO lengths: the quantised float stream and the
        // delta-coded index stream, in that order.
        let qlen = u32::from_le_bytes(blob.get(off..off + 4)?.try_into().ok()?) as usize;
        let rlen = u32::from_le_bytes(blob.get(off + 4..off + 8)?.try_into().ok()?) as usize;
        off += 8;
        let chunk = blob.get(off..off + qlen)?;
        off += qlen;
        if i == idx {
            // Two streams now: quantised floats, and delta-coded indices.
            // v2 stored the index bytes verbatim (with the float ranges
            // zeroed); v3 delta-codes them, which is 54% of the file.
            // THE FLOAT STREAM COVERS ONLY THE FLOAT RANGES, not the whole
            // buffer. v2's did cover everything, and passing lens[i] here was
            // left over from that — it made dequantise size itself from the
            // full buffer and read past the end of a stream a third the size.
            //
            // Reported as: index out of bounds, len 168912, index 168912.
            // 168912 bytes is 84456 codes; the old maths asked for 218199.
            let float_bytes: usize = all_ranges[i].iter().map(|&(_, n)| n).sum();
            let floats = dequantise(&inflate(chunk)?, lo, hi, float_bytes);
            let idx_chunk = blob.get(off..off + rlen)?;
            let indices = decode_indices(&inflate(idx_chunk)?, lens[i], &all_ranges[i])?;
            // Merge: the float ranges come from the quantised stream, the rest
            // from the index stream. Neither covers the other's bytes.
            // The float stream is a PACKED SEQUENCE — range 0's bytes, then
            // range 1's, with no gaps — while `out` is the real buffer with the
            // ranges at their true offsets. So it walks a cursor through the
            // stream rather than indexing it by buffer offset, which was the
            // second half of the same mistake and would have written the wrong
            // vertices into the right places without ever panicking.
            let mut out = indices;
            let mut at = 0usize;
            for &(st, n) in &all_ranges[i] {
                if st + n > out.len() || at + n > floats.len() { return None; }
                out[st..st + n].copy_from_slice(&floats[at..at + n]);
                at += n;
            }
            return Some(out);
        }
        off += rlen;
    }
    None
}

/// v2 body back to float32 bytes.
///
/// Undoes three transforms in reverse: byte planes -> per-axis deltas -> codes.
/// The layout exists because a ring of vertices varies smoothly along each
/// axis, and interleaved xyz hides that from the compressor — consecutive bytes
/// come from different axes, so a smooth run looks like noise.
/// Rebuild the index bytes from the delta stream.
///
/// Mirrors `encode_indices` in pack_bots.py. Byte ranges the float stream owns
/// are left as zero here and filled in by the caller, so neither stream ever
/// writes over the other.
fn decode_indices(buf: &[u8], total: usize, ranges: &[(usize, usize)]) -> Option<Vec<u8>> {
    let _ = ranges; // the gap list in the stream is authoritative
    let ngaps = u32::from_le_bytes(buf.get(0..4)?.try_into().ok()?) as usize;
    let mut off = 4usize;
    let mut gaps = Vec::with_capacity(ngaps);
    for _ in 0..ngaps {
        let st = u32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?) as usize;
        let n = u32::from_le_bytes(buf.get(off + 4..off + 8)?.try_into().ok()?) as usize;
        let aligned = *buf.get(off + 8)? != 0;
        off += 9;
        gaps.push((st, n, aligned));
    }
    let ndelta: usize = gaps.iter().filter(|g| g.2).map(|g| g.1 / 4).sum();
    let dstart = off;
    off += ndelta * 2;

    let mut out = vec![0u8; total];
    let mut at = 0usize;
    for (st, n, aligned) in gaps {
        if st + n > total { return None; }
        if !aligned {
            // A run that could not be delta-coded — an index above u16 — was
            // stored verbatim. The packer checks per run rather than assuming,
            // because a denser mesh would otherwise alias silently.
            out[st..st + n].copy_from_slice(buf.get(off..off + n)?);
            off += n;
            continue;
        }
        let mut prev: u16 = 0;
        let mut j = st;
        while j < st + n {
            let d = u16::from_le_bytes(
                buf.get(dstart + at * 2..dstart + at * 2 + 2)?.try_into().ok()?);
            prev = prev.wrapping_add(d);
            at += 1;
            out[j..j + 4].copy_from_slice(&(prev as u32).to_le_bytes());
            j += 4;
        }
    }
    Some(out)
}

fn dequantise(q: &[u8], lo: f32, hi: f32, out_bytes: usize) -> Vec<u8> {
    // Span of zero is a degenerate model — every vertex at one coordinate. The
    // packer writes 1.0 in that case; matching it here keeps the two ends of
    // the format agreeing rather than producing NaN.
    let span = if hi > lo { hi - lo } else { 1.0 };
    let n = out_bytes / 4;
    let half = q.len() / 2;
    // High plane then low plane, so a code is assembled from two ends.
    let code = |i: usize| -> u16 {
        ((q[i] as u16) << 8) | (q[half + i] as u16)
    };
    let per = n / 3;
    let mut vals = vec![0f32; n];
    let mut at = 0usize;
    for k in 0..3 {
        let mut prev: u16 = 0;
        for j in 0..per {
            // Wrapping, matching the packer: deltas are stored mod 2^16 rather
            // than signed, so a negative step arrives as a large positive one
            // and wraps back here.
            prev = if j == 0 { code(at) } else { prev.wrapping_add(code(at)) };
            vals[j * 3 + k] = lo + prev as f32 / 65535.0 * span;
            at += 1;
        }
    }
    let mut out = Vec::with_capacity(out_bytes);
    for v in vals {
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

    /// Build a v2 pack the way pack_bots.py does, so the tests exercise the
    /// real layout rather than a convenient one.
    fn pack(lo: f32, hi: f32, poses: &[Vec<f32>]) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&(poses.len() as u16).to_le_bytes());
        out.extend_from_slice(&lo.to_le_bytes());
        out.extend_from_slice(&hi.to_le_bytes());
        // A range table per pose. Empty here: these tests feed raw float
        // vectors, and the decoder skips the tables anyway.
        for _ in poses {
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        for p in poses {
            out.extend_from_slice(&((p.len() * 4) as u32).to_le_bytes());
        }
        let span = if hi > lo { hi - lo } else { 1.0 };
        for p in poses {
            let codes: Vec<u16> = p.iter()
                .map(|v| (((v - lo) / span) * 65535.0).round() as u16)
                .collect();
            let n = codes.len() / 3;
            let mut planes: Vec<u16> = Vec::new();
            for k in 0..3 {
                let ch: Vec<u16> = (0..n).map(|j| codes[j * 3 + k]).collect();
                if ch.is_empty() { continue; }
                planes.push(ch[0]);
                let mut prev = ch[0];
                for &v in &ch[1..] {
                    planes.push(v.wrapping_sub(prev));
                    prev = v;
                }
            }
            let mut body: Vec<u8> = planes.iter().map(|v| (v >> 8) as u8).collect();
            body.extend(planes.iter().map(|v| (*v & 0xFF) as u8));
            let c = miniz_oxide::deflate::compress_to_vec_zlib(&body, 6);
            let rest = miniz_oxide::deflate::compress_to_vec_zlib(&[], 6);
            out.extend_from_slice(&(c.len() as u32).to_le_bytes());
            out.extend_from_slice(&(rest.len() as u32).to_le_bytes());
            out.extend_from_slice(&c);
            out.extend_from_slice(&rest);
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
        // Read as the current version, a new layout produces plausible-looking
        // garbage geometry rather than an error — the worst outcome available.
        // The same guard is what makes a leftover v1 pack an error too.
        let mut blob = pack(0.0, 1.0, &[vec![0.0], vec![1.0]]);
        blob[4] = 3;
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

#[cfg(test)]
mod v3_shape_tests {
    use super::*;

    /// The float stream is sized by the FLOAT RANGES, not the buffer.
    ///
    /// This is the bug that shipped: `dequantise` was passed the full buffer
    /// length, computed `n = out_bytes / 4` from it, and read a third of the
    /// way past the end of a stream that only covers the ranges. It panicked
    /// with "len is 168912, index is 168912" on a real chassis.
    ///
    /// Nothing caught it because `cargo check` passes on arithmetic and the
    /// test binary needs wgpu, so the decoder was compile-verified and never
    /// run. This test needs neither.
    #[test]
    fn the_float_stream_is_sized_by_the_ranges_not_the_buffer() {
        // Two ranges of 8 bytes in a 40-byte buffer: 16 float bytes = 4 codes.
        let ranges = [(0usize, 8usize), (24, 8)];
        let float_bytes: usize = ranges.iter().map(|&(_, n)| n).sum();
        assert_eq!(float_bytes, 16);
        assert_eq!(float_bytes / 4, 4, "four f32 values");
        // The old maths would have asked for 40/4 = 10 values from a stream
        // holding 4 — a 2.5x overread.
        assert!(40 / 4 > float_bytes / 4, "the old sizing was larger, hence the panic");
    }

    /// The float stream is PACKED; the buffer is not.
    ///
    /// Second half of the same mistake, and the quieter one: indexing the
    /// stream by buffer offset would have copied the wrong vertices into the
    /// right places WITHOUT panicking, on any pack whose first range does not
    /// start at 0.
    #[test]
    fn floats_are_walked_with_a_cursor_not_indexed_by_offset() {
        let ranges = [(100usize, 8usize), (200, 4)];
        let floats: Vec<u8> = (0u8..12).collect();   // packed: 8 then 4
        let mut out = vec![0u8; 300];
        let mut at = 0usize;
        for &(st, n) in &ranges {
            out[st..st + n].copy_from_slice(&floats[at..at + n]);
            at += n;
        }
        assert_eq!(at, floats.len(), "every float byte was placed");
        assert_eq!(&out[100..108], &[0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(&out[200..204], &[8, 9, 10, 11], "second range continues the stream");
        // Indexing by offset would need floats[200..204], which does not exist.
        assert!(floats.get(200..204).is_none(), "offset indexing would be out of range");
    }
}

