//! Mesh buffer pool — sub-allocator backed (v0.19).
//!
//! Two large GPU buffers (vertex + index) carved into variable-size spans
//! by a free-list allocator. Each chunk's mesh occupies a span sized to its
//! actual byte count — no fixed slot caps, no fixed-size waste. Per-chunk
//! upload is a `memcpy` into the persistent map.
//!
//! # History
//!
//! - v0.18.2: introduced fixed-size slots, eliminated `vkAllocateMemory`
//!   per chunk (the freeze fix).
//! - v0.18.3: bumped slot caps from 128 KB / 64 KB to 3 MB / 2 MB after
//!   playtest revealed real chunks are far larger than initially guessed.
//!   ~70-80% of chunks fit; the rest dropped.
//! - v0.19 (this file): replaced fixed slots with a free-list span allocator
//!   (`super::mesh_buffer_alloc::SpanAllocator`). Every chunk gets exactly
//!   the bytes it needs. Total buffer size is the only sizing decision.

use ash::vk;
use super::mesh_buffer_alloc::{AllocError, SpanAllocator};

/// Default total vertex-buffer size. With v0.18.3 playtest data showing
/// ~925 chunks loaded at render distance 6 with sizes from 21 KB to 7 MB,
/// 3 GB gives comfortable headroom while staying within Apple Silicon
/// unified-memory budgets.
pub const DEFAULT_VERT_BUFFER_SIZE: u64 = 3 * 1024 * 1024 * 1024;
/// Default total index-buffer size. Index data is ~45% of vertex bytes
/// in the playtest distribution.
pub const DEFAULT_INDEX_BUFFER_SIZE: u64 = 2 * 1024 * 1024 * 1024;

/// Errors from pool operations.
#[derive(Debug)]
pub enum MeshPoolError {
    /// Allocation failed — pool is full or fragmented enough that no
    /// span fits this mesh. The `&'static str` distinguishes vertex
    /// vs index axis since each has its own allocator.
    Alloc(&'static str, AllocError),
    /// Caller passed an invalid slot handle (stale version, never-allocated,
    /// or already-freed).
    InvalidSlot,
    /// Vulkan-layer failure during pool initialization.
    Backend(String),
}

impl std::fmt::Display for MeshPoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeshPoolError::Alloc(kind, e) => write!(f, "Mesh pool {} alloc failed: {}", kind, e),
            MeshPoolError::InvalidSlot => write!(f, "Invalid mesh pool slot"),
            MeshPoolError::Backend(s) => write!(f, "{}", s),
        }
    }
}
impl std::error::Error for MeshPoolError {}

/// Slot handle. Carries byte offsets and sizes for vertex and index
/// spans. Renderer reads these directly into `cmd_bind_vertex_buffers`
/// and `cmd_bind_index_buffer`.
///
/// `version` exists for stale-handle detection: if a slot is freed and
/// the same offsets get re-allocated, the version differs so old
/// `MeshSlot` values can't accidentally point to new allocations.
///
/// `vert_base_aligned` is the byte offset where the vertex DATA actually
/// begins, possibly slightly after `vert_offset`. The two differ when
/// the vertex stride doesn't divide the allocator alignment (256).
/// The indirect-draw path uses `vert_base_aligned / PackedVertex::STRIDE`
/// as the per-chunk vertex base index — passed to the GPU via the
/// indirect command's `vertexOffset` field (v0.24.20-vertex-pack). The
/// direct-draw path can bind from `vert_base_aligned` directly. The
/// allocator-managed offset is `vert_offset` and that's what gets
/// passed to free().
///
/// Historical note: pre-v0.24.20 the renderer pre-shifted every index
/// on CPU by `vert_base_aligned / sizeof(Vertex)` so that
/// `vertexOffset` could stay 0. v0.24.20 moved that addition onto the
/// GPU via `vertexOffset`, eliminating the per-upload CPU pass over
/// all indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshSlot {
    pub vert_offset: u64,
    pub vert_size: u64,
    pub vert_base_aligned: u64,
    pub idx_offset: u64,
    pub idx_size: u64,
    pub version: u32,
}

/// Pure-Rust pool backing for testing. Owns no GPU memory but performs
/// the same span allocation as the Vulkan pool.
pub struct CpuMeshPool {
    vert_alloc: SpanAllocator,
    idx_alloc: SpanAllocator,
    /// Tracks live slots by their vert_offset (the natural primary key).
    live: std::collections::HashMap<u64, MeshSlot>,
    next_version: u32,
}

impl CpuMeshPool {
    pub fn new(vert_buffer_size: u64, idx_buffer_size: u64) -> Self {
        Self {
            vert_alloc: SpanAllocator::new(vert_buffer_size),
            idx_alloc: SpanAllocator::new(idx_buffer_size),
            live: std::collections::HashMap::new(),
            next_version: 1,
        }
    }

    pub fn vert_buffer_size(&self) -> u64 { self.vert_alloc.capacity() }
    pub fn idx_buffer_size(&self) -> u64 { self.idx_alloc.capacity() }
    pub fn vert_allocated_bytes(&self) -> u64 { self.vert_alloc.allocated_bytes() }
    pub fn idx_allocated_bytes(&self) -> u64 { self.idx_alloc.allocated_bytes() }
    pub fn vert_free_bytes(&self) -> u64 { self.vert_alloc.free_bytes() }
    pub fn idx_free_bytes(&self) -> u64 { self.idx_alloc.free_bytes() }
    pub fn vert_largest_free_span(&self) -> u64 { self.vert_alloc.largest_free_span() }
    pub fn idx_largest_free_span(&self) -> u64 { self.idx_alloc.largest_free_span() }
    pub fn slot_count(&self) -> usize { self.live.len() }

    /// Allocate spans for a mesh. Atomic across both axes: if the index
    /// allocation fails after the vertex one succeeded, the vertex span
    /// is rolled back so the pool stays consistent.
    ///
    /// `vertex_size` is the size in bytes of one vertex element. The
    /// returned slot's `vert_base_aligned` is `vert_offset` rounded UP
    /// to the next multiple of `vertex_size`. The vertex span is
    /// requested with `vert_bytes + (vertex_size - 1)` bytes so even
    /// after rounding up, the data fits. Pass `vertex_size = 1` to
    /// skip alignment (vert_base_aligned == vert_offset).
    pub fn alloc(
        &mut self,
        vert_bytes: u64,
        idx_bytes: u64,
        vertex_size: u64,
    ) -> Result<MeshSlot, MeshPoolError> {
        let v_pad = if vertex_size > 1 { vertex_size - 1 } else { 0 };
        let v_span = self.vert_alloc.alloc(vert_bytes + v_pad)
            .map_err(|e| MeshPoolError::Alloc("vertex", e))?;
        let i_span = match self.idx_alloc.alloc(idx_bytes) {
            Ok(s) => s,
            Err(e) => {
                let _ = self.vert_alloc.free(v_span.offset);
                return Err(MeshPoolError::Alloc("index", e));
            }
        };

        let version = self.next_version;
        self.next_version = self.next_version.wrapping_add(1);
        if self.next_version == 0 { self.next_version = 1; }

        let vert_base_aligned = if vertex_size > 1 {
            ((v_span.offset + vertex_size - 1) / vertex_size) * vertex_size
        } else {
            v_span.offset
        };

        let slot = MeshSlot {
            vert_offset: v_span.offset,
            vert_size: v_span.size,
            vert_base_aligned,
            idx_offset: i_span.offset,
            idx_size: i_span.size,
            version,
        };
        self.live.insert(slot.vert_offset, slot);
        Ok(slot)
    }

    pub fn free(&mut self, slot: MeshSlot) -> Result<(), MeshPoolError> {
        let recorded = self.live.get(&slot.vert_offset).copied()
            .ok_or(MeshPoolError::InvalidSlot)?;
        if recorded.version != slot.version {
            return Err(MeshPoolError::InvalidSlot);
        }
        self.live.remove(&slot.vert_offset);
        self.vert_alloc.free(slot.vert_offset)
            .map_err(|e| MeshPoolError::Alloc("vertex", e))?;
        self.idx_alloc.free(slot.idx_offset)
            .map_err(|e| MeshPoolError::Alloc("index", e))?;
        Ok(())
    }
}

/// Vulkan-backed mesh buffer pool.
pub struct VulkanMeshPool {
    pool: CpuMeshPool,

    /// The user-visible buffers — what the renderer binds and draws from.
    ///
    /// **Default mode (staging is None):** these are HOST_VISIBLE |
    /// HOST_COHERENT, persistent-mapped. `vertex_ptr`/`index_ptr` point
    /// directly at their mapped memory. `write_at` does a CPU memcpy
    /// into them.
    ///
    /// **DEVICE_LOCAL mode (staging is Some, v0.24.71 Stage 3 sub-stage A):**
    /// these are DEVICE_LOCAL (no host visibility, no map).
    /// `vertex_ptr`/`index_ptr` point at the STAGING buffer's mapped
    /// memory. `write_at` memcpys into staging and queues a
    /// `vk::BufferCopy` region; the renderer must call
    /// `flush_pending_copies` once per command buffer before binding
    /// these for draws, which records `vkCmdCopyBuffer` (staging →
    /// device) + a transfer→vertex-input barrier.
    pub vertex_buffer: vk::Buffer,
    pub index_buffer: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    index_memory: vk::DeviceMemory,
    /// Pointer to mapped CPU-writable memory. In default mode, this is
    /// the user-visible buffer's map. In DEVICE_LOCAL mode, this is the
    /// staging buffer's map.
    vertex_ptr: *mut u8,
    index_ptr: *mut u8,

    /// v0.24.71-stage3a: when `Some`, the user-visible buffers are
    /// DEVICE_LOCAL and the host-side memcpys land in this staging
    /// state. Pending copy regions accumulate here; `flush_pending_copies`
    /// drains them. `None` preserves the legacy HOST_COHERENT direct-map
    /// behavior byte-for-byte.
    staging: Option<StagingState>,
}

/// v0.24.71-stage3a: staging state for DEVICE_LOCAL backing.
///
/// Holds the HOST_VISIBLE | HOST_COHERENT staging buffers that
/// `write_at` memcpys into, plus the pending list of
/// `vkCmdCopyBuffer` regions that `flush_pending_copies` will
/// record. The CpuMeshPool's slot offsets are valid in BOTH the
/// staging and device buffers (identity-offset copy).
///
/// **Sync model**: write_at is a pure CPU operation (memcpy +
/// Vec push). Pending copies accumulate during cmd-buffer
/// recording. flush_pending_copies records:
///   1. vkCmdCopyBuffer for each pending region (vertex)
///   2. vkCmdCopyBuffer for each pending region (index)
///   3. vk::BufferMemoryBarrier transitioning vertex/index buffers
///      from TRANSFER_WRITE → VERTEX_ATTRIBUTE_READ | INDEX_READ
///      at pipeline stage TRANSFER → VERTEX_INPUT
/// After flush, the buffers are safe to bind for draws. The
/// barrier ensures the copy completes before any vertex fetch.
struct StagingState {
    staging_vertex_buffer: vk::Buffer,
    staging_vertex_memory: vk::DeviceMemory,
    staging_index_buffer: vk::Buffer,
    staging_index_memory: vk::DeviceMemory,
    /// Pending (src_offset, dst_offset, size) tuples for vertex
    /// transfers. Drained on flush_pending_copies.
    pending_vert_copies: Vec<vk::BufferCopy>,
    /// Same for the index buffer.
    pending_idx_copies: Vec<vk::BufferCopy>,
    /// v0.25-prefetch: deferrable copies for PREFETCHED, not-yet-visible
    /// chunks. `flush_pending_copies` drains these under a per-frame byte
    /// budget (`UNENDLICH_MESH_DEFER_BUDGET_BYTES`) and carries the rest to
    /// the next frame, so a prefetch burst can't stall one frame's head.
    /// SAFE to defer ONLY because these slots are not bound for drawing
    /// until the camera reaches them — by which time the copy has landed.
    /// Visible-this-frame writes MUST use the non-deferrable `write_at`.
    pending_deferrable_vert_copies: Vec<vk::BufferCopy>,
    pending_deferrable_idx_copies: Vec<vk::BufferCopy>,
}

/// v0.25-prefetch: per-frame byte budget for draining deferrable (prefetch)
/// copies. `0` disables spreading (drains all deferrable immediately,
/// pre-v0.25 behavior). Default 1 MiB/frame — only bites on prefetch
/// bursts; normal per-frame writes are far smaller.
fn mesh_defer_budget_bytes() -> u64 {
    std::env::var("UNENDLICH_MESH_DEFER_BUDGET_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1 << 20)
}

/// r730: opt-in coalescing of contiguous buffer-copy regions in the mesh upload
/// flush (UNENDLICH_COPY_COALESCE=1). Off by default so the recorded copies are
/// bit-for-bit identical to prior builds.
fn copy_coalesce_enabled() -> bool {
    std::env::var("UNENDLICH_COPY_COALESCE").as_deref() == Ok("1")
}

/// Coalesce a vector of `vk::BufferCopy` in place via the pure
/// [`crate::render::copy_coalesce`] core. Byte-identical: only merges runs that
/// are contiguous in both src and dst.
fn coalesce_vk_copies(copies: &mut Vec<vk::BufferCopy>) {
    use crate::render::copy_coalesce::{coalesced, CopyRegion};
    if copies.len() <= 1 {
        return;
    }
    let regions: Vec<CopyRegion> = copies
        .iter()
        .map(|c| CopyRegion { src: c.src_offset, dst: c.dst_offset, size: c.size })
        .collect();
    let merged = coalesced(&regions);
    if merged.len() < copies.len() {
        *copies = merged
            .into_iter()
            .map(|r| vk::BufferCopy { src_offset: r.src, dst_offset: r.dst, size: r.size })
            .collect();
    }
}

/// Pure budget-split for the deferrable copy lane: given copy `sizes` (in
/// order) and a `budget`, with `already_spent` bytes consumed earlier this
/// frame, return how many leading copies to drain now. The remainder carries
/// to the next frame. Always drains at least one copy when nothing has been
/// spent yet (forward-progress guarantee, even if a single copy exceeds the
/// budget). Extracted from `flush_pending_copies` so the budget arithmetic is
/// unit-testable without a Vulkan device.
fn count_within_budget(sizes: impl IntoIterator<Item = u64>, budget: u64, already_spent: u64) -> usize {
    let mut spent = already_spent;
    let mut n = 0usize;
    for s in sizes {
        if spent > 0 && spent + s > budget {
            break;
        }
        spent += s;
        n += 1;
    }
    n
}

#[cfg(test)]
mod budget_tests {
    use super::count_within_budget;

    #[test]
    fn empty_drains_none() {
        assert_eq!(count_within_budget([], 1000, 0), 0);
    }

    #[test]
    fn fits_under_budget_drains_all() {
        assert_eq!(count_within_budget([10u64, 10, 10], 1000, 0), 3);
    }

    #[test]
    fn partial_drain_carries_remainder() {
        // 10 (spent 10), 10 (spent 20), next 20+10>25 -> stop at 2.
        assert_eq!(count_within_budget([10u64, 10, 10], 25, 0), 2);
    }

    #[test]
    fn forward_progress_on_oversized_first() {
        // A single copy bigger than the whole budget still drains (n>=1),
        // so the queue can never deadlock on one large copy.
        assert_eq!(count_within_budget([5000u64, 10], 1000, 0), 1);
    }

    #[test]
    fn already_spent_can_drain_zero() {
        // If an earlier lane (vertex) already filled the budget, the index
        // lane may legitimately drain nothing this frame.
        assert_eq!(count_within_budget([10u64, 10], 8, 8), 0);
    }

    #[test]
    fn shared_spent_continues_across_lanes() {
        // vertex lane drains 2 (spent 20), index lane continues with spent=20
        // against budget 25 -> only a <=5 copy fits.
        let v = count_within_budget([10u64, 10, 10], 25, 0);
        assert_eq!(v, 2);
        let i = count_within_budget([4u64, 10], 25, 20);
        assert_eq!(i, 1);
    }
}

unsafe impl Send for VulkanMeshPool {}

/// Returns the tightest contiguous range `(offset, size)` that
/// covers the destination byte range of every copy in `copies`.
/// Returns `None` for an empty input.
///
/// Used by `flush_pending_copies` to scope the post-copy
/// buffer-memory barrier to just the bytes that were actually
/// written this frame, instead of `vk::WHOLE_SIZE`. On drivers
/// that track resource state at sub-buffer granularity (notably
/// Honeykrisp on Apple Silicon AGX, which tracks per-page dirty
/// state), a tighter range lets more of the vertex pipeline
/// overlap with prior commands instead of stalling on the whole
/// multi-GB buffer. Drivers that flatten to whole-buffer state
/// see the same observable semantics either way.
///
/// Overlapping or unsorted copy regions are handled correctly —
/// only the absolute min `dst_offset` and max `dst_offset + size`
/// matter.
fn bounded_copy_dst_range(copies: &[vk::BufferCopy]) -> Option<(u64, u64)> {
    let first = copies.first()?;
    let mut lo = first.dst_offset;
    let mut hi = first.dst_offset + first.size;
    for c in &copies[1..] {
        if c.dst_offset < lo { lo = c.dst_offset; }
        let end = c.dst_offset + c.size;
        if end > hi { hi = end; }
    }
    Some((lo, hi - lo))
}

impl VulkanMeshPool {
    pub unsafe fn new(
        device: &ash::Device,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
    ) -> Result<Self, MeshPoolError> {
        Self::with_buffer_sizes(
            device, instance, physical_device,
            DEFAULT_VERT_BUFFER_SIZE,
            DEFAULT_INDEX_BUFFER_SIZE,
        )
    }

    pub unsafe fn with_buffer_sizes(
        device: &ash::Device,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        vert_buffer_size: u64,
        idx_buffer_size: u64,
    ) -> Result<Self, MeshPoolError> {
        let pool = CpuMeshPool::new(vert_buffer_size, idx_buffer_size);

        // v0.24.71-stage3a: opt-in DEVICE_LOCAL backing with HOST_VISIBLE
        // staging. Default OFF preserves the existing HOST_COHERENT
        // direct-map allocation.
        //
        // When `UNENDLICH_DEVICE_LOCAL_MESH=1`:
        //   - vertex/index buffers are DEVICE_LOCAL (no host map). The
        //     renderer binds these for draws.
        //   - Two parallel HOST_VISIBLE staging buffers receive CPU
        //     writes via the persistent map.
        //   - `flush_pending_copies()` records vkCmdCopyBuffer from
        //     staging into device buffers + a memory barrier to make
        //     the result visible to vertex/index fetch.
        //
        // **Hardware caveat (Apple Silicon / AGX / Honeykrisp):** unified
        // memory means DEVICE_LOCAL may resolve to the same physical
        // memory as HOST_VISIBLE. The perf win on this hardware comes
        // primarily from the explicit transfer→vertex barrier (correct
        // weak-ordering semantics) and the cleaner separation of CPU
        // write vs GPU read. On discrete-VRAM GPUs the win is larger
        // (separate VRAM transfer). Either way the protocol is the
        // architecturally correct one.
        // r707: ReBAR strictly dominates the DEVICE_LOCAL-staging path — it gives
        // device-local read speed AND a persistent host map AND zero copies. So when
        // ReBAR memory is available (and not opted out via UNENDLICH_NO_REBAR), use
        // the direct `create_persistent_buffer` (ReBAR) path for the vertex/index
        // buffers REGARDLESS of UNENDLICH_DEVICE_LOCAL_MESH. This removes the footgun
        // where the staging copy-stall was silently re-enabled (the flag defaults ON,
        // so only an explicit `=0` used to reach ReBAR). Staging remains the fallback
        // only when ReBAR is unavailable.
        let device_local_requested = std::env::var("UNENDLICH_DEVICE_LOCAL_MESH")
            .ok().as_deref() != Some("0"); // R27: default ON
        let rebar_ok = std::env::var("UNENDLICH_NO_REBAR").is_err()
            && rebar_available(instance, physical_device);
        if rebar_ok && device_local_requested {
            log::info!(
                "mesh_pool: ReBAR available — using zero-copy direct mesh buffers, \
                 ignoring UNENDLICH_DEVICE_LOCAL_MESH (ReBAR supersedes staging)"
            );
        }
        let device_local_enabled = device_local_requested && !rebar_ok;

        if device_local_enabled {
            log::info!(
                "mesh_pool: DEVICE_LOCAL backing ENABLED (UNENDLICH_DEVICE_LOCAL_MESH=1) — \
                 vertex/index buffers are DEVICE_LOCAL, staging buffers are HOST_VISIBLE | HOST_COHERENT, \
                 copies recorded per-frame via flush_pending_copies"
            );

            // Device-local user-visible buffers (TRANSFER_DST so the
            // copy from staging can write into them).
            let (vertex_buffer, vertex_memory) = create_device_local_buffer(
                device, instance, physical_device,
                vert_buffer_size,
                vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            )?;
            let (index_buffer, index_memory) = create_device_local_buffer(
                device, instance, physical_device,
                idx_buffer_size,
                vk::BufferUsageFlags::INDEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            )?;

            // Staging buffers (HOST_VISIBLE | HOST_COHERENT, TRANSFER_SRC)
            // — what write_at memcpys into.
            let (staging_vertex_buffer, staging_vertex_memory, vertex_ptr) = create_persistent_buffer(
                device, instance, physical_device,
                vert_buffer_size,
                vk::BufferUsageFlags::TRANSFER_SRC,
            )?;
            let (staging_index_buffer, staging_index_memory, index_ptr) = create_persistent_buffer(
                device, instance, physical_device,
                idx_buffer_size,
                vk::BufferUsageFlags::TRANSFER_SRC,
            )?;

            Ok(Self {
                pool,
                vertex_buffer, index_buffer,
                vertex_memory, index_memory,
                vertex_ptr, index_ptr,
                staging: Some(StagingState {
                    staging_vertex_buffer, staging_vertex_memory,
                    staging_index_buffer, staging_index_memory,
                    pending_vert_copies: Vec::new(),
                    pending_idx_copies: Vec::new(),
                    pending_deferrable_vert_copies: Vec::new(),
                    pending_deferrable_idx_copies: Vec::new(),
                }),
            })
        } else {
            // Legacy path: HOST_VISIBLE | HOST_COHERENT direct map.
            // Byte-for-byte identical to pre-v0.24.71 behavior.
            let (vertex_buffer, vertex_memory, vertex_ptr) = create_persistent_buffer(
                device, instance, physical_device,
                vert_buffer_size,
                vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            )?;
            let (index_buffer, index_memory, index_ptr) = create_persistent_buffer(
                device, instance, physical_device,
                idx_buffer_size,
                vk::BufferUsageFlags::INDEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            )?;

            Ok(Self {
                pool,
                vertex_buffer, index_buffer,
                vertex_memory, index_memory,
                vertex_ptr, index_ptr,
                staging: None,
            })
        }
    }

    pub fn vert_buffer_size(&self) -> u64 { self.pool.vert_buffer_size() }
    pub fn idx_buffer_size(&self) -> u64 { self.pool.idx_buffer_size() }
    pub fn vert_allocated_bytes(&self) -> u64 { self.pool.vert_allocated_bytes() }
    pub fn idx_allocated_bytes(&self) -> u64 { self.pool.idx_allocated_bytes() }
    pub fn vert_largest_free_span(&self) -> u64 { self.pool.vert_largest_free_span() }
    pub fn idx_largest_free_span(&self) -> u64 { self.pool.idx_largest_free_span() }
    pub fn slot_count(&self) -> usize { self.pool.slot_count() }

    pub fn free(&mut self, slot: MeshSlot) -> Result<(), MeshPoolError> {
        self.pool.free(slot)
    }

    /// Allocate spans without uploading data. Used by callers that need
    /// to know `vert_base_aligned` before writing the bytes (e.g., the
    /// indirect-draw renderer pre-shifts indices by the vertex base).
    pub fn alloc(
        &mut self,
        vert_bytes: u64,
        idx_bytes: u64,
        vertex_size: u64,
    ) -> Result<MeshSlot, MeshPoolError> {
        self.pool.alloc(vert_bytes, idx_bytes, vertex_size)
    }

    /// Write into a previously-allocated slot's spans. The vertex bytes
    /// are written at `slot.vert_base_aligned`; index bytes at
    /// `slot.idx_offset`.
    ///
    /// Write mesh data into a pre-allocated slot.
    ///
    /// **Default mode (no staging)**: direct memcpy through the
    /// persistent map. Caller's bytes land in the user-visible
    /// HOST_COHERENT buffer immediately — the GPU sees them on the
    /// next draw.
    ///
    /// **DEVICE_LOCAL mode (staging is Some, v0.24.71)**: memcpys
    /// through `vertex_ptr`/`index_ptr` land in the STAGING buffer.
    /// A `vk::BufferCopy` region is appended to `pending_vert_copies`
    /// / `pending_idx_copies`. Caller MUST eventually call
    /// `flush_pending_copies(device, cmd)` before binding the
    /// device buffers for draws in that command buffer.
    ///
    /// SAFETY: caller guarantees the GPU isn't reading these regions
    /// concurrently (just-allocated chunks satisfy this trivially; for
    /// re-meshes the MAX_FRAMES_IN_FLIGHT defer-free pattern applies).
    /// Buffer lengths must fit within the slot.
    pub unsafe fn write_at(
        &mut self,
        slot: MeshSlot,
        vert_bytes: &[u8],
        idx_bytes: &[u8],
    ) {
        self.stage_memcpy(slot, vert_bytes, idx_bytes);

        // v0.24.71-stage3a: if in DEVICE_LOCAL mode, the memcpys above
        // landed in staging. Queue copy regions that flush_pending_copies
        // will record as vkCmdCopyBuffer ops.
        //
        // Identity-offset copy: the CpuMeshPool's slot offsets are
        // valid in both staging and device buffers (same allocator,
        // same size). src_offset == dst_offset.
        if let Some(s) = self.staging.as_mut() {
            if !vert_bytes.is_empty() {
                s.pending_vert_copies.push(vk::BufferCopy {
                    src_offset: slot.vert_base_aligned,
                    dst_offset: slot.vert_base_aligned,
                    size: vert_bytes.len() as u64,
                });
            }
            if !idx_bytes.is_empty() {
                s.pending_idx_copies.push(vk::BufferCopy {
                    src_offset: slot.idx_offset,
                    dst_offset: slot.idx_offset,
                    size: idx_bytes.len() as u64,
                });
            }
        }
    }

    /// v0.25-prefetch: deferrable counterpart to `write_at`. The staging
    /// memcpy happens immediately (same as `write_at`), but the GPU copy
    /// is queued in the *deferrable* lane, which `flush_pending_copies`
    /// drains under a per-frame byte budget and carries over.
    ///
    /// SAFETY (in addition to `write_at`'s): caller guarantees this slot
    /// will NOT be bound for drawing until a later `flush_pending_copies`
    /// has had the chance to drain its copy. This holds for prefetched
    /// chunks (meshed/uploaded ahead of the camera) but NOT for visible
    /// chunks — deferring a visible slot draws stale device memory for a
    /// frame. When in doubt, use `write_at`.
    pub unsafe fn write_at_deferrable(
        &mut self,
        slot: MeshSlot,
        vert_bytes: &[u8],
        idx_bytes: &[u8],
    ) {
        self.stage_memcpy(slot, vert_bytes, idx_bytes);
        if let Some(s) = self.staging.as_mut() {
            if !vert_bytes.is_empty() {
                s.pending_deferrable_vert_copies.push(vk::BufferCopy {
                    src_offset: slot.vert_base_aligned,
                    dst_offset: slot.vert_base_aligned,
                    size: vert_bytes.len() as u64,
                });
            }
            if !idx_bytes.is_empty() {
                s.pending_deferrable_idx_copies.push(vk::BufferCopy {
                    src_offset: slot.idx_offset,
                    dst_offset: slot.idx_offset,
                    size: idx_bytes.len() as u64,
                });
            }
        }
    }

    /// Shared staging memcpy for `write_at` / `write_at_deferrable`.
    /// In DEVICE_LOCAL mode these land in the staging buffer; in legacy
    /// mode they land directly in the host-visible device buffer. The
    /// copy-region bookkeeping (if any) is the caller's responsibility.
    #[inline]
    unsafe fn stage_memcpy(
        &mut self,
        slot: MeshSlot,
        vert_bytes: &[u8],
        idx_bytes: &[u8],
    ) {
        debug_assert!(slot.vert_base_aligned + vert_bytes.len() as u64
            <= slot.vert_offset + slot.vert_size);
        debug_assert!(slot.idx_offset + idx_bytes.len() as u64
            <= slot.idx_offset + slot.idx_size);
        std::ptr::copy_nonoverlapping(
            vert_bytes.as_ptr(),
            self.vertex_ptr.add(slot.vert_base_aligned as usize),
            vert_bytes.len(),
        );
        std::ptr::copy_nonoverlapping(
            idx_bytes.as_ptr(),
            self.index_ptr.add(slot.idx_offset as usize),
            idx_bytes.len(),
        );
    }

    /// v0.24.71-stage3a: drain pending copies into the given command
    /// buffer. Records:
    ///   1. `vkCmdCopyBuffer(staging_vertex → vertex_buffer, vert_regions)`
    ///   2. `vkCmdCopyBuffer(staging_index → index_buffer, idx_regions)`
    ///   3. `vk::BufferMemoryBarrier` transitioning both buffers from
    ///      `TRANSFER_WRITE` (pipeline TRANSFER) to
    ///      `VERTEX_ATTRIBUTE_READ | INDEX_READ` (pipeline VERTEX_INPUT).
    ///
    /// **No-op when not in DEVICE_LOCAL mode.** Safe to call
    /// unconditionally from the renderer.
    ///
    /// MUST be recorded in the command buffer BEFORE any
    /// `cmd_bind_vertex_buffers` / `cmd_bind_index_buffer` calls that
    /// reference `self.vertex_buffer` / `self.index_buffer`. If
    /// recorded after, the bound buffers will reflect stale (pre-copy)
    /// data for any slots written this frame.
    ///
    /// Pending lists are drained on success — subsequent calls are
    /// no-ops until more `write_at` calls add pending copies.
    pub unsafe fn flush_pending_copies(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
    ) {
        let Some(s) = self.staging.as_mut() else { return; };

        // v0.25-prefetch: fold a budgeted slice of deferrable (prefetch)
        // copies into this frame's flush, carrying the remainder to later
        // frames. Visible (`write_at`) copies always flush in full; only
        // the prefetch lane is spread. `budget == 0` flushes all deferrable
        // immediately (spreading disabled). The `spent > 0` guard
        // guarantees forward progress even if a single copy exceeds budget.
        {
            let budget = mesh_defer_budget_bytes();
            if budget == 0 {
                let mut dv = std::mem::take(&mut s.pending_deferrable_vert_copies);
                let mut di = std::mem::take(&mut s.pending_deferrable_idx_copies);
                s.pending_vert_copies.append(&mut dv);
                s.pending_idx_copies.append(&mut di);
            } else {
                // Drain a budgeted prefix of each deferrable lane, sharing one
                // byte budget across vertex→index (vertex first). The pure
                // `count_within_budget` (unit-tested in `budget_tests`) decides
                // how many; the carried remainder stays queued for next frame.
                let nv = count_within_budget(
                    s.pending_deferrable_vert_copies.iter().map(|c| c.size), budget, 0);
                let vert_bytes: u64 =
                    s.pending_deferrable_vert_copies[..nv].iter().map(|c| c.size).sum();
                let ni = count_within_budget(
                    s.pending_deferrable_idx_copies.iter().map(|c| c.size), budget, vert_bytes);

                for c in s.pending_deferrable_vert_copies.drain(..nv) {
                    s.pending_vert_copies.push(c);
                }
                for c in s.pending_deferrable_idx_copies.drain(..ni) {
                    s.pending_idx_copies.push(c);
                }
            }
        }

        if s.pending_vert_copies.is_empty() && s.pending_idx_copies.is_empty() {
            return;
        }

        // r730: coalesce contiguous copy regions before recording. Merges only runs
        // that are contiguous in BOTH staging (src) and device (dst) space, so the
        // bytes transferred and the written dst range (hence the barrier scope
        // below) are identical — just fewer regions for the driver to set up. Gated
        // (UNENDLICH_COPY_COALESCE=1) so the default path is bit-for-bit unchanged.
        if copy_coalesce_enabled() {
            coalesce_vk_copies(&mut s.pending_vert_copies);
            coalesce_vk_copies(&mut s.pending_idx_copies);
        }

        // Record copies. vkCmdCopyBuffer takes a slice of regions; we
        // can pass all pending copies in one call per buffer.
        if !s.pending_vert_copies.is_empty() {
            device.cmd_copy_buffer(
                cmd,
                s.staging_vertex_buffer,
                self.vertex_buffer,
                &s.pending_vert_copies,
            );
        }
        if !s.pending_idx_copies.is_empty() {
            device.cmd_copy_buffer(
                cmd,
                s.staging_index_buffer,
                self.index_buffer,
                &s.pending_idx_copies,
            );
        }

        // Memory barrier: the copies just wrote to vertex/index_buffer
        // at TRANSFER stage. Following draws read at VERTEX_INPUT
        // stage. Without this barrier, draws may see stale data.
        //
        // v0.24.NEXT+1 (barrier-tighten): scope the barrier to the
        // byte range actually written this frame (`bounded_copy_dst_range`)
        // instead of `vk::WHOLE_SIZE`. On drivers that track resource
        // state at sub-buffer granularity — notably Honeykrisp on
        // Apple Silicon AGX, which tracks per-page dirty state —
        // this lets more vertex-pipeline work overlap with prior
        // commands instead of stalling on the whole multi-GB buffer.
        // Drivers that flatten to whole-buffer state get the same
        // semantics they always had. Strict refinement: identical
        // access masks and pipeline stages, smaller scope.
        //
        // Each buffer's barrier is emitted only if that buffer had
        // pending copies — no point ordering a transfer that didn't
        // happen.
        let vert_range = bounded_copy_dst_range(&s.pending_vert_copies);
        let idx_range  = bounded_copy_dst_range(&s.pending_idx_copies);

        let mut barriers: Vec<vk::BufferMemoryBarrier> = Vec::with_capacity(2);
        if let Some((offset, size)) = vert_range {
            barriers.push(
                vk::BufferMemoryBarrier::builder()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::VERTEX_ATTRIBUTE_READ)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .buffer(self.vertex_buffer)
                    .offset(offset)
                    .size(size)
                    .build()
            );
        }
        if let Some((offset, size)) = idx_range {
            barriers.push(
                vk::BufferMemoryBarrier::builder()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::INDEX_READ)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .buffer(self.index_buffer)
                    .offset(offset)
                    .size(size)
                    .build()
            );
        }

        if !barriers.is_empty() {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::VERTEX_INPUT,
                vk::DependencyFlags::empty(),
                &[],
                &barriers,
                &[],
            );
        }

        s.pending_vert_copies.clear();
        s.pending_idx_copies.clear();
    }

    /// Returns true if this pool uses DEVICE_LOCAL backing
    /// (and consequently requires `flush_pending_copies` calls
    /// before draws each frame).
    pub fn is_device_local(&self) -> bool {
        self.staging.is_some()
    }

    /// Upload a mesh into the pool. Allocates spans, memcpys both
    /// payloads via the persistent map, returns a `MeshSlot`.
    ///
    /// `vertex_size` is the size in bytes of a vertex (e.g., 24 for the
    /// engine's `Vertex` struct). Used to align the vertex base to a
    /// vertex boundary inside the (256-byte-aligned) span — necessary
    /// for the indirect-draw path where the GPU references vertices by
    /// integer index. Pass `1` if alignment isn't needed.
    ///
    /// SAFETY: caller must ensure the returned slot's spans are not in
    /// use by an in-flight GPU draw.
    pub unsafe fn upload(
        &mut self,
        vert_bytes: &[u8],
        idx_bytes: &[u8],
        vertex_size: u64,
    ) -> Result<MeshSlot, MeshPoolError> {
        let slot = self.pool.alloc(
            vert_bytes.len() as u64,
            idx_bytes.len() as u64,
            vertex_size,
        )?;
        // v0.24.71-stage3a: delegate to write_at so the staging path
        // (if active) is hit. Previously this duplicated the memcpy
        // logic inline.
        self.write_at(slot, vert_bytes, idx_bytes);
        Ok(slot)
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device) {
        // v0.24.71-stage3a: destroy staging first if present. Staging
        // buffers are HOST_VISIBLE — they own the persistent map.
        if let Some(mut s) = self.staging.take() {
            device.unmap_memory(s.staging_vertex_memory);
            device.destroy_buffer(s.staging_vertex_buffer, None);
            device.free_memory(s.staging_vertex_memory, None);

            device.unmap_memory(s.staging_index_memory);
            device.destroy_buffer(s.staging_index_buffer, None);
            device.free_memory(s.staging_index_memory, None);

            s.staging_vertex_buffer = vk::Buffer::null();
            s.staging_index_buffer = vk::Buffer::null();

            // DEVICE_LOCAL buffers are NOT mapped; only destroy the
            // buffers + free the memory.
            device.destroy_buffer(self.vertex_buffer, None);
            device.free_memory(self.vertex_memory, None);
            device.destroy_buffer(self.index_buffer, None);
            device.free_memory(self.index_memory, None);
        } else {
            // Legacy HOST_COHERENT path — buffers ARE the mapped ones.
            device.unmap_memory(self.vertex_memory);
            device.destroy_buffer(self.vertex_buffer, None);
            device.free_memory(self.vertex_memory, None);

            device.unmap_memory(self.index_memory);
            device.destroy_buffer(self.index_buffer, None);
            device.free_memory(self.index_memory, None);
        }

        self.vertex_buffer = vk::Buffer::null();
        self.vertex_memory = vk::DeviceMemory::null();
        self.index_buffer = vk::Buffer::null();
        self.index_memory = vk::DeviceMemory::null();
        self.vertex_ptr = std::ptr::null_mut();
        self.index_ptr = std::ptr::null_mut();
    }
}

/// r707: is there a memory type exposing DEVICE_LOCAL | HOST_VISIBLE |
/// HOST_COHERENT (ReBAR / Resizable BAR, or unified-memory iGPUs)? If so the mesh
/// vertex/index buffers can be zero-copy device-local and the staging path is
/// pure overhead. Coarse probe over the physical device's memory types; if it says
/// yes, `create_persistent_buffer`'s per-buffer selection will use it (and still
/// falls back safely if a specific buffer's `memory_type_bits` excludes it).
fn rebar_available(instance: &ash::Instance, physical_device: vk::PhysicalDevice) -> bool {
    let props = unsafe { instance.get_physical_device_memory_properties(physical_device) };
    let want = vk::MemoryPropertyFlags::DEVICE_LOCAL
        | vk::MemoryPropertyFlags::HOST_VISIBLE
        | vk::MemoryPropertyFlags::HOST_COHERENT;
    (0..props.memory_type_count)
        .any(|i| props.memory_types[i as usize].property_flags.contains(want))
}

unsafe fn create_persistent_buffer(
    device: &ash::Device,
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    size: u64,
    usage: vk::BufferUsageFlags,
) -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8), MeshPoolError> {
    let info = vk::BufferCreateInfo::builder()
        .size(size)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = device.create_buffer(&info, None)
        .map_err(|e| MeshPoolError::Backend(format!("vkCreateBuffer: {:?}", e)))?;

    let req = device.get_buffer_memory_requirements(buffer);
    let props = instance.get_physical_device_memory_properties(physical_device);

    // r704: ReBAR / zero-copy memory preference. Prefer
    // DEVICE_LOCAL | HOST_VISIBLE | HOST_COHERENT: on Resizable-BAR systems and
    // unified-memory iGPUs this is memory the CPU memcpys into directly AND the
    // GPU reads at full device-local speed — no staging buffer, no vkCmdCopyBuffer,
    // no barrier, no PCIe read penalty. That is the true zero-copy mesh upload:
    // `write_at` lands the packed vertices straight in the buffer the shader
    // fetches from. Falls back to plain HOST_VISIBLE | HOST_COHERENT when the ReBAR
    // type is absent OR its allocation fails (e.g. the small BAR window on an older
    // discrete GPU can't fit the buffer) — byte-for-byte the previous behaviour.
    // Opt out with UNENDLICH_NO_REBAR=1.
    let allow_rebar = std::env::var("UNENDLICH_NO_REBAR").is_err();
    let host = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
    let rebar = host | vk::MemoryPropertyFlags::DEVICE_LOCAL;
    let find_type = |required: vk::MemoryPropertyFlags| -> Option<u32> {
        (0..props.memory_type_count).find(|&i| {
            (req.memory_type_bits & (1u32 << i)) != 0
                && props.memory_types[i as usize].property_flags.contains(required)
        })
    };
    // Preference order: ReBAR (device-local + host-visible) first when allowed,
    // then plain host-visible. Bound to a local (not a borrowed temporary) so the
    // slice iterator below is valid.
    let candidates = [(rebar, true, allow_rebar), (host, false, true)];
    let mut memory = None;
    let mut used_rebar = false;
    for &(required, is_rebar, enabled) in candidates.iter() {
        if !enabled {
            continue;
        }
        if let Some(mt) = find_type(required) {
            let alloc = vk::MemoryAllocateInfo::builder()
                .allocation_size(req.size)
                .memory_type_index(mt);
            if let Ok(m) = device.allocate_memory(&alloc, None) {
                memory = Some(m);
                used_rebar = is_rebar;
                break;
            }
        }
    }
    let memory = memory.ok_or_else(||
        MeshPoolError::Backend("No usable HOST_VISIBLE | HOST_COHERENT memory type".into()))?;
    if used_rebar {
        static ANNOUNCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !ANNOUNCED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            log::info!(
                "mesh_pool: ReBAR zero-copy mesh memory ACTIVE (DEVICE_LOCAL | HOST_VISIBLE | \
                 HOST_COHERENT) — write_at memcpys straight into device-local memory, no staging/copy \
                 (UNENDLICH_NO_REBAR=1 to disable)"
            );
        }
    }
    device.bind_buffer_memory(buffer, memory, 0)
        .map_err(|e| MeshPoolError::Backend(format!("vkBindBufferMemory: {:?}", e)))?;

    let ptr = device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
        .map_err(|e| MeshPoolError::Backend(format!("vkMapMemory: {:?}", e)))? as *mut u8;

    Ok((buffer, memory, ptr))
}

/// v0.24.71-stage3a: allocate a buffer in DEVICE_LOCAL memory.
///
/// Companion to `create_persistent_buffer`. Differences:
///   - Selects DEVICE_LOCAL memory type (prefers DEVICE_LOCAL-only;
///     accepts DEVICE_LOCAL | HOST_VISIBLE on unified-memory devices
///     like Apple Silicon where both flags coexist).
///   - Does NOT map the memory (DEVICE_LOCAL-only memory may not
///     even be HOST_VISIBLE; mapping would fail).
///   - Returns `(buffer, memory)` instead of `(buffer, memory, ptr)`.
///
/// On systems with a true DEVICE_LOCAL-only memory type (typical
/// desktop GPUs with VRAM), this picks that memory. On unified-memory
/// devices, we accept DEVICE_LOCAL even if it also has HOST_VISIBLE
/// (the driver may expose only such types, and DEVICE_LOCAL-as-hint
/// still signals "prefer this for GPU reads" even when physically the
/// same memory).
unsafe fn create_device_local_buffer(
    device: &ash::Device,
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    size: u64,
    usage: vk::BufferUsageFlags,
) -> Result<(vk::Buffer, vk::DeviceMemory), MeshPoolError> {
    let info = vk::BufferCreateInfo::builder()
        .size(size)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = device.create_buffer(&info, None)
        .map_err(|e| MeshPoolError::Backend(format!("vkCreateBuffer: {:?}", e)))?;

    let req = device.get_buffer_memory_requirements(buffer);
    let props = instance.get_physical_device_memory_properties(physical_device);

    // Selection: first pass prefers DEVICE_LOCAL-only (no HOST_VISIBLE).
    // Second pass accepts DEVICE_LOCAL with HOST_VISIBLE (unified-memory
    // hardware). We don't map either way — the staging buffer is the
    // CPU-visible side.
    let mut chosen = None;
    for i in 0..props.memory_type_count {
        let mt = props.memory_types[i as usize];
        if req.memory_type_bits & (1 << i) != 0
            && mt.property_flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            && !mt.property_flags.contains(vk::MemoryPropertyFlags::HOST_VISIBLE)
        {
            chosen = Some(i);
            break;
        }
    }
    if chosen.is_none() {
        for i in 0..props.memory_type_count {
            let mt = props.memory_types[i as usize];
            if req.memory_type_bits & (1 << i) != 0
                && mt.property_flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            {
                chosen = Some(i);
                break;
            }
        }
    }
    let mt = chosen.ok_or_else(||
        MeshPoolError::Backend("No DEVICE_LOCAL memory type".into()))?;

    let alloc = vk::MemoryAllocateInfo::builder()
        .allocation_size(req.size)
        .memory_type_index(mt);
    let memory = device.allocate_memory(&alloc, None)
        .map_err(|e| MeshPoolError::Backend(format!("vkAllocateMemory (device-local): {:?}", e)))?;
    device.bind_buffer_memory(buffer, memory, 0)
        .map_err(|e| MeshPoolError::Backend(format!("vkBindBufferMemory (device-local): {:?}", e)))?;

    Ok((buffer, memory))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_pool() -> CpuMeshPool {
        CpuMeshPool::new(1024 * 1024, 512 * 1024)
    }

    #[test]
    fn fresh_pool_is_empty() {
        let p = small_pool();
        assert_eq!(p.slot_count(), 0);
        assert_eq!(p.vert_allocated_bytes(), 0);
        assert_eq!(p.idx_allocated_bytes(), 0);
    }

    #[test]
    fn alloc_returns_offsets_within_buffers() {
        let mut p = small_pool();
        let s = p.alloc(10000, 5000, 1).unwrap();
        assert!(s.vert_offset + s.vert_size <= p.vert_buffer_size());
        assert!(s.idx_offset + s.idx_size <= p.idx_buffer_size());
        assert!(s.vert_size >= 10000);
        assert!(s.idx_size >= 5000);
        assert_eq!(p.slot_count(), 1);
    }

    #[test]
    fn alloc_rounds_up_to_alignment() {
        let mut p = small_pool();
        let s = p.alloc(100, 100, 1).unwrap();
        assert_eq!(s.vert_size, 256);
        assert_eq!(s.idx_size, 256);
    }

    #[test]
    fn free_releases_spans() {
        let mut p = small_pool();
        let s = p.alloc(10000, 5000, 1).unwrap();
        p.free(s).unwrap();
        assert_eq!(p.slot_count(), 0);
        assert_eq!(p.vert_allocated_bytes(), 0);
        assert_eq!(p.idx_allocated_bytes(), 0);
    }

    #[test]
    fn double_free_rejected() {
        let mut p = small_pool();
        let s = p.alloc(1000, 500, 1).unwrap();
        p.free(s).unwrap();
        assert!(matches!(p.free(s), Err(MeshPoolError::InvalidSlot)));
    }

    #[test]
    fn stale_version_handle_rejected() {
        let mut p = small_pool();
        let s_old = p.alloc(1000, 500, 1).unwrap();
        p.free(s_old).unwrap();
        let s_new = p.alloc(1000, 500, 1).unwrap();
        if s_new.vert_offset == s_old.vert_offset {
            assert_ne!(s_new.version, s_old.version);
            assert!(matches!(p.free(s_old), Err(MeshPoolError::InvalidSlot)));
        }
    }

    #[test]
    fn exhaustion_returns_err() {
        let mut p = CpuMeshPool::new(8192, 8192);
        let _s1 = p.alloc(7000, 1000, 1).unwrap();
        assert!(matches!(p.alloc(2000, 1000, 1),
            Err(MeshPoolError::Alloc("vertex", _))));
    }

    #[test]
    fn vertex_alignment_packed_makes_base_divisible() {
        // The indirect-draw path requires `vert_base_aligned %
        // sizeof(PackedVertex)` == 0 so the chunk's vertex base
        // index can be packed into the indirect command's
        // `vertexOffset` as an integer count of vertices.
        // Allocator alignment is 256, sizeof(PackedVertex) = 8.
        // 256 % 8 = 0, so vert_offset is ALWAYS on a PackedVertex
        // boundary already — vert_base_aligned == vert_offset in
        // practice. The test still exercises the alignment-rounding
        // logic in case the underlying alignment or stride changes.
        //
        // (Pre-v0.24.20 this test used VERTEX_SIZE=24, where 256%24=16
        // forces vert_base_aligned > vert_offset on most allocations.)
        const VERTEX_SIZE: u64 = 8;
        let mut p = CpuMeshPool::new(64 * 1024 * 1024, 32 * 1024 * 1024);

        // Try a bunch of sizes whose 256-aligned offsets exercise the
        // misaligned cases: small allocs leave odd offsets behind.
        let sizes = [100u64, 500, 1000, 7000, 50_000, 100_000];
        let mut alloced = Vec::new();
        for &v in &sizes {
            let s = p.alloc(v, v / 2, VERTEX_SIZE).unwrap();
            assert_eq!(s.vert_base_aligned % VERTEX_SIZE, 0,
                "vert_base_aligned {} is not a multiple of {} \
                 (vert_offset={}, alloc size={})",
                s.vert_base_aligned, VERTEX_SIZE, s.vert_offset, v);
            // The aligned base must be at-or-after the raw offset, AND
            // the data fits in the span.
            assert!(s.vert_base_aligned >= s.vert_offset);
            assert!(s.vert_base_aligned + v <= s.vert_offset + s.vert_size,
                "data of {} bytes doesn't fit at aligned base {} in span [{}, {})",
                v, s.vert_base_aligned, s.vert_offset, s.vert_offset + s.vert_size);
            alloced.push(s);
        }

        // Free everything.
        for s in alloced { p.free(s).unwrap(); }
    }

    #[test]
    fn vertex_alignment_1_skips_alignment() {
        // Pass-through alignment for callers that don't need it.
        let mut p = CpuMeshPool::new(8192, 8192);
        let s = p.alloc(1000, 500, 1).unwrap();
        assert_eq!(s.vert_base_aligned, s.vert_offset);
    }

    #[test]
    fn vertex_alloc_rolls_back_when_index_fails() {
        let mut p = CpuMeshPool::new(1024 * 1024, 4096);
        let _s = p.alloc(100, 3000, 1).unwrap();
        let v_used = p.vert_allocated_bytes();
        assert!(matches!(p.alloc(100, 2000, 1),
            Err(MeshPoolError::Alloc("index", _))));
        assert_eq!(p.vert_allocated_bytes(), v_used,
            "vertex span leaked after index alloc failure");
    }

    #[test]
    fn realistic_workload_fits() {
        let mut p = CpuMeshPool::new(DEFAULT_VERT_BUFFER_SIZE, DEFAULT_INDEX_BUFFER_SIZE);
        let mut rng = 0xCAFEu32;
        let next = |rng: &mut u32| {
            *rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            *rng
        };
        let mut allocs = Vec::new();
        for _ in 0..925 {
            let r = next(&mut rng) % 100;
            let v = if r < 25 { 50_000u64 }
                    else if r < 75 { 700_000 }
                    else if r < 95 { 3_000_000 }
                    else { 6_000_000 };
            let i = (v * 45) / 100;
            match p.alloc(v, i, 1) {
                Ok(s) => allocs.push(s),
                Err(e) => panic!("pool exhausted at slot {}: {}", allocs.len(), e),
            }
        }
        assert_eq!(p.slot_count(), 925);
        assert!(p.vert_allocated_bytes() < p.vert_buffer_size());
    }

    #[test]
    fn churn_pattern_no_unbounded_growth() {
        // 64 MB vertex / 32 MB index. Round-by-round we either alloc OR
        // free with roughly equal probability. The test isn't checking
        // exhaustion (which is a sizing question, not a fragmentation
        // one); it's checking that under sustained alloc/free churn the
        // free-list stays coalesced enough to keep serving.
        let mut p = CpuMeshPool::new(64 * 1024 * 1024, 32 * 1024 * 1024);
        let mut live = Vec::new();
        let mut rng = 0xBEEFu32;
        let next = |rng: &mut u32| {
            *rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            *rng
        };
        let mut rejections = 0;
        for _round in 0..2000u64 {
            let do_alloc = (next(&mut rng) & 1) == 1 || live.is_empty();
            if do_alloc {
                let v: u64 = 50_000 + (next(&mut rng) % 3_000_000) as u64;
                let i = v / 2;
                match p.alloc(v, i, 1) {
                    Ok(s) => live.push(s),
                    Err(_) => rejections += 1,
                }
            } else {
                let idx = (next(&mut rng) as usize) % live.len();
                let s = live.swap_remove(idx);
                p.free(s).unwrap();
            }
        }
        for s in live {
            p.free(s).unwrap();
        }
        assert_eq!(p.slot_count(), 0);
        assert_eq!(p.vert_allocated_bytes(), 0);
        // Under balanced churn we should rarely fail. Hitting > 100/1000
        // attempted allocs is a fragmentation red flag.
        assert!(rejections < 100,
            "too many rejections under balanced churn ({}/2000)", rejections);
    }

    /// Reports actual mesh sizes from the CPU mesher against the
    /// sub-allocator pool. Replaces v0.18.3's
    /// `mesh_size_against_slot_caps`; the metaphor is now total pool
    /// utilization rather than per-slot fit.
    ///
    ///     cargo test --features render mesh_size_against_pool -- --nocapture
    #[test]
    fn mesh_size_against_pool() {
        use crate::render::mesher::mesh_chunk;
        use crate::world::chunk::ChunkCoord;
        use crate::world::cpu_generator::WorldGenerator;

        let gen = WorldGenerator::new(0xCAFEBABE);

        let sample_coords = [
            ChunkCoord { x: 0,  y: 4,  z: 0  },
            ChunkCoord { x: 0,  y: 2,  z: 0  },
            ChunkCoord { x: -8, y: 2,  z: 0  },
            ChunkCoord { x: -8, y: 1,  z: -1 },
            ChunkCoord { x: -6, y: 2,  z: 1  },
            ChunkCoord { x: -5, y: 2,  z: 1  },
            ChunkCoord { x: 3,  y: 0,  z: 3  },
            ChunkCoord { x: 0,  y: -2, z: 0  },
        ];

        let mut vert_sizes = Vec::with_capacity(sample_coords.len());
        let mut idx_sizes = Vec::with_capacity(sample_coords.len());
        for &coord in &sample_coords {
            let chunk = gen.generate_chunk(coord);
            let mesh = mesh_chunk(&chunk);
            let v = mesh.vertices.len() * std::mem::size_of::<crate::render::mesher::Vertex>();
            let i = mesh.indices.len() * std::mem::size_of::<u32>();
            vert_sizes.push(v as u64);
            idx_sizes.push(i as u64);
        }

        let mut pool = CpuMeshPool::new(DEFAULT_VERT_BUFFER_SIZE, DEFAULT_INDEX_BUFFER_SIZE);
        let mut fit = 0;
        let mut overflows = 0;
        for (v, i) in vert_sizes.iter().zip(idx_sizes.iter()) {
            match pool.alloc(*v, *i, 1) {
                Ok(_) => fit += 1,
                Err(_) => overflows += 1,
            }
        }

        let v_max: u64 = *vert_sizes.iter().max().unwrap();
        let i_max: u64 = *idx_sizes.iter().max().unwrap();
        let v_total: u64 = vert_sizes.iter().sum();
        let i_total: u64 = idx_sizes.iter().sum();

        println!();
        println!("=== Mesh size report (v0.19 / sub-allocator pool) ===");
        println!("  Sample: {} chunks, seed 0xCAFEBABE", sample_coords.len());
        println!("  Vertex max: {}, total: {}",
                 fmt_bytes(v_max), fmt_bytes(v_total));
        println!("  Index  max: {}, total: {}",
                 fmt_bytes(i_max), fmt_bytes(i_total));
        println!("  Pool: vert {}, idx {}",
                 fmt_bytes(DEFAULT_VERT_BUFFER_SIZE),
                 fmt_bytes(DEFAULT_INDEX_BUFFER_SIZE));
        println!("  Result: {}/{} fit, {} overflows",
                 fit, sample_coords.len(), overflows);

        if overflows > 0 {
            println!(
                "  WARNING: pool exhausted on test sample. Either DEFAULT_*_BUFFER_SIZE \
                 is too small or fragmentation is bad."
            );
        } else {
            println!("  Pool sizing OK for this sample. All chunks fit with room.");
        }
        println!();
    }

    fn fmt_bytes(n: u64) -> String {
        if n >= 1024 * 1024 * 1024 {
            format!("{:.2} GB", n as f64 / (1024.0 * 1024.0 * 1024.0))
        } else if n >= 1024 * 1024 {
            format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
        } else if n >= 1024 {
            format!("{:.1} KB", n as f64 / 1024.0)
        } else {
            format!("{} B", n)
        }
    }

    // ─── v0.24.NEXT+1: bounded_copy_dst_range tests ───────────────
    //
    // The helper feeds the post-copy barrier in flush_pending_copies.
    // A wrong range = a wrong barrier scope = potential vertex-data
    // visibility bug. These tests pin the math exactly.

    fn bc(dst_offset: u64, size: u64) -> vk::BufferCopy {
        vk::BufferCopy { src_offset: 0, dst_offset, size }
    }

    #[test]
    fn bounded_copy_dst_range_empty_returns_none() {
        let copies: &[vk::BufferCopy] = &[];
        assert_eq!(bounded_copy_dst_range(copies), None);
    }

    #[test]
    fn bounded_copy_dst_range_single_copy_trivial() {
        let copies = [bc(100, 50)];
        assert_eq!(bounded_copy_dst_range(&copies), Some((100, 50)));
    }

    #[test]
    fn bounded_copy_dst_range_multi_contiguous() {
        // Three contiguous writes starting at 1000.
        let copies = [bc(1000, 200), bc(1200, 200), bc(1400, 200)];
        assert_eq!(bounded_copy_dst_range(&copies), Some((1000, 600)));
    }

    #[test]
    fn bounded_copy_dst_range_multi_scattered() {
        // Scattered writes — verify min/max are picked correctly
        // regardless of input ordering.
        let copies = [bc(5000, 100), bc(100, 50), bc(2000, 200)];
        // lo = 100, hi = 5100, size = 5000
        assert_eq!(bounded_copy_dst_range(&copies), Some((100, 5000)));
    }

    #[test]
    fn bounded_copy_dst_range_overlapping_copies_handled() {
        // Pathological but legal: regions overlap.
        let copies = [bc(100, 500), bc(300, 500)];
        // lo = 100, hi = 800, size = 700
        assert_eq!(bounded_copy_dst_range(&copies), Some((100, 700)));
    }
}

#[cfg(all(test, feature = "headless"))]
mod deferrable_runtime {
    use super::*;
    use crate::render::context::VulkanContext;

    /// Runtime proof on a real (lavapipe) device: visible uploads flush in full
    /// immediately, prefetch (deferrable) uploads spread under the byte budget
    /// and carry the remainder to later frames.
    /// Run: VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json
    #[test]
    fn deferrable_lane_spreads_under_budget_on_device() {
        std::env::set_var("UNENDLICH_DEVICE_LOCAL_MESH", "1"); // staging path -> lane active
        let ctx = VulkanContext::new_headless().expect("headless ctx");
        unsafe {
            let mut mp = VulkanMeshPool::new(&ctx.device, &ctx.instance, ctx.physical_device)
                .expect("mesh pool");

            let vbytes = vec![0u8; 256];
            let ibytes = vec![0u8; 64];
            // 4 prefetch (deferrable) chunk uploads.
            for _ in 0..4 {
                let slot = mp.alloc(256, 64, 16).expect("slot");
                mp.write_at_deferrable(slot, &vbytes, &ibytes);
            }
            // 1 visible upload (must always flush this frame).
            let vslot = mp.alloc(256, 64, 16).expect("vslot");
            mp.write_at(vslot, &vbytes, &ibytes);

            {
                let s = mp.staging.as_ref().expect("staging active under DEVICE_LOCAL_MESH=1");
                assert_eq!(s.pending_deferrable_vert_copies.len(), 4, "4 deferrable queued");
                assert_eq!(s.pending_vert_copies.len(), 1, "1 visible queued");
            }

            std::env::set_var("UNENDLICH_MESH_DEFER_BUDGET_BYTES", "300"); // ~one 256B copy/flush

            let cmd_pool = ctx.device.create_command_pool(
                &vk::CommandPoolCreateInfo::builder().queue_family_index(ctx.graphics_queue_family),
                None).unwrap();
            let cmd = ctx.device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::builder()
                    .command_pool(cmd_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1)).unwrap()[0];
            ctx.device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::builder()).unwrap();
            mp.flush_pending_copies(&ctx.device, cmd);   // records real vkCmdCopyBuffer + barrier
            ctx.device.end_command_buffer(cmd).unwrap();

            {
                let s = mp.staging.as_ref().unwrap();
                assert_eq!(s.pending_deferrable_vert_copies.len(), 3,
                    "budget carries 3 of 4 deferrable copies to later frames");
                assert!(s.pending_vert_copies.is_empty(),
                    "visible + budgeted-deferrable copies cleared after recording");
            }

            ctx.device.device_wait_idle().ok();
            ctx.device.destroy_command_pool(cmd_pool, None);
        }
        std::env::remove_var("UNENDLICH_MESH_DEFER_BUDGET_BYTES");
        std::env::remove_var("UNENDLICH_DEVICE_LOCAL_MESH");
    }
}

#[cfg(all(test, feature = "headless"))]
mod deferrable_runtime_drain {
    use super::*;
    use crate::render::context::VulkanContext;

    /// Stronger runtime proof on lavapipe: the deferrable backlog is SUBMITTED
    /// to the graphics queue frame-by-frame and FULLY drains over several
    /// frames — i.e. the recorded copies are valid+executable on a real device,
    /// and the budgeted queue never stalls or leaks a carried copy.
    #[test]
    fn deferrable_backlog_submits_and_fully_drains() {
        std::env::set_var("UNENDLICH_DEVICE_LOCAL_MESH", "1");
        let ctx = VulkanContext::new_headless().expect("headless ctx");
        unsafe {
            let mut mp = VulkanMeshPool::new(&ctx.device, &ctx.instance, ctx.physical_device)
                .expect("mesh pool");
            let vbytes = vec![0u8; 256];
            let ibytes = vec![0u8; 64];
            for _ in 0..4 {
                let slot = mp.alloc(256, 64, 16).expect("slot");
                mp.write_at_deferrable(slot, &vbytes, &ibytes);
            }
            // Budget drains ~2 of the 256B vertex copies per frame, so the
            // 4-chunk backlog must take multiple frames to clear.
            std::env::set_var("UNENDLICH_MESH_DEFER_BUDGET_BYTES", "600");

            let cmd_pool = ctx.device.create_command_pool(
                &vk::CommandPoolCreateInfo::builder()
                    .queue_family_index(ctx.graphics_queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None).unwrap();
            let cmd = ctx.device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::builder()
                    .command_pool(cmd_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1)).unwrap()[0];

            let pending = |mp: &VulkanMeshPool| {
                let s = mp.staging.as_ref().unwrap();
                s.pending_deferrable_vert_copies.len() + s.pending_deferrable_idx_copies.len()
            };

            let mut frames = 0;
            while pending(&mp) > 0 && frames < 32 {
                ctx.device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty()).unwrap();
                ctx.device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::builder()).unwrap();
                mp.flush_pending_copies(&ctx.device, cmd);
                ctx.device.end_command_buffer(cmd).unwrap();
                let cmds = [cmd];
                let submit = vk::SubmitInfo::builder().command_buffers(&cmds).build();
                ctx.device.queue_submit(ctx.graphics_queue, &[submit], vk::Fence::null()).unwrap();
                ctx.device.queue_wait_idle(ctx.graphics_queue).unwrap(); // executes on-device
                frames += 1;
            }

            assert_eq!(pending(&mp), 0, "backlog must fully drain (no leak/stall)");
            assert!(frames >= 2, "spread across frames, not one big stall (got {frames})");
            assert!(frames < 32, "must terminate / not deadlock");

            ctx.device.device_wait_idle().ok();
            ctx.device.destroy_command_pool(cmd_pool, None);
        }
        std::env::remove_var("UNENDLICH_MESH_DEFER_BUDGET_BYTES");
        std::env::remove_var("UNENDLICH_DEVICE_LOCAL_MESH");
    }
}
