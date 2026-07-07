//! `sim::sound_resolve` — resolve one emitted sound as the listener hears it.
//!
//! Four physically-grounded audio cores sat orphaned; this chains them into the
//! single answer a mixer needs — a gain, a pitch factor, and a reverb wet/dry:
//!
//! * [`sound_attenuation`](crate::world::sound_attenuation) — distance rolloff,
//! * [`audio_occlusion`](crate::sim::audio_occlusion) — walls between dampen it,
//! * [`doppler_shift`](crate::world::doppler_shift) — relative motion shifts pitch,
//! * [`reverb_wetness`](crate::world::reverb_wetness) — enclosure decides the wash.
//!
//! It composes those cores; it adds only the geometry (distance + the radial
//! closing speed for Doppler). Pure and deterministic.

use crate::sim::audio_occlusion;
use crate::world::{doppler_shift, reverb_wetness, sound_attenuation};

/// A sound source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emitter {
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    /// Base volume `>= 0` (1.0 = nominal).
    pub volume: f32,
}

/// The listener.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub pos: [f32; 3],
    pub vel: [f32; 3],
}

/// The acoustic environment between emitter and listener.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Env {
    /// Solid blocks on the line of sight (occlusion).
    pub solid_blocks_between: u32,
    /// Occlusion attenuation per blocking block, in dB.
    pub per_block_db: f32,
    /// Enclosure `0` (open) .. `1` (sealed) for reverb.
    pub enclosure: f32,
    /// Room size `0..1` for reverb.
    pub room_size: f32,
}

/// Tuning constants for the falloff + Doppler.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Acoustics {
    /// Distance within which the sound is at full volume.
    pub reference: f32,
    /// Inverse rolloff steepness beyond the reference.
    pub rolloff: f32,
    /// Speed of sound (e.g. 343 m/s).
    pub speed_of_sound: f32,
}

/// What the listener actually hears.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedSound {
    /// Final gain (`volume × distance attenuation × occlusion`), `>= 0`.
    pub gain: f32,
    /// Pitch multiplier from Doppler (`> 1` approaching, `< 1` receding).
    pub pitch: f32,
    /// Reverb wet/dry mix `0..1`.
    pub wetness: f32,
    /// Distance from listener to emitter.
    pub distance: f32,
}

/// Resolve `e` for `l` through `env`, with falloff/Doppler tuning `ac`. The gain
/// chains distance attenuation and occlusion; the pitch comes from the radial
/// closing speed; the wetness from the enclosure. Pure.
pub fn resolve(e: &Emitter, l: &Listener, env: &Env, ac: &Acoustics) -> ResolvedSound {
    let dx = e.pos[0] - l.pos[0];
    let dy = e.pos[1] - l.pos[1];
    let dz = e.pos[2] - l.pos[2];
    let distance = (dx * dx + dy * dy + dz * dz).max(0.0).sqrt();

    let atten = sound_attenuation::attenuation(distance, ac.reference, ac.rolloff);
    let occ = audio_occlusion::occlusion_gain(env.solid_blocks_between, env.per_block_db);
    let gain = (e.volume.max(0.0) * atten * occ).max(0.0);

    // Radial closing speed: positive when the gap is shrinking (approaching).
    let closing = if distance > 1e-4 {
        let (ux, uy, uz) = (dx / distance, dy / distance, dz / distance);
        let (rvx, rvy, rvz) = (
            e.vel[0] - l.vel[0],
            e.vel[1] - l.vel[1],
            e.vel[2] - l.vel[2],
        );
        -(rvx * ux + rvy * uy + rvz * uz)
    } else {
        0.0
    };
    let pitch = doppler_shift::pitch_factor(closing, ac.speed_of_sound);
    let wetness = reverb_wetness::wetness(env.enclosure, env.room_size);

    ResolvedSound { gain, pitch, wetness, distance }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ac() -> Acoustics {
        Acoustics { reference: 4.0, rolloff: 1.0, speed_of_sound: 343.0 }
    }
    fn env() -> Env {
        Env { solid_blocks_between: 0, per_block_db: 3.0, enclosure: 0.0, room_size: 0.5 }
    }
    fn emitter(pos: [f32; 3], vel: [f32; 3]) -> Emitter {
        Emitter { pos, vel, volume: 1.0 }
    }
    fn listener() -> Listener {
        Listener { pos: [0.0; 3], vel: [0.0; 3] }
    }

    #[test]
    fn farther_is_quieter() {
        let near = resolve(&emitter([5.0, 0.0, 0.0], [0.0; 3]), &listener(), &env(), &ac());
        let far = resolve(&emitter([40.0, 0.0, 0.0], [0.0; 3]), &listener(), &env(), &ac());
        assert!(far.gain < near.gain && far.distance > near.distance);
    }

    #[test]
    fn walls_muffle_it() {
        let open = resolve(&emitter([10.0, 0.0, 0.0], [0.0; 3]), &listener(), &env(), &ac());
        let mut walled = env();
        walled.solid_blocks_between = 4;
        let muffled = resolve(&emitter([10.0, 0.0, 0.0], [0.0; 3]), &listener(), &walled, &ac());
        assert!(muffled.gain < open.gain, "blocks between dampen the sound");
    }

    #[test]
    fn approaching_raises_pitch_receding_lowers() {
        // emitter at +x moving toward the listener (−x) ⇒ approaching ⇒ pitch up.
        let approach = resolve(&emitter([10.0, 0.0, 0.0], [-30.0, 0.0, 0.0]), &listener(), &env(), &ac());
        let recede = resolve(&emitter([10.0, 0.0, 0.0], [30.0, 0.0, 0.0]), &listener(), &env(), &ac());
        let still = resolve(&emitter([10.0, 0.0, 0.0], [0.0; 3]), &listener(), &env(), &ac());
        assert!(approach.pitch > 1.0, "approaching ⇒ higher pitch");
        assert!(recede.pitch < 1.0, "receding ⇒ lower pitch");
        assert!((still.pitch - 1.0).abs() < 1e-4, "stationary ⇒ no shift");
    }

    #[test]
    fn tangential_motion_no_doppler() {
        // moving in +z while offset along +x ⇒ no radial component ⇒ no shift.
        let r = resolve(&emitter([10.0, 0.0, 0.0], [0.0, 0.0, 40.0]), &listener(), &env(), &ac());
        assert!((r.pitch - 1.0).abs() < 1e-3, "purely tangential motion ⇒ no pitch shift");
    }

    #[test]
    fn sealed_large_room_is_wet() {
        let mut e = env();
        e.enclosure = 1.0;
        e.room_size = 1.0;
        let r = resolve(&emitter([5.0, 0.0, 0.0], [0.0; 3]), &listener(), &e, &ac());
        assert!(r.wetness > 0.9, "a sealed large room is reverberant");
        assert!((0.0..=1.0).contains(&r.wetness));
    }

    #[test]
    fn deterministic_and_finite() {
        let e = emitter([7.0, 2.0, -3.0], [1.0, 0.0, -4.0]);
        let a = resolve(&e, &listener(), &env(), &ac());
        let b = resolve(&e, &listener(), &env(), &ac());
        assert_eq!(a, b);
        for v in [a.gain, a.pitch, a.wetness, a.distance] {
            assert!(v.is_finite());
        }
    }
}
