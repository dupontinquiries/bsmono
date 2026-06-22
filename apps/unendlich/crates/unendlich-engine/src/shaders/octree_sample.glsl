// octree_sample.glsl — GPU sample function for the v0.24.23
// OctreeArray wire format.
//
// Pairs with octree_types.glsl (struct + constants + helpers).
// See that file for the on-the-wire byte layout.
//
// Usage:
//
//   #include "octree_types.glsl"
//   layout(std430, set = 0, binding = 0) readonly buffer OctreeNodes {
//       OctreeNode nodes[];
//   };
//   #include "octree_sample.glsl"
//
//   void main() {
//       uint block = octree_sample(base, uvec3(x, y, z));
//       ...
//   }
//
// `octree_sample` references the global `nodes[]` symbol declared
// by the caller's SSBO block. GLSL doesn't permit passing SSBOs as
// function parameters, so this is the least-bad coupling.
//
// One chunk's array is a contiguous slice `[base .. base+count)` of
// the global `nodes[]` table. The caller passes `base` (typically
// from `OctreeSlot::base_node_index` on the CPU side); node 0 of
// each slice is that chunk's root.

#ifndef OCTREE_SAMPLE_GLSL
#define OCTREE_SAMPLE_GLSL

// Sample the block_id at chunk-local (x, y, z) where each coord is
// in [0, OCTREE_CHUNK_SIZE). `base` is the index of this chunk's
// root in the globally-bound `nodes[]` SSBO.
//
// Mirrors the CPU `OctreeArray::sample` logic in octree.rs:
// - start at the root (index `base`)
// - if leaf, return its block_id
// - otherwise pick the child by which half of the cube `pos` lies
//   in, halve the cell size, descend
//
// The loop is bounded by `OCTREE_DEPTH + 1` so a malformed octree
// can't hang the GPU. The fallback at the end returns the deepest
// node's block payload (undefined for an internal node, but at
// least terminates); the verification probe will catch divergence
// vs the CPU.
//
// IMPORTANT: `OctreeNode.children[i]` stores indices that are
// **chunk-local** (offsets into the chunk's own node array as it
// existed on the CPU before upload). The global SSBO is the
// concatenation of every chunk's nodes, so to read the next node
// during descent we must rebase: `nodes[base + child_index]`. The
// first iteration uses `cur = base` directly because that's already
// the absolute index of this chunk's root. Subsequent iterations
// add `base` again because the children field gives chunk-local
// offsets.
//
// (v0.24.26-octree-on-gpu-fix1: the falsification probe caught this
// on the first run — without rebasing, every descent past the root
// read from near nodes[0], which is whatever the first uploaded
// chunk's bytes are. 33.7% of probes mismatched on the broken
// version because chunks with fully-collapsed roots — pure-air sky
// chunks, pure-stone deep chunks — happened to not trigger the
// descent and still passed.)
uint octree_sample(uint base, uvec3 pos) {
    uint cur = base;
    uvec3 origin = uvec3(0u);
    uint half_size = OCTREE_CHUNK_SIZE >> 1u;  // 16 for CHUNK_SIZE=32

    for (uint iter = 0u; iter < OCTREE_DEPTH + 1u; ++iter) {
        OctreeNode n = nodes[cur];
        if (octree_node_is_leaf(n)) {
            return octree_node_leaf_block(n);
        }
        uvec3 bit = uvec3(
            (pos.x - origin.x) >= half_size ? 1u : 0u,
            (pos.y - origin.y) >= half_size ? 1u : 0u,
            (pos.z - origin.z) >= half_size ? 1u : 0u
        );
        uint octant = bit.x | (bit.y << 1u) | (bit.z << 2u);
        origin += bit * half_size;
        if (half_size > 0u) half_size = half_size >> 1u;
        // Rebase: children are chunk-local, the SSBO is global.
        cur = base + n.children[octant];
    }
    return octree_node_leaf_block(nodes[cur]);
}

// Like `octree_sample`, but also reports the covering leaf's extent
// (`leaf_origin`, `leaf_size`) — the chunk-local cube `[leaf_origin ..
// leaf_origin+leaf_size)` over which the returned block is uniform. Enables the
// mesher's buried-cell fast path: a solid cell strictly interior to a uniform
// leaf has all six axis-neighbors equal, so every face culls. Same descent as
// `octree_sample` (byte-identical block result); only the extent is extra.
uint octree_sample_ext(uint base, uvec3 pos, out uvec3 leaf_origin, out uint leaf_size) {
    uint cur = base;
    uvec3 origin = uvec3(0u);
    uint node_size = OCTREE_CHUNK_SIZE;       // 32 at the root
    uint half_size = node_size >> 1u;          // 16

    for (uint iter = 0u; iter < OCTREE_DEPTH + 1u; ++iter) {
        OctreeNode n = nodes[cur];
        if (octree_node_is_leaf(n)) {
            leaf_origin = origin;
            leaf_size = node_size;
            return octree_node_leaf_block(n);
        }
        uvec3 bit = uvec3(
            (pos.x - origin.x) >= half_size ? 1u : 0u,
            (pos.y - origin.y) >= half_size ? 1u : 0u,
            (pos.z - origin.z) >= half_size ? 1u : 0u
        );
        uint octant = bit.x | (bit.y << 1u) | (bit.z << 2u);
        origin += bit * half_size;
        node_size = half_size;
        if (half_size > 0u) half_size = half_size >> 1u;
        cur = base + n.children[octant];
    }
    leaf_origin = origin;
    leaf_size = (node_size == 0u) ? 1u : node_size;
    return octree_node_leaf_block(nodes[cur]);
}
//
//   bool region_stable_since(uint base, uvec3 rmin, uvec3 rmax,
//                            uint since_tick);
//
// — descend from root; at any node whose last_modified_tick is
// strictly less than since_tick, return true (entire subtree
// stable). At any node fully contained in [rmin, rmax) whose
// last_modified_tick >= since_tick, return false (dirty). The
// implementation mirrors the CPU one in octree.rs. Filed for the
// ship that wires the first deferment-aware consumer (likely the
// remesh-trigger path).

#endif // OCTREE_SAMPLE_GLSL
