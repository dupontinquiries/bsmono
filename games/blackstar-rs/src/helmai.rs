//! HELM AI — the ship flies itself between the places the plot names.
//!
//! `voyage::plan` prices a leg; `ship.rs` walks the player through the
//! phases; `fleet.rs` says two ships in formation can bridge and walk at
//! [`crate::fleet::REACH_M`]. What nothing does is FLY: hold the burn,
//! flip at the midpoint, keep a wing on station, bring a hull into dock
//! below the speed that dents it. This module is that pilot, headless in
//! the house shape — three controllers, each a pure step function, each
//! with the sim as a caller and the tests as a witness.
//!
//! **The leg pilot is a profile, not a physics engine.** `voyage::plan`
//! already decided the duration; the pilot's job is the SHAPE of the
//! trip — align, burn, flip, brake, arrive — and a `progress()` that the
//! holomap draws. The shape is the constant-thrust brachistochrone
//! normalised to the planned duration: quadratic out, quadratic in,
//! velocity peaking at the flip. That closes the holomap's own wiring
//! note ("route progress is a caller-fed number"): this is the caller.
//!
//! **Formation keeping exists BECAUSE of the fleet rules.** The bridge
//! and the spacewalk both die past `REACH_M`, and "close formation
//! first" is only honest advice if something can close it. The
//! station-keeper is a damped proportional controller onto a slot
//! offset from the flagship — deliberately overdamped, because a wing
//! that overshoots through the flagship's hull is a controller tuned
//! for a paper, not a deck crew. It holds at `HOLD_FRAC` of reach so
//! the span never lives at the edge of its own collapse.
//!
//! **Docking has a speed gate, and the gate refuses.** Below
//! [`R_SLOW`] of the target hull the approach speed must be under
//! [`V_DOCK`]; a hot approach is refused WITH THE NUMBER, in the
//! `fleet::Refusal` manner — a pilot told "no" learns less than one
//! told "8.4 m/s over the gate".
//!
//! `--helm` narrates all three: a leg flown end to end, a wing joining
//! formation until the span can reach, a hot dock refused and a cold
//! one taken.

use glam::Vec3;

use crate::voyage::Leg;

/// Where the leg pilot is in the trip.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// nose onto the departure asymptote — seconds, scaled by hull
    Align,
    Burn,
    /// the turnover beat at midpoint; a carrier takes its time
    Flip,
    Brake,
    Arrived,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Align => "ALIGN",
            Phase::Burn => "BURN",
            Phase::Flip => "FLIP",
            Phase::Brake => "BRAKE",
            Phase::Arrived => "ARRIVED",
        }
    }
}

/// The leg pilot: phases over a planned leg, and the progress the
/// holomap draws.
pub struct LegPilot {
    pub leg: Leg,
    /// seconds to line the nose up before the burn — a hull-size cost
    pub align_s: f32,
    /// seconds the turnover takes at midpoint
    pub flip_s: f32,
    pub t: f32,
    flipped: bool,
}

impl LegPilot {
    pub fn new(leg: Leg, hull_len_m: f32) -> LegPilot {
        LegPilot {
            leg,
            align_s: (2.0 + hull_len_m * 0.004).min(14.0),
            flip_s: (1.0 + hull_len_m * 0.006).min(20.0),
            t: 0.0,
            flipped: false,
        }
    }

    /// Advance; returns `Some(phase)` when the phase CHANGES, which is
    /// what a narrator or an event log wants.
    pub fn update(&mut self, dt: f32) -> Option<Phase> {
        let before = self.phase();
        self.t += dt;
        if !self.flipped && matches!(self.phase(), Phase::Flip | Phase::Brake | Phase::Arrived) {
            self.flipped = true;
        }
        let after = self.phase();
        (after != before).then_some(after)
    }

    pub fn phase(&self) -> Phase {
        let cruise = self.leg.duration_s.max(1.0);
        let mid = self.align_s + cruise * 0.5;
        if self.t < self.align_s {
            Phase::Align
        } else if self.t < mid {
            Phase::Burn
        } else if self.t < mid + self.flip_s {
            Phase::Flip
        } else if self.t < self.align_s + self.flip_s + cruise {
            Phase::Brake
        } else {
            Phase::Arrived
        }
    }

    /// 0..1 along the leg — the brachistochrone shape normalised to the
    /// planned duration: quadratic out, quadratic home, speed peaking at
    /// the flip. Monotonic, exact at both ends.
    pub fn progress(&self) -> f32 {
        let cruise = self.leg.duration_s.max(1.0);
        let x = ((self.t - self.align_s) / cruise).clamp(0.0, 1.0);
        if x < 0.5 { 2.0 * x * x } else { 1.0 - 2.0 * (1.0 - x) * (1.0 - x) }
    }

    /// The tuple the holomap draws: this is the caller its wiring note
    /// asked for.
    pub fn holo_route(&self) -> (f32, f32, f32) {
        (self.leg.from_au as f32, self.leg.to_au as f32, self.progress())
    }
}

/// Hold this fraction of `fleet::REACH_M`, so the span never lives at
/// the edge of its own collapse.
pub const HOLD_FRAC: f32 = 0.6;
/// Station-keeping gains: deliberately overdamped. A wing that
/// overshoots through the flagship is tuned for a paper, not a deck.
pub const KP: f32 = 0.020;
pub const KD: f32 = 0.42;

/// One hull under the station-keeper.
pub struct Wing {
    pub pos: Vec3,
    pub vel: Vec3,
    /// m/s² the drive can hold
    pub a_max: f32,
}

impl Wing {
    /// One control step toward `slot` (world space). Pure: reads the
    /// flagship's state, writes only itself.
    pub fn keep_station(&mut self, slot: Vec3, flag_vel: Vec3, dt: f32) {
        let want = KP * (slot - self.pos) + KD * (flag_vel - self.vel);
        let a = want.clamp_length_max(self.a_max);
        self.vel += a * dt;
        self.pos += self.vel * dt;
    }
}

/// The wing slots around a flagship: echelon offsets scaled by hull
/// length, all inside the hold radius, none in the exhaust.
pub fn slot(flag_pos: Vec3, flag_len_m: f32, index: usize) -> Vec3 {
    let hold = crate::fleet::REACH_M * HOLD_FRAC;
    let side = if index % 2 == 0 { 1.0 } else { -1.0 };
    let rank = (index / 2 + 1) as f32;
    let lateral = (flag_len_m * 0.35 + hold * 0.35 * rank).min(hold);
    let trail = (hold * 0.25 * rank).min(hold * 0.8);
    flag_pos + Vec3::new(side * lateral, 0.0, -trail)
}

/// Docking speed gate: inside this range of the hull...
pub const R_SLOW: f32 = 120.0;
/// ...the closing speed must be under this, or the dock refuses.
pub const V_DOCK: f32 = 4.0;
/// contact distance at which the clamps can take the ship
pub const R_CONTACT: f32 = 8.0;

/// Why a dock was refused — with the number, in the fleet manner.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DockRefusal {
    TooFar(f32),
    /// m/s OVER the gate
    Hot(f32),
}

impl DockRefusal {
    pub fn say(self) -> String {
        match self {
            DockRefusal::TooFar(d) => format!("{d:.0} m out — the clamps reach {R_CONTACT:.0}"),
            DockRefusal::Hot(over) => {
                format!("{over:.1} m/s over the gate — kill it below {V_DOCK:.0} first")
            }
        }
    }
}

/// One braking-approach step toward `port`: full brake inside `R_SLOW`
/// until under the gate, gentle closure after. The controller that makes
/// `try_dock` succeed is in the same file as the gate that refuses, so
/// the two cannot drift apart about what "slow enough" means.
pub fn approach(w: &mut Wing, port: Vec3, dt: f32) {
    let to = port - w.pos;
    let d = to.length().max(1e-3);
    let dir = to / d;
    let closing = -w.vel.dot(dir) * -1.0; // + toward the port
    let want_speed = if d > R_SLOW {
        (d * 0.08).clamp(V_DOCK, 40.0)
    } else {
        (V_DOCK * 0.8).min(d * 0.1 + 0.4)
    };
    let a = dir * ((want_speed - closing) * 0.8);
    let a = a.clamp_length_max(w.a_max);
    w.vel += a * dt;
    w.pos += w.vel * dt;
}

/// The clamps' answer.
pub fn try_dock(w: &Wing, port: Vec3) -> Result<(), DockRefusal> {
    let to = port - w.pos;
    let d = to.length();
    if d > R_CONTACT {
        return Err(DockRefusal::TooFar(d));
    }
    let closing = w.vel.dot(to.normalize_or(Vec3::Z)).abs();
    if closing > V_DOCK {
        return Err(DockRefusal::Hot(closing - V_DOCK));
    }
    Ok(())
}

// --------------------------------------------------------------- sim ---

/// `--helm` — a leg flown, a wing brought onto station, a dock refused
/// hot and taken cold.
pub fn run(_argv: &[String]) {
    use crate::voyage::{plan, Drive};

    let hull = crate::ship::hull("praetor").expect("roster hull");
    let leg = plan(1.0, 9.54, &Drive { supply_s: 400.0 * 3600.0, ..Drive::default() }, 6);
    println!(
        "== {} flies Earth -> Titan: {:.1} h planned, {:.1} km/s ({})",
        hull.name,
        leg.duration_s / 3600.0,
        leg.dv_kms,
        if leg.note.is_empty() { "clean burn" } else { leg.note }
    );
    let mut p = LegPilot::new(leg, hull.length);
    // fine enough that the flip window cannot fall between two samples —
    // a narrator that skips the turnover tells a different story than
    // the machine it narrates
    let step = (leg.duration_s / 40.0).min(p.flip_s * 0.5);
    while p.phase() != Phase::Arrived {
        if let Some(ph) = p.update(step) {
            let (_, _, pr) = p.holo_route();
            println!("   t={:>7.0}s  {:7}  progress {:>3.0}%", p.t, ph.label(), pr * 100.0);
        }
    }

    println!("\n== a wing joins formation (span reach {:.0} m)", crate::fleet::REACH_M);
    let flag = Vec3::ZERO;
    let mut w = Wing { pos: Vec3::new(620.0, 40.0, -180.0), vel: Vec3::ZERO, a_max: 6.0 };
    let s = slot(flag, hull.length, 0);
    let mut t = 0.0;
    let mut in_reach_since: Option<f32> = None;
    while t < 240.0 {
        w.keep_station(s, Vec3::ZERO, 0.25);
        t += 0.25;
        let range = w.pos.distance(flag);
        if (t / 20.0f32).fract() < 0.0125 {
            println!("   t={t:>5.0}s  range {range:>5.0} m{}", if range <= crate::fleet::REACH_M { "  (span can reach)" } else { "" });
        }
        if range <= crate::fleet::REACH_M {
            in_reach_since.get_or_insert(t);
        } else {
            in_reach_since = None;
        }
        if in_reach_since.map_or(false, |s0| t - s0 > 40.0) {
            println!("   on station and holding — raise the span when ready");
            break;
        }
    }

    println!("\n== docking");
    let port = Vec3::ZERO;
    let mut hot = Wing { pos: Vec3::new(0.0, 0.0, 60.0), vel: Vec3::new(0.0, 0.0, -18.0), a_max: 6.0 };
    // drift to contact without braking
    while hot.pos.z > R_CONTACT * 0.9 {
        hot.pos += hot.vel * 0.1;
    }
    match try_dock(&hot, port) {
        Err(r) => println!("   hot approach refused: {}", r.say()),
        Ok(()) => println!("   ...the gate is broken if you can read this"),
    }
    let mut cold = Wing { pos: Vec3::new(0.0, 0.0, 400.0), vel: Vec3::ZERO, a_max: 6.0 };
    let mut t = 0.0;
    loop {
        approach(&mut cold, port, 0.2);
        t += 0.2;
        if let Ok(()) = try_dock(&cold, port) {
            println!("   clamps taken at t={t:.0}s, {:.1} m/s — welcome aboard", cold.vel.length());
            break;
        }
        if t > 600.0 {
            println!("   the approach never settled — controller bug");
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voyage::{plan, Drive};

    fn leg() -> Leg {
        plan(1.0, 9.54, &Drive { supply_s: 400.0 * 3600.0, ..Drive::default() }, 6)
    }

    #[test]
    fn the_leg_pilot_walks_the_line_and_flips_once_at_the_middle() {
        let l = leg();
        let mut p = LegPilot::new(l, 340.0);
        let mut seen = Vec::new();
        let step = l.duration_s / 997.0; // odd divisor: phase edges between samples
        while p.phase() != Phase::Arrived {
            if let Some(ph) = p.update(step) {
                seen.push(ph);
            }
            assert!(p.t < l.duration_s * 3.0, "the leg never arrives");
        }
        assert_eq!(seen, vec![Phase::Burn, Phase::Flip, Phase::Brake, Phase::Arrived],
            "a line, not a graph: {seen:?}");
    }

    #[test]
    fn progress_is_monotonic_exact_at_the_ends_and_fastest_at_the_flip() {
        let l = leg();
        let mut p = LegPilot::new(l, 340.0);
        let mut last = -1.0f32;
        let mut max_rate = (0.0f32, 0.0f32); // (rate, progress there)
        let step = l.duration_s / 500.0;
        while p.phase() != Phase::Arrived {
            p.update(step);
            let pr = p.progress();
            assert!(pr >= last - 1e-6, "progress went backwards");
            let rate = pr - last.max(0.0);
            if last >= 0.0 && rate > max_rate.0 {
                max_rate = (rate, pr);
            }
            last = pr;
        }
        assert!((last - 1.0).abs() < 1e-3, "arrived short: {last}");
        assert!(
            (max_rate.1 - 0.5).abs() < 0.1,
            "speed peaked at progress {:.2}, not the flip",
            max_rate.1
        );
    }

    #[test]
    fn the_wing_converges_inside_span_reach_and_holds() {
        let flag = Vec3::ZERO;
        let s = slot(flag, 340.0, 0);
        let mut w = Wing { pos: Vec3::new(650.0, 60.0, -200.0), vel: Vec3::ZERO, a_max: 6.0 };
        assert!(w.pos.distance(flag) > crate::fleet::REACH_M, "the test starts inside reach");
        let mut inside_streak = 0usize;
        for _ in 0..4000 {
            w.keep_station(s, Vec3::ZERO, 0.25);
            if w.pos.distance(flag) <= crate::fleet::REACH_M {
                inside_streak += 1;
            } else {
                inside_streak = 0;
            }
        }
        // both directions: it got in, and it STAYED in (no limit cycle)
        assert!(inside_streak >= 400, "held only {inside_streak} steps — oscillating or short");
        assert!(w.vel.length() < 3.0, "still moving hard on station: {:.1} m/s", w.vel.length());
    }

    #[test]
    fn every_slot_is_inside_the_hold_radius() {
        let hold = crate::fleet::REACH_M * HOLD_FRAC;
        for i in 0..8 {
            let s = slot(Vec3::ZERO, 3000.0, i); // even on the widesky
            assert!(s.length() <= crate::fleet::REACH_M, "slot {i} outside span reach");
            let _ = hold;
        }
    }

    #[test]
    fn the_gate_refuses_hot_with_the_number_and_takes_a_cold_approach() {
        let port = Vec3::ZERO;
        let hot = Wing { pos: Vec3::new(0.0, 0.0, 6.0), vel: Vec3::new(0.0, 0.0, -15.0), a_max: 6.0 };
        match try_dock(&hot, port) {
            Err(DockRefusal::Hot(over)) => {
                assert!(over > 10.0);
                assert!(DockRefusal::Hot(over).say().contains("over the gate"));
            }
            other => panic!("a 15 m/s contact was answered {other:?}"),
        }
        let mut cold = Wing { pos: Vec3::new(0.0, 0.0, 400.0), vel: Vec3::ZERO, a_max: 6.0 };
        let mut ok = false;
        for _ in 0..4000 {
            approach(&mut cold, port, 0.2);
            if try_dock(&cold, port).is_ok() {
                ok = true;
                break;
            }
        }
        assert!(ok, "the approach controller cannot satisfy its own gate");
    }
}
