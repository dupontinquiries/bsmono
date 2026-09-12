//! space — reverb and delay, the two things a send bus exists for.
//!
//! The mixer has had busses and sends since the console panel landed,
//! and the effect registry had nothing worth sending to: `voice::Delay`
//! is a tuned comb (a timbre, mono, by design not a room) and there was
//! no reverb of any kind. The footage in `docs/agents/producer-workflows.md`
//! shows delay and reverb on sends in every project. These are them.
//!
//! ## Channels, the same bargain `playrate` makes
//!
//! `Instance::process_audio` is one sample in, one out, and the mixdown
//! feeds one instance L then R. A reverb or a ping-pong delay is a
//! relationship between the channels, so both keep one state per
//! channel and a phase counter that says which channel the next call
//! belongs to; the `channels` param (default 2) is how a mono caller
//! opts out. Guessing from call patterns is not a thing a sample can
//! tell you.
//!
//! ## The reverb: Schroeder/Moorer, as Freeverb arranged it
//!
//! Eight parallel low-pass-feedback combs into four series allpasses per
//! channel, the right channel's line lengths offset by 23 samples so the
//! two decays decorrelate into width. Not a convolution and not a
//! feedback-delay network: this one is forty lines, allocates nothing
//! after construction, and its tail is a known exponential — which is
//! what makes `size` a number the tests can check rather than a feel.
//! The comb lengths are Freeverb's, tuned at 44.1 kHz and scaled to the
//! running rate so a 48 kHz project gets the same room, not a smaller one.
//!
//! ## The delay
//!
//! Time in milliseconds, feedback, a one-pole low-pass in the feedback
//! path (each repeat duller than the last — the loop is the right place
//! for it, as `voice::Delay`'s docs argue), mix, and ping-pong: with it
//! on, each channel's repeat feeds the OTHER channel's line. Tempo sync
//! is the shell's job — `instantiate` has a rate and no tempo — and
//! [`beats_to_ms`] is the one line it needs.

/// A circular delay line with an integer read distance.
#[derive(Debug, Clone)]
struct Line {
    buf: Vec<f32>,
    write: usize,
}

impl Line {
    fn new(len: usize) -> Line { Line { buf: vec![0.0; len.max(1)], write: 0 } }
    /// The sample written `len` calls ago, then write the new one.
    fn tick(&mut self, input: f32) -> f32 {
        let out = self.buf[self.write];
        self.buf[self.write] = input;
        self.write = (self.write + 1) % self.buf.len();
        out
    }
    fn len(&self) -> usize { self.buf.len() }
    fn clear(&mut self) { self.buf.fill(0.0); self.write = 0; }
}

/// A feedback comb with a one-pole low-pass in the loop — Freeverb's cell.
#[derive(Debug, Clone)]
struct Comb {
    line: Line,
    filter: f32,
    feedback: f32,
    damp: f32,
}

impl Comb {
    fn process(&mut self, input: f32) -> f32 {
        let out = self.line.buf[self.line.write];
        // damp = 0 is a plain comb; damp → 1 is a very dark one.
        self.filter = out * (1.0 - self.damp) + self.filter * self.damp;
        let write = input + self.filter * self.feedback;
        self.line.buf[self.line.write] = write;
        self.line.write = (self.line.write + 1) % self.line.len();
        out
    }
}

/// Schroeder allpass, feedback 0.5 — diffuses without colouring.
#[derive(Debug, Clone)]
struct Allpass {
    line: Line,
}

impl Allpass {
    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.line.buf[self.line.write];
        let out = -input + delayed;
        self.line.buf[self.line.write] = input + delayed * 0.5;
        self.line.write = (self.line.write + 1) % self.line.len();
        out
    }
}

/// Freeverb's tunings, in samples at 44.1 kHz.
const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];
/// The right channel's lines are this much longer: decorrelated decays.
const STEREO_SPREAD: usize = 23;
/// How `size` (0..1) maps to comb feedback: Freeverb's scale and offset.
const ROOM_SCALE: f32 = 0.28;
const ROOM_OFFSET: f32 = 0.7;
const DAMP_SCALE: f32 = 0.4;
/// The comb sum is eight lines deep; Freeverb's output scale keeps the
/// wet at a sane level.
const WET_SCALE: f32 = 0.015;

/// One channel's room.
#[derive(Debug, Clone)]
struct Room {
    combs: Vec<Comb>,
    allpasses: Vec<Allpass>,
}

impl Room {
    fn new(rate: f32, offset: usize, feedback: f32, damp: f32) -> Room {
        let scale = rate / 44_100.0;
        let at = |n: usize| ((n + offset) as f32 * scale).round() as usize;
        Room {
            combs: COMB_TUNING.iter().map(|&n| Comb {
                line: Line::new(at(n)), filter: 0.0, feedback, damp,
            }).collect(),
            allpasses: ALLPASS_TUNING.iter().map(|&n| Allpass { line: Line::new(at(n)) }).collect(),
        }
    }
    fn process(&mut self, input: f32) -> f32 {
        let mut out = 0.0;
        for c in &mut self.combs { out += c.process(input); }
        for a in &mut self.allpasses { out = a.process(out); }
        out
    }
}

/// Reverb: `size` 0..1 (decay), `damp` 0..1 (high-frequency loss per
/// bounce), `predelay` ms, `mix` 0..1 (1 = fully wet, which is what a
/// send bus wants), `width` 0..1 (0 = mono room, 1 = full decorrelation).
#[derive(Debug, Clone)]
pub struct Reverb {
    rooms: Vec<Room>,
    predelay: Vec<Line>,
    channels: usize,
    phase: usize,
    pub mix: f32,
    pub width: f32,
    /// Each channel's wet for the frame being returned — computed all at
    /// once when the frame's last channel arrives, so the width matrix
    /// mixes the SAME frame's rooms. Every channel therefore outputs one
    /// frame late, uniformly: a one-sample latency, which for a room is
    /// nothing, where a half-stale matrix was a high-pass on the tail
    /// (measured: 74% of the tail's energy differed between the ears at
    /// width 0 before this).
    out: Vec<f32>,
    /// Each channel's input this frame: every room is fed the MONO sum
    /// of all channels (Freeverb's rule — a hard-left hit still fills the
    /// whole room; the width comes from the decays, not the feed).
    last_in: Vec<f32>,
    wet: Vec<f32>,
}

impl Reverb {
    pub fn new(rate: f32, size: f32, damp: f32, predelay_ms: f32, mix: f32, width: f32,
               channels: usize) -> Reverb {
        let channels = channels.max(1);
        let feedback = size.clamp(0.0, 1.0) * ROOM_SCALE + ROOM_OFFSET;
        let damp = damp.clamp(0.0, 1.0) * DAMP_SCALE;
        let pre = ((predelay_ms.max(0.0) / 1000.0) * rate).round() as usize;
        Reverb {
            rooms: (0..channels).map(|c| Room::new(rate, c * STEREO_SPREAD, feedback, damp)).collect(),
            predelay: (0..channels).map(|_| Line::new(pre.max(1))).collect(),
            channels, phase: 0,
            mix: mix.clamp(0.0, 1.0), width: width.clamp(0.0, 1.0),
            out: vec![0.0; channels], last_in: vec![0.0; channels], wet: vec![0.0; channels],
        }
    }

    /// One sample of one channel — channels cycle, L then R then L. The
    /// wet returned is the previous frame's (see `out`); the dry is this
    /// sample's.
    pub fn process(&mut self, x: f32) -> f32 {
        let ch = self.phase;
        self.phase = (self.phase + 1) % self.channels;
        self.last_in[ch] = x;
        let wet = self.out[ch];
        if ch == self.channels - 1 {
            // The frame is complete: feed every room the mono sum, then
            // mix the rooms for width, all from the same instant.
            let feed = self.last_in.iter().sum::<f32>() / self.channels as f32;
            for c in 0..self.channels {
                let delayed = if self.predelay[c].len() > 1 { self.predelay[c].tick(feed) } else { feed };
                self.wet[c] = self.rooms[c].process(delayed) * WET_SCALE;
            }
            // At width 0 every channel hears the same (mono) room.
            let mean = self.wet.iter().sum::<f32>() / self.channels as f32;
            for c in 0..self.channels {
                self.out[c] = self.wet[c] * self.width + mean * (1.0 - self.width);
            }
        }
        x * (1.0 - self.mix) + wet * self.mix
    }

    /// Live: the comb feedback is read per sample; changing it mid-tail
    /// lengthens or shortens the tail from here, no click.
    pub fn set_size(&mut self, size: f32) {
        let feedback = size.clamp(0.0, 1.0) * ROOM_SCALE + ROOM_OFFSET;
        for r in &mut self.rooms { for c in &mut r.combs { c.feedback = feedback; } }
    }
    /// Live, for the same reason.
    pub fn set_damp(&mut self, damp: f32) {
        let damp = damp.clamp(0.0, 1.0) * DAMP_SCALE;
        for r in &mut self.rooms { for c in &mut r.combs { c.damp = damp; } }
    }

    pub fn clear(&mut self) {
        for r in &mut self.rooms {
            for c in &mut r.combs { c.line.clear(); c.filter = 0.0; }
            for a in &mut r.allpasses { a.line.clear(); }
        }
        for p in &mut self.predelay { p.clear(); }
        self.wet.fill(0.0);
        self.out.fill(0.0);
        self.last_in.fill(0.0);
        self.phase = 0;
    }
}

/// Stereo delay with an in-loop low-pass and optional ping-pong.
#[derive(Debug, Clone)]
pub struct Delay {
    lines: Vec<Line>,
    /// One-pole low-pass state per channel, in the feedback path.
    lp: Vec<f32>,
    lp_coeff: f32,
    feedback: f32,
    pub mix: f32,
    pub ping_pong: bool,
    channels: usize,
    phase: usize,
    /// The repeat each channel's line produced this frame, for ping-pong
    /// to cross into the next channel's write.
    last_out: Vec<f32>,
    /// Each channel's input this frame: ping-pong feeds the MONO sum into
    /// one line, so a hard-left hit is not lost to the right line.
    last_in: Vec<f32>,
}

impl Delay {
    /// `time_ms` sizes the line exactly; `feedback` 0..0.98; `damp_hz`
    /// is the loop low-pass's corner; `mix` 0..1.
    pub fn new(rate: f32, time_ms: f32, feedback: f32, damp_hz: f32, mix: f32,
               ping_pong: bool, channels: usize) -> Delay {
        let channels = channels.max(1);
        let len = ((time_ms.max(1.0) / 1000.0) * rate).round() as usize;
        let lp_coeff = (-2.0 * std::f32::consts::PI * damp_hz.clamp(20.0, rate * 0.49) / rate).exp();
        Delay {
            lines: (0..channels).map(|_| Line::new(len)).collect(),
            lp: vec![0.0; channels], lp_coeff,
            feedback: feedback.clamp(0.0, 0.98), mix: mix.clamp(0.0, 1.0),
            ping_pong: ping_pong && channels > 1,
            channels, phase: 0, last_out: vec![0.0; channels], last_in: vec![0.0; channels],
        }
    }

    /// Live. The time is NOT: it is the line's length, and changing it
    /// means reallocating or reading across a discontinuity — a real
    /// tape-style time glide is its own feature, not a setter.
    pub fn set_feedback(&mut self, fb: f32) { self.feedback = fb.clamp(0.0, 0.98); }
    /// Live: the loop filter's coefficient is read per sample.
    pub fn set_damp_hz(&mut self, rate: f32, damp_hz: f32) {
        self.lp_coeff = (-2.0 * std::f32::consts::PI * damp_hz.clamp(20.0, rate * 0.49) / rate).exp();
    }

    pub fn process(&mut self, x: f32) -> f32 {
        let ch = self.phase;
        self.phase = (self.phase + 1) % self.channels;
        self.last_in[ch] = x;
        // Ping-pong: this channel's line is fed the PREVIOUS channel's
        // repeat (L's repeat goes into R's line, R's into L's), so a hit
        // bounces across the field instead of stacking.
        let fed = if self.ping_pong {
            let from = (ch + self.channels - 1) % self.channels;
            self.last_out[from]
        } else { self.last_out[ch] };
        // Low-pass inside the loop: each repeat duller than the last.
        self.lp[ch] = fed * (1.0 - self.lp_coeff) + self.lp[ch] * self.lp_coeff;
        // Ping-pong puts the dry — the mono sum of the frame, complete by
        // the LAST channel's call — into that one line, so the first
        // repeat lands opposite the hit; otherwise each channel its own.
        let dry_in = if self.ping_pong {
            if ch == self.channels - 1 { self.last_in.iter().sum::<f32>() / self.channels as f32 }
            else { 0.0 }
        } else { x };
        let out = self.lines[ch].tick(dry_in + self.lp[ch] * self.feedback);
        self.last_out[ch] = out;
        x * (1.0 - self.mix) + out * self.mix
    }

    pub fn clear(&mut self) {
        for l in &mut self.lines { l.clear(); }
        self.lp.fill(0.0);
        self.last_out.fill(0.0);
        self.last_in.fill(0.0);
        self.phase = 0;
    }
}

/// A note length in milliseconds at a tempo — the shell's sync helper
/// (`beats` 0.25 = a sixteenth, 0.75 = a dotted eighth, the trap default).
pub fn beats_to_ms(bpm: f32, beats: f32) -> f32 {
    60_000.0 / bpm.max(1.0) * beats.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RMS over a slice.
    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// Feed an impulse through a MONO reverb, return `seconds` of tail.
    fn impulse_tail(size: f32, damp: f32, seconds: f32) -> Vec<f32> {
        let rate = 48_000.0;
        let mut r = Reverb::new(rate, size, damp, 0.0, 1.0, 1.0, 1);
        let n = (rate * seconds) as usize;
        (0..n).map(|i| r.process(if i == 0 { 1.0 } else { 0.0 })).collect()
    }

    #[test]
    fn the_tail_decays_and_a_bigger_room_decays_slower() {
        for size in [0.2, 0.5, 0.9] {
            let tail = impulse_tail(size, 0.3, 3.0);
            let early = rms(&tail[4800..9600]);
            let late = rms(&tail[96_000..100_800]);
            assert!(early > late * 4.0, "size {size}: 0.1s ({early}) vs 2s ({late}) — the room must decay");
            assert!(tail.iter().all(|v| v.is_finite() && v.abs() < 1.0), "size {size}: stable, in range");
        }
        // A bigger room keeps more energy at two seconds.
        let small = rms(&impulse_tail(0.2, 0.3, 2.2)[96_000..]);
        let big = rms(&impulse_tail(0.9, 0.3, 2.2)[96_000..]);
        assert!(big > small * 10.0, "size 0.9 ({big}) should ring far longer than 0.2 ({small})");
        // And damping takes the top off faster: the tail's average
        // absolute first difference (a crude brightness) drops with damp.
        let bright = impulse_tail(0.7, 0.0, 1.0);
        let dark = impulse_tail(0.7, 1.0, 1.0);
        let diff = |t: &[f32]| t.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / rms(t).max(1e-9);
        assert!(diff(&bright[24_000..]) > diff(&dark[24_000..]) * 1.5,
                "damped tail is smoother: {} vs {}", diff(&bright[24_000..]), diff(&dark[24_000..]));
    }

    #[test]
    fn mix_zero_is_a_wire_and_mix_one_has_no_dry() {
        let mut dry = Reverb::new(48_000.0, 0.5, 0.5, 10.0, 0.0, 1.0, 2);
        for i in 0..2000 {
            let x = ((i * 7) % 13) as f32 / 13.0 - 0.5;
            assert_eq!(dry.process(x), x, "mix 0 passes the sample through untouched");
        }
        // Fully wet with predelay: the first samples after an impulse are
        // silence (nothing has reached the room yet) — the dry is gone.
        let mut wet = Reverb::new(48_000.0, 0.5, 0.5, 20.0, 1.0, 1.0, 1);
        let first: Vec<f32> = (0..900).map(|i| wet.process(if i == 0 { 1.0 } else { 0.0 })).collect();
        assert!(first.iter().all(|&v| v == 0.0), "20 ms of predelay at 48k is 960 silent samples");
        let later: Vec<f32> = (0..4800).map(|_| wet.process(0.0)).collect();
        assert!(rms(&later) > 0.0, "and then the room speaks");
    }

    #[test]
    fn the_stereo_rooms_are_two_rooms_and_width_zero_makes_them_one() {
        // An impulse on L only. At width 1 the two channels' tails differ
        // (different line lengths); at width 0 they are identical.
        let tails = |width: f32| -> (Vec<f32>, Vec<f32>) {
            let mut r = Reverb::new(48_000.0, 0.6, 0.3, 0.0, 1.0, width, 2);
            let (mut l, mut rr) = (Vec::new(), Vec::new());
            for i in 0..48_000 {
                l.push(r.process(if i == 0 { 1.0 } else { 0.0 }));
                rr.push(r.process(0.0));
            }
            (l, rr)
        };
        let (l, r) = tails(1.0);
        let d: f32 = l.iter().zip(&r).map(|(a, b)| (a - b).abs()).sum::<f32>();
        assert!(d > 1.0, "wide: L and R decay differently ({d})");
        assert!(rms(&r[2400..]) > 0.0, "a hard-left hit still fills the right room: {}", rms(&r));
        let (l0, r0) = tails(0.0);
        let d0: f32 = l0.iter().zip(&r0).map(|(a, b)| (a - b).abs()).sum::<f32>();
        let e0: f32 = l0.iter().map(|v| v.abs()).sum::<f32>();
        assert_eq!(d0, 0.0, "width 0: one room in both ears, the same frame in both ({d0} of {e0})");
        assert!(d > e0 * 0.2, "and width 1 really was two rooms ({d} of {e0})");
    }

    #[test]
    fn a_delay_repeats_at_its_time_and_each_repeat_is_quieter_and_duller() {
        let rate = 48_000.0;
        let mut d = Delay::new(rate, 100.0, 0.5, 4000.0, 1.0, false, 1);
        let n = 4800 * 4 + 10;
        let out: Vec<f32> = (0..n).map(|i| d.process(if i == 0 { 1.0 } else { 0.0 })).collect();
        // Repeats land at exactly 100 ms multiples (the LP smears each
        // by a few samples; the peak stays put).
        let peak_near = |t: usize| -> (usize, f32) {
            let lo = t.saturating_sub(20);
            let hi = (t + 40).min(out.len());
            (lo..hi).map(|i| (i, out[i].abs())).fold((0, 0.0f32), |m, v| if v.1 > m.1 { v } else { m })
        };
        let (i1, p1) = peak_near(4800);
        let (i2, p2) = peak_near(9600);
        let (i3, p3) = peak_near(14_400);
        assert!((i1 as i64 - 4800).abs() <= 10 && (i2 as i64 - 9600).abs() <= 10 && (i3 as i64 - 14_400).abs() <= 10,
                "repeats at 100/200/300 ms: {i1} {i2} {i3}");
        assert!(p1 > p2 && p2 > p3, "feedback 0.5: each quieter ({p1} {p2} {p3})");
        // The loop's one-pole spreads each repeat over a few dozen samples
        // and keeps its SUM (unity at DC), so the feedback ratio shows in
        // energy, not in peak height.
        let energy = |t: usize| out[t - 20..t + 400].iter().map(|v| v.abs()).sum::<f32>();
        let (e1, e2, e3) = (energy(4800), energy(9600), energy(14_400));
        assert!((e2 / e1 - 0.5).abs() < 0.08 && (e3 / e2 - 0.5).abs() < 0.08,
                "feedback 0.5 halves each repeat: {e1} → {e2} → {e3}");
        // Fully wet: no dry hit at 0, and nothing before the first repeat.
        assert_eq!(out[0], 0.0, "mix 1.0 has no dry");
        assert!(out[..4780].iter().all(|&v| v == 0.0), "nothing before the first repeat");
        // Duller: the second repeat is wider than the first (the LP).
        let width = |t: usize| (t.saturating_sub(5)..t + 60).filter(|&i| out[i].abs() > 0.01 * out[t].abs().max(1e-9)).count();
        assert!(width(i2) >= width(i1), "later repeats are smeared by the loop filter");
    }

    #[test]
    fn ping_pong_alternates_channels_and_plain_stereo_does_not() {
        let rate = 48_000.0;
        let run = |pp: bool| -> (Vec<f32>, Vec<f32>) {
            let mut d = Delay::new(rate, 50.0, 0.6, 20_000.0, 1.0, pp, 2);
            let (mut l, mut r) = (Vec::new(), Vec::new());
            for i in 0..2400 * 5 {
                l.push(d.process(if i == 0 { 1.0 } else { 0.0 }));
                r.push(d.process(0.0));
            }
            (l, r)
        };
        let energy = |x: &[f32], t: usize| x[t.saturating_sub(5)..t + 50].iter().map(|v| v.abs()).sum::<f32>();
        let (l, r) = run(true);
        // Hit on L at 0 (its mono sum, 0.5, enters the right line). First
        // repeat: R at 50 ms. Second: L at 100 ms (R's out crossed into
        // L's line). Third: R at 150 ms.
        assert!(energy(&r, 2400) > 0.3 && energy(&l, 2400) < 0.05, "50 ms: right ({}) not left ({})", energy(&r, 2400), energy(&l, 2400));
        assert!(energy(&l, 4800) > 0.15 && energy(&r, 4800) < 0.05, "100 ms: left ({}) not right ({})", energy(&l, 4800), energy(&r, 4800));
        assert!(energy(&r, 7200) > 0.05 && energy(&l, 7200) < 0.05, "150 ms: right again");
        let (l, r) = run(false);
        assert!(energy(&l, 2400) > 0.3 && energy(&r, 2400) == 0.0, "plain: a left hit repeats on the left only");
        assert!(energy(&l, 4800) > 0.15);
    }

    #[test]
    fn feedback_is_capped_below_one_and_a_cleared_line_is_silent() {
        let mut d = Delay::new(48_000.0, 10.0, 5.0, 8000.0, 1.0, false, 1);
        let mut peak = 0.0f32;
        for i in 0..48_000 {
            let v = d.process(if i == 0 { 1.0 } else { 0.0 });
            assert!(v.is_finite());
            peak = peak.max(v.abs());
        }
        assert!(peak <= 1.0, "a feedback of 5 is clamped to 0.98: it cannot run away ({peak})");
        d.clear();
        for _ in 0..1000 { assert_eq!(d.process(0.0), 0.0); }
        let mut r = Reverb::new(48_000.0, 1.0, 0.0, 0.0, 1.0, 1.0, 2);
        for _ in 0..1000 { r.process(1.0); }
        r.clear();
        assert_eq!(r.process(0.0), 0.0, "a cleared room is silent");
    }

    #[test]
    fn sync_helper_is_the_note_length_at_the_tempo() {
        assert!((beats_to_ms(140.0, 1.0) - 428.571).abs() < 0.01, "a quarter at 140");
        assert!((beats_to_ms(140.0, 0.75) - 321.428).abs() < 0.01, "the dotted eighth");
        assert!((beats_to_ms(120.0, 0.25) - 125.0).abs() < 1e-3, "a sixteenth at 120");
        assert_eq!(beats_to_ms(0.0, 1.0), 60_000.0, "a tempo of zero is treated as one, not a division by it");
    }

    #[test]
    fn the_room_at_48k_is_the_same_room_as_at_44_1k() {
        // The tunings scale with the rate: a 48k room decays over the same
        // TIME, not the same sample count.
        let tail_at = |rate: f32| -> f32 {
            let mut r = Reverb::new(rate, 0.5, 0.3, 0.0, 1.0, 1.0, 1);
            let n = (rate * 1.5) as usize;
            let t: Vec<f32> = (0..n).map(|i| r.process(if i == 0 { 1.0 } else { 0.0 })).collect();
            let s = (rate * 1.0) as usize;
            rms(&t[s..s + (rate * 0.1) as usize])
        };
        let (a, b) = (tail_at(44_100.0), tail_at(48_000.0));
        assert!((a - b).abs() < a * 0.35, "one second in, the two rates hold similar energy: {a} vs {b}");
    }
}
