//! `sim::sound_mix` — gameplay audio gain/pan + voice budgeting (WID r57-07).
//!
//! Output-side mixing math: distance attenuation, equal-power stereo pan, and a
//! voice limit that keeps the most audible emitters. Distinct from
//! `sim::sound_masking` (the dragon-hearing dB/occlusion model) — this just
//! decides per-emitter gains; the bin owns the device, buffers, and playback.
//!
//! Pure: logic in, data out — no `crate::render`, no GPU, no audio I/O, no
//! clock, no RNG, no `.unwrap()`/`.expect()` in non-test code. Release-safe
//! (no `#[cfg]`-gated helpers); compiles clean under `cargo check --release`.

use std::f32::consts::PI;

/// Distance attenuation curve between `min_dist` and `max_dist`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rolloff {
    /// Straight line from 1 → 0.
    Linear,
    /// Concave — faster initial falloff (`1/d`-like).
    Inverse,
    /// Steeper still (`1/d²`-like).
    InverseSquare,
}

/// The listener's pose. `forward`/`right` are orientation axes (need not be
/// normalized; a zero vector is handled gracefully).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub pos: [f32; 3],
    pub forward: [f32; 3],
    pub right: [f32; 3],
}

/// A positioned sound source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emitter {
    pub id: u32,
    pub pos: [f32; 3],
    pub volume01: f32,
    pub min_dist: f32,
    pub max_dist: f32,
    pub priority: f32,
    pub rolloff: Rolloff,
}

/// A mixed voice: per-channel gain for one emitter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Voice {
    pub id: u32,
    pub gain_l: f32,
    pub gain_r: f32,
}

/// Shapes the falloff between min and max (documented; higher = faster early drop).
const ROLL_K: f32 = 4.0;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Distance attenuation in `[0,1]`: `1` at/inside `min_dist`, `0` at/beyond
/// `max_dist`, monotonically non-increasing between, shaped by `rolloff`.
pub fn attenuation(e: &Emitter, listener_pos: [f32; 3]) -> f32 {
    let d = dist(e.pos, listener_pos);
    if !d.is_finite() {
        return 0.0;
    }
    let lo = e.min_dist.max(0.0);
    let hi = e.max_dist;
    if d <= lo {
        return 1.0;
    }
    if !(hi > lo) || d >= hi {
        // degenerate band (hi<=lo) or beyond max ⇒ silent
        return 0.0;
    }
    let t = ((d - lo) / (hi - lo)).clamp(0.0, 1.0); // 0 at min .. 1 at max
    // base curve: 1 at t=0, 0 at t=1, strictly decreasing
    let base = (1.0 - t) / (1.0 + ROLL_K * t);
    let g = match e.rolloff {
        Rolloff::Linear => 1.0 - t,
        Rolloff::Inverse => base,
        Rolloff::InverseSquare => base * base,
    };
    clamp01(g)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalized(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len.is_finite() && len > 1e-12 {
        Some([v[0] / len, v[1] / len, v[2] / len])
    } else {
        None
    }
}

/// Stereo pan in `[-1, 1]`: `-1` hard left .. `+1` hard right, `~0` dead ahead,
/// from the listener's `right` axis. A zero `right` (or coincident emitter) ⇒ `0`.
pub fn pan(l: &Listener, emitter_pos: [f32; 3]) -> f32 {
    let dir = [emitter_pos[0] - l.pos[0], emitter_pos[1] - l.pos[1], emitter_pos[2] - l.pos[2]];
    match (normalized(dir), normalized(l.right)) {
        (Some(du), Some(rn)) => dot(du, rn).clamp(-1.0, 1.0),
        _ => 0.0,
    }
}

/// Mix emitters into at most `max_voices` voices, keeping the most audible
/// (`audibility = attenuation · volume01 · priority`). Emitters with zero volume,
/// at/beyond `max_dist`, or non-positive audibility are dropped. Each kept voice
/// is equal-power panned: `gain_l² + gain_r² == gain²`, `gain = attenuation · volume01`.
pub fn mix(l: &Listener, emitters: &[Emitter], max_voices: usize) -> Vec<Voice> {
    // score survivors
    let mut scored: Vec<(usize, f32)> = Vec::new();
    let mut i = 0;
    while i < emitters.len() {
        let e = &emitters[i];
        let att = attenuation(e, l.pos);
        let vol = clamp01(e.volume01);
        if vol > 0.0 && att > 0.0 {
            let aud = att * vol * e.priority;
            if aud > 0.0 {
                scored.push((i, aud));
            }
        }
        i += 1;
    }
    // most audible first; deterministic tie-break by emitter id
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| emitters[a.0].id.cmp(&emitters[b.0].id)));
    if scored.len() > max_voices {
        scored.truncate(max_voices);
    }

    let mut out = Vec::with_capacity(scored.len());
    for (idx, _aud) in scored {
        let e = &emitters[idx];
        let gain = attenuation(e, l.pos) * clamp01(e.volume01);
        let p = pan(l, e.pos);
        // equal-power pan law: θ ∈ [0, π/2] as p goes -1 → +1
        let theta = (p + 1.0) * 0.5 * (PI * 0.5);
        out.push(Voice { id: e.id, gain_l: gain * theta.cos(), gain_r: gain * theta.sin() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emitter(id: u32, pos: [f32; 3], vol: f32, rolloff: Rolloff) -> Emitter {
        Emitter { id, pos, volume01: vol, min_dist: 1.0, max_dist: 10.0, priority: 1.0, rolloff }
    }
    fn listener() -> Listener {
        Listener { pos: [0.0, 0.0, 0.0], forward: [0.0, 0.0, 1.0], right: [1.0, 0.0, 0.0] }
    }

    #[test]
    fn attenuation_endpoints_and_monotone() {
        for roll in [Rolloff::Linear, Rolloff::Inverse, Rolloff::InverseSquare] {
            let e = emitter(0, [0.0, 0.0, 0.0], 1.0, roll);
            // at/inside min_dist ⇒ 1
            assert_eq!(attenuation(&e, [0.0, 0.0, 0.0]), 1.0);
            assert_eq!(attenuation(&e, [0.5, 0.0, 0.0]), 1.0, "inside min");
            assert_eq!(attenuation(&e, [1.0, 0.0, 0.0]), 1.0, "at min");
            // at/beyond max ⇒ 0
            assert_eq!(attenuation(&e, [10.0, 0.0, 0.0]), 0.0, "at max");
            assert_eq!(attenuation(&e, [50.0, 0.0, 0.0]), 0.0, "beyond max");
            // monotone non-increasing + in range over a sweep
            let mut prev = 2.0f32;
            let mut d = 0.0;
            while d <= 12.0 {
                let a = attenuation(&e, [d, 0.0, 0.0]);
                assert!((0.0..=1.0).contains(&a), "in range");
                assert!(a <= prev + 1e-6, "monotone non-increasing ({roll:?} at d={d})");
                prev = a;
                d += 0.1;
            }
        }
    }

    #[test]
    fn rolloff_steepness_ordering_between() {
        // between min and max, InverseSquare <= Inverse <= Linear (steeper falloff)
        let mk = |r| emitter(0, [0.0, 0.0, 0.0], 1.0, r);
        let p = [5.0, 0.0, 0.0];
        let lin = attenuation(&mk(Rolloff::Linear), p);
        let inv = attenuation(&mk(Rolloff::Inverse), p);
        let invsq = attenuation(&mk(Rolloff::InverseSquare), p);
        assert!(invsq <= inv + 1e-6 && inv <= lin + 1e-6, "steepness order: {invsq} <= {inv} <= {lin}");
    }

    #[test]
    fn pan_left_right_ahead() {
        let l = listener(); // right = +x, forward = +z
        assert!(pan(&l, [5.0, 0.0, 0.0]) > 0.5, "emitter to the right ⇒ positive");
        assert!(pan(&l, [-5.0, 0.0, 0.0]) < -0.5, "emitter to the left ⇒ negative");
        assert!(pan(&l, [0.0, 0.0, 5.0]).abs() < 1e-6, "dead ahead ⇒ ~0");
        // bounded
        assert!((-1.0..=1.0).contains(&pan(&l, [3.0, 2.0, 1.0])));
    }

    #[test]
    fn mix_caps_keeps_most_audible_and_drops_inaudible() {
        let l = listener();
        let emitters = [
            emitter(1, [1.0, 0.0, 0.0], 1.0, Rolloff::Linear),   // close, loud ⇒ most audible
            emitter(2, [8.0, 0.0, 0.0], 1.0, Rolloff::Linear),   // far, quieter
            emitter(3, [2.0, 0.0, 0.0], 0.0, Rolloff::Linear),   // volume 0 ⇒ dropped
            emitter(4, [100.0, 0.0, 0.0], 1.0, Rolloff::Linear), // beyond max ⇒ dropped
            emitter(5, [3.0, 0.0, 0.0], 0.8, Rolloff::Linear),   // mid
        ];
        let voices = mix(&l, &emitters, 2);
        assert!(voices.len() <= 2, "respects max_voices");
        let ids: Vec<u32> = voices.iter().map(|v| v.id).collect();
        assert!(ids.contains(&1), "keeps the most audible");
        assert!(!ids.contains(&3) && !ids.contains(&4), "drops vol0 and beyond-max");
        // generous budget: exactly the 3 audible survive (1,2,5), not 3/4
        let all = mix(&l, &emitters, 10);
        let mut got: Vec<u32> = all.iter().map(|v| v.id).collect();
        got.sort();
        assert_eq!(got, vec![1, 2, 5]);
    }

    #[test]
    fn equal_power_pan() {
        let l = listener();
        let emitters = [
            emitter(1, [3.0, 0.0, 1.0], 0.9, Rolloff::Inverse),
            emitter(2, [-2.0, 1.0, 4.0], 0.7, Rolloff::Linear),
            emitter(3, [0.0, 0.0, 2.0], 1.0, Rolloff::InverseSquare),
        ];
        for v in mix(&l, &emitters, 10) {
            // recover the emitter's expected overall gain
            let e = emitters.iter().find(|e| e.id == v.id).copied().unwrap();
            let gain = attenuation(&e, l.pos) * e.volume01;
            let power = v.gain_l * v.gain_l + v.gain_r * v.gain_r;
            assert!((power - gain * gain).abs() < 1e-4, "equal-power: {power} vs {}", gain * gain);
            assert!(v.gain_l >= 0.0 && v.gain_r >= 0.0);
        }
    }

    #[test]
    fn finite_with_zero_right_and_forward() {
        // degenerate listener axes must not produce NaN.
        let l = Listener { pos: [0.0, 0.0, 0.0], forward: [0.0, 0.0, 0.0], right: [0.0, 0.0, 0.0] };
        assert_eq!(pan(&l, [5.0, 0.0, 0.0]), 0.0, "zero right ⇒ centered");
        let e = emitter(1, [2.0, 0.0, 0.0], 1.0, Rolloff::Linear);
        for v in mix(&l, &[e], 4) {
            assert!(v.gain_l.is_finite() && v.gain_r.is_finite());
        }
    }

    #[test]
    fn deterministic() {
        let l = listener();
        let emitters = [
            emitter(1, [3.0, 0.0, 1.0], 0.9, Rolloff::Inverse),
            emitter(2, [-2.0, 1.0, 4.0], 0.7, Rolloff::Linear),
        ];
        assert_eq!(mix(&l, &emitters, 8), mix(&l, &emitters, 8));
        assert_eq!(mix(&l, &emitters, 0).len(), 0, "max_voices 0 ⇒ empty");
    }
}
