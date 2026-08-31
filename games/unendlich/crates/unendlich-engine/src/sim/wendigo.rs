//! The **Wendigo** — the creature `sim::lauscher.rs`'s own doc comment already
//! named and left unbuilt: *"Where a Wendigo is hunger and a Rake is the shape
//! at the foot of the bed, the Lauscher is the sense of being heard."* The
//! Lauscher hunts by sound; the Wendigo hunts by cold. See
//! `docs/design/legendary_weapons_and_bestiary_addendum.md` for the full
//! design rationale — this module is that design, implemented.
//!
//! ## Structure, deliberately mirrored from `lauscher.rs`
//! Same shape (pure deterministic decision core, an environmental gate, a
//! `Mode` state machine, integer/fixed-point, rolls back with the sim),
//! different hook: Lauscher imprints a fixed lair and chases loudness; the
//! Wendigo has no lair at all — it doesn't exist as an active creature until
//! `gate()` says the world is cold and dark enough, and it hunts caloric/
//! warmth deficit instead of sound. See [`Mode`] for the state-by-state
//! contrast with Lauscher's four modes.
//!
//! ## Untameable, structurally
//! This module has no bond/trust state and no path into `sim::bond_tame`.
//! That's not a hostile flag on top of tameable content — it simply never
//! computes a bond delta, for anything, ever. See the design doc's reasoning:
//! Wendigo and Rake exist specifically to be things a bond system says no to,
//! so it means something when it says yes to a dragon instead.
//!
//! ## Three variants, one shared hunt core
//! `Variant` picks the *trap* a Stalking Wendigo sets once it's closed on a
//! target — the gate/hunt/mode logic above is identical across all three.
//! - [`Variant::Vine`] — Wood-element. Traps the target behind real
//!   `BlockId::Vine` placements (see [`vine_trap_positions`]) and applies the
//!   existing `Element::Wood` root/slow affliction (`sim::affliction`) — reuses
//!   two systems that already exist rather than inventing a snare stat.
//! - [`Variant::Quicksand`] — Earth-element. No quicksand block exists yet;
//!   modelled as a `sim::affliction`-driven sink+slow rather than new voxel
//!   physics (see [`quicksand_affliction`] and its doc for exactly what's
//!   simulated vs. what a real quicksand block would need).
//! - [`Variant::Stonefall`] — Stone-element, shallow-cave dweller, crawls
//!   (see [`Locomotion::Crawl`]). Drops ceiling rock for burst damage +
//!   stagger rather than a sustained bind — see [`stonefall_targets`].
//!
//! ## Honest scope note
//! The Vine variant's whole point — "hope you brought an axe, not a
//! longsword" — needs a real tool-dependent mining-speed system, which this
//! engine does not have today (block breaking is a fixed per-click cooldown,
//! same speed regardless of tool — see `unendlich_fullgame.rs`'s
//! `BLOCK_ACTION_COOLDOWN_MS`). [`vine_mine_seconds`] implements that
//! calculation for the vine trap specifically; wiring equivalent tool-speed
//! variance into ALL mining is a separate, larger balance decision this
//! module does not make. Likewise, animated vine/root visuals are a render-
//! side task with no logic dependency on this module — this file only
//! decides WHERE vines go and how long they take to clear, not how they wave.

use crate::sim::elemental::Element;
use crate::sim::fixed::Vec3;
use crate::sim::legendary_weapon::WeaponClass;
use crate::world::blocks::BlockId;

// ─────────────────────────── shared gate/hunt core ───────────────────────────

/// Surface temperature (°C, from `temperature.rs`'s scalar field) at or below
/// which the world is cold enough for a Wendigo to exist at all. Above this,
/// there is structurally nothing to gate awake — it isn't hiding, it isn't
/// "there" yet.
pub const GATE_MAX_TEMP_C: i32 = -2;
/// Surface light (0..=255) at or below which it counts as night for the gate.
/// Looser than Lauscher's darkness threshold on purpose — this creature's
/// premise is "the world is starving", not "the world is lightless".
pub const GATE_MAX_LIGHT: u8 = 60;
/// Warmth/fullness (0..=255, same normalisation as a player's vitality/heat
/// state) below which a target reads as prey — hungriest and coldest is
/// "loudest" to this creature, inverted from Lauscher's real acoustic model.
pub const HUNGER_THRESHOLD: u8 = 90;
/// Ticks a fixed target-lock persists after the target's warmth recovers
/// past threshold before the hunt gives up — it doesn't let go instantly,
/// but it isn't infinite either.
pub const RELEASE_GRACE_TICKS: u16 = 40;

/// What the Wendigo is doing right now. Same four-state shape as
/// [`crate::sim::lauscher::Mode`], different meanings:
/// - `Dormant` (Lauscher's `InLair`, but there is no lair — this means "does
///   not currently exist as an active entity", not "is somewhere hiding").
/// - `Prowling` — the gate has opened (cold + dark) and it is real now and
///   moving, but hasn't fixed a target yet.
/// - `Stalking` — has a caloric-deficit target fixed; closes distance
///   slowly and sets its variant's trap once in range, rather than sprinting
///   (Lauscher's `Hunting` closes on a *location*; this closes on a *body*,
///   patiently).
/// - `StarvedOff` — dawn, warmth recovered past threshold, or the gate closed
///   mid-hunt: it doesn't give up violently, the conditions sustaining it
///   just stop, mid-stride.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Dormant,
    Prowling,
    Stalking,
    StarvedOff,
}

/// Which elemental family this Wendigo instance is, and therefore which trap
/// it sets once `Mode::Stalking` closes on a target. All three share the
/// exact same gate/hunt core above — only the trap differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    /// Wood — ensnaring vines/roots. The reference archetype from the design
    /// doc; the other two are siblings, not the "real" one.
    Vine,
    /// Earth — sinks and slows rather than binding with a physical obstacle.
    Quicksand,
    /// Stone — shallow-cave dweller, crawls, drops ceiling rock.
    Stonefall,
}

impl Variant {
    pub fn element(self) -> Element {
        match self {
            Variant::Vine => Element::Wood,
            // sim::elemental::Element has no dedicated Earth/Stone entries
            // distinct from Dust — both map to Dust, the closest existing
            // "solid ground" element, same reasoning as the bestiary doc's
            // element_idx->Element bridge for legendary weapons.
            Variant::Quicksand | Variant::Stonefall => Element::Dust,
        }
    }

    /// How this variant gets around. Only `Stonefall` crawls — the user's
    /// specific ask for the cave variant; the other two use whatever generic
    /// locomotion the caller's creature/anatomy system already drives.
    pub fn locomotion(self) -> Locomotion {
        match self {
            Variant::Vine | Variant::Quicksand => Locomotion::Walk,
            Variant::Stonefall => Locomotion::Crawl,
        }
    }
}

/// Ground-contact style. `Crawl` is a pure tag for now — this module doesn't
/// own gait animation (that's `pose.rs`/`hero_forge.rs`'s job); it exists so
/// a caller wiring this into the anatomy/pose system has a real signal to
/// switch on, instead of inventing one at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locomotion {
    Walk,
    Crawl,
}

/// Environmental gate inputs for one tick, at the Wendigo's candidate spawn/
/// activity site.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateReading {
    pub surface_temp_c: i32,
    pub surface_light: u8,
    pub is_night: bool,
}

/// A potential target's current state — whatever a player's or fauna's real
/// hunger/warmth values resolve to, normalised 0..=255 (0 = starving/frozen,
/// 255 = full/warm). This module doesn't own that scale; it just consumes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetState {
    pub warmth: u8,
    pub distance: Fixed3,
}

/// Cheap 3D distance-ish type kept local so this module doesn't pull in a
/// heavier vector crate for one field; mirrors `Vec3`'s fixed-point shape.
pub type Fixed3 = Vec3;

/// Decide the next [`Mode`] for one Wendigo instance this tick. Pure: same
/// inputs, same output, every call. `current` is the mode coming into this
/// tick; `gate` is this tick's environment reading; `target` is the best
/// (lowest-warmth) candidate in range, if any; `release_timer` counts down
/// once a locked target's warmth recovers, and should be reset to
/// [`RELEASE_GRACE_TICKS`] by the caller whenever a target is (re)locked.
pub fn advance(
    current: Mode,
    gate: GateReading,
    target: Option<TargetState>,
    release_timer: u16,
) -> Mode {
    let gate_open = gate.surface_temp_c <= GATE_MAX_TEMP_C
        && gate.surface_light <= GATE_MAX_LIGHT
        && gate.is_night;

    if !gate_open {
        // The world stopped being cold/dark enough. Doesn't matter what it
        // was doing — the premise it runs on is gone.
        return match current {
            Mode::Dormant => Mode::Dormant,
            _ => Mode::StarvedOff,
        };
    }

    match current {
        Mode::Dormant | Mode::StarvedOff => Mode::Prowling,
        Mode::Prowling => match target {
            Some(t) if t.warmth < HUNGER_THRESHOLD => Mode::Stalking,
            _ => Mode::Prowling,
        },
        Mode::Stalking => match target {
            Some(t) if t.warmth < HUNGER_THRESHOLD => Mode::Stalking,
            _ if release_timer > 0 => Mode::Stalking, // grace period, still closing on last fix
            _ => Mode::Prowling, // target recovered and grace expired — back to prowling, not starved
        },
    }
}

// ─────────────────────────────── Vine variant ────────────────────────────────

/// Where to place ensnaring `BlockId::Vine` around a target position, so the
/// caller can actually set the blocks. Returns up to 4 cardinal-adjacent
/// offsets (never the target's own cell) plus one directly overhead, mimicking
/// roots erupting around and above rather than through the target. Only
/// offsets whose corresponding world cell the caller confirms is air/
/// vine-eligible should actually be placed — this function is pure geometry,
/// it has no world access and never claims a cell is actually free.
pub fn vine_trap_positions(target_block: [i32; 3]) -> [[i32; 3]; 5] {
    let [x, y, z] = target_block;
    [
        [x + 1, y, z],
        [x - 1, y, z],
        [x, y, z + 1],
        [x, y, z - 1],
        [x, y + 1, z],
    ]
}

/// Real per-tool mining-time multiplier for a `BlockId::Vine` trap cell.
/// `hardness` already exists per-block (`BlockId::hardness`); this is the
/// missing other half — a tool-affinity multiplier — implemented HERE for
/// the vine trap specifically rather than as a blanket change to all mining
/// (see this module's doc comment for why that's a separate decision).
///
/// Axe/blade classes are fast against vine (cutting tools against plant
/// matter); Greatsword/Greataxe are slow despite being "bigger" weapons —
/// a longsword's mass and reach are wasted on hacking through a tangle at
/// point-blank range, which is the exact in-world joke the design doc set up.
/// `None` (bare hands or a non-weapon item) is baseline speed.
pub fn vine_mine_seconds(tool: Option<WeaponClass>) -> f32 {
    const BASE_SECONDS: f32 = 1.8; // bare-hands baseline to clear one vine cell
    let mult = match tool {
        Some(WeaponClass::Axe) => 0.35,        // built for exactly this
        Some(WeaponClass::Dagger) => 0.55,     // fast, short strokes work on tangles
        Some(WeaponClass::Blade) => 0.60,      // a real edge, still efficient
        Some(WeaponClass::Greataxe) => 0.50,   // heavy but still an axe
        Some(WeaponClass::Spear) => 0.85,      // point weapon, awkward on a mass of vine
        Some(WeaponClass::Greatsword) => 1.35, // exactly the "brought a longsword" case
        Some(WeaponClass::Staff) | Some(WeaponClass::Wand) => 1.6, // not a cutting tool at all
        None => 1.0,
    };
    BASE_SECONDS * mult
}

// ──────────────────────────── Quicksand variant ─────────────────────────────

/// The quicksand "trap" as an affliction application, since no quicksand
/// block exists yet. This is honest about being a stand-in: a real quicksand
/// block would need its own physics (sink rate, depth-based visibility,
/// suffocation risk at full submersion) that this function does not attempt.
/// What IS simulated: a strong slow (heavier than Wood's existing 1/2 root
/// factor) for a fixed duration, applied through the existing
/// `sim::affliction::Afflicted` pipeline — reuses real infrastructure rather
/// than inventing a second status-effect system alongside it.
///
/// Returns `(element, intensity, ticks)` shaped for direct use with
/// `Afflicted::apply` — see that function's signature in `sim::affliction`.
pub fn quicksand_affliction() -> (Element, crate::sim::fixed::Fixed, i32) {
    use crate::sim::fixed::Fixed;
    // Dust intensity high enough that, once affliction.rs's per-element slow
    // table is extended with a Dust entry (currently only Wood/Water slow —
    // seeis this module's doc note), a caller can read this as "much slower
    // than rooted". Ticks: ~6 seconds at a 20-tick-per-second sim rate.
    (Element::Dust, Fixed::from_frac(3, 4), 120)
}

// ───────────────────────────── Stonefall variant ─────────────────────────────

/// One ceiling-rock drop event: where it lands and how hard it hits. Pure
/// geometry + damage numbers; the caller is responsible for confirming there
/// really is a low ceiling above `target_block` (this function doesn't have
/// world access) before actually spawning the effect — see the doc note
/// below on why that check has to live in the caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RockDrop {
    pub land_at: [i32; 3],
    pub damage: i32,
    /// Movement stagger duration in ticks — burst control, not a bind. This
    /// is deliberately NOT a sustained trap like Vine/Quicksand: the design
    /// doc's ask was "drop rocks from the ceiling", a punishment for standing
    /// still under one, not an escape puzzle.
    pub stagger_ticks: u16,
}

/// Compute the rock-drop event(s) for a Stonefall Wendigo targeting a
/// position. `ceiling_height` is the number of blocks of clear air above the
/// target before solid ground is hit (the caller measures this — shallow-
/// cave detection needs a real upward raycast against `Chunk`/`ChunkManager`,
/// which this pure-logic module has no access to and shouldn't reach for).
/// Returns `None` if the ceiling is too high for this to read as a "shallow
/// cave" drop (the whole point is a LOW ceiling, not open sky).
pub const SHALLOW_CAVE_MAX_CLEARANCE: i32 = 4;

pub fn stonefall_targets(target_block: [i32; 3], ceiling_clearance: i32) -> Option<RockDrop> {
    if ceiling_clearance <= 0 || ceiling_clearance > SHALLOW_CAVE_MAX_CLEARANCE {
        return None;
    }
    // Closer ceilings hit harder (less warning, less time to react) and
    // stagger longer — a drop from directly overhead is worse than one that
    // grazed in from a taller pocket.
    let closeness01 = 1.0 - (ceiling_clearance as f32 - 1.0) / (SHALLOW_CAVE_MAX_CLEARANCE as f32 - 1.0).max(1.0);
    let damage = (6.0 + closeness01 * 6.0).round() as i32; // 6..=12
    let stagger_ticks = (10.0 + closeness01 * 10.0).round() as u16; // 10..=20
    Some(RockDrop { land_at: target_block, damage, stagger_ticks })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(temp_c: i32, light: u8, night: bool) -> GateReading {
        GateReading { surface_temp_c: temp_c, surface_light: light, is_night: night }
    }

    fn warm_target(warmth: u8) -> TargetState {
        TargetState { warmth, distance: Vec3::ZERO }
    }

    // ----- gate/mode core -----

    #[test]
    fn dormant_stays_dormant_when_gate_is_closed() {
        assert_eq!(advance(Mode::Dormant, gate(10, 200, false), None, 0), Mode::Dormant);
    }

    #[test]
    fn dormant_wakes_to_prowling_when_cold_dark_and_night() {
        let g = gate(GATE_MAX_TEMP_C, GATE_MAX_LIGHT, true);
        assert_eq!(advance(Mode::Dormant, g, None, 0), Mode::Prowling);
    }

    #[test]
    fn warm_weather_does_not_wake_it_even_at_night_in_the_dark() {
        // Cold gate specifically required — night+dark alone isn't enough,
        // unlike Lauscher's darkness-only lair check.
        let g = gate(GATE_MAX_TEMP_C + 10, 10, true);
        assert_eq!(advance(Mode::Dormant, g, None, 0), Mode::Dormant);
    }

    #[test]
    fn daylight_does_not_wake_it_even_if_cold() {
        let g = gate(GATE_MAX_TEMP_C, GATE_MAX_LIGHT, false);
        assert_eq!(advance(Mode::Dormant, g, None, 0), Mode::Dormant);
    }

    #[test]
    fn prowling_locks_onto_a_hungry_cold_target() {
        let g = gate(GATE_MAX_TEMP_C, GATE_MAX_LIGHT, true);
        let t = warm_target(HUNGER_THRESHOLD - 1);
        assert_eq!(advance(Mode::Prowling, g, Some(t), 0), Mode::Stalking);
    }

    #[test]
    fn prowling_ignores_a_well_fed_warm_target() {
        let g = gate(GATE_MAX_TEMP_C, GATE_MAX_LIGHT, true);
        let t = warm_target(HUNGER_THRESHOLD + 50);
        assert_eq!(advance(Mode::Prowling, g, Some(t), 0), Mode::Prowling);
    }

    #[test]
    fn stalking_holds_through_grace_period_after_target_recovers() {
        let g = gate(GATE_MAX_TEMP_C, GATE_MAX_LIGHT, true);
        let recovered = warm_target(255);
        assert_eq!(advance(Mode::Stalking, g, Some(recovered), 5), Mode::Stalking);
    }

    #[test]
    fn stalking_releases_to_prowling_once_grace_expires() {
        let g = gate(GATE_MAX_TEMP_C, GATE_MAX_LIGHT, true);
        let recovered = warm_target(255);
        assert_eq!(advance(Mode::Stalking, g, Some(recovered), 0), Mode::Prowling);
    }

    #[test]
    fn any_active_mode_starves_off_when_gate_closes_mid_hunt() {
        let closed = gate(20, 200, false);
        assert_eq!(advance(Mode::Stalking, closed, Some(warm_target(0)), 30), Mode::StarvedOff);
        assert_eq!(advance(Mode::Prowling, closed, None, 0), Mode::StarvedOff);
    }

    #[test]
    fn starved_off_re_wakes_to_prowling_once_the_gate_reopens() {
        let g = gate(GATE_MAX_TEMP_C, GATE_MAX_LIGHT, true);
        assert_eq!(advance(Mode::StarvedOff, g, None, 0), Mode::Prowling);
    }

    // ----- variant metadata -----

    #[test]
    fn only_stonefall_crawls() {
        assert_eq!(Variant::Vine.locomotion(), Locomotion::Walk);
        assert_eq!(Variant::Quicksand.locomotion(), Locomotion::Walk);
        assert_eq!(Variant::Stonefall.locomotion(), Locomotion::Crawl);
    }

    #[test]
    fn vine_is_the_only_wood_element_variant() {
        assert_eq!(Variant::Vine.element(), Element::Wood);
        assert_eq!(Variant::Quicksand.element(), Element::Dust);
        assert_eq!(Variant::Stonefall.element(), Element::Dust);
    }

    // ----- vine trap -----

    #[test]
    fn vine_trap_positions_never_include_the_targets_own_cell() {
        let target = [10, 5, -3];
        for p in vine_trap_positions(target) {
            assert_ne!(p, target);
        }
    }

    #[test]
    fn vine_trap_positions_are_four_cardinal_plus_one_overhead() {
        let target = [0, 0, 0];
        let positions = vine_trap_positions(target);
        assert_eq!(positions.len(), 5);
        let overhead_count = positions.iter().filter(|p| p[1] > target[1]).count();
        assert_eq!(overhead_count, 1, "expected exactly one overhead placement");
    }

    #[test]
    fn axe_clears_vine_far_faster_than_a_greatsword() {
        let axe_time = vine_mine_seconds(Some(WeaponClass::Axe));
        let greatsword_time = vine_mine_seconds(Some(WeaponClass::Greatsword));
        assert!(axe_time < greatsword_time,
            "axe ({axe_time}s) should clear vine faster than greatsword ({greatsword_time}s)");
        // The specific case the design doc calls out: bringing the wrong
        // weapon should visibly cost you, not be a rounding error.
        assert!(greatsword_time / axe_time > 2.0,
            "greatsword should be meaningfully (>2x) slower than axe, not marginally");
    }

    #[test]
    fn bare_hands_is_worse_than_any_bladed_tool_but_better_than_a_staff() {
        let bare = vine_mine_seconds(None);
        let axe = vine_mine_seconds(Some(WeaponClass::Axe));
        let staff = vine_mine_seconds(Some(WeaponClass::Staff));
        assert!(axe < bare, "a real axe should beat bare hands");
        assert!(bare < staff, "bare hands should still beat trying to mine with a staff");
    }

    #[test]
    fn all_weapon_classes_produce_a_positive_finite_mine_time() {
        for class in [
            WeaponClass::Dagger, WeaponClass::Blade, WeaponClass::Greatsword,
            WeaponClass::Axe, WeaponClass::Greataxe, WeaponClass::Spear,
            WeaponClass::Staff, WeaponClass::Wand,
        ] {
            let t = vine_mine_seconds(Some(class));
            assert!(t > 0.0 && t.is_finite(), "{class:?} produced {t}");
        }
    }

    // ----- quicksand -----

    #[test]
    fn quicksand_affliction_is_dust_element_with_positive_duration() {
        let (element, intensity, ticks) = quicksand_affliction();
        assert_eq!(element, Element::Dust);
        assert!(intensity > crate::sim::fixed::Fixed::ZERO);
        assert!(ticks > 0);
    }

    // ----- stonefall -----

    #[test]
    fn stonefall_refuses_open_sky_and_zero_clearance() {
        assert_eq!(stonefall_targets([0, 0, 0], 0), None);
        assert_eq!(stonefall_targets([0, 0, 0], SHALLOW_CAVE_MAX_CLEARANCE + 1), None);
        assert_eq!(stonefall_targets([0, 0, 0], 100), None);
    }

    #[test]
    fn stonefall_accepts_shallow_clearance_and_lands_on_the_target() {
        let drop = stonefall_targets([3, 60, -8], 2).expect("shallow ceiling should trigger");
        assert_eq!(drop.land_at, [3, 60, -8]);
        assert!(drop.damage > 0);
        assert!(drop.stagger_ticks > 0);
    }

    #[test]
    fn a_tighter_ceiling_hits_harder_and_staggers_longer() {
        let tight = stonefall_targets([0, 0, 0], 1).unwrap();
        let loose = stonefall_targets([0, 0, 0], SHALLOW_CAVE_MAX_CLEARANCE).unwrap();
        assert!(tight.damage >= loose.damage,
            "tighter ceiling ({}) should hit at least as hard as looser ({})", tight.damage, loose.damage);
        assert!(tight.stagger_ticks >= loose.stagger_ticks);
    }

    #[test]
    fn stonefall_damage_and_stagger_stay_in_documented_ranges() {
        for clearance in 1..=SHALLOW_CAVE_MAX_CLEARANCE {
            let d = stonefall_targets([0, 0, 0], clearance).unwrap();
            assert!((6..=12).contains(&d.damage), "clearance {clearance} -> damage {}", d.damage);
            assert!((10..=20).contains(&d.stagger_ticks), "clearance {clearance} -> stagger {}", d.stagger_ticks);
        }
    }
}
