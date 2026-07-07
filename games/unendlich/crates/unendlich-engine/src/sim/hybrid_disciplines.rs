//! sim::hybrid_disciplines — access gates for the hybrid / sub / novel benders.
//!
//! From `resonance_bending_spec §6–7`: beyond the primary elements (gated by
//! `sim::affinity_access`), a bender in the right environment unlocks *hybrid*
//! and *novel* disciplines — each requiring several environmental properties to
//! hold at once, with no separate unlock. This module is those multi-property
//! gates, transcribed from the spec, so the move set expands automatically when
//! the world conditions align (a moonlit wetland enables Blood Bending; a deep
//! biomass-rich seabed enables Coral Bending; all four elements balanced plus
//! high resonance enables Temporal Alignment). Pure, deterministic, std-only.
//!
//! Property letters follow the *bending spec* (§2.1/§5): `w` wind, `h` humidity,
//! `r` radiant/heat, `s` stone/soil, `p` pressure, `l` light, `b` biomass — note
//! `l` here is **light** (moonlight/darkness), distinct from `affinity_access`'s
//! lightning channel.

/// Ambient environmental properties in `[0,1]` (spec §2.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvProps {
    pub w: f32,
    pub h: f32,
    pub r: f32,
    pub s: f32,
    pub p: f32,
    pub l: f32,
    pub b: f32,
}

impl EnvProps {
    pub fn zero() -> EnvProps {
        EnvProps { w: 0.0, h: 0.0, r: 0.0, s: 0.0, p: 0.0, l: 0.0, b: 0.0 }
    }
}

/// The hybrid / sub / novel disciplines from §6–7.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HybridDiscipline {
    /// Water + Earth.
    Plantgrowth,
    /// Water + Air.
    VoidMist,
    /// Air + Fire — Pulse.
    Vibrational,
    /// Earth sub — Red Vector.
    Vector,
    /// Water + Earth deep.
    Coral,
    /// Water sub — Moonlit Fluid.
    Blood,
    /// Water sub — Cold State.
    IcePhase,
    /// All four elements + high resonance.
    TemporalAlignment,
}

impl HybridDiscipline {
    pub const ALL: [HybridDiscipline; 8] = [
        HybridDiscipline::Plantgrowth,
        HybridDiscipline::VoidMist,
        HybridDiscipline::Vibrational,
        HybridDiscipline::Vector,
        HybridDiscipline::Coral,
        HybridDiscipline::Blood,
        HybridDiscipline::IcePhase,
        HybridDiscipline::TemporalAlignment,
    ];

    pub fn label(self) -> &'static str {
        match self {
            HybridDiscipline::Plantgrowth => "Plantgrowth",
            HybridDiscipline::VoidMist => "Void Mist",
            HybridDiscipline::Vibrational => "Vibrational",
            HybridDiscipline::Vector => "Vector",
            HybridDiscipline::Coral => "Coral",
            HybridDiscipline::Blood => "Blood",
            HybridDiscipline::IcePhase => "Ice / Phase-Change",
            HybridDiscipline::TemporalAlignment => "Temporal Alignment",
        }
    }
}

/// Resonance tier (0..=3, spec §4.1) required for Temporal Alignment — the top
/// tier.
pub const TEMPORAL_RESONANCE_TIER: u8 = 3;

/// Is `d` accessible given the ambient properties and the player's resonance
/// tier (0..=3)? Gates are transcribed directly from spec §6–7.
pub fn accessible(d: HybridDiscipline, e: &EnvProps, resonance_tier: u8) -> bool {
    match d {
        // H>0.4 AND S>0.2 AND B>0.3 AND L>0.3
        HybridDiscipline::Plantgrowth => e.h > 0.4 && e.s > 0.2 && e.b > 0.3 && e.l > 0.3,
        // H>0.6 AND W>0.4 AND L<0.25
        HybridDiscipline::VoidMist => e.h > 0.6 && e.w > 0.4 && e.l < 0.25,
        // W>0.5 AND R>0.5 AND P<0.4
        HybridDiscipline::Vibrational => e.w > 0.5 && e.r > 0.5 && e.p < 0.4,
        // S>0.7 AND P>0.7
        HybridDiscipline::Vector => e.s > 0.7 && e.p > 0.7,
        // H>0.8 AND S>0.3 AND B>0.5 AND P>0.5
        HybridDiscipline::Coral => e.h > 0.8 && e.s > 0.3 && e.b > 0.5 && e.p > 0.5,
        // L<0.25 AND H>0.3 AND B>0.4 (moonlit)
        HybridDiscipline::Blood => e.l < 0.25 && e.h > 0.3 && e.b > 0.4,
        // H>0.4 AND R<0.2 AND L>0.0 (cold)
        HybridDiscipline::IcePhase => e.h > 0.4 && e.r < 0.2 && e.l > 0.0,
        // H>=0.4 AND R>=0.4 AND W>=0.4 AND S>=0.4 AND top resonance tier
        HybridDiscipline::TemporalAlignment => {
            e.h >= 0.4
                && e.r >= 0.4
                && e.w >= 0.4
                && e.s >= 0.4
                && resonance_tier >= TEMPORAL_RESONANCE_TIER
        }
    }
}

/// All disciplines accessible right now, in `ALL` order.
pub fn all_accessible(e: &EnvProps, resonance_tier: u8) -> Vec<HybridDiscipline> {
    HybridDiscipline::ALL
        .iter()
        .copied()
        .filter(|&d| accessible(d, e, resonance_tier))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(w: f32, h: f32, r: f32, s: f32, p: f32, l: f32, b: f32) -> EnvProps {
        EnvProps { w, h, r, s, p, l, b }
    }

    #[test]
    fn blood_bending_needs_darkness_humidity_and_biomass() {
        // moonlit wetland: low light, wet, alive
        let ok = env(0.0, 0.5, 0.0, 0.0, 0.0, 0.10, 0.6);
        assert!(accessible(HybridDiscipline::Blood, &ok, 0));
        // too bright ⇒ no
        let bright = env(0.0, 0.5, 0.0, 0.0, 0.0, 0.50, 0.6);
        assert!(!accessible(HybridDiscipline::Blood, &bright, 0));
        // no biomass ⇒ no
        let barren = env(0.0, 0.5, 0.0, 0.0, 0.0, 0.10, 0.1);
        assert!(!accessible(HybridDiscipline::Blood, &barren, 0));
    }

    #[test]
    fn coral_needs_deep_wet_rich_high_pressure() {
        let deep = env(0.0, 0.9, 0.0, 0.4, 0.6, 0.0, 0.6);
        assert!(accessible(HybridDiscipline::Coral, &deep, 0));
        let shallow = env(0.0, 0.9, 0.0, 0.4, 0.2, 0.0, 0.6); // low pressure
        assert!(!accessible(HybridDiscipline::Coral, &shallow, 0));
    }

    #[test]
    fn ice_needs_cold_and_wet() {
        let cold = env(0.0, 0.5, 0.1, 0.0, 0.0, 0.5, 0.0);
        assert!(accessible(HybridDiscipline::IcePhase, &cold, 0));
        let warm = env(0.0, 0.5, 0.5, 0.0, 0.0, 0.5, 0.0); // R too high
        assert!(!accessible(HybridDiscipline::IcePhase, &warm, 0));
    }

    #[test]
    fn vector_needs_dense_stone_under_pressure() {
        assert!(accessible(HybridDiscipline::Vector, &env(0.0, 0.0, 0.0, 0.8, 0.8, 0.0, 0.0), 0));
        assert!(!accessible(HybridDiscipline::Vector, &env(0.0, 0.0, 0.0, 0.6, 0.8, 0.0, 0.0), 0));
    }

    #[test]
    fn void_mist_and_vibrational_gates() {
        // Void Mist: humid, windy, dark
        assert!(accessible(HybridDiscipline::VoidMist, &env(0.5, 0.7, 0.0, 0.0, 0.0, 0.1, 0.0), 0));
        // Vibrational: windy, hot, low pressure
        assert!(accessible(HybridDiscipline::Vibrational, &env(0.6, 0.0, 0.6, 0.0, 0.3, 0.0, 0.0), 0));
        assert!(!accessible(HybridDiscipline::Vibrational, &env(0.6, 0.0, 0.6, 0.0, 0.5, 0.0, 0.0), 0), "pressure too high");
    }

    #[test]
    fn temporal_alignment_needs_all_four_balanced_and_top_resonance() {
        let balanced = env(0.5, 0.5, 0.5, 0.5, 0.0, 0.0, 0.0);
        assert!(!accessible(HybridDiscipline::TemporalAlignment, &balanced, 2), "needs top tier");
        assert!(accessible(HybridDiscipline::TemporalAlignment, &balanced, 3));
        let unbalanced = env(0.5, 0.5, 0.5, 0.3, 0.0, 0.0, 0.0); // S too low
        assert!(!accessible(HybridDiscipline::TemporalAlignment, &unbalanced, 3));
    }

    #[test]
    fn all_accessible_collects_the_active_set() {
        // moonlit wetland at rest: Blood should be in the set, Coral not.
        let e = env(0.0, 0.5, 0.0, 0.0, 0.0, 0.1, 0.6);
        let set = all_accessible(&e, 0);
        assert!(set.contains(&HybridDiscipline::Blood));
        assert!(!set.contains(&HybridDiscipline::Coral));
    }

    #[test]
    fn deterministic() {
        let e = env(0.5, 0.7, 0.3, 0.4, 0.5, 0.1, 0.5);
        assert_eq!(all_accessible(&e, 3), all_accessible(&e, 3));
    }
}
