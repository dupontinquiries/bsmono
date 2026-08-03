//! ACTUATORS — one primitive under suspension, drive, control surfaces and
//! the fold itself.
//!
//! The observation that got us here: a `cyber::Muscle` that only responds to
//! torque is decoration, because the fold is still a scripted lerp underneath
//! it. Make the muscle DRIVE the fold and the transformation stops being an
//! animation with a physics costume. And once you have written "an actuator
//! reaching for a target against a load, with compliance", you have also
//! written a suspension strut, a drive unit and an aileron — they differ in
//! what the load IS and nothing else.
//!
//!   * **Suspension** — target is ride height, load is weight. It settles.
//!   * **Drive** — target is a wheel speed, load is ground reaction through
//!     grip. It saturates, and saturating IS wheelspin.
//!   * **Control surface** — target is a deflection, load is air pressure
//!     that rises with the square of airspeed. So the same stick input moves
//!     less at speed, which is why fast aircraft feel heavy without anyone
//!     writing a speed-to-authority curve.
//!   * **Fold** — target is the joint angle at the end of the transform,
//!     load is the inertia of what is being moved.
//!
//! That last one is the prize. `content::CHASSIS` currently carries a
//! hand-authored `xform_dur` per chassis: juggernaut 1.35, raptor 0.80. If
//! the fold is powered, that number is not authored, it EMERGES — a heavy
//! chassis folds slowly because its actuators are moving more metal, and a
//! test below asserts exactly that with no duration table involved.
//!
//! It also gives failure states for free. An actuator that cannot beat its
//! load gives way rather than winning silently: a fold that stalls half-open
//! under damage, a jump that does not clear, a control surface that will not
//! deflect at Mach. None of those need a special case.

/// A linear or rotational actuator with a force ceiling.
///
/// Units are deliberately unlabelled — metres and newtons for a strut,
/// radians and newton-metres for a joint. The maths is identical and giving
/// it two names would mean writing it twice.
#[derive(Clone, Copy, Debug)]
pub struct Actuator {
    /// most it can push or pull. The ceiling is what makes it powered
    /// rather than kinematic.
    pub max_force: f32,
    /// how hard it pulls toward the target, per unit of error
    pub stiffness: f32,
    /// resistance to velocity. Too little and it rings; too much and it
    /// crawls.
    pub damping: f32,
    /// travel limits. Hitting one is a stop, not a suggestion.
    pub lo: f32,
    pub hi: f32,
}

impl Default for Actuator {
    fn default() -> Actuator {
        Actuator { max_force: 6_000.0, stiffness: 40_000.0, damping: 2_600.0, lo: -1.0, hi: 1.0 }
    }
}

/// Where an actuator currently is.
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    pub pos: f32,
    pub vel: f32,
    /// true while the actuator is asking for more than it has
    pub saturated: bool,
    /// true while it is against a travel stop
    pub bottomed: bool,
}

impl Actuator {
    /// Advance by `dt`, reaching for `target` against `load`, moving `mass`.
    ///
    /// Returns the force actually delivered, which is the interesting
    /// number: it is clamped, so an actuator asked for more than it has
    /// gives way instead of winning. Wheelspin, a stalled fold and a
    /// control surface that will not move at speed are all this one clamp.
    pub fn step(&self, st: &mut State, target: f32, load: f32, mass: f32, dt: f32) -> f32 {
        let want = self.stiffness * (target - st.pos) - self.damping * st.vel;
        let force = want.clamp(-self.max_force, self.max_force);
        st.saturated = want.abs() > self.max_force;

        let net = force - load;
        st.vel += net / mass.max(1e-4) * dt;
        st.pos += st.vel * dt;

        st.bottomed = false;
        if st.pos <= self.lo {
            st.pos = self.lo;
            // a stop absorbs rather than bounces; a suspension that
            // trampolines off its bump stop is a broken suspension
            st.vel = st.vel.max(0.0) * 0.15;
            st.bottomed = true;
        } else if st.pos >= self.hi {
            st.pos = self.hi;
            st.vel = st.vel.min(0.0) * 0.15;
            st.bottomed = true;
        }
        force
    }

    /// Can it hold this load at all?
    pub fn can_hold(&self, load: f32) -> bool {
        load.abs() <= self.max_force
    }

    /// Roughly how long to travel `distance` against `load` moving `mass`,
    /// in seconds. Used for a HUD estimate; the real answer comes from
    /// stepping it.
    pub fn travel_time(&self, distance: f32, load: f32, mass: f32) -> f32 {
        let net = (self.max_force - load.abs()).max(1.0);
        (2.0 * distance.abs() * mass.max(1e-4) / net).sqrt()
    }
}

// ------------------------------------------------------------- adapters ---

/// A suspension strut: target is ride height, load is weight.
pub fn suspension(travel: f32, rate: f32) -> Actuator {
    Actuator {
        max_force: rate * travel * 2.5,
        stiffness: rate,
        damping: (rate * 0.06).max(200.0),
        lo: -travel,
        hi: 0.0,
    }
}

/// A drive unit. `grip` caps the force the ground will accept — asking for
/// more is wheelspin, which the saturation flag reports for free.
pub fn drive(power: f32, grip: f32) -> Actuator {
    Actuator {
        max_force: power.min(grip),
        stiffness: power * 4.0,
        damping: power * 0.25,
        lo: f32::NEG_INFINITY,
        hi: f32::INFINITY,
    }
}

/// Aerodynamic load on a control surface at this airspeed.
///
/// Goes as v², like everything else in a fluid, which is why the same stick
/// deflects less at speed without anyone writing an authority curve.
pub fn air_load(airspeed: f32, air_density: f32, area: f32) -> f32 {
    0.5 * air_density * airspeed * airspeed * area
}

/// A control surface: strong enough to move at cruise, not at the redline.
pub fn control_surface(authority: f32) -> Actuator {
    Actuator {
        max_force: authority,
        stiffness: authority * 8.0,
        damping: authority * 0.9,
        lo: -0.6,
        hi: 0.6,
    }
}

/// A fold joint, sized to what it has to move.
///
/// `part_mass` stands in for the inertia of everything hanging off this
/// joint. The actuator is scaled to the chassis, not to the part, which is
/// the whole point: a big machine has bigger actuators AND more to move, and
/// the ratio between them is what decides how quickly it folds.
/// DAMPING IS TUNED TO WHAT IT MOVES, which is not a detail. With a fixed
/// damping constant the light joint comes out overdamped and CREEPS while
/// the heavy one lands near critical and arrives first — so a heavy chassis
/// folded faster, which is both wrong and the opposite of the point. Real
/// actuators are tuned per joint, so damping is set from the mass to hold
/// the damping ratio near critical: c = 2*zeta*sqrt(k*m).
///
/// With that, arrival time follows the natural frequency sqrt(k/m), so more
/// metal really is slower, and it falls out of the maths rather than a
/// table.
pub fn fold_joint(chassis_power: f32, part_mass: f32) -> Actuator {
    let k = chassis_power * 3.0;
    let m = part_mass.max(1e-3);
    // slightly past critical: a limb that overshoots its stop looks broken
    let zeta = 1.08;
    Actuator {
        max_force: chassis_power,
        stiffness: k,
        damping: 2.0 * zeta * (k * m).sqrt(),
        lo: -std::f32::consts::PI,
        hi: std::f32::consts::PI,
    }
}

/// Drive a fold to completion and report how long it took.
///
/// This is the replacement for an authored `xform_dur`: duration is what the
/// actuators and the mass produce, not a number somebody typed.
pub fn fold_time(act: &Actuator, angle: f32, mass: f32, load: f32, dt: f32, cap_s: f32) -> Option<f32> {
    let mut st = State::default();
    let mut t = 0.0;
    while t < cap_s {
        act.step(&mut st, angle, load, mass, dt);
        t += dt;
        if (st.pos - angle).abs() < angle.abs() * 0.02 && st.vel.abs() < angle.abs() * 0.05 {
            return Some(t);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 240.0;

    fn settle(a: &Actuator, target: f32, load: f32, mass: f32, secs: f32) -> State {
        let mut st = State::default();
        for _ in 0..(secs / DT) as i32 {
            a.step(&mut st, target, load, mass, DT);
        }
        st
    }

    #[test]
    fn it_reaches_its_target_and_stops_ringing() {
        let a = Actuator::default();
        let st = settle(&a, 0.5, 0.0, 40.0, 3.0);
        assert!((st.pos - 0.5).abs() < 0.01, "did not arrive: {}", st.pos);
        assert!(st.vel.abs() < 0.05, "still ringing at {}", st.vel);
    }

    #[test]
    fn too_little_damping_overshoots_and_too_much_crawls() {
        let base = Actuator { lo: -10.0, hi: 10.0, ..Actuator::default() };
        let loose = Actuator { damping: 100.0, ..base };
        let tight = Actuator { damping: 40_000.0, ..base };
        // loose overshoots on the way
        let mut st = State::default();
        let mut peak = 0.0f32;
        for _ in 0..(1.0 / DT) as i32 {
            loose.step(&mut st, 1.0, 0.0, 20.0, DT);
            peak = peak.max(st.pos);
        }
        assert!(peak > 1.05, "an underdamped actuator did not overshoot: {peak}");
        // tight has not got there yet
        let slow = settle(&tight, 1.0, 0.0, 20.0, 0.3);
        assert!(slow.pos < 0.9, "an overdamped actuator arrived instantly: {}", slow.pos);
    }

    #[test]
    fn an_actuator_asked_for_more_than_it_has_gives_way() {
        // the clamp that makes it POWERED rather than kinematic
        let weak = Actuator { max_force: 50.0, lo: -10.0, hi: 10.0, ..Actuator::default() };
        assert!(!weak.can_hold(500.0));
        let st = settle(&weak, 1.0, 500.0, 20.0, 2.0);
        assert!(st.pos < 0.0, "it beat a load ten times its strength: {}", st.pos);
        assert!(st.saturated, "it gave way without reporting saturation");
        // and a strong one holds the same load
        let strong = Actuator { max_force: 5_000.0, lo: -10.0, hi: 10.0, ..Actuator::default() };
        let ok = settle(&strong, 1.0, 500.0, 20.0, 3.0);
        assert!((ok.pos - 1.0).abs() < 0.1, "a strong actuator failed: {}", ok.pos);
    }

    #[test]
    fn a_travel_stop_absorbs_rather_than_bounces() {
        let a = suspension(0.12, 60_000.0);
        let st = settle(&a, -1.0, 40_000.0, 300.0, 1.5);
        assert!(st.bottomed, "it never reached the stop");
        assert!((st.pos + 0.12).abs() < 1e-4, "it went through the stop: {}", st.pos);
        assert!(st.vel.abs() < 0.5, "it trampolined off the bump stop: {}", st.vel);
    }

    #[test]
    fn suspension_sinks_further_under_more_weight_until_it_cannot() {
        let a = suspension(0.15, 50_000.0);
        let light = settle(&a, 0.0, 3_000.0, 250.0, 2.0).pos;
        let heavy = settle(&a, 0.0, 9_000.0, 250.0, 2.0).pos;
        assert!(heavy < light, "more weight did not compress it: {light} -> {heavy}");
        let crushed = settle(&a, 0.0, 1e6, 250.0, 2.0).pos;
        assert!((crushed + 0.15).abs() < 1e-3, "it compressed past its travel");
    }

    #[test]
    fn wheelspin_is_just_saturation() {
        // grip caps what the ground will accept; asking for more spins
        let gripped = drive(8_000.0, 8_000.0);
        let ice = drive(8_000.0, 400.0);
        let mut a = State::default();
        let mut b = State::default();
        gripped.step(&mut a, 30.0, 2_000.0, 300.0, DT);
        ice.step(&mut b, 30.0, 2_000.0, 300.0, DT);
        assert!(!ice.can_hold(2_000.0), "ice held a load it should not");
        assert!(gripped.can_hold(2_000.0));
        assert!(b.saturated, "the low-grip unit did not report spinning");
    }

    #[test]
    fn the_same_stick_moves_less_at_speed() {
        // aerodynamic authority falls out of v^2; nobody writes a curve
        let s = control_surface(900.0);
        let slow = settle(&s, 0.5, air_load(60.0, 1.0, 0.35), 3.0, 1.0).pos;
        let fast = settle(&s, 0.5, air_load(240.0, 1.0, 0.35), 3.0, 1.0).pos;
        assert!(fast < slow, "deflection did not fall with speed: {slow} -> {fast}");
        // and in vacuum there is nothing to push against, so full deflection
        let space = settle(&s, 0.5, air_load(240.0, 0.0, 0.35), 3.0, 1.0).pos;
        assert!((space - 0.5).abs() < 0.02, "a surface met resistance in vacuum");
    }

    #[test]
    fn air_load_is_quadratic_in_speed() {
        let a = air_load(50.0, 1.0, 1.0);
        let b = air_load(100.0, 1.0, 1.0);
        assert!((b / a - 4.0).abs() < 1e-3);
        assert_eq!(air_load(100.0, 0.0, 1.0), 0.0);
    }

    #[test]
    fn a_heavy_chassis_folds_slower_with_nobody_authoring_a_duration() {
        // THE PRIZE. content::CHASSIS carries xform_dur by hand — juggernaut
        // 1.35, raptor 0.80. Powered, that number is not authored: a heavy
        // machine folds slowly because its actuators are moving more metal.
        let light = fold_time(&fold_joint(9_000.0, 60.0), 2.4, 60.0, 0.0, DT, 20.0)
            .expect("light never folded");
        let heavy = fold_time(&fold_joint(9_000.0, 420.0), 2.4, 420.0, 0.0, DT, 20.0)
            .expect("heavy never folded");
        assert!(heavy > light * 1.5, "mass did not slow the fold: {light} -> {heavy}");
        // and a stronger chassis folds the same mass faster
        let quick = fold_time(&fold_joint(30_000.0, 420.0), 2.4, 420.0, 0.0, DT, 20.0)
            .expect("strong never folded");
        assert!(quick < heavy, "more power did not fold faster: {heavy} -> {quick}");
        // the ratio should track sqrt(m), which is what a spring-mass does
        let ratio = heavy / light;
        let expect = (420.0f32 / 60.0).sqrt();
        assert!((ratio / expect - 1.0).abs() < 0.35, "ratio {ratio} vs sqrt(m) {expect}");
    }

    #[test]
    fn a_fold_under_too_much_load_stalls_half_open() {
        // damage, or something sitting on you. No special case needed: the
        // actuator simply cannot reach, and a stalled transform is a state
        // the game can show rather than a bug.
        let act = fold_joint(2_000.0, 200.0);
        assert!(fold_time(&act, 2.4, 200.0, 8_000.0, DT, 6.0).is_none());
        let st = settle(&act, 2.4, 8_000.0, 200.0, 4.0);
        assert!(st.pos < 2.0, "it completed a fold it could not power: {}", st.pos);
        assert!(st.saturated);
    }

    #[test]
    fn the_time_estimate_is_in_the_right_neighbourhood() {
        // used for a HUD readout, so it only has to be close
        let act = fold_joint(9_000.0, 420.0);
        let est = act.travel_time(2.4, 0.0, 420.0);
        let real = fold_time(&act, 2.4, 420.0, 0.0, DT, 20.0).unwrap();
        assert!(est > real * 0.15 && est < real * 6.0, "estimate {est} vs real {real}");
    }
}
