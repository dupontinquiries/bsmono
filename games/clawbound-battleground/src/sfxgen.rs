//! sfxgen -- the game's sound effects, designed in code.
//!
//! Every ancestor gets a sonic identity from who they are, and each of its
//! four abilities a sound from what the ability does:
//!
//! - **Base pitch from role**: guardians low and heavy, warriors, then
//!   assassins, hunters, mages highest; nudged by the ancestor's id so no
//!   two share one.
//! - **Brightness from the accent colour** (luminance): golden Suneye rings,
//!   dusky Nightpelt stays dark.
//! - **Grit from role**: guardians and warriors driven and crunchy, mages
//!   clean.
//! - **A recipe per archetype**: a bolt is a descending zap, a dash a
//!   whoosh, a nova a boom, a shield glassy chimes, a storm rumble and
//!   crackle, a heal a bell chord... Ultimates get a sub layer and a tail.
//!
//! Plus the shared sounds the cues ask for (`sfx/hit`, `sfx/hit_big`,
//! `sfx/whiff`, `sfx/levelup`, `sfx/tower_down`, `sfx/cast`).
//! `examples/gen_sfx` renders them all to `.opus` under the audio root, in
//! the keys `cues` plays; a studio file of the same name simply replaces
//! one.

use crate::gods::{Archetype, GodDef, Role};

pub const RATE: f32 = 48_000.0;

/// A cheap, deterministic noise source.
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// Chamberlin state-variable filter: low, band, high at once.
#[derive(Default, Clone, Copy)]
struct Svf {
    low: f32,
    band: f32,
}
impl Svf {
    fn run(&mut self, x: f32, hz: f32, q: f32) -> (f32, f32, f32) {
        let f = 2.0 * (std::f32::consts::PI * (hz / RATE).clamp(0.0001, 0.24)).sin();
        let q = 1.0 / q.max(0.5);
        self.low += f * self.band;
        let high = x - self.low - q * self.band;
        self.band += f * high;
        (self.low, self.band, high)
    }
}

/// Who an ancestor sounds like.
#[derive(Debug, Clone, Copy)]
pub struct Flavor {
    pub base: f32,
    pub bright: f32,
    pub grit: f32,
    pub seed: u32,
}

impl Flavor {
    pub fn of(def: &GodDef) -> Flavor {
        let (base, grit) = match def.role {
            Role::Guardian => (72.0, 0.7),
            Role::Warrior => (110.0, 0.55),
            Role::Assassin => (160.0, 0.35),
            Role::Hunter => (220.0, 0.3),
            Role::Mage => (300.0, 0.15),
        };
        let a = def.palette[3];
        let bright = (0.2126 * a[0] + 0.7152 * a[1] + 0.0722 * a[2]).clamp(0.0, 1.0);
        let nudge = 1.0 + ((def.id as f32 * 0.618).fract() - 0.5) * 0.3;
        Flavor { base: base * nudge, bright, grit, seed: 0x1234_5678 ^ def.id.wrapping_mul(2654435761) }
    }
}

/// Fade a layer's last 10 ms: a layer cut mid-decay clicks where it ends
/// (seen in the spectrograms as a vertical line at 0.65 s in a nova and 0.8 s
/// in a heal, where the reverb tail begins).
fn tail(mut x: Vec<f32>) -> Vec<f32> {
    let n = x.len();
    let f = secs(0.010).min(n);
    for i in 0..f {
        x[n - 1 - i] *= i as f32 / f as f32;
    }
    x
}

fn secs(n: f32) -> usize {
    (n * RATE) as usize
}

/// Exponential decay envelope with a short linear attack.
fn env(t: f32, attack: f32, decay: f32) -> f32 {
    if t < attack {
        t / attack.max(1e-4)
    } else {
        (-(t - attack) / decay.max(1e-4)).exp()
    }
}

fn lerp_exp(a: f32, b: f32, u: f32) -> f32 {
    a * (b / a).powf(u.clamp(0.0, 1.0))
}

/// A few damped combs: space for ultimates and the soft sounds.
fn reverb(x: &mut Vec<f32>, wet: f32, tail: f32) {
    let extra = secs(tail);
    x.resize(x.len() + extra, 0.0);
    let dry = x.clone();
    for (d, g) in [(1116usize, 0.78f32), (1277, 0.76), (1491, 0.74), (1617, 0.72)] {
        let mut buf = vec![0.0f32; d];
        let mut i = 0;
        let mut lp = 0.0f32;
        for (k, s) in dry.iter().enumerate() {
            let y = buf[i];
            lp = lp * 0.3 + y * 0.7;
            buf[i] = s + lp * g;
            i = (i + 1) % d;
            x[k] += y * wet * 0.25;
        }
    }
}

/// Peak to `peak`, 2 ms fade in, 12 ms fade out: no clicks.
fn finish(mut x: Vec<f32>, peak: f32) -> Vec<f32> {
    let m = x.iter().fold(0.0f32, |a, v| a.max(v.abs())).max(1e-6);
    let g = peak / m;
    let (fi, fo) = (secs(0.002), secs(0.012));
    let n = x.len();
    for (i, v) in x.iter_mut().enumerate() {
        let mut k = g;
        if i < fi {
            k *= i as f32 / fi as f32;
        }
        if i + fo > n {
            k *= (n - i) as f32 / fo as f32;
        }
        *v *= k;
    }
    x
}

fn drive(v: f32, grit: f32) -> f32 {
    let k = 1.0 + grit * 4.0;
    (v * k).tanh() / k.tanh()
}

// --- layers ---------------------------------------------------------------

fn zap(len: f32, f0: f32, f1: f32, fl: Flavor, n: &mut Noise) -> Vec<f32> {
    let mut out = vec![0.0; secs(len)];
    let (mut ph, mut svf, mut svf2) = (0.0f32, Svf::default(), Svf::default());
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / RATE;
        let u = t / len;
        let f = lerp_exp(f0, f1, u.powf(0.6));
        ph = (ph + f / RATE).fract();
        let saw = ph * 2.0 - 1.0;
        let (lp, _, _) = svf.run(saw, 1500.0 + 4500.0 * fl.bright * (1.0 - u), 1.2);
        let (_, bp, _) = svf2.run(n.next(), lerp_exp(3200.0, 700.0, u), 2.5);
        *o = drive(lp * 0.8 + bp * 0.5, fl.grit * 0.6) * env(t, 0.004, len * 0.35);
    }
    tail(out)
}

fn whoosh(len: f32, f0: f32, f1: f32, q: f32, n: &mut Noise) -> Vec<f32> {
    let mut out = vec![0.0; secs(len)];
    let mut svf = Svf::default();
    for (i, o) in out.iter_mut().enumerate() {
        let u = i as f32 / out_len(len);
        let f = lerp_exp(f0, f1, u);
        let (_, bp, _) = svf.run(n.next(), f, q);
        *o = bp * (std::f32::consts::PI * u).sin().powf(0.7);
    }
    tail(out)
}

fn out_len(len: f32) -> f32 {
    secs(len) as f32
}

fn thud(len: f32, f0: f32, f1: f32, grit: f32, n: &mut Noise) -> Vec<f32> {
    let mut out = vec![0.0; secs(len)];
    let mut ph = 0.0f32;
    let mut svf = Svf::default();
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / RATE;
        let f = lerp_exp(f0, f1, (t / (len * 0.5)).min(1.0));
        ph = (ph + f / RATE).fract();
        let body = (ph * std::f32::consts::TAU).sin() * env(t, 0.002, len * 0.3);
        let (lp, _, _) = svf.run(n.next(), lerp_exp(5000.0, 300.0, t / len), 0.8);
        let click = lp * env(t, 0.0005, 0.04);
        *o = drive(body + click * 0.7, grit);
    }
    tail(out)
}

/// Additive / light-FM tones: bells, chimes, glass.
fn tones(len: f32, freqs: &[f32], attack: f32, decay: f32, fm: f32, trem: f32) -> Vec<f32> {
    let mut out = vec![0.0; secs(len)];
    for (k, &f) in freqs.iter().enumerate() {
        let (mut pc, mut pm) = (0.0f32, 0.0f32);
        for (i, o) in out.iter_mut().enumerate() {
            let t = i as f32 / RATE;
            pm = (pm + f * 1.41 / RATE).fract();
            let m = (pm * std::f32::consts::TAU).sin() * fm * env(t, 0.001, decay * 0.4);
            pc = (pc + f * (1.0 + m * 0.02) / RATE).fract();
            let tr = 1.0 - trem * (0.5 + 0.5 * (t * 11.0 * std::f32::consts::TAU).sin());
            *o += (pc * std::f32::consts::TAU + m).sin() * env(t, attack, decay) * tr / (1.0 + k as f32 * 0.4);
        }
    }
    tail(out)
}

fn mix_at(dst: &mut Vec<f32>, src: &[f32], at: f32, gain: f32) {
    let o = secs(at);
    if dst.len() < o + src.len() {
        dst.resize(o + src.len(), 0.0);
    }
    for (i, s) in src.iter().enumerate() {
        dst[o + i] += s * gain;
    }
}

// --- recipes ----------------------------------------------------------------

/// The sound of `def`'s ability in `slot`.
pub fn ability(def: &GodDef, slot: usize) -> Vec<f32> {
    let fl = Flavor::of(def);
    let ab = &def.abilities[slot];
    let b = fl.base;
    let mut n = Noise(fl.seed ^ (slot as u32).wrapping_mul(0x9E37_79B9) | 1);
    let mut x: Vec<f32> = Vec::new();
    match ab.arch {
        Archetype::Bolt { .. } => {
            x = zap(0.32, b * 6.0, b * 1.2, fl, &mut n);
        }
        Archetype::Volley { .. } => {
            for (k, m) in [3.0f32, 3.6, 4.3].iter().enumerate() {
                let c = zap(0.13, b * m * 1.6, b * m * 0.7, fl, &mut n);
                mix_at(&mut x, &c, k as f32 * 0.07, 0.8);
            }
        }
        Archetype::Lob { .. } => {
            let w = whoosh(0.3, 400.0, 1600.0 + 1200.0 * fl.bright, 1.8, &mut n);
            mix_at(&mut x, &w, 0.0, 0.8);
            let t = thud(0.35, b * 1.2, b * 0.45, fl.grit, &mut n);
            mix_at(&mut x, &t, 0.27, 1.0);
        }
        Archetype::Beam { .. } => {
            let len = 0.7;
            x = vec![0.0; secs(len)];
            let (mut p1, mut p2, mut svf, mut hp) = (0.0f32, 0.0f32, Svf::default(), Svf::default());
            for (i, o) in x.iter_mut().enumerate() {
                let t = i as f32 / RATE;
                let vib = 1.0 + 0.012 * (t * 7.0 * std::f32::consts::TAU).sin();
                p1 = (p1 + b * 2.0 * vib / RATE).fract();
                p2 = (p2 + b * 2.02 * vib / RATE).fract();
                let (lp, _, _) = svf.run(p1 + p2 - 1.0, 1800.0 + 2500.0 * fl.bright, 1.5);
                let (_, _, hiss) = hp.run(n.next(), 5000.0, 0.7);
                let e = if t < 0.05 { t / 0.05 } else if t > len - 0.15 { (len - t) / 0.15 } else { 1.0 };
                *o = drive(lp * 0.7 + hiss * 0.15, fl.grit * 0.5) * e;
            }
        }
        Archetype::Ring { .. } => {
            let len = 0.5;
            x = tones(len, &[b * 4.0, b * 6.02, b * 8.9], 0.02, 0.3, 1.5, 0.0);
            // a rising charge under the bell
            let mut ph = 0.0f32;
            for (i, o) in x.iter_mut().enumerate() {
                let u = i as f32 / secs(len) as f32;
                ph = (ph + lerp_exp(b * 2.0, b * 5.0, u) / RATE).fract();
                *o += (ph * std::f32::consts::TAU).sin() * 0.3 * u * (1.0 - u) * 4.0;
            }
        }
        Archetype::Storm { .. } => {
            let len = 0.9;
            x = vec![0.0; secs(len)];
            let (mut rumble, mut crack, mut ph) = (Svf::default(), Svf::default(), 0.0f32);
            for (i, o) in x.iter_mut().enumerate() {
                let t = i as f32 / RATE;
                let u = t / len;
                let (lp, _, _) = rumble.run(n.next(), 140.0, 0.9);
                ph = (ph + b * 0.5 / RATE).fract();
                let sub = (ph * std::f32::consts::TAU).sin() * 0.4;
                let spark = if n.next() > 0.9985 - 0.002 * u { 4.0 } else { 0.0 };
                let (_, bp, _) = crack.run(spark + n.next() * 0.05, 2500.0 + 2000.0 * fl.bright, 3.0);
                *o = drive(lp * 2.5 + sub + bp * 0.9, fl.grit) * (u * 6.0).min(1.0) * (1.0 - u).powf(0.6);
            }
        }
        Archetype::Dash { .. } => {
            let w = whoosh(0.3, 300.0, 2600.0, 1.4, &mut n);
            mix_at(&mut x, &w, 0.0, 1.0);
            let t = thud(0.12, b * 1.4, b * 0.8, fl.grit, &mut n);
            mix_at(&mut x, &t, 0.0, 0.6);
        }
        Archetype::Blink { .. } => {
            let len = 0.28;
            x = vec![0.0; secs(len)];
            let mut ph = 0.0f32;
            for (i, o) in x.iter_mut().enumerate() {
                let t = i as f32 / RATE;
                ph = (ph + lerp_exp(b * 4.0, b * 18.0, (t / 0.08).min(1.0)) / RATE).fract();
                *o = (ph * std::f32::consts::TAU).sin() * env(t, 0.002, 0.05);
            }
            let sh = tones(0.25, &[b * 8.0, b * 12.1], 0.03, 0.12, 0.3, 0.6);
            mix_at(&mut x, &sh, 0.03, 0.5);
            let click = thud(0.03, 3000.0, 1500.0, 0.0, &mut n);
            mix_at(&mut x, &click, 0.0, 0.4);
        }
        Archetype::Pull { .. } => {
            let len = 0.45;
            let mut w = whoosh(len, 2500.0, 300.0, 1.6, &mut n);
            let m = w.len() as f32;
            for (i, v) in w.iter_mut().enumerate() {
                *v *= (i as f32 / m).powf(1.8) * 2.0; // swelling: sucked in
            }
            mix_at(&mut x, &w, 0.0, 1.0);
            let t = thud(0.3, b * 2.0, b * 0.6, fl.grit, &mut n);
            mix_at(&mut x, &t, len - 0.05, 0.9);
        }
        Archetype::Nova { .. } => {
            let t = thud(0.65, b * 1.6, 36.0, fl.grit, &mut n);
            mix_at(&mut x, &t, 0.0, 1.0);
            let w = whoosh(0.5, 6000.0, 300.0, 0.9, &mut n);
            mix_at(&mut x, &w, 0.0, 0.8);
        }
        Archetype::Shield { .. } => {
            x = tones(0.55, &[b * 4.0, b * 5.0, b * 6.0], 0.08, 0.35, 0.2, 0.35);
        }
        Archetype::Buff { .. } => {
            for (k, r) in [1.0f32, 1.25, 1.5, 2.0].iter().enumerate() {
                let n1 = tones(0.18, &[b * 4.0 * r, b * 12.0 * r], 0.005, 0.08, 0.6, 0.0);
                mix_at(&mut x, &n1, k as f32 * 0.075, 0.8);
            }
        }
        Archetype::Heal { .. } => {
            x = tones(0.8, &[b * 4.0, b * 5.0, b * 6.0, b * 8.0], 0.01, 0.35, 0.8, 0.0);
            reverb(&mut x, 0.6, 0.3);
        }
    }
    let mut x = tail(x);
    if ab.ult {
        // weight: a sub swell under it, and space after it
        let len = x.len() as f32 / RATE;
        let mut ph = 0.0f32;
        for (i, v) in x.iter_mut().enumerate() {
            let u = i as f32 / (len * RATE);
            ph = (ph + b * 0.5 / RATE).fract();
            *v += (ph * std::f32::consts::TAU).sin() * 0.45 * (u * 4.0).min(1.0) * (1.0 - u);
        }
        reverb(&mut x, 0.7, 0.35);
    }
    finish(x, 0.85)
}

/// The shared sounds, by bank key.
pub const GENERIC: [&str; 6] = ["sfx/hit", "sfx/hit_big", "sfx/whiff", "sfx/levelup", "sfx/tower_down", "sfx/cast"];

pub fn generic(key: &str) -> Vec<f32> {
    let mut n = Noise(0xC0FF_EE11 ^ (key.len() as u32).wrapping_mul(7919));
    let x = match key {
        "sfx/hit" => thud(0.12, 160.0, 70.0, 0.5, &mut n),
        "sfx/hit_big" => {
            let mut x = thud(0.28, 140.0, 45.0, 0.8, &mut n);
            let w = whoosh(0.15, 4000.0, 900.0, 1.2, &mut n);
            mix_at(&mut x, &w, 0.0, 0.7);
            x
        }
        "sfx/whiff" => whoosh(0.22, 900.0, 2600.0, 2.2, &mut n),
        "sfx/levelup" => {
            let mut x = Vec::new();
            for (k, r) in [1.0f32, 1.26, 1.5, 2.0, 2.52].iter().enumerate() {
                let t = tones(0.25, &[520.0 * r, 1560.0 * r], 0.004, 0.12, 0.8, 0.0);
                mix_at(&mut x, &t, k as f32 * 0.06, 0.7);
            }
            reverb(&mut x, 0.5, 0.25);
            x
        }
        "sfx/tower_down" => {
            let len = 1.3;
            let mut x = vec![0.0; secs(len)];
            let (mut rumble, mut debris) = (Svf::default(), Svf::default());
            for (i, o) in x.iter_mut().enumerate() {
                let t = i as f32 / RATE;
                let u = t / len;
                let (lp, _, _) = rumble.run(n.next(), 110.0, 0.8);
                let hit = if n.next() > 0.997 + 0.002 * u { 3.0 } else { 0.0 };
                let (_, bp, _) = debris.run(hit, 1400.0, 2.0);
                *o = drive(lp * 3.0 * (1.0 - u).powf(0.8) + bp * (1.0 - u), 0.7);
            }
            let t = thud(0.5, 90.0, 30.0, 0.9, &mut n);
            mix_at(&mut x, &t, 0.0, 1.0);
            x
        }
        _ => {
            let fl = Flavor { base: 180.0, bright: 0.6, grit: 0.3, seed: 1 };
            zap(0.2, 1100.0, 260.0, fl, &mut n)
        }
    };
    finish(x, 0.8)
}

/// Every sound to generate: (bank key, samples at RATE, mono).
pub fn all() -> Vec<(String, Vec<f32>)> {
    let mut out = Vec::new();
    for def in crate::gods::ROSTER.iter() {
        let who = def.name.to_ascii_lowercase().replace([' ', '\''], "");
        for slot in 0..4 {
            out.push((format!("sfx/cast/{who}/{}", slot + 1), ability(def, slot)));
        }
    }
    for k in GENERIC {
        out.push((k.to_string(), generic(k)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gods::ROSTER;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// Every sound is finite, audible, never clipping, and short enough to
    /// read as an effect.
    #[test]
    fn every_sound_is_clean_audible_and_short() {
        for (k, x) in all() {
            assert!(x.iter().all(|v| v.is_finite()), "{k}: not finite");
            let peak = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
            assert!(peak <= 0.851 && peak > 0.5, "{k}: peak {peak}");
            assert!(rms(&x) > 0.03, "{k}: nearly silent");
            let len = x.len() as f32 / RATE;
            assert!((0.1..=1.7).contains(&len), "{k}: {len:.2} s");
            assert!(x[0].abs() < 0.01 && x[x.len() - 1].abs() < 0.01, "{k}: clicks at an edge");
        }
    }

    /// Two ancestors with the same kind of ability still sound like
    /// themselves: the same archetype at each's base pitch and colour
    /// differs, measured by correlation.
    #[test]
    fn the_same_archetype_sounds_different_per_ancestor() {
        let mut dashes: Vec<Vec<f32>> = Vec::new();
        for def in ROSTER.iter() {
            for s in 0..4 {
                if matches!(def.abilities[s].arch, Archetype::Dash { .. }) && !def.abilities[s].ult {
                    dashes.push(ability(def, s));
                }
            }
        }
        assert!(dashes.len() >= 3);
        for i in 0..dashes.len() {
            for j in i + 1..dashes.len() {
                let n = dashes[i].len().min(dashes[j].len());
                let dot: f32 = (0..n).map(|k| dashes[i][k] * dashes[j][k]).sum();
                let c = dot / (rms(&dashes[i][..n]) * rms(&dashes[j][..n]) * n as f32).max(1e-9);
                assert!(c.abs() < 0.9, "two ancestors' dashes nearly identical (r = {c:.2})");
            }
        }
    }

    /// Guardians are lower than mages: the flavour follows the role.
    #[test]
    fn roles_set_the_pitch() {
        let g = ROSTER.iter().find(|d| d.role == Role::Guardian).unwrap();
        let m = ROSTER.iter().find(|d| d.role == Role::Mage).unwrap();
        assert!(Flavor::of(g).base < Flavor::of(m).base);
        assert!(Flavor::of(g).grit > Flavor::of(m).grit);
    }

    /// The generated keys are exactly what the cues play.
    #[test]
    fn the_keys_are_the_cues_keys() {
        let keys: Vec<String> = all().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys.len(), ROSTER.len() * 4 + GENERIC.len());
        let game = crate::sim::Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        let c = crate::cues::cues_for(&crate::sim::Event::Cast { god: 0, slot: 2, aim: glam::Vec3::ZERO }, &game);
        assert!(keys.contains(&c[0].keys[0]), "the cast cue's first key {} is generated", c[0].keys[0]);
        for g in ["sfx/hit", "sfx/whiff", "sfx/levelup", "sfx/tower_down", "sfx/cast"] {
            assert!(keys.iter().any(|k| k == g));
        }
    }
}
