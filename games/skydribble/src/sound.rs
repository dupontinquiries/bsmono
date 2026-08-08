//! All sound is synthesized live — engine, wind, boost, ball pings, ring
//! chimes, boost-pad blips, plus a soft generative pentatonic soundtrack.
//! No audio assets, no decoders: just cpal and a tiny mixer.
//!
//! Built only with the default `audio` feature; without it this module
//! compiles to a no-op with the same API, so the game runs silent instead of
//! failing on machines without ALSA headers.

#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(feature = "audio"), allow(dead_code))] // payloads unused by the silent stub
pub enum Sfx {
    Bounce(f32),   // ball vs world, arg = impact speed
    Touch(f32),    // car vs ball, arg = impact speed
    Chime(u32),    // ring pass, arg = combo count
    CarRing,       // car flew through a ring
    PadSmall,
    PadBig,
    Jump,
    Dodge,
    Land(f32),     // car landing, arg = impact speed
    Goal,          // ball in the net: a little chord fanfare
    Horn,          // team scored in a sparta match: stadium airhorn
}

// ======================================================================
#[cfg(feature = "audio")]
mod imp {
    use super::Sfx;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::Arc;

    pub struct Params {
        engine: AtomicU32, // f32 bits, 0..1
        wind: AtomicU32,   // f32 bits, 0..1
        // ---- adaptive-audio sense (the measurement seam) ---------------
        // Fed once per frame by the game from world measurements; the music
        // and mix read these to react. Extend here: per-event ducking,
        // proximity reverb sends, crowd walla layers, etc.
        prox: AtomicU32,    // 0..1 closeness to the ball
        tension: AtomicU32, // 0..1 ball near a goal mouth
        swarm: AtomicU32,   // 0..1 how mobbed the player is (bots nearby)
        boost: AtomicBool,
        slide: AtomicBool,
        master: AtomicU32, // f32 bits, 0..1
        music: AtomicBool,
    }

    fn setf(a: &AtomicU32, v: f32) {
        a.store(v.to_bits(), Ordering::Relaxed);
    }
    fn getf(a: &AtomicU32) -> f32 {
        f32::from_bits(a.load(Ordering::Relaxed))
    }

    pub struct Sound {
        _stream: Option<cpal::Stream>,
        params: Arc<Params>,
        tx: Sender<Sfx>,
        pub volume: f32,
        pub music_on: bool,
    }

    impl Sound {
        pub fn new(volume: f32, music_on: bool, muted: bool) -> Sound {
            let params = Arc::new(Params {
                engine: AtomicU32::new(0),
                wind: AtomicU32::new(0),
                prox: AtomicU32::new(0),
                tension: AtomicU32::new(0),
                swarm: AtomicU32::new(0),
                boost: AtomicBool::new(false),
                slide: AtomicBool::new(false),
                master: AtomicU32::new(if muted { 0.0f32 } else { volume }.to_bits()),
                music: AtomicBool::new(music_on),
            });
            let (tx, rx) = channel::<Sfx>();
            let stream = if muted { None } else { build_stream(params.clone(), rx) };
            if !muted && stream.is_none() {
                eprintln!("audio: no output device / unsupported format — running silent");
            }
            Sound { _stream: stream, params, tx, volume, music_on }
        }

        /// Adaptive-audio inputs, measured by the game each frame:
        /// `prox` = closeness to the ball, `tension` = ball near a goal,
        /// `swarm` = local bot crowding. The music leans on them below;
        /// this is the seam for any future event-driven mixing.
        pub fn set_sense(&self, prox: f32, tension: f32, swarm: f32) {
            setf(&self.params.prox, prox.clamp(0.0, 1.0));
            setf(&self.params.tension, tension.clamp(0.0, 1.0));
            setf(&self.params.swarm, swarm.clamp(0.0, 1.0));
        }

        pub fn set_state(&self, engine: f32, wind: f32, boost: bool, slide: bool) {
            setf(&self.params.engine, engine.clamp(0.0, 1.0));
            setf(&self.params.wind, wind.clamp(0.0, 1.0));
            self.params.boost.store(boost, Ordering::Relaxed);
            self.params.slide.store(slide, Ordering::Relaxed);
        }

        pub fn set_volume(&mut self, v: f32) {
            self.volume = v.clamp(0.0, 1.0);
            setf(&self.params.master, self.volume);
        }

        pub fn set_music(&mut self, on: bool) {
            self.music_on = on;
            self.params.music.store(on, Ordering::Relaxed);
        }

        pub fn fire(&self, s: Sfx) {
            let _ = self.tx.send(s);
        }
    }

    fn build_stream(params: Arc<Params>, rx: Receiver<Sfx>) -> Option<cpal::Stream> {
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let cfg = device.default_output_config().ok()?;
        let sr = cfg.sample_rate().0 as f32;
        let ch = cfg.channels() as usize;
        let mut synth = Synth::new(sr, params, rx);
        let err = |e| eprintln!("audio stream error: {e}");
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => device
                .build_output_stream(
                    &cfg.into(),
                    move |data: &mut [f32], _| {
                        for frame in data.chunks_mut(ch) {
                            let s = synth.sample();
                            for out in frame.iter_mut() {
                                *out = s;
                            }
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
                        for frame in data.chunks_mut(ch) {
                            let s = (synth.sample().clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                            for out in frame.iter_mut() {
                                *out = s;
                            }
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
                        for frame in data.chunks_mut(ch) {
                            let s = synth.sample().clamp(-1.0, 1.0);
                            let v = ((s * 0.5 + 0.5) * u16::MAX as f32) as u16;
                            for out in frame.iter_mut() {
                                *out = v;
                            }
                        }
                    },
                    err,
                    None,
                )
                .ok()?,
            _ => return None,
        };
        stream.play().ok()?;
        Some(stream)
    }

    // ------------------------------------------------------------------
    struct Voice {
        t: f32,
        dur: f32,
        f0: f32,
        f1: f32,     // pitch glides f0 -> f1 over the voice
        amp: f32,
        noise: f32,  // 0 = pure tone, 1 = pure noise
        phase: f32,
        second: f32, // extra partial mix (adds sparkle to chimes)
    }

    struct Synth {
        sr: f32,
        params: Arc<Params>,
        rx: Receiver<Sfx>,
        voices: Vec<Voice>,
        // continuous layers
        eng_phase: f32,
        eng_phase2: f32,
        eng_lp: f32,
        wind_lp: f32,
        boost_lp: f32,
        g_engine: f32,
        g_wind: f32,
        g_boost: f32,
        g_slide: f32,
        noise_state: u32,
        // music
        beat_t: f32,
        beat_len: f32,
        step: u32,
        pad_phase: [f32; 3],
        g_music: f32,
        poll: u32,
    }

    impl Synth {
        fn new(sr: f32, params: Arc<Params>, rx: Receiver<Sfx>) -> Synth {
            Synth {
                sr,
                params,
                rx,
                voices: Vec::with_capacity(32),
                eng_phase: 0.0,
                eng_phase2: 0.0,
                eng_lp: 0.0,
                wind_lp: 0.0,
                boost_lp: 0.0,
                g_engine: 0.0,
                g_wind: 0.0,
                g_boost: 0.0,
                g_slide: 0.0,
                noise_state: 0x12345,
                beat_t: 0.0,
                beat_len: 0.42,
                step: 0,
                pad_phase: [0.0; 3],
                g_music: 0.0,
                poll: 0,
            }
        }

        fn noise(&mut self) -> f32 {
            // xorshift white noise
            let mut x = self.noise_state;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.noise_state = x;
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        }

        fn spawn(&mut self, v: Voice) {
            if self.voices.len() < 32 {
                self.voices.push(v);
            }
        }

        fn handle(&mut self, s: Sfx) {
            match s {
                Sfx::Bounce(sp) => {
                    let a = (sp / 18.0).clamp(0.05, 1.0);
                    self.spawn(Voice { t: 0.0, dur: 0.22 + a * 0.1, f0: 150.0 + a * 90.0, f1: 80.0, amp: 0.5 * a, noise: 0.12, phase: 0.0, second: 0.3 });
                }
                Sfx::Touch(sp) => {
                    let a = (sp / 14.0).clamp(0.06, 1.0);
                    self.spawn(Voice { t: 0.0, dur: 0.09, f0: 95.0, f1: 60.0, amp: 0.42 * a, noise: 0.45, phase: 0.0, second: 0.0 });
                }
                Sfx::Chime(combo) => {
                    let lift = 1.0 + 0.059 * combo.min(9) as f32; // pitch climbs with the combo
                    self.spawn(Voice { t: 0.0, dur: 0.55, f0: 690.0 * lift, f1: 690.0 * lift, amp: 0.30, noise: 0.0, phase: 0.0, second: 0.55 });
                    self.spawn(Voice { t: 0.0, dur: 0.75, f0: 1035.0 * lift, f1: 1035.0 * lift, amp: 0.22, noise: 0.0, phase: 0.0, second: 0.4 });
                }
                Sfx::CarRing => {
                    self.spawn(Voice { t: 0.0, dur: 0.3, f0: 520.0, f1: 640.0, amp: 0.14, noise: 0.05, phase: 0.0, second: 0.3 });
                }
                Sfx::PadSmall => {
                    self.spawn(Voice { t: 0.0, dur: 0.1, f0: 900.0, f1: 1150.0, amp: 0.16, noise: 0.0, phase: 0.0, second: 0.2 });
                }
                Sfx::PadBig => {
                    self.spawn(Voice { t: 0.0, dur: 0.4, f0: 320.0, f1: 980.0, amp: 0.26, noise: 0.0, phase: 0.0, second: 0.35 });
                }
                Sfx::Jump => {
                    self.spawn(Voice { t: 0.0, dur: 0.13, f0: 220.0, f1: 130.0, amp: 0.2, noise: 0.5, phase: 0.0, second: 0.0 });
                }
                Sfx::Dodge => {
                    self.spawn(Voice { t: 0.0, dur: 0.18, f0: 420.0, f1: 120.0, amp: 0.24, noise: 0.75, phase: 0.0, second: 0.0 });
                }
                Sfx::Horn => {
                    for (f, det) in [(174.6, 0.0), (176.1, 0.2), (87.3, 0.1)] {
                        self.spawn(Voice { t: 0.0, dur: 0.85, f0: f + det, f1: f * 0.97, amp: 0.30, noise: 0.10, phase: 0.0, second: 0.65 });
                    }
                }
                Sfx::Goal => {
                    for (f, amp, dur) in [(523.25, 0.26, 1.0), (659.25, 0.20, 0.9), (783.99, 0.18, 1.2), (130.8, 0.22, 0.6)] {
                        self.spawn(Voice { t: 0.0, dur, f0: f, f1: f, amp, noise: 0.0, phase: 0.0, second: 0.5 });
                    }
                }
                Sfx::Land(sp) => {
                    let a = (sp / 12.0).clamp(0.0, 1.0);
                    if a > 0.08 {
                        self.spawn(Voice { t: 0.0, dur: 0.14, f0: 70.0, f1: 45.0, amp: 0.5 * a, noise: 0.6, phase: 0.0, second: 0.0 });
                    }
                }
            }
        }

        fn music_step(&mut self) {
            // gentle pentatonic pluck, wandering by xorshift, resting often
            let scale = [0.0f32, 3.0, 5.0, 7.0, 10.0, 12.0, 15.0, 17.0];
            self.step = self.step.wrapping_add(1);
            let mut x = self.noise_state;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.noise_state = x;
            if x % 5 == 0 {
                return; // rest beat
            }
            let prox = getf(&self.params.prox);
            let tension = getf(&self.params.tension);
            let swarm = getf(&self.params.swarm);
            let deg = scale[(x >> 8) as usize % scale.len()];
            let base = 220.0 * (2.0f32).powf(deg / 12.0);
            // proximity leans the plucks in; tension adds a high shimmer;
            // a mob adds war-drum weight. (The adaptive-audio seam in action.)
            self.spawn(Voice { t: 0.0, dur: 0.9, f0: base, f1: base, amp: 0.085 * (1.0 + prox * 0.5), noise: 0.0, phase: 0.0, second: 0.25 });
            if tension > 0.4 && (x >> 16) % 3 == 0 {
                self.spawn(Voice { t: 0.0, dur: 0.5, f0: base * 3.0, f1: base * 3.0, amp: 0.05 * tension, noise: 0.0, phase: 0.0, second: 0.6 });
            }
            if self.step % 8 == 0 || (swarm > 0.3 && self.step % 4 == 0) {
                self.spawn(Voice { t: 0.0, dur: 2.4, f0: 110.0, f1: 110.0, amp: 0.05 + swarm * 0.05, noise: swarm * 0.15, phase: 0.0, second: 0.2 });
            }
        }

        fn sample(&mut self) -> f32 {
            // poll control messages every 64 samples
            self.poll = self.poll.wrapping_add(1);
            if self.poll & 63 == 0 {
                while let Ok(s) = self.rx.try_recv() {
                    self.handle(s);
                }
            }

            let master = super::imp::getf(&self.params.master);
            let engine_amt = super::imp::getf(&self.params.engine);
            let wind_amt = super::imp::getf(&self.params.wind);
            let boost_on = self.params.boost.load(std::sync::atomic::Ordering::Relaxed);
            let slide_on = self.params.slide.load(std::sync::atomic::Ordering::Relaxed);
            let music_on = self.params.music.load(std::sync::atomic::Ordering::Relaxed);

            // gain smoothing to avoid clicks
            let k = 0.0015;
            self.g_engine += (engine_amt - self.g_engine) * k;
            self.g_wind += (wind_amt - self.g_wind) * k;
            self.g_boost += ((boost_on as u32 as f32) - self.g_boost) * (k * 3.0);
            self.g_slide += ((slide_on as u32 as f32) - self.g_slide) * (k * 3.0);
            self.g_music += ((music_on as u32 as f32) - self.g_music) * k;

            let dt = 1.0 / self.sr;
            let mut out = 0.0;

            // ---- engine: two detuned saws through a one-pole low pass ----
            let f0 = 42.0 + self.g_engine * 118.0;
            self.eng_phase = (self.eng_phase + f0 * dt).fract();
            self.eng_phase2 = (self.eng_phase2 + f0 * 1.011 * dt).fract();
            let raw = (self.eng_phase * 2.0 - 1.0) * 0.6 + (self.eng_phase2 * 2.0 - 1.0) * 0.4;
            let cut = (0.02 + self.g_engine * 0.10).min(0.5);
            self.eng_lp += (raw - self.eng_lp) * cut;
            out += self.eng_lp * self.g_engine * 0.30;

            // ---- wind: filtered noise, louder with speed ----
            let n = self.noise();
            self.wind_lp += (n - self.wind_lp) * 0.045;
            out += self.wind_lp * (0.04 + self.g_wind * 0.5) * 0.5;

            // ---- boost: brighter hiss ----
            self.boost_lp += (n - self.boost_lp) * 0.30;
            out += (n - self.boost_lp) * self.g_boost * 0.23;
            // ---- powerslide scrub ----
            out += self.wind_lp * self.g_slide * self.g_engine * 0.35;

            // ---- music ----
            if self.g_music > 0.003 {
                self.beat_t += dt;
                if self.beat_t >= self.beat_len {
                    self.beat_t -= self.beat_len;
                    self.music_step();
                }
                let mut pad = 0.0;
                for (i, f) in [110.0f32, 164.81, 220.0].iter().enumerate() {
                    self.pad_phase[i] = (self.pad_phase[i] + f * dt).fract();
                    pad += (self.pad_phase[i] * std::f32::consts::TAU).sin();
                }
                let breathe = ((self.step as f32 * 0.7 + self.beat_t * 1.5).sin() * 0.5 + 0.5) * 0.4 + 0.6;
                out += pad * 0.016 * breathe * self.g_music;
            }

            // ---- one-shot voices ----
            let mut i = 0;
            while i < self.voices.len() {
                let v = &mut self.voices[i];
                v.t += dt;
                if v.t >= v.dur {
                    self.voices.swap_remove(i);
                    continue;
                }
                let u = v.t / v.dur;
                let env = (1.0 - u).powf(2.2);
                let f = v.f0 + (v.f1 - v.f0) * u;
                v.phase = (v.phase + f * dt).fract();
                let tone = (v.phase * std::f32::consts::TAU).sin()
                    + (v.phase * 2.0 * std::f32::consts::TAU).sin() * v.second;
                let nz = {
                    let mut x = self.noise_state;
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    self.noise_state = x;
                    (x as f32 / u32::MAX as f32) * 2.0 - 1.0
                };
                out += (tone * (1.0 - v.noise) + nz * v.noise) * env * v.amp;
                i += 1;
            }

            // soft clip
            let s = out * master;
            s / (1.0 + s.abs() * 0.6)
        }
    }
}

// ======================================================================
#[cfg(not(feature = "audio"))]
mod imp {
    use super::Sfx;

    /// Silent stand-in with the same surface as the real thing.
    pub struct Sound {
        pub volume: f32,
        pub music_on: bool,
    }

    impl Sound {
        pub fn new(volume: f32, music_on: bool, _muted: bool) -> Sound {
            Sound { volume, music_on }
        }
        pub fn set_state(&self, _e: f32, _w: f32, _b: bool, _s: bool) {}
        pub fn set_sense(&self, _p: f32, _t: f32, _s: f32) {}
        pub fn set_volume(&mut self, v: f32) {
            self.volume = v.clamp(0.0, 1.0);
        }
        pub fn set_music(&mut self, on: bool) {
            self.music_on = on;
        }
        pub fn fire(&self, _s: Sfx) {}
    }
}

pub use imp::Sound;
