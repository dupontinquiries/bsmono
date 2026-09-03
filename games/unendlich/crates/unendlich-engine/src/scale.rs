//! SCALE — how long a block is, and the eight answers currently on file.
//!
//! This module exists because the engine has no block-to-metre constant. The
//! conversion is 1.0 by assumption, unwritten, in some places — and something
//! else entirely in others. That is fine while nothing needs a real number,
//! and it is the blocker for two things now on the table: changing the block
//! size (every worldgen amplitude and structure template is authored in block
//! counts, so halving the block silently halves the world in metres), and
//! anything realism-directed, which needs physical units to mean something.
//!
//! ## What is actually in the tree
//!
//! A survey of gravity constants, because gravity is the one number that
//! makes a unit system visible — it is the only place where "how long is a
//! block" and "how long is a second" have to agree:
//!
//! | where | value | unit it is in |
//! |---|---|---|
//! | `crate::constants::GRAVITY` | 32.0 | blocks/s² — **no consumers found** |
//! | `crate::physics::constants::GRAVITY` | 28.0 | blocks/s², governs the player |
//! | `crate::sim::state::GRAVITY_PER_TICK` | −20 per tick² | fixed-point, deterministic sim |
//! | `crate::world::entity` (private) | −9.8 | m/s², applied to block velocities |
//! | `crate::sim::dive` (private) | 9.81 | m/s² |
//! | `crate::sim::leap::GRAVITY` | — | used by `locomotion_sim` |
//! | `crate::spark_burst` (private) | 4.0 | visual only, no claim to physics |
//! | `crate::world::water_waves::WW_GRAVITY` | 9.8 | m/s², wave dispersion |
//!
//! **The player falls at 28 and loose entities fall at 9.8, in the same
//! world.** That is not a tuning decision; it is two unit systems in one
//! simulation. Water-wave dispersion solves against real 9.8 m/s² while the
//! body watching the waves accelerates at 28 blocks/s².
//!
//! ## And the two documented scales disagree
//!
//! `physics::constants::GRAVITY`'s own comment derives 32 blocks/s² from
//! 9.81 m/s² "with 1 block ≈ 0.3 m". But `constants::PLAYER_HEIGHT` is 1.8
//! blocks, which at 0.3 m/block is a 54 cm player. The two constants imply
//! block lengths that differ by a factor of three, and both are shipped.
//!
//! ## What this module does, and does not, do
//!
//! It does NOT change any of them. Retuning gravity is a gameplay change that
//! wants a playtest, and doing it silently inside a scale cleanup is how a
//! game gets a feel regression nobody can bisect.
//!
//! What it does is (a) name the scale in one place, (b) provide the
//! conversions so new code has somewhere correct to reach for, and (c) pin
//! the current state in a CHARACTERISATION test — asserting the inconsistency
//! exists, so that whoever consolidates it gets a failure telling them the
//! survey above is now stale rather than silently drifting.
//!
//! Same contract as `blackstar-rs`'s `footing.rs` phantom-floor test: the
//! assertion approves nothing, it records.

/// How many metres one block edge is.
///
/// **1.0 is a decision, not a measurement.** It is the value that makes
/// `constants::PLAYER_HEIGHT = 1.8` mean a 1.8 m person, which is the reading
/// the rest of the content was authored against. The competing reading —
/// `physics::constants::GRAVITY`'s "1 block ≈ 0.3 m" — is Minecraft's
/// convention and is not consistent with the player's own height.
///
/// This is the dial a block-size change turns. Everything expressed in blocks
/// (terrain amplitudes, cave radii, tree heights, structure templates) is
/// implicitly multiplied by it, which is why changing it is a content
/// migration and not a constant edit.
pub const METRES_PER_BLOCK: f32 = 1.0;

/// Standard gravity, m/s². The physical constant, unmodified.
pub const EARTH_G_MS2: f32 = 9.806_65;

/// Blocks per metre.
pub const BLOCKS_PER_METRE: f32 = 1.0 / METRES_PER_BLOCK;

/// Convert a length in blocks to metres.
#[inline]
pub fn blocks_to_m(blocks: f32) -> f32 {
    blocks * METRES_PER_BLOCK
}

/// Convert a length in metres to blocks.
#[inline]
pub fn m_to_blocks(metres: f32) -> f32 {
    metres * BLOCKS_PER_METRE
}

/// What gravity WOULD be, in blocks/s², if the world ran at Earth's.
///
/// Provided so that anything realism-directed has a correct number to reach
/// for. Deliberately not wired into the player: the current feel is 28, and
/// changing it is a playtest, not a refactor.
#[inline]
pub fn earth_gravity_blocks() -> f32 {
    EARTH_G_MS2 * BLOCKS_PER_METRE
}

/// How many times Earth's gravity the player currently experiences. About 2.9
/// at the shipped values — the world is a heavy planet and nobody decided
/// that on purpose.
#[inline]
pub fn player_gravity_in_g() -> f32 {
    blocks_to_m(crate::physics::constants::GRAVITY) / EARTH_G_MS2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_conversions_round_trip() {
        for m in [0.0f32, 0.5, 1.8, 32.0, 1024.0] {
            let back = blocks_to_m(m_to_blocks(m));
            assert!((back - m).abs() < 1e-4, "{m} m round-tripped to {back}");
        }
        assert_eq!(blocks_to_m(1.0), METRES_PER_BLOCK);
    }

    #[test]
    fn the_player_is_a_person_sized_object_at_this_scale() {
        // The reading METRES_PER_BLOCK is chosen to preserve. If a block-size
        // change lands without rescaling PLAYER_HEIGHT, this fails and says so.
        let h = blocks_to_m(crate::constants::PLAYER_HEIGHT);
        assert!(
            (1.5..=2.1).contains(&h),
            "the player is {h:.2} m tall at {METRES_PER_BLOCK} m/block — either the scale \
             or PLAYER_HEIGHT moved without the other"
        );
    }

    #[test]
    fn gravity_is_currently_defined_in_at_least_three_unit_systems() {
        // CHARACTERISATION, not approval. Each assertion pins a value the
        // module doc surveys. A failure here means the survey is stale, which
        // is the only way anyone finds out that it was.

        // 1. blocks/s², governs the player
        assert_eq!(
            crate::physics::constants::GRAVITY,
            28.0,
            "the player's gravity moved; update scale.rs's survey"
        );

        // 2. blocks/s², defined at the crate root, and (as of this writing)
        //    consumed by nothing. Kept asserted so that if someone wires it
        //    up, the disagreement with 28.0 becomes a visible failure rather
        //    than a mysterious change in fall speed.
        assert_eq!(crate::constants::GRAVITY, 32.0);
        assert_ne!(
            crate::constants::GRAVITY,
            crate::physics::constants::GRAVITY,
            "two crate constants named GRAVITY finally agree — consolidate them and \
             delete this assertion"
        );

        // 3. m/s², real, used for wave dispersion in the same world
        assert_eq!(crate::world::water_waves::WW_GRAVITY, 9.8);

        // And the consequence, stated as a number so it cannot be waved away:
        let g = player_gravity_in_g();
        assert!(
            (2.5..3.5).contains(&g),
            "the player experiences {g:.2} g; the survey in this module says ~2.9"
        );
    }

    #[test]
    fn the_two_documented_block_lengths_disagree_by_about_three() {
        // physics::constants::GRAVITY's comment derives 32 blocks/s² from
        // 9.81 m/s² at "1 block ~= 0.3 m". PLAYER_HEIGHT = 1.8 blocks only
        // reads as a person at ~1 m/block. Both ship. This test is the record
        // of that, so a future scale decision starts from the contradiction
        // rather than rediscovering it.
        let implied_by_gravity_comment = 0.3f32;
        let implied_by_player_height = 1.8 / crate::constants::PLAYER_HEIGHT; // metres per block for a 1.8 m person
        let ratio = implied_by_player_height / implied_by_gravity_comment;
        assert!(
            (3.0..3.7).contains(&ratio),
            "the two implied block lengths now differ by {ratio:.2}x, not ~3.3x — \
             if the scale was reconciled, delete this test"
        );
    }
}
