//! `sim::bending_feedback` — backlash: being wrong is costly.
//!
//! `resonance_bending_spec.docx v2.0` makes every discipline punish error
//! (Stormstrike: "being wrong about where the bolt goes is costly"). This
//! unifies that into one backlash model the player feels across disciplines:
//! a misprediction cost (generalising `bending_stormstrike::redirect_cost` to
//! any committed-vs-actual aim), an overdraw penalty for forcing un-charged
//! techniques, and a per-discipline mapping of error→hurt.
//!
//! Composition (caller/integration tier): `overdraw_backlash` takes an
//! `overdraw01` scalar the bending runtime sources from its charge/resonance
//! state; `discipline_backlash` takes the discipline id the runtime already
//! tracks. This module is pure math; it imports nothing render/charge-specific.
//!
//! Pure and deterministic: data in, data out. No GPU, IO, clock, or RNG; no
//! `.unwrap()`/`.expect()`. All inputs sanitised; outputs finite and in `[0,1]`.

/// What being wrong costs the bender. Every field is in `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Backlash {
    /// Damage dealt back to the bender.
    pub self_damage01: f32,
    /// How badly the bender is staggered / loses control.
    pub stagger01: f32,
    /// Fraction of resonance/charge bled away.
    pub resonance_loss01: f32,
}

impl Backlash {
    /// No backlash.
    pub const fn none() -> Backlash {
        Backlash { self_damage01: 0.0, stagger01: 0.0, resonance_loss01: 0.0 }
    }
}

#[inline]
fn san(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}
#[inline]
fn clamp01(x: f32) -> f32 {
    san(x).max(0.0).min(1.0)
}
#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let a = [san(a[0]), san(a[1]), san(a[2])];
    let len2 = dot(a, a);
    if len2 <= 1e-12 {
        return None;
    }
    let inv = 1.0 / len2.sqrt();
    Some([a[0] * inv, a[1] * inv, a[2] * inv])
}

/// Angular-error cost in `[0, 1]` between a committed aim and the actual
/// target: `(1 - cos θ) / 2`. `0` dead-on, `0.5` perpendicular, `1` opposite,
/// monotone rising with the angle. A degenerate (zero/non-finite) aim means
/// you committed to nothing — maximum cost `1.0`, never NaN.
///
/// This is the generalisation of `bending_stormstrike::redirect_cost` (same
/// formula) to any discipline's commit-vs-actual aim.
pub fn mispredict(committed_aim: [f32; 3], actual_target: [f32; 3]) -> f32 {
    match (normalize(committed_aim), normalize(actual_target)) {
        (Some(a), Some(b)) => (((1.0 - dot(a, b)) * 0.5) as f32).clamp(0.0, 1.0),
        _ => 1.0,
    }
}

/// Backlash from forcing un-charged techniques. `overdraw01` (clamped to
/// `[0,1]`) is how far past a safe charge the bender pushed. Every field rises
/// monotonically with overdraw and is `0` at `0`.
pub fn overdraw_backlash(overdraw01: f32) -> Backlash {
    let od = clamp01(overdraw01);
    Backlash {
        // forcing it a little stings, a lot wrecks you (quadratic)
        self_damage01: clamp01(od * od),
        // overdraw breaks your form (linear)
        stagger01: clamp01(od),
        // and bleeds up to half your resonance
        resonance_loss01: clamp01(0.5 * od),
    }
}

/// Per-discipline error cost. `mispredict01` (e.g. from [`mispredict`]) and
/// `power01` are clamped to `[0,1]`; severity `= mispredict01 * power01` (a
/// wrong commit hurts more the more power was behind it). Each field rises with
/// both inputs and is `0` when either is `0`. The split across fields is the
/// discipline's flavour:
///
/// - `0` Stormstrike — grounds the bolt on the bender: heavy self-damage + stagger.
/// - `1` Thermal Phase — burns/freezes the bender: dominant self-damage.
/// - `2` Heavy Brine — crushes inward: dominant stagger.
/// - other — a balanced backlash.
pub fn discipline_backlash(discipline: u8, mispredict01: f32, power01: f32) -> Backlash {
    let m = clamp01(mispredict01);
    let p = clamp01(power01);
    let sev = m * p; // in [0,1], monotone in each input

    let (sd, st, rl) = match discipline {
        0 => (1.0, 0.8, 0.3), // Stormstrike
        1 => (1.0, 0.4, 0.5), // Thermal Phase
        2 => (0.7, 1.0, 0.4), // Heavy Brine
        _ => (0.6, 0.6, 0.6), // balanced default
    };
    Backlash {
        self_damage01: clamp01(sd * sev),
        stagger01: clamp01(st * sev),
        resonance_loss01: clamp01(rl * sev),
    }
}

/// Combine two backlashes, saturating-adding each field and clamping to `[0,1]`.
pub fn combine(a: Backlash, b: Backlash) -> Backlash {
    Backlash {
        self_damage01: clamp01(clamp01(a.self_damage01) + clamp01(b.self_damage01)),
        stagger01: clamp01(clamp01(a.stagger01) + clamp01(b.stagger01)),
        resonance_loss01: clamp01(clamp01(a.resonance_loss01) + clamp01(b.resonance_loss01)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(b: Backlash) -> [f32; 3] {
        [b.self_damage01, b.stagger01, b.resonance_loss01]
    }
    fn all_in_range(b: Backlash) -> bool {
        fields(b).iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    }

    #[test]
    fn mispredict_endpoints_and_monotone() {
        let fwd = [1.0, 0.0, 0.0];
        assert!((mispredict(fwd, fwd) - 0.0).abs() < 1e-6, "dead-on -> 0");
        assert!((mispredict(fwd, [-1.0, 0.0, 0.0]) - 1.0).abs() < 1e-6, "opposite -> 1");
        assert!((mispredict(fwd, [0.0, 1.0, 0.0]) - 0.5).abs() < 1e-6, "perp -> 0.5");
        let mut prev = -1.0;
        for i in 0..=180 {
            let th = (i as f32).to_radians();
            let c = mispredict(fwd, [th.cos(), th.sin(), 0.0]);
            assert!(c.is_finite() && (0.0..=1.0).contains(&c));
            assert!(c + 1e-5 >= prev, "must rise with angular error");
            prev = c;
        }
        // degenerate / non-finite -> max cost, no NaN
        assert_eq!(mispredict([0.0, 0.0, 0.0], fwd), 1.0);
        assert!(mispredict(fwd, [f32::NAN, 0.0, 0.0]).is_finite());
        assert_eq!(mispredict(fwd, [f32::INFINITY, 0.0, 0.0]), 1.0);
    }

    #[test]
    fn overdraw_zero_is_none_and_monotone() {
        assert_eq!(overdraw_backlash(0.0), Backlash::none());
        let mut prev = fields(overdraw_backlash(0.0));
        let mut od = 0.0;
        while od <= 1.0 {
            let cur = fields(overdraw_backlash(od));
            for k in 0..3 {
                assert!(cur[k] >= prev[k] - 1e-7, "overdraw field {} not monotone", k);
            }
            assert!(all_in_range(overdraw_backlash(od)));
            prev = cur;
            od += 0.05;
        }
        // clamps for out-of-range overdraw
        assert!(all_in_range(overdraw_backlash(5.0)));
        assert!(all_in_range(overdraw_backlash(-3.0)));
        assert_eq!(overdraw_backlash(-3.0), Backlash::none());
    }

    #[test]
    fn discipline_monotone_in_mispredict_and_power() {
        for disc in 0..=4u8 {
            // zero when either input is zero
            assert_eq!(discipline_backlash(disc, 0.0, 1.0), Backlash::none());
            assert_eq!(discipline_backlash(disc, 1.0, 0.0), Backlash::none());
            // monotone rising in mispredict (fixed power)
            let mut prev = fields(discipline_backlash(disc, 0.0, 0.8));
            let mut m = 0.0;
            while m <= 1.0 {
                let cur = fields(discipline_backlash(disc, m, 0.8));
                for k in 0..3 {
                    assert!(cur[k] >= prev[k] - 1e-7, "disc {} not monotone in mispredict (field {})", disc, k);
                }
                prev = cur;
                m += 0.1;
            }
            // monotone rising in power (fixed mispredict)
            let mut prevp = fields(discipline_backlash(disc, 0.7, 0.0));
            let mut p = 0.0;
            while p <= 1.0 {
                let cur = fields(discipline_backlash(disc, 0.7, p));
                for k in 0..3 {
                    assert!(cur[k] >= prevp[k] - 1e-7, "disc {} not monotone in power (field {})", disc, k);
                }
                prevp = cur;
                p += 0.1;
            }
            assert!(all_in_range(discipline_backlash(disc, 1.0, 1.0)));
        }
    }

    #[test]
    fn combine_saturates_and_clamps() {
        let big = Backlash { self_damage01: 0.8, stagger01: 0.9, resonance_loss01: 0.7 };
        let c = combine(big, big);
        assert_eq!(c, Backlash { self_damage01: 1.0, stagger01: 1.0, resonance_loss01: 1.0 });
        // combine with none is identity (within clamp)
        assert_eq!(combine(big, Backlash::none()), big);
        // out-of-range inputs are clamped before adding
        let dirty = Backlash { self_damage01: 5.0, stagger01: -2.0, resonance_loss01: f32::NAN };
        let r = combine(dirty, Backlash::none());
        assert!(all_in_range(r));
        assert_eq!(r.self_damage01, 1.0);
        assert_eq!(r.stagger01, 0.0);
        assert_eq!(r.resonance_loss01, 0.0);
    }

    #[test]
    fn deterministic_and_seeded_sweep() {
        assert_eq!(discipline_backlash(0, 0.6, 0.7), discipline_backlash(0, 0.6, 0.7));
        assert_eq!(overdraw_backlash(0.3), overdraw_backlash(0.3));

        // deterministic seeded sweep (R20): all paths finite & in range
        let mut x: u64 = 0xBEAD_0066_u64;
        let mut next = || { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); (x >> 33) as u32 };
        let span = |v: u32, lo: f32, hi: f32| lo + (v % 1000) as f32 / 1000.0 * (hi - lo);
        for _ in 0..5000 {
            let aim = [span(next(), -2.0, 2.0), span(next(), -2.0, 2.0), span(next(), -2.0, 2.0)];
            let tgt = [span(next(), -2.0, 2.0), span(next(), -2.0, 2.0), span(next(), -2.0, 2.0)];
            let mp = mispredict(aim, tgt);
            assert!(mp.is_finite() && (0.0..=1.0).contains(&mp));
            let ob = overdraw_backlash(span(next(), -0.5, 1.5));
            let db = discipline_backlash((next() % 7) as u8, span(next(), -0.2, 1.2), span(next(), -0.2, 1.2));
            assert!(all_in_range(ob) && all_in_range(db));
            assert!(all_in_range(combine(ob, db)));
        }
    }
}
