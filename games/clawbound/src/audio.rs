//! audio — the sound, synthesized: beds for the air (`cues::Ambience`), a voice per
//! `cues::Cue`. No assets, nothing decoded.
//!
//! The game had no sound. This is the house pattern (skydribble, blackstar-rs, road-warrior):
//! a tiny mixer in the cpal callback, fed over a channel. The mixer ([`Synth`]) is pure and
//! always compiled, so its beds and voices are tested here without a device; only the
//! stream ([`Audio`]) is behind the `sound` feature (part of `app`, never on wasm32), and a
//! machine with no output device runs silent. Nothing here has been heard in the sandbox
//! that wrote it: the voices are a first pass by their numbers.
//!
//! Beds: wind (band-passed noise swelling slowly), rain (bright noise, dense), thunder
//! (a rumble now and then, in a storm), the snow hush (the wind reads softer; the beds sit
//! lower), a tension drone that rises with the hunter (two low detuned sines and, past
//! two thirds, a heartbeat), and the breath of running.

use crate::cues::{Ambience, Cue};
use crate::weather::Sky;
use std::sync::mpsc::{channel, Receiver, Sender};

#[derive(Clone, Copy, Debug)]
pub enum Msg {
    Air(Ambience),
    Cue(Cue),
    /// 0..1; 0 is mute.
    Master(f32),
}

#[derive(Clone, Copy, Debug)]
struct Voice {
    t: f32,
    dur: f32,
    f0: f32,
    f1: f32,
    amp: f32,
    noise: f32,
    phase: f32,
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

const MAX_VOICES: usize = 32;

pub struct Synth {
    sr: f32,
    rx: Receiver<Msg>,
    voices: Vec<Voice>,
    air: Ambience,
    air_s: Ambience,
    master: f32,
    noise_state: u32,
    // the beds' filters and clocks
    wind_lp: f32,
    wind_lp2: f32,
    wind_swell: f32,
    rain_hp_prev: f32,
    rain_hp: f32,
    thunder_t: f32,
    drone_phase: [f32; 2],
    heart_t: f32,
    breath_t: f32,
    t: f32,
    /// How many heartbeats and thunder rolls the beds have fired (the tests count them).
    beats: u32,
    thunders: u32,
}

impl Synth {
    pub fn new(sr: f32, rx: Receiver<Msg>) -> Synth {
        Synth {
            sr,
            rx,
            voices: Vec::with_capacity(MAX_VOICES),
            air: Ambience::default(),
            air_s: Ambience::default(),
            master: 1.0,
            noise_state: 0x9E37_79B9,
            wind_lp: 0.0,
            wind_lp2: 0.0,
            wind_swell: 0.0,
            rain_hp_prev: 0.0,
            rain_hp: 0.0,
            thunder_t: 4.0,
            drone_phase: [0.0; 2],
            heart_t: 0.0,
            breath_t: 0.0,
            t: 0.0,
            beats: 0,
            thunders: 0,
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

    /// Take everything the game sent since the last call: once per buffer in the stream.
    pub fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Air(a) => self.air = a,
                Msg::Master(v) => self.master = v.clamp(0.0, 1.0),
                Msg::Cue(c) => self.cue(c),
            }
        }
    }

    fn cue(&mut self, c: Cue) {
        match c {
            // a short rush of air and a soft thump at the end of the bound
            Cue::Leap => {
                self.spawn(Voice::noise(0.22, 0.16));
                self.spawn(Voice::tone(0.08, 120.0, 60.0, 0.25).after(0.3));
            }
            // a quick tumble: two soft thumps
            Cue::Dodge => {
                self.spawn(Voice::tone(0.07, 140.0, 70.0, 0.22));
                self.spawn(Voice::tone(0.07, 120.0, 60.0, 0.18).after(0.12));
            }
            // brambles: a dry scratchy crackle
            Cue::Snag => {
                for k in 0..4 {
                    self.spawn(Voice::noise(0.05, 0.22).after(k as f32 * 0.07));
                }
            }
            // the hawk: a screech down and a thud
            Cue::HawkGrab => {
                self.spawn(Voice::tone(0.5, 2400.0, 900.0, 0.3));
                self.spawn(Voice::noise(0.15, 0.3).after(0.05));
            }
            // tearing free: a rip and a hard landing
            Cue::BrokeFree => {
                self.spawn(Voice::noise(0.3, 0.35));
                self.spawn(Voice::tone(0.12, 90.0, 45.0, 0.4).after(0.25));
            }
            // the hunter has something: one sharp low note
            Cue::HunterAlert => self.spawn(Voice::tone(0.35, 180.0, 160.0, 0.3)),
            // on you: a rising two-note bark
            Cue::HunterChase => {
                self.spawn(Voice::tone(0.18, 220.0, 330.0, 0.4));
                self.spawn(Voice::tone(0.3, 330.0, 440.0, 0.4).after(0.2));
            }
            // lost you: a falling sigh
            Cue::HunterLost => self.spawn(Voice::tone(0.7, 300.0, 150.0, 0.2)),
            // a cat taken: a yowl cut short; yours is lower and longer
            Cue::CatCaught { player } => {
                if player {
                    self.spawn(Voice::tone(0.8, 500.0, 180.0, 0.45));
                    self.spawn(Voice::noise(0.4, 0.3).after(0.2));
                } else {
                    self.spawn(Voice::tone(0.4, 700.0, 350.0, 0.25));
                }
            }
            // out: a bright rising chirp, two for yourself
            Cue::CatEscaped { player } => {
                self.spawn(Voice::tone(0.25, 880.0, 1320.0, 0.2));
                if player {
                    self.spawn(Voice::tone(0.35, 1320.0, 1760.0, 0.22).after(0.2));
                }
            }
            // a mouthful: a soft crunch
            Cue::PreyPounced => {
                self.spawn(Voice::noise(0.08, 0.25));
                self.spawn(Voice::tone(0.1, 200.0, 90.0, 0.2));
            }
            // water
            Cue::Splash => {
                self.spawn(Voice::noise(0.4, 0.35));
                self.spawn(Voice::tone(0.3, 500.0, 200.0, 0.1));
            }
            // a landed combo: a quick rising triplet
            Cue::Combo => {
                for (k, f) in [660.0f32, 880.0, 1100.0].iter().enumerate() {
                    self.spawn(Voice::tone(0.12, *f, *f, 0.2).after(k as f32 * 0.07));
                }
            }
            // the pelt took it: a dull thump and a shimmer
            Cue::Shielded => {
                self.spawn(Voice::tone(0.15, 150.0, 80.0, 0.35));
                self.spawn(Voice::tone(0.4, 1500.0, 2000.0, 0.1).after(0.05));
            }
            // the sky turning: rain begins with a hush, a storm with a roll, clearing with
            // a single high note; snow and fog are silent arrivals
            Cue::SkyChange(sky) => match sky {
                Sky::Rain => self.spawn(Voice::noise(1.2, 0.18)),
                Sky::Storm => self.spawn(Voice::tone(1.6, 55.0, 30.0, 0.5)),
                Sky::Clear => self.spawn(Voice::tone(0.5, 1200.0, 1600.0, 0.12)),
                Sky::Overcast | Sky::Fog | Sky::Snow => {}
            },
            // the round: a rising major arpeggio, or a falling minor one
            Cue::Win => {
                for (k, f) in [523.25f32, 659.25, 783.99, 1046.5].iter().enumerate() {
                    self.spawn(Voice::tone(0.4, *f, *f, 0.18).after(k as f32 * 0.1));
                }
            }
            Cue::Lose => {
                for (k, f) in [440.0f32, 349.23, 261.63, 220.0].iter().enumerate() {
                    self.spawn(Voice::tone(0.5, *f, *f, 0.18).after(k as f32 * 0.14));
                }
            }
        }
    }

    pub fn sample(&mut self) -> f32 {
        let dt = 1.0 / self.sr;
        self.t += dt;
        // the air moves slowly: a storm arriving is a swell, not a switch
        let k_air = (dt * 1.5).min(1.0);
        let k_fast = (dt * 6.0).min(1.0);
        let a = self.air;
        let s = &mut self.air_s;
        s.wind += (a.wind - s.wind) * k_air;
        s.rain += (a.rain - s.rain) * k_air;
        s.storm += (a.storm - s.storm) * k_air;
        s.snow += (a.snow - s.snow) * k_air;
        s.tension += (a.tension - s.tension) * k_fast;
        s.running += (a.running - s.running) * k_fast;
        let s = self.air_s;

        let n = self.white();
        let mut out = 0.0f32;

        // WIND: noise through two low-passes, gain swelling on a slow wander
        self.wind_swell = 0.5 + 0.5 * (self.t * 0.37).sin() * (self.t * 0.11).cos();
        let kw = 1.0 - (-2.0 * std::f32::consts::PI * (300.0 + 700.0 * s.wind) * dt).exp();
        self.wind_lp += (n - self.wind_lp) * kw;
        self.wind_lp2 += (self.wind_lp - self.wind_lp2) * kw;
        out += self.wind_lp2 * s.wind * (0.3 + 0.7 * self.wind_swell) * 0.9;

        // RAIN: noise with the lows removed (a one-pole high-pass), dense and even
        let hp = n - self.rain_hp_prev + 0.97 * self.rain_hp;
        self.rain_hp_prev = n;
        self.rain_hp = hp;
        out += hp * s.rain * 0.12;

        // THUNDER: in a storm, a rumble every few seconds
        if s.storm > 0.5 {
            self.thunder_t -= dt;
            if self.thunder_t <= 0.0 {
                self.thunder_t = 5.0 + 7.0 * (0.5 + 0.5 * (self.t * 1.7).sin());
                self.thunders += 1;
                self.spawn(Voice::tone(1.8, 48.0, 28.0, 0.45 * s.storm));
                self.spawn(Voice::noise(1.2, 0.12).after(0.1));
            }
        }

        // TENSION: two detuned low sines, and past two thirds a heartbeat
        if s.tension > 0.01 {
            let f = 55.0 + 25.0 * s.tension;
            self.drone_phase[0] = (self.drone_phase[0] + f * dt).fract();
            self.drone_phase[1] = (self.drone_phase[1] + f * 1.01 * dt).fract();
            let d = (self.drone_phase[0] * std::f32::consts::TAU).sin() * 0.6 + (self.drone_phase[1] * std::f32::consts::TAU).sin() * 0.4;
            out += d * s.tension * 0.16;
            if s.tension > 0.66 {
                self.heart_t -= dt;
                if self.heart_t <= 0.0 {
                    let beat = 0.9 - 0.4 * s.tension; // faster as it climbs
                    self.heart_t = beat;
                    self.beats += 1;
                    self.spawn(Voice::tone(0.09, 70.0, 45.0, 0.3));
                    self.spawn(Voice::tone(0.07, 60.0, 40.0, 0.22).after(0.14));
                }
            }
        }

        // RUNNING: a breath every stride, soft noise
        if s.running > 0.05 {
            self.breath_t -= dt;
            if self.breath_t <= 0.0 {
                self.breath_t = 0.42;
                self.spawn(Voice::noise(0.18, 0.08 * s.running));
            }
        }

        // the snow hush: everything sits a little lower under snow
        out *= 1.0 - 0.3 * s.snow;

        // the one-shots
        let mut i = 0;
        while i < self.voices.len() {
            let vn = self.white();
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
            out += (tone * (1.0 - v.noise) + vn * v.noise) * env * v.amp;
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

#[cfg(all(feature = "sound", not(target_arch = "wasm32")))]
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
            let err = |e| eprintln!("[clawbound] audio stream error: {e}");
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

#[cfg(all(feature = "sound", not(target_arch = "wasm32")))]
pub use stream::Audio;

/// The silent build: the same handle, nothing behind it.
#[cfg(not(all(feature = "sound", not(target_arch = "wasm32"))))]
pub struct Audio;

#[cfg(not(all(feature = "sound", not(target_arch = "wasm32"))))]
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

    /// Still air is silence; wind, rain and tension each bring their bed up; the master at
    /// zero is exact silence.
    #[test]
    fn the_beds_follow_the_air_and_still_air_is_silent() {
        let (tx, mut s) = pair(44_100.0);
        assert_eq!(rms(&mut s, 0.3), 0.0, "no air, no sound");
        for (name, air) in [
            ("wind", Ambience { wind: 1.0, ..Default::default() }),
            ("rain", Ambience { rain: 1.0, ..Default::default() }),
            ("tension", Ambience { tension: 0.5, ..Default::default() }),
        ] {
            let (tx, mut s) = pair(44_100.0);
            tx.send(Msg::Air(air)).unwrap();
            s.drain();
            rms(&mut s, 2.0); // the swell
            let level = rms(&mut s, 1.0);
            assert!(level > 0.01 && level < 0.5, "{name}: {level}");
        }
        tx.send(Msg::Air(Ambience { wind: 1.0, ..Default::default() })).unwrap();
        tx.send(Msg::Master(0.0)).unwrap();
        s.drain();
        rms(&mut s, 2.0);
        assert_eq!(rms(&mut s, 0.5), 0.0, "muted is zero");
    }

    /// High tension adds a heartbeat; a storm adds thunder: both show up as voices spawned
    /// by the beds themselves, and a storm is louder than rain alone.
    #[test]
    fn tension_beats_and_a_storm_rumbles() {
        let (tx, mut s) = pair(44_100.0);
        tx.send(Msg::Air(Ambience { tension: 1.0, ..Default::default() })).unwrap();
        s.drain();
        rms(&mut s, 3.0);
        assert!((4..=8).contains(&s.beats), "a heart at half-second beats for three seconds: {}", s.beats);
        let (tx, mut calm) = pair(44_100.0);
        tx.send(Msg::Air(Ambience { tension: 0.4, ..Default::default() })).unwrap();
        calm.drain();
        rms(&mut calm, 3.0);
        assert_eq!(calm.beats, 0, "below two thirds the drone is all there is");
        let (tx, mut s) = pair(44_100.0);
        tx.send(Msg::Air(Ambience { rain: 1.0, storm: 1.0, wind: 0.5, ..Default::default() })).unwrap();
        s.drain();
        rms(&mut s, 3.0);
        let storm = rms(&mut s, 12.0);
        assert!((1..=3).contains(&s.thunders), "thunder every five to twelve seconds: {}", s.thunders);
        let (tx, mut r) = pair(44_100.0);
        tx.send(Msg::Air(Ambience { rain: 0.7, ..Default::default() })).unwrap();
        r.drain();
        rms(&mut r, 3.0);
        let rain = rms(&mut r, 6.0);
        assert!(storm > rain, "a storm is more than rain: {storm} vs {rain}");
    }

    /// Every cue makes a sound and dies away, the pool is bounded and the output in range.
    #[test]
    fn every_cue_sounds_and_then_stops_and_a_pile_of_them_clips() {
        let all = [
            Cue::Leap,
            Cue::Dodge,
            Cue::Snag,
            Cue::HawkGrab,
            Cue::BrokeFree,
            Cue::HunterAlert,
            Cue::HunterChase,
            Cue::HunterLost,
            Cue::CatCaught { player: true },
            Cue::CatCaught { player: false },
            Cue::CatEscaped { player: true },
            Cue::CatEscaped { player: false },
            Cue::PreyPounced,
            Cue::Splash,
            Cue::Combo,
            Cue::Shielded,
            Cue::SkyChange(Sky::Rain),
            Cue::SkyChange(Sky::Storm),
            Cue::SkyChange(Sky::Clear),
            Cue::Win,
            Cue::Lose,
        ];
        for c in all {
            let (tx, mut s) = pair(44_100.0);
            tx.send(Msg::Cue(c)).unwrap();
            s.drain();
            assert!(!s.voices.is_empty(), "{c:?} spawned nothing");
            let burst = rms(&mut s, 0.4);
            assert!(burst > 0.003, "{c:?} is inaudible: {burst}");
            rms(&mut s, 2.5);
            assert!(s.voices.is_empty(), "{c:?} left voices playing after three seconds");
        }
        for silent in [Cue::SkyChange(Sky::Snow), Cue::SkyChange(Sky::Fog), Cue::SkyChange(Sky::Overcast)] {
            let (tx, mut s) = pair(44_100.0);
            tx.send(Msg::Cue(silent)).unwrap();
            s.drain();
            assert!(s.voices.is_empty(), "{silent:?} is a silent arrival");
        }
        let (tx, mut s) = pair(44_100.0);
        for _ in 0..200 {
            tx.send(Msg::Cue(Cue::CatCaught { player: true })).unwrap();
        }
        s.drain();
        assert!(s.voices.len() <= MAX_VOICES);
        for _ in 0..4410 {
            let x = s.sample();
            assert!((-1.0..=1.0).contains(&x) && x.is_finite());
        }
    }
}
