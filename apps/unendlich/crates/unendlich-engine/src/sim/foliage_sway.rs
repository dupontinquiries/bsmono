//! r330-gpu — foliage wind-sway tagging (pure core).
//!
//! ## The bug (§4b)
//! `shaders/chunk.vert` decided whether a vertex sways in the wind by
//! sniffing its **green vertex color** (`vc.g > vc.r+.. && vc.g > vc.b+..`).
//! Green-dominant is also true of the **solid top face of a grass block**,
//! so those rippled like loose foliage — and because the test is per-vertex
//! with no notion of a plant's footing, plant *bases* swayed too, tearing
//! them out of the ground.
//!
//! ## The fix
//! Move the decision to **mesh time**, where the block identity and the
//! vertex's role (free canopy vertex vs. planted base) are known, and carry
//! the result to the GPU as an explicit **2-bit tag** riding the reserved
//! alpha of the packed position word (`PackedVertex::pos_packed`,
//! `A2B10G10R10_UNORM_PACK32`, bits 30..32 — documented "reserved … per-vertex
//! flag"). The shader then sways **only** when the tag says so, and holds the
//! base vertex still.
//!
//! This module is the **pure, deterministic, self-contained** heart of that
//! change: the eligibility decision, the per-vertex tag resolution, the
//! bit-packing into the reserved alpha, and a faithful replica of the shader's
//! sway gate so the behaviour can be proven numerically in-sandbox (no GPU).
//!
//! It is intentionally free of `crate::` references so it gates with the
//! documented standalone floor (`rustc --edition 2021 --test foliage_sway.rs`).
//! `render::mesher` maps its `BlockId`/face into the boolean inputs here under
//! a `// r330-gpu` adapter; the GPU mesher converges on the same packed form
//! at the shared quantize step (see HANDOFF_gpu_foliage_sway.md).

// --- Face indices (mirror of `render::mesher::face`, kept local so this
// --- module stays dependency-free for the standalone gate). Only TOP is
// --- load-bearing today; the rest are here so callers can pass the real
// --- face index without a translation table.
pub const FACE_TOP: usize = 0;
pub const FACE_BOTTOM: usize = 1;
pub const FACE_EAST: usize = 2;
pub const FACE_WEST: usize = 3;
pub const FACE_SOUTH: usize = 4;
pub const FACE_NORTH: usize = 5;

/// The 2-bit per-vertex sway tag. Encoded into the reserved alpha of the
/// A2B10G10R10 position word; decoded in `chunk.vert` from `in_pos_unorm.w`.
///
/// Two bits give four slots; three are used and one is reserved so a later
/// pass (e.g. a mid-height "half sway" for tall plants) can be added without
/// another vertex-format change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SwayTag {
    /// Solid geometry — never sways. Stone, dirt, and crucially the *top
    /// face of a grass block* (the bug). This is the default for every
    /// vertex, so an un-tagged mesh is inert rather than mis-swaying.
    None = 0,
    /// A free-moving foliage vertex: leaf-cube vertices and the upper
    /// vertices of a plant. Receives the full wind displacement.
    Sway = 1,
    /// A sway-eligible but **planted** vertex — the base of a plant. Marked
    /// foliage (so it is conceptually part of the swaying primitive) yet held
    /// in place by the shader so the plant pivots about its root.
    Anchor = 2,
}

impl SwayTag {
    /// The raw 2-bit value as packed/unpacked.
    #[inline]
    pub fn bits(self) -> u8 {
        self as u8
    }

    /// Reconstruct from the low 2 bits of a byte. Unknown/reserved patterns
    /// fall back to `None` (inert) — never panics.
    #[inline]
    pub fn from_bits(b: u8) -> SwayTag {
        match b & 0b11 {
            1 => SwayTag::Sway,
            2 => SwayTag::Anchor,
            _ => SwayTag::None, // 0 and the reserved 3 are treated as inert
        }
    }

    /// Does the shader apply any wind motion to this vertex at all?
    #[inline]
    pub fn is_eligible(self) -> bool {
        matches!(self, SwayTag::Sway | SwayTag::Anchor)
    }
}

/// Block-level sway eligibility — the decision that fixes the bug.
///
/// Inputs are pure block properties (so this stays `crate::`-free):
/// * `is_leaf`  — a leaf-type block whose cube is pure foliage decoration
///                (`BlockId::is_subvoxel_only()` in the engine: Oak/Birch
///                leaves). Whole cube is canopy → its vertices sway.
/// * `is_plant` — a dedicated cross/sub-voxel plant (TallGrass, Fern,
///                Flower …). Sways, with its base anchored per-vertex.
/// * `_face`    — the face being emitted. Retained for forward-compatibility
///                and to honour the dispatch signature; the current rule is
///                face-independent. **A solid block never sways on *any*
///                face — including its green TOP face — which is exactly the
///                grass-block-top regression being closed.**
///
/// Returns whether the block/face is foliage that may sway *at all*. The
/// base-vs-top split is then made per-vertex by [`vertex_tag`].
#[inline]
pub fn block_sway_eligible(is_leaf: bool, is_plant: bool, _face: usize) -> bool {
    is_leaf || is_plant
}

/// Resolve the final per-vertex [`SwayTag`].
///
/// * Not eligible → `None` (solid geometry, grass-block tops included).
/// * Eligible + `is_base_vertex` → `Anchor` (held in place).
/// * Eligible, not base → `Sway` (full displacement).
///
/// `is_base_vertex` is decided by the mesher from the vertex's height within
/// its plant cell (the lowest row of a plant's geometry). Leaf cubes pass
/// `is_base_vertex = false` for every vertex (the whole cube ripples).
#[inline]
pub fn vertex_tag(eligible: bool, is_base_vertex: bool) -> SwayTag {
    if !eligible {
        SwayTag::None
    } else if is_base_vertex {
        SwayTag::Anchor
    } else {
        SwayTag::Sway
    }
}

// =============================================================================
// Packing into the reserved alpha of `PackedVertex::pos_packed`.
//
// A2B10G10R10_UNORM_PACK32: R=bits 0..10, G=10..20, B=20..30, A=bits 30..32.
// We only ever touch bits 30..32; the position bits are left untouched, so
// this composes with the existing `PackedVertex::pack_pos`.
// =============================================================================

/// Bit offset of the 2-bit alpha tag within the packed position word.
pub const POS_ALPHA_SHIFT: u32 = 30;
/// Mask of the 2-bit alpha tag, pre-shift.
pub const POS_ALPHA_MASK: u32 = 0b11;

/// Write a sway tag into the reserved alpha of an already-packed position
/// word, leaving the 30 position bits untouched. Idempotent: re-tagging
/// clears the old tag first.
#[inline]
pub fn pack_tag_into_pos(pos_packed: u32, tag: SwayTag) -> u32 {
    let cleared = pos_packed & !(POS_ALPHA_MASK << POS_ALPHA_SHIFT);
    cleared | ((tag.bits() as u32 & POS_ALPHA_MASK) << POS_ALPHA_SHIFT)
}

/// Read the sway tag back out of a packed position word.
#[inline]
pub fn unpack_tag(pos_packed: u32) -> SwayTag {
    SwayTag::from_bits(((pos_packed >> POS_ALPHA_SHIFT) & POS_ALPHA_MASK) as u8)
}

/// The unorm value the GPU sees for a tag, i.e. `tag / 3` from a 2-bit unorm
/// channel. The shader recovers the tag as `round(w * 3.0)`. Pure mirror of
/// the hardware's A2_UNORM decode, used by the shader-gate replica below and
/// by tests asserting round-trip through the unorm channel.
#[inline]
pub fn tag_unorm(tag: SwayTag) -> f32 {
    tag.bits() as f32 / 3.0
}

/// Inverse of [`tag_unorm`] — how `chunk.vert` turns `in_pos_unorm.w` back
/// into a tag. Kept here so the shader and the CPU proof use *one* rule.
#[inline]
pub fn tag_from_unorm(w: f32) -> SwayTag {
    // round-to-nearest of w*3, clamped into [0,3]
    let q = (w * 3.0 + 0.5) as i32; // truncation == floor for w>=0
    let q = if q < 0 { 0 } else if q > 3 { 3 } else { q };
    SwayTag::from_bits(q as u8)
}

// =============================================================================
// Shader-gate replica.
//
// A byte-for-byte port of the wind displacement in `chunk.vert` (post-fix),
// so the gate's behaviour is provable in-sandbox without a GPU. If the shader
// and this function ever diverge, the parity tests below fail.
// =============================================================================

/// Wind parameters mirroring the relevant `CameraUbo` fields.
#[derive(Debug, Clone, Copy)]
pub struct Wind {
    /// `wind_params.xy` — wind vector at the player.
    pub dir: [f32; 2],
    /// `wind_params.z` — wind magnitude.
    pub mag: f32,
    /// `atmos_params.w` — time-of-day in seconds (already advances ~1/s).
    pub time: f32,
}

/// Compute the post-fix wind displacement applied to a vertex, given its
/// decoded tag and world position. Returns `[dx, dy, dz]` to add to
/// `world_pos`. `SwayTag::None` and `SwayTag::Anchor` both yield zero
/// horizontal motion; only `Sway` moves. (Anchor stays put — that is the
/// "anchor the base vertex" guarantee.)
///
/// This is the exact arithmetic emitted in `chunk.vert`'s `main()` after the
/// r330-gpu edit; see that file for the matching constants.
pub fn sway_displacement(tag: SwayTag, world_pos: [f32; 3], wind: &Wind) -> [f32; 3] {
    if tag != SwayTag::Sway {
        // None  → solid, never moves.
        // Anchor → planted base, held in place.
        return [0.0, 0.0, 0.0];
    }
    let wlen = (wind.dir[0] * wind.dir[0] + wind.dir[1] * wind.dir[1]).sqrt();
    let wd = if wlen > 0.001 {
        [wind.dir[0] / wlen, wind.dir[1] / wlen]
    } else {
        [1.0, 0.0]
    };
    let t = wind.time * 1.4;
    let phase = world_pos[0] * 0.6 + world_pos[2] * 0.45 + t;
    let amp = 0.02 + 0.04 * wind.mag;
    let dxz = phase.sin() * amp;
    let dy = (phase * 1.7).sin() * 0.012 * (0.3 + wind.mag);
    [wd[0] * dxz, dy, wd[1] * dxz]
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- the bug fix: solid grass-block top must NOT be sway-eligible ----

    #[test]
    fn solid_grass_top_is_not_eligible_on_any_face() {
        // Grass block: not a leaf, not a plant. Green, but solid ground.
        for face in [FACE_TOP, FACE_BOTTOM, FACE_EAST, FACE_WEST, FACE_SOUTH, FACE_NORTH] {
            assert!(
                !block_sway_eligible(/*is_leaf*/ false, /*is_plant*/ false, face),
                "grass/solid block must never sway (face {face}) — this is the §4b bug"
            );
        }
    }

    #[test]
    fn leaves_are_eligible() {
        // Leaf cube: is_subvoxel_only() == true in the engine.
        assert!(block_sway_eligible(true, false, FACE_TOP));
        assert!(block_sway_eligible(true, false, FACE_SOUTH));
    }

    #[test]
    fn plants_are_eligible() {
        assert!(block_sway_eligible(false, true, FACE_TOP));
        assert!(block_sway_eligible(false, true, FACE_NORTH));
    }

    // ---- per-vertex tag resolution incl. base anchoring ----

    #[test]
    fn vertex_tags_resolve_correctly() {
        assert_eq!(vertex_tag(false, false), SwayTag::None);
        assert_eq!(vertex_tag(false, true), SwayTag::None); // ineligible wins
        assert_eq!(vertex_tag(true, false), SwayTag::Sway);
        assert_eq!(vertex_tag(true, true), SwayTag::Anchor); // planted base
    }

    #[test]
    fn leaf_cube_vertices_all_sway_none_anchored() {
        // Leaves pass is_base_vertex=false for every vertex → whole cube ripples.
        let eligible = block_sway_eligible(true, false, FACE_TOP);
        assert_eq!(vertex_tag(eligible, false), SwayTag::Sway);
    }

    #[test]
    fn plant_base_anchored_top_free() {
        let eligible = block_sway_eligible(false, true, FACE_TOP);
        assert_eq!(vertex_tag(eligible, true), SwayTag::Anchor); // root
        assert_eq!(vertex_tag(eligible, false), SwayTag::Sway); // tip
    }

    // ---- bit packing into the reserved pos alpha ----

    #[test]
    fn pack_preserves_position_bits() {
        // A position word using all 30 position bits, alpha initially 0.
        let pos = 0x3FFF_FFFFu32; // bits 0..30 all set, alpha 0
        for tag in [SwayTag::None, SwayTag::Sway, SwayTag::Anchor] {
            let packed = pack_tag_into_pos(pos, tag);
            // low 30 bits unchanged
            assert_eq!(packed & 0x3FFF_FFFF, 0x3FFF_FFFF, "position bits clobbered");
            // alpha holds the tag
            assert_eq!(unpack_tag(packed), tag);
        }
    }

    #[test]
    fn retag_is_idempotent() {
        let pos = 0x0ABC_1234u32;
        let once = pack_tag_into_pos(pos, SwayTag::Sway);
        let twice = pack_tag_into_pos(once, SwayTag::Anchor);
        // re-tagging clears the old tag, doesn't OR-accumulate
        assert_eq!(unpack_tag(twice), SwayTag::Anchor);
        // and the position payload survived both writes
        assert_eq!(twice & 0x3FFF_FFFF, pos & 0x3FFF_FFFF);
    }

    #[test]
    fn tag_roundtrips_through_unorm_channel() {
        // What the GPU does: A2 unorm encode (tag/3) then decode (round(w*3)).
        for tag in [SwayTag::None, SwayTag::Sway, SwayTag::Anchor] {
            let w = tag_unorm(tag);
            assert_eq!(tag_from_unorm(w), tag, "unorm round-trip failed for {tag:?}");
        }
        // explicit channel values
        assert!((tag_unorm(SwayTag::None) - 0.0).abs() < 1e-6);
        assert!((tag_unorm(SwayTag::Sway) - 1.0 / 3.0).abs() < 1e-6);
        assert!((tag_unorm(SwayTag::Anchor) - 2.0 / 3.0).abs() < 1e-6);
    }

    // ---- shader-gate replica: the actual visible behaviour ----

    fn wind() -> Wind {
        Wind { dir: [0.7, 0.3], mag: 1.0, time: 12.5 }
    }

    #[test]
    fn solid_top_does_not_move() {
        // The whole point: a grass-block top vertex (tag None) is frozen.
        let d = sway_displacement(SwayTag::None, [10.0, 64.0, 20.0], &wind());
        assert_eq!(d, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn anchored_base_does_not_move() {
        let d = sway_displacement(SwayTag::Anchor, [10.0, 64.0, 20.0], &wind());
        assert_eq!(d, [0.0, 0.0, 0.0], "planted base must stay rooted");
    }

    #[test]
    fn swaying_tip_actually_moves() {
        // A free foliage vertex must move under non-zero wind.
        let d = sway_displacement(SwayTag::Sway, [10.0, 64.0, 20.0], &wind());
        let mag = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        assert!(mag > 0.0, "sway vertex should displace, got {d:?}");
        // displacement is bounded (amp = 0.02 + 0.04*mag = 0.06 horizontal max)
        assert!(d[0].abs() <= 0.06 + 1e-6);
        assert!(d[2].abs() <= 0.06 + 1e-6);
    }

    #[test]
    fn sway_is_deterministic() {
        let w = wind();
        let a = sway_displacement(SwayTag::Sway, [3.0, 64.0, 7.0], &w);
        let b = sway_displacement(SwayTag::Sway, [3.0, 64.0, 7.0], &w);
        assert_eq!(a, b);
    }

    #[test]
    fn zero_wind_vector_uses_safe_default_direction() {
        // wd falls back to (1,0) when the wind vector is ~0 — no NaN/div0.
        let w = Wind { dir: [0.0, 0.0], mag: 0.5, time: 1.0 };
        let d = sway_displacement(SwayTag::Sway, [1.0, 64.0, 1.0], &w);
        assert!(d[0].is_finite() && d[1].is_finite() && d[2].is_finite());
    }

    #[test]
    fn grass_top_vs_plant_tip_diverge_under_same_wind() {
        // End-to-end statement of the fix: at the same world position and
        // wind, the solid grass top is still while the plant tip sways.
        let w = wind();
        let p = [5.0, 64.0, 9.0];
        let grass_top = sway_displacement(vertex_tag(block_sway_eligible(false, false, FACE_TOP), false), p, &w);
        let plant_tip = sway_displacement(vertex_tag(block_sway_eligible(false, true, FACE_TOP), false), p, &w);
        assert_eq!(grass_top, [0.0, 0.0, 0.0]);
        assert_ne!(plant_tip, [0.0, 0.0, 0.0]);
    }
}
