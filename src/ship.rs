//! THE SHIP — boarding, plotting, leaving, arriving.
//!
//! `hub_plan.h` had more in it than the room graph already ported: each
//! destination carries a TRAVEL MODE — drive, fly, land or walk — which is
//! the engine saying that arriving somewhere is not one action. You drive
//! onto Rust Flats. You fly into Ember Descent because the thermals off the
//! lava pools will carry you. You *land* on the Frost Shelf, light, because
//! ice shelves calve under weight. That distinction is the whole reason this
//! module exists rather than a loading screen.
//!
//! Ships are big and they transform, same as everything else here. The data
//! makes that explicit: `widesky_aka_3km_space_carrier` is three kilometres
//! long, and `reference_ship_parts_for_transformation` is a modular kit for
//! exactly this. So `Hull` carries a length in metres and the phase machine
//! scales its own timings by it — a corvette snaps into landing configuration
//! and a carrier takes most of a minute, because a body that size cannot do
//! anything quickly and should not look like it can.
//!
//! The phases are deliberately a line and not a graph. Board, lift, plot,
//! cruise, descend, arrive: every one of those is a place the player is
//! standing, and a state machine that can jump between them is a menu
//! wearing a ship costume.

use glam::Vec3;

use crate::bigbody::{BigBody, Drive, Shape};
use crate::menu::{Fmt, Item, Menu, Row};
use crate::patrol;

/// How you get onto the ground at the far end. Straight from `hub_plan.h`'s
/// `travel` column: 0 drive, 1 fly, 2 land, 3 walk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arrival {
    /// wheels down outside, drive in
    Drive,
    /// launch from the hangar and fly in under your own power
    Fly,
    /// the ship itself sets down — the only one where its mass matters
    Land,
    /// dock and walk out
    Walk,
}

impl Arrival {
    pub fn label(self) -> &'static str {
        match self {
            Arrival::Drive => "DRIVE IN",
            Arrival::Fly => "FLY IN",
            Arrival::Land => "SET DOWN",
            Arrival::Walk => "DOCK",
        }
    }
    /// Does the ship come down the well, or stay in orbit and drop you?
    pub fn ship_descends(self) -> bool {
        matches!(self, Arrival::Land | Arrival::Walk)
    }
}

/// Hull class. Length drives everything slow about a ship.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Lander,
    Corvette,
    Frigate,
    Carrier,
}

pub struct Hull {
    pub id: &'static str,
    pub name: &'static str,
    pub class: Class,
    /// metres, bow to stern
    pub length: f32,
    /// can it transform into a landing or combat configuration?
    pub transforms: bool,
    /// how many bots it can carry down
    pub berths: u32,
}

/// Drawn from the imported ship models. `widesky` really is a 3 km carrier
/// and it really should feel like one.
pub const HULLS: &[Hull] = &[
    Hull { id: "afterburn", name: "AFTERBURN", class: Class::Lander,   length: 42.0,   transforms: true,  berths: 4 },
    Hull { id: "hawkdrop",  name: "HAWKDROP",  class: Class::Lander,   length: 28.0,   transforms: true,  berths: 2 },
    Hull { id: "spearhead", name: "SPEARHEAD", class: Class::Corvette, length: 90.0,   transforms: true,  berths: 6 },
    Hull { id: "darkwing",  name: "DARKWING",  class: Class::Corvette, length: 110.0,  transforms: true,  berths: 6 },
    Hull { id: "briskfall", name: "BRISKFALL", class: Class::Corvette, length: 74.0,   transforms: false, berths: 4 },
    Hull { id: "warpzone",  name: "WARPZONE",  class: Class::Frigate,  length: 260.0,  transforms: false, berths: 20 },
    Hull { id: "praetor",   name: "PRAETOR",   class: Class::Frigate,  length: 340.0,  transforms: true,  berths: 24 },
    Hull { id: "nemesis",   name: "NEMESIS",   class: Class::Frigate,  length: 480.0,  transforms: true,  berths: 40 },
    Hull { id: "ark",       name: "THE ARK",   class: Class::Carrier,  length: 1600.0, transforms: true,  berths: 90 },
    Hull { id: "widesky",   name: "WIDESKY",   class: Class::Carrier,  length: 3000.0, transforms: false, berths: 140 },
];

pub fn hull(id: &str) -> Option<&'static Hull> {
    HULLS.iter().find(|h| h.id == id)
}

impl Hull {
    /// Seconds to fold between cruise and landing configuration.
    ///
    /// Scales with length, because a three-kilometre hull moving its own
    /// structure is not a bot doing a somersault. A lander takes about four
    /// seconds; the Ark takes the better part of a minute.
    pub fn fold_sec(&self) -> f32 {
        if !self.transforms {
            return 0.0;
        }
        (3.0 + self.length * 0.017).min(70.0)
    }
    /// Seconds to climb out of a well of this gravity.
    pub fn ascent_sec(&self, g: f32) -> f32 {
        (6.0 + g * 2.4 + self.length * 0.006).min(90.0)
    }
    /// Can it physically put down, or must it stay up and drop you?
    ///
    /// A 3 km carrier does not land on an ice shelf. Nothing over 600 m
    /// comes down a well at all — it holds orbit and the hangar does the
    /// work, which is what the hangar is for.
    pub fn can_land(&self) -> bool {
        self.length <= 600.0
    }
}

/// Where the player is in the journey. A line, not a graph.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// on the ground, ship open
    Docked,
    /// walking aboard; the hangar door is the transition
    Boarding,
    /// folding to launch configuration
    Folding,
    /// climbing out of the well
    Ascent,
    /// in orbit with the navigation console live — this is where plotting
    /// happens, and it is a PLACE rather than a menu
    Plotting,
    /// hit hard enough that nothing is flying it any more. Not a cutscene:
    /// the hull is now a falling BigBody and the world's own physics has it.
    Crashing,
    /// down, in one piece or several. The hull is terrain from here.
    Wrecked,
    /// under way
    Cruise,
    /// coming down, or holding while the hangar drops you
    Descent,
    /// boots on the far ground
    Arrived,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Docked => "DOCKED",
            Phase::Boarding => "BOARDING",
            Phase::Folding => "RECONFIGURING",
            Phase::Ascent => "ASCENT",
            Phase::Plotting => "PLOTTING",
            Phase::Cruise => "CRUISE",
            Phase::Descent => "DESCENT",
            Phase::Crashing => "GOING DOWN",
            Phase::Wrecked => "WRECKED",
            Phase::Arrived => "ARRIVED",
        }
    }
    /// Can the player walk around the ship interior right now?
    pub fn interior(self) -> bool {
        matches!(self, Phase::Docked | Phase::Boarding | Phase::Plotting | Phase::Cruise)
    }
}

/// A plotted course.
#[derive(Clone, Debug)]
pub struct Course {
    pub to: &'static str,
    pub arrival: Arrival,
    /// hours of cruise; the bridge shows it and the player waits or skips
    pub hours: f32,
    /// 0..1 of tank
    pub fuel: f32,
    /// true when the route crosses the shield boundary, which is a decision
    /// rather than a distance
    pub crosses_shield: bool,
}

/// Straight-line-ish cost between two destinations, in arbitrary units the
/// bridge turns into hours. Ordering along ROUTE stands in for distance:
/// the route is laid out inner to outer, so index difference is a decent
/// proxy and it costs nothing to compute.
pub fn leg_cost(from: &str, to: &str) -> f32 {
    let i = patrol::ROUTE.iter().position(|d| d.id == from);
    let j = patrol::ROUTE.iter().position(|d| d.id == to);
    match (i, j) {
        (Some(a), Some(b)) => ((a as i32 - b as i32).abs() as f32).max(1.0),
        _ => 6.0,
    }
}

pub struct Ship {
    pub hull: &'static Hull,
    pub phase: Phase,
    /// where the ship physically is
    pub at: &'static str,
    pub course: Option<Course>,
    /// seconds left in the current phase, when it is timed
    timer: f32,
    /// 0..1
    pub fuel: f32,
    pub crew: u32,
    /// the hull as a collider — solid to stand in, and an entity that can be
    /// shot out of the sky
    pub body: BigBody,
}

impl Ship {
    pub fn new(hull_id: &str, at: &'static str) -> Ship {
        let h = hull(hull_id).unwrap_or(&HULLS[0]);
        // a crude hull box scaled off the real length, with a hangar bay
        // carved aft — the bay is what makes it a place rather than a rock
        let half = Vec3::new(h.length * 0.16, h.length * 0.09, h.length * 0.5);
        let shape = Shape {
            half,
            round: (h.length * 0.02).max(0.5),
            bay: Some((
                Vec3::new(0.0, -half.y * 0.35, half.z * 0.45),
                Vec3::new(half.x * 0.5, half.y * 0.45, half.z * 0.22),
            )),
        };
        Ship {
            body: BigBody::new(shape, Vec3::ZERO),
            hull: h,
            phase: Phase::Docked,
            at,
            course: None,
            timer: 0.0,
            fuel: 1.0,
            crew: 1,
        }
    }

    /// TAKE FIRE. Past its hit points the flight script stops and gravity
    /// takes over — which is the whole design: crashing is not a special
    /// mode, it is the absence of one.
    pub fn hit(&mut self, amount: f32, at: Vec3) {
        if matches!(self.phase, Phase::Wrecked | Phase::Docked) {
            return;
        }
        self.body.damage(amount, at);
        if self.body.drive == Drive::Falling && self.phase != Phase::Crashing {
            self.phase = Phase::Crashing;
            self.timer = 0.0;
            self.course = None;
        }
    }

    /// True while anything aboard should be under ordinary world physics
    /// rather than being carried by a script.
    pub fn uncontrolled(&self) -> bool {
        matches!(self.phase, Phase::Crashing)
    }

    pub fn here(&self) -> Option<&'static patrol::Destination> {
        patrol::find(self.at)
    }

    /// Plot a course. Refuses rather than silently failing, because a
    /// navigation console that does nothing when you press it is worse than
    /// one that says why.
    pub fn plot(&mut self, to: &'static str, arrival: Arrival) -> Result<&Course, &'static str> {
        if self.phase != Phase::Plotting && self.phase != Phase::Docked {
            return Err("not at the console");
        }
        let dest = patrol::find(to).ok_or("no such destination")?;
        if to == self.at {
            return Err("already there");
        }
        // a ship that cannot land cannot promise to
        if arrival.ship_descends() && !self.hull.can_land() {
            return Err("hull too large to set down — drop from orbit instead");
        }
        // and nothing sets down where there is no ground
        if arrival.ship_descends() && dest.terrain == patrol::Terrain::DeepSpace {
            return Err("nothing to land on");
        }
        let cost = leg_cost(self.at, to);
        let fuel = (cost * 0.08 * (self.hull.length / 200.0).max(0.4)).min(1.0);
        if fuel > self.fuel {
            return Err("not enough fuel for that leg");
        }
        let crosses = (self.at == "shield_edge") != (to == "shield_edge")
            || to == "outer_rim"
            || self.at == "outer_rim";
        self.course = Some(Course {
            to,
            arrival,
            hours: cost * 1.7,
            fuel,
            crosses_shield: crosses,
        });
        Ok(self.course.as_ref().unwrap())
    }

    /// Walk aboard. The hangar door is the transition, not a fade.
    pub fn board(&mut self) -> bool {
        if self.phase != Phase::Docked {
            return false;
        }
        self.phase = Phase::Boarding;
        self.timer = 4.0;
        true
    }

    /// Commit to the plotted course.
    pub fn launch(&mut self) -> Result<(), &'static str> {
        if self.course.is_none() {
            return Err("no course plotted");
        }
        if !matches!(self.phase, Phase::Plotting | Phase::Docked) {
            return Err("already under way");
        }
        let g = self.here().map(|d| d.gravity).unwrap_or(1.0);
        if self.hull.transforms {
            self.phase = Phase::Folding;
            self.timer = self.hull.fold_sec();
        } else {
            self.phase = Phase::Ascent;
            self.timer = self.hull.ascent_sec(g);
        }
        Ok(())
    }

    /// Seconds remaining in whatever is currently happening.
    pub fn remaining(&self) -> f32 {
        self.timer
    }

    pub fn update(&mut self, dt: f32) -> Option<Phase> {
        if self.phase == Phase::Crashing {
            // nothing scripted left: the body is falling and the ground it
            // meets is the world's, same as for any car
            if self.body.drive == Drive::Wrecked {
                self.phase = Phase::Wrecked;
                return Some(Phase::Wrecked);
            }
            return None;
        }
        if self.timer <= 0.0 {
            return None;
        }
        self.timer -= dt;
        if self.timer > 0.0 {
            return None;
        }
        self.timer = 0.0;
        let g = self.here().map(|d| d.gravity).unwrap_or(1.0);
        self.phase = match self.phase {
            Phase::Boarding => Phase::Docked, // aboard, ship still down
            Phase::Folding => {
                self.timer = self.hull.ascent_sec(g);
                Phase::Ascent
            }
            Phase::Ascent => Phase::Plotting,
            Phase::Cruise => {
                // arrive: the ship is now there whether or not you are down
                if let Some(c) = self.course.clone() {
                    self.at = c.to;
                    self.fuel = (self.fuel - c.fuel).max(0.0);
                    self.timer = 8.0 + self.hull.length * 0.01;
                    Phase::Descent
                } else {
                    Phase::Plotting
                }
            }
            Phase::Descent => Phase::Arrived,
            p => p,
        };
        Some(self.phase)
    }

    /// Leave orbit. Separate from `launch` so the player can stand on the
    /// bridge with a course plotted and not take it yet — which is where
    /// half the fiction of a ship lives.
    pub fn engage(&mut self) -> Result<(), &'static str> {
        if self.phase != Phase::Plotting {
            return Err("not in orbit");
        }
        let c = self.course.as_ref().ok_or("no course plotted")?;
        self.timer = c.hours * 0.8;
        self.phase = Phase::Cruise;
        Ok(())
    }

    /// The bridge, as a menu.
    ///
    /// Built from `menu.rs` rather than hand-rolled, which is what that
    /// abstraction was for: a destination list is rows, and the console is
    /// the same navigation code as every other screen. Unreachable legs stay
    /// VISIBLE and disabled with the reason in the hint, because a course
    /// you cannot take is information.
    pub fn bridge_menu(&self) -> Menu {
        let mut rows = vec![Row::heading("NAVIGATION")];
        for d in patrol::ROUTE {
            if d.id == self.at {
                continue;
            }
            let cost = leg_cost(self.at, d.id);
            let fuel = (cost * 0.08 * (self.hull.length / 200.0).max(0.4)).min(1.0);
            let mut r = Row::action(d.id, d.name);
            if fuel > self.fuel {
                r = r.disabled().hint("not enough fuel for that leg");
            } else if d.terrain == patrol::Terrain::DeepSpace && !self.hull.can_land() {
                r = r.hint("drop from orbit; this hull does not set down");
            } else {
                r = r.hint(d.note);
            }
            rows.push(r);
        }
        rows.push(Row::gap());
        rows.push(Row::heading("SHIP"));
        rows.push(Row::slider("fuel", "FUEL", self.fuel, 0.0, 1.0, 0.01, Fmt::Percent).disabled());
        rows.push(Row::choice(
            "arrival",
            "ARRIVAL",
            0,
            &["SET DOWN", "DROP: FLY IN", "DROP: DRIVE IN", "DOCK"],
        ));
        if self.course.is_some() {
            rows.push(Row::action("engage", "ENGAGE").hint("leave orbit on the plotted course"));
        }
        let mut m = Menu::new(&format!("BRIDGE — {}", self.hull.name), rows);
        m.view = 10;
        m
    }
}

/// Turn the arrival choice index back into an Arrival.
pub fn arrival_from_index(i: usize) -> Arrival {
    match i {
        1 => Arrival::Fly,
        2 => Arrival::Drive,
        3 => Arrival::Walk,
        _ => Arrival::Land,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_decides_what_a_ship_can_do() {
        let lander = hull("hawkdrop").unwrap();
        let carrier = hull("widesky").unwrap();
        assert!(lander.can_land() && !carrier.can_land());
        // and a big hull that transforms takes far longer about it
        let ark = hull("ark").unwrap();
        assert!(ark.fold_sec() > lander.fold_sec() * 4.0, "{} vs {}", ark.fold_sec(), lander.fold_sec());
        // a hull that does not transform folds in no time, because it does not
        assert_eq!(hull("briskfall").unwrap().fold_sec(), 0.0);
        // nothing takes more than a minute and ten
        for h in HULLS {
            assert!(h.fold_sec() <= 70.0 && h.ascent_sec(9.81) <= 90.0, "{}", h.id);
        }
    }

    #[test]
    fn a_carrier_cannot_promise_to_set_down() {
        let mut s = Ship::new("widesky", "earth");
        assert!(s.plot("mars", Arrival::Land).is_err());
        // but it can hold orbit and drop you
        assert!(s.plot("mars", Arrival::Fly).is_ok());
    }

    #[test]
    fn nothing_lands_on_open_space() {
        let mut s = Ship::new("hawkdrop", "earth");
        assert!(s.plot("outer_rim", Arrival::Land).is_err());
        assert!(s.plot("outer_rim", Arrival::Fly).is_ok());
    }

    #[test]
    fn plotting_refuses_out_loud_rather_than_doing_nothing() {
        let mut s = Ship::new("hawkdrop", "earth");
        assert_eq!(s.plot("earth", Arrival::Land).unwrap_err(), "already there");
        assert!(s.plot("nowhere", Arrival::Fly).is_err());
        s.fuel = 0.0;
        assert_eq!(
            s.plot("cybertron", Arrival::Fly).unwrap_err(),
            "not enough fuel for that leg"
        );
    }

    #[test]
    fn the_journey_runs_in_order_and_ends_somewhere_else() {
        let mut s = Ship::new("spearhead", "earth");
        s.plot("mars", Arrival::Land).unwrap();
        s.launch().unwrap();
        // a transforming hull folds BEFORE it climbs
        assert_eq!(s.phase, Phase::Folding);
        let mut seen = vec![s.phase];
        for _ in 0..20000 {
            if let Some(p) = s.update(1.0 / 60.0) {
                seen.push(p);
                if p == Phase::Plotting && s.course.is_some() {
                    s.engage().unwrap();
                    // engage sets Cruise directly, so update never returns
                    // it — record it here or the trace has a hole in it
                    seen.push(s.phase);
                }
                if p == Phase::Arrived {
                    break;
                }
            }
        }
        assert_eq!(*seen.last().unwrap(), Phase::Arrived, "never arrived: {seen:?}");
        assert!(seen.contains(&Phase::Ascent) && seen.contains(&Phase::Cruise));
        assert_eq!(s.at, "mars", "the ship did not move");
        assert!(s.fuel < 1.0, "the trip was free");
    }

    #[test]
    fn you_can_stand_on_the_bridge_with_a_course_and_not_take_it() {
        let mut s = Ship::new("spearhead", "earth");
        s.phase = Phase::Plotting;
        s.plot("mars", Arrival::Land).unwrap();
        // plotting alone must not move the ship
        for _ in 0..600 {
            s.update(1.0 / 60.0);
        }
        assert_eq!(s.at, "earth");
        assert_eq!(s.phase, Phase::Plotting);
        s.engage().unwrap();
        assert_eq!(s.phase, Phase::Cruise);
    }

    #[test]
    fn the_interior_is_walkable_only_when_it_makes_sense() {
        assert!(Phase::Plotting.interior() && Phase::Cruise.interior());
        // nobody is strolling the corridors mid-reconfiguration or on the
        // way down
        assert!(!Phase::Folding.interior());
        assert!(!Phase::Ascent.interior());
        assert!(!Phase::Descent.interior());
    }

    #[test]
    fn the_bridge_menu_shows_unreachable_legs_instead_of_hiding_them() {
        let mut s = Ship::new("hawkdrop", "earth");
        s.fuel = 0.05;
        let m = s.bridge_menu();
        let disabled = m.rows.iter().filter(|r| !r.enabled && r.item == Item::Action).count();
        assert!(disabled > 0, "an empty tank hid every destination");
        // and it never lists where you already are
        assert!(m.rows.iter().all(|r| r.id != "earth"));
        // the disabled ones still say why
        assert!(m.rows.iter().filter(|r| !r.enabled && r.item == Item::Action)
            .all(|r| r.hint.is_some()));
    }

    #[test]
    fn being_shot_down_mid_ascent_hands_you_to_the_world() {
        let mut s = Ship::new("spearhead", "earth");
        s.plot("mars", Arrival::Land).unwrap();
        s.launch().unwrap();
        for _ in 0..600 {
            s.update(1.0 / 60.0);
            if s.phase == Phase::Ascent {
                break;
            }
        }
        assert_eq!(s.phase, Phase::Ascent);
        assert!(!s.uncontrolled());

        // hit it off the centreline
        let flank = s.body.frame.to_world(Vec3::new(s.body.shape.half.x, 0.0, 0.0));
        s.hit(2.0, flank);
        assert_eq!(s.phase, Phase::Crashing);
        assert!(s.uncontrolled(), "still being flown after being shot down");
        assert!(s.course.is_none(), "it is still holding a course while falling");
        assert_eq!(s.body.drive, crate::bigbody::Drive::Falling);

        // the timer no longer advances anything: only the world does
        for _ in 0..600 {
            s.update(1.0 / 60.0);
        }
        assert_eq!(s.phase, Phase::Crashing, "a script finished the crash for us");

        // now run it as a falling body against real ground
        let g = Vec3::new(0.0, -9.81, 0.0);
        s.body.frame.pos.y = 400.0;
        for _ in 0..20000 {
            s.body.step(1.0 / 60.0, g, &|p: Vec3| p.y);
            s.update(1.0 / 60.0);
            if s.phase == Phase::Wrecked {
                break;
            }
        }
        assert_eq!(s.phase, Phase::Wrecked, "never came down");
        assert!(s.body.contains(s.body.frame.pos), "the wreck stopped being solid");
    }

    #[test]
    fn a_docked_ship_cannot_be_shot_out_of_the_sky() {
        let mut s = Ship::new("hawkdrop", "earth");
        s.hit(99.0, Vec3::ZERO);
        assert_eq!(s.phase, Phase::Docked);
    }

    #[test]
    fn a_bigger_hull_is_a_bigger_place() {
        // the collider is scaled off the real length, so a carrier really is
        // somewhere you could walk around
        let l = Ship::new("hawkdrop", "earth");
        let c = Ship::new("widesky", "earth");
        assert!(c.body.shape.half.z > l.body.shape.half.z * 20.0);
        // and both have a bay you can be inside without being inside the hull
        for s in [&l, &c] {
            let (bc, _) = s.body.shape.bay.unwrap();
            let p = s.body.frame.to_world(bc);
            assert!(s.body.in_bay(p) && !s.body.contains(p));
        }
    }

    #[test]
    fn arrival_modes_come_back_the_way_they_went_in() {
        for (i, a) in [(0, Arrival::Land), (1, Arrival::Fly), (2, Arrival::Drive), (3, Arrival::Walk)] {
            assert_eq!(arrival_from_index(i), a);
        }
        assert!(Arrival::Land.ship_descends() && Arrival::Walk.ship_descends());
        assert!(!Arrival::Fly.ship_descends() && !Arrival::Drive.ship_descends());
    }
}
