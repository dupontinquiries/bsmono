//! audio.rs — a small, low-resolution *procedural* sound engine. Everything here is pure Rust DSP that
//! synthesises sample buffers (`Vec<f32>`, mono, [-1,1]) from scratch — no samples on disk, no external
//! crates. That makes the whole core unit-testable without an audio device.
//!
//! What it models:
//!   * **wind whistle** — band-passed noise with slow gusting, the band tightening with strength;
//!   * **water drips** — a pitched "plink" with a fast pluck envelope and a little downward glide;
//!   * **moisture system** — a 0..1 wetness that sets how often surfaces drip and how damp (dark) they sound;
//!   * **wood creak** — resonant filtered-noise groan with stick-slip amplitude;
//!   * **metal ring** — a sum of inharmonic decaying partials; a `coupling` term bleeds energy between
//!     partials to model several joined metal meshes *ringing into each other* (transmission);
//!   * **echo / reverb** — a feedback delay line sized by the space, for cave/hall tails.
//!
//! Actual playback lives in the feature-gated [`play`] submodule (off by default) so enabling sound never
//! threatens the normal build.

/// Low sample rate — deliberately lo-fi, and cheap to synthesise.
pub const SR: u32 = 22_050;
const SRF: f32 = SR as f32;

/// Deterministic xorshift noise source in [-1, 1]. Used for every stochastic generator so output is
/// reproducible from a seed (and tests are stable).
#[derive(Clone)]
pub struct Noise {
    s: u32,
}
impl Noise {
    pub fn new(seed: u32) -> Self {
        Noise { s: seed | 1 }
    }
    #[inline]
    pub fn next(&mut self) -> f32 {
        // xorshift32
        let mut x = self.s;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.s = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// A 2-pole state-variable filter — gives cheap low-pass / band-pass / high-pass outputs at once. We use the
/// band-pass tap for whistle and creak resonances.
#[derive(Clone)]
struct Svf {
    f: f32,
    q: f32,
    lp: f32,
    bp: f32,
}
impl Svf {
    fn new(cutoff_hz: f32, q: f32) -> Self {
        let f = 2.0 * (std::f32::consts::PI * (cutoff_hz / SRF).min(0.45)).sin();
        Svf { f, q: 1.0 / q.max(0.5), lp: 0.0, bp: 0.0 }
    }
    /// Returns (low, band, high).
    #[inline]
    fn process(&mut self, x: f32) -> (f32, f32, f32) {
        self.lp += self.f * self.bp;
        let hp = x - self.lp - self.q * self.bp;
        self.bp += self.f * hp;
        (self.lp, self.bp, hp)
    }
}

fn buf(seconds: f32) -> Vec<f32> {
    vec![0.0; ((seconds.max(0.0)) * SRF) as usize + 1]
}

/// Wind whistle: noise pushed through a resonant band-pass whose centre rises and whose amplitude gusts on
/// slow LFOs. `strength` 0..~1.5 raises the pitch/level and tightens the resonance into more of a whistle.
pub fn wind(seconds: f32, strength: f32, seed: u32) -> Vec<f32> {
    let mut out = buf(seconds);
    let mut n = Noise::new(seed ^ 0xA1B2);
    let s = strength.clamp(0.0, 2.0);
    let centre = 480.0 + s * 540.0;
    let mut filt = Svf::new(centre, 2.5 + s * 3.5);
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SRF;
        // two slow gusts multiply into an irregular swell
        let gust = (0.55 + 0.45 * (t * 0.7).sin()) * (0.6 + 0.4 * (t * 0.23 + 1.3).sin());
        let (_lp, bp, _hp) = filt.process(n.next());
        *o = (bp * (0.5 + 0.5 * s) * gust).clamp(-1.0, 1.0);
    }
    out
}

/// A single water drip: a short pitched pluck (fast attack, exponential decay) with a small downward glide
/// and a bright click at the onset. `wetness` 0..1 darkens and lengthens the tail (a damp cave plink).
pub fn drip(pitch_hz: f32, wetness: f32, seed: u32) -> Vec<f32> {
    let w = wetness.clamp(0.0, 1.0);
    let dur = 0.10 + w * 0.18;
    let mut out = buf(dur);
    let mut n = Noise::new(seed ^ 0x51FF);
    let decay = 26.0 - w * 14.0; // wetter → slower decay
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SRF;
        let env = (-t * decay).exp();
        let glide = pitch_hz * (1.0 - 0.12 * t / dur); // slight pitch drop
        let body = (2.0 * std::f32::consts::PI * glide * t).sin();
        let click = if t < 0.004 { n.next() * (1.0 - t / 0.004) } else { 0.0 };
        *o = ((body * 0.8 + click * 0.5) * env).clamp(-1.0, 1.0);
    }
    out
}

/// Wood creak: band-passed noise with a slow groan envelope and stick-slip amplitude jumps.
pub fn creak(seconds: f32, seed: u32) -> Vec<f32> {
    let mut out = buf(seconds);
    let mut n = Noise::new(seed ^ 0x3C2A);
    let mut filt = Svf::new(220.0, 6.0);
    let mut slip = 0.0f32;
    let mut hold = 0i32;
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SRF;
        // groan envelope: slow rise then decay across the whole event
        let env = (t / (seconds * 0.3)).min(1.0) * (-(t / seconds) * 2.2).exp();
        // stick-slip: hold a level, then jump — gives the irregular "rrrk"
        if hold <= 0 {
            slip = 0.2 + 0.8 * (n.next() * 0.5 + 0.5);
            hold = (SRF * (0.01 + 0.05 * (n.next() * 0.5 + 0.5))) as i32;
        }
        hold -= 1;
        let (_l, bp, _h) = filt.process(n.next());
        *o = (bp * env * slip).clamp(-1.0, 1.0);
    }
    out
}

/// Metal ring: a struck-metal tone built from inharmonic partials, each decaying faster the higher it is.
/// `coupling` 0..1 bleeds a little of each partial's energy into its neighbours — modelling several joined
/// metal meshes transmitting the ring between them (a richer, longer, more beating shimmer).
pub fn metal_ring(fundamental: f32, seconds: f32, coupling: f32, seed: u32) -> Vec<f32> {
    let mut out = buf(seconds);
    // bell/bar-like inharmonic ratios
    let ratios = [1.0f32, 2.76, 5.40, 8.93, 13.34];
    let mut amps = [1.0f32, 0.6, 0.42, 0.3, 0.18];
    // coupling spreads energy to neighbours (a crude transmission/normalisation)
    let c = coupling.clamp(0.0, 1.0);
    let base = amps;
    for k in 0..amps.len() {
        let lo = if k > 0 { base[k - 1] } else { 0.0 };
        let hi = if k + 1 < base.len() { base[k + 1] } else { 0.0 };
        amps[k] = base[k] * (1.0 - 0.4 * c) + (lo + hi) * 0.5 * c;
    }
    let mut n = Noise::new(seed ^ 0x7E11);
    let detune = 1.0 + (n.next() * 0.002); // tiny detune for beating
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SRF;
        let mut s = 0.0;
        for (k, &r) in ratios.iter().enumerate() {
            let f = fundamental * r * detune;
            let decay = (1.5 + r * 0.9) * (1.0 - 0.4 * c); // coupling lengthens the tail
            let env = (-t * decay).exp();
            s += (2.0 * std::f32::consts::PI * f * t).sin() * amps[k] * env;
        }
        *o = (s * 0.5).clamp(-1.0, 1.0);
    }
    out
}

/// Feedback-delay echo / reverb. `room` 0..1 sizes the delay (≈ up to 0.32 s) and feedback; `mix` 0..1 sets
/// wet level. The output carries a decaying tail past the input, so it returns a longer buffer.
pub fn echo(input: &[f32], room: f32, mix: f32) -> Vec<f32> {
    let room = room.clamp(0.0, 1.0);
    let mix = mix.clamp(0.0, 1.0);
    let delay = ((0.04 + room * 0.28) * SRF) as usize + 1;
    let fb = 0.25 + room * 0.55;
    let tail = (delay as f32 * (1.0 + 4.0 * room)) as usize;
    let n = input.len() + tail;
    let mut wet = vec![0.0f32; n]; // recirculating feedback line
    let mut out = vec![0.0f32; n];
    for i in 0..n {
        let dry = if i < input.len() { input[i] } else { 0.0 };
        let echoed = if i >= delay { wet[i - delay] * fb } else { 0.0 };
        wet[i] = (dry + echoed).clamp(-1.5, 1.5);
        out[i] = (dry * (1.0 - mix) + wet[i] * mix).clamp(-1.5, 1.5);
    }
    out
}

/// Acoustic properties of a surface material — drives the ambient sound design of a space.
#[derive(Clone, Copy, Debug)]
pub struct Material {
    /// 0..1 reverberance (stone/ice ring; foliage is dead).
    pub reverb: f32,
    /// 0..1 how readily this surface drips (porous wet rock high; metal low).
    pub drip: f32,
    /// 0..1 creak tendency (timber high).
    pub creak: f32,
    /// 0..1 metallic ring tendency.
    pub ring: f32,
}
impl Material {
    pub fn stone() -> Self {
        Material { reverb: 0.8, drip: 0.5, creak: 0.05, ring: 0.1 }
    }
    pub fn wood() -> Self {
        Material { reverb: 0.25, drip: 0.2, creak: 0.85, ring: 0.1 }
    }
    pub fn metal() -> Self {
        Material { reverb: 0.55, drip: 0.1, creak: 0.15, ring: 0.9 }
    }
    pub fn foliage() -> Self {
        Material { reverb: 0.12, drip: 0.35, creak: 0.1, ring: 0.0 }
    }
    pub fn ice() -> Self {
        Material { reverb: 0.7, drip: 0.15, creak: 0.2, ring: 0.5 }
    }
}

/// The ambient sound bed for a space: how much wind, how wet (drip density + tail), and how reverberant.
#[derive(Clone, Copy, Debug)]
pub struct AmbientBed {
    pub wind: f32,
    pub wetness: f32,
    pub reverb: f32,
}
impl AmbientBed {
    /// Build a bed from a dominant material + an exposure (0 enclosed → 1 open to the wind).
    pub fn from(material: Material, exposure: f32) -> Self {
        AmbientBed {
            wind: (0.2 + exposure * 1.1).min(1.5),
            wetness: material.drip,
            reverb: material.reverb * (1.0 - 0.5 * exposure), // open spaces ring less
        }
    }
}

/// Render a loopable ambient bed: gusting wind, sparse drips scattered by `wetness`, run through the room
/// echo. Returns `seconds` (plus the echo tail) of mono audio. Pure + deterministic for a given seed.
pub fn render_bed(bed: AmbientBed, seconds: f32, seed: u32) -> Vec<f32> {
    let mut mix = wind(seconds, bed.wind, seed);
    // sprinkle drips: more frequent when wetter
    let mut n = Noise::new(seed ^ 0xD12B);
    let count = (bed.wetness * seconds * 3.0) as i32;
    for k in 0..count {
        let at = ((n.next() * 0.5 + 0.5) * seconds * SRF) as usize;
        let pitch = 700.0 + (n.next() * 0.5 + 0.5) * 1600.0;
        let d = drip(pitch, bed.wetness, seed.wrapping_add(k as u32 * 2654435761));
        for (j, s) in d.iter().enumerate() {
            if at + j < mix.len() {
                mix[at + j] = (mix[at + j] + s * 0.5).clamp(-1.0, 1.0);
            }
        }
    }
    echo(&mix, bed.reverb, 0.3 * bed.reverb)
}

/// Peak absolute sample — handy for tests and for normalising before playback.
pub fn peak(b: &[f32]) -> f32 {
    b.iter().fold(0.0f32, |m, &x| m.max(x.abs()))
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────────
// Gameplay one-shots — short transient sounds for shooting, impacts, footsteps, pickups. All pure DSP.
// ─────────────────────────────────────────────────────────────────────────────────────────────────────────

/// A weapon report: a sharp broadband transient (the muzzle blast), a short resonant body that sets the
/// "caliber" (lower body = beefier gun), and a fast exponential decay. `bore` 0..1 deepens and lengthens it.
pub fn gunshot(bore: f32, seed: u32) -> Vec<f32> {
    let bore = bore.clamp(0.0, 1.0);
    let len = 0.10 + bore * 0.16;
    let mut out = buf(len);
    let mut n = Noise::new(seed ^ 0x6017);
    // body resonance: low for big bores, brighter for small
    let body_hz = 320.0 - bore * 210.0;
    let mut body = Svf::new(body_hz.max(70.0), 1.6);
    let mut crack = Svf::new(2600.0 - bore * 900.0, 0.9);
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SRF;
        let blast = (-t * (90.0 - bore * 45.0)).exp(); // very fast transient
        let tail = (-t * (26.0 - bore * 14.0)).exp(); // body decay
        let x = n.next();
        let (lo, _b, _h) = body.process(x);
        let (_l2, bp, _h2) = crack.process(x);
        let s = bp * blast * 0.9 + lo * tail * (0.6 + bore * 0.5);
        *o = (s * 1.1).clamp(-1.0, 1.0);
    }
    out
}

/// A bullet/melee impact on a surface. `hardness` 0 (soft thud — flesh/dirt) → 1 (sharp tick — metal/stone).
pub fn impact(hardness: f32, seed: u32) -> Vec<f32> {
    let h = hardness.clamp(0.0, 1.0);
    let len = 0.06 + (1.0 - h) * 0.10;
    let mut out = buf(len);
    let mut n = Noise::new(seed ^ 0x14AC);
    let mut f = Svf::new(400.0 + h * 2800.0, 1.0 + h * 2.0);
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SRF;
        let env = (-t * (60.0 + h * 90.0)).exp();
        let (lo, bp, _hp) = f.process(n.next());
        let s = (lo * (1.0 - h) + bp * h) * env;
        *o = (s * 1.2).clamp(-1.0, 1.0);
    }
    out
}

/// A footstep on a surface: a soft filtered thump whose brightness/decay vary with the material's `hardness`
/// (mud/foliage soft and dull, stone/metal sharp). A faint grit transient sits on top.
pub fn footstep(hardness: f32, seed: u32) -> Vec<f32> {
    let h = hardness.clamp(0.0, 1.0);
    let mut out = buf(0.12);
    let mut n = Noise::new(seed ^ 0xF007);
    let mut thump = Svf::new(120.0 + h * 80.0, 1.2);
    let mut grit = Svf::new(2200.0 + h * 1000.0, 0.8);
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SRF;
        let env = (-t * 34.0).exp();
        let x = n.next();
        let (lo, _b, _hp) = thump.process(x);
        let (_l, _bp, hp) = grit.process(x);
        let s = lo * env + hp * env * 0.18 * h;
        *o = (s * 1.3).clamp(-1.0, 1.0);
    }
    out
}

/// An explosion: a deep low-frequency rumble (filtered noise) under a sharp crack, with a long decay. `size`
/// 0..1 lengthens the tail and lowers the rumble.
pub fn explosion(size: f32, seed: u32) -> Vec<f32> {
    let size = size.clamp(0.0, 1.0);
    let len = 0.5 + size * 1.2;
    let mut out = buf(len);
    let mut n = Noise::new(seed ^ 0xE7B0);
    let mut rumble = Svf::new(70.0 - size * 30.0, 1.3);
    let mut crack = Svf::new(1800.0, 0.8);
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SRF;
        let crack_env = (-t * 40.0).exp();
        let rumble_env = (-t * (3.0 - size * 1.8)).exp();
        let x = n.next();
        let (lo, _b, _h) = rumble.process(x);
        let (_l, bp, _hp) = crack.process(x);
        let s = lo * rumble_env * 1.1 + bp * crack_env * 0.5;
        *o = (s * 1.0).clamp(-1.0, 1.0);
    }
    out
}

/// A UI/pickup blip: a short tonal chirp that glides up (reward) or down. `up` true rises in pitch.
pub fn blip(base_hz: f32, up: bool, seed: u32) -> Vec<f32> {
    let mut out = buf(0.16);
    let _ = seed;
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SRF;
        let glide = if up { 1.0 + t * 4.0 } else { 1.0 - t * 1.2 };
        let f = (base_hz * glide).max(40.0);
        let env = (-t * 16.0).exp() * (1.0 - (-t * 200.0).exp()); // pluck attack + decay
        let s = (2.0 * std::f32::consts::PI * f * t).sin();
        *o = (s * env * 0.7).clamp(-1.0, 1.0);
    }
    out
}

/// A mechanical reload: two short clicks (mag out, mag in) separated by a small gap.
pub fn reload_click(seed: u32) -> Vec<f32> {
    let mut out = buf(0.45);
    let mut n = Noise::new(seed ^ 0xC11C);
    let mut f = Svf::new(2200.0, 1.4);
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SRF;
        // two clicks at t≈0.02 and t≈0.30
        let c1 = (-(t - 0.02).abs() * 220.0).exp();
        let c2 = (-(t - 0.30).abs() * 220.0).exp();
        let (_l, bp, _h) = f.process(n.next());
        *o = (bp * (c1 + c2) * 0.9).clamp(-1.0, 1.0);
    }
    out
}

#[cfg(feature = "sound")]
pub mod play {
    //! Feature-gated real-time playback via `rodio`. Off by default. Degrades gracefully to silence if no
    //! audio device is available, so enabling the feature can never panic the app.
    use super::SR;
    use std::sync::Mutex;

    pub struct Audio {
        _stream: rodio::OutputStream,
        handle: rodio::OutputStreamHandle,
        bed: Mutex<Option<rodio::Sink>>,
    }

    impl Audio {
        /// Try to open the default output device. Returns `None` (silent) if none is available.
        pub fn new() -> Option<Self> {
            let (stream, handle) = rodio::OutputStream::try_default().ok()?;
            Some(Audio { _stream: stream, handle, bed: Mutex::new(None) })
        }
        /// Fire a one-shot sound buffer.
        pub fn play(&self, samples: Vec<f32>) {
            if let Ok(sink) = rodio::Sink::try_new(&self.handle) {
                sink.append(rodio::buffer::SamplesBuffer::new(1, SR, samples));
                sink.detach();
            }
        }
        /// Start (or replace) a looping ambient bed.
        pub fn set_bed(&self, samples: Vec<f32>) {
            if let Ok(sink) = rodio::Sink::try_new(&self.handle) {
                use rodio::Source;
                sink.append(rodio::buffer::SamplesBuffer::new(1, SR, samples).repeat_infinite());
                if let Ok(mut g) = self.bed.lock() {
                    *g = Some(sink);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finite(b: &[f32]) -> bool {
        b.iter().all(|x| x.is_finite())
    }
    fn rms(b: &[f32]) -> f32 {
        (b.iter().map(|x| x * x).sum::<f32>() / b.len().max(1) as f32).sqrt()
    }


    #[test]
    fn gameplay_oneshots_are_bounded_and_audible() {
        let shots = [
            gunshot(0.2, 1), gunshot(0.9, 2), impact(0.1, 3), impact(0.95, 4),
            footstep(0.2, 5), footstep(0.9, 6), explosion(0.8, 7), blip(440.0, true, 8),
            blip(330.0, false, 9), reload_click(10),
        ];
        for s in &shots {
            assert!(!s.is_empty(), "one-shot should produce samples");
            assert!(finite(s), "no NaN/inf in audio");
            assert!(peak(s) <= 1.0, "stays within [-1,1]");
            assert!(rms(s) > 0.0005, "one-shot should be audible");
        }
        // a big bore gun is longer/beefier than a small one
        assert!(gunshot(0.9, 1).len() > gunshot(0.1, 1).len());
        // explosion tail is much longer than an impact tick
        assert!(explosion(1.0, 1).len() > impact(1.0, 1).len() * 3);
    }

    #[test]
    fn wind_is_bounded_and_audible() {
        let w = wind(0.5, 1.0, 7);
        assert_eq!(w.len(), (0.5 * SRF) as usize + 1);
        assert!(finite(&w));
        assert!(peak(&w) <= 1.0);
        assert!(rms(&w) > 0.001, "wind should make sound");
    }

    #[test]
    fn drip_plucks_then_decays() {
        let d = drip(1200.0, 0.5, 3);
        assert!(finite(&d));
        // energy front-loaded: first eighth louder than last eighth
        let n = d.len() / 8;
        let front = rms(&d[..n]);
        let back = rms(&d[d.len() - n..]);
        assert!(front > back * 4.0, "drip should decay (front {front}, back {back})");
    }

    #[test]
    fn metal_ring_decays_and_coupling_extends_tail() {
        let plain = metal_ring(440.0, 1.0, 0.0, 1);
        let coupled = metal_ring(440.0, 1.0, 1.0, 1);
        assert!(finite(&plain) && finite(&coupled));
        let tailwin = plain.len() / 6;
        // decays: start louder than end
        assert!(rms(&plain[..tailwin]) > rms(&plain[plain.len() - tailwin..]));
        // coupling lengthens the ring → more energy left in the tail
        let pt = rms(&plain[plain.len() - tailwin..]);
        let ct = rms(&coupled[coupled.len() - tailwin..]);
        assert!(ct >= pt, "coupling should sustain the tail (plain {pt}, coupled {ct})");
    }

    #[test]
    fn creak_is_bounded_nonzero() {
        let c = creak(0.6, 9);
        assert!(finite(&c) && peak(&c) <= 1.0 && rms(&c) > 0.0001);
    }

    #[test]
    fn echo_adds_a_tail() {
        let mut imp = vec![0.0f32; 100];
        imp[0] = 1.0;
        let e = echo(&imp, 0.8, 0.5);
        assert!(e.len() > imp.len(), "echo returns a tail");
        assert!(finite(&e));
        // there is a delayed repeat somewhere past the input
        let late = &e[imp.len()..];
        assert!(peak(late) > 0.05, "a reflection should ring on past the input");
    }

    #[test]
    fn material_and_bed_ranges_are_sane() {
        for m in [Material::stone(), Material::wood(), Material::metal(), Material::foliage(), Material::ice()] {
            for v in [m.reverb, m.drip, m.creak, m.ring] {
                assert!((0.0..=1.0).contains(&v));
            }
        }
        let bed = AmbientBed::from(Material::stone(), 0.2);
        let b = render_bed(bed, 0.4, 5);
        assert!(finite(&b) && peak(&b) <= 1.5 && rms(&b) > 0.0005);
    }
}
