//! `sim::net::chunk_stream` — world-streaming chunk record selection + framing
//! (WID r332-coordinator).
//!
//! Roadmap **item 6** of `docs/sections/SECTION_multiplayer.md` (§B); grounds
//! `docs/COMPRESSION_DECISION.md`. When a client joins, the host streams the world
//! a chunk at a time over the **`ChannelId::Bulk`** channel (`sim::net_channels`,
//! reliable-ordered, throttled below snapshots). Per the validated decision, per chunk:
//!   - **generated & unmodified → the seed** (coord + world seed, ~16 B): the client
//!     regenerates it bit-exactly via the deterministic `WorldGenerator`.
//!   - **modified, lightly → an lzma'd delta** vs the regenerated base (sparse
//!     `(index,value)` overrides).
//!   - **modified, heavily → lzma'd dense voxels** (the whole 32 KB array).
//! The **octree never crosses the wire**; the client rebuilds it locally.
//!
//! This module is the **pure record/selection/delta logic**: it decides seed-vs-delta-
//! vs-dense, builds/parses the wire record framing, and builds/applies the byte delta.
//! The **lzma compress/decompress call and the socket are bin glue** — the record
//! carries *opaque* payload bytes the bin (de)compresses; this module never links lzma.
//!
//! Pure: fixed-layout little-endian wire (mirrors `sim::net::wire`), no IO/clock/RNG,
//! `decode_record` never panics / returns `None` on bad input, lengths capped so a
//! hostile size can't blow allocation, saturating arithmetic.

/// Datagram family ("UN"), same as the rest of `sim::net`.
pub const CHUNK_STREAM_MAGIC: u16 = 0x554E;
pub const CHUNK_STREAM_VERSION: u8 = 1;

const KIND_SEED: u8 = 0;
const KIND_DENSE: u8 = 1;
const KIND_DELTA: u8 = 2;

/// Cells in a chunk (32³). Dense voxel payloads are one byte per cell (32 KB),
/// per COMPRESSION_DECISION's measured "dense 32.0K".
pub const CHUNK_CELLS: usize = 32 * 32 * 32;

/// Above this many differing cells, a delta is no longer worthwhile vs. sending the
/// dense array — [`build_delta`] returns `None` and the selector falls back to Dense.
pub const MAX_DELTA_CELLS: usize = CHUNK_CELLS / 4;

/// Chunk coordinate (chunk-space, not voxel-space).
pub type Coord = [i32; 3];

/// What the host should actually compress + send for a chunk (the *decision*; the bin
/// performs the lzma + framing using the rest of this module).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendPlan {
    /// Pristine: send the seed; client regenerates. No compression.
    Seed,
    /// Lightly modified: send an lzma'd sparse delta vs the regenerated base.
    Delta,
    /// Heavily modified: send the lzma'd dense voxel array.
    Dense,
}

/// Decide how to send a chunk. `delta_threshold_frac` is the fraction of cells above
/// which a delta is not worth it (heavier → Dense). Pure; no allocation.
pub fn select_plan(
    modified: bool,
    edited_cells: u32,
    total_cells: u32,
    delta_threshold_frac: f32,
) -> SendPlan {
    if !modified || edited_cells == 0 {
        return SendPlan::Seed;
    }
    let frac = if total_cells == 0 {
        1.0
    } else {
        edited_cells as f32 / total_cells as f32
    };
    if frac <= delta_threshold_frac && (edited_cells as usize) <= MAX_DELTA_CELLS {
        SendPlan::Delta
    } else {
        SendPlan::Dense
    }
}

/// A decoded chunk-stream record. The payload bytes for `Dense`/`Delta` are **opaque**
/// (lzma'd by the bin); `decode_record` does not decompress them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChunkRecord {
    /// Regenerate from `(coord, world_seed)`.
    Seed { coord: Coord, world_seed: u64 },
    /// lzma'd dense voxel array (opaque bytes).
    Dense { coord: Coord, comp: Vec<u8> },
    /// lzma'd sparse delta vs the regenerated base (opaque bytes).
    Delta { coord: Coord, comp: Vec<u8> },
}

impl ChunkRecord {
    pub fn coord(&self) -> Coord {
        match self {
            ChunkRecord::Seed { coord, .. }
            | ChunkRecord::Dense { coord, .. }
            | ChunkRecord::Delta { coord, .. } => *coord,
        }
    }
}

// ── Sparse delta (pure, on raw decompressed bytes) ──────────────────────────

/// Build a sparse delta of `modified` vs `base` (both one byte per cell, equal length):
/// a list of `(u16 index, u8 value)` for every differing cell. Returns `None` if the
/// lengths differ, exceed a `u16` index space, or more than [`MAX_DELTA_CELLS`] differ
/// (caller should send Dense instead). The returned bytes are what the bin lzma's.
pub fn build_delta(base: &[u8], modified: &[u8]) -> Option<Vec<u8>> {
    if base.len() != modified.len() || base.len() > u16::MAX as usize + 1 {
        return None;
    }
    let mut diffs: Vec<(u16, u8)> = Vec::new();
    for (i, (b, m)) in base.iter().zip(modified.iter()).enumerate() {
        if b != m {
            if diffs.len() >= MAX_DELTA_CELLS {
                return None; // not worth a delta — send Dense
            }
            diffs.push((i as u16, *m));
        }
    }
    let mut out = Vec::with_capacity(2 + diffs.len() * 3);
    out.extend_from_slice(&(diffs.len() as u16).to_le_bytes());
    for (idx, val) in diffs {
        out.extend_from_slice(&idx.to_le_bytes());
        out.push(val);
    }
    Some(out)
}

/// Apply a sparse delta to a regenerated `base`, returning the reconstructed modified
/// chunk. `None` on malformed/truncated delta or an index outside `base`. Never panics.
pub fn apply_delta(base: &[u8], delta: &[u8]) -> Option<Vec<u8>> {
    let mut r = Reader::new(delta);
    let n = r.u16()? as usize;
    let mut out = base.to_vec();
    for _ in 0..n {
        let idx = r.u16()? as usize;
        let val = r.u8()?;
        let slot = out.get_mut(idx)?; // bounds-checked: bad index → None
        *slot = val;
    }
    Some(out)
}

// ── Record framing (mirrors sim::net::wire; decode never panics) ────────────

/// Encode a record. Seed records are tiny (no payload); Dense/Delta carry a u32-length-
/// prefixed opaque (already-lzma'd) blob.
pub fn encode_record(rec: &ChunkRecord) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&CHUNK_STREAM_MAGIC.to_le_bytes());
    b.push(CHUNK_STREAM_VERSION);
    match rec {
        ChunkRecord::Seed { coord, world_seed } => {
            b.push(KIND_SEED);
            write_coord(&mut b, *coord);
            b.extend_from_slice(&world_seed.to_le_bytes());
        }
        ChunkRecord::Dense { coord, comp } => {
            b.push(KIND_DENSE);
            write_coord(&mut b, *coord);
            b.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            b.extend_from_slice(comp);
        }
        ChunkRecord::Delta { coord, comp } => {
            b.push(KIND_DELTA);
            write_coord(&mut b, *coord);
            b.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            b.extend_from_slice(comp);
        }
    }
    b
}

/// Max opaque payload bytes a record may declare, so a hostile length can't blow up
/// allocation. Generous vs. a 32 KB dense chunk (lzma'd ≈ sub-KB), pre-compression cap.
pub const MAX_PAYLOAD_BYTES: u32 = 1 << 20; // 1 MiB

/// Decode a record. `None` on bad magic/version/kind, truncation, or an over-cap length.
pub fn decode_record(buf: &[u8]) -> Option<ChunkRecord> {
    let mut r = Reader::new(buf);
    if r.u16()? != CHUNK_STREAM_MAGIC {
        return None;
    }
    if r.u8()? != CHUNK_STREAM_VERSION {
        return None;
    }
    let kind = r.u8()?;
    let coord = read_coord(&mut r)?;
    match kind {
        KIND_SEED => {
            let world_seed = r.u64()?;
            Some(ChunkRecord::Seed { coord, world_seed })
        }
        KIND_DENSE | KIND_DELTA => {
            let len = r.u32()?;
            if len > MAX_PAYLOAD_BYTES {
                return None;
            }
            let comp = r.take(len as usize)?.to_vec();
            if kind == KIND_DENSE {
                Some(ChunkRecord::Dense { coord, comp })
            } else {
                Some(ChunkRecord::Delta { coord, comp })
            }
        }
        _ => None,
    }
}

fn write_coord(b: &mut Vec<u8>, c: Coord) {
    for v in c {
        b.extend_from_slice(&v.to_le_bytes());
    }
}
fn read_coord(r: &mut Reader) -> Option<Coord> {
    Some([r.i32()?, r.i32()?, r.i32()?])
}

// ── Bounded reader (mirrors wire::Reader; never panics) ─────────────────────

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Reader<'a> {
        Reader { b, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.b.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        let s = self.take(2)?;
        Some(u16::from_le_bytes([s[0], s[1]]))
    }
    fn i32(&mut self) -> Option<i32> {
        let s = self.take(4)?;
        Some(i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u32(&mut self) -> Option<u32> {
        let s = self.take(4)?;
        Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Some(u64::from_le_bytes(a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_plan_seed_delta_dense() {
        assert_eq!(select_plan(false, 0, 32768, 0.1), SendPlan::Seed);
        assert_eq!(select_plan(true, 0, 32768, 0.1), SendPlan::Seed, "no edits → seed");
        assert_eq!(select_plan(true, 100, 32768, 0.1), SendPlan::Delta, "light → delta");
        assert_eq!(select_plan(true, 20000, 32768, 0.1), SendPlan::Dense, "heavy → dense");
        // exactly at the fraction boundary stays delta
        assert_eq!(select_plan(true, 3276, 32768, 0.1), SendPlan::Delta);
    }

    #[test]
    fn delta_round_trips() {
        let base = vec![7u8; 64];
        let mut modified = base.clone();
        modified[3] = 9;
        modified[40] = 1;
        modified[63] = 255;
        let d = build_delta(&base, &modified).expect("delta builds");
        let recon = apply_delta(&base, &d).expect("delta applies");
        assert_eq!(recon, modified);
    }

    #[test]
    fn empty_delta_when_identical() {
        let base = vec![3u8; 32];
        let d = build_delta(&base, &base).expect("builds");
        assert_eq!(&d, &0u16.to_le_bytes(), "zero diffs");
        assert_eq!(apply_delta(&base, &d).unwrap(), base);
    }

    #[test]
    fn build_delta_rejects_mismatched_len_and_too_many_diffs() {
        assert!(build_delta(&[1, 2, 3], &[1, 2]).is_none(), "len mismatch");
        // all cells differ in a > MAX_DELTA_CELLS chunk → None (caller sends Dense)
        let base = vec![0u8; CHUNK_CELLS];
        let modified = vec![1u8; CHUNK_CELLS];
        assert!(build_delta(&base, &modified).is_none(), "too many diffs → Dense");
    }

    #[test]
    fn apply_delta_rejects_bad_input() {
        let base = vec![0u8; 16];
        // claims 1 entry but is truncated
        assert!(apply_delta(&base, &[1, 0]).is_none());
        // index out of range for base
        let mut bad = Vec::new();
        bad.extend_from_slice(&1u16.to_le_bytes());
        bad.extend_from_slice(&999u16.to_le_bytes());
        bad.push(5);
        assert!(apply_delta(&base, &bad).is_none(), "oob index → None");
    }

    #[test]
    fn record_round_trips_all_kinds() {
        let seed = ChunkRecord::Seed { coord: [-3, 4, 17], world_seed: 0xDEAD_BEEF_1234 };
        let dense = ChunkRecord::Dense { coord: [1, 2, 3], comp: vec![9, 8, 7, 6] };
        let delta = ChunkRecord::Delta { coord: [0, -1, 0], comp: vec![1, 2] };
        for rec in [seed, dense, delta] {
            let buf = encode_record(&rec);
            assert_eq!(decode_record(&buf).expect("decodes"), rec);
        }
    }

    #[test]
    fn seed_record_is_tiny() {
        let buf = encode_record(&ChunkRecord::Seed { coord: [0, 0, 0], world_seed: 1 });
        // magic(2)+version(1)+kind(1)+coord(12)+seed(8) = 24 bytes, no payload
        assert_eq!(buf.len(), 24);
    }

    #[test]
    fn decode_rejects_bad_input() {
        let good = encode_record(&ChunkRecord::Dense { coord: [1, 1, 1], comp: vec![1, 2, 3] });
        let mut bad = good.clone();
        bad[0] ^= 0xFF; // magic
        assert!(decode_record(&bad).is_none());
        let mut bad = good.clone();
        bad[2] = 99; // version
        assert!(decode_record(&bad).is_none());
        assert!(decode_record(&good[..good.len() - 1]).is_none(), "truncated payload");
        assert!(decode_record(&[]).is_none());
    }

    #[test]
    fn decode_refuses_hostile_payload_length() {
        let mut b = Vec::new();
        b.extend_from_slice(&CHUNK_STREAM_MAGIC.to_le_bytes());
        b.push(CHUNK_STREAM_VERSION);
        b.push(KIND_DENSE);
        write_coord(&mut b, [0, 0, 0]);
        b.extend_from_slice(&(MAX_PAYLOAD_BYTES + 1).to_le_bytes());
        assert!(decode_record(&b).is_none(), "over-cap length refused, no giant alloc");
    }

    #[test]
    fn coord_accessor_and_determinism() {
        let rec = ChunkRecord::Delta { coord: [5, 6, 7], comp: vec![1] };
        assert_eq!(rec.coord(), [5, 6, 7]);
        assert_eq!(encode_record(&rec), encode_record(&rec));
    }
}
