//! CIRCUIT WAVE — `escalation::WaveMakeup`, as agents standing
//! somewhere.
//!
//! `escalation.rs`'s own doc comment names exactly this as the missing
//! half: it "does not spawn anything, place anything in a world" — the
//! arena/spawn foundation it said this codebase didn't have anywhere
//! yet for PvE. That foundation now exists in two pieces that have
//! never been introduced to each other: `enemy::Enemy` (a real agent
//! with a position, HP, and a state machine) and `corrupt_body.rs` (a
//! stage's actual body). This module is the introduction: read a wave
//! number, get real, positioned, individually-bodied hostiles back.
//!
//! **Why not `field::Field::muster`.** `field.rs` already stands a wave
//! in the world, but its whole shape is built around `campaign::Fighter`
//! / `content::Troop` — a raid roster with named characters, elements,
//! and a fold-to-vehicle story. Escalation's wave is a count
//! (`grunts`/`corrupted`/`hijacks`/`titan`), not a roster, and forcing
//! it through `Troop` would mean inventing fictional troop entries for
//! numbers that were never meant to have one. `enemy::Enemy::new_kind`
//! is the layer BELOW `Field` that both this and `field.rs` already
//! share (`field::kind_for` maps a `Troop` down to the same
//! `EnemyKind`) — so this module targets that shared layer instead of
//! going through the roster layer above it.
//!
//! **Which kinds get a `corrupt_body::BodyRecipe`.** Only Corrupted and
//! Apex — `field::kind_for`'s own rule that Apex is a Corrupted
//! champion, not a separate faction. Grunts and Hijacks are ordinary
//! troopers wearing an existing chassis outright; the jitter/morph
//! story is specifically the Corrupted's, per `corruption.rs`'s own
//! doc.

use crate::corrupt_body::{self, BodyRecipe};
use crate::corruption::Stage;
use crate::enemy::Enemy;
use crate::escalation::{self, EnemyKind, WaveMakeup};
use crate::moons::MoonSpec;
use crate::raidgen::Rng;
use crate::weapons::Element;
use glam::Vec3;

/// (guess) — a muster ring clear of `circuit_map::FIXTURE_RING_R`
/// (14 m) so a spawning wave doesn't stand inside the shops.
pub const MUSTER_RING_R: f32 = 30.0;

/// (guess) — starting HP per kind, matching the relative toughness
/// order the C engine's own valence table implies (apex >> corrupted >
/// hijack > grunt) without claiming to port exact numbers that were
/// never in the reference for a 3D agent.
///
/// **Hijack is not actually a guess**: `enemy::Enemy::new_kind` hardcodes
/// 45.0 for `EnemyKind::Hijack` regardless of what's passed in (checked
/// by running `--circuit-wave` and seeing 45 come back instead of an
/// earlier, wrong guess of 90 here). Stated as the real number, not a
/// second guess that would be silently discarded.
fn base_hp(kind: EnemyKind) -> f32 {
    match kind {
        EnemyKind::Grunt => 60.0,
        EnemyKind::Hijack => 45.0,
        EnemyKind::Corrupted => 140.0,
        EnemyKind::Apex => 900.0,
    }
}

/// Wave number -> corruption stage, per the design brief's table
/// (waves 1-4 Stage One, 5-9 Stage Two, 10+ Stage Three).
pub fn stage_for_wave(wave: i32) -> Stage {
    if wave < 5 {
        Stage::One
    } else if wave < 10 {
        Stage::Two
    } else {
        Stage::Three
    }
}

/// One spawned hostile: the real agent, and (for Corrupted/Apex only)
/// what to draw it as.
pub struct SpawnedFoe {
    pub agent: Enemy,
    pub recipe: Option<BodyRecipe>,
}

/// A deterministic ring of `n` positions, stop-local, radius
/// `MUSTER_RING_R`, jittered a little off perfect spacing so a wave
/// doesn't look like a parade formation. `rng` is threaded through
/// rather than reseeded per call, so calling this for several kinds in
/// one wave doesn't stack every kind onto the same angles.
fn ring_positions(n: usize, rng: &mut Rng) -> Vec<Vec3> {
    (0..n)
        .map(|i| {
            let base = (i as f32 / n.max(1) as f32) * std::f32::consts::TAU;
            let jitter = rng.f(-0.3, 0.3);
            let r = MUSTER_RING_R + rng.f(-3.0, 3.0);
            let a = base + jitter;
            Vec3::new(a.cos() * r, 0.0, a.sin() * r)
        })
        .collect()
}

/// Scale a wave's counts by `mult` (`circuit::Circuit::pressure_mul`),
/// rounding rather than truncating so a small pressure bump (e.g. 1.25x
/// on one grunt) doesn't silently vanish. `titan` is never scaled — a
/// titan wave is a fixed structural event, not a count.
pub fn scale_makeup(m: WaveMakeup, mult: f32) -> WaveMakeup {
    let scale = |n: i32| ((n as f32) * mult).round().max(0.0) as i32;
    WaveMakeup { grunts: scale(m.grunts), corrupted: scale(m.corrupted), hijacks: scale(m.hijacks), titan: m.titan }
}

/// Same as `spawn_wave`, but with `escalation::makeup(wave)` scaled by
/// `pressure_mul` first — the ring's lap pressure (`circuit::Circuit::
/// pressure_mul`) reaching all the way into how many bodies actually
/// stand in the arena, not just a number on a HUD.
pub fn spawn_wave_scaled(spec: &MoonSpec, wave: i32, pressure_mul: f32, seed: u64, t: f32) -> Vec<SpawnedFoe> {
    spawn_from_makeup(scale_makeup(escalation::makeup(wave), pressure_mul), stage_for_wave(wave), seed, t)
}

/// The shared body behind `spawn_wave`/`spawn_wave_scaled` — factored
/// out so scaling can't drift from the unscaled path by reimplementing
/// the spawn loop a second time.
fn spawn_from_makeup(makeup: WaveMakeup, stage: Stage, seed: u64, t: f32) -> Vec<SpawnedFoe> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::new();
    let mut next_id = 0i32;

    let mut spawn_group = |count: i32, kind: EnemyKind, out: &mut Vec<SpawnedFoe>, next_id: &mut i32| {
        let positions = ring_positions(count.max(0) as usize, &mut rng);
        for pos in positions {
            let agent = Enemy::new_kind(*next_id, pos, base_hp(kind), kind, Element::None);
            let recipe = if matches!(kind, EnemyKind::Corrupted | EnemyKind::Apex) {
                let body_seed = seed ^ (*next_id as u64).wrapping_mul(0x9E37_79B9);
                Some(corrupt_body::recipe_for(stage, "corrupted_generic", body_seed, t))
            } else {
                None
            };
            out.push(SpawnedFoe { agent, recipe });
            *next_id += 1;
        }
    };

    spawn_group(makeup.grunts, EnemyKind::Grunt, &mut out, &mut next_id);
    spawn_group(makeup.corrupted, EnemyKind::Corrupted, &mut out, &mut next_id);
    spawn_group(makeup.hijacks, EnemyKind::Hijack, &mut out, &mut next_id);
    if makeup.titan {
        spawn_group(1, EnemyKind::Apex, &mut out, &mut next_id);
    }
    out
}

/// Spawn one wave's worth of agents for `spec`, positioned stop-local
/// (add `circuit_layout::stop_local_to_world` on top for a world
/// position, same convention as `circuit_map`'s fixtures). `seed`
/// should vary per wave/stop so two different arenas don't muster
/// identically; `t` feeds `corrupt_body`'s Stage-3 morph clock.
pub fn spawn_wave(spec: &MoonSpec, wave: i32, seed: u64, t: f32) -> Vec<SpawnedFoe> {
    spawn_from_makeup(escalation::makeup(wave), stage_for_wave(wave), seed, t)
}

/// Valence value if every agent in this wave were killed — lets a
/// caller sanity-check a spawned wave against `escalation::makeup`'s
/// own economy without recomputing it by hand.
pub fn wave_total_valence(makeup: &WaveMakeup) -> i64 {
    escalation::valence_value(EnemyKind::Grunt) * makeup.grunts as i64
        + escalation::valence_value(EnemyKind::Corrupted) * makeup.corrupted as i64
        + escalation::valence_value(EnemyKind::Hijack) * makeup.hijacks as i64
        + if makeup.titan { escalation::valence_value(EnemyKind::Apex) } else { 0 }
}

/// `--circuit-wave <wave>`: prints a spawned wave's roster and total
/// valence, so the escalation curve can be read as actual agent counts
/// rather than just the makeup numbers.
pub fn run(argv: &[String]) {
    let wave: i32 = argv
        .iter()
        .position(|a| a == "--circuit-wave")
        .and_then(|i| argv.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let spec = crate::moons::by_id("tithe").unwrap();
    let foes = spawn_wave(&spec, wave, 42, 0.0);
    println!("CIRCUIT WAVE {wave} at {} — stage {:?}\n", spec.name, stage_for_wave(wave));
    for f in &foes {
        println!(
            "  #{:<3} {:?}  hp {:>4.0}  pos ({:>6.1}, {:>6.1})  recipe: {}",
            f.agent.id,
            f.agent.kind,
            f.agent.hp,
            f.agent.position.x,
            f.agent.position.z,
            match &f.recipe {
                Some(BodyRecipe::Chassis { .. }) => "chassis+jitter",
                Some(BodyRecipe::Morph { .. }) => "no fixed silhouette",
                None => "-",
            }
        );
    }
    let makeup = escalation::makeup(wave);
    println!("\n{} agents, {} valence if wiped", foes.len(), wave_total_valence(&makeup));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tithe() -> MoonSpec {
        crate::moons::by_id("tithe").unwrap()
    }

    #[test]
    fn spawned_counts_match_the_makeup_exactly() {
        for wave in 1..=15 {
            let spec = tithe();
            let makeup = escalation::makeup(wave);
            let foes = spawn_wave(&spec, wave, 1, 0.0);
            let grunts = foes.iter().filter(|f| f.agent.kind == EnemyKind::Grunt).count() as i32;
            let corrupted = foes.iter().filter(|f| f.agent.kind == EnemyKind::Corrupted).count() as i32;
            let hijacks = foes.iter().filter(|f| f.agent.kind == EnemyKind::Hijack).count() as i32;
            let apex = foes.iter().filter(|f| f.agent.kind == EnemyKind::Apex).count() as i32;
            assert_eq!(grunts, makeup.grunts, "wave {wave}");
            assert_eq!(corrupted, makeup.corrupted, "wave {wave}");
            assert_eq!(hijacks, makeup.hijacks, "wave {wave}");
            assert_eq!(apex, if makeup.titan { 1 } else { 0 }, "wave {wave}");
        }
    }

    #[test]
    fn only_corrupted_and_apex_carry_a_body_recipe() {
        let foes = spawn_wave(&tithe(), 10, 7, 0.0); // wave 10: titan, plenty of every kind
        for f in &foes {
            let has_recipe = f.recipe.is_some();
            let should = matches!(f.agent.kind, EnemyKind::Corrupted | EnemyKind::Apex);
            assert_eq!(has_recipe, should, "{:?}", f.agent.kind);
        }
    }

    #[test]
    fn stage_boundaries_match_the_brief() {
        assert_eq!(stage_for_wave(1), Stage::One);
        assert_eq!(stage_for_wave(4), Stage::One);
        assert_eq!(stage_for_wave(5), Stage::Two);
        assert_eq!(stage_for_wave(9), Stage::Two);
        assert_eq!(stage_for_wave(10), Stage::Three);
        assert_eq!(stage_for_wave(50), Stage::Three);
    }

    #[test]
    fn late_wave_corrupted_bodies_have_no_fixed_silhouette() {
        let foes = spawn_wave(&tithe(), 12, 3, 1.0);
        let corrupted: Vec<_> = foes
            .iter()
            .filter(|f| matches!(f.agent.kind, EnemyKind::Corrupted | EnemyKind::Apex))
            .collect();
        assert!(!corrupted.is_empty());
        for f in corrupted {
            assert!(matches!(f.recipe, Some(BodyRecipe::Morph { .. })));
        }
    }

    #[test]
    fn early_wave_corrupted_bodies_are_a_fixed_chassis() {
        let foes = spawn_wave(&tithe(), 2, 3, 1.0);
        let corrupted: Vec<_> =
            foes.iter().filter(|f| f.agent.kind == EnemyKind::Corrupted).collect();
        assert!(!corrupted.is_empty(), "wave 2 should already have corrupted per escalation::makeup");
        for f in corrupted {
            assert!(matches!(f.recipe, Some(BodyRecipe::Chassis { .. })));
        }
    }

    #[test]
    fn every_agent_id_in_a_wave_is_unique() {
        let foes = spawn_wave(&tithe(), 10, 9, 0.0);
        let mut ids: Vec<i32> = foes.iter().map(|f| f.agent.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before);
    }

    #[test]
    fn every_position_is_finite_and_no_two_coincide() {
        let foes = spawn_wave(&tithe(), 8, 55, 0.0);
        for f in &foes {
            assert!(f.agent.position.is_finite());
        }
        for i in 0..foes.len() {
            for j in (i + 1)..foes.len() {
                assert!(
                    foes[i].agent.position.distance(foes[j].agent.position) > 0.1,
                    "{i} and {j} nearly coincide"
                );
            }
        }
    }

    #[test]
    fn spawn_is_deterministic_for_the_same_seed() {
        let a = spawn_wave(&tithe(), 6, 123, 0.5);
        let b = spawn_wave(&tithe(), 6, 123, 0.5);
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.agent.position, y.agent.position);
            assert_eq!(x.agent.kind, y.agent.kind);
        }
    }

    #[test]
    fn different_seeds_do_not_produce_identical_layouts() {
        let a = spawn_wave(&tithe(), 6, 1, 0.0);
        let b = spawn_wave(&tithe(), 6, 2, 0.0);
        let same = a.iter().zip(b.iter()).all(|(x, y)| x.agent.position == y.agent.position);
        assert!(!same);
    }

    #[test]
    fn wave_total_valence_matches_a_hand_sum() {
        let makeup = escalation::makeup(5);
        let expect = escalation::valence_value(EnemyKind::Grunt) * makeup.grunts as i64
            + escalation::valence_value(EnemyKind::Corrupted) * makeup.corrupted as i64
            + escalation::valence_value(EnemyKind::Hijack) * makeup.hijacks as i64
            + if makeup.titan { escalation::valence_value(EnemyKind::Apex) } else { 0 };
        assert_eq!(wave_total_valence(&makeup), expect);
    }

    #[test]
    fn spawned_hp_matches_what_enemy_new_kind_actually_returns() {
        // regression against exactly the mismatch this module found once
        // already: `base_hp` is a request, `enemy::Enemy::new_kind` is
        // free to override it per kind, and the spawned agent's `hp`
        // must reflect the REAL value, not the one asked for.
        for wave in [1, 6, 12] {
            for f in spawn_wave(&tithe(), wave, 4, 0.0) {
                let requested = base_hp(f.agent.kind);
                let direct = Enemy::new_kind(0, Vec3::ZERO, requested, f.agent.kind, Element::None);
                assert_eq!(f.agent.hp, direct.hp, "{:?}", f.agent.kind);
                assert_eq!(f.agent.max_hp, direct.max_hp, "{:?}", f.agent.kind);
            }
        }
    }

    #[test]
    fn scale_makeup_rounds_rather_than_truncates() {
        let m = WaveMakeup { grunts: 3, corrupted: 1, hijacks: 2, titan: false };
        let scaled = scale_makeup(m, 1.25); // 3*1.25=3.75->4, 1*1.25=1.25->1, 2*1.25=2.5->3
        assert_eq!(scaled.grunts, 4);
        assert_eq!(scaled.corrupted, 1);
        assert_eq!(scaled.hijacks, 3);
    }

    #[test]
    fn scale_makeup_never_scales_the_titan_flag() {
        let m = WaveMakeup { grunts: 4, corrupted: 2, hijacks: 3, titan: true };
        assert!(scale_makeup(m, 0.1).titan);
        assert!(scale_makeup(m, 5.0).titan);
    }

    #[test]
    fn scale_makeup_never_goes_negative() {
        let m = WaveMakeup { grunts: 4, corrupted: 0, hijacks: 3, titan: false };
        let scaled = scale_makeup(m, 0.0);
        assert_eq!(scaled.grunts, 0);
        assert_eq!(scaled.hijacks, 0);
    }

    #[test]
    fn spawn_wave_scaled_at_1x_matches_unscaled_counts() {
        for wave in [1, 5, 10] {
            let a = spawn_wave(&tithe(), wave, 9, 0.0);
            let b = spawn_wave_scaled(&tithe(), wave, 1.0, 9, 0.0);
            assert_eq!(a.len(), b.len(), "wave {wave}");
        }
    }

    #[test]
    fn spawn_wave_scaled_grows_with_lap_pressure() {
        let pressure = 1.0 + 4.0 * crate::circuit::LAP_PRESSURE_PER_LAP;
        let base = spawn_wave(&tithe(), 6, 9, 0.0);
        let pressured = spawn_wave_scaled(&tithe(), 6, pressure, 9, 0.0);
        assert!(pressured.len() >= base.len(), "pressure must never shrink a wave");
    }
}
