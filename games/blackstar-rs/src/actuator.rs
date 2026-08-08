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
    /// Advance a LINEAR actuator: a strut, a ram, a drive unit.
    ///
    /// `mass` is genuinely mass here and `load` is genuinely a force, so
    /// F = ma is the right equation. The rotational cases are NOT this —
    /// see `step_angular`.
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

    /// Advance a ROTATIONAL actuator: a joint, a control surface.
    ///
    /// The linear step is wrong here and was wrong here until this existed.
    /// Two things it got wrong:
    ///
    /// **Inertia, not mass.** Angular acceleration is tau / I, and I is
    /// sum(m*r^2) — it goes as the SQUARE of how far the mass sits from the
    /// pivot. A limb with its mass at the tip is enormously harder to swing
    /// than the same limb with its mass at the shoulder, and using mass
    /// alone makes them identical.
    ///
    /// **Leverage varies with angle.** Gravity's moment on a limb is
    /// m*g*r*cos(theta) measured from horizontal: maximum when the limb is
    /// held out straight, ZERO when it hangs straight down or points
    /// straight up. Using a constant load makes a fold equally hard all the
    /// way round, which is why it never stalled in the place a real one
    /// would — halfway, with the arm out.
    ///
    /// `inertia` is I about this joint's axis. `gravity_torque` is the peak
    /// moment, i.e. m*g*r_cm; the cos falls out of the current angle here.
    pub fn step_angular(
        &self,
        st: &mut State,
        target: f32,
        inertia: f32,
        gravity_torque: f32,
        extra_load: f32,
        dt: f32,
    ) -> f32 {
        let want = self.stiffness * (target - st.pos) - self.damping * st.vel;
        let force = want.clamp(-self.max_force, self.max_force);
        st.saturated = want.abs() > self.max_force;

        // THE MOMENT ARM. Zero hanging down, greatest held out horizontal.
        let load = gravity_torque * st.pos.cos() + extra_load;
        let net = force - load;
        st.vel += net / inertia.max(1e-6) * dt;
        st.pos += st.vel * dt;

        st.bottomed = false;
        if st.pos <= self.lo {
            st.pos = self.lo;
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

/// Moment of inertia of a set of point masses about a pivot: sum(m * r^2).
///
/// This is the number the fold has been missing. It is why a long arm is
/// slow and a stubby one is quick even at the same weight — the r is
/// SQUARED, so doubling a limb's length quadruples what it takes to swing.
pub fn inertia_about(pivot: [f32; 3], points: &[([f32; 3], f32)]) -> f32 {
    points
        .iter()
        .map(|(p, m)| {
            let r2 = (p[0] - pivot[0]).powi(2) + (p[1] - pivot[1]).powi(2) + (p[2] - pivot[2]).powi(2);
            m * r2
        })
        .sum()
}

/// Peak gravity moment on a limb: m * g * distance to its centre of mass.
///
/// Peak, because the cos of the current angle is applied per step. Held out
/// horizontal this is the whole of it; hanging straight down it is nothing.
pub fn gravity_torque(mass: f32, g: f32, r_cm: f32) -> f32 {
    mass * g * r_cm
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
pub fn fold_joint(chassis_power: f32, inertia: f32) -> Actuator {
    let k = chassis_power * 3.0;
    let m = inertia.max(1e-3);
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
pub fn fold_time(
    act: &Actuator,
    angle: f32,
    inertia: f32,
    gravity_torque: f32,
    dt: f32,
    cap_s: f32,
) -> Option<f32> {
    let mut st = State::default();
    let mut t = 0.0;
    while t < cap_s {
        act.step_angular(&mut st, angle, inertia, gravity_torque, 0.0, dt);
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
        assert!(heavy > light * 1.5, "inertia did not slow the fold: {light} -> {heavy}");
        let quick = fold_time(&fold_joint(30_000.0, 420.0), 2.4, 420.0, 0.0, DT, 20.0)
            .expect("strong never folded");
        assert!(quick < heavy, "more power did not fold faster: {heavy} -> {quick}");
        let ratio = heavy / light;
        let expect = (420.0f32 / 60.0).sqrt();
        assert!((ratio / expect - 1.0).abs() < 0.35, "ratio {ratio} vs sqrt(I) {expect}");
    }

    #[test]
    fn a_fold_under_too_much_load_stalls_half_open() {
        // damage, or something sitting on you. No special case needed: the
        // actuator simply cannot reach, and a stalled transform is a state
        // the game can show rather than a bug.
        let act = fold_joint(2_000.0, 200.0);
        assert!(fold_time(&act, 2.4, 200.0, 8_000.0, DT, 6.0).is_none());
        let mut st = State::default();
        for _ in 0..(4.0 / DT) as i32 {
            act.step_angular(&mut st, 2.4, 200.0, 8_000.0, 0.0, DT);
        }
        assert!(st.pos < 2.0, "it completed a fold it could not power: {}", st.pos);
    }

    #[test]
    fn inertia_goes_as_the_square_of_the_arm() {
        // the thing scalar mass could not express: same weight, twice the
        // reach, FOUR times as hard to swing
        let near = inertia_about([0.0; 3], &[([1.0, 0.0, 0.0], 10.0)]);
        let far = inertia_about([0.0; 3], &[([2.0, 0.0, 0.0], 10.0)]);
        assert!((far / near - 4.0).abs() < 1e-4, "{near} -> {far}");
        // mass at the pivot costs nothing to rotate
        assert!(inertia_about([0.0; 3], &[([0.0; 3], 500.0)]) < 1e-6);
    }

    #[test]
    fn a_long_limb_folds_far_slower_than_a_stubby_one_of_the_same_weight() {
        // and this is why it matters visually: it is REACH that makes a
        // fold look heavy, not weight
        let mass = 40.0;
        let stub = inertia_about([0.0; 3], &[([0.5, 0.0, 0.0], mass)]);
        let long = inertia_about([0.0; 3], &[([1.5, 0.0, 0.0], mass)]);
        let a = fold_time(&fold_joint(9_000.0, stub), 2.0, stub, 0.0, DT, 30.0).unwrap();
        let b = fold_time(&fold_joint(9_000.0, long), 2.0, long, 0.0, DT, 30.0).unwrap();
        assert!(b > a * 2.0, "reach did not slow the fold: {a} -> {b}");
    }

    #[test]
    fn gravity_has_no_leverage_straight_down_and_all_of_it_horizontal() {
        // the load a fold actually fights, and the reason a real one stalls
        // halfway rather than at the start
        let act = fold_joint(3_000.0, 30.0);
        let peak = gravity_torque(60.0, 9.81, 0.9);
        let mut hanging = State { pos: -std::f32::consts::FRAC_PI_2, ..State::default() };
        let mut out = State { pos: 0.0, ..State::default() };
        // one step each, same command: the horizontal one is fighting
        // hundreds of newton-metres, the hanging one nothing
        act.step_angular(&mut hanging, 0.0, 30.0, peak, 0.0, DT);
        act.step_angular(&mut out, 0.6, 30.0, peak, 0.0, DT);
        let load_down = peak * (-std::f32::consts::FRAC_PI_2).cos();
        let load_out = peak * 0.0f32.cos();
        assert!(load_down.abs() < 1e-3, "gravity had leverage on a hanging limb");
        assert!(load_out > 500.0, "gravity had no leverage on an outstretched one");
    }

    #[test]
    fn a_joint_that_lifts_from_hanging_can_still_stall_at_horizontal() {
        // the failure the constant-load model could never produce: it starts
        // fine and gives out where the leverage is worst
        let peak = gravity_torque(90.0, 9.81, 1.1);
        let weak = Actuator {
            max_force: peak * 0.55,
            stiffness: peak * 2.0,
            damping: peak * 0.9,
            lo: -std::f32::consts::FRAC_PI_2,
            hi: std::f32::consts::FRAC_PI_2,
        };
        let mut st = State { pos: -std::f32::consts::FRAC_PI_2, ..State::default() };
        for _ in 0..(6.0 / DT) as i32 {
            weak.step_angular(&mut st, std::f32::consts::FRAC_PI_2, 40.0, peak, 0.0, DT);
        }
        // it got off the bottom but nowhere near the top
        assert!(st.pos > -1.4, "it never left the stop: {}", st.pos);
        assert!(st.pos < 0.2, "it powered through the worst leverage: {}", st.pos);
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
