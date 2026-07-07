//! Sparse surface representation of a chunk (v0.24.5 — GPU-mesher path).
//!
//! Two representations coexist:
//!
//! **`SurfaceRecord` (v0.24.5):** one record per emitting (block, face).
//! Compact at 16 bytes; ~1500-3000 records per typical surface chunk;
//! consumed by the v1/v2 GPU kernel that emits one quad per face bit.
//!
//! **`SurfaceRun` (v0.24.9):** one record per contiguous run of blocks
//! along a face's U-axis. Same 16 bytes. ~3-5× compression vs records
//! for typical chunks; consumed by the v3 GPU kernel that emits one
//! quad per run, getting partial greedy meshing benefits without
//! requiring sequential greedy on GPU.
//!
//! Both formats use **world coordinates** (not chunk-local) so they
//! survive the future octree migration. Octree leaves carry world
//! coords; the extractor's implementation changes (octree walk vs
//! dense scan) but the output type is stable.
//!
//! # What this is NOT
//!
//! - Not a replacement for `Chunk`. The dense grid stays as CPU-side
//!   authoritative storage until Layer C migrates it.
//! - Not full 2D greedy. SurfaceRun only merges along one axis (U)
//!   per face direction; full greedy merges along both U and V. v3
//!   gets us part of greedy's wins (~3-5× compression) without
//!   parallelization-hostile 2D merging on GPU.
//! - Not a wire format for save/load. Transient render-path structure.

use crate::constants::CHUNK_SIZE;
use crate::world::blocks::BlockId;
use crate::world::chunk::Chunk;

/// Face direction bits. Matches the GLSL kernel's expected bit positions.
pub mod face {
    pub const PX: u8 = 1 << 0; // +X
    pub const NX: u8 = 1 << 1; // -X
    pub const PY: u8 = 1 << 2; // +Y
    pub const NY: u8 = 1 << 3; // -Y
    pub const PZ: u8 = 1 << 4; // +Z
    pub const NZ: u8 = 1 << 5; // -Z
}

/// One surface block's record. 16 bytes, std430-friendly.
///
/// `packed` encodes:
///   bits 0..8  : block_type (BlockId as u8)
///   bits 8..14 : face_mask  (6 bits, see `face` module)
///   bits 14..32: reserved (0 in v1; future: per-face material flags,
///                AO bits, light level, etc.)
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceRecord {
    pub world_x: i32,
    pub world_y: i32,
    pub world_z: i32,
    pub packed: u32,
}

impl SurfaceRecord {
    pub const SIZE: usize = std::mem::size_of::<Self>();

    pub fn new(world_x: i32, world_y: i32, world_z: i32, block: BlockId, face_mask: u8) -> Self {
        let packed = (block as u32) | ((face_mask as u32 & 0x3F) << 8);
        Self { world_x, world_y, world_z, packed }
    }

    pub fn block(self) -> BlockId {
        // SAFETY: we wrote a valid BlockId value into the low byte at
        // construction. If a future caller constructs SurfaceRecord by
        // other means and writes an out-of-range byte here, this will
        // hit the unreachable arm in BlockId's TryFrom (if/when added).
        // For now BlockId is `repr(u8)` and constructed from known
        // BlockId values only.
        let b = (self.packed & 0xFF) as u8;
        // Unsafe transmute mirrors what the production codebase does
        // elsewhere; safe-fallback would require BlockId::try_from(u8)
        // which isn't on every variant today.
        unsafe { std::mem::transmute::<u8, BlockId>(b) }
    }

    pub fn face_mask(self) -> u8 {
        ((self.packed >> 8) & 0x3F) as u8
    }
}

/// Statically assert the layout. The GPU shader expects 16 bytes; if
/// anyone changes the struct, the kernel binding breaks.
const _: () = assert!(SurfaceRecord::SIZE == 16);

/// Extract the surface (exposed-face blocks) from a chunk into a
/// `Vec<SurfaceRecord>`. CPU implementation v1 — scans the dense block
/// grid once.
///
/// Future octree implementation will replace this with a tree walk;
/// downstream code (the GPU mesher) consumes the same `Vec` regardless.
///
/// Cost: O(CHUNK_VOLUME × 6) reads, allocates a vec of size = number of
/// surface blocks (typically 5–15% of CHUNK_VOLUME for terrain). Comparable
/// to the CPU mesher's first phase.
pub fn extract_surface(chunk: &Chunk) -> Vec<SurfaceRecord> {
    let (origin_x, origin_y, origin_z) = chunk.coord.block_origin();
    let mut out = Vec::with_capacity(2048);

    for y in 0..CHUNK_SIZE {
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let here = chunk.get(x, y, z);
                if !is_meshable(here) {
                    continue;
                }
                let mut mask: u8 = 0;
                // Inline neighbor lookups using produces_face_against,
                // matching the CPU mesher's predicate exactly.
                if here.produces_face_against(get_local(chunk, x + 1, y, z)) { mask |= face::PX; }
                if here.produces_face_against(get_local(chunk, x - 1, y, z)) { mask |= face::NX; }
                if here.produces_face_against(get_local(chunk, x, y + 1, z)) { mask |= face::PY; }
                if here.produces_face_against(get_local(chunk, x, y - 1, z)) { mask |= face::NY; }
                if here.produces_face_against(get_local(chunk, x, y, z + 1)) { mask |= face::PZ; }
                if here.produces_face_against(get_local(chunk, x, y, z - 1)) { mask |= face::NZ; }
                if mask == 0 {
                    continue; // fully buried, no surface contribution
                }
                out.push(SurfaceRecord::new(
                    origin_x + x,
                    origin_y + y,
                    origin_z + z,
                    here,
                    mask,
                ));
            }
        }
    }
    out
}

fn is_meshable(b: BlockId) -> bool {
    // Mirrors `render::mesher::is_meshable` so CPU and GPU paths agree
    // on which blocks contribute to the surface set.
    !b.is_air() && b.is_solid() && !b.is_subvoxel_only()
}

fn get_local(chunk: &Chunk, x: i32, y: i32, z: i32) -> BlockId {
    if x < 0 || x >= CHUNK_SIZE
        || y < 0 || y >= CHUNK_SIZE
        || z < 0 || z >= CHUNK_SIZE
    {
        // Out-of-bounds: treat as air for face-emission. Same convention
        // as the CPU mesher's `get_local`. Boundary faces between adjacent
        // chunks are emitted by both sides (accepted redundancy until
        // cross-chunk seam culling).
        return BlockId::Air;
    }
    chunk.get(x, y, z)
}

// ═════════════════════════════════════════════════════════════════════
// SurfaceRun — 1D-merged surface representation (v0.24.9)
// ═════════════════════════════════════════════════════════════════════

/// Face direction identifier. 0-5 covers all six axis-aligned faces.
/// Distinguished from `face::*` bit masks: a Run has exactly ONE face
/// direction, not a mask of possibly several.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceDir {
    PosX = 0,
    NegX = 1,
    PosY = 2,
    NegY = 3,
    PosZ = 4,
    NegZ = 5,
}

impl FaceDir {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => FaceDir::PosX, 1 => FaceDir::NegX,
            2 => FaceDir::PosY, 3 => FaceDir::NegY,
            4 => FaceDir::PosZ, 5 => FaceDir::NegZ,
            _ => panic!("invalid face dir {}", v),
        }
    }

    /// Run extends along this axis (0=X, 1=Y, 2=Z). The other in-plane
    /// axis is the V axis (not relevant for run-merging; only U is).
    fn run_axis(self) -> usize {
        match self {
            // For +X/-X faces (slice on X, free axes Y and Z), run along Y.
            FaceDir::PosX | FaceDir::NegX => 1,
            // For +Y/-Y faces (slice on Y, free axes X and Z), run along X.
            FaceDir::PosY | FaceDir::NegY => 0,
            // For +Z/-Z faces (slice on Z, free axes X and Y), run along X.
            FaceDir::PosZ | FaceDir::NegZ => 0,
        }
    }

    /// Slice axis (the axis the face is perpendicular to).
    fn slice_axis(self) -> usize {
        match self {
            FaceDir::PosX | FaceDir::NegX => 0,
            FaceDir::PosY | FaceDir::NegY => 1,
            FaceDir::PosZ | FaceDir::NegZ => 2,
        }
    }

    /// The other in-plane axis.
    fn v_axis(self) -> usize {
        match self {
            FaceDir::PosX | FaceDir::NegX => 2, // Z
            FaceDir::PosY | FaceDir::NegY => 2, // Z
            FaceDir::PosZ | FaceDir::NegZ => 1, // Y
        }
    }

    /// Direction toward neighbor across this face: +1 or -1 along the slice axis.
    fn neighbor_offset(self) -> i32 {
        match self {
            FaceDir::PosX | FaceDir::PosY | FaceDir::PosZ => 1,
            FaceDir::NegX | FaceDir::NegY | FaceDir::NegZ => -1,
        }
    }
}

/// A contiguous run of blocks of the same type all emitting the same
/// face direction. The run starts at `(world_x, world_y, world_z)` and
/// extends `length` blocks along this face's run axis.
///
/// 16 bytes, std430-friendly:
///   packed bits 0..8   = block_type
///   packed bits 8..11  = face dir (0-5)
///   packed bits 11..27 = length (1..65535)
///   packed bits 27..32 = reserved
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceRun {
    pub world_x: i32,
    pub world_y: i32,
    pub world_z: i32,
    pub packed: u32,
}

const _: () = assert!(std::mem::size_of::<SurfaceRun>() == 16);

impl SurfaceRun {
    pub const SIZE: usize = std::mem::size_of::<Self>();

    pub fn new(world_x: i32, world_y: i32, world_z: i32,
               block: BlockId, face: FaceDir, length: u16) -> Self
    {
        assert!(length >= 1, "run length must be >= 1");
        let packed = (block as u32)
            | ((face as u32 & 0x7) << 8)
            | ((length as u32 & 0xFFFF) << 11);
        Self { world_x, world_y, world_z, packed }
    }

    pub fn block(self) -> BlockId {
        let b = (self.packed & 0xFF) as u8;
        unsafe { std::mem::transmute::<u8, BlockId>(b) }
    }

    pub fn face(self) -> FaceDir {
        FaceDir::from_u8(((self.packed >> 8) & 0x7) as u8)
    }

    pub fn length(self) -> u16 {
        ((self.packed >> 11) & 0xFFFF) as u16
    }
}

/// Extract surface runs from a chunk. Single-pass implementation (v0.24.13).
///
/// Walks the chunk in `(y, z, x)` order ONCE. Per cell, checks all 6 face
/// directions. For each emitting face, either starts a new run, extends an
/// in-progress run, or closes a run and emits.
///
/// **Run-merge predicate**: two consecutive blocks along U form one run if:
///   - Both emit the given face direction (per `produces_face_against`),
///   - Both have the same `block_type`.
///
/// Per face direction the U axis differs:
///   - +X/-X: U-axis = Y. The inner x-loop doesn't advance U; runs span
///     multiple iterations of the outer y-loop. Per-(slice=x, V=z) open-run
///     state must be remembered across y. `open_x_pos` and `open_x_neg`
///     are `[CHUNK_SIZE; CHUNK_SIZE]` arrays indexed by (slice, V).
///   - +Y/-Y/+Z/-Z: U-axis = X. The inner x-loop advances U. Runs reset
///     at x=0 of every (y, z); a single scalar `open` suffices.
///
/// On AGX this runs ~3× faster than the original 6-pass version because
/// each chunk cell is read once instead of six times.
///
/// Output count is typically 3-5× smaller than `extract_surface` for
/// natural terrain. Same output as the 6-pass version (verified by tests
/// in `tests::runs_v2_matches_six_pass`).
pub fn extract_surface_runs(chunk: &Chunk) -> Vec<SurfaceRun> {
    let (origin_x, origin_y, origin_z) = chunk.coord.block_origin();
    let mut out = Vec::with_capacity(2048);

    // Open-run state for U=Y faces (+X, -X). Indexed by [slice=x][V=z].
    // Each entry: (is_open, block_type, run_start_y).
    // `u8::MAX` in block_type means "no open run." When a run closes we
    // emit and reset.
    #[derive(Copy, Clone)]
    struct OpenY { block: BlockId, start_y: i32 }
    let mut open_xp: [[Option<OpenY>; CHUNK_SIZE as usize]; CHUNK_SIZE as usize] =
        [[None; CHUNK_SIZE as usize]; CHUNK_SIZE as usize];
    let mut open_xn: [[Option<OpenY>; CHUNK_SIZE as usize]; CHUNK_SIZE as usize] =
        [[None; CHUNK_SIZE as usize]; CHUNK_SIZE as usize];

    // Open-run state for U=X faces (+Y, -Y, +Z, -Z). These reset at the
    // start of every (y, z) row because the inner x-loop walks U directly.
    // Tracked as scalars during the inner loop.

    for y in 0..CHUNK_SIZE {
        for z in 0..CHUNK_SIZE {
            // Reset U=X open runs at start of each row.
            let mut open_yp: Option<(BlockId, i32)> = None; // (block, start_x)
            let mut open_yn: Option<(BlockId, i32)> = None;
            let mut open_zp: Option<(BlockId, i32)> = None;
            let mut open_zn: Option<(BlockId, i32)> = None;

            for x in 0..CHUNK_SIZE {
                let here = chunk.get(x, y, z);
                let meshable = is_meshable(here);

                // Per-face emission check. For each face, compute the
                // neighbor's BlockId and ask whether `here` emits across.
                let emits_xp = meshable && here.produces_face_against(get_local(chunk, x + 1, y, z));
                let emits_xn = meshable && here.produces_face_against(get_local(chunk, x - 1, y, z));
                let emits_yp = meshable && here.produces_face_against(get_local(chunk, x, y + 1, z));
                let emits_yn = meshable && here.produces_face_against(get_local(chunk, x, y - 1, z));
                let emits_zp = meshable && here.produces_face_against(get_local(chunk, x, y, z + 1));
                let emits_zn = meshable && here.produces_face_against(get_local(chunk, x, y, z - 1));

                // ─── U=X faces: +Y, -Y, +Z, -Z ─────────────────────────
                // These coalesce along x within this (y,z) row.

                macro_rules! handle_ux {
                    ($emits:expr, $open:ident, $face:expr) => {
                        if $emits {
                            match $open {
                                Some((b, _)) if b == here => { /* extend in place */ }
                                Some((b, sx)) => {
                                    let length = (x - sx) as u16;
                                    out.push(SurfaceRun::new(
                                        origin_x + sx, origin_y + y, origin_z + z,
                                        b, $face, length,
                                    ));
                                    $open = Some((here, x));
                                }
                                None => { $open = Some((here, x)); }
                            }
                        } else if let Some((b, sx)) = $open {
                            let length = (x - sx) as u16;
                            out.push(SurfaceRun::new(
                                origin_x + sx, origin_y + y, origin_z + z,
                                b, $face, length,
                            ));
                            $open = None;
                        }
                    };
                }
                handle_ux!(emits_yp, open_yp, FaceDir::PosY);
                handle_ux!(emits_yn, open_yn, FaceDir::NegY);
                handle_ux!(emits_zp, open_zp, FaceDir::PosZ);
                handle_ux!(emits_zn, open_zn, FaceDir::NegZ);

                // ─── U=Y faces: +X, -X ─────────────────────────────────
                // These coalesce along y. State stored per (slice=x, V=z).

                macro_rules! handle_uy {
                    ($emits:expr, $open_arr:ident, $face:expr) => {
                        let slot = &mut $open_arr[x as usize][z as usize];
                        if $emits {
                            match *slot {
                                Some(OpenY { block, .. }) if block == here => {
                                    /* extend */
                                }
                                Some(OpenY { block, start_y }) => {
                                    let length = (y - start_y) as u16;
                                    out.push(SurfaceRun::new(
                                        origin_x + x, origin_y + start_y, origin_z + z,
                                        block, $face, length,
                                    ));
                                    *slot = Some(OpenY { block: here, start_y: y });
                                }
                                None => {
                                    *slot = Some(OpenY { block: here, start_y: y });
                                }
                            }
                        } else if let Some(OpenY { block, start_y }) = *slot {
                            let length = (y - start_y) as u16;
                            out.push(SurfaceRun::new(
                                origin_x + x, origin_y + start_y, origin_z + z,
                                block, $face, length,
                            ));
                            *slot = None;
                        }
                    };
                }
                handle_uy!(emits_xp, open_xp, FaceDir::PosX);
                handle_uy!(emits_xn, open_xn, FaceDir::NegX);
            }

            // End of x-row: close all U=X open runs.
            macro_rules! close_ux {
                ($open:ident, $face:expr) => {
                    if let Some((b, sx)) = $open {
                        let length = (CHUNK_SIZE - sx) as u16;
                        out.push(SurfaceRun::new(
                            origin_x + sx, origin_y + y, origin_z + z,
                            b, $face, length,
                        ));
                    }
                };
            }
            close_ux!(open_yp, FaceDir::PosY);
            close_ux!(open_yn, FaceDir::NegY);
            close_ux!(open_zp, FaceDir::PosZ);
            close_ux!(open_zn, FaceDir::NegZ);
        }
    }

    // End of y-axis: close all U=Y open runs in both +X and -X grids.
    for slice in 0..CHUNK_SIZE as usize {
        for v in 0..CHUNK_SIZE as usize {
            if let Some(OpenY { block, start_y }) = open_xp[slice][v] {
                let length = (CHUNK_SIZE - start_y) as u16;
                out.push(SurfaceRun::new(
                    origin_x + slice as i32, origin_y + start_y, origin_z + v as i32,
                    block, FaceDir::PosX, length,
                ));
            }
            if let Some(OpenY { block, start_y }) = open_xn[slice][v] {
                let length = (CHUNK_SIZE - start_y) as u16;
                out.push(SurfaceRun::new(
                    origin_x + slice as i32, origin_y + start_y, origin_z + v as i32,
                    block, FaceDir::NegX, length,
                ));
            }
        }
    }

    out
}

/// Original 6-pass run extractor. Kept as a reference implementation for
/// the `runs_v2_matches_six_pass` test in v0.24.13 and for hardware-side
/// bench comparison. The single-pass version above is the production
/// path; this one is slower but is straightforwardly correct, useful as
/// a test oracle and as a side-by-side measurement on hardware.
#[doc(hidden)]
pub fn extract_surface_runs_six_pass(chunk: &Chunk) -> Vec<SurfaceRun> {
    let (origin_x, origin_y, origin_z) = chunk.coord.block_origin();
    let mut out = Vec::with_capacity(2048);

    for face_u8 in 0u8..6 {
        let face = FaceDir::from_u8(face_u8);
        let slice_ax = face.slice_axis();
        let run_ax = face.run_axis();
        let v_ax = face.v_axis();
        let nbr_off = face.neighbor_offset();

        for slice in 0..CHUNK_SIZE {
            for v in 0..CHUNK_SIZE {
                let mut u = 0i32;
                while u < CHUNK_SIZE {
                    let mut here_coord = [0i32; 3];
                    here_coord[slice_ax] = slice;
                    here_coord[run_ax] = u;
                    here_coord[v_ax] = v;
                    let here = get_local(chunk, here_coord[0], here_coord[1], here_coord[2]);

                    if !is_meshable(here) {
                        u += 1;
                        continue;
                    }
                    let mut nbr_coord = here_coord;
                    nbr_coord[slice_ax] += nbr_off;
                    let nbr = get_local(chunk, nbr_coord[0], nbr_coord[1], nbr_coord[2]);
                    if !here.produces_face_against(nbr) {
                        u += 1;
                        continue;
                    }

                    let run_start_u = u;
                    let run_block = here;
                    let mut run_end_u = u + 1;
                    while run_end_u < CHUNK_SIZE {
                        let mut next_here = here_coord;
                        next_here[run_ax] = run_end_u;
                        let nh = get_local(chunk, next_here[0], next_here[1], next_here[2]);
                        if nh != run_block { break; }
                        let mut next_nbr = next_here;
                        next_nbr[slice_ax] += nbr_off;
                        let nn = get_local(chunk, next_nbr[0], next_nbr[1], next_nbr[2]);
                        if !nh.produces_face_against(nn) { break; }
                        run_end_u += 1;
                    }

                    let mut start_coord = here_coord;
                    start_coord[run_ax] = run_start_u;
                    let length = (run_end_u - run_start_u) as u16;
                    out.push(SurfaceRun::new(
                        origin_x + start_coord[0],
                        origin_y + start_coord[1],
                        origin_z + start_coord[2],
                        run_block, face, length,
                    ));

                    u = run_end_u;
                }
            }
        }
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunk::ChunkCoord;

    fn make_test_chunk(coord: ChunkCoord) -> Chunk {
        Chunk::empty(coord)
    }

    #[test]
    fn empty_chunk_yields_no_surface() {
        let chunk = make_test_chunk(ChunkCoord::new(0, 0, 0));
        let surface = extract_surface(&chunk);
        assert!(surface.is_empty());
    }

    #[test]
    fn single_block_emits_six_faces() {
        let mut chunk = make_test_chunk(ChunkCoord::new(0, 0, 0));
        chunk.set(5, 5, 5, BlockId::Stone);
        let surface = extract_surface(&chunk);
        assert_eq!(surface.len(), 1, "one surface block");
        let rec = surface[0];
        assert_eq!(rec.block(), BlockId::Stone);
        assert_eq!(rec.face_mask(), 0x3F, "all 6 faces exposed to air");
        assert_eq!(rec.world_x, 5);
        assert_eq!(rec.world_y, 5);
        assert_eq!(rec.world_z, 5);
    }

    #[test]
    fn fully_buried_block_has_no_record() {
        let mut chunk = make_test_chunk(ChunkCoord::new(0, 0, 0));
        // 3×3×3 cube of stone; centre cell at (5,5,5) is buried.
        for dx in 4..=6 {
            for dy in 4..=6 {
                for dz in 4..=6 {
                    chunk.set(dx, dy, dz, BlockId::Stone);
                }
            }
        }
        let surface = extract_surface(&chunk);
        // 27 cells in cube, 1 buried, 26 on surface.
        assert_eq!(surface.len(), 26);
        // The centre should NOT be in the surface set.
        let centre_present = surface.iter().any(|r|
            r.world_x == 5 && r.world_y == 5 && r.world_z == 5);
        assert!(!centre_present, "centre cell (5,5,5) is buried and must be omitted");
    }

    #[test]
    fn world_coords_offset_by_chunk_coord() {
        let mut chunk = make_test_chunk(ChunkCoord::new(2, -1, 3));
        chunk.set(0, 0, 0, BlockId::Stone);
        let surface = extract_surface(&chunk);
        assert_eq!(surface.len(), 1);
        let rec = surface[0];
        // chunk_origin = (coord * CHUNK_SIZE) = (64, -32, 96)
        assert_eq!(rec.world_x, 64);
        assert_eq!(rec.world_y, -32);
        assert_eq!(rec.world_z, 96);
    }

    #[test]
    fn boundary_block_emits_outward_faces() {
        let mut chunk = make_test_chunk(ChunkCoord::new(0, 0, 0));
        // Place a stone at the +X boundary. Its +X face is against
        // out-of-bounds (treated as air) and should be emitted.
        chunk.set(31, 5, 5, BlockId::Stone);
        let surface = extract_surface(&chunk);
        assert_eq!(surface.len(), 1);
        let rec = surface[0];
        assert!(rec.face_mask() & face::PX != 0, "+X face emitted at chunk boundary");
    }

    #[test]
    fn record_size_is_16_bytes() {
        // Pins the wire format — if the struct changes the GLSL kernel
        // binding will break too.
        assert_eq!(std::mem::size_of::<SurfaceRecord>(), 16);
    }

    #[test]
    fn packed_roundtrip() {
        let rec = SurfaceRecord::new(10, -5, 7, BlockId::Dirt, face::PY | face::NZ);
        assert_eq!(rec.block(), BlockId::Dirt);
        assert_eq!(rec.face_mask(), face::PY | face::NZ);
        assert_eq!(rec.world_x, 10);
        assert_eq!(rec.world_y, -5);
        assert_eq!(rec.world_z, 7);
    }

    // ─── SurfaceRun tests ───────────────────────────────────────────

    #[test]
    fn run_size_is_16_bytes() {
        assert_eq!(std::mem::size_of::<SurfaceRun>(), 16);
    }

    #[test]
    fn run_packed_roundtrip() {
        let r = SurfaceRun::new(10, -5, 7, BlockId::Stone, FaceDir::PosY, 8);
        assert_eq!(r.block(), BlockId::Stone);
        assert_eq!(r.face(), FaceDir::PosY);
        assert_eq!(r.length(), 8);
        assert_eq!(r.world_x, 10);
        assert_eq!(r.world_y, -5);
        assert_eq!(r.world_z, 7);
    }

    #[test]
    fn run_max_length() {
        // Length fits in 16 bits → max 65535.
        let r = SurfaceRun::new(0, 0, 0, BlockId::Stone, FaceDir::PosX, 65535);
        assert_eq!(r.length(), 65535);
    }

    #[test]
    fn empty_chunk_yields_no_runs() {
        let chunk = make_test_chunk(ChunkCoord::new(0, 0, 0));
        let runs = extract_surface_runs(&chunk);
        assert!(runs.is_empty());
    }

    #[test]
    fn single_block_emits_six_runs_of_length_1() {
        let mut chunk = make_test_chunk(ChunkCoord::new(0, 0, 0));
        chunk.set(5, 5, 5, BlockId::Stone);
        let runs = extract_surface_runs(&chunk);
        // 6 faces × 1 block each = 6 runs of length 1.
        assert_eq!(runs.len(), 6, "one run per exposed face");
        for r in &runs {
            assert_eq!(r.block(), BlockId::Stone);
            assert_eq!(r.length(), 1);
            assert_eq!(r.world_x, 5);
            assert_eq!(r.world_y, 5);
            assert_eq!(r.world_z, 5);
        }
        // All 6 face directions present.
        let mut faces: Vec<FaceDir> = runs.iter().map(|r| r.face()).collect();
        faces.sort_by_key(|f| *f as u8);
        assert_eq!(faces, vec![
            FaceDir::PosX, FaceDir::NegX,
            FaceDir::PosY, FaceDir::NegY,
            FaceDir::PosZ, FaceDir::NegZ,
        ]);
    }

    #[test]
    fn horizontal_line_merges_along_run_axis() {
        // Lay 5 stones in a row along X at y=5, z=5.
        // For face direction +Y (top): run axis is X. The 5 blocks
        // all emit the +Y face (air above) and have the same type, so
        // they should merge into ONE run of length 5.
        let mut chunk = make_test_chunk(ChunkCoord::new(0, 0, 0));
        for x in 5..10 {
            chunk.set(x, 5, 5, BlockId::Stone);
        }
        let runs = extract_surface_runs(&chunk);

        // Count runs per face direction.
        let py_runs: Vec<&SurfaceRun> = runs.iter().filter(|r| r.face() == FaceDir::PosY).collect();
        // The +Y face should be one run of length 5 starting at x=5.
        assert_eq!(py_runs.len(), 1, "PosY should produce one run for the line");
        assert_eq!(py_runs[0].length(), 5);
        assert_eq!(py_runs[0].world_x, 5);

        // The +X face has run axis Y. Each block has its own column,
        // but the end of the line (x=9) has +X face exposed; the others
        // are blocked by their +X neighbor. So +X should have 1 run of
        // length 1.
        let px_runs: Vec<&SurfaceRun> = runs.iter().filter(|r| r.face() == FaceDir::PosX).collect();
        assert_eq!(px_runs.len(), 1);
        assert_eq!(px_runs[0].length(), 1);
        assert_eq!(px_runs[0].world_x, 9);
    }

    #[test]
    fn different_block_types_do_not_merge() {
        // Stone at x=5, Dirt at x=6, Stone at x=7. All at y=5, z=5.
        // For +Y face (run along X): expect 3 separate runs because
        // block types differ.
        let mut chunk = make_test_chunk(ChunkCoord::new(0, 0, 0));
        chunk.set(5, 5, 5, BlockId::Stone);
        chunk.set(6, 5, 5, BlockId::Dirt);
        chunk.set(7, 5, 5, BlockId::Stone);
        let runs = extract_surface_runs(&chunk);
        let py_runs: Vec<&SurfaceRun> = runs.iter().filter(|r| r.face() == FaceDir::PosY).collect();
        assert_eq!(py_runs.len(), 3, "different block types must not merge");
    }

    #[test]
    fn runs_compress_versus_records() {
        // 10×10 flat plate of stone at y=5. All have +Y exposed.
        // Records: 10×10 = 100 records (each emits +Y face mask).
        // Runs: 10 runs of length 10 (along X within each Z).
        // The plate ALSO emits other faces around its edges; those
        // won't compress as well, but +Y/-Y will.
        let mut chunk = make_test_chunk(ChunkCoord::new(0, 0, 0));
        for z in 5..15 {
            for x in 5..15 {
                chunk.set(x, 5, z, BlockId::Stone);
            }
        }
        let records = extract_surface(&chunk);
        let runs = extract_surface_runs(&chunk);
        // Runs should be meaningfully fewer than records.
        assert!(runs.len() < records.len(),
            "runs ({}) should compress vs records ({})", runs.len(), records.len());
        // +Y face: 10 runs of length 10. Sum of lengths = 100 blocks.
        let py_runs: Vec<&SurfaceRun> = runs.iter().filter(|r| r.face() == FaceDir::PosY).collect();
        assert_eq!(py_runs.len(), 10, "10 rows along Z, one run each");
        let total_len: u32 = py_runs.iter().map(|r| r.length() as u32).sum();
        assert_eq!(total_len, 100, "10 runs × 10 length = 100 blocks");
    }

    // ─── v0.24.13 single-pass extractor equivalence ────────────────────

    /// Builds a chunk with several different surface configurations so
    /// the single-pass and six-pass extractors have to agree across many
    /// edge cases.
    fn make_messy_chunk() -> Chunk {
        let mut chunk = make_test_chunk(ChunkCoord::new(2, -3, 7));
        // A solid 8×8×8 cube at (8..16, 4..12, 8..16)
        for y in 4..12 {
            for z in 8..16 {
                for x in 8..16 {
                    chunk.set(x, y, z, BlockId::Stone);
                }
            }
        }
        // A line of dirt poking out at (20..30, 6, 20)
        for x in 20..30 {
            chunk.set(x, 6, 20, BlockId::Dirt);
        }
        // Stone-dirt-stone alternation at (3..8, 10, 5)
        for x in 3..8 {
            let b = if (x - 3) % 2 == 0 { BlockId::Stone } else { BlockId::Dirt };
            chunk.set(x, 10, 5, b);
        }
        // Single block at edge of chunk (boundary test)
        chunk.set(31, 0, 31, BlockId::Stone);
        chunk.set(0, 31, 0, BlockId::Dirt);
        // Two adjacent blocks along Y at (5, 20..22, 5) — tests +X/-X runs
        // which extend along Y.
        chunk.set(5, 20, 5, BlockId::Stone);
        chunk.set(5, 21, 5, BlockId::Stone);
        chunk
    }

    /// Sort runs into a canonical order so two extractor implementations
    /// that produce the same logical set in different orderings compare
    /// equal.
    fn sort_runs(mut runs: Vec<SurfaceRun>) -> Vec<SurfaceRun> {
        runs.sort_by_key(|r| (r.face() as u8, r.world_x, r.world_y, r.world_z, r.length()));
        runs
    }

    #[test]
    fn runs_v2_matches_six_pass() {
        let chunk = make_messy_chunk();
        let a = sort_runs(extract_surface_runs(&chunk));
        let b = sort_runs(extract_surface_runs_six_pass(&chunk));
        assert_eq!(a.len(), b.len(),
            "single-pass produced {} runs, six-pass produced {}",
            a.len(), b.len());
        for (i, (ra, rb)) in a.iter().zip(b.iter()).enumerate() {
            assert_eq!(ra, rb,
                "run {} differs: single-pass={:?} six-pass={:?}",
                i, ra, rb);
        }
    }

    #[test]
    fn runs_v2_empty_chunk() {
        let chunk = make_test_chunk(ChunkCoord::new(0, 0, 0));
        let a = extract_surface_runs(&chunk);
        let b = extract_surface_runs_six_pass(&chunk);
        assert_eq!(a.len(), 0);
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn runs_v2_full_chunk() {
        // Fully solid chunk — all blocks emit only their boundary faces
        // (the chunk-edge ones).
        let mut chunk = make_test_chunk(ChunkCoord::new(0, 0, 0));
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    chunk.set(x, y, z, BlockId::Stone);
                }
            }
        }
        let a = sort_runs(extract_surface_runs(&chunk));
        let b = sort_runs(extract_surface_runs_six_pass(&chunk));
        assert_eq!(a, b);
        // Sanity: ALL 6 chunk-boundary faces emit. Each face is 32×32 =
        // 1024 cells worth of face area. With U-merging, each face is
        // 32 runs of length 32. So we expect 6 × 32 = 192 runs total.
        assert_eq!(a.len(), 192, "fully solid chunk: 6 faces × 32 runs per face");
    }

    #[test]
    fn runs_v2_single_block_at_31_31_31() {
        // Boundary test: a single block at the +X/+Y/+Z corner.
        let mut chunk = make_test_chunk(ChunkCoord::new(0, 0, 0));
        chunk.set(31, 31, 31, BlockId::Stone);
        let a = sort_runs(extract_surface_runs(&chunk));
        let b = sort_runs(extract_surface_runs_six_pass(&chunk));
        assert_eq!(a, b);
        assert_eq!(a.len(), 6, "single block: 6 faces × 1 run each");
    }
}

