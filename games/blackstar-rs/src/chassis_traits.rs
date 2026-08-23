//! chassis_traits — what a vehicle IS, as a vector rather than a word.
//!
//! ## The problem with the string
//!
//! `fleet.json` sorts 173 chassis into ten categories, and two of them —
//! `hovertank` and `flyerf` — exist only because a chassis was two things at
//! once. That is the failure mode of a string label: the vocabulary grows a new
//! word every time reality declines to be exclusive, and the new word carries
//! no relationship to the old ones. Nothing in `hovertank` tells a physics
//! solver it should behave like a `hoverer` for lift and a `tank` for traction.
//!
//! The game currently decides flight with `bot.starts_with("jet_")` — one
//! boolean, from a filename. A chassis is not one boolean.
//!
//! ## The vector
//!
//! Six independent traits in 0..1. Independent is the point: a hovertank is
//! high on `hover` AND high on `armour`, and no new category is needed to say
//! so. `fleet.json` already ships `grounded: 0.5` for exactly that chassis —
//! a continuous number the string discarded.
//!
//! ## What it is for
//!
//! Not compression. PHYSICS. A tread, a tire and a fin respond differently to
//! the same forces, and today the solver cannot tell them apart because the
//! only signal is a filename prefix. Traits give each response a coefficient
//! instead of a branch:
//!
//! * `traction` scales grip and how sharply lateral slip is corrected
//! * `lift` scales how much airflow becomes upward force
//! * `hover` decouples ground contact from support
//! * `mass_bias` moves the centre of support fore/aft
//!
//! ## What it deliberately is NOT
//!
//! It is not learned here, and not from geometry. The categories in fleet.json
//! are the harvester's, derived from the source model's own name and structure,
//! and they are evidence rather than noise. A classifier over vertices trained
//! on 173 examples would be less accurate than the labels it was trained on.
//!
//! The honest version of "learn this" is: seed from the labels, let the traits
//! be edited per chassis, and only then consider fitting the parts that remain
//! ambiguous. Seeding is this module. The rest needs part-level labels that do
//! not exist yet — the fleet's 46 part names are robot anatomy (`pelvis`,
//! `clavl`, `thighr`) and contain exactly one locomotion word, `wheel`. No
//! tread, no fin, no rotor.

/// A chassis's physical character, as six independent 0..1 traits.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Traits {
    /// Grip against the ground. Treads high, tires medium, skids low.
    pub traction: f32,
    /// Airflow becomes upward force. Wings and fins high.
    pub lift: f32,
    /// Supported without ground contact.
    pub hover: f32,
    /// Resists damage and acceleration alike.
    pub armour: f32,
    /// Responds quickly to steering input.
    pub agility: f32,
    /// Where support sits: 0 fully rear, 0.5 centred, 1 fully forward.
    pub mass_bias: f32,
}

impl Default for Traits {
    /// A neutral ground vehicle. Chosen so an UNKNOWN chassis drives rather
    /// than falls through the world or flies away — the failure of an unknown
    /// label should be dull, not spectacular.
    fn default() -> Self {
        Traits { traction: 0.6, lift: 0.0, hover: 0.0,
                 armour: 0.4, agility: 0.5, mass_bias: 0.5 }
    }
}

impl Traits {
    /// Seed from `fleet.json`'s category string.
    ///
    /// A translation, not a classifier. The categories are the harvester's and
    /// carry real information; this turns each into the vector that says the
    /// same thing without the exclusivity.
    pub fn from_category(cat: &str) -> Traits {
        let t = |traction, lift, hover, armour, agility, mass_bias| Traits {
            traction, lift, hover, armour, agility, mass_bias };
        match cat {
            "car"       => t(0.75, 0.05, 0.0, 0.30, 0.70, 0.50),
            "racer"     => t(0.85, 0.10, 0.0, 0.20, 0.90, 0.45),
            "truck"     => t(0.80, 0.00, 0.0, 0.60, 0.30, 0.55),
            // Treads: the most grip and the least agility, which is the pair
            // that makes a tank feel like a tank rather than a heavy car.
            "tank"      => t(0.95, 0.00, 0.0, 0.90, 0.20, 0.50),
            "flyer"     => t(0.10, 0.90, 0.2, 0.25, 0.80, 0.40),
            "flyerf"    => t(0.10, 0.95, 0.2, 0.20, 0.90, 0.35),
            "spaceship" => t(0.00, 0.30, 0.9, 0.40, 0.60, 0.50),
            "hoverer"   => t(0.20, 0.20, 0.9, 0.30, 0.70, 0.50),
            // The two hybrids the string vocabulary had to invent a word for.
            // Here they are just both things at once, and no new variant was
            // needed to express it.
            "hovertank" => t(0.30, 0.10, 0.8, 0.90, 0.30, 0.50),
            "boat"      => t(0.40, 0.00, 0.5, 0.70, 0.25, 0.55),
            _ => Traits::default(),
        }
    }

    /// Blend toward another set, for a chassis that is partly something else.
    ///
    /// The operation a string cannot do at all, and the reason the vector
    /// exists: "70% tank, 30% hoverer" is a sentence about a real chassis.
    pub fn blend(self, other: Traits, w: f32) -> Traits {
        let w = w.clamp(0.0, 1.0);
        // Endpoint-exact lerp. `a + (b - a) * w` is monotonic but NOT exact at
        // w = 1: for tank->hoverer traction it returns 0.19999999 where the
        // hoverer's own value is 0.2, so a fully blended chassis was not equal
        // to the thing it blended into. That matters wherever a blend is
        // compared, keyed, or thresholded — `flies()` is a threshold on lift,
        // and a chassis one ulp under the line does not fly.
        //
        // The two standard forms trade off against each other: the difference
        // form is monotonic, `a*(1-w) + b*w` is endpoint-exact but can be
        // non-monotonic in the interior. Taking the endpoints explicitly keeps
        // both properties.
        let m = |a: f32, b: f32| {
            if w <= 0.0 {
                a
            } else if w >= 1.0 {
                b
            } else {
                a + (b - a) * w
            }
        };
        Traits {
            traction: m(self.traction, other.traction),
            lift: m(self.lift, other.lift),
            hover: m(self.hover, other.hover),
            armour: m(self.armour, other.armour),
            agility: m(self.agility, other.agility),
            mass_bias: m(self.mass_bias, other.mass_bias),
        }
    }

    /// Does this chassis fly, for the one boolean the loader still needs?
    ///
    /// Replaces `bot.starts_with("jet_")`. Kept as a threshold on `lift` rather
    /// than a stored flag, so the two cannot disagree — a chassis with wings
    /// and a flag saying otherwise is a bug that only shows up in the air.
    pub fn flies(&self) -> bool { self.lift >= 0.5 }

    /// Is it supported without touching the ground?
    pub fn floats(&self) -> bool { self.hover >= 0.5 }

    /// Grip coefficient for the solver, given how much weight is on the
    /// contact patch.
    ///
    /// Hovering LIFTS weight off the patch, so a hovertank has a tank's
    /// traction rating and much less usable grip. Multiplying rather than
    /// branching is what lets one formula serve both.
    pub fn grip(&self) -> f32 { self.traction * (1.0 - self.hover * 0.8) }

    /// How this chassis should compact through the fold.
    ///
    /// compact.rs is explicit that its two styles "should not be one knob", and my
    /// first pass set one anyway — `Params::default()` for everything.
    ///
    /// * A body with real vehicle bodywork wants `Shrink`: "a door does not stretch
    ///   into a shoulder, it compacts and re-emerges". Cars, trucks, tanks.
    /// * A body whose robot mode has limbs with no vehicle-mode counterpart wants
    ///   `Grow`, which makes "mass appear to be added rather than rearranged".
    ///
    /// Keyed off `armour`, which is the trait that tracks how much bodywork there
    /// is to fold — a tank is panels all the way down, a racer is a shell over a
    /// frame. Not off a category string, for the reason the whole trait vector
    /// exists: a hovertank is both things and no word covers it.
    ///
    /// `strength` scales with armour too. The eye-tuned 0.55 was chosen across six
    /// body types as the value that avoided "noise on the small ones and mush on
    /// the large ones" — so a heavily panelled body can take more compaction than
    /// a sparse one, and this stays inside the range that tuning covered rather
    /// than reaching past it.
    pub fn compaction(&self) -> crate::compact::Params {
        let mut p = crate::compact::Params::default();
        if self.armour < 0.35 {
            // Little bodywork to fold: the robot's limbs are new mass, not
            // rearranged panels.
            p.style = crate::compact::Style::Grow;
        }
        // 0.45 at no armour to 0.65 at full, around the tuned 0.55.
        p.strength = 0.45 + 0.20 * self.armour.clamp(0.0, 1.0);
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hovertank_is_both_things_without_a_new_category() {
        // The whole argument for the vector. The string vocabulary had to
        // invent "hovertank"; here it is a tank's armour and a hoverer's
        // support, and nothing new was needed.
        let h = Traits::from_category("hovertank");
        let tank = Traits::from_category("tank");
        let hov = Traits::from_category("hoverer");
        assert_eq!(h.armour, tank.armour, "should be as armoured as a tank");
        assert!(h.hover >= hov.hover - 0.15, "should hover nearly as well");
    }

    #[test]
    fn hovering_costs_grip_even_at_full_traction() {
        // A tank's tread rating with the weight lifted off it is not a tank's
        // grip. One formula, no branch.
        let tank = Traits::from_category("tank");
        let ht = Traits::from_category("hovertank");
        assert!(tank.grip() > ht.grip() * 2.0, "{} vs {}", tank.grip(), ht.grip());
    }

    #[test]
    fn treads_grip_harder_and_turn_slower_than_tires() {
        // The pair that makes a tank feel like a tank rather than a heavy car.
        let tank = Traits::from_category("tank");
        let car = Traits::from_category("car");
        assert!(tank.traction > car.traction);
        assert!(tank.agility < car.agility);
    }

    #[test]
    fn flight_comes_from_lift_not_from_a_filename() {
        // Replaces bot.starts_with("jet_"), which made a naming convention
        // load-bearing for physics.
        assert!(Traits::from_category("flyer").flies());
        assert!(!Traits::from_category("tank").flies());
        assert!(!Traits::from_category("car").flies());
    }

    #[test]
    fn flies_is_derived_so_it_cannot_disagree_with_lift() {
        // A stored flag beside a lift value is two facts that can drift. A
        // winged chassis flagged as grounded is a bug that only appears in
        // mid-air.
        let mut t = Traits::default();
        t.lift = 0.9;
        assert!(t.flies());
        t.lift = 0.1;
        assert!(!t.flies());
    }

    #[test]
    fn an_unknown_category_drives_rather_than_falls_or_flies() {
        // The failure of an unknown label should be dull.
        let u = Traits::from_category("submarine_thing");
        assert!(!u.flies() && !u.floats());
        assert!(u.grip() > 0.0);
    }

    #[test]
    fn blending_expresses_what_a_string_cannot() {
        // "70% tank, 30% hoverer" is a sentence about a real chassis, and no
        // vocabulary of words can hold it.
        let t = Traits::from_category("tank");
        let h = Traits::from_category("hoverer");
        let mix = t.blend(h, 0.3);
        assert!(mix.hover > t.hover && mix.hover < h.hover);
        assert!(mix.traction < t.traction && mix.traction > h.traction);
    }

    #[test]
    fn blending_is_clamped_at_both_ends() {
        let t = Traits::from_category("tank");
        let h = Traits::from_category("hoverer");
        assert_eq!(t.blend(h, -1.0), t);
        assert_eq!(t.blend(h, 2.0), h);
        // exactly at the ends, not merely near them
        assert_eq!(t.blend(h, 0.0), t);
        assert_eq!(t.blend(h, 1.0), h);
    }

    #[test]
    fn a_full_blend_flies_exactly_as_the_thing_it_blended_into() {
        // the reason endpoint exactness is not pedantry: `flies()` is a
        // threshold on lift, so a chassis one ulp under the line silently
        // stops flying
        for a in ["tank", "hoverer", "flyer", "boat", "spaceship"] {
            for b in ["tank", "hoverer", "flyer", "boat", "spaceship"] {
                let (x, y) = (Traits::from_category(a), Traits::from_category(b));
                assert_eq!(x.blend(y, 1.0), y, "{a} fully blended into {b} is not {b}");
                assert_eq!(x.blend(y, 1.0).flies(), y.flies(), "{a}->{b} changed flight");
            }
        }
    }

    #[test]
    fn blending_stays_monotonic_in_the_interior() {
        // the property the difference form was chosen for; taking the
        // endpoints explicitly must not have cost it
        let t = Traits::from_category("tank");
        let h = Traits::from_category("hoverer");
        let mut prev = t.hover;
        for i in 0..=20 {
            let v = t.blend(h, i as f32 / 20.0).hover;
            assert!(v >= prev - 1e-6, "hover went backwards at w={}", i as f32 / 20.0);
            prev = v;
        }
    }

    #[test]
    fn every_fleet_category_maps_to_something_deliberate() {
        // If a category falls through to Default it is silently a neutral car,
        // which for a spaceship is wrong in a way nothing reports.
        for c in ["car", "racer", "truck", "tank", "flyer", "flyerf",
                  "spaceship", "hoverer", "hovertank", "boat"] {
            assert_ne!(Traits::from_category(c), Traits::default(),
                       "{c} fell through to the default");
        }
    }

    #[test]
    fn a_panelled_body_shrinks_and_a_sparse_one_grows() {
        // compact.rs: Shrink is "the one for vehicle panels: a door does not
        // stretch into a shoulder"; Grow is for "limbs and weapons that have no
        // vehicle-mode counterpart to fold from".
        use crate::compact::Style;
        assert_eq!(Traits::from_category("tank").compaction().style, Style::Shrink);
        assert_eq!(Traits::from_category("truck").compaction().style, Style::Shrink);
        assert_eq!(Traits::from_category("racer").compaction().style, Style::Grow);
        assert_eq!(Traits::from_category("car").compaction().style, Style::Grow);
    }

    #[test]
    fn strength_stays_inside_the_range_someone_actually_tuned() {
        // 0.55 was picked by eye across six body types. Scaling with armour is
        // reasonable; reaching past what was looked at is not.
        for c in ["car", "racer", "truck", "tank", "flyer", "spaceship",
                  "hoverer", "hovertank", "boat", "unknown"] {
            let s = Traits::from_category(c).compaction().strength;
            assert!((0.45..=0.65).contains(&s), "{c} -> {s}");
        }
    }

    #[test]
    fn traits_stay_in_range() {
        for c in ["car", "racer", "truck", "tank", "flyer", "flyerf",
                  "spaceship", "hoverer", "hovertank", "boat", "unknown"] {
            let t = Traits::from_category(c);
            for (name, v) in [("traction", t.traction), ("lift", t.lift),
                              ("hover", t.hover), ("armour", t.armour),
                              ("agility", t.agility), ("mass_bias", t.mass_bias)] {
                assert!((0.0..=1.0).contains(&v), "{c}.{name} = {v}");
            }
        }
    }
}
