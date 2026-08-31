//! The **Sabretooth** — a fast, low-health pack hunter. See
//! `docs/design/legendary_weapons_and_bestiary_addendum.md`: the intended
//! answer to "what hunts the player instead of waiting to be hunted."
//!
//! Design contrast with `sim::mob::MobKind::Seeker` (the existing "chases the
//! nearest player" mob): a Seeker just closes distance and stays there. A
//! Sabretooth's whole kit is a single committed burst — [`Mode::Pounce`] is a
//! knockdown lunge, not a sustained melee trade — so the punish/reward shape
//! is "create distance immediately after being hit", not "out-trade it".
//! [`Mode::Recovering`] is the deliberate opening that punishes staying close
//! after a dodged or landed pounce instead of resetting instantly to a fresh
//! threat.
//!
//! ## Fire variant
//! [`Variant::Fire`] swaps the instant pounce for a telegraphed charge-then-
//! rush: [`Mode::Charging`] holds position and visibly winds up (a real
//! window to punish or flee, not a snap attack), then [`Mode::FireRush`]
//! commits to a straight-line dash that deals fire damage through anything
//! in its path rather than a single-target hit. This is a genuinely
//! different pressure shape from the base pounce (avoidable-by-reading vs.
//! avoidable-by-distance), not a reskin.
//!
//! Pure deterministic decision core, same discipline as `lauscher.rs` and
//! `wendigo.rs`: no RNG, no clock, no I/O, rolls back with the sim.

use crate::sim::fixed::Vec3;

/// Which kit this Sabretooth instance runs. `Base` is the melee pounce
/// archetype from the design doc; `Fire` is the charge-and-rush variant
/// requested as a follow-up — same creature silhouette, different ability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    Base,
    Fire,
}

/// Sabretooth behavior state.
///
/// Base variant cycle: `Prowl -> Approach -> Pounce -> Recovering -> Prowl`.
/// Fire variant cycle: `Prowl -> Approach -> Charging -> FireRush -> Recovering -> Prowl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Wandering, no target locked. Same idle shape a Seeker mob would use.
    #[default]
    Prowl,
    /// Target locked, closing distance at full speed.
    Approach,
    /// BASE ONLY. A single committed lunge — one hit, knockdown, no
    /// sustained combo. Very short by design (see [`POUNCE_TICKS`]).
    Pounce,
    /// FIRE ONLY. Holding position, winding up — a real telegraph window.
    /// See [`CHARGE_TICKS`] for how long a player has to react.
    Charging,
    /// FIRE ONLY. Committed straight-line dash along `rush_heading`, dealing
    /// fire damage to anything in its path rather than a single target.
    FireRush,
    /// Landed or missed its committed action — Base's post-pounce recovery
    /// or Fire's post-rush recovery. Deliberately vulnerable: this is the
    /// "create distance and you're rewarded" window from the design doc.
    Recovering,
}

/// Ticks a base-variant pounce lasts once committed — short and explosive,
/// not a sustained attack.
pub const POUNCE_TICKS: u16 = 6;
/// Ticks a Fire-variant charge-up telegraphs before the rush fires — long
/// enough to be a real, readable window (about 1.25s at a 20-tick sim rate).
pub const CHARGE_TICKS: u16 = 25;
/// Ticks a Fire-variant rush travels once committed.
pub const RUSH_TICKS: u16 = 10;
/// Recovery window after either variant's committed action resolves — the
/// vulnerable opening the design doc's punish/reward loop depends on.
pub const RECOVERY_TICKS: u16 = 18;
/// World-unit range at which Base commits to a pounce.
pub const POUNCE_RANGE: i32 = 3;
/// World-unit range at which Fire commits to a charge (longer — it closes
/// the rest of the distance during the rush itself, not before committing).
pub const CHARGE_RANGE: i32 = 8;
/// World-unit range at which either variant locks onto a target and starts
/// closing (`Approach`).
pub const AGGRO_RANGE: i32 = 14;

/// One tick's decision inputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inputs {
    /// Straight-line distance to the nearest valid target, if any (world
    /// units). `None` = nothing in aggro range.
    pub target_distance: Option<i32>,
    /// Whether this creature took damage in the last tick — feeds
    /// [`Mode::Recovering`]'s "you're vulnerable if it landed OR missed and
    /// you punished it anyway" symmetry; not currently branched on
    /// differently by outcome, kept as an explicit input so a caller (or a
    /// later revision) can without changing this function's shape.
    pub took_damage_last_tick: bool,
}

/// Advance one Sabretooth's [`Mode`] by one tick. Pure: `current` plus
/// `ticks_in_mode` (how long it's been in `current`, reset to 0 by the caller
/// on every mode change this function returns) plus this tick's `inputs`
/// determine the next mode, every time, identically.
pub fn advance(variant: Variant, current: Mode, ticks_in_mode: u16, inputs: Inputs) -> Mode {
    match (variant, current) {
        (_, Mode::Prowl) => match inputs.target_distance {
            Some(d) if d <= AGGRO_RANGE => Mode::Approach,
            _ => Mode::Prowl,
        },
        (Variant::Base, Mode::Approach) => match inputs.target_distance {
            Some(d) if d <= POUNCE_RANGE => Mode::Pounce,
            Some(d) if d <= AGGRO_RANGE => Mode::Approach,
            _ => Mode::Prowl, // target got away
        },
        (Variant::Fire, Mode::Approach) => match inputs.target_distance {
            Some(d) if d <= CHARGE_RANGE => Mode::Charging,
            Some(d) if d <= AGGRO_RANGE => Mode::Approach,
            _ => Mode::Prowl,
        },
        (Variant::Base, Mode::Pounce) => {
            if ticks_in_mode + 1 >= POUNCE_TICKS { Mode::Recovering } else { Mode::Pounce }
        }
        (Variant::Fire, Mode::Charging) => {
            if ticks_in_mode + 1 >= CHARGE_TICKS { Mode::FireRush } else { Mode::Charging }
        }
        (Variant::Fire, Mode::FireRush) => {
            if ticks_in_mode + 1 >= RUSH_TICKS { Mode::Recovering } else { Mode::FireRush }
        }
        (_, Mode::Recovering) => {
            if ticks_in_mode + 1 >= RECOVERY_TICKS { Mode::Prowl } else { Mode::Recovering }
        }
        // Modes that don't belong to this variant (e.g. Base reaching
        // Charging somehow) — fall back to Prowl rather than panicking or
        // getting stuck; should never actually be reachable given the
        // transitions above, but a decision core should have no undefined
        // states.
        _ => Mode::Prowl,
    }
}

/// A committed Fire-variant rush's straight-line path, so the caller can
/// actually run the fire-damage-along-path sweep. Pure geometry: this
/// function does not know about hitboxes, terrain, or damage falloff — it
/// hands back the line, the caller resolves what it hits.
pub fn rush_path(origin: Vec3, heading_xz: (i32, i32), tiles: i32) -> Vec<[i32; 3]> {
    let (dx, dz) = heading_xz;
    // normalise to a single-cell-per-tick step in one of the 8 compass
    // directions (matches the mob layer's existing 0..8 heading convention).
    let sx = dx.signum();
    let sz = dz.signum();
    let ox = origin.x.to_f32().round() as i32;
    let oy = origin.y.to_f32().round() as i32;
    let oz = origin.z.to_f32().round() as i32;
    (1..=tiles.max(0)).map(|i| [ox + sx * i, oy, oz + sz * i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(dist: Option<i32>) -> Inputs {
        Inputs { target_distance: dist, took_damage_last_tick: false }
    }

    // ----- shared prowl/approach -----

    #[test]
    fn prowl_ignores_targets_outside_aggro_range() {
        let m = advance(Variant::Base, Mode::Prowl, 0, inputs(Some(AGGRO_RANGE + 1)));
        assert_eq!(m, Mode::Prowl);
    }

    #[test]
    fn prowl_locks_onto_a_target_within_aggro_range() {
        let m = advance(Variant::Base, Mode::Prowl, 0, inputs(Some(AGGRO_RANGE)));
        assert_eq!(m, Mode::Approach);
    }

    #[test]
    fn approach_loses_a_target_that_flees_past_aggro_range() {
        let m = advance(Variant::Base, Mode::Approach, 3, inputs(Some(AGGRO_RANGE + 1)));
        assert_eq!(m, Mode::Prowl);
    }

    // ----- base pounce -----

    #[test]
    fn base_commits_to_pounce_within_pounce_range() {
        let m = advance(Variant::Base, Mode::Approach, 5, inputs(Some(POUNCE_RANGE)));
        assert_eq!(m, Mode::Pounce);
    }

    #[test]
    fn base_pounce_is_short_and_commits_to_recovery() {
        let mut mode = Mode::Pounce;
        let mut t = 0u16;
        for _ in 0..POUNCE_TICKS {
            mode = advance(Variant::Base, mode, t, inputs(Some(1)));
            t += 1;
            if mode != Mode::Pounce { break; }
        }
        assert_eq!(mode, Mode::Recovering, "pounce should resolve into recovery, not linger");
    }

    #[test]
    fn recovering_returns_to_prowl_after_its_window() {
        let mut mode = Mode::Recovering;
        for t in 0..RECOVERY_TICKS {
            mode = advance(Variant::Base, mode, t, inputs(None));
        }
        assert_eq!(mode, Mode::Prowl);
    }

    // ----- fire variant -----

    #[test]
    fn fire_charges_instead_of_pouncing_within_charge_range() {
        let m = advance(Variant::Fire, Mode::Approach, 2, inputs(Some(CHARGE_RANGE)));
        assert_eq!(m, Mode::Charging);
    }

    #[test]
    fn fire_charge_is_a_real_telegraph_longer_than_base_pounce() {
        assert!(CHARGE_TICKS > POUNCE_TICKS * 2,
            "charge ({CHARGE_TICKS}) should be a meaningfully longer, readable window than pounce ({POUNCE_TICKS})");
    }

    #[test]
    fn fire_charge_holds_for_its_full_duration_before_rushing() {
        let mut mode = Mode::Charging;
        for t in 0..CHARGE_TICKS - 1 {
            mode = advance(Variant::Fire, mode, t, inputs(Some(1)));
            assert_eq!(mode, Mode::Charging, "should not rush early at tick {t}");
        }
        mode = advance(Variant::Fire, mode, CHARGE_TICKS - 1, inputs(Some(1)));
        assert_eq!(mode, Mode::FireRush);
    }

    #[test]
    fn fire_rush_resolves_to_recovery_after_its_duration() {
        let mut mode = Mode::FireRush;
        for t in 0..RUSH_TICKS {
            mode = advance(Variant::Fire, mode, t, inputs(Some(1)));
        }
        assert_eq!(mode, Mode::Recovering);
    }

    #[test]
    fn base_variant_never_enters_fire_only_modes() {
        // Structural guarantee: even if Approach were somehow ticked with
        // Fire-range distances, Base's own match arm governs it and can only
        // ever produce Approach/Pounce/Prowl.
        for d in 0..=AGGRO_RANGE {
            let m = advance(Variant::Base, Mode::Approach, 0, inputs(Some(d)));
            assert!(matches!(m, Mode::Approach | Mode::Pounce | Mode::Prowl));
        }
    }

    // ----- rush path -----

    #[test]
    fn rush_path_produces_the_requested_tile_count() {
        let path = rush_path(Vec3::ZERO, (1, 0), 5);
        assert_eq!(path.len(), 5);
    }

    #[test]
    fn rush_path_steps_one_cell_per_tile_in_the_heading_direction() {
        let path = rush_path(Vec3::ZERO, (1, 0), 3);
        assert_eq!(path, vec![[1, 0, 0], [2, 0, 0], [3, 0, 0]]);
    }

    #[test]
    fn rush_path_handles_diagonal_headings() {
        let path = rush_path(Vec3::ZERO, (-1, 1), 2);
        assert_eq!(path, vec![[-1, 0, 1], [-2, 0, 2]]);
    }

    #[test]
    fn rush_path_with_zero_tiles_is_empty() {
        assert!(rush_path(Vec3::ZERO, (1, 0), 0).is_empty());
    }
}
