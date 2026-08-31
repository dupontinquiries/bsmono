//! shipgrav — zero-g, spin gravity, and the console that chooses.
//!
//! NOT `gravity`, which is the world's attraction-to-a-body maths. This
//! is the SHIP's own gravity: what the crew stands on, produced by
//! rotating a deployed tether. The two never interact — one is a field
//! you fall into, the other is a floor you make.
//!
//! ## Why a transformer can do this and a hull cannot
//!
//! Artificial gravity that is not hand-waved has to come from
//! ACCELERATION, and a ship that is not under thrust has one honest
//! option: spin. A rigid hull spinning about its long axis gives almost
//! nothing at the spine and a useless amount at the skin, because the
//! radius is the whole equation.
//!
//! A TRANSFORMER can deploy a tether — swing mass out on a line, rotate
//! the pair about their common centre, and the radius becomes whatever
//! the tether is long enough to make. That is the one design here that
//! does not require inventing physics, and it is available precisely
//! because this ship already folds into other shapes.
//!
//! So the console offers three states and the middle one is the real
//! feature.
//!
//! ## Scale, and the two sets of consoles
//!
//! `aboard::HULL_HEIGHTS` already makes a bot's ship 1.4 km and a
//! person's 280 m — the same rooms at two scales. A bridge on a bot's
//! ship therefore has controls a person cannot reach, which is not a
//! bug to fix but a fact to furnish: the primary console is sized for
//! the hull's own occupant, and a human-scale repeater sits beside it.
//!
//! A player of either size can fly the ship. Neither is a passenger.

use glam::Vec3;

/// Earth surface gravity, the unit everything here is quoted in.
pub const G: f32 = 9.81;

/// What the ring is doing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    /// Nothing deployed. Free fall — the default in space, and the
    /// state the ship must be in to manoeuvre.
    #[default]
    Zero,
    /// Tether out, pair rotating. Gravity is real: it is centripetal
    /// acceleration, and everything about it follows from radius and
    /// rate.
    Spin,
    /// Tether retracting or extending. NOT a third setting — a
    /// transition, held as its own state because a tether that is
    /// halfway out is neither safe to thrust with nor giving useful
    /// gravity, and a UI that showed only the endpoints would let
    /// someone burn the engines mid-deploy.
    Deploying,
}

/// The rotating tether.
#[derive(Clone, Copy, Debug)]
pub struct Tether {
    /// Metres from the rotation centre to the habitat end.
    pub radius: f32,
    /// Radians per second.
    pub rate: f32,
    /// 0..1, how far the line is out. Gravity only counts at 1.0.
    pub extended: f32,
    pub mode: Mode,
}

/// Below this the rotation is felt as motion rather than weight.
///
/// About 2 rpm. The Coriolis literature puts comfort somewhere under
/// 2–3 rpm for an unadapted person; above it, turning your head makes
/// the room appear to tilt. This is the reason a tether exists at all
/// — a short radius can only reach useful gravity by spinning fast
/// enough to make people sick.
pub const COMFORT_RATE: f32 = 0.21;

impl Tether {
    /// A tether sized for a hull.
    ///
    /// Length scales with the ship because a bot's 1.4 km hull can pay
    /// out a longer line than a person's 280 m one, and a longer line
    /// is a gentler spin for the same gravity.
    pub fn for_hull(hull_length: f32) -> Tether {
        Tether {
            // Half the hull again: long enough to matter, short enough
            // that the deployed pair is not a manoeuvring hazard the
            // length of a city.
            radius: (hull_length * 0.5).max(8.0),
            rate: 0.0,
            extended: 0.0,
            mode: Mode::Zero,
        }
    }

    /// Felt gravity at the habitat end, in g.
    ///
    /// `ω²r`, and zero unless the tether is fully out — a partly
    /// deployed line is swinging, not spinning, and reporting a
    /// fraction of gravity for it would invite someone to stand up
    /// during a transition.
    pub fn gravity_g(&self) -> f32 {
        if self.mode != Mode::Spin || self.extended < 1.0 {
            return 0.0;
        }
        (self.rate * self.rate * self.radius) / G
    }

    /// The rate needed for a target gravity at this radius.
    ///
    /// The console solves this rather than asking for a rate: nobody
    /// wants a ship in radians per second, and the number that matters
    /// is how heavy the floor feels.
    pub fn rate_for(&self, target_g: f32) -> f32 {
        ((target_g.max(0.0) * G) / self.radius.max(0.01)).sqrt()
    }

    /// Would this target be comfortable, or does it need a longer line?
    ///
    /// Reported rather than enforced. A crew that wants a full g out of
    /// a short tether can have it and feel awful; refusing would be the
    /// ship overruling the person flying it, and the number is right
    /// there to argue with.
    pub fn comfortable(&self, target_g: f32) -> bool {
        self.rate_for(target_g) <= COMFORT_RATE
    }

    /// The radius a comfortable spin at this gravity would need.
    pub fn radius_for_comfort(target_g: f32) -> f32 {
        (target_g.max(0.0) * G) / (COMFORT_RATE * COMFORT_RATE)
    }

    /// Advance the deploy/retract animation.
    ///
    /// `secs_full` is how long a full extension takes. Returns true when
    /// the state changed, so a caller can announce it once rather than
    /// every frame.
    pub fn tick(&mut self, dt: f32, secs_full: f32) -> bool {
        let step = dt / secs_full.max(0.1);
        match self.mode {
            Mode::Deploying if self.extended < 1.0 => {
                self.extended = (self.extended + step).min(1.0);
                if self.extended >= 1.0 {
                    self.mode = Mode::Spin;
                    return true;
                }
                false
            }
            // Retracting is the same animation backwards, and it is
            // entered by asking for Zero while extended. Spin STOPS
            // first: reeling in a rotating tether would spin the pair
            // up as the radius shrinks, which is the one failure mode
            // that turns a comfort feature into a centrifuge.
            Mode::Zero if self.extended > 0.0 => {
                self.rate = 0.0;
                self.extended = (self.extended - step).max(0.0);
                self.extended <= 0.0
            }
            _ => false,
        }
    }

    /// May the ship burn its engines?
    ///
    /// Only stowed. A tether under thrust is a line with a mass on the
    /// end being accelerated sideways, and this is the check that keeps
    /// "transform and fly" from being available at the exact moment it
    /// would tear the ship apart.
    pub fn safe_to_thrust(&self) -> bool {
        self.extended <= 0.0
    }
}

/// Who a console is built for.
///
/// Not cosmetic: `HULL_HEIGHTS` makes a bot's ship five times a
/// person's, so a bridge on a bot's ship has controls a human cannot
/// reach. The repeater is how a human crews a bot's ship without the
/// ship being rebuilt for them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scale {
    /// Sized for whatever the hull was built for.
    Primary,
    /// A human-sized repeater beside it, on a bot-scale hull.
    Human,
}

/// A gravity console: where it is, and who can use it.
#[derive(Clone, Copy, Debug)]
pub struct Console {
    pub at: Vec3,
    pub scale: Scale,
    /// Reach height in metres — what a body of that size can operate.
    pub height: f32,
}

/// Human standing height, for the repeater.
pub const HUMAN_H: f32 = 1.8;

/// Console placement for a room, given the hull's own occupant height.
///
/// Returns the primary always, and a human repeater ONLY when the hull's
/// occupant is big enough that a person could not use the primary. A
/// repeater on a human ship is furniture nobody needs, and a room full
/// of duplicate controls is harder to read than one with a single set.
pub fn consoles(centre: Vec3, half: Vec3, occupant_h: f32) -> Vec<Console> {
    let mut out = vec![Console {
        // Against the forward wall, at the occupant's own working
        // height rather than the room's centre — a console at the
        // midpoint of a room is a table, not a station.
        at: centre + Vec3::new(0.0, occupant_h * 0.5, -half.z * 0.8),
        scale: Scale::Primary,
        height: occupant_h,
    }];
    // Twice human height is the threshold: below it a person can still
    // reach a console built for the occupant, and a second set would be
    // clutter.
    if occupant_h > HUMAN_H * 2.0 {
        out.push(Console {
            // Beside it, offset by the primary's own width so the two
            // do not intersect on a big hull.
            at: centre + Vec3::new(occupant_h * 0.6, HUMAN_H * 0.5, -half.z * 0.8),
            scale: Scale::Human,
            height: HUMAN_H,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stowed_tether_has_no_gravity_and_a_deploying_one_has_none_either() {
        // A partly deployed line is SWINGING, not spinning. Reporting a
        // fraction of gravity for it would invite somebody to stand up
        // during a transition.
        let mut t = Tether::for_hull(280.0);
        assert_eq!(t.gravity_g(), 0.0);
        t.mode = Mode::Deploying;
        t.extended = 0.6;
        assert_eq!(t.gravity_g(), 0.0);
    }

    #[test]
    fn gravity_is_omega_squared_r() {
        // Not a curve fit: the console's whole claim to being believable
        // is that this is the real equation.
        let mut t = Tether::for_hull(280.0);
        t.mode = Mode::Spin;
        t.extended = 1.0;
        t.rate = t.rate_for(1.0);
        assert!((t.gravity_g() - 1.0).abs() < 0.001, "{}", t.gravity_g());
    }

    #[test]
    fn a_longer_tether_reaches_the_same_gravity_more_slowly() {
        // The entire reason a transformer deploys a line instead of
        // spinning its hull.
        let short = Tether::for_hull(40.0);
        let long = Tether::for_hull(1400.0);
        assert!(long.rate_for(1.0) < short.rate_for(1.0));
    }

    #[test]
    fn comfort_is_reported_not_enforced() {
        // A crew that wants a full g out of a short tether can have it
        // and feel awful. Refusing would be the ship overruling the
        // person flying it.
        let short = Tether::for_hull(20.0);
        assert!(!short.comfortable(1.0));
        // And the console can say what would fix it.
        assert!(Tether::radius_for_comfort(1.0) > short.radius);
    }

    #[test]
    fn retracting_stops_the_spin_before_reeling_in() {
        // Reeling in a rotating tether spins the pair UP as the radius
        // shrinks — conservation of angular momentum turns a comfort
        // feature into a centrifuge.
        let mut t = Tether::for_hull(280.0);
        t.mode = Mode::Spin;
        t.extended = 1.0;
        t.rate = t.rate_for(1.0);
        t.mode = Mode::Zero;
        t.tick(0.1, 10.0);
        assert_eq!(t.rate, 0.0, "the spin should stop before the line comes in");
        assert!(t.extended < 1.0);
    }

    #[test]
    fn thrust_is_refused_until_the_tether_is_stowed() {
        // A tether under thrust is a mass on a line being accelerated
        // sideways. This is the check that stops "transform and fly"
        // being available at the moment it would tear the ship apart.
        let mut t = Tether::for_hull(280.0);
        assert!(t.safe_to_thrust());
        t.extended = 0.01;
        assert!(!t.safe_to_thrust());
    }

    #[test]
    fn deploying_reports_its_completion_once() {
        // So a caller can announce it rather than saying "gravity on"
        // every frame for as long as it stays on.
        let mut t = Tether::for_hull(280.0);
        t.mode = Mode::Deploying;
        let mut changes = 0;
        for _ in 0..40 {
            if t.tick(0.5, 10.0) { changes += 1; }
        }
        assert_eq!(changes, 1, "state change should fire exactly once");
        assert_eq!(t.mode, Mode::Spin);
    }

    #[test]
    fn a_bot_hull_gets_a_human_repeater_and_a_human_hull_does_not() {
        // HULL_HEIGHTS makes a bot's ship five times a person's, so a
        // bot bridge has controls a human cannot reach. A repeater on a
        // human ship is furniture nobody needs.
        let bot = consoles(Vec3::ZERO, Vec3::splat(20.0), 9.0);
        let human = consoles(Vec3::ZERO, Vec3::splat(4.0), 1.8);
        assert_eq!(bot.len(), 2);
        assert!(bot.iter().any(|c| c.scale == Scale::Human));
        assert_eq!(human.len(), 1);
        assert_eq!(human[0].scale, Scale::Primary);
    }

    #[test]
    fn the_two_consoles_do_not_occupy_the_same_spot() {
        // They are both against the forward wall; a shared position
        // would render them inside one another.
        let c = consoles(Vec3::ZERO, Vec3::splat(20.0), 9.0);
        assert!((c[0].at - c[1].at).length() > 1.0, "{:?}", c);
    }
}
