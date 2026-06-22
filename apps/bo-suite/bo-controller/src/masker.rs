// src/masker.rs (vendored from bo-mux — see header note below)
#![allow(dead_code)]
//
// Frequency sanitizer — generates shaped continuous noise patterns that
// psychoacoustically mask common environmental sound sources (car engine
// drone, road noise, fan hum, HVAC, refrigerator). Intended to mix at low
// amplitude underneath whatever else the user is listening to.
//
// HONEST SCOPE — this is NOT active noise cancellation. True ANC requires
// a microphone feedback loop and inverse-phase response in real time,
// which neither a remote speaker controller nor a non-headphone speaker
// can practically do. What this module does instead:
//
//   1. Generates pink/brown noise of arbitrary length using the
//      Voss-McCartney algorithm (O(1) per sample, no FFT).
//   2. Applies a peaking-EQ filter to *boost* energy at the offending
//      noise's center frequency, so the masker's spectral profile aligns
//      with the noise source's profile.
//   3. The aligned masker raises the just-noticeable-difference (JND)
//      threshold for the noise, making it perceptually fade — the ear's
//      auditory system stops attending to it (loudness summation +
//      attention masking, well-studied in psychoacoustics).
//
// This works best for *continuous, narrowband* noise (a refrigerator
// compressor, a single fan, road tire roar) and poorly for *transient*
// noise (door slams, traffic spikes). It is also not a substitute for
// real hearing protection in high-SPL environments — it shifts perception,
// not physics.
//
// Output is generated as mono 16-bit PCM at a configurable sample rate
// (default 48 kHz) so the chan_server can wrap it as a WAV stream and
// distribute it through the same per-channel route used for the soundscape
// engine.

use std::f32::consts::PI;

/// A named preset describing what kind of environmental noise to mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskerPreset {
    /// Pure pink noise — equal energy per octave. Good general-purpose
    /// masker that covers most everyday noise without bias.
    Pink,
    /// Brown (red) noise — more energy in low frequencies. Better for
    /// rumbly noise like distant traffic or HVAC ducts.
    Brown,
    /// Car-engine drone — emphasis at 30-150 Hz. Tuned for in-cabin
    /// road noise during driving.
    EngineDrone,
    /// Highway tire roar — emphasis at 200-1000 Hz broadband.
    RoadNoise,
    /// AC/fan hum — peaks at 60 Hz and its 120 Hz harmonic (US mains
    /// frequency); shift the EngineDrone preset for 50 Hz countries.
    FanHum,
    /// HVAC drone — broadband with peak around 300 Hz where most
    /// air-handler ducts resonate.
    HvacDrone,
}

impl MaskerPreset {
    /// Stable identifier used over the wire and in config.
    pub fn id(&self) -> &'static str {
        match self {
            MaskerPreset::Pink        => "pink",
            MaskerPreset::Brown       => "brown",
            MaskerPreset::EngineDrone => "engine",
            MaskerPreset::RoadNoise   => "road",
            MaskerPreset::FanHum      => "fan",
            MaskerPreset::HvacDrone   => "hvac",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "pink"   => Some(MaskerPreset::Pink),
            "brown"  => Some(MaskerPreset::Brown),
            "engine" => Some(MaskerPreset::EngineDrone),
            "road"   => Some(MaskerPreset::RoadNoise),
            "fan"    => Some(MaskerPreset::FanHum),
            "hvac"   => Some(MaskerPreset::HvacDrone),
            _ => None,
        }
    }

    /// (center_hz, q) — center frequency of the peaking EQ for this preset
    /// and its Q (sharpness). None means no EQ (raw pink/brown).
    fn eq_profile(&self) -> Option<(f32, f32)> {
        match self {
            MaskerPreset::Pink | MaskerPreset::Brown => None,
            MaskerPreset::EngineDrone => Some((80.0,  0.7)),  // wide bump 30-150
            MaskerPreset::RoadNoise   => Some((500.0, 0.9)),  // wide 200-1000
            MaskerPreset::FanHum      => Some((60.0,  3.0)),  // narrow 60 Hz spike
            MaskerPreset::HvacDrone   => Some((300.0, 1.2)),
        }
    }

    /// Base noise color before the EQ shaping. Brown (1/f^2) gives a
    /// stronger LF bias for the lower-frequency presets.
    fn base_color(&self) -> NoiseColor {
        match self {
            MaskerPreset::Pink                          => NoiseColor::Pink,
            MaskerPreset::Brown | MaskerPreset::EngineDrone | MaskerPreset::FanHum
                                                        => NoiseColor::Brown,
            MaskerPreset::RoadNoise | MaskerPreset::HvacDrone
                                                        => NoiseColor::Pink,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoiseColor { Pink, Brown }

/// Voss-McCartney pink-noise generator. State: 16 octave rows that update
/// at staggered rates so the sum has a 1/f spectrum. Cheap and accurate
/// enough that you can't audibly distinguish it from FFT-based pink noise.
pub struct PinkGen {
    rows: [f32; 16],
    counter: u32,
    rng_state: u32,
    running_sum: f32,
}

impl PinkGen {
    pub fn new(seed: u32) -> Self {
        Self { rows: [0.0; 16], counter: 0,
               rng_state: seed.max(1), running_sum: 0.0 }
    }

    fn next_random(&mut self) -> f32 {
        // xorshift32 — small, fast, deterministic from the seed.
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x;
        // Map to [-1.0, 1.0]
        ((x as f32) / (u32::MAX as f32 / 2.0)) - 1.0
    }

    /// Yield the next pink-noise sample in [-1.0, 1.0].
    pub fn next_sample(&mut self) -> f32 {
        self.counter = self.counter.wrapping_add(1);
        // Find the lowest set bit of counter — that row updates.
        let row = self.counter.trailing_zeros().min(15) as usize;
        let old = self.rows[row];
        let new = self.next_random();
        self.rows[row] = new;
        self.running_sum += new - old;
        // White-noise jitter added per-sample (the "+1" in V-M).
        let jitter = self.next_random() * 0.5;
        (self.running_sum + jitter) / 18.0   // scale to keep within [-1,1]
    }
}

/// Brown (red) noise: cumulative integration of white noise, with leak so
/// the DC component doesn't run away.
pub struct BrownGen {
    rng_state: u32,
    last: f32,
}

impl BrownGen {
    pub fn new(seed: u32) -> Self {
        Self { rng_state: seed.max(1), last: 0.0 }
    }

    pub fn next_sample(&mut self) -> f32 {
        let mut x = self.rng_state;
        x ^= x << 13; x ^= x >> 17; x ^= x << 5;
        self.rng_state = x;
        let white = ((x as f32) / (u32::MAX as f32 / 2.0)) - 1.0;
        // Leaky integrator: drift toward zero by 0.5% per sample.
        self.last = self.last * 0.995 + white * 0.05;
        self.last.clamp(-1.0, 1.0)
    }
}

/// A simple biquad peaking EQ. State = two history samples (x[n-1], x[n-2],
/// y[n-1], y[n-2]). Coefficients derived from RBJ's Audio EQ cookbook for
/// a peaking-EQ filter with given center freq, Q, and gain (dB).
pub struct PeakingEq {
    b0: f32, b1: f32, b2: f32, a1: f32, a2: f32,
    x1: f32, x2: f32, y1: f32, y2: f32,
}

impl PeakingEq {
    /// Create a peaking EQ at `center_hz` with quality factor `q` and
    /// gain `db_gain` (positive = boost, negative = cut), at the given
    /// `sample_rate`.
    ///
    /// Defended: same NaN/Inf coercion as bo-mux's PeakingEq. Without
    /// this, a non-finite parameter produces NaN coefficients which
    /// poison `x1/x2/y1/y2` on the first sample and every subsequent
    /// output is NaN. The masker's `as i16` cast turns those into 0
    /// (silent samples), so the failure is *silent dead audio*
    /// rather than visible distortion — exactly the kind of bug
    /// that takes a while to diagnose. Sister defense in
    /// bo-mux/src/masker.rs::PeakingEq.
    pub fn new(sample_rate: f32, center_hz: f32, q: f32, db_gain: f32) -> Self {
        let sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else { 48_000.0 };
        let center_hz = if center_hz.is_finite() && center_hz > 0.0 {
            center_hz.min(sample_rate * 0.49)  // below Nyquist
        } else { 1_000.0 };
        let q = if q.is_finite() && q > 0.0 { q } else { 0.707 };
        let db_gain = if db_gain.is_finite() { db_gain } else { 0.0 };
        let a = 10f32.powf(db_gain / 40.0);
        let w0 = 2.0 * PI * center_hz / sample_rate;
        let (sin_w0, cos_w0) = (w0.sin(), w0.cos());
        let alpha = sin_w0 / (2.0 * q);

        let b0 =  1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 =  1.0 - alpha * a;
        let a0 =  1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 =  1.0 - alpha / a;

        Self {
            b0: b0 / a0, b1: b1 / a0, b2: b2 / a0,
            a1: a1 / a0, a2: a2 / a0,
            x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0,
        }
    }

    pub fn process(&mut self, x: f32) -> f32 {
        // Sanitize input — same shape as bo-mux PeakingEq + the
        // mixer's biquad path. A NaN reaching this filter would
        // poison y1/y2 forever; resetting on output corruption
        // catches the rare coefficient-NaN case lazily.
        let x = if x.is_finite() { x } else { 0.0 };
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
              - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1; self.x1 = x;
        if !y.is_finite() {
            self.x1 = 0.0; self.x2 = 0.0;
            self.y1 = 0.0; self.y2 = 0.0;
            return 0.0;
        }
        self.y2 = self.y1; self.y1 = y;
        y
    }
}

/// The masker: combines a noise color with optional peaking EQ shaping and
/// an output gain. Caller pulls samples one at a time (or in chunks).
pub struct Masker {
    pink: Option<PinkGen>,
    brown: Option<BrownGen>,
    eq: Option<PeakingEq>,
    gain: f32,
}

impl Masker {
    pub fn new(preset: MaskerPreset, sample_rate: u32, gain_db: f32, seed: u32) -> Self {
        let (pink, brown) = match preset.base_color() {
            NoiseColor::Pink  => (Some(PinkGen::new(seed)), None),
            NoiseColor::Brown => (None, Some(BrownGen::new(seed))),
        };
        let eq = preset.eq_profile().map(|(hz, q)| {
            // +9 dB boost at the peak — significant but not so much that
            // it clips when summed with the noise floor.
            PeakingEq::new(sample_rate as f32, hz, q, 9.0)
        });
        // NaN / Inf defense: db_to_linear(NaN) = NaN, which would
        // multiply every subsequent sample to NaN and propagate
        // through the audio path. Coerce non-finite gain to 0 dB
        // (unity), keeping the masker audible at full level.
        let safe_gain_db = if gain_db.is_finite() { gain_db } else { 0.0 };
        Self { pink, brown, eq, gain: db_to_linear(safe_gain_db) }
    }

    pub fn next_sample(&mut self) -> f32 {
        let raw = if let Some(p) = self.pink.as_mut() { p.next_sample() }
                  else if let Some(b) = self.brown.as_mut() { b.next_sample() }
                  else { 0.0 };
        let shaped = if let Some(eq) = self.eq.as_mut() { eq.process(raw) } else { raw };
        (shaped * self.gain).clamp(-1.0, 1.0)
    }

    /// Generate a chunk of mono 16-bit PCM samples (LE) of `duration_ms`
    /// at `sample_rate`. Suitable for wrapping in a WAV header and serving
    /// over HTTP. The returned Vec is `(sample_rate * duration_ms / 1000)`
    /// frames × 2 bytes each.
    pub fn render_pcm_16le(&mut self, sample_rate: u32, duration_ms: u32) -> Vec<u8> {
        let n_samples = (sample_rate as u64 * duration_ms as u64 / 1000) as usize;
        let mut out = Vec::with_capacity(n_samples * 2);
        for _ in 0..n_samples {
            let s = self.next_sample();
            let i16_val = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
            out.extend_from_slice(&i16_val.to_le_bytes());
        }
        out
    }
}

fn db_to_linear(db: f32) -> f32 { 10f32.powf(db / 20.0) }

/// Convenience: render a chunk of masker audio directly as a complete
/// mono WAV (suitable for serving via chan_server).
pub fn render_wav(preset: MaskerPreset, sample_rate: u32, duration_ms: u32, gain_db: f32) -> Vec<u8> {
    let mut m = Masker::new(preset, sample_rate, gain_db, 0xCAFEBABE);
    let pcm = m.render_pcm_16le(sample_rate, duration_ms);
    wrap_mono_wav(&pcm, sample_rate, 16)
}

/// The frequency sanitizer: combines a shaped-noise masker with a
/// phase-tracked anti-tone generator. The user picks a preset (which sets
/// the noise color + EQ band) and optionally a fundamental frequency for
/// the anti-tones (the periodic rumble they want to cancel).
///
/// Mix balance: 1.0 noise + 0.7 anti-tones, clipped to [-1, 1]. The
/// anti-tone weight is lower because tonal energy is more attention-
/// grabbing than broadband, so a bit goes a long way.
pub struct Sanitizer {
    masker: Masker,
    anti: Option<crate::cancel_tones::AntiToneGen>,
}

impl Sanitizer {
    pub fn new(
        preset: MaskerPreset,
        sample_rate: u32,
        gain_db: f32,
        antitone_freq: Option<f32>,
    ) -> Self {
        let anti = antitone_freq.map(|hz| {
            crate::cancel_tones::AntiToneGen::new(sample_rate, hz, 4, true)
        });
        Self {
            masker: Masker::new(preset, sample_rate, gain_db, 0xCAFEBABE),
            anti,
        }
    }

    pub fn next_sample(&mut self) -> f32 {
        let n = self.masker.next_sample();
        let a = self.anti.as_mut().map(|g| g.next_sample() * 0.7).unwrap_or(0.0);
        (n + a).clamp(-1.0, 1.0)
    }

    pub fn render_pcm_16le(&mut self, sample_rate: u32, duration_ms: u32) -> Vec<u8> {
        let n_samples = (sample_rate as u64 * duration_ms as u64 / 1000) as usize;
        let mut out = Vec::with_capacity(n_samples * 2);
        for _ in 0..n_samples {
            let s = self.next_sample();
            let i16_val = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
            out.extend_from_slice(&i16_val.to_le_bytes());
        }
        out
    }

    /// Adjust the anti-tone fundamental on the fly (the masker is
    /// unaffected). Returns true if the change took effect, false if
    /// anti-tones aren't enabled on this sanitizer.
    pub fn set_antitone_frequency(&mut self, hz: f32) -> bool {
        match self.anti.as_mut() {
            Some(g) => { g.set_frequency(hz); true }
            None => false,
        }
    }
}

/// Convenience: render a chunk of sanitizer audio directly as a mono WAV.
pub fn render_sanitizer_wav(
    preset: MaskerPreset,
    sample_rate: u32,
    duration_ms: u32,
    gain_db: f32,
    antitone_freq: Option<f32>,
) -> Vec<u8> {
    let mut s = Sanitizer::new(preset, sample_rate, gain_db, antitone_freq);
    let pcm = s.render_pcm_16le(sample_rate, duration_ms);
    wrap_mono_wav(&pcm, sample_rate, 16)
}

fn wrap_mono_wav(samples: &[u8], sample_rate: u32, bits: u16) -> Vec<u8> {
    let byte_rate = sample_rate * (bits as u32 / 8);
    let block_align = bits / 8;
    let data_len = samples.len() as u32;
    let mut w = Vec::with_capacity(44 + samples.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVE");
    w.extend_from_slice(b"fmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());    // PCM
    w.extend_from_slice(&1u16.to_le_bytes());    // mono
    w.extend_from_slice(&sample_rate.to_le_bytes());
    w.extend_from_slice(&byte_rate.to_le_bytes());
    w.extend_from_slice(&block_align.to_le_bytes());
    w.extend_from_slice(&bits.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    w.extend_from_slice(samples);
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_ids_roundtrip() {
        for p in [MaskerPreset::Pink, MaskerPreset::Brown,
                  MaskerPreset::EngineDrone, MaskerPreset::RoadNoise,
                  MaskerPreset::FanHum, MaskerPreset::HvacDrone] {
            assert_eq!(MaskerPreset::from_id(p.id()), Some(p));
        }
        assert!(MaskerPreset::from_id("not-a-preset").is_none());
    }

    #[test]
    fn pink_noise_stays_in_range() {
        let mut p = PinkGen::new(42);
        for _ in 0..10_000 {
            let s = p.next_sample();
            assert!(s.is_finite(), "non-finite sample");
            assert!(s.abs() <= 1.1, "sample {} out of range", s);  // small headroom for transients
        }
    }

    #[test]
    fn brown_noise_drifts_but_stays_bounded() {
        let mut b = BrownGen::new(0xC0FFEE);
        for _ in 0..100_000 {
            let s = b.next_sample();
            assert!(s.is_finite());
            assert!(s.abs() <= 1.0, "brown noise unbounded: {}", s);
        }
    }

    #[test]
    fn deterministic_with_same_seed() {
        let mut a = PinkGen::new(1);
        let mut b = PinkGen::new(1);
        for _ in 0..1000 {
            assert_eq!(a.next_sample().to_bits(), b.next_sample().to_bits());
        }
    }

    #[test]
    fn peaking_eq_boosts_at_center_freq() {
        // Drive the EQ with a sinusoid at the target freq and compare RMS
        // out vs in. With +12 dB boost we expect ~4x amplitude.
        let sr = 48_000.0;
        let mut eq = PeakingEq::new(sr, 60.0, 1.0, 12.0);
        let mut rms_out = 0.0_f32;
        let mut rms_in = 0.0_f32;
        let n = 4800; // 100 ms
        // Skip the first 100 samples to let the filter settle.
        for i in 0..(n + 100) {
            let t = i as f32 / sr;
            let x = (2.0 * PI * 60.0 * t).sin();
            let y = eq.process(x);
            if i >= 100 {
                rms_in += x * x;
                rms_out += y * y;
            }
        }
        let ratio = (rms_out / rms_in).sqrt();
        assert!(ratio > 3.0 && ratio < 5.0,
                "expected ~4x boost at center, got {}", ratio);
    }

    #[test]
    fn render_wav_produces_valid_riff() {
        let wav = render_wav(MaskerPreset::Pink, 48_000, 100, -20.0);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        // mono, 48k, 16-bit, ~100ms = 4800 frames * 2 bytes = 9600 data bytes
        let data_start = wav.iter().position(|&b| b == b'd')
            .map(|p| if &wav[p..p+4] == b"data" { Some(p) } else { None })
            .flatten();
        assert!(data_start.is_some(), "no data chunk found");
    }

    #[test]
    fn gain_attenuates_amplitude() {
        // At -40 dB the output amplitude should be much smaller than at 0.
        let mut hot  = Masker::new(MaskerPreset::Pink, 48_000,   0.0, 1);
        let mut cold = Masker::new(MaskerPreset::Pink, 48_000, -40.0, 1);
        let mut hot_rms = 0.0_f32;
        let mut cold_rms = 0.0_f32;
        for _ in 0..5000 {
            let h = hot.next_sample();
            let c = cold.next_sample();
            hot_rms += h * h;
            cold_rms += c * c;
        }
        let ratio = (hot_rms / cold_rms).sqrt();
        // -40 dB is 100x linear; expect roughly that ratio (allowing for noise).
        assert!(ratio > 50.0,
                "expected gain attenuation ~100x, got {}", ratio);
    }

    #[test]
    fn engine_drone_preset_emphasizes_low_frequencies() {
        // Crude spectral check: render brown-based engine preset vs pink,
        // measure low-band energy via simple bandpass.
        let mut engine = Masker::new(MaskerPreset::EngineDrone, 48_000, 0.0, 1);
        let mut pink   = Masker::new(MaskerPreset::Pink,        48_000, 0.0, 1);
        let mut e_lp = 0.0_f32;  // engine low-pass running avg
        let mut p_lp = 0.0_f32;
        let alpha = 0.005;       // first-order LP, ~80 Hz cutoff @ 48k
        let mut e_rms = 0.0_f32;
        let mut p_rms = 0.0_f32;
        for _ in 0..10_000 {
            let e = engine.next_sample();
            let p = pink.next_sample();
            e_lp = e_lp * (1.0 - alpha) + e * alpha;
            p_lp = p_lp * (1.0 - alpha) + p * alpha;
            e_rms += e_lp * e_lp;
            p_rms += p_lp * p_lp;
        }
        // EngineDrone should have noticeably more low-freq energy than Pink.
        assert!(e_rms > p_rms,
                "engine preset should have more LF energy than pink: e={}, p={}",
                e_rms, p_rms);
    }
}
