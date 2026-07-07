//! `sim::secondary_stats` — the three computed character stats shown under the
//! POWER resonance rating: **Mobility**, **Resilience**, **Recovery**. The player
//! never sets these; they emerge from equipped gear, active bending discipline,
//! companion bonds, passives, level, and location, and they can shift without a
//! gear change (entering a location, bonding a companion, toggling a passive).
//!
//! This is the pure derivation: contributor tables (gear weight, discipline,
//! armour material) plus the three aggregation formulas and their downstream
//! effects — move-speed multiplier, damage reduction, and chi-regen rate. The
//! caller supplies the situational bonuses (passives, enchants, companion,
//! location) as summed inputs so the core stays independent of the progression
//! systems that produce them. Pure/std-only `f32` math.
//!
//! Formulas and the worked "Rootbender Level 50" examples come straight from the
//! Secondary Stats spec and are pinned in the tests.

// --- ranges & bases ---
pub const MOBILITY_MIN: f32 = 10.0;
pub const MOBILITY_MAX: f32 = 120.0;
/// Baseline Mobility: movement speed is `mobility / MOBILITY_BASELINE`.
pub const MOBILITY_BASELINE: f32 = 50.0;

pub const RESILIENCE_MIN: f32 = 0.0;
pub const RESILIENCE_MAX: f32 = 130.0;
/// Damage reduction per point below the diminishing knee (≈3.5% per 10 pts).
pub const RESIL_RATE: f32 = 0.0035;
/// Above this Resilience the per-point mitigation is softened.
pub const RESIL_KNEE: f32 = 80.0;
/// Post-knee mitigation efficiency.
pub const RESIL_KNEE_FALLOFF: f32 = 0.85;

pub const RECOVERY_MIN: f32 = 0.0;
pub const RECOVERY_MAX: f32 = 120.0;
/// Chi regenerated per second at Recovery 0.
pub const BASE_CHI_REGEN: f32 = 2.0;

/// Gear weight class — the dominant Mobility contributor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GearWeight {
    Bare,
    Cloth,
    Leather,
    /// Bark / medium plate.
    Medium,
    /// Heavy stone plate.
    Heavy,
}

impl GearWeight {
    /// Mobility contribution of this weight class.
    pub fn mobility(self) -> f32 {
        match self {
            GearWeight::Bare => 50.0,
            GearWeight::Cloth => 35.0,
            GearWeight::Leather => 20.0,
            GearWeight::Medium => -15.0,
            GearWeight::Heavy => -35.0,
        }
    }
}

/// Active bending discipline — its Mobility contribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discipline {
    Air,
    Storm,
    Water,
    /// Earth / Root.
    Earth,
    /// Fire / Forge.
    Fire,
    Information,
}

impl Discipline {
    pub fn mobility(self) -> f32 {
        match self {
            Discipline::Air => 30.0,
            Discipline::Storm => 20.0,
            Discipline::Water => 15.0,
            Discipline::Earth => 0.0,
            Discipline::Fire => 0.0,
            Discipline::Information => 5.0,
        }
    }
}

/// Armour material — its Resilience contribution per equipped slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmorMaterial {
    Bark,
    Stone,
    Cloth,
    Leather,
}

impl ArmorMaterial {
    pub fn resilience_per_slot(self) -> f32 {
        match self {
            ArmorMaterial::Bark => 16.0,
            ArmorMaterial::Stone => 20.0,
            ArmorMaterial::Cloth => 4.0,
            ArmorMaterial::Leather => 8.0,
        }
    }
}

// ---------------------------------------------------------------------------
// MOBILITY — movement speed, aerial agility, dodge distance.
// ---------------------------------------------------------------------------

/// Compute Mobility: `50 base + gear + discipline + passives + 0.2·level`,
/// clamped to `[10, 120]`. `passive_bonus` is the summed contribution of active
/// mobility passives and companion movement bonds.
pub fn mobility(gear: GearWeight, discipline: Discipline, passive_bonus: f32, level: u32) -> f32 {
    let raw = MOBILITY_BASELINE + gear.mobility() + discipline.mobility() + passive_bonus + 0.2 * level as f32;
    raw.clamp(MOBILITY_MIN, MOBILITY_MAX)
}

/// Movement-speed multiplier from Mobility: `mobility / 50`. 50 → 1×, 100 → 2×,
/// 25 → 0.5×.
pub fn move_speed_multiplier(mobility: f32) -> f32 {
    mobility / MOBILITY_BASELINE
}

// ---------------------------------------------------------------------------
// RESILIENCE — damage mitigation, knockback/entangle resistance.
// ---------------------------------------------------------------------------

/// Compute Resilience: `material·slots + enchant·slots + passives + companion`,
/// clamped to `[0, 130]`. `passive_bonus` sums conditional passives (Grounded
/// Form, Bark Skin, …); `enchant_per_slot` the per-slot resilience seal; and
/// `companion_bonus` any earth-hardening aura.
pub fn resilience(
    material: ArmorMaterial,
    slots: u32,
    passive_bonus: f32,
    enchant_per_slot: f32,
    companion_bonus: f32,
) -> f32 {
    let per_slot = material.resilience_per_slot() + enchant_per_slot;
    let raw = per_slot * slots as f32 + passive_bonus + companion_bonus;
    raw.clamp(RESILIENCE_MIN, RESILIENCE_MAX)
}

/// Fraction of incoming damage mitigated by a Resilience value, in `[0, ~0.45]`.
/// ≈3.5% per 10 points, with softened returns above the knee (80). At 102 this
/// lands near the spec's ~35%.
pub fn damage_reduction(resilience: f32) -> f32 {
    let r = resilience.max(0.0);
    let dr = if r <= RESIL_KNEE {
        RESIL_RATE * r
    } else {
        RESIL_RATE * RESIL_KNEE + RESIL_RATE * RESIL_KNEE_FALLOFF * (r - RESIL_KNEE)
    };
    dr.clamp(0.0, 0.9)
}

// ---------------------------------------------------------------------------
// RECOVERY — chi regen, rest healing, cooldown reduction.
// ---------------------------------------------------------------------------

/// Compute Recovery: `base + 0.4·level + 10·maxrank_companions + passives +
/// location`, clamped to `[0, 120]`. Pass the base regen contribution (spec uses
/// 10) as `base`.
pub fn recovery(
    base: f32,
    level: u32,
    maxrank_companions: u32,
    passive_bonus: f32,
    location_bonus: f32,
) -> f32 {
    let raw = base + 0.4 * level as f32 + 10.0 * maxrank_companions as f32 + passive_bonus + location_bonus;
    raw.clamp(RECOVERY_MIN, RECOVERY_MAX)
}

/// Chi regenerated per second from Recovery: `2·(1 + recovery/100)`. Recovery 0 →
/// 2/s, 100 → 4/s (doubled), 68 → 3.36/s.
pub fn chi_regen_per_sec(recovery: f32) -> f32 {
    BASE_CHI_REGEN * (1.0 + recovery.max(0.0) / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn move_speed_multiplier_matches_spec_anchors() {
        assert!(approx(move_speed_multiplier(50.0), 1.0, 1e-6), "50 = baseline");
        assert!(approx(move_speed_multiplier(100.0), 2.0, 1e-6), "100 = 2×");
        assert!(approx(move_speed_multiplier(25.0), 0.5, 1e-6), "25 = 0.5×");
    }

    #[test]
    fn mobility_clamps_and_orders() {
        // bare Air bender is fast; heavy-plate Earth bender is slow
        let air = mobility(GearWeight::Bare, Discipline::Air, 20.0, 50);
        let tank = mobility(GearWeight::Heavy, Discipline::Earth, 0.0, 1);
        assert!(air > tank);
        assert!(air <= MOBILITY_MAX && tank >= MOBILITY_MIN);
        // a maxed-out build is capped at 120
        assert_eq!(mobility(GearWeight::Bare, Discipline::Air, 100.0, 50), MOBILITY_MAX);
    }

    #[test]
    fn resilience_matches_rootbender_example() {
        // spec: bark ×5 (80) + Grounded Form (8) + Bark Skin stationary (12)
        //       + Grove Pendant (2) = 102
        let r = resilience(ArmorMaterial::Bark, 5, /*passives*/ 8.0 + 12.0, /*enchant*/ 0.0, /*companion*/ 2.0);
        assert!(approx(r, 102.0, 1e-4), "got {r}");
        // ~35% damage reduction at 102
        let dr = damage_reduction(r);
        assert!(dr > 0.33 && dr < 0.37, "≈35% mitigation, got {dr}");
    }

    #[test]
    fn damage_reduction_is_monotone_with_softened_knee() {
        assert!(approx(damage_reduction(0.0), 0.0, 1e-6));
        // linear below the knee: 80 → 28%
        assert!(approx(damage_reduction(80.0), 0.28, 1e-4));
        // past the knee the slope is gentler than straight linear
        let past = damage_reduction(120.0);
        let linear = RESIL_RATE * 120.0;
        assert!(past < linear && past > damage_reduction(80.0), "softened but still rising");
    }

    #[test]
    fn recovery_matches_rootbender_example() {
        // spec: 20 (level 50) + 20 (two MAX companions) + 10 (Canopy Bond ×2)
        //       + 8 (Grand Grove) + 10 (base) = 68
        let rec = recovery(/*base*/ 10.0, /*level*/ 50, /*companions*/ 2, /*passives*/ 10.0, /*location*/ 8.0);
        assert!(approx(rec, 68.0, 1e-4), "got {rec}");
        // chi regen 3.36/s at Recovery 68
        assert!(approx(chi_regen_per_sec(rec), 3.36, 1e-3), "got {}", chi_regen_per_sec(rec));
        // Living Aura (+20) would push to 88 temporarily
        let aura = recovery(10.0, 50, 2, 10.0 + 20.0, 8.0);
        assert!(approx(aura, 88.0, 1e-4));
    }

    #[test]
    fn chi_regen_anchors() {
        assert!(approx(chi_regen_per_sec(0.0), 2.0, 1e-6), "base 2/s");
        assert!(approx(chi_regen_per_sec(100.0), 4.0, 1e-6), "100 → doubled");
    }

    #[test]
    fn contributor_tables_are_ordered() {
        // lighter gear = more mobile
        assert!(GearWeight::Bare.mobility() > GearWeight::Cloth.mobility());
        assert!(GearWeight::Cloth.mobility() > GearWeight::Medium.mobility());
        assert!(GearWeight::Medium.mobility() > GearWeight::Heavy.mobility());
        // Air is the most mobile discipline; Earth/Fire the least
        assert!(Discipline::Air.mobility() > Discipline::Storm.mobility());
        assert_eq!(Discipline::Earth.mobility(), 0.0);
        // stone is the toughest armour per slot, cloth the flimsiest
        assert!(ArmorMaterial::Stone.resilience_per_slot() > ArmorMaterial::Bark.resilience_per_slot());
        assert!(ArmorMaterial::Cloth.resilience_per_slot() < ArmorMaterial::Leather.resilience_per_slot());
    }
}
