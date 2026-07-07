//! r677: waterfall POI environmental profiles — from
//! `01_World_and_Environment/fluid_pois_waterfalls.docx` §1.1.
//!
//! A waterfall is a *singularity* in the river flow field, and around it the
//! local environmental properties the bending systems read (H humidity,
//! R radiant-heat, W wind, P pressure, S seismic, L luminosity, B brine/mineral
//! richness) are shifted by a POI-type-specific profile. The doc gives the peak
//! deltas within 30 blocks of the centroid, decaying with an inverse-square
//! falloff to zero at 120 blocks.
//!
//! This is the pure table + falloff: it composes with `sim::bending_field`
//! (which produces the base ambient fields) and thereby `sim::affinity_access`
//! (which gates bending on them) — a Thermal Waterfall's +R/+B nudges Fire and
//! Brine access up nearby; a Frozen Waterfall's −R nudges it down. Deterministic,
//! std-only.
//!
//! Property order is the doc's: `[H, R, W, P, S, L, B]`. (Note this differs from
//! `bending_field`'s field order; callers map by name, not index, when bridging.)

/// Index of each property in the 7-element modifier arrays (doc column order).
pub const H: usize = 0; // humidity
pub const R: usize = 1; // radiant heat
pub const W: usize = 2; // wind
pub const P: usize = 3; // pressure
pub const S: usize = 4; // seismic
pub const L: usize = 5; // luminosity
pub const B: usize = 6; // brine / base-material mineral richness

/// Peak within this radius (blocks): the full modifier applies.
pub const PEAK_RADIUS: f32 = 30.0;
/// Modifiers reach zero at this radius (blocks).
pub const ZERO_RADIUS: f32 = 120.0;

/// The ten waterfall POI archetypes (doc §1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaterfallPoi {
    AlpinePlunge,
    TieredTufa,
    Curtain,
    VeilGate,
    CaveWaterfall,
    FrozenWaterfall,
    ThermalWaterfall,
    Lavafall,
    MegaWaterfall,
    HybridLavaWater,
}

impl WaterfallPoi {
    pub const ALL: [WaterfallPoi; 10] = [
        WaterfallPoi::AlpinePlunge,
        WaterfallPoi::TieredTufa,
        WaterfallPoi::Curtain,
        WaterfallPoi::VeilGate,
        WaterfallPoi::CaveWaterfall,
        WaterfallPoi::FrozenWaterfall,
        WaterfallPoi::ThermalWaterfall,
        WaterfallPoi::Lavafall,
        WaterfallPoi::MegaWaterfall,
        WaterfallPoi::HybridLavaWater,
    ];

    pub fn name(self) -> &'static str {
        match self {
            WaterfallPoi::AlpinePlunge => "Alpine Plunge",
            WaterfallPoi::TieredTufa => "Tiered / Tufa",
            WaterfallPoi::Curtain => "Curtain",
            WaterfallPoi::VeilGate => "Veil Gate",
            WaterfallPoi::CaveWaterfall => "Cave Waterfall",
            WaterfallPoi::FrozenWaterfall => "Frozen Waterfall",
            WaterfallPoi::ThermalWaterfall => "Thermal Waterfall",
            WaterfallPoi::Lavafall => "Lavafall",
            WaterfallPoi::MegaWaterfall => "Mega Waterfall",
            WaterfallPoi::HybridLavaWater => "Hybrid Lava-Water",
        }
    }

    /// Peak property deltas `[H, R, W, P, S, L, B]` at the centroid, digitised
    /// from the doc's §1.1 summary table. (Sparse-row zero columns for Cave /
    /// Lavafall are read conservatively; all values are tunable.)
    pub fn peak_modifiers(self) -> [f32; 7] {
        match self {
            WaterfallPoi::AlpinePlunge => [0.35, -0.20, 0.18, -0.05, 0.08, -0.05, 0.00],
            WaterfallPoi::TieredTufa => [0.30, -0.10, 0.08, 0.00, 0.04, 0.06, 0.12],
            WaterfallPoi::Curtain => [0.40, -0.15, 0.22, -0.08, 0.00, 0.08, 0.00],
            WaterfallPoi::VeilGate => [0.48, -0.18, 0.05, 0.10, 0.00, -0.15, 0.00],
            WaterfallPoi::CaveWaterfall => [0.42, 0.20, 0.10, -0.25, 0.00, 0.08, 0.00],
            WaterfallPoi::FrozenWaterfall => [0.15, -0.40, 0.05, 0.05, 0.00, 0.12, 0.05],
            WaterfallPoi::ThermalWaterfall => [0.38, 0.30, 0.12, 0.08, 0.06, -0.08, 0.20],
            WaterfallPoi::Lavafall => [0.05, 0.70, 0.15, 0.12, 0.00, -0.10, 0.30],
            WaterfallPoi::MegaWaterfall => [0.55, -0.20, 0.35, -0.15, 0.05, 0.05, 0.00],
            WaterfallPoi::HybridLavaWater => [0.25, 0.50, 0.28, 0.15, 0.15, -0.20, 0.25],
        }
    }

    /// RT60 reverberation-time range in **seconds** (doc §2) — how long sound
    /// takes to decay 60 dB at the POI. Cave/Mega falls are cathedral-scale
    /// (10–30 s, "the most extreme natural echo in the world"); frozen falls are
    /// damped (2–6 s, brittle/percussive). Feeds the Choir/voice-echo and the
    /// Lauscher acoustic ecology: longer RT60 = more distinct echo repeats and
    /// more audio-disoriented Lauschers nearby. Returns `(min, max)`.
    pub fn rt60_range(self) -> (f32, f32) {
        match self {
            WaterfallPoi::AlpinePlunge => (3.0, 10.0),
            WaterfallPoi::TieredTufa => (4.0, 12.0),
            WaterfallPoi::Curtain => (6.0, 15.0),
            WaterfallPoi::VeilGate => (6.0, 18.0),
            WaterfallPoi::CaveWaterfall => (10.0, 30.0),
            WaterfallPoi::FrozenWaterfall => (2.0, 6.0),
            WaterfallPoi::ThermalWaterfall => (3.0, 8.0),
            WaterfallPoi::Lavafall => (2.0, 8.0),
            WaterfallPoi::MegaWaterfall => (10.0, 30.0),
            // doc's Hybrid consequence row is unclear in the source; mid-range.
            WaterfallPoi::HybridLavaWater => (3.0, 10.0),
        }
    }

    /// Midpoint RT60 (seconds) — a single representative reverb time.
    pub fn rt60_typical(self) -> f32 {
        let (lo, hi) = self.rt60_range();
        0.5 * (lo + hi)
    }
}

/// Distance falloff in `[0,1]`: full (1.0) within `PEAK_RADIUS`, then an
/// inverse-square-shaped decay to 0.0 at `ZERO_RADIUS`, and 0 beyond.
pub fn falloff(dist_blocks: f32) -> f32 {
    if dist_blocks <= PEAK_RADIUS {
        1.0
    } else if dist_blocks >= ZERO_RADIUS {
        0.0
    } else {
        let t = (ZERO_RADIUS - dist_blocks) / (ZERO_RADIUS - PEAK_RADIUS); // 1→0
        t * t // quadratic (inverse-square) shape
    }
}

/// The property deltas a `poi` contributes at `dist_blocks` from its centroid:
/// `peak_modifiers · falloff(dist)`.
pub fn modifiers_at(poi: WaterfallPoi, dist_blocks: f32) -> [f32; 7] {
    let f = falloff(dist_blocks);
    let peak = poi.peak_modifiers();
    let mut out = [0.0f32; 7];
    for i in 0..7 {
        out[i] = peak[i] * f;
    }
    out
}

/// Apply one waterfall's contribution onto a base property vector `[H..B]` at a
/// distance, clamping each property back into `[0, 1]`.
pub fn apply(base: [f32; 7], poi: WaterfallPoi, dist_blocks: f32) -> [f32; 7] {
    let m = modifiers_at(poi, dist_blocks);
    let mut out = base;
    for i in 0..7 {
        out[i] = (out[i] + m[i]).clamp(0.0, 1.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falloff_is_full_then_quadratic_then_zero() {
        assert_eq!(falloff(0.0), 1.0);
        assert_eq!(falloff(PEAK_RADIUS), 1.0);
        assert_eq!(falloff(ZERO_RADIUS), 0.0);
        assert_eq!(falloff(200.0), 0.0);
        // midpoint (75 blocks): t = (120-75)/90 = 0.5 → 0.25
        assert!((falloff(75.0) - 0.25).abs() < 1e-6);
        // monotone non-increasing
        assert!(falloff(40.0) > falloff(80.0));
    }

    #[test]
    fn thermal_waterfall_raises_heat_and_brine_nearby() {
        let m = modifiers_at(WaterfallPoi::ThermalWaterfall, 10.0);
        assert!(m[R] > 0.0, "thermal falls are hot");
        assert!(m[B] > 0.0, "thermal falls deposit minerals (brine)");
        // frozen falls do the opposite for heat
        assert!(WaterfallPoi::FrozenWaterfall.peak_modifiers()[R] < 0.0);
        // lavafall is the hottest
        assert!(
            WaterfallPoi::Lavafall.peak_modifiers()[R]
                > WaterfallPoi::ThermalWaterfall.peak_modifiers()[R]
        );
    }

    #[test]
    fn every_poi_raises_humidity() {
        // Water POIs all add humidity (H > 0) — even the lavafall's spray.
        for poi in WaterfallPoi::ALL {
            assert!(poi.peak_modifiers()[H] > 0.0, "{} should add humidity", poi.name());
        }
    }

    #[test]
    fn apply_clamps_into_unit_range() {
        // A base already near 1 for humidity can't exceed 1 after a wet POI.
        let base = [0.95, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5];
        let out = apply(base, WaterfallPoi::MegaWaterfall, 5.0);
        assert!(out[H] <= 1.0 && out[H] >= 0.0);
        // far away → base unchanged
        let far = apply(base, WaterfallPoi::MegaWaterfall, 500.0);
        assert_eq!(far, base);
    }

    #[test]
    fn distance_scales_the_effect() {
        let near = modifiers_at(WaterfallPoi::Curtain, 10.0)[W];
        let mid = modifiers_at(WaterfallPoi::Curtain, 75.0)[W];
        assert!(near > mid && mid > 0.0);
    }

    #[test]
    fn rt60_cave_is_cathedral_scale_frozen_is_damped() {
        assert_eq!(WaterfallPoi::CaveWaterfall.rt60_range(), (10.0, 30.0));
        assert!(
            WaterfallPoi::CaveWaterfall.rt60_typical()
                > WaterfallPoi::FrozenWaterfall.rt60_typical()
        );
        // every POI has a positive, ordered range
        for poi in WaterfallPoi::ALL {
            let (lo, hi) = poi.rt60_range();
            assert!(lo > 0.0 && hi >= lo, "{} range {:?}", poi.name(), (lo, hi));
        }
    }
}
