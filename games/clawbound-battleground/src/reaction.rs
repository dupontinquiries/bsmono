//! reaction -- a bot acts a reaction time after it sees.
//!
//! The brain (`ai::think`) decides every tick from the world as it is that
//! tick; before this, the body did it the same tick, so a bot answered a
//! cast, a step into range or a turned fight in 1/60 s -- faster than
//! anyone can play against. Now what a person would have to NOTICE comes
//! [`delay_for`] late:
//!
//! * a change of INTENT ([`Intent`]: the brain's state, its target, a goal
//!   off its lane -- an enemy come into reach, a fight turned, a stone to
//!   step away from) reaches the body a reaction time after the tick that
//!   decided it; until then the body goes on doing what it was;
//! * every cast is carried out a reaction time after it was decided, aimed
//!   where the brain saw the target then.
//!
//! Once the body is on an intent it follows it as the brain does, tick by
//! tick: steering round a wall, keeping a distance, walking a lane are
//! motor skill, not reaction. (Delaying those too was tried first: a bot
//! steering a stone corridor on what it saw 0.2 s before dithered against
//! the wall for 16 s in the Quarry.) There is no prediction in any of
//! this -- no reading ahead of what will happen, just a late answer to
//! what did -- so a dodge starts late, an engage lands a beat after the
//! opening, and a cast at a moving cat is aimed where it was.
//!
//! What is NOT delayed: the brain's own bookkeeping (its state, stance,
//! lane waypoint, the calls it makes), and a cat's basic attacks, which
//! are the body's (`systems::god_autoattacks`) for a player too.
//!
//! The delay is the match rule `rules::MatchRules::bot_reaction` (default
//! [`PRO`]), spread ±[`SPREAD`] across the bots so a team does not move as
//! one clock. Zero is the old instant bot, exactly.

use glam::Vec3;
use std::collections::VecDeque;

/// A strong player's visual reaction, s: the default.
pub const PRO: f32 = 0.20;
/// Bots differ by up to this share either way (a fixed draw per bot).
pub const SPREAD: f32 = 0.15;

/// What the brain means to be doing: a change here is what takes a
/// reaction time to reach the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Intent {
    pub state: crate::sim::AiState,
    pub target: Option<crate::sim::TargetRef>,
    pub off_lane: bool,
}

/// One tick's decision, due at `due`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Act {
    pub due: f32,
    pub move_dir: Vec3,
    pub cast: Option<(u32, Vec3)>,
    pub intent: Intent,
}

/// Bot `gi`'s delay under a match reaction of `base` s.
pub fn delay_for(base: f32, gi: usize) -> f32 {
    if base <= 0.0 {
        return 0.0;
    }
    // a fixed draw per seat in -1..1 (no rng: the delay must not move the
    // sim's own random stream)
    let h = ((gi as u32).wrapping_mul(0x9E37_79B9) >> 16) as f32 / 65535.0 * 2.0 - 1.0;
    base * (1.0 + SPREAD * h)
}

/// Queue `act` (decided at `now`) to fall due `delay` later, and take what
/// is due. `body` is the intent the body is on: it moves to a new one when
/// the first act of it falls due. The move: this tick's decision if the
/// body is already on its intent (it tracks), else the latest move that
/// fell due (it goes on with what it was doing); None if neither. And
/// every cast that fell due, in order.
pub fn through(queue: &mut VecDeque<Act>, body: &mut Option<Intent>, now: f32, delay: f32, act: Act) -> (Option<Vec3>, Vec<(u32, Vec3)>) {
    queue.push_back(Act { due: now + delay, ..act });
    let mut due_move = None;
    let mut casts = Vec::new();
    while queue.front().is_some_and(|a| a.due <= now + 1e-6) {
        let a = queue.pop_front().unwrap();
        *body = Some(a.intent);
        due_move = Some(a.move_dir);
        if let Some(c) = a.cast {
            casts.push(c);
        }
    }
    let mv = if *body == Some(act.intent) { Some(act.move_dir) } else { due_move };
    (mv, casts)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sim::{AiState, TargetRef};

    fn intent(state: AiState) -> Intent {
        Intent { state, target: (state == AiState::Fight).then_some(TargetRef::God(3)), off_lane: false }
    }

    fn act(x: f32, cast: Option<u32>, state: AiState) -> Act {
        Act { due: 0.0, move_dir: Vec3::new(x, 0.0, 0.0), cast: cast.map(|s| (s, Vec3::ZERO)), intent: intent(state) }
    }

    /// No delay: each decision is done the tick it is made, as before.
    #[test]
    fn zero_is_the_instant_bot() {
        let (mut q, mut body) = (VecDeque::new(), None);
        for k in 0..6 {
            let st = if k < 3 { AiState::Push } else { AiState::Fight };
            let (mv, casts) = through(&mut q, &mut body, k as f32 / 60.0, 0.0, act(k as f32, (k == 3).then_some(2), st));
            assert_eq!(mv, Some(Vec3::new(k as f32, 0.0, 0.0)));
            assert_eq!(casts.len(), (k == 3) as usize);
        }
        assert!(q.is_empty());
    }

    /// With 0.2 s: walking its lane, the bot tracks tick by tick; a fight
    /// decided at tick 20 reaches the body 0.2 s later (it walks on until
    /// then), and from then it tracks the fight tick by tick; a cast
    /// decided at tick 25 is cast 0.2 s after.
    #[test]
    fn a_change_of_mind_reaches_the_body_a_reaction_later() {
        let (mut q, mut body) = (VecDeque::new(), Some(intent(AiState::Push)));
        let dt = 1.0 / 60.0;
        let mut cast_at = None;
        for k in 0..60 {
            let now = k as f32 * dt;
            let st = if k < 20 { AiState::Push } else { AiState::Fight };
            let (mv, casts) = through(&mut q, &mut body, now, 0.2, act(k as f32, (k == 25).then_some(1), st));
            let m = mv.expect("the body stood still").x as i32;
            if k < 20 {
                assert_eq!(m, k, "walking the lane is tracked, not delayed");
            } else if now < 20.0 * dt + 0.2 - 1e-4 {
                assert!(m < 20, "tick {k}: the body turned to the fight before it could have noticed it");
            } else {
                assert_eq!(m, k, "tick {k}: once on the fight it tracks it");
            }
            if !casts.is_empty() {
                cast_at = Some(now);
            }
        }
        let c = cast_at.expect("never cast");
        assert!((c - (25.0 * dt + 0.2)).abs() < dt, "the cast decided at tick 25 was done at {c}");
    }

    #[test]
    fn bots_differ_but_stay_near_the_setting() {
        let d: Vec<f32> = (0..30).map(|i| delay_for(PRO, i)).collect();
        assert!(d.iter().all(|x| (PRO * (1.0 - SPREAD) - 1e-4..=PRO * (1.0 + SPREAD) + 1e-4).contains(x)), "{d:?}");
        let (lo, hi) = d.iter().fold((f32::MAX, 0.0f32), |(a, b), &x| (a.min(x), b.max(x)));
        assert!(hi - lo > PRO * SPREAD, "the bots all react alike: {lo}..{hi}");
        assert_eq!(delay_for(0.0, 7), 0.0);
    }
}
