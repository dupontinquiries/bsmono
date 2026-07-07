//! `sim::water_shape` — the lifecycle of a conjured water shape: how it persists,
//! moves, and decays back to fluid, per its state modifier.
//!
//! A parsed [`shape_word::StateMod`](crate::sim::shape_word::StateMod) becomes a
//! [`WaterState`], and the fluid scheduler ticks each active shape (§6.3):
//! * **Hold** — no decay; persists until the bender drops SUSTAIN.
//! * **Decay** — persists `duration` seconds, then reverts to fluid.
//! * **Flow** — duration-limited *and* drifts along its velocity each tick; a
//!   collision (reported by the caller) converts it to a splash early.
//! * **Break** — a fast projectile; on impact the caller applies torrent force.
//! * **Frozen / Mist** — resolved at creation, so the scheduler retires them.
//!
//! This core owns only the *timing and intent*: given `dt` it decides whether a
//! shape should expire, hold, translate, or advance as a projectile, and returns
//! the movement delta. The caller performs the actual world translation and
//! reports collisions back (the geometry stays in the engine). Pure/std-only.

use crate::sim::shape_word::StateMod;

/// The lifecycle state of an active water shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaterState {
    /// Sustained; only ends when the bender releases it.
    Hold,
    /// Reverts to fluid after `duration` seconds.
    Decay { duration: f32 },
    /// Drifts along its velocity, reverting after `duration` seconds (or on a
    /// collision reported by the caller).
    Flow { duration: f32 },
    /// A projectile travelling at `speed` blocks/sec along its velocity.
    Break { speed: f32 },
    /// Immediate ice conversion — retired by the scheduler.
    Frozen,
    /// Humidity field — resolved at creation, retired by the scheduler.
    Mist,
}

impl WaterState {
    /// Build a lifecycle state from a parsed state modifier, given the situational
    /// `duration` (from hold time / env) and projectile `speed` (from env / tier).
    /// `Hold`/`Frozen` ignore the numeric inputs; a phrase with no state modifier
    /// defaults to a plain `Decay`.
    pub fn from_state_mod(state: Option<StateMod>, duration: f32, speed: f32) -> WaterState {
        match state {
            Some(StateMod::Hold) => WaterState::Hold,
            Some(StateMod::Flow) => WaterState::Flow { duration },
            Some(StateMod::Break) => WaterState::Break { speed },
            Some(StateMod::Frozen) => WaterState::Frozen,
            None => WaterState::Decay { duration },
        }
    }
}

/// What the scheduler should do with a shape this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TickOutcome {
    /// Leave the shape as-is (e.g. a sustained Hold).
    Persist,
    /// Revert the shape to fluid and drop the task.
    Expire,
    /// Drift the shape by `delta` this tick (Flow).
    Translate { delta: [f32; 3] },
    /// Advance the projectile by `delta` this tick (Break); the caller resolves
    /// any collision and, on a hit, applies torrent force + expires the shape.
    Advance { delta: [f32; 3] },
}

/// An active water shape being ticked by the fluid scheduler. `blocks` are owned
/// by the caller; this tracks only the state and elapsed time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaterShapeTask {
    pub state: WaterState,
    /// Velocity in blocks/sec (drift for Flow; direction for Break).
    pub velocity: [f32; 3],
    /// Seconds since creation.
    pub elapsed: f32,
}

impl WaterShapeTask {
    pub fn new(state: WaterState, velocity: [f32; 3]) -> Self {
        WaterShapeTask { state, velocity, elapsed: 0.0 }
    }

    /// Advance the shape by `dt` seconds and decide its fate this tick.
    pub fn tick(&mut self, dt: f32) -> TickOutcome {
        self.elapsed += dt;
        match self.state {
            WaterState::Hold => TickOutcome::Persist,
            WaterState::Decay { duration } => {
                if self.elapsed >= duration {
                    TickOutcome::Expire
                } else {
                    TickOutcome::Persist
                }
            }
            WaterState::Flow { duration } => {
                if self.elapsed >= duration {
                    TickOutcome::Expire
                } else {
                    TickOutcome::Translate { delta: scale(self.velocity, dt) }
                }
            }
            WaterState::Break { speed } => {
                let dir = normalize(self.velocity);
                TickOutcome::Advance { delta: scale(dir, speed * dt) }
            }
            WaterState::Frozen | WaterState::Mist => TickOutcome::Expire,
        }
    }

    /// Fraction of the shape's lifetime elapsed (0..1), or `None` for states with
    /// no fixed duration (Hold / Break / instantly-resolved).
    pub fn life_fraction(&self) -> Option<f32> {
        match self.state {
            WaterState::Decay { duration } | WaterState::Flow { duration } if duration > 0.0 => {
                Some((self.elapsed / duration).clamp(0.0, 1.0))
            }
            _ => None,
        }
    }
}

fn scale(v: [f32; 3], k: f32) -> [f32; 3] {
    [v[0] * k, v[1] * k, v[2] * k]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1.0e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 0.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_never_decays() {
        let mut t = WaterShapeTask::new(WaterState::Hold, [0.0; 3]);
        for _ in 0..1000 {
            assert_eq!(t.tick(0.1), TickOutcome::Persist, "sustained shape persists");
        }
        assert_eq!(t.life_fraction(), None);
    }

    #[test]
    fn decay_expires_after_its_duration() {
        let mut t = WaterShapeTask::new(WaterState::Decay { duration: 1.0 }, [0.0; 3]);
        // 9 ticks of 0.1s = 0.9s → still alive
        for _ in 0..9 {
            assert_eq!(t.tick(0.1), TickOutcome::Persist);
        }
        assert!(t.life_fraction().unwrap() > 0.85);
        // the tick that crosses 1.0s expires it
        assert_eq!(t.tick(0.15), TickOutcome::Expire);
    }

    #[test]
    fn flow_translates_then_expires() {
        let mut t = WaterShapeTask::new(WaterState::Flow { duration: 1.0 }, [2.0, 0.0, 0.0]);
        // each 0.1s tick drifts velocity*dt = 0.2 blocks in +x
        match t.tick(0.1) {
            TickOutcome::Translate { delta } => assert!((delta[0] - 0.2).abs() < 1e-6),
            other => panic!("expected translate, got {other:?}"),
        }
        // run out the clock
        for _ in 0..9 {
            t.tick(0.1);
        }
        assert_eq!(t.tick(0.1), TickOutcome::Expire, "flow ends after its duration");
    }

    #[test]
    fn break_advances_as_a_normalized_projectile() {
        // velocity magnitude shouldn't matter for Break — only its direction and speed
        let mut t = WaterShapeTask::new(WaterState::Break { speed: 10.0 }, [0.0, 0.0, 5.0]);
        match t.tick(0.1) {
            TickOutcome::Advance { delta } => {
                // normalized +z * speed(10) * dt(0.1) = 1.0 in +z
                assert!((delta[2] - 1.0).abs() < 1e-6, "got {delta:?}");
                assert!(delta[0].abs() < 1e-6 && delta[1].abs() < 1e-6);
            }
            other => panic!("expected advance, got {other:?}"),
        }
        assert_eq!(t.life_fraction(), None, "break has no fixed lifetime here");
    }

    #[test]
    fn frozen_and_mist_are_retired_immediately() {
        let mut frozen = WaterShapeTask::new(WaterState::Frozen, [0.0; 3]);
        assert_eq!(frozen.tick(0.1), TickOutcome::Expire);
        let mut mist = WaterShapeTask::new(WaterState::Mist, [0.0; 3]);
        assert_eq!(mist.tick(0.1), TickOutcome::Expire);
    }

    #[test]
    fn state_mod_maps_to_lifecycle() {
        assert_eq!(WaterState::from_state_mod(Some(StateMod::Hold), 5.0, 10.0), WaterState::Hold);
        assert_eq!(
            WaterState::from_state_mod(Some(StateMod::Flow), 4.0, 10.0),
            WaterState::Flow { duration: 4.0 }
        );
        assert_eq!(
            WaterState::from_state_mod(Some(StateMod::Break), 4.0, 12.0),
            WaterState::Break { speed: 12.0 }
        );
        assert_eq!(WaterState::from_state_mod(Some(StateMod::Frozen), 4.0, 10.0), WaterState::Frozen);
        // no modifier → a plain timed decay
        assert_eq!(WaterState::from_state_mod(None, 3.0, 10.0), WaterState::Decay { duration: 3.0 });
    }
}
