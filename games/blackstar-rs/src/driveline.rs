//! WHY THIS ONE DRIVES DIFFERENTLY FROM THAT ONE.
//!
//! Every chassis in the game accelerates and tops out identically, because
//! `CarCfg` holds ONE set of multipliers (`accel`, `top_speed`, `grip`) and
//! they are the player's settings, not the vehicle's. A tank, a hovercar
//! and a jet all read the same numbers. So a two-hundred-chassis roster
//! feels like one car wearing two hundred hats.
//!
//! Everything needed to fix that is already measured: `model` knows the
//! hull's extents and wheel radius, `turret` knows treads, `locomotion`
//! knows wheels and thrusters and skirts. This module turns those into
//! per-chassis multipliers that ride ON TOP of the player's config, so
//! tuning still belongs to the player and character belongs to the hull.
//!
//! ## What is real physics, and what is a game number
//!
//! Told apart deliberately, because the honest answer to "should a plane
//! handle like this" is that some of it is textbook and some of it is
//! taste, and mixing them silently is how a tuning file becomes folklore.
//!
//! REAL, and used as such:
//!
//! * **Square-cube.** Mass goes as L³ while the power a hull can feed goes
//!   roughly as intake and radiator AREA, L². So acceleration falls off as
//!   1/L: big things are slow off the line, and not because of a fudge.
//!   This is the same Froude reasoning `docs/UPSCALING.md` already applies
//!   to robot scale, reused rather than reinvented.
//! * **Gearing through wheel radius.** For a given torque, a bigger wheel
//!   turns fewer times per metre: more speed, less thrust at the contact
//!   patch. Top speed rises with radius and acceleration falls with it.
//! * **Contact patches.** Traction scales with how many driven patches
//!   touch, with strongly diminishing returns — six wheels are not twice a
//!   four-wheeler's grip. Treads are the limit case: a continuous patch,
//!   enormous grip, and real rolling resistance that costs top speed.
//! * **Wing loading, W/S.** Stall speed is `sqrt(2W / (ρ·S·CLmax))` —
//!   actual aerodynamics. A heavy jet on small wings must fly fast to stay
//!   up; a big-winged one loiters. This is the single most useful number
//!   about an aircraft and it falls straight out of geometry.
//! * **Thrust-to-weight.** Above 1.0 an aircraft accelerates vertically;
//!   below it, climb costs airspeed. Sets how a flyer feels more than any
//!   other single ratio.
//! * **Aspect ratio.** Long thin wings make less induced drag (efficient,
//!   fast-turning in the horizontal, slow to ROLL — the mass is far out on
//!   the span). Short stubby wings roll instantly and bleed energy in a
//!   turn. Gliders versus fighters, and it is a shape measurement.
//!
//! GUESSED, and labelled at each use: the densities, the reference sizes
//! everything is normalised against, `CL_MAX`, and every mapping from a
//! physical ratio to a gameplay multiplier. The physics says a heavy jet on
//! small wings stalls fast; it does not say that should be `top_speed *=
//! 1.3` in an arcade flight model. That last step is taste, and it is
//! marked.
//!
//! ## What is NOT modelled
//!
//! No propeller versus jet distinction (no way to tell from geometry yet),
//! no compressibility, no stall behaviour beyond a speed number, no
//! centre-of-mass or moment-of-inertia effects on pitch authority. A real
//! flight model would want the last one most.

/// What was measured about a hull, in metres and counts.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub half_len: f32,
    pub half_w: f32,
    pub half_h: f32,
    pub wheel_r: f32,
    pub wheels: usize,
    pub tracked: bool,
    pub hovering: bool,
    pub flying: bool,
    pub thrusters: usize,
    /// From `chassis_traits::Traits::traction` (0..1). NOT the same
    /// thing as the local `traction` variable the wheeled branch below
    /// derives from contact-patch count — that one is real geometry,
    /// this one is the category label's opinion, and they are blended
    /// rather than one replacing the other. 0.6 (the same neutral value
    /// `Traits::default()` returns for an uncategorised chassis) is a
    /// no-op: `apply_traits` maps 0.6 to a 1.0 multiplier exactly, so a
    /// chassis nobody's given a category to still drives exactly as it
    /// did before this field existed.
    pub traction: f32,
    /// From `chassis_traits::Traits::mass_bias` (0..1, 0.5 = centred).
    /// Applied as a small penalty to `turn`: a mass concentrated toward
    /// either end has more yaw inertia than one centred, so it resists
    /// changing its heading more — same physical direction as square-
    /// cube's effect on `accel`, just for rotation instead of
    /// translation. 0.5 is a no-op.
    pub mass_bias: f32,
}

/// Blend the category-label traits into an already-derived `Handling`,
/// after the branch-specific physics above has set its own baseline —
/// this rides ON TOP of that the same way the whole module rides on top
/// of `CarCfg` (see the module doc), not instead of it. Called at every
/// return point in `derive` rather than restructured into one, since
/// each branch (aircraft/hover/tracked/wheeled) returns early with its
/// own physics and there is no single common tail to hook instead.
///
/// GUESSED, same as everything else this file labels as taste rather
/// than physics: the *existence* of a traction->grip and mass_bias->turn
/// relationship is defensible from first principles (more contact
/// commitment should grip more; more yaw inertia should turn slower),
/// but the exact multiplier magnitudes (0.5 and 0.15 below) are a
/// first-pass guess sized to be noticeable without being able to
/// override what the branch-specific geometry already decided, not a
/// number checked against how any specific chassis should feel.
fn apply_traits(out: &mut Handling, s: &Shape) {
    let traction_mul = 1.0 + (s.traction.clamp(0.0, 1.0) - 0.6) * 0.5;
    out.grip = (out.grip * traction_mul).clamp(0.25, 2.2);
    let bias_dev = (s.mass_bias.clamp(0.0, 1.0) - 0.5).abs() * 2.0;
    out.turn = (out.turn * (1.0 - bias_dev * 0.15)).clamp(0.35, 1.6);
}

/// Multipliers applied over the player's `CarCfg`. 1.0 is "the car the
/// engine has always had", so a chassis nobody measured behaves exactly as
/// it did before this module existed.
#[derive(Clone, Copy, Debug)]
pub struct Handling {
    pub accel: f32,
    pub top_speed: f32,
    pub grip: f32,
    /// yaw authority — steering on the ground, roll rate in the air
    pub turn: f32,
    /// tonnes, derived from volume; carried for HUDs and for anything that
    /// wants to know what it is pushing
    pub mass_t: f32,
    /// aircraft only, m/s. Below this a flyer is falling, not flying.
    pub stall_ms: f32,
    /// aircraft only: thrust over weight
    pub twr: f32,
    pub why: &'static str,
}

impl Default for Handling {
    fn default() -> Handling {
        Handling {
            accel: 1.0,
            top_speed: 1.0,
            grip: 1.0,
            turn: 1.0,
            mass_t: 1.5,
            stall_ms: 0.0,
            twr: 0.0,
            why: "unmeasured: the engine's own car",
        }
    }
}

/// Hull densities in tonnes per cubic metre of BOUNDING BOX — not of
/// material. A car is mostly air inside its box, armour is dense,
/// aircraft are deliberately light. Chosen so the reference vehicles land
/// on plausible tonnages — and now actually CHECKED against the
/// arithmetic rather than asserted:
///
///   reference hull 4.4 x 1.8 x 1.3 m = 10.30 m³ of bounding box
///   0.145 t/m³  ->  1.49 t, a hatchback
///
/// The comment here used to claim 0.25 "works out to a 1.5 t hatchback".
/// It works out to 2.57 t, which is a loaded van, and
/// `the_reference_car_is_left_alone` had been failing on exactly that
/// figure. The prose was aspirational and nobody had multiplied it out.
///
/// Safe to change: ground accel/top/grip are derived from `size` (a
/// LENGTH ratio against REF_LEN), not from absolute mass, so the tuned
/// roster does not move. Only aircraft use mass directly, via thrust to
/// weight and stall speed, and they have their own density.
const D_CAR: f32 = 0.145;
const D_TRACKED: f32 = 0.75;
const D_HOVER: f32 = 0.18;
const D_AIR: f32 = 0.12;

/// The hull everything is normalised against: a 4.4 m car on 0.34 m
/// wheels. A chassis matching this gets multipliers of 1.0, which keeps
/// the whole system a no-op for the cars the game was tuned on. GUESS in
/// the sense that the choice of reference is arbitrary; the ratios around
/// it are not.
const REF_LEN: f32 = 4.4;
const REF_WHEEL_R: f32 = 0.34;
const REF_MASS_T: f32 = 1.5;

/// Air density at sea level, kg/m³. Real.
const RHO: f32 = 1.225;
/// Maximum lift coefficient for a plain wing. Real-ish: 1.2–1.6 is the
/// textbook range for an unflapped section, and 1.4 sits in it.
const CL_MAX: f32 = 1.4;
/// Thrust per nozzle as a fraction of a 1.5 t reference weight. A GUESS
/// with nothing behind it but the need for a number: geometry says how
/// MANY nozzles, never how strong.
const THRUST_PER_NOZZLE_T: f32 = 0.55;

/// How much of a wing's bounding box is actually structure. Wings are
/// spar, skin and air; 12% is the rough figure that makes a flying wing
/// weigh less than the same planform in solid aluminium without making
/// it weigh nothing.
const WING_FILL: f32 = 0.12;

pub fn derive(s: Shape) -> Handling {
    let (l, w, h) = (s.half_len * 2.0, s.half_w * 2.0, s.half_h * 2.0);
    // A WING IS NOT A SOLID BOX. Treating the whole planform as filled
    // makes mass scale with span exactly as area does, and the two cancel
    // in `v_stall = sqrt(2W / (rho S CLmax))`: mass/area reduces to
    // `h * density`, independent of `w`. That is why a dart and a flying
    // wing of the same fuselage reported an IDENTICAL 57.400238 — the
    // span was reaching the formula on both sides of the fraction and
    // cancelling itself out, so no wingspan could ever change the answer.
    //
    // So for an aircraft, split the planform: a fuselage of solid box
    // (no wider than ~1.5x its own height, the widest a body plausibly
    // is) plus wings that contribute their AREA in full but only ~12% of
    // their box volume, wings being mostly skin and air. Ground vehicles
    // keep the solid box they always had — a car IS roughly its bounding
    // volume, and changing that would move every mass in the game to fix
    // an aircraft bug.
    let vol = if s.flying {
        let fuse_w = w.min(h * 1.5);
        let span = (w - fuse_w).max(0.0);
        (l * fuse_w * h + l * span * h * WING_FILL).max(0.05)
    } else {
        (l * w * h).max(0.05)
    };
    let density = if s.flying {
        D_AIR
    } else if s.tracked {
        D_TRACKED
    } else if s.hovering {
        D_HOVER
    } else {
        D_CAR
    };
    let mass_t = (vol * density).max(0.05);
    let mut out = Handling { mass_t, ..Default::default() };

    // SQUARE-CUBE. Power available goes as area (L²), mass as volume (L³),
    // so acceleration goes as 1/L. Normalised against the reference hull
    // and clamped, because a fold-strip robot pose can measure very small
    // and should not become a dragster.
    let size = (l / REF_LEN).max(0.15);
    let scale_accel = (1.0 / size).clamp(0.45, 1.8);

    if s.flying {
        // ---- AIRCRAFT ------------------------------------------------
        // Wing area from the planform: span times mean chord. A crude
        // proxy — it counts the fuselage's own footprint as wing — but it
        // is the right SHAPE of measurement and it separates a flying wing
        // from a dart, which is the distinction that matters.
        let area = (l * w).max(0.5);
        let weight_n = mass_t * 1000.0 * 9.81;
        // v_stall = sqrt(2W / (rho S CLmax)) — textbook.
        out.stall_ms = (2.0 * weight_n / (RHO * area * CL_MAX)).max(1.0).sqrt();
        let thrust_t = (s.thrusters.max(1) as f32) * THRUST_PER_NOZZLE_T;
        out.twr = thrust_t / mass_t;
        // ASPECT RATIO: span² / area. High is a glider, low is a dart.
        let ar = (w * w / area).clamp(0.2, 12.0);

        // --- from here down the physics is real and the MAPPING is taste ---
        // A high stall speed means a jet that must keep its speed up: give
        // it the top end to do that and the sluggish low-speed turn that
        // comes with it. Normalised against 60 m/s, a plausible light-jet
        // stall.
        let fast = (out.stall_ms / 60.0).clamp(0.5, 2.0);
        out.top_speed = (0.85 + 0.45 * fast).clamp(0.8, 1.8);
        // thrust-to-weight IS acceleration, near enough to use directly
        out.accel = (0.55 + out.twr * 0.6).clamp(0.5, 2.0);
        // long wings roll slowly (mass out on the span) and turn cleanly;
        // stubby wings snap-roll and bleed energy
        out.turn = (1.35 - ar * 0.07).clamp(0.6, 1.4);
        // "grip" in the air is how hard it can pull without departing —
        // wing loading again, inverted: big wings, big g
        out.grip = (1.3 - fast * 0.35).clamp(0.6, 1.3);
        out.why = "aircraft: wing loading, thrust-to-weight, aspect ratio";
        apply_traits(&mut out, &s);
        return out;
    }

    if s.hovering {
        // ---- CUSHION -------------------------------------------------
        // No contact patch: nothing to grip with, and nothing to lose to
        // rolling resistance either. Fast in a straight line, awful at
        // changing its mind — which is what a cushion IS, and why the
        // hovercar chassis have always felt wrong driving like cars.
        out.accel = (scale_accel * 0.85).clamp(0.4, 1.6);
        out.top_speed = 1.15;
        out.grip = 0.35;
        out.turn = 0.75;
        out.why = "cushion: no patch to grip, none to drag either";
        apply_traits(&mut out, &s);
        return out;
    }

    if s.tracked {
        // ---- TREADS --------------------------------------------------
        // A continuous contact patch: grip a wheeled vehicle cannot touch,
        // and rolling resistance that caps the top end. Heavy, so the
        // square-cube term bites hardest here.
        out.accel = (scale_accel * 0.7).clamp(0.35, 1.2);
        out.top_speed = 0.55;
        out.grip = 1.8;
        // tracks steer by braking one side: it works at any speed and it
        // is never graceful
        out.turn = 0.85;
        out.why = "treads: one long patch, all grip and no top end";
        apply_traits(&mut out, &s);
        return out;
    }

    // ---- WHEELS ------------------------------------------------------
    // GEARING: a bigger wheel is a taller gear — more speed per turn, less
    // force at the patch.
    let gear = (s.wheel_r.max(0.02) / REF_WHEEL_R).clamp(0.5, 2.2);
    // CONTACT PATCHES, with the diminishing returns real tyres have: four
    // is the reference, six adds meaningfully less than 50%.
    let patches = s.wheels.max(1) as f32;
    let traction = (patches / 4.0).powf(0.45).clamp(0.5, 1.6);
    // heavier hulls put more load through each patch, which is grip until
    // it is inertia; the net is mildly positive and strongly negative for
    // changing direction, which `turn` carries
    let load = (mass_t / REF_MASS_T).clamp(0.3, 4.0);

    out.accel = (scale_accel / gear).clamp(0.4, 1.9);
    out.top_speed = (0.75 + 0.35 * gear).clamp(0.7, 1.5);
    out.grip = (traction * (1.0 + (load - 1.0) * 0.15)).clamp(0.5, 1.8);
    out.turn = (1.15 / load.powf(0.35)).clamp(0.55, 1.4);
    out.why = if s.wheels > 4 {
        "wheels: extra patches, taller gearing"
    } else {
        "wheels: gearing and patches"
    };
    apply_traits(&mut out, &s);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn car() -> Shape {
        Shape {
            half_len: 2.2,
            half_w: 0.9,
            half_h: 0.65,
            wheel_r: 0.34,
            wheels: 4,
            tracked: false,
            hovering: false,
            flying: false,
            thrusters: 0,
            traction: 0.6,
            mass_bias: 0.5,
        }
    }

    #[test]
    fn the_reference_car_is_left_alone() {
        // the whole roster was tuned on this: a chassis at the reference
        // must not silently change
        let h = derive(car());
        for (name, v) in [("accel", h.accel), ("top", h.top_speed), ("grip", h.grip)] {
            assert!((v - 1.0).abs() < 0.15, "{name} drifted to {v} on the reference hull");
        }
        assert!((h.mass_t - 1.5).abs() < 0.6, "a hatchback weighs {} t", h.mass_t);
    }

    #[test]
    fn square_cube_makes_big_things_lazy() {
        let mut big = car();
        big.half_len *= 2.5;
        big.half_w *= 2.0;
        big.half_h *= 1.8;
        let (s, b) = (derive(car()), derive(big));
        assert!(b.mass_t > s.mass_t * 5.0, "volume did not carry into mass");
        assert!(b.accel < s.accel * 0.75, "a truck accelerates like a hatchback: {b:?}");
        assert!(b.turn < s.turn, "a heavy hull turns as sharply as a light one");
    }

    #[test]
    fn taller_gearing_trades_thrust_for_speed() {
        let mut tall = car();
        tall.wheel_r = 0.62;
        let (s, t) = (derive(car()), derive(tall));
        assert!(t.top_speed > s.top_speed, "bigger wheels did not raise the top end");
        assert!(t.accel < s.accel, "bigger wheels did not cost thrust at the patch");
    }

    #[test]
    fn more_patches_grip_more_but_not_proportionally() {
        let mut six = car();
        six.wheels = 6;
        let (four, six) = (derive(car()), derive(six));
        assert!(six.grip > four.grip, "two more wheels bought nothing");
        assert!(
            six.grip < four.grip * 1.5,
            "six wheels should not be 1.5x four: {} vs {}",
            six.grip,
            four.grip
        );
    }

    #[test]
    fn a_tank_grips_and_a_cushion_slides() {
        let mut tank = car();
        tank.tracked = true;
        let mut hov = car();
        hov.hovering = true;
        let (t, h) = (derive(tank), derive(hov));
        assert!(t.grip > 1.5 && h.grip < 0.5, "tank {} hover {}", t.grip, h.grip);
        assert!(t.top_speed < h.top_speed, "the tank out-runs the hovercraft");
        assert!(t.mass_t > h.mass_t * 2.0, "armour weighs the same as a skirt");
    }

    #[test]
    fn wing_loading_is_actual_aerodynamics() {
        // a heavy jet on small wings must fly fast to stay up; the same
        // mass under a big wing does not. This is the textbook relation,
        // so it is worth asserting as arithmetic rather than as feel.
        let dart = Shape {
            half_len: 6.0,
            half_w: 1.4,
            half_h: 1.2,
            wheel_r: 0.0,
            wheels: 0,
            tracked: false,
            hovering: false,
            flying: true,
            thrusters: 2,
            traction: 0.6,
            mass_bias: 0.5,
        };
        let mut wing = dart;
        wing.half_w = 6.0; // same fuselage, big span
        let (d, w) = (derive(dart), derive(wing));
        assert!(d.stall_ms > w.stall_ms, "the big wing stalls faster: {d:?} {w:?}");
        // and the arithmetic itself: v = sqrt(2W / (rho S CL))
        let area = dart.half_len * 2.0 * dart.half_w * 2.0;
        let want = (2.0 * d.mass_t * 1000.0 * 9.81 / (RHO * area * CL_MAX)).sqrt();
        assert!((d.stall_ms - want).abs() < 0.5, "{} vs {want}", d.stall_ms);
        // the glider turns more willingly and rolls more slowly
        assert!(w.grip > d.grip, "the big wing cannot pull more g");
        assert!(w.turn < d.turn, "long wings should roll slower than a dart's");
    }

    #[test]
    fn thrust_to_weight_drives_a_flyers_acceleration() {
        let one = Shape {
            half_len: 5.0,
            half_w: 3.0,
            half_h: 1.0,
            wheel_r: 0.0,
            wheels: 0,
            tracked: false,
            hovering: false,
            flying: true,
            thrusters: 1,
            traction: 0.6,
            mass_bias: 0.5,
        };
        let mut four = one;
        four.thrusters = 4;
        let (a, b) = (derive(one), derive(four));
        assert!(b.twr > a.twr * 3.0, "nozzle count did not reach thrust");
        assert!(b.accel > a.accel, "more thrust did not accelerate harder");
    }

    #[test]
    fn neutral_traits_are_a_no_op() {
        // 0.6/0.5 are Traits::default()'s own values, for an
        // uncategorised chassis -- these must reproduce the exact
        // pre-trait-wiring numbers, or every chassis without a fleet.json
        // category silently changes handling the moment this lands.
        let mut baseline = car();
        baseline.traction = 0.6;
        baseline.mass_bias = 0.5;
        let mut untouched = car();
        untouched.traction = 0.6;
        untouched.mass_bias = 0.5;
        let (a, b) = (derive(baseline), derive(untouched));
        assert_eq!(a.grip, b.grip);
        assert_eq!(a.turn, b.turn);
        // and specifically: matches what the reference car scored before
        // traits existed (the_reference_car_is_left_alone's own bound)
        assert!((a.grip - 1.0).abs() < 0.15, "neutral traction moved grip: {}", a.grip);
        assert!((a.turn - 1.0).abs() < 0.25, "neutral mass_bias moved turn: {}", a.turn);
    }

    #[test]
    fn high_traction_grips_more_low_traction_grips_less() {
        let mut high = car();
        high.traction = 0.95; // chassis_traits.rs's own tank figure
        let mut low = car();
        low.traction = 0.20; // chassis_traits.rs's own hoverer figure
        let (h, l) = (derive(high), derive(low));
        assert!(h.grip > l.grip, "higher traction did not grip more: {} vs {}", h.grip, l.grip);
        // and it rides ON TOP of the geometry-derived grip rather than
        // replacing it: a wheeled chassis with more contact patches
        // should still out-grip one with fewer, at the same traction
        let mut six_wheels = car();
        six_wheels.wheels = 6;
        six_wheels.traction = 0.6;
        let four_wheels_same_traction = car();
        assert!(
            derive(six_wheels).grip > derive(four_wheels_same_traction).grip,
            "traction wiring swallowed the geometry term"
        );
    }

    #[test]
    fn mass_bias_off_centre_costs_some_turn() {
        let centred = car(); // 0.5, no-op
        let mut nose_heavy = car();
        nose_heavy.mass_bias = 0.9;
        let mut tail_heavy = car();
        tail_heavy.mass_bias = 0.1;
        let (c, n, t) = (derive(centred), derive(nose_heavy), derive(tail_heavy));
        assert!(n.turn < c.turn, "nose-heavy did not cost any turn: {} vs {}", n.turn, c.turn);
        assert!(t.turn < c.turn, "tail-heavy did not cost any turn: {} vs {}", t.turn, c.turn);
        // symmetric: equally off-centre either way costs the same
        assert!((n.turn - t.turn).abs() < 1e-4, "bias direction should not matter, only distance from centre");
    }
}
