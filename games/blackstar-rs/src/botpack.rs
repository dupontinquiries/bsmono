//! botpack — read the quantised strips: `bots.pack` and `walk.pack`.
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
//! v3 (per-pose zlib):
//!
//! ```text
//! "BSPK"  u16 version=3  u16 poses  f32 lo  f32 hi
//! poses x range table
//! poses x u32 original byte length
//! poses x (u32 qlen, u32 rlen, zlib(quantised stream), zlib(index stream))
//! ```
//!
//! v4 (solid zstd) carries THE SAME two raw streams per pose — same codes,
//! same box, same index deltas — but compresses the whole strip as one zstd
//! frame instead of zlib per pose. zlib's 32 KB window cannot see from one
//! pose to the next; a strip-wide window can, and poses of one chassis are
//! highly redundant (often the entire index stream repeats verbatim).
//! Measured on a 14-strip sample: 49% of the v3 size, byte-identical decode.
//!
//! ```text
//! "BSPK"  u16 version=4  u16 poses  f32 lo  f32 hi
//! poses x range table                      (as v3)
//! poses x u32 original byte length         (as v3)
//! poses x (u32 qraw, u32 rraw)             UNCOMPRESSED stream lengths
//! u32 zlen, zstd frame = q0..qN-1 ++ r0..rN-1
//! ```
//!
//! Because the streams are untouched, v3 <-> v4 is a lossless transcode
//! (`tools/pack_v4.py`), and the fold-safety argument above transfers as-is.

use std::path::Path;

pub const MAGIC: &[u8; 4] = b"BSPK";

/// Decode the pose that `dir` names, where `dir` is `<chassis>/fNN`.
///
/// Returns the float32 bytes the gltf loader would have read from scene.bin, so
/// the caller cannot tell the difference — which is what makes the pack a
/// storage change rather than a format change.
pub fn pose_buffer(dir: &Path) -> Option<Vec<u8>> {
    let chassis = dir.parent()?;
    let (kind, idx) = pose_index(dir)?;
    // WHICH PACK depends on the pose prefix. A chassis has two independent
    // strips — f00.. is the fold, w00.. the walk cycle — packed separately
    // because they have separate quantisation boxes and nothing ever lerps a
    // fold pose toward a walk pose.
    //
    // The first version hardcoded "bots.pack" and pose_index only accepted 'f',
    // so a walk pose failed both checks and the loader panicked with
    // "no scene.bin and no bots.pack for assets/bots/analog/w00".
    let file = match kind {
        Strip::Fold => "bots.pack",
        Strip::Walk => "walk.pack",
    };
    let path = chassis.join(file);
    let blob = std::fs::read(&path).ok()?;
    // v4 packs decompress the WHOLE strip per call, and the loader asks for a
    // chassis's poses consecutively (f00, f01, ...) — so cache the last
    // decompressed frame. One entry, not a map: the access pattern is strictly
    // sequential per chassis, and a map would pin every strip's frame
    // (~2-10 MB each, ~300 chassis) in memory for no second hit.
    if blob.len() >= 6 && &blob[..4] == MAGIC
        && u16::from_le_bytes([blob[4], blob[5]]) == 4
    {
        use std::sync::{Arc, Mutex, OnceLock};
        static LAST: OnceLock<Mutex<Option<(std::path::PathBuf, usize, Arc<Vec<u8>>)>>> =
            OnceLock::new();
        let h = parse_header(&blob)?;
        let (frame_off, zlen) = v4_frame_extent(&blob, &h)?;
        let cache = LAST.get_or_init(|| Mutex::new(None));
        let frame = {
            let mut slot = cache.lock().ok()?;
            match &*slot {
                // zlen in the key so a repacked file at the same path misses.
                Some((p, z, f)) if p == &path && *z == zlen => f.clone(),
                _ => {
                    let f = Arc::new(unzstd(blob.get(frame_off..frame_off + zlen)?)?);
                    *slot = Some((path.clone(), zlen, f.clone()));
                    f
                }
            }
        };
        return v4_extract(&blob, &h, &frame, idx);
    }
    decode_pose(&blob, idx)
}

/// Which strip a pose directory belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Strip { Fold, Walk }

/// `f03` -> (Fold, 3), `w01` -> (Walk, 1).
///
/// The directory name IS the pose order within its strip — the packer sorts by
/// it — so a mismatch here would silently return the wrong pose rather than
/// fail. Both prefixes are handled explicitly rather than "strip one char",
/// because an unknown prefix should return None and let the caller fall back to
/// scene.bin, not index into whichever pack it guessed.
fn pose_index(dir: &Path) -> Option<(Strip, usize)> {
    let name = dir.file_name()?.to_str()?;
    let (kind, rest) = match name.as_bytes().first()? {
        b'f' => (Strip::Fold, &name[1..]),
        b'w' => (Strip::Walk, &name[1..]),
        _ => return None,
    };
    Some((kind, rest.parse::<usize>().ok()?))
}

/// The header both versions share: magic through the length table.
struct Header {
    poses: usize,
    lo: f32,
    hi: f32,
    all_ranges: Vec<Vec<(usize, usize)>>,
    lens: Vec<usize>,
    /// Byte offset of the first version-specific byte.
    off: usize,
}

fn parse_header(blob: &[u8]) -> Option<Header> {
    let poses = u16::from_le_bytes(blob.get(6..8)?.try_into().ok()?) as usize;
    let lo = f32::from_le_bytes(blob.get(8..12)?.try_into().ok()?);
    let hi = f32::from_le_bytes(blob.get(12..16)?.try_into().ok()?);

    let mut off = 16usize;
    // A range table PER POSE, because the same strip can lay its parts out in
    // a different order in each pose. The float ranges say which bytes the
    // quantised stream fills in, and everything else is index data the second
    // stream rebuilds.
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
    Some(Header { poses, lo, hi, all_ranges, lens, off })
}

/// Rebuild one pose's scene.bin bytes from its two UNCOMPRESSED streams.
///
/// Shared by v3 (streams inflated from per-pose zlib) and v4 (streams sliced
/// out of the strip-wide zstd frame) — the streams themselves are identical
/// between the versions, which is what makes the transcode lossless.
fn rebuild(
    qraw: &[u8],
    rraw: &[u8],
    ranges: &[(usize, usize)],
    total: usize,
    lo: f32,
    hi: f32,
) -> Option<Vec<u8>> {
    // THE FLOAT STREAM COVERS ONLY THE FLOAT RANGES, not the whole buffer.
    // v2's did cover everything, and sizing dequantise from the full buffer
    // read past the end of a stream a third the size — reported as
    // "index out of bounds, len 168912, index 168912".
    let float_bytes: usize = ranges.iter().map(|&(_, n)| n).sum();
    let floats = dequantise(qraw, lo, hi, float_bytes);
    let indices = decode_indices(rraw, total, ranges)?;
    // Merge: the float ranges come from the quantised stream, the rest from
    // the index stream. The float stream is a PACKED SEQUENCE — range 0's
    // bytes, then range 1's, with no gaps — while `out` is the real buffer
    // with the ranges at their true offsets. So walk a cursor through the
    // stream rather than indexing it by buffer offset, which would write the
    // wrong vertices into the right places without ever panicking.
    let mut out = indices;
    let mut at = 0usize;
    for &(st, n) in ranges {
        if st + n > out.len() || at + n > floats.len() {
            return None;
        }
        out[st..st + n].copy_from_slice(&floats[at..at + n]);
        at += n;
    }
    Some(out)
}

/// Pull one pose out of a pack blob.
pub fn decode_pose(blob: &[u8], idx: usize) -> Option<Vec<u8>> {
    if blob.len() < 16 || &blob[..4] != MAGIC {
        return None;
    }
    let version = u16::from_le_bytes(blob[4..6].try_into().ok()?);
    match version {
        // Refused rather than guessed for anything else: a v1 pack read as v2
        // walks off the end of the buffer, and a future layout read as a known
        // one would produce plausible-looking garbage geometry. Both are worse
        // than an error.
        //
        // v1 packs are upgraded in place by tools/pack_upgrade.py, v3 packs to
        // v4 by tools/pack_v4.py; both transcode the CODES rather than
        // re-quantising the floats — so the upgrade is lossless by
        // construction, not by measurement.
        3 => decode_pose_v3(blob, idx),
        4 => {
            let h = parse_header(blob)?;
            let (frame_off, zlen) = v4_frame_extent(blob, &h)?;
            let frame = unzstd(blob.get(frame_off..frame_off + zlen)?)?;
            v4_extract(blob, &h, &frame, idx)
        }
        _ => None,
    }
}

/// Where the v4 zstd frame sits in the blob, without decompressing it.
fn v4_frame_extent(blob: &[u8], h: &Header) -> Option<(usize, usize)> {
    let z_off = h.off + h.poses * 8; // past the (qraw, rraw) pairs
    let zlen = u32::from_le_bytes(blob.get(z_off..z_off + 4)?.try_into().ok()?) as usize;
    Some((z_off + 4, zlen))
}

/// Slice pose `idx`'s two raw streams out of the decompressed v4 frame and
/// rebuild the buffer.
fn v4_extract(blob: &[u8], h: &Header, frame: &[u8], idx: usize) -> Option<Vec<u8>> {
    if idx >= h.poses {
        return None;
    }
    let mut off = h.off;
    let mut qlens = Vec::with_capacity(h.poses);
    let mut rlens = Vec::with_capacity(h.poses);
    for _ in 0..h.poses {
        qlens.push(u32::from_le_bytes(blob.get(off..off + 4)?.try_into().ok()?) as usize);
        rlens.push(u32::from_le_bytes(blob.get(off + 4..off + 8)?.try_into().ok()?) as usize);
        off += 8;
    }
    // Frame layout is all q streams then all r streams — matching how the
    // measurement was taken. Grouping like with like keeps the index streams
    // adjacent, which is where the cross-pose matches are.
    let qtotal: usize = qlens.iter().sum();
    let qstart: usize = qlens[..idx].iter().sum();
    let rstart: usize = qtotal + rlens[..idx].iter().sum::<usize>();
    let qraw = frame.get(qstart..qstart + qlens[idx])?;
    let rraw = frame.get(rstart..rstart + rlens[idx])?;
    rebuild(qraw, rraw, &h.all_ranges[idx], *h.lens.get(idx)?, h.lo, h.hi)
}

fn decode_pose_v3(blob: &[u8], idx: usize) -> Option<Vec<u8>> {
    let h = parse_header(blob)?;
    let Header { poses, lo, hi, all_ranges, lens, mut off } = h;
    if idx >= poses {
        return None;
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
            // Two streams: quantised floats and delta-coded indices. v2
            // stored the index bytes verbatim (with the float ranges zeroed);
            // v3 delta-codes them, which is 54% of the file. The stream
            // sizing and merge pitfalls live in `rebuild`'s comments.
            let idx_chunk = blob.get(off..off + rlen)?;
            return rebuild(&inflate(chunk)?, &inflate(idx_chunk)?,
                           &all_ranges[i], lens[i], lo, hi);
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

/// zstd inflate for v4 frames.
///
/// ruzstd rather than the `zstd` crate for the same reason miniz_oxide rather
/// than flate2 above: this only ever DECOMPRESSES a file the repo produces,
/// and ruzstd is pure Rust — no C toolchain in the cross builds. (Gotcha for
/// upgrades: ruzstd 0.8 moved `StreamingDecoder` to `ruzstd::decoding::` and
/// needs a newer cargo resolver; the import below is the 0.7 path.)
fn unzstd(data: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut dec = ruzstd::StreamingDecoder::new(data).ok()?;
    let mut out = Vec::new();
    dec.read_to_end(&mut out).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a v3 pack the way pack_bots.py's `pack_chassis` does now —
    /// matching decode_pose's actual requirement (`version != 3 ->
    /// None`). Previously wrote a v2 blob (`2u16` hardcoded, empty
    /// per-pose range tables) and its own comment said so; that stopped
    /// being true the moment decode_pose was upgraded to require v3
    /// (the delta-coded-indices migration), and every test below that
    /// checks actual decoded values — not just is_some()/is_none() —
    /// was failing as a result, silently misfiled as environment noise
    /// for a long time before anyone read what they actually asserted.
    ///
    /// One range per pose, covering the WHOLE buffer in bytes (every
    /// value in these synthetic poses is float data, nothing is index
    /// data) — which per pack_bots.py's `encode_indices` gap-finding
    /// loop means zero gaps, so the index stream is the real encoding
    /// of that case: `struct.pack("<I", 0)`, four zero bytes, zlib'd —
    /// not an empty stream, which starves decode_indices's own first
    /// read (`ngaps` from those same four bytes).
    ///
    /// Quantisation itself is unchanged from before and matches
    /// `dequantise`'s assumption of three EQUAL-length channels
    /// (`per = n / 3`, contiguous grouping) — which is what
    /// `pack_bots.py`'s real `q[k::3]` striding also produces whenever
    /// the float count is a multiple of 3, which every real chassis is
    /// (xyz triples). Test poses below were changed to respect that —
    /// they previously used counts that were NOT multiples of 3
    /// (4, 2, 1), which silently drops the remainder under contiguous
    /// grouping and was a second, independent bug in the fixture, not
    /// something this format needs to tolerate for real data.
    fn pack(lo: f32, hi: f32, poses: &[Vec<f32>]) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&3u16.to_le_bytes());
        out.extend_from_slice(&(poses.len() as u16).to_le_bytes());
        out.extend_from_slice(&lo.to_le_bytes());
        out.extend_from_slice(&hi.to_le_bytes());
        for p in poses {
            let byte_len = (p.len() * 4) as u32;
            out.extend_from_slice(&1u32.to_le_bytes()); // one range
            out.extend_from_slice(&0u32.to_le_bytes()); // start
            out.extend_from_slice(&byte_len.to_le_bytes()); // count, BYTES
        }
        for p in poses {
            out.extend_from_slice(&((p.len() * 4) as u32).to_le_bytes());
        }
        for (body, idx_body) in raw_streams(lo, hi, poses) {
            let c = miniz_oxide::deflate::compress_to_vec_zlib(&body, 6);
            let rest = miniz_oxide::deflate::compress_to_vec_zlib(&idx_body, 6);
            out.extend_from_slice(&(c.len() as u32).to_le_bytes());
            out.extend_from_slice(&(rest.len() as u32).to_le_bytes());
            out.extend_from_slice(&c);
            out.extend_from_slice(&rest);
        }
        out
    }

    /// The two UNCOMPRESSED streams per pose, exactly as pack_bots.py's
    /// `quantise` + `encode_indices` produce them for all-float poses.
    /// Shared by the v3 and v4 fixtures — the format versions differ only in
    /// how these bytes are compressed, and the tests should pin that.
    fn raw_streams(lo: f32, hi: f32, poses: &[Vec<f32>]) -> Vec<(Vec<u8>, Vec<u8>)> {
        let span = if hi > lo { hi - lo } else { 1.0 };
        poses.iter().map(|p| {
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
            // ngaps = 0, real encoding (see doc comment above), not an
            // empty stream.
            (body, 0u32.to_le_bytes().to_vec())
        }).collect()
    }

    /// A valid zstd frame of RAW blocks — no encoder dependency, and ruzstd
    /// decodes it like any other frame. Single-segment, 8-byte content size.
    fn store_zstd(payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0x28, 0xB5, 0x2F, 0xFD];
        out.push(0b1110_0000); // FCS=8 bytes (bits 7-6), single segment (bit 5)
        out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        if payload.is_empty() {
            out.extend_from_slice(&[0b1, 0, 0]); // one last, empty, raw block
            return out;
        }
        let mut it = payload.chunks(0x1FFFF).peekable(); // < the 128 KB block cap
        while let Some(c) = it.next() {
            let last = it.peek().is_none() as u32;
            let hdr = last | ((c.len() as u32) << 3); // type bits 1-2 = 0 = raw
            out.extend_from_slice(&hdr.to_le_bytes()[..3]);
            out.extend_from_slice(c);
        }
        out
    }

    /// Same header as pack(), then the v4 tail: uncompressed stream lengths
    /// and one solid zstd frame of q-streams then r-streams.
    fn pack_v4(lo: f32, hi: f32, poses: &[Vec<f32>]) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&(poses.len() as u16).to_le_bytes());
        out.extend_from_slice(&lo.to_le_bytes());
        out.extend_from_slice(&hi.to_le_bytes());
        for p in poses {
            let byte_len = (p.len() * 4) as u32;
            out.extend_from_slice(&1u32.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&byte_len.to_le_bytes());
        }
        for p in poses {
            out.extend_from_slice(&((p.len() * 4) as u32).to_le_bytes());
        }
        let streams = raw_streams(lo, hi, poses);
        for (q, r) in &streams {
            out.extend_from_slice(&(q.len() as u32).to_le_bytes());
            out.extend_from_slice(&(r.len() as u32).to_le_bytes());
        }
        let mut payload = Vec::new();
        for (q, _) in &streams { payload.extend_from_slice(q); }
        for (_, r) in &streams { payload.extend_from_slice(r); }
        let frame = store_zstd(&payload);
        out.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        out.extend_from_slice(&frame);
        out
    }

    fn as_floats(b: &[u8]) -> Vec<f32> {
        b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
    }

    #[test]
    fn a_pose_round_trips_within_a_quantisation_step() {
        // six values: a multiple of 3, matching real vertex data (xyz
        // triples) and `dequantise`'s three-equal-channels assumption —
        // see pack()'s doc comment for why 4 (the old count here) is
        // not a case this format is meant to round-trip exactly
        let a = vec![-1.0f32, 0.0, 0.5, 1.0, -0.25, 0.75];
        let b = vec![0.1f32, 0.2, 0.3, 0.4, 0.05, 0.15];
        let blob = pack(-1.0, 1.0, &[a.clone(), b]);
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
        // Three values per pose (a multiple of 3 — see pack()'s doc
        // comment); `shared` stays at index 0 either way.
        let shared = 0.25f32;
        let blob = pack(-1.3, 1.08, &[vec![shared, -1.0, 0.2], vec![shared, 1.08, -0.4]]);
        let p0 = as_floats(&decode_pose(&blob, 0).unwrap())[0];
        let p1 = as_floats(&decode_pose(&blob, 1).unwrap())[0];
        assert_eq!(p0, p1, "a stationary vertex moved between poses");
    }

    #[test]
    fn every_pose_is_reachable() {
        // three-value poses per pack()'s doc comment (n=1 previously,
        // which is not a multiple of 3 but happened not to matter here
        // since this test only checks is_some()/is_none, not values —
        // changed anyway so every test in this module agrees on what a
        // valid pose looks like, rather than each hand-waving a
        // different edge case)
        let blob = pack(0.0, 1.0, &[vec![0.0, 0.1, 0.2], vec![0.5, 0.5, 0.5], vec![1.0, 0.9, 0.8]]);
        for i in 0..3 {
            assert!(decode_pose(&blob, i).is_some(), "pose {i}");
        }
        assert!(decode_pose(&blob, 3).is_none(), "read past the end");
    }

    #[test]
    fn the_output_length_matches_the_original_exactly() {
        // The loader hands these bytes to gltf, which indexes by byte offset —
        // a buffer one float short is a mesh that reads into the next
        // accessor's data. 36 rather than 37: a multiple of 3 (see
        // pack()'s doc comment) — length is checked here, not value
        // correctness, but keeping every fixture in this module
        // consistent is worth more than one fewer float.
        let a = vec![0.0f32; 36];
        let blob = pack(0.0, 1.0, &[a.clone(), a.clone()]);
        assert_eq!(decode_pose(&blob, 0).unwrap().len(), 36 * 4);
    }

    #[test]
    fn a_wrong_magic_is_refused() {
        let mut blob = pack(0.0, 1.0, &[vec![0.0, 0.0, 0.0], vec![1.0, 1.0, 1.0]]);
        blob[0] = b'X';
        assert!(decode_pose(&blob, 0).is_none());
    }

    #[test]
    fn a_future_version_is_refused_rather_than_guessed() {
        // Read as the current version, a new layout produces plausible-looking
        // garbage geometry rather than an error — the worst outcome available.
        // The same guard is what makes a leftover v1 (or v2) pack an error too.
        // The probe has to go one past the NEWEST version the decoder
        // accepts to actually simulate a future layout. That was 4 when v3
        // was current; v4 (solid zstd) is now a real version, so the probe
        // moves to 5. (Relabelling a v3 body as 4 is separately refused —
        // its "frame length" reads from mid-chunk garbage — but that is
        // luck of the layout, not the guard this test pins.)
        let mut blob = pack(0.0, 1.0, &[vec![0.0, 0.0, 0.0], vec![1.0, 1.0, 1.0]]);
        blob[4] = 5;
        assert!(decode_pose(&blob, 0).is_none());
    }

    #[test]
    fn a_v4_pack_decodes_byte_identically_to_its_v3_twin() {
        // THE TRANSCODE PROPERTY. v4 carries the same raw streams as v3 under
        // different compression, so decoding must produce the same bytes —
        // not merely close floats. This is what lets tools/pack_v4.py rewrite
        // 296 packs and call the result lossless by construction.
        let poses = vec![
            vec![-1.0f32, 0.0, 0.5, 1.0, -0.25, 0.75],
            vec![0.1f32, 0.2, 0.3, 0.4, 0.05, 0.15],
            vec![-0.5f32, 0.9, -0.9, 0.33, 0.0, -0.33],
        ];
        let v3 = pack(-1.0, 1.0, &poses);
        let v4 = pack_v4(-1.0, 1.0, &poses);
        for i in 0..poses.len() {
            assert_eq!(decode_pose(&v3, i), decode_pose(&v4, i), "pose {i}");
            assert!(decode_pose(&v4, i).is_some(), "pose {i} decoded");
        }
        assert!(decode_pose(&v4, poses.len()).is_none(), "read past the end");
    }

    #[test]
    fn a_truncated_v4_frame_is_none_rather_than_a_panic() {
        // Same reason as the v3 truncation test: half-written packs happen.
        // Every cut point must fail closed — the loader falls back to
        // scene.bin on None, but garbage geometry has no fallback.
        let blob = pack_v4(0.0, 1.0, &[vec![0.0, 1.0, 0.5], vec![0.5, 0.5, 0.5]]);
        for cut in [4, 10, 20, blob.len() / 2, blob.len() - 3] {
            assert!(decode_pose(&blob[..cut], 1).is_none(), "cut at {cut}");
        }
    }

    #[test]
    fn a_truncated_blob_returns_none_rather_than_panicking() {
        // Half-written packs happen: a full disk during packing is exactly how
        // this session has been going.
        //
        // Decodes the LAST pose, not the first: decode_pose returns as
        // soon as it reaches the requested pose, so trimming bytes off
        // the very end of the blob only matters if the target pose's
        // own data is what got trimmed. Decoding pose 0 from a blob
        // truncated near the end was passing for the wrong reason —
        // pose 0's bytes were all present regardless, only pose 1's
        // (never read) were missing.
        let blob = pack(0.0, 1.0, &[vec![0.0, 1.0, 0.5], vec![0.5, 0.5, 0.5]]);
        let last = 1;
        for cut in [4, 10, 20, blob.len() - 3] {
            assert!(decode_pose(&blob[..cut], last).is_none(), "cut at {cut}");
        }
    }

    #[test]
    fn a_degenerate_span_does_not_produce_nan() {
        // Every vertex at one coordinate. The packer writes span 1.0; matching
        // that here keeps both ends of the format agreeing.
        let blob = pack(0.5, 0.5, &[vec![0.5, 0.5, 0.5], vec![0.5, 0.5, 0.5]]);
        let got = as_floats(&decode_pose(&blob, 0).unwrap());
        assert!(got.iter().all(|v| v.is_finite()), "{got:?}");
    }

    #[test]
    fn the_pose_index_comes_from_the_directory_name() {
        // The packer sorts pose directories by name, so f03 must decode chunk
        // 3 — an off-by-one here returns the WRONG POSE, which renders fine and
        // folds wrong.
        assert_eq!(pose_index(Path::new("/x/quickfire/f03")), Some((Strip::Fold, 3)));
        assert_eq!(pose_index(Path::new("/x/quickfire/f00")), Some((Strip::Fold, 0)));
        assert_eq!(pose_index(Path::new("/x/quickfire/notapose")), None);
    }

    #[test]
    fn a_walk_pose_resolves_to_the_walk_strip() {
        // The bug this fixes: pose_index accepted only 'f' and pose_buffer
        // hardcoded "bots.pack", so w00 failed both and the loader panicked
        // with "no scene.bin and no bots.pack for .../analog/w00".
        assert_eq!(pose_index(Path::new("/x/analog/w00")), Some((Strip::Walk, 0)));
        assert_eq!(pose_index(Path::new("/x/analog/w03")), Some((Strip::Walk, 3)));
    }

    #[test]
    fn an_unknown_prefix_is_none_rather_than_a_guess() {
        // None lets the caller fall back to scene.bin. Guessing a strip would
        // index into whichever pack it picked and return real geometry from
        // the wrong animation.
        assert_eq!(pose_index(Path::new("/x/analog/q00")), None);
        assert_eq!(pose_index(Path::new("/x/analog/f")), None);
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

