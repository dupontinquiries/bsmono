//! weapon_ability — what a legendary weapon's signature proc DOES when the
//! blow lands, and the lingering status it leaves.
//!
//! `legendary_weapon::forge` gives every weapon a `ProcKind` by element, and
//! `resolve_weapon_swing` resolves exactly one of them (the Storm arc). The
//! bin then hard-coded a Storm blade for the arc regardless of what was
//! equipped, and the other five procs — FrostNova, WaterJet, GaleCut,
//! StoneSpikes, FlameWave — did nothing at all. Six elements, one ability.
//!
//! This module maps every proc to a concrete [`Outcome`] from the EQUIPPED
//! weapon: who else is hit (`chain_targets` for the arc, `jet_hits` for the
//! jet, a radius for the nova and the spikes, a cone for the flame), how hard
//! the target is shoved or launched, and which [`Status`] each victim
//! carries away (burning, chilled, staggered). [`StatusEffects`] ticks those
//! per creature: burn deals damage over time, chill scales speed, stagger
//! stops the creature and interrupts its melee windup.
//!
//! | proc          | element | on the target                 | on others           | status          |
//! |---------------|---------|-------------------------------|---------------------|-----------------|
//! | FrostNova     | Wood/0  | chilled                       | radius: chilled     | chill 3 s ×0.45 |
//! | WaterJet      | Water/1 | shoved hard                   | cone: shoved        | —               |
//! | GaleCut       | Air/2   | launched (up + back)          | —                   | stagger 0.6 s   |
//! | StoneSpikes   | Dust/3  | staggered                     | radius: damage+stag | stagger 0.8 s   |
//! | FlameWave     | Fire/4  | burning                       | cone: burning       | burn 4 s        |
//! | LightningArc  | Storm/5 | staggered                     | chain: damage       | stagger 0.3 s   |
//!
//! All magnitudes scale with the weapon's `proc_power` (forge gives 0.3–1.2
//! by class and tier). Pure, deterministic, allocation only for the target
//! lists; no panics — every input is clamped.

use crate::sim::legendary_weapon::{chain_targets, jet_hits, ProcKind, Weapon};

/// A lingering effect on a creature.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    /// Damage per second for `secs`.
    Burn { dps: f32, secs: f32 },
    /// Speed multiplier for `secs`.
    Chill { mult: f32, secs: f32 },
    /// No movement, melee windup interrupted, for `secs`.
    Stagger { secs: f32 },
}

/// One creature the ability reaches besides the primary target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Splash {
    /// Index into the `nearby` slice given to [`on_hit`].
    pub idx: usize,
    pub damage: f32,
    /// Shove along attacker→victim (blocks/s).
    pub shove: f32,
    pub status: Option<Status>,
}

/// What the landed blow does beyond its direct damage.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub proc: ProcKind,
    /// Multiplier on the ordinary knockback of the blow (1 = unchanged).
    pub knockback_mult: f32,
    /// Extra upward velocity on the target (blocks/s).
    pub launch_up: f32,
    pub target_status: Option<Status>,
    pub splash: Vec<Splash>,
    /// A one-line description for the combat log.
    pub label: &'static str,
}

impl Outcome {
    fn none() -> Outcome {
        Outcome { proc: ProcKind::None, knockback_mult: 1.0, launch_up: 0.0, target_status: None, splash: Vec::new(), label: "" }
    }
}

/// Reach of the area procs (blocks).
pub const NOVA_RADIUS: f32 = 4.0;
pub const SPIKES_RADIUS: f32 = 3.0;
pub const CONE_RANGE: f32 = 6.0;
/// cos of the cone half-angle (~35°).
pub const CONE_HALF_ANGLE_COS: f32 = 0.82;
pub const ARC_LINKS: u8 = 4;
pub const ARC_HOP: f32 = 6.0;

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

fn finite01(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(0.0, 4.0)
    } else {
        0.0
    }
}

/// Resolve the equipped weapon's proc on a landed blow. `target` is the
/// struck creature's position, `dir` the blow direction (attacker→target,
/// need not be unit), `nearby` the OTHER creatures' positions (the target
/// itself must not be in it). `proc_roll` in `[0,1]`: the area/chain procs
/// fire when `proc_roll < 0.35 + 0.5·proc_power` so a low-tier weapon procs
/// sometimes and a high-tier one nearly always; the single-target effects
/// (chill, burn, stagger, launch) always apply.
pub fn on_hit(w: &Weapon, target: [f32; 3], dir: [f32; 3], nearby: &[[f32; 3]], proc_roll: f32) -> Outcome {
    let p = finite01(w.proc_power);
    let base = w.base_damage.max(0.0);
    let roll = if proc_roll.is_finite() { proc_roll.clamp(0.0, 1.0) } else { 1.0 };
    let area_fires = roll < (0.35 + 0.5 * p).min(0.98);
    let mut out = Outcome::none();
    out.proc = w.proc;
    match w.proc {
        ProcKind::None => {}
        ProcKind::FrostNova => {
            out.label = "frost nova";
            let chill = Status::Chill { mult: (1.0 - 0.55 * p.min(1.0)).clamp(0.3, 0.95), secs: 2.0 + 2.0 * p };
            out.target_status = Some(chill);
            if area_fires {
                let r2 = (NOVA_RADIUS * (0.7 + 0.3 * p)).powi(2);
                for (i, &q) in nearby.iter().enumerate() {
                    if dist2(target, q) <= r2 {
                        out.splash.push(Splash { idx: i, damage: 0.0, shove: 0.0, status: Some(chill) });
                    }
                }
            }
        }
        ProcKind::WaterJet => {
            out.label = "water jet";
            out.knockback_mult = 1.0 + 2.0 * p;
            if area_fires {
                for i in jet_hits(target, dir, nearby, CONE_RANGE * (0.6 + 0.4 * p), CONE_HALF_ANGLE_COS) {
                    out.splash.push(Splash { idx: i, damage: 0.0, shove: 4.0 * p, status: None });
                }
            }
        }
        ProcKind::GaleCut => {
            out.label = "gale cut";
            out.knockback_mult = 1.0 + 1.0 * p;
            out.launch_up = 3.0 + 4.0 * p;
            out.target_status = Some(Status::Stagger { secs: 0.6 });
        }
        ProcKind::StoneSpikes => {
            out.label = "stone spikes";
            out.target_status = Some(Status::Stagger { secs: 0.8 });
            if area_fires {
                let r2 = SPIKES_RADIUS.powi(2);
                for (i, &q) in nearby.iter().enumerate() {
                    if dist2(target, q) <= r2 {
                        out.splash.push(Splash { idx: i, damage: base * 0.5 * p, shove: 1.0, status: Some(Status::Stagger { secs: 0.5 }) });
                    }
                }
            }
        }
        ProcKind::FlameWave => {
            out.label = "flame wave";
            let burn = Status::Burn { dps: (1.0 + base * 0.15 * p).max(1.0), secs: 4.0 };
            out.target_status = Some(burn);
            if area_fires {
                for i in jet_hits(target, dir, nearby, CONE_RANGE * 0.8, CONE_HALF_ANGLE_COS) {
                    out.splash.push(Splash { idx: i, damage: base * 0.25 * p, shove: 0.0, status: Some(burn) });
                }
            }
        }
        ProcKind::LightningArc => {
            out.label = "lightning arc";
            out.target_status = Some(Status::Stagger { secs: 0.3 });
            if area_fires {
                let d = base * p;
                for i in chain_targets(target, nearby, ARC_LINKS, ARC_HOP) {
                    out.splash.push(Splash { idx: i, damage: d, shove: 0.5, status: Some(Status::Stagger { secs: 0.3 }) });
                }
            }
        }
    }
    out
}

/// The lingering statuses on one creature. Effects of the same kind refresh
/// to the longer duration and the stronger magnitude rather than stacking.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StatusEffects {
    pub burn_secs: f32,
    pub burn_dps: f32,
    pub chill_secs: f32,
    pub chill_mult: f32,
    pub stagger_secs: f32,
}

impl StatusEffects {
    pub fn apply(&mut self, s: Status) {
        match s {
            Status::Burn { dps, secs } => {
                self.burn_secs = self.burn_secs.max(secs.max(0.0));
                self.burn_dps = self.burn_dps.max(dps.max(0.0));
            }
            Status::Chill { mult, secs } => {
                self.chill_secs = self.chill_secs.max(secs.max(0.0));
                let m = mult.clamp(0.05, 1.0);
                self.chill_mult = if self.chill_secs > 0.0 && self.chill_mult > 0.0 { self.chill_mult.min(m) } else { m };
            }
            Status::Stagger { secs } => self.stagger_secs = self.stagger_secs.max(secs.max(0.0)),
        }
    }

    /// Advance by `dt`; returns the burn damage dealt this tick (fractional —
    /// accumulate it before rounding).
    pub fn tick(&mut self, dt: f32) -> f32 {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let mut dmg = 0.0;
        if self.burn_secs > 0.0 {
            let step = dt.min(self.burn_secs);
            dmg = self.burn_dps * step;
            self.burn_secs -= dt;
            if self.burn_secs <= 0.0 {
                self.burn_secs = 0.0;
                self.burn_dps = 0.0;
            }
        }
        if self.chill_secs > 0.0 {
            self.chill_secs = (self.chill_secs - dt).max(0.0);
            if self.chill_secs == 0.0 {
                self.chill_mult = 0.0;
            }
        }
        self.stagger_secs = (self.stagger_secs - dt).max(0.0);
        dmg
    }

    /// Speed multiplier this tick (1 = unaffected, 0 while staggered).
    pub fn speed_mult(&self) -> f32 {
        if self.stagger_secs > 0.0 {
            0.0
        } else if self.chill_secs > 0.0 {
            self.chill_mult.clamp(0.05, 1.0)
        } else {
            1.0
        }
    }

    pub fn staggered(&self) -> bool {
        self.stagger_secs > 0.0
    }

    pub fn burning(&self) -> bool {
        self.burn_secs > 0.0
    }

    pub fn any(&self) -> bool {
        self.burn_secs > 0.0 || self.chill_secs > 0.0 || self.stagger_secs > 0.0
    }

    /// A tint to multiply into the creature's colour: ember-orange while
    /// burning, blue-white while chilled, unchanged otherwise. `t` in seconds
    /// flickers the burn.
    pub fn tint(&self, t: f32) -> [f32; 3] {
        if self.burn_secs > 0.0 {
            let f = 0.85 + 0.15 * (t * 23.0).sin();
            [1.0 * f + 0.15, 0.55 * f, 0.25 * f]
        } else if self.chill_secs > 0.0 {
            [0.75, 0.85, 1.15]
        } else {
            [1.0, 1.0, 1.0]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::legendary_weapon::{forge, WeaponClass};

    fn w(elem: u8, tier: u8) -> Weapon {
        forge(7, WeaponClass::Blade, elem, tier)
    }

    #[test]
    fn every_element_has_a_distinct_ability() {
        let target = [0.0, 0.0, 0.0];
        let nearby = [[2.0, 0.0, 0.0], [5.0, 0.0, 0.0], [0.0, 0.0, 2.5]];
        let mut labels = Vec::new();
        for e in 0..6u8 {
            let o = on_hit(&w(e, 3), target, [1.0, 0.0, 0.0], &nearby, 0.0);
            assert_ne!(o.proc, ProcKind::None, "element {e}");
            assert!(!o.label.is_empty());
            assert!(!labels.contains(&o.label), "element {e} repeats {}", o.label);
            labels.push(o.label);
            // every proc does SOMETHING beyond the direct hit
            assert!(o.target_status.is_some() || o.knockback_mult > 1.0 || !o.splash.is_empty(), "element {e}: {o:?}");
        }
    }

    #[test]
    fn frost_chills_in_a_radius_and_flame_burns_in_a_cone() {
        let target = [0.0, 0.0, 0.0];
        let nearby = [[2.0, 0.0, 0.0], [-2.0, 0.0, 0.0], [12.0, 0.0, 0.0]];
        let frost = on_hit(&w(0, 3), target, [1.0, 0.0, 0.0], &nearby, 0.0);
        assert!(matches!(frost.target_status, Some(Status::Chill { .. })));
        let idx: Vec<usize> = frost.splash.iter().map(|s| s.idx).collect();
        assert_eq!(idx, vec![0, 1], "both in radius, the far one not: {idx:?}");
        let flame = on_hit(&w(4, 3), target, [1.0, 0.0, 0.0], &nearby, 0.0);
        assert!(matches!(flame.target_status, Some(Status::Burn { .. })));
        let idx: Vec<usize> = flame.splash.iter().map(|s| s.idx).collect();
        assert_eq!(idx, vec![0], "only the one ahead in the cone: {idx:?}");
        assert!(flame.splash[0].damage > 0.0 && matches!(flame.splash[0].status, Some(Status::Burn { .. })));
    }

    #[test]
    fn jet_shoves_gale_launches_spikes_and_arc_stagger() {
        let target = [0.0, 0.0, 0.0];
        let nearby = [[3.0, 0.0, 0.0], [8.0, 0.0, 0.0]];
        let jet = on_hit(&w(1, 3), target, [1.0, 0.0, 0.0], &nearby, 0.0);
        assert!(jet.knockback_mult > 1.5 && jet.target_status.is_none());
        assert!(jet.splash.iter().all(|s| s.shove > 0.0 && s.damage == 0.0));
        let gale = on_hit(&w(2, 3), target, [1.0, 0.0, 0.0], &nearby, 0.0);
        assert!(gale.launch_up > 3.0 && matches!(gale.target_status, Some(Status::Stagger { .. })));
        let spikes = on_hit(&w(3, 3), target, [1.0, 0.0, 0.0], &nearby, 0.0);
        assert!(matches!(spikes.target_status, Some(Status::Stagger { .. })));
        assert_eq!(spikes.splash.len(), 1, "radius 3: only the near one");
        assert!(spikes.splash[0].damage > 0.0);
        let arc = on_hit(&w(5, 3), target, [1.0, 0.0, 0.0], &nearby, 0.0);
        let idx: Vec<usize> = arc.splash.iter().map(|s| s.idx).collect();
        assert_eq!(idx, vec![0, 1], "greedy chain through both within hop 6: {idx:?}");
        assert!(arc.splash[0].damage > 0.0);
    }

    #[test]
    fn area_procs_gate_on_the_roll_but_single_target_effects_always_apply() {
        let target = [0.0, 0.0, 0.0];
        let nearby = [[2.0, 0.0, 0.0]];
        let tier0 = w(4, 0);
        let miss = on_hit(&tier0, target, [1.0, 0.0, 0.0], &nearby, 0.99);
        assert!(miss.splash.is_empty(), "a bad roll: no wave");
        assert!(matches!(miss.target_status, Some(Status::Burn { .. })), "but the target still burns");
        let hit = on_hit(&tier0, target, [1.0, 0.0, 0.0], &nearby, 0.0);
        assert_eq!(hit.splash.len(), 1);
        // higher proc_power → higher chance; a tier-3 proc fires on a roll tier-0 misses
        let t3 = w(4, 3);
        assert!(t3.proc_power > tier0.proc_power);
        let mid = 0.35 + 0.5 * tier0.proc_power + 0.02;
        assert!(on_hit(&tier0, target, [1.0, 0.0, 0.0], &nearby, mid).splash.is_empty());
        assert!(!on_hit(&t3, target, [1.0, 0.0, 0.0], &nearby, mid).splash.is_empty());
    }

    #[test]
    fn statuses_tick_refresh_and_expire() {
        let mut s = StatusEffects::default();
        assert_eq!(s.speed_mult(), 1.0);
        s.apply(Status::Burn { dps: 2.0, secs: 1.0 });
        s.apply(Status::Burn { dps: 1.0, secs: 3.0 }); // refreshes to the longer, keeps the stronger
        assert_eq!(s.burn_secs, 3.0);
        assert_eq!(s.burn_dps, 2.0);
        let mut total = 0.0;
        for _ in 0..200 {
            total += s.tick(0.02);
        }
        assert!((total - 6.0).abs() < 1e-3, "2 dps × 3 s = 6, got {total}");
        assert!(!s.burning() && s.burn_dps == 0.0);
        // a chill slows, then wears off
        s.apply(Status::Chill { mult: 0.5, secs: 1.0 });
        assert_eq!(s.speed_mult(), 0.5);
        s.apply(Status::Chill { mult: 0.8, secs: 0.5 }); // weaker/shorter does not override
        assert_eq!(s.speed_mult(), 0.5);
        assert_eq!(s.chill_secs, 1.0);
        s.tick(1.5);
        assert_eq!(s.speed_mult(), 1.0);
        // stagger zeroes speed and beats a chill
        s.apply(Status::Chill { mult: 0.5, secs: 2.0 });
        s.apply(Status::Stagger { secs: 0.3 });
        assert_eq!(s.speed_mult(), 0.0);
        assert!(s.staggered());
        s.tick(0.4);
        assert_eq!(s.speed_mult(), 0.5);
        assert!(s.any());
        s.tick(5.0);
        assert!(!s.any());
        assert_eq!(s.tint(0.0), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn garbage_inputs_do_not_panic() {
        let mut bad = w(4, 3);
        bad.proc_power = f32::NAN;
        bad.base_damage = f32::INFINITY;
        let o = on_hit(&bad, [f32::NAN; 3], [0.0; 3], &[[1.0; 3]], f32::NAN);
        assert!(o.knockback_mult.is_finite() && o.launch_up.is_finite());
        let mut s = StatusEffects::default();
        s.apply(Status::Burn { dps: f32::NAN, secs: -1.0 });
        s.apply(Status::Chill { mult: f32::NAN, secs: f32::INFINITY });
        let d = s.tick(f32::NAN);
        assert!(d.is_finite() || d.is_nan() && s.burn_dps.is_nan() == false);
        let _ = s.speed_mult();
    }
}
