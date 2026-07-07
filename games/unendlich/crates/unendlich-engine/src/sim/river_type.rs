//! sim::river_type — the seven river types and their gameplay profiles.
//!
//! From `river_systems §2/§10`: rivers come in seven characters, each with a
//! distinct flow speed, wave-surf reach, submerged-swimming feel, and value to a
//! drinking dragon. The world already carves rivers (`river_carve_field`) and
//! pushes bodies along them (`current_force`/`flow_field`); this adds the *type*
//! layer — a classification plus the per-type numbers systems read: how fast the
//! current runs, how far a Wave Surf carries, how clear the water is when
//! submerged (feeds the swim camera), and how much a drink refills a dragon's
//! energy. Values are the doc's table midpoints. Composes with `swim`/
//! `locomotion_mode` (camera), `dragon_energy` (thirst), and water bending.
//! Pure, deterministic, std-only.

/// How rough the surface is (affects wave-surf stability and hazards).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Turbulence {
    VeryLow,
    Low,
    Moderate,
    High,
    Extreme,
}

/// The seven river types (§2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiverType {
    /// Alpine / mountain — fast, extreme turbulence, crystal clear.
    Alpine,
    /// Glacial — cold, milky turquoise, opaque.
    Glacial,
    /// Braided — fragmented channels over gravel.
    Braided,
    /// Meandering — slow lowland river, sustains a wave indefinitely.
    Meandering,
    /// Blackwater — slow, dark, near-invisible (stealth transit).
    Blackwater,
    /// Whitewater — sediment-heavy rapids, near-zero visibility.
    Whitewater,
    /// Spring-Fed — the clearest water of all.
    SpringFed,
}

/// The gameplay numbers for a river type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RiverProfile {
    /// Representative surface flow speed (m/s).
    pub flow_speed: f32,
    pub turbulence: Turbulence,
    /// How far a Wave Surf carries a rider (blocks).
    pub wave_max_travel: f32,
    /// Camera drift as a fraction of flow while submerged (0..1).
    pub drift_fraction: f32,
    /// Underwater visibility (blocks) — feeds fog / swim camera clarity.
    pub submerged_visibility: f32,
    /// Energy a drinking dragon regains per drink event (fraction, 0..1).
    pub dragon_thirst_refill: f32,
}

impl RiverType {
    pub const ALL: [RiverType; 7] = [
        RiverType::Alpine,
        RiverType::Glacial,
        RiverType::Braided,
        RiverType::Meandering,
        RiverType::Blackwater,
        RiverType::Whitewater,
        RiverType::SpringFed,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RiverType::Alpine => "Alpine",
            RiverType::Glacial => "Glacial",
            RiverType::Braided => "Braided",
            RiverType::Meandering => "Meandering",
            RiverType::Blackwater => "Blackwater",
            RiverType::Whitewater => "Whitewater",
            RiverType::SpringFed => "Spring-Fed",
        }
    }

    /// The full gameplay profile (doc table midpoints).
    pub fn profile(self) -> RiverProfile {
        use Turbulence::*;
        match self {
            RiverType::Alpine => RiverProfile {
                flow_speed: 5.0,
                turbulence: Extreme,
                wave_max_travel: 80.0,
                drift_fraction: 0.70,
                submerged_visibility: 20.0,
                dragon_thirst_refill: 0.11,
            },
            RiverType::Glacial => RiverProfile {
                flow_speed: 3.5,
                turbulence: High,
                wave_max_travel: 100.0,
                drift_fraction: 0.60,
                submerged_visibility: 6.0,
                dragon_thirst_refill: 0.13,
            },
            RiverType::Braided => RiverProfile {
                flow_speed: 3.5,
                turbulence: Moderate,
                wave_max_travel: 40.0,
                drift_fraction: 0.40,
                submerged_visibility: 10.0,
                dragon_thirst_refill: 0.07,
            },
            RiverType::Meandering => RiverProfile {
                flow_speed: 1.2,
                turbulence: Low,
                wave_max_travel: 300.0,
                drift_fraction: 0.30,
                submerged_visibility: 13.0,
                dragon_thirst_refill: 0.15,
            },
            RiverType::Blackwater => RiverProfile {
                flow_speed: 1.75,
                turbulence: VeryLow,
                wave_max_travel: 150.0,
                drift_fraction: 0.20,
                submerged_visibility: 4.0,
                dragon_thirst_refill: 0.09,
            },
            RiverType::Whitewater => RiverProfile {
                flow_speed: 4.0,
                turbulence: Extreme,
                wave_max_travel: 60.0,
                drift_fraction: 0.50,
                submerged_visibility: 2.0,
                dragon_thirst_refill: 0.06,
            },
            RiverType::SpringFed => RiverProfile {
                flow_speed: 2.0,
                turbulence: VeryLow,
                wave_max_travel: 200.0,
                drift_fraction: 0.15,
                submerged_visibility: 42.0,
                dragon_thirst_refill: 0.14,
            },
        }
    }

    /// Convenience: current speed (m/s) for `current_force`.
    pub fn flow_speed(self) -> f32 {
        self.profile().flow_speed
    }
    /// Convenience: dragon energy per drink for `dragon_energy`.
    pub fn dragon_thirst_refill(self) -> f32 {
        self.profile().dragon_thirst_refill
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_has_a_label_and_sane_profile() {
        for t in RiverType::ALL {
            assert!(!t.label().is_empty());
            let p = t.profile();
            assert!(p.flow_speed > 0.0);
            assert!(p.wave_max_travel > 0.0);
            assert!((0.0..=1.0).contains(&p.drift_fraction));
            assert!(p.submerged_visibility > 0.0);
            assert!((0.0..=1.0).contains(&p.dragon_thirst_refill));
        }
    }

    #[test]
    fn meandering_sustains_the_longest_wave() {
        let longest = RiverType::ALL.iter().max_by(|a, b| {
            a.profile().wave_max_travel.partial_cmp(&b.profile().wave_max_travel).unwrap()
        });
        assert_eq!(longest, Some(&RiverType::Meandering));
    }

    #[test]
    fn spring_fed_is_clearest_and_whitewater_murkiest() {
        let clearest = RiverType::ALL.iter().max_by(|a, b| {
            a.profile().submerged_visibility.partial_cmp(&b.profile().submerged_visibility).unwrap()
        });
        let murkiest = RiverType::ALL.iter().min_by(|a, b| {
            a.profile().submerged_visibility.partial_cmp(&b.profile().submerged_visibility).unwrap()
        });
        assert_eq!(clearest, Some(&RiverType::SpringFed));
        assert_eq!(murkiest, Some(&RiverType::Whitewater));
    }

    #[test]
    fn fast_rivers_drift_the_camera_more() {
        assert!(RiverType::Alpine.profile().drift_fraction > RiverType::SpringFed.profile().drift_fraction);
    }

    #[test]
    fn turbulence_ranks() {
        assert!(RiverType::Whitewater.profile().turbulence >= Turbulence::High);
        assert!(RiverType::Blackwater.profile().turbulence <= Turbulence::Low);
    }

    #[test]
    fn convenience_accessors_match_profile() {
        for t in RiverType::ALL {
            assert_eq!(t.flow_speed(), t.profile().flow_speed);
            assert_eq!(t.dragon_thirst_refill(), t.profile().dragon_thirst_refill);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(RiverType::Meandering.profile(), RiverType::Meandering.profile());
    }
}
