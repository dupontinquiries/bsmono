//! SUITABILITY — which way of getting around survives which world.
//!
//! `patrol::Destination` already carries the whole environment vector:
//! `gravity` in g (0.0 at the shield edge, 1.15 at the heaviest surface on the
//! route), `weather` (does air move), `terrain` (surface, tumbling rock, open
//! space) and `sky` (atmosphere, vacuum, ice glare, ember, murk). Sixteen
//! distinct gravities are authored across the route. NOTHING READ ANY OF IT to
//! decide what you should be driving.
//!
//! That is the gap this module closes, and the reason it matters is that
//! gravity is not a feel knob to be tuned once — it is a design input. Low g
//! means a car cannot brake and a jump becomes a launch. No air means a
//! turbine has nothing to bite and a hover skirt has nothing to sit on. Dust
//! in motion means an intake eats the planet. Those are the choices the player
//! should be making before they drop, and every input is already in the table.
//!
//! ## The rules are physics, not taste
//!
//! * **Traction scales with weight.** Available friction is `mu * m * g`, so
//!   at 0.06 g a wheeled body has six percent of the grip it was designed
//!   around: it cannot accelerate, corner or stop. This is a hard floor, not a
//!   handling penalty.
//! * **A cushion needs a fluid.** Hover is pressure under a skirt. In vacuum
//!   there is nothing to pressurise, at any gravity.
//! * **Airbreathing needs air.** A turbine in vacuum is ballast. A reaction
//!   drive does not care, which is why [`Intake`] is a separate input rather
//!   than folded into `locomotion::Mode` — the existing enum says HOW a thing
//!   moves, not WHAT IT BREATHES, and inventing a variant would have put a
//!   propulsion fact inside a shape classifier.
//! * **No floor, no ground modes.** `Terrain::DeepSpace` and
//!   `Terrain::AsteroidField` are the cases the route already distinguishes.
//! * **Moving dust wrecks intakes.** `weather` plus a particulate sky is the
//!   condition, and it degrades rather than forbids — you can fly a turbine
//!   through a dust storm, you just should not plan to.
//!
//! ## It explains itself
//!
//! Same contract as `locomotion::Decision::why`: a verdict a driver cannot
//! interrogate is a verdict they will route around. [`Verdict`] carries the
//! reasons that fired, so a loadout screen can print WHY the racer is greyed
//! out on Io instead of just greying it out.
//!
//! This module decides nothing about damage, speed or handling. It answers one
//! question — should you take this thing down there — and leaves the driving
//! to `physics.rs`.

use crate::locomotion::Mode;
use crate::patrol::{Destination, Sky, Terrain};

/// What the propulsion consumes. Orthogonal to `locomotion::Mode`, which is
/// about shape and contact rather than about breathing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Intake {
    /// Turbines, fans, ducted lift: needs a working fluid from outside.
    Airbreathing,
    /// Rockets, ion, gravitic: carries its own reaction mass.
    Reaction,
    /// Wheels, legs, tracks: consumes nothing, needs ground.
    None,
}

/// How well it will go.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Fitness {
    /// Works as designed.
    Good,
    /// Will function, worse. Bring it if you have a reason.
    Degraded,
    /// Will not work at all. Not a penalty — an absence of the thing it
    /// needs.
    Unusable,
}

/// One thing the environment did to this choice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Reason {
    pub fitness: Fitness,
    pub because: &'static str,
}

/// The answer, with its working shown.
#[derive(Clone, Debug)]
pub struct Verdict {
    pub fitness: Fitness,
    pub reasons: Vec<Reason>,
}

impl Verdict {
    pub fn usable(&self) -> bool {
        self.fitness != Fitness::Unusable
    }
    /// The reason that decided it — the worst one. `None` on a clean pass.
    pub fn deciding(&self) -> Option<Reason> {
        self.reasons.iter().copied().max_by_key(|r| r.fitness)
    }
}

/// Below this, in g, a wheeled body has too little weight to put force
/// through its contact patch. 0.10 g is a tenth of the grip the chassis was
/// designed around; the route has five destinations under it.
pub const TRACTION_FLOOR_G: f32 = 0.10;
/// Below this, wheels still bite but badly.
pub const TRACTION_POOR_G: f32 = 0.25;
/// Above this, a flyer is fighting its own weight on every lift cycle.
pub const HEAVY_LIFT_G: f32 = 1.10;

/// Does this sky have something a turbine or a skirt can push against.
fn has_working_fluid(sky: Sky) -> bool {
    match sky {
        Sky::Atmosphere | Sky::Murk | Sky::Ember => true,
        // ice haze is haze in near-vacuum: visually thick, mechanically empty
        Sky::Glare | Sky::Vacuum => false,
        // deep space, and the shield seen from under it — both airless
        Sky::Starfield | Sky::Lattice => false,
    }
}

/// A sky that carries particulates — grit, ash, ice crystal. Only matters
/// when the air is moving.
fn particulate(sky: Sky) -> bool {
    matches!(sky, Sky::Murk | Sky::Ember | Sky::Glare)
}

/// Should you take this down there.
pub fn assess(dest: &Destination, mode: Mode, intake: Intake) -> Verdict {
    let mut reasons: Vec<Reason> = Vec::new();
    let fluid = has_working_fluid(dest.sky);
    let grounded = matches!(dest.terrain, Terrain::Surface | Terrain::Procedural);

    // -- no floor --------------------------------------------------------
    if !grounded && matches!(mode, Mode::Wheeled | Mode::Hover) {
        reasons.push(Reason {
            fitness: Fitness::Unusable,
            because: "nothing to stand on: this is open space or tumbling rock",
        });
    }

    // -- traction --------------------------------------------------------
    if mode == Mode::Wheeled && grounded {
        if dest.gravity < TRACTION_FLOOR_G {
            reasons.push(Reason {
                fitness: Fitness::Unusable,
                because: "too little weight for grip: it will not accelerate, turn or stop",
            });
        } else if dest.gravity < TRACTION_POOR_G {
            reasons.push(Reason {
                fitness: Fitness::Degraded,
                because: "light on its wheels: long stopping distance, easy to spin",
            });
        }
    }

    // -- hovering, which is two different machines ----------------------
    //
    // A SKIRT is pressure trapped under a lip and needs a fluid to trap.
    // THRUSTER LIFT — ion, plasma, whatever the hull is carrying — pushes
    // against its own reaction mass and does not care what is outside. Which
    // one a body has is `intake`, not `mode`: this is the same distinction
    // that makes a turbine and a rocket different flyers, and hover has it
    // too. Getting this wrong is how a fleet of reaction-lift hovercraft ends
    // up refused on every airless world in the system.
    if mode == Mode::Hover && grounded {
        match intake {
            // passive ground effect: no engine of its own, so the fluid is
            // the only thing holding it up
            Intake::None if !fluid => reasons.push(Reason {
                fitness: Fitness::Unusable,
                because: "no atmosphere to pressurise: a passive skirt has nothing to sit on",
            }),
            // ducted fans and lift turbines: an engine, but one that breathes.
            // The airbreathing rule below already refuses it in vacuum; this
            // note is for the case where the air is there but thin work.
            Intake::Airbreathing => {}
            // thruster lift holds altitude by burning, and weight is what it
            // is burning against
            Intake::Reaction if dest.gravity > HEAVY_LIFT_G => reasons.push(Reason {
                fitness: Fitness::Degraded,
                because: "heavy world: thruster lift burns hard just to stay off the ground",
            }),
            _ => {}
        }
    }

    // -- breathing -------------------------------------------------------
    if intake == Intake::Airbreathing && !fluid {
        reasons.push(Reason {
            fitness: Fitness::Unusable,
            because: "airbreathing propulsion in vacuum: the intake is ballast",
        });
    }
    if intake == Intake::Airbreathing && dest.weather && particulate(dest.sky) {
        reasons.push(Reason {
            fitness: Fitness::Degraded,
            because: "moving dust into an open intake: expect erosion and flameout",
        });
    }

    // -- lift ------------------------------------------------------------
    if mode == Mode::Flight {
        if dest.gravity > HEAVY_LIFT_G {
            reasons.push(Reason {
                fitness: Fitness::Degraded,
                because: "heavy world: lift costs more than the airframe was drawn for",
            });
        }
        if intake == Intake::Reaction && !fluid && dest.gravity > TRACTION_POOR_G {
            reasons.push(Reason {
                fitness: Fitness::Degraded,
                because: "no aerodynamic lift here: every metre of altitude is burnt fuel",
            });
        }
    }

    let fitness = reasons.iter().map(|r| r.fitness).max().unwrap_or(Fitness::Good);
    Verdict { fitness, reasons }
}

/// The best of the three ground/air modes for a place, for a default
/// suggestion. Ties break toward the simplest thing that works.
pub fn best_mode(dest: &Destination, intake: Intake) -> Option<Mode> {
    [Mode::Wheeled, Mode::Hover, Mode::Flight]
        .into_iter()
        .map(|m| (m, assess(dest, m, intake).fitness))
        .filter(|(_, f)| *f != Fitness::Unusable)
        .min_by_key(|(_, f)| *f)
        .map(|(m, _)| m)
}


// ------------------------------------------------------- getting out of it ---
//
// `assess` answers whether a mode WORKS here. It does not answer what it costs
// to stop being in it, and that turns out to be the more tactical question.
//
// A car that folds is already standing where it was driving. A jet is a
// thousand feet up doing three hundred, and becoming a robot means arriving at
// the ground first — which is a separate event with its own duration, and one
// the enemy gets to watch. In a corridor it is worse than slow: there is no
// room to unfold into, and the mode you wanted is simply unavailable.
//
// The inversion worth knowing, because it falls out of the physics rather than
// out of tuning: LOW GRAVITY MAKES FLIGHT CHEAP TO HOLD AND EXPENSIVE TO LEAVE.
// Free-fall time from a given altitude is `sqrt(2h/g)`, so at 0.13 g coming
// down takes nearly three times as long as at 1 g. The place where flying is
// most efficient is the place where you are most committed to it.

/// The room available where the fold would happen. A caller fills this from
/// `worldhost::WorldHost::clearance` probes; this module does no world queries
/// of its own.
#[derive(Clone, Copy, Debug)]
pub struct Space {
    /// Metres of clear air above.
    pub headroom_m: f32,
    /// Metres of clear space to the sides, the narrower of the two.
    pub width_m: f32,
}

impl Space {
    /// Outdoors, nothing near.
    pub const OPEN: Space = Space { headroom_m: 1000.0, width_m: 1000.0 };
}

/// The altitude a flyer is assumed to be holding when the order comes. Not a
/// simulation of where it actually is — a nominal number so the comparison
/// between worlds is honest, since the same altitude costs different time
/// under different gravity.
pub const ENGAGEMENT_ALT_M: f32 = 120.0;
/// Standard gravity, m/s², for turning `Destination::gravity` into an
/// acceleration.
pub const G0: f32 = 9.806_65;
/// Headroom a body needs to unfold and climb away under power.
pub const CLIMB_OUT_HEADROOM_M: f32 = 40.0;
/// Width a body needs to unfold into at all, in any mode.
pub const UNFOLD_WIDTH_M: f32 = 6.0;

/// What changing modes costs here.
#[derive(Clone, Debug)]
pub struct Transition {
    /// Seconds from the order to being useful in the new mode, including the
    /// fold itself.
    pub seconds: f32,
    /// Set when it cannot be done here at all.
    pub blocked: Option<&'static str>,
    /// Everything that contributed, for a UI that explains itself.
    pub notes: Vec<&'static str>,
}

impl Transition {
    pub fn possible(&self) -> bool {
        self.blocked.is_none()
    }
}

/// Time to fall `ENGAGEMENT_ALT_M` under this world's gravity, in seconds.
/// Vacuum free-fall: no drag term, because the worlds where this matters most
/// are the ones with no air to provide one.
pub fn descent_seconds(gravity_g: f32) -> f32 {
    let g = (gravity_g * G0).max(0.05);
    (2.0 * ENGAGEMENT_ALT_M / g).sqrt()
}

/// What it costs to go from `from` to `to` at this place, in this much room.
pub fn transition(from: Mode, to: Mode, dest: &Destination, space: Space) -> Transition {
    let mut notes = Vec::new();
    let mut blocked = None;
    // The fold itself, from `xform::BLEND_SEC` — one number, shared with the
    // thing that actually animates it.
    let mut seconds = crate::xform::BLEND_SEC;

    if from == to {
        return Transition { seconds: 0.0, blocked: None, notes };
    }

    if space.width_m < UNFOLD_WIDTH_M {
        blocked = Some("no room to unfold: the body is wider than the space");
        notes.push("tight quarters");
    }

    // Leaving flight means arriving at the ground first.
    if from == Mode::Flight {
        let d = descent_seconds(dest.gravity);
        seconds += d;
        if dest.gravity < TRACTION_POOR_G {
            notes.push("light world: the descent is the slow part, and it is watched");
        } else {
            notes.push("descent before the fold");
        }
    }

    // Entering flight needs somewhere to go.
    if to == Mode::Flight {
        if space.headroom_m < CLIMB_OUT_HEADROOM_M {
            blocked = blocked.or(Some("no headroom to climb out: this is an indoor mode change"));
        }
        if dest.gravity > HEAVY_LIFT_G {
            seconds += 1.0;
            notes.push("heavy world: slow off the ground");
        }
    }

    // Wheeled is the cheap one, and this is the whole tactical point of it.
    if from == Mode::Wheeled || to == Mode::Wheeled {
        notes.push("ground mode: already where the fight is");
    }

    Transition { seconds, blocked, notes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patrol::ROUTE;

    fn dest(id: &str) -> &'static Destination {
        ROUTE.iter().find(|d| d.id == id).unwrap_or_else(|| panic!("no destination {id}"))
    }


    #[test]
    fn leaving_flight_is_slower_where_gravity_is_weaker() {
        // The inversion: the world that makes flying cheapest makes quitting
        // it dearest. Physics, not balance.
        let light = dest("glaciem"); // 0.44 g
        let heavy = ROUTE
            .iter()
            .filter(|d| matches!(d.terrain, Terrain::Surface))
            .max_by(|a, b| a.gravity.partial_cmp(&b.gravity).unwrap())
            .unwrap();
        assert!(heavy.gravity > light.gravity, "picked the wrong pair");
        let t_light = transition(Mode::Flight, Mode::Wheeled, light, Space::OPEN);
        let t_heavy = transition(Mode::Flight, Mode::Wheeled, heavy, Space::OPEN);
        assert!(
            t_light.seconds > t_heavy.seconds,
            "{:.1}s on {} g vs {:.1}s on {} g — the descent term is not doing anything",
            t_light.seconds,
            light.gravity,
            t_heavy.seconds,
            heavy.gravity
        );
    }

    #[test]
    fn a_car_folds_where_it_stands_and_a_jet_does_not() {
        let d = dest("glaciem");
        let car = transition(Mode::Wheeled, Mode::Hover, d, Space::OPEN);
        let jet = transition(Mode::Flight, Mode::Hover, d, Space::OPEN);
        assert!(
            jet.seconds > car.seconds * 2.0,
            "a jet reached the ground in {:.1}s against a car's {:.1}s",
            jet.seconds,
            car.seconds
        );
        assert_eq!(car.seconds, crate::xform::BLEND_SEC, "a ground fold should be just the fold");
    }

    #[test]
    fn a_corridor_forbids_taking_off_but_not_folding_up() {
        let d = dest("glaciem");
        let tight = Space { headroom_m: 4.0, width_m: 9.0 };
        assert!(
            !transition(Mode::Wheeled, Mode::Flight, d, tight).possible(),
            "took off indoors"
        );
        assert!(
            transition(Mode::Flight, Mode::Wheeled, d, tight).possible(),
            "coming down should still be allowed; it is the climb that needs sky"
        );
    }

    #[test]
    fn something_narrower_than_the_body_refuses_every_fold() {
        let d = dest("glaciem");
        let vice = Space { headroom_m: 1000.0, width_m: 2.0 };
        for (a, b) in [(Mode::Wheeled, Mode::Hover), (Mode::Flight, Mode::Wheeled)] {
            let t = transition(a, b, d, vice);
            assert!(!t.possible(), "{a:?} -> {b:?} unfolded inside two metres");
            assert!(t.blocked.unwrap().contains("unfold"));
        }
    }

    #[test]
    fn the_transition_says_why_as_well_as_how_long() {
        // Same contract as `locomotion::Decision::why` and `Verdict::deciding`:
        // a number a driver cannot interrogate is a number they will not trust.
        let d = dest("glaciem");
        let t = transition(Mode::Flight, Mode::Wheeled, d, Space::OPEN);
        assert!(!t.notes.is_empty(), "a descent happened and nothing said so");
        assert!(
            t.notes.iter().any(|n| n.contains("descent")),
            "notes did not mention the descent: {:?}",
            t.notes
        );
        let car = transition(Mode::Wheeled, Mode::Hover, d, Space::OPEN);
        assert!(car.notes.iter().any(|n| n.contains("ground mode")), "{:?}", car.notes);
    }

    #[test]
    fn staying_put_costs_nothing() {
        let d = dest("glaciem");
        let t = transition(Mode::Flight, Mode::Flight, d, Space::OPEN);
        assert_eq!(t.seconds, 0.0);
        assert!(t.possible());
    }

    #[test]
    fn the_route_actually_spans_enough_gravity_for_this_to_matter() {
        // If every destination were 1 g this module would be theatre. It is
        // not: the authored spread is what makes the choice real.
        let lo = ROUTE.iter().map(|d| d.gravity).fold(f32::INFINITY, f32::min);
        let hi = ROUTE.iter().map(|d| d.gravity).fold(0.0f32, f32::max);
        assert!(lo <= 0.02, "lightest destination is {lo} g");
        assert!(hi >= 1.0, "heaviest destination is {hi} g");
        let under_floor = ROUTE.iter().filter(|d| d.gravity < TRACTION_FLOOR_G).count();
        assert!(under_floor >= 3, "only {under_floor} places refuse wheels; the rule is idle");
    }

    #[test]
    fn a_car_cannot_drive_where_it_has_no_weight() {
        for d in ROUTE.iter().filter(|d| d.gravity < TRACTION_FLOOR_G) {
            let v = assess(d, Mode::Wheeled, Intake::None);
            assert_eq!(v.fitness, Fitness::Unusable, "{} allowed wheels at {} g", d.id, d.gravity);
            assert!(v.deciding().is_some(), "{} refused wheels without saying why", d.id);
        }
    }

    #[test]
    fn a_passive_skirt_needs_something_to_sit_on_but_thruster_lift_does_not() {
        // The distinction the first draft of this module got wrong: it refused
        // ALL hover in vacuum, which would have grounded every reaction-lift
        // design in the fleet on every airless world.
        let glaciem = dest("glaciem");
        assert!(!has_working_fluid(glaciem.sky), "ice glare is haze in near-vacuum, not air");
        assert_eq!(
            assess(glaciem, Mode::Hover, Intake::None).fitness,
            Fitness::Unusable,
            "a passive skirt should have nothing to press against here"
        );
        assert_eq!(
            assess(glaciem, Mode::Hover, Intake::Airbreathing).fitness,
            Fitness::Unusable,
            "a lift fan still needs air"
        );
        assert!(
            assess(glaciem, Mode::Hover, Intake::Reaction).usable(),
            "thruster lift pushes against its own mass and should not care about the sky"
        );
    }

    #[test]
    fn thruster_lift_pays_for_weight_rather_than_for_air() {
        let heavy = ROUTE
            .iter()
            .filter(|d| matches!(d.terrain, Terrain::Surface) && d.gravity > HEAVY_LIFT_G)
            .max_by(|a, b| a.gravity.partial_cmp(&b.gravity).unwrap());
        let Some(heavy) = heavy else { return };
        let v = assess(heavy, Mode::Hover, Intake::Reaction);
        assert_eq!(v.fitness, Fitness::Degraded, "{} at {} g", heavy.id, heavy.gravity);
        assert!(v.deciding().unwrap().because.contains("burns"));
    }

    #[test]
    fn a_turbine_is_ballast_in_vacuum_and_a_rocket_is_not() {
        let airless = ROUTE
            .iter()
            .find(|d| !has_working_fluid(d.sky) && matches!(d.terrain, Terrain::Surface))
            .expect("the route has an airless surface");
        assert_eq!(assess(airless, Mode::Flight, Intake::Airbreathing).fitness, Fitness::Unusable);
        assert!(
            assess(airless, Mode::Flight, Intake::Reaction).usable(),
            "{} refused a reaction drive, which needs nothing from the sky",
            airless.id
        );
    }

    #[test]
    fn open_space_refuses_everything_that_touches_the_ground() {
        for d in ROUTE.iter().filter(|d| matches!(d.terrain, Terrain::DeepSpace | Terrain::AsteroidField))
        {
            assert_eq!(assess(d, Mode::Wheeled, Intake::None).fitness, Fitness::Unusable);
            assert_eq!(assess(d, Mode::Hover, Intake::None).fitness, Fitness::Unusable);
            assert!(
                assess(d, Mode::Flight, Intake::Reaction).usable(),
                "{} left nothing at all that can move",
                d.id
            );
        }
    }

    #[test]
    fn nowhere_on_the_route_is_completely_impassable() {
        // The design promise: every place admits SOMETHING. A destination that
        // refuses all three modes for both intakes is a place the player
        // cannot reach, which is a content bug rather than a hard world.
        for d in ROUTE.iter() {
            let any = [Intake::None, Intake::Reaction, Intake::Airbreathing]
                .into_iter()
                .any(|i| best_mode(d, i).is_some());
            assert!(any, "{} admits no way of getting around at all", d.id);
        }
    }

    #[test]
    fn the_verdict_names_the_rule_that_decided_it() {
        let d = ROUTE
            .iter()
            .find(|d| d.gravity < TRACTION_FLOOR_G && matches!(d.terrain, Terrain::Surface));
        if let Some(d) = d {
            let v = assess(d, Mode::Wheeled, Intake::None);
            let r = v.deciding().expect("no deciding reason");
            assert_eq!(r.fitness, Fitness::Unusable);
            assert!(r.because.contains("weight"), "unexpected reason: {}", r.because);
        }
    }
}
