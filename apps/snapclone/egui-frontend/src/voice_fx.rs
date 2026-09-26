//! voice_fx — per-speaker processing on INCOMING voice.
//!
//! Two jobs, both done per remote contact and saved against them:
//!
//! 1. **Protect the listener from loudness they did not choose.** The common
//!    pattern is a speaker who maxes their own input gain so they never fall
//!    short of headroom, then raises their voice over whatever their room is
//!    doing. On your device that arrives as someone who is suddenly far too
//!    loud. See [`Leveler`].
//! 2. **Fix a specific bad microphone** with a small, visual EQ: two wide bands
//!    for broad tone and two narrow ones for a single problem frequency. See
//!    [`Eq4`].
//!
//! ## The leveler only ever turns things DOWN
//!
//! That is the whole design, and it is what the first version got wrong. A
//! conventional compressor with make-up gain lifts the quiet parts, and between
//! words the quiet part is the SPEAKER'S ROOM — so it raised everyone's
//! background noise to make them more even. For "stop people being too loud on
//! my device" that is backwards. Downward-only means the noise floor can never
//! rise, and a well-behaved speaker passes untouched.
//!
//! ## What makes it elegant rather than just a limiter
//!
//! A fixed threshold and ratio flattens a maxed-out speaker into a wall — it
//! reads as "fidelity" only in the sense that it stops the clipping. What reads
//! as elegance is adapting to the speaker: track how loud THEY habitually are,
//! and bring them to a comfortable listening level with a slow, broad gain,
//! leaving a fast limiter only for genuine peaks. See [`Leveler`] for the two
//! stages and the one rule that makes the slow stage work.

use std::collections::BTreeMap;

/// The playback rate. Matches `sink_cushion::SAMPLE_RATE`.
pub const SAMPLE_RATE: f32 = 16_000.0;

/// A direct-form-I biquad.
///
/// DF1 rather than transposed DF2: DF2's internal state is scaled by the
/// filter's gain, and a narrow high-Q boost can overflow it on a transient. At
/// 16 kHz the extra multiplies are free and DF1's state is just recent samples.
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl Biquad {
    /// RBJ peaking EQ. Negative `gain_db` is a cut. At 0 dB it is exactly
    /// unity -- b and a collapse to the same polynomial.
    fn peaking(f0: f32, q: f32, gain_db: f32) -> Biquad {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = std::f32::consts::TAU * f0.clamp(20.0, SAMPLE_RATE * 0.45) / SAMPLE_RATE;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha / a;
        Biquad {
            b: [(1.0 + alpha * a) / a0, -2.0 * c / a0, (1.0 - alpha * a) / a0],
            a: [-2.0 * c / a0, (1.0 - alpha / a) / a0],
            ..Default::default()
        }
    }

    /// Change the coefficients WITHOUT clearing the history.
    ///
    /// Dragging a band in the panel re-designs the filter every frame. Clearing
    /// the state each time would click on every mouse move; keeping it lets the
    /// response slide under the audio.
    fn retune(&mut self, next: Biquad) {
        self.b = next.b;
        self.a = next.a;
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

// ─────────────────────────────── EQ ───────────────────────────────

/// One EQ band's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

/// Q for a wide band -- broad tone, about two octaves.
pub const WIDE_Q: f32 = 0.7;
/// Q for a narrow band -- one problem frequency, about a third of an octave.
pub const NARROW_Q: f32 = 4.3;
/// Largest boost or cut a band allows.
///
/// ±12 dB. Enough to fix a bad mic, not enough to build a feedback-prone peak
/// out of a narrow boost -- which is the one way an EQ panel can hurt the
/// listener it exists to serve.
pub const MAX_GAIN_DB: f32 = 12.0;

/// Two wide bands and two narrow ones.
///
/// Wide for "this person sounds boomy" or "thin"; narrow for "there is a
/// whistle at 2.4 kHz". Four is deliberately few: it fits a phone panel with
/// room to drag, and more bands than problems is how an EQ becomes a place to
/// fiddle rather than fix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Eq4 {
    pub bands: [Band; 4],
}

impl Default for Eq4 {
    /// FLAT. Every band at 0 dB, which is exactly unity, so a new contact
    /// sounds precisely as they sent.
    fn default() -> Self {
        Eq4 {
            bands: [
                Band { hz: 180.0, gain_db: 0.0, q: WIDE_Q },   // wide: body / boom
                Band { hz: 3_000.0, gain_db: 0.0, q: WIDE_Q }, // wide: presence
                Band { hz: 1_000.0, gain_db: 0.0, q: NARROW_Q }, // narrow
                Band { hz: 5_000.0, gain_db: 0.0, q: NARROW_Q }, // narrow
            ],
        }
    }
}

impl Eq4 {
    /// Is every band at 0 dB?
    ///
    /// Used to skip the filters entirely, so an untouched contact costs nothing
    /// and is bit-exact rather than "flat to within rounding".
    pub fn is_flat(&self) -> bool {
        self.bands.iter().all(|b| b.gain_db.abs() < 1e-3)
    }

    /// Clamp every band into the range the panel allows.
    ///
    /// Applied on load as well as on edit: a saved file is input, and one
    /// edited by hand or written by an older build must not smuggle in a
    /// +40 dB narrow boost.
    pub fn clamped(mut self) -> Eq4 {
        for (i, b) in self.bands.iter_mut().enumerate() {
            b.hz = b.hz.clamp(40.0, 7_000.0);
            b.gain_db = b.gain_db.clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
            // A band's CHARACTER is fixed: wide stays wide, narrow narrow. The
            // panel has two of each, and a narrow band dragged to Q 0.7 would
            // silently become a third wide one.
            b.q = if i < 2 { WIDE_Q } else { NARROW_Q };
        }
        self
    }

    /// Magnitude response in dB at `hz`, for drawing the curve.
    ///
    /// Computed from the same coefficients the audio uses, so the line on
    /// screen cannot disagree with what the listener hears.
    pub fn response_db(&self, hz: f32) -> f32 {
        let w = std::f32::consts::TAU * hz / SAMPLE_RATE;
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let mut total = 0.0;
        for band in &self.bands {
            let f = Biquad::peaking(band.hz, band.q, band.gain_db);
            let nr = f.b[0] + f.b[1] * c1 + f.b[2] * c2;
            let ni = -(f.b[1] * s1 + f.b[2] * s2);
            let dr = 1.0 + f.a[0] * c1 + f.a[1] * c2;
            let di = -(f.a[0] * s1 + f.a[1] * s2);
            let mag = ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt();
            total += 20.0 * mag.max(1e-9).log10();
        }
        total
    }

    /// Serialise for the per-contact store: one line, fixed field order.
    pub fn to_line(&self) -> String {
        self.bands
            .iter()
            .map(|b| format!("{:.1},{:.2}", b.hz, b.gain_db))
            .collect::<Vec<_>>()
            .join(";")
    }

    /// Parse a line written by [`Eq4::to_line`].
    ///
    /// Anything malformed yields FLAT rather than an error. A contact whose
    /// saved EQ cannot be read should sound as they sent, not fail to play.
    pub fn from_line(line: &str) -> Eq4 {
        let mut eq = Eq4::default();
        let parts: Vec<&str> = line.trim().split(';').collect();
        if parts.len() != 4 {
            return eq;
        }
        for (i, p) in parts.iter().enumerate() {
            let mut kv = p.split(',');
            let (Some(h), Some(g)) = (kv.next(), kv.next()) else { return Eq4::default() };
            let (Ok(hz), Ok(gain_db)) = (h.trim().parse::<f32>(), g.trim().parse::<f32>()) else {
                return Eq4::default();
            };
            if !hz.is_finite() || !gain_db.is_finite() {
                return Eq4::default();
            }
            eq.bands[i].hz = hz;
            eq.bands[i].gain_db = gain_db;
        }
        eq.clamped()
    }
}

// ───────────────────────────── LEVELER ─────────────────────────────

/// Comfortable listening level for speech, in dBFS.
pub const TARGET_DB: f32 = -20.0;

/// The hard ceiling for peaks, in dBFS. Nothing leaves the leveler above this.
pub const CEILING_DB: f32 = -3.0;

/// Below this a frame is not speech and does not move the loudness estimate.
pub const SPEECH_GATE_DB: f32 = -45.0;

/// Protects the listener from a speaker who is too loud. Two stages:
///
/// * **Slow broad gain.** Tracks how loud this speaker habitually is over a few
///   seconds and applies a gentle reduction to bring them to `TARGET_DB`. This
///   is the part that handles "maxed their input": a speaker sitting at -8 dBFS
///   all call gets about -12 dB, applied so slowly it reads as their natural
///   level rather than as processing.
/// * **Fast peak limiter.** A few-millisecond reduction above `CEILING_DB`, for
///   the shout the slow stage has not caught up with yet.
///
/// NEITHER STAGE EVER BOOSTS. Both gains are clamped to <= 0 dB.
///
/// ## The rule that makes the slow stage work: gate the estimate on speech
///
/// If silence fed the loudness estimate, every pause would drag it down, the
/// estimate would read the speaker as quieter than they are, and the moment
/// they spoke again they would be under-reduced -- too loud for exactly as long
/// as it took to re-learn. So the estimate only moves on frames above
/// `SPEECH_GATE_DB`, and during silence it holds.
#[derive(Debug, Clone)]
pub struct Leveler {
    loud_db: f32,
    peak_env: f32,
    slow: f32,
    peak_release: f32,
}

impl Default for Leveler {
    fn default() -> Self {
        Leveler::new()
    }
}

impl Leveler {
    pub fn new() -> Leveler {
        let coef = |ms: f32| (-1.0 / (ms * 0.001 * SAMPLE_RATE)).exp();
        Leveler {
            // Start AT the target, so a new speaker is neither boosted nor cut
            // until there is evidence about them.
            loud_db: TARGET_DB,
            peak_env: 0.0,
            // ~2 s: long enough to be a habit rather than a word.
            slow: coef(2_000.0),
            peak_release: coef(80.0),
        }
    }

    /// The slow stage's current gain in dB. Never positive.
    pub fn broad_gain_db(&self) -> f32 {
        (TARGET_DB - self.loud_db).min(0.0)
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            let x = *s;
            let level_db = 20.0 * (x.abs() + 1e-9).log10();
            // Speech-gated loudness estimate -- see the type docs.
            if level_db > SPEECH_GATE_DB {
                self.loud_db = self.slow * self.loud_db + (1.0 - self.slow) * level_db;
            }
            let broad = 10f32.powf(self.broad_gain_db() / 20.0);
            let y = x * broad;
            // Fast limiter on what the slow stage let through.
            //
            // INSTANT ATTACK. The first version used a 2 ms attack and let 57
            // samples through above the ceiling on a loud tone. An attack-
            // smoothed envelope follows the AVERAGE rectified level -- about
            // 0.64 of the peak for a sine -- so by the time it reaches the
            // ceiling the actual peaks are ~1.57x above it. A limiter's whole
            // promise is a ceiling, and any smoothing on the attack breaks it.
            //
            // Taking the max makes the envelope never less than the current
            // sample, so `ceiling / env` provably holds |out| <= ceiling. The
            // release still smooths, which is where limiting would otherwise
            // be audible as pumping.
            let a = y.abs();
            self.peak_env = a.max(self.peak_env * self.peak_release);
            let ceiling = 10f32.powf(CEILING_DB / 20.0);
            let limit = if self.peak_env > ceiling { ceiling / self.peak_env } else { 1.0 };
            *s = y * limit.min(1.0);
        }
    }
}

// ───────────────────────────── CHANNEL ─────────────────────────────

/// One remote contact's settings and the state they need.
#[derive(Debug, Clone, Default)]
pub struct ChannelFx {
    pub eq: Eq4,
    pub level: bool,
    filters: [Biquad; 4],
    tuned: Option<Eq4>,
    leveler: Leveler,
    /// The app-wide speech boost (Settings > Camera > Accessibility), added on
    /// top of this contact's own EQ. Separate from `eq` so the per-contact store
    /// never saves a global setting as a personal one.
    boost: Boost,
}

/// The speech-boost setting: a presence lift and a boom cut, applied to every
/// incoming voice. `auto_only` applies it only to frames loud enough to be
/// speech (`SPEECH_GATE_DB`), so a quiet line's hiss is not boosted with it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Boost {
    /// dB added to the presence band (3 kHz) and removed from the body band
    /// (180 Hz, at half the amount).
    pub presence_db: f32,
    pub auto_only: bool,
}

impl Boost {
    pub const OFF: Boost = Boost { presence_db: 0.0, auto_only: false };
    /// From the setting's key: off / mild / strong.
    pub fn from_setting(key: &str, auto_only: bool) -> Boost {
        let presence_db = match key { "mild" => 4.0, "strong" => 8.0, _ => 0.0 };
        Boost { presence_db, auto_only }
    }
    /// The EQ actually run: `base` with the boost folded into its bands.
    fn apply(self, base: &Eq4) -> Eq4 {
        let mut e = *base;
        e.bands[0].gain_db -= self.presence_db / 2.0;
        e.bands[1].gain_db += self.presence_db;
        e
    }
}

impl ChannelFx {
    /// Flat EQ and leveling ON.
    ///
    /// Leveling defaults on because it only ever reduces: a speaker at a normal
    /// level passes untouched, so it costs nothing for anyone who was already
    /// fine and protects against the one who is not.
    pub fn new() -> ChannelFx {
        ChannelFx { level: true, ..Default::default() }
    }

    /// How far the leveler is currently turning this contact down, in dB.
    /// Never positive.
    ///
    /// Worth showing next to a contact's name: "this person is being turned
    /// down 9 dB" tells the listener their level is being managed, and tells
    /// them WHY someone sounds quieter than they expected.
    pub fn level_reduction_db(&self) -> f32 {
        if self.level { self.leveler.broad_gain_db() } else { 0.0 }
    }

    /// Re-design the filters if the EQ changed since the last block, keeping
    /// their history so a drag in the panel does not click.
    fn sync(&mut self, target: Eq4) {
        if self.tuned == Some(target) {
            return;
        }
        for (f, band) in self.filters.iter_mut().zip(target.bands.iter()) {
            f.retune(Biquad::peaking(band.hz, band.q, band.gain_db));
        }
        self.tuned = Some(target);
    }

    pub fn set_boost(&mut self, b: Boost) { self.boost = b; }

    pub fn process(&mut self, buf: &mut [f32]) {
        // The boost applies to this block unless it is auto-only and the block
        // is below the speech gate.
        let speech = {
            let ms = buf.iter().map(|s| s * s).sum::<f32>() / buf.len().max(1) as f32;
            10.0 * ms.max(1e-12).log10() >= SPEECH_GATE_DB
        };
        let target = if self.boost.presence_db != 0.0 && (!self.boost.auto_only || speech) {
            self.boost.apply(&self.eq)
        } else {
            self.eq
        };
        if !target.is_flat() {
            self.sync(target);
            for s in buf.iter_mut() {
                let mut v = *s;
                for f in self.filters.iter_mut() {
                    v = f.run(v);
                }
                *s = v;
            }
        }
        if self.level {
            self.leveler.process(buf);
        }
    }
}

/// Every remote contact's processing, with their saved settings.
#[derive(Debug, Clone, Default)]
pub struct VoiceFx {
    channels: BTreeMap<String, ChannelFx>,
    boost: Boost,
}

impl VoiceFx {
    pub fn new() -> VoiceFx {
        VoiceFx::default()
    }

    pub fn channel(&mut self, contact: &str) -> &mut ChannelFx {
        let boost = self.boost;
        self.channels.entry(contact.to_string()).or_insert_with(|| {
            let mut c = ChannelFx::new();
            c.set_boost(boost);
            c
        })
    }

    /// Apply the speech-boost setting to every contact, current and future.
    pub fn set_boost(&mut self, b: Boost) {
        self.boost = b;
        for c in self.channels.values_mut() {
            c.set_boost(b);
        }
    }

    pub fn process(&mut self, contact: &str, buf: &mut [f32]) {
        self.channel(contact).process(buf);
    }

    /// Settings for every contact, one line each, for the per-contact store.
    ///
    /// Only contacts whose settings differ from the default are written, so the
    /// file holds decisions a person made rather than a line per everyone ever
    /// heard. Keyed by contact id, not display name: names change and collide.
    pub fn save(&self) -> String {
        let mut out = String::new();
        for (id, c) in &self.channels {
            if c.eq.is_flat() && c.level {
                continue;
            }
            out.push_str(&format!("{}\t{}\t{}\n", id, c.level as u8, c.eq.to_line()));
        }
        out
    }

    /// Restore saved settings. Unreadable lines are skipped, not fatal: one bad
    /// entry must not lose every other contact's EQ.
    pub fn load(text: &str) -> VoiceFx {
        let mut fx = VoiceFx::new();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() != 3 || f[0].trim().is_empty() {
                continue;
            }
            let c = fx.channel(f[0].trim());
            c.level = f[1].trim() != "0";
            c.eq = Eq4::from_line(f[2]);
        }
        fx
    }
}

#[cfg(test)]
mod tests {
    /// The boost lifts the presence band of incoming voice, only when on, for
    /// every contact (new ones too), never into the per-contact store, and --
    /// auto-only -- not on frames below the speech gate.
    #[test]
    fn speech_boost_lifts_presence_only_when_on() {
        let tone = |hz: f32, amp: f32| -> Vec<f32> {
            (0..4800).map(|i| (i as f32 / SAMPLE_RATE * hz * std::f32::consts::TAU).sin() * amp).collect()
        };
        let rms = |x: &[f32]| (x[2400..].iter().map(|s| s * s).sum::<f32>() / 2400.0).sqrt();
        let run = |fx: &mut VoiceFx, who: &str, x: &[f32]| { let mut b = x.to_vec(); fx.process(who, &mut b); rms(&b) };
        let mut fx = VoiceFx::new();
        fx.channel("a").level = false;
        let loud = tone(3_000.0, 0.1);
        let flat = run(&mut fx, "a", &loud);
        assert!((flat / rms(&loud) - 1.0).abs() < 0.01, "boost off must be unity");
        fx.set_boost(Boost::from_setting("strong", false));
        let boosted = run(&mut fx, "a", &loud);
        assert!(boosted / flat > 1.8, "strong boost at 3 kHz: {:.2}x", boosted / flat);
        // A contact first heard AFTER the setting gets it too.
        fx.channel("b").level = false;
        assert!(run(&mut fx, "b", &loud) / flat > 1.8);
        // Auto-only: a quiet (non-speech) block is not boosted.
        fx.set_boost(Boost::from_setting("strong", true));
        let hiss = tone(3_000.0, 0.001);
        let q = run(&mut fx, "a", &hiss);
        assert!((q / rms(&hiss) - 1.0).abs() < 0.05, "auto-only boosted a sub-gate block: {:.2}x", q / rms(&hiss));
        // The global setting is not saved as anyone's personal EQ.
        assert!(fx.channel("a").eq.is_flat(), "the boost was written into the contact's own EQ");
        let _ = run(&mut fx, "c", &loud);
        assert!(!fx.save().contains("c\t"), "an untouched contact was saved because of the global boost: {}", fx.save());
    }

    use super::*;

    fn sine(hz: f32, amp: f32, secs: f32) -> Vec<f32> {
        let n = (secs * SAMPLE_RATE) as usize;
        (0..n).map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / SAMPLE_RATE).sin()).collect()
    }
    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }
    fn tail(v: &[f32]) -> &[f32] {
        &v[v.len() / 2..]
    }
    fn dbfs(v: &[f32]) -> f32 {
        20.0 * rms(v).max(1e-9).log10()
    }

    // ── leveler ──

    /// THE PURPOSE. A speaker who maxed their input is brought toward the
    /// listening level instead of arriving far too loud.
    #[test]
    fn a_maxed_out_speaker_is_brought_down() {
        let mut l = Leveler::new();
        let mut b = sine(500.0, 0.6, 6.0); // ~ -7 dBFS: maxed and shouting
        l.process(&mut b);
        let out = dbfs(tail(&b));
        assert!(out < -15.0, "should approach {TARGET_DB} dBFS, got {out:.1}");
    }

    /// NEVER BOOSTS. The first version's make-up gain raised the speaker's room
    /// noise; downward-only cannot.
    #[test]
    fn a_quiet_speaker_and_their_room_are_never_lifted() {
        for amp in [0.001f32, 0.01, 0.05] {
            let mut l = Leveler::new();
            let src = sine(500.0, amp, 4.0);
            let mut b = src.clone();
            l.process(&mut b);
            assert!(rms(tail(&b)) <= rms(tail(&src)) * 1.0001,
                    "amp {amp} was boosted: {} -> {}", rms(tail(&src)), rms(tail(&b)));
            assert!(l.broad_gain_db() <= 0.0);
        }
    }

    /// A speaker already at a comfortable level passes essentially untouched --
    /// which is why leveling can default on.
    #[test]
    fn a_normal_speaker_is_left_alone() {
        let mut l = Leveler::new();
        let amp = 10f32.powf(TARGET_DB / 20.0) * std::f32::consts::SQRT_2;
        let src = sine(500.0, amp, 4.0);
        let mut b = src.clone();
        l.process(&mut b);
        let d = dbfs(tail(&b)) - dbfs(tail(&src));
        assert!(d.abs() < 1.5, "a speaker at target changed by {d:.2} dB");
    }

    /// THE GATE. Silence must not drag the estimate down; if it did, the next
    /// word after a pause would be under-reduced -- too loud until re-learned.
    #[test]
    fn a_pause_does_not_make_the_next_shout_too_loud() {
        let mut l = Leveler::new();
        let mut loud = sine(500.0, 0.6, 5.0);
        l.process(&mut loud);
        let learned = l.broad_gain_db();
        let mut silence = vec![0.0f32; (3.0 * SAMPLE_RATE) as usize];
        l.process(&mut silence);
        assert!((l.broad_gain_db() - learned).abs() < 0.5,
                "3 s of silence moved the reduction {learned:.1} -> {:.1}", l.broad_gain_db());
    }

    /// Peaks are held under the ceiling even before the slow stage adapts.
    #[test]
    fn a_sudden_peak_is_held_under_the_ceiling() {
        let mut l = Leveler::new();
        let mut b = sine(500.0, 0.99, 0.3);
        l.process(&mut b);
        let ceiling = 10f32.powf(CEILING_DB / 20.0);
        let over = b[(0.05 * SAMPLE_RATE) as usize..].iter().filter(|s| s.abs() > ceiling * 1.05).count();
        assert_eq!(over, 0, "{over} samples escaped the ceiling after the attack");
    }

    // ── EQ ──

    /// FLAT IS BIT-EXACT and skipped entirely, so an untouched contact costs
    /// nothing and sounds precisely as sent.
    #[test]
    fn a_flat_eq_leaves_the_signal_untouched() {
        let mut c = ChannelFx::new();
        c.level = false;
        let src = sine(1_000.0, 0.3, 0.2);
        let mut b = src.clone();
        c.process(&mut b);
        assert_eq!(b, src);
    }

    /// A narrow cut removes its frequency and leaves a neighbour an octave
    /// away nearly alone -- the difference between a narrow and a wide band.
    #[test]
    fn a_narrow_band_is_narrow() {
        let mut eq = Eq4::default();
        eq.bands[2] = Band { hz: 2_400.0, gain_db: -12.0, q: NARROW_Q };
        assert!(eq.response_db(2_400.0) < -10.0, "{}", eq.response_db(2_400.0));
        assert!(eq.response_db(1_200.0).abs() < 1.0, "an octave down: {}", eq.response_db(1_200.0));
    }

    /// A wide band reaches a long way: an octave out it is still doing real work.
    #[test]
    fn a_wide_band_is_wide() {
        let mut eq = Eq4::default();
        eq.bands[0] = Band { hz: 200.0, gain_db: -6.0, q: WIDE_Q };
        assert!(eq.response_db(400.0) < -1.5, "an octave up: {}", eq.response_db(400.0));
    }

    /// THE CURVE ON SCREEN IS THE CURVE YOU HEAR: `response_db` comes from the
    /// same coefficients as the audio. Measured against a real tone.
    #[test]
    fn the_drawn_response_matches_the_processed_audio() {
        let mut c = ChannelFx::new();
        c.level = false;
        c.eq.bands[1] = Band { hz: 3_000.0, gain_db: 6.0, q: WIDE_Q };
        let src = sine(3_000.0, 0.2, 1.0);
        let mut b = src.clone();
        c.process(&mut b);
        let heard = dbfs(tail(&b)) - dbfs(tail(&src));
        let drawn = c.eq.response_db(3_000.0);
        assert!((heard - drawn).abs() < 0.5, "drawn {drawn:.2} vs heard {heard:.2}");
    }

    /// A band's character is fixed: wide stays wide. A narrow band dragged to
    /// low Q would silently become a third wide one.
    #[test]
    fn clamping_keeps_two_wide_and_two_narrow() {
        let mut eq = Eq4::default();
        eq.bands[3].q = 0.5;
        eq.bands[0].gain_db = 40.0;
        let c = eq.clamped();
        assert_eq!(c.bands[3].q, NARROW_Q);
        assert_eq!(c.bands[0].gain_db, MAX_GAIN_DB, "no +40 dB narrow peak");
    }

    // ── per-contact persistence ──

    /// SAVED PER CONTACT and restored exactly.
    #[test]
    fn a_contacts_eq_survives_a_save_and_load() {
        let mut fx = VoiceFx::new();
        let c = fx.channel("sc-alice");
        c.eq.bands[2] = Band { hz: 2_400.0, gain_db: -9.5, q: NARROW_Q };
        c.level = false;
        let back = VoiceFx::load(&fx.save());
        let mut back = back;
        let r = back.channel("sc-alice");
        assert!((r.eq.bands[2].hz - 2_400.0).abs() < 0.1);
        assert!((r.eq.bands[2].gain_db + 9.5).abs() < 0.01);
        assert!(!r.level);
    }

    /// Only DECISIONS are written, not a line per everyone ever heard.
    #[test]
    fn default_contacts_are_not_written() {
        let mut fx = VoiceFx::new();
        fx.channel("sc-untouched");
        assert!(fx.save().is_empty(), "{:?}", fx.save());
    }

    /// A saved file is input. A hand-edited +40 dB must be clamped, and garbage
    /// must fall back to flat rather than fail to play.
    #[test]
    fn a_hostile_or_broken_save_is_made_safe() {
        let mut fx = VoiceFx::load("sc-bob\t1\t1000,40;3000,0;1000,0;5000,0\n");
        assert_eq!(fx.channel("sc-bob").eq.bands[0].gain_db, MAX_GAIN_DB);
        let mut fx = VoiceFx::load("sc-carl\t1\tgarbage\nsc-dee\t1\t1000,NaN;1,1;1,1;1,1\n");
        assert!(fx.channel("sc-carl").eq.is_flat());
        assert!(fx.channel("sc-dee").eq.is_flat(), "NaN must not reach a filter");
    }

    /// One bad line must not lose every other contact.
    #[test]
    fn a_bad_line_does_not_lose_the_others() {
        let text = "garbage line\nsc-good\t0\t180,3;3000,0;1000,0;5000,0\n";
        let mut fx = VoiceFx::load(text);
        assert!((fx.channel("sc-good").eq.bands[0].gain_db - 3.0).abs() < 0.01);
    }

    /// Per-contact STATE: one loud contact must not pump another's leveler.
    /// Measured at the HEAD, where ducking is audible -- the earlier version
    /// measured the tail and passed even with state shared.
    #[test]
    fn a_loud_contact_does_not_level_a_quiet_one() {
        let mut fx = VoiceFx::new();
        let mut loud = sine(500.0, 0.9, 3.0);
        fx.process("loud", &mut loud);
        let mut quiet = sine(500.0, 0.05, 0.5);
        fx.process("quiet", &mut quiet);
        let mut alone = sine(500.0, 0.05, 0.5);
        ChannelFx::new().process(&mut alone);
        let head = |v: &[f32]| rms(&v[..(0.02 * SAMPLE_RATE) as usize]);
        assert!((head(&quiet) - head(&alone)).abs() < 1e-6,
                "{} vs {}", head(&quiet), head(&alone));
    }
}
