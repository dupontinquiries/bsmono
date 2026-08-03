//! POSTURE — where you are when you fold, and why it matters.
//!
//! Once gravity's moment is real, the transform stops being one number and
//! becomes a question about circumstances. Two of them were worth checking,
//! and one has a counterintuitive answer.
//!
//! **Does kneeling or putting your hands down help?** Yes, and for a reason
//! that is not "it looks braced". A limb only costs its actuator anything
//! while the actuator is holding it up. Put a hand on the ground and the
//! GROUND holds that arm, so its weight leaves the fold budget entirely.
//! Kneeling does the same for the legs and drops the torso's centre of mass
//! closer to the pivot, which cuts the moment arm as well as the load. It is
//! the same reason a person stands up from a chair by leaning forward first.
//!
//! **Is folding mid-air different from folding on the ground?** Yes, and the
//! surprising part is that it is EASIER. In free fall the body and every
//! limb accelerate together, so relative to the body there is no weight at
//! all — `m*g*r*cos(theta)` vanishes and the actuators fight nothing but
//! inertia. A bot genuinely does transform faster in the air.
//!
//! The catch is the one that makes it a decision rather than a free win:
//! ON THE GROUND THE PLANET ABSORBS YOUR REACTION TORQUE. In the air nothing
//! does, so every newton-metre the fold applies to a limb applies equal and
//! opposite to the body, and angular momentum has to come out somewhere. You
//! fold faster and you arrive SPINNING, by an amount that scales with how
//! much you moved and how little body you had to move it against. That is
//! why jumping and folding into a flyer works — a flyer wants to be pointed
//! somewhere and can correct — and why folding into a truck mid-air lands
//! you on your roof.
//!
//! **Running into a vehicle** adds a deadline rather than a load. The wheels
//! have to be down and turning before the body arrives, so what matters is
//! whether the fold finishes inside the time your speed gives you.

use crate::actuator::{fold_joint, fold_time, gravity_torque, Actuator};

/// Where the bot is when it starts to fold.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Stance {
    /// upright, everything held up by its own actuators
    Standing,
    /// one knee down: the legs are supported and the torso has dropped
    Kneeling,
    /// hands and knees — the most support, and the arms are committed
    Braced,
    /// in free fall
    Airborne,
    /// on the ground and moving, in m/s
    Running(f32),
}

impl Stance {
    pub fn label(self) -> &'static str {
        match self {
            Stance::Standing => "STANDING",
            Stance::Kneeling => "KNEELING",
            Stance::Braced => "BRACED",
            Stance::Airborne => "AIRBORNE",
            Stance::Running(_) => "RUNNING",
        }
    }

    /// Fraction of the body's weight the GROUND is carrying rather than the
    /// actuators. This is the whole of why bracing helps.
    pub fn supported(self) -> f32 {
        match self {
            Stance::Standing => 0.35,
            Stance::Kneeling => 0.62,
            Stance::Braced => 0.80,
            Stance::Airborne => 0.0,
            // running carries weight but the legs are busy cycling, so less
            // of it is available as a brace
            Stance::Running(_) => 0.25,
        }
    }

    /// How much the moment arm shrinks. Kneeling pulls the mass toward the
    /// pivot as well as taking load off it — two effects, and the second is
    /// the bigger one because r is squared in the inertia.
    pub fn arm_scale(self) -> f32 {
        match self {
            Stance::Kneeling => 0.78,
            Stance::Braced => 0.66,
            _ => 1.0,
        }
    }

    /// Does the planet absorb the reaction torque?
    pub fn grounded(self) -> bool {
        !matches!(self, Stance::Airborne)
    }

    /// Are the arms available to fold, or holding you up?
    ///
    /// Braced is the best support and the worst availability, which is what
    /// stops it being strictly better than kneeling.
    pub fn arms_free(self) -> bool {
        !matches!(self, Stance::Braced)
    }
}

/// What a fold from this stance costs and produces.
#[derive(Clone, Copy, Debug)]
pub struct Fold {
    pub seconds: f32,
    /// radians per second of body rotation left over. Zero on the ground.
    pub body_spin: f32,
    /// true if the fold finished at all
    pub completed: bool,
    /// true if the arms had to be folded but were holding the bot up
    pub arms_blocked: bool,
}

/// Simulate a fold from a stance.
///
/// `inertia` is the moment of the moving mass about the joint, `mass` its
/// weight, `r_cm` the distance to its centre of mass, `body_inertia` the
/// moment of the whole bot — which is what the reaction torque has to spin.
/// Robust to nonsense: zero gravity, zero inertia and a still body all
/// return something sensible rather than dividing by them.
pub fn fold_from(
    stance: Stance,
    power: f32,
    inertia: f32,
    mass: f32,
    g: f32,
    r_cm: f32,
    body_inertia: f32,
    angle: f32,
    needs_arms: bool,
) -> Fold {
    let arm = r_cm * stance.arm_scale();
    // bracing removes supported weight from the budget; free fall removes
    // ALL of it, because nothing has weight relative to a falling frame
    let unsupported = if stance.grounded() { 1.0 - stance.supported() } else { 0.0 };
    let load = gravity_torque(mass.max(0.0), g.max(0.0), arm) * unsupported;
    // the same arm scaling applies to what has to be swung
    let i = (inertia.max(1e-4)) * stance.arm_scale().powi(2);

    let act: Actuator = fold_joint(power.max(1.0), i);
    let t = fold_time(&act, angle, i, load, 1.0 / 240.0, 12.0);

    // REACTION. On the ground the planet takes it. In the air the body does,
    // and angular momentum is conserved: I_limb * w_limb = -I_body * w_body.
    let spin = if stance.grounded() || body_inertia <= 1e-6 {
        0.0
    } else {
        let secs = t.unwrap_or(12.0).max(1e-3);
        let limb_rate = angle / secs;
        (i * limb_rate / body_inertia).abs()
    };

    Fold {
        seconds: t.unwrap_or(f32::INFINITY),
        body_spin: spin,
        completed: t.is_some(),
        arms_blocked: needs_arms && !stance.arms_free(),
    }
}

/// Running into a vehicle is a deadline, not a load: the wheels have to be
/// down before the body arrives. Returns the time available at this speed.
pub fn touchdown_window(speed_ms: f32, clearance_m: f32) -> f32 {
    if speed_ms <= 0.01 {
        return f32::INFINITY;
    }
    (clearance_m.max(0.0) / speed_ms).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // a mid-sized bot: 90 kg limb, 1.1 m to its centre, 2.4 rad of fold
    fn go(stance: Stance, g: f32) -> Fold {
        fold_from(stance, 6_000.0, 110.0, 90.0, g, 1.1, 900.0, 2.4, false)
    }

    #[test]
    fn bracing_makes_a_transform_easier_and_kneeling_helps_less() {
        let stand = go(Stance::Standing, 9.81);
        let kneel = go(Stance::Kneeling, 9.81);
        let brace = go(Stance::Braced, 9.81);
        assert!(kneel.seconds < stand.seconds, "kneeling did not help: {stand:?} {kneel:?}");
        assert!(brace.seconds < kneel.seconds, "bracing did not beat kneeling");
        // and the ground carries more of you the lower you get
        assert!(Stance::Braced.supported() > Stance::Kneeling.supported());
        assert!(Stance::Kneeling.supported() > Stance::Standing.supported());
    }

    #[test]
    fn bracing_is_not_strictly_better_because_it_costs_you_your_arms() {
        // otherwise there would be no decision
        let brace = fold_from(Stance::Braced, 6_000.0, 110.0, 90.0, 9.81, 1.1, 900.0, 2.4, true);
        let kneel = fold_from(Stance::Kneeling, 6_000.0, 110.0, 90.0, 9.81, 1.1, 900.0, 2.4, true);
        assert!(brace.arms_blocked, "hands on the ground were still free to fold");
        assert!(!kneel.arms_blocked);
        assert!(Stance::Standing.arms_free() && Stance::Airborne.arms_free());
    }

    #[test]
    fn folding_in_free_fall_is_faster_than_folding_on_your_feet() {
        // the counterintuitive one: in free fall nothing has weight relative
        // to the body, so the actuators fight inertia alone
        let ground = go(Stance::Standing, 9.81);
        let air = go(Stance::Airborne, 9.81);
        assert!(air.seconds < ground.seconds, "air was not faster: {ground:?} {air:?}");

        // BUT NOT FASTER THAN BRACED, which I expected and was wrong about.
        // Free fall removes the WEIGHT; bracing removes some of the weight
        // AND pulls the mass toward the pivot, and the second effect is
        // squared in the inertia. So a braced fold beats a falling one, and
        // the best transform is still the one you set up for.
        let brace = go(Stance::Braced, 9.81);
        assert!(brace.seconds < air.seconds, "bracing lost to free fall: {brace:?} {air:?}");
    }

    #[test]
    fn but_you_arrive_spinning_because_nothing_absorbed_the_reaction() {
        let ground = go(Stance::Standing, 9.81);
        let air = go(Stance::Airborne, 9.81);
        assert_eq!(ground.body_spin, 0.0, "the planet failed to absorb the reaction");
        assert!(air.body_spin > 0.05, "a mid-air fold produced no body rotation");
    }

    #[test]
    fn a_heavier_body_soaks_up_more_of_its_own_reaction() {
        // conservation: the same limb swung against more body spins it less
        let light = fold_from(Stance::Airborne, 6_000.0, 110.0, 90.0, 9.81, 1.1, 300.0, 2.4, false);
        let heavy = fold_from(Stance::Airborne, 6_000.0, 110.0, 90.0, 9.81, 1.1, 3_000.0, 2.4, false);
        assert!(heavy.body_spin < light.body_spin, "{light:?} {heavy:?}");
        assert!((light.body_spin / heavy.body_spin - 10.0).abs() < 1.0, "not inversely proportional");
    }

    #[test]
    fn low_gravity_narrows_the_gap_between_ground_and_air() {
        // on Luna there is barely any weight to be relieved of, so bracing
        // buys much less — the advice changes with the destination
        let earth_gain = go(Stance::Standing, 9.81).seconds - go(Stance::Braced, 9.81).seconds;
        let luna_gain = go(Stance::Standing, 1.62).seconds - go(Stance::Braced, 1.62).seconds;
        assert!(luna_gain < earth_gain, "bracing helped as much on Luna: {earth_gain} {luna_gain}");
        // and in freefall gravity is irrelevant by construction
        let a = go(Stance::Airborne, 9.81).seconds;
        let b = go(Stance::Airborne, 1.62).seconds;
        assert!((a - b).abs() < 1e-4, "free fall cared about gravity");
    }

    #[test]
    fn running_gives_a_deadline_rather_than_a_load() {
        // wheels down before the body arrives
        let fast = touchdown_window(30.0, 1.2);
        let slow = touchdown_window(6.0, 1.2);
        assert!(fast < slow, "speed did not shorten the window");
        // and the real cost of folding while moving is GROUND COVERED: the
        // fold takes as long as it takes, so speed decides how much runway
        // it eats. A sprinting transform needs a clear road ahead of it.
        let fold = go(Stance::Running(30.0), 9.81).seconds;
        let sprint_run = 30.0 * fold;
        let jog_run = 6.0 * fold;
        assert!(sprint_run > jog_run * 4.0, "speed did not lengthen the run: {sprint_run} {jog_run}");
        assert!(sprint_run > 10.0, "a 30 m/s fold covered almost no ground");
        // standing still gives you forever
        assert!(touchdown_window(0.0, 1.2).is_infinite());
    }

    #[test]
    fn running_braces_less_than_kneeling_because_the_legs_are_busy() {
        assert!(Stance::Running(10.0).supported() < Stance::Kneeling.supported());
        assert!(Stance::Running(10.0).grounded());
        assert_eq!(go(Stance::Running(10.0), 9.81).body_spin, 0.0);
    }

    #[test]
    fn nonsense_inputs_do_not_produce_nonsense_answers() {
        // robustness: zero gravity, zero inertia, a body with no moment, a
        // fold of nothing. None of these should divide by zero or hang.
        for f in [
            fold_from(Stance::Standing, 6_000.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, false),
            fold_from(Stance::Airborne, 1.0, 0.0, -5.0, -9.0, 0.0, 0.0, 2.4, false),
            fold_from(Stance::Braced, 0.0, 1e9, 1e9, 9.81, 10.0, 1e-9, 3.0, true),
        ] {
            assert!(!f.seconds.is_nan(), "NaN seconds");
            assert!(!f.body_spin.is_nan(), "NaN spin");
            assert!(f.body_spin >= 0.0);
        }
        // an impossible fold reports not completing rather than pretending
        let stuck = fold_from(Stance::Standing, 1.0, 5_000.0, 5_000.0, 9.81, 3.0, 900.0, 3.0, false);
        assert!(!stuck.completed && stuck.seconds.is_infinite());
    }
}
