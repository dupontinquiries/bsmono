//! academy — the game already knows what you did, so let it teach.
//!
//! Skydribble grew a lot of depth (air dribbles, flip resets, wave dashes,
//! pinches, portal flings) and never explained any of it. The replay system
//! solved the hard half of a tutorial without meaning to: it recognises
//! tricks from physics events, so a lesson can be *verified* rather than
//! guessed at. "Do a wave dash" is a real instruction when the game can tell
//! whether you did one.
//!
//! Each lesson sets the board — car here, ball there, moving like this — so
//! the trick is possible on the first try instead of needing a lucky bounce.
//! Get it and the moment replays itself, which is both the reward and the
//! demonstration for next time.

use crate::replay::Trick;
use glam::Vec3;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Skill {
    /// pull off a recognised trick
    Do(Trick),
    /// send the ball away at N m/s
    BallSpeed(f32),
    /// get the car N metres up
    Height(f32),
    /// touch the ball N times without it landing
    Combo(u32),
}

/// How the board is set when a lesson starts, relative to the car.
#[derive(Clone, Copy)]
pub struct Setup {
    /// ball offset in front of / above the car
    pub ahead: f32,
    pub up: f32,
    /// initial ball velocity
    pub vel: Vec3,
    /// start the car in the air this high (0 = on the ground)
    pub car_up: f32,
}

pub struct Lesson {
    pub name: &'static str,
    pub hint: &'static str,
    /// what the coach says when you are close but not there
    pub nudge: &'static str,
    pub map: usize,
    pub setup: Setup,
    pub skill: Skill,
    pub limit: f32,
}

const GROUND: Setup = Setup { ahead: 6.0, up: 0.0, vel: Vec3::ZERO, car_up: 0.0 };

pub const LESSONS: &[Lesson] = &[
    Lesson {
        name: "THE FLIP",
        hint: "Jump, then jump again while pushing a direction.",
        nudge: "That was a jump — press it a second time in the air, holding a direction.",
        map: 20,
        setup: GROUND,
        skill: Skill::Do(Trick::Flip),
        limit: 60.0,
    },
    Lesson {
        name: "THE WAVE DASH",
        hint: "Jump, then flip INTO the ground as you land. The flip becomes speed.",
        nudge: "Flip later — you want to be landing as the flip fires.",
        map: 20,
        setup: GROUND,
        skill: Skill::Do(Trick::WaveDash),
        limit: 90.0,
    },
    Lesson {
        name: "POWER SHOT",
        hint: "Boost into the ball with the nose pointed where you want it.",
        nudge: "Faster — hit it with the car's nose, not its side.",
        map: 20,
        setup: GROUND,
        skill: Skill::BallSpeed(26.0),
        limit: 60.0,
    },
    Lesson {
        name: "THE AERIAL",
        hint: "Jump, point the nose at the ball, and boost.",
        nudge: "Pitch the nose UP first, then hold boost.",
        map: 20,
        setup: Setup { ahead: 9.0, up: 8.0, vel: Vec3::new(0.0, 4.0, 0.0), car_up: 0.0 },
        skill: Skill::Do(Trick::Aerial),
        limit: 90.0,
    },
    Lesson {
        name: "GOING UP",
        hint: "Aerial higher — twenty metres off the floor.",
        nudge: "Keep the nose up and feather the boost.",
        map: 20,
        setup: GROUND,
        skill: Skill::Height(20.0),
        limit: 90.0,
    },
    Lesson {
        name: "KEEPY UPPY",
        hint: "Four touches without letting the ball land.",
        nudge: "Softer — tap it up rather than hitting it away.",
        map: 25,
        setup: Setup { ahead: 5.0, up: 5.0, vel: Vec3::ZERO, car_up: 0.0 },
        skill: Skill::Combo(4),
        limit: 90.0,
    },
    Lesson {
        name: "THE AIR DRIBBLE",
        hint: "Pop the ball up, chase it into the air, and keep touching it.",
        nudge: "Stay under and behind it — small touches, not swings.",
        map: 25,
        setup: Setup { ahead: 4.5, up: 3.0, vel: Vec3::new(0.0, 7.0, -4.0), car_up: 0.0 },
        skill: Skill::Do(Trick::AirDribble),
        limit: 120.0,
    },
    Lesson {
        name: "THE FLIP RESET",
        hint: "Put your wheels on the underside of the ball while airborne.",
        nudge: "All four wheels have to touch it — come at it from below.",
        map: 25,
        setup: Setup { ahead: 3.0, up: 14.0, vel: Vec3::ZERO, car_up: 6.0 },
        skill: Skill::Do(Trick::FlipReset),
        limit: 150.0,
    },
    Lesson {
        name: "THE PINCH",
        hint: "Trap the ball between your car and a wall.",
        nudge: "Closer to the wall — the ball has to be caught between the two.",
        map: 25,
        setup: Setup { ahead: 6.0, up: 0.0, vel: Vec3::new(0.0, 0.0, -8.0), car_up: 0.0 },
        skill: Skill::Do(Trick::Pinch),
        limit: 120.0,
    },
    Lesson {
        name: "THE FLING",
        hint: "Fall into the floor portal. Speed in is speed out.",
        nudge: "Get more height before you drop in — the fling keeps whatever you bring.",
        map: 31,
        setup: GROUND,
        skill: Skill::BallSpeed(1.0),
        limit: 120.0,
    },
];

pub struct Session {
    pub idx: usize,
    pub t: f32,
    pub done: bool,
    /// best progress seen, for the coach's nudge
    pub best: f32,
    pub nudged: bool,
    /// resets the board when you drift too far or the ball settles
    pub idle: f32,
}

impl Session {
    pub fn new(idx: usize) -> Session {
        Session { idx, t: 0.0, done: false, best: 0.0, nudged: false, idle: 0.0 }
    }
    pub fn lesson(&self) -> &'static Lesson {
        &LESSONS[self.idx.min(LESSONS.len() - 1)]
    }
}

/// Which lessons have been passed — one bit each, saved as a string.
#[derive(Clone, Default)]
pub struct Passed(pub u64);

impl Passed {
    pub fn has(&self, i: usize) -> bool {
        self.0 & (1u64 << i.min(63)) != 0
    }
    pub fn set(&mut self, i: usize) {
        self.0 |= 1u64 << i.min(63);
    }
    pub fn count(&self) -> usize {
        (0..LESSONS.len()).filter(|i| self.has(*i)).count()
    }
    pub fn encode(&self) -> String {
        self.0.to_string()
    }
    pub fn decode(s: &str) -> Passed {
        Passed(s.trim().parse().unwrap_or(0))
    }
}

/// Progress toward a lesson, 0..1 — drives the bar and the coach.
pub fn progress(skill: Skill, ball_speed: f32, height: f32, combo: u32, trick_hit: bool) -> f32 {
    match skill {
        Skill::Do(_) => {
            if trick_hit {
                1.0
            } else {
                0.0
            }
        }
        Skill::BallSpeed(v) => (ball_speed / v).clamp(0.0, 1.0),
        Skill::Height(m) => (height / m).clamp(0.0, 1.0),
        Skill::Combo(n) => (combo as f32 / n as f32).clamp(0.0, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from blackstar-rs's `--sim academy` (src/sim.rs), a manual
    // println!-driven harness nobody runs as part of `cargo test`. Same
    // discipline as `net.rs`'s sim-mp port: real logic, zero automated
    // coverage before this, only checked when someone remembered to run
    // the binary by hand. Split into one #[test] per property instead of
    // one big function, so a future failure names exactly what broke.

    #[test]
    fn every_lesson_names_a_real_map_and_has_real_text() {
        assert!(LESSONS.len() >= 8, "only {} lessons", LESSONS.len());
        for l in LESSONS {
            assert!(l.hint.len() > 12, "{}'s hint is too short to teach anything: {:?}", l.name, l.hint);
            assert!(l.nudge.len() > 12, "{}'s nudge is too short to help: {:?}", l.name, l.nudge);
        }
        // map is a world::style() index in the actual game; this crate
        // doesn't depend on blackstar-rs's world.rs, so this test can only
        // check for the obviously-wrong case (unset/negative-as-huge via
        // wraparound), not the real STYLE_COUNT bound -- that half of the
        // original check has to stay in blackstar-rs's own suite, which
        // depends on both this crate and world.rs and can check it for real.
        for l in LESSONS {
            assert!(l.map < 200, "{}'s map index {} looks like it was never set", l.name, l.map);
        }
    }

    #[test]
    fn the_hardest_tricks_are_taught_and_no_lesson_duplicates_another() {
        let taught: Vec<Trick> = LESSONS
            .iter()
            .filter_map(|l| match l.skill {
                Skill::Do(t) => Some(t),
                _ => None,
            })
            .collect();
        assert!(taught.contains(&Trick::FlipReset), "nothing teaches the flip reset");
        assert!(taught.contains(&Trick::AirDribble), "nothing teaches the air dribble");
        let mut u = taught.clone();
        u.sort_by_key(|t| t.name());
        u.dedup();
        assert_eq!(u.len(), taught.len(), "two lessons verify the same trick");
    }

    #[test]
    fn a_trick_lesson_is_only_passed_when_the_trick_actually_lands() {
        // progress must come from the detector, not from time passing --
        // stuffing in a maxed-out speed/height/combo without trick_hit=true
        // must still read as zero for a Skill::Do lesson.
        assert_eq!(progress(Skill::Do(Trick::FlipReset), 99.0, 99.0, 99, false), 0.0);
        assert_eq!(progress(Skill::Do(Trick::FlipReset), 0.0, 0.0, 0, true), 1.0);
    }

    #[test]
    fn measured_lessons_scale_linearly_with_the_measurement() {
        assert!((progress(Skill::BallSpeed(20.0), 10.0, 0.0, 0, false) - 0.5).abs() < 1e-5);
        assert_eq!(progress(Skill::BallSpeed(20.0), 40.0, 0.0, 0, false), 1.0, "should clamp at 1.0, not overshoot");
    }

    #[test]
    fn height_and_combo_lessons_scale_the_same_way_as_ball_speed() {
        assert!((progress(Skill::Height(20.0), 0.0, 5.0, 0, false) - 0.25).abs() < 1e-5);
        assert!((progress(Skill::Combo(4), 0.0, 0.0, 2, false) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn lessons_that_need_the_ball_up_actually_set_it_up() {
        // the board has to make the trick POSSIBLE, not just theoretically
        // reachable -- a flip-reset or air-dribble lesson with the ball on
        // the ground would be asking for something the setup can't deliver.
        for l in LESSONS.iter().filter(|l| matches!(l.skill, Skill::Do(Trick::FlipReset) | Skill::Do(Trick::AirDribble))) {
            assert!(l.setup.up > 2.0 || l.setup.vel.y > 2.0, "{} doesn't actually put the ball up", l.name);
        }
    }

    #[test]
    fn passed_tracks_bits_correctly_and_survives_a_save_round_trip() {
        let mut p = Passed::default();
        assert_eq!(p.count(), 0, "a fresh Passed should have nothing learned");
        p.set(0);
        p.set(7);
        assert!(p.has(0) && p.has(7) && !p.has(3));
        let round = Passed::decode(&p.encode());
        assert!(round.has(0) && round.has(7));
        assert_eq!(round.count(), 2);
    }

    #[test]
    fn a_mangled_passed_string_decodes_as_nothing_learned_rather_than_failing() {
        // Passed::decode is fed straight from a save file line -- a hand-
        // edited or corrupted save shouldn't panic or crash progression.
        let p = Passed::decode("not a number");
        assert_eq!(p.count(), 0);
    }
}
