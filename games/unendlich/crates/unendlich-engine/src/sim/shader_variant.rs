//! `shader_variant` — pick the cheapest chunk shader variant per chunk
//! (EXPERIMENTAL, r244).
//!
//! `chunk.frag` gates its expensive per-fragment features behind compile-time
//! flags (`CHUNK_FOG`, `CHUNK_FOG_STEPS`, `CHUNK_WATER`, `CHUNK_DYNAMIC_LIGHTS`,
//! `CHUNK_SHADOWS`). The engine compiles a handful of **variants** of the
//! shader with some features removed, then binds the lightest variant a chunk
//! actually needs — a far, flat, waterless chunk doesn't pay for reflections,
//! a point-light loop, full fog steps, or shadow PCF it can't meaningfully
//! show. Quality loss is minimal because the removed work isn't visible at that
//! distance / in that content; the cost saving is per fragment, so it scales
//! with how much of the screen those chunks cover.
//!
//! This module is the **decision**: chunk properties in → the variant flag-set
//! out. Pure + deterministic + unit-tested. Producing the actual variant
//! pipelines and binding them is the gated integration (`shader-variants`
//! feature); see `docs/design/shader_variants.md`.
//!
//! Quality-preserving rules (conservative — when unsure, keep the feature):
//! - **Water shading** only matters if the chunk *contains water*. No water →
//!   drop `CHUNK_WATER` (reflection/refraction/caustics/glint) with zero
//!   visible change.
//! - **Dynamic (point) lights** only matter if a light is near the chunk. No
//!   nearby dynamic light → sun + ambient only.
//! - **Shadow PCF** is the classic distance cull: distant chunks' self-shadows
//!   are sub-pixel. Past a shadow distance, drop `CHUNK_SHADOWS` (fully lit).
//! - **Fog steps** scale down with distance bands — near chunks get the full
//!   march (fog is most visible close, in the depth gradient you move through);
//!   far chunks are mostly a flat aerial-perspective tint that a 1–2 step march
//!   (or the froxel volume) covers. Never below 1 while fog is on.
//!
//! The "high" variant (everything on, full steps) is byte-identical to the
//! pre-variant shader, so anything the rules keep is unchanged.

#![allow(dead_code)]

use crate::world::scheduler::{ChunkTickBand, TickScheduleParams};

/// What the engine knows about a chunk when choosing its shader variant.
#[derive(Clone, Copy, Debug)]
pub struct ChunkShadeInfo {
    /// Distance band (reuses the streaming bands: Render = near, Simulation =
    /// mid/far, CatchUp = farthest still drawn).
    pub band: ChunkTickBand,
    /// The chunk contains at least one water voxel (surface or volume).
    pub has_water: bool,
    /// A dynamic/point light source is within influence range of the chunk.
    pub has_dynamic_light: bool,
    /// The chunk is within the shadow-casting distance (self-shadows visible).
    pub within_shadow_dist: bool,
}

/// The variant flag-set handed to the shader compiler (one set of `-D` defines)
/// and used as the pipeline key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChunkVariant {
    pub fog: bool,
    pub fog_steps: u8,
    pub water: bool,
    pub dynamic_lights: bool,
    pub shadows: bool,
}

impl ChunkVariant {
    /// The full-quality variant — byte-identical to the un-gated shader.
    pub const HIGH: ChunkVariant = ChunkVariant {
        fog: true,
        fog_steps: 6,
        water: true,
        dynamic_lights: true,
        shadows: true,
    };

    /// The `-D` defines for this variant, for the shader compiler. Stable order
    /// so the same variant always yields the same key/command.
    pub fn defines(&self) -> Vec<String> {
        vec![
            format!("CHUNK_FOG={}", self.fog as u8),
            format!("CHUNK_FOG_STEPS={}", self.fog_steps.max(1)),
            format!("CHUNK_WATER={}", self.water as u8),
            format!("CHUNK_DYNAMIC_LIGHTS={}", self.dynamic_lights as u8),
            format!("CHUNK_SHADOWS={}", self.shadows as u8),
        ]
    }

    /// A compact key for caching the compiled pipeline (one bit per flag +
    /// steps). Distinct variants → distinct keys.
    pub fn key(&self) -> u32 {
        (self.fog as u32)
            | ((self.water as u32) << 1)
            | ((self.dynamic_lights as u32) << 2)
            | ((self.shadows as u32) << 3)
            | ((self.fog_steps as u32) << 8)
    }
}

/// Fog step counts per distance band (near gets the full march; far is mostly
/// flat aerial tint). Tunable.
fn fog_steps_for_band(band: ChunkTickBand) -> u8 {
    match band {
        ChunkTickBand::Render => 6,     // near: full march
        ChunkTickBand::Simulation => 3, // mid: half
        ChunkTickBand::CatchUp => 2,    // far: minimal (aerial tint dominates)
    }
}

/// Choose the cheapest variant that preserves acceptable quality for a chunk.
/// Conservative: a feature is only dropped when it cannot meaningfully show.
pub fn select(info: &ChunkShadeInfo, _p: &TickScheduleParams) -> ChunkVariant {
    ChunkVariant {
        // fog stays on at all bands (aerial perspective matters far too), but
        // the step count drops with distance.
        fog: true,
        fog_steps: fog_steps_for_band(info.band),
        // water shading only if there's water to shade.
        water: info.has_water,
        // point-light loop only if a dynamic light is in range.
        dynamic_lights: info.has_dynamic_light,
        // shadow PCF only within shadow distance.
        shadows: info.within_shadow_dist,
    }
}

/// Count of distinct variants the engine could need (for sizing the pipeline
/// cache). Not all are realised — only those `select` actually returns — but
/// this bounds it: 3 fog-step tiers × 2 water × 2 lights × 2 shadows = 24.
pub const MAX_VARIANTS: usize = 3 * 2 * 2 * 2;

#[cfg(test)]
mod tests {
    use super::*;

    fn info(band: ChunkTickBand, water: bool, light: bool, shadow: bool) -> ChunkShadeInfo {
        ChunkShadeInfo { band, has_water: water, has_dynamic_light: light, within_shadow_dist: shadow }
    }

    #[test]
    fn near_full_water_lit_shadowed_is_high_ish() {
        let p = TickScheduleParams::default();
        let v = select(&info(ChunkTickBand::Render, true, true, true), &p);
        // near chunk with everything present → full quality
        assert_eq!(v, ChunkVariant::HIGH);
    }

    #[test]
    fn far_flat_dry_chunk_drops_water_and_shadows() {
        let p = TickScheduleParams::default();
        // far, no water, no dynamic light, beyond shadow distance
        let v = select(&info(ChunkTickBand::CatchUp, false, false, false), &p);
        assert!(!v.water, "no water → no water shading");
        assert!(!v.dynamic_lights, "no light → no point-light loop");
        assert!(!v.shadows, "beyond shadow dist → no PCF");
        assert!(v.fog, "fog (aerial tint) stays on far");
        assert_eq!(v.fog_steps, 2, "far → minimal fog march");
        // and it's cheaper than HIGH
        assert_ne!(v, ChunkVariant::HIGH);
    }

    #[test]
    fn water_chunk_keeps_water_even_when_far() {
        let p = TickScheduleParams::default();
        let v = select(&info(ChunkTickBand::CatchUp, true, false, false), &p);
        assert!(v.water, "a distant lake still needs water shading to read as water");
    }

    #[test]
    fn dynamic_light_kept_when_present() {
        let p = TickScheduleParams::default();
        let v = select(&info(ChunkTickBand::Render, false, true, true), &p);
        assert!(v.dynamic_lights);
    }

    #[test]
    fn fog_steps_decrease_with_distance() {
        let p = TickScheduleParams::default();
        let near = select(&info(ChunkTickBand::Render, false, false, true), &p).fog_steps;
        let mid = select(&info(ChunkTickBand::Simulation, false, false, true), &p).fog_steps;
        let far = select(&info(ChunkTickBand::CatchUp, false, false, false), &p).fog_steps;
        assert!(near > mid && mid > far, "near {near} > mid {mid} > far {far}");
        assert!(far >= 1, "never zero fog steps while fog is on");
    }

    #[test]
    fn defines_are_stable_and_complete() {
        let d = ChunkVariant::HIGH.defines();
        assert_eq!(d.len(), 5);
        assert!(d.iter().any(|s| s == "CHUNK_FOG=1"));
        assert!(d.iter().any(|s| s == "CHUNK_SHADOWS=1"));
        assert!(d.iter().any(|s| s == "CHUNK_FOG_STEPS=6"));
    }

    #[test]
    fn distinct_variants_distinct_keys() {
        let a = ChunkVariant::HIGH;
        let b = ChunkVariant { shadows: false, ..a };
        let c = ChunkVariant { water: false, ..a };
        let d = ChunkVariant { fog_steps: 2, ..a };
        assert_ne!(a.key(), b.key());
        assert_ne!(a.key(), c.key());
        assert_ne!(a.key(), d.key());
        assert_ne!(b.key(), c.key());
    }
}
