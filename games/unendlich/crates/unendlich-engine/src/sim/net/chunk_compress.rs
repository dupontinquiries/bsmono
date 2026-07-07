//! `sim::net::chunk_compress` — the deferred payload codec for `chunk_stream`.
//!
//! [`chunk_stream`](crate::sim::net::chunk_stream) decides seed-vs-delta-vs-dense,
//! frames records, and builds byte deltas, but it carries the `comp` payload as
//! *opaque bytes* — "the bin lzma's them". This is that codec, dependency-free: a
//! voxel chunk is mostly long runs of one block (air above, stone below), so a
//! PackBits-style run-length scheme crushes a 32 KB dense chunk to a fraction
//! without linking lzma, and it never expands incompressible input by more than
//! ~1/128. It then ties the whole path together: [`seal_dense`]/[`seal_delta`] on
//! the host, [`open`] on the client, composing `select`/`build_delta`/`apply_delta`
//! with compression so a chunk goes voxels → record → bytes → voxels.
//!
//! Pure and panic-free. [`decompress`] is bounded by an explicit `max_out`, so a
//! hostile or corrupt payload can neither panic nor exhaust memory.

use crate::sim::net::chunk_stream::{apply_delta, build_delta, ChunkRecord, Coord};

/// PackBits run-length encode. Output layout, one token at a time:
/// * control `0..=127` → the next `control+1` bytes are literals;
/// * control `-1..=-127` (stored as `0xFF..=0x81`) → the next byte repeated
///   `1 - control` times (i.e. `2..=128`);
/// * `-128` is never emitted (reserved no-op on the decode side).
pub fn compress(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let n = raw.len();
    let mut i = 0;
    while i < n {
        // How long is the run of identical bytes starting at i (capped at 128)?
        let mut run = 1;
        while i + run < n && raw[i + run] == raw[i] && run < 128 {
            run += 1;
        }
        if run >= 2 {
            out.push((1i32 - run as i32) as i8 as u8); // -1..=-127
            out.push(raw[i]);
            i += run;
        } else {
            // Gather a literal stretch up to 128, stopping where a run of ≥2 begins.
            let start = i;
            let mut lit = 0;
            while i < n && lit < 128 {
                if i + 1 < n && raw[i] == raw[i + 1] {
                    break; // a run starts here — let the run branch take it
                }
                i += 1;
                lit += 1;
            }
            out.push((lit as i32 - 1) as i8 as u8); // 0..=127
            out.extend_from_slice(&raw[start..start + lit]);
        }
    }
    out
}

/// Inverse of [`compress`], refusing to produce more than `max_out` bytes (so a
/// crafted payload can't blow up allocation). `None` on truncation or overflow.
pub fn decompress(comp: &[u8], max_out: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < comp.len() {
        let ctrl = comp[i] as i8;
        i += 1;
        if ctrl >= 0 {
            let count = ctrl as usize + 1; // 1..=128 literals
            if i + count > comp.len() || out.len() + count > max_out {
                return None;
            }
            out.extend_from_slice(&comp[i..i + count]);
            i += count;
        } else if ctrl == -128 {
            // reserved no-op
        } else {
            let count = (1 - ctrl as i32) as usize; // 2..=128
            if i >= comp.len() || out.len() + count > max_out {
                return None;
            }
            let val = comp[i];
            i += 1;
            out.resize(out.len() + count, val);
        }
    }
    Some(out)
}

// ── End-to-end seal (host) / open (client) ──────────────────────────────────

/// Host: a heavily-modified chunk → a compressed `Dense` record.
pub fn seal_dense(coord: Coord, voxels: &[u8]) -> ChunkRecord {
    ChunkRecord::Dense { coord, comp: compress(voxels) }
}

/// Host: a lightly-modified chunk → a compressed `Delta` record vs `base` (the
/// seed-regenerated chunk). `None` if a delta isn't viable (caller sends Dense).
pub fn seal_delta(coord: Coord, base: &[u8], modified: &[u8]) -> Option<ChunkRecord> {
    let delta = build_delta(base, modified)?;
    Some(ChunkRecord::Delta { coord, comp: compress(&delta) })
}

/// Client: reconstruct a chunk's voxels from a decoded record.
/// * `Seed`  → the caller's deterministic regeneration (`regen_base`) is the answer;
/// * `Dense` → decompress the payload;
/// * `Delta` → decompress, then apply the sparse delta onto `regen_base`.
///
/// `regen_base` is the client's bit-exact regeneration of `(coord, world_seed)`;
/// it's used as the answer for `Seed`, the base for `Delta`, and ignored for
/// `Dense`. `max_cells` bounds decompression. `None` on any malformed payload.
pub fn open(rec: &ChunkRecord, regen_base: &[u8], max_cells: usize) -> Option<Vec<u8>> {
    match rec {
        ChunkRecord::Seed { .. } => Some(regen_base.to_vec()),
        ChunkRecord::Dense { comp, .. } => decompress(comp, max_cells),
        ChunkRecord::Delta { comp, .. } => {
            let delta = decompress(comp, max_cells)?;
            apply_delta(regen_base, &delta)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::net::chunk_stream::{decode_record, encode_record, select_plan, SendPlan, CHUNK_CELLS};

    fn voxel_chunk() -> Vec<u8> {
        // Realistic shape: a big air run, a band of stone, a sprinkle of variety.
        let mut v = vec![0u8; CHUNK_CELLS]; // 0 = air
        for c in v.iter_mut().take(CHUNK_CELLS / 2) {
            *c = 1; // stone half
        }
        v[100] = 7;
        v[8000] = 9;
        v[20000] = 3;
        v
    }

    #[test]
    fn roundtrips_and_crushes_a_voxel_chunk() {
        let v = voxel_chunk();
        let c = compress(&v);
        assert_eq!(decompress(&c, CHUNK_CELLS).as_deref(), Some(&v[..]));
        assert!(c.len() < v.len() / 10, "long runs compress hugely: {} vs {}", c.len(), v.len());
    }

    #[test]
    fn roundtrips_incompressible_without_much_bloat() {
        // Pseudo-random (no runs) — PackBits worst case is +~1/128.
        let mut v = vec![0u8; 5000];
        let mut x = 0x1234_5678u32;
        for b in v.iter_mut() {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            *b = (x >> 24) as u8;
        }
        let c = compress(&v);
        assert_eq!(decompress(&c, v.len()).as_deref(), Some(&v[..]));
        assert!(c.len() <= v.len() + v.len() / 64 + 8, "bounded expansion");
    }

    #[test]
    fn empty_and_single_and_full_run() {
        assert_eq!(compress(&[]), Vec::<u8>::new());
        assert_eq!(decompress(&[], 10).as_deref(), Some(&[][..]));
        assert_eq!(decompress(&compress(&[42]), 4).as_deref(), Some(&[42][..]));
        let run = vec![5u8; 1000];
        assert_eq!(decompress(&compress(&run), 1000).as_deref(), Some(&run[..]));
    }

    #[test]
    fn decompress_rejects_truncated_and_overlong() {
        // run token with no value byte
        assert!(decompress(&[0xFF], 10).is_none());
        // literal token claiming 3 bytes but only 1 present
        assert!(decompress(&[0x02, 0xAA], 10).is_none());
        // would exceed max_out
        let big = compress(&vec![1u8; 500]);
        assert!(decompress(&big, 100).is_none());
    }

    #[test]
    fn dense_path_voxels_to_record_to_voxels() {
        let v = voxel_chunk();
        let rec = seal_dense([1, 2, 3], &v);
        // It travels as a framed record:
        let wire = encode_record(&rec);
        let back = decode_record(&wire).expect("decodes");
        let got = open(&back, &[], CHUNK_CELLS).expect("opens");
        assert_eq!(got, v);
        assert_eq!(back.coord(), [1, 2, 3]);
    }

    #[test]
    fn delta_path_reconstructs_against_regen_base() {
        let base = voxel_chunk(); // what the client regenerates from the seed
        let mut modified = base.clone();
        modified[100] = 200;
        modified[101] = 201;
        let rec = seal_delta([4, 5, 6], &base, &modified).expect("delta viable");
        let wire = encode_record(&rec);
        let back = decode_record(&wire).expect("decodes");
        let got = open(&back, &base, CHUNK_CELLS).expect("opens");
        assert_eq!(got, modified, "delta on the regen base yields the modified chunk");
    }

    #[test]
    fn seed_path_returns_regeneration() {
        let regen = voxel_chunk();
        let rec = ChunkRecord::Seed { coord: [0, 0, 0], world_seed: 0x5EED };
        assert_eq!(open(&rec, &regen, CHUNK_CELLS).as_deref(), Some(&regen[..]));
    }

    #[test]
    fn select_plan_matches_the_seal_used() {
        // sanity: an unmodified chunk is Seed; a lightly edited one is Delta.
        assert_eq!(select_plan(false, 0, CHUNK_CELLS as u32, 0.25), SendPlan::Seed);
        assert_eq!(select_plan(true, 3, CHUNK_CELLS as u32, 0.25), SendPlan::Delta);
        assert_eq!(select_plan(true, CHUNK_CELLS as u32, CHUNK_CELLS as u32, 0.25), SendPlan::Dense);
    }
}
