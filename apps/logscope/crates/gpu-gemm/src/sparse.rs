//! sparse.rs — sparse-residency planning + (gated) binding, the architectural replacement for the slab arena.
//!
//! Honeykrisp advertises `sparseResidencyBuffer` with a 512 GiB sparse address space. That lets us create
//! ONE virtual buffer spanning all the resident weights and back it with physical memory blocks bound via
//! `vkQueueBindSparse` — instead of ~20 separate ≤2 GiB slab buffers. Why it's the right architecture:
//!   * One buffer, global offsets — tensors already bind as (offset,len) VIEWS (see `Tensor.view`), so a
//!     sparse tensor is just a view into the single buffer; nothing in the kernels changes.
//!   * No VA-doubling — physical blocks are bound as we go, so the load peak is the resident size, not 2×
//!     (the staging pile-up that crashed the 70B before the per-slab drain).
//!   * Rebindable — the same mechanism that pages weights IN can page them OUT, which is the clean
//!     substrate for layer streaming later (bind the working set, unbind the rest).
//!
//! The per-binding 2 GiB `maxStorageBufferRange` limit is a non-issue: the BUFFER is huge but each TENSOR
//! binds a ≤2 GiB slice (the biggest 70B weight is ~117 MB), so bindings stay tiny.
//!
//! This module is the pure planner (always compiled, tested) + a gated unsafe binder (compile-checked
//! against wgpu-hal/ash, draft-until-hardware) behind the `sparse` feature.

const GIB: f64 = (1u64 << 30) as f64;
fn gib(b: u64) -> f64 { b as f64 / GIB }

/// How to lay a model's resident weights into one sparse buffer backed by physical memory blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SparsePlan {
    pub buffer_bytes: u64,   // virtual size of the single sparse buffer (weights rounded up to block granularity)
    pub block_bytes: u64,    // size of each physical VkDeviceMemory block bound into the buffer
    pub block_count: u32,    // number of physical blocks (must be ≤ maxMemoryAllocationCount)
    pub sparse_align: u64,   // the sparse block granularity all sizes/offsets must be multiples of
}
impl SparsePlan {
    /// The physical memory actually committed (block_count × block_bytes) — may slightly exceed the weight
    /// bytes because the tail block is whole. This is what counts against the GPU's memory, not the VA size.
    pub fn committed_bytes(&self) -> u64 { self.block_count as u64 * self.block_bytes }
}

/// Why a sparse layout can't be planned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SparseError {
    TooManyBlocks { needed: u32, max: u32 }, // weights ÷ block size exceeds maxMemoryAllocationCount
    ZeroInput,
}

fn round_up(n: u64, a: u64) -> u64 { let a = a.max(1); (n + a - 1) / a * a }

/// Plan the sparse layout for `weight_bytes` of resident weights. `block_bytes` is the desired physical
/// block size (bigger = fewer allocations; rounded up to `sparse_align`). `max_alloc_count` is the driver's
/// `maxMemoryAllocationCount` (4096 on Honeykrisp) — the plan keeps block_count under it. The virtual buffer
/// is the weights rounded up to a whole number of blocks.
pub fn plan_sparse(weight_bytes: u64, block_bytes: u64, sparse_align: u64, max_alloc_count: u32) -> Result<SparsePlan, SparseError> {
    if weight_bytes == 0 { return Err(SparseError::ZeroInput); }
    let align = sparse_align.max(1);
    let block = round_up(block_bytes.max(align), align);
    let block_count_u64 = (weight_bytes + block - 1) / block;
    if block_count_u64 > max_alloc_count as u64 {
        // too many blocks at this size — the caller should raise block_bytes and retry
        return Err(SparseError::TooManyBlocks { needed: block_count_u64 as u32, max: max_alloc_count });
    }
    let block_count = block_count_u64 as u32;
    Ok(SparsePlan {
        buffer_bytes: block_count as u64 * block, // VA rounded to whole blocks
        block_bytes: block,
        block_count,
        sparse_align: align,
    })
}

/// Choose a physical block size so the block_count fits under `max_alloc_count`, starting from `preferred`
/// and doubling until it fits. Returns a plan that always fits (blocks grow until they do). This is what the
/// loader calls — it never fails for a model that fits in memory at all.
pub fn plan_sparse_auto(weight_bytes: u64, preferred_block: u64, sparse_align: u64, max_alloc_count: u32) -> SparsePlan {
    let mut block = preferred_block.max(sparse_align.max(1));
    loop {
        match plan_sparse(weight_bytes, block, sparse_align, max_alloc_count) {
            Ok(p) => return p,
            Err(_) => block = block.saturating_mul(2),
        }
    }
}

/// Which physical block a given weight offset falls in, and the offset within that block — the mapping the
/// binder uses to know which VkDeviceMemory backs a byte range. (Weights are contiguous in the VA, so this
/// is just division; sparse binding maps block B to VA range [B·block, (B+1)·block).)
pub fn block_of(offset: u64, block_bytes: u64) -> (u32, u64) {
    let b = block_bytes.max(1);
    ((offset / b) as u32, offset % b)
}

/// A human summary for the loader log.
pub fn describe(p: &SparsePlan) -> String {
    format!("sparse buffer {:.1} GiB VA · {} physical blocks × {:.2} GiB (committed {:.1} GiB, align {} KiB)",
        gib(p.buffer_bytes), p.block_count, gib(p.block_bytes), gib(p.committed_bytes()), p.sparse_align / 1024)
}

#[cfg(feature = "sparse")]
pub mod bind;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_the_70b_into_few_big_blocks() {
        // 40.3 GiB of q4 weights, 8 GiB blocks, 64 KiB sparse granularity, 4096 max allocs (Honeykrisp)
        let weights = (40.3 * GIB) as u64;
        let p = plan_sparse(weights, (8.0 * GIB) as u64, 64 * 1024, 4096).unwrap();
        assert_eq!(p.block_count, 6, "40.3 GiB ÷ 8 GiB → 6 blocks");
        assert!(p.buffer_bytes >= weights, "the VA covers all the weights");
        assert!(p.committed_bytes() >= weights);
        assert_eq!(p.block_bytes % (64 * 1024), 0, "block size is a multiple of sparse granularity");
        // far fewer allocations than the driver's 4096 ceiling — nowhere near the allocation-count wall
        assert!(p.block_count < 4096);
    }

    #[test]
    fn too_many_small_blocks_is_rejected_then_auto_grows() {
        let weights = (40.0 * GIB) as u64;
        // 4 MiB blocks → ~10240 blocks > 4096 → rejected
        assert!(matches!(plan_sparse(weights, 4 << 20, 64 * 1024, 4096), Err(SparseError::TooManyBlocks { .. })));
        // auto picks a block size that fits under the ceiling
        let p = plan_sparse_auto(weights, 4 << 20, 64 * 1024, 4096);
        assert!(p.block_count <= 4096, "auto grows blocks until they fit: {}", p.block_count);
        assert!(p.committed_bytes() >= weights);
    }

    #[test]
    fn block_mapping_is_contiguous_division() {
        let block = 8u64 << 30; // 8 GiB
        assert_eq!(block_of(0, block), (0, 0));
        assert_eq!(block_of(block - 1, block), (0, block - 1));
        assert_eq!(block_of(block, block), (1, 0));       // first byte of block 1
        assert_eq!(block_of(block + 100, block), (1, 100));
    }

    #[test]
    fn zero_and_alignment_edges() {
        assert_eq!(plan_sparse(0, 1 << 30, 64 * 1024, 4096), Err(SparseError::ZeroInput));
        // a sub-granularity request still yields at least one whole aligned block
        let p = plan_sparse(1000, 1000, 64 * 1024, 4096).unwrap();
        assert_eq!(p.block_bytes, 64 * 1024);
        assert_eq!(p.block_count, 1);
        assert_eq!(p.buffer_bytes, 64 * 1024);
    }
}
