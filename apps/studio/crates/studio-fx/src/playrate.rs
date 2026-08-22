//! playrate — effects that re-read time: tape stop and halftime.
//!
//! Both are the same machine wearing different curves: buffer what arrives,
//! read it back at a playback rate that is not 1.0, with linear
//! interpolation between frames. A slowing read IS the pitch drop — no
//! pitch shifter involved, which is exactly why hardware tape stops and the
//! trap halftime sound the way they do.
//!
//! ## Channels, honestly
//!
//! `Instance::process_audio` is one-sample-in-one-sample-out, and the
//! mixdown feeds one instance L then R in sequence (its own documented
//! pattern). A resampler with per-call state would smear the two channels
//! into each other, so these keep one buffer PER channel and advance the
//! shared read position once per FRAME (after the last channel's call).
//! The `channels` param (default 2, matching the mixdown) is how a mono
//! caller opts out — guessing channel count from call patterns is not a
//! thing a sample can tell you.

/// One channel's history plus a shared fractional reader.
#[derive(Debug, Clone)]
struct ChannelBufs {
    bufs: Vec<Vec<f32>>,
    channels: usize,
    /// Which channel the NEXT call belongs to.
    phase: usize,
    /// Whole frames ingested.
    frames: u64,
}

impl ChannelBufs {
    fn new(channels: usize) -> ChannelBufs {
        let channels = channels.max(1);
        ChannelBufs { bufs: vec![Vec::new(); channels], channels, phase: 0, frames: 0 }
    }
    /// Ingest one sample; returns `(channel, frame_completed)`.
    fn push(&mut self, x: f32) -> (usize, bool) {
        let ch = self.phase;
        self.bufs[ch].push(x);
        self.phase = (self.phase + 1) % self.channels;
        let done = self.phase == 0;
        if done { self.frames += 1; }
        (ch, done)
    }
    /// Linear interpolation at fractional frame `pos` in channel `ch`,
    /// clamped to what has actually arrived.
    fn read(&self, ch: usize, pos: f64) -> f32 {
        let buf = &self.bufs[ch];
        if buf.is_empty() { return 0.0; }
        let last = buf.len() - 1;
        let i = (pos.max(0.0) as usize).min(last);
        let j = (i + 1).min(last);
        let frac = (pos - i as f64).clamp(0.0, 1.0) as f32;
        buf[i] * (1.0 - frac) + buf[j] * frac
    }
}

/// The tape grinding to a halt: unity playback until `start` seconds into
/// the material, then the read rate ramps linearly 1 → 0 over `length`
/// seconds (a linear RATE ramp is what a decelerating capstan does — the
/// perceived pitch glide falls out of it), then silence. State is
/// per-material: the mixdown builds one instance per clip, so "seconds in"
/// means seconds into THAT clip.
#[derive(Debug, Clone)]
pub struct TapeStop {
    start_frames: f64,
    len_frames: f64,
    bufs: ChannelBufs,
    /// Fractional read position, in frames, from the very first frame.
    read: f64,
    /// This frame's outputs, computed per channel as its sample arrives.
    hold: Vec<f32>,
}

impl TapeStop {
    pub fn new(rate: f32, start_s: f32, length_s: f32, channels: usize) -> TapeStop {
        TapeStop {
            start_frames: (start_s.max(0.0) * rate) as f64,
            len_frames: (length_s.max(0.01) * rate) as f64,
            bufs: ChannelBufs::new(channels),
            read: 0.0,
            hold: vec![0.0; channels.max(1)],
        }
    }

    /// Rate below which the tape counts as stopped, and the rate at which
    /// the level begins fading toward it. Both are RATE thresholds rather
    /// than times so the fade is proportional to any stop length.
    const STOPPED: f64 = 0.01;
    const FADE_FROM: f64 = 0.05;

    fn stop_gain(rate_now: f64) -> f32 {
        if rate_now <= Self::STOPPED {
            0.0
        } else if rate_now >= Self::FADE_FROM {
            1.0
        } else {
            ((rate_now - Self::STOPPED) / (Self::FADE_FROM - Self::STOPPED)) as f32
        }
    }

    pub fn process(&mut self, x: f32) -> f32 {
        let frame = self.bufs.frames as f64;
        let (ch, frame_done) = self.bufs.push(x);
        let out = if frame < self.start_frames {
            // Before the stop begins the tape runs at speed: passthrough,
            // and the read head rides along so the ramp starts from HERE,
            // not from a stale position.
            x
        } else {
            let t = frame - self.start_frames;
            let rate_now = (1.0 - t / self.len_frames).max(0.0);
            // Below STOPPED the tape has stopped, and a stopped tape is
            // SILENT — not frozen on whatever sample the head last sat on.
            // Holding that sample is a DC offset (measurably ~0.02 here),
            // which is a thump on the way in and DC in the mix on the way
            // out; the FADE avoids trading that thump for a hard cut.
            self.bufs.read(ch, self.read) * Self::stop_gain(rate_now)
        };
        self.hold[ch] = out;
        if frame_done {
            let t = frame - self.start_frames;
            if frame < self.start_frames {
                self.read = frame + 1.0;
            } else {
                let rate_now = (1.0 - t / self.len_frames).max(0.0);
                self.read += rate_now;
            }
        }
        out
    }
}

/// The trap halftime: every `window` seconds, jump back to that window's
/// start and read it at half speed — you hear each window's FIRST half an
/// octave down, in time. The window is seconds rather than bars because an
/// effect instance has no tempo; the UI applying it can do the bars-to-
/// seconds arithmetic where the tempo actually lives.
#[derive(Debug, Clone)]
pub struct Halftime {
    window_frames: u64,
    bufs: ChannelBufs,
    /// Frames dropped off the front of the buffers (ring economy).
    base: u64,
}

impl Halftime {
    pub fn new(rate: f32, window_s: f32, channels: usize) -> Halftime {
        Halftime {
            window_frames: ((window_s.max(0.01) * rate) as u64).max(1),
            bufs: ChannelBufs::new(channels),
            base: 0,
        }
    }

    /// Rate below which the tape counts as stopped, and the rate at which
    /// the level begins fading toward it. Both are RATE thresholds rather
    /// than times so the fade is proportional to any stop length.
    const STOPPED: f64 = 0.01;
    const FADE_FROM: f64 = 0.05;

    fn stop_gain(rate_now: f64) -> f32 {
        if rate_now <= Self::STOPPED {
            0.0
        } else if rate_now >= Self::FADE_FROM {
            1.0
        } else {
            ((rate_now - Self::STOPPED) / (Self::FADE_FROM - Self::STOPPED)) as f32
        }
    }

    pub fn process(&mut self, x: f32) -> f32 {
        let frame = self.bufs.frames;
        let (ch, frame_done) = self.bufs.push(x);
        let win_start = frame - frame % self.window_frames;
        let into = frame - win_start;
        let pos = (win_start - self.base) as f64 + into as f64 * 0.5;
        let out = self.bufs.read(ch, pos);
        if frame_done && frame > self.base + self.window_frames * 4 {
            // Nothing before the current window is ever read again; keep at
            // most a few windows so an hour-long bus doesn't hoard an hour.
            let drop = (win_start - self.base - self.window_frames) as usize;
            if drop > 0 {
                for b in &mut self.bufs.bufs {
                    b.drain(..drop.min(b.len()));
                }
                self.base += drop as u64;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn sine_stereo(hz: f32, frames: usize) -> Vec<f32> {
        (0..frames).flat_map(|i| {
            let s = (i as f32 / RATE * hz * std::f32::consts::TAU).sin();
            [s, s]
        }).collect()
    }

    fn crossings(ch0: &[f32]) -> usize {
        ch0.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count()
    }

    fn dechannel(interleaved: &[f32]) -> Vec<f32> {
        interleaved.iter().step_by(2).copied().collect()
    }

    #[test]
    fn a_tape_stop_glides_the_pitch_down_and_ends_in_silence() {
        let mut fx = TapeStop::new(RATE, 0.0, 1.0, 2);
        let input = sine_stereo(440.0, 48_000);
        let out: Vec<f32> = input.iter().map(|&s| fx.process(s)).collect();
        let ch0 = dechannel(&out);
        // Early: near full speed, ~440Hz. Late (70–80% in, rate ~0.25):
        // roughly a quarter of the crossings. After the ramp: silence.
        let early = crossings(&ch0[0..4800]);
        let late = crossings(&ch0[33_600..38_400]);
        assert!(early > 80, "early is near pitch: {early} crossings/100ms");
        assert!(late * 3 < early, "late has slowed hard: {late} vs {early}");
        let tail: f32 = ch0[47_600..].iter().map(|s| s.abs()).sum();
        assert!(tail < 1e-3, "the tape has stopped: {tail}");
    }

    #[test]
    fn a_stopped_tape_is_silent_not_frozen_on_a_dc_offset() {
        // The failure this caught: with a pure rate ramp the read head
        // freezes and keeps emitting its last sample forever — a DC offset
        // (a thump, and DC into whatever bus it feeds), not a stop. Fed a
        // CONSTANT so a frozen head would hold 0.9 indefinitely and no
        // zero-crossing luck can hide it.
        let mut fx = TapeStop::new(RATE, 0.0, 0.5, 2);
        let out: Vec<f32> = (0..2 * 48_000).map(|_| fx.process(0.9)).collect();
        let ch0 = dechannel(&out);
        // Well past the ramp (0.5s), every sample is truly zero.
        assert!(ch0[30_000..].iter().all(|s| *s == 0.0),
                "max after the stop: {}",
                ch0[30_000..].iter().fold(0f32, |a, s| a.max(s.abs())));
        // And it got there by fading, not by cutting: no single-sample jump
        // bigger than the input's own level during the approach.
        let biggest_jump = ch0[..30_000].windows(2)
            .map(|w| (w[1] - w[0]).abs()).fold(0f32, f32::max);
        assert!(biggest_jump < 0.5, "the fade has no cliff: {biggest_jump}");
    }

    #[test]
    fn before_its_start_a_tape_stop_is_a_wire() {
        let mut fx = TapeStop::new(RATE, 0.5, 0.5, 2);
        let input = sine_stereo(440.0, 48_000);
        let out: Vec<f32> = input.iter().map(|&s| fx.process(s)).collect();
        assert_eq!(&out[..2 * 24_000], &input[..2 * 24_000],
                   "bit-exact passthrough until the stop begins");
    }

    #[test]
    fn halftime_is_an_octave_down_and_stays_in_its_window() {
        let mut fx = Halftime::new(RATE, 0.25, 2);
        let input = sine_stereo(440.0, 48_000);
        let out: Vec<f32> = input.iter().map(|&s| fx.process(s)).collect();
        let ch0 = dechannel(&out);
        // A middle window (frames 24000..36000, i.e. windows 2..3): half the
        // crossings of the input's 440Hz — that is 220, the octave.
        let n = crossings(&ch0[24_000..36_000]);
        let reference = crossings(&dechannel(&input)[24_000..36_000]);
        let ratio = n as f32 / reference as f32;
        assert!((ratio - 0.5).abs() < 0.06, "octave down: ratio {ratio} ({n} vs {reference})");
    }

    #[test]
    fn halftime_ring_economy_does_not_change_the_sound() {
        // The drain that keeps long material from hoarding memory must be
        // inaudible: same input, tiny window (drains many times) — the
        // output around a late window boundary still reads from the RIGHT
        // frames. Compare against a fresh instance fed only the tail's
        // relevant windows... simpler and stronger: no sample is NaN and a
        // late window still halves the pitch, which requires correct reads
        // after many drains.
        let mut fx = Halftime::new(RATE, 0.05, 2);
        let input = sine_stereo(440.0, 96_000);
        let out: Vec<f32> = input.iter().map(|&s| fx.process(s)).collect();
        assert!(out.iter().all(|s| s.is_finite()));
        let ch0 = dechannel(&out);
        let late = crossings(&ch0[88_000..93_000]);
        let reference = crossings(&dechannel(&input)[88_000..93_000]);
        let ratio = late as f32 / reference as f32;
        assert!((ratio - 0.5).abs() < 0.1, "still the octave after drains: {ratio}");
    }

    #[test]
    fn stereo_channels_do_not_bleed_through_the_shared_reader() {
        // L carries a sine, R carries silence: after the effect, R must
        // still be silence — the per-channel buffers are the whole point of
        // ChannelBufs, and a shared buffer would smear L into R.
        let mut fx = Halftime::new(RATE, 0.25, 2);
        let input: Vec<f32> = (0..48_000).flat_map(|i| {
            [(i as f32 / RATE * 440.0 * std::f32::consts::TAU).sin(), 0.0]
        }).collect();
        let out: Vec<f32> = input.iter().map(|&s| fx.process(s)).collect();
        let r_energy: f32 = out.iter().skip(1).step_by(2).map(|s| s.abs()).sum();
        assert!(r_energy < 1e-3, "the silent channel stayed silent: {r_energy}");
        let l_energy: f32 = out.iter().step_by(2).map(|s| s.abs()).sum();
        assert!(l_energy > 100.0, "and the loud one stayed loud");
    }
}
