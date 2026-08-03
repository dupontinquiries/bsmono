//! CONTENT, ported from blackstar's `content/*.json`.
//!
//! Three tables, chosen because each one closes a gap something already
//! built was leaving open rather than because it was next in the directory.
//!
//! * **Chassis** carries `xform_dur` and `fold_spin`. `xform::Xform` has had
//!   a `dur_mul` field since the state machine landed and nothing has ever
//!   set it — every chassis folded at the same speed. These are the numbers
//!   that were missing, and they are a real spread: a juggernaut takes 1.35x
//!   as long as standard and a raptor 0.8x, so a heavy machine visibly
//!   labours through the same fold a light one snaps through.
//! * **Troops** are the hostiles `patrol.rs` needs. It has factions, events
//!   and a `credit_kill` and nothing to kill. Seventeen records: ten player
//!   classes, four enemies, three elemental variants.
//! * **Films** are the campaign spine, from the script material — two
//!   trilogies, and the shape of the second one is a deliberate inversion of
//!   the first rather than more of it.
//!
//! Every `fire_mode` in the troop data maps onto a `weapons::Verb`, which is
//! checked by a test: content that names a weapon the engine cannot fire is
//! a crash waiting for whoever adds the seventh chassis.

use crate::weapons::{Element, Verb};

// ------------------------------------------------------------- chassis ---

pub struct Chassis {
    pub id: &'static str,
    /// fold duration multiplier — feeds `xform::Xform::dur_mul`
    pub xform_dur: f32,
    /// degrees of spin the fold carries, for the choreography
    pub fold_spin: f32,
    pub bulk: f32,
    pub height: f32,
    pub arm: f32,
    pub leg: f32,
}

pub const CHASSIS: &[Chassis] = &[
    Chassis { id: "standard",   xform_dur: 1.00, fold_spin: 220.0, bulk: 1.00, height: 1.00, arm: 1.00, leg: 1.00 },
    Chassis { id: "angular",    xform_dur: 0.78, fold_spin: 300.0, bulk: 0.88, height: 1.02, arm: 1.06, leg: 1.02 },
    Chassis { id: "juggernaut", xform_dur: 1.35, fold_spin: 160.0, bulk: 1.28, height: 0.96, arm: 1.10, leg: 0.94 },
    Chassis { id: "strider",    xform_dur: 0.92, fold_spin: 240.0, bulk: 0.82, height: 1.12, arm: 1.04, leg: 1.12 },
    Chassis { id: "raptor",     xform_dur: 0.80, fold_spin: 300.0, bulk: 0.78, height: 1.18, arm: 0.85, leg: 1.32 },
    Chassis { id: "bruiser",    xform_dur: 1.28, fold_spin: 170.0, bulk: 1.38, height: 0.86, arm: 1.40, leg: 0.84 },
    Chassis { id: "sentinel",   xform_dur: 1.05, fold_spin: 250.0, bulk: 0.76, height: 1.34, arm: 1.12, leg: 1.18 },
];

pub fn chassis(id: &str) -> Option<&'static Chassis> {
    CHASSIS.iter().find(|c| c.id == id)
}

// -------------------------------------------------------------- troops ---

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// something the player can be
    PlayerClass,
    /// a straightforward hostile
    Enemy,
    /// an elemental variant — the coalition races, or something wearing one
    Variant,
}

pub struct Troop {
    pub id: &'static str,
    pub role: Role,
    pub chassis: &'static str,
    /// how it shoots, as one of the engine's verbs
    pub fire: Verb,
    /// which element it fights as, for `Element::advantage`
    pub element: Element,
    pub tint: [u8; 3],
}

/// The `fire_mode` strings in the JSON, resolved. `suppress` and `beam` both
/// land on Beam — a suppression beam is a beam that slows, and the slow is a
/// weapon-profile property rather than a different verb.
pub const TROOPS: &[Troop] = &[
    // ---- player classes ----
    Troop { id: "vanguard",    role: Role::PlayerClass, chassis: "standard",   fire: Verb::Push,   element: Element::Water,    tint: [150, 170, 200] },
    Troop { id: "interceptor", role: Role::PlayerClass, chassis: "angular",    fire: Verb::Cone,   element: Element::Air,      tint: [170, 200, 190] },
    Troop { id: "titan",       role: Role::PlayerClass, chassis: "juggernaut", fire: Verb::Chain,  element: Element::Electric, tint: [140, 150, 175] },
    Troop { id: "specialist",  role: Role::PlayerClass, chassis: "strider",    fire: Verb::Beam,   element: Element::Ice,      tint: [160, 200, 220] },
    Troop { id: "reaver",      role: Role::PlayerClass, chassis: "raptor",     fire: Verb::Bank,   element: Element::Rock,     tint: [180, 165, 140] },
    Troop { id: "warden",      role: Role::PlayerClass, chassis: "bruiser",    fire: Verb::Beam,   element: Element::Ice,      tint: [150, 185, 205] },
    Troop { id: "pyre",        role: Role::PlayerClass, chassis: "angular",    fire: Verb::Flak,   element: Element::Fire,     tint: [220, 130, 70]  },
    Troop { id: "prime",       role: Role::PlayerClass, chassis: "sentinel",   fire: Verb::Shove,  element: Element::Water,    tint: [90, 140, 210]  },
    Troop { id: "scout",       role: Role::PlayerClass, chassis: "raptor",     fire: Verb::Pool,   element: Element::Vine,     tint: [120, 180, 120] },
    Troop { id: "siege",       role: Role::PlayerClass, chassis: "juggernaut", fire: Verb::Pool,   element: Element::Rock,     tint: [165, 150, 130] },
    // ---- hostiles: what patrol has been missing ----
    Troop { id: "grunt",            role: Role::Enemy, chassis: "standard", fire: Verb::Push,   element: Element::None,     tint: [130, 130, 140] },
    Troop { id: "blackstar_raider", role: Role::Enemy, chassis: "angular",  fire: Verb::Pierce, element: Element::Electric, tint: [70, 70, 90]    },
    Troop { id: "ravager",          role: Role::Enemy, chassis: "bruiser",  fire: Verb::Flak,   element: Element::Fire,     tint: [180, 90, 60]   },
    Troop { id: "slipstream",       role: Role::Enemy, chassis: "raptor",   fire: Verb::Cone,   element: Element::Air,      tint: [160, 190, 200] },
    // ---- elemental variants ----
    Troop { id: "ashfall_lancer",     role: Role::Variant, chassis: "raptor",   fire: Verb::Flak,     element: Element::Fire, tint: [210, 110, 60]  },
    Troop { id: "hoarfrost_sentinel", role: Role::Variant, chassis: "sentinel", fire: Verb::Beam,     element: Element::Ice,  tint: [170, 215, 235] },
    Troop { id: "magma_breaker",      role: Role::Variant, chassis: "bruiser",  fire: Verb::Pool,     element: Element::Fire, tint: [200, 80, 40]   },
];

pub fn troop(id: &str) -> Option<&'static Troop> {
    TROOPS.iter().find(|t| t.id == id)
}

/// Everything that can be spawned against the player.
pub fn hostiles() -> impl Iterator<Item = &'static Troop> {
    TROOPS.iter().filter(|t| t.role != Role::PlayerClass)
}

// --------------------------------------------------------------- films ---

/// The campaign spine. Six films in two trilogies, and the second is a
/// deliberate inversion of the first rather than an escalation of it: where
/// one is disaster, war and sacrifice, two opens into wonder — the heroes
/// stop defending something and start discovering something.
pub struct Film {
    pub n: u8,
    pub title: &'static str,
    pub subtitle: &'static str,
    /// the register the whole film plays in, which is what a mission set
    /// built for it should inherit
    pub tone: &'static str,
    /// where its missions should mostly happen — patrol destination ids
    pub theatres: &'static [&'static str],
}

pub const FILMS: &[Film] = &[
    Film { n: 1, title: "DOOMSDAY", subtitle: "The Awakening",
        tone: "disaster and horror; the fracture arrives before anyone understands it",
        theatres: &["earth", "luna", "mars"] },
    Film { n: 2, title: "DOMINION", subtitle: "God of Chaos",
        tone: "occupation; Earth has fallen and the fight is from underneath",
        theatres: &["earth", "cybertron", "europa"] },
    Film { n: 3, title: "DESTINY", subtitle: "The Core",
        tone: "revelation; the shield, the coalition, and why Earth sits at the centre",
        theatres: &["earth", "cybertron", "shield_edge"] },
    Film { n: 4, title: "FIRST LIGHT", subtitle: "Contact and Wonder",
        tone: "exploration; the cosmos is vast and strange and nobody is defending anything",
        theatres: &["glaciem", "helix", "uncharted"] },
    Film { n: 5, title: "SOLAR", subtitle: "The Scale of the Threat",
        tone: "escalation; a fire clone who absorbs stars, and what that means for scale",
        theatres: &["io", "titan", "outer_rim"] },
    Film { n: 6, title: "PHOTON", subtitle: "The Watch's Ceiling",
        tone: "climax; the apex form nobody has fully mapped",
        theatres: &["shield_edge", "outer_rim", "cybertron"] },
];

pub fn film(n: u8) -> Option<&'static Film> {
    FILMS.iter().find(|f| f.n == n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patrol;

    #[test]
    fn every_troop_names_a_chassis_that_exists() {
        for t in TROOPS {
            assert!(chassis(t.chassis).is_some(), "{} rides unknown chassis {}", t.id, t.chassis);
        }
    }

    #[test]
    fn ids_are_unique_everywhere() {
        for (i, c) in CHASSIS.iter().enumerate() {
            assert!(CHASSIS.iter().skip(i + 1).all(|o| o.id != c.id), "duplicate chassis {}", c.id);
        }
        for (i, t) in TROOPS.iter().enumerate() {
            assert!(TROOPS.iter().skip(i + 1).all(|o| o.id != t.id), "duplicate troop {}", t.id);
        }
    }

    #[test]
    fn the_fold_durations_actually_differ() {
        // the whole reason to port this table: dur_mul was never set and
        // every chassis folded identically
        let slow = chassis("juggernaut").unwrap().xform_dur;
        let fast = chassis("raptor").unwrap().xform_dur;
        assert!(slow > 1.2 && fast < 0.85, "{slow} vs {fast}");
        // and nothing is degenerate — a 0 would divide by zero in the blend
        for c in CHASSIS {
            assert!(c.xform_dur > 0.1 && c.xform_dur < 3.0, "{} has dur {}", c.id, c.xform_dur);
            assert!(c.fold_spin > 0.0, "{} does not turn at all", c.id);
        }
    }

    #[test]
    fn heavy_chassis_fold_slower_and_spin_less() {
        // the data should say something, not just be seven rows of noise
        let mut pairs: Vec<(f32, f32)> = CHASSIS.iter().map(|c| (c.bulk, c.xform_dur)).collect();
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let light = pairs.first().unwrap().1;
        let heavy = pairs.last().unwrap().1;
        assert!(heavy > light, "bulk and fold duration are unrelated ({light} -> {heavy})");
        for c in CHASSIS {
            if c.bulk > 1.2 {
                assert!(c.fold_spin < 200.0, "{} is heavy but whips round", c.id);
            }
        }
    }

    #[test]
    fn patrol_has_something_to_fight() {
        // patrol.rs has factions, events and credit_kill and had nothing to
        // kill; this is that gap closed
        assert!(hostiles().count() >= 4);
        assert!(hostiles().any(|t| t.role == Role::Enemy));
        assert!(hostiles().any(|t| t.role == Role::Variant));
        // and every hostile fights as something, so Element::advantage —
        // implemented, tested, and called by nothing — finally has a defender
        assert!(hostiles().filter(|t| t.element != Element::None).count() >= 5);
    }

    #[test]
    fn there_are_six_films_in_order() {
        assert_eq!(FILMS.len(), 6);
        for (i, f) in FILMS.iter().enumerate() {
            assert_eq!(f.n as usize, i + 1, "{} is out of order", f.title);
            assert!(!f.theatres.is_empty(), "{} happens nowhere", f.title);
        }
        assert_eq!(film(6).unwrap().title, "PHOTON");
    }

    #[test]
    fn every_theatre_is_a_real_destination() {
        // the check that stops the campaign pointing at a map that does not
        // exist — the same class of bug RAIDS.md records the raid generator
        // catching when phases referenced kuiper and unicron
        for f in FILMS {
            for t in f.theatres {
                assert!(patrol::find(t).is_some(), "film {} names unknown theatre {t}", f.n);
            }
        }
    }

    #[test]
    fn the_second_trilogy_goes_somewhere_the_first_does_not() {
        let early: Vec<&str> = FILMS[..3].iter().flat_map(|f| f.theatres.iter().copied()).collect();
        let late: Vec<&str> = FILMS[3..].iter().flat_map(|f| f.theatres.iter().copied()).collect();
        let new = late.iter().filter(|t| !early.contains(t)).count();
        assert!(new >= 4, "trilogy two revisits trilogy one's map ({new} new theatres)");
    }
}
