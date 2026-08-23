//! RESCALE — what it costs to make the player a real-sized robot.
//!
//! `docs/PRIORITIES.md` puts "walking around as a working robot" first, and
//! the obvious way in is `board::Avatar`, which models a switch between
//! `Scale::Titan` and `Scale::Human`. Wiring it turns out not to be the
//! problem.
//!
//! ## The scale switch is not a size multiplier
//!
//! `apply_car_dims` already takes a `size` and threads it through every
//! dimension, so the plumbing exists. But `cfg car_size` is clamped to
//! **0.55–2.0**, and against the engine's 0.38 m tuning body:
//!
//! ```text
//!   a human  1.75 m  needs size  4.6
//!   a bot    8.50 m  needs size 22.4
//! ```
//!
//! Both are outside the clamp, and raising the clamp is not the fix — at 22x
//! every constant tuned around that body is wrong, and they are wrong by
//! DIFFERENT AMOUNTS. That is the actual blocker, and it is worth having the
//! numbers before anyone starts.
//!
//! ## Why the exponents differ
//!
//! Under a length scale `k`, holding real gravity fixed and requiring the
//! motion to LOOK the same:
//!
//! | quantity | scales as | why |
//! |---|---|---|
//! | length | `k` | by definition |
//! | time | `√k` | a pendulum's period; a body twice as tall falls for √2 as long |
//! | speed | `√k` | length over time |
//! | acceleration | `1` | `k / (√k)²` — gravity is already right |
//! | mass | `k³` | volume, at constant density |
//!
//! So speeds go as `√k` and accelerations do not move at all. Scaling every
//! number by 22 — the intuitive thing — makes a bot that accelerates
//! twenty-two times too hard and reaches five times the speed it should.
//!
//! **That is the trap this module exists to price.** Froude scaling is not a
//! detail; it is the difference between a giant that feels heavy and one that
//! feels like a car in a costume.

/// The engine's tuning body, from `scale::RL_BODY_H_M`.
pub const BODY_H_M: f32 = 0.38;

/// How the world would have to grow for the player to be this tall.
pub fn factor_for(height_m: f32) -> f32 {
    height_m / BODY_H_M
}

/// What a constant is a measure of, which is what decides its exponent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Length,
    Speed,
    Accel,
    Mass,
    /// a rate per second — drains, regen
    PerTime,
    // No `Ratio`. I added one out of habit for "already dimensionless", and
    // none of the nine constants is: every one of them has units, which is
    // the whole reason they need classifying. A variant nothing takes is a
    // distinction the table does not make.
}

impl Kind {
    /// The exponent of `k` under Froude scaling.
    ///
    /// Accelerations do NOT scale, which is the counter-intuitive one and the
    /// reason a naive rescale feels wrong: gravity is already correct at any
    /// size, so multiplying it makes a heavier-feeling world rather than a
    /// bigger one.
    pub fn exponent(self) -> f32 {
        match self {
            Kind::Length => 1.0,
            Kind::Speed => 0.5,
            Kind::Accel => 0.0,
            Kind::Mass => 3.0,
            // per-second rates run on the stretched clock
            Kind::PerTime => -0.5,
        }
    }
    pub fn scale(self, v: f32, k: f32) -> f32 {
        v * k.powf(self.exponent())
    }
}

/// One of the engine's tuned constants.
pub struct Tuned {
    pub name: &'static str,
    pub value: f32,
    pub kind: Kind,
    pub note: &'static str,
}

/// `physics.rs`'s constants, classified.
pub static CONSTANTS: &[Tuned] = &[
    Tuned { name: "GRAV", value: 6.5, kind: Kind::Accel, note: "RL 650 uu/s^2" },
    Tuned { name: "CAR_MAX", value: 23.0, kind: Kind::Speed, note: "supersonic cap" },
    Tuned { name: "DRIVE_MAX", value: 14.1, kind: Kind::Speed, note: "throttle top" },
    Tuned { name: "BOOST_ACCEL", value: 9.92, kind: Kind::Accel, note: "boost push" },
    Tuned { name: "BOOST_DRAIN", value: 33.3, kind: Kind::PerTime, note: "boost per second" },
    Tuned { name: "BALL_R", value: 0.9325, kind: Kind::Length, note: "RL 93.25 uu" },
    Tuned { name: "BALL_MAX", value: 60.0, kind: Kind::Speed, note: "ball speed cap" },
    Tuned { name: "M_CAR", value: 180.0, kind: Kind::Mass, note: "chassis mass" },
    Tuned { name: "M_BALL", value: 30.0, kind: Kind::Mass, note: "ball mass" },
];

/// What the constant becomes at scale `k`.
pub fn rescaled(t: &Tuned, k: f32) -> f32 {
    t.kind.scale(t.value, k)
}

/// How wrong the naive answer is: multiplying everything by `k`.
///
/// The ratio between what somebody would type and what the physics wants.
/// One means the naive answer happens to be right.
pub fn naive_error(t: &Tuned, k: f32) -> f32 {
    let naive = t.value * k;
    naive / rescaled(t, k).max(1e-6)
}

/// Gravity at a destination, in the engine's units, once the world is scaled.
///
/// The reason `GRAV` does not move: it is an acceleration, and an
/// acceleration is already scale-free. A rescaled world should read real
/// surface gravity straight off `patrol::Destination::gravity` rather than
/// multiplying the engine's 6.5 by anything.
pub fn gravity_after_rescale(dest_g: f32) -> f32 {
    9.81 * dest_g
}

// ------------------------------------------------------------- driver ---

pub fn run() {
    println!("RESCALE — what a real-sized robot costs\n");
    println!("  `apply_car_dims` already threads a `size` through every dimension,");
    println!("  so the plumbing is there. `cfg car_size` clamps it to 0.55-2.0:\n");
    for (n, h) in [("a human", 1.75f32), ("a bot", 8.5)] {
        println!("    {:<8} {:>5.2} m needs size {:>6.2}", n, h, factor_for(h));
    }
    println!("\n  both outside the clamp — and raising it is not the fix.\n");

    println!("  FROUDE SCALING: the exponents differ, which is the trap\n");
    println!("    length k    time sqrt(k)    speed sqrt(k)    accel 1    mass k^3\n");
    println!("  a body twice as tall falls for sqrt(2) as long, so speeds go as");
    println!("  sqrt(k) and ACCELERATIONS DO NOT MOVE AT ALL. Scaling every number");
    println!("  by k — the intuitive thing — makes a bot that accelerates k times");
    println!("  too hard and tops out sqrt(k) times too fast.\n");

    for (label, h) in [("HUMAN", 1.75f32), ("BOT", 8.5)] {
        let k = factor_for(h);
        println!("  {label} SCALE (k = {k:.1})\n");
        println!(
            "  {:>12} {:>10} {:>12} {:>12} {:>8}  {}",
            "constant", "now", "correct", "naive (xk)", "error", "what it is"
        );
        for t in CONSTANTS {
            let want = rescaled(t, k);
            let naive = t.value * k;
            let err = naive_error(t, k);
            println!(
                "  {:>12} {:>10.2} {:>12.2} {:>12.2} {:>7.1}x  {:?}, {}",
                t.name, t.value, want, naive, err, t.kind, t.note
            );
        }
        println!();
    }

    println!("  WHAT THAT MEANS IN PRACTICE\n");
    let k = factor_for(8.5);
    let top = rescaled(&CONSTANTS[1], k);
    println!(
        "    a bot's top speed becomes {top:.0} m/s ({:.0} km/h), not {:.0}.",
        top * 3.6,
        23.0 * k
    );
    println!("    gravity stays 6.5 — it was never wrong, it is an acceleration.");
    println!(
        "    chassis mass goes {:.0} -> {:.0} t, because mass is a volume.",
        180.0 / 1000.0,
        rescaled(&CONSTANTS[7], k) / 1000.0
    );
    println!("\n    and a rescaled world should read gravity straight off the");
    println!("    destination instead: earth {:.2}, luna {:.2} m/s^2.",
        gravity_after_rescale(1.0), gravity_after_rescale(0.166));

    // FROUDE IS NECESSARY AND NOT SUFFICIENT, and the number says so
    println!("\n  AND THE SCALED NUMBER IS STILL ABSURD\n");
    let car_bl = 23.0 / BODY_H_M;
    let bot_bl = top / 8.5;
    println!("    the RL car does 23 m/s at 0.38 m — {car_bl:.0} BODY LENGTHS PER SECOND.");
    println!("    Froude does not preserve that: speed goes as sqrt(k) and length as");
    println!("    k, so relative speed goes as 1/sqrt(k) and big things are");
    println!("    relatively SLOWER. {car_bl:.0} bl/s becomes {bot_bl:.1}.\n");
    println!("    So the scaling does most of the work. Not all of it: a cheetah");
    println!("    manages about 4 bl/s and is the fastest thing on land, so the");
    println!("    scaled bot is still {:.1}x that — and {:.0} km/h.\n", bot_bl / 4.0, top * 3.6);
    println!("    Scaling tells you how a number changes with size. It cannot tell");
    println!("    you the number was never a robot's to begin with.");

    println!("\n  SO THE PRIORITY IS NOT 'WIRE Avatar IN'. It is: pick a scale, apply");
    println!("  these exponents to `physics.rs`, and then RE-DERIVE the feel");
    println!("  numbers rather than scaling them, because Rocket League's were never");
    println!("  a robot's. That is a day of work and a playtest, not a switch.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neither_scale_fits_the_existing_clamp() {
        // the concrete blocker: the plumbing exists and the range forbids it
        for h in [1.75f32, 8.5] {
            let k = factor_for(h);
            assert!(k > 2.0, "{h} m needs only {k}x — the clamp would allow it");
        }
        assert!(factor_for(8.5) > 20.0);
    }

    #[test]
    fn accelerations_do_not_scale_and_that_is_the_trap() {
        // the counter-intuitive one. Gravity is already correct at any size,
        // so multiplying it makes a HEAVIER world rather than a bigger one.
        let k = factor_for(8.5);
        let grav = &CONSTANTS[0];
        assert_eq!(grav.kind, Kind::Accel);
        assert!((rescaled(grav, k) - grav.value).abs() < 1e-4, "gravity moved");
        // and the naive answer is wrong by the full factor
        assert!((naive_error(grav, k) - k).abs() < 0.01);
    }

    #[test]
    fn speeds_go_as_the_square_root_not_the_factor() {
        let k = factor_for(8.5);
        let top = &CONSTANTS[1];
        assert_eq!(top.kind, Kind::Speed);
        let want = rescaled(top, k);
        assert!((want / top.value - k.sqrt()).abs() < 0.01);
        // the naive answer is sqrt(k) times too fast — about five here
        assert!(naive_error(top, k) > 4.0 && naive_error(top, k) < 5.5);
    }

    #[test]
    fn mass_is_a_volume() {
        let k = factor_for(8.5);
        let m = CONSTANTS.iter().find(|t| t.name == "M_CAR").unwrap();
        assert_eq!(m.kind, Kind::Mass);
        // 180 kg becomes about two thousand tonnes at 22x
        let t = rescaled(m, k) / 1000.0;
        assert!(t > 1000.0, "a 8.5 m frame masses only {t:.0} t");
        // and the naive answer is wildly light
        assert!(naive_error(m, k) < 0.01);
    }

    #[test]
    fn every_constant_is_classified() {
        // an unclassified constant is one somebody will scale by k out of
        // habit, which is the mistake this module exists to prevent
        assert_eq!(CONSTANTS.len(), 9, "physics.rs gained or lost a constant");
        for t in CONSTANTS {
            assert!(!t.note.is_empty(), "{} has no note", t.name);
            assert!(t.value > 0.0);
        }
        // and the exponents are the Froude set, not something invented here
        assert_eq!(Kind::Length.exponent(), 1.0);
        assert_eq!(Kind::Speed.exponent(), 0.5);
        assert_eq!(Kind::Accel.exponent(), 0.0);
        assert_eq!(Kind::Mass.exponent(), 3.0);
    }

    #[test]
    fn a_rescaled_world_reads_gravity_off_the_destination() {
        // the payoff: once the body is real-sized, `patrol`'s gravity column
        // is directly usable instead of being compressed toward 1
        assert!((gravity_after_rescale(1.0) - 9.81).abs() < 0.01);
        assert!(gravity_after_rescale(0.166) < 2.0, "luna should be gentle");
        // every destination gives a sane number
        for d in crate::patrol::ROUTE.iter() {
            let g = gravity_after_rescale(d.gravity);
            assert!((0.0..60.0).contains(&g), "{} gives {g} m/s^2", d.id);
        }
    }

    #[test]
    fn froude_is_necessary_and_not_sufficient() {
        // The exponents are right and the INPUT is not. A Rocket League car
        // does sixty body lengths a second; scaling preserves that, so an
        // 8.5 m bot inherits 392 km/h. A cheetah manages about four body
        // lengths and is the fastest thing on land.
        //
        // Scaling tells you how a number changes with size. It cannot tell
        // you the number was never a robot's to begin with.
        let car_bl = 23.0 / BODY_H_M;
        assert!(car_bl > 50.0, "the RL car is {car_bl} bl/s");

        let k = factor_for(8.5);
        let scaled_top = rescaled(&CONSTANTS[1], k);
        let bot_bl = scaled_top / 8.5;

        // FROUDE DOES NOT PRESERVE BODY-LENGTHS-PER-SECOND — I asserted that
        // and it is wrong. Speed goes as sqrt(k) and length as k, so relative
        // speed goes as 1/sqrt(k): big things are relatively SLOWER, which is
        // why an elephant is not as quick for its size as a mouse. 61 bl/s
        // becomes 12.8.
        assert!(bot_bl < car_bl / 4.0, "{car_bl} -> {bot_bl}, not a fifth");
        assert!((bot_bl / car_bl - 1.0 / k.sqrt()).abs() < 0.01);

        // So the scaling does most of the work — and not all of it. A cheetah
        // manages about four body lengths a second and is the fastest thing
        // on land; the scaled bot is still three times that, and 392 km/h.
        assert!(bot_bl > 8.0, "the input turned out fine after all: {bot_bl}");
        assert!(scaled_top * 3.6 > 300.0);
    }

    #[test]
    fn scaling_is_reversible() {
        for t in CONSTANTS {
            let k = 22.4;
            let there = t.kind.scale(t.value, k);
            let back = t.kind.scale(there, 1.0 / k);
            assert!((back - t.value).abs() / t.value < 1e-4, "{} did not round trip", t.name);
        }
    }
}
