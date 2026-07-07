//! Player gesture detection — spec §3 (gesture grammar) + §10.2 (detection).
//!
//! Turns one frame of raw player input — look direction, movement, and the cast
//! button's timing — into the `(Stance, Gesture, Directional, Element)`
//! quadruple the bending grammar consumes (`gesture_resolve`). This is the INPUT
//! half; `gesture_resolve` is the rules half; `bending_runtime` drives both via
//! `BenderState::update_from_input`.
//!
//! Pure and deterministic: no input device access, no time, no allocation.

use crate::sim::gesture_resolve::{Directional, Element, Gesture, Stance};

/// Phase of the cast button this frame (the timing state machine of §10.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressPhase {
    /// Not pressed — the player is not casting.
    Idle,
    /// Pressed and released inside the tap window.
    Tap,
    /// Held past the tap window (a sustained press).
    Hold,
    /// Held while a charge builds toward a release.
    Charging,
    /// Let go this frame.
    Released,
}

/// One frame of raw player input.
#[derive(Debug, Clone, Copy)]
pub struct InputState {
    /// Unit look direction (component `[1]` is up).
    pub look: [f32; 3],
    /// World-space movement velocity (units/s).
    pub velocity: [f32; 3],
    /// Feet off the ground.
    pub airborne: bool,
    /// Sprint modifier held.
    pub sprint: bool,
    /// Cast-button phase this frame.
    pub press: PressPhase,
    /// Charge fraction `0..=1` (meaningful while `Charging`/`Released`).
    pub charge01: f32,
    /// A second tap landed inside the echo window.
    pub double_tap: bool,
    /// Player-selected element (passed straight through).
    pub element: Element,
}

/// The detected gesture quadruple, ready for `gesture_resolve::resolve`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Detected {
    pub stance: Stance,
    pub gesture: Gesture,
    pub directional: Directional,
    pub element: Element,
    /// `false` when the player isn't casting (button `Idle`) — skip the tick.
    pub active: bool,
}

// Tuning thresholds (spec §10.2), kept as named consts so behaviour is legible.
const STILL_SPEED: f32 = 0.5; // below this horizontal speed = standing still
const SPRINT_HINT_SPEED: f32 = 6.0; // moving this fast reads as a surge even un-flagged
const PITCH_LIFT: f32 = 0.55; // look.y above this = aiming up
const PITCH_GROUND: f32 = -0.55; // look.y below this = aiming down
const ALIGN: f32 = 0.5; // |look·move| above this = fore/aft; below = lateral
const CHARGE_MIN: f32 = 0.3; // releasing below this is a plain Release, not ChargeRelease

fn horiz_speed(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[2] * v[2]).sqrt()
}

fn stance_of(s: &InputState, speed: f32) -> Stance {
    if s.airborne {
        Stance::Aerial
    } else if s.sprint || speed >= SPRINT_HINT_SPEED {
        Stance::Surge
    } else if speed < STILL_SPEED {
        Stance::Rooted
    } else {
        Stance::Flow
    }
}

/// Returns `(gesture, active)`; `active` is false only when the button is idle.
fn gesture_of(s: &InputState) -> (Gesture, bool) {
    if s.double_tap {
        return (Gesture::Echo, true);
    }
    match s.press {
        PressPhase::Idle => (Gesture::Strike, false), // gesture is a don't-care
        PressPhase::Tap => (Gesture::Strike, true),
        PressPhase::Hold => (Gesture::Sustain, true),
        PressPhase::Charging => (Gesture::Sustain, true), // holding a charge reads as sustain
        PressPhase::Released => {
            if s.charge01 >= CHARGE_MIN {
                (Gesture::ChargeRelease, true)
            } else {
                (Gesture::Release, true)
            }
        }
    }
}

fn directional_of(s: &InputState, speed: f32) -> Directional {
    // Pitch dominates: a clear up/down aim sets Lift/Ground.
    if s.look[1] >= PITCH_LIFT {
        return Directional::Lift;
    }
    if s.look[1] <= PITCH_GROUND {
        return Directional::Ground;
    }
    // Otherwise read horizontal movement relative to look.
    if speed < STILL_SPEED {
        return Directional::Radial; // standing still → self-centred / omnidirectional
    }
    let look_h = (s.look[0] * s.look[0] + s.look[2] * s.look[2]).sqrt().max(1e-6);
    let dot = (s.look[0] * s.velocity[0] + s.look[2] * s.velocity[2]) / (look_h * speed.max(1e-6));
    if dot >= ALIGN {
        Directional::Push // moving where you look
    } else if dot <= -ALIGN {
        Directional::Pull // moving away from where you look
    } else {
        Directional::Sweep // moving across your look
    }
}

/// Detect the gesture quadruple for this frame.
pub fn detect(s: &InputState) -> Detected {
    let speed = horiz_speed(s.velocity);
    let (gesture, active) = gesture_of(s);
    Detected {
        stance: stance_of(s, speed),
        gesture,
        directional: directional_of(s, speed),
        element: s.element,
        active,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> InputState {
        InputState {
            look: [0.0, 0.0, 1.0],
            velocity: [0.0, 0.0, 0.0],
            airborne: false,
            sprint: false,
            press: PressPhase::Tap,
            charge01: 0.0,
            double_tap: false,
            element: Element::Fire,
        }
    }

    #[test]
    fn idle_is_inactive() {
        let mut s = base();
        s.press = PressPhase::Idle;
        assert!(!detect(&s).active);
    }

    #[test]
    fn gesture_from_press_timing() {
        let mut s = base();
        s.press = PressPhase::Tap;
        assert_eq!(detect(&s).gesture, Gesture::Strike);
        s.press = PressPhase::Hold;
        assert_eq!(detect(&s).gesture, Gesture::Sustain);
        s.press = PressPhase::Released;
        s.charge01 = 0.9;
        assert_eq!(detect(&s).gesture, Gesture::ChargeRelease);
        s.charge01 = 0.1;
        assert_eq!(detect(&s).gesture, Gesture::Release);
    }

    #[test]
    fn double_tap_is_echo() {
        let mut s = base();
        s.press = PressPhase::Hold;
        s.double_tap = true;
        assert_eq!(detect(&s).gesture, Gesture::Echo);
        assert!(detect(&s).active);
    }

    #[test]
    fn stance_from_movement() {
        let mut s = base();
        // standing still
        assert_eq!(detect(&s).stance, Stance::Rooted);
        // moving at a walk
        s.velocity = [0.0, 0.0, 3.0];
        assert_eq!(detect(&s).stance, Stance::Flow);
        // sprinting
        s.sprint = true;
        assert_eq!(detect(&s).stance, Stance::Surge);
        // airborne wins over everything
        s.airborne = true;
        assert_eq!(detect(&s).stance, Stance::Aerial);
    }

    #[test]
    fn fast_movement_reads_as_surge() {
        let mut s = base();
        s.velocity = [0.0, 0.0, 9.0]; // > SPRINT_HINT_SPEED, no flag
        assert_eq!(detect(&s).stance, Stance::Surge);
    }

    #[test]
    fn directional_from_pitch() {
        let mut s = base();
        s.look = [0.0, 1.0, 0.0];
        assert_eq!(detect(&s).directional, Directional::Lift);
        s.look = [0.0, -1.0, 0.0];
        assert_eq!(detect(&s).directional, Directional::Ground);
    }

    #[test]
    fn directional_from_movement_vs_look() {
        let mut s = base();
        s.look = [0.0, 0.0, 1.0];
        s.velocity = [0.0, 0.0, 5.0]; // with look
        assert_eq!(detect(&s).directional, Directional::Push);
        s.velocity = [0.0, 0.0, -5.0]; // against look
        assert_eq!(detect(&s).directional, Directional::Pull);
        s.velocity = [5.0, 0.0, 0.0]; // across look
        assert_eq!(detect(&s).directional, Directional::Sweep);
        s.velocity = [0.0, 0.0, 0.0]; // still
        assert_eq!(detect(&s).directional, Directional::Radial);
    }

    #[test]
    fn element_passes_through() {
        let mut s = base();
        s.element = Element::Water;
        assert_eq!(detect(&s).element, Element::Water);
    }

    #[test]
    fn deterministic() {
        let mut s = base();
        s.look = [0.2, 0.3, 0.9];
        s.velocity = [1.0, 0.0, 4.0];
        s.press = PressPhase::Released;
        s.charge01 = 0.7;
        assert_eq!(detect(&s), detect(&s));
    }
}
