//! `sim::shadow_lod` — how much geometry a chunk contributes to the shadow
//! pass, as a function of the main-view LOD it's already been classified
//! into (`world::lod::LodLevel`).
//!
//! Deliberately does NOT re-derive distance from chunk/player coordinates:
//! `desired_lod_for` already did that work and its band curve is tuned and
//! tested (`lod_bands_for_rd_with_fidelity`). Shadow detail rides on top of
//! that classification instead of computing a second, parallel notion of
//! "how far is this chunk" — one distance curve, two things read it.
//!
//! Two tiers today (Detailed / Basic). A `None` tier — chunks that don't
//! cast into the shadow map at all — is a deliberate follow-up: it needs a
//! decision about correctness (skipping a caster inside a cascade it
//! actually overlaps causes a visible missing shadow, not just a coarser
//! one), so it's being done as its own change after Detailed/Basic ships
//! and its stride interaction has been played with.
//!
//! Takes the caller's LOD as a plain `u8` rather than importing
//! `world::lod::LodLevel` — mirrors the convention in `sim::mesh_budget`
//! (see its `NUM_LOD` doc comment): these perf-core modules stay
//! standalone/dependency-free on `world`, so this one is `rustc --test`-able
//! on its own like `shadow_cascade` and `shadow_refresh_plan`. Callers pass
//! `LodLevel::index()`; the numbering (Near=0, Mid=1, Far=2, VeryFar=3) is
//! mirrored here, not derived from it.

/// Shadow-caster detail tier. Orthogonal to `LodLevel`: a chunk can be full
/// color-detail (`LodLevel::Near`) but the sun doesn't need the same mesh —
/// shadows are low-frequency, so even Near casters can drop to a coarser
/// shadow-only mesh once cheap proxy geometry exists (tracked separately;
/// until then `Detailed` reuses the same mesh as the color pass).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ShadowLod {
    /// Full-resolution shadow geometry. Reserved for casters close enough
    /// that missing detail (a doorway's exact silhouette, a fence gap)
    /// would read as a lighting bug rather than normal LOD popping.
    Detailed,
    /// Coarse/simplified shadow geometry. Correct for anything the color
    /// pass has already decided doesn't need full detail — if `LodLevel`
    /// downgraded it, the shadow it casts can downgrade too without the
    /// player noticing, since they can't see the fine detail it would have
    /// been shadowing either.
    Basic,
}

impl ShadowLod {
    pub fn short_name(self) -> &'static str {
        match self {
            ShadowLod::Detailed => "detailed",
            ShadowLod::Basic => "basic",
        }
    }
}

/// Number of LOD tiers this module knows about (mirrors
/// `world::lod::LodLevel::COUNT` / `sim::mesh_budget::NUM_LOD`: Near=0,
/// Mid=1, Far=2, VeryFar=3).
pub const NUM_LOD: u8 = 4;

/// Map a chunk's main-view LOD tier (`LodLevel::index()` as a `u8`: Near=0,
/// Mid=1, Far=2, VeryFar=3) to its shadow-caster detail. Tier 0 (Near) casts
/// detailed shadows — close enough that shadow silhouette detail is
/// visible; every tier the color pass already coarsened (1/2/3) casts
/// `Basic` shadows. Any tier `>= NUM_LOD` (a future LOD level this module
/// hasn't been updated for) also falls back to `Basic` — the safe,
/// already-in-use default — rather than panicking.
pub fn shadow_lod_for(lod_index: u8) -> ShadowLod {
    match lod_index {
        0 => ShadowLod::Detailed,
        _ => ShadowLod::Basic,
    }
}

/// How much longer a caster in this tier can go between shadow refreshes,
/// on top of the configured `shadow_chunk_stride`. `Basic` casters are
/// coarse enough that staleness is far less visible, so they can be
/// amortized harder than `Detailed` ones without the sun looking "behind."
/// Multiplies into `sim::shadow_refresh_plan`'s effective stride for that
/// caster (not yet wired into the planner call site — see module docs).
pub fn stride_multiplier(tier: ShadowLod) -> u32 {
    match tier {
        ShadowLod::Detailed => 1,
        ShadowLod::Basic => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_is_detailed_rest_is_basic() {
        assert_eq!(shadow_lod_for(0), ShadowLod::Detailed, "0 = Near");
        assert_eq!(shadow_lod_for(1), ShadowLod::Basic, "1 = Mid");
        assert_eq!(shadow_lod_for(2), ShadowLod::Basic, "2 = Far");
        assert_eq!(shadow_lod_for(3), ShadowLod::Basic, "3 = VeryFar");
    }

    #[test]
    fn every_known_tier_and_beyond_is_covered() {
        // NUM_LOD tiers today, plus a couple past it (a future LodLevel
        // variant this module hasn't been taught about) must not panic and
        // must land on the safe Basic default.
        for lod_index in 0..NUM_LOD + 4 {
            let tier = shadow_lod_for(lod_index);
            if lod_index >= NUM_LOD {
                assert_eq!(tier, ShadowLod::Basic, "unknown tier {lod_index} should default to Basic");
            }
        }
    }

    #[test]
    fn num_lod_matches_documented_tier_count() {
        assert_eq!(NUM_LOD, 4);
    }

    #[test]
    fn basic_amortizes_at_least_as_hard_as_detailed() {
        assert!(stride_multiplier(ShadowLod::Basic) >= stride_multiplier(ShadowLod::Detailed));
        assert_eq!(stride_multiplier(ShadowLod::Detailed), 1, "detailed must not amortize beyond the base stride");
    }

    #[test]
    fn short_name_stable_for_logging() {
        assert_eq!(ShadowLod::Detailed.short_name(), "detailed");
        assert_eq!(ShadowLod::Basic.short_name(), "basic");
    }
}
