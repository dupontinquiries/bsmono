//! `sim::smoke_tells` — what leaks out of a dragon's smoke while it is hidden
//! in it, so a fight in smoke is readable without being a giveaway.
//!
//! Neither side has perfect vision (no boss-fight plot armour): the dragon
//! hunts you by nose and ear through its smoke, and you get only what light
//! escapes the cloud:
//!
//! * **Flashes** while it charges a bending cast: the charge lights the smoke
//!   from inside, scattered — a pulse of light with **no direction** (the whole
//!   cloud glows), quickening as the charge builds. You know something is
//!   coming and roughly when; not from where. Disorienting by design.
//! * **Eyes** just before the attacks that come with a look — a lunge, a
//!   breath: for a short window before the blow the eyes open and glow. Two
//!   small bright points punch through smoke that hides the whole body
//!   (a point source needs far less transmittance than a silhouette), so this
//!   is the one moment you can place it. Cunning dragons open their eyes late:
//!   a shorter window.
//!
//! When you can see the dragon anyway (transmittance above `smoke_wall::
//! SIGHT_FLOOR`) there are no tells — you have the real thing. Pure.

use crate::sim::smoke_wall::SIGHT_FLOOR;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Tell {
    /// 0..1 diffuse flash this instant (directionless).
    pub flash: f32,
    /// 0..1 visibility of its eyes (0 = shut or not visible).
    pub eyes: f32,
}

/// Seconds before a looking attack that the eyes open: 0.55 s for a dull
/// dragon down to 0.25 s for the most cunning.
pub fn eye_window(cunning: f32) -> f32 {
    0.55 - 0.3 * cunning.clamp(0.0, 1.0)
}

/// Eyes are visible if at least this much of their light gets through.
pub const EYES_FLOOR: f32 = 0.15;

/// `view_t`: transmittance from your eye to its BODY (same model as its eyes
/// on you) -- at or above `SIGHT_FLOOR` you can see it and there is nothing to
/// tell. `eye_t`: transmittance to its EYES, which lead a strike from the front
/// edge of the smoke, so they can be much clearer than the body behind them.
/// (One value for both hid the eyes exactly when they should show: a shell-game
/// soak struck with no eyes, because the clear path to the eyes read as "you
/// can see the dragon".) `charge01`: a bending cast's charge (0 if none).
/// `strike_in`: seconds until a lunge/breath lands. `time`: any clock (s).
pub fn tell(view_t: f32, eye_t: f32, charge01: f32, strike_in: Option<f32>, cunning: f32, time: f32) -> Tell {
    let t = if view_t.is_finite() { view_t.clamp(0.0, 1.0) } else { 1.0 };
    if t >= SIGHT_FLOOR {
        return Tell::default();
    }
    let te = if eye_t.is_finite() { eye_t.clamp(0.0, 1.0) } else { t };
    let c = if charge01.is_finite() { charge01.clamp(0.0, 1.0) } else { 0.0 };
    let flash = if c > 0.0 {
        // Pulses quicken from 2 Hz to 8 Hz as the charge builds.
        let rate = 2.0 + 6.0 * c;
        let phase = (time * rate).fract();
        let pulse = (1.0 - phase * 4.0).max(0.0); // a sharp flash, then dark
        c.powf(1.5) * (1.0 - t) * 0.8 * pulse
    } else {
        0.0
    };
    let eyes = match strike_in {
        Some(s) if s.is_finite() && s >= 0.0 && s <= eye_window(cunning) => {
            // A small bright point needs far less transmittance than a body.
            let v = te.powf(0.4);
            if v >= EYES_FLOOR { v } else { 0.0 }
        }
        _ => 0.0,
    };
    Tell { flash, eyes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_leaks_when_you_can_see_it() {
        assert_eq!(tell(0.8, 0.8, 1.0, Some(0.1), 0.5, 0.0), Tell::default());
    }

    #[test]
    fn a_charge_in_the_smoke_flashes_faster_and_brighter_but_without_direction() {
        // Peak of the flash over a second of pulses, at low and high charge.
        let peak = |c: f32| (0..100).map(|i| tell(0.05, 0.05, c, None, 0.5, i as f32 * 0.01).flash).fold(0.0, f32::max);
        let count = |c: f32| (0..100).filter(|&i| {
            let a = tell(0.05, 0.05, c, None, 0.5, i as f32 * 0.01).flash;
            let b = tell(0.05, 0.05, c, None, 0.5, (i + 1) as f32 * 0.01).flash;
            a == 0.0 && b > 0.0
        }).count();
        assert!(peak(1.0) > peak(0.3) * 2.0);
        assert!(count(1.0) > count(0.2), "pulses quicken: {} vs {}", count(1.0), count(0.2));
        assert_eq!(tell(0.05, 0.05, 0.0, None, 0.5, 0.0).flash, 0.0);
    }

    #[test]
    fn eyes_open_only_just_before_the_blow_and_later_if_cunning() {
        let t = 0.05; // the body is hidden (well under SIGHT_FLOOR)…
        assert!(tell(t, t, 0.0, Some(0.2), 0.0, 0.0).eyes > EYES_FLOOR, "…but the eyes punch through");
        assert_eq!(tell(t, t, 0.0, Some(1.0), 0.0, 0.0).eyes, 0.0, "too early");
        assert_eq!(tell(t, t, 0.0, None, 0.0, 0.0).eyes, 0.0);
        // At 0.4 s out, a dull dragon's eyes are open, a cunning one's are not.
        assert!(tell(t, t, 0.0, Some(0.4), 0.0, 0.0).eyes > 0.0);
        assert_eq!(tell(t, t, 0.0, Some(0.4), 1.0, 0.0).eyes, 0.0);
    }

    #[test]
    fn eyes_at_the_edge_show_while_the_body_stays_hidden() {
        // Body deep in the draft (hidden), eyes at its front edge (clear).
        let tl = tell(0.001, 0.5, 0.0, Some(0.1), 0.5, 0.0);
        assert!(tl.eyes > 0.5, "eyes {}", tl.eyes);
        // But if you can see the body, there is nothing to tell.
        assert_eq!(tell(0.8, 0.9, 0.0, Some(0.1), 0.5, 0.0), Tell::default());
    }

    #[test]
    fn deep_enough_smoke_hides_even_the_eyes() {
        assert_eq!(tell(0.001, 0.001, 0.0, Some(0.1), 0.0, 0.0).eyes, 0.0);
    }
}
