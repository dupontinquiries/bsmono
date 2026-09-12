//! melee — a creature-versus-creature melee exchange, composed from the
//! leaf combat cores and timed so the blow lands where the animation lands.
//!
//! The leaves exist and are tested on their own: `hit_chance` (accuracy vs
//! evasion), `damage_after_armor` (mitigation), `knockback_impulse`,
//! `overkill_damage`, `combat_stance` (lean / standoff from health and
//! threat). What did not exist is the loop that turns two creatures in
//! reach into a sequence of strikes: WHEN a creature commits to a strike,
//! how long it winds up, at what instant the blow resolves, how long it is
//! open afterwards, and whether being hit mid-windup interrupts it. That
//! loop is [`tick`].
//!
//! Timing is the contract with the animation. [`Bout::strike01`] sweeps
//! `0→1` across windup + strike + recover and is handed straight to
//! `anim_rig::MotionInput::strike`; the blow resolves at
//! [`IMPACT_AT`], the progress where `anim_rig::strike_curve` peaks. So the
//! damage number appears on the frame the lunge is furthest forward, for any
//! creature, without the animation and the rules knowing about each other.
//!
//! Pure: std-only, deterministic (the caller supplies the `roll`), no
//! allocation, no panics; every input is clamped.

use crate::sim::combat_stance::{combat_stance, CreatureState, Stance};
use crate::sim::damage_after_armor;
use crate::sim::hit_chance;
use crate::sim::knockback_impulse;
use crate::sim::overkill_damage;

/// Progress through a strike at which the blow resolves — `anim_rig::strike_curve`'s peak.
pub const IMPACT_AT: f32 = 0.45;
/// Armor mitigation curve constant (`damage_after_armor::mitigated`'s `k`).
pub const ARMOR_K: f32 = 10.0;
/// Knockback resist for the default creature (0 = full shove).
pub const DEFAULT_RESIST: f32 = 0.2;
/// A hit during windup with at least this much of the attacker's max HP in
/// damage interrupts the strike.
pub const FLINCH_FRACTION: f32 = 0.08;

/// Everything the exchange reads about one creature. Build from the roamer's
/// stats; `Default` is a mid-sized, average brawler.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fighter {
    pub hp: f32,
    pub max_hp: f32,
    /// Blocks/s² scale for knockback; heavier shoves less far.
    pub mass: f32,
    /// How far the strike lands from the body centre (blocks).
    pub reach: f32,
    /// Raw damage of a landed blow.
    pub power: f32,
    pub accuracy: f32,
    pub evasion: f32,
    pub armor: f32,
    /// Left/right limb health for the stance's guard side.
    pub left_hp: f32,
    pub right_hp: f32,
}

impl Default for Fighter {
    fn default() -> Self {
        Fighter {
            hp: 40.0,
            max_hp: 40.0,
            mass: 2.0,
            reach: 1.2,
            power: 6.0,
            accuracy: 1.0,
            evasion: 0.5,
            armor: 0.0,
            left_hp: 20.0,
            right_hp: 20.0,
        }
    }
}

/// The attacker's phase in the exchange loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Out of reach: closing (or holding at standoff when hurt).
    Approach,
    /// Committed; the blow has not landed yet.
    Windup,
    /// The blow is landing / has landed this strike.
    Strike,
    /// Open after the blow; can be punished.
    Recover,
}

/// What the mover should do this tick with the distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Close,
    Hold,
    Back,
}

/// Per-attacker exchange state. One per creature that fights.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bout {
    pub phase: Phase,
    /// Seconds into the current strike (windup start = 0).
    pub t: f32,
    /// Seconds until the next strike may begin (after recover).
    pub cooldown: f32,
    /// Whether this strike has resolved (one hit per strike).
    resolved: bool,
}

impl Default for Bout {
    fn default() -> Self {
        Bout { phase: Phase::Approach, t: 0.0, cooldown: 0.0, resolved: false }
    }
}

/// Strike timing for a body of `mass`: (windup, strike, recover) seconds.
/// A light creature snaps (0.18 / 0.10 / 0.22); a heavy one telegraphs
/// (up to 0.55 / 0.18 / 0.6). Every value is bounded and positive.
pub fn strike_timing(mass: f32) -> (f32, f32, f32) {
    let m = if mass.is_finite() { mass.clamp(0.2, 40.0) } else { 2.0 };
    // 0..1 over the mass range on a log-ish curve so a 2× mass is a modest slowdown
    let k = ((m / 0.2).ln() / (40.0f32 / 0.2).ln()).clamp(0.0, 1.0);
    (0.18 + 0.37 * k, 0.10 + 0.08 * k, 0.22 + 0.38 * k)
}

/// Total strike duration for a body of `mass`.
pub fn strike_duration(mass: f32) -> f32 {
    let (w, s, r) = strike_timing(mass);
    w + s + r
}

impl Bout {
    /// Progress through the strike `[0,1]` for the animation; `0` while
    /// approaching or on cooldown.
    pub fn strike01(&self, mass: f32) -> f32 {
        match self.phase {
            Phase::Approach => 0.0,
            _ => (self.t / strike_duration(mass)).clamp(0.0, 1.0),
        }
    }

    /// Being hit during windup for enough damage interrupts the strike (it
    /// returns to Approach with a short cooldown). Returns whether it did.
    pub fn flinch(&mut self, damage: f32, max_hp: f32) -> bool {
        if self.phase == Phase::Windup && damage >= FLINCH_FRACTION * max_hp.max(1.0) {
            *self = Bout { phase: Phase::Approach, t: 0.0, cooldown: 0.25, resolved: false };
            true
        } else {
            false
        }
    }
}

/// The result of a resolved blow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Exchange {
    pub hit: bool,
    /// Damage after armor (0 on a miss).
    pub damage: f32,
    /// Knockback speed to add along attacker→defender (blocks/s; 0 on a miss).
    pub knockback: f32,
    /// The defender's HP after the blow.
    pub defender_hp: f32,
    pub lethal: bool,
    pub overkill: f32,
}

/// Resolve one blow. `roll` in `[0,1]` from the caller's deterministic
/// source; a hit when `roll < hit_chance`. Pure.
pub fn resolve(attacker: &Fighter, defender: &Fighter, roll: f32) -> Exchange {
    let roll = if roll.is_finite() { roll.clamp(0.0, 1.0) } else { 0.5 };
    let p = hit_chance::hit_chance(attacker.accuracy, defender.evasion);
    let hp0 = defender.hp.max(0.0);
    if roll >= p {
        return Exchange { hit: false, damage: 0.0, knockback: 0.0, defender_hp: hp0, lethal: false, overkill: 0.0 };
    }
    let damage = damage_after_armor::mitigated(attacker.power, defender.armor, ARMOR_K);
    let hp1 = (hp0 - damage).max(0.0);
    let knockback = knockback_impulse::impulse(damage, defender.mass, DEFAULT_RESIST);
    Exchange {
        hit: true,
        damage,
        knockback,
        defender_hp: hp1,
        lethal: hp1 <= 0.0 && hp0 > 0.0,
        overkill: overkill_damage::overkill(damage, hp0),
    }
}

/// The attacker's stance toward the defender at `dist`: lean, guard side,
/// preferred standoff. `threat` in `[0,1]` from the caller's assessment.
pub fn stance(attacker: &Fighter, threat: f32, dist_base: f32) -> Stance {
    combat_stance(CreatureState {
        overall_hp: attacker.hp,
        max_hp: attacker.max_hp,
        left_hp: attacker.left_hp,
        right_hp: attacker.right_hp,
        vital_exposure: 0.5,
        threat,
        base_distance: dist_base.max(attacker.reach),
        melee_distance: attacker.reach,
    })
}

/// One tick of the attacker's loop. `dist` is centre-to-centre; the blow can
/// land when `dist <= attacker.reach + defender_radius`. Returns the movement
/// the attacker wants and, on the impact frame, the resolved blow.
///
/// * Approach: `Close` until in reach, then start a windup (if off
///   cooldown). A badly hurt attacker (`stance.lean < -0.5`) holds instead:
///   `Back` when inside its standoff, `Hold` otherwise — it does not commit.
/// * Windup → Strike at the windup time; the blow resolves the first tick
///   `strike01 >= IMPACT_AT` — ONLY if the defender is still in reach then
///   (a target that stepped away is a miss with no roll).
/// * Recover → Approach after the recover time, with a cooldown.
pub fn tick(
    bout: &mut Bout,
    attacker: &Fighter,
    defender: &Fighter,
    defender_radius: f32,
    dist: f32,
    threat: f32,
    dt: f32,
    roll: f32,
) -> (Move, Option<Exchange>) {
    let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
    let dist = if dist.is_finite() { dist.max(0.0) } else { f32::MAX };
    let in_reach = dist <= attacker.reach + defender_radius.max(0.0);
    let (w, s, _r) = strike_timing(attacker.mass);
    let total = strike_duration(attacker.mass);
    bout.cooldown = (bout.cooldown - dt).max(0.0);
    match bout.phase {
        Phase::Approach => {
            let st = stance(attacker, threat, attacker.reach * 2.5);
            if st.lean < -0.5 {
                // hurt: keep the standoff, do not commit
                return (if dist < st.standoff * 0.9 { Move::Back } else { Move::Hold }, None);
            }
            if !in_reach {
                return (Move::Close, None);
            }
            if bout.cooldown > 0.0 {
                return (Move::Hold, None);
            }
            *bout = Bout { phase: Phase::Windup, t: 0.0, cooldown: 0.0, resolved: false };
            (Move::Hold, None)
        }
        Phase::Windup | Phase::Strike | Phase::Recover => {
            bout.t += dt;
            let p = bout.t / total;
            let mut hit = None;
            if bout.phase == Phase::Windup && bout.t >= w {
                bout.phase = Phase::Strike;
            }
            if bout.phase == Phase::Strike && !bout.resolved && p >= IMPACT_AT {
                bout.resolved = true;
                hit = Some(if in_reach {
                    resolve(attacker, defender, roll)
                } else {
                    // the target stepped out during the windup: whiff
                    Exchange { hit: false, damage: 0.0, knockback: 0.0, defender_hp: defender.hp.max(0.0), lethal: false, overkill: 0.0 }
                });
            }
            if bout.phase == Phase::Strike && bout.t >= w + s {
                bout.phase = Phase::Recover;
                if !bout.resolved {
                    // dt so coarse the impact instant was skipped: resolve now
                    bout.resolved = true;
                    hit = Some(if in_reach { resolve(attacker, defender, roll) } else {
                        Exchange { hit: false, damage: 0.0, knockback: 0.0, defender_hp: defender.hp.max(0.0), lethal: false, overkill: 0.0 }
                    });
                }
            }
            if bout.phase == Phase::Recover && bout.t >= total {
                *bout = Bout { phase: Phase::Approach, t: 0.0, cooldown: 0.15, resolved: false };
            }
            // during a strike the body drives forward on its own; hold position
            (Move::Hold, hit)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(att: &Fighter, def: &mut Fighter, dist: f32, secs: f32, dt: f32, roll: f32) -> (Vec<(f32, Phase, f32)>, Vec<Exchange>) {
        let mut b = Bout::default();
        let mut log = Vec::new();
        let mut hits = Vec::new();
        let mut t = 0.0;
        while t < secs {
            let (_mv, ex) = tick(&mut b, att, def, 0.4, dist, 0.6, dt, roll);
            if let Some(e) = ex {
                def.hp = e.defender_hp;
                hits.push(e);
            }
            log.push((t, b.phase, b.strike01(att.mass)));
            t += dt;
        }
        (log, hits)
    }

    #[test]
    fn the_blow_resolves_where_the_lunge_peaks() {
        // The contract with the animation: IMPACT_AT is strike_curve's peak.
        let peak = crate::anim_rig::strike_curve(IMPACT_AT);
        assert!((peak - 1.0).abs() < 1e-5, "strike_curve({IMPACT_AT}) = {peak}, not the peak");
        for p in [0.0, 0.2, 0.7, 1.0] {
            assert!(crate::anim_rig::strike_curve(p) < peak);
        }
    }

    #[test]
    fn timing_is_bounded_and_heavier_is_slower() {
        let (w1, s1, r1) = strike_timing(0.5);
        let (w2, s2, r2) = strike_timing(20.0);
        assert!(w2 > w1 && s2 > s1 && r2 > r1);
        for m in [f32::NAN, -5.0, 0.0, 1e9] {
            let (w, s, r) = strike_timing(m);
            assert!(w > 0.0 && s > 0.0 && r > 0.0 && (w + s + r) < 1.5, "{m}: {w} {s} {r}");
        }
    }

    #[test]
    fn a_strike_lands_on_the_impact_frame_and_only_once() {
        let att = Fighter::default();
        let mut def = Fighter::default();
        let dt = 1.0 / 60.0;
        let (log, hits) = run(&att, &mut def, 1.0, strike_duration(att.mass) + 0.05, dt, 0.0);
        assert_eq!(hits.len(), 1, "one blow per strike: {hits:?}");
        assert!(hits[0].hit && hits[0].damage > 0.0);
        // The hit is logged at the first tick with strike01 >= IMPACT_AT.
        let first_at = log.iter().position(|(_, ph, p)| *ph != Phase::Approach && *p >= IMPACT_AT).unwrap();
        let hit_tick = {
            // replay to find the tick index of the hit
            let mut b = Bout::default();
            let mut i = 0;
            let mut found = None;
            let mut t = 0.0;
            while t < strike_duration(att.mass) + 0.05 {
                let (_m, ex) = tick(&mut b, &att, &def, 0.4, 1.0, 0.6, dt, 0.0);
                if ex.is_some() && found.is_none() { found = Some(i); }
                i += 1;
                t += dt;
            }
            found.unwrap()
        };
        assert_eq!(hit_tick, first_at, "the blow resolves where the lunge peaks");
        // phases run Approach → Windup → Strike → Recover → Approach
        let seq: Vec<Phase> = log.iter().map(|l| l.1).fold(Vec::new(), |mut v, p| { if v.last() != Some(&p) { v.push(p); } v });
        assert_eq!(seq, vec![Phase::Windup, Phase::Strike, Phase::Recover, Phase::Approach]);
        // strike01 rises monotonically through the strike and is 0 while approaching
        let mut last = 0.0;
        for (_, ph, p) in &log {
            if *ph == Phase::Approach { assert_eq!(*p, 0.0); last = 0.0; } else { assert!(*p >= last); last = *p; }
        }
    }

    #[test]
    fn out_of_reach_closes_and_a_target_that_steps_away_is_a_whiff() {
        let att = Fighter::default();
        let def = Fighter::default();
        let mut b = Bout::default();
        let (mv, ex) = tick(&mut b, &att, &def, 0.4, 5.0, 0.6, 1.0 / 60.0, 0.0);
        assert_eq!((mv, ex), (Move::Close, None));
        assert_eq!(b.phase, Phase::Approach);
        // in reach: commits
        tick(&mut b, &att, &def, 0.4, 1.0, 0.6, 1.0 / 60.0, 0.0);
        assert_eq!(b.phase, Phase::Windup);
        // the target steps away before impact
        let mut hits = Vec::new();
        for _ in 0..80 {
            let (_m, ex) = tick(&mut b, &att, &def, 0.4, 4.0, 0.6, 1.0 / 60.0, 0.0);
            if let Some(e) = ex { hits.push(e); }
        }
        assert_eq!(hits.len(), 1);
        assert!(!hits[0].hit && hits[0].damage == 0.0, "whiff: {:?}", hits[0]);
    }

    #[test]
    fn misses_do_nothing_and_hits_mitigate_shove_and_kill() {
        let att = Fighter { power: 12.0, ..Default::default() };
        let def = Fighter { hp: 10.0, armor: 5.0, ..Default::default() };
        let miss = resolve(&att, &def, 0.999);
        assert!(!miss.hit && miss.damage == 0.0 && miss.knockback == 0.0 && miss.defender_hp == 10.0 && !miss.lethal);
        let hit = resolve(&att, &def, 0.0);
        assert!(hit.hit);
        assert!(hit.damage < 12.0 && hit.damage > 0.0, "armor mitigates: {}", hit.damage);
        assert!(hit.knockback > 0.0);
        let unarmored = resolve(&att, &Fighter { hp: 10.0, ..Default::default() }, 0.0);
        assert!((unarmored.damage - 12.0).abs() < 1e-5 && unarmored.lethal && unarmored.overkill > 1.9, "{unarmored:?}");
        // heavier defender is shoved less
        let heavy = resolve(&att, &Fighter { mass: 20.0, ..Default::default() }, 0.0);
        assert!(heavy.knockback < unarmored.knockback);
        // evasion lowers the hit chance: a roll that hits a slow target misses a nimble one
        let nimble = Fighter { evasion: 5.0, ..Default::default() };
        let p_slow = hit_chance::hit_chance(att.accuracy, def.evasion);
        let p_nimble = hit_chance::hit_chance(att.accuracy, nimble.evasion);
        assert!(p_nimble < p_slow);
        assert!(!resolve(&att, &nimble, (p_slow + p_nimble) * 0.5).hit);
        assert!(resolve(&att, &def, (p_slow + p_nimble) * 0.5).hit);
    }

    #[test]
    fn a_hurt_attacker_holds_its_standoff_instead_of_committing() {
        let att = Fighter { hp: 4.0, ..Default::default() };
        let def = Fighter::default();
        let mut b = Bout::default();
        let st = stance(&att, 0.6, att.reach * 2.5);
        assert!(st.lean < -0.5, "nearly dead is defensive: {st:?}");
        let (mv, ex) = tick(&mut b, &att, &def, 0.4, 1.0, 0.6, 1.0 / 60.0, 0.0);
        assert_eq!(mv, Move::Back, "inside the standoff: back off");
        assert!(ex.is_none() && b.phase == Phase::Approach, "never commits");
        let (mv2, _) = tick(&mut b, &att, &def, 0.4, st.standoff + 1.0, 0.6, 1.0 / 60.0, 0.0);
        assert_eq!(mv2, Move::Hold);
    }

    #[test]
    fn a_hit_during_windup_flinches_but_a_scratch_does_not() {
        let att = Fighter::default();
        let def = Fighter::default();
        let mut b = Bout::default();
        tick(&mut b, &att, &def, 0.4, 1.0, 0.6, 1.0 / 60.0, 0.0);
        assert_eq!(b.phase, Phase::Windup);
        assert!(!b.flinch(1.0, att.max_hp), "1 of 40 is a scratch");
        assert_eq!(b.phase, Phase::Windup);
        assert!(b.flinch(6.0, att.max_hp));
        assert_eq!(b.phase, Phase::Approach);
        assert!(b.cooldown > 0.0);
        // not in windup: no flinch
        let mut c = Bout { phase: Phase::Recover, t: 0.5, cooldown: 0.0, resolved: true };
        assert!(!c.flinch(30.0, att.max_hp));
    }

    #[test]
    fn a_coarse_dt_still_resolves_exactly_one_blow() {
        let att = Fighter::default();
        let mut def = Fighter::default();
        let (_log, hits) = run(&att, &mut def, 1.0, 3.0, 0.25, 0.0);
        // 3 s at dt 0.25 with a ~0.7 s strike + 0.15 cooldown: several strikes, each one blow
        assert!(hits.len() >= 2 && hits.len() <= 5, "{}", hits.len());
        assert!(def.hp < 40.0);
    }

    #[test]
    fn garbage_inputs_do_not_panic() {
        let att = Fighter { mass: f32::NAN, power: f32::INFINITY, accuracy: -1.0, ..Default::default() };
        let def = Fighter { hp: f32::NAN, armor: -3.0, mass: 0.0, ..Default::default() };
        let mut b = Bout::default();
        for &d in &[f32::NAN, f32::INFINITY, -1.0, 0.0] {
            for &dt in &[f32::NAN, -1.0, 10.0, 1.0 / 60.0] {
                let (_m, ex) = tick(&mut b, &att, &def, f32::NAN, d, f32::NAN, dt, f32::NAN);
                if let Some(e) = ex {
                    assert!(e.knockback.is_finite() && e.defender_hp.is_finite() || e.defender_hp.is_nan(), "{e:?}");
                }
                let _ = b.strike01(att.mass);
            }
        }
    }
}
