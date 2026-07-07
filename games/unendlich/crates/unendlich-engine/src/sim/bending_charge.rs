//! Resonance — the bending **resource** model (r66-02). Per
//! `resonance_bending_spec.docx v2.0`: *no chi bar, no cooldown.* Techniques are
//! gated by **resonance**, a charge that builds from sustained, correct,
//! on-rhythm bending and is spent by techniques. Forcing a technique you
//! haven't charged is allowed but incurs an **overdraw** debt that recovers
//! slowly — the cost of impatience, replacing the old cooldown timer entirely.
//!
//! This is the resource the whole discipline set sits on (r66-01 reads
//! `power01` from [`Resonance::spend`]). Pure & deterministic: dt/inputs in,
//! state out. No clock, RNG, I/O, GPU; no `unwrap`/`expect`/`panic!`; charge
//! stays in `[0,1]` and overdraw in `[0,1]` for any input (incl. NaN/∞/huge dt).

/// Charge accrued per second at full on-rhythm bending (`on_rhythm01 = 1`).
pub const BUILD_RATE: f32 = 0.5; // ~2 s of clean bending to fully charge
/// Idle charge bleed per second (resonance fades when not maintained).
pub const CHARGE_BLEED: f32 = 0.05;
/// Overdraw debt recovered per second (slow — forcing techniques lingers).
pub const OVERDRAW_RECOVERY: f32 = 0.10;
/// Debt above which the bender counts as overdrawn (small tolerance).
pub const OVERDRAW_THRESHOLD: f32 = 0.05;

/// The resonance resource. `charge01` is available resonance `[0,1]`;
/// `overdraw01` is accumulated debt `[0,1]` from forcing uncharged techniques.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resonance {
    pub charge01: f32,
    pub overdraw01: f32,
}

/// Finite-or-0, then clamp to `[0,1]`.
#[inline]
fn c01(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}
/// Finite, non-negative dt (NaN/∞ → 0 so a bad frame is a no-op, never a jump).
#[inline]
fn dt_ok(dt: f32) -> f32 {
    if dt.is_finite() {
        dt.max(0.0)
    } else {
        0.0
    }
}

impl Resonance {
    /// Empty resonance (no charge, no debt).
    pub fn new() -> Resonance {
        Resonance { charge01: 0.0, overdraw01: 0.0 }
    }

    /// Fully charged, no debt.
    pub fn full() -> Resonance {
        Resonance { charge01: 1.0, overdraw01: 0.0 }
    }

    /// Accrue charge from on-rhythm/correct bending this step. Rate scales with
    /// `on_rhythm01 ∈ [0,1]` (sloppy/off-rhythm bending barely builds). Capped
    /// at 1.0; never decreases here.
    pub fn build(&mut self, on_rhythm01: f32, dt: f32) {
        self.build_scaled(on_rhythm01, dt, 1.0);
    }

    /// Like [`build`](Self::build) but scales the gain by `gain_mul` (e.g. a
    /// focus item's `resonance_gain_mul`). Non-finite/negative multipliers are
    /// treated as `0` so a bad modifier can't add charge. Still capped at 1.0.
    pub fn build_scaled(&mut self, on_rhythm01: f32, dt: f32, gain_mul: f32) {
        let r = c01(on_rhythm01);
        let mul = if gain_mul.is_finite() { gain_mul.max(0.0) } else { 0.0 };
        let gained = BUILD_RATE * r * dt_ok(dt) * mul;
        self.charge01 = (self.charge01 + gained).clamp(0.0, 1.0);
    }

    /// Spend up to `cost01` of charge on a technique, returning the realized
    /// `power01`.
    ///
    /// - Within budget (`cost ≤ charge`): full power (`= cost`), charge drops by
    ///   `cost`.
    /// - Over budget: the technique still fires at **reduced** power (only the
    ///   charge that was available), charge goes to 0, and the shortfall is
    ///   added to `overdraw` — forcing it costs you.
    pub fn spend(&mut self, cost01: f32) -> f32 {
        let cost = c01(cost01);
        if cost <= self.charge01 {
            self.charge01 -= cost;
            cost
        } else {
            let realized = self.charge01; // only what was available
            let shortfall = cost - realized;
            self.charge01 = 0.0;
            self.overdraw01 = (self.overdraw01 + shortfall).clamp(0.0, 1.0);
            realized
        }
    }

    /// Time passes: overdraw debt recovers slowly and idle charge bleeds. Both
    /// stay within `[0,1]` (never negative).
    pub fn decay(&mut self, dt: f32) {
        let d = dt_ok(dt);
        self.overdraw01 = (self.overdraw01 - OVERDRAW_RECOVERY * d).clamp(0.0, 1.0);
        self.charge01 = (self.charge01 - CHARGE_BLEED * d).clamp(0.0, 1.0);
    }

    /// Whether the bender is currently overdrawn (debt past the tolerance).
    pub fn is_overdrawn(&self) -> bool {
        self.overdraw01 > OVERDRAW_THRESHOLD
    }
}

impl Default for Resonance {
    fn default() -> Self {
        Resonance::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_and_full_endpoints() {
        assert_eq!(Resonance::new(), Resonance { charge01: 0.0, overdraw01: 0.0 });
        assert_eq!(Resonance::full(), Resonance { charge01: 1.0, overdraw01: 0.0 });
        assert!(!Resonance::new().is_overdrawn());
    }

    #[test]
    fn build_raises_charge_capped_and_faster_on_rhythm() {
        let mut clean = Resonance::new();
        clean.build(1.0, 0.5);
        let mut sloppy = Resonance::new();
        sloppy.build(0.3, 0.5);
        assert!(clean.charge01 > sloppy.charge01, "on-rhythm builds faster");
        assert!(clean.charge01 > 0.0);
        // capped at 1.0 over a long, clean build
        let mut m = Resonance::new();
        for _ in 0..100 {
            m.build(1.0, 0.5);
        }
        assert_eq!(m.charge01, 1.0, "charge caps at 1.0");
        // off-rhythm (0) builds nothing
        let mut z = Resonance::new();
        z.build(0.0, 1.0);
        assert_eq!(z.charge01, 0.0);
    }

    #[test]
    fn spend_within_budget_full_power_lowers_charge() {
        let mut r = Resonance::full();
        let power = r.spend(0.4);
        assert!((power - 0.4).abs() < 1e-6, "full power within budget");
        assert!((r.charge01 - 0.6).abs() < 1e-6);
        assert_eq!(r.overdraw01, 0.0, "no debt within budget");
        assert!(!r.is_overdrawn());
    }

    #[test]
    fn spend_over_budget_reduced_power_raises_overdraw() {
        let mut r = Resonance { charge01: 0.3, overdraw01: 0.0 };
        let power = r.spend(0.8); // forcing it
        assert!((power - 0.3).abs() < 1e-6, "reduced to available charge");
        assert!(power < 0.8, "less than requested");
        assert_eq!(r.charge01, 0.0, "charge drained");
        assert!((r.overdraw01 - 0.5).abs() < 1e-6, "shortfall became debt");
        assert!(r.is_overdrawn());
    }

    #[test]
    fn decay_recovers_overdraw_and_bleeds_charge_within_range() {
        let mut r = Resonance { charge01: 0.8, overdraw01: 0.6 };
        let od0 = r.overdraw01;
        let ch0 = r.charge01;
        r.decay(1.0);
        assert!(r.overdraw01 < od0, "overdraw recovers over time");
        assert!(r.charge01 < ch0, "idle charge bleeds");
        // long decay clears debt and charge to the floor, never negative
        for _ in 0..1000 {
            r.decay(1.0);
        }
        assert_eq!(r.overdraw01, 0.0);
        assert_eq!(r.charge01, 0.0);
        assert!(!r.is_overdrawn());
    }

    #[test]
    fn charge_and_overdraw_stay_in_range_under_hostile_input() {
        let mut r = Resonance::new();
        // garbage dt / rhythm must not corrupt state
        r.build(f32::NAN, f32::INFINITY);
        r.build(5.0, -3.0);
        r.build(2.0, 1e30);
        let _ = r.spend(f32::NAN);
        let _ = r.spend(-1.0);
        let _ = r.spend(1e30);
        r.decay(f32::NAN);
        r.decay(-10.0);
        assert!((0.0..=1.0).contains(&r.charge01), "charge {} out of range", r.charge01);
        assert!((0.0..=1.0).contains(&r.overdraw01), "overdraw {} out of range", r.overdraw01);
        assert!(r.charge01.is_finite() && r.overdraw01.is_finite());
    }

    #[test]
    fn overdraw_clamps_at_one() {
        let mut r = Resonance::new();
        for _ in 0..10 {
            r.spend(1.0); // each forces a full-cost shortfall onto an empty bar
        }
        assert!(r.overdraw01 <= 1.0 && r.overdraw01 > OVERDRAW_THRESHOLD);
    }

    #[test]
    fn realized_power_never_exceeds_request_or_charge() {
        let mut r = Resonance { charge01: 0.5, overdraw01: 0.0 };
        let p = r.spend(0.2);
        assert!(p <= 0.2 + 1e-6 && p <= 0.5 + 1e-6);
        assert!((0.0..=1.0).contains(&p));
    }

    #[test]
    fn deterministic() {
        let run = || {
            let mut r = Resonance::new();
            let mut log = Vec::new();
            for i in 0..20 {
                r.build(0.7, 0.1);
                let p = r.spend(0.15);
                r.decay(0.05);
                log.push((p, r.charge01, r.overdraw01, r.is_overdrawn()));
                let _ = i;
            }
            log
        };
        let first = run();
        for _ in 0..50 {
            assert_eq!(run(), first, "non-deterministic");
        }
    }

    #[test]
    fn build_scaled_multiplies_gain() {
        let (rhy, dt) = (1.0, 0.1); // small dt → stay below the 1.0 cap (linear region)
        let mut a = Resonance::new();
        a.build_scaled(rhy, dt, 1.0);
        let mut b = Resonance::new();
        b.build_scaled(rhy, dt, 2.0);
        assert!((b.charge01 - 2.0 * a.charge01).abs() < 1e-6, "2x gain ⇒ 2x charge");
        let mut z = Resonance::new();
        z.build_scaled(rhy, dt, 0.0);
        assert_eq!(z.charge01, 0.0, "zero multiplier builds nothing");
        let mut neg = Resonance::new();
        neg.build_scaled(rhy, dt, -5.0);
        assert_eq!(neg.charge01, 0.0, "negative multiplier treated as zero");
        let mut p = Resonance::new();
        p.build(rhy, dt);
        assert_eq!(p.charge01, a.charge01, "build delegates to build_scaled(.., 1.0)");
    }
}
