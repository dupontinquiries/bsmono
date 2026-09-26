//! studio-fx — the effects.
//!
//! Nothing here is novel, and that is the point: EQ, dynamics, transform,
//! colour and retiming are *settled*, with textbook-correct behaviour that can
//! be asserted rather than eyeballed. Each one is a plain struct with a `process`
//! and no allocation in the hot path, so the same code runs in a render and in a
//! realtime callback.
//!
//! Effects are addressed by `studio_core::Effect { kind, params }` — an opaque
//! name plus a parameter map — so a project made with a plugin you lack still
//! opens. [`instantiate`] is the bridge: it turns that description into
//! something that runs, or `None` if this build does not know the kind.

pub mod schema;
pub mod compositor;
pub mod reference;
pub mod analysis;
pub mod playrate;
pub mod space;
pub mod saturation;
pub mod multiband;
pub mod imager;
pub mod chain;
pub mod assistant;
pub mod retune;
pub mod adaptive_sat;
pub mod presets;
pub mod voice;
pub mod psola;
pub mod realtune;
pub mod autotune;
pub mod loudness;

use std::collections::BTreeMap;
use studio_core::Ticks;

fn p(params: &BTreeMap<String, f32>, k: &str, default: f32) -> f32 {
    params.get(k).copied().unwrap_or(default)
}

// ── EQ ──────────────────────────────────────────────────────────────────────

/// A biquad, from the RBJ cookbook coefficients everyone uses.
///
/// Direct Form I: it costs one more state variable than DF-II and is markedly
/// better behaved with time-varying coefficients, which is exactly what happens
/// when a user drags a frequency knob.
#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    b0: f32, b1: f32, b2: f32, a1: f32, a2: f32,
    x1: f32, x2: f32, y1: f32, y2: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BandKind { Peaking, LowShelf, HighShelf, LowPass, HighPass }

impl Biquad {
    pub fn design(kind: BandKind, rate: f32, freq: f32, q: f32, gain_db: f32) -> Biquad {
        // Clamp before the trig: a frequency at or past Nyquist produces
        // coefficients that blow up, and a user CAN drag a slider there.
        let f = freq.clamp(1.0, rate * 0.49);
        let q = q.max(0.01);
        let a = 10f32.powf(gain_db / 40.0);
        let w = std::f32::consts::TAU * f / rate;
        let (sn, cs) = (w.sin(), w.cos());
        let alpha = sn / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            BandKind::Peaking => (
                1.0 + alpha * a, -2.0 * cs, 1.0 - alpha * a,
                1.0 + alpha / a, -2.0 * cs, 1.0 - alpha / a,
            ),
            BandKind::LowShelf => {
                let s = 2.0 * (a).sqrt() * alpha;
                (a * ((a + 1.0) - (a - 1.0) * cs + s),
                 2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                 a * ((a + 1.0) - (a - 1.0) * cs - s),
                 (a + 1.0) + (a - 1.0) * cs + s,
                 -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                 (a + 1.0) + (a - 1.0) * cs - s)
            }
            BandKind::HighShelf => {
                let s = 2.0 * (a).sqrt() * alpha;
                (a * ((a + 1.0) + (a - 1.0) * cs + s),
                 -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                 a * ((a + 1.0) + (a - 1.0) * cs - s),
                 (a + 1.0) - (a - 1.0) * cs + s,
                 2.0 * ((a - 1.0) - (a + 1.0) * cs),
                 (a + 1.0) - (a - 1.0) * cs - s)
            }
            BandKind::LowPass => (
                (1.0 - cs) / 2.0, 1.0 - cs, (1.0 - cs) / 2.0,
                1.0 + alpha, -2.0 * cs, 1.0 - alpha,
            ),
            BandKind::HighPass => (
                (1.0 + cs) / 2.0, -(1.0 + cs), (1.0 + cs) / 2.0,
                1.0 + alpha, -2.0 * cs, 1.0 - alpha,
            ),
        };
        Biquad { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0,
                 x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }

    /// The RBJ second-order ALLPASS: unity magnitude, a phase turn about
    /// `freq`. At Q = 1/sqrt(2) it is EXACTLY an LR4 crossover's low + high
    /// halves summed (analog: (s^4 + 1)/(s^2 + sqrt2 s + 1)^2 = (s^2 - sqrt2 s
    /// + 1)/(s^2 + sqrt2 s + 1); the bilinear transform keeps it) -- one
    /// section where the multiband ran four.
    pub fn allpass(rate: f32, freq: f32, q: f32) -> Biquad {
        let f = freq.clamp(1.0, rate * 0.49);
        let w = std::f32::consts::TAU * f / rate;
        let (sn, cs) = (w.sin(), w.cos());
        let alpha = sn / (2.0 * q.max(0.01));
        let a0 = 1.0 + alpha;
        Biquad { b0: (1.0 - alpha) / a0, b1: -2.0 * cs / a0, b2: 1.0, a1: -2.0 * cs / a0, a2: (1.0 - alpha) / a0,
                 x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
              - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1; self.x1 = x;
        self.y2 = self.y1; self.y1 = y;
        y
    }

    /// Magnitude response at `freq`, for drawing the curve. Computed from the
    /// coefficients rather than measured, so the display cannot disagree with
    /// what the filter actually does.
    pub fn magnitude_db(&self, rate: f32, freq: f32) -> f32 {
        let w = std::f32::consts::TAU * freq / rate;
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        let num = (nr * nr + ni * ni).sqrt();
        let den = (dr * dr + di * di).sqrt().max(1e-12);
        20.0 * (num / den).max(1e-12).log10()
    }

    pub fn reset(&mut self) { self.x1 = 0.0; self.x2 = 0.0; self.y1 = 0.0; self.y2 = 0.0; }
    /// Advance the memory as an identity filter would (y = x), without the
    /// arithmetic: what a flat band does instead of `process`.
    #[inline]
    pub fn pass(&mut self, x: f32) { self.x2 = self.x1; self.x1 = x; self.y2 = self.y1; self.y1 = x; }
    /// These coefficients with `old`'s memory — a redesign that does not
    /// restart the filter.
    pub fn with_state_of(mut self, old: &Biquad) -> Biquad {
        self.x1 = old.x1; self.x2 = old.x2; self.y1 = old.y1; self.y2 = old.y2;
        self
    }
}

/// What a band was designed from, kept so it can be redesigned live.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandDesign { pub kind: BandKind, pub freq: f32, pub q: f32, pub gain_db: f32 }

impl BandDesign {
    /// A band that does nothing: a bell or shelf at 0 dB.
    pub fn is_flat(&self) -> bool {
        self.gain_db == 0.0 && matches!(self.kind, BandKind::Peaking | BandKind::LowShelf | BandKind::HighShelf)
    }
}

/// A stack of bands. `designs` and `rate` are filled by `instantiate` so
/// a band can be redesigned in place (`set_band`); an `Eq` built by hand
/// from biquads has neither and cannot be ridden, which is fine — the
/// automation path only ever sees instantiated ones.
#[derive(Debug, Clone, Default)]
pub struct Eq {
    pub bands: Vec<Biquad>,
    pub designs: Vec<BandDesign>,
    pub rate: f32,
}

impl Eq {
    /// A FLAT band -- peaking or shelf at 0 dB -- is the identity (at A = 1
    /// the RBJ numerator and denominator are the same floats; computing it
    /// only adds the rounding of its five-term sum, ~2e-6 at 0.5), so it is not
    /// computed: it only records its input as its history, which is
    /// what it would have held (y = x), so a ride that later gives it gain
    /// picks up seamlessly. Every imported FL EQ starts flat (its settings
    /// are not decoded): 49 of them were a third of Balance's render.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let mut acc = x;
        for (i, b) in self.bands.iter_mut().enumerate() {
            if self.designs.get(i).is_some_and(BandDesign::is_flat) { b.pass(acc); } else { acc = b.process(acc); }
        }
        acc
    }
    /// Redesign band `i` around one changed value, keeping the filter's
    /// memory: a transposed-direct-form biquad takes new coefficients
    /// mid-stream without a click, which is what makes a cutoff sweep
    /// a ride and not a series of restarts.
    pub fn set_band(&mut self, i: usize, freq: Option<f32>, q: Option<f32>, gain_db: Option<f32>) -> bool {
        let (Some(d), Some(b)) = (self.designs.get_mut(i), self.bands.get_mut(i)) else { return false };
        if let Some(f) = freq { d.freq = f; }
        if let Some(v) = q { d.q = v; }
        if let Some(g) = gain_db { d.gain_db = g; }
        let fresh = Biquad::design(d.kind, self.rate, d.freq, d.q, d.gain_db);
        *b = fresh.with_state_of(b);
        true
    }
    pub fn magnitude_db(&self, rate: f32, freq: f32) -> f32 {
        self.bands.iter().map(|b| b.magnitude_db(rate, freq)).sum()
    }
}

// ── dynamics ────────────────────────────────────────────────────────────────

/// Feed-forward compressor with a soft knee.
///
/// Detection is in dB on the peak of the signal, with separate attack and
/// release time constants — the shape every hardware unit has and every listener
/// expects. The knee is the part people notice: a hard knee on a vocal sounds
/// like the compressor "grabbing", so the default is soft.
#[derive(Debug, Clone, Copy)]
pub struct Compressor {
    pub threshold_db: f32,
    pub ratio: f32,
    pub knee_db: f32,
    pub makeup_db: f32,
    attack_c: f32,
    release_c: f32,
    env_db: f32,
}

fn time_const(ms: f32, rate: f32) -> f32 {
    if ms <= 0.0 { 0.0 } else { (-1.0 / (ms * 0.001 * rate)).exp() }
}

fn to_db(x: f32) -> f32 { 20.0 * x.abs().max(1e-9).log10() }
fn from_db(db: f32) -> f32 { 10f32.powf(db / 20.0) }
/// dB → linear, for sibling modules.
pub fn from_db_pub(db: f32) -> f32 { from_db(db) }

impl Compressor {
    pub fn new(rate: f32, threshold_db: f32, ratio: f32, attack_ms: f32, release_ms: f32) -> Compressor {
        Compressor {
            threshold_db, ratio: ratio.max(1.0), knee_db: 6.0, makeup_db: 0.0,
            attack_c: time_const(attack_ms, rate),
            release_c: time_const(release_ms, rate),
            env_db: -120.0,
        }
    }

    /// Static curve: input level in dB → output level in dB.
    pub fn curve_db(&self, x_db: f32) -> f32 {
        let over = x_db - self.threshold_db;
        let k = self.knee_db.max(0.0);
        if k > 0.0 && over.abs() <= k / 2.0 {
            // quadratic soft knee: continuous in value AND slope at both ends,
            // which is what keeps the transition inaudible
            let t = over + k / 2.0;
            x_db + (1.0 / self.ratio - 1.0) * t * t / (2.0 * k)
        } else if over > 0.0 {
            self.threshold_db + over / self.ratio
        } else {
            x_db
        }
    }

    /// Gain reduction currently applied, in dB (negative). For the meter.
    pub fn reduction_db(&self) -> f32 { (self.curve_db(self.env_db) - self.env_db).min(0.0) }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let in_db = to_db(x);
        // envelope follows fast up, slow down — a compressor that released as
        // fast as it attacked would pump on every syllable
        let c = if in_db > self.env_db { self.attack_c } else { self.release_c };
        self.env_db = in_db + c * (self.env_db - in_db);
        let gain_db = self.curve_db(self.env_db) - self.env_db;
        x * from_db(gain_db + self.makeup_db)
    }
}

/// Brickwall limiter: a compressor at infinite ratio with **lookahead**, so the
/// gain is already down when the transient arrives.
///
/// Without lookahead a limiter must either overshoot (letting a sample through
/// above the ceiling, which is the one thing a limiter must never do) or use an
/// attack so fast it distorts. Delaying the signal by the lookahead window and
/// computing gain from the future is what makes it truly brickwall.
#[derive(Debug, Clone)]
pub struct Limiter {
    pub ceiling_db: f32,
    release_c: f32,
    gain_db: f32,
    delay: Vec<f32>,
    head: usize,
    /// The lookahead window's gain needs as a MONOTONIC DEQUE (index, need),
    /// needs increasing from the front: the front is the window's minimum.
    /// Was an O(window) scan per sample -- on Balance, twelve limiters were
    /// 76% of the whole render (examples/profile_mix). Same answer, exactly.
    win: std::collections::VecDeque<(u64, f32)>,
    count: u64,
}

impl Limiter {
    pub fn new(rate: f32, ceiling_db: f32, lookahead_ms: f32, release_ms: f32) -> Limiter {
        let n = ((lookahead_ms * 0.001 * rate) as usize).max(1);
        Limiter {
            ceiling_db,
            release_c: time_const(release_ms, rate),
            gain_db: 0.0,
            delay: vec![0.0; n],
            head: 0,
            win: std::collections::VecDeque::with_capacity(n + 1),
            count: 0,
        }
    }

    pub fn lookahead_samples(&self) -> usize { self.delay.len() }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        // The gain applied to the sample leaving the delay line must account for
        // every sample still INSIDE it — including the one just pushed. The
        // first cut computed gain from the incoming sample and applied it to the
        // outgoing one, so a transient dropped the gain while a quiet old sample
        // was passing, and the gain had released again by the time the loud one
        // emerged. The test caught it leaking 1.03 against a 0.91 ceiling.
        let need = {
            let over = to_db(x) - self.ceiling_db;
            if over > 0.0 { -over } else { 0.0 }
        };
        let delayed = self.delay[self.head];
        self.delay[self.head] = x;
        self.head = (self.head + 1) % self.delay.len();

        // Required gain over the whole lookahead window (the last n needs,
        // this one included): the deque's front, amortised O(1). Slots not
        // yet filled count as 0 dB, as the zeroed window did before.
        while self.win.back().is_some_and(|b| b.1 >= need) { self.win.pop_back(); }
        self.win.push_back((self.count, need));
        let n = self.delay.len() as u64;
        while self.win.front().is_some_and(|f| f.0 + n <= self.count) { self.win.pop_front(); }
        self.count += 1;
        let target = self.win.front().map_or(0.0, |f| f.1).min(0.0);
        self.gain_db = if target < self.gain_db {
            target // attack is instant: that is what "brickwall" means
        } else {
            target + self.release_c * (self.gain_db - target)
        };
        delayed * from_db(self.gain_db)
    }
}

// ── video: transform ────────────────────────────────────────────────────────

/// Position, scale, rotation and crop, as kdenlive's transform effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Normalised 0..1 of the frame, so a transform survives a resolution change
    /// — the reason to store fractions instead of pixels.
    pub x: f32,
    pub y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    /// Degrees, clockwise.
    pub rotation: f32,
    /// Rotation/scale origin, normalised within the source.
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub opacity: f32,
    /// Crop as fractions taken off each edge.
    pub crop: (f32, f32, f32, f32),
}

impl Default for Transform {
    fn default() -> Self {
        Transform { x: 0.0, y: 0.0, scale_x: 1.0, scale_y: 1.0, rotation: 0.0,
                    anchor_x: 0.5, anchor_y: 0.5, opacity: 1.0, crop: (0.0, 0.0, 0.0, 0.0) }
    }
}

impl Transform {
    pub fn is_identity(&self) -> bool { *self == Transform::default() }

    /// Map a point in source space (0..1) to frame space.
    pub fn apply(&self, sx: f32, sy: f32) -> (f32, f32) {
        let (dx, dy) = (sx - self.anchor_x, sy - self.anchor_y);
        let (sxx, syy) = (dx * self.scale_x, dy * self.scale_y);
        let r = self.rotation.to_radians();
        let (c, s) = (r.cos(), r.sin());
        (self.anchor_x + sxx * c - syy * s + self.x,
         self.anchor_y + sxx * s + syy * c + self.y)
    }

    /// The inverse map, which is what a renderer actually needs: for each
    /// output pixel, where in the source do I sample?
    pub fn invert(&self, fx: f32, fy: f32) -> (f32, f32) {
        let (dx, dy) = (fx - self.x - self.anchor_x, fy - self.y - self.anchor_y);
        let r = (-self.rotation).to_radians();
        let (c, s) = (r.cos(), r.sin());
        let (rx, ry) = (dx * c - dy * s, dx * s + dy * c);
        (self.anchor_x + rx / self.scale_x.abs().max(1e-6),
         self.anchor_y + ry / self.scale_y.abs().max(1e-6))
    }

    /// Is a source point inside the crop?
    pub fn cropped_in(&self, sx: f32, sy: f32) -> bool {
        let (l, t, r, b) = self.crop;
        sx >= l && sx <= 1.0 - r && sy >= t && sy <= 1.0 - b
    }
}

// ── video: colour ───────────────────────────────────────────────────────────

/// Lift / gamma / gain plus saturation — the grade every colourist reaches for
/// first, and the one both source editors expose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    /// Added to the shadows (per channel).
    pub lift: [f32; 3],
    /// Midtone curve, per channel; 1.0 is neutral.
    pub gamma: [f32; 3],
    /// Multiplies the highlights.
    pub gain: [f32; 3],
    /// 0 = greyscale, 1 = untouched, >1 = more.
    pub saturation: f32,
}

impl Default for Color {
    fn default() -> Self {
        Color { lift: [0.0; 3], gamma: [1.0; 3], gain: [1.0; 3], saturation: 1.0 }
    }
}

impl Color {
    pub fn is_identity(&self) -> bool { *self == Color::default() }

    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut out = [0.0f32; 3];
        for i in 0..3 {
            // lift first, then gamma, then gain: the order every grading tool
            // uses, and changing it silently changes the look of a project
            let v = (rgb[i] + self.lift[i]).max(0.0);
            let v = v.powf(1.0 / self.gamma[i].max(1e-3));
            out[i] = v * self.gain[i];
        }
        if (self.saturation - 1.0).abs() > 1e-6 {
            // Rec.709 luma — using a flat average instead is the classic error
            // that makes skin tones go grey-green
            let y = 0.2126 * out[0] + 0.7152 * out[1] + 0.0722 * out[2];
            for c in out.iter_mut() { *c = y + (*c - y) * self.saturation; }
        }
        [out[0].clamp(0.0, 1.0), out[1].clamp(0.0, 1.0), out[2].clamp(0.0, 1.0)]
    }
}

// ── retiming ────────────────────────────────────────────────────────────────

/// Speed change over a clip: constant, or a ramp between two speeds.
///
/// Retiming is where naive implementations break, because the useful question
/// is not "how fast" but "given a timeline instant, WHICH source instant do I
/// read". For a ramp that is the integral of the speed curve, not the speed —
/// getting it wrong makes audio and picture drift apart across the ramp even
/// though both are individually "correct".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Retime {
    pub from_speed: f32,
    pub to_speed: f32,
    /// Keep pitch when speed changes (a resampler decision, recorded here).
    pub preserve_pitch: bool,
    pub reverse: bool,
}

impl Default for Retime {
    fn default() -> Self {
        Retime { from_speed: 1.0, to_speed: 1.0, preserve_pitch: true, reverse: false }
    }
}

impl Retime {
    pub fn is_identity(&self) -> bool { *self == Retime::default() }

    /// Source offset to read at `into_clip` along a clip of length `clip_len`.
    pub fn source_at(&self, into_clip: Ticks, clip_len: Ticks) -> Ticks {
        if clip_len.0 <= 0 { return Ticks::ZERO; }
        let t = (into_clip.0 as f64 / clip_len.0 as f64).clamp(0.0, 1.0);
        let a = self.from_speed.max(0.01) as f64;
        let b = self.to_speed.max(0.01) as f64;
        // integral of a linear speed ramp from a to b over [0, t]
        let consumed = (a * t + (b - a) * t * t / 2.0) * clip_len.0 as f64;
        let off = if self.reverse {
            let total = (a + b) / 2.0 * clip_len.0 as f64;
            total - consumed
        } else {
            consumed
        };
        Ticks(off.max(0.0) as i64)
    }

    /// How much source a clip of `clip_len` consumes — what the editor needs to
    /// know before it can trim.
    pub fn source_needed(&self, clip_len: Ticks) -> Ticks {
        let a = self.from_speed.max(0.01) as f64;
        let b = self.to_speed.max(0.01) as f64;
        Ticks(((a + b) / 2.0 * clip_len.0 as f64) as i64)
    }
}

// ── the registry ────────────────────────────────────────────────────────────

/// A running effect instance.
pub enum Instance {
    Eq(Eq),
    Compressor(Compressor),
    Limiter(Limiter),
    Multiband(Box<multiband::Multiband>),
    Saturator(saturation::Saturator),
    /// Plain gain, in dB — a trim stage positioned WHEREVER it sits in the
    /// chain, unlike `Bus.gain`/`Clip.gain`, which are each a single scalar
    /// always applied at one fixed point (the fader, after the whole insert
    /// chain — see `mixdown`'s bus-fold docs). Without this, "boost 6dB
    /// going into the limiter" (the clip-into-limiter mastering trick this
    /// codebase's own README describes) had no way to land anywhere in
    /// `Bus.chain` at all — mirrors `chain::Module::Gain`, the same concept
    /// `Chain`'s own strip already has.
    Gain(f32, f32),
    Transform(Transform),
    Color(Color),
    /// A feedback comb whose loop is low-passed — the tuned-comb voice, not a
    /// room. `mix` is how much of the delayed signal joins the dry, and it is
    /// what decides whether this reads as timbre or as an echo.
    Comb(Box<voice::Delay>, f32),
    /// Weight under a voice. `mix` is deliberately low by default: above about
    /// -12 dB it stops being weight and becomes a second voice.
    Sub(Box<voice::SubOctave>, f32),
    Shift(Box<voice::Shifter>),
    Crush(voice::Crush),
    Gate(voice::Gate),
    Retime(Retime),
    /// The clip's blend mode and opacity for the compositor — a pass-through
    /// for audio, a layer instruction for video.
    Blend(compositor::BlendMode, f32),
    /// The trap pair — see `playrate`'s module docs for why they carry
    /// their own per-channel buffers inside a per-sample interface.
    /// Live, note-locking pitch correction. Unlike `autotune`'s offline
    /// pass — which is deliberately NOT a kind here, because it needs the
    /// whole take — this one is streaming with fixed latency, so it is a
    /// legitimate insert.
    RealTune(Box<realtune::RealtimeTune>),
    /// Saturation whose drive follows register and sibilance.
    AdaptiveSat(Box<adaptive_sat::AdaptiveSat>),
    TapeStop(playrate::TapeStop),
    Halftime(playrate::Halftime),
    /// The send pair — see `space`'s module docs. Both carry per-channel
    /// state behind the per-sample interface, like the playrate pair.
    Reverb(Box<space::Reverb>),
    Delay(Box<space::Delay>),
}

impl Instance {
    /// Run one audio sample through this instance, for the variants that are
    /// audio effects at all. `Transform`/`Color`/`Retime` are video-only —
    /// passed through unchanged rather than refused, so a caller iterating a
    /// mixed audio/video effect chain doesn't need to filter kinds itself
    /// before calling this; it can just call it and trust silence for the
    /// kinds that don't apply.
    pub fn process_audio(&mut self, x: f32) -> f32 {
        match self {
            Instance::Eq(e) => e.process(x),
            Instance::Compressor(c) => c.process(x),
            Instance::Limiter(l) => l.process(x),
            Instance::Multiband(m) => m.process(x),
            Instance::Saturator(s) => s.process(x),
            Instance::Gain(db, level) => x * from_db_pub(*db) * *level,
            Instance::Blend(..) => x, // a layer instruction: silence for audio, per the doc above
            Instance::RealTune(t) => t.process(x),
            Instance::AdaptiveSat(a) => a.process(x),
            // Comb and Sub are PARALLEL: they add to the dry rather than
            // replacing it. A serial comb loses the words, and a serial sub is
            // an octave-down voice rather than weight under one.
            Instance::Comb(d, mix) => x + d.process(x) * *mix,
            Instance::Sub(s, mix) => x + s.process(x) * *mix,
            Instance::Shift(sh) => sh.process(x),
            Instance::Crush(c) => c.process(x),
            Instance::Gate(g) => g.process(x),
            Instance::TapeStop(t) => t.process(x),
            Instance::Halftime(h) => h.process(x),
            Instance::Reverb(r) => r.process(x),
            Instance::Delay(d) => d.process(x),
            Instance::Transform(_) | Instance::Color(_) | Instance::Retime(_) => x,
        }
    }

    /// Whether `set_param` would accept `key` on an instance of `kind` —
    /// the static answer, for a UI deciding whether to offer "automate"
    /// on a control, and the table the live-set test pins `set_param`
    /// against. The digit-suffixed EQ keys are live for any band index.
    pub fn is_live_param(kind: &str, key: &str) -> bool {
        let stem = key.trim_end_matches(|c: char| c.is_ascii_digit());
        let live: &[&str] = match kind {
            "gain" => &["db", "level"],
            "eq" => return key != stem && matches!(stem, "freq" | "q" | "gain"),
            "compressor" => &["threshold", "ratio", "knee", "makeup"],
            "limiter" => &["ceiling"],
            "saturation" | "adaptivesat" => &["drive", "bias", "mix"],
            "comb" | "sub" => &["mix"],
            "crush" => &["bits"],
            "gate" => &["threshold"],
            "reverb" => &["mix", "width", "size", "damp"],
            "delay" => &["mix", "feedback", "damp"],
            "blend" => &["opacity"],
            _ => &[],
        };
        live.contains(&key)
    }

    /// Change one parameter on a running instance without losing its
    /// state — the automation path. Returns whether the key is one this
    /// kind can move live; the caller ignores a `false` (a ride on a
    /// parameter the effect cannot change mid-stream does nothing, rather
    /// than restarting the effect at every point). What can move: the
    /// things read per sample (gains, thresholds, mixes, feedback,
    /// damping, a filter's design with its memory kept); what cannot: a
    /// delay's time, a limiter's lookahead, a grain length, a channel
    /// count — buffer sizes, not values. `rate` is needed only by the
    /// delay's damping; every other setter is rate-free.
    /// Tell an instance the song position (in frames) of the first sample it
    /// is about to receive. Only effects whose decisions run on a grid of
    /// their own need it (the autotune's analysis hops); for the rest, a no-op.
    pub fn align(&mut self, at: i64) {
        if let Instance::RealTune(t) = self { t.align(at); }
    }

    pub fn set_param(&mut self, key: &str, v: f32, rate: f32) -> bool {
        match self {
            Instance::Gain(db, level) => match key {
                "db" => { *db = v; true }
                "level" => { *level = v.max(0.0); true }
                _ => false,
            },
            Instance::Eq(eq) => {
                let (which, i) = match key.find(|c: char| c.is_ascii_digit()) {
                    Some(n) => (&key[..n], key[n..].parse::<usize>().ok()),
                    None => (key, None),
                };
                let Some(i) = i else { return false };
                match which {
                    "freq" => eq.set_band(i, Some(v), None, None),
                    "q" => eq.set_band(i, None, Some(v), None),
                    "gain" => eq.set_band(i, None, None, Some(v)),
                    _ => false,
                }
            }
            Instance::Compressor(c) => match key {
                "threshold" => { c.threshold_db = v; true }
                "ratio" => { c.ratio = v.max(1.0); true }
                "knee" => { c.knee_db = v.max(0.0); true }
                "makeup" => { c.makeup_db = v; true }
                _ => false,
            },
            Instance::Limiter(l) => { if key == "ceiling" { l.ceiling_db = v; true } else { false } }
            Instance::Saturator(s) => match key {
                "drive" => { s.drive = v.clamp(0.0, 1.0); true }
                "bias" => { s.bias = v; true }
                "mix" => { s.mix = v.clamp(0.0, 1.0); true }
                _ => false,
            },
            Instance::AdaptiveSat(a) => match key {
                "drive" => { a.base_drive = v.clamp(0.0, 1.0); true }
                "bias" => { a.sat.bias = v; true }
                "mix" => { a.sat.mix = v.clamp(0.0, 1.0); true }
                _ => false,
            },
            Instance::Comb(_, mix) | Instance::Sub(_, mix) => {
                if key == "mix" { *mix = v; true } else { false }
            }
            Instance::Crush(c) => { if key == "bits" { c.set_bits(v.round().clamp(1.0, 16.0) as u8); true } else { false } }
            Instance::Gate(g) => { if key == "threshold" { g.set_threshold_db(v); true } else { false } }
            Instance::Reverb(r) => match key {
                "mix" => { r.mix = v.clamp(0.0, 1.0); true }
                "width" => { r.width = v.clamp(0.0, 1.0); true }
                "size" => { r.set_size(v); true }
                "damp" => { r.set_damp(v); true }
                _ => false,
            },
            Instance::Delay(d) => match key {
                "mix" => { d.mix = v.clamp(0.0, 1.0); true }
                "feedback" => { d.set_feedback(v); true }
                "damp" => { d.set_damp_hz(rate, v); true }
                _ => false,
            },
            Instance::Blend(_, opacity) => { if key == "opacity" { *opacity = v.clamp(0.0, 1.0); true } else { false } }
            Instance::Multiband(_) | Instance::Shift(_) | Instance::RealTune(_)
            | Instance::TapeStop(_) | Instance::Halftime(_)
            | Instance::Transform(_) | Instance::Color(_) | Instance::Retime(_) => false,
        }
    }
}

/// Build a running effect from a project's stored description, or `None` when
/// this build does not know the kind — which is how a project made with a
/// plugin you lack still opens.
///
/// `"imager"` is deliberately NOT one of the kinds this builds, even though
/// `imager::Imager` is fully implemented and tested: `Instance::process_audio`
/// takes one sample and returns one, but an imager needs L and R
/// SIMULTANEOUSLY (mid/side processing is inherently a relationship between
/// the two channels, not a per-channel function) — the same reason
/// `studio_fx::chain::Module::is_stereo()` exists to flag imager specially in
/// the `Chain` strip. Fitting it into this mono per-sample interface would
/// mean feeding it L and R sequentially through one instance, which is not
/// stereo imaging at all, just a wrong answer that happens to compile. A bus
/// or clip effect chain wanting stereo imaging needs a genuinely stereo-aware
/// processing path here, which is real, separate work — not a missing match
/// arm to fill in.
pub fn instantiate(effect: &studio_core::Effect, rate: f32) -> Option<Instance> {
    let q = &effect.params;
    Some(match effect.kind.as_str() {
        "eq" => {
            let n = p(q, "bands", 1.0).clamp(1.0, 8.0) as usize;
            let mut eq = Eq { rate, ..Eq::default() };
            for i in 0..n {
                let d = BandDesign {
                    kind: match p(q, &format!("kind{i}"), 0.0) as i32 {
                        1 => BandKind::LowShelf, 2 => BandKind::HighShelf,
                        3 => BandKind::LowPass, 4 => BandKind::HighPass,
                        _ => BandKind::Peaking,
                    },
                    freq: p(q, &format!("freq{i}"), 1000.0),
                    q: p(q, &format!("q{i}"), 0.707),
                    gain_db: p(q, &format!("gain{i}"), 0.0),
                };
                eq.bands.push(Biquad::design(d.kind, rate, d.freq, d.q, d.gain_db));
                eq.designs.push(d);
            }
            Instance::Eq(eq)
        }
        "compressor" => {
            let mut c = Compressor::new(rate, p(q, "threshold", -18.0), p(q, "ratio", 4.0),
                                        p(q, "attack", 10.0), p(q, "release", 120.0));
            c.knee_db = p(q, "knee", 6.0);
            c.makeup_db = p(q, "makeup", 0.0);
            Instance::Compressor(c)
        }
        "comb" => Instance::Comb(
            Box::new(voice::Delay::new(
                rate,
                240.0,
                p(q, "delay", 70.0),
                p(q, "feedback", 0.6),
                p(q, "damp", 1500.0),
            )),
            p(q, "mix", 0.35),
        ),
        "sub" => Instance::Sub(
            Box::new(voice::SubOctave::new(rate)),
            p(q, "mix", 0.25),
        ),
        "shift" => Instance::Shift(Box::new(voice::Shifter::new(
            rate,
            p(q, "semitones", -3.0),
            p(q, "grain", 60.0),
        ))),
        "crush" => Instance::Crush(voice::Crush::new(
            rate,
            p(q, "bits", 16.0).clamp(1.0, 16.0) as u8,
            p(q, "hold", rate),
        )),
        "gate" => Instance::Gate(voice::Gate::new(
            rate,
            p(q, "threshold", -40.0),
            p(q, "hold", 80.0),
            p(q, "release", 30.0),
        )),
        "limiter" => Instance::Limiter(Limiter::new(rate, p(q, "ceiling", -0.3),
                                                    p(q, "lookahead", 5.0), p(q, "release", 50.0))),
        "multiband" => {
            // Crossover points, not output-band count — an N-crossover
            // description yields N+1 bands, the same convention
            // `multiband::Multiband::new` itself uses.
            let n = p(q, "bands", 2.0).clamp(1.0, 6.0) as usize;
            let freqs: Vec<f32> = (0..n)
                .map(|i| p(q, &format!("freq{i}"), 200.0 * 2f32.powi(i as i32 + 1)))
                .collect();
            let mut mb = multiband::Multiband::new(rate, &freqs);
            // Per-band gain and bypass are exposed now; per-band compressor
            // threshold/ratio/attack/release are left at Multiband::new's own
            // defaults — a reasonable first cut of control, not a cut corner:
            // the essential structural parameter (crossover points, which
            // bands exist at all) is here, finer per-band dynamics tuning is
            // a natural follow-on rather than something this pass needed to
            // finish to be useful.
            for (i, band) in mb.bands.iter_mut().enumerate() {
                band.gain_db = p(q, &format!("band{i}_gain"), 0.0);
                band.bypass = p(q, &format!("band{i}_bypass"), 1.0) > 0.5;
            }
            Instance::Multiband(Box::new(mb))
        }
        "saturation" => {
            let profile = match p(q, "profile", 0.0) as i32 {
                1 => saturation::Profile::Tape,
                2 => saturation::Profile::Console,
                3 => saturation::Profile::Diode,
                _ => saturation::Profile::Tube,
            };
            let mut sat = saturation::Saturator::new(profile);
            sat.drive = p(q, "drive", 0.0).clamp(0.0, 1.0);
            sat.bias = p(q, "bias", 0.0);
            sat.mix = p(q, "mix", 1.0).clamp(0.0, 1.0);
            sat.auto_gain = p(q, "auto_gain", 1.0) > 0.5;
            Instance::Saturator(sat)
        }
        "gain" => Instance::Gain(p(q, "db", 0.0), p(q, "level", 1.0).max(0.0)),
        "transform" => Instance::Transform(Transform {
            x: p(q, "x", 0.0), y: p(q, "y", 0.0),
            scale_x: p(q, "scale_x", 1.0), scale_y: p(q, "scale_y", 1.0),
            rotation: p(q, "rotation", 0.0),
            anchor_x: p(q, "anchor_x", 0.5), anchor_y: p(q, "anchor_y", 0.5),
            opacity: p(q, "opacity", 1.0),
            crop: (p(q, "crop_l", 0.0), p(q, "crop_t", 0.0), p(q, "crop_r", 0.0), p(q, "crop_b", 0.0)),
        }),
        // The playrate pair. `channels` defaults to 2 because that is what
        // mixdown feeds (L then R into one instance); a mono caller says so.
        "adaptivesat" => {
            let profile = match p(q, "profile", 0.0) as i32 {
                1 => saturation::Profile::Tape,
                2 => saturation::Profile::Console,
                3 => saturation::Profile::Diode,
                _ => saturation::Profile::Tube,
            };
            let mut a = adaptive_sat::AdaptiveSat::new(
                profile, rate, p(q, "channels", 2.0).clamp(1.0, 8.0) as usize);
            a.base_drive = p(q, "drive", 0.35).clamp(0.0, 1.0);
            a.note_amount = p(q, "note_amount", 0.5).clamp(0.0, 1.0);
            a.sibilance_amount = p(q, "sib_amount", 0.5).clamp(0.0, 1.0);
            a.link = if p(q, "tied", 0.0) > 0.5 {
                adaptive_sat::Link::Tied
            } else {
                adaptive_sat::Link::Independent
            };
            a.sat.bias = p(q, "bias", 0.0);
            a.sat.mix = p(q, "mix", 1.0).clamp(0.0, 1.0);
            Instance::AdaptiveSat(Box::new(a))
        }
        "autotune" => {
            // Notes are given as a packed list: note_count, then note0..N as
            // MIDI numbers. Params are a flat f32 map (the model's choice),
            // so a variable-length set has to be encoded somehow; counting
            // beats a sentinel, and an absent count means key-lock mode.
            let count = p(q, "notes", 0.0).clamp(0.0, 24.0) as usize;
            let target = if count > 0 {
                realtune::Target::Notes(
                    (0..count).map(|i| p(q, &format!("note{i}"), 60.0)
                        .clamp(0.0, 127.0) as u8).collect())
            } else {
                realtune::Target::Scale(retune::Scale {
                    root: p(q, "root", 0.0).clamp(0.0, 11.0) as u8,
                    mode: match p(q, "mode", 0.0) as i32 {
                        1 => retune::Mode::HarmonicMinor,
                        2 => retune::Mode::Chromatic,
                        _ => retune::Mode::Minor,
                    },
                })
            };
            let mut t = realtune::RealtimeTune::new(
                target, rate, p(q, "channels", 2.0).clamp(1.0, 8.0) as usize);
            t.retune_speed_ms = p(q, "speed", 30.0).max(0.0);
            t.voiced_threshold = p(q, "voiced", 0.15).clamp(0.0, 1.0);
            t.slide = p(q, "slide", 0.0) >= 0.5;
            t.amount = p(q, "amount", 1.0).clamp(0.0, 1.0);
            t.tolerance_cents = p(q, "tolerance", 0.0).clamp(0.0, 100.0);
            Instance::RealTune(Box::new(t))
        }
        "tapestop" => Instance::TapeStop(playrate::TapeStop::new(
            rate, p(q, "start", 0.0), p(q, "length", 1.0),
            p(q, "channels", 2.0).max(1.0) as usize)),
        "halftime" => Instance::Halftime(playrate::Halftime::new(
            rate, p(q, "window", 0.5),
            p(q, "channels", 2.0).max(1.0) as usize)),
        // The send pair. `mix` defaults to a plausible INSERT amount; a
        // send bus sets it to 1.0 (all wet), which is what the mixer's
        // "+ fx" on a bus does.
        "reverb" => Instance::Reverb(Box::new(space::Reverb::new(
            rate, p(q, "size", 0.5), p(q, "damp", 0.4), p(q, "predelay", 10.0),
            p(q, "mix", 0.3), p(q, "width", 1.0),
            p(q, "channels", 2.0).max(1.0) as usize))),
        "delay" => Instance::Delay(Box::new(space::Delay::new(
            rate, p(q, "time", 321.4), p(q, "feedback", 0.4), p(q, "damp", 5000.0),
            p(q, "mix", 0.3), p(q, "pingpong", 0.0) > 0.5,
            p(q, "channels", 2.0).max(1.0) as usize))),
        "color" => Instance::Color(Color {
            lift: [p(q, "lift_r", 0.0), p(q, "lift_g", 0.0), p(q, "lift_b", 0.0)],
            gamma: [p(q, "gamma_r", 1.0), p(q, "gamma_g", 1.0), p(q, "gamma_b", 1.0)],
            gain: [p(q, "gain_r", 1.0), p(q, "gain_g", 1.0), p(q, "gain_b", 1.0)],
            saturation: p(q, "saturation", 1.0),
        }),
        "blend" => { let (m, o) = compositor::blend_of(effect); Instance::Blend(m, o) }
        "retime" => Instance::Retime(Retime {
            from_speed: p(q, "from_speed", 1.0), to_speed: p(q, "to_speed", 1.0),
            preserve_pitch: p(q, "preserve_pitch", 1.0) > 0.5,
            reverse: p(q, "reverse", 0.0) > 0.5,
        }),
        _ => return None,
    })
}

#[cfg(test)]
mod limiter_deque_tests {
    use super::*;

    /// The limiter as it was: an O(window) scan per sample.
    struct Naive { ceiling_db: f32, release_c: f32, gain_db: f32, delay: Vec<f32>, need: Vec<f32>, head: usize }
    impl Naive {
        fn process(&mut self, x: f32) -> f32 {
            let need = { let over = to_db(x) - self.ceiling_db; if over > 0.0 { -over } else { 0.0 } };
            let delayed = self.delay[self.head];
            self.delay[self.head] = x;
            self.need[self.head] = need;
            self.head = (self.head + 1) % self.delay.len();
            let target = self.need.iter().copied().fold(0.0f32, f32::min);
            self.gain_db = if target < self.gain_db { target } else { target + self.release_c * (self.gain_db - target) };
            delayed * from_db(self.gain_db)
        }
    }

    #[test]
    fn the_deque_limiter_is_bit_identical_to_the_scan_it_replaced() {
        let rate = 48_000.0;
        let mut fast = Limiter::new(rate, -0.3, 5.0, 60.0);
        let n = fast.lookahead_samples();
        let mut slow = Naive { ceiling_db: -0.3, release_c: time_const(60.0, rate), gain_db: 0.0, delay: vec![0.0; n], need: vec![0.0; n], head: 0 };
        let mut x = 99u64;
        let mut rnd = move || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (x % 20_000) as f32 / 10_000.0 - 1.0 };
        for i in 0..200_000 {
            // quiet stretches, loud bursts, lone spikes: every path of the deque
            let amp = if (i / 3000) % 3 == 0 { 0.2 } else if i % 997 == 0 { 4.0 } else { 1.6 };
            let s = rnd() * amp;
            let (a, b) = (fast.process(s), slow.process(s));
            assert_eq!(a.to_bits(), b.to_bits(), "sample {i}: {a} vs {b}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    #[test]
    fn a_flat_eq_band_leaves_the_signal_alone() {
        // 0 dB of peaking must be a true bypass, not "nearly" — a chain of
        // eight neutral bands has to sum to silence-of-effect.
        let mut b = Biquad::design(BandKind::Peaking, RATE, 1000.0, 0.707, 0.0);
        for i in 0..500 {
            let x = (i as f32 * 0.01).sin();
            assert!((b.process(x) - x).abs() < 1e-4, "sample {i} was altered");
        }
        assert!(b.magnitude_db(RATE, 1000.0).abs() < 1e-3);
    }

    #[test]
    fn a_boost_shows_up_at_its_centre_and_not_far_away() {
        let b = Biquad::design(BandKind::Peaking, RATE, 1000.0, 1.0, 9.0);
        assert!((b.magnitude_db(RATE, 1000.0) - 9.0).abs() < 0.2, "the boost is where it was asked for");
        assert!(b.magnitude_db(RATE, 60.0).abs() < 1.0, "and not down in the bass");
        assert!(b.magnitude_db(RATE, 16_000.0).abs() < 1.0, "nor up in the air");
        // a cut is the mirror of a boost
        let c = Biquad::design(BandKind::Peaking, RATE, 1000.0, 1.0, -9.0);
        assert!((c.magnitude_db(RATE, 1000.0) + 9.0).abs() < 0.2);
    }

    #[test]
    fn filters_stay_stable_at_absurd_settings() {
        // A user can drag a frequency slider past Nyquist; coefficients must be
        // clamped before the trig rather than producing NaN and a burst of noise.
        for f in [0.0, 1.0, RATE * 0.5, RATE, RATE * 4.0] {
            for q in [0.0, 0.001, 100.0] {
                let mut b = Biquad::design(BandKind::LowPass, RATE, f, q, 0.0);
                let mut y = 0.0;
                for i in 0..1000 { y = b.process((i as f32 * 0.1).sin()); }
                assert!(y.is_finite(), "f={f} q={q} produced {y}");
            }
        }
    }

    #[test]
    fn a_compressor_only_works_above_its_threshold() {
        let c = Compressor::new(RATE, -20.0, 4.0, 1.0, 50.0);
        // well below: untouched
        assert!((c.curve_db(-40.0) - -40.0).abs() < 1e-3);
        // well above: reduced by the ratio
        let out = c.curve_db(-4.0);
        let expect = -20.0 + 16.0 / 4.0;
        assert!((out - expect).abs() < 0.01, "4:1 above threshold: {out} vs {expect}");
        // the curve never increases level (without makeup) and never inverts
        let mut last = f32::NEG_INFINITY;
        for db in -60..0 {
            let y = c.curve_db(db as f32);
            assert!(y >= last - 1e-3, "the curve must be monotonic");
            assert!(y <= db as f32 + 1e-3, "and must not amplify");
            last = y;
        }
    }

    #[test]
    fn the_soft_knee_is_continuous_where_a_hard_one_would_kink() {
        // The knee is the audible part: a discontinuity in slope is what makes
        // a compressor sound like it is grabbing.
        let c = Compressor::new(RATE, -20.0, 4.0, 1.0, 50.0);
        let step = 0.05;
        let mut prev_slope: Option<f32> = None;
        let mut x = -30.0f32;
        while x < -10.0 {
            let slope = (c.curve_db(x + step) - c.curve_db(x)) / step;
            if let Some(p) = prev_slope {
                assert!((slope - p).abs() < 0.15, "slope jumped at {x}: {p} -> {slope}");
            }
            prev_slope = Some(slope);
            x += step;
        }
    }

    #[test]
    fn a_compressor_attacks_faster_than_it_releases() {
        // Otherwise it pumps on every syllable.
        let mut c = Compressor::new(RATE, -20.0, 8.0, 1.0, 200.0);
        for _ in 0..(RATE as usize / 100) { c.process(0.9); }
        let grabbed = c.reduction_db();
        assert!(grabbed < -3.0, "it should be working: {grabbed} dB");
        for _ in 0..(RATE as usize / 1000) { c.process(0.0001); }
        assert!(c.reduction_db() < grabbed + 3.0, "release must not snap back instantly");
    }

    #[test]
    fn a_limiter_never_lets_a_sample_past_the_ceiling() {
        // The one thing a limiter must not do. Lookahead is what makes it
        // possible without distortion.
        let ceiling = -1.0f32;
        let mut l = Limiter::new(RATE, ceiling, 5.0, 50.0);
        assert!(l.lookahead_samples() > 100, "lookahead must be real");
        let peak_allowed = from_db(ceiling) * 1.02; // 2% slack for smoothing
        let mut worst = 0.0f32;
        for i in 0..20_000 {
            // quiet passage, then a sudden transient — the case that catches a
            // limiter with no lookahead
            let x = if i % 5000 == 0 { 4.0 } else { (i as f32 * 0.05).sin() * 0.2 };
            worst = worst.max(l.process(x).abs());
        }
        assert!(worst <= peak_allowed, "output hit {worst}, ceiling is {peak_allowed}");
    }

    #[test]
    fn a_transform_inverts_exactly() {
        // The renderer needs the inverse map (for each output pixel, sample
        // where?), so `apply` and `invert` disagreeing shifts the picture.
        let t = Transform { x: 0.1, y: -0.05, scale_x: 1.7, scale_y: 0.8,
                            rotation: 33.0, anchor_x: 0.5, anchor_y: 0.5,
                            opacity: 1.0, crop: (0.0, 0.0, 0.0, 0.0) };
        for (sx, sy) in [(0.0, 0.0), (0.5, 0.5), (1.0, 0.25), (0.3, 0.9)] {
            let (fx, fy) = t.apply(sx, sy);
            let (bx, by) = t.invert(fx, fy);
            assert!((bx - sx).abs() < 1e-4 && (by - sy).abs() < 1e-4,
                    "({sx},{sy}) -> ({fx},{fy}) -> ({bx},{by})");
        }
        assert!(Transform::default().is_identity());
        assert_eq!(Transform::default().apply(0.3, 0.7), (0.3, 0.7), "identity moves nothing");
    }

    #[test]
    fn crop_excludes_only_what_is_outside_it() {
        let t = Transform { crop: (0.1, 0.2, 0.1, 0.0), ..Default::default() };
        assert!(t.cropped_in(0.5, 0.5));
        assert!(!t.cropped_in(0.05, 0.5), "left edge");
        assert!(!t.cropped_in(0.5, 0.1), "top edge");
        assert!(!t.cropped_in(0.95, 0.5), "right edge");
        assert!(t.cropped_in(0.5, 0.99), "the bottom was not cropped");
    }

    #[test]
    fn a_neutral_grade_is_a_true_bypass_and_saturation_uses_real_luma() {
        let c = Color::default();
        assert!(c.is_identity());
        for rgb in [[0.0, 0.0, 0.0], [0.5, 0.25, 0.75], [1.0, 1.0, 1.0]] {
            let out = c.apply(rgb);
            for i in 0..3 { assert!((out[i] - rgb[i]).abs() < 1e-5, "{rgb:?} was altered"); }
        }
        // desaturating fully must give Rec.709 luma, not a flat average — using
        // the average is what turns skin tones grey-green
        let grey = Color { saturation: 0.0, ..Default::default() }.apply([1.0, 0.0, 0.0]);
        assert!((grey[0] - 0.2126).abs() < 1e-3, "red weighs 0.2126, not 1/3");
        assert!((grey[0] - grey[1]).abs() < 1e-5 && (grey[1] - grey[2]).abs() < 1e-5);
        // and output is clamped, so a heavy gain cannot emit an out-of-range pixel
        let hot = Color { gain: [4.0; 3], ..Default::default() }.apply([0.9, 0.9, 0.9]);
        assert!(hot.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn retiming_reads_the_right_source_instant() {
        let len = Ticks::from_seconds(10.0);
        // constant double speed: a 10s clip consumes 20s of source
        let fast = Retime { from_speed: 2.0, to_speed: 2.0, ..Default::default() };
        assert_eq!(fast.source_needed(len), Ticks::from_seconds(20.0));
        assert_eq!(fast.source_at(Ticks::from_seconds(5.0), len), Ticks::from_seconds(10.0));
        // identity
        let id = Retime::default();
        assert!(id.is_identity());
        assert_eq!(id.source_at(Ticks::from_seconds(3.0), len), Ticks::from_seconds(3.0));

        // A RAMP is where naive code breaks: the source position is the
        // INTEGRAL of the speed curve, not the speed. Ramping 1x → 3x over a
        // 10s clip consumes the mean (2x) = 20s, and the halfway point is NOT
        // halfway through the source.
        let ramp = Retime { from_speed: 1.0, to_speed: 3.0, ..Default::default() };
        assert_eq!(ramp.source_needed(len), Ticks::from_seconds(20.0));
        let mid = ramp.source_at(Ticks::from_seconds(5.0), len);
        assert!((mid.to_seconds() - 7.5).abs() < 0.01, "integral, not speed: {}", mid.to_seconds());
        assert!(mid.to_seconds() > 5.0, "and not linear");
        // monotonic: time must never run backwards inside a forward ramp
        let mut prev = Ticks::ZERO;
        for i in 0..=100 {
            let t = ramp.source_at(Ticks(len.0 * i / 100), len);
            assert!(t.0 >= prev.0, "source time went backwards at {i}%");
            prev = t;
        }
        // reverse runs from the end back to the start
        let rev = Retime { reverse: true, ..Default::default() };
        assert_eq!(rev.source_at(Ticks::ZERO, len), Ticks::from_seconds(10.0));
        assert_eq!(rev.source_at(len, len), Ticks::ZERO);
    }

    #[test]
    fn effects_instantiate_from_a_project_and_unknown_kinds_decline() {
        let mut params = std::collections::BTreeMap::new();
        params.insert("threshold".to_string(), -12.0);
        params.insert("ratio".to_string(), 6.0);
        let e = studio_core::Effect { kind: "compressor".into(), params, enabled: true };
        match instantiate(&e, RATE) {
            Some(Instance::Compressor(c)) => {
                assert_eq!(c.threshold_db, -12.0);
                assert_eq!(c.ratio, 6.0);
            }
            _ => panic!("should have built a compressor"),
        }
        // an unknown plugin declines rather than failing the project
        let unknown = studio_core::Effect { kind: "sparkle_v9".into(),
            params: Default::default(), enabled: true };
        assert!(instantiate(&unknown, RATE).is_none());
        // and defaults fill in for a description that omits parameters
        let bare = studio_core::Effect { kind: "transform".into(),
            params: Default::default(), enabled: true };
        match instantiate(&bare, RATE) {
            Some(Instance::Transform(t)) => assert!(t.is_identity()),
            _ => panic!("transform should default to identity"),
        }
    }

    /// A flat band is the identity to float rounding (the premise of skipping
    /// it; NOT to the bit -- the five-term sum rounds, which this test found),
    /// and an EQ that skipped its flat band, then had it raised by a ride,
    /// matches one that computed the band all along.
    #[test]
    fn a_flat_band_is_skipped_and_a_later_ride_picks_up_seamlessly() {
        let x: Vec<f32> = (0..4_000).map(|i| ((i * 7919 % 1000) as f32 / 500.0 - 1.0) * 0.5).collect();
        for kind in [BandKind::Peaking, BandKind::LowShelf, BandKind::HighShelf] {
            let mut b = Biquad::design(kind, 48_000.0, 700.0, 0.9, 0.0);
            let worst = x.iter().map(|v| (b.process(*v) - v).abs()).fold(0.0f32, f32::max);
            assert!(worst < 1e-5, "{kind:?} at 0 dB is the identity to rounding: {worst}");
        }
        let e = studio_core::Effect { kind: "eq".into(), enabled: true,
            params: [("bands".to_string(), 1.0), ("freq0".to_string(), 700.0), ("gain0".to_string(), 0.0)].into() };
        let Some(Instance::Eq(mut eq)) = instantiate(&e, 48_000.0) else { panic!("an eq") };
        let mut reference = Biquad::design(BandKind::Peaking, 48_000.0, 700.0, 0.707, 0.0);
        let mut worst = 0.0f32;
        for (i, v) in x.iter().enumerate() {
            if i == 2_000 {
                eq.set_band(0, None, None, Some(6.0));
                reference = Biquad::design(BandKind::Peaking, 48_000.0, 700.0, 0.707, 6.0).with_state_of(&reference);
            }
            worst = worst.max((eq.process(*v) - reference.process(*v)).abs());
        }
        assert!(worst < 1e-5, "skipped-then-ridden vs computed throughout (a seam would be ~1e-2): {worst}");
    }

    #[test]
    fn process_audio_dispatches_to_the_right_audio_effect_and_passes_video_kinds_through() {
        let mut params = std::collections::BTreeMap::new();
        params.insert("bands".to_string(), 1.0);
        params.insert("freq0".to_string(), 1000.0);
        params.insert("gain0".to_string(), 6.0);
        let eq_effect = studio_core::Effect { kind: "eq".into(), params, enabled: true };
        let mut eq_instance = instantiate(&eq_effect, RATE).unwrap();
        // A +6dB peaking EQ must actually change a signal at that frequency —
        // this is the dispatch working, not a no-op passthrough.
        let mut changed = false;
        for i in 0..200 {
            let t = i as f32 / RATE;
            let x = (std::f32::consts::TAU * 1000.0 * t).sin();
            let y = eq_instance.process_audio(x);
            if (y - x).abs() > 1e-3 {
                changed = true;
            }
        }
        assert!(changed, "an EQ instance's process_audio should actually filter, not pass through");

        // Video-only kinds pass audio through unchanged rather than silencing
        // it or panicking — a mixed chain iterating kinds blindly must not
        // need to filter video effects out first.
        let transform_effect = studio_core::Effect { kind: "transform".into(), params: Default::default(), enabled: true };
        let mut transform_instance = instantiate(&transform_effect, RATE).unwrap();
        assert_eq!(transform_instance.process_audio(0.42), 0.42);
    }

    #[test]
    fn multiband_instantiates_from_a_project_effect_and_actually_processes() {
        let mut params = std::collections::BTreeMap::new();
        params.insert("bands".to_string(), 1.0); // one crossover -> two bands
        params.insert("freq0".to_string(), 1000.0);
        params.insert("band0_bypass".to_string(), 0.0); // enable the compressor on band 0
        params.insert("band1_gain".to_string(), -12.0); // and cut band 1's gain hard
        let effect = studio_core::Effect { kind: "multiband".into(), params, enabled: true };
        let inst = instantiate(&effect, RATE).expect("multiband should now instantiate");
        match &inst {
            Instance::Multiband(m) => assert_eq!(m.bands.len(), 2, "one crossover point should yield two bands"),
            _ => panic!("expected a Multiband instance"),
        }
        // A signal well above the crossover should read quieter than the
        // same signal well below it, since band1 (the upper band) has a
        // -12dB gain cut and band0 does not. Two FRESH instances — reusing
        // one across both sweeps would let the crossover filter's residual
        // low-frequency energy bleed into the high-frequency measurement.
        let mut low_inst = instantiate(&effect, RATE).unwrap();
        let mut high_inst = instantiate(&effect, RATE).unwrap();
        let mut low_peak = 0.0f32;
        let mut high_peak = 0.0f32;
        for i in 0..4000 {
            let t = i as f32 / RATE;
            low_peak = low_peak.max(low_inst.process_audio((std::f32::consts::TAU * 200.0 * t).sin()).abs());
        }
        for i in 0..4000 {
            let t = i as f32 / RATE;
            high_peak = high_peak.max(high_inst.process_audio((std::f32::consts::TAU * 4000.0 * t).sin()).abs());
        }
        assert!(high_peak < low_peak * 0.6, "the upper band's -12dB cut should read measurably quieter: low {low_peak}, high {high_peak}");
    }

    #[test]
    fn saturation_instantiates_from_a_project_effect_and_actually_drives() {
        let mut params = std::collections::BTreeMap::new();
        params.insert("profile".to_string(), 0.0); // Tube
        params.insert("drive".to_string(), 0.9);
        let effect = studio_core::Effect { kind: "saturation".into(), params, enabled: true };
        let mut inst = instantiate(&effect, RATE).expect("saturation should now instantiate");
        let mut clean = instantiate(&studio_core::Effect {
            kind: "saturation".into(),
            params: { let mut p = std::collections::BTreeMap::new(); p.insert("drive".to_string(), 0.0); p },
            enabled: true,
        }, RATE).unwrap();

        let mut differs = false;
        for i in 0..500 {
            let t = i as f32 / RATE;
            let x = (std::f32::consts::TAU * 200.0 * t).sin();
            let driven = inst.process_audio(x);
            let dry = clean.process_audio(x);
            if (driven - dry).abs() > 1e-3 { differs = true; }
            assert!(driven.is_finite());
        }
        assert!(differs, "drive=0.9 should audibly differ from drive=0.0 through the same dispatch path");
    }

    #[test]
    fn imager_is_deliberately_not_an_instantiable_kind() {
        // Pinning the documented reason, not just the absence: Instance's
        // process_audio is mono (one sample in, one out), and imaging is
        // inherently a two-channel-at-once operation — see instantiate's
        // own doc comment for why this is a scope boundary, not an oversight.
        let effect = studio_core::Effect { kind: "imager".into(), params: Default::default(), enabled: true };
        assert!(instantiate(&effect, RATE).is_none());
    }

    #[test]
    fn gain_instantiates_and_applies_exactly_the_requested_db() {
        let mut params = std::collections::BTreeMap::new();
        params.insert("db".to_string(), -6.0206); // -6.0206dB = exactly half amplitude
        let effect = studio_core::Effect { kind: "gain".into(), params, enabled: true };
        let mut inst = instantiate(&effect, RATE).expect("gain should instantiate");
        let out = inst.process_audio(1.0);
        assert!((out - 0.5).abs() < 1e-3, "-6.0206dB should read as almost exactly half amplitude, got {out}");
    }

    #[test]
    fn a_default_gain_effect_is_unity_not_silence() {
        // A bare Effect{kind:"gain"} with no "db" param must default to 0dB
        // (unchanged), the same "sane default for an omitted parameter"
        // convention every other kind here already follows — not to 0.0
        // linear gain, which would silently mute anything using the default.
        let effect = studio_core::Effect { kind: "gain".into(), params: Default::default(), enabled: true };
        let mut inst = instantiate(&effect, RATE).unwrap();
        assert_eq!(inst.process_audio(0.37), 0.37);
    }
}

#[cfg(test)]
mod live_param_tests {
    use super::*;
    use std::collections::BTreeMap;

    fn fx(kind: &str, params: &[(&str, f32)]) -> studio_core::Effect {
        studio_core::Effect {
            kind: kind.into(),
            params: params.iter().map(|(k, v)| ((*k).to_string(), *v)).collect::<BTreeMap<_, _>>(),
            enabled: true,
        }
    }

    #[test]
    fn the_static_live_table_and_set_param_agree_on_every_schema_key() {
        // `is_live_param` is what a UI offers "automate" on; `set_param`
        // is what the render calls. They must be the same set, and the
        // rest must refuse quietly. Every kind, every expanded key (an
        // EQ with three bands, so the digit rule is exercised past 0).
        let mut live_seen = 0;
        for (kind, _) in schema::KINDS {
            if schema::BUS_STEREO_KINDS.contains(kind) {
                // stereo, run by the mixdown on a bus: must NOT be buildable
                // here, or the mono chain would take it and do it wrong
                let e = studio_core::Effect { kind: (*kind).into(), params: schema::defaults(kind), enabled: true };
                assert!(instantiate(&e, 48_000.0).is_none(), "{kind} is stereo: not a per-sample instance");
                continue;
            }
            let mut params = schema::defaults(kind);
            if *kind == "eq" { params.insert("bands".into(), 3.0); }
            let e = studio_core::Effect { kind: (*kind).into(), params, enabled: true };
            let mut inst = instantiate(&e, 48_000.0).expect(kind);
            for spec in schema::expand(&e).unwrap() {
                let ok = inst.set_param(&spec.key, spec.default, 48_000.0);
                assert_eq!(ok, Instance::is_live_param(kind, &spec.key),
                           "{kind}.{}: set_param={ok}, is_live_param says otherwise", spec.key);
                if ok { live_seen += 1; }
            }
            assert!(!inst.set_param("no-such-key", 1.0, 48_000.0), "{kind}: an unknown key is refused");
            assert!(!Instance::is_live_param(kind, "no-such-key"));
            // A set to the default changed nothing audible: the instance
            // still runs and stays finite.
            for i in 0..200 { assert!(inst.process_audio((i as f32 * 0.1).sin() * 0.5).is_finite()); }
        }
        assert!(live_seen >= 30, "the live set is not a handful: {live_seen}");
        assert!(!Instance::is_live_param("eq", "freq"), "an EQ key needs its band index");
        assert!(!Instance::is_live_param("eq", "kind0"), "a band's TYPE is a redesign from scratch, not a ride");
        assert!(Instance::is_live_param("eq", "gain7"));
    }

    #[test]
    fn an_eq_sweep_keeps_the_filters_memory_where_a_reinstantiate_would_not() {
        // A low-pass swept 200 Hz → 4 kHz over a 200 Hz tone, redesigned
        // every 64 samples. Redesigning in place keeps x1/x2/y1/y2; the
        // alternative (build a fresh instance per block) restarts the
        // filter from zero and puts a step in the output every block.
        let rate = 48_000.0;
        let tone = |i: usize| (2.0 * std::f32::consts::PI * 200.0 * i as f32 / rate).sin() * 0.5;
        let params = [("bands", 1.0), ("kind0", 3.0), ("freq0", 200.0), ("q0", 0.707), ("gain0", 0.0)];
        let mut live = instantiate(&fx("eq", &params), rate).unwrap();
        let mut out_live = Vec::new();
        let mut out_restart = Vec::new();
        let blocks = 150;
        for b in 0..blocks {
            let freq = 200.0 + (4000.0 - 200.0) * b as f32 / blocks as f32;
            assert!(live.set_param("freq0", freq, rate));
            let mut p = params.to_vec();
            p[2] = ("freq0", freq);
            let mut fresh = instantiate(&fx("eq", &p), rate).unwrap();
            for i in 0..64 {
                let x = tone(b * 64 + i);
                out_live.push(live.process_audio(x));
                out_restart.push(fresh.process_audio(x));
            }
        }
        let max_step = |o: &[f32]| o.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        // A 200 Hz tone at 48k moves at most ~0.013 per sample; the live
        // sweep stays near that, the restart path jumps by an order more.
        let (live_step, restart_step) = (max_step(&out_live), max_step(&out_restart));
        assert!(live_step < 0.03, "live redesign: no clicks ({live_step})");
        assert!(restart_step > live_step * 3.0, "a restart per block clicks ({restart_step} vs {live_step})");
        assert!(out_live.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn a_room_shrunk_mid_tail_decays_faster_from_there() {
        let rate = 48_000.0;
        let mut r = instantiate(&fx("reverb", &[("size", 1.0), ("damp", 0.0), ("mix", 1.0), ("channels", 1.0)]), rate).unwrap();
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        let mut tail: Vec<f32> = (0..48_000).map(|i| r.process_audio(if i == 0 { 1.0 } else { 0.0 })).collect();
        let before = rms(&tail[40_000..48_000]);
        assert!(r.set_param("size", 0.0, rate));
        tail.clear();
        tail.extend((0..24_000).map(|_| r.process_audio(0.0)));
        let after = rms(&tail[16_000..24_000]);
        assert!(after < before * 0.1, "size 1 → 0 mid-tail: the room empties ({before} → {after})");
        // The mix can be ridden to zero and the dry comes straight through.
        assert!(r.set_param("mix", 0.0, rate));
        assert_eq!(r.process_audio(0.25), 0.25);
    }
}

#[cfg(test)]
mod playrate_registry_tests {
    use super::*;

    fn fx(kind: &str, params: &[(&str, f32)]) -> studio_core::Effect {
        studio_core::Effect {
            kind: kind.into(),
            params: params.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            enabled: true,
        }
    }

    #[test]
    fn tapestop_and_halftime_are_reachable_from_a_project_chain() {
        // DSP that instantiate() does not know is DSP no bus, clip, or UI
        // can ever use — the state these two were in until now.
        let mut ts = instantiate(&fx("tapestop", &[("start", 0.0), ("length", 0.25)]), 48_000.0)
            .expect("tapestop instantiates");
        let mut ht = instantiate(&fx("halftime", &[("window", 0.25)]), 48_000.0)
            .expect("halftime instantiates");
        // And they are AUDIO effects in the dispatch, not passthroughs: a
        // constant fed through a completed tape stop comes out silent.
        let out: Vec<f32> = (0..2 * 24_000).map(|_| ts.process_audio(0.8)).collect();
        assert!(out[2 * 20_000..].iter().all(|s| *s == 0.0), "tapestop stopped the tape");
        // Halftime's first window is a wire (nothing to replay yet), so
        // check it is at least wired to the audio path by feeding a ramp
        // and seeing the second window differ from the input.
        let input: Vec<f32> = (0..2 * 48_000).map(|i| (i / 2) as f32 / 48_000.0).collect();
        let out: Vec<f32> = input.iter().map(|&s| ht.process_audio(s)).collect();
        assert_ne!(&out[2 * 24_000..2 * 30_000], &input[2 * 24_000..2 * 30_000],
                   "the second window is replayed, not passed through");
    }
}

#[cfg(test)]
mod realtune_registry_tests {
    use super::*;

    fn fx(kind: &str, params: &[(&str, f32)]) -> studio_core::Effect {
        studio_core::Effect {
            kind: kind.into(),
            params: params.iter().map(|(k, v)| ((*k).to_string(), *v)).collect(),
            enabled: true,
        }
    }

    #[test]
    fn a_chain_can_hold_a_live_tuner_and_it_corrects_through_the_chain() {
        // The point of the registry entry: this is reachable as an insert,
        // which the offline autotune pass deliberately is not.
        let mut inst = instantiate(&fx("autotune", &[("notes", 1.0), ("note0", 69.0),
                                                     ("speed", 5.0), ("channels", 1.0)]), 48_000.0)
            .expect("autotune is a registered kind");
        let out: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 / 48_000.0 * 452.0 * std::f32::consts::TAU).sin() * 0.5)
            .map(|x| inst.process_audio(x))
            .collect();
        // Zero crossings, for the reason realtune's own tests give.
        let (mut first, mut last, mut n) = (None, 0.0f32, 0usize);
        for i in 24_001..44_000 {
            if out[i - 1] < 0.0 && out[i] >= 0.0 {
                let t = i as f32 - 1.0 + (-out[i - 1]) / (out[i] - out[i - 1]);
                if first.is_none() { first = Some(t); } else { n += 1; }
                last = t;
            }
        }
        let hz = n as f32 * 48_000.0 / (last - first.unwrap());
        let cents = 1200.0 * (hz / 440.0).log2();
        assert!(cents.abs() < 25.0, "chain output at {hz:.1} Hz ({cents:.1} cents off A440)");
    }

    #[test]
    fn key_lock_mode_is_what_an_absent_note_count_means() {
        // Params are a flat f32 map, so the note SET is length-prefixed;
        // zero (or absent) must mean "use the scale", not "lock to nothing".
        let inst = instantiate(&fx("autotune", &[("root", 9.0), ("mode", 0.0)]), 48_000.0);
        assert!(matches!(inst, Some(Instance::RealTune(_))));
    }
}

#[cfg(test)]
mod adaptive_sat_registry_tests {
    use super::*;

    fn fx(kind: &str, params: &[(&str, f32)]) -> studio_core::Effect {
        studio_core::Effect {
            kind: kind.into(),
            params: params.iter().map(|(k, v)| ((*k).to_string(), *v)).collect(),
            enabled: true,
        }
    }

    #[test]
    fn the_chain_can_hold_an_adaptive_saturator_and_it_adapts() {
        // Through the registry, not the struct: the same 808 and lead a
        // producer would put on two channels must come out with different
        // character from ONE setting.
        let params = [("profile", 1.0), ("drive", 0.5), ("note_amount", 0.8),
                      ("sib_amount", 0.5), ("channels", 1.0)];
        let mut low = instantiate(&fx("adaptivesat", &params), 48_000.0).expect("registered");
        let mut high = instantiate(&fx("adaptivesat", &params), 48_000.0).expect("registered");
        let tone = |hz: f32| (0..24_000)
            .map(|i| (i as f32 / 48_000.0 * hz * std::f32::consts::TAU).sin() * 0.4)
            .collect::<Vec<f32>>();
        let l: Vec<f32> = tone(55.0).iter().map(|x| low.process_audio(*x)).collect();
        let h: Vec<f32> = tone(660.0).iter().map(|x| high.process_audio(*x)).collect();

        let harm = |buf: &[f32], f0: f32| {
            let mut t = 0.0f32;
            for k in 2..=5 {
                let f = f0 * k as f32;
                let (mut re, mut im) = (0.0f32, 0.0f32);
                for (i, x) in buf.iter().enumerate() {
                    let a = std::f32::consts::TAU * f * i as f32 / 48_000.0;
                    re += x * a.cos();
                    im += x * a.sin();
                }
                t += (re * re + im * im).sqrt() / buf.len() as f32;
            }
            t / buf.iter().map(|v| v.abs()).sum::<f32>().max(1e-9) * buf.len() as f32
        };
        assert!(harm(&l, 55.0) > harm(&h, 660.0) * 1.5,
                "the low note came out dirtier: {:.4} vs {:.4}",
                harm(&l, 55.0), harm(&h, 660.0));
    }

    #[test]
    fn tied_is_reachable_from_params() {
        let a = instantiate(&fx("adaptivesat", &[("tied", 1.0)]), 48_000.0);
        assert!(matches!(a, Some(Instance::AdaptiveSat(_))));
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    /// A master-chain stage driven hard by a loud passage must let go: after
    /// its release, a quiet signal passes at (near) unity. Goat's whole-song
    /// render lost ~10 dB of low end at 3:04 (peaks -16.6 dBFS) that a render
    /// STARTED at 3:04 did not -- gain reduction carried from the loud intro?
    /// (gain reduction during the loud passage, gain on the quiet tail), dB.
    /// The first must be real, or "it recovered" says nothing: a stage that
    /// never engaged recovers trivially.
    fn recovers(kind: &str) -> (f32, f32) {
        let rate = 48_000u32;
        let mut params = std::collections::BTreeMap::new();
        // a default multiband has every band BYPASSED (crossovers only), so it
        // cannot hold gain; engage its bands, or the test measures a split-and-sum
        if kind == "multiband" { for i in 0..3 { params.insert(format!("band{i}_bypass"), 0.0f32); } }
        let e = studio_core::Effect { kind: kind.into(), enabled: true, params };
        let mut fx = instantiate(&e, rate as f32).expect("an instance");
        let tone = |i: usize, a: f32| a * (i as f32 / rate as f32 * 60.0 * std::f32::consts::TAU).sin();
        // 2 s at +12 dBFS (a float master bus goes over 0; Goat's intro did)
        let loud = 4.0f32;
        let hot: Vec<f32> = (0..rate as usize * 2).map(|i| fx.process_audio(tone(i, loud))).collect();
        let hot_rms = (hot[rate as usize..].iter().map(|x| x * x).sum::<f32>() / rate as f32).sqrt();
        let reduction = 20.0 * (hot_rms / (loud / std::f32::consts::SQRT_2)).log10();
        let quiet = 0.03f32;                                                        // -30 dBFS
        let n = rate as usize * 6;
        let mut out = Vec::with_capacity(n);
        for i in 0..n { out.push(fx.process_audio(tone(i, quiet))); }
        let tail = &out[rate as usize * 4..];                                      // after 4 s of quiet
        let rms = (tail.iter().map(|x| x * x).sum::<f32>() / tail.len() as f32).sqrt();
        (reduction, 20.0 * (rms / (quiet / std::f32::consts::SQRT_2)).log10())
    }

    #[test]
    fn the_limiter_lets_go_after_a_loud_passage() {
        let (red, g) = recovers("limiter");
        assert!(red < -3.0, "limiter must engage on the loud passage: {red:+.1} dB");
        assert!(g.abs() < 1.0, "limiter still {g:+.1} dB after 4 s of quiet");
    }

    #[test]
    fn the_compressor_lets_go_after_a_loud_passage() {
        let (red, g) = recovers("compressor");
        assert!(red < -3.0, "compressor must engage on the loud passage: {red:+.1} dB");
        assert!(g.abs() < 1.0, "compressor still {g:+.1} dB after 4 s of quiet");
    }

    #[test]
    fn the_multiband_lets_go_after_a_loud_passage() {
        let (red, g) = recovers("multiband");
        assert!(red < -3.0, "multiband must engage on the loud passage: {red:+.1} dB");
        assert!(g.abs() < 1.0, "multiband still {g:+.1} dB after 4 s of quiet");
    }
}
