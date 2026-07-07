// octree_types.glsl — type + constant definitions for the GPU
// OctreeArray reader. Companion to octree_sample.glsl.
//
// Split rationale (v0.24.26-octree-on-gpu): the consuming shader
// must declare the `nodes[]` SSBO BEFORE including the sample
// function (GLSL doesn't allow SSBOs as function parameters, so
// the sample function references a global `nodes[]`). That means
// the include order is:
//
//   #include "octree_types.glsl"          // <- struct, constants
//   layout(std430, ...) buffer OctreeNodes {
//       OctreeNode nodes[];               // <- now nodes[] is in scope
//   };
//   #include "octree_sample.glsl"         // <- sample function uses nodes[]
//
// Keeping types+constants in this file makes it safe to include
// before any SSBO declaration. octree_sample.glsl is included after.
//
// The CPU twin is `crates/unendlich-engine/src/world/octree.rs`. Any
// change to the bit layout MUST happen in both files in lockstep.
//
// On-the-wire node format (must match Rust OctreeNode exactly):
//
//   offset  size  field
//   ------  ----  -----
//       0     4   packed_header (uint)
//                 bit 31:      is_leaf
//                 bits 30..28: depth (0..=5)
//                 bits 27..8:  reserved (zero)
//                 bits 7..0:   block_id (leaves only)
//       4     4   last_modified_tick (uint)
//                 Max tick stamped on any leaf at or below this node.
//                 Cache-deferment consumers compare against their
//                 own "I last read at tick T" timestamp.
//       8    32   children[8]  (uint × 8)
//                 octant index = (z_hi << 2) | (y_hi << 1) | x_hi
//   ------  --
//      40   total

#ifndef OCTREE_TYPES_GLSL
#define OCTREE_TYPES_GLSL

// Must match the Rust constant of the same name.
#define OCTREE_DEPTH 5u

// Chunk edge in blocks. At depth 5, half-size starts at CHUNK_SIZE/2.
#define OCTREE_CHUNK_SIZE 32u

#define OCTREE_FLAG_IS_LEAF 0x80000000u
#define OCTREE_SHIFT_DEPTH  28u
#define OCTREE_MASK_DEPTH   0x70000000u
#define OCTREE_MASK_BLOCK   0x000000FFu

struct OctreeNode {
    uint packed_header;
    uint last_modified_tick;
    uint children[8];
};

bool octree_node_is_leaf(OctreeNode n) {
    return (n.packed_header & OCTREE_FLAG_IS_LEAF) != 0u;
}

uint octree_node_depth(OctreeNode n) {
    return (n.packed_header & OCTREE_MASK_DEPTH) >> OCTREE_SHIFT_DEPTH;
}

uint octree_node_leaf_block(OctreeNode n) {
    return n.packed_header & OCTREE_MASK_BLOCK;
}

#endif // OCTREE_TYPES_GLSL
