//! All sound synthesized, no assets: an engine drone driven by `cues::engine`, a one-shot
//! per `cues::Cue`, and the shell's own two stings (a run lost, a level done).
//!
//! The game had no sound at all. This is the house pattern (skydribble, blackstar-rs): a
//! tiny mixer in the cpal callback, fed over a channel, nothing decoded. The mixer
//! ([`Synth`]) is pure and always compiled, so its voices are tested here without a device;
//! only [`Audio`] -- the stream -- is behind the `sound` feature (default on; build with
//! `--no-default-features` for a silent binary with no ALSA dependency). Nothing here has
//! been heard in the sandbox that wrote it: the voices are a first pass.

use road_warrior_core::cues::{Cue, Engine};
use std::sync::mpsc::{channel, Receiver, Sender};

/// The shell's own moments, outside the car's cues.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sting {
    /// The run is lost: a crash, or the tank ran dry.
    Lost,
    /// The level is done.
    LevelComplete,
}

#[derive(Clone, Copy, Debug)]
pub enum Msg {
    Engine(Engine),
    Cue(Cue),
    Sting(Sting),
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
    /// seconds before the voice starts (a second note in a chord, a sputter's second blip)
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
    rpm: f32,
    load: f32,
    rpm_s: f32,
    load_s: f32,
    eng_phase: f32,
    eng_phase2: f32,
    eng_lp: f32,
    master: f32,
    noise_state: u32,
}

const MAX_VOICES: usize = 24;
/// The idle note and the note at top speed, Hz. A four-cylinder's fundamental.
const ENGINE_IDLE_HZ: f32 = 38.0;
const ENGINE_TOP_HZ: f32 = 150.0;
/// The drone's gain at idle and the gain the throttle adds.
const ENGINE_IDLE_GAIN: f32 = 0.05;
const ENGINE_LOAD_GAIN: f32 = 0.22;

impl Synth {
    pub fn new(sr: f32, rx: Receiver<Msg>) -> Synth {
        Synth {
            sr,
            rx,
            voices: Vec::with_capacity(MAX_VOICES),
            rpm: 0.0,
            load: 0.0,
            rpm_s: 0.0,
            load_s: 0.0,
            eng_phase: 0.0,
            eng_phase2: 0.0,
            eng_lp: 0.0,
            master: 1.0,
            noise_state: 0x2545_F491,
        }
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

    /// Take everything the game sent since the last call. Called once per buffer by the
    /// stream; a test calls it directly.
    pub fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            self.handle(m);
        }
    }

    fn handle(&mut self, m: Msg) {
        match m {
            Msg::Engine(e) => {
                self.rpm = e.rpm.clamp(0.0, 1.25);
                self.load = e.load.clamp(0.0, 1.0);
            }
            Msg::Master(v) => self.master = v.clamp(0.0, 1.0),
            Msg::Cue(c) => self.cue(c),
            Msg::Sting(s) => self.sting(s),
        }
    }

    fn cue(&mut self, c: Cue) {
        match c {
            // a rising hiss: air under the car
            Cue::Launch => {
                self.spawn(Voice::noise(0.25, 0.18));
                self.spawn(Voice::tone(0.25, 220.0, 660.0, 0.08));
            }
            // a low thud; the clean one gets the nitro's chime on top
            Cue::Land { clean } => {
                self.spawn(Voice::tone(0.14, 90.0, 40.0, 0.5));
                if clean {
                    self.spawn(Voice::tone(0.18, 880.0, 1320.0, 0.12).after(0.05));
                } else {
                    self.spawn(Voice::noise(0.3, 0.45));
                }
            }
            // a metallic crack, louder for a bigger hit
            Cue::Hit { amount } => {
                let a = (0.2 + amount / 40.0).min(0.6);
                self.spawn(Voice::noise(0.12, a));
                self.spawn(Voice::tone(0.1, 1600.0, 300.0, a * 0.6));
            }
            // the ram lands on the boss: a heavy, lower thud
            Cue::Ram { amount } => {
                let a = (0.3 + amount / 60.0).min(0.7);
                self.spawn(Voice::tone(0.22, 70.0, 30.0, a));
                self.spawn(Voice::noise(0.2, a * 0.5));
            }
            // the jet: a long hiss with a bit of whistle
            Cue::Nitro => {
                self.spawn(Voice::noise(0.6, 0.25));
                self.spawn(Voice::tone(0.6, 400.0, 900.0, 0.06));
            }
            // a sputter: three fading low blips
            Cue::OutOfGas => {
                for k in 0..3 {
                    self.spawn(Voice::tone(0.09, 60.0, 35.0, 0.35 - k as f32 * 0.08).after(k as f32 * 0.16));
                }
            }
        }
    }

    fn sting(&mut self, s: Sting) {
        match s {
            // a falling two-note and a crunch
            Sting::Lost => {
                self.spawn(Voice::noise(0.35, 0.4));
                self.spawn(Voice::tone(0.5, 330.0, 165.0, 0.2).after(0.1));
                self.spawn(Voice::tone(0.6, 247.0, 110.0, 0.2).after(0.4));
            }
            // a rising major arpeggio
            Sting::LevelComplete => {
                for (k, f) in [523.25f32, 659.25, 783.99, 1046.5].iter().enumerate() {
                    self.spawn(Voice::tone(0.35, *f, *f, 0.16).after(k as f32 * 0.09));
                }
            }
        }
    }

    pub fn sample(&mut self) -> f32 {
        let dt = 1.0 / self.sr;
        // the engine: two detuned saws, smoothed so a gear change is a slide not a step
        self.rpm_s += (self.rpm - self.rpm_s) * (dt * 6.0).min(1.0);
        self.load_s += (self.load - self.load_s) * (dt * 10.0).min(1.0);
        let f = ENGINE_IDLE_HZ + (ENGINE_TOP_HZ - ENGINE_IDLE_HZ) * self.rpm_s;
        self.eng_phase = (self.eng_phase + f * dt).fract();
        self.eng_phase2 = (self.eng_phase2 + f * 1.007 * dt).fract();
        let saw = (self.eng_phase * 2.0 - 1.0) * 0.6 + (self.eng_phase2 * 2.0 - 1.0) * 0.4;
        // a one-pole low-pass opens with the load: the throttle makes it brighter, not only louder
        let cutoff = 200.0 + 900.0 * self.load_s;
        let k = (1.0 - (-2.0 * std::f32::consts::PI * cutoff * dt).exp()).clamp(0.0, 1.0);
        self.eng_lp += (saw - self.eng_lp) * k;
        let mut out = self.eng_lp * (ENGINE_IDLE_GAIN + ENGINE_LOAD_GAIN * self.load_s);

        // the one-shots
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

/// A sender for the game side and the synth the stream (or a test) runs.
pub fn pair(sr: f32) -> (Sender<Msg>, Synth) {
    let (tx, rx) = channel();
    (tx, Synth::new(sr, rx))
}

#[cfg(feature = "sound")]
mod stream {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    /// The game's handle on the sound: a sender and the stream it keeps alive.
    pub struct Audio {
        _stream: cpal::Stream,
        tx: Sender<Msg>,
    }

    impl Audio {
        /// `None` on a machine with no output device: the game runs silent.
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

    fn rms(s: &mut Synth, secs: f32) -> f32 {
        let n = (secs * s.sr) as usize;
        let mut acc = 0.0f64;
        for _ in 0..n {
            let x = s.sample() as f64;
            acc += x * x;
        }
        (acc / n as f64).sqrt() as f32
    }

    /// The drone is there at idle, louder and brighter under load, and follows the sent
    /// note; the master at zero is silence.
    #[test]
    fn the_engine_drone_follows_the_note_and_the_master_mutes_it() {
        let (tx, mut s) = pair(48_000.0);
        let idle = rms(&mut s, 0.5);
        assert!(idle > 0.005 && idle < 0.08, "an idling engine is quiet, not silent: {idle}");
        tx.send(Msg::Engine(Engine { rpm: 1.0, load: 1.0 })).unwrap();
        s.drain();
        rms(&mut s, 0.5); // the smoothing settles
        let full = rms(&mut s, 0.5);
        assert!(full > idle * 3.0, "full throttle is well over idle: {full} vs {idle}");
        tx.send(Msg::Master(0.0)).unwrap();
        s.drain();
        assert_eq!(rms(&mut s, 0.2), 0.0, "muted is zero, not quiet");
        tx.send(Msg::Master(1.0)).unwrap();
        s.drain();
        assert!(rms(&mut s, 0.2) > idle);
    }

    /// Every cue and sting makes a sound over the drone and dies away: the mix is louder
    /// for the first tenth of a second than the drone alone, and back to the drone within
    /// two seconds with no voice left.
    #[test]
    fn every_cue_and_sting_sounds_and_then_stops() {
        let cues = [
            Msg::Cue(Cue::Launch),
            Msg::Cue(Cue::Land { clean: true }),
            Msg::Cue(Cue::Land { clean: false }),
            Msg::Cue(Cue::Hit { amount: 20.0 }),
            Msg::Cue(Cue::Ram { amount: 30.0 }),
            Msg::Cue(Cue::Nitro),
            Msg::Cue(Cue::OutOfGas),
            Msg::Sting(Sting::Lost),
            Msg::Sting(Sting::LevelComplete),
        ];
        for m in cues {
            let (tx, mut s) = pair(44_100.0);
            let floor = rms(&mut s, 0.3);
            tx.send(m).unwrap();
            s.drain();
            assert!(!s.voices.is_empty(), "{m:?} spawned nothing");
            let burst = rms(&mut s, 0.1);
            assert!(burst > floor * 2.0, "{m:?} is not audible over the drone: {burst} vs {floor}");
            let later = rms(&mut s, 2.0);
            assert!(s.voices.is_empty(), "{m:?} left voices playing after two seconds");
            assert!(later < burst, "{m:?} did not die away: {later} vs {burst}");
        }
    }

    /// The output stays in range however much lands at once, and the voice pool is bounded.
    #[test]
    fn a_pile_of_cues_clips_rather_than_overflows_and_the_pool_is_bounded() {
        let (tx, mut s) = pair(44_100.0);
        for _ in 0..100 {
            tx.send(Msg::Cue(Cue::Land { clean: false })).unwrap();
        }
        tx.send(Msg::Engine(Engine { rpm: 9.0, load: 9.0 })).unwrap();
        s.drain();
        assert!(s.voices.len() <= MAX_VOICES);
        assert_eq!(s.rpm, 1.25);
        assert_eq!(s.load, 1.0);
        for _ in 0..4410 {
            let x = s.sample();
            assert!((-1.0..=1.0).contains(&x) && x.is_finite(), "{x}");
        }
    }
}
