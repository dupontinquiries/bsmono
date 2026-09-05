//! CORRUPT BODY — turning `corruption::Stage` into something drawable,
//! at zero new asset bytes.
//!
//! `corruption.rs` is the film-script rules (evasion, detection,
//! absorption); it says nothing about what a Corrupted LOOKS like,
//! deliberately — that module's whole point is being a pure rules
//! engine. This module is the bridge from a stage to a body, and it
//! reads the doc's own language literally rather than inventing a
//! separate "horror body" asset pipeline:
//!
//! * **Stage 1/2** — "multiple alt-mode components emerging in wrong
//!   places" is a displacement, not a new shape: take an existing bot
//!   chassis from the pack that already ships (`botpack.rs`), and jitter
//!   its parts along their own local axes on a resonance clock. The
//!   chassis costs nothing new; the jitter is a handful of floats.
//! * **Stage 3** — "no fixed silhouette... absorbs mass" is, read
//!   plainly, a body with no single shape at all. `world::Sdf` already
//!   has `Sphere`/`Capsule`/`RBox`, and blending fields is what an SDF
//!   scene already does for free (`space::BlendLoad`). A Stage-3 body
//!   is a time-varying weighted mix of those three primitives — the
//!   cheapest possible body in the renderer, and the one the doc
//!   describes without anyone drawing a new mesh.
//!
//! This module does not touch `world::World` or `botpack`'s real file
//! I/O — it only computes the RECIPE (which chassis id, what jitter,
//! what blend weights) from a stage and a clock. `circuit_map.rs` is
//! where a recipe becomes an actual `Shape`.

use crate::corruption::Stage;

/// Per-part jitter for a Stage 1/2 body: an existing chassis, displaced
/// along its own local axes on a sine clock. Amplitude in metres,
/// matching the doc's escalation ("personal, close, wrong scale" at
/// Stage 1, worse at Stage 2) — (guess) magnitudes, tagged so a later
/// pass can retune them against real chassis part scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Jitter {
    pub amplitude_m: f32,
    pub hz: f32,
    /// phase offset so a crowd of Corrupted don't all twitch in unison
    pub phase: f32,
}

impl Jitter {
    /// Displacement at time `t`, in `[-amplitude_m, amplitude_m]`.
    /// Deterministic in `t` — same clock in, same wrongness out, so a
    /// replay looks identical.
    pub fn offset(&self, t: f32) -> f32 {
        self.amplitude_m * (t * self.hz * std::f32::consts::TAU + self.phase).sin()
    }
}

/// Stage-3 blend weights over the three primitives `world::Sdf` already
/// has. Always sums to 1 (a convex combination), so a caller doing
/// `w.sphere * f_sphere(p) + ...` gets a properly weighted field
/// without renormalising itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MorphWeights {
    pub sphere: f32,
    pub capsule: f32,
    pub rbox: f32,
}

impl MorphWeights {
    fn normalized(sphere: f32, capsule: f32, rbox: f32) -> MorphWeights {
        let sum = (sphere + capsule + rbox).max(1e-6);
        MorphWeights { sphere: sphere / sum, capsule: capsule / sum, rbox: rbox / sum }
    }

    /// "No fixed silhouette" as three offset cosine waves, clamped
    /// non-negative before normalising, so the shape genuinely swims
    /// between the three primitives rather than converging on a
    /// steady blend.
    pub fn at(t: f32, hz: f32, phase: f32) -> MorphWeights {
        let tau = std::f32::consts::TAU;
        let a = (t * hz * tau + phase).cos().max(0.0) + 0.05;
        let b = (t * hz * tau + phase + tau / 3.0).cos().max(0.0) + 0.05;
        let c = (t * hz * tau + phase + 2.0 * tau / 3.0).cos().max(0.0) + 0.05;
        MorphWeights::normalized(a, b, c)
    }
}

/// What to draw for a Corrupted at a given stage. Never both variants
/// at once — a caller matching on this can't accidentally treat a
/// Stage 3 body as having a fixed chassis.
#[derive(Clone, Debug, PartialEq)]
pub enum BodyRecipe {
    Chassis { chassis_id: &'static str, jitter: Jitter },
    Morph { weights: MorphWeights },
}

/// (guess) — jitter amplitude/rate per stage; Stage 3 never reaches
/// here (see `recipe_for`), so only One/Two are listed.
fn jitter_for(stage: Stage) -> Jitter {
    match stage {
        Stage::One => Jitter { amplitude_m: 0.03, hz: 0.6, phase: 0.0 },
        Stage::Two => Jitter { amplitude_m: 0.12, hz: 1.1, phase: 0.0 },
        Stage::Three => Jitter { amplitude_m: 0.0, hz: 0.0, phase: 0.0 }, // unreachable in practice
    }
}
/// (guess) — how fast the Stage-3 blend swims.
const MORPH_HZ: f32 = 0.35;

/// The recipe for one Corrupted body: `chassis_id` names an existing
/// entry in the bot pack (real selection — which id, for which
/// archetype — is `circuit_map.rs`'s job once it can see the pack;
/// this module only decides Chassis-vs-Morph and the jitter/weights).
/// `seed` gives each individual body its own phase so a crowd doesn't
/// move in lockstep.
pub fn recipe_for(stage: Stage, chassis_id: &'static str, seed: u64, t: f32) -> BodyRecipe {
    match stage {
        Stage::One | Stage::Two => {
            let mut j = jitter_for(stage);
            j.phase = phase_from_seed(seed);
            BodyRecipe::Chassis { chassis_id, jitter: j }
        }
        Stage::Three => {
            let phase = phase_from_seed(seed);
            BodyRecipe::Morph { weights: MorphWeights::at(t, MORPH_HZ, phase) }
        }
    }
}

fn phase_from_seed(seed: u64) -> f32 {
    let mut r = crate::raidgen::Rng::new(seed);
    r.f(0.0, std::f32::consts::TAU)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_one_and_two_are_always_a_chassis_never_a_morph() {
        for stage in [Stage::One, Stage::Two] {
            match recipe_for(stage, "some_chassis", 1, 0.5) {
                BodyRecipe::Chassis { .. } => {}
                BodyRecipe::Morph { .. } => panic!("{stage:?} must stay a fixed chassis"),
            }
        }
    }

    #[test]
    fn stage_three_never_resolves_to_a_chassis() {
        for seed in 0..50u64 {
            for t in [0.0, 1.0, 2.5, 100.0] {
                match recipe_for(Stage::Three, "some_chassis", seed, t) {
                    BodyRecipe::Morph { .. } => {}
                    BodyRecipe::Chassis { .. } => panic!("Stage::Three must have no fixed silhouette"),
                }
            }
        }
    }

    #[test]
    fn jitter_offset_is_bounded_by_its_own_amplitude() {
        for stage in [Stage::One, Stage::Two] {
            let j = jitter_for(stage);
            for i in 0..1000 {
                let t = i as f32 * 0.037;
                let o = j.offset(t);
                assert!(
                    o.abs() <= j.amplitude_m + 1e-5,
                    "{stage:?} offset {o} exceeded amplitude {}",
                    j.amplitude_m
                );
            }
        }
    }

    #[test]
    fn stage_two_jitters_harder_than_stage_one() {
        assert!(jitter_for(Stage::Two).amplitude_m > jitter_for(Stage::One).amplitude_m);
    }

    #[test]
    fn jitter_is_deterministic_given_the_same_clock() {
        let a = recipe_for(Stage::One, "x", 42, 3.3);
        let b = recipe_for(Stage::One, "x", 42, 3.3);
        assert_eq!(a, b);
    }

    #[test]
    fn different_seeds_give_different_phases() {
        let a = recipe_for(Stage::One, "x", 1, 0.0);
        let b = recipe_for(Stage::One, "x", 2, 0.0);
        assert_ne!(a, b, "two different bodies must not twitch in lockstep");
    }

    #[test]
    fn morph_weights_always_sum_to_one() {
        for seed in 0..200u64 {
            for t in [0.0, 0.7, 5.0, 123.456] {
                let phase = phase_from_seed(seed);
                let w = MorphWeights::at(t, MORPH_HZ, phase);
                let sum = w.sphere + w.capsule + w.rbox;
                assert!((sum - 1.0).abs() < 1e-4, "sum={sum}");
                assert!(w.sphere >= 0.0 && w.capsule >= 0.0 && w.rbox >= 0.0);
            }
        }
    }

    #[test]
    fn morph_weights_actually_vary_over_time_no_fixed_silhouette() {
        // the whole point of Stage 3: it must not converge on a steady
        // blend. Sample across a window and require real spread.
        let phase = phase_from_seed(7);
        let mut min_sphere = f32::MAX;
        let mut max_sphere = f32::MIN;
        for i in 0..200 {
            let t = i as f32 * 0.05;
            let w = MorphWeights::at(t, MORPH_HZ, phase);
            min_sphere = min_sphere.min(w.sphere);
            max_sphere = max_sphere.max(w.sphere);
        }
        assert!(
            max_sphere - min_sphere > 0.15,
            "Stage 3 silhouette barely moved: {min_sphere}..{max_sphere}"
        );
    }

    #[test]
    fn chassis_id_passes_through_unchanged() {
        match recipe_for(Stage::One, "hussar_tank", 5, 0.0) {
            BodyRecipe::Chassis { chassis_id, .. } => assert_eq!(chassis_id, "hussar_tank"),
            _ => panic!(),
        }
    }
}
