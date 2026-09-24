// Procedural ambience: wind that howls through whatever shape you're standing
// in, rain, thunder, and a reverb that opens up in the right places.
//
// Layering:
//   Acoustics   headless: where am I → how the wind howls here + how much the
//               place echoes (`acoustics_at`). Pure geometry on consts.
//   WindSynth   headless DSP: noise → shaped resonant bands → Schroeder
//               reverb. `render()` fills an interleaved f32 buffer. Tested.
//   Output      the cpal shell (feature "audio"): one output stream, a shared
//               `AudioParams` the game thread writes once a frame.
//
// The howl is "shape-based" in the sense that the resonant band the wind
// excites depends on the geometry around the listener: a whistle over open
// water, a hollow moan under stone and between ruin walls, a broad hiss in
// dense canopy, a high keening along the cliff rim — and the gust front
// drives both pitch (rising with the gust) and amplitude.

use crate::consts::{lane_clearance, river_dist, water_depth, Team, ARENA_RADIUS};
use glam::Vec3;

/// What the game thread hands the synth each frame.
#[derive(Clone, Copy, Debug)]
pub struct AudioParams {
    /// steady wind 0..1 (from |Atmosphere.wind| / ~10)
    pub wind: f32,
    /// gust front 0..1
    pub gust: f32,
    pub rain: f32,
    pub snow: f32,
    pub night: f32,
    /// -1 (left) .. 1 (right): where the wind comes from relative to the camera
    pub wind_pan: f32,
    /// place-derived, see `Acoustics`
    pub howl_hz: f32,
    pub howl_q: f32,
    pub howl_gain: f32,
    pub reverb: f32,
    /// monotonically increasing counter; the synth fires a thunder roll when it changes
    pub thunder_count: u32,
    /// how far away the last strike was (0 = on top of you, 1 = far)
    pub thunder_dist: f32,
    /// 0..1: how much water is around the listener (lapping, drips)
    pub water: f32,
    pub master: f32,
    /// monotonically increasing counter; the synth starts the intro
    /// fanfare (`brass`) when it changes
    pub fanfare_count: u32,
}

impl Default for AudioParams {
    fn default() -> Self {
        AudioParams {
            wind: 0.15,
            gust: 0.0,
            rain: 0.0,
            snow: 0.0,
            night: 0.0,
            wind_pan: 0.0,
            howl_hz: 320.0,
            howl_q: 2.0,
            howl_gain: 0.4,
            reverb: 0.12,
            thunder_count: 0,
            thunder_dist: 1.0,
            water: 0.0,
            master: 0.8,
            fanfare_count: 0,
        }
    }
}

/// Where you're standing, in acoustic terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Acoustics {
    pub howl_hz: f32,
    pub howl_q: f32,
    pub howl_gain: f32,
    pub reverb: f32,
    pub water: f32,
    pub kind: Place,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Jungle,
    Lane,
    River,
    Base,
    CliffRim,
    Bridge,
}

/// Sample the environment. Cheap enough to run every frame.
pub fn acoustics_at(pos: Vec3) -> Acoustics {
    let r = pos.length();
    let depth = water_depth(pos.x, pos.z);
    let rd = river_dist(pos.x, pos.z);
    let lane = lane_clearance(pos.x, pos.z);
    let near_base = [Team::Blue, Team::Red].iter().map(|t| pos.distance(t.base_pos())).fold(f32::MAX, f32::min);

    // classify, then blend the edges so nothing pops
    let base_w = ((26.0 - near_base) / 12.0).clamp(0.0, 1.0);
    let cliff_w = ((r - (ARENA_RADIUS - 18.0)) / 14.0).clamp(0.0, 1.0);
    let river_w = ((9.0 - rd) / 5.0).clamp(0.0, 1.0);
    let bridge_w = if rd < 7.0 && lane < 4.0 && depth < 0.05 { 1.0 } else { 0.0 };
    let lane_w = ((6.0 - lane) / 3.0).clamp(0.0, 1.0) * (1.0 - river_w) * (1.0 - base_w);
    let jungle_w = (1.0 - base_w).min(1.0 - cliff_w).min(1.0 - river_w).min(1.0 - lane_w).max(0.0);

    // per-place voicings: (hz, q, gain, reverb)
    let jungle = (260.0, 1.4, 0.55, 0.10); // broad hiss through leaves, dead air
    let lane_v = (340.0, 2.2, 0.45, 0.16); // open corridor
    let river = (720.0, 5.0, 0.55, 0.22); // whistle over water, a little slap-back
    let base = (150.0, 7.0, 0.65, 0.55); // stone gates: a hollow moan, big echo
    let cliff = (1100.0, 9.0, 0.7, 0.42); // keening along the rim
    let bridge = (200.0, 6.0, 0.6, 0.48); // boards and a channel underneath

    let mut acc = [0.0f32; 4];
    let mut wsum = 0.0;
    for (w, v) in [(jungle_w, jungle), (lane_w, lane_v), (river_w, river), (base_w, base), (cliff_w, cliff), (bridge_w, bridge)] {
        acc[0] += v.0 * w;
        acc[1] += v.1 * w;
        acc[2] += v.2 * w;
        acc[3] += v.3 * w;
        wsum += w;
    }
    let wsum = wsum.max(1e-3);
    let kind = {
        let ws = [(jungle_w, Place::Jungle), (lane_w, Place::Lane), (river_w, Place::River), (base_w, Place::Base), (cliff_w, Place::CliffRim), (bridge_w, Place::Bridge)];
        ws.iter().fold((f32::MIN, Place::Jungle), |b, &(w, k)| if w > b.0 { (w, k) } else { b }).1
    };
    Acoustics {
        howl_hz: acc[0] / wsum,
        howl_q: acc[1] / wsum,
        howl_gain: acc[2] / wsum,
        reverb: acc[3] / wsum,
        water: (river_w * (0.4 + 0.6 * (depth / 1.0).clamp(0.0, 1.0))).clamp(0.0, 1.0),
        kind,
    }
}

// --- DSP ------------------------------------------------------------------

/// Chamberlin state-variable filter: one multiply-add per sample per output.
#[derive(Clone, Copy, Default)]
struct Svf {
    low: f32,
    band: f32,
}

impl Svf {
    #[inline]
    fn run(&mut self, x: f32, hz: f32, q: f32, sr: f32) -> (f32, f32, f32) {
        // stable for f < sr/6 with two half-steps
        let f = (2.0 * (std::f32::consts::PI * hz / sr).min(0.45).sin()).clamp(0.001, 1.2);
        let damp = (1.0 / q.max(0.5)).min(2.0);
        let mut high = 0.0;
        for _ in 0..2 {
            self.low += f * self.band;
            high = x - self.low - damp * self.band;
            self.band += f * high;
        }
        (self.low, self.band, high)
    }
}

struct Comb {
    buf: Vec<f32>,
    i: usize,
    fb: f32,
    damp_z: f32,
}

impl Comb {
    fn new(len: usize, fb: f32) -> Comb {
        Comb { buf: vec![0.0; len.max(1)], i: 0, fb, damp_z: 0.0 }
    }
    #[inline]
    fn run(&mut self, x: f32, damp: f32) -> f32 {
        let y = self.buf[self.i];
        self.damp_z = y * (1.0 - damp) + self.damp_z * damp;
        self.buf[self.i] = x + self.damp_z * self.fb;
        self.i = (self.i + 1) % self.buf.len();
        y
    }
}

struct Allpass {
    buf: Vec<f32>,
    i: usize,
}

impl Allpass {
    fn new(len: usize) -> Allpass {
        Allpass { buf: vec![0.0; len.max(1)], i: 0 }
    }
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let b = self.buf[self.i];
        let y = -0.5 * x + b;
        self.buf[self.i] = x + b * 0.5;
        self.i = (self.i + 1) % self.buf.len();
        y
    }
}

/// Schroeder reverb: 4 parallel combs → 2 series allpasses, per channel.
struct Reverb {
    combs: [Comb; 4],
    aps: [Allpass; 2],
}

impl Reverb {
    fn new(sr: f32, spread: usize) -> Reverb {
        let s = sr / 44100.0;
        let l = |n: f32| (n * s) as usize + spread;
        Reverb {
            combs: [Comb::new(l(1116.0), 0.82), Comb::new(l(1188.0), 0.80), Comb::new(l(1277.0), 0.79), Comb::new(l(1356.0), 0.78)],
            aps: [Allpass::new(l(556.0)), Allpass::new(l(441.0))],
        }
    }
    #[inline]
    fn run(&mut self, x: f32, size: f32) -> f32 {
        // size 0..1 scales feedback (short slap → long hall) and damping
        let damp = 0.35 - size * 0.15;
        let mut acc = 0.0;
        for c in self.combs.iter_mut() {
            let fb_scale = 0.82 + size * 0.34; // ~0.7 s tail (open) → ~2.5 s (stone hall)
            let saved = c.fb;
            c.fb = saved * fb_scale;
            acc += c.run(x, damp);
            c.fb = saved;
        }
        let mut y = acc * 0.25;
        for a in self.aps.iter_mut() {
            y = a.run(y);
        }
        y
    }
}

pub struct WindSynth {
    sr: f32,
    rng: u32,
    // smoothed control values (so parameter jumps never click)
    wind: f32,
    gust: f32,
    rain: f32,
    howl_hz: f32,
    howl_q: f32,
    howl_gain: f32,
    reverb_amt: f32,
    water: f32,
    pan: f32,
    // voices
    roar: Svf,
    howl_l: Svf,
    howl_r: Svf,
    keen: Svf,
    rain_hp: Svf,
    lap: Svf,
    thunder_lp: Svf,
    thunder_env: f32,
    thunder_dist: f32,
    last_thunder: u32,
    last_fanfare: u32,
    /// The intro fanfare while it plays (`brass`), mixed over the ambience.
    fanfare: Option<crate::brass::Fanfare>,
    fanfare_buf: Vec<f32>,
    lap_phase: f32,
    lfo: f32,
    rev_l: Reverb,
    rev_r: Reverb,
    dc_l: f32,
    dc_r: f32,
}

impl WindSynth {
    pub fn new(sample_rate: f32) -> WindSynth {
        WindSynth {
            sr: sample_rate,
            rng: 0x2545_F491,
            wind: 0.15,
            gust: 0.0,
            rain: 0.0,
            howl_hz: 300.0,
            howl_q: 2.0,
            howl_gain: 0.4,
            reverb_amt: 0.1,
            water: 0.0,
            pan: 0.0,
            roar: Svf::default(),
            howl_l: Svf::default(),
            howl_r: Svf::default(),
            keen: Svf::default(),
            rain_hp: Svf::default(),
            lap: Svf::default(),
            thunder_lp: Svf::default(),
            thunder_env: 0.0,
            thunder_dist: 1.0,
            last_thunder: 0,
            last_fanfare: 0,
            fanfare: None,
            fanfare_buf: Vec::new(),
            lap_phase: 0.0,
            lfo: 0.0,
            rev_l: Reverb::new(sample_rate, 0),
            rev_r: Reverb::new(sample_rate, 23),
            dc_l: 0.0,
            dc_r: 0.0,
        }
    }

    #[inline]
    fn noise(&mut self) -> f32 {
        // xorshift32 → -1..1
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Fill `out` (interleaved, `channels` wide) with ambience for `p`.
    pub fn render(&mut self, p: &AudioParams, out: &mut [f32], channels: usize) {
        let sr = self.sr;
        let n = out.len() / channels.max(1);
        // per-block control smoothing (≈ 60 ms)
        let k = 1.0 - (-(n as f32) / (sr * 0.06)).exp();
        self.wind += (p.wind.clamp(0.0, 1.0) - self.wind) * k;
        self.gust += (p.gust.clamp(0.0, 1.0) - self.gust) * k;
        self.rain += (p.rain.clamp(0.0, 1.0) - self.rain) * k;
        self.howl_hz += (p.howl_hz - self.howl_hz) * k;
        self.howl_q += (p.howl_q - self.howl_q) * k;
        self.howl_gain += (p.howl_gain - self.howl_gain) * k;
        self.reverb_amt += (p.reverb.clamp(0.0, 1.0) - self.reverb_amt) * k;
        self.water += (p.water.clamp(0.0, 1.0) - self.water) * k;
        self.pan += (p.wind_pan.clamp(-1.0, 1.0) - self.pan) * k;
        if p.thunder_count != self.last_thunder {
            self.last_thunder = p.thunder_count;
            self.thunder_env = 1.0;
            self.thunder_dist = p.thunder_dist.clamp(0.0, 1.0);
        }

        let master = p.master.clamp(0.0, 1.0);
        let dt = 1.0 / sr;
        let thunder_decay = (-dt / (1.6 + 2.5 * self.thunder_dist)).exp();
        let lap_hz = 0.6 + self.water * 0.5;

        if p.fanfare_count != self.last_fanfare {
            self.last_fanfare = p.fanfare_count;
            self.fanfare = Some(crate::brass::Fanfare::new(sr));
        }
        self.fanfare_buf.clear();
        self.fanfare_buf.resize(n, 0.0);
        if let Some(f) = self.fanfare.as_mut() {
            f.render(&mut self.fanfare_buf);
            if f.finished() {
                self.fanfare = None;
            }
        }
        for i in 0..n {
            let w = self.noise();
            self.lfo += dt;
            let lfo = (self.lfo * 0.37).sin() * 0.5 + 0.5;

            // --- wind body: a low roar whose centre climbs with the gust
            let roar_hz = 90.0 + self.wind * 120.0 + self.gust * 160.0;
            let (_, roar_b, _) = self.roar.run(w, roar_hz, 0.9, sr);
            let roar = roar_b * (0.25 + self.wind * 0.6 + self.gust * 0.9);

            // --- the howl: resonant band set by the place, pitch bending up
            // with the gust front (the classic rising moan), Q sharper in
            // enclosed places, slightly detuned per ear for width
            let bend = 1.0 + self.gust * 0.65 + lfo * 0.08;
            let q = self.howl_q * (1.0 + self.gust * 0.6);
            let (_, hl, _) = self.howl_l.run(w, self.howl_hz * bend * 0.985, q, sr);
            let (_, hr, _) = self.howl_r.run(w, self.howl_hz * bend * 1.015, q, sr);
            let howl_amp = self.howl_gain * (0.15 + self.wind * 0.35 + self.gust * 1.3) * (0.7 + 0.3 * lfo);
            // --- a faint high keen that only the strongest gusts bring out
            let (_, keen_b, _) = self.keen.run(w, 2400.0 + self.gust * 1200.0, 6.0, sr);
            let keen = keen_b * self.gust * self.gust * 0.25;

            // --- rain: bright noise, denser and darker as it gets heavy
            let (_, _, rain_h) = self.rain_hp.run(w, 1800.0 - self.rain * 600.0, 0.7, sr);
            let rain = rain_h * self.rain * 0.55 * (0.85 + 0.15 * self.noise().abs());

            // --- water lapping: slow amplitude-modulated low band near the river
            self.lap_phase += dt * lap_hz;
            let lap_env = ((self.lap_phase * std::f32::consts::TAU).sin() * 0.5 + 0.5).powf(2.5);
            let (lap_l, _, _) = self.lap.run(w, 420.0, 1.2, sr);
            let lap = lap_l * lap_env * self.water * 0.5;

            // --- thunder: a lowpassed noise roll with a slow decay; distance
            // rolls off the top and delays nothing (we're not that precise)
            let mut thunder = 0.0;
            if self.thunder_env > 0.0005 {
                let (t_l, _, _) = self.thunder_lp.run(w, 60.0 + (1.0 - self.thunder_dist) * 260.0, 0.8, sr);
                let rumble = 0.6 + 0.4 * ((self.lfo * 11.0).sin() * (self.lfo * 7.3).cos()).abs();
                thunder = t_l * self.thunder_env * rumble * (1.4 - self.thunder_dist * 0.9) * 2.0;
                self.thunder_env *= thunder_decay;
            }

            // --- mix, pan the howl by wind heading, then reverb per ear
            let pl = (0.5 - self.pan * 0.35).clamp(0.15, 0.85);
            let pr = 1.0 - pl;
            let dry_l = roar * 0.7 + (hl * howl_amp + keen) * pl * 2.0 + rain + lap + thunder;
            let dry_r = roar * 0.7 + (hr * howl_amp + keen) * pr * 2.0 + rain + lap + thunder;
            let size = self.reverb_amt;
            let wet_l = self.rev_l.run(dry_l * 0.8, size);
            let wet_r = self.rev_r.run(dry_r * 0.8, size);
            let mut l = dry_l * (1.0 - size * 0.4) + wet_l * size * 1.4;
            let mut r = dry_r * (1.0 - size * 0.4) + wet_r * size * 1.4;
            // dc block + soft clip
            self.dc_l += (l - self.dc_l) * 0.002;
            self.dc_r += (r - self.dc_r) * 0.002;
            l = soft_clip((l - self.dc_l) * master * 0.5);
            r = soft_clip((r - self.dc_r) * master * 0.5);
            // the fanfare over the top, centred, after the ambience's own clip
            let fan = self.fanfare_buf.get(i).copied().unwrap_or(0.0) * master;
            l = soft_clip(l + fan);
            r = soft_clip(r + fan);

            let base = i * channels;
            match channels {
                1 => out[base] = (l + r) * 0.5,
                _ => {
                    out[base] = l;
                    out[base + 1] = r;
                    for c in 2..channels {
                        out[base + c] = (l + r) * 0.5;
                    }
                }
            }
        }
    }
}

#[inline]
fn soft_clip(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

// --- output shell -----------------------------------------------------------

#[cfg(feature = "audio")]
pub mod output {
    use super::{AudioParams, WindSynth};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::{Arc, Mutex};

    /// A running output stream plus the handle the game thread writes to.
    pub struct Ambience {
        _stream: cpal::Stream,
        pub params: Arc<Mutex<AudioParams>>,
    }

    impl Ambience {
        /// Open the default output device. Returns `None` (and logs) when there
        /// is no device — the game runs silent rather than failing.
        pub fn start() -> Option<Ambience> {
            let host = cpal::default_host();
            let device = match host.default_output_device() {
                Some(d) => d,
                None => {
                    eprintln!("[clawbound] audio: no output device — running silent");
                    return None;
                }
            };
            let cfg = match device.default_output_config() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[clawbound] audio: no default config ({e}) — running silent");
                    return None;
                }
            };
            let sample_format = cfg.sample_format();
            let config: cpal::StreamConfig = cfg.into();
            let channels = config.channels as usize;
            let sr = config.sample_rate.0 as f32;
            let params = Arc::new(Mutex::new(AudioParams::default()));
            let shared = params.clone();
            let mut synth = WindSynth::new(sr);
            let mut snapshot = AudioParams::default();
            let err_fn = |e| eprintln!("[clawbound] audio stream error: {e}");

            let stream = match sample_format {
                cpal::SampleFormat::F32 => device.build_output_stream(
                    &config,
                    move |data: &mut [f32], _| {
                        if let Ok(p) = shared.try_lock() {
                            snapshot = *p;
                        }
                        synth.render(&snapshot, data, channels);
                    },
                    err_fn,
                    None,
                ),
                cpal::SampleFormat::I16 => {
                    let mut scratch: Vec<f32> = Vec::new();
                    device.build_output_stream(
                        &config,
                        move |data: &mut [i16], _| {
                            if let Ok(p) = shared.try_lock() {
                                snapshot = *p;
                            }
                            scratch.resize(data.len(), 0.0);
                            synth.render(&snapshot, &mut scratch, channels);
                            for (o, s) in data.iter_mut().zip(scratch.iter()) {
                                *o = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                            }
                        },
                        err_fn,
                        None,
                    )
                }
                other => {
                    eprintln!("[clawbound] audio: unsupported sample format {other:?} — running silent");
                    return None;
                }
            };
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[clawbound] audio: could not open stream ({e}) — running silent");
                    return None;
                }
            };
            if let Err(e) = stream.play() {
                eprintln!("[clawbound] audio: could not start stream ({e}) — running silent");
                return None;
            }
            eprintln!("[clawbound] audio: {sr} Hz, {channels} ch, {sample_format:?}");
            Some(Ambience { _stream: stream, params })
        }

        pub fn set(&self, p: AudioParams) {
            if let Ok(mut g) = self.params.lock() {
                *g = p;
            }
        }
    }
}

/// Seconds the ambience takes to fade to silence when a match ends.
pub const FADE_OUT_SECS: f32 = 0.6;

/// The ambience with no match to listen to (the menu, the select screen):
/// last frame's params, master falling to silence over FADE_OUT_SECS. The
/// app only ever SET params from a live game, so when a match ended and the
/// game was dropped, the synth kept playing that match's wind on the menu,
/// forever (reported).
pub fn fade_out(prev: AudioParams, dt: f32) -> AudioParams {
    let step = 0.8 / FADE_OUT_SECS * dt.max(0.0);
    AudioParams { master: (prev.master - step).max(0.0), ..prev }
}

/// Build this frame's `AudioParams` from the game + camera. Headless.
pub fn params_for(game: &crate::sim::Game, listener: Vec3, cam_yaw: f32, thunder_count: u32, thunder_dist: f32) -> AudioParams {
    let atm = game.weather.atmosphere();
    let ac = acoustics_at(listener);
    let ws = atm.wind.length();
    // wind heading relative to the camera: +1 = from the right
    let cam_right = Vec3::new(cam_yaw.sin(), 0.0, -cam_yaw.cos());
    let pan = if ws > 0.01 { atm.wind.normalize().dot(cam_right) } else { 0.0 };
    AudioParams {
        wind: (ws / 10.0).clamp(0.0, 1.0),
        gust: atm.gust,
        rain: atm.rain,
        snow: atm.snow,
        night: atm.night,
        wind_pan: pan,
        howl_hz: ac.howl_hz,
        howl_q: ac.howl_q,
        howl_gain: ac.howl_gain * (1.0 - atm.snow * 0.5), // snow muffles
        reverb: ac.reverb * (1.0 - atm.snow * 0.4),
        thunder_count,
        thunder_dist,
        water: ac.water,
        master: 0.8,
        fanfare_count: 0, // the app sets this; the sim knows nothing of the intro
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::river_x;

    #[test]
    fn places_have_distinct_voices() {
        let jungle = acoustics_at(Vec3::new(-60.0, 0.0, -30.0));
        let river = acoustics_at(Vec3::new(river_x(1.0, 5.0), 0.0, 5.0));
        let base = acoustics_at(Team::Blue.base_pos() + Vec3::new(3.0, 0.0, 0.0));
        // somewhere on the rim away from lanes and bases
        let rim = (0..36)
            .map(|k| {
                let a = k as f32 * 0.1745;
                acoustics_at(Vec3::new(a.cos() * (ARENA_RADIUS - 5.0), 0.0, a.sin() * (ARENA_RADIUS - 5.0)))
            })
            .find(|a| a.kind == Place::CliffRim)
            .expect("some stretch of the rim is pure cliff");
        assert_eq!(river.kind, Place::River);
        assert_eq!(base.kind, Place::Base);
        assert!(river.howl_hz > jungle.howl_hz, "water whistles higher than leaves");
        assert!(base.reverb > jungle.reverb * 3.0, "stone echoes: {} vs {}", base.reverb, jungle.reverb);
        assert!(rim.howl_hz > river.howl_hz, "the cliff rim keens highest: {} vs {}", rim.howl_hz, river.howl_hz);
        assert!(river.water > 0.3 && jungle.water < 0.01);
    }

    #[test]
    fn synth_is_bounded_and_responds_to_gusts() {
        let mut s = WindSynth::new(48000.0);
        let calm = AudioParams { wind: 0.1, gust: 0.0, ..Default::default() };
        let gusty = AudioParams { wind: 0.6, gust: 1.0, ..Default::default() };
        let mut buf = vec![0.0f32; 48000 * 2];
        // warm up, then measure RMS calm vs gusty
        for _ in 0..4 {
            s.render(&calm, &mut buf, 2);
        }
        let rms = |b: &[f32]| (b.iter().map(|x| x * x).sum::<f32>() / b.len() as f32).sqrt();
        let calm_rms = rms(&buf);
        for _ in 0..4 {
            s.render(&gusty, &mut buf, 2);
        }
        let gust_rms = rms(&buf);
        assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0), "output must stay in range");
        assert!(gust_rms > calm_rms * 1.8, "a gust must be audibly louder: {calm_rms} -> {gust_rms}");
    }

    #[test]
    fn thunder_fires_once_per_count_and_decays() {
        let mut s = WindSynth::new(44100.0);
        let mut p = AudioParams { wind: 0.0, howl_gain: 0.0, ..Default::default() };
        let mut buf = vec![0.0f32; 4410];
        let rms = |b: &[f32]| (b.iter().map(|x| x * x).sum::<f32>() / b.len() as f32).sqrt();
        s.render(&p, &mut buf, 1);
        let quiet = rms(&buf);
        p.thunder_count = 1;
        p.thunder_dist = 0.2;
        s.render(&p, &mut buf, 1);
        let loud = rms(&buf);
        assert!(loud > quiet * 3.0, "thunder should roll: {quiet} -> {loud}");
        for _ in 0..80 {
            s.render(&p, &mut buf, 1);
        }
        let later = rms(&buf);
        assert!(later < loud * 0.3, "and fade: {loud} -> {later}");
    }

    #[test]
    fn reverb_lengthens_the_tail() {
        // drive the room with rain, cut the rain, and listen to what's left
        let rms = |b: &[f32]| (b.iter().map(|x| x * x).sum::<f32>() / b.len() as f32).sqrt();
        let tail = |reverb: f32, rain: f32| {
            let mut s = WindSynth::new(44100.0);
            let mut buf = vec![0.0f32; 2205];
            let on = AudioParams { reverb, rain, wind: 0.0, howl_gain: 0.0, ..Default::default() };
            let off = AudioParams { rain: 0.0, ..on };
            for _ in 0..20 {
                s.render(&on, &mut buf, 1);
            }
            let mut sum = 0.0;
            for k in 0..30 {
                s.render(&off, &mut buf, 1);
                if k >= 8 {
                    sum += rms(&buf);
                }
            }
            sum
        };
        // subtract each room's steady floor (the wind body never fully stops)
        let w = tail(0.9, 1.0) - tail(0.9, 0.0);
        let d = tail(0.0, 1.0) - tail(0.0, 0.0);
        assert!(w > d * 3.0 && w > 0.003, "a bigger room rings longer: wet {w} dry {d}");
    }
}

#[cfg(test)]
mod fanfare_tests {
    use super::*;

    /// The intro's fanfare plays through the same synth the game hears:
    /// bumping the counter adds the brass over the ambience, once; leaving
    /// it alone adds nothing.
    #[test]
    fn the_fanfare_plays_through_the_live_synth_once_per_bump() {
        let sr = 22050.0;
        let run = |bump: bool| {
            let mut w = WindSynth::new(sr);
            let mut p = AudioParams::default();
            let mut out = vec![0.0f32; 2 * 1024];
            let mut energy = 0.0f32;
            for block in 0..((3.0 * sr / 1024.0) as usize) {
                if bump && block == 2 {
                    p.fanfare_count = 1;
                }
                w.render(&p, &mut out, 2);
                energy += out.iter().map(|x| x * x).sum::<f32>();
            }
            energy
        };
        let (quiet, loud) = (run(false), run(true));
        assert!(loud > quiet * 3.0, "the fanfare is heard: {loud} vs {quiet}");
        assert!(loud.is_finite());
    }
}

#[cfg(test)]
mod fade_tests {
    use super::*;

    /// Out of a match the ambience fades -- monotonically, not a cut -- to
    /// exactly silence within FADE_OUT_SECS, and stays there; and the synth
    /// at master 0 outputs nothing.
    #[test]
    fn out_of_a_match_the_wind_fades_to_silence() {
        let mut p = AudioParams::default();
        p.wind = 0.9;
        let start = p.master;
        let mut last = start;
        let mut t = 0.0;
        while t < FADE_OUT_SECS + 0.05 {
            p = fade_out(p, 1.0 / 60.0);
            assert!(p.master <= last + 1e-6, "not monotone");
            assert!(p.master > 0.0 || t > FADE_OUT_SECS * 0.5, "cut, not faded");
            last = p.master;
            t += 1.0 / 60.0;
        }
        assert_eq!(p.master, 0.0);
        assert_eq!(fade_out(p, 1.0).master, 0.0);
        assert_eq!(p.wind, 0.9, "only the level fades");
        let mut w = WindSynth::new(22050.0);
        let mut out = vec![0.0f32; 2 * 2048];
        for _ in 0..8 {
            w.render(&p, &mut out, 2);
        }
        assert!(out.iter().all(|x| x.abs() < 1e-6), "wind audible at master 0");
    }
}
