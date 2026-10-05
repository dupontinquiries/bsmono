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
    /// servo-sweep over the whole blend — true = folding up into the robot
    /// (rising), false = folding down into the vehicle (falling)
    XformFold(bool),
    /// the metal clank on the frame the fold actually lands
    XformSnap,
    // ── combat. The game had a gun, a sabre, section damage and a death and not one sound for
    // any of it until c21: every cue above is the air-dribble heritage. All synthesized like
    // the rest; the voices are a first pass nobody has heard in this sandbox.
    /// the weapon leaves: the verb's delivery decides the voice (`weapons::Verb::delivery`)
    Shot(crate::ballistics::Delivery),
    /// the sabre arc, whether or not it connected
    Swing,
    /// your shot lands on a bot, arg = damage
    Strike(f32),
    /// a hit on YOUR body, arg = damage
    Hurt(f32),
    /// structural failure: the body goes down
    Down,
    /// the field rebuild stands the body back up
    Rebuilt,
    /// a console / menu row moved or pressed
    Click,
}

/// A sound event plus where it landed in the mix.
///
/// The MIXING happens on the game thread, in `mixer` — pure, tested, and
/// checkable in a sandbox with no audio device. What crosses to the audio
/// thread is the ANSWER: two channel gains and a filter cutoff. That keeps
/// the callback dumb (no world queries, no allocation, no distance math on
/// the realtime thread) and keeps the part that can be wrong in the part
/// that can be tested.
#[derive(Clone, Copy, Debug)]
pub struct Cue {
    pub sfx: Sfx,
    pub gain_l: f32,
    pub gain_r: f32,
    /// One-pole cutoff in Hz; 20 kHz means "open", i.e. no filtering.
    pub cutoff_hz: f32,
}

impl Cue {
    /// Centred, unfiltered, full gain — the right answer for anything
    /// attached to the listener: your own engine, your own jump, the UI.
    pub fn centred(sfx: Sfx) -> Cue {
        // equal-power centre, so a centred cue and a panned one at the same
        // distance carry the same power rather than the centre being louder
        let c = std::f32::consts::FRAC_1_SQRT_2;
        Cue { sfx, gain_l: c, gain_r: c, cutoff_hz: 20_000.0 }
    }

    /// From a mixed voice — position, occlusion and all.
    pub fn at(sfx: Sfx, v: &crate::mixer::Voice) -> Cue {
        Cue { sfx, gain_l: v.gain_l, gain_r: v.gain_r, cutoff_hz: v.cutoff_hz }
    }
}

// ======================================================================
#[cfg(feature = "audio")]
mod imp {
    use super::{Cue, Sfx};
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
        tx: Sender<Cue>,
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
            let (tx, rx) = channel::<Cue>();
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

        /// A non-positional cue: your own body, your own UI. Centred.
        pub fn fire(&self, s: Sfx) {
            let _ = self.tx.send(Cue::centred(s));
        }

        /// A cue that happened SOMEWHERE — already mixed by `mixer`.
        pub fn fire_cue(&self, c: Cue) {
            let _ = self.tx.send(c);
        }
    }

    /// Lay a stereo pair into however many channels the device actually has.
    ///
    /// MONO DEVICES MUST NOT LOSE THE RIGHT CHANNEL. A 1-channel device (and
    /// they exist — some Bluetooth headsets, some phone speakers) would
    /// otherwise be handed `l` only, which silently deletes everything panned
    /// hard right. Downmix instead. Beyond two channels the extra outputs
    /// (centre, LFE, surrounds) are left silent rather than fed a copy: a
    /// duplicate in the surrounds collapses the image the pan just created.
    fn write_frame<T: Copy>(frame: &mut [T], l: f32, r: f32, conv: impl Fn(f32) -> T) {
        match frame.len() {
            0 => {}
            1 => frame[0] = conv((l + r) * std::f32::consts::FRAC_1_SQRT_2),
            _ => {
                frame[0] = conv(l);
                frame[1] = conv(r);
                for out in frame.iter_mut().skip(2) {
                    *out = conv(0.0);
                }
            }
        }
    }

    fn build_stream(params: Arc<Params>, rx: Receiver<Cue>) -> Option<cpal::Stream> {
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
                            let (l, r) = synth.sample();
                            write_frame(frame, l, r, |v| v);
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
                            let (l, r) = synth.sample();
                            write_frame(frame, l, r, |v| (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
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
                            let (l, r) = synth.sample();
                            write_frame(frame, l, r, |v| {
                                ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16
                            });
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
        // ---- placement, from the Cue that spawned it -------------------
        // Per-voice rather than per-frame: a chime fired while you were
        // facing it should FINISH where it started, not swing across the
        // stereo field because the camera turned during its tail.
        gain_l: f32,
        gain_r: f32,
        /// one-pole coefficient for the muffle filter; 1.0 = open
        lp_k: f32,
        lp: f32,
    }

    impl Voice {
        /// A voice with no placement yet. `Synth::spawn` stamps the live
        /// cue's gains onto it — every literal in `handle` describes the
        /// SOUND, and nothing in there should have to know about panning.
        fn unplaced() -> Voice {
            Voice {
                t: 0.0, dur: 0.0, f0: 0.0, f1: 0.0, amp: 0.0, noise: 0.0,
                phase: 0.0, second: 0.0,
                gain_l: std::f32::consts::FRAC_1_SQRT_2,
                gain_r: std::f32::consts::FRAC_1_SQRT_2,
                lp_k: 1.0, lp: 0.0,
            }
        }
    }

    struct Synth {
        sr: f32,
        /// placement of the cue currently being handled; `spawn` reads it
        cue_l: f32,
        cue_r: f32,
        cue_k: f32,
        params: Arc<Params>,
        rx: Receiver<Cue>,
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
        fn new(sr: f32, params: Arc<Params>, rx: Receiver<Cue>) -> Synth {
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
                cue_l: std::f32::consts::FRAC_1_SQRT_2,
                cue_r: std::f32::consts::FRAC_1_SQRT_2,
                cue_k: 1.0,
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

        fn spawn(&mut self, mut v: Voice) {
            if self.voices.len() < 32 {
                // The cue being handled decides where this voice sits. Stamped
                // once, at spawn, and never revisited: a chime fired while you
                // faced it should finish where it started rather than sweep
                // across the field because the camera turned during its tail.
                v.gain_l = self.cue_l;
                v.gain_r = self.cue_r;
                v.lp_k = self.cue_k;
                v.lp = 0.0;
                self.voices.push(v);
            }
        }

        /// One-pole low-pass coefficient for a cutoff in Hz.
        /// At/above Nyquist the filter is open and this is exactly 1.0, so an
        /// unoccluded cue costs nothing and cannot tint the sound.
        fn lp_coeff(&self, cutoff_hz: f32) -> f32 {
            if !cutoff_hz.is_finite() || cutoff_hz >= self.sr * 0.5 {
                return 1.0;
            }
            let k = 1.0 - (-std::f32::consts::TAU * cutoff_hz / self.sr).exp();
            k.clamp(0.0, 1.0)
        }

        fn handle(&mut self, c: Cue) {
            self.cue_l = if c.gain_l.is_finite() { c.gain_l.clamp(0.0, 4.0) } else { 0.0 };
            self.cue_r = if c.gain_r.is_finite() { c.gain_r.clamp(0.0, 4.0) } else { 0.0 };
            self.cue_k = self.lp_coeff(c.cutoff_hz);
            let s = c.sfx;
            match s {
                Sfx::Bounce(sp) => {
                    let a = (sp / 18.0).clamp(0.05, 1.0);
                    self.spawn(Voice { t: 0.0, dur: 0.22 + a * 0.1, f0: 150.0 + a * 90.0, f1: 80.0, amp: 0.5 * a, noise: 0.12, phase: 0.0, second: 0.3, ..Voice::unplaced() });
                }
                Sfx::Touch(sp) => {
                    let a = (sp / 14.0).clamp(0.06, 1.0);
                    self.spawn(Voice { t: 0.0, dur: 0.09, f0: 95.0, f1: 60.0, amp: 0.42 * a, noise: 0.45, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                }
                Sfx::Chime(combo) => {
                    let lift = 1.0 + 0.059 * combo.min(9) as f32; // pitch climbs with the combo
                    self.spawn(Voice { t: 0.0, dur: 0.55, f0: 690.0 * lift, f1: 690.0 * lift, amp: 0.30, noise: 0.0, phase: 0.0, second: 0.55, ..Voice::unplaced() });
                    self.spawn(Voice { t: 0.0, dur: 0.75, f0: 1035.0 * lift, f1: 1035.0 * lift, amp: 0.22, noise: 0.0, phase: 0.0, second: 0.4, ..Voice::unplaced() });
                }
                Sfx::CarRing => {
                    self.spawn(Voice { t: 0.0, dur: 0.3, f0: 520.0, f1: 640.0, amp: 0.14, noise: 0.05, phase: 0.0, second: 0.3, ..Voice::unplaced() });
                }
                Sfx::PadSmall => {
                    self.spawn(Voice { t: 0.0, dur: 0.1, f0: 900.0, f1: 1150.0, amp: 0.16, noise: 0.0, phase: 0.0, second: 0.2, ..Voice::unplaced() });
                }
                Sfx::PadBig => {
                    self.spawn(Voice { t: 0.0, dur: 0.4, f0: 320.0, f1: 980.0, amp: 0.26, noise: 0.0, phase: 0.0, second: 0.35, ..Voice::unplaced() });
                }
                Sfx::Jump => {
                    self.spawn(Voice { t: 0.0, dur: 0.13, f0: 220.0, f1: 130.0, amp: 0.2, noise: 0.5, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                }
                Sfx::Dodge => {
                    self.spawn(Voice { t: 0.0, dur: 0.18, f0: 420.0, f1: 120.0, amp: 0.24, noise: 0.75, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                }
                Sfx::Horn => {
                    for (f, det) in [(174.6, 0.0), (176.1, 0.2), (87.3, 0.1)] {
                        self.spawn(Voice { t: 0.0, dur: 0.85, f0: f + det, f1: f * 0.97, amp: 0.30, noise: 0.10, phase: 0.0, second: 0.65, ..Voice::unplaced() });
                    }
                }
                Sfx::Goal => {
                    for (f, amp, dur) in [(523.25, 0.26, 1.0), (659.25, 0.20, 0.9), (783.99, 0.18, 1.2), (130.8, 0.22, 0.6)] {
                        self.spawn(Voice { t: 0.0, dur, f0: f, f1: f, amp, noise: 0.0, phase: 0.0, second: 0.5, ..Voice::unplaced() });
                    }
                }
                Sfx::Land(sp) => {
                    let a = (sp / 12.0).clamp(0.0, 1.0);
                    if a > 0.08 {
                        self.spawn(Voice { t: 0.0, dur: 0.14, f0: 70.0, f1: 45.0, amp: 0.5 * a, noise: 0.6, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                    }
                }
                Sfx::XformFold(to_robot) => {
                    // one long glide for the whole 1.3s blend, plus a
                    // detuned octave-up layer so it reads as machinery
                    // rather than a single clean tone. Direction carries
                    // the meaning: rising into the robot, falling into
                    // the vehicle — the ear gets which way it's folding
                    // before the eye finishes the shot.
                    let (f0, f1) = if to_robot { (110.0, 360.0) } else { (360.0, 110.0) };
                    self.spawn(Voice { t: 0.0, dur: 1.15, f0, f1, amp: 0.22, noise: 0.35, phase: 0.0, second: 0.5, ..Voice::unplaced() });
                    self.spawn(Voice {
                        t: 0.0,
                        dur: 1.05,
                        f0: f0 * 2.02,
                        f1: f1 * 2.02,
                        amp: 0.10,
                        noise: 0.25,
                        phase: 0.31,
                        second: 0.0,
                        ..Voice::unplaced()
                    });
                }
                Sfx::XformSnap => {
                    // the landing beat: a bright metallic transient over a
                    // short low thump, fired the frame the fold finishes
                    self.spawn(Voice { t: 0.0, dur: 0.16, f0: 1500.0, f1: 220.0, amp: 0.45, noise: 0.55, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                    self.spawn(Voice { t: 0.0, dur: 0.28, f0: 95.0, f1: 55.0, amp: 0.35, noise: 0.2, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                }
                Sfx::Shot(deliv) => {
                    use crate::ballistics::Delivery as D;
                    match deliv {
                        // mass thrown: a crack that drops fast, over a short low thump
                        D::Kinetic => {
                            self.spawn(Voice { t: 0.0, dur: 0.09, f0: 1900.0, f1: 240.0, amp: 0.42, noise: 0.7, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                            self.spawn(Voice { t: 0.0, dur: 0.18, f0: 110.0, f1: 60.0, amp: 0.3, noise: 0.3, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                        }
                        // a pressure event: a boom with a long tail
                        D::Blast => {
                            self.spawn(Voice { t: 0.0, dur: 0.45, f0: 140.0, f1: 40.0, amp: 0.5, noise: 0.6, phase: 0.0, second: 0.2, ..Voice::unplaced() });
                        }
                        // light: a steady high tone with a hiss, held a little
                        D::Beam => {
                            self.spawn(Voice { t: 0.0, dur: 0.3, f0: 1200.0, f1: 1500.0, amp: 0.22, noise: 0.4, phase: 0.0, second: 0.45, ..Voice::unplaced() });
                        }
                        // contact: the swing voice carries it
                        D::Melee => {
                            self.spawn(Voice { t: 0.0, dur: 0.16, f0: 300.0, f1: 900.0, amp: 0.2, noise: 0.85, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                        }
                    }
                }
                Sfx::Swing => {
                    // a whoosh: filtered noise that rises and dies
                    self.spawn(Voice { t: 0.0, dur: 0.2, f0: 250.0, f1: 1100.0, amp: 0.24, noise: 0.9, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                }
                Sfx::Strike(dmg) => {
                    let a = (dmg / 40.0).clamp(0.15, 1.0);
                    self.spawn(Voice { t: 0.0, dur: 0.12, f0: 700.0, f1: 180.0, amp: 0.35 * a, noise: 0.5, phase: 0.0, second: 0.2, ..Voice::unplaced() });
                }
                Sfx::Hurt(dmg) => {
                    // a dull thud at the listener, heavier with the hit
                    let a = (dmg / 45.0).clamp(0.2, 1.0);
                    self.spawn(Voice { t: 0.0, dur: 0.2 + 0.1 * a, f0: 90.0, f1: 40.0, amp: 0.5 * a, noise: 0.45, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                    self.spawn(Voice { t: 0.0, dur: 0.08, f0: 2400.0, f1: 600.0, amp: 0.18 * a, noise: 0.6, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                }
                Sfx::Down => {
                    // the frame gives: a long falling groan under a rattle
                    self.spawn(Voice { t: 0.0, dur: 1.1, f0: 180.0, f1: 35.0, amp: 0.4, noise: 0.3, phase: 0.0, second: 0.5, ..Voice::unplaced() });
                    self.spawn(Voice { t: 0.0, dur: 0.5, f0: 900.0, f1: 120.0, amp: 0.25, noise: 0.8, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                }
                Sfx::Rebuilt => {
                    // the fold-snap's clank under a rising glide (a voice's `t` starts at 0:
                    // a negative start is louder, not later, so no staggered notes here)
                    self.spawn(Voice { t: 0.0, dur: 0.14, f0: 1400.0, f1: 300.0, amp: 0.35, noise: 0.5, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                    self.spawn(Voice { t: 0.0, dur: 0.5, f0: 392.0, f1: 659.25, amp: 0.22, noise: 0.0, phase: 0.0, second: 0.4, ..Voice::unplaced() });
                }
                Sfx::Click => {
                    self.spawn(Voice { t: 0.0, dur: 0.04, f0: 1800.0, f1: 1200.0, amp: 0.14, noise: 0.2, phase: 0.0, second: 0.0, ..Voice::unplaced() });
                }
            }
        }

        fn music_step(&mut self) {
            // The score is not IN the world: reset placement to centred and
            // open, or the pad inherits the pan and muffle of whichever shot
            // happened to be the last cue handled.
            self.cue_l = std::f32::consts::FRAC_1_SQRT_2;
            self.cue_r = std::f32::consts::FRAC_1_SQRT_2;
            self.cue_k = 1.0;
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
            self.spawn(Voice { t: 0.0, dur: 0.9, f0: base, f1: base, amp: 0.085 * (1.0 + prox * 0.5), noise: 0.0, phase: 0.0, second: 0.25, ..Voice::unplaced() });
            if tension > 0.4 && (x >> 16) % 3 == 0 {
                self.spawn(Voice { t: 0.0, dur: 0.5, f0: base * 3.0, f1: base * 3.0, amp: 0.05 * tension, noise: 0.0, phase: 0.0, second: 0.6, ..Voice::unplaced() });
            }
            if self.step % 8 == 0 || (swarm > 0.3 && self.step % 4 == 0) {
                self.spawn(Voice { t: 0.0, dur: 2.4, f0: 110.0, f1: 110.0, amp: 0.05 + swarm * 0.05, noise: swarm * 0.15, phase: 0.0, second: 0.2, ..Voice::unplaced() });
            }
        }

        /// One stereo frame.
        ///
        /// The continuous layers — engine, wind, boost, slide, the pad — stay
        /// CENTRED on purpose: they are your own machine and your own head,
        /// not things in the world, and panning them would make the cockpit
        /// swim. Only one-shot voices carry placement.
        fn sample(&mut self) -> (f32, f32) {
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
            let mut out = 0.0;              // centred bus (engine/wind/music)
            let (mut vl, mut vr) = (0.0f32, 0.0f32); // placed one-shots

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
                let raw = (tone * (1.0 - v.noise) + nz * v.noise) * env * v.amp;
                // muffle: one-pole low-pass, open (k=1) when unoccluded
                v.lp += (raw - v.lp) * v.lp_k;
                let sig = v.lp;
                vl += sig * v.gain_l;
                vr += sig * v.gain_r;
                i += 1;
            }

            // centred bus + placed voices, soft-clipped per channel
            let c = out * std::f32::consts::FRAC_1_SQRT_2;
            let clip = |x: f32| { let s = x * master; s / (1.0 + s.abs() * 0.6) };
            (clip(c + vl), clip(c + vr))
        }
    }
}

// ======================================================================
#[cfg(not(feature = "audio"))]
mod imp {
    use super::{Cue, Sfx};

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
        pub fn fire_cue(&self, _c: Cue) {}
    }
}

pub use imp::Sound;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_centred_cue_carries_the_same_power_as_a_panned_one() {
        // If centred cues were (1.0, 1.0) they would be 3 dB louder than
        // anything the mixer places, and every UI blip would duck the world.
        let c = Cue::centred(Sfx::Jump);
        let p = c.gain_l * c.gain_l + c.gain_r * c.gain_r;
        assert!((p - 1.0).abs() < 1e-5, "centred power {p} != 1.0");
        assert_eq!(c.cutoff_hz, 20_000.0, "a centred cue must not be filtered");
    }

    #[test]
    fn a_cue_carries_the_mixers_placement_through_unchanged() {
        // The seam: mixer decides, sound.rs applies. A Cue that quietly
        // dropped the pan or the cutoff would leave the mixer's tests green
        // and the game still mono — the exact failure this test exists for.
        let l = crate::mixer::Listener {
            pos: [0.0; 3],
            forward: [0.0, 0.0, 1.0],
            right: [1.0, 0.0, 0.0],
        };
        let mut e = crate::mixer::Emitter::shot(0, [20.0, 0.0, 4.0], 1.0);
        e.occluded_by = 1.5;
        let v = crate::mixer::mix(&l, &[e], 1);
        let v = v.first().expect("the emitter was audible");
        let c = Cue::at(Sfx::Bounce(4.0), v);
        assert_eq!(c.gain_l, v.gain_l);
        assert_eq!(c.gain_r, v.gain_r);
        assert_eq!(c.cutoff_hz, v.cutoff_hz);
        // and it really is placed: hard-ish right, and muffled
        assert!(c.gain_r > c.gain_l * 1.5, "a source on the right did not pan right");
        assert!(c.cutoff_hz < 20_000.0, "an occluded source was not muffled");
    }
}
