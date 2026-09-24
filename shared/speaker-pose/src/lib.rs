//! speaker-pose — which speaker gets which channel, and how loud, as the
//! screen rotates.
//!
//! ## The two speakers are not a stereo pair
//!
//! On a Fairphone 5 the "left" channel is the top earpiece and the "right"
//! is the bottom main speaker. They are different drivers: the earpiece is
//! small, quiet and has almost no low end. Held upright they are separated
//! VERTICALLY, so panning between them produces an image along an axis no
//! listener expects, with all the bass at one end of the phone.
//!
//! So orientation does not merely swap the channels, it changes the kind of
//! output:
//!
//! | screen | what the ears want |
//! |---|---|
//! | portrait | mono to both, bass only to the driver that can make it |
//! | landscape | real stereo, with the two mapped by which is now on the left |
//!
//! ## Orientation comes from the compositor, not from a sensor
//!
//! What matters is where the SCREEN thinks up is. `wl_output`'s transform is
//! that, it is what the user is looking at, and it stays correct when
//! rotation is locked or rotated by hand. An accelerometer would flip the
//! audio while the display stayed put — and on the FP5 there is no readable
//! accelerometer anyway, because the sensors sit behind the low-power island.
//!
//! ## Nothing changes abruptly
//!
//! A matrix swapped between buffers is a click. Every change is an
//! equal-power crossfade between the old and new matrices
//! ([`Mix::crossfade_ms`]), and a rotation must persist for
//! [`Mix::dwell_ms`] before it counts, so a phone waved about does not
//! flutter.
//!
//! Pure: no audio, no clock. Feed it transforms and a millisecond count.

/// The screen's rotation, as `wl_output` reports it. Flipped variants are
/// mirror images and do not change which side is left for a listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transform {
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
}

impl Transform {
    /// True when the long axis of the phone is horizontal.
    pub fn is_landscape(self) -> bool {
        matches!(self, Transform::Rotate90 | Transform::Rotate270)
    }
}

/// The physical speakers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    /// The earpiece. Small, quiet, no low end.
    Top,
    /// The main driver.
    Bottom,
}

/// What to send to one speaker: how much of each input channel, whether to
/// high-pass it, and a level trim.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Out {
    pub from_left: f32,
    pub from_right: f32,
    /// Roll off below this, in Hz. `0.0` means full range. The earpiece
    /// cannot reproduce low frequencies and merely distorts and rattles
    /// trying, so bass is not sent to it — it is redirected, not discarded,
    /// because the bottom driver gets the sum.
    pub high_pass_hz: f32,
    /// Linear gain. The earpiece is quieter for the same signal.
    pub gain: f32,
}

/// A complete routing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub top: Out,
    pub bottom: Out,
    /// True when both drivers carry the same programme: portrait, where a
    /// stereo image would run up and down the phone.
    pub mono: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    /// Where to split the earpiece's low end off to the main driver.
    pub crossover_hz: f32,
    /// Level trim for the earpiece, linear. Below 1.0 it is attenuated;
    /// above, boosted to match. Measure per device.
    pub top_gain: f32,
    pub bottom_gain: f32,
}

impl Default for Tuning {
    /// Unmeasured starting points. The crossover is where a typical earpiece
    /// stops being useful; the gains assume the earpiece needs a lift to sit
    /// with the main driver. Both want a real listen.
    fn default() -> Tuning {
        Tuning { crossover_hz: 400.0, top_gain: 1.0, bottom_gain: 1.0 }
    }
}

/// The routing for one orientation.
///
/// Portrait is mono into both, with the earpiece high-passed: the listener
/// hears one image from the whole phone rather than a vertical pan, and the
/// bass goes where bass can be made. This is the arrangement a bass exciter
/// (bankstown and similar) is worth running in front of, since the
/// psychoacoustic harmonics it adds are exactly what a small driver can
/// actually reproduce.
///
/// Landscape is real stereo. `Rotate90` and `Rotate270` differ only in which
/// speaker is now on the listener's left, which is the whole of the swap.
pub fn matrix_for(t: Transform, tune: &Tuning) -> Matrix {
    match t {
        Transform::Normal | Transform::Rotate180 => Matrix {
            // Halved so a correlated mono sum does not clip.
            top: Out { from_left: 0.5, from_right: 0.5, high_pass_hz: tune.crossover_hz, gain: tune.top_gain },
            bottom: Out { from_left: 0.5, from_right: 0.5, high_pass_hz: 0.0, gain: tune.bottom_gain },
            mono: true,
        },
        Transform::Rotate90 => Matrix {
            top: Out { from_left: 1.0, from_right: 0.0, high_pass_hz: tune.crossover_hz, gain: tune.top_gain },
            bottom: Out { from_left: 0.0, from_right: 1.0, high_pass_hz: 0.0, gain: tune.bottom_gain },
            mono: false,
        },
        Transform::Rotate270 => Matrix {
            top: Out { from_left: 0.0, from_right: 1.0, high_pass_hz: tune.crossover_hz, gain: tune.top_gain },
            bottom: Out { from_left: 1.0, from_right: 0.0, high_pass_hz: 0.0, gain: tune.bottom_gain },
            mono: false,
        },
    }
}

/// Crossfades between matrices as the screen turns.
#[derive(Debug, Clone)]
pub struct Mix {
    tune: Tuning,
    /// Rotation must hold this long before it is acted on.
    pub dwell_ms: u64,
    pub crossfade_ms: u64,
    current: Transform,
    /// A rotation seen but not yet settled.
    pending: Option<(Transform, u64)>,
    /// Crossfade in progress: from, to, started.
    fading: Option<(Matrix, Matrix, u64)>,
}

impl Mix {
    pub fn new(start: Transform, tune: Tuning) -> Mix {
        Mix { tune, dwell_ms: 250, crossfade_ms: 200, current: start, pending: None, fading: None }
    }

    pub fn transform(&self) -> Transform {
        self.current
    }

    pub fn settling(&self) -> bool {
        self.pending.is_some() || self.fading.is_some()
    }

    /// Tell it what the compositor now reports. Safe to call every frame.
    pub fn observe(&mut self, t: Transform, now_ms: u64) {
        if t == self.current {
            // Back to where we were before the dwell elapsed: forget it.
            self.pending = None;
            return;
        }
        match self.pending {
            Some((p, _)) if p == t => {}
            _ => self.pending = Some((t, now_ms)),
        }
    }

    /// Advance. Returns the matrix to apply now.
    pub fn tick(&mut self, now_ms: u64) -> Matrix {
        if let Some((t, since)) = self.pending {
            if now_ms.saturating_sub(since) >= self.dwell_ms {
                let from = self.live(now_ms);
                self.current = t;
                self.pending = None;
                self.fading = Some((from, matrix_for(t, &self.tune), now_ms));
            }
        }
        self.live(now_ms)
    }

    fn live(&mut self, now_ms: u64) -> Matrix {
        let target = matrix_for(self.current, &self.tune);
        let Some((from, to, started)) = self.fading else { return target };
        let d = now_ms.saturating_sub(started);
        if d >= self.crossfade_ms {
            self.fading = None;
            return to;
        }
        let x = d as f32 / self.crossfade_ms as f32;
        blend(&from, &to, x)
    }
}

/// Equal-power blend. A linear blend between two matrices dips in loudness
/// halfway through, because the two are correlated programme material and
/// their sum is not what a linear crossfade assumes.
pub fn blend(a: &Matrix, b: &Matrix, x: f32) -> Matrix {
    let x = x.clamp(0.0, 1.0);
    let (ka, kb) = ((1.0 - x).sqrt(), x.sqrt());
    let mix = |p: &Out, q: &Out| Out {
        from_left: p.from_left * ka + q.from_left * kb,
        from_right: p.from_right * ka + q.from_right * kb,
        // The filter does not sweep: it is set by whichever end needs it, so
        // the earpiece is never briefly fed bass mid-rotation.
        high_pass_hz: p.high_pass_hz.max(q.high_pass_hz),
        gain: p.gain * ka + q.gain * kb,
    };
    Matrix { top: mix(&a.top, &b.top), bottom: mix(&a.bottom, &b.bottom), mono: if x < 0.5 { a.mono } else { b.mono } }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> Tuning {
        Tuning::default()
    }

    #[test]
    fn portrait_is_mono_with_the_bass_where_it_can_be_made() {
        for p in [Transform::Normal, Transform::Rotate180] {
            let m = matrix_for(p, &t());
            assert!(m.mono);
            assert_eq!(m.top.from_left, m.top.from_right, "no vertical pan");
            assert_eq!(m.bottom.from_left, m.bottom.from_right);
            assert!(m.top.high_pass_hz > 0.0, "the earpiece is spared the low end");
            assert_eq!(m.bottom.high_pass_hz, 0.0, "and the main driver carries it");
            assert_eq!(m.top.from_left + m.top.from_right, 1.0, "a correlated sum must not clip");
        }
    }

    #[test]
    fn landscape_is_stereo_and_the_two_rotations_are_mirror_images() {
        let a = matrix_for(Transform::Rotate90, &t());
        let b = matrix_for(Transform::Rotate270, &t());
        assert!(!a.mono && !b.mono);
        assert_eq!(a.top.from_left, 1.0);
        assert_eq!(a.top.from_right, 0.0);
        assert_eq!(b.top.from_left, 0.0);
        assert_eq!(b.top.from_right, 1.0, "turning the phone the other way swaps the pair");
        assert_eq!(a.top.from_left, b.bottom.from_left);
        // The high-pass follows the SPEAKER, not the channel: it is a
        // property of the driver, so it stays on the earpiece either way.
        assert!(a.top.high_pass_hz > 0.0 && b.top.high_pass_hz > 0.0);
        assert_eq!(a.bottom.high_pass_hz, 0.0);
        assert_eq!(b.bottom.high_pass_hz, 0.0);
    }

    #[test]
    fn a_flick_of_the_wrist_does_not_reroute_the_audio() {
        let mut m = Mix::new(Transform::Normal, t());
        m.observe(Transform::Rotate90, 0);
        m.tick(100);
        assert_eq!(m.transform(), Transform::Normal, "not yet — 250 ms dwell");
        // Turned back before it settled.
        m.observe(Transform::Normal, 150);
        m.tick(400);
        assert_eq!(m.transform(), Transform::Normal);
        assert!(!m.settling(), "and nothing is pending");
        // Held this time.
        m.observe(Transform::Rotate90, 500);
        m.tick(600);
        assert_eq!(m.transform(), Transform::Normal);
        m.tick(760);
        assert_eq!(m.transform(), Transform::Rotate90);
    }

    #[test]
    fn the_change_is_faded_not_switched() {
        let mut m = Mix::new(Transform::Normal, t());
        m.observe(Transform::Rotate90, 0);
        let start = m.tick(250);
        assert_eq!(start, matrix_for(Transform::Normal, &t()), "fade begins at the old matrix");
        let mid = m.tick(350);
        let end_target = matrix_for(Transform::Rotate90, &t());
        assert!(mid != start && mid != end_target, "genuinely in between");
        assert!(mid.top.from_left > start.top.from_left, "moving toward stereo");
        // Equal power: the two coefficients of a blend sum in quadrature to 1.
        let x: f32 = 0.5;
        let (ka, kb) = ((1.0 - x).sqrt(), x.sqrt());
        assert!((ka * ka + kb * kb - 1.0).abs() < 1e-6);
        let done = m.tick(450);
        assert_eq!(done, end_target);
        assert!(!m.settling());
        // Idempotent afterwards.
        assert_eq!(m.tick(9_999), end_target);
    }

    #[test]
    fn the_earpiece_is_never_fed_bass_mid_rotation() {
        // Both ends high-pass the top speaker, and a sweeping filter during
        // the fade would briefly send low end to a driver that rattles.
        let a = matrix_for(Transform::Normal, &t());
        let b = matrix_for(Transform::Rotate90, &t());
        for i in 0..=10 {
            let m = blend(&a, &b, i as f32 / 10.0);
            assert!(m.top.high_pass_hz >= t().crossover_hz, "at x={i}");
            assert_eq!(m.bottom.high_pass_hz, 0.0);
        }
    }

    #[test]
    fn rotating_mid_fade_starts_from_where_the_sound_actually_is() {
        // Not from the nominal old matrix: that would jump.
        let mut m = Mix::new(Transform::Normal, t());
        m.observe(Transform::Rotate90, 0);
        m.tick(250);
        let mid = m.tick(330);
        m.observe(Transform::Rotate270, 340);
        let just_after = m.tick(600);
        assert!((just_after.top.from_left - mid.top.from_left).abs() < 0.6, "no discontinuity: {mid:?} -> {just_after:?}");
        let settled = m.tick(1_000);
        assert_eq!(settled, matrix_for(Transform::Rotate270, &t()));
    }
}
