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
    /// ability ids; resolved through `ability()`
    pub abilities: &'static [&'static str],
}

/// The `fire_mode` strings in the JSON, resolved. `suppress` and `beam` both
/// land on Beam — a suppression beam is a beam that slows, and the slow is a
/// weapon-profile property rather than a different verb.
pub const TROOPS: &[Troop] = &[
    // ---- player classes ----
    Troop { id: "vanguard",    role: Role::PlayerClass, chassis: "standard",   fire: Verb::Push,   element: Element::Water,    tint: [150, 170, 200], abilities: &["dash", "overclock"] },
    Troop { id: "interceptor", role: Role::PlayerClass, chassis: "angular",    fire: Verb::Cone,   element: Element::Air,      tint: [170, 200, 190], abilities: &["dash", "cloak"] },
    Troop { id: "titan",       role: Role::PlayerClass, chassis: "juggernaut", fire: Verb::Chain,  element: Element::Electric, tint: [140, 150, 175], abilities: &["shield", "turret"] },
    Troop { id: "specialist",  role: Role::PlayerClass, chassis: "strider",    fire: Verb::Beam,   element: Element::Ice,      tint: [160, 200, 220], abilities: &["cloak", "blink"] },
    Troop { id: "reaver",      role: Role::PlayerClass, chassis: "raptor",     fire: Verb::Bank,   element: Element::Rock,     tint: [180, 165, 140], abilities: &["dash", "pulse"] },
    Troop { id: "warden",      role: Role::PlayerClass, chassis: "bruiser",    fire: Verb::Beam,   element: Element::Ice,      tint: [150, 185, 205], abilities: &["shield", "resonance"] },
    Troop { id: "pyre",        role: Role::PlayerClass, chassis: "angular",    fire: Verb::Flak,   element: Element::Fire,     tint: [220, 130, 70], abilities: &["overclock", "pulse"] },
    Troop { id: "prime",       role: Role::PlayerClass, chassis: "sentinel",   fire: Verb::Shove,  element: Element::Water,    tint: [90, 140, 210], abilities: &["shield", "resonance"] },
    Troop { id: "scout",       role: Role::PlayerClass, chassis: "raptor",     fire: Verb::Pool,   element: Element::Vine,     tint: [120, 180, 120], abilities: &["hover", "dash"] },
    Troop { id: "siege",       role: Role::PlayerClass, chassis: "juggernaut", fire: Verb::Pool,   element: Element::Rock,     tint: [165, 150, 130], abilities: &["turret", "shield"] },
    // ---- hostiles: what patrol has been missing ----
    Troop { id: "grunt",            role: Role::Enemy, chassis: "standard", fire: Verb::Push,   element: Element::None,     tint: [130, 130, 140], abilities: &[] },
    Troop { id: "blackstar_raider", role: Role::Enemy, chassis: "angular",  fire: Verb::Pierce, element: Element::Electric, tint: [70, 70, 90], abilities: &["dash"] },
    Troop { id: "ravager",          role: Role::Enemy, chassis: "bruiser",  fire: Verb::Flak,   element: Element::Fire,     tint: [180, 90, 60], abilities: &["shield"] },
    Troop { id: "slipstream",       role: Role::Enemy, chassis: "raptor",   fire: Verb::Cone,   element: Element::Air,      tint: [160, 190, 200], abilities: &["hover", "dash"] },
    // ---- elemental variants ----
    Troop { id: "ashfall_lancer",     role: Role::Variant, chassis: "raptor",   fire: Verb::Flak,     element: Element::Fire, tint: [210, 110, 60], abilities: &["overclock"] },
    Troop { id: "hoarfrost_sentinel", role: Role::Variant, chassis: "sentinel", fire: Verb::Beam,     element: Element::Ice,  tint: [170, 215, 235], abilities: &["shield", "resonance"] },
    Troop { id: "magma_breaker",      role: Role::Variant, chassis: "bruiser",  fire: Verb::Pool,     element: Element::Fire, tint: [200, 80, 40], abilities: &["pulse"] },
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

// ----------------------------------------------------------- abilities ---
//
// Referenced by the troop records and dropped on the first pass. A perk is
// always on and multiplies something; an active is a thing you press.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AbilityKind {
    Perk,
    Active,
}

pub struct Ability {
    pub id: &'static str,
    pub kind: AbilityKind,
    /// what it multiplies or deploys, as the C engine named it
    pub effect: &'static str,
    pub value: f32,
}

pub const ABILITIES: &[Ability] = &[
    Ability { id: "hover",     kind: AbilityKind::Perk,   effect: "jump_mul",     value: 1.25 },
    Ability { id: "dash",      kind: AbilityKind::Perk,   effect: "move_mul",     value: 1.12 },
    Ability { id: "shield",    kind: AbilityKind::Perk,   effect: "deploy_armor", value: 40.0 },
    Ability { id: "cloak",     kind: AbilityKind::Perk,   effect: "detect_mul",   value: 0.60 },
    // fire_mul below 1 is FASTER: it scales the cooldown, not the rate
    Ability { id: "overclock", kind: AbilityKind::Perk,   effect: "fire_mul",     value: 0.80 },
    Ability { id: "turret",    kind: AbilityKind::Active, effect: "deploy_sentry", value: 12.0 },
    Ability { id: "resonance", kind: AbilityKind::Active, effect: "shock_pulse",  value: 1.0 },
    Ability { id: "pulse",     kind: AbilityKind::Active, effect: "cone_blast",   value: 1.0 },
    Ability { id: "blink",     kind: AbilityKind::Active, effect: "teleport",     value: 1.0 },
];

pub fn ability(id: &str) -> Option<&'static Ability> {
    ABILITIES.iter().find(|a| a.id == id)
}

// ---------------------------------------------------------- commanders ---
//
// Five archetypes with four dials each. They are the reason a squad of
// grunts is not just more grunts: the same troops under an aggressor and
// under a baiter play differently. The C engine sourced the flavour from
// Sun Tzu (Giles, 1910 — public domain), which is where the encirclement
// and tempo principles come from too.

pub struct Commander {
    pub id: &'static str,
    pub aggression: f32,
    pub caution: f32,
    /// how strongly the squad tries to get around you rather than at you
    pub encircle: f32,
    /// how readily it breaks off and reforms
    pub regroup: f32,
    pub desc: &'static str,
}

pub const COMMANDERS: &[Commander] = &[
    Commander { id: "aggressor",  aggression: 0.85, caution: 0.15, encircle: 0.30, regroup: 0.20,
        desc: "presses relentlessly; concentrates on the player and rarely yields ground" },
    Commander { id: "surrounder", aggression: 0.60, caution: 0.30, encircle: 0.90, regroup: 0.40,
        desc: "uses numbers to encircle, then closes the ring" },
    Commander { id: "skirmisher", aggression: 0.55, caution: 0.55, encircle: 0.50, regroup: 0.75,
        desc: "trades and withdraws; never commits to a line it cannot leave" },
    Commander { id: "baiter",     aggression: 0.45, caution: 0.65, encircle: 0.65, regroup: 0.60,
        desc: "offers a target worth chasing, and chooses the ground you chase it onto" },
    Commander { id: "warden",     aggression: 0.35, caution: 0.85, encircle: 0.25, regroup: 0.85,
        desc: "holds; gives ground slowly and makes every metre cost something" },
];

pub fn commander(id: &str) -> Option<&'static Commander> {
    COMMANDERS.iter().find(|c| c.id == id)
}

// ------------------------------------------------------------- missions ---

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Objective {
    Eliminate,
    Survive,
    Breach,
    Control,
    Protect,
}

pub struct Mission {
    pub order: u8,
    pub id: &'static str,
    /// which film this beat belongs to
    pub film: u8,
    /// the patrol destination it is sited on
    pub dest: &'static str,
    /// the named place within it — the C engine's map names are locales, not
    /// worlds, and four of the eight are somewhere on Earth
    pub site: &'static str,
    pub objective: Objective,
    /// the troop that leads the opposition, if one does
    pub champion: Option<&'static str>,
    pub beat: &'static str,
}

pub const MISSIONS: &[Mission] = &[
    Mission { order: 1, id: "the_pluto_line", film: 1, dest: "pluto", site: "the outer containment line",
        objective: Objective::Eliminate, champion: Some("hoarfrost_sentinel"),
        beat: "hold the outer containment line; Cryotek attacks" },
    Mission { order: 2, id: "the_scavenger_king", film: 1, dest: "outer_rim", site: "the Kuiper drift",
        objective: Objective::Eliminate, champion: Some("blackstar_raider"),
        beat: "Blackstar, the Kuiper warlord" },
    Mission { order: 3, id: "the_red_extraction", film: 1, dest: "mars", site: "the extraction rig",
        objective: Objective::Breach, champion: None,
        beat: "tear the Mars extraction rig apart; the shard is already gone" },
    Mission { order: 4, id: "the_beach_opens", film: 1, dest: "earth", site: "the beach",
        objective: Objective::Protect, champion: None,
        beat: "Earth's shore tears open; get the civilians clear" },
    Mission { order: 5, id: "what_jhiaxus_grew", film: 2, dest: "earth", site: "the vault",
        objective: Objective::Eliminate, champion: None,
        beat: "the architect of the Corrupted" },
    Mission { order: 6, id: "the_mantle_dive", film: 2, dest: "earth", site: "the foundry",
        objective: Objective::Control, champion: None,
        beat: "Hot Rod becomes Rodimus" },
    Mission { order: 7, id: "the_drowned_approach", film: 2, dest: "earth", site: "the trench",
        objective: Objective::Survive, champion: None,
        beat: "hold the descent as the Corrupted converge on the wound" },
    Mission { order: 8, id: "beneath_the_pacific", film: 3, dest: "cybertron", site: "the gravity well",
        objective: Objective::Eliminate, champion: None,
        beat: "Unicron; the truce; the gravity well" },
];

pub fn missions_of(film: u8) -> impl Iterator<Item = &'static Mission> {
    MISSIONS.iter().filter(move |m| m.film == film)
}

// ---------------------------------------------------------------- modes ---

pub struct GameMode {
    pub id: &'static str,
    /// 0 means endless
    pub waves: u32,
    pub endless: bool,
}

pub const MODES: &[GameMode] = &[
    GameMode { id: "arena_fight", waves: 8, endless: false },
    GameMode { id: "survival",    waves: 0, endless: true },
    GameMode { id: "stalkers",    waves: 0, endless: true },
    GameMode { id: "defense",     waves: 10, endless: false },
];

#[cfg(test)]
mod content2_tests {
    use super::*;
    use crate::patrol;

    #[test]
    fn every_troop_ability_exists() {
        for t in TROOPS {
            for a in t.abilities {
                assert!(ability(a).is_some(), "{} has unknown ability {a}", t.id);
            }
            assert!(t.abilities.len() <= 3, "{} carries {} abilities", t.id, t.abilities.len());
        }
        // player classes should all bring something; a plain grunt need not
        for t in TROOPS.iter().filter(|t| t.role == Role::PlayerClass) {
            assert!(!t.abilities.is_empty(), "{} has no abilities at all", t.id);
        }
    }

    #[test]
    fn perks_and_actives_are_told_apart() {
        assert_eq!(ABILITIES.len(), 9);
        assert!(ABILITIES.iter().any(|a| a.kind == AbilityKind::Perk));
        assert!(ABILITIES.iter().any(|a| a.kind == AbilityKind::Active));
        // the one that reads backwards: fire_mul BELOW one is faster,
        // because it scales the cooldown rather than the rate
        let o = ability("overclock").unwrap();
        assert!(o.value < 1.0 && o.effect == "fire_mul");
        // and the multipliers are all sane rather than accidentally zero
        for a in ABILITIES.iter().filter(|a| a.effect.ends_with("_mul")) {
            assert!(a.value > 0.1 && a.value < 3.0, "{} has mul {}", a.id, a.value);
        }
    }

    #[test]
    fn commander_dials_are_in_range_and_actually_differ() {
        for c in COMMANDERS {
            for (n, v) in [("aggression", c.aggression), ("caution", c.caution),
                           ("encircle", c.encircle), ("regroup", c.regroup)] {
                assert!((0.0..=1.0).contains(&v), "{} {n} is {v}", c.id);
            }
        }
        // aggression and caution should oppose, or the archetypes are noise
        let agg = commander("aggressor").unwrap();
        let war = commander("warden").unwrap();
        assert!(agg.aggression > war.aggression && agg.caution < war.caution);
        // and the surrounder should be the one that surrounds
        assert!(COMMANDERS.iter().all(|c| c.id == "surrounder"
            || c.encircle <= commander("surrounder").unwrap().encircle));
    }

    #[test]
    fn every_mission_is_sited_somewhere_real() {
        for m in MISSIONS {
            assert!(patrol::find(m.dest).is_some(), "{} is sited on unknown {}", m.id, m.dest);
            assert!(film(m.film).is_some(), "{} belongs to no film", m.id);
            if let Some(c) = m.champion {
                assert!(troop(c).is_some(), "{} is led by unknown troop {c}", m.id);
            }
        }
    }

    #[test]
    fn the_campaign_is_ordered_and_unique() {
        for (i, m) in MISSIONS.iter().enumerate() {
            assert_eq!(m.order as usize, i + 1, "{} is out of order", m.id);
            assert!(MISSIONS.iter().skip(i + 1).all(|o| o.id != m.id), "duplicate {}", m.id);
        }
        // films run forward: a later mission never belongs to an earlier film
        let mut f = 0;
        for m in MISSIONS {
            assert!(m.film >= f, "{} goes back to film {}", m.id, m.film);
            f = m.film;
        }
    }

    #[test]
    fn the_first_trilogy_has_missions_and_the_second_does_not_yet() {
        for n in 1..=3u8 {
            assert!(missions_of(n).count() > 0, "film {n} has no missions");
        }
        // trilogy two is scripted but unwritten as missions; asserting it so
        // the gap is recorded rather than discovered
        assert_eq!(missions_of(4).count() + missions_of(5).count() + missions_of(6).count(), 0);
    }

    #[test]
    fn modes_agree_with_themselves() {
        for m in MODES {
            assert_eq!(m.endless, m.waves == 0, "{} disagrees about being endless", m.id);
        }
    }
}

// --------------------------------------------------------- signatures ---
//
// The back-mounted piece a bot fights with. Ten of them, and every one is a
// TRADE: nothing here is strictly better than anything else, which a test
// enforces rather than trusting. `fire_mul` reads backwards the same way
// overclock does — it scales the cooldown, so above 1 is slower.

pub struct Signature {
    pub id: &'static str,
    pub dmg: f32,
    /// cooldown multiplier: ABOVE one is slower
    pub fire: f32,
    /// incoming damage multiplier: below one is tougher
    pub taken: f32,
    pub speed: f32,
    pub extra_pellets: u32,
    pub knockback: f32,
    pub slow: f32,
    pub splash: f32,
}

pub const SIGNATURES: &[Signature] = &[
    Signature { id: "wing_cannon",  dmg: 0.95, fire: 0.95, taken: 1.15, speed: 1.10, extra_pellets: 1, knockback: 0.0,  slow: 0.0, splash: 0.0 },
    Signature { id: "heavy_cannon", dmg: 1.35, fire: 1.35, taken: 0.90, speed: 0.90, extra_pellets: 0, knockback: 6.0,  slow: 0.0, splash: 3.0 },
    Signature { id: "crest",        dmg: 1.05, fire: 1.05, taken: 0.80, speed: 0.95, extra_pellets: 0, knockback: 4.0,  slow: 0.0, splash: 0.0 },
    Signature { id: "cable",        dmg: 0.85, fire: 0.70, taken: 1.05, speed: 1.15, extra_pellets: 0, knockback: 0.0,  slow: 0.30, splash: 0.0 },
    Signature { id: "harpoon",      dmg: 1.10, fire: 1.10, taken: 1.00, speed: 1.00, extra_pellets: 0, knockback: 10.0, slow: 0.40, splash: 0.0 },
    Signature { id: "halo",         dmg: 1.00, fire: 1.00, taken: 1.00, speed: 1.05, extra_pellets: 0, knockback: 0.0,  slow: 0.0, splash: 6.0 },
    Signature { id: "solar",        dmg: 1.20, fire: 1.10, taken: 1.00, speed: 1.00, extra_pellets: 0, knockback: 0.0,  slow: 0.0, splash: 4.0 },
    Signature { id: "aegis",        dmg: 1.00, fire: 1.00, taken: 0.70, speed: 0.95, extra_pellets: 0, knockback: 4.0,  slow: 0.0, splash: 0.0 },
    Signature { id: "recon",        dmg: 0.95, fire: 1.05, taken: 1.05, speed: 1.20, extra_pellets: 0, knockback: 0.0,  slow: 0.0, splash: 0.0 },
    Signature { id: "mortar",       dmg: 1.45, fire: 1.35, taken: 0.85, speed: 0.75, extra_pellets: 0, knockback: 14.0, slow: 0.0, splash: 11.0 },
];

pub fn signature(id: &str) -> Option<&'static Signature> {
    SIGNATURES.iter().find(|s| s.id == id)
}

// ---------------------------------------------------------------- hub ---
//
// The ship. Eight rooms with exits, which is a GRAPH rather than a menu —
// and that is the loaded-world-section idea: a bridge you walk to instead of
// a "choose destination" screen, an upgrade bay instead of a loadout tab.
// The airlock is the interesting one: it says "no gravity past this door",
// which is the zero-g movement the asteroid fields need, in a place where
// nothing is shooting at you while you learn it.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoomKind {
    Bridge,
    Corridor,
    Upgrade,
    Vendor,
    Quest,
    Spacewalk,
    Hangar,
}

pub struct Room {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: RoomKind,
    pub pos: [f32; 3],
    pub exits: &'static [&'static str],
    pub blurb: &'static str,
}

pub const HUB: &[Room] = &[
    Room { id: "bridge", name: "Bridge", kind: RoomKind::Bridge, pos: [0.0, 0.0, 24.0],
        exits: &["corridor_fore", "quarters"],
        blurb: "Navigation console. Choose where the ship goes next." },
    Room { id: "corridor_fore", name: "Forward Corridor", kind: RoomKind::Corridor, pos: [0.0, 0.0, 12.0],
        exits: &["bridge", "upgrade_bay", "market"],
        blurb: "Spine of the ship. Viewports on both sides." },
    Room { id: "upgrade_bay", name: "Upgrade Bay", kind: RoomKind::Upgrade, pos: [-9.0, 0.0, 6.0],
        exits: &["corridor_fore", "hangar"],
        blurb: "Chassis rigs and part racks. Rebuild your bot here." },
    Room { id: "market", name: "Trade Deck", kind: RoomKind::Vendor, pos: [9.0, 0.0, 6.0],
        exits: &["corridor_fore", "quarters"],
        blurb: "Whoever docked last is still selling." },
    Room { id: "quarters", name: "Crew Quarters", kind: RoomKind::Quest, pos: [7.0, 0.0, -6.0],
        exits: &["market", "bridge", "corridor_aft"],
        blurb: "Where the crew bring you problems." },
    Room { id: "corridor_aft", name: "Aft Corridor", kind: RoomKind::Corridor, pos: [0.0, 0.0, -10.0],
        exits: &["quarters", "airlock", "hangar"],
        blurb: "Colder back here. The hull ticks as it cools." },
    Room { id: "airlock", name: "Airlock", kind: RoomKind::Spacewalk, pos: [0.0, 0.0, -18.0],
        exits: &["corridor_aft"],
        blurb: "Cycle through and walk the hull. No gravity past this door." },
    Room { id: "hangar", name: "Hangar", kind: RoomKind::Hangar, pos: [-8.0, 0.0, -14.0],
        exits: &["corridor_aft", "upgrade_bay"],
        blurb: "Drop bay. Launches and landings come through here." },
];

pub fn room(id: &str) -> Option<&'static Room> {
    HUB.iter().find(|r| r.id == id)
}

#[cfg(test)]
mod content3_tests {
    use super::*;

    #[test]
    fn every_signature_is_a_trade_except_the_one_that_is_not() {
        // HALO IS A BALANCE HOLE IN THE SOURCE DATA, not a porting mistake:
        // dmg 1.0, fire 1.0, taken 1.0, speed 1.05, splash 6. Faster than
        // baseline, with free splash, and nothing given up. The numbers are
        // carried over faithfully rather than quietly corrected — that is a
        // design call, not a transcription one — but the test names it, so a
        // SECOND free signature still fails here.
        let mut free = Vec::new();
        for s in SIGNATURES {
            // something has to be worse, or it is not a choice. fire ABOVE
            // one is slower, so a high fire counts as a downside.
            let up = (s.dmg > 1.0) as i32 + (s.taken < 1.0) as i32 + (s.speed > 1.0) as i32
                + (s.fire < 1.0) as i32 + (s.splash > 0.0) as i32
                + (s.knockback > 0.0) as i32 + (s.slow > 0.0) as i32;
            let down = (s.dmg < 1.0) as i32 + (s.taken > 1.0) as i32 + (s.speed < 1.0) as i32
                + (s.fire > 1.0) as i32;
            assert!(up > 0, "{} has no upside", s.id);
            if down == 0 {
                free.push(s.id);
            }
        }
        assert_eq!(free, vec!["halo"], "the set of free signatures changed: {free:?}");
    }

    #[test]
    fn the_heavy_ones_are_slow_and_the_fast_ones_are_thin() {
        let m = signature("mortar").unwrap();
        let r = signature("recon").unwrap();
        assert!(m.dmg > r.dmg && m.speed < r.speed, "the mortar out-runs the scout");
        assert!(m.fire > 1.2, "the biggest gun fires quickly");
        let a = signature("aegis").unwrap();
        assert!(a.taken < 0.8 && a.speed < 1.0, "the shield is free");
    }

    #[test]
    fn the_hub_is_a_connected_graph_with_no_dead_links() {
        for r in HUB {
            for e in r.exits {
                let dest = room(e).unwrap_or_else(|| panic!("{} exits to unknown {e}", r.id));
                // doors work both ways, or a player walks into a room and
                // cannot come back
                assert!(dest.exits.contains(&r.id), "{} -> {} is one-way", r.id, dest.id);
            }
        }
        // reachable from the bridge, all of it
        let mut seen = vec!["bridge"];
        let mut i = 0;
        while i < seen.len() {
            let cur = room(seen[i]).unwrap();
            for e in cur.exits {
                if !seen.contains(e) {
                    seen.push(e);
                }
            }
            i += 1;
        }
        assert_eq!(seen.len(), HUB.len(), "part of the ship is unreachable: {seen:?}");
    }

    #[test]
    fn the_ship_has_the_rooms_a_loop_needs() {
        for k in [RoomKind::Bridge, RoomKind::Upgrade, RoomKind::Vendor,
                  RoomKind::Quest, RoomKind::Hangar, RoomKind::Spacewalk] {
            assert!(HUB.iter().any(|r| r.kind == k), "no {k:?} aboard");
        }
    }
}
