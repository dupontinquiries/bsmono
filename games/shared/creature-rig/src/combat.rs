//! combat — the timers that make a fight read on the body.
//!
//! `anim_rig::MotionInput` is a per-frame SNAPSHOT: `state`, `progress`,
//! `strike`, `impact`, `leg_health`. Something has to move those over time,
//! and until now nothing did — the rig harness set them by hand each frame
//! and the games left them at their defaults, so a creature being hit,
//! casting, or squaring up looked exactly like one walking.
//!
//! This is that something. Events go in (`hit`, `begin_cast`, `begin_strike`),
//! time passes (`tick`), and [`Combat::apply`] fills a `MotionInput`.
//!
//! ## It drives the EXISTING fields, deliberately
//!
//! The tempting move was to add flinch and guard fields to `MotionInput` and
//! teach `pose()` about them. That widens this crate's divergence from
//! `unendlich-engine`, which it was extracted from byte-identical and which
//! still carries its own copy — every field added here is a merge conflict
//! there later. So:
//!
//! | combat concept | existing field it drives |
//! |---|---|
//! | taking a hit | `impact` — a squash, which is what a flinch looks like |
//! | casting | `state = Casting` + `progress`, through `cast_curve` |
//! | lunging | `strike`, which already has anticipation/recover shaping |
//! | wounded | `leg_health`, which already scales swing into a limp |
//! | guarded stance | `swing_scale`, tightened |
//!
//! Only a DIRECTIONAL flinch genuinely needs new geometry — being hit from
//! behind should pitch the body differently from being hit in the flank — and
//! that is left out rather than smuggled in. See [`Combat::flinch_dir`],
//! which computes it and documents what it is waiting for.
//!
//! ## Timers, not booleans
//!
//! A "in combat" flag makes a body snap between two carriages. Every value
//! here decays, because the interesting part of a fight is the settling: a
//! creature that was hit a moment ago is still shaking it off, and a creature
//! that has not been hit for ten seconds has dropped its guard. A flag cannot
//! express either.

use crate::anim_rig::MotionInput;
use crate::sim::liveliness::MotionState;

/// How long a hit reaction lasts, seconds.
pub const FLINCH_SECS: f32 = 0.35;
/// How long after the last hit a creature stays visibly guarded.
pub const GUARD_SECS: f32 = 6.0;
/// A guarded body moves tighter than a travelling one.
pub const GUARD_SWING: f32 = 0.82;

/// A timed overlay: `t` counts down from `dur`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Timer {
    t: f32,
    dur: f32,
}

impl Timer {
    fn start(&mut self, dur: f32) {
        self.dur = dur.max(1e-3);
        self.t = self.dur;
    }
    fn tick(&mut self, dt: f32) {
        self.t = (self.t - dt).max(0.0);
    }
    fn active(&self) -> bool {
        self.t > 0.0
    }
    /// 0 at the start of the timer, 1 at its end — the direction
    /// `liveliness::cast_curve` and `anim_rig::strike_curve` both expect.
    fn progress(&self) -> f32 {
        if self.dur <= 0.0 {
            return 1.0;
        }
        (1.0 - self.t / self.dur).clamp(0.0, 1.0)
    }
    /// 1 at the start, decaying to 0 — for things that fade rather than play.
    fn remaining(&self) -> f32 {
        if self.dur <= 0.0 {
            return 0.0;
        }
        (self.t / self.dur).clamp(0.0, 1.0)
    }
}

/// Everything a fight does to a body.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Combat {
    flinch: Timer,
    /// Body-local direction the last hit came FROM, `[fore, right]`,
    /// normalised-ish. Kept for [`Self::flinch_dir`].
    flinch_from: [f32; 2],
    /// Severity of the last hit, 0..1.
    flinch_power: f32,
    cast: Timer,
    strike: Timer,
    /// Seconds since the last hit; saturates at [`GUARD_SECS`].
    since_hit: f32,
    /// 0..1 health, driving the limp.
    health: f32,
    dying: bool,
    death: Timer,
}

impl Combat {
    pub fn new() -> Combat {
        Combat { health: 1.0, since_hit: GUARD_SECS, ..Default::default() }
    }

    /// Took damage. `from` is the body-local direction the blow came from as
    /// `[fore, right]`; `severity` 0..1; `health` the fraction remaining.
    ///
    /// A new hit RESTARTS the flinch rather than adding to it. Accumulating
    /// would let a fast attacker pin a body in permanent recoil, which reads
    /// as a seizure rather than a beating — and the recoil is meant to show
    /// each blow, so the last one is the one worth showing.
    pub fn hit(&mut self, from: [f32; 2], severity: f32, health: f32) {
        let s = severity.clamp(0.0, 1.0);
        // A glancing hit must not cancel the tail of a heavy one.
        if !self.flinch.active() || s >= self.flinch_power {
            self.flinch.start(FLINCH_SECS);
            self.flinch_power = s;
            let len = (from[0] * from[0] + from[1] * from[1]).sqrt();
            self.flinch_from = if len > 1e-4 { [from[0] / len, from[1] / len] } else { [0.0, 0.0] };
        }
        self.since_hit = 0.0;
        self.health = health.clamp(0.0, 1.0);
    }

    pub fn begin_cast(&mut self, secs: f32) {
        self.cast.start(secs);
    }
    pub fn begin_strike(&mut self, secs: f32) {
        self.strike.start(secs);
    }

    /// Start dying. `secs` is how long the body takes to go limp.
    pub fn begin_death(&mut self, secs: f32) {
        self.dying = true;
        self.death.start(secs);
    }

    pub fn is_dying(&self) -> bool {
        self.dying
    }
    pub fn health(&self) -> f32 {
        self.health
    }

    /// Is the creature carrying itself as if a fight is on?
    ///
    /// Decays over [`GUARD_SECS`] rather than flipping, so a body relaxes out
    /// of a fight instead of snapping out of it.
    pub fn guard(&self) -> f32 {
        if self.dying {
            return 0.0;
        }
        (1.0 - self.since_hit / GUARD_SECS).clamp(0.0, 1.0)
    }

    /// Current flinch strength, 0..1, decaying.
    pub fn flinch(&self) -> f32 {
        self.flinch.remaining() * self.flinch_power
    }

    /// Where the current flinch should throw the body, `[fore, right]`,
    /// scaled by its strength.
    ///
    /// NOT applied by [`Self::apply`], because `MotionInput` has no
    /// directional field and adding one would deepen this crate's divergence
    /// from the `unendlich-engine` copy it was extracted from. Exposed so a
    /// caller that owns its own body transform can use it today — offsetting
    /// the whole creature is a reasonable approximation — and so the value is
    /// already computed and tested when a `flinch: [f32; 2]` field is
    /// eventually worth adding to `pose()`.
    pub fn flinch_dir(&self) -> [f32; 2] {
        let f = self.flinch();
        [self.flinch_from[0] * f, self.flinch_from[1] * f]
    }

    pub fn tick(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.25);
        self.flinch.tick(dt);
        self.cast.tick(dt);
        self.strike.tick(dt);
        self.death.tick(dt);
        self.since_hit = (self.since_hit + dt).min(GUARD_SECS);
    }

    /// Which liveliness state the body is in, and how far through it.
    ///
    /// Priority is dying, then casting, then whatever locomotion says.
    /// Striking deliberately does NOT take the state: a strike is an overlay
    /// on top of a moving body (`MotionInput::strike`), and forcing the state
    /// would make a lunging creature stop walking mid-lunge.
    pub fn state(&self, moving: bool) -> (MotionState, f32) {
        if self.dying {
            return (MotionState::Dying, self.death.progress());
        }
        if self.cast.active() {
            return (MotionState::Casting, self.cast.progress());
        }
        (if moving { MotionState::Walking } else { MotionState::Idle }, 0.0)
    }

    /// Write everything onto a `MotionInput`.
    ///
    /// Leaves `gait_phase`, `speed` and `turn` alone — those belong to
    /// locomotion, and a combat overlay that moved them would be fighting the
    /// gait selector for the same fields.
    pub fn apply(&self, m: &mut MotionInput, moving: bool) {
        let (state, progress) = self.state(moving);
        m.state = state;
        m.progress = progress;
        m.strike = if self.strike.active() { self.strike.progress() } else { 0.0 };
        // A hit reads as a squash, which is what `impact` already does for a
        // landing. Reusing it rather than adding a field is the whole point;
        // see the module header.
        m.impact = self.flinch().max(m.impact);
        // A wounded creature limps: leg_health already scales the swing
        // asymmetrically. Floored at 0.3 the way the rig harness does, since
        // a leg_health of 0 stops the legs entirely and a dying creature
        // should fall, not freeze.
        m.leg_health = self.health.max(0.3);
        // A guarded body is tighter. Multiplied into whatever the gait asked
        // for rather than replacing it, so a bounding guarded cat is still
        // bounding.
        m.swing_scale *= 1.0 - (1.0 - GUARD_SWING) * self.guard();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(c: &mut Combat, secs: f32) {
        let dt = 1.0 / 60.0;
        for _ in 0..((secs / dt) as i32) {
            c.tick(dt);
        }
    }

    #[test]
    fn a_hit_flinches_then_settles() {
        let mut c = Combat::new();
        c.hit([1.0, 0.0], 1.0, 0.8);
        assert!(c.flinch() > 0.9, "full recoil at the moment of the hit");
        run(&mut c, FLINCH_SECS + 0.1);
        assert_eq!(c.flinch(), 0.0, "and it must settle, not persist");
    }

    /// A fast attacker must not pin a body in permanent recoil -- that reads
    /// as a seizure. A new hit restarts the flinch; it does not accumulate.
    #[test]
    fn repeated_hits_restart_the_flinch_rather_than_stacking_it() {
        let mut c = Combat::new();
        for _ in 0..10 {
            c.hit([1.0, 0.0], 1.0, 0.5);
            run(&mut c, 0.05);
        }
        assert!(c.flinch() <= 1.0, "never exceeds full: {}", c.flinch());
    }

    /// ...but a glancing blow must not cancel the tail of a heavy one.
    #[test]
    fn a_light_hit_does_not_cut_short_a_heavy_ones_recoil() {
        let mut c = Combat::new();
        c.hit([1.0, 0.0], 1.0, 0.5);
        run(&mut c, 0.05);
        let heavy = c.flinch();
        c.hit([1.0, 0.0], 0.1, 0.5);
        assert!(c.flinch() >= heavy - 1e-3, "light hit weakened the recoil: {} -> {}", heavy, c.flinch());
    }

    /// Direction is kept so the body can be thrown the right way -- the one
    /// thing that genuinely needs a new rig field.
    #[test]
    fn the_flinch_remembers_which_way_the_blow_came_from() {
        let mut c = Combat::new();
        c.hit([0.0, 3.0], 1.0, 1.0);
        let d = c.flinch_dir();
        assert!(d[1] > 0.5, "thrown along +right: {d:?}");
        assert!(d[0].abs() < 1e-3, "and not along fore: {d:?}");
        // Normalised: a huge input vector does not produce a huge throw.
        assert!(d[1] <= 1.0);
        // Decays with the flinch.
        run(&mut c, FLINCH_SECS + 0.1);
        assert_eq!(c.flinch_dir(), [0.0, 0.0]);
    }

    /// Guard decays instead of flipping, so a body relaxes out of a fight.
    #[test]
    fn guard_rises_on_a_hit_and_decays_over_seconds() {
        let mut c = Combat::new();
        assert_eq!(c.guard(), 0.0, "a fresh creature is not braced");
        c.hit([1.0, 0.0], 0.5, 1.0);
        assert!(c.guard() > 0.99);
        run(&mut c, GUARD_SECS * 0.5);
        let mid = c.guard();
        assert!(mid > 0.2 && mid < 0.8, "mid-decay, got {mid}");
        run(&mut c, GUARD_SECS);
        assert_eq!(c.guard(), 0.0);
    }

    /// Casting owns the state; striking does not, because a lunge is an
    /// overlay on a moving body.
    #[test]
    fn casting_takes_the_state_but_striking_does_not() {
        let mut c = Combat::new();
        c.begin_cast(1.0);
        assert_eq!(c.state(false).0, MotionState::Casting);
        run(&mut c, 1.1);
        assert_eq!(c.state(false).0, MotionState::Idle);

        c.begin_strike(0.5);
        assert_eq!(c.state(true).0, MotionState::Walking, "a lunging creature keeps walking");
        // `progress` is 0 at the START of a timer (the direction strike_curve
        // expects: anticipation first), so the overlay reads 0.0 on the frame
        // the strike begins and climbs from the first tick. The assertion
        // used to come before any tick and could never pass — the test and
        // the timer landed in the same commit (1f11f2004) and disagreed.
        let mut m = MotionInput::default();
        c.apply(&mut m, true);
        assert_eq!(m.strike, 0.0, "the first frame is the anticipation's start");
        c.tick(1.0 / 60.0);
        c.apply(&mut m, true);
        assert!(m.strike > 0.0 && m.strike < 0.1, "but the strike overlay is set once it is under way: {}", m.strike);
        run(&mut c, 0.6);
        c.apply(&mut m, true);
        assert_eq!(m.strike, 0.0, "and gone when the strike is over");
    }

    /// Dying outranks everything -- a creature hit while dying must not snap
    /// back to Walking.
    #[test]
    fn dying_outranks_casting_and_movement() {
        let mut c = Combat::new();
        c.begin_cast(2.0);
        c.begin_death(1.0);
        assert_eq!(c.state(true).0, MotionState::Dying);
        c.hit([1.0, 0.0], 1.0, 0.0);
        assert_eq!(c.state(true).0, MotionState::Dying);
        assert_eq!(c.guard(), 0.0, "a dying body is not braced");
    }

    /// Death progress runs 0 -> 1 so `limpness` hands the body to the ragdoll.
    #[test]
    fn death_progress_runs_forward_to_fully_limp() {
        let mut c = Combat::new();
        c.begin_death(1.0);
        assert!(c.state(false).1 < 0.1);
        run(&mut c, 1.0);
        assert!(c.state(false).1 > 0.9, "got {}", c.state(false).1);
    }

    /// apply() must drive the rig's own fields, not invent new ones.
    #[test]
    fn apply_fills_the_existing_motion_input_fields() {
        let mut c = Combat::new();
        c.hit([1.0, 0.0], 1.0, 0.45);
        let mut m = MotionInput::default();
        let before_swing = m.swing_scale;
        c.apply(&mut m, true);
        assert_eq!(m.state, MotionState::Walking);
        assert!(m.impact > 0.9, "a hit squashes: {}", m.impact);
        assert!((m.leg_health - 0.45).abs() < 1e-3, "wounded legs limp: {}", m.leg_health);
        assert!(m.swing_scale < before_swing, "a guarded body moves tighter");
    }

    /// leg_health floors above zero: a dying creature should fall, not freeze
    /// with its legs stopped.
    #[test]
    fn a_creature_at_zero_health_still_has_moving_legs() {
        let mut c = Combat::new();
        c.hit([1.0, 0.0], 1.0, 0.0);
        let mut m = MotionInput::default();
        c.apply(&mut m, true);
        assert!(m.leg_health >= 0.3, "got {}", m.leg_health);
    }

    /// The overlay must not fight the gait selector for locomotion's fields.
    #[test]
    fn apply_leaves_locomotion_alone() {
        let mut c = Combat::new();
        c.hit([1.0, 0.0], 1.0, 0.5);
        let mut m = MotionInput { gait_phase: 1.23, speed: 7.0, turn: -0.4, ..Default::default() };
        c.apply(&mut m, true);
        assert_eq!(m.gait_phase, 1.23);
        assert_eq!(m.speed, 7.0);
        assert_eq!(m.turn, -0.4);
    }

    /// A long stalled frame must not run the timers backwards or past zero.
    #[test]
    fn a_pathological_dt_is_survivable() {
        let mut c = Combat::new();
        c.hit([1.0, 0.0], 1.0, 0.5);
        c.begin_cast(0.5);
        for _ in 0..5 {
            c.tick(10.0);
            assert!(c.flinch() >= 0.0 && c.flinch() <= 1.0);
            assert!(c.guard() >= 0.0 && c.guard() <= 1.0);
            assert!(c.state(false).1.is_finite());
        }
        assert_eq!(c.flinch(), 0.0);
    }

    /// A fresh creature reads as unharmed, not as a corpse with zero health.
    #[test]
    fn a_new_combat_is_unhurt_and_unbraced() {
        let c = Combat::new();
        assert_eq!(c.health(), 1.0);
        assert!(!c.is_dying());
        assert_eq!(c.flinch(), 0.0);
        assert_eq!(c.guard(), 0.0);
        let mut m = MotionInput::default();
        c.apply(&mut m, false);
        assert_eq!(m.state, MotionState::Idle);
        assert_eq!(m.leg_health, 1.0);
    }
}
