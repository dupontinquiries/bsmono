//! Ambient sound floor and masking.
//!
//! Hearing in the engine is per-source and unmasked: each sound is judged on its
//! own attenuated loudness. But a real listener picks a sound out of a *noise
//! floor* — wind, leaves rustling, a nearby stream, a flock of chattering birds.
//! A loud environment raises the bar to notice you, so a player creeping through
//! a windy grove is genuinely harder to hear than one in dead silence, even
//! making the same noise.
//!
//! This adds that floor. Environmental [`NoiseEmitter`]s (trees in wind, water,
//! other noisy mobs) and a global wind level combine — sublinearly, the way real
//! loudness does — into an ambient floor at the listener, occlusion-aware (a wall
//! between you and the rustling trees lowers their contribution). Detection then
//! works on the *signal over noise*: a sound registers only as far as it rises
//! above the floor. Pure and world-generic.

use crate::sim::fixed::Vec3;
use crate::sim::physics::WorldQuery;
use crate::sim::sound::{heard_loudness_world, Loudness, SoundSource};

/// A standing environmental noise source — a tree rustling in wind, a stream, a
/// noisy mob. Contributes to the listener's floor, attenuated like any sound.
#[derive(Debug, Clone, Copy)]
pub struct NoiseEmitter {
    pub pos: Vec3,
    pub loudness: Loudness,
}

/// Combine two loudness levels the way sound actually adds: the louder dominates
/// and the quieter contributes a diminishing share, so stacking noise raises the
/// floor sublinearly rather than summing to absurd totals.
pub fn combine_loudness(a: Loudness, b: Loudness) -> Loudness {
    let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
    hi.saturating_add(lo / 2)
}

/// The ambient noise floor at `listener`: global `wind` combined with every
/// nearby emitter's occlusion-attenuated contribution.
pub fn ambient_floor<W: WorldQuery>(
    listener: Vec3,
    wind: Loudness,
    emitters: &[NoiseEmitter],
    world: &W,
) -> Loudness {
    let mut floor = wind.max(0);
    for e in emitters {
        let contrib = heard_loudness_world(SoundSource { pos: e.pos, loudness: e.loudness }, listener, world);
        if contrib > 0 {
            floor = combine_loudness(floor, contrib);
        }
    }
    floor
}

/// How far a sound rises above the noise floor — the part a listener can
/// actually pick out. Clamped at zero (fully masked).
pub fn signal_over_noise(perceived_source: Loudness, floor: Loudness) -> Loudness {
    (perceived_source - floor).max(0)
}

/// Whether `perceived_source` is audible against `floor` at `threshold` — i.e.
/// it clears the noise by at least the listener's sensitivity.
pub fn audible_over(perceived_source: Loudness, floor: Loudness, threshold: Loudness) -> bool {
    signal_over_noise(perceived_source, floor) >= threshold
}

/// The masked loudness of a source as heard against an ambient floor: the
/// occlusion-attenuated perceived loudness, minus what the floor swallows. Feed
/// this where you'd otherwise feed raw `heard_loudness_world` so masking applies.
pub fn heard_loudness_masked<W: WorldQuery>(
    source: SoundSource,
    listener: Vec3,
    floor: Loudness,
    world: &W,
) -> Loudness {
    let perceived = heard_loudness_world(source, listener, world);
    signal_over_noise(perceived, floor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixed::Fixed;
    use crate::sim::physics::{BlockPhysics, SceneWorld};

    fn at(x: i64, z: i64) -> Vec3 {
        Vec3::new(Fixed::from_int(x), Fixed::from_int(1), Fixed::from_int(z))
    }

    #[test]
    fn combine_is_sublinear() {
        // Two equal sources are louder than one but far short of doubling.
        let c = combine_loudness(40, 40);
        assert!(c > 40 && c < 80, "got {c}");
        // Order doesn't matter.
        assert_eq!(combine_loudness(10, 50), combine_loudness(50, 10));
    }

    #[test]
    fn floor_rises_with_nearby_emitters() {
        let world = SceneWorld::new(); // open air
        let listener = at(0, 0);
        let quiet = ambient_floor(listener, 5, &[], &world);
        let windy = ambient_floor(listener, 5, &[NoiseEmitter { pos: at(2, 0), loudness: 60 }], &world);
        assert!(windy > quiet, "a rustling tree nearby raises the floor");
    }

    #[test]
    fn an_occluding_wall_lowers_the_floor_contribution() {
        let listener = at(0, 0);
        let emitter = NoiseEmitter { pos: at(6, 0), loudness: 80 };
        let open = SceneWorld::new();
        let mut walled = SceneWorld::new();
        for y in 0..4 {
            walled.set(3, y, 0, BlockPhysics::solid(Fixed::ZERO)); // wall between
        }
        let f_open = ambient_floor(listener, 0, &[emitter], &open);
        let f_walled = ambient_floor(listener, 0, &[emitter], &walled);
        assert!(f_walled < f_open, "a wall muffles the rustling trees");
    }

    #[test]
    fn a_quiet_field_exposes_you_a_noisy_grove_hides_you() {
        let world = SceneWorld::new();
        let listener = at(0, 0);
        let threshold = 30;
        // Same player, same distance, same noise — only the environment differs.
        let player = SoundSource { pos: at(8, 0), loudness: 80 };

        let open_floor = ambient_floor(listener, 4, &[], &world);
        let grove_floor = ambient_floor(
            listener,
            20,
            &[
                NoiseEmitter { pos: at(3, 1), loudness: 70 },
                NoiseEmitter { pos: at(2, -2), loudness: 70 },
                NoiseEmitter { pos: at(4, 2), loudness: 60 },
            ],
            &world,
        );
        assert!(grove_floor > open_floor);

        let heard_open = heard_loudness_masked(player, listener, open_floor, &world);
        let heard_grove = heard_loudness_masked(player, listener, grove_floor, &world);
        assert!(heard_open >= threshold, "in the open the player stands out: {heard_open}");
        assert!(heard_grove < heard_open, "the grove masks the player: {heard_grove} < {heard_open}");
    }

    #[test]
    fn full_masking_zeroes_a_quiet_sound() {
        // A faint sound under a loud floor is inaudible (clamped at zero).
        assert_eq!(signal_over_noise(20, 50), 0);
        assert!(!audible_over(20, 50, 1));
    }
}
