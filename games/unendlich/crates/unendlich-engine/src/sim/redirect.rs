//! sim::redirect — routing an incoming elemental technique (REDIRECT gesture).
//!
//! From `resonance_bending_spec §3.1/§4.2`: the REDIRECT gesture applies a PULL
//! to an *incoming external* elemental technique — it **routes** the attack
//! rather than blocking it, sending it off along the defender's aim, and a
//! successful re-route *gains* Resonance (§4.2). This is that pure interaction:
//! given an incoming technique and the defender's aim + timing, decide whether
//! the redirect lands, where the technique now goes, and how much Resonance is
//! earned (more for reversing it back the way it came, more for tight timing).
//! You can only redirect an element you can bend. Composes with
//! `technique_catalogue` (the incoming move) and `resonance_state` (the gain).
//! Pure, deterministic, std-only.

/// Latest lead time (seconds before impact) at which a redirect can be committed.
pub const REDIRECT_WINDOW_SECS: f32 = 0.35;
/// Inner window for a clean redirect (full Resonance); outside it (but still in
/// the outer window) the route lands but the gain is reduced.
pub const PERFECT_WINDOW_SECS: f32 = 0.15;
/// Base Resonance for any successful redirect, before the reversal bonus.
pub const BASE_GAIN: f32 = 0.1;
/// Resonance multiplier for a late (grazed-timing) redirect.
pub const GRAZE_TIMING_MUL: f32 = 0.6;

/// An incoming external technique bearing down on the defender.
#[derive(Clone, Copy, Debug)]
pub struct Incoming {
    /// Technique power / magnitude (≥ 0).
    pub power: f32,
    /// Travel direction (need not be normalised).
    pub dir: [f32; 3],
    /// Seconds until it reaches the defender.
    pub time_to_impact: f32,
}

/// The defender's redirect input this instant.
#[derive(Clone, Copy, Debug)]
pub struct RedirectAttempt {
    /// Aim direction the technique will be routed along (need not be normalised).
    pub aim: [f32; 3],
    /// Whether the REDIRECT gesture is committed.
    pub committed: bool,
}

/// Result of a redirect attempt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RedirectResult {
    /// No route — not committed, wrong timing, or an element you can't bend.
    Missed,
    /// Routed along `new_dir` (unit vector); the defender gains `resonance_gain`.
    Redirected { new_dir: [f32; 3], resonance_gain: f32 },
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if m > 1e-6 {
        [v[0] / m, v[1] / m, v[2] / m]
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Resolve a redirect. `can_bend_element` is whether the defender has access to
/// the incoming technique's element (you can only grip what you can bend).
pub fn attempt_redirect(inc: &Incoming, att: &RedirectAttempt, can_bend_element: bool) -> RedirectResult {
    if !att.committed || !can_bend_element {
        return RedirectResult::Missed;
    }
    let t = inc.time_to_impact;
    // Must commit before impact and within the reaction window (not too early).
    if !(t > 0.0 && t <= REDIRECT_WINDOW_SECS) {
        return RedirectResult::Missed;
    }
    let aim = norm(att.aim);
    let indir = norm(inc.dir);
    // Reversal: 0 when routed the way it was already going, 1 when sent straight
    // back at the attacker — reversing it is the harder, higher-reward play.
    let reversal = ((1.0 - dot(aim, indir)) * 0.5).clamp(0.0, 1.0);
    let timing = if t <= PERFECT_WINDOW_SECS { 1.0 } else { GRAZE_TIMING_MUL };
    let gain = (BASE_GAIN + inc.power.max(0.0) * reversal) * timing;
    RedirectResult::Redirected { new_dir: aim, resonance_gain: gain }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inc(power: f32, dir: [f32; 3], t: f32) -> Incoming {
        Incoming { power, dir, time_to_impact: t }
    }
    fn att(aim: [f32; 3]) -> RedirectAttempt {
        RedirectAttempt { aim, committed: true }
    }

    #[test]
    fn no_commit_or_wrong_element_misses() {
        let i = inc(1.0, [0.0, 0.0, 1.0], 0.1);
        assert_eq!(
            attempt_redirect(&i, &RedirectAttempt { aim: [1.0, 0.0, 0.0], committed: false }, true),
            RedirectResult::Missed
        );
        assert_eq!(attempt_redirect(&i, &att([1.0, 0.0, 0.0]), false), RedirectResult::Missed);
    }

    #[test]
    fn too_late_or_too_early_misses() {
        let late = inc(1.0, [0.0, 0.0, 1.0], 0.0); // already hit
        assert_eq!(attempt_redirect(&late, &att([1.0, 0.0, 0.0]), true), RedirectResult::Missed);
        let early = inc(1.0, [0.0, 0.0, 1.0], REDIRECT_WINDOW_SECS + 0.1);
        assert_eq!(attempt_redirect(&early, &att([1.0, 0.0, 0.0]), true), RedirectResult::Missed);
    }

    #[test]
    fn a_clean_redirect_routes_along_aim_and_gains_resonance() {
        let i = inc(1.0, [0.0, 0.0, 1.0], 0.1); // within perfect window
        match attempt_redirect(&i, &att([1.0, 0.0, 0.0]), true) {
            RedirectResult::Redirected { new_dir, resonance_gain } => {
                assert!((new_dir[0] - 1.0).abs() < 1e-4, "routed along aim");
                assert!(resonance_gain > 0.0);
            }
            _ => panic!("should redirect"),
        }
    }

    #[test]
    fn reversing_gains_more_than_deflecting() {
        let i = inc(1.0, [0.0, 0.0, 1.0], 0.1);
        // send it straight back (aim opposite incoming): max reversal
        let back = attempt_redirect(&i, &att([0.0, 0.0, -1.0]), true);
        // deflect sideways (perpendicular): half reversal
        let side = attempt_redirect(&i, &att([1.0, 0.0, 0.0]), true);
        let (gb, gs) = match (back, side) {
            (RedirectResult::Redirected { resonance_gain: a, .. }, RedirectResult::Redirected { resonance_gain: b, .. }) => (a, b),
            _ => panic!(),
        };
        assert!(gb > gs, "reversal {gb} should beat deflection {gs}");
    }

    #[test]
    fn tight_timing_gains_more_than_a_late_route() {
        let perfect = inc(1.0, [0.0, 0.0, 1.0], PERFECT_WINDOW_SECS - 0.01);
        let late = inc(1.0, [0.0, 0.0, 1.0], REDIRECT_WINDOW_SECS - 0.01);
        let gp = match attempt_redirect(&perfect, &att([0.0, 0.0, -1.0]), true) {
            RedirectResult::Redirected { resonance_gain, .. } => resonance_gain,
            _ => panic!(),
        };
        let gl = match attempt_redirect(&late, &att([0.0, 0.0, -1.0]), true) {
            RedirectResult::Redirected { resonance_gain, .. } => resonance_gain,
            _ => panic!(),
        };
        assert!(gp > gl, "perfect timing {gp} beats late {gl}");
    }

    #[test]
    fn stronger_incoming_yields_more_on_reversal() {
        let weak = inc(0.5, [0.0, 0.0, 1.0], 0.1);
        let strong = inc(2.0, [0.0, 0.0, 1.0], 0.1);
        let gw = match attempt_redirect(&weak, &att([0.0, 0.0, -1.0]), true) {
            RedirectResult::Redirected { resonance_gain, .. } => resonance_gain,
            _ => panic!(),
        };
        let gs = match attempt_redirect(&strong, &att([0.0, 0.0, -1.0]), true) {
            RedirectResult::Redirected { resonance_gain, .. } => resonance_gain,
            _ => panic!(),
        };
        assert!(gs > gw);
    }

    #[test]
    fn deterministic() {
        let i = inc(1.0, [0.0, 0.0, 1.0], 0.1);
        assert_eq!(attempt_redirect(&i, &att([1.0, 0.0, 0.0]), true), attempt_redirect(&i, &att([1.0, 0.0, 0.0]), true));
    }
}
