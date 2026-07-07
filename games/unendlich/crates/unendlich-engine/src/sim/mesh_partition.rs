//! `sim::mesh_partition` — hh-r332-t3: directional sub-mesh partitioning so a
//! new neighbour chunk doesn't force a full re-mesh of this chunk.
//!
//! **The problem (r332 PERF profile):** `world/chunk_manager.rs` notes a
//! "remeshing storm — ~35 re-meshes per chunk over a session." When a new
//! neighbour appears it bumps this chunk's version → the chunk is **fully**
//! re-meshed (all 6 face planes + interior decoration) even though only the
//! **one boundary plane facing the new neighbour** can actually change (its
//! faces are revealed or culled by the neighbour-aware face culling).
//!
//! **The scheme:** split a chunk's boundary mesh into **6 directional
//! sub-meshes**, one per axis-aligned face direction. The greedy mesher
//! already walks each of the 6 face planes (`AXIS_CONFIGS` in `render/mesher.rs`),
//! so bucketing a quad into its sub-mesh is a *label*, not a re-walk
//! (`assign_quad`). A neighbour event on side `S` then re-meshes only the `S`
//! sub-mesh (`dirtied_by_neighbor`) — 1/6 of the boundary, interior decoration
//! (grass/flowers, always interior per FINDINGS-r62-05) never touched.
//!
//! Pure & self-contained: data in, data out. No IO/RNG/clock, no
//! `unwrap`/`expect`/`panic!`, no `crate::` deps (so it gates standalone with
//! `rustc --edition 2021 --test`). The bin wiring (mesh_buffer_pool slots,
//! mesher buckets, chunk_manager dirty-scoping) is *proposed* in
//! `HANDOFF_mesh_partition.md`, not applied here.

/// One of the 6 axis-aligned boundary directions of a chunk. A chunk's
/// boundary mesh is partitioned into one sub-mesh per direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SubMeshDir {
    NegX,
    PosX,
    NegY,
    PosY,
    NegZ,
    PosZ,
}

// Single-element static slices so `dirtied_by_neighbor` can return a borrow
// without allocating.
const D_NEGX: [SubMeshDir; 1] = [SubMeshDir::NegX];
const D_POSX: [SubMeshDir; 1] = [SubMeshDir::PosX];
const D_NEGY: [SubMeshDir; 1] = [SubMeshDir::NegY];
const D_POSY: [SubMeshDir; 1] = [SubMeshDir::PosY];
const D_NEGZ: [SubMeshDir; 1] = [SubMeshDir::NegZ];
const D_POSZ: [SubMeshDir; 1] = [SubMeshDir::PosZ];

impl SubMeshDir {
    /// All six directions in canonical index order (matches [`index`]).
    pub const ALL: [SubMeshDir; 6] = [
        SubMeshDir::NegX,
        SubMeshDir::PosX,
        SubMeshDir::NegY,
        SubMeshDir::PosY,
        SubMeshDir::NegZ,
        SubMeshDir::PosZ,
    ];

    /// Stable `0..6` index, for indexing a `[T; 6]` per-chunk sub-mesh slot
    /// array (e.g. `mesh_buffer_pool`'s proposed `[Option<MeshSlot>; 6]`).
    pub fn index(self) -> usize {
        match self {
            SubMeshDir::NegX => 0,
            SubMeshDir::PosX => 1,
            SubMeshDir::NegY => 2,
            SubMeshDir::PosY => 3,
            SubMeshDir::NegZ => 4,
            SubMeshDir::PosZ => 5,
        }
    }

    /// Inverse of [`index`]. `None` for `i >= 6`.
    pub fn from_index(i: usize) -> Option<SubMeshDir> {
        SubMeshDir::ALL.get(i).copied()
    }

    /// The opposite face direction. At a boundary, the neighbour's facing
    /// sub-mesh is the opposite of the side it sits on relative to this chunk.
    pub fn opposite(self) -> SubMeshDir {
        match self {
            SubMeshDir::NegX => SubMeshDir::PosX,
            SubMeshDir::PosX => SubMeshDir::NegX,
            SubMeshDir::NegY => SubMeshDir::PosY,
            SubMeshDir::PosY => SubMeshDir::NegY,
            SubMeshDir::NegZ => SubMeshDir::PosZ,
            SubMeshDir::PosZ => SubMeshDir::NegZ,
        }
    }

    /// `(axis, sign)` where axis is `0=X, 1=Y, 2=Z` and sign is `+1` / `-1`.
    pub fn axis_sign(self) -> (usize, i32) {
        match self {
            SubMeshDir::NegX => (0, -1),
            SubMeshDir::PosX => (0, 1),
            SubMeshDir::NegY => (1, -1),
            SubMeshDir::PosY => (1, 1),
            SubMeshDir::NegZ => (2, -1),
            SubMeshDir::PosZ => (2, 1),
        }
    }

    /// Build a direction from an `(axis, delta)` pair, matching
    /// `chunk_manager::bump_neighbor_if_edge`'s convention (axis `0/1/2`,
    /// delta `+1` for the positive neighbour, `-1` for the negative). `None`
    /// for an unknown axis or a zero/!±1 delta.
    pub fn from_axis_delta(axis: usize, delta: i32) -> Option<SubMeshDir> {
        match (axis, delta) {
            (0, -1) => Some(SubMeshDir::NegX),
            (0, 1) => Some(SubMeshDir::PosX),
            (1, -1) => Some(SubMeshDir::NegY),
            (1, 1) => Some(SubMeshDir::PosY),
            (2, -1) => Some(SubMeshDir::NegZ),
            (2, 1) => Some(SubMeshDir::PosZ),
            _ => None,
        }
    }

    /// Bucket a greedy quad by its outward face normal (an axis-aligned unit
    /// vector). Picks the dominant axis, so it is exact for the mesher's
    /// `FACE_NORMALS` and robust to small float noise. `None` only for a
    /// degenerate (all-zero) normal — never panics.
    pub fn from_face_normal(n: [f32; 3]) -> Option<SubMeshDir> {
        let ax = n[0].abs();
        let ay = n[1].abs();
        let az = n[2].abs();
        if ax == 0.0 && ay == 0.0 && az == 0.0 {
            return None;
        }
        if ax >= ay && ax >= az {
            Some(if n[0] >= 0.0 { SubMeshDir::PosX } else { SubMeshDir::NegX })
        } else if ay >= ax && ay >= az {
            Some(if n[1] >= 0.0 { SubMeshDir::PosY } else { SubMeshDir::NegY })
        } else {
            Some(if n[2] >= 0.0 { SubMeshDir::PosZ } else { SubMeshDir::NegZ })
        }
    }

    /// Map the mesher's `face::*` index to a sub-mesh direction. Matches
    /// `render/mesher.rs`'s `mod face` exactly:
    /// `TOP=0(+Y), BOTTOM=1(-Y), EAST=2(+X), WEST=3(-X), SOUTH=4(+Z), NORTH=5(-Z)`.
    /// Exact and integer — preferred for wiring since the mesher already holds
    /// `cfg.face`. `None` for `face >= 6`.
    pub fn from_face_index(face: usize) -> Option<SubMeshDir> {
        match face {
            0 => Some(SubMeshDir::PosY), // TOP
            1 => Some(SubMeshDir::NegY), // BOTTOM
            2 => Some(SubMeshDir::PosX), // EAST
            3 => Some(SubMeshDir::NegX), // WEST
            4 => Some(SubMeshDir::PosZ), // SOUTH
            5 => Some(SubMeshDir::NegZ), // NORTH
            _ => None,
        }
    }
}

/// Per-quad assignment entry point — the "bucket each emitted greedy quad into
/// its directional sub-mesh" step. Thin wrapper over
/// [`SubMeshDir::from_face_normal`]; returns `Option` (vs. the brief's
/// non-`Option` sketch) to stay panic-free per SHARED_RULES §1 — a degenerate
/// normal yields `None` instead of a panic. Real greedy quads always have a
/// unit axis normal, so callers can treat `None` as "skip / decoration".
pub fn assign_quad(face_normal: [f32; 3]) -> Option<SubMeshDir> {
    SubMeshDir::from_face_normal(face_normal)
}

/// The sub-meshes of THIS chunk that a neighbour appearing/leaving on `side`
/// can change: exactly the one plane facing `side`. The interior and the other
/// five planes are untouched — this is the whole win of the partition (1/6 of
/// the boundary re-meshed per neighbour event instead of the whole chunk).
pub fn dirtied_by_neighbor(side: SubMeshDir) -> &'static [SubMeshDir] {
    match side {
        SubMeshDir::NegX => &D_NEGX,
        SubMeshDir::PosX => &D_POSX,
        SubMeshDir::NegY => &D_NEGY,
        SubMeshDir::PosY => &D_POSY,
        SubMeshDir::NegZ => &D_NEGZ,
        SubMeshDir::PosZ => &D_POSZ,
    }
}

/// A tiny 6-bit set tracking which directional sub-meshes are dirty (need a
/// re-mesh / re-upload). The manager marks sides on neighbour events and drains
/// the set when it rebuilds, so only dirty sub-meshes are rebuilt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubMeshSet {
    bits: u8,
}

impl SubMeshSet {
    /// An empty set (nothing dirty).
    pub fn empty() -> SubMeshSet {
        SubMeshSet { bits: 0 }
    }

    /// A full set (all six dirty) — e.g. a freshly meshed chunk, or `ALL`.
    pub fn all() -> SubMeshSet {
        SubMeshSet { bits: 0b0011_1111 }
    }

    fn bit(side: SubMeshDir) -> u8 {
        1u8 << side.index()
    }

    /// Mark `side` dirty.
    pub fn mark(&mut self, side: SubMeshDir) {
        self.bits |= SubMeshSet::bit(side);
    }

    /// Clear `side`.
    pub fn unmark(&mut self, side: SubMeshDir) {
        self.bits &= !SubMeshSet::bit(side);
    }

    /// Is `side` dirty?
    pub fn contains(self, side: SubMeshDir) -> bool {
        self.bits & SubMeshSet::bit(side) != 0
    }

    /// Is anything dirty?
    pub fn any(self) -> bool {
        self.bits != 0
    }

    /// How many sub-meshes are dirty (0..=6).
    pub fn count(self) -> u32 {
        self.bits.count_ones()
    }

    /// Return the current dirty set and clear `self` (drain). The manager calls
    /// this when it rebuilds: it re-meshes/re-uploads exactly the returned
    /// directions and leaves the chunk clean.
    pub fn take_dirty(&mut self) -> SubMeshSet {
        let drained = *self;
        self.bits = 0;
        drained
    }

    /// Iterate the dirty directions in canonical [`SubMeshDir::ALL`] order.
    /// Alloc-free.
    pub fn iter(self) -> impl Iterator<Item = SubMeshDir> {
        SubMeshDir::ALL.into_iter().filter(move |&d| self.contains(d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The mesher's convention, copied from render/mesher.rs for the
    // cross-check below (FACE_NORMALS + the face::* indices). If the mesher's
    // convention ever changes, `face_index_and_normal_agree` will catch it.
    const FACE_NORMALS: [[f32; 3]; 6] = [
        [0.0, 1.0, 0.0],  // 0 TOP    +Y
        [0.0, -1.0, 0.0], // 1 BOTTOM -Y
        [1.0, 0.0, 0.0],  // 2 EAST   +X
        [-1.0, 0.0, 0.0], // 3 WEST   -X
        [0.0, 0.0, 1.0],  // 4 SOUTH  +Z
        [0.0, 0.0, -1.0], // 5 NORTH  -Z
    ];

    #[test]
    fn dirtied_by_neighbor_returns_exactly_the_facing_plane() {
        for d in SubMeshDir::ALL {
            let dirty = dirtied_by_neighbor(d);
            assert_eq!(dirty.len(), 1, "{d:?} must dirty exactly one sub-mesh");
            assert_eq!(dirty[0], d, "{d:?} must dirty its own facing plane");
        }
    }

    #[test]
    fn assign_quad_buckets_each_face_normal_and_matches_face_index() {
        // Every face normal buckets, and the normal-based and index-based
        // entry points agree — proving both match the mesher's convention.
        let expected = [
            SubMeshDir::PosY, SubMeshDir::NegY, SubMeshDir::PosX,
            SubMeshDir::NegX, SubMeshDir::PosZ, SubMeshDir::NegZ,
        ];
        for (i, n) in FACE_NORMALS.iter().enumerate() {
            assert_eq!(assign_quad(*n), Some(expected[i]), "normal {n:?}");
            assert_eq!(SubMeshDir::from_face_index(i), Some(expected[i]), "face {i}");
            assert_eq!(SubMeshDir::from_face_normal(*n), SubMeshDir::from_face_index(i),
                "normal vs index entry points must agree for face {i}");
        }
    }

    #[test]
    fn neighbor_event_dirties_one_submesh_not_six() {
        let mut set = SubMeshSet::empty();
        for &d in dirtied_by_neighbor(SubMeshDir::PosX) {
            set.mark(d);
        }
        assert_eq!(set.count(), 1, "a neighbour event must dirty 1 sub-mesh, not 6");
        assert!(set.contains(SubMeshDir::PosX));
        assert!(!set.contains(SubMeshDir::NegX));
        assert!(!set.contains(SubMeshDir::PosY));
    }

    #[test]
    fn submesh_set_mark_take_roundtrips_and_clears() {
        let mut set = SubMeshSet::empty();
        set.mark(SubMeshDir::PosX);
        set.mark(SubMeshDir::NegZ);
        assert_eq!(set.count(), 2);
        let drained = set.take_dirty();
        assert!(drained.contains(SubMeshDir::PosX));
        assert!(drained.contains(SubMeshDir::NegZ));
        assert!(!set.any(), "take_dirty must leave the set empty");
        assert_eq!(set.count(), 0);
        // Drained directions iterate in canonical order.
        let got: Vec<SubMeshDir> = drained.iter().collect();
        assert_eq!(got, vec![SubMeshDir::PosX, SubMeshDir::NegZ]);
    }

    #[test]
    fn all_covers_every_variant_and_index_roundtrips() {
        let all = SubMeshSet::all();
        for d in SubMeshDir::ALL {
            assert!(all.contains(d), "all() must contain {d:?}");
            assert_eq!(SubMeshDir::from_index(d.index()), Some(d), "index roundtrip {d:?}");
        }
        assert_eq!(all.count(), 6);
        assert_eq!(SubMeshDir::from_index(6), None, "out-of-range index → None");
        // ALL is exactly the 6 distinct variants.
        let collected: Vec<SubMeshDir> = SubMeshSet::all().iter().collect();
        assert_eq!(collected.len(), 6);
    }

    #[test]
    fn axis_delta_and_opposite_match_the_wiring_convention() {
        // (axis, delta) ↔ dir round-trips, and opposite flips the sign.
        for d in SubMeshDir::ALL {
            let (axis, sign) = d.axis_sign();
            assert_eq!(SubMeshDir::from_axis_delta(axis, sign), Some(d));
            assert_eq!(d.opposite().opposite(), d, "opposite is an involution");
            let (oaxis, osign) = d.opposite().axis_sign();
            assert_eq!(oaxis, axis, "opposite stays on the same axis");
            assert_eq!(osign, -sign, "opposite flips the sign");
        }
        assert_eq!(SubMeshDir::from_axis_delta(3, 1), None, "bad axis → None");
        assert_eq!(SubMeshDir::from_axis_delta(0, 0), None, "zero delta → None");
    }

    #[test]
    fn degenerate_normal_is_none_and_dominant_axis_wins() {
        assert_eq!(SubMeshDir::from_face_normal([0.0, 0.0, 0.0]), None);
        assert_eq!(assign_quad([0.0, 0.0, 0.0]), None);
        // Non-unit but clearly +X dominant still buckets to PosX.
        assert_eq!(SubMeshDir::from_face_normal([0.9, 0.1, -0.05]), Some(SubMeshDir::PosX));
        assert_eq!(SubMeshDir::from_face_normal([-0.2, -0.8, 0.3]), Some(SubMeshDir::NegY));
    }

    #[test]
    fn deterministic() {
        // Same inputs → same outputs across repeated calls.
        for _ in 0..3 {
            assert_eq!(dirtied_by_neighbor(SubMeshDir::PosZ), &[SubMeshDir::PosZ]);
            assert_eq!(assign_quad([1.0, 0.0, 0.0]), Some(SubMeshDir::PosX));
            let mut s = SubMeshSet::all();
            assert_eq!(s.take_dirty().count(), 6);
            assert_eq!(s.count(), 0);
        }
    }
}
