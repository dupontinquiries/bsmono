//! Spatial mixer — the headless half of the audio upgrade.
//!
//! `sound.rs` synthesizes everything live and mixes it MONO: a bounce is as
//! loud behind you as in front, and a shot fired across the bay is as loud as
//! one at your feet. That is the single biggest reason the audio reads as
//! arcade. This module is the missing layer: given a listener pose and a set
//! of positioned emitters, decide per-voice stereo gains, drop what cannot be
//! heard, and report how muffled each source is by the geometry between.
//!
//! PORTED from `games/unendlich/crates/unendlich-engine/src/sim/`:
//! `sound_mix` (attenuation / equal-power pan / voice budget) and
//! `audio_occlusion` (per-unit dB attenuation + spectral muffle hint). That
//! code is pure, tested, and was written against a voxel world; the only
//! adaptation here is the occlusion probe — unendlich counts solid voxels on
//! the segment, blackstar marches its own SDF (`World::sdf`), which is why
//! the count is a f32 of solid DISTANCE rather than a u32 of blocks.
//!
//! Pure by construction: no cpal, no device, no clock, no RNG. `sound.rs`
//! owns the stream and consumes `Voice`s. That seam is what lets the mix be
//! tested headlessly in this sandbox, where there is no audio device at all.

use std::f32::consts::PI;

/// Distance attenuation curve between `min_dist` and `max_dist`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rolloff {
    /// Straight line from 1 → 0.
    Linear,
    /// Concave — faster initial falloff (`1/d`-like).
    Inverse,
    /// Steeper still (`1/d²`-like). The right default for gunfire and impacts
    /// in the open: real sound loses 6 dB per doubling of distance.
    InverseSquare,
}

/// The listener's pose. `forward`/`right` need not be normalized; a zero
/// vector is handled rather than producing NaN.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub pos: [f32; 3],
    pub forward: [f32; 3],
    pub right: [f32; 3],
}

/// A positioned sound source for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emitter {
    pub id: u32,
    pub pos: [f32; 3],
    pub volume01: f32,
    pub min_dist: f32,
    pub max_dist: f32,
    /// Weights the voice-budget cut. A boss's footfall should survive a cut
    /// that drops the twentieth piece of debris.
    pub priority: f32,
    pub rolloff: Rolloff,
    /// Solid world distance between listener and emitter, from
    /// [`occlusion_march`]. 0 = clear line.
    pub occluded_by: f32,
}

impl Emitter {
    /// A sensible in-world one-shot: audible to 120 units, inverse-square.
    pub fn shot(id: u32, pos: [f32; 3], volume01: f32) -> Emitter {
        Emitter {
            id,
            pos,
            volume01,
            min_dist: 3.0,
            max_dist: 120.0,
            priority: 1.0,
            rolloff: Rolloff::InverseSquare,
            occluded_by: 0.0,
        }
    }
}

/// A mixed voice: what `sound.rs` needs to render one emitter this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Voice {
    pub id: u32,
    pub gain_l: f32,
    pub gain_r: f32,
    /// One-pole low-pass cutoff in Hz. 20 kHz = open (no filtering needed).
    pub cutoff_hz: f32,
}

/// Shapes the falloff between min and max (higher = faster early drop).
const ROLL_K: f32 = 4.0;

// ---- occlusion constants (unendlich `audio_occlusion`, retuned for units) --
/// A fully-occluded source is muffled, not silenced — you feel a thud through
/// a bulkhead.
pub const OCCLUSION_FLOOR: f32 = 0.05;
/// Attenuation per world unit of solid material on the segment. unendlich used
/// 3 dB per voxel block; blackstar's units are metres and its hull plate is
/// thin, so the same 3 dB is charged per HALF metre of solid.
pub const DEFAULT_PER_UNIT_DB: f32 = 6.0;
/// Full (unoccluded) bandwidth for the muffle hint.
pub const FULL_BANDWIDTH_HZ: f32 = 20_000.0;
/// Lowest cutoff the muffle hint falls to.
pub const MUFFLE_FLOOR_HZ: f32 = 250.0;
/// Fraction of the cutoff surviving each world unit of solid.
pub const MUFFLE_SURVIVE_PER_UNIT: f32 = 0.45;

fn clamp01(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
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

/// Distance attenuation in `[0,1]`: `1` at/inside `min_dist`, `0` at/beyond
/// `max_dist`, monotonically non-increasing between.
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
        return 0.0;
    }
    let t = ((d - lo) / (hi - lo)).clamp(0.0, 1.0);
    let base = (1.0 - t) / (1.0 + ROLL_K * t);
    let g = match e.rolloff {
        Rolloff::Linear => 1.0 - t,
        Rolloff::Inverse => base,
        Rolloff::InverseSquare => base * base,
    };
    clamp01(g)
}

/// Stereo pan in `[-1, 1]`: `-1` hard left, `+1` hard right, `~0` dead ahead.
pub fn pan(l: &Listener, emitter_pos: [f32; 3]) -> f32 {
    let dir = [
        emitter_pos[0] - l.pos[0],
        emitter_pos[1] - l.pos[1],
        emitter_pos[2] - l.pos[2],
    ];
    match (normalized(dir), normalized(l.right)) {
        (Some(du), Some(rn)) => dot(du, rn).clamp(-1.0, 1.0),
        _ => 0.0,
    }
}

/// Gain multiplier for `solid` world units of material on the segment.
///
/// Exactly `1.0` at zero (no float drift on the common clear-line case),
/// floored at [`OCCLUSION_FLOOR`], always finite. A non-finite or negative
/// `per_unit_db` is treated as no attenuation — occlusion must never amplify.
pub fn occlusion_gain(solid: f32, per_unit_db: f32) -> f32 {
    if !(solid > 0.0) {
        return 1.0;
    }
    let per = if per_unit_db.is_finite() { per_unit_db.max(0.0) } else { 0.0 };
    let total_db = per * solid.min(64.0);
    let g = 10f32.powf(-total_db / 20.0);
    if g.is_finite() {
        g.clamp(OCCLUSION_FLOOR, 1.0)
    } else {
        OCCLUSION_FLOOR
    }
}

/// Low-pass cutoff hint for a source behind `solid` units of material.
/// Occlusion is spectral, not just level: a bulkhead eats highs first.
pub fn muffle_cutoff_hz(solid: f32) -> f32 {
    if !(solid > 0.0) {
        return FULL_BANDWIDTH_HZ;
    }
    let c = FULL_BANDWIDTH_HZ * MUFFLE_SURVIVE_PER_UNIT.powf(solid.min(64.0));
    if c.is_finite() {
        c.max(MUFFLE_FLOOR_HZ)
    } else {
        MUFFLE_FLOOR_HZ
    }
}

/// Mix emitters into at most `max_voices` voices, keeping the most audible
/// (`audibility = attenuation · volume · priority · occlusion`).
///
/// Each kept voice is equal-power panned, so a source sweeping past the
/// listener holds constant perceived loudness instead of dipping in the
/// middle: `gain_l² + gain_r² == gain²`.
pub fn mix(l: &Listener, emitters: &[Emitter], max_voices: usize) -> Vec<Voice> {
    let mut scored: Vec<(usize, f32)> = Vec::new();
    for (i, e) in emitters.iter().enumerate() {
        let att = attenuation(e, l.pos);
        let vol = clamp01(e.volume01);
        let occ = occlusion_gain(e.occluded_by, DEFAULT_PER_UNIT_DB);
        if vol > 0.0 && att > 0.0 {
            // Occlusion weights the CUT as well as the gain: a shot through a
            // bulkhead should lose its seat to one in the room with you.
            let aud = att * vol * e.priority.max(0.0) * occ;
            if aud > 0.0 {
                scored.push((i, aud));
            }
        }
    }
    // most audible first; deterministic tie-break by emitter id
    scored.sort_by(|a, b| {
        b.1.total_cmp(&a.1).then_with(|| emitters[a.0].id.cmp(&emitters[b.0].id))
    });
    scored.truncate(max_voices);

    scored
        .into_iter()
        .map(|(idx, _)| {
            let e = &emitters[idx];
            let gain = attenuation(e, l.pos)
                * clamp01(e.volume01)
                * occlusion_gain(e.occluded_by, DEFAULT_PER_UNIT_DB);
            let p = pan(l, e.pos);
            // equal-power pan law: θ ∈ [0, π/2] as p goes -1 → +1
            let theta = (p + 1.0) * 0.5 * (PI * 0.5);
            Voice {
                id: e.id,
                gain_l: gain * theta.cos(),
                gain_r: gain * theta.sin(),
                cutoff_hz: muffle_cutoff_hz(e.occluded_by),
            }
        })
        .collect()
}

/// March a straight segment and total the SOLID distance crossed.
///
/// unendlich counted solid voxels; blackstar has no voxel grid, so this
/// samples the world SDF at a fixed step and sums the steps that land inside
/// geometry. `steps` bounds the cost: this runs per-emitter per-frame, never
/// per-sample, and occlusion changes slowly enough to re-march every N frames.
///
/// Generic over the probe so it is testable with a synthetic wall and has no
/// dependency on `World` — the pure/glue seam this module exists to hold.
pub fn occlusion_march<F: FnMut([f32; 3]) -> f32>(
    from: [f32; 3],
    to: [f32; 3],
    steps: u32,
    mut sdf: F,
) -> f32 {
    let n = steps.clamp(1, 256);
    let seg = dist(from, to);
    if !seg.is_finite() || seg <= 1e-6 {
        return 0.0;
    }
    let step = seg / n as f32;
    let dir = [
        (to[0] - from[0]) / seg,
        (to[1] - from[1]) / seg,
        (to[2] - from[2]) / seg,
    ];
    let mut solid = 0.0;
    for i in 0..n {
        // sample at the middle of each step, so a step is counted as solid
        // when most of it is inside rather than when its leading edge is
        let t = (i as f32 + 0.5) * step;
        let p = [
            from[0] + dir[0] * t,
            from[1] + dir[1] * t,
            from[2] + dir[2] * t,
        ];
        let d = sdf(p);
        if d.is_finite() && d < 0.0 {
            solid += step;
        }
    }
    solid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listener() -> Listener {
        Listener { pos: [0.0, 0.0, 0.0], forward: [0.0, 0.0, 1.0], right: [1.0, 0.0, 0.0] }
    }

    #[test]
    fn attenuation_is_bounded_and_monotone_for_every_rolloff() {
        for roll in [Rolloff::Linear, Rolloff::Inverse, Rolloff::InverseSquare] {
            let mut e = Emitter::shot(0, [0.0, 0.0, 0.0], 1.0);
            e.min_dist = 1.0;
            e.max_dist = 10.0;
            e.rolloff = roll;
            assert_eq!(attenuation(&e, [1.0, 0.0, 0.0]), 1.0, "at min_dist");
            assert_eq!(attenuation(&e, [10.0, 0.0, 0.0]), 0.0, "at max_dist");
            let mut prev = 2.0f32;
            let mut d = 0.0;
            while d <= 12.0 {
                let a = attenuation(&e, [d, 0.0, 0.0]);
                assert!((0.0..=1.0).contains(&a), "{roll:?} out of range at {d}: {a}");
                assert!(a <= prev + 1e-6, "{roll:?} rose at {d}");
                prev = a;
                d += 0.1;
            }
        }
    }

    #[test]
    fn a_source_sweeping_past_holds_constant_power_and_actually_moves() {
        // The property equal-power pan exists for: a sound crossing in front
        // of the listener must hold constant perceived loudness AND arrive
        // from the right place.
        //
        // The power assertion ALONE is not enough, and this test was briefly
        // written that way: `power == gain²` is satisfied by dumping every
        // sample into the left channel (theta = 0), so it passed against a
        // pan that did not pan. It has to assert the balance MOVES with the
        // source, which is the mechanism the pan law is here for.
        let l = listener();
        let mut balance = Vec::new();
        for i in 0..=32 {
            let ang = (i as f32 / 32.0) * PI - PI * 0.5; // hard left → hard right
            let e = Emitter::shot(0, [ang.sin() * 5.0, 0.0, ang.cos() * 5.0], 1.0);
            let v = mix(&l, &[e], 8);
            assert_eq!(v.len(), 1);
            let power = v[0].gain_l * v[0].gain_l + v[0].gain_r * v[0].gain_r;
            let gain = attenuation(&e, l.pos);
            assert!(
                (power - gain * gain).abs() < 1e-4,
                "power {power} != gain² {} at angle {ang}",
                gain * gain
            );
            balance.push(v[0].gain_r - v[0].gain_l);
        }
        // left-to-right sweep ⇒ the balance rises monotonically from
        // negative (left) to positive (right), crossing zero in the middle
        assert!(balance[0] < -0.5, "the leftmost source was not on the left: {}", balance[0]);
        assert!(
            *balance.last().expect("swept") > 0.5,
            "the rightmost source was not on the right"
        );
        assert!(
            balance[balance.len() / 2].abs() < 0.1,
            "the source dead ahead was not centred: {}",
            balance[balance.len() / 2]
        );
        for w in balance.windows(2) {
            assert!(w[1] >= w[0] - 1e-4, "the pan went backwards: {} then {}", w[0], w[1]);
        }
    }

    #[test]
    fn pan_puts_left_left_and_right_right() {
        let l = listener(); // right = +x
        assert!(pan(&l, [10.0, 0.0, 0.0]) > 0.9, "a source on +x must pan right");
        assert!(pan(&l, [-10.0, 0.0, 0.0]) < -0.9, "a source on -x must pan left");
        assert!(pan(&l, [0.0, 0.0, 10.0]).abs() < 1e-4, "dead ahead must be centred");
        // degenerate listener must not produce NaN
        let bad = Listener { pos: [0.0; 3], forward: [0.0; 3], right: [0.0; 3] };
        assert_eq!(pan(&bad, [1.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn the_voice_budget_keeps_the_audible_and_drops_the_rest() {
        let l = listener();
        // twelve identical shots at increasing distance; keep four
        let ems: Vec<Emitter> = (0..12)
            .map(|i| Emitter::shot(i, [0.0, 0.0, 4.0 + i as f32 * 8.0], 1.0))
            .collect();
        let v = mix(&l, &ems, 4);
        assert_eq!(v.len(), 4, "budget not honoured");
        let kept: Vec<u32> = v.iter().map(|x| x.id).collect();
        assert_eq!(kept, vec![0, 1, 2, 3], "kept the far ones over the near ones");
        // and nothing beyond max_dist survives at all
        let far = Emitter::shot(99, [0.0, 0.0, 5000.0], 1.0);
        assert!(mix(&l, &[far], 8).is_empty(), "an inaudible source took a voice");
    }

    #[test]
    fn occlusion_reduces_gain_and_bandwidth_together_and_never_amplifies() {
        assert_eq!(occlusion_gain(0.0, DEFAULT_PER_UNIT_DB), 1.0, "clear line must be exactly 1.0");
        assert_eq!(muffle_cutoff_hz(0.0), FULL_BANDWIDTH_HZ);
        let mut prev_g = 1.0f32;
        let mut prev_c = FULL_BANDWIDTH_HZ;
        let mut s = 0.25;
        while s <= 20.0 {
            let g = occlusion_gain(s, DEFAULT_PER_UNIT_DB);
            let c = muffle_cutoff_hz(s);
            assert!(g <= prev_g + 1e-9 && g >= OCCLUSION_FLOOR - 1e-9, "gain {g} at {s}");
            assert!(c <= prev_c + 1e-3 && c >= MUFFLE_FLOOR_HZ - 1e-3, "cutoff {c} at {s}");
            prev_g = g;
            prev_c = c;
            s += 0.25;
        }
        // a hostile per-unit value must never make a sound louder
        for bad in [-5.0f32, f32::NAN, f32::INFINITY] {
            assert!(occlusion_gain(3.0, bad) <= 1.0, "per_unit_db {bad} amplified");
        }
    }

    #[test]
    fn a_wall_between_costs_the_source_its_voice() {
        // The gameplay property: two identical shots, one behind a bulkhead.
        // The occluded one must be quieter AND darker AND lose a budget fight.
        let l = listener();
        let clear = Emitter::shot(0, [0.0, 0.0, 20.0], 1.0);
        let mut walled = Emitter::shot(1, [0.0, 0.0, 18.0], 1.0); // NEARER
        walled.occluded_by = 2.0;
        let v = mix(&l, &[clear, walled], 1);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, 0, "the nearer source behind a wall beat the clear one");
        let both = mix(&l, &[clear, walled], 2);
        let c = both.iter().find(|v| v.id == 0).expect("clear voice");
        let w = both.iter().find(|v| v.id == 1).expect("walled voice");
        let cp = c.gain_l * c.gain_l + c.gain_r * c.gain_r;
        let wp = w.gain_l * w.gain_l + w.gain_r * w.gain_r;
        assert!(wp < cp, "the walled source was not quieter: {wp} vs {cp}");
        assert!(w.cutoff_hz < c.cutoff_hz, "the walled source was not muffled");
    }

    #[test]
    fn the_march_measures_solid_distance_and_ignores_open_space() {
        // synthetic wall: solid for z in [10, 12]
        let wall = |p: [f32; 3]| if (10.0..12.0).contains(&p[2]) { -1.0 } else { 1.0 };
        let solid = occlusion_march([0.0, 0.0, 0.0], [0.0, 0.0, 30.0], 128, wall);
        assert!((solid - 2.0).abs() < 0.3, "expected ~2 units of wall, got {solid}");
        // clear line reads exactly zero, and a zero-length segment is safe
        let open = occlusion_march([0.0, 0.0, 0.0], [0.0, 0.0, 9.0], 64, wall);
        assert_eq!(open, 0.0, "open air measured as solid");
        assert_eq!(occlusion_march([1.0; 3], [1.0; 3], 64, wall), 0.0, "zero-length segment");
        // a source INSIDE the wall is fully occluded, not accidentally clear
        let inside = occlusion_march([0.0, 0.0, 11.0], [0.0, 0.0, 30.0], 128, wall);
        assert!(inside > 0.5, "a listener inside solid read as clear: {inside}");
    }
}
