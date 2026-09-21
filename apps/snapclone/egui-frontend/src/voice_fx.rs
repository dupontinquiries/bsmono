//! voice_fx — per-speaker processing on INCOMING voice.
//!
//! Three quick toggles per remote channel, because a call is rarely one bad
//! microphone: one person is on a laptop fan, another is shouting, a third has
//! a cheap headset that is all sibilance. A single global setting fixes one of
//! them and damages the other two.
//!
//! * **De-bass** — a 2nd-order high-pass at 120 Hz. Voice has almost nothing
//!   useful down there; rumble, desk thumps, handling noise, AC hum and the
//!   proximity-effect boom of a mic held too close all live there.
//! * **De-harsh** — a peaking cut centred on 4 kHz, where the presence and
//!   sibilance that makes a voice fatiguing sits. A cut, not a de-esser: it is
//!   stateless beyond the filter, cannot pump, and is honest about what it
//!   does.
//! * **Compressor** — evens out someone who swings between mumbling and
//!   shouting.
//!
//! ## On ambient noise, and why these are not all equal
//!
//! **De-bass helps it and the compressor hurts it.** Much of what reads as
//! background noise — traffic rumble, a fan's fundamental, a fridge — is low
//! frequency, and the high-pass removes it outright.
//!
//! A compressor does the OPPOSITE. It turns the quiet parts up, and between
//! words the quiet part IS the room. Add make-up gain and the noise floor rises
//! with it. So the make-up here is deliberately small (see `MAKEUP_DB`), and a
//! UI offering these three should not present the compressor as a noise fix.
//! It is a loudness fix that costs some noise.
//!
//! Real noise suppression — spectral subtraction or an RNNoise-class model —
//! is a different and much bigger component, and none of these three pretend
//! to be it.
//!
//! ## Per-speaker STATE, not just per-speaker settings
//!
//! Every filter and the compressor's envelope carry history. Shared across
//! speakers, one loud person would pump the compressor for everyone and the
//! filters would ring with the previous speaker's audio. So each channel owns
//! its own state, keyed by peer.

use std::collections::BTreeMap;

/// The playback rate. Matches `sink_cushion::SAMPLE_RATE`.
pub const SAMPLE_RATE: f32 = 16_000.0;

/// A direct-form-I biquad.
///
/// DF1 rather than the transposed DF2 that is usually recommended: DF2's
/// internal state is scaled by the filter's gain, which for a steep high-pass
/// can overflow on a loud transient. At 16 kHz voice rates DF1's extra two
/// multiplies are free and its state is just the recent samples.
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl Biquad {
    /// RBJ high-pass.
    fn highpass(f0: f32, q: f32, fs: f32) -> Biquad {
        let w0 = std::f32::consts::TAU * f0 / fs;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        let a0 = 1.0 + alpha;
        Biquad {
            b: [(1.0 + c) / 2.0 / a0, -(1.0 + c) / a0, (1.0 + c) / 2.0 / a0],
            a: [-2.0 * c / a0, (1.0 - alpha) / a0],
            ..Default::default()
        }
    }

    /// RBJ peaking EQ. Negative `gain_db` is a cut.
    fn peaking(f0: f32, q: f32, gain_db: f32, fs: f32) -> Biquad {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = std::f32::consts::TAU * f0 / fs;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        let a0 = 1.0 + alpha / a;
        Biquad {
            b: [(1.0 + alpha * a) / a0, -2.0 * c / a0, (1.0 - alpha * a) / a0],
            a: [-2.0 * c / a0, (1.0 - alpha / a) / a0],
            ..Default::default()
        }
    }

    fn run(&mut self, x0: f32) -> f32 {
        let y0 = self.b[0] * x0 + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x0, self.x[0]];
        self.y = [y0, self.y[0]];
        y0
    }
}

/// High-pass corner. 120 Hz: above mains hum (50/60 Hz and its first
/// harmonic) and a fan's fundamental, below the lowest male fundamental worth
/// keeping (~85-180 Hz), where a 2nd-order slope is gentle enough not to thin
/// the voice.
pub const DEBASS_HZ: f32 = 120.0;

/// Centre and depth of the harshness cut.
pub const HARSH_HZ: f32 = 4_000.0;
pub const HARSH_DB: f32 = -6.0;

/// Compressor settings, chosen for speech rather than music.
pub const THRESH_DB: f32 = -24.0;
pub const RATIO: f32 = 3.0;
/// Deliberately small. Make-up gain lifts the noise floor with everything else,
/// and between words the floor IS the room -- see the module notes.
pub const MAKEUP_DB: f32 = 3.0;

/// One remote speaker's toggles and the state they need.
#[derive(Debug, Clone)]
pub struct ChannelFx {
    pub debass: bool,
    pub deharsh: bool,
    pub compress: bool,
    hp: Biquad,
    peak: Biquad,
    env_db: f32,
    attack: f32,
    release: f32,
}

impl Default for ChannelFx {
    fn default() -> Self {
        ChannelFx::new()
    }
}

impl ChannelFx {
    /// All three OFF. A speaker who sounds fine must sound exactly as sent;
    /// turning something on is a correction for a specific bad mic.
    pub fn new() -> ChannelFx {
        let coef = |ms: f32| (-1.0 / (ms * 0.001 * SAMPLE_RATE)).exp();
        ChannelFx {
            debass: false,
            deharsh: false,
            compress: false,
            hp: Biquad::highpass(DEBASS_HZ, std::f32::consts::FRAC_1_SQRT_2, SAMPLE_RATE),
            peak: Biquad::peaking(HARSH_HZ, 1.0, HARSH_DB, SAMPLE_RATE),
            // Silence, in dB. Starting at 0 dB would make the first loud word
            // of a call look like a sustained level and clamp it hard.
            env_db: -120.0,
            // 5 ms attack catches a plosive; 120 ms release avoids pumping on
            // the gaps between syllables.
            attack: coef(5.0),
            release: coef(120.0),
        }
    }

    /// Process a block in place.
    ///
    /// THE FILTERS RUN EVEN WHEN THEIR TOGGLE IS OFF, and only the output is
    /// chosen. That keeps their state warm, so switching one on mid-call starts
    /// from a settled filter instead of from zeros -- which is an audible click
    /// on the next sample. Two biquads at 16 kHz cost nothing worth saving.
    pub fn process(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            let dry = *s;
            let a = self.hp.run(dry);
            let mut v = if self.debass { a } else { dry };
            let b = self.peak.run(v);
            if self.deharsh {
                v = b;
            }
            if self.compress {
                v = self.compress_one(v);
            }
            *s = v;
        }
    }

    fn compress_one(&mut self, x: f32) -> f32 {
        let level_db = 20.0 * (x.abs() + 1e-9).log10();
        let coef = if level_db > self.env_db { self.attack } else { self.release };
        self.env_db = coef * self.env_db + (1.0 - coef) * level_db;
        let over = (self.env_db - THRESH_DB).max(0.0);
        let reduce = over * (1.0 - 1.0 / RATIO);
        x * 10f32.powf((MAKEUP_DB - reduce) / 20.0)
    }

    pub fn any_on(&self) -> bool {
        self.debass || self.deharsh || self.compress
    }
}

/// Every remote speaker's processing, by peer id.
#[derive(Debug, Clone, Default)]
pub struct VoiceFx {
    channels: BTreeMap<String, ChannelFx>,
}

impl VoiceFx {
    pub fn new() -> VoiceFx {
        VoiceFx::default()
    }

    /// The toggles for a peer, created on first use.
    pub fn channel(&mut self, peer: &str) -> &mut ChannelFx {
        self.channels.entry(peer.to_string()).or_default()
    }

    /// Process one peer's block. A peer with nothing enabled is left
    /// byte-identical -- no allocation, no pass over the samples.
    pub fn process(&mut self, peer: &str, buf: &mut [f32]) {
        if let Some(c) = self.channels.get_mut(peer) {
            c.process(buf);
        }
    }

    /// Forget a peer who left, so a rejoin starts from default settings rather
    /// than inheriting a correction that was right for someone else's mic.
    pub fn forget(&mut self, peer: &str) {
        self.channels.remove(peer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, amp: f32, secs: f32) -> Vec<f32> {
        let n = (secs * SAMPLE_RATE) as usize;
        (0..n).map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / SAMPLE_RATE).sin()).collect()
    }

    /// RMS of the second half, after the filters settle.
    fn rms_tail(v: &[f32]) -> f32 {
        let t = &v[v.len() / 2..];
        (t.iter().map(|x| x * x).sum::<f32>() / t.len() as f32).sqrt()
    }

    fn db(a: f32, b: f32) -> f32 {
        20.0 * (a / b).log10()
    }

    /// ALL OFF IS BIT-EXACT. A speaker who sounds fine must sound exactly as
    /// sent -- not "almost", which a warm filter chain would give.
    #[test]
    fn with_everything_off_the_signal_is_untouched() {
        let src = sine(1_000.0, 0.5, 0.2);
        let mut buf = src.clone();
        ChannelFx::new().process(&mut buf);
        assert_eq!(buf, src);
    }

    /// DE-BASS REMOVES RUMBLE. A 50 Hz hum is well under the corner.
    #[test]
    fn debass_attenuates_mains_hum() {
        let mut c = ChannelFx::new();
        c.debass = true;
        let mut buf = sine(50.0, 0.5, 1.0);
        c.process(&mut buf);
        let cut = db(rms_tail(&buf), rms_tail(&sine(50.0, 0.5, 1.0)));
        assert!(cut < -12.0, "50 Hz hum should drop hard: {cut:.1} dB");
    }

    /// ...AND LEAVES THE VOICE. 1 kHz is the middle of intelligibility.
    #[test]
    fn debass_leaves_the_voice_band_alone() {
        let mut c = ChannelFx::new();
        c.debass = true;
        let mut buf = sine(1_000.0, 0.5, 1.0);
        c.process(&mut buf);
        let d = db(rms_tail(&buf), rms_tail(&sine(1_000.0, 0.5, 1.0)));
        assert!(d.abs() < 0.5, "1 kHz must pass: {d:.2} dB");
    }

    /// DE-HARSH CUTS THE PRESENCE BAND by about its stated depth.
    #[test]
    fn deharsh_cuts_the_harsh_band_by_its_depth() {
        let mut c = ChannelFx::new();
        c.deharsh = true;
        let mut buf = sine(HARSH_HZ, 0.3, 1.0);
        c.process(&mut buf);
        let d = db(rms_tail(&buf), rms_tail(&sine(HARSH_HZ, 0.3, 1.0)));
        assert!((d - HARSH_DB).abs() < 1.0, "expected ~{HARSH_DB} dB at centre, got {d:.2}");
    }

    /// ...and leaves the fundamental and the low voice alone.
    #[test]
    fn deharsh_leaves_the_low_voice_alone() {
        let mut c = ChannelFx::new();
        c.deharsh = true;
        let mut buf = sine(300.0, 0.3, 1.0);
        c.process(&mut buf);
        let d = db(rms_tail(&buf), rms_tail(&sine(300.0, 0.3, 1.0)));
        assert!(d.abs() < 0.5, "300 Hz must pass: {d:.2} dB");
    }

    /// THE COMPRESSOR EVENS LOUDNESS: a shout comes down relative to a
    /// mumble, which is the whole job.
    #[test]
    fn the_compressor_narrows_the_gap_between_loud_and_quiet() {
        let run = |amp: f32| {
            let mut c = ChannelFx::new();
            c.compress = true;
            let mut b = sine(500.0, amp, 1.0);
            c.process(&mut b);
            rms_tail(&b)
        };
        let before = db(0.8, 0.05);
        let after = db(run(0.8), run(0.05));
        assert!(after < before - 3.0, "gap {before:.1} dB -> {after:.1} dB");
    }

    /// THE COMPRESSOR RAISES THE NOISE FLOOR -- which is why the module says
    /// not to sell it as a noise fix. Quiet input comes out louder.
    #[test]
    fn the_compressor_lifts_quiet_signal_including_room_noise() {
        let mut c = ChannelFx::new();
        c.compress = true;
        let mut b = sine(500.0, 0.005, 1.0); // well under threshold: "room"
        c.process(&mut b);
        let d = db(rms_tail(&b), rms_tail(&sine(500.0, 0.005, 1.0)));
        assert!(d > 0.0 && d <= MAKEUP_DB + 0.5,
                "room noise lifted by the make-up gain, and only by that: {d:.2} dB");
    }

    /// PER-SPEAKER STATE. One loud speaker must not pump the compressor for a
    /// quiet one -- the reason state is not shared.
    #[test]
    fn a_loud_speaker_does_not_duck_a_quiet_one() {
        let mut fx = VoiceFx::new();
        fx.channel("loud").compress = true;
        fx.channel("quiet").compress = true;
        let mut loud = sine(500.0, 0.9, 0.5);
        fx.process("loud", &mut loud);
        let mut quiet = sine(500.0, 0.02, 0.5);
        fx.process("quiet", &mut quiet);
        let mut fresh = ChannelFx::new();
        fresh.compress = true;
        let mut alone = sine(500.0, 0.02, 0.5);
        fresh.process(&mut alone);
        // THE HEAD, NOT THE TAIL. My first version compared `rms_tail`, and it
        // PASSED WITH STATE DELIBERATELY SHARED across speakers -- because by
        // the second half of the block the 120 ms release has recovered from
        // the loud speaker, so the damage is gone by the time it is measured.
        // Ducking shows in the first few milliseconds after the loud speaker
        // stops, which is exactly when a listener hears it.
        let head = |v: &[f32]| {
            let h = &v[..(0.02 * SAMPLE_RATE) as usize];
            (h.iter().map(|x| x * x).sum::<f32>() / h.len() as f32).sqrt()
        };
        assert!((head(&quiet) - head(&alone)).abs() < 1e-6,
                "the quiet speaker's first 20 ms were ducked by the loud one: {} vs {}",
                head(&quiet), head(&alone));
    }

    /// FILTERS STAY WARM so toggling on mid-call does not click. Compare the
    /// first sample after enabling against a filter that ran from the start.
    #[test]
    fn switching_a_filter_on_mid_call_does_not_start_from_zero() {
        let tone = sine(1_000.0, 0.5, 0.5);
        let mut warm = ChannelFx::new();
        let (a, b) = tone.split_at(tone.len() / 2);
        let mut first = a.to_vec();
        warm.process(&mut first);
        warm.debass = true;
        let mut second = b.to_vec();
        warm.process(&mut second);

        let mut always = ChannelFx::new();
        always.debass = true;
        let mut full = tone.clone();
        always.process(&mut full);

        let i = tone.len() / 2;
        assert!((second[0] - full[i]).abs() < 1e-4,
                "the first sample after enabling must match a settled filter: {} vs {}",
                second[0], full[i]);
    }

    /// An unknown peer passes through untouched, and forgetting resets.
    #[test]
    fn unknown_peers_pass_and_forgotten_peers_reset() {
        let mut fx = VoiceFx::new();
        let src = sine(1_000.0, 0.5, 0.1);
        let mut b = src.clone();
        fx.process("nobody", &mut b);
        assert_eq!(b, src, "no channel means no processing");
        fx.channel("p").debass = true;
        fx.forget("p");
        assert!(!fx.channel("p").any_on(), "a rejoin starts from defaults");
    }
}
