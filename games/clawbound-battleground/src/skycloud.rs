//! skycloud -- cloud that drifts across Starwell Glacier and hides who is in it.
//!
//! Two kinds, both on the playing surface (not the scenery below it):
//!
//! * **Banks** -- great soft masses 35-65 u across that come in over the
//!   rim and cross the map at walking pace, fading in as they arrive and out
//!   as they leave. A fight caught in one is a fight nobody outside can see.
//! * **Wisps** -- long thin streams that blow through faster, thickening and
//!   thinning every few seconds: a sight line across one flickers.
//!
//! Sight: a line of sight is broken when the cloud along it adds up to
//! [`SIGHT_DEPTH`] u of full-density cloud. Two cats inside the same bank
//! still see each other at arm's length ([`CLOSE`]), and not much further.
//!
//! This is sim, not scenery: who can see whom decides who can be auto-
//! attacked and who the bots go for, and what the player is shown
//! ([`Vision`], `Game::sees`). So it is a pure function of the match's cloud
//! seed and clock -- every peer and every replay has the same weather, on
//! every graphics preset. Sky map only; elsewhere nothing is hidden.

use glam::Vec3;

use crate::sim::Rng;

/// A bank may come in each epoch (s), with this chance.
pub const BANK_EPOCH: f32 = 45.0;
pub const BANK_CHANCE: f32 = 0.8;
/// A wisp may blow through each epoch (s).
pub const WISP_EPOCH: f32 = 9.0;
pub const WISP_CHANCE: f32 = 0.75;
/// How many units of full-density cloud break a line of sight.
pub const SIGHT_DEPTH: f32 = 6.0;
/// Closer than this, cats always see each other: it follows from
/// `SIGHT_DEPTH` (cloud is never denser than 1), and a test holds it so.
pub const CLOSE: f32 = 5.0;
/// How far a cat sees at all (clear air).
pub const SIGHT_RANGE: f32 = 48.0;
/// Banks come in from this far out.
const RIM: f32 = 340.0;
/// Seconds for a cloud to thicken as it forms, or thin as it goes.
const FADE: f32 = 18.0;
/// The first bank can arrive this long into the match (no cloud at the
/// opening: the lanes meet in clear air).
pub const FIRST_CLOUD: f32 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bank,
    Wisp,
}

/// One cloud, where it is now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cloud {
    pub kind: Kind,
    pub pos: Vec3,
    /// Its heading (unit, horizontal) and speed.
    pub dir: Vec3,
    /// A bank's radius; a wisp's half-length along `dir`.
    pub r: f32,
    /// A wisp's half-width.
    pub w: f32,
    /// How thick it is now, 0..=1 (fading in and out; a wisp's pulse).
    pub thick: f32,
    /// A number of its own, for the shape of its edge and the scene's puffs.
    pub seed: u32,
    /// Seconds since it formed.
    pub age: f32,
}

fn hash(a: u64, b: u64) -> u64 {
    let mut x = a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    x
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The clouds over the map at match time `t`, for cloud seed `seed`.
pub fn clouds(seed: u64, t: f32) -> Vec<Cloud> {
    let mut out = Vec::new();
    if t < FIRST_CLOUD {
        return out;
    }
    // banks: each lives long enough to cross; look back far enough to find
    // every one still on the map
    let e_now = (t / BANK_EPOCH).floor() as i64;
    for e in (e_now - 8).max(0)..=e_now {
        let mut rng = Rng::new(hash(seed, e as u64 * 2 + 1));
        if rng.f32() > BANK_CHANCE {
            continue;
        }
        let born = e as f32 * BANK_EPOCH + rng.f32() * BANK_EPOCH * 0.5;
        if born < FIRST_CLOUD {
            continue;
        }
        let a = rng.f32() * std::f32::consts::TAU;
        let start = Vec3::new(a.cos() * RIM, 0.0, a.sin() * RIM);
        // across the map, not straight through the middle every time
        let aim = Vec3::new(rng.range(-150.0, 150.0), 0.0, rng.range(-150.0, 150.0));
        let dir = (aim - start).normalize_or_zero();
        let speed = rng.range(2.2, 3.6);
        let r = rng.range(35.0, 65.0);
        let life = (2.0 * RIM) / speed;
        let age = t - born;
        if age < 0.0 || age > life {
            continue;
        }
        let thick = smooth(age / FADE).min(smooth((life - age) / FADE));
        out.push(Cloud { kind: Kind::Bank, pos: start + dir * speed * age, dir: dir * speed, r, w: r, thick, seed: rng.next_u() as u32, age });
    }
    // wisps: short-lived streams blowing through anywhere over the islands
    let e_now = (t / WISP_EPOCH).floor() as i64;
    for e in (e_now - 10).max(0)..=e_now {
        let mut rng = Rng::new(hash(seed ^ 0x5157, e as u64 * 2 + 1));
        if rng.f32() > WISP_CHANCE {
            continue;
        }
        let born = e as f32 * WISP_EPOCH + rng.f32() * WISP_EPOCH;
        if born < FIRST_CLOUD {
            continue;
        }
        let life = rng.range(40.0, 75.0);
        let age = t - born;
        if age < 0.0 || age > life {
            continue;
        }
        let at = Vec3::new(rng.range(-260.0, 260.0), 0.0, rng.range(-240.0, 240.0));
        let a = rng.f32() * std::f32::consts::TAU;
        let dir = Vec3::new(a.cos(), 0.0, a.sin());
        let speed = rng.range(4.5, 7.0);
        // it pulses: thick for a few seconds, thin for a few
        let (period, phase) = (rng.range(6.0, 11.0), rng.f32() * std::f32::consts::TAU);
        let pulse = 0.5 + 0.5 * (age / period * std::f32::consts::TAU + phase).sin();
        let fade = smooth(age / 6.0).min(smooth((life - age) / 6.0));
        let thick = fade * smooth((pulse - 0.25) / 0.5);
        let start = at - dir * speed * life * 0.5;
        out.push(Cloud {
            kind: Kind::Wisp,
            pos: start + dir * speed * age,
            dir: dir * speed,
            r: rng.range(22.0, 42.0),
            w: rng.range(4.0, 8.0),
            thick,
            seed: rng.next_u() as u32,
            age,
        });
    }
    out
}

impl Cloud {
    /// Density of this cloud at (x, z), 0..=1.
    pub fn density(&self, x: f32, z: f32) -> f32 {
        if self.thick <= 0.0 {
            return 0.0;
        }
        let d = Vec3::new(x - self.pos.x, 0.0, z - self.pos.z);
        match self.kind {
            Kind::Bank => {
                // a ragged, slowly changing edge
                let a = d.z.atan2(d.x);
                let s = self.seed as f32 * 0.001;
                let edge = self.r * (1.0 + 0.16 * (3.0 * a + s + self.age * 0.03).sin() + 0.08 * (7.0 * a - s * 2.0 - self.age * 0.05).sin());
                let k = 1.0 - smooth((d.length() - edge * 0.6) / (edge * 0.4));
                k * self.thick
            }
            Kind::Wisp => {
                let along = self.dir.normalize_or_zero();
                let (u, v) = (d.dot(along), d.dot(Vec3::new(-along.z, 0.0, along.x)));
                // a stream, fraying at its ends, a little wavy
                let v = v - (u * 0.08 + self.seed as f32 * 0.01).sin() * self.w * 0.6;
                let lu = 1.0 - smooth((u.abs() - self.r * 0.6) / (self.r * 0.4));
                let lv = 1.0 - smooth((v.abs() - self.w * 0.4) / (self.w * 0.6));
                lu * lv * self.thick
            }
        }
    }
}

/// All the cloud at (x, z), 0..=1.
pub fn density(cs: &[Cloud], x: f32, z: f32) -> f32 {
    cs.iter().map(|c| c.density(x, z)).sum::<f32>().min(1.0)
}

/// Can someone at `a` see someone at `b` through the clouds `cs`? Within
/// [`CLOSE`], always; beyond [`SIGHT_RANGE`], never; between, unless the
/// cloud along the line adds up to [`SIGHT_DEPTH`].
pub fn sees(cs: &[Cloud], a: Vec3, b: Vec3) -> bool {
    let d = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
    let len = d.length();
    if len > SIGHT_RANGE {
        return false;
    }
    // only the clouds near the line
    let near: Vec<&Cloud> = cs.iter().filter(|c| c.thick > 0.0 && seg_dist(a, b, c.pos) < c.r * 1.2 + 10.0).collect();
    if near.is_empty() {
        return true;
    }
    let step = 1.0;
    let n = (len / step).ceil() as usize;
    let mut fog = 0.0;
    for k in 0..n {
        let p = a + d * ((k as f32 + 0.5) / n as f32);
        fog += near.iter().map(|c| c.density(p.x, p.z)).sum::<f32>().min(1.0) * (len / n as f32);
        if fog >= SIGHT_DEPTH {
            return false;
        }
    }
    true
}

fn seg_dist(a: Vec3, b: Vec3, p: Vec3) -> f32 {
    let ab = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
    let ap = Vec3::new(p.x - a.x, 0.0, p.z - a.z);
    let t = (ap.dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    (ap - ab * t).length()
}

/// How a cloud is drawn: billows (`skyprops::cloud_puff`, 11 u across at
/// scale 1) as (offset from the cloud's centre, scale x/y/z, how opaque
/// relative to the cloud's thickness). Laid from the cloud's own seed, so a
/// bank keeps its shape as it drifts; the billows breathe with its age.
pub fn puffs(c: &Cloud) -> Vec<(Vec3, Vec3, f32)> {
    let mut rng = Rng::new(c.seed as u64 * 2 + 1);
    let mut out = Vec::new();
    match c.kind {
        Kind::Bank => {
            let n = ((c.r / 10.0).powi(2) * 0.9) as usize + 6;
            for k in 0..n {
                // a sunflower spiral, jittered, so the disc is evenly filled
                let f = (k as f32 + 0.5) / n as f32;
                let a = k as f32 * 2.399 + rng.f32() * 0.6;
                let rr = c.r * 0.85 * f.sqrt();
                let off = Vec3::new(a.cos() * rr, 2.0 + rng.f32() * 3.0, a.sin() * rr);
                let s = rng.range(2.0, 3.0) * (1.0 + 0.1 * (c.age * 0.07 + k as f32).sin());
                let edge = 1.0 - smooth((f.sqrt() - 0.7) / 0.3) * 0.6;
                out.push((off, Vec3::new(s, s * 0.8, s), edge));
                // a second, higher layer over the middle: a bank is a hill of
                // cloud, not a lid on the floor
                if f < 0.6 && k % 2 == 0 {
                    let up = off + Vec3::new(rng.range(-4.0, 4.0), 5.0 + rng.f32() * 3.0, rng.range(-4.0, 4.0));
                    out.push((up, Vec3::new(s * 0.9, s * 0.7, s * 0.9), edge * 0.9));
                }
            }
        }
        Kind::Wisp => {
            let along = c.dir.normalize_or_zero();
            let side = Vec3::new(-along.z, 0.0, along.x);
            let n = (c.r * 2.0 / 7.0) as usize + 2;
            for k in 0..n {
                let u = (k as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                let wave = (u * c.r * 0.08 + c.seed as f32 * 0.01).sin() * c.w * 0.6;
                let off = along * (u * c.r) + side * wave + Vec3::Y * (1.0 + rng.f32() * 2.0);
                let s = rng.range(0.7, 1.0) * c.w / 6.0;
                out.push((off, Vec3::new(s * 1.3, s * 0.5, s * 0.8), 1.0 - smooth((u.abs() - 0.6) / 0.4)));
            }
        }
    }
    out
}

/// Who each team can see, refreshed a few times a second (`Game::vision_tick`).
#[derive(Debug, Clone, Default)]
pub struct Vision {
    /// `seen[team][god]`: can that team see that cat. Empty = everyone seen.
    pub seen: [Vec<bool>; 2],
    pub next_t: f32,
    /// The clouds at the last refresh (for the scene and the bots).
    pub clouds: Vec<Cloud>,
}

/// Refresh rate of `Vision`, Hz.
pub const VISION_HZ: f32 = 8.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn over(seed: u64, from: f32, to: f32, every: f32) -> Vec<(f32, Vec<Cloud>)> {
        let mut v = Vec::new();
        let mut t = from;
        while t < to {
            v.push((t, clouds(seed, t)));
            t += every;
        }
        v
    }

    #[test]
    fn clear_at_the_start_then_banks_and_wisps_come_and_go() {
        assert!(clouds(7, 30.0).is_empty(), "the opening is clear");
        let run = over(7, 60.0, 1800.0, 5.0);
        let banks: Vec<usize> = run.iter().map(|(_, c)| c.iter().filter(|c| c.kind == Kind::Bank && c.thick > 0.5).count()).collect();
        let wisps: Vec<usize> = run.iter().map(|(_, c)| c.iter().filter(|c| c.kind == Kind::Wisp).count()).collect();
        let mean_b = banks.iter().sum::<usize>() as f32 / banks.len() as f32;
        assert!((1.5..6.0).contains(&mean_b), "a few banks over the map at a time: {mean_b}");
        assert!(banks.iter().any(|&b| b == 0) || banks.iter().min().unwrap() <= &1, "and sometimes clear-ish");
        assert!(wisps.iter().sum::<usize>() > run.len() * 3, "wisps blowing through");
        // the same seed, the same sky; another seed, another sky
        assert_eq!(clouds(7, 900.0), clouds(7, 900.0));
        assert_ne!(clouds(7, 900.0), clouds(8, 900.0));
    }

    #[test]
    fn clouds_move_smoothly() {
        for (t, cs) in over(3, 200.0, 800.0, 7.0) {
            let later = clouds(3, t + 0.1);
            for c in &cs {
                if let Some(d) = later.iter().find(|d| d.seed == c.seed) {
                    assert!((d.pos - c.pos).length() < 1.0, "no jumps");
                    assert!((d.thick - c.thick).abs() < 0.2, "no pops (a wisp pulses over a second or so)");
                }
            }
        }
    }

    #[test]
    fn a_bank_breaks_sight_and_near_is_always_seen() {
        let bank = Cloud { kind: Kind::Bank, pos: Vec3::ZERO, dir: Vec3::X, r: 40.0, w: 40.0, thick: 1.0, seed: 1, age: 100.0 };
        let cs = [bank];
        // from outside into the middle, and right through it
        assert!(!sees(&cs, Vec3::new(-44.0, 0.0, 0.0), Vec3::ZERO), "cannot see into a bank");
        assert!(!sees(&cs, Vec3::new(-22.0, 0.0, 0.0), Vec3::new(22.0, 0.0, 0.0)), "or across one");
        assert!(CLOSE < SIGHT_DEPTH);
        assert!(sees(&cs, Vec3::new(-CLOSE * 0.5, 0.0, 0.0), Vec3::new(CLOSE * 0.5, 0.0, 0.0)), "arm's length, always");
        assert!(!sees(&cs, Vec3::new(-5.0, 0.0, 0.0), Vec3::new(5.0, 0.0, 0.0)), "but not across the room");
        assert!(sees(&cs, Vec3::new(-60.0, 0.0, 50.0), Vec3::new(-20.0, 0.0, 60.0)), "clear air beside it");
        assert!(!sees(&[], Vec3::ZERO, Vec3::new(60.0, 0.0, 0.0)), "nothing past sight range");
        // a thinning bank lets sight back through
        let thin = [Cloud { thick: 0.05, ..bank }];
        assert!(sees(&thin, Vec3::new(-20.0, 0.0, 0.0), Vec3::new(20.0, 0.0, 0.0)));
    }

    #[test]
    fn a_wisp_cuts_a_line_only_while_it_is_thick() {
        let w = Cloud { kind: Kind::Wisp, pos: Vec3::ZERO, dir: Vec3::X * 5.0, r: 30.0, w: 7.0, thick: 1.0, seed: 0, age: 10.0 };
        // a line across the stream
        let (a, b) = (Vec3::new(0.0, 0.0, -20.0), Vec3::new(0.0, 0.0, 20.0));
        assert!(!sees(&[w], a, b), "thick: cut");
        assert!(sees(&[Cloud { thick: 0.2, ..w }], a, b), "thin: seen");
        // along the real schedule, some line somewhere flickers
        let mut flips = 0;
        let mut last = None;
        for (_, cs) in over(11, 300.0, 700.0, 0.5) {
            if let Some(c) = cs.iter().find(|c| c.kind == Kind::Wisp && c.age > 8.0) {
                let side = Vec3::new(-c.dir.z, 0.0, c.dir.x).normalize_or_zero() * 15.0;
                let now = sees(&cs, c.pos - side, c.pos + side);
                if last.is_some_and(|l| l != now) {
                    flips += 1;
                }
                last = Some(now);
            }
        }
        assert!(flips >= 3, "sight across wisps comes and goes: {flips} changes");
    }
}
