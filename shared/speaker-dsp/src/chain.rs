//! chain — the whole speaker path for a two-speaker phone, built from the other modules.
//!
//! ```text
//!   L ─┐                       ┌─ protective HP (top)    ─ + ─ warmth ─ compressor ─ limiter ─► top
//!      ├─ mono ─┬──────────────┤                           │
//!   R ─┘        │              └─ protective HP (bottom) ─ + ─ warmth ─ compressor ─ limiter ─► bottom
//!               └─ virtual bass (harmonics above both cutoffs) ──────────┘
//! ```
//!
//! Bass is mono (low frequencies carry no useful stereo information and a phone's speakers are
//! centimetres apart), so L and R are summed once, the harmonics are made once, and each speaker
//! gets its own protective high-pass, tone, dynamics and ceiling — the earpiece is a smaller driver
//! with a lower ceiling than the main speaker.
//!
//! ## Where the power efficiency comes from
//!
//! 1. **Nothing below the cutoff is sent.** A small driver radiates negligible power there but
//!    still moves and heats; the 4th-order high-pass removes it. (Tested as the fraction of output
//!    energy below the cutoff.)
//! 2. **A hard ceiling.** The lookahead limiter keeps what the harmonic stage adds from reaching
//!    the voice coil — people raise the volume because virtual bass sounds good.
//! 3. **No work at rest.** The virtual-bass stage returns immediately on silence, and with
//!    `amount == 0` it is skipped entirely.

use crate::loudness::LoudnessComp;
use crate::model::SpeakerModel;
use crate::vbass::{VirtualBass, VirtualBassParams};
use studio_fx::{BandKind, Biquad, Compressor, Limiter};

const Q_BUTTERWORTH: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// Centre of the low-mid warmth lift, Hz. Above both speakers' cutoffs, so it is radiated.
const WARMTH_HZ: f32 = 700.0;

#[derive(Debug, Clone)]
struct Output {
    hp: [Biquad; 2],
    warmth: Biquad,
    comp: Compressor,
    lim: Limiter,
}

impl Output {
    fn new(rate: f32, m: &SpeakerModel) -> Output {
        let hp = Biquad::design(BandKind::HighPass, rate, m.f_low_hz, Q_BUTTERWORTH, 0.0);
        // A gentle compressor: 2:1 well above the ceiling-ish region, soft knee. It only evens out
        // dynamics; the limiter, not this, is the protection.
        let mut comp = Compressor::new(rate, m.ceiling_dbfs - 9.0, 2.0, 15.0, 160.0);
        comp.knee_db = 8.0;
        Output {
            hp: [hp; 2],
            warmth: Biquad::design(BandKind::Peaking, rate, WARMTH_HZ, 0.7, 0.0),
            comp,
            lim: Limiter::new(rate, m.ceiling_dbfs, 2.0, 80.0),
        }
    }

    #[inline]
    fn run(&mut self, dry: f32, harmonics: f32) -> f32 {
        // The protective high-pass sees the SUM, so "nothing below the cutoff reaches the speaker"
        // holds whatever the harmonic stage does, not only because it happens to filter itself.
        let mut s = dry + harmonics;
        for f in &mut self.hp { s = f.process(s); }
        s = self.warmth.process(s);
        s = self.comp.process(s);
        self.lim.process(s)
    }
}

#[derive(Debug, Clone)]
pub struct SpeakerChain {
    rate: f32,
    vb: VirtualBass,
    base_amount: f32,
    loudness: LoudnessComp,
    level: f32,
    top: Output,
    bottom: Output,
    top_model: SpeakerModel,
    bottom_model: SpeakerModel,
}

impl SpeakerChain {
    /// `top` is the earpiece-as-speaker, `bottom` the main speaker.
    pub fn new(rate: f32, top: SpeakerModel, bottom: SpeakerModel, params: VirtualBassParams) -> SpeakerChain {
        // Harmonics are made ONCE, down to the most permissive speaker's cutoff; each speaker's own
        // protective high-pass (which sees the sum) then trims what it cannot use. Using the HIGHER
        // cutoff here would starve a case-loaded main speaker of the 2nd/3rd harmonics the old
        // chain gave it just because the earpiece cannot play them.
        let lowest = top.f_low_hz.min(bottom.f_low_hz);
        let shared = SpeakerModel {
            name: "shared",
            f_low_hz: lowest,
            f_harm_top_hz: top.f_harm_top_hz.min(bottom.f_harm_top_hz).max(lowest * 1.5),
            ceiling_dbfs: top.ceiling_dbfs.min(bottom.ceiling_dbfs),
            provenance: top.provenance,
        };
        let mut c = SpeakerChain {
            rate,
            vb: VirtualBass::new(rate, &shared, params),
            base_amount: params.amount,
            loudness: LoudnessComp::default(),
            level: 0.75,
            top: Output::new(rate, &top),
            bottom: Output::new(rate, &bottom),
            top_model: top,
            bottom_model: bottom,
        };
        c.set_listening_level(0.75);
        c
    }

    /// The FP5 as assumed (see [`SpeakerModel`]), default tune.
    pub fn fp5(rate: f32) -> SpeakerChain {
        SpeakerChain::new(rate, SpeakerModel::fp5_earpiece(), SpeakerModel::fp5_bottom(), VirtualBassParams::default())
    }

    /// Tell the chain the sink volume (0..1) so loudness compensation can follow it.
    pub fn set_listening_level(&mut self, level: f32) {
        self.level = level;
        let c = self.loudness.at(level);
        let mut p = self.vb.params;
        p.amount = self.base_amount * c.vbass_scale;
        self.vb.set_params(p);
        let w = |m: &SpeakerModel, rate: f32| Biquad::design(BandKind::Peaking, rate, WARMTH_HZ, 0.7, c.warmth_db.min(m.ceiling_dbfs.abs()));
        self.top.warmth = w(&self.top_model, self.rate);
        self.bottom.warmth = w(&self.bottom_model, self.rate);
    }

    pub fn listening_level(&self) -> f32 { self.level }

    /// Samples of delay the chain adds (the limiter's lookahead). A host reports this so video
    /// stays in sync.
    pub fn latency_samples(&self) -> usize { self.top.lim.lookahead_samples().max(self.bottom.lim.lookahead_samples()) }

    /// One stereo frame in, `(top, bottom)` out.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let mono = 0.5 * (l + r);
        let h = self.vb.process(mono);
        (self.top.run(mono, h), self.bottom.run(mono, h))
    }

    pub fn process_block(&mut self, l: &[f32], r: &[f32], top: &mut [f32], bottom: &mut [f32]) {
        let n = l.len().min(r.len()).min(top.len()).min(bottom.len());
        for i in 0..n {
            let (t, b) = self.process(l[i], r[i]);
            top[i] = t;
            bottom[i] = b;
        }
    }
}
