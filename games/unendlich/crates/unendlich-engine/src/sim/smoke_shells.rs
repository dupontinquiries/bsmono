//! `sim::smoke_shells` — the cups-and-ball smoke game.
//!
//! Rather than blacking out the screen (which kills immersion), a smoking
//! dragon raises several **identical, dense drafts** of smoke and hides inside
//! one. The drafts circle the player on weaving paths at different speeds, so
//! they **cross** — and where two overlap, a cunning dragon may slip from one
//! into the other, unseen, like the ball passing between cups. The player's job
//! is to keep track. Around them the air stays clear; only the drafts are
//! thick, and thick enough to swallow a dragon whole.
//!
//! Honesty rules, each tested:
//! * every draft is the same size, density and look (no visual giveaway);
//! * the dragon changes draft **only** while two drafts overlap;
//! * the real draft is not systematically the nearest, farthest or slowest;
//! * the one moment it reveals itself is its strike: its draft darts in, and
//!   `strike_in` lets the shell show its eyes just before (`smoke_tells`).
//!
//! Pure and seeded; positions in, positions out.

use crate::sim::smoke_wall::SmokeVolume;
use std::f32::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Draft {
    pub pos: [f32; 3],
    /// Orbit parameters: angle, angular speed (rad/s), base radius, weave.
    pub angle: f32,
    pub omega: f32,
    pub orbit_r: f32,
    pub weave_amp: f32,
    pub weave_hz: f32,
    pub phase: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Strike {
    /// Circling.
    Idle { next_in: f32 },
    /// Darting in at the player; lands when `t` reaches `dur`.
    Dart { t: f32, dur: f32 },
    /// Pulling back out to its orbit.
    Recover { t: f32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shells {
    pub drafts: Vec<Draft>,
    /// Index of the draft the dragon is in.
    pub hidden: usize,
    /// Draft radius and height (m): big enough to swallow this dragon.
    pub radius: f32,
    pub height: f32,
    /// The dragon's own body radius: a strike closes to within reach of it,
    /// so a strike is also your opening to hit back.
    pub body_radius: f32,
    pub left: f32,
    pub strike: Strike,
    cunning: f32,
    seed: u64,
    n: u64,
    /// Pairs currently overlapping (so a switch is counted once per crossing).
    overlapping: Vec<(usize, usize)>,
}

/// What happened this tick.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShellTick {
    /// Pairs that began to overlap this tick.
    pub crossings: Vec<(usize, usize)>,
    /// The dragon moved from one draft to another (only ever at a crossing).
    pub switched: Option<(usize, usize)>,
    /// Its draft crossed another while circling: a chance to switch.
    pub offered: bool,
    /// Seconds until the strike lands, while darting (for the eyes tell).
    pub strike_in: Option<f32>,
    /// The strike landed this tick (the shell applies the bite).
    pub struck: bool,
}

fn h01(seed: u64, i: u64) -> f32 {
    let mut z = seed ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
}

/// How many drafts a dragon raises: 3, up to 5 for the most cunning.
pub fn draft_count(cunning: f32) -> usize {
    3 + (cunning.clamp(0.0, 1.0) * 2.0).round() as usize
}

impl Shells {
    /// Raise the drafts around `center` (the player's ground position).
    /// `body_radius`, `body_height`: the dragon's, so a draft can hide it.
    pub fn new(center: [f32; 3], body_radius: f32, body_height: f32, cunning: f32, duration: f32, seed: u64) -> Shells {
        let cunning = cunning.clamp(0.0, 1.0);
        let n = draft_count(cunning);
        let radius = (body_radius * 1.6).clamp(4.0, 12.0);
        let height = (body_height * 1.8 + 4.0).clamp(8.0, 30.0);
        let drafts = (0..n)
            .map(|i| {
                let j = |k: u64| h01(seed, (i as u64) * 16 + k);
                let angle = TAU * i as f32 / n as f32 + j(1) * 0.4;
                // Alternate directions and speeds: paths cross.
                let dir = if i % 2 == 0 { 1.0 } else { -1.0 };
                let omega = dir * (0.18 + 0.12 * j(2));
                let orbit_r = 14.0 + radius + 4.0 * j(3);
                let mut d = Draft {
                    pos: [0.0; 3],
                    angle,
                    omega,
                    orbit_r,
                    weave_amp: 3.0 + 3.0 * j(4),
                    weave_hz: 0.08 + 0.08 * j(5),
                    phase: j(6) * TAU,
                };
                d.pos = orbit_pos(&d, center, 0.0);
                d
            })
            .collect();
        let hidden = (h01(seed, 999) * n as f32) as usize % n;
        Shells {
            drafts,
            hidden,
            radius,
            height,
            body_radius: body_radius.max(0.5),
            left: duration.max(0.0),
            strike: Strike::Idle { next_in: 6.0 + 4.0 * h01(seed, 998) },
            cunning,
            seed,
            n: 0,
            overlapping: Vec::new(),
        }
    }

    pub fn active(&self) -> bool {
        self.left > 0.0
    }

    /// Where the dragon is (its draft's centre).
    pub fn dragon_pos(&self) -> [f32; 3] {
        self.drafts[self.hidden].pos
    }

    /// Advance. `center`: the player's ground position (drafts orbit it).
    pub fn step(&mut self, center: [f32; 3], dt: f32) -> ShellTick {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.5) } else { 0.0 };
        let mut out = ShellTick::default();
        if !self.active() || !center.iter().all(|v| v.is_finite()) {
            return out;
        }
        self.left = (self.left - dt).max(0.0);
        self.n += 1;
        let t_now = self.n as f32 * dt; // only used for weave phase continuity
        for (i, d) in self.drafts.iter_mut().enumerate() {
            d.angle += d.omega * dt;
            let orbit = orbit_pos(d, center, t_now);
            let darting = i == self.hidden && !matches!(self.strike, Strike::Idle { .. });
            if darting {
                // The striking draft leaves its orbit toward the player.
                let k = match self.strike {
                    // Arrives at 80% of the dart and holds: the blow lands
                    // from full reach, not from where it was a tick earlier.
                    Strike::Dart { t, dur } => (t / (dur * 0.8)).clamp(0.0, 1.0),
                    Strike::Recover { t } => (1.0 - t / 1.5).clamp(0.0, 1.0),
                    Strike::Idle { .. } => 0.0,
                };
                // Close until its body's edge is ~0.8 m from you: in its bite
                // reach, and in yours. (Stopping at the DRAFT's edge left it
                // 6 m out -- it could bite from there and nothing could hit it.)
                // 0.8 m, not more: a size-3 dragon is ~19 m long (body radius
                // 9.5), and at 1.5 m past it a bare hand (1.2 m) could not
                // reach back -- a fight-driver soak swung at air for minutes.
                let reach = 1.0 - (self.body_radius + 0.8) / (dist2(orbit, center).sqrt().max(1.0));
                let e = k * reach.clamp(0.0, 1.0);
                for a in 0..3 {
                    d.pos[a] = orbit[a] + (center[a] - orbit[a]) * e;
                }
                d.pos[1] = orbit[1];
            } else {
                d.pos = orbit;
            }
        }
        // Crossings: pairs whose drafts now overlap and did not last tick.
        let mut now_over = Vec::new();
        for a in 0..self.drafts.len() {
            for b in a + 1..self.drafts.len() {
                if dist2(self.drafts[a].pos, self.drafts[b].pos) < (self.radius * 1.2).powi(2) {
                    now_over.push((a, b));
                    if !self.overlapping.contains(&(a, b)) {
                        out.crossings.push((a, b));
                        // At a crossing that involves its draft, it may slip
                        // across: likelier the more cunning. Never while striking.
                        let involved = a == self.hidden || b == self.hidden;
                        if involved && matches!(self.strike, Strike::Idle { .. }) {
                            out.offered = true;
                            let roll = h01(self.seed ^ 0xC055, self.n ^ ((a as u64) << 20) ^ ((b as u64) << 30));
                            if roll < 0.25 + 0.6 * self.cunning {
                                let to = if a == self.hidden { b } else { a };
                                out.switched = Some((self.hidden, to));
                                self.hidden = to;
                            }
                        }
                    }
                }
            }
        }
        self.overlapping = now_over;
        // The strike cycle.
        self.strike = match self.strike {
            Strike::Idle { next_in } => {
                let n = next_in - dt;
                if n <= 0.0 { Strike::Dart { t: 0.0, dur: 1.6 } } else { Strike::Idle { next_in: n } }
            }
            Strike::Dart { t, dur } => {
                let t = t + dt;
                out.strike_in = Some((dur - t).max(0.0));
                if t >= dur {
                    out.struck = true;
                    Strike::Recover { t: 0.0 }
                } else {
                    Strike::Dart { t, dur }
                }
            }
            Strike::Recover { t } => {
                let t = t + dt;
                if t >= 1.5 {
                    Strike::Idle { next_in: 6.0 + 5.0 * h01(self.seed, self.n) - 2.0 * self.cunning }
                } else {
                    Strike::Recover { t }
                }
            }
        };
        out
    }

    /// The drafts as smoke volumes (for sight: `smoke_wall::transmittance`,
    /// and for the wisps). All identical; dense enough to hide what is inside.
    pub fn volumes(&self, owner: u64) -> Vec<SmokeVolume> {
        self.drafts
            .iter()
            .map(|d| SmokeVolume {
                center: [d.pos[0], d.pos[1] + self.height * 0.5, d.pos[2]],
                along: [1.0, 0.0],
                half: [self.radius, self.radius, self.height * 0.5],
                density: 2.2,
                age: 0.0,
                life: f32::INFINITY,
                across0: self.radius,
                owner,
            })
            .collect()
    }
}

fn orbit_pos(d: &Draft, center: [f32; 3], t: f32) -> [f32; 3] {
    let r = d.orbit_r + d.weave_amp * (TAU * d.weave_hz * t + d.phase).sin();
    [center[0] + r * d.angle.cos(), center[1], center[2] + r * d.angle.sin()]
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::smoke_wall::{transmittance, SIGHT_FLOOR};

    fn run(cunning: f32, secs: f32, seed: u64) -> (Shells, Vec<ShellTick>) {
        let mut s = Shells::new([0.0, 64.0, 0.0], 4.0, 5.0, cunning, 1e9, seed);
        let ticks = (0..(secs / 0.1) as usize).map(|_| s.step([0.0, 64.0, 0.0], 0.1)).collect();
        (s, ticks)
    }

    #[test]
    fn more_cunning_more_cups() {
        assert_eq!(draft_count(0.0), 3);
        assert_eq!(draft_count(1.0), 5);
    }

    #[test]
    fn every_draft_is_identical_and_thick_enough_to_hide_a_dragon() {
        let s = Shells::new([0.0, 64.0, 0.0], 4.0, 5.0, 0.5, 30.0, 1);
        let v = s.volumes(7);
        assert!(v.windows(2).all(|w| w[0].half == w[1].half && w[0].density == w[1].density));
        // Looking at the dragon inside its draft from 30 m outside: hidden.
        let d = s.dragon_pos();
        let eye = [d[0] + 30.0, 65.6, d[2]];
        let t = transmittance(&v, eye, [d[0], d[1] + 2.5, d[2]], 1.0);
        assert!(t < SIGHT_FLOOR * 0.2, "t {t}");
        // …while the air between drafts stays clear.
        let clear = transmittance(&v, [0.0, 65.6, 0.0], [0.0, 65.6, 0.1], 1.0);
        assert!(clear > 0.99);
    }

    #[test]
    fn the_drafts_cross_and_the_dragon_only_switches_at_a_crossing() {
        let (_, ticks) = run(0.9, 120.0, 3);
        let crossings: usize = ticks.iter().map(|t| t.crossings.len()).sum();
        assert!(crossings >= 4, "only {crossings} crossings in 2 minutes");
        let switches: Vec<_> = ticks.iter().filter(|t| t.switched.is_some()).collect();
        assert!(!switches.is_empty(), "a cunning dragon never slipped across");
        for t in &switches {
            let (from, to) = t.switched.unwrap();
            assert!(
                t.crossings.contains(&(from.min(to), from.max(to))),
                "switched {from}->{to} without those two crossing"
            );
        }
    }

    #[test]
    fn a_dull_dragon_switches_less_often_when_offered() {
        // Per chance, not per minute: more cunning also means more drafts, so
        // raw switch counts would conflate the two.
        let rate = |c: f32| {
            let (mut sw, mut off) = (0usize, 0usize);
            for seed in 0..12u64 {
                for t in run(c, 120.0, seed).1 {
                    sw += t.switched.is_some() as usize;
                    off += t.offered as usize;
                }
            }
            sw as f32 / off.max(1) as f32
        };
        let (dull, sharp) = (rate(0.0), rate(1.0));
        assert!(sharp > dull + 0.3, "sharp {sharp:.2} dull {dull:.2}");
    }

    #[test]
    fn the_real_draft_is_not_always_the_nearest() {
        // Over many seeds and times, how often is the dragon's draft the one
        // nearest the player? About 1/n, not always.
        let (mut nearest, mut total) = (0, 0);
        for seed in 0..20u64 {
            let (_, _) = run(0.5, 0.0, seed);
            let mut s = Shells::new([0.0, 64.0, 0.0], 4.0, 5.0, 0.5, 1e9, seed);
            for k in 0..300 {
                s.step([0.0, 64.0, 0.0], 0.1);
                if k % 10 != 0 || !matches!(s.strike, Strike::Idle { .. }) {
                    continue;
                }
                let d: Vec<f32> = s.drafts.iter().map(|d| dist2(d.pos, [0.0, 64.0, 0.0])).collect();
                let near = (0..d.len()).min_by(|&a, &b| d[a].partial_cmp(&d[b]).unwrap()).unwrap();
                nearest += (near == s.hidden) as usize;
                total += 1;
            }
        }
        let share = nearest as f32 / total as f32;
        assert!(share < 0.6, "the dragon's draft was the nearest {:.0}% of the time", share * 100.0);
    }

    #[test]
    fn its_strike_darts_in_and_warns_first() {
        let mut s = Shells::new([0.0, 64.0, 0.0], 4.0, 5.0, 0.5, 1e9, 9);
        let mut warned = false;
        let mut closest = f32::MAX;
        for _ in 0..200 {
            let t = s.step([0.0, 64.0, 0.0], 0.1);
            if let Some(w) = t.strike_in {
                warned = true;
                assert!(w >= 0.0);
            }
            closest = closest.min(dist2(s.dragon_pos(), [0.0, 64.0, 0.0]).sqrt());
            if t.struck {
                assert!(warned, "struck without a warning window");
                assert!(closest < s.body_radius + 2.0, "struck from {closest:.1} m (body {})", s.body_radius);
                return;
            }
        }
        panic!("never struck in 20 s");
    }

    #[test]
    fn it_ends_and_is_deterministic() {
        let mut s = Shells::new([0.0, 64.0, 0.0], 4.0, 5.0, 0.5, 1.0, 4);
        for _ in 0..20 {
            s.step([0.0, 64.0, 0.0], 0.1);
        }
        assert!(!s.active());
        assert_eq!(run(0.7, 30.0, 5).0, run(0.7, 30.0, 5).0);
    }
}
