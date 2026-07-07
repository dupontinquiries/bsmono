//! GPU-resident octree buffer (v0.24.26-octree-on-gpu).
//!
//! Owns a single HOST_VISIBLE | HOST_COHERENT storage buffer that holds
//! every loaded chunk's `OctreeArray` nodes laid out as a flat
//! contiguous array. Per-chunk slices are sub-allocated via the same
//! [`SpanAllocator`] used by the mesh buffer pool: variable-size, no
//! fixed per-chunk caps, fragmentation handled by best-fit + free-list
//! coalescing.
//!
//! ## Why this exists
//!
//! `architecture-v1.md` §3.1 calls for the world to live on the GPU as
//! an octree, read by all consumers (mesher, renderer materials,
//! physics, raycast, item region-reads). v0.24.23 landed the CPU-side
//! `OctreeArray` as the canonical representation; the production
//! `ChunkManager` carries an `Arc<OctreeArray>` per chunk in a
//! `DashMap` parallel to the dense `Chunk`. v0.24.24 added the
//! `OctreeBlockSampler` adapter so existing physics paths could opt
//! into the octree without disturbing the dense path.
//!
//! Up through v0.24.25 the octree never touched the GPU. Compute
//! shaders that needed world data either:
//! - read the dense `BlockPool` (legacy `chunk_mesh.comp`, deprecated),
//! - or consumed a `SurfaceRecord[]` CPU-extracted via
//!   `world::surface::extract_surface` (current production GPU mesher
//!   path, `chunk_mesh_sparse.comp`).
//!
//! Both are tangents to the architecture target. This module is the
//! foundation that lets the next ship retarget consumers onto a
//! single GPU substrate.
//!
//! ## What's wired this ship
//!
//! - The buffer exists, persistently mapped, with span allocator.
//! - `WorldRenderer` calls into [`OctreeBufferPool::upload`] alongside
//!   the existing mesh upload so a chunk's octree lands on the GPU
//!   exactly when its mesh does.
//! - One descriptor binding (the new `OctreeProbePipeline` compute
//!   pipeline) reads from this buffer to validate the format
//!   end-to-end. See `octree_probe_pipeline.rs`.
//!
//! ## What's NOT wired this ship
//!
//! - The production mesher does not yet read from the octree (still
//!   eats `SurfaceRecord[]` via `chunk_mesh_sparse.comp`). Next ship.
//! - The vertex/fragment shaders don't read from the octree. They
//!   read uploaded vertex meshes. Eventual material-on-GPU work will
//!   change that.
//! - The CPU-side raycast (`chunk_manager::raycast_octree`) doesn't
//!   move to GPU. Next ship after the mesher migrates.
//!
//! The substrate has to exist before consumers migrate. That's the
//! purpose of this file.
//!
//! ## Memory budget
//!
//! Sparse octrees are small. Sample data from the v0.24.23 bench:
//! - All-air chunk: 1 node (40 B) — single collapsed leaf.
//! - All-stone chunk: 1 node — same.
//! - Surface chunk (mixed terrain): 200-2000 nodes (8-80 KB).
//! - Worst-case fully-heterogeneous chunk: ~5000 nodes (200 KB).
//!
//! At RD=6 with ~925 chunks loaded, ~600 are surface or interesting:
//! - 600 × 80 KB = 48 MB (worst case at this RD)
//! - 925 × 80 KB = 74 MB (with churn headroom)
//!
//! Default buffer size is 256 MB — comfortable for RD=6..10 and stays
//! well inside Apple unified-memory budgets. Tunable.
//!
//! ## SAFETY surface
//!
//! - The persistent map pointer is `*mut u8` and held for the lifetime
//!   of the pool. The host-coherent flag means CPU writes are visible
//!   to the GPU without explicit flush, but the caller is still
//!   responsible for not writing into a region the GPU is currently
//!   reading. The renderer's defer-destroy pattern
//!   (`MAX_FRAMES_IN_FLIGHT`) handles that the same way it handles the
//!   mesh pool.
//! - `unsafe impl Send` is necessary because the raw pointer field
//!   would otherwise make the type `!Send`. The pointer is only
//!   dereferenced through `&mut self` methods, and the underlying
//!   memory is owned for the lifetime of the pool.

#![cfg(feature = "render-core")]

use ash::{vk, Device, Instance};
use std::collections::HashMap;

use crate::render::mesh_buffer_alloc::{AllocError, Span, SpanAllocator};
use crate::world::chunk::ChunkCoord;
use crate::world::octree::{OctreeArray, OCTREE_NODE_BYTES};

/// Default total octree buffer size, in bytes. See module docs for the
/// budget calculation. Tunable via [`OctreeBufferPool::with_capacity`]
/// for benches or stress tests.
pub const DEFAULT_OCTREE_BUFFER_SIZE: u64 = 256 * 1024 * 1024;

/// Resolve the live octree pool capacity in bytes from the
/// `UNENDLICH_OCTREE_POOL_MB` env var, falling back to
/// [`DEFAULT_OCTREE_BUFFER_SIZE`]. Clamped to 64..=16384 MiB so a
/// fat-fingered value can't request more memory than any consumer GPU
/// has, but the upper bound is high enough for headroom on workstation
/// hardware (M2 Ultra unified memory, 24 GB+ discrete cards).
pub fn runtime_octree_pool_bytes() -> u64 {
    const MIN_MB: u64 = 64;
    const MAX_MB: u64 = 16 * 1024;
    let mb = std::env::var("UNENDLICH_OCTREE_POOL_MB")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|m| m.clamp(MIN_MB, MAX_MB) * 1024 * 1024)
        .unwrap_or(DEFAULT_OCTREE_BUFFER_SIZE);
    mb
}

/// Per-chunk handle in the octree buffer. Carries the byte offset, the
/// node count, and a version stamp for stale-handle detection (same
/// pattern as [`crate::render::mesh_buffer_pool::MeshSlot`]).
///
/// GPU consumers receive these as `(base_node_index, node_count)` —
/// i.e., the byte offset divided by [`OCTREE_NODE_BYTES`]. The base
/// node index is what gets pushed into per-chunk metadata SSBOs in
/// shaders that read the octree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OctreeSlot {
    /// Byte offset into the storage buffer where this chunk's first
    /// node lives. Always a multiple of [`OCTREE_NODE_BYTES`] in
    /// practice because the allocator's alignment (256 B) is a
    /// multiple of the node size (40 B)? **No.** 256 % 40 = 16, so
    /// alignment differs. We track the byte offset and divide on the
    /// way out so the per-allocation slack at the start of each span
    /// is unused (a few wasted nodes per chunk; trivial).
    pub byte_offset: u64,
    /// Number of bytes allocated for this chunk's octree. Always ≥
    /// `node_count × OCTREE_NODE_BYTES`. The trailing slack is unused.
    pub byte_size: u64,
    /// Number of nodes this chunk's octree contains. The
    /// `nodes[0..node_count]` window starting at the base index is
    /// what consumers read.
    pub node_count: u32,
    /// Stale-handle detection. Same role as `MeshSlot::version`.
    pub version: u32,
}

impl OctreeSlot {
    /// Convert byte offset to node-index for shader push constants.
    ///
    /// The contract is "GPU consumers index the node array as if it
    /// were `OctreeNode[]`," so the offset must be on a node boundary.
    /// We enforce that during [`OctreeBufferPool::upload`] by aligning
    /// the byte_offset upward to a multiple of `OCTREE_NODE_BYTES`
    /// when we write the payload.
    pub fn base_node_index(&self) -> u32 {
        debug_assert_eq!(self.byte_offset % OCTREE_NODE_BYTES as u64, 0,
            "OctreeSlot byte_offset not node-aligned: {} % {} = {}",
            self.byte_offset, OCTREE_NODE_BYTES,
            self.byte_offset % OCTREE_NODE_BYTES as u64);
        (self.byte_offset / OCTREE_NODE_BYTES as u64) as u32
    }
}

/// Error type for the pool. Mirrors `MeshPoolError` shape.
#[derive(Debug)]
pub enum OctreePoolError {
    /// Span allocator refused: pool exhausted or fragmented.
    Alloc(AllocError),
    /// Stale [`OctreeSlot`] passed to `free` (already-freed or never-allocated).
    InvalidSlot,
    /// Vulkan returned an error during pool initialization.
    Backend(String),
}

impl std::fmt::Display for OctreePoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OctreePoolError::Alloc(e) => write!(f, "octree pool alloc failed: {}", e),
            OctreePoolError::InvalidSlot => write!(f, "invalid octree slot"),
            OctreePoolError::Backend(s) => write!(f, "{}", s),
        }
    }
}
impl std::error::Error for OctreePoolError {}

/// CPU-side bookkeeping. Splits cleanly from the Vulkan-owning
/// [`OctreeBufferPool`] so tests can exercise the allocator logic
/// without a Vulkan device, mirroring `mesh_buffer_pool::CpuMeshPool`.
pub struct CpuOctreePool {
    alloc: SpanAllocator,
    /// Live slots keyed by `byte_offset` so `free` can detect bogus
    /// inputs and recover the size to return to the allocator.
    live: HashMap<u64, OctreeSlot>,
    next_version: u32,
}

impl CpuOctreePool {
    pub fn new(capacity: u64) -> Self {
        Self {
            alloc: SpanAllocator::new(capacity),
            live: HashMap::new(),
            next_version: 1,
        }
    }

    pub fn capacity(&self) -> u64 { self.alloc.capacity() }
    pub fn allocated_bytes(&self) -> u64 { self.alloc.allocated_bytes() }
    pub fn free_bytes(&self) -> u64 { self.alloc.free_bytes() }
    pub fn largest_free_span(&self) -> u64 { self.alloc.largest_free_span() }
    pub fn slot_count(&self) -> usize { self.live.len() }

    /// Reserve space for `node_count` octree nodes. The returned slot
    /// is the caller's handle for writing the data and for the
    /// matching [`Self::free`].
    pub fn alloc(&mut self, node_count: u32) -> Result<OctreeSlot, OctreePoolError> {
        if node_count == 0 {
            // Degenerate: a zero-node octree shouldn't exist (every
            // chunk has at least a root). Treat as an error rather
            // than allocating a zero-size span that the consumer
            // can't read from.
            return Err(OctreePoolError::Alloc(AllocError::Exhausted {
                wanted: 0, free_total: 0, free_largest: 0,
            }));
        }
        let bytes_needed = node_count as u64 * OCTREE_NODE_BYTES as u64;
        let span: Span = self.alloc.alloc(bytes_needed).map_err(OctreePoolError::Alloc)?;
        // Promote the span's offset to a node-aligned address. The
        // SpanAllocator aligns to 256 B; 256 % 40 = 16, so the first
        // node-aligned position may sit a few bytes into the span.
        // The wasted bytes at the head are charged to this slot and
        // returned to the allocator on free.
        let node_aligned_offset = {
            let rem = span.offset % OCTREE_NODE_BYTES as u64;
            if rem == 0 {
                span.offset
            } else {
                span.offset + (OCTREE_NODE_BYTES as u64 - rem)
            }
        };
        // The aligned offset must still fit inside the allocated span
        // along with the payload. The 256-byte allocator alignment
        // guarantees this for our 40-byte nodes (worst-case slack
        // is 40-16=24 bytes at the head, and bytes_needed ≤
        // node_count * 40 < span.size which is ≥ bytes_needed
        // rounded up to 256).
        debug_assert!(node_aligned_offset + bytes_needed <= span.offset + span.size,
            "node alignment pushed past span end: aligned {} + bytes {} vs span {}..{}",
            node_aligned_offset, bytes_needed, span.offset, span.offset + span.size);

        let version = self.next_version;
        self.next_version = self.next_version.wrapping_add(1);
        if self.next_version == 0 { self.next_version = 1; }

        let slot = OctreeSlot {
            byte_offset: node_aligned_offset,
            byte_size: span.size,
            node_count,
            version,
        };
        // Live map keyed by the SPAN'S OFFSET (not the aligned one) so
        // `free` can return the original span to the allocator.
        self.live.insert(span.offset, slot);
        Ok(slot)
    }

    /// Release a previously-allocated slot. Returns `InvalidSlot` for
    /// stale or unknown handles (defensive, but cheap).
    pub fn free(&mut self, slot: OctreeSlot) -> Result<(), OctreePoolError> {
        // Recover the SPAN offset from the aligned offset. Either the
        // slot's byte_offset is already span-aligned (256 B), or it's
        // up to OCTREE_NODE_BYTES-1 ahead. Walk back to the nearest
        // 256 B boundary at or before the aligned offset to find the
        // span the allocator knows about.
        const SPAN_ALIGN: u64 = 256;
        let span_offset = slot.byte_offset / SPAN_ALIGN * SPAN_ALIGN;
        let recorded = self.live.get(&span_offset).copied()
            .ok_or(OctreePoolError::InvalidSlot)?;
        if recorded.version != slot.version {
            return Err(OctreePoolError::InvalidSlot);
        }
        self.live.remove(&span_offset);
        self.alloc.free(span_offset).map_err(OctreePoolError::Alloc)?;
        Ok(())
    }
}

/// Vulkan-backed octree buffer pool. Owns the storage buffer + memory +
/// persistent map, delegates allocator bookkeeping to [`CpuOctreePool`].
/// Lives on `WorldRenderer`.
pub struct OctreeBufferPool {
    cpu: CpuOctreePool,
    pub buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    map_ptr: *mut u8,
    capacity: u64,
    /// Maps each chunk to its current slot. Used by `evict` and by the
    /// probe pipeline to enumerate live chunks. Kept on the
    /// VulkanPool (not CpuOctreePool) because the CPU pool is meant to
    /// be testable in isolation; per-chunk identity belongs to the
    /// Vulkan layer.
    slots_by_chunk: HashMap<ChunkCoord, OctreeSlot>,
    /// v0.24.31-version-tracked: content version per resident chunk.
    /// Used by `upload_if_changed` to short-circuit redundant uploads
    /// when the same `(coord, version)` is already on the GPU. This
    /// is the chunk's content version (from the streaming layer);
    /// distinct from `OctreeSlot.version` which is a slot-replacement
    /// counter for stale-handle detection.
    ///
    /// At steady state the mesher dispatches the SAME chunks
    /// repeatedly because the chunk-manager iterator yields them on
    /// every frame's eligibility scan. Without this map, the mesher
    /// uploads ~64 chunks × 10 µs = 640 µs per batch on data that's
    /// already there. With it, post-first-batch dispatches do zero
    /// uploads for unchanged chunks.
    content_versions: HashMap<ChunkCoord, u64>,
}

// SAFETY: `map_ptr` is a raw pointer into device memory we own; all
// reads/writes go through `&mut self` methods. The Device and Vulkan
// objects are Send by `ash`'s contract.
unsafe impl Send for OctreeBufferPool {}

impl OctreeBufferPool {
    /// Create the pool with the default buffer capacity.
    ///
    /// SAFETY: caller must keep `device` and `instance` alive while
    /// the returned pool is in use. Capacity defaults to
    /// [`DEFAULT_OCTREE_BUFFER_SIZE`] but is tunable via the
    /// `UNENDLICH_OCTREE_POOL_MB` environment variable for systems with
    /// device-memory headroom — at 256 MiB the pool is a hard ceiling well
    /// below typical workstation GPU memory; raising it directly raises the
    /// number of resident chunks before OOM fallback fires.
    pub unsafe fn new(
        device: &Device,
        instance: &Instance,
        physical_device: vk::PhysicalDevice,
    ) -> Result<Self, OctreePoolError> {
        let cap = runtime_octree_pool_bytes();
        Self::with_capacity(device, instance, physical_device, cap)
    }

    /// Create the pool with a specific buffer capacity. Useful for
    /// tests and benches that don't need the full 256 MB.
    pub unsafe fn with_capacity(
        device: &Device,
        instance: &Instance,
        physical_device: vk::PhysicalDevice,
        capacity: u64,
    ) -> Result<Self, OctreePoolError> {
        let (buffer, memory, map_ptr) = create_octree_buffer(
            device, instance, physical_device, capacity,
        )?;
        Ok(Self {
            cpu: CpuOctreePool::new(capacity),
            buffer,
            memory,
            map_ptr,
            capacity,
            slots_by_chunk: HashMap::new(),
            content_versions: HashMap::new(),
        })
    }

    /// Scatter resident chunks' base node indices into a caller grid, for the
    /// neural feature pass's hint-ray march. `origin` = the grid's min chunk coord,
    /// `dims` = its extent in chunks; `out` (len dims.x*dims.y*dims.z, indexed
    /// `(z*dy + y)*dx + x`) must be pre-filled with EMPTY (u32::MAX). Only chunks
    /// inside the grid are written; O(resident chunks).
    pub fn fill_base_grid(&self, origin: [i32; 3], dims: [u32; 3], out: &mut [u32]) {
        let (dx, dy, dz) = (dims[0] as i32, dims[1] as i32, dims[2] as i32);
        for (coord, slot) in &self.slots_by_chunk {
            let (rx, ry, rz) = (coord.x - origin[0], coord.y - origin[1], coord.z - origin[2]);
            if rx < 0 || ry < 0 || rz < 0 || rx >= dx || ry >= dy || rz >= dz {
                continue;
            }
            let idx = ((rz * dy + ry) * dx + rx) as usize;
            if idx < out.len() {
                out[idx] = slot.base_node_index();
            }
        }
    }

    pub fn capacity(&self) -> u64 { self.capacity }
    pub fn allocated_bytes(&self) -> u64 { self.cpu.allocated_bytes() }
    /// Bytes in the single largest contiguous free span. When this drops far
    /// below `capacity() - allocated_bytes()`, the pool is fragmented and
    /// will OOM on chunks larger than this span even though the totals look
    /// healthy. Surfaced in `gpu_mem:` so we can see fragmentation early.
    pub fn largest_free_span(&self) -> u64 { self.cpu.largest_free_span() }
    pub fn free_bytes(&self) -> u64 { self.cpu.free_bytes() }
    pub fn live_chunks(&self) -> usize { self.slots_by_chunk.len() }
    pub fn slot(&self, coord: ChunkCoord) -> Option<OctreeSlot> {
        self.slots_by_chunk.get(&coord).copied()
    }
    /// Iterate live (coord, slot) pairs. Used by the probe pipeline to
    /// build its verification work-list.
    pub fn iter_slots(&self) -> impl Iterator<Item = (ChunkCoord, OctreeSlot)> + '_ {
        self.slots_by_chunk.iter().map(|(c, s)| (*c, *s))
    }

    /// Upload (or replace) the GPU-resident octree for `coord`. If an
    /// older slot exists, it is freed AFTER the new one writes (no-op
    /// here; the v0.24.26 ship doesn't defer-destroy octree slots
    /// because no GPU read-pipeline depends on them YET. The probe
    /// pipeline runs synchronously, releases the descriptor before
    /// returning, so an immediate free is safe. When a real consumer
    /// (mesher, raycast) lands, this will need MAX_FRAMES_IN_FLIGHT
    /// deferment like the mesh pool — filed for that ship).
    ///
    /// SAFETY: the persistent map write is to HOST_VISIBLE |
    /// HOST_COHERENT memory. As long as no GPU pipeline is reading
    /// this slot's range at the moment of write, the host-coherent
    /// flag ensures the GPU sees the new bytes on its next read.
    /// During v0.24.26 the only reader is the probe pipeline which is
    /// invoked explicitly and finishes before the next upload.
    pub fn upload(
        &mut self,
        coord: ChunkCoord,
        octree: &OctreeArray,
    ) -> Result<OctreeSlot, OctreePoolError> {
        let node_count = octree.nodes.len();
        if node_count == 0 {
            // An empty octree shouldn't happen — every chunk has a
            // root. Treat as a programming error rather than silently
            // succeeding with no allocation.
            return Err(OctreePoolError::Alloc(AllocError::Exhausted {
                wanted: 0, free_total: 0, free_largest: 0,
            }));
        }
        // Cap at u32 so the SSBO-index math doesn't overflow. 5-deep
        // octree caps the node count at ~5000 per chunk, so this is a
        // belt-and-braces check.
        let node_count_u32 = u32::try_from(node_count)
            .map_err(|_| OctreePoolError::Alloc(AllocError::Exhausted {
                wanted: u32::MAX as u64,
                free_total: self.cpu.free_bytes(),
                free_largest: self.cpu.largest_free_span(),
            }))?;

        let new_slot = self.cpu.alloc(node_count_u32)?;
        // Write octree nodes into the persistent map. Bytes-of-T is
        // safe here because OctreeNode is `repr(C)` and its size is
        // statically asserted to be 40 bytes (see octree.rs).
        let src_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                octree.nodes.as_ptr() as *const u8,
                node_count * OCTREE_NODE_BYTES,
            )
        };
        unsafe {
            std::ptr::copy_nonoverlapping(
                src_bytes.as_ptr(),
                self.map_ptr.add(new_slot.byte_offset as usize),
                src_bytes.len(),
            );
        }

        // Replace any prior slot for this chunk. Free the old AFTER
        // the new write so any in-flight (synchronous) probe read
        // still sees consistent data — but in practice probes are
        // single-shot and finished before this is called.
        let old_slot = self.slots_by_chunk.insert(coord, new_slot);
        if let Some(old) = old_slot {
            // free errors are ignorable here: the worst case is a
            // leaked span which the allocator will surface as
            // exhaustion much later. We log so the leak isn't silent.
            if let Err(e) = self.cpu.free(old) {
                log::warn!("octree pool: free of replaced slot for {:?} failed: {} \
                            (continuing; slot is leaked)", coord, e);
            }
        }
        Ok(new_slot)
    }

    /// Drop a chunk's octree slot. Called from the renderer's
    /// `evict_chunk` path. No-op if the chunk isn't tracked.
    pub fn evict(&mut self, coord: ChunkCoord) {
        if let Some(slot) = self.slots_by_chunk.remove(&coord) {
            if let Err(e) = self.cpu.free(slot) {
                log::warn!("octree pool: evict for {:?} failed: {} (slot leaked)", coord, e);
            }
        }
        // v0.24.31-version-tracked: drop the content version too so a
        // future re-upload doesn't get short-circuited against stale.
        self.content_versions.remove(&coord);
    }

    /// v0.24.31-version-tracked: upload only if the chunk's content
    /// version differs from what's currently in the pool. When the
    /// version matches, returns the existing slot without copying
    /// any bytes.
    ///
    /// Callers that don't track content version (e.g., the renderer's
    /// `upload_chunk_octree` path) should keep using `upload` — that
    /// path is unaffected by this optimization.
    ///
    /// `content_version` is interpreted only by Eq. Any monotonic
    /// counter the streaming layer provides works; this method just
    /// compares against the previously-stored value.
    pub fn upload_if_changed(
        &mut self,
        coord: ChunkCoord,
        octree: &OctreeArray,
        content_version: u64,
    ) -> Result<OctreeSlot, OctreePoolError> {
        if let Some(existing_slot) = self.slots_by_chunk.get(&coord).copied() {
            if self.content_versions.get(&coord) == Some(&content_version) {
                // Already resident at this version — fast path.
                return Ok(existing_slot);
            }
        }
        let slot = self.upload(coord, octree)?;
        self.content_versions.insert(coord, content_version);
        Ok(slot)
    }

    /// Tear down the buffer + memory. Caller must guarantee no GPU
    /// work references this pool's buffer when destroy is called.
    pub unsafe fn destroy(&mut self, device: &Device) {
        device.unmap_memory(self.memory);
        device.destroy_buffer(self.buffer, None);
        device.free_memory(self.memory, None);
        self.slots_by_chunk.clear();
        self.content_versions.clear();
    }
}

/// Internal buffer-creation helper. Same recipe as
/// `mesh_buffer_pool::create_persistent_buffer` but for a STORAGE
/// buffer (the octree is read as SSBO by compute and graphics
/// shaders).
unsafe fn create_octree_buffer(
    device: &Device,
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
    size: u64,
) -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8), OctreePoolError> {
    let info = vk::BufferCreateInfo::builder()
        .size(size)
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = device.create_buffer(&info, None)
        .map_err(|e| OctreePoolError::Backend(format!("vkCreateBuffer: {:?}", e)))?;

    let req = device.get_buffer_memory_requirements(buffer);
    let props = instance.get_physical_device_memory_properties(physical_device);

    let mut chosen = None;
    for i in 0..props.memory_type_count {
        let mt = props.memory_types[i as usize];
        if req.memory_type_bits & (1 << i) != 0
            && mt.property_flags.contains(
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )
        {
            chosen = Some(i);
            break;
        }
    }
    let mt = chosen.ok_or_else(||
        OctreePoolError::Backend("No HOST_VISIBLE | HOST_COHERENT memory type".into()))?;

    let alloc = vk::MemoryAllocateInfo::builder()
        .allocation_size(req.size)
        .memory_type_index(mt);
    let memory = device.allocate_memory(&alloc, None)
        .map_err(|e| OctreePoolError::Backend(format!("vkAllocateMemory: {:?}", e)))?;
    device.bind_buffer_memory(buffer, memory, 0)
        .map_err(|e| OctreePoolError::Backend(format!("vkBindBufferMemory: {:?}", e)))?;

    let ptr = device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
        .map_err(|e| OctreePoolError::Backend(format!("vkMapMemory: {:?}", e)))? as *mut u8;

    Ok((buffer, memory, ptr))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Allocator returns node-aligned offsets so consumers can use
    /// `base_node_index()` without surprise.
    #[test]
    fn alloc_offsets_are_node_aligned() {
        let mut p = CpuOctreePool::new(1024 * 1024);
        let s1 = p.alloc(100).unwrap();
        assert_eq!(s1.byte_offset % OCTREE_NODE_BYTES as u64, 0);
        assert_eq!(s1.node_count, 100);
        let s2 = p.alloc(73).unwrap();
        assert_eq!(s2.byte_offset % OCTREE_NODE_BYTES as u64, 0);
        assert_eq!(s2.node_count, 73);
        // Slots should be distinct.
        assert_ne!(s1.byte_offset, s2.byte_offset);
    }

    /// base_node_index round-trips byte_offset cleanly. This is the
    /// math the shader push-constant uses.
    #[test]
    fn base_node_index_is_byte_offset_over_node_size() {
        let mut p = CpuOctreePool::new(1024 * 1024);
        let s = p.alloc(50).unwrap();
        let want = (s.byte_offset / OCTREE_NODE_BYTES as u64) as u32;
        assert_eq!(s.base_node_index(), want);
    }

    /// Freeing a slot returns its bytes to the allocator.
    #[test]
    fn free_returns_bytes() {
        let mut p = CpuOctreePool::new(64 * 1024);
        let s = p.alloc(200).unwrap();
        let used = p.allocated_bytes();
        assert!(used >= 200 * OCTREE_NODE_BYTES as u64);
        p.free(s).unwrap();
        assert_eq!(p.allocated_bytes(), 0);
        assert_eq!(p.slot_count(), 0);
    }

    /// Stale handles can't free real allocations.
    #[test]
    fn stale_slot_rejected() {
        let mut p = CpuOctreePool::new(64 * 1024);
        let s = p.alloc(200).unwrap();
        p.free(s).unwrap();
        // Same handle, second free: stale because the slot has been
        // removed from the live map.
        let err = p.free(s).unwrap_err();
        assert!(matches!(err, OctreePoolError::InvalidSlot));
    }

    /// Zero-node alloc is rejected (the underlying octree is malformed).
    #[test]
    fn zero_node_alloc_rejected() {
        let mut p = CpuOctreePool::new(64 * 1024);
        let err = p.alloc(0).unwrap_err();
        assert!(matches!(err, OctreePoolError::Alloc(_)));
    }

    /// Many allocations then frees in mixed order — fragmentation
    /// regression check, same shape as the mesh pool's churn test.
    #[test]
    fn churn_pattern_does_not_leak() {
        let mut p = CpuOctreePool::new(8 * 1024 * 1024);
        let mut live = Vec::new();
        let mut rng = 0xACEDu32;
        let next = |r: &mut u32| {
            *r = r.wrapping_mul(1103515245).wrapping_add(12345);
            *r
        };
        for _ in 0..1500 {
            let do_alloc = (next(&mut rng) & 1) == 1 || live.is_empty();
            if do_alloc {
                let n = 1 + (next(&mut rng) % 500) as u32;
                if let Ok(s) = p.alloc(n) { live.push(s); }
            } else {
                let i = (next(&mut rng) as usize) % live.len();
                let s = live.swap_remove(i);
                p.free(s).unwrap();
            }
        }
        for s in live { p.free(s).unwrap(); }
        assert_eq!(p.allocated_bytes(), 0);
        assert_eq!(p.slot_count(), 0);
    }
}
