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

pub mod analysis;
pub mod saturation;
pub mod multiband;
pub mod imager;

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
}

/// A stack of bands.
#[derive(Debug, Clone, Default)]
pub struct Eq { pub bands: Vec<Biquad> }

impl Eq {
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.bands.iter_mut().fold(x, |acc, b| b.process(acc))
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
    /// Gain required by each sample in the lookahead window, parallel to `delay`.
    need: Vec<f32>,
    head: usize,
}

impl Limiter {
    pub fn new(rate: f32, ceiling_db: f32, lookahead_ms: f32, release_ms: f32) -> Limiter {
        let n = ((lookahead_ms * 0.001 * rate) as usize).max(1);
        Limiter {
            ceiling_db,
            release_c: time_const(release_ms, rate),
            gain_db: 0.0,
            delay: vec![0.0; n],
            need: vec![0.0; n],
            head: 0,
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
        self.need[self.head] = need;
        self.head = (self.head + 1) % self.delay.len();

        // Required gain over the whole lookahead window. O(window) per sample —
        // a monotonic deque would make it O(1), and is the right optimisation
        // later; being audibly correct first is worth more than being fast at
        // letting samples through.
        let target = self.need.iter().copied().fold(0.0f32, f32::min);
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
    Transform(Transform),
    Color(Color),
    Retime(Retime),
}

/// Build a running effect from a project's stored description, or `None` when
/// this build does not know the kind — which is how a project made with a
/// plugin you lack still opens.
pub fn instantiate(effect: &studio_core::Effect, rate: f32) -> Option<Instance> {
    let q = &effect.params;
    Some(match effect.kind.as_str() {
        "eq" => {
            let n = p(q, "bands", 1.0).clamp(1.0, 8.0) as usize;
            let mut eq = Eq::default();
            for i in 0..n {
                eq.bands.push(Biquad::design(
                    match p(q, &format!("kind{i}"), 0.0) as i32 {
                        1 => BandKind::LowShelf, 2 => BandKind::HighShelf,
                        3 => BandKind::LowPass, 4 => BandKind::HighPass,
                        _ => BandKind::Peaking,
                    },
                    rate,
                    p(q, &format!("freq{i}"), 1000.0),
                    p(q, &format!("q{i}"), 0.707),
                    p(q, &format!("gain{i}"), 0.0),
                ));
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
        "limiter" => Instance::Limiter(Limiter::new(rate, p(q, "ceiling", -0.3),
                                                    p(q, "lookahead", 5.0), p(q, "release", 50.0))),
        "transform" => Instance::Transform(Transform {
            x: p(q, "x", 0.0), y: p(q, "y", 0.0),
            scale_x: p(q, "scale_x", 1.0), scale_y: p(q, "scale_y", 1.0),
            rotation: p(q, "rotation", 0.0),
            anchor_x: p(q, "anchor_x", 0.5), anchor_y: p(q, "anchor_y", 0.5),
            opacity: p(q, "opacity", 1.0),
            crop: (p(q, "crop_l", 0.0), p(q, "crop_t", 0.0), p(q, "crop_r", 0.0), p(q, "crop_b", 0.0)),
        }),
        "color" => Instance::Color(Color {
            lift: [p(q, "lift_r", 0.0), p(q, "lift_g", 0.0), p(q, "lift_b", 0.0)],
            gamma: [p(q, "gamma_r", 1.0), p(q, "gamma_g", 1.0), p(q, "gamma_b", 1.0)],
            gain: [p(q, "gain_r", 1.0), p(q, "gain_g", 1.0), p(q, "gain_b", 1.0)],
            saturation: p(q, "saturation", 1.0),
        }),
        "retime" => Instance::Retime(Retime {
            from_speed: p(q, "from_speed", 1.0), to_speed: p(q, "to_speed", 1.0),
            preserve_pitch: p(q, "preserve_pitch", 1.0) > 0.5,
            reverse: p(q, "reverse", 0.0) > 0.5,
        }),
        _ => return None,
    })
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
}
