//! trials — a reason to visit all thirty-six maps.
//!
//! The game had a sandbox and no goals: you could fly a marble run, drive a
//! chess board or ride a wave, and nothing ever asked you to. A TRIAL names
//! one thing to do on one map, times you doing it, and remembers your best.
//!
//! The design rule here is that every trial is scored from state the physics
//! already produces — air time, touches, height, speed, goals — so nothing
//! in this file can disagree with the simulation. And each one is chosen to
//! teach the mechanic its map was built around: PERPLEXUS asks you to stay
//! in the tubes, CHAMBER to ride a moving piece, APERTURE to fling.
//!
//! A run also records a GHOST: your car's path at 20 Hz. Beat a trial and
//! the ghost is kept, so the next attempt races the last best.

use glam::{Quat, Vec3};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Goal {
    /// score N goals
    Goals(u32),
    /// keep the ball airborne and touched for N seconds (air dribble)
    AirDribble(f32),
    /// reach N metres above the map's ground
    Height(f32),
    /// hold N m/s for a moment
    Speed(f32),
    /// N touches without the ball hitting the ground
    Combo(u32),
    /// spend N seconds inside tube routes
    InTube(f32),
    /// N seconds riding a moving platform
    OnPlatform(f32),
    /// travel N metres through portals in one run
    PortalMetres(f32),
}

pub struct Trial {
    pub name: &'static str,
    pub map: usize,
    pub goal: Goal,
    /// seconds allowed; the score is how fast you finish
    pub limit: f32,
    pub blurb: &'static str,
}

/// The tour. Ordered roughly by difficulty, and deliberately spread across
/// the map list so finishing it means seeing everything the game has.
pub const TRIALS: &[Trial] = &[
    Trial { name: "FIRST TOUCH", map: 20, goal: Goal::Goals(1), limit: 60.0, blurb: "Put one away." },
    Trial { name: "TERMINAL VELOCITY", map: 20, goal: Goal::Speed(34.0), limit: 45.0, blurb: "Boost until the world blurs." },
    Trial { name: "KEEPY UPPY", map: 25, goal: Goal::Combo(6), limit: 60.0, blurb: "Six touches, no bounce." },
    Trial { name: "THE LONG CARRY", map: 25, goal: Goal::AirDribble(3.0), limit: 90.0, blurb: "Three seconds of air dribble." },
    Trial { name: "CEILING", map: 25, goal: Goal::Height(30.0), limit: 45.0, blurb: "Thirty metres up." },
    Trial { name: "THROUGH THE WORKS", map: 21, goal: Goal::InTube(8.0), limit: 120.0, blurb: "Eight seconds inside the pipes." },
    Trial { name: "CANOPY RUN", map: 24, goal: Goal::InTube(10.0), limit: 150.0, blurb: "Ride the vine lines." },
    Trial { name: "THE FLING", map: 31, goal: Goal::PortalMetres(120.0), limit: 90.0, blurb: "A hundred and twenty metres of portal travel." },
    Trial { name: "READ THE MACHINE", map: 32, goal: Goal::PortalMetres(90.0), limit: 120.0, blurb: "Same, but the mouths are moving." },
    Trial { name: "PASSENGER", map: 30, goal: Goal::OnPlatform(12.0), limit: 120.0, blurb: "Twelve seconds carried by the machine." },
    Trial { name: "KNIGHT TAKES ALL", map: 35, goal: Goal::OnPlatform(8.0), limit: 120.0, blurb: "Ride a piece across the board." },
    Trial { name: "DROP SHOT", map: 26, goal: Goal::Goals(3), limit: 120.0, blurb: "Three through the floor." },
    Trial { name: "SURF", map: 34, goal: Goal::OnPlatform(10.0), limit: 120.0, blurb: "Ten seconds on the swell." },
    Trial { name: "THE WYRM", map: 33, goal: Goal::Height(45.0), limit: 90.0, blurb: "Forty-five metres over the caldera." },
    Trial { name: "GRAND TOUR", map: 21, goal: Goal::Combo(10), limit: 180.0, blurb: "Ten touches, anywhere, no bounce." },
];

/// Live state for the trial being attempted.
pub struct Run {
    pub idx: usize,
    pub t: f32,
    pub done: bool,
    pub failed: bool,
    // progress accumulators
    pub goals: u32,
    pub air: f32,
    pub height: f32,
    pub speed: f32,
    pub combo: u32,
    pub tube_t: f32,
    pub plat_t: f32,
    pub portal_m: f32,
    /// 20 Hz path of this attempt
    pub ghost: Vec<(Vec3, Quat)>,
    ghost_acc: f32,
}

impl Run {
    pub fn new(idx: usize) -> Run {
        Run {
            idx,
            t: 0.0,
            done: false,
            failed: false,
            goals: 0,
            air: 0.0,
            height: 0.0,
            speed: 0.0,
            combo: 0,
            tube_t: 0.0,
            plat_t: 0.0,
            portal_m: 0.0,
            ghost: Vec::with_capacity(4096),
            ghost_acc: 0.0,
        }
    }

    pub fn trial(&self) -> &'static Trial {
        &TRIALS[self.idx.min(TRIALS.len() - 1)]
    }

    /// How far along, 0..1 — drives the HUD bar.
    pub fn progress(&self) -> f32 {
        let g = self.trial().goal;
        let f = match g {
            Goal::Goals(n) => self.goals as f32 / n as f32,
            Goal::AirDribble(s) => self.air / s,
            Goal::Height(m) => self.height / m,
            Goal::Speed(v) => self.speed / v,
            Goal::Combo(n) => self.combo as f32 / n as f32,
            Goal::InTube(s) => self.tube_t / s,
            Goal::OnPlatform(s) => self.plat_t / s,
            Goal::PortalMetres(m) => self.portal_m / m,
        };
        f.clamp(0.0, 1.0)
    }

    /// Feed one frame of world state. Returns true on the frame it completes.
    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        dt: f32,
        car_pos: Vec3,
        car_rot: Quat,
        car_speed: f32,
        ground_y: f32,
        goals_scored: u32,
        air_dribble_t: f32,
        combo: u32,
        in_tube: bool,
        on_platform: bool,
        portal_step: f32,
    ) -> bool {
        if self.done || self.failed {
            return false;
        }
        self.t += dt;
        self.goals = goals_scored;
        self.air = self.air.max(air_dribble_t);
        self.height = self.height.max(car_pos.y - ground_y);
        self.speed = self.speed.max(car_speed);
        self.combo = self.combo.max(combo);
        if in_tube {
            self.tube_t += dt;
        }
        if on_platform {
            self.plat_t += dt;
        }
        self.portal_m += portal_step;
        // the ghost: 20 Hz is plenty to race against and cheap to keep
        self.ghost_acc += dt;
        if self.ghost_acc >= 0.05 {
            self.ghost_acc = 0.0;
            self.ghost.push((car_pos, car_rot));
        }
        if self.progress() >= 1.0 {
            self.done = true;
            return true;
        }
        if self.t > self.trial().limit {
            self.failed = true;
        }
        false
    }
}

/// Where the ghost was at time `t` — linear between 20 Hz samples.
pub fn ghost_at(ghost: &[(Vec3, Quat)], t: f32) -> Option<(Vec3, Quat)> {
    if ghost.len() < 2 {
        return None;
    }
    let f = (t / 0.05).max(0.0);
    let i = f.floor() as usize;
    if i + 1 >= ghost.len() {
        return ghost.last().copied();
    }
    let a = ghost[i];
    let b = ghost[i + 1];
    let k = f - i as f32;
    Some((a.0.lerp(b.0, k), a.1.slerp(b.1, k)))
}

/// Best times, one slot per trial (0 = never finished).
#[derive(Clone)]
pub struct Bests(pub Vec<f32>);

impl Default for Bests {
    fn default() -> Bests {
        Bests(vec![0.0; TRIALS.len()])
    }
}

impl Bests {
    pub fn get(&self, i: usize) -> f32 {
        self.0.get(i).copied().unwrap_or(0.0)
    }
    /// Returns true if this is a new record.
    pub fn record(&mut self, i: usize, t: f32) -> bool {
        while self.0.len() < TRIALS.len() {
            self.0.push(0.0);
        }
        let cur = self.0[i];
        if cur <= 0.0 || t < cur {
            self.0[i] = t;
            true
        } else {
            false
        }
    }
    pub fn finished(&self) -> usize {
        self.0.iter().filter(|&&t| t > 0.0).count()
    }
    /// Serialised for the save file: space-separated seconds.
    pub fn encode(&self) -> String {
        self.0.iter().map(|t| format!("{t:.2}")).collect::<Vec<_>>().join(" ")
    }
    pub fn decode(s: &str) -> Bests {
        let mut v: Vec<f32> = s.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        while v.len() < TRIALS.len() {
            v.push(0.0);
        }
        Bests(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 120.0;

    // Ported from blackstar-rs's `--sim trials` (src/sim.rs) -- real
    // challenge-mode logic (goal detection, the clock, personal-best
    // tracking, ghost replay) with zero automated coverage before this,
    // same shape as the academy.rs and net.rs conversions.

    #[test]
    fn there_are_enough_trials_spanning_enough_maps() {
        assert!(TRIALS.len() >= 12, "only {} trials", TRIALS.len());
        let maps: std::collections::HashSet<usize> = TRIALS.iter().map(|t| t.map).collect();
        assert!(maps.len() >= 8, "trials only span {} distinct maps", maps.len());
        // Same crate-boundary note as academy.rs's lesson-map test: `map` is
        // a world::style() index in blackstar-rs, which this crate doesn't
        // depend on, so this can only catch the obviously-unset case, not
        // the real STYLE_COUNT bound -- that check has to stay in
        // blackstar-rs's own suite.
        for t in TRIALS {
            assert!(t.map < 200, "{}'s map index {} looks unset", t.name, t.map);
        }
    }

    #[test]
    fn every_trial_gives_enough_time_to_actually_win() {
        assert!(TRIALS.iter().all(|t| t.limit >= 30.0), "a trial's limit is under 30s");
    }

    #[test]
    fn every_goal_type_is_actually_reachable_within_its_limit() {
        // drive a trial to completion with plausible inputs -- if a goal
        // type can never trigger `done`, a player would hit a trial that
        // literally cannot be won, which no amount of skill fixes.
        let hit = |g: Goal| -> bool {
            let idx = TRIALS.iter().position(|t| t.goal == g).unwrap_or(0);
            let mut r = Run::new(idx);
            let mut t = 0.0f32;
            let mut done = false;
            while t < TRIALS[idx].limit && !done {
                let p = Vec3::new(t * 3.0, 90.0, 0.0); // high enough to satisfy height goals
                done = r.tick(
                    DT, p, Quat::IDENTITY, 40.0, 0.0,
                    (t * 0.6) as u32,
                    t.min(9.0),
                    (t * 2.0) as u32,
                    true, true,
                    0.6,
                );
                t += DT;
            }
            done
        };
        for t in TRIALS {
            assert!(hit(t.goal), "{} ({:?}) never completes with plausible inputs", t.name, t.goal);
        }
    }

    #[test]
    fn running_out_of_time_fails_the_run() {
        let mut r = Run::new(0);
        let mut t = 0.0;
        while t < TRIALS[0].limit + 1.0 {
            r.tick(DT, Vec3::ZERO, Quat::IDENTITY, 0.0, 0.0, 0, 0.0, 0, false, false, 0.0);
            t += DT;
        }
        assert!(r.failed && !r.done, "a run that never scores should fail, not hang open forever");
    }

    #[test]
    fn bests_only_records_an_improvement_and_survives_a_save_round_trip() {
        let mut b = Bests::default();
        assert!(b.record(3, 12.0), "a first finish should always be a record");
        assert!(!b.record(3, 15.0), "a slower run should not overwrite a faster one");
        assert!(b.record(3, 9.5), "a faster run should record");
        assert!((b.get(3) - 9.5).abs() < 1e-3, "the best time wasn't kept");
        let round = Bests::decode(&b.encode());
        assert!((round.get(3) - 9.5).abs() < 0.01, "best time didn't survive the save format");
        assert_eq!(round.finished(), 1);
    }

    #[test]
    fn a_mangled_bests_string_pads_to_zero_rather_than_panicking() {
        let b = Bests::decode("not numbers at all");
        assert_eq!(b.finished(), 0);
        assert_eq!(b.0.len(), TRIALS.len(), "should still pad out to one slot per trial");
    }

    #[test]
    fn the_ghost_follows_the_run_and_clamps_past_its_end() {
        let mut r = Run::new(0);
        for k in 0..600 {
            let p = Vec3::new(k as f32 * 0.1, 2.0, 0.0);
            r.tick(DT, p, Quat::IDENTITY, 10.0, 0.0, 0, 0.0, 0, false, false, 0.0);
        }
        let mid = ghost_at(&r.ghost, r.t * 0.5).map(|(p, _)| p.x).unwrap_or(-1.0);
        let end = ghost_at(&r.ghost, r.t * 10.0).map(|(p, _)| p.x).unwrap_or(-1.0);
        assert!(mid > 25.0 && mid < 35.0, "ghost midpoint x={mid} doesn't track the run");
        assert!(end > 55.0, "ghost should clamp to the run's last known position, not vanish: end x={end}");
    }
}
