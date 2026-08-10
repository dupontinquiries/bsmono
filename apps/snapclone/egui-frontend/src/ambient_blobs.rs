//! ambient_blobs — `AmbientBlobs.cpp`, ported.
//!
//! Slow drifting colour fields behind the UI, for the synthetic palettes only.
//!
//! ## Three numbers that are not decoration
//!
//! The C++ header explains each, and the port keeps them because they were
//! chosen against a constraint rather than by eye:
//!
//! * **Opacity caps at 0.18** — the same value as the background tint
//!   saturation cap, both picked to keep text contrast inside WCAG AA for the
//!   dark palettes shipped. *"Going higher kills readability"*, and readability
//!   is the thing an ambient background is trading against.
//! * **Independent x/y frequencies with irrational-ratio periods**, so a blob's
//!   path never quite repeats. A repeating path is the difference between
//!   ambient motion and a screensaver you start watching.
//! * **Blobs only paint on synthetic palettes.** On the hand-tuned static ones
//!   they are noise, so the whole effect is skipped rather than dimmed — which
//!   is also why this costs nothing when off.
//!
//! ## Phase, not position
//!
//! Each blob stores phases and derives its position, so there is no state to
//! drift and a resize is free — positions are fractions of the current size.
//! The Qt version caches a rendered pixmap per blob and re-renders when the
//! palette mutates; that is a paint-layer concern and stays in the shell.

/// Peak alpha the blob is drawn at, modulated down per frame.
pub const PEAK_ALPHA: f32 = 0.20;
/// Hard ceiling on per-frame opacity — the WCAG-derived number.
pub const MAX_ALPHA: f32 = 0.18;

const TWO_PI: f32 = 6.283_185_3;

/// One drifting field.
#[derive(Clone, Debug, PartialEq)]
pub struct Blob {
    pub x_phase: f32,
    pub y_phase: f32,
    pub pulse_phase: f32,
    x_freq: f32,
    y_freq: f32,
    pulse_freq: f32,
    /// Radius as a fraction of the smaller viewport side.
    pub base_radius: f32,
    pub color_idx: usize,
}

impl Blob {
    /// Centre as a fraction of the viewport, 0..=1.
    ///
    /// The 0.45 amplitude keeps a blob's centre off the very edge: a field
    /// centred at 0 is a half-field, which reads as a rectangle rather than a
    /// glow.
    pub fn centre(&self) -> (f32, f32) {
        (0.5 + 0.45 * self.x_phase.sin(), 0.5 + 0.45 * self.y_phase.sin())
    }

    /// Radius in pixels for a viewport whose shorter side is `side`.
    pub fn radius(&self, side: f32) -> f32 {
        self.base_radius * side * (1.0 + 0.15 * self.pulse_phase.sin())
    }

    /// Per-frame opacity, clamped to the readability ceiling.
    ///
    /// The original's curve is `0.06 + 0.06 * (1 + sin(pulse * 0.75))`, which
    /// spans 0.06..=0.18 — so the cap is where the curve already tops out, and
    /// clamping is a guard against a future edit rather than a change in
    /// behaviour. Kept explicit so that edit gets caught here instead of in a
    /// contrast complaint.
    pub fn opacity(&self) -> f32 {
        (0.06 + 0.06 * (1.0 + (self.pulse_phase * 0.75).sin())).min(MAX_ALPHA)
    }
}

/// The field as a whole.
#[derive(Clone, Debug)]
pub struct AmbientBlobs {
    blobs: Vec<Blob>,
    speed: f32,
    /// Off for static palettes: on hand-tuned colours the blobs are noise.
    active: bool,
}

impl Default for AmbientBlobs {
    fn default() -> Self { AmbientBlobs { blobs: Vec::new(), speed: 1.0, active: false } }
}

impl AmbientBlobs {
    /// Build `n` blobs deterministically from a seed.
    ///
    /// Deterministic rather than randomised so a screenshot is reproducible and
    /// a test can assert on motion. The original seeds from a QRandomGenerator;
    /// the frequencies matter, the entropy source does not.
    pub fn seeded(n: usize, seed: u64) -> AmbientBlobs {
        // Avalanche the seed rather than `seed | 1`. That idiom avoids
        // xorshift's zero state by forcing the low bit, which also maps EVERY
        // EVEN SEED ONTO ITS ODD NEIGHBOUR — 42 and 43 produce an identical
        // field. Found by the determinism test asserting two seeds differ,
        // which is the assertion people usually skip because it looks obvious.
        let mut s = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .rotate_left(31)
            .wrapping_add(0xD1B5_4A32_D192_ED03)
            | 1;
        let mut next = || {
            s ^= s << 13; s ^= s >> 7; s ^= s << 17;
            ((s >> 11) as f32) / (u64::MAX >> 11) as f32
        };
        let blobs = (0..n).map(|i| {
            // Periods of 45..70 seconds on each axis, drawn independently. Two
            // axes with unequal periods give a Lissajous path whose ratio is
            // almost never rational, which is what stops it repeating.
            Blob {
                x_phase: next() * TWO_PI,
                y_phase: next() * TWO_PI,
                pulse_phase: next() * TWO_PI,
                x_freq: TWO_PI / (45.0 + next() * 25.0),
                y_freq: TWO_PI / (45.0 + next() * 25.0),
                pulse_freq: TWO_PI / (6.0 + next() * 4.0),
                base_radius: 0.35 + next() * 0.25,
                color_idx: i,
            }
        }).collect();
        AmbientBlobs { blobs, speed: 1.0, active: false }
    }

    pub fn set_active(&mut self, on: bool) { self.active = on; }
    pub fn is_active(&self) -> bool { self.active }
    pub fn blobs(&self) -> &[Blob] { if self.active { &self.blobs } else { &[] } }

    /// Speed multiplier, bounded as the original bounds it.
    ///
    /// The floor is not zero: a speed of zero would freeze the field mid-drift,
    /// which looks like the app hung rather than like stillness. Someone who
    /// wants no motion turns the effect off.
    pub fn set_speed(&mut self, m: f32) { self.speed = m.clamp(0.1, 4.0); }
    pub fn speed(&self) -> f32 { self.speed }

    /// Advance every phase by `dt` seconds.
    ///
    /// Phases wrap into 0..2π rather than growing without bound: an app left
    /// open for days would otherwise accumulate a float large enough that
    /// adding a frame's delta changes nothing, and the motion would silently
    /// stop.
    pub fn tick(&mut self, dt: f32) {
        if !self.active || dt <= 0.0 { return; }
        // An absurd delta — a resumed suspend — is treated as one frame rather
        // than teleporting every blob to an arbitrary phase.
        let dt = dt.min(0.25);
        for b in self.blobs.iter_mut() {
            b.x_phase = (b.x_phase + b.x_freq * dt * self.speed) % TWO_PI;
            b.y_phase = (b.y_phase + b.y_freq * dt * self.speed) % TWO_PI;
            b.pulse_phase = (b.pulse_phase + b.pulse_freq * dt * self.speed) % TWO_PI;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field() -> AmbientBlobs {
        let mut f = AmbientBlobs::seeded(4, 0xC0FFEE);
        f.set_active(true);
        f
    }

    #[test]
    fn a_static_palette_gets_no_blobs_at_all() {
        // Skipped rather than dimmed: on hand-tuned colours they are noise, and
        // skipping is also why the effect costs nothing when off.
        let f = AmbientBlobs::seeded(4, 1);
        assert!(!f.is_active());
        assert!(f.blobs().is_empty());
    }

    #[test]
    fn opacity_never_exceeds_the_readability_ceiling() {
        // 0.18 is the WCAG-derived cap shared with the background tint
        // saturation. Above it, text contrast leaves the safe zone — which is
        // the thing an ambient background trades against.
        let mut f = field();
        for _ in 0..2_000 {
            f.tick(0.05);
            for b in f.blobs() {
                assert!(b.opacity() <= MAX_ALPHA, "opacity {} exceeded", b.opacity());
                assert!(b.opacity() >= 0.0);
            }
        }
    }

    #[test]
    fn a_blob_stays_off_the_very_edge() {
        // A field centred at 0 is a half-field, which reads as a rectangle
        // rather than a glow.
        let mut f = field();
        for _ in 0..500 {
            f.tick(0.1);
            for b in f.blobs() {
                let (cx, cy) = b.centre();
                assert!((0.05..=0.95).contains(&cx), "cx {cx}");
                assert!((0.05..=0.95).contains(&cy), "cy {cy}");
            }
        }
    }

    #[test]
    fn the_path_does_not_repeat_on_a_short_cycle() {
        // Independent x and y periods give a Lissajous ratio that is almost
        // never rational — the difference between ambient motion and a
        // screensaver you start watching.
        let mut f = field();
        let first = f.blobs()[0].centre();
        let mut returned_to_start = 0;
        for _ in 0..3_000 {
            f.tick(0.05);
            let c = f.blobs()[0].centre();
            if (c.0 - first.0).abs() < 0.001 && (c.1 - first.1).abs() < 0.001 {
                returned_to_start += 1;
            }
        }
        assert!(returned_to_start <= 2, "path repeats {returned_to_start} times in 150s");
    }

    #[test]
    fn phases_wrap_rather_than_growing_without_bound() {
        // An app open for days would otherwise accumulate a float large enough
        // that adding a frame's delta changes nothing, and the motion would
        // silently stop.
        let mut f = field();
        for _ in 0..20_000 { f.tick(0.25); }
        for b in f.blobs() {
            assert!(b.x_phase.abs() <= TWO_PI, "x_phase {}", b.x_phase);
            assert!(b.pulse_phase.abs() <= TWO_PI);
        }
        let before = f.blobs()[0].centre();
        f.tick(0.05);
        assert_ne!(f.blobs()[0].centre(), before, "still moving after 80 minutes");
    }

    #[test]
    fn a_resumed_suspend_does_not_teleport_the_field() {
        // A huge delta clamps to one frame; blobs jumping to arbitrary phases
        // on resume is a visible glitch at exactly the moment a user looks.
        let mut f = field();
        let before = f.blobs()[0].centre();
        f.tick(4_000.0);
        let after = f.blobs()[0].centre();
        assert!((after.0 - before.0).abs() < 0.1, "jumped from {before:?} to {after:?}");
    }

    #[test]
    fn speed_is_bounded_and_never_zero() {
        // Zero would freeze the field mid-drift, which looks like the app hung
        // rather than like stillness. Someone who wants no motion turns the
        // effect off.
        let mut f = field();
        f.set_speed(0.0);
        assert_eq!(f.speed(), 0.1);
        f.set_speed(99.0);
        assert_eq!(f.speed(), 4.0);
        f.set_speed(-5.0);
        assert_eq!(f.speed(), 0.1);
    }

    #[test]
    fn an_inactive_field_does_not_advance() {
        // Otherwise the phases drift while invisible and switching to a
        // synthetic palette shows a field that has jumped.
        let mut f = AmbientBlobs::seeded(3, 7);
        let before = f.blobs.clone();
        f.tick(1.0);
        assert_eq!(f.blobs, before);
    }

    #[test]
    fn the_same_seed_gives_the_same_field() {
        // Deterministic so a screenshot is reproducible and a test can assert
        // on motion — the frequencies matter, the entropy source does not.
        let a = AmbientBlobs::seeded(4, 42);
        let b = AmbientBlobs::seeded(4, 42);
        assert_eq!(a.blobs, b.blobs);
        assert_ne!(AmbientBlobs::seeded(4, 43).blobs, a.blobs);
    }

    #[test]
    fn radius_scales_with_the_viewport_rather_than_being_fixed() {
        // Blobs sized in pixels would swamp a phone and vanish on a desktop.
        let f = field();
        let b = &f.blobs()[0];
        assert!(b.radius(1000.0) > b.radius(300.0) * 2.0);
    }

    #[test]
    fn a_negative_delta_does_not_run_the_field_backwards() {
        let mut f = field();
        let before = f.blobs()[0].centre();
        f.tick(-1.0);
        assert_eq!(f.blobs()[0].centre(), before);
    }
}
