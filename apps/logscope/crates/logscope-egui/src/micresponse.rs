//! micresponse.rs — driving a physical object from sound, so it reads as alive rather than lit.
//!
//! ## Why a single loudness scalar is not enough
//!
//! The obvious design maps microphone level to brightness or scale. Everything then moves the *same
//! way* — a leaf, a rune and a bead all swell together — and it reads as a meter with a costume on.
//!
//! Physical objects respond to different properties of the same sound:
//!
//! * a **leaf** vibrates with sustained amplitude and keeps moving after the sound stops, because it
//!   has mass;
//! * a **rune** pulses on ONSETS — the strike, not the sustain — and decays sharply;
//! * a **bead** or water surface ripples with the *rate* of change rather than the level.
//!
//! So this extracts three drivers and lets each shape choose. That is the whole difference between
//! "reacts to sound" and "looks like it exists".
//!
//! ## The two bugs that make any of this feel wrong
//!
//! **Frame-rate dependence.** A decay written as `x *= 0.9` per frame decays twice as fast at 120fps as
//! at 60. The object then behaves differently on a phone and a desktop, and nobody attributes it to
//! the animation. Every rate here is per-SECOND and integrated with `dt`.
//!
//! **Never returning to rest.** A responder that settles at a small non-zero value looks permanently
//! agitated in a silent room — worse than no animation, because it implies input that is not there.

/// Physical drivers extracted from an audio frame.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Drivers {
    /// Smoothed amplitude, 0..1. Sustains while sound continues.
    pub level: f32,
    /// Positive spikes when energy rises sharply — a strike, a consonant, a door.
    pub onset: f32,
    /// Rate of change of level; signed, so a fade-out reads differently from a swell.
    pub flux: f32,
}

/// How a shape converts drivers into motion.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Responder {
    /// Sustained amplitude, with inertia: keeps moving after the sound stops.
    Leaf,
    /// Onsets only, sharp decay. A rune lights on the strike, not the hum.
    Rune,
    /// Rate of change: ripples on the swell and the fade alike.
    Bead,
    /// Level with almost no smoothing — for a shape meant to read as a meter.
    Pearl,
}

impl Responder {
    /// Seconds for the response to fall to ~37% after the input stops.
    ///
    /// This is the mass of the thing. A leaf is heavy and keeps going; a rune is a light that switches.
    pub fn decay_s(self) -> f32 {
        match self { Responder::Leaf => 0.85, Responder::Rune => 0.18,
                     Responder::Bead => 0.45, Responder::Pearl => 0.12 }
    }

    /// Which driver this shape listens to.
    pub fn drive(self, d: &Drivers) -> f32 {
        match self {
            Responder::Leaf => d.level,
            Responder::Rune => d.onset,
            Responder::Bead => d.flux.abs(),
            Responder::Pearl => d.level,
        }
    }
}

/// A responder's animated state.
#[derive(Clone, Copy, Debug, Default)]
pub struct Motion { pub value: f32, pub velocity: f32 }

/// Below this the responder is snapped to rest.
///
/// Without it a decay approaches zero asymptotically and the object stays faintly agitated forever,
/// implying input that is not there.
pub const REST_EPS: f32 = 0.002;

impl Motion {
    /// Advance by `dt` seconds.
    ///
    /// Rates are per SECOND and integrated with dt, so a phone at 60fps and a desktop at 120 behave
    /// identically. Written as `x *= 0.9` per frame instead, the object would decay twice as fast on
    /// the faster machine and nobody would attribute it to the animation.
    pub fn step(&mut self, r: Responder, d: &Drivers, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);              // a stalled frame must not teleport the animation
        let target = r.drive(d).clamp(0.0, 1.0);
        // Rise is immediate for an onset-driven shape and eased for a mass-driven one; the difference
        // between a light switching on and a leaf being pushed.
        let rise = match r { Responder::Rune | Responder::Pearl => 1.0, _ => 6.0 * dt };
        if target > self.value { self.value += (target - self.value) * rise.min(1.0); }
        // Exponential decay at the shape's own rate.
        let k = (-dt / r.decay_s().max(1e-3)).exp();
        self.value *= k;
        // Inertia: only shapes with mass overshoot and settle.
        if matches!(r, Responder::Leaf | Responder::Bead) {
            self.velocity = self.velocity * k + (target - self.value) * dt * 4.0;
            self.value += self.velocity * dt;
        }
        self.value = self.value.clamp(0.0, 1.2);
        if self.value < REST_EPS && target < REST_EPS {
            self.value = 0.0;
            self.velocity = 0.0;
        }
    }
}

/// Extract drivers from a level reading and the previous one.
pub fn drivers(level: f32, prev_level: f32, dt: f32) -> Drivers {
    let l = level.clamp(0.0, 1.0);
    let flux = if dt > 0.0 { (l - prev_level) / dt } else { 0.0 };
    // An onset is a RISE, not a level: a loud steady tone has none. Half-wave rectified for that
    // reason — a fade-out is not a strike.
    Drivers { level: l, onset: (flux * 0.25).clamp(0.0, 1.0), flux }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silence() -> Drivers { Drivers::default() }

    #[test]
    fn a_rune_fires_on_the_STRIKE_and_a_leaf_on_the_SUSTAIN() {
        // The whole point: one loudness scalar would move both the same way and read as a meter with a
        // costume. A steady tone has amplitude and no onset.
        let steady = Drivers { level: 0.8, onset: 0.0, flux: 0.0 };
        assert_eq!(Responder::Rune.drive(&steady), 0.0, "a hum is not a strike");
        assert!(Responder::Leaf.drive(&steady) > 0.5, "but it does sustain a leaf");

        let strike = Drivers { level: 0.8, onset: 0.9, flux: 4.0 };
        assert!(Responder::Rune.drive(&strike) > 0.5);
    }

    #[test]
    fn a_steady_tone_produces_NO_onset() {
        // Half-wave rectification: an onset is a rise. Without this a rune would stay lit through a
        // sustained note and stop reading as a strike at all.
        let d = drivers(0.7, 0.7, 1.0 / 60.0);
        assert_eq!(d.onset, 0.0);
        let rising = drivers(0.7, 0.1, 1.0 / 60.0);
        assert!(rising.onset > 0.0, "a rise is an onset");
        let falling = drivers(0.1, 0.7, 1.0 / 60.0);
        assert_eq!(falling.onset, 0.0, "a fade is not a strike");
    }

    #[test]
    fn the_decay_is_FRAME_RATE_INDEPENDENT() {
        // `x *= 0.9` per frame decays twice as fast at 120fps as at 60, so the object behaves
        // differently on a phone and a desktop and nobody attributes it to the animation.
        let mut a = Motion { value: 1.0, velocity: 0.0 };
        let mut b = Motion { value: 1.0, velocity: 0.0 };
        for _ in 0..60 { a.step(Responder::Rune, &silence(), 1.0 / 60.0); }
        for _ in 0..120 { b.step(Responder::Rune, &silence(), 1.0 / 120.0); }
        assert!((a.value - b.value).abs() < 1e-3,
            "one second of decay must match: 60fps {} vs 120fps {}", a.value, b.value);
    }

    #[test]
    fn silence_returns_the_object_to_EXACT_rest() {
        // An exponential decay approaches zero asymptotically. Settling at a small non-zero value looks
        // permanently agitated in a quiet room — worse than no animation, because it implies input that
        // is not there.
        let mut m = Motion { value: 1.0, velocity: 0.3 };
        for _ in 0..300 { m.step(Responder::Leaf, &silence(), 1.0 / 60.0); }
        assert_eq!(m.value, 0.0);
        assert_eq!(m.velocity, 0.0);
    }

    #[test]
    fn a_leaf_keeps_moving_after_the_sound_stops_and_a_rune_does_not() {
        // Mass. This is the difference between a light switching off and a thing that was pushed.
        let loud = Drivers { level: 1.0, onset: 1.0, flux: 2.0 };
        let (mut leaf, mut rune) = (Motion::default(), Motion::default());
        for _ in 0..30 { leaf.step(Responder::Leaf, &loud, 1.0 / 60.0);
                         rune.step(Responder::Rune, &loud, 1.0 / 60.0); }
        for _ in 0..12 { leaf.step(Responder::Leaf, &silence(), 1.0 / 60.0);
                         rune.step(Responder::Rune, &silence(), 1.0 / 60.0); }
        assert!(leaf.value > rune.value * 2.0,
            "after 200ms of silence: leaf {} should still carry motion, rune {} should be out",
            leaf.value, rune.value);
    }

    #[test]
    fn a_stalled_frame_does_not_teleport_the_animation() {
        // A 2-second dt from a hitch would jump the decay to zero and the object would visibly snap.
        // Clamping makes a stall look like a slow frame rather than a glitch.
        let mut m = Motion { value: 1.0, velocity: 0.0 };
        m.step(Responder::Leaf, &silence(), 2.0);
        assert!(m.value > 0.5, "one hitched frame must not erase the state, got {}", m.value);
    }

    #[test]
    fn the_response_never_exceeds_its_clamp() {
        // Inertia can overshoot, which is wanted — but an unbounded overshoot would drive a scale or a
        // brightness past anything the shader expects.
        let mut m = Motion::default();
        let loud = Drivers { level: 1.0, onset: 1.0, flux: 20.0 };
        for _ in 0..600 { m.step(Responder::Bead, &loud, 1.0 / 60.0); }
        assert!(m.value <= 1.2, "got {}", m.value);
    }
}
