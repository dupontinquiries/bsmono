//! anim — which gait a species uses, and how hard.
//!
//! This module used to contain a whole pose solver. It does not any more, and
//! the reason is worth recording: `unendlich-engine` already had one, better
//! than the one written here, and it has now been extracted to the shared
//! `creature-rig` crate. Its header diagnoses clawbound's exact bug in the
//! same words this module was written to fix — *"a leg that translates
//! instead of pivoting at the hip never plants"*.
//!
//! What was genuinely missing there, and is what remains here:
//!
//! * **Gait taxonomy.** `creature_rig::anim_rig` applies one diagonal-pair
//!   pattern (plus a `hop` flag). Real quadrupeds change *footfall pattern*
//!   with speed, not just frequency.
//! * **Species builds.** A bison must never bound and a mammoth must never
//!   leave a walk. That is a fact about animals, not about posing boxes, so
//!   it belongs to the game rather than to the rig.
//!
//! So: this picks the gait and shapes the drive; `creature-rig` does the
//! geometry. [`Anim::motion_input`] is the join.
//!
//! ## Footfall patterns now reach the rig
//!
//! `anim_rig` originally hard-coded one pattern (diagonal pairs, plus a `hop`
//! flag), so `Walk`, `Pace` and `Canter` all rendered as a trot. It now takes
//! a `leg_phases` table and [`Gait::leg_phases`] supplies it, so the whole
//! taxonomy renders.
//!
//! One thing to keep in mind when adding a creature: the table is indexed
//! `[FL, FR, HL, HR]` and legs must be supplied to the rig in that order.
//! Getting it wrong yields a different real gait — a pace instead of a trot —
//! rather than an error, which is exactly the trap the rig's own built-in
//! rule fell into.

use creature_rig::anim_rig::{MotionInput, SPEED_SATURATE};
use creature_rig::sim::liveliness::MotionState;

/// A quadruped footfall pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gait {
    /// 4-beat lateral sequence. Slow, always three feet down — what heavy
    /// animals use and what reads as "walking".
    Walk,
    /// 2-beat diagonal pairs. The default cruise, and the only pattern
    /// `anim_rig` natively draws.
    Trot,
    /// 2-beat lateral pairs. A stalking cat's creep.
    Pace,
    /// 3-beat, one lead.
    Canter,
    /// Front pair then hind pair. The cat sprint, and what a dash is.
    Bound,
}

impl Gait {
    /// Does this pattern move both legs of a pair together?
    pub fn is_paired(self) -> bool {
        matches!(self, Gait::Bound)
    }

    /// The rig's per-leg phase table for this gait.
    ///
    /// Indexed `[FL, FR, HL, HR]`, which is the order this game supplies
    /// `Role::Leg` parts in. That ordering is load-bearing — see the presets'
    /// docs in `creature_rig::anim_rig`: supplying legs in a different order
    /// and reusing these tables produces a different real gait rather than an
    /// error.
    pub fn leg_phases(self) -> [f32; 4] {
        use creature_rig::anim_rig as rig;
        match self {
            Gait::Walk => rig::LEG_PHASES_WALK,
            Gait::Trot => rig::LEG_PHASES_TROT,
            Gait::Pace => rig::LEG_PHASES_PACE,
            Gait::Canter => rig::LEG_PHASES_CANTER,
            Gait::Bound => rig::LEG_PHASES_BOUND,
        }
    }

    /// Fraction of the cycle a foot is on the ground. Faster gaits have a
    /// real suspension phase; a walk never leaves the floor.
    pub fn duty(self) -> f32 {
        match self {
            Gait::Walk => 0.65,
            Gait::Trot => 0.50,
            Gait::Pace => 0.50,
            Gait::Canter => 0.40,
            Gait::Bound => 0.32,
        }
    }

    /// How much limb swing this gait wants, as a multiplier on the rig's
    /// `swing_scale`. A bound throws the legs; a walk does not.
    pub fn swing_scale(self) -> f32 {
        match self {
            Gait::Walk => 0.75,
            Gait::Trot => 1.00,
            Gait::Pace => 0.85,
            Gait::Canter => 1.15,
            Gait::Bound => 1.35,
        }
    }
}

/// A species' body and gait ladder.
///
/// Speeds are absolute world units/sec rather than normalised, because a
/// bison and a hare moving at the same 8 u/s are doing genuinely different
/// things.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Build {
    /// Shoulder height, world units. Sets stride length.
    pub height: f32,
    pub length: f32,
    /// 0 = light, 1 = bison-heavy.
    pub heft: f32,
    pub walk_to: f32,
    pub trot_to: f32,
    /// The gait above `trot_to`.
    pub top_gait: Gait,
    pub can_pace: bool,
}

impl Build {
    pub const fn feline() -> Build {
        Build { height: 1.1, length: 2.2, heft: 0.35, walk_to: 2.5, trot_to: 7.0, top_gait: Gait::Bound, can_pace: true }
    }
    pub const fn mini_cat() -> Build {
        Build { height: 0.6, length: 1.2, heft: 0.20, walk_to: 2.0, trot_to: 6.0, top_gait: Gait::Bound, can_pace: true }
    }
    pub const fn hare() -> Build {
        Build { height: 0.4, length: 0.7, heft: 0.10, walk_to: 1.5, trot_to: 3.0, top_gait: Gait::Bound, can_pace: false }
    }
    pub const fn stag() -> Build {
        Build { height: 1.5, length: 2.4, heft: 0.45, walk_to: 2.5, trot_to: 8.0, top_gait: Gait::Canter, can_pace: false }
    }
    /// Bison-class: heavy, walks and trots, never bounds.
    pub const fn megafauna() -> Build {
        Build { height: 2.4, length: 4.2, heft: 1.0, walk_to: 2.0, trot_to: 6.0, top_gait: Gait::Canter, can_pace: false }
    }
    /// Mammoth-class. Never leaves a walk: a suspension phase on something
    /// this heavy reads as weightless, which is the opposite of what the
    /// silhouette promises.
    pub const fn colossus() -> Build {
        Build { height: 4.0, length: 7.0, heft: 1.0, walk_to: 3.0, trot_to: f32::INFINITY, top_gait: Gait::Walk, can_pace: false }
    }

    pub fn gait_for(&self, speed: f32) -> Gait {
        // `top_gait` is a CEILING, not just "what comes after trot". A
        // colossus is `top_gait: Walk` with `trot_to: INFINITY`, and without
        // this guard the `<= trot_to` arm below is always true for it, so it
        // trotted at anything over `walk_to`. The test that says a mammoth
        // never leaves a walk caught this the first time it was compiled.
        if self.top_gait == Gait::Walk && speed > self.walk_to {
            return Gait::Walk;
        }
        if speed <= self.walk_to {
            if self.can_pace && speed < self.walk_to * 0.45 {
                Gait::Pace
            } else {
                Gait::Walk
            }
        } else if speed <= self.trot_to {
            Gait::Trot
        } else {
            self.top_gait
        }
    }

    /// The gait, capped by what the animal can still sustain.
    ///
    /// Step 3 of `docs/WARRIORS_MECHANICS.md`. `wildkit::sim::stamina` came
    /// across with the right shape already -- `can_sprint()` tests against
    /// `SPRINT_FLOOR` rather than against zero, which IS the panting halt:
    /// once spent you cannot sprint again until recovered past the floor, so a
    /// chase costs something that outlives the chase.
    ///
    /// ## Stamina caps the GAIT, it does not slow the animal
    ///
    /// The tempting wiring is to scale `speed` by the stamina multiplier and
    /// feed that to `gait_for`. That is wrong twice over. It makes an
    /// exhausted animal animate as though it were moving slowly while it is
    /// still covering ground, so the feet skate; and it silently changes what
    /// the caller asked for, since `speed` is a measurement of the world and
    /// not a request.
    ///
    /// An exhausted animal moving fast is still moving fast. What it cannot do
    /// is SUSTAIN a suspension phase, so it drops to the next gait down and
    /// takes more steps to cover the same ground. That is what a spent horse
    /// or a spent cat actually looks like.
    pub fn gait_for_spent(&self, speed: f32, stamina: &wildkit::sim::stamina::Stamina) -> Gait {
        let want = self.gait_for(speed);
        if stamina.can_sprint() {
            return want;
        }
        // Below the floor, the suspension gaits are off the table. Walk and
        // Trot are not: an exhausted animal still walks, and refusing to let
        // it would leave it frozen mid-field.
        match want {
            Gait::Bound | Gait::Canter => Gait::Trot,
            other => other,
        }
    }

    /// The speed at which this animal is "going flat out" for the rig's
    /// stretch and swing saturation. `trot_to` for most; `walk_to` for an
    /// animal whose ladder stops at a walk, whose `trot_to` is infinite.
    pub fn cruise_top(&self) -> f32 {
        if self.trot_to.is_finite() { self.trot_to } else { self.walk_to }
    }

    /// Stride frequency (cycles/sec).
    ///
    /// Derived from stride LENGTH, not picked per gait, so the feet keep up
    /// with the ground instead of skating — and so a colossus takes slow
    /// enormous steps while a hare buzzes, from one rule.
    pub fn stride_hz(&self, speed: f32) -> f32 {
        let stride_len = (self.height * 1.6).max(0.2);
        (speed / stride_len).clamp(0.0, 12.0)
    }
}

/// Per-creature animation state: the gait phase and the smoothing that keeps
/// a gait change from snapping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anim {
    pub build: Build,
    /// Gait phase in RADIANS, matching `anim_rig::MotionInput::gait_phase`.
    pub gait_phase: f32,
    /// Seconds, for the rig's breath cycle.
    pub time: f32,
    speed_s: f32,
    gait: Gait,
}

impl Anim {
    pub fn new(build: Build) -> Anim {
        Anim { build, gait_phase: 0.0, time: 0.0, speed_s: 0.0, gait: Gait::Walk }
    }

    pub fn gait(&self) -> Gait {
        self.gait
    }
    pub fn smoothed_speed(&self) -> f32 {
        self.speed_s
    }

    /// Advance the phase. `airborne` freezes it — feet do not cycle in the
    /// air, they reach for the ground.
    pub fn advance(&mut self, dt: f32, speed: f32, airborne: bool) {
        let dt = dt.clamp(0.0, 0.25);
        self.time += dt;
        // Smooth before selecting: raw speed crossing a threshold every frame
        // in a fight makes the legs stutter between patterns, and the
        // creature should commit to a gait for a beat.
        self.speed_s += (speed - self.speed_s) * (dt * 6.0).min(1.0);
        self.gait = self.build.gait_for(self.speed_s);
        if !airborne {
            let hz = self.build.stride_hz(self.speed_s);
            self.gait_phase =
                (self.gait_phase + hz * dt * std::f32::consts::TAU) % std::f32::consts::TAU;
        }
    }

    /// Build the rig's input for this frame.
    ///
    /// `turn` is normalised `[-1,1]` (+ = right), matching `anim_rig`.
    /// `impact` is landing squash `[0,1]`.
    pub fn motion_input(&self, turn: f32, gaze: (f32, f32), impact: f32, state: MotionState, progress: f32) -> MotionInput {
        MotionInput {
            gait_phase: self.gait_phase,
            speed: self.speed_s,
            // Saturate relative to THIS animal's own top gait rather than the
            // rig's global constant: otherwise a colossus at its flat-out
            // 3 u/s registers as barely moving and never swings its legs,
            // while a hare pins the scale permanently.
            max_speed: self.build.cruise_top().min(SPEED_SATURATE * 2.0).max(1.0),
            leg_health: 1.0,
            flying: false,
            fly_power: 0.0,
            turn: turn.clamp(-1.0, 1.0),
            gaze_yaw: gaze.0,
            gaze_pitch: gaze.1,
            state,
            progress,
            time: self.time,
            impact: impact.clamp(0.0, 1.0),
            settle: 0.0,
            strike: 0.0,
            // The bound is expressed as a phase table rather than `hop`,
            // which collapses every leg to phase 0. The table says which
            // pairs, so a bound reads as front-then-hind instead of a
            // four-legged pogo.
            hop: false,
            leg_phases: Some(self.gait.leg_phases()),
            // Heavier animals swing less freely, on top of the gait's own
            // drive.
            swing_scale: self.gait.swing_scale() * (1.0 - 0.25 * self.build.heft),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bison must never bound and a mammoth must never leave a walk. This
    /// is the whole reason this layer exists on top of the shared rig.
    #[test]
    fn heavy_builds_cap_below_bounding() {
        assert_eq!(Build::megafauna().gait_for(50.0), Gait::Canter);
        assert_eq!(Build::colossus().gait_for(50.0), Gait::Walk);
        assert_eq!(Build::feline().gait_for(50.0), Gait::Bound);
    }

    #[test]
    fn gait_changes_up_with_speed() {
        let c = Build::feline();
        assert_eq!(c.gait_for(0.5), Gait::Pace, "a slow cat creeps");
        assert_eq!(c.gait_for(2.0), Gait::Walk);
        assert_eq!(c.gait_for(5.0), Gait::Trot);
        assert_eq!(c.gait_for(10.0), Gait::Bound);
    }

    /// Stride frequency from stride length, or the feet skate.
    #[test]
    fn small_builds_step_faster_at_the_same_speed() {
        let v = 5.0;
        assert!(Build::hare().stride_hz(v) > Build::feline().stride_hz(v));
        assert!(Build::feline().stride_hz(v) > Build::colossus().stride_hz(v));
        assert_eq!(Build::feline().stride_hz(0.0), 0.0);
    }

    /// Every gait must map to a distinct footfall table, or the taxonomy is
    /// decoration.
    #[test]
    fn each_gait_maps_to_a_distinct_phase_table() {
        use creature_rig::anim_rig as rig;
        assert_eq!(Gait::Trot.leg_phases(), rig::LEG_PHASES_TROT);
        assert_eq!(Gait::Bound.leg_phases(), rig::LEG_PHASES_BOUND);
        let all = [Gait::Walk, Gait::Trot, Gait::Pace, Gait::Canter, Gait::Bound];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.leg_phases(), b.leg_phases(), "{a:?} and {b:?} share a table");
            }
        }
        // A pace and a trot are the pair most easily confused, and the rig's
        // own default conflated them.
        assert_ne!(Gait::Pace.leg_phases(), Gait::Trot.leg_phases());
    }

    /// Bounding pairs the legs, and the table says so rather than the `hop`
    /// flag, which would collapse all four to one phase.
    #[test]
    fn bounding_pairs_front_then_hind_via_the_table() {
        let t = Gait::Bound.leg_phases();
        assert_eq!(t[0], t[1], "front pair together");
        assert_eq!(t[2], t[3], "hind pair together");
        assert!((t[0] - t[2]).abs() > 0.1, "and the pairs oppose");
        assert!(Gait::Bound.is_paired());
        for g in [Gait::Walk, Gait::Trot, Gait::Pace, Gait::Canter] {
            assert!(!g.is_paired(), "{g:?} must not pair");
        }
    }

    /// The rig must receive the table, and must not also get `hop` — which
    /// would override it.
    #[test]
    fn motion_input_carries_the_table_and_not_hop() {
        let mut a = Anim::new(Build::feline());
        for _ in 0..200 {
            a.advance(1.0 / 60.0, 12.0, false);
        }
        assert_eq!(a.gait(), Gait::Bound);
        let m = a.motion_input(0.0, (0.0, 0.0), 0.0, MotionState::Idle, 0.0);
        assert!(!m.hop, "hop would collapse the table");
        assert_eq!(m.leg_phases, Some(Gait::Bound.leg_phases()));
    }

    #[test]
    fn duty_falls_as_the_gait_quickens() {
        assert!(Gait::Walk.duty() > Gait::Trot.duty());
        assert!(Gait::Trot.duty() > Gait::Canter.duty());
        assert!(Gait::Canter.duty() > Gait::Bound.duty());
        assert!(Gait::Walk.duty() > 0.5, "a walk always has feet down");
        assert!(Gait::Bound.duty() < 0.5, "a bound has a suspension phase");
    }

    /// Feet must not cycle off the ground.
    #[test]
    fn airborne_freezes_the_phase() {
        let mut a = Anim::new(Build::feline());
        a.advance(0.1, 8.0, false);
        let moved = a.gait_phase;
        assert!(moved > 0.0);
        for _ in 0..20 {
            a.advance(1.0 / 60.0, 8.0, true);
        }
        assert_eq!(a.gait_phase, moved, "phase must freeze in the air");
    }

    /// Phase must stay in `[0, TAU)` — it feeds a rig that takes radians and
    /// runs for a whole match.
    #[test]
    fn phase_stays_bounded_over_a_long_match() {
        let mut a = Anim::new(Build::feline());
        for _ in 0..(60 * 60 * 10) {
            a.advance(1.0 / 60.0, 9.0, false);
        }
        assert!(a.gait_phase.is_finite());
        assert!((0.0..std::f32::consts::TAU).contains(&a.gait_phase), "got {}", a.gait_phase);
    }

    /// A gait change must not snap: smoothing means the selected gait lags a
    /// step change in speed rather than flipping on the frame.
    #[test]
    fn speed_is_smoothed_before_the_gait_is_chosen() {
        let mut a = Anim::new(Build::feline());
        a.advance(1.0 / 60.0, 20.0, false);
        assert!(a.smoothed_speed() < 20.0, "must not jump to the new speed");
        assert_ne!(a.gait(), Gait::Bound, "gait lags the step change");
        for _ in 0..120 {
            a.advance(1.0 / 60.0, 20.0, false);
        }
        assert_eq!(a.gait(), Gait::Bound, "and then commits");
    }

    /// `max_speed` must be per-animal, or a colossus flat-out reads as
    /// standing still and never swings a leg.
    #[test]
    fn saturation_speed_is_per_build_not_global() {
        let colossus = Anim::new(Build::colossus()).motion_input(0.0, (0.0, 0.0), 0.0, MotionState::Idle, 0.0);
        let cat = Anim::new(Build::feline()).motion_input(0.0, (0.0, 0.0), 0.0, MotionState::Idle, 0.0);
        assert!(colossus.max_speed < cat.max_speed);
        assert!(colossus.max_speed >= 1.0, "never zero, which would divide out");
    }

    /// Heavier animals swing less freely than light ones in the same gait.
    #[test]
    fn heft_damps_the_swing() {
        let mut light = Anim::new(Build::mini_cat());
        let mut heavy = Anim::new(Build::megafauna());
        for _ in 0..200 {
            light.advance(1.0 / 60.0, 4.0, false);
            heavy.advance(1.0 / 60.0, 4.0, false);
        }
        let l = light.motion_input(0.0, (0.0, 0.0), 0.0, MotionState::Idle, 0.0);
        let h = heavy.motion_input(0.0, (0.0, 0.0), 0.0, MotionState::Idle, 0.0);
        assert!(l.swing_scale > h.swing_scale, "light {} vs heavy {}", l.swing_scale, h.swing_scale);
    }

    /// Inputs the rig clamps anyway are clamped here too, so a caller passing
    /// a raw yaw rate cannot push the rig outside its tested range.
    #[test]
    fn turn_and_impact_are_clamped() {
        let a = Anim::new(Build::feline());
        let m = a.motion_input(9.0, (0.0, 0.0), 5.0, MotionState::Idle, 0.0);
        assert_eq!(m.turn, 1.0);
        assert_eq!(m.impact, 1.0);
        let m = a.motion_input(-9.0, (0.0, 0.0), -5.0, MotionState::Idle, 0.0);
        assert_eq!(m.turn, -1.0);
        assert_eq!(m.impact, 0.0);
    }

    /// A pathological dt must not corrupt the phase.
    #[test]
    fn a_long_frame_is_survivable() {
        let mut a = Anim::new(Build::feline());
        for _ in 0..10 {
            a.advance(5.0, 9.0, false);
            assert!(a.gait_phase.is_finite());
            assert!((0.0..std::f32::consts::TAU).contains(&a.gait_phase));
        }
    }

    // ── stamina caps the gait ────────────────────────────────────────────

    use wildkit::sim::stamina::{Stamina, SPRINT_FLOOR};

    /// A rested animal is unaffected -- stamina must not change anything until
    /// it runs out, or every gait threshold shifts under a feature that was
    /// supposed to add one state.
    #[test]
    fn a_rested_animal_gaits_exactly_as_before() {
        let b = Build::hare();
        let full = Stamina::full();
        for speed in [0.0f32, 0.5, 1.4, 2.9, 5.0, 20.0] {
            assert_eq!(b.gait_for_spent(speed, &full), b.gait_for(speed),
                "speed {speed} changed for a rested animal");
        }
    }

    /// THE PANTING HALT. Spent, the suspension gaits are off the table and the
    /// animal drops to a trot -- still moving, and still moving fast.
    #[test]
    fn a_spent_animal_drops_out_of_a_bound() {
        let b = Build::hare();
        let spent = Stamina::new(SPRINT_FLOOR * 0.5);
        assert!(!spent.can_sprint());
        let fast = b.trot_to + 5.0;
        assert_eq!(b.gait_for(fast), Gait::Bound, "the premise: rested, it bounds");
        assert_eq!(b.gait_for_spent(fast, &spent), Gait::Trot);
    }

    /// Canter is a suspension gait too, and drops the same way.
    #[test]
    fn a_spent_animal_drops_out_of_a_canter() {
        let b = Build::stag();
        let spent = Stamina::new(0.0);
        let fast = b.trot_to + 1.0;
        assert_eq!(b.gait_for(fast), Gait::Canter);
        assert_eq!(b.gait_for_spent(fast, &spent), Gait::Trot);
    }

    /// An exhausted animal still WALKS. Refusing the low gaits would leave it
    /// frozen mid-field, which is worse than tired.
    #[test]
    fn exhaustion_never_takes_away_walking() {
        let b = Build::megafauna();
        let spent = Stamina::new(0.0);
        for speed in [0.1f32, 1.0, b.walk_to] {
            let g = b.gait_for_spent(speed, &spent);
            assert!(matches!(g, Gait::Walk | Gait::Pace), "speed {speed} gave {g:?}");
        }
        assert_eq!(b.gait_for_spent(b.trot_to - 0.1, &spent), Gait::Trot,
            "and still trots");
    }

    /// A colossus never bounds anyway, so exhaustion changes nothing for it --
    /// the cap must not invent a gait change where there was no suspension to
    /// lose.
    #[test]
    fn a_build_that_never_bounds_is_unaffected_by_exhaustion() {
        let b = Build::colossus();
        let spent = Stamina::new(0.0);
        for speed in [0.5f32, 3.0, 50.0] {
            assert_eq!(b.gait_for_spent(speed, &spent), b.gait_for(speed));
        }
    }

    /// Recovery past the floor restores the top gait -- the other half of the
    /// panting halt, and the reason the floor is a floor rather than zero.
    #[test]
    fn recovering_past_the_floor_restores_the_bound() {
        let b = Build::hare();
        let fast = b.trot_to + 5.0;
        let just_under = Stamina::new(SPRINT_FLOOR - 0.01);
        let just_over = Stamina::new(SPRINT_FLOOR + 0.01);
        assert_eq!(b.gait_for_spent(fast, &just_under), Gait::Trot);
        assert_eq!(b.gait_for_spent(fast, &just_over), Gait::Bound);
    }
}
