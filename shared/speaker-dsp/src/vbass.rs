//! vbass — virtual bass: the "missing fundamental" trick, placed where the speaker can use it.
//!
//! ## The psychoacoustics
//!
//! The ear infers a pitch from the *spacing* of a harmonic series, not only from the lowest
//! partial: play 400, 480 and 560 Hz together and listeners hear an 80 Hz note, even with nothing
//! at 80 Hz. A phone speaker radiates almost nothing below a few hundred hertz, so instead of
//! sending it bass it cannot reproduce, we synthesise the harmonics of that bass and send those.
//! This is what bankstown does (band-limit the bass, pass it through a nonlinearity, high-pass the
//! result, blend it back), and what Waves' MaxxBass patented as a "nonlinear device".
//!
//! ## What is different here
//!
//! bankstown stresses the 2nd and 3rd harmonic. For a note at 80 Hz those are 160 and 240 Hz —
//! below a phone speaker's cutoff, so they are neither heard nor free: they only move the cone.
//! The harmonics that matter are the first ones ABOVE the speaker's cutoff (5th-7th for an 80 Hz
//! note on a 400 Hz speaker), so the output is filtered to [`SpeakerModel::f_low_hz`],
//! [`SpeakerModel::f_harm_top_hz`] and nothing else survives to the speaker.
//!
//! ## Why it normalises first
//!
//! A nonlinearity makes harmonics in proportion to how hard it is driven, so a quiet bass line
//! through a fixed curve makes no harmonics and a loud one makes too many. The band is divided by
//! its own envelope before the curve and multiplied by it after, so the harmonic *character* is
//! constant and the harmonic *level* follows the music.
//!
//! ## Warmth
//!
//! An asymmetric curve makes EVEN harmonics as well as odd (see `studio_fx::saturation`): the 2nd
//! harmonic is an octave, which reads as thickness, where odd-only harmonics read as edge.
//! `warmth` biases the curve; the default is deliberately gentle.

use crate::model::SpeakerModel;
use studio_fx::saturation::{Profile, Saturator};
use studio_fx::{BandKind, Biquad};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VirtualBassParams {
    /// Bottom of the band whose harmonics are synthesised, Hz (bankstown `floor`).
    pub floor_hz: f32,
    /// Top of that band, Hz (bankstown `ceil`).
    pub ceil_hz: f32,
    /// Level of the synthesised harmonics, linear. 0 disables (and skips all work).
    pub amount: f32,
    /// 0..1: how hard the curve is driven, i.e. how far up the harmonic series it reaches.
    pub drive: f32,
    /// 0..1: asymmetry, i.e. how much EVEN-harmonic warmth.
    pub warmth: f32,
}

impl Default for VirtualBassParams {
    fn default() -> Self {
        // Conservative on purpose: smooth and warm, not bassy. A real tune comes from listening
        // plus a measured speaker model.
        VirtualBassParams { floor_hz: 45.0, ceil_hz: 180.0, amount: 0.45, drive: 0.6, warmth: 0.35 }
    }
}

#[derive(Debug, Clone)]
pub struct VirtualBass {
    pub params: VirtualBassParams,
    band_hp: [Biquad; 2],
    band_lp: [Biquad; 2],
    sat: Saturator,
    out_hp: [Biquad; 3],
    out_lp: [Biquad; 2],
    env: f32,
    release: f32,
}

const Q_BUTTERWORTH: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// Below this envelope the band is silence: do no work and make no harmonics.
const SILENCE: f32 = 1e-5;

impl VirtualBass {
    pub fn new(rate: f32, speaker: &SpeakerModel, params: VirtualBassParams) -> VirtualBass {
        let hp = |f: f32| Biquad::design(BandKind::HighPass, rate, f, Q_BUTTERWORTH, 0.0);
        let lp = |f: f32| Biquad::design(BandKind::LowPass, rate, f, Q_BUTTERWORTH, 0.0);
        let mut sat = Saturator::new(Profile::Tape);
        sat.auto_gain = false; // the envelope normalisation owns the level
        let mut v = VirtualBass {
            params,
            band_hp: [hp(params.floor_hz); 2],
            band_lp: [lp(params.ceil_hz); 2],
            sat,
            // The output band: stay above the speaker's cutoff (a little below it, so the
            // speaker's own roll-off — not ours — shapes the edge) and below the harshness zone.
            // Three sections (36 dB/oct): the 3rd harmonic of the bass is the strongest thing the
            // curve makes and sits just below the cutoff; a gentler slope left it only ~6 dB under the
            // in-band harmonics (measured), where it would move the cone for no audible gain.
            //
            // The corner is the HIGHER of the speaker's cutoff and just under the band's top
            // (`ceil_hz`): the synthesised signal must not overlap the band the dry bass already
            // occupies. Where it did (a 40 Hz-cutoff speaker let the wet fundamental through) it was
            // added to the dry bass with a phase the filters' delays decided, and the bass response
            // rippled by about +/-3.5 dB between 45 and 250 Hz (measured) — the opposite of smooth.
            out_hp: [hp((speaker.f_low_hz * 0.95).max(params.ceil_hz * 0.9)); 3],
            out_lp: [lp(speaker.f_harm_top_hz); 2],
            env: 0.0,
            // ~40 ms release at 48 kHz: fast enough to follow a kick, slow enough not to ripple.
            release: (-1.0 / (0.040 * rate)).exp(),
        };
        v.apply_params();
        v
    }

    fn apply_params(&mut self) {
        self.sat.drive = self.params.drive.clamp(0.0, 1.0);
        self.sat.bias = self.params.warmth.clamp(0.0, 1.0) * 0.3;
    }

    pub fn set_params(&mut self, p: VirtualBassParams) {
        self.params = p;
        self.apply_params();
    }

    /// The harmonics ONLY — what to ADD to the (high-passed) dry signal. Silence in, silence out.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        if self.params.amount <= 0.0 { return 0.0; }
        let mut b = x;
        for f in &mut self.band_hp { b = f.process(b); }
        for f in &mut self.band_lp { b = f.process(b); }
        let a = b.abs();
        self.env = if a > self.env { a } else { self.env * self.release };
        if self.env < SILENCE { return 0.0; }
        let normalised = (b / self.env).clamp(-1.0, 1.0);
        let mut h = self.sat.process(normalised) * self.env;
        for f in &mut self.out_hp { h = f.process(h); }
        for f in &mut self.out_lp { h = f.process(h); }
        h * self.params.amount
    }
}
