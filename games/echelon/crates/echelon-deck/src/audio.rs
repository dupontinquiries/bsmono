//! All sound synthesized, no assets: a one-shot per `cues::Cue`. The Command Deck had no
//! sound at all. This is the house pattern (road-warrior, clawbound, skydribble): a tiny
//! mixer in the cpal callback, fed over a channel, nothing decoded. The mixer ([`Synth`])
//! is pure and always compiled, so its voices are tested here without a device; only
//! [`Audio`] -- the stream -- is behind the `sound` feature (needs ALSA headers on Linux;
//! the default build and the tape stay silent and dependency-free). Nothing here has been
//! heard in the sandbox that wrote it: the voices are a first pass.

use crate::cues::Cue;
use std::sync::mpsc::{channel, Receiver, Sender};

#[derive(Clone, Copy, Debug)]
pub enum Msg {
    Cue(Cue),
    /// 0..1; 0 is mute.
    Master(f32),
}

/// One short synthesized sound: a sine (or noise) sweeping `f0 -> f1` over `dur` seconds
/// with a fast attack and an exponential tail.
#[derive(Clone, Copy, Debug)]
struct Voice {
    t: f32,
    dur: f32,
    f0: f32,
    f1: f32,
    amp: f32,
    /// 0 = pure tone, 1 = pure noise.
    noise: f32,
    phase: f32,
    /// seconds before the voice starts (a later note of a phrase)
    delay: f32,
}

impl Voice {
    fn tone(dur: f32, f0: f32, f1: f32, amp: f32) -> Voice {
        Voice { t: 0.0, dur, f0, f1, amp, noise: 0.0, phase: 0.0, delay: 0.0 }
    }
    fn noise(dur: f32, amp: f32) -> Voice {
        Voice { t: 0.0, dur, f0: 0.0, f1: 0.0, amp, noise: 1.0, phase: 0.0, delay: 0.0 }
    }
    fn after(mut self, delay: f32) -> Voice {
        self.delay = delay;
        self
    }
}

/// The mixer. `sample()` is one mono sample; the stream writes it to every channel.
pub struct Synth {
    sr: f32,
    rx: Receiver<Msg>,
    voices: Vec<Voice>,
    master: f32,
    noise_state: u32,
}

const MAX_VOICES: usize = 24;

impl Synth {
    pub fn new(sr: f32, rx: Receiver<Msg>) -> Synth {
        Synth { sr, rx, voices: Vec::with_capacity(MAX_VOICES), master: 1.0, noise_state: 0x2545_F491 }
    }

    fn white(&mut self) -> f32 {
        let mut x = self.noise_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise_state = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    fn spawn(&mut self, v: Voice) {
        if self.voices.len() < MAX_VOICES {
            self.voices.push(v);
        }
    }

    /// Voices sounding or waiting their turn (the tests' view of the mixer).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn voices(&self) -> usize {
        self.voices.len()
    }

    /// Take everything the deck sent since the last call. Called once per buffer by the
    /// stream; a test calls it directly.
    pub fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            self.handle(m);
        }
    }

    fn handle(&mut self, m: Msg) {
        match m {
            Msg::Master(v) => self.master = v.clamp(0.0, 1.0),
            Msg::Cue(c) => self.cue(c),
        }
    }

    fn cue(&mut self, c: Cue) {
        match c {
            // a short two-tone blip: the order is heard
            Cue::Ack => {
                self.spawn(Voice::tone(0.05, 880.0, 880.0, 0.12));
                self.spawn(Voice::tone(0.07, 1320.0, 1320.0, 0.1).after(0.05));
            }
            // a rising three-note chime
            Cue::Trained => {
                for (k, f) in [659.25f32, 783.99, 1046.5].iter().enumerate() {
                    self.spawn(Voice::tone(0.16, *f, *f, 0.12).after(k as f32 * 0.07));
                }
            }
            // a lower two-note, settled
            Cue::Built => {
                self.spawn(Voice::tone(0.2, 220.0, 220.0, 0.16));
                self.spawn(Voice::tone(0.3, 330.0, 330.0, 0.14).after(0.12));
            }
            // a thud and a falling tone: something of ours is gone
            Cue::Lost => {
                self.spawn(Voice::noise(0.18, 0.3));
                self.spawn(Voice::tone(0.35, 180.0, 70.0, 0.25).after(0.03));
            }
            // a crack; ours slightly lower and heavier than theirs
            Cue::Hit { ours } => {
                let (f, a) = if ours { (900.0, 0.16) } else { (1500.0, 0.11) };
                self.spawn(Voice::noise(0.05, a));
                self.spawn(Voice::tone(0.07, f, f * 0.4, a * 0.7));
            }
            // a rising major arpeggio
            Cue::Victory => {
                for (k, f) in [523.25f32, 659.25, 783.99, 1046.5].iter().enumerate() {
                    self.spawn(Voice::tone(0.4, *f, *f, 0.16).after(k as f32 * 0.1));
                }
            }
            // a falling minor pair and a rumble
            Cue::Defeat => {
                self.spawn(Voice::tone(0.5, 311.13, 155.56, 0.2));
                self.spawn(Voice::tone(0.7, 233.08, 116.54, 0.2).after(0.35));
                self.spawn(Voice::noise(0.6, 0.12).after(0.2));
            }
        }
    }

    pub fn sample(&mut self) -> f32 {
        let dt = 1.0 / self.sr;
        let mut out = 0.0;
        let mut i = 0;
        while i < self.voices.len() {
            let n = self.white();
            let v = &mut self.voices[i];
            if v.delay > 0.0 {
                v.delay -= dt;
                i += 1;
                continue;
            }
            let u = v.t / v.dur;
            if u >= 1.0 {
                self.voices.swap_remove(i);
                continue;
            }
            let env = (u * 60.0).min(1.0) * (-4.0 * u).exp();
            let f = v.f0 + (v.f1 - v.f0) * u;
            v.phase = (v.phase + f * dt).fract();
            let tone = (v.phase * std::f32::consts::TAU).sin();
            v.t += dt;
            out += (tone * (1.0 - v.noise) + n * v.noise) * env * v.amp;
            i += 1;
        }
        (out * self.master).clamp(-1.0, 1.0)
    }
}

/// A sender for the deck side and the synth the stream (or a test) runs.
pub fn pair(sr: f32) -> (Sender<Msg>, Synth) {
    let (tx, rx) = channel();
    (tx, Synth::new(sr, rx))
}

#[cfg(feature = "sound")]
mod stream {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    /// The deck's handle on the sound: a sender and the stream it keeps alive.
    pub struct Audio {
        _stream: cpal::Stream,
        tx: Sender<Msg>,
    }

    impl Audio {
        /// `None` on a machine with no output device: the deck runs silent.
        pub fn start() -> Option<Audio> {
            let host = cpal::default_host();
            let device = host.default_output_device()?;
            let cfg = device.default_output_config().ok()?;
            let sr = cfg.sample_rate().0 as f32;
            let ch = cfg.channels().max(1) as usize;
            let (tx, mut synth) = pair(sr);
            let err = |e| eprintln!("audio stream error: {e}");
            let stream = match cfg.sample_format() {
                cpal::SampleFormat::F32 => device
                    .build_output_stream(
                        &cfg.into(),
                        move |data: &mut [f32], _| {
                            synth.drain();
                            for frame in data.chunks_mut(ch) {
                                let s = synth.sample();
                                frame.fill(s);
                            }
                        },
                        err,
                        None,
                    )
                    .ok()?,
                cpal::SampleFormat::I16 => device
                    .build_output_stream(
                        &cfg.into(),
                        move |data: &mut [i16], _| {
                            synth.drain();
                            for frame in data.chunks_mut(ch) {
                                let s = (synth.sample() * i16::MAX as f32) as i16;
                                frame.fill(s);
                            }
                        },
                        err,
                        None,
                    )
                    .ok()?,
                cpal::SampleFormat::U16 => device
                    .build_output_stream(
                        &cfg.into(),
                        move |data: &mut [u16], _| {
                            synth.drain();
                            for frame in data.chunks_mut(ch) {
                                let s = ((synth.sample() * 0.5 + 0.5) * u16::MAX as f32) as u16;
                                frame.fill(s);
                            }
                        },
                        err,
                        None,
                    )
                    .ok()?,
                _ => return None,
            };
            stream.play().ok()?;
            Some(Audio { _stream: stream, tx })
        }

        pub fn send(&self, m: Msg) {
            let _ = self.tx.send(m);
        }
    }
}

#[cfg(feature = "sound")]
pub use stream::Audio;

/// The silent build: the same handle, nothing behind it.
#[cfg(not(feature = "sound"))]
pub struct Audio;

#[cfg(not(feature = "sound"))]
impl Audio {
    pub fn start() -> Option<Audio> {
        None
    }
    pub fn send(&self, _m: Msg) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(s: &mut Synth, n: usize) -> f32 {
        let mut acc = 0.0f32;
        for _ in 0..n {
            let x = s.sample();
            acc += x * x;
        }
        (acc / n as f32).sqrt()
    }

    #[test]
    fn every_cue_sounds_then_falls_silent() {
        let sr = 48_000.0;
        for c in [Cue::Ack, Cue::Trained, Cue::Built, Cue::Lost, Cue::Hit { ours: true }, Cue::Hit { ours: false }, Cue::Victory, Cue::Defeat] {
            let (tx, mut s) = pair(sr);
            assert_eq!(rms(&mut s, 480), 0.0, "silence before {c:?}");
            tx.send(Msg::Cue(c)).unwrap();
            s.drain();
            assert!(s.voices() >= 1, "{c:?} spawned nothing");
            let loud = rms(&mut s, 4800); // the first 100 ms
            assert!(loud > 0.005, "{c:?} is inaudible: rms {loud}");
            let _ = rms(&mut s, 96_000); // two seconds on: everything has decayed and been dropped
            assert_eq!(s.voices(), 0, "{c:?} left a voice sounding");
            assert_eq!(rms(&mut s, 480), 0.0, "{c:?} did not fall silent");
        }
    }

    #[test]
    fn the_master_mutes_and_the_mix_stays_in_range() {
        let (tx, mut s) = pair(48_000.0);
        for _ in 0..30 {
            tx.send(Msg::Cue(Cue::Victory)).unwrap(); // far more than MAX_VOICES
        }
        s.drain();
        assert_eq!(s.voices(), MAX_VOICES, "voices are capped");
        for _ in 0..48_000 {
            let x = s.sample();
            assert!((-1.0..=1.0).contains(&x));
        }
        tx.send(Msg::Master(0.0)).unwrap();
        tx.send(Msg::Cue(Cue::Lost)).unwrap();
        s.drain();
        assert_eq!(rms(&mut s, 4800), 0.0, "muted");
    }
}
