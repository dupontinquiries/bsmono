//! Motion profiles: HOW the lens travels from A to B, as distinct from
//! whether it is allowed to ([`crate::Focus`]).
//!
//! The shape matters for three separate reasons, and they happen to agree:
//!
//! **Noise.** Coil force follows current change, so what you hear is
//! acceleration, not speed. A step demands infinite acceleration and gets a
//! tick; a profile that begins and ends with zero acceleration is close to
//! silent even when it covers the same distance in the same time.
//!
//! **Settling.** A lens on a spring rings after an abrupt stop. Frames taken
//! during that ring are soft in a way no stabilisation fixes, because the
//! focal plane itself is moving. Ending at zero velocity AND zero
//! acceleration is what makes "the move is done" true rather than nominal.
//!
//! **Looks.** On video, a focus pull that steps or hunts reads as cheap. The
//! same minimum-jerk curve a camera operator turns by hand is what a follow
//! focus produces mechanically.
//!
//! So [`Shape::MinimumJerk`] is the default for everything that matters, and
//! the others exist to be compared against it.
//!
//! ## Focus bracketing
//!
//! [`bracket`] plans a burst at several focus distances, for depth from
//! focus or for stacking. Two things it gets right that a naive loop does
//! not:
//!
//! - **Spacing.** Defocus blur scales with the reciprocal of distance, and a
//!   VCM's displacement is roughly linear in drive current, so evenly spaced
//!   DAC codes are approximately evenly spaced in dioptres — which is the
//!   spacing depth-from-focus wants. Even spacing in *metres* would waste
//!   most of the burst on the far field.
//! - **Direction.** A spring-and-coil actuator has real hysteresis: the code
//!   that puts the lens at a given distance differs depending on whether you
//!   arrived from below or above. A bracket therefore always travels one
//!   way, and always approaches its first stop from the same side, so the
//!   frames are comparable to each other and across bursts.

/// How to get from A to B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// 10t³ − 15t⁴ + 6t⁵. Zero velocity and zero acceleration at both ends.
    /// Quietest, settles fastest, and the one to use unless testing.
    MinimumJerk,
    /// 3t² − 2t³. Zero velocity at the ends, but acceleration jumps at the
    /// start and stop — audible as a soft tick at each end.
    Smoothstep,
    /// Constant velocity. Acceleration is impulsive at both ends: two clear
    /// ticks. Useful as the thing to compare against.
    Linear,
}

impl Shape {
    /// Fraction of the distance covered at fraction `t` of the duration.
    pub fn at(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Shape::MinimumJerk => t * t * t * (10.0 + t * (-15.0 + 6.0 * t)),
            Shape::Smoothstep => t * t * (3.0 - 2.0 * t),
            Shape::Linear => t,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// DAC codes per second. The cap that makes a pull look deliberate
    /// rather than snapped.
    pub max_velocity: f32,
    /// Samples per second sent to the actuator.
    pub rate_hz: f32,
    /// After the last sample, how long before the lens is trusted to be
    /// still. Measure this per device; the default is a guess and is
    /// deliberately generous.
    pub settle_ms: u64,
}

impl Default for Limits {
    /// Stills and autofocus: crossing the full 1023-code range takes about
    /// half a second at the peak, which is the order a phone AF sweep runs
    /// at. The cap exists to refuse the absurd, not to slow ordinary moves.
    fn default() -> Limits {
        Limits { max_velocity: 4_000.0, rate_hz: 200.0, settle_ms: 60 }
    }
}

impl Limits {
    /// Video: a pull you can watch. Roughly a third of the speed, and a
    /// longer settle, because on video a residual ring is visible as
    /// breathing rather than merely soft.
    pub fn video() -> Limits {
        Limits { max_velocity: 1_200.0, rate_hz: 200.0, settle_ms: 120 }
    }

    /// Burst: as fast as the actuator will go quietly, since the whole
    /// bracket has to fit inside a plausible shutter press.
    pub fn burst() -> Limits {
        Limits { max_velocity: 6_000.0, rate_hz: 250.0, settle_ms: 40 }
    }
}

/// One commanded position at one moment, relative to the start of the move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub t_ms: u64,
    pub pos: u16,
}

/// A planned move: the samples to write, and when it is safe to expose.
#[derive(Debug, Clone, PartialEq)]
pub struct Move {
    pub samples: Vec<Sample>,
    /// Duration of the motion itself.
    pub duration_ms: u64,
    /// `duration_ms` plus settling — the earliest a frame is trustworthy.
    pub ready_ms: u64,
}

/// Plan a move. `want_ms` is the duration asked for; it is stretched if that
/// would exceed [`Limits::max_velocity`], because a pull that is too fast to
/// be quiet is worse than one that takes longer.
pub fn plan(from: u16, to: u16, want_ms: u64, shape: Shape, lim: &Limits) -> Move {
    let dist = (to as f32 - from as f32).abs();
    // Peak velocity of a minimum-jerk profile is 1.875 d/T; smoothstep 1.5;
    // linear 1.0. Stretch the duration so the PEAK obeys the limit, not the
    // average — the average is not what the actuator feels.
    let peak_factor = match shape {
        Shape::MinimumJerk => 1.875,
        Shape::Smoothstep => 1.5,
        Shape::Linear => 1.0,
    };
    let min_ms = if lim.max_velocity > 0.0 { (dist * peak_factor / lim.max_velocity * 1000.0).ceil() as u64 } else { want_ms };
    let duration_ms = want_ms.max(min_ms).max(1);
    let n = ((duration_ms as f32 / 1000.0) * lim.rate_hz).round().max(1.0) as u64;
    let mut samples = Vec::with_capacity(n as usize + 1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let p = from as f32 + (to as f32 - from as f32) * shape.at(t);
        samples.push(Sample { t_ms: duration_ms * i / n, pos: p.round().clamp(0.0, u16::MAX as f32) as u16 });
    }
    // The endpoint must be exact: rounding that leaves the lens one code
    // short means the next move starts from somewhere you did not record.
    if let Some(last) = samples.last_mut() {
        last.pos = to;
        last.t_ms = duration_ms;
    }
    Move { samples, duration_ms, ready_ms: duration_ms + lim.settle_ms }
}

/// One stop in a focus bracket.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    pub pos: u16,
    /// When to move (start of the travel to this stop).
    pub move_at_ms: u64,
    /// When this frame may be exposed: the lens is there and has settled.
    pub expose_at_ms: u64,
}

/// Plan a burst across `frames` focus positions from `near` to `far`.
///
/// `run_up` is how far below the first stop to start, so that stop is
/// approached in the same direction as every later one — hysteresis makes an
/// unapproached first frame disagree with the rest.
pub fn bracket(near: u16, far: u16, frames: usize, per_move_ms: u64, run_up: u16, lim: &Limits) -> Vec<Stop> {
    if frames == 0 {
        return Vec::new();
    }
    let (lo, hi) = if near <= far { (near, far) } else { (far, near) };
    let start = lo.saturating_sub(run_up);
    let mut out = Vec::with_capacity(frames);
    let mut t = 0u64;
    let mut cur = start;
    for i in 0..frames {
        // Even spacing in DAC codes, which is approximately even in dioptres.
        let pos = if frames == 1 { lo } else { lo + (((hi - lo) as u64 * i as u64) / (frames - 1) as u64) as u16 };
        let m = plan(cur, pos, per_move_ms, Shape::MinimumJerk, lim);
        out.push(Stop { pos, move_at_ms: t, expose_at_ms: t + m.ready_ms });
        t += m.ready_ms;
        cur = pos;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_jerk_starts_and_stops_gently() {
        // What you hear is acceleration, so the test is about the ENDS.
        let m = plan(0, 1000, 500, Shape::MinimumJerk, &Limits::default());
        let s = &m.samples;
        let first = s[1].pos - s[0].pos;
        let mid = s[s.len() / 2].pos as i32 - s[s.len() / 2 - 1].pos as i32;
        let last = s[s.len() - 1].pos - s[s.len() - 2].pos;
        assert!(first < 2, "leaves rest almost imperceptibly, moved {first}");
        assert!(last < 2, "arrives the same way, moved {last}");
        assert!(mid > 10, "and does the work in the middle, {mid}");
        // Linear, for contrast: the same step at every point including the ends.
        let l = plan(0, 1000, 500, Shape::Linear, &Limits::default());
        let lf = l.samples[1].pos - l.samples[0].pos;
        assert!(lf >= 9, "linear slams straight to full speed, {lf}");
    }

    #[test]
    fn endpoints_are_exact_and_monotonic_both_ways() {
        for (a, b) in [(0u16, 1023u16), (1023, 0), (400, 600), (600, 400)] {
            let m = plan(a, b, 300, Shape::MinimumJerk, &Limits::default());
            assert_eq!(m.samples.first().unwrap().pos, a);
            assert_eq!(m.samples.last().unwrap().pos, b, "must land exactly on target");
            assert_eq!(m.samples.last().unwrap().t_ms, m.duration_ms);
            let up = b > a;
            for w in m.samples.windows(2) {
                if up {
                    assert!(w[1].pos >= w[0].pos, "no backtracking");
                } else {
                    assert!(w[1].pos <= w[0].pos, "no backtracking");
                }
                assert!(w[1].t_ms > w[0].t_ms || w[1].t_ms == w[0].t_ms);
            }
        }
    }

    #[test]
    fn too_fast_a_pull_is_stretched_rather_than_rushed() {
        let lim = Limits { max_velocity: 1_000.0, ..Limits::default() };
        // 1000 codes asked for in 100 ms peaks at ~18 750 codes/s.
        let m = plan(0, 1000, 100, Shape::MinimumJerk, &lim);
        assert!(m.duration_ms >= 1_875, "stretched to respect peak velocity, got {}", m.duration_ms);
        // The peak step, converted to codes per second, obeys the limit.
        let dt = 1000.0 / lim.rate_hz;
        let peak = m.samples.windows(2).map(|w| (w[1].pos as f32 - w[0].pos as f32).abs()).fold(0.0, f32::max);
        assert!(peak / dt * 1000.0 <= lim.max_velocity * 1.05, "peak velocity {}", peak / dt * 1000.0);
        // A move that already fits keeps the duration it asked for.
        let easy = plan(500, 520, 200, Shape::MinimumJerk, &lim);
        assert_eq!(easy.duration_ms, 200);
    }

    #[test]
    fn a_frame_is_only_trustworthy_after_settling() {
        let lim = Limits { settle_ms: 80, ..Limits::default() };
        let m = plan(0, 300, 200, Shape::MinimumJerk, &lim);
        assert_eq!(m.ready_ms, m.duration_ms + 80, "exposing at duration_ms catches the ring");
    }

    #[test]
    fn a_bracket_travels_one_way_and_approaches_its_first_stop() {
        let lim = Limits::default();
        let b = bracket(200, 800, 5, 60, 40, &lim);
        assert_eq!(b.len(), 5);
        assert_eq!(b[0].pos, 200, "first stop is the near end");
        assert_eq!(b[4].pos, 800, "last is the far end");
        for w in b.windows(2) {
            assert!(w[1].pos > w[0].pos, "monotonic, so hysteresis is consistent");
            assert!(w[1].move_at_ms >= w[0].expose_at_ms, "never moves before the previous frame is taken");
        }
        // Even spacing in DAC codes ~ even in dioptres, which is the point.
        let gaps: Vec<i32> = b.windows(2).map(|w| w[1].pos as i32 - w[0].pos as i32).collect();
        assert!(gaps.iter().all(|g| (*g - gaps[0]).abs() <= 1), "evenly spaced: {gaps:?}");
        // Reversing near/far gives the same plan: direction is the policy's,
        // not the caller's.
        assert_eq!(bracket(800, 200, 5, 60, 40, &lim), b);
    }

    #[test]
    fn a_bracket_fits_in_a_predictable_time() {
        let lim = Limits { settle_ms: 50, ..Limits::default() };
        let _ = &lim;
        let b = bracket(0, 1023, 8, 40, 0, &Limits { settle_ms: 50, ..Limits::burst() });
        let total = b.last().unwrap().expose_at_ms;
        assert!(total < 2_000, "8 frames in under 2 s, got {total} ms");
        assert!(b[0].expose_at_ms >= 50, "even the first frame waits for settling");
        assert!(bracket(0, 100, 0, 40, 0, &lim).is_empty());
        // Video limits make the same bracket slower, by design.
        let slow = bracket(0, 1023, 8, 40, 0, &Limits::video());
        assert!(slow.last().unwrap().expose_at_ms > total * 2, "video pacing is deliberately slower");
        assert_eq!(bracket(0, 100, 1, 40, 0, &lim).len(), 1);
    }
}
