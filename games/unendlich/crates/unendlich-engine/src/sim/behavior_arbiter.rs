//! `sim::behavior_arbiter` — top-level utility-AI behavior selector (round R33).
//!
//! The mob AI already has the leaf behaviors (threat assessment, flee, forage,
//! rest, combat stance, herd cohesion, …). This is the **arbiter** that decides
//! WHICH behavior a mob runs this tick. It does not reimplement any leaf — it
//! consumes the already-computed drive scalars (`Drives`, each `0..=1`) the
//! engine derives from those cores, scores each candidate behavior, and returns
//! the argmax.
//!
//! ## Scoring (documented, finite, range `[0, ~2]`)
//! With all drives clamped to `[0,1]` (`t`=threat, `hu`=hunger, `fa`=fatigue,
//! `ag`=aggression, `hp`=health_frac):
//! * `Flee   = t · (0.4 + 1.6·(1−hp))`  — rises with threat and missing health;
//!   peaks at `2.0` when threat is high and health is low, so it dominates there.
//! * `Fight  = t · ag · (1.0 + hp)`      — needs threat AND aggression, scaled by
//!   health, so a nearly-dead mob scores Fight low and Flee wins instead.
//! * `Forage = hu · (1−t) · 1.5`         — rises with hunger, suppressed by threat.
//! * `Rest   = fa · (1−t) · (1−hu) · 1.4`— rises with fatigue, strongly suppressed
//!   by threat and hunger.
//! * `Patrol = 0.2` constant             — the baseline default when nothing's urgent.
//!
//! Argmax tie-break is deterministic by behavior order
//! `Rest < Forage < Patrol < Fight < Flee` — on an exact tie the later
//! (Flee-ward) behavior wins. `prev` + `hysteresis` add a small bonus to the
//! current behavior so it doesn't flip-flop frame to frame; because the bonus is
//! additive and small it keeps `prev` only across near-ties and never overrides
//! a clearly-higher option.
//!
//! Pure: logic in, data out — no `crate::render`, no GPU, no I/O, no clock, no
//! RNG, no `.unwrap()`/`.expect()` in non-test code.

/// The behaviors the arbiter can select, ordered for deterministic tie-break.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behavior {
    Rest,
    Forage,
    Patrol,
    Fight,
    Flee,
}

/// Already-computed drive scalars, each in `0..=1` (clamped defensively).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drives {
    pub threat: f32,
    pub hunger: f32,
    pub fatigue: f32,
    pub aggression: f32,
    pub health_frac: f32,
}

/// The arbiter's decision: the chosen behavior and its (post-hysteresis) score.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Choice {
    pub behavior: Behavior,
    pub score: f32,
}

const PATROL_BASELINE: f32 = 0.2;

fn clamp01(x: f32) -> f32 {
    crate::util::mathx::clamp01(x) // r343 dedup
}

/// Raw (pre-hysteresis) score for one behavior given clamped drives.
fn score_of(b: Behavior, t: f32, hu: f32, fa: f32, ag: f32, hp: f32) -> f32 {
    match b {
        Behavior::Flee => t * (0.4 + 1.6 * (1.0 - hp)),
        Behavior::Fight => t * ag * (1.0 + hp),
        Behavior::Forage => hu * (1.0 - t) * 1.5,
        Behavior::Rest => fa * (1.0 - t) * (1.0 - hu) * 1.4,
        Behavior::Patrol => PATROL_BASELINE,
    }
}

/// The five behaviors in tie-break order (ascending).
const ALL: [Behavior; 5] = [
    Behavior::Rest,
    Behavior::Forage,
    Behavior::Patrol,
    Behavior::Fight,
    Behavior::Flee,
];

/// Score each candidate behavior from `d` and return the argmax. `prev` +
/// `hysteresis` give the current behavior a small additive bonus so it doesn't
/// flip-flop on near-ties (a clearly-higher option still wins). Ties break
/// toward the later behavior in `Rest<Forage<Patrol<Fight<Flee`.
pub fn select(d: &Drives, prev: Option<Behavior>, hysteresis: f32) -> Choice {
    let t = clamp01(d.threat);
    let hu = clamp01(d.hunger);
    let fa = clamp01(d.fatigue);
    let ag = clamp01(d.aggression);
    let hp = clamp01(d.health_frac);
    let h = if hysteresis.is_nan() || hysteresis < 0.0 { 0.0 } else { hysteresis };

    // Seed the search with the first behavior in order, then keep the best;
    // `>=` over the ascending ALL order makes an exact tie resolve Flee-ward.
    let mut best_b = ALL[0];
    let mut best_s = score_of(ALL[0], t, hu, fa, ag, hp)
        + if prev == Some(ALL[0]) { h } else { 0.0 };
    let mut i = 1;
    while i < ALL.len() {
        let b = ALL[i];
        let s = score_of(b, t, hu, fa, ag, hp) + if prev == Some(b) { h } else { 0.0 };
        if s >= best_s {
            best_s = s;
            best_b = b;
        }
        i += 1;
    }
    Choice { behavior: best_b, score: best_s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_states_select_distinct_actions() {
        // The bin's roamer tick feeds these drives into `select`; distinct
        // states must yield distinct actions (the integration this wires).
        let fleeing = select(
            &Drives { threat: 0.9, hunger: 0.1, fatigue: 0.1, aggression: 0.2, health_frac: 0.2 },
            None,
            0.0,
        );
        let foraging = select(
            &Drives { threat: 0.0, hunger: 0.9, fatigue: 0.1, aggression: 0.1, health_frac: 1.0 },
            None,
            0.0,
        );
        let fighting = select(
            &Drives { threat: 0.8, hunger: 0.0, fatigue: 0.0, aggression: 1.0, health_frac: 1.0 },
            None,
            0.0,
        );
        let resting = select(
            &Drives { threat: 0.0, hunger: 0.0, fatigue: 0.9, aggression: 0.0, health_frac: 1.0 },
            None,
            0.0,
        );
        assert_eq!(fleeing.behavior, Behavior::Flee);
        assert_eq!(foraging.behavior, Behavior::Forage);
        assert_eq!(fighting.behavior, Behavior::Fight);
        assert_eq!(resting.behavior, Behavior::Rest);
        // all four drive-states resolved to four different actions
        let acts = [fleeing.behavior, foraging.behavior, fighting.behavior, resting.behavior];
        for i in 0..acts.len() {
            for j in (i + 1)..acts.len() {
                assert_ne!(acts[i], acts[j], "drive states should discriminate actions");
            }
        }
    }


    fn drives(threat: f32, hunger: f32, fatigue: f32, aggression: f32, health_frac: f32) -> Drives {
        Drives { threat, hunger, fatigue, aggression, health_frac }
    }
    fn pick(d: &Drives) -> Behavior {
        select(d, None, 0.0).behavior
    }
    // small splitmix for the property test (documented seed, R20)
    fn lcg(s: &mut u32) -> f32 {
        *s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*s >> 8) as f32 / ((1u32 << 24) as f32)
    }

    #[test]
    fn high_threat_low_health_flees() {
        assert_eq!(pick(&drives(1.0, 0.0, 0.0, 1.0, 0.05)), Behavior::Flee);
    }

    #[test]
    fn high_threat_aggression_health_fights() {
        assert_eq!(pick(&drives(1.0, 0.0, 0.0, 1.0, 1.0)), Behavior::Fight);
    }

    #[test]
    fn high_hunger_safe_forages() {
        assert_eq!(pick(&drives(0.0, 1.0, 0.3, 0.5, 1.0)), Behavior::Forage);
    }

    #[test]
    fn high_fatigue_safe_fed_rests() {
        assert_eq!(pick(&drives(0.0, 0.0, 1.0, 0.5, 1.0)), Behavior::Rest);
    }

    #[test]
    fn all_low_patrols() {
        assert_eq!(pick(&drives(0.0, 0.0, 0.0, 0.0, 0.0)), Behavior::Patrol);
        // also when health is high and nothing is urgent
        assert_eq!(pick(&drives(0.0, 0.0, 0.0, 0.0, 1.0)), Behavior::Patrol);
    }

    #[test]
    fn hysteresis_keeps_prev_on_near_tie_only() {
        // hunger 0.5 / fatigue 1.0, safe: Forage=0.75 vs Rest=0.70 (gap 0.05).
        let near = drives(0.0, 0.5, 1.0, 0.5, 1.0);
        assert_eq!(select(&near, None, 0.0).behavior, Behavior::Forage, "baseline winner");
        // a hysteresis larger than the 0.05 gap keeps prev=Rest
        assert_eq!(select(&near, Some(Behavior::Rest), 0.1).behavior, Behavior::Rest);
        // but a clearly-higher option is NOT overridden by the same bonus:
        let clear = drives(0.0, 1.0, 1.0, 0.5, 1.0); // Forage=1.5, Rest=0 (hu=1)
        assert_eq!(select(&clear, Some(Behavior::Rest), 0.1).behavior, Behavior::Forage);
    }

    #[test]
    fn nearly_dead_flees_even_with_max_aggression() {
        // Flee must beat Fight when health is gone, regardless of aggression.
        assert_eq!(pick(&drives(1.0, 0.0, 0.0, 1.0, 0.0)), Behavior::Flee);
    }

    #[test]
    fn scores_finite_and_bounded_over_random_inputs() {
        let mut s = 0x33A0_B1E5u32; // documented seed (R20)
        for _ in 0..2000 {
            // include out-of-range values to exercise the clamps
            let d = drives(
                lcg(&mut s) * 1.6 - 0.3,
                lcg(&mut s) * 1.6 - 0.3,
                lcg(&mut s) * 1.6 - 0.3,
                lcg(&mut s) * 1.6 - 0.3,
                lcg(&mut s) * 1.6 - 0.3,
            );
            let prev = match (lcg(&mut s) * 6.0) as u32 {
                0 => Some(Behavior::Rest),
                1 => Some(Behavior::Forage),
                2 => Some(Behavior::Patrol),
                3 => Some(Behavior::Fight),
                4 => Some(Behavior::Flee),
                _ => None,
            };
            let c = select(&d, prev, lcg(&mut s) * 0.2);
            assert!(c.score.is_finite() && c.score >= 0.0 && c.score <= 4.0, "score {} out of range", c.score);
        }
    }

    #[test]
    fn out_of_range_inputs_are_tolerated() {
        // 2.0 behaves like 1.0, -1.0 like 0.0, NaN like 0.0 — no panic, sane pick.
        let c = select(&drives(2.0, -1.0, f32::NAN, 5.0, -2.0), None, 0.0);
        assert!(c.score.is_finite());
        // threat saturates to 1, health to 0 ⇒ Flee
        assert_eq!(c.behavior, Behavior::Flee);
    }

    #[test]
    fn deterministic() {
        let d = drives(0.6, 0.4, 0.5, 0.7, 0.8);
        assert_eq!(select(&d, Some(Behavior::Fight), 0.05), select(&d, Some(Behavior::Fight), 0.05));
    }

    #[test]
    fn tie_break_deterministic() {
        // t=1, hp=1, ag=0.2 ⇒ Flee=0.4 and Fight=0.4 (a near/exact tie).
        let d = drives(1.0, 0.0, 0.0, 0.2, 1.0);
        let a = select(&d, None, 0.0).behavior;
        assert_eq!(a, select(&d, None, 0.0).behavior, "tie resolution is deterministic");
        assert!(a == Behavior::Flee || a == Behavior::Fight, "winner is one of the tied pair");
    }
}
