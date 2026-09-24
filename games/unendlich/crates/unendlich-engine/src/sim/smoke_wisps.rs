//! `sim::smoke_wisps` — what a dragon's smoke looks like: wisps that carry
//! how it **feels**, never where it **is**.
//!
//! Design rule (from play discussion): smoke that trails, parts around, or
//! puffs from the dragon gives its position away and defeats the point of
//! smoking. So nothing here knows the dragon's position — by construction:
//! [`WispField::emit`] spawns wisps uniformly inside the smoke *volumes*
//! (`smoke_wall`), and [`WispField::step`] takes the volumes, the wind, the
//! player, and a feeling per volume owner. No dragon coordinates cross this
//! API.
//!
//! What it does carry is emotion ([`Feeling`] → [`Look`]):
//! * **Hunger** — thin, pale, sickly tendrils creep along the ground and
//!   *reach toward you*. It tells you that you are being hunted, not from where.
//! * **Rage** — the whole cloud churns: dark, fast, dense, with embers that
//!   pulse on a quick heartbeat.
//! * **Fear / pain** — the cloud draws in on itself, thins, goes cold grey,
//!   and stutters.
//! * **Calm** (a patient, cunning hunter) — almost nothing moves. Heavy, still,
//!   quiet. The worst one.
//! * **Breath** — the whole cloud swells and settles with the dragon's
//!   breathing, in the same phase everywhere, so it carries tempo, not
//!   direction; hunger and rage quicken it.
//!
//! Motion is a divergence-free curl-noise field (wisps curl and fold instead
//! of blobbing), plus buoyancy and wind. Pure and seeded.

use crate::sim::smoke_wall::SmokeVolume;
use std::f32::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Feeling {
    pub hunger: f32,
    pub rage: f32,
    pub fear: f32,
    pub calm: f32,
}

impl Feeling {
    /// From state the game already has: hunger = 1 − energy; rage from fresh
    /// pain and a fierce temperament; fear from low health; calm from a
    /// cunning mind that is not hurt.
    pub fn from_state(hunger01: f32, pain: f32, hp01: f32, ferocity: f32, cunning: f32) -> Feeling {
        let c = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        let fear = c((0.5 - c(hp01)) * 2.0);
        let rage = c(pain * 4.0 * (0.4 + ferocity) + 0.3 * c(ferocity) - fear * 0.5);
        let hunger = c(hunger01);
        let calm = c(cunning * (1.0 - rage) * (1.0 - fear) * (1.0 - 0.5 * hunger));
        Feeling { hunger, rage, fear, calm }
    }
}

/// How a feeling shows in the smoke.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// Curl-noise strength (m/s) and its time rate.
    pub churn: f32,
    pub churn_rate: f32,
    /// Base colour (linear RGB) and opacity multiplier.
    pub color: [f32; 3],
    pub opacity: f32,
    /// Breaths per second and how much the cloud swells (0..1).
    pub breath_hz: f32,
    pub breath_depth: f32,
    /// Drift toward the player (m/s) — hunger's reaching tendrils.
    pub reach: f32,
    /// Drift toward the volume's centre (m/s) — fear drawing in.
    pub withdraw: f32,
    /// Buoyancy (m/s): heavy calm or sooty rage sinks, thin fear rises.
    pub rise: f32,
    /// 0..1 embers glowing inside, and their pulse rate (Hz).
    pub ember: f32,
    pub ember_hz: f32,
    /// Emission multiplier (wisps per volume per second).
    pub density: f32,
}

pub fn look(f: &Feeling) -> Look {
    let (h, r, fe, ca) = (f.hunger, f.rage, f.fear, f.calm);
    let grey = [0.30, 0.30, 0.31];
    let sick = [0.46, 0.49, 0.38];
    let soot = [0.10, 0.08, 0.08];
    let cold = [0.42, 0.46, 0.52];
    let w = (1.0 + h + r + fe).max(1e-3);
    let mix = |k: usize| (grey[k] + sick[k] * h + soot[k] * r + cold[k] * fe) / w;
    Look {
        churn: (0.25 + 2.2 * r + 0.5 * h + 0.8 * fe) * (1.0 - 0.85 * ca),
        churn_rate: 0.2 + 1.5 * r + 0.8 * fe,
        color: [mix(0), mix(1), mix(2)],
        opacity: 0.55 + 0.35 * r + 0.15 * ca - 0.3 * fe - 0.15 * h,
        breath_hz: 0.15 + 0.35 * h + 0.55 * r + 0.3 * fe - 0.08 * ca,
        breath_depth: 0.12 + 0.2 * r + 0.1 * h,
        reach: 1.4 * h,
        withdraw: 1.6 * fe,
        rise: 0.35 - 0.3 * ca - 0.2 * r + 0.3 * fe,
        ember: r.max(0.0),
        ember_hz: 1.2 + 1.5 * r,
        density: 0.7 + 0.6 * r + 0.2 * ca - 0.35 * fe,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wisp {
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub age: f32,
    pub life: f32,
    pub size: f32,
    /// Which volume owner (dragon id) it belongs to, for its [`Look`].
    pub owner: u64,
    /// Index of the volume it was born in, for withdrawal.
    pub home: [f32; 3],
    pub phase: f32,
}

/// A soft round card facing the camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Card {
    pub center: [f32; 3],
    pub radius: f32,
    pub color: [f32; 3],
    pub alpha: f32,
}

pub struct WispField {
    pub wisps: Vec<Wisp>,
    pub cap: usize,
    seed: u64,
    n: u64,
    acc: f32,
    pub time: f32,
}

fn h01(seed: u64, i: u64) -> f32 {
    let mut z = seed ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
}

/// Divergence-free curl noise: velocity = curl of a smooth vector potential
/// built from a few seeded sines. Exactly divergence-free analytically, so
/// wisps fold and curl rather than bunch up or spread out.
pub fn curl(p: [f32; 3], t: f32, seed: u64) -> [f32; 3] {
    // Potential A = (a1, a2, a3); v = ∇ × A.
    let k = |i: u64| 0.06 + 0.1 * h01(seed, i);
    let ph = |i: u64| h01(seed, 100 + i) * TAU;
    let (kx, ky, kz) = (k(1), k(2), k(3));
    let (p1, p2, p3) = (ph(1) + t, ph(2) + 0.7 * t, ph(3) + 1.3 * t);
    // a1 = sin(ky y + p1) cos(kz z)  a2 = sin(kz z + p2) cos(kx x)  a3 = sin(kx x + p3) cos(ky y)
    let (x, y, z) = (p[0], p[1], p[2]);
    let da3_dy = -(kx * x + p3).sin() * ky * (ky * y).sin();
    let da2_dz = kz * (kz * z + p2).cos() * (kx * x).cos();
    let da1_dz = -(ky * y + p1).sin() * kz * (kz * z).sin();
    let da3_dx = kx * (kx * x + p3).cos() * (ky * y).cos();
    let da2_dx = -(kz * z + p2).sin() * kx * (kx * x).sin();
    let da1_dy = ky * (ky * y + p1).cos() * (kz * z).cos();
    let s = 1.0 / (kx.max(ky).max(kz));
    [(da3_dy - da2_dz) * s, (da1_dz - da3_dx) * s, (da2_dx - da1_dy) * s]
}

impl WispField {
    pub fn new(cap: usize, seed: u64) -> WispField {
        WispField { wisps: Vec::with_capacity(cap), cap, seed, n: 0, acc: 0.0, time: 0.0 }
    }

    /// Spawn wisps inside the volumes, spread uniformly through each —
    /// never at a dragon. `owner_of[i]` names volume `i`'s owner.
    pub fn emit(&mut self, vols: &[SmokeVolume], owner_of: &[u64], looks: &impl Fn(u64) -> Look, dt: f32) {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.5) } else { 0.0 };
        for (i, v) in vols.iter().enumerate() {
            if !v.alive() {
                continue;
            }
            let owner = owner_of.get(i).copied().unwrap_or(0);
            let lk = looks(owner);
            let volume = v.half[0] * v.half[1] * v.half[2] * 8.0;
            self.acc += dt * lk.density.max(0.0) * (volume / 60.0).clamp(1.0, 40.0) * v.density_at(v.center).min(1.0) * 6.0;
            while self.acc >= 1.0 {
                self.acc -= 1.0;
                if self.wisps.len() >= self.cap {
                    self.wisps.remove(0);
                }
                self.n += 1;
                let r = |j: u64| h01(self.seed ^ self.n, j) * 2.0 - 1.0;
                let (a, c, y) = (r(1) * v.half[0], r(2) * v.half[1], r(3) * v.half[2]);
                let pos = [
                    v.center[0] + a * v.along[0] - c * v.along[1],
                    v.center[1] + y,
                    v.center[2] + a * v.along[1] + c * v.along[0],
                ];
                self.wisps.push(Wisp {
                    pos,
                    vel: [0.0; 3],
                    age: 0.0,
                    life: 5.0 + 5.0 * h01(self.seed ^ self.n, 4),
                    size: 2.5 + 2.5 * h01(self.seed ^ self.n, 5),
                    owner,
                    home: v.center,
                    phase: h01(self.seed ^ self.n, 6) * TAU,
                });
            }
        }
    }

    /// Advance every wisp. No dragon position is an input.
    pub fn step(&mut self, looks: &impl Fn(u64) -> Look, wind: [f32; 2], player: Option<[f32; 3]>, dt: f32) {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.5) } else { 0.0 };
        self.time += dt;
        let t = self.time;
        let seed = self.seed;
        let w = |v: f32| if v.is_finite() { v.clamp(-30.0, 30.0) } else { 0.0 };
        for s in self.wisps.iter_mut() {
            let lk = looks(s.owner);
            let c = curl(s.pos, t * lk.churn_rate, seed);
            let mut target = [
                c[0] * lk.churn + w(wind[0]) * 0.5,
                c[1] * lk.churn * 0.6 + lk.rise,
                c[2] * lk.churn + w(wind[1]) * 0.5,
            ];
            if let Some(pp) = player.filter(|p| p.iter().all(|v| v.is_finite())) {
                // Hunger reaches along the ground toward you.
                let (dx, dz) = (pp[0] - s.pos[0], pp[2] - s.pos[2]);
                let l = (dx * dx + dz * dz).sqrt().max(1.0);
                target[0] += lk.reach * dx / l;
                target[2] += lk.reach * dz / l;
                target[1] -= lk.reach * 0.3;
            }
            // Fear draws the cloud in on itself.
            let (hx, hz) = (s.home[0] - s.pos[0], s.home[2] - s.pos[2]);
            let hl = (hx * hx + hz * hz).sqrt().max(1.0);
            target[0] += lk.withdraw * hx / hl;
            target[2] += lk.withdraw * hz / hl;
            let k = (dt * 2.0).min(1.0);
            for i in 0..3 {
                s.vel[i] += (target[i] - s.vel[i]) * k;
                s.pos[i] += s.vel[i] * dt;
            }
            s.age += dt;
            s.size += dt * (0.25 + 0.3 * lk.churn.min(2.0));
        }
        self.wisps.retain(|s| s.age < s.life);
    }

    /// One card per wisp. The breath swells every card in the same phase.
    pub fn cards(&self, looks: &impl Fn(u64) -> Look) -> Vec<Card> {
        let t = self.time;
        self.wisps
            .iter()
            .map(|s| {
                let lk = looks(s.owner);
                let u = s.age / s.life.max(1e-3);
                let fade = (u * 5.0).min(1.0) * (1.0 - u).max(0.0);
                let breath = 1.0 + lk.breath_depth * (TAU * lk.breath_hz * t).sin();
                let ember = if lk.ember > 0.0 {
                    lk.ember * (0.5 + 0.5 * (TAU * lk.ember_hz * t + s.phase).sin()).powi(6)
                } else {
                    0.0
                };
                Card {
                    center: s.pos,
                    radius: s.size * breath,
                    color: [
                        lk.color[0] + ember * 0.9,
                        lk.color[1] + ember * 0.3,
                        lk.color[2] + ember * 0.05,
                    ],
                    alpha: (lk.opacity * fade * 0.75).clamp(0.0, 0.85),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol() -> SmokeVolume {
        SmokeVolume::wall([0.0, 64.0, 0.0], [-10.0, 64.0, 0.0], [10.0, 64.0, 0.0], 20.0, 14.0, 60.0)
    }

    fn run(f: Feeling, player: Option<[f32; 3]>, secs: f32) -> WispField {
        let lk = look(&f);
        let looks = |_o: u64| lk;
        let mut w = WispField::new(400, 7);
        let v = [vol()];
        for _ in 0..(secs / 0.1) as usize {
            w.emit(&v, &[1], &looks, 0.1);
            w.step(&looks, [0.0, 0.0], player, 0.1);
        }
        w
    }

    fn mean(w: &WispField, f: impl Fn(&Wisp) -> f32) -> f32 {
        w.wisps.iter().map(f).sum::<f32>() / w.wisps.len().max(1) as f32
    }

    #[test]
    fn curl_noise_is_divergence_free() {
        let e = 1e-2;
        for i in 0..50 {
            let p = [i as f32 * 3.1, 60.0 + i as f32 * 0.7, i as f32 * -2.3];
            let d = |a: usize| {
                let mut p1 = p;
                let mut p0 = p;
                p1[a] += e;
                p0[a] -= e;
                (curl(p1, 1.0, 3)[a] - curl(p0, 1.0, 3)[a]) / (2.0 * e)
            };
            let div = d(0) + d(1) + d(2);
            assert!(div.abs() < 5e-3, "div {div} at {p:?}");
        }
    }

    #[test]
    fn wisps_are_born_inside_the_volume_not_at_the_dragon() {
        let lk = look(&Feeling::default());
        let mut w = WispField::new(1000, 1);
        w.emit(&[vol()], &[1], &|_| lk, 2.0);
        assert!(!w.wisps.is_empty());
        let v = vol();
        for s in &w.wisps {
            assert!(v.density_at(s.pos) > 0.0 || (s.pos[1] - v.center[1]).abs() <= v.half[2] + 0.01);
        }
        // Spread across the whole wall, not a clump.
        let xs: Vec<f32> = w.wisps.iter().map(|s| s.pos[2]).collect();
        let span = xs.iter().cloned().fold(f32::MIN, f32::max) - xs.iter().cloned().fold(f32::MAX, f32::min);
        assert!(span > v.half[0], "span {span}");
    }

    #[test]
    fn hunger_reaches_toward_you() {
        let you = [0.0, 64.0, 40.0];
        let hungry = run(Feeling { hunger: 1.0, ..Default::default() }, Some(you), 8.0);
        let calm = run(Feeling { calm: 1.0, ..Default::default() }, Some(you), 8.0);
        let toward = |w: &WispField| mean(w, |s| s.vel[2]);
        assert!(toward(&hungry) > 0.8, "hungry drift {}", toward(&hungry));
        assert!(toward(&hungry) > toward(&calm) + 0.6);
    }

    #[test]
    fn rage_churns_and_darkens_calm_barely_moves() {
        let rage = run(Feeling { rage: 1.0, ..Default::default() }, None, 6.0);
        let calm = run(Feeling { calm: 1.0, ..Default::default() }, None, 6.0);
        let speed = |w: &WispField| mean(w, |s| (s.vel[0].powi(2) + s.vel[1].powi(2) + s.vel[2].powi(2)).sqrt());
        assert!(speed(&rage) > speed(&calm) * 4.0, "rage {} calm {}", speed(&rage), speed(&calm));
        let (lr, lc) = (look(&Feeling { rage: 1.0, ..Default::default() }), look(&Feeling { calm: 1.0, ..Default::default() }));
        assert!(lr.color.iter().sum::<f32>() < lc.color.iter().sum::<f32>());
        assert!(lr.ember > 0.5 && lc.ember == 0.0);
        assert!(lr.breath_hz > lc.breath_hz * 2.0);
    }

    #[test]
    fn fear_draws_the_cloud_in() {
        let afraid = run(Feeling { fear: 1.0, ..Default::default() }, None, 8.0);
        let calm = run(Feeling { calm: 1.0, ..Default::default() }, None, 8.0);
        let spread = |w: &WispField| mean(w, |s| ((s.pos[0] - s.home[0]).powi(2) + (s.pos[2] - s.home[2]).powi(2)).sqrt());
        assert!(spread(&afraid) < spread(&calm) * 0.8, "afraid {} calm {}", spread(&afraid), spread(&calm));
        assert!(look(&Feeling { fear: 1.0, ..Default::default() }).opacity < look(&Feeling::default()).opacity);
    }

    #[test]
    fn the_breath_is_the_same_everywhere() {
        let w = run(Feeling { rage: 0.6, hunger: 0.4, ..Default::default() }, None, 3.0);
        let lk = look(&Feeling { rage: 0.6, hunger: 0.4, ..Default::default() });
        let cards = w.cards(&|_| lk);
        // Every card is scaled by the same breath factor: size ratio card/wisp is uniform.
        let ratios: Vec<f32> = cards.iter().zip(&w.wisps).map(|(c, s)| c.radius / s.size).collect();
        let (lo, hi) = (ratios.iter().cloned().fold(f32::MAX, f32::min), ratios.iter().cloned().fold(f32::MIN, f32::max));
        assert!(hi - lo < 1e-4, "breath differs across the cloud: {lo}..{hi}");
    }

    #[test]
    fn feelings_come_from_state() {
        let starving = Feeling::from_state(0.9, 0.0, 1.0, 0.3, 0.2);
        assert!(starving.hunger > 0.8 && starving.fear == 0.0);
        let wounded = Feeling::from_state(0.2, 0.3, 0.2, 0.8, 0.5);
        assert!(wounded.fear > 0.5);
        let stung = Feeling::from_state(0.2, 0.2, 0.9, 0.9, 0.5);
        assert!(stung.rage > 0.6);
        let patient = Feeling::from_state(0.1, 0.0, 1.0, 0.1, 0.9);
        assert!(patient.calm > 0.7 && patient.rage < 0.2);
    }

    #[test]
    fn capped_and_deterministic() {
        let a = run(Feeling { rage: 1.0, ..Default::default() }, None, 30.0);
        let b = run(Feeling { rage: 1.0, ..Default::default() }, None, 30.0);
        assert!(a.wisps.len() <= 400);
        assert_eq!(a.wisps, b.wisps);
    }
}
