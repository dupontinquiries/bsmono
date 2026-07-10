//! r688: bending visual-effect parameters — from
//! `02_Bending_and_Disciplines/bending_shaders.docx`.
//!
//! The doc fixes the exact look of every bending effect: the Stormstrike
//! **lightning arc** (a white plasma core inside a blue-white penumbra, with a
//! broad electrical aura where it activates metal), the Heavy-Brine held charge,
//! the Thermal-Phase failure cracks and steam flash, and the Information
//! discipline's gold light-path lines. Each is specified as an HDR-linear colour,
//! a peak emissive multiplier (pre-tonemap, needs an FP16 framebuffer), and a
//! bloom radius in screen pixels @1080p.
//!
//! This is the pure parameter table + a quality-LOD scale — no GPU, no draw. The
//! renderer's emissive/bloom pass reads `params(effect)` to draw each effect with
//! the doc-correct colour/intensity/bloom; the held-item / weapon renderer reads
//! `discipline_core` to tint a charged item by its discipline. Deterministic.

/// The four bending disciplines with a distinct on-screen identity (doc §1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Discipline {
    /// ⚡ Electrical architecture — white plasma core, blue-white penumbra, no fire.
    Stormstrike,
    /// 🟣 Dense mineral suspension — purple-black manganese, viscous, iridescent.
    HeavyBrine,
    /// 🔥 Diagnostic, not destructive — structural-failure gradients, white steam.
    ThermalPhase,
    /// ✨ Near-invisible — only gold light-path lines mark the observation geometry.
    Information,
}

/// The discrete bending effects the doc gives explicit render parameters for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// The sharp hot centre of a lightning arc — pure white, very narrow bloom.
    ArcCore,
    /// The cool blue-white halo around the arc — wide bloom.
    ArcPenumbra,
    /// The broad electrical aura where an arc energises metal.
    MetalActivation,
    /// The contained glow inside a Stormstrike Held-Charge orb.
    HeldChargeInterior,
    /// A thin Thermal-Phase failure crack, backlit hot.
    FailureCrack,
    /// The concussive white Flash-Steam-Shock flash — very wide, fills the periphery.
    SteamFlash,
    /// Information discipline's gold light-path lines — sub-1.0, no bloom.
    LightPathLines,
}

/// Render parameters for one [`Effect`] (doc §3 table).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FxParams {
    /// HDR-linear RGB colour (pre-tonemap).
    pub color: [f32; 3],
    /// Peak emissive multiplier (e.g. `6.0` = 6.0×). Needs an FP16 framebuffer.
    pub peak_emissive: f32,
    /// Bloom radius in screen pixels @1080p. `0.0` = no bloom (sub-1.0 emissive).
    pub bloom_radius_px: f32,
}

impl Effect {
    /// The doc-specified parameters for this effect. Values are transcribed from
    /// the bending_shaders.docx effect table (colours HDR-linear, pre-tonemap).
    pub fn params(self) -> FxParams {
        match self {
            Effect::ArcCore => FxParams { color: [1.00, 1.00, 1.00], peak_emissive: 6.0, bloom_radius_px: 2.0 },
            Effect::ArcPenumbra => FxParams { color: [0.70, 0.82, 1.00], peak_emissive: 1.8, bloom_radius_px: 18.0 },
            Effect::MetalActivation => FxParams { color: [0.50, 0.72, 1.00], peak_emissive: 2.8, bloom_radius_px: 25.0 },
            Effect::HeldChargeInterior => FxParams { color: [0.55, 0.75, 1.00], peak_emissive: 4.0, bloom_radius_px: 0.0 },
            Effect::FailureCrack => FxParams { color: [1.00, 0.55, 0.00], peak_emissive: 6.0, bloom_radius_px: 4.0 },
            Effect::SteamFlash => FxParams { color: [1.00, 1.00, 1.00], peak_emissive: 2.5, bloom_radius_px: 60.0 },
            Effect::LightPathLines => FxParams { color: [1.00, 0.88, 0.35], peak_emissive: 0.8, bloom_radius_px: 0.0 },
        }
    }

    /// True if this effect emits above 1.0 and therefore blooms (needs the HDR path).
    pub fn blooms(self) -> bool {
        let p = self.params();
        p.peak_emissive > 1.0 && p.bloom_radius_px > 0.0
    }
}

impl Discipline {
    /// A single representative "core" colour + peak emissive for tinting a
    /// charged held item / weapon by its discipline (the held-item renderer reads
    /// this so a Stormstrike staff crackles white-blue, a Brine one glows purple).
    pub fn core_fx(self) -> FxParams {
        match self {
            Discipline::Stormstrike => Effect::ArcCore.params(),
            Discipline::HeavyBrine => FxParams { color: [0.32, 0.10, 0.38], peak_emissive: 1.2, bloom_radius_px: 8.0 },
            Discipline::ThermalPhase => Effect::FailureCrack.params(),
            Discipline::Information => Effect::LightPathLines.params(),
        }
    }
}

/// Render quality tiers (doc §4 LOD table) — scale sample counts / bloom fidelity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Ultra,
    High,
    Medium,
    Low,
}

impl Quality {
    /// Brine-mist ray-sample count for volumetrics at this tier (doc §4).
    pub fn brine_mist_samples(self) -> u32 {
        match self {
            Quality::Ultra => 64,
            Quality::High => 32,
            Quality::Medium => 16,
            Quality::Low => 8,
        }
    }

    /// Fraction to scale a bloom radius by at this tier (cheaper glow when low).
    pub fn bloom_scale(self) -> f32 {
        match self {
            Quality::Ultra => 1.0,
            Quality::High => 1.0,
            Quality::Medium => 0.75,
            Quality::Low => 0.5,
        }
    }

    /// Whether the per-pixel Conduction-Read overlay is drawn (off at Low).
    pub fn conduction_overlay(self) -> bool {
        !matches!(self, Quality::Low)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_core_is_pure_white_and_brightest() {
        let core = Effect::ArcCore.params();
        assert_eq!(core.color, [1.0, 1.0, 1.0]);
        assert_eq!(core.peak_emissive, 6.0);
        // the core blooms tighter than the penumbra
        assert!(core.bloom_radius_px < Effect::ArcPenumbra.params().bloom_radius_px);
    }

    #[test]
    fn metal_activation_has_the_widest_arc_aura() {
        let m = Effect::MetalActivation.params();
        assert!(m.bloom_radius_px > Effect::ArcPenumbra.params().bloom_radius_px);
        assert!(m.color[2] > m.color[0]); // blue-dominant electrical aura
    }

    #[test]
    fn steam_flash_is_the_widest_of_all() {
        let all = [
            Effect::ArcCore, Effect::ArcPenumbra, Effect::MetalActivation,
            Effect::HeldChargeInterior, Effect::FailureCrack, Effect::SteamFlash,
            Effect::LightPathLines,
        ];
        let widest = all.iter().copied().max_by(|a, b| {
            a.params().bloom_radius_px.partial_cmp(&b.params().bloom_radius_px).unwrap()
        }).unwrap();
        assert_eq!(widest, Effect::SteamFlash);
    }

    #[test]
    fn light_path_lines_are_gold_and_do_not_bloom() {
        let l = Effect::LightPathLines.params();
        assert!(l.color[0] > l.color[2]); // warm/gold, not blue
        assert!(!Effect::LightPathLines.blooms()); // sub-1.0 emissive
    }

    #[test]
    fn failure_crack_is_hot_and_blooms() {
        let c = Effect::FailureCrack.params();
        assert!(c.color[0] > c.color[1] && c.color[1] > c.color[2]); // hot orange
        assert!(Effect::FailureCrack.blooms());
    }

    #[test]
    fn quality_scales_down_monotonically() {
        assert!(Quality::Ultra.brine_mist_samples() > Quality::Low.brine_mist_samples());
        assert!(Quality::Ultra.bloom_scale() >= Quality::Low.bloom_scale());
        assert!(Quality::Ultra.conduction_overlay() && !Quality::Low.conduction_overlay());
    }

    #[test]
    fn stormstrike_core_matches_the_arc_core() {
        assert_eq!(Discipline::Stormstrike.core_fx(), Effect::ArcCore.params());
    }
}
