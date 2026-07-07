//! Persistent storage for chunks.
//!
//! Storage philosophy (from design notes): chunks are *born procedural* and
//! only the explicit deltas need to be saved. Every block in a fresh chunk
//! came from the world generator and is regenerable from `(coord, seed,
//! generator_version)` — about 20 bytes of seed material. Player edits and
//! simulation changes are the only data that *can't* be regenerated, so
//! those are what we persist.
//!
//! File layout: one file per chunk. Region directories group nearby chunks
//! for filesystem friendliness (most filesystems get slow with thousands of
//! files in a single directory).
//!
//! ```
//! worlds/
//!   <world_name>/
//!     world.mkw                  # world metadata: seed, generator version, last play time
//!     region/
//!       <rx>_<ry>_<rz>/          # 16×16×16 chunks per region
//!         chunk_<cx>_<cy>_<cz>.mks
//! ```
//!
//! .mks file format (binary, little-endian):
//! ```text
//! [magic: 4 bytes  = "MKS1"]
//! [format version: u16 = 1]
//! [chunk coord: 3 × i32]
//! [generator version: u32]   # used to invalidate stale saves on worldgen changes
//! [edit count: u32]
//! [edits: edit_count × (u16 index, u8 block_id) tuples]
//! ```
//!
//! No compression, no checksum, no batching — these are intentional MVP
//! choices. When chunk save volume becomes a real concern, add zstd; when
//! corruption becomes a real concern, add a CRC32 in the header.
//!
//! Threading: this module is intentionally synchronous. The chunk_manager
//! is responsible for offloading saves to a background thread when ready
//! (current code is sync; see `save_chunk_sync` doc).

use crate::world::blocks::BlockId;
use crate::world::chunk::{Chunk, ChunkCoord};
use std::convert::TryFrom;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Bumped whenever the worldgen output changes in a way that invalidates
/// saved chunks. (Bumping this means: next time the player loads the world,
/// pristine chunks regenerate fresh, but explicit edits replay over the
/// new procedural baseline. Edits "stick" through worldgen changes.)
pub const GENERATOR_VERSION: u32 = 1;

const MAGIC: [u8; 4] = *b"MKS1";
const FORMAT_VERSION: u16 = 1;

/// 16 chunks per region edge (so 4096 chunks per region cube).
pub const REGION_SIZE: i32 = 16;

#[derive(Debug)]
pub enum StorageError {
    Io(std::io::Error),
    BadMagic([u8; 4]),
    UnsupportedVersion(u16),
    GeneratorVersionMismatch { saved: u32, current: u32 },
    CoordMismatch { expected: ChunkCoord, actual: ChunkCoord },
    UnknownBlockId(u8),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::Io(e) => write!(f, "I/O error: {}", e),
            StorageError::BadMagic(m) => write!(f, "bad magic bytes: {:?}", m),
            StorageError::UnsupportedVersion(v) => write!(f, "unsupported format version: {}", v),
            StorageError::GeneratorVersionMismatch { saved, current } =>
                write!(f, "save written by generator v{}, current is v{}", saved, current),
            StorageError::CoordMismatch { expected, actual } =>
                write!(f, "chunk coord mismatch: file has {:?}, expected {:?}", actual, expected),
            StorageError::UnknownBlockId(id) => write!(f, "unknown block id: {}", id),
        }
    }
}
impl std::error::Error for StorageError {}
impl From<std::io::Error> for StorageError { fn from(e: std::io::Error) -> Self { StorageError::Io(e) } }

/// Compute the region directory path for a chunk coord.
pub fn region_path(world_root: &Path, chunk: ChunkCoord) -> PathBuf {
    let rx = chunk.x.div_euclid(REGION_SIZE);
    let ry = chunk.y.div_euclid(REGION_SIZE);
    let rz = chunk.z.div_euclid(REGION_SIZE);
    world_root.join("region").join(format!("{}_{}_{}", rx, ry, rz))
}

/// Compute the chunk-file path for a chunk coord.
pub fn chunk_path(world_root: &Path, chunk: ChunkCoord) -> PathBuf {
    region_path(world_root, chunk).join(format!("chunk_{}_{}_{}.mks", chunk.x, chunk.y, chunk.z))
}

/// Save a chunk's explicit edits to disk, synchronously. Returns Ok(false)
/// if the chunk has no edits (no file written). Returns Ok(true) if a file
/// was written.
///
/// Synchronous by design — async save belongs to the chunk_manager once
/// it's ready to manage a save thread pool. For now, callers should only
/// invoke this on chunk eviction (rare) or world-shutdown (one-time).
pub fn save_chunk_sync(
    world_root: &Path,
    chunk: &Chunk,
) -> Result<bool, StorageError> {
    if chunk.explicit_indices.is_empty() {
        // Nothing to save — chunk is fully procedural.
        return Ok(false);
    }

    let region_dir = region_path(world_root, chunk.coord);
    std::fs::create_dir_all(&region_dir)?;
    let file_path = chunk_path(world_root, chunk.coord);

    let mut buf: Vec<u8> = Vec::with_capacity(64 + chunk.explicit_indices.len() * 3);
    buf.extend_from_slice(&MAGIC);
    buf.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    buf.extend_from_slice(&chunk.coord.x.to_le_bytes());
    buf.extend_from_slice(&chunk.coord.y.to_le_bytes());
    buf.extend_from_slice(&chunk.coord.z.to_le_bytes());
    buf.extend_from_slice(&GENERATOR_VERSION.to_le_bytes());
    buf.extend_from_slice(&(chunk.explicit_indices.len() as u32).to_le_bytes());
    for &idx in &chunk.explicit_indices {
        let block = chunk.blocks[idx as usize] as u8;
        buf.extend_from_slice(&idx.to_le_bytes());
        buf.push(block);
    }

    // Atomic write: write to temp then rename. Avoids leaving half-written
    // files if the process is killed mid-save. (Filesystem race protection
    // is left to callers; we serialize by per-chunk file.)
    let tmp_path = file_path.with_extension("mks.tmp");
    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(&buf)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp_path, &file_path)?;
    Ok(true)
}

/// Load explicit edits for a chunk and overlay them onto an already-generated
/// procedural chunk. Returns Ok(0) if there's no save file (pristine chunk).
/// Returns Ok(N) for N edits applied.
///
/// `chunk` must already contain the procedural base (from worldgen). This
/// function MUTATES it to apply persisted edits. The chunk's version is
/// bumped per edit.
pub fn load_chunk_edits(
    world_root: &Path,
    chunk: &mut Chunk,
) -> Result<usize, StorageError> {
    let file_path = chunk_path(world_root, chunk.coord);
    if !file_path.exists() {
        return Ok(0);
    }

    let mut buf = Vec::new();
    std::fs::File::open(&file_path)?.read_to_end(&mut buf)?;
    let mut cur = std::io::Cursor::new(&buf[..]);

    let mut magic = [0u8; 4];
    cur.read_exact(&mut magic)?;
    if magic != MAGIC {
        return Err(StorageError::BadMagic(magic));
    }
    let mut version = [0u8; 2];
    cur.read_exact(&mut version)?;
    let version = u16::from_le_bytes(version);
    if version != FORMAT_VERSION {
        return Err(StorageError::UnsupportedVersion(version));
    }
    let mut coord_buf = [0u8; 4];
    cur.read_exact(&mut coord_buf)?;
    let cx = i32::from_le_bytes(coord_buf);
    cur.read_exact(&mut coord_buf)?;
    let cy = i32::from_le_bytes(coord_buf);
    cur.read_exact(&mut coord_buf)?;
    let cz = i32::from_le_bytes(coord_buf);
    let saved_coord = ChunkCoord::new(cx, cy, cz);
    if saved_coord != chunk.coord {
        return Err(StorageError::CoordMismatch {
            expected: chunk.coord, actual: saved_coord,
        });
    }
    let mut gv_buf = [0u8; 4];
    cur.read_exact(&mut gv_buf)?;
    let saved_gv = u32::from_le_bytes(gv_buf);
    if saved_gv != GENERATOR_VERSION {
        // We don't error — the design is "edits stick across worldgen
        // changes". Log instead (callers can decide what to do with mismatch).
        log::warn!("Chunk {:?}: saved with generator v{}, current v{}; applying edits anyway",
            chunk.coord, saved_gv, GENERATOR_VERSION);
    }
    let mut count_buf = [0u8; 4];
    cur.read_exact(&mut count_buf)?;
    let edit_count = u32::from_le_bytes(count_buf);

    for _ in 0..edit_count {
        let mut idx_buf = [0u8; 2];
        cur.read_exact(&mut idx_buf)?;
        let idx = u16::from_le_bytes(idx_buf);
        let mut block_buf = [0u8; 1];
        cur.read_exact(&mut block_buf)?;
        let block = BlockId::try_from(block_buf[0])
            .map_err(StorageError::UnknownBlockId)?;
        // Apply directly to blocks + mark explicit. We bypass `set()` to
        // avoid bumping version per edit (we'll bump once at end).
        chunk.blocks[idx as usize] = block;
        chunk.explicit_indices.insert(idx);
    }
    if edit_count > 0 {
        chunk.version += 1;
        chunk.dirty = false; // saved state matches in-memory state
    }
    Ok(edit_count as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_world() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn pristine_chunk_writes_no_file() {
        let dir = temp_world();
        let chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let wrote = save_chunk_sync(dir.path(), &chunk).unwrap();
        assert!(!wrote);
        assert!(!chunk_path(dir.path(), chunk.coord).exists());
    }

    #[test]
    fn edited_chunk_round_trips() {
        let dir = temp_world();
        let mut chunk = Chunk::empty(ChunkCoord::new(2, -1, 5));
        chunk.set(3, 4, 5, BlockId::Stone);
        chunk.set(6, 7, 8, BlockId::Grass);
        save_chunk_sync(dir.path(), &chunk).unwrap();

        let mut loaded = Chunk::empty(chunk.coord);
        let n = load_chunk_edits(dir.path(), &mut loaded).unwrap();
        assert_eq!(n, 2);
        assert_eq!(loaded.get(3, 4, 5), BlockId::Stone);
        assert_eq!(loaded.get(6, 7, 8), BlockId::Grass);
        // Indices match.
        assert_eq!(loaded.explicit_count(), 2);
    }

    #[test]
    fn no_save_file_returns_zero() {
        let dir = temp_world();
        let mut chunk = Chunk::empty(ChunkCoord::new(99, 99, 99));
        let n = load_chunk_edits(dir.path(), &mut chunk).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn region_path_groups_chunks() {
        let root = Path::new("/tmp/world");
        let p1 = region_path(root, ChunkCoord::new(0, 0, 0));
        let p2 = region_path(root, ChunkCoord::new(15, 15, 15));
        let p3 = region_path(root, ChunkCoord::new(16, 0, 0));
        // chunks (0,0,0) and (15,15,15) share a region; (16,0,0) is in next.
        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
    }
}
