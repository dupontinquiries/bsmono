//! HOW A THING STAYS UP.
//!
//! `car.flight` was one flag meaning "never grounded, plus lift and nose
//! thrust", and it was wrong twice over.
//!
//! **It stayed on in robot mode.** A bot that has unfolded onto its legs is
//! standing on the ground; it does not float. The flag belongs to the
//! VEHICLE half of a chassis, not to the chassis, and that is a one-line fix
//! in the transform.
//!
//! **And it treated every flyer the same.** A Harrier and an F-16 and a
//! shuttle do not stay up by the same means, and the difference is not
//! decoration — it decides whether you can stop, and whether you can fly at
//! all where there is no air:
//!
//!   * **Wing.** Lift goes as v² and needs air. Below stall speed there is
//!     no lift at all, so a jet cannot hover and cannot loiter; it commits
//!     to a pass. In vacuum a wing is a decoration.
//!   * **Rotor.** Lift comes from throttle rather than airspeed, so a VTOL
//!     or a helicopter holds station at zero knots. It also needs air, and
//!     it pays for the privilege with a low top speed — a rotor disc that
//!     works standing still is a rotor disc that stops working fast.
//!   * **Thrust.** Lift is throttle alone, air or no air. The only one that
//!     works off-world, which is why the spaceship hulls are in this
//!     category and why they are the only things that can operate at the
//!     Outer Rim — and it burns fuel to do what a wing gets free.
//!
//! Classification is by name and proportion, and it is deliberately
//! conservative: an unrecognised flyer is a Wing, because that is the
//! commonest case and the one whose failure mode (you fell out of the sky
//! going too slowly) is legible rather than baffling.

/// What holds a chassis up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lift {
    /// fixed wing: lift from airspeed
    Wing,
    /// rotor or ducted fan: lift from throttle, still needs atmosphere
    Rotor,
    /// reaction thrust: lift from throttle, works in vacuum
    Thrust,
}

/// Below this airspeed a wing produces nothing.
pub const STALL_MS: f32 = 26.0;
/// Lift coefficient for a wing, per (m/s)² per unit air density.
pub const WING_K: f32 = 0.016;
/// A rotor's lift at full throttle in one atmosphere, in g.
pub const ROTOR_G: f32 = 1.55;
/// A thruster's, which is the same in vacuum.
pub const THRUST_G: f32 = 1.40;
/// Rotors are slow. This is the fraction of a jet's top speed they reach.
pub const ROTOR_TOP_FRAC: f32 = 0.45;

impl Lift {
    pub fn label(self) -> &'static str {
        match self {
            Lift::Wing => "WING",
            Lift::Rotor => "ROTOR",
            Lift::Thrust => "THRUST",
        }
    }

    /// Can it hold station with no forward speed?
    pub fn can_hover(self) -> bool {
        !matches!(self, Lift::Wing)
    }

    /// Does it work where there is no atmosphere?
    pub fn works_in_vacuum(self) -> bool {
        matches!(self, Lift::Thrust)
    }

    /// Airspeed below which there is no lift. Zero for anything that hovers.
    pub fn stall_speed(self) -> f32 {
        match self {
            Lift::Wing => STALL_MS,
            _ => 0.0,
        }
    }

    /// Upward acceleration available, in m/s².
    ///
    /// `air` is atmospheric density relative to Earth's, so 0 is vacuum and
    /// 0.006 is Mars. `g` is the local gravity, used only to express rotor
    /// and thrust output in g — a hull that lifts at 1.4 g lifts at 1.4 g
    /// wherever it is, and whether that beats the local gravity is exactly
    /// the question.
    pub fn lift_accel(self, airspeed: f32, throttle: f32, air: f32, g: f32) -> f32 {
        let t = throttle.clamp(0.0, 1.0);
        match self {
            Lift::Wing => {
                if airspeed < STALL_MS || air <= 1e-4 {
                    0.0
                } else {
                    WING_K * airspeed * airspeed * air
                }
            }
            // a rotor still bites on air; thin air is a real ceiling, which
            // is why helicopters have one
            Lift::Rotor => ROTOR_G * g * t * air.min(1.0),
            Lift::Thrust => THRUST_G * g * t,
        }
    }

    /// Can this chassis stay up here at all?
    ///
    /// The question a HUD should answer before you launch, and the reason
    /// the categories exist: taking a jet to Luna is a category error, not a
    /// difficulty setting.
    pub fn viable(self, air: f32, g: f32) -> bool {
        // full throttle, and for a wing a generous cruise speed
        self.lift_accel(60.0, 1.0, air, g) >= g
    }

    /// Top speed as a fraction of the chassis' nominal maximum.
    pub fn top_speed_frac(self) -> f32 {
        match self {
            Lift::Rotor => ROTOR_TOP_FRAC,
            _ => 1.0,
        }
    }

    /// GUESS FROM THE MODEL. Name first, because the imports are named for
    /// what they are, then proportion.
    ///
    /// `span_ratio` is beam over length: a wing is wide for its length, a
    /// rocket is not. Deliberately conservative — anything unrecognised is a
    /// Wing, because that is the commonest case and its failure mode (you
    /// fell out of the sky going too slowly) teaches you something, where a
    /// hovering brick teaches you nothing.
    pub fn classify(name: &str, span_ratio: f32) -> Lift {
        let n = name.to_ascii_lowercase();
        // rotors and ducted fans announce themselves
        const ROTOR: &[&str] = &[
            "heli", "copter", "rotor", "vtol", "hover", "harrier", "osprey", "drone", "quad",
        ];
        const SPACE: &[&str] = &[
            "spaceship", "orbital", "lander", "shuttle", "rocket", "ufo", "starship", "carrier",
            "freighter", "dropship", "cruiser", "warpzone", "ark",
        ];
        // MATCH WHOLE SEGMENTS FOR SHORT KEYS. A bare substring test read
        // "ark" inside bulwARK and dARKguard and called both spacecraft,
        // which is exactly the kind of thing that would have been blamed on
        // the shape heuristic later. Names here are underscore-delimited, so
        // the segments are the words.
        let seg: Vec<&str> = n.split(|c: char| !c.is_ascii_alphanumeric()).collect();
        let hit = |keys: &[&str]| {
            keys.iter().any(|k| {
                if k.len() <= 4 {
                    seg.iter().any(|w| w == k)
                } else {
                    n.contains(k)
                }
            })
        };
        if hit(ROTOR) {
            return Lift::Rotor;
        }
        if hit(SPACE) {
            return Lift::Thrust;
        }
        // no name to go on: a body with real span glides, a narrow one does
        // not and must be pushing itself
        if span_ratio >= 0.62 {
            Lift::Wing
        } else {
            Lift::Thrust
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EARTH_AIR: f32 = 1.0;
    const G: f32 = 9.81;

    #[test]
    fn a_jet_cannot_hover_and_a_rotor_can() {
        assert!(!Lift::Wing.can_hover());
        assert!(Lift::Rotor.can_hover() && Lift::Thrust.can_hover());
        // at a standstill a wing has nothing
        assert_eq!(Lift::Wing.lift_accel(0.0, 1.0, EARTH_AIR, G), 0.0);
        // and a rotor holds its own weight
        assert!(Lift::Rotor.lift_accel(0.0, 1.0, EARTH_AIR, G) > G);
    }

    #[test]
    fn a_wing_stalls_below_its_stall_speed() {
        let just_under = Lift::Wing.lift_accel(STALL_MS - 0.1, 1.0, EARTH_AIR, G);
        let just_over = Lift::Wing.lift_accel(STALL_MS + 0.1, 1.0, EARTH_AIR, G);
        assert_eq!(just_under, 0.0, "a stalled wing still flew");
        assert!(just_over > 0.0);
        // and lift grows with the SQUARE of speed, not linearly
        let a = Lift::Wing.lift_accel(40.0, 1.0, EARTH_AIR, G);
        let b = Lift::Wing.lift_accel(80.0, 1.0, EARTH_AIR, G);
        assert!((b / a - 4.0).abs() < 0.01, "lift is not quadratic: {a} -> {b}");
    }

    #[test]
    fn only_thrust_works_where_there_is_no_air() {
        for l in [Lift::Wing, Lift::Rotor] {
            assert_eq!(l.lift_accel(120.0, 1.0, 0.0, G), 0.0, "{l:?} flew in vacuum");
            assert!(!l.works_in_vacuum());
        }
        assert!(Lift::Thrust.lift_accel(0.0, 1.0, 0.0, G) > G);
        assert!(Lift::Thrust.works_in_vacuum());
    }

    #[test]
    fn thin_air_is_a_real_ceiling_for_a_rotor() {
        // helicopters have a service ceiling for exactly this reason
        let sea = Lift::Rotor.lift_accel(0.0, 1.0, 1.0, G);
        let thin = Lift::Rotor.lift_accel(0.0, 1.0, 0.25, G);
        assert!(thin < sea * 0.3, "thin air did not cost the rotor anything");
        assert!(thin < G, "a rotor still hovered on a quarter of an atmosphere");
    }

    #[test]
    fn viability_answers_the_question_before_you_launch() {
        // Earth: everything flies
        for l in [Lift::Wing, Lift::Rotor, Lift::Thrust] {
            assert!(l.viable(1.0, G), "{l:?} cannot fly on Earth");
        }
        // Luna: only reaction thrust. Taking a jet there is a category
        // error, not a difficulty setting.
        assert!(!Lift::Wing.viable(0.0, 1.62));
        assert!(!Lift::Rotor.viable(0.0, 1.62));
        assert!(Lift::Thrust.viable(0.0, 1.62));
        // Mars: thin enough that a rotor struggles even at a third of a g
        assert!(!Lift::Rotor.viable(0.006, 3.72));
        assert!(Lift::Thrust.viable(0.006, 3.72));
    }

    #[test]
    fn a_rotor_pays_for_hovering_with_speed() {
        assert!(Lift::Rotor.top_speed_frac() < Lift::Wing.top_speed_frac());
        assert_eq!(Lift::Thrust.top_speed_frac(), 1.0);
    }

    #[test]
    fn the_obvious_models_classify_the_obvious_way() {
        assert_eq!(Lift::classify("imported_flyer_helicopter_ka52", 0.9), Lift::Rotor);
        assert_eq!(Lift::classify("jet_vtol_harrier", 0.7), Lift::Rotor);
        assert_eq!(Lift::classify("imported_spaceship_nemesis", 0.3), Lift::Thrust);
        assert_eq!(Lift::classify("imported_spaceship_afterburn_aka_large_orbital_lander", 0.4), Lift::Thrust);
        assert_eq!(Lift::classify("flyer_c_130j_super_hercules", 1.4), Lift::Wing);
        assert_eq!(Lift::classify("jet_dawnshade", 0.65), Lift::Wing);
    }

    #[test]
    fn a_short_keyword_does_not_match_inside_a_longer_word() {
        // found by printing the shipped roster: "ark" matched bulwARK and
        // dARKguard and made both of them spacecraft
        assert_eq!(Lift::classify("jet_bulwark", 0.8), Lift::Wing);
        assert_eq!(Lift::classify("jet_darkguard", 0.9), Lift::Wing);
        // while the real one still lands
        assert_eq!(Lift::classify("transformers_fall_of_cybertron_ark", 0.3), Lift::Thrust);
        // and a long keyword may still match inside a word, which is what
        // makes "spaceship_afterburn" work
        assert_eq!(Lift::classify("imported_spaceship_albatar", 0.9), Lift::Thrust);
    }

    #[test]
    fn a_name_beats_a_shape_and_shape_beats_nothing() {
        // a helicopter with a wide disc must not be read as a wing
        assert_eq!(Lift::classify("hover_thing", 1.8), Lift::Rotor);
        // an unnamed narrow body is pushing itself
        assert_eq!(Lift::classify("unknown_thing", 0.2), Lift::Thrust);
        // and an unnamed wide one glides — the conservative default
        assert_eq!(Lift::classify("unknown_thing", 1.1), Lift::Wing);
    }

    #[test]
    fn throttle_matters_to_the_hoverers_and_not_to_the_wing() {
        let half = Lift::Rotor.lift_accel(0.0, 0.5, 1.0, G);
        let full = Lift::Rotor.lift_accel(0.0, 1.0, 1.0, G);
        assert!((full / half - 2.0).abs() < 1e-4);
        // a wing does not care what the throttle says, only what the air is
        // doing over it
        let a = Lift::Wing.lift_accel(60.0, 0.1, 1.0, G);
        let b = Lift::Wing.lift_accel(60.0, 1.0, 1.0, G);
        assert_eq!(a, b, "a wing's lift changed with the throttle");
    }
}
