//! particles — dirt, dust and smoke in a scene, moved by sound.
//!
//! * **Deterministic.** A system is its parameters, a seed and the drive
//!   signals; stepping it from frame 0 always gives the same particles, so
//!   the preview, a seek and the export agree without storing any state.
//! * **Presets for a look, parameters for the rest:** floating dirt (a slow
//!   upward drift with spin -- a hero lifting off), spaceship dust (a glowing
//!   burst that spreads and slows), creepy smoke (big soft puffs curling in
//!   a swirling flow). Each is just a [`Params`]; every number is open.
//! * **Moved by sound.** [`envelope`] turns audio -- a synth on a silent
//!   control bus, the violins, anything -- into a 0..1 drive per frame, and
//!   each preset says how much that drive raises emission, turbulence and
//!   glow. The smoke thickens and churns when the strings swell.
//! * **Rides a rig.** The emitter is given per frame, so it can be the
//!   `tracker` rig's position: dust that follows a figure through a shot.
//!
//! The swirl is curl noise: the curl of a smooth, slowly changing field. A
//! curl has no divergence, so particles circulate instead of bunching up or
//! clearing out -- the look of smoke, and pinned by a test.

use crate::framepipe::Frame;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Blend { Add, Over }

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// Particles born per second at no drive.
    pub rate: f32,
    /// Seconds a particle lives.
    pub life: f32,
    /// Radius in pixels at birth and at death.
    pub size: (f32, f32),
    pub color: [f32; 3],
    /// Peak opacity (0..1).
    pub alpha: f32,
    /// Initial speed, px/s, and how far off its main direction it may be sent (radians).
    pub speed: f32,
    pub spread: f32,
    /// Main direction of the initial velocity, radians (-pi/2 is up); `None` = outward from the emitter.
    pub direction: Option<f32>,
    /// px/s^2; negative y rises.
    pub gravity: (f32, f32),
    /// Fraction of velocity lost per second.
    pub drag: f32,
    /// Curl-noise strength (px/s) and feature size (px).
    pub swirl: f32,
    pub swirl_scale: f32,
    /// Emitter half-size in px (an ellipse).
    pub emit_radius: (f32, f32),
    pub blend: Blend,
    /// How much a full drive multiplies emission, adds to swirl, and brightens.
    pub drive_rate: f32,
    pub drive_swirl: f32,
    pub drive_glow: f32,
}

impl Params {
    pub fn floating_dirt() -> Params {
        Params { rate: 40.0, life: 3.5, size: (1.4, 0.8), color: [0.36, 0.27, 0.18], alpha: 0.9,
                 speed: 12.0, spread: 0.6, direction: Some(-std::f32::consts::FRAC_PI_2), gravity: (0.0, -9.0),
                 drag: 0.15, swirl: 6.0, swirl_scale: 60.0, emit_radius: (120.0, 10.0), blend: Blend::Over,
                 drive_rate: 2.0, drive_swirl: 6.0, drive_glow: 0.0 }
    }
    pub fn space_dust() -> Params {
        Params { rate: 120.0, life: 2.2, size: (1.2, 2.4), color: [0.75, 0.9, 1.0], alpha: 0.7,
                 speed: 90.0, spread: std::f32::consts::PI, direction: None, gravity: (0.0, 0.0),
                 drag: 1.2, swirl: 10.0, swirl_scale: 90.0, emit_radius: (8.0, 8.0), blend: Blend::Add,
                 drive_rate: 3.0, drive_swirl: 20.0, drive_glow: 1.5 }
    }
    pub fn creepy_smoke() -> Params {
        Params { rate: 18.0, life: 5.0, size: (6.0, 22.0), color: [0.62, 0.64, 0.7], alpha: 0.22,
                 speed: 14.0, spread: 0.5, direction: Some(-std::f32::consts::FRAC_PI_2), gravity: (0.0, -4.0),
                 drag: 0.4, swirl: 28.0, swirl_scale: 45.0, emit_radius: (30.0, 6.0), blend: Blend::Over,
                 drive_rate: 1.5, drive_swirl: 45.0, drive_glow: 0.4 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle { pub x: f32, pub y: f32, pub vx: f32, pub vy: f32, pub age: f32, pub life: f32, pub seed: u32 }

/// A running system. Step it with the frame's emitter position and drive.
#[derive(Debug, Clone, PartialEq)]
pub struct System { pub p: Params, pub parts: Vec<Particle>, rng: u64, carry: f32, t: f32, pub drive: f32 }

impl System {
    pub fn new(p: Params, seed: u64) -> System { System { p, parts: Vec::new(), rng: seed.max(1), carry: 0.0, t: 0.0, drive: 0.0 } }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13; self.rng ^= self.rng >> 7; self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Advance by `dt` seconds with the emitter at `at` (px) and `drive` 0..1.
    pub fn step(&mut self, dt: f32, at: (f32, f32), drive: f32) {
        let d = drive.clamp(0.0, 1.0);
        self.drive = d;
        let p = self.p;
        // birth: fractional births carry over, so a low rate still emits evenly
        self.carry += p.rate * (1.0 + p.drive_rate * d) * dt;
        while self.carry >= 1.0 {
            self.carry -= 1.0;
            let (a, r) = (self.rand() * std::f32::consts::TAU, self.rand().sqrt());
            let (ox, oy) = (a.cos() * r * p.emit_radius.0, a.sin() * r * p.emit_radius.1);
            let dir = match p.direction { Some(base) => base + (self.rand() - 0.5) * 2.0 * p.spread, None => oy.atan2(ox) + (self.rand() - 0.5) * p.spread };
            let v = p.speed * (0.6 + 0.8 * self.rand());
            let seed = (self.rand() * u32::MAX as f32) as u32;
            let life = p.life * (0.7 + 0.6 * self.rand());
            self.parts.push(Particle { x: at.0 + ox, y: at.1 + oy, vx: dir.cos() * v, vy: dir.sin() * v, age: 0.0, life, seed });
        }
        // motion: gravity, drag, and the swirl field
        let swirl = p.swirl + p.drive_swirl * d;
        let t = self.t;
        for q in self.parts.iter_mut() {
            let (cx, cy) = curl(q.x, q.y, t, p.swirl_scale);
            q.vx += p.gravity.0 * dt; q.vy += p.gravity.1 * dt;
            let k = (1.0 - p.drag * dt).max(0.0);
            q.vx *= k; q.vy *= k;
            q.x += (q.vx + cx * swirl) * dt; q.y += (q.vy + cy * swirl) * dt;
            q.age += dt;
        }
        self.parts.retain(|q| q.age < q.life);
        self.t += dt;
    }

    /// Draw onto a frame: soft round sprites, fading in and out over life.
    pub fn draw(&self, f: &mut Frame) {
        let p = &self.p;
        let glow = 1.0 + p.drive_glow * self.drive;
        for q in &self.parts {
            let u = (q.age / q.life).clamp(0.0, 1.0);
            let rad = p.size.0 + (p.size.1 - p.size.0) * u;
            let fade = (u * 6.0).min(1.0) * (1.0 - u).powf(1.5);
            let a = (p.alpha * fade).clamp(0.0, 1.0);
            if a < 0.004 { continue; }
            let r = (rad * 2.2).ceil() as i32;
            let (x0, y0) = (q.x.round() as i32, q.y.round() as i32);
            for dy in -r..=r { for dx in -r..=r {
                let (x, y) = (x0 + dx, y0 + dy);
                if x < 0 || y < 0 || x >= f.w as i32 || y >= f.h as i32 { continue; }
                let dd = ((x as f32 - q.x).powi(2) + (y as f32 - q.y).powi(2)) / (rad * rad).max(0.25);
                let w = a * (-dd).exp();
                if w < 0.003 { continue; }
                let o = f.at(x as u32, y as u32);
                let mut v = [0u8; 3];
                for c in 0..3 {
                    let s = (p.color[c] * glow).min(1.5) * 255.0;
                    let base = o[c] as f32;
                    v[c] = match p.blend { Blend::Add => base + s * w, Blend::Over => base + (s - base) * w }.clamp(0.0, 255.0) as u8;
                }
                f.set(x as u32, y as u32, v);
            }}
        }
    }
}

/// Curl of a smooth, slowly changing scalar field: a divergence-free flow.
pub fn curl(x: f32, y: f32, t: f32, scale: f32) -> (f32, f32) {
    let s = 1.0 / scale.max(1.0);
    // psi(x, y, t) = sum of drifting sines; curl = (dpsi/dy, -dpsi/dx)
    let (a, b) = (x * s, y * s);
    let dpdx = s * (a * 1.3 + t * 0.4).cos() * (b * 0.9 - t * 0.3).sin() * 1.3
             + s * 0.5 * (a * 2.1 - b * 1.7 + t * 0.7).cos() * 2.1;
    let dpdy = s * (a * 1.3 + t * 0.4).sin() * (b * 0.9 - t * 0.3).cos() * 0.9
             - s * 0.5 * (a * 2.1 - b * 1.7 + t * 0.7).cos() * 1.7;
    (dpdy * scale, -dpdx * scale)
}

/// Audio -> a 0..1 drive per video frame: loudness over each frame's span,
/// mapped from -60 dBFS (0) to 0 dBFS (1), with a quick rise and a slower fall
/// so a pluck blooms and lingers rather than flickering.
pub fn envelope(mono: &[f32], sample_rate: u32, fps: f64) -> Vec<f32> {
    let hop = (sample_rate as f64 / fps).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(mono.len() / hop + 1);
    let mut level = 0.0f32;
    for chunk in mono.chunks(hop) {
        let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32).sqrt();
        let v = ((20.0 * rms.max(1e-6).log10() + 60.0) / 60.0).clamp(0.0, 1.0);
        level = if v > level { level + (v - level) * 0.6 } else { level + (v - level) * 0.12 };
        out.push(level);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(p: Params, seed: u64, drive: impl Fn(usize) -> f32, frames: usize) -> System {
        let mut s = System::new(p, seed);
        for i in 0..frames { s.step(1.0 / 24.0, (160.0, 120.0), drive(i)); }
        s
    }

    #[test]
    fn the_same_seed_is_the_same_smoke() {
        let a = run(Params::creepy_smoke(), 7, |i| (i as f32 / 48.0).min(1.0), 96);
        let b = run(Params::creepy_smoke(), 7, |i| (i as f32 / 48.0).min(1.0), 96);
        assert_eq!(a, b, "preview, seek and export must agree");
        assert_ne!(a.parts, run(Params::creepy_smoke(), 8, |i| (i as f32 / 48.0).min(1.0), 96).parts);
    }

    #[test]
    fn the_sound_thickens_and_churns_the_smoke() {
        let quiet = run(Params::creepy_smoke(), 3, |_| 0.0, 96);
        let loud = run(Params::creepy_smoke(), 3, |_| 1.0, 96);
        assert!(loud.parts.len() as f32 > quiet.parts.len() as f32 * 1.8, "emission follows the drive: {} vs {}", loud.parts.len(), quiet.parts.len());
        let spread = |s: &System| { let n = s.parts.len() as f32; let mx = s.parts.iter().map(|q| q.x).sum::<f32>() / n;
            s.parts.iter().map(|q| (q.x - mx).abs()).sum::<f32>() / n };
        assert!(spread(&loud) > spread(&quiet) * 1.3, "turbulence follows the drive: {} vs {}", spread(&loud), spread(&quiet));
    }

    #[test]
    fn the_swirl_circulates_rather_than_bunching() {
        // numerical divergence of the flow is tiny next to its curl
        let (h, sc) = (0.5f32, 45.0f32);
        let (mut div, mut rot) = (0.0f32, 0.0f32);
        for i in 0..40 { for j in 0..30 {
            let (x, y, t) = (i as f32 * 7.0, j as f32 * 7.0, 1.3);
            let (ax, _) = curl(x + h, y, t, sc); let (bx, _) = curl(x - h, y, t, sc);
            let (_, ay) = curl(x, y + h, t, sc); let (_, by) = curl(x, y - h, t, sc);
            let (cx0, _) = curl(x, y + h, t, sc); let (cx1, _) = curl(x, y - h, t, sc);
            let (_, dy0) = curl(x + h, y, t, sc); let (_, dy1) = curl(x - h, y, t, sc);
            div += ((ax - bx) + (ay - by)).abs() / (2.0 * h);
            rot += ((dy0 - dy1) - (cx0 - cx1)).abs() / (2.0 * h);
        }}
        assert!(div < rot * 0.02, "divergence {div} vs curl {rot}");
    }

    #[test]
    fn each_preset_moves_the_way_its_name_says() {
        let dirt = run(Params::floating_dirt(), 1, |_| 0.3, 72);
        let up = dirt.parts.iter().map(|q| q.y).sum::<f32>() / dirt.parts.len() as f32;
        assert!(up < 118.0, "dirt floats UP from the emitter: mean y {up}");
        let dust = run(Params::space_dust(), 1, |_| 0.3, 36);
        let out = dust.parts.iter().map(|q| (q.x - 160.0).hypot(q.y - 120.0)).sum::<f32>() / dust.parts.len() as f32;
        assert!(out > 25.0, "dust bursts OUTWARD: mean distance {out}");
        let smoke = run(Params::creepy_smoke(), 1, |_| 0.3, 96);
        assert!(smoke.parts.iter().all(|q| q.age < q.life), "nothing outlives its life");
    }

    #[test]
    fn the_emitter_can_ride_a_rig() {
        // an emitter moving 3 px/frame to the right: new particles are born along its path
        let mut s = System::new(Params::space_dust(), 5);
        for i in 0..24 { s.step(1.0 / 24.0, (40.0 + 3.0 * i as f32, 100.0), 0.5); }
        let young: Vec<&Particle> = s.parts.iter().filter(|q| q.age < 0.1).collect();
        let mx = young.iter().map(|q| q.x).sum::<f32>() / young.len().max(1) as f32;
        assert!((mx - (40.0 + 3.0 * 23.0)).abs() < 15.0, "newborns are at the emitter's CURRENT place: {mx}");
    }

    #[test]
    fn an_envelope_blooms_and_lingers() {
        let sr = 48_000;
        let mut a = vec![0.0f32; sr * 2];
        for (i, s) in a.iter_mut().enumerate().skip(sr / 2).take(sr / 4) { *s = 0.5 * (i as f32 * 0.05).sin(); }
        let e = envelope(&a, sr as u32, 24.0);
        assert!(e[5] < 0.05, "silence is 0");
        let peak = e.iter().cloned().fold(0.0, f32::max);
        assert!(peak > 0.7, "a -9 dBFS burst drives it high: {peak}");
        let after = e[(0.75 * 24.0) as usize + 4];
        assert!(after > 0.2 && after < peak, "and it lingers after the burst ends: {after}");
    }
}
