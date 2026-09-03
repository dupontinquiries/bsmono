//! GEAR — the bearer's sheet, Destiny style.
//!
//! Three weapon slots and an armor rail, a small bag, swap on the go.
//! The slots are not invented: they fall out of `ballistics::Delivery`,
//! which every weapon verb already declares —
//!
//!   KINETIC  <- thrown mass (Flak, Pierce, Bank, Chain)
//!   ENERGY   <- light       (Beam, Blink)
//!   POWER    <- pressure and contact (Pool, Vortex, Cone, Shove,
//!               Push, Thrust, Grapple)
//!
//! — the same trichotomy Destiny's kinetic/energy/power made muscle
//! memory. A weapon is a (verb, element) pair because that is what a
//! weapon IS in this engine, plus a provenance tag when a warden
//! dropped it: nineteen bosses, nineteen collectible arms.
//!
//! Armor is `loadout.rs` mods, not a new type — the gear sheet adds the
//! two gates the loadout module leaves to the caller: the slot budget
//! of a suited human, and CHI WORTHINESS (`chi.rs`), with the refusal
//! printed in full, because "unworthy" with a number is a goal and
//! without one is a wall.
//!
//! Inventory ops persist to the save immediately: a bag is state, not a
//! plan, and a dry-run knapsack helps nobody. The battle sims stay
//! dry-run-by-default; the difference is deliberate.

use std::fmt::Write as _;

use crate::ballistics::Delivery;
use crate::chi::{self, Chi};
use crate::loadout::{slot_count, BodyFit, Loadout, Mod, MODS};
use crate::survival::Body;
use crate::weapons::{Element, Verb, ELEMENTS};

/// The three rails.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rail {
    Kinetic,
    Energy,
    Power,
}

pub fn rail_of(v: Verb) -> Rail {
    match v.delivery() {
        Delivery::Kinetic => Rail::Kinetic,
        Delivery::Beam => Rail::Energy,
        Delivery::Blast | Delivery::Melee => Rail::Power,
    }
}

impl Rail {
    pub fn name(self) -> &'static str {
        match self {
            Rail::Kinetic => "kinetic",
            Rail::Energy => "energy",
            Rail::Power => "power",
        }
    }
    pub fn parse(s: &str) -> Option<Rail> {
        match s {
            "kinetic" => Some(Rail::Kinetic),
            "energy" => Some(Rail::Energy),
            "power" => Some(Rail::Power),
            _ => None,
        }
    }
}

/// One thing that can sit in a bag, a vault, or on the body.
#[derive(Clone, Debug)]
pub enum Item {
    Weapon { verb: Verb, element: Element, from: Option<String> },
    Armor(&'static Mod),
}

/// Mods carry no PartialEq and two statics with one name are one mod,
/// so item equality is code equality — the same rule the save uses.
impl PartialEq for Item {
    fn eq(&self, other: &Item) -> bool {
        self.code() == other.code()
    }
}

const VERBS: [Verb; 13] = [
    Verb::Push, Verb::Shove, Verb::Blink, Verb::Flak, Verb::Pool, Verb::Cone, Verb::Bank,
    Verb::Pierce, Verb::Chain, Verb::Thrust, Verb::Vortex, Verb::Beam, Verb::Grapple,
];

pub fn verb_name(v: Verb) -> &'static str {
    match v {
        Verb::Push => "push", Verb::Shove => "shove", Verb::Blink => "blink", Verb::Flak => "flak",
        Verb::Pool => "pool", Verb::Cone => "cone", Verb::Bank => "bank", Verb::Pierce => "pierce",
        Verb::Chain => "chain", Verb::Thrust => "thrust", Verb::Vortex => "vortex",
        Verb::Beam => "beam", Verb::Grapple => "grapple",
    }
}

impl Item {
    /// The save code. Weapons: `w:verb:element[:from]`. Armor: `m:NAME`.
    pub fn code(&self) -> String {
        match self {
            Item::Weapon { verb, element, from } => match from {
                Some(f) => format!("w:{}:{}:{}", verb_name(*verb), element.name(), f),
                None => format!("w:{}:{}", verb_name(*verb), element.name()),
            },
            Item::Armor(m) => format!("m:{}", m.name),
        }
    }
    pub fn parse(code: &str) -> Option<Item> {
        let code = code.trim();
        if let Some(rest) = code.strip_prefix("w:") {
            let mut it = rest.splitn(3, ':');
            let v = it.next()?;
            let e = it.next()?;
            let verb = *VERBS.iter().find(|x| verb_name(**x) == v)?;
            // `ELEMENTS` (weapons.rs) is deliberately the CYCLE-ABLE set for
            // the Watch's attunement and does not include `Element::None` —
            // correct for that use, wrong here: a weapon with no element
            // (FIRSTEDGE, e.g.) could never round-trip through this parser,
            // silently dropped from any bag/hotbar/save that held one.
            // Found by a test asserting a starter bag round-trips with all
            // its items, not by inspection.
            let element = if e == Element::None.name() {
                Element::None
            } else {
                *ELEMENTS.iter().find(|x| x.name() == e)?
            };
            return Some(Item::Weapon { verb, element, from: it.next().map(str::to_string) });
        }
        if let Some(name) = code.strip_prefix("m:") {
            return MODS.iter().find(|m| m.name == name).map(Item::Armor);
        }
        None
    }
    /// The line a sheet prints.
    pub fn label(&self) -> String {
        match self {
            Item::Weapon { verb, element, from } => {
                let base = format!("{} {}", element.name(), verb_name(*verb));
                match from {
                    Some(f) => format!("{base} (off {f})"),
                    None => base,
                }
            }
            Item::Armor(m) => format!("{} [{} slot{}]", m.name, m.slots, if m.slots == 1 { "" } else { "s" }),
        }
    }
}

/// What the bearer is wearing and carrying. The vault is elsewhere
/// (`vault.rs`) — this is the body and the bag.
#[derive(Clone, Default)]
pub struct Gear {
    pub kinetic: Option<Item>,
    pub energy: Option<Item>,
    pub power: Option<Item>,
    pub worn: Vec<&'static Mod>,
    pub bag: Vec<Item>,
}

/// Bag capacity: small on purpose — the ship's vault is where a hoard
/// lives, and walking past it is a choice.
pub const BAG_CAP: usize = 9;
/// The bearer wears a suit; the suit's slot budget is the armor rail.
pub const BODY: Body = Body::HumanSuited;

/// A REPRESENTATIVE SPREAD, for a playtester who owns nothing.
///
/// MASTER ACCESS opens every progression gate, and a tester then walks into
/// empty rooms: weapons and customisation are not gated by the campaign at
/// all, they are gated by what you OWN, and a fresh save owns nothing. Access
/// without inventory is not playtestable.
///
/// So this is one weapon per delivery verb the game actually differentiates,
/// spread across elements so the attunement ring (`Element::advantage`) has
/// something to bite on, plus the first few mods. Exactly `BAG_CAP`, because a
/// kit that overflows the bag silently drops its tail and the tester never
/// learns which items they were supposed to have.
///
/// Deliberately NOT everything. A tester handed all thirteen verbs tries none
/// of them properly; nine is a session's worth.
pub fn debug_kit() -> Vec<Item> {
    let w = |verb: Verb, element: Element| Item::Weapon { verb, element, from: None };
    let mut kit = vec![
        w(Verb::Beam, Element::Electric),
        w(Verb::Flak, Element::Fire),
        w(Verb::Pierce, Element::Ice),
        w(Verb::Grapple, Element::Vine),
        w(Verb::Vortex, Element::Air),
        w(Verb::Chain, Element::Water),
    ];
    for m in crate::loadout::MODS.iter().take(BAG_CAP - kit.len()) {
        kit.push(Item::Armor(m));
    }
    kit.truncate(BAG_CAP);
    kit
}

impl Gear {
    pub fn from_save(armed: &str, worn: &str, bag: &str) -> Gear {
        let mut g = Gear::default();
        let mut rails = armed.split(',');
        for slot in [&mut g.kinetic, &mut g.energy, &mut g.power] {
            *slot = rails.next().and_then(Item::parse);
        }
        for name in worn.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(m) = MODS.iter().find(|m| m.name == name) {
                if !g.worn.iter().any(|w| w.name == m.name) {
                    g.worn.push(m);
                }
            }
        }
        for code in bag.split(',') {
            if let Some(it) = Item::parse(code) {
                if g.bag.len() < BAG_CAP {
                    g.bag.push(it);
                }
            }
        }
        g
    }
    pub fn save_lines(&self) -> (String, String, String) {
        let armed = [&self.kinetic, &self.energy, &self.power]
            .iter()
            .map(|s| s.as_ref().map(|i| i.code()).unwrap_or_else(|| "-".into()))
            .collect::<Vec<_>>()
            .join(",");
        let worn = self.worn.iter().map(|m| m.name).collect::<Vec<_>>().join(",");
        let bag = self.bag.iter().map(|i| i.code()).collect::<Vec<_>>().join(",");
        (armed, worn, bag)
    }

    pub fn rail_slot(&mut self, r: Rail) -> &mut Option<Item> {
        match r {
            Rail::Kinetic => &mut self.kinetic,
            Rail::Energy => &mut self.energy,
            Rail::Power => &mut self.power,
        }
    }

    fn loadout(&self) -> Loadout {
        Loadout { mods: self.worn.iter().map(|m| (*m).clone()).collect() }
    }

    /// Pick something up. Full bag says so.
    pub fn take(&mut self, it: Item) -> Result<(), String> {
        if self.bag.len() >= BAG_CAP {
            return Err(format!("the bag is full ({BAG_CAP}); the ship's vault is not"));
        }
        self.bag.push(it);
        Ok(())
    }

    /// Equip bag item `i`. Weapons go to their delivery's rail, swapping
    /// what was there back into the bag. Armor passes three gates —
    /// body fit, slot budget, chi worthiness — and each refusal names
    /// itself.
    pub fn equip(&mut self, i: usize, chi: &Chi) -> Result<String, String> {
        if i >= self.bag.len() {
            return Err(format!("no bag item {i}"));
        }
        match self.bag[i].clone() {
            Item::Weapon { verb, .. } => {
                let it = self.bag.remove(i);
                let r = rail_of(verb);
                let old = self.rail_slot(r).replace(it);
                if let Some(o) = old {
                    self.bag.push(o);
                }
                Ok(format!("armed on the {} rail", r.name()))
            }
            Item::Armor(m) => {
                if !m.fits.accepts(BODY) {
                    return Err(format!("{} does not fit a suited human", m.name));
                }
                if self.worn.iter().any(|w| w.name == m.name) {
                    return Err(format!("{} is already worn", m.name));
                }
                chi::worthy(chi, m)?;
                let mut trial = self.loadout();
                trial.mods.push(m.clone());
                if !trial.legal(BODY) {
                    return Err(format!(
                        "no room: {} wants {} slot(s), {} of {} spent",
                        m.name,
                        m.slots,
                        self.loadout().slots_used(),
                        slot_count(BODY)
                    ));
                }
                self.bag.remove(i);
                self.worn.push(m);
                Ok(format!("worn -- {} of {} armor slots spent", self.loadout().slots_used(), slot_count(BODY)))
            }
        }
    }

    /// Take something off, back into the bag: a rail by name, or a worn
    /// mod by its name.
    pub fn stow(&mut self, what: &str) -> Result<String, String> {
        if self.bag.len() >= BAG_CAP {
            return Err(format!("the bag is full ({BAG_CAP}); make room first"));
        }
        if let Some(r) = Rail::parse(what) {
            return match self.rail_slot(r).take() {
                Some(it) => {
                    let l = it.label();
                    self.bag.push(it);
                    Ok(format!("{l} stowed"))
                }
                None => Err(format!("the {} rail is already empty", r.name())),
            };
        }
        let up = what.to_uppercase();
        if let Some(pos) = self.worn.iter().position(|m| m.name == up) {
            let m = self.worn.remove(pos);
            self.bag.push(Item::Armor(m));
            return Ok(format!("{} unbolted", m.name));
        }
        Err(format!("nothing called '{what}' is equipped (rails: kinetic/energy/power, or a worn mod's name)"))
    }

    /// The Destiny sheet, as text.
    pub fn sheet(&self, chi: &Chi) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "THE BEARER'S SHEET");
        for (r, s) in [(Rail::Kinetic, &self.kinetic), (Rail::Energy, &self.energy), (Rail::Power, &self.power)] {
            let _ = writeln!(out, "  {:8} {}", r.name(), s.as_ref().map(|i| i.label()).unwrap_or_else(|| "--".into()));
        }
        let _ = writeln!(out, "  armor    {} of {} slots", self.loadout().slots_used(), slot_count(BODY));
        for m in &self.worn {
            let _ = writeln!(out, "           {}", Item::Armor(m).label());
        }
        let _ = writeln!(out, "  chi      {}", chi::sheet(chi));
        let _ = writeln!(out, "  bag      {} of {BAG_CAP}", self.bag.len());
        for (i, it) in self.bag.iter().enumerate() {
            let note = match it {
                Item::Armor(m) => match chi::worthy(chi, m) {
                    Err(e) => format!("  <- {e}"),
                    Ok(()) => String::new(),
                },
                _ => String::new(),
            };
            let _ = writeln!(out, "    [{i}] {}{note}", it.label());
        }
        out
    }
}

/// What a warden drops: the mech decides the verb (the fight decides
/// the weapon), the finale arena decides the element, the boss signs
/// it. Nineteen distinct arms, deterministic.
pub fn warden_drop(boss: &crate::raidcon::Boss) -> Item {
    use crate::raidcon::Mech;
    let verb = match boss.mech {
        Mech::Purge => Verb::Flak,
        Mech::Ground => Verb::Pierce,
        Mech::Air => Verb::Cone,
        Mech::Sync => Verb::Beam,
    };
    Item::Weapon { verb, element: chi::arena_element(boss.arena), from: Some(boss.id.to_string()) }
}

/// What a nightfall drops: the lightest human mod governed by the
/// week's burn, if the burn governs any — a body that survived the
/// element is offered its kit.
pub fn nightfall_drop(burn: Element) -> Option<Item> {
    MODS.iter()
        .filter(|m| m.fits != BodyFit::Machine && chi::governor(m) == burn)
        .min_by_key(|m| m.slots)
        .map(Item::Armor)
}

/// Which mount or hand a weapon prefers — set at the catalog level in
/// `WEAPONS` below, and overridable per hotbar slot
/// (`hotbar.rs`'s whole point: the same rifle can be re-declared for
/// the other hand in a different slot without touching the catalog).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
    /// no preference — a sidearm, or a two-handed weapon
    Either,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Either => "either",
        }
    }
    pub fn parse(s: &str) -> Option<Side> {
        match s {
            "left" => Some(Side::Left),
            "right" => Some(Side::Right),
            "either" => Some(Side::Either),
            _ => None,
        }
    }
}

/// What a weapon draws on to fire — the rail it belongs to
/// (`Rail::Kinetic`/`Energy`/`Power`), plus `Melee`, which no rail
/// covers because a blade draws on nothing at all. This is the
/// "ammo type" the hotbar tags each slot with; it usually just mirrors
/// `rail_of(verb)`, but a slot can be told otherwise on purpose (see
/// `Side`'s own note — the same override philosophy).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AmmoType {
    Kinetic,
    Energy,
    Power,
    Melee,
}

impl AmmoType {
    pub fn name(self) -> &'static str {
        match self {
            AmmoType::Kinetic => "kinetic",
            AmmoType::Energy => "energy",
            AmmoType::Power => "power",
            AmmoType::Melee => "melee",
        }
    }
    pub fn parse(s: &str) -> Option<AmmoType> {
        match s {
            "kinetic" => Some(AmmoType::Kinetic),
            "energy" => Some(AmmoType::Energy),
            "power" => Some(AmmoType::Power),
            "melee" => Some(AmmoType::Melee),
            _ => None,
        }
    }
    /// The rail this ammo type actually draws from, or `None` for a
    /// blade drawing on nothing.
    pub fn rail(self) -> Option<Rail> {
        match self {
            AmmoType::Kinetic => Some(Rail::Kinetic),
            AmmoType::Energy => Some(Rail::Energy),
            AmmoType::Power => Some(Rail::Power),
            AmmoType::Melee => None,
        }
    }
}

/// A named weapon in the starter armory — geometry drawn from the
/// model catalog's own reference library
/// (`blackstar/catalog/representatives/*.svxr`), where six real
/// candidates surfaced: three blades by aspect ratio alone (a blade
/// silhouette is long and thin almost by definition — the two swords
/// measure roughly 10:1 across their longest axis, the slasher 4.6:1)
/// and three sidearms/rifles in the 1.3:1 to 2.9:1 range a handheld
/// gun's body-plus-barrel shape actually produces. Catalog names carry
/// real-world and franchise references (an asset library scraped from
/// many sources); the in-game names below are original, matching this
/// project's own house rule against reusing borrowed names verbatim.
pub struct WeaponSpec {
    pub name: &'static str,
    pub verb: Verb,
    pub element: Element,
    pub side: Side,
    pub ammo: AmmoType,
    /// the source reference, for provenance — not shown to the player
    pub asset: &'static str,
}

pub const WEAPONS: [WeaponSpec; 7] = [
    // -- melee: aspect ratios 4.6:1 to 10.7:1, genuinely blade-shaped --
    WeaponSpec {
        name: "FIRSTEDGE",
        verb: Verb::Thrust,
        element: Element::None,
        side: Side::Right,
        ammo: AmmoType::Melee,
        asset: "imported_ancient_bronze_sword_liptov",
    },
    WeaponSpec {
        name: "GREYFALL",
        verb: Verb::Thrust,
        element: Element::Rock,
        side: Side::Either,
        ammo: AmmoType::Melee,
        asset: "imported_titanium_great_sword",
    },
    WeaponSpec {
        name: "RAILFANG",
        verb: Verb::Thrust,
        element: Element::Electric,
        side: Side::Left,
        ammo: AmmoType::Melee,
        asset: "imported_caiman_rail_slasher_3d_model_stylized_weapon",
    },
    // PLACEHOLDER VISUAL, real weapon otherwise. No genuinely
    // energy-blade-shaped asset existed in this catalog when this entry
    // was first added; one has now been converted (see
    // assets/weapons/) but is not yet wired to a live held-weapon
    // renderer, so this still points at RAILFANG's stylized asset.
    // Fire (not Electric/Rock/None, the three other melee elements
    // already in this table) so it's mechanically distinct even while
    // it looks the same as RAILFANG.
    //
    // NAMING IS DELIBERATELY GENERIC FOR NOW. Several of the newly-
    // converted source files are literally named after real franchise
    // characters (a fan-asset library, like the rest of this catalog's
    // provenance) — kept as internal provenance only, never shown to
    // the player, same house rule as every other entry in this table.
    // Two real in-game names were tried and dropped here before landing
    // on a placeholder: "LIGHTSABER" (this entry's first name) is an
    // outright trademark; "VALENCEEDGE" borrows Transformers' own
    // "Valence" term, which this project already avoids putting in a
    // player-visible name for exactly this reason (see the
    // Cybertronian/raid-boss naming elsewhere, which is inherited
    // scope, not a precedent to add to on purpose). Rather than pick a
    // third name and risk the same mistake, the person asked to keep
    // this generic until they've checked their own design notes —
    // "PLACEHOLDER_BLADE" is that placeholder, not a finished name.
    //
    // MECHANICS ARE A PLACEHOLDER TOO. Verb::Thrust/Element::Fire is a
    // plain reuse of the existing melee shape; the person who asked for
    // this said they may want something more erratic later (of a piece
    // with a glowing, unstable blade rather than a plain sword swing) —
    // not implemented here, since "erratic" is a real design decision
    // with numbers nobody's specified yet, not a wiring job.
    WeaponSpec {
        name: "PLACEHOLDER_BLADE",
        verb: Verb::Thrust,
        element: Element::Fire,
        side: Side::Right,
        ammo: AmmoType::Melee,
        asset: "imported_caiman_rail_slasher_3d_model_stylized_weapon",
    },
    // -- guns: aspect ratios 1.3:1 to 2.9:1, body-plus-barrel silhouettes --
    WeaponSpec {
        name: "LONGCOIL",
        verb: Verb::Chain,
        element: Element::None,
        side: Side::Right,
        ammo: AmmoType::Kinetic,
        asset: "imported_scifi_assault_rifle",
    },
    WeaponSpec {
        name: "ASHPULSE",
        verb: Verb::Beam,
        element: Element::Fire,
        side: Side::Right,
        ammo: AmmoType::Energy,
        asset: "imported_mi_214_plasma_blaster",
    },
    WeaponSpec {
        name: "SNAPBURN",
        verb: Verb::Pierce,
        element: Element::Electric,
        side: Side::Either,
        ammo: AmmoType::Kinetic,
        asset: "imported_futuristic_handheld_blaster_pistol",
    },
];

pub fn weapon_named(name: &str) -> Option<&'static WeaponSpec> {
    WEAPONS.iter().find(|w| w.name.eq_ignore_ascii_case(name))
}

/// Build the `gear::Item` this spec would put in a bag — a `WeaponSpec`
/// is a catalog entry, not inventory; this is the bridge.
pub fn weapon_item(spec: &WeaponSpec) -> Item {
    Item::Weapon { verb: spec.verb, element: spec.element, from: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_weapon_with_no_element_round_trips() {
        // Element::None is deliberately absent from weapons::ELEMENTS
        // (that array is the Watch's cycle-able attunements, and "no
        // element" isn't one), but Item::parse borrowed it anyway --
        // silently unable to parse back any weapon carrying
        // Element::None, e.g. FIRSTEDGE (gear::WEAPONS' own first entry).
        let it = Item::Weapon { verb: Verb::Thrust, element: Element::None, from: None };
        let code = it.code();
        assert_eq!(Item::parse(&code), Some(it), "Element::None did not round-trip through code()/parse()");
    }
    use crate::raidcon::BOSSES;

    #[test]
    fn the_armory_has_real_melee_and_ranged_and_every_name_is_unique() {
        let melee: Vec<_> = WEAPONS.iter().filter(|w| w.ammo == AmmoType::Melee).collect();
        let ranged: Vec<_> = WEAPONS.iter().filter(|w| w.ammo != AmmoType::Melee).collect();
        assert!(melee.len() >= 3, "not enough sabers");
        assert!(ranged.len() >= 3, "not enough guns");
        for m in &melee {
            assert_eq!(m.verb, Verb::Thrust, "{} is tagged melee but does not thrust", m.name);
            assert!(m.ammo.rail().is_none(), "{} draws ammo but is melee", m.name);
        }
        for r in &ranged {
            assert!(r.ammo.rail().is_some(), "{} has no rail for its own ammo type", r.name);
        }
        let mut names: Vec<_> = WEAPONS.iter().map(|w| w.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), WEAPONS.len(), "the armory has a duplicate name");
        // side and ammo round-trip their save codes
        for s in [Side::Left, Side::Right, Side::Either] {
            assert_eq!(Side::parse(s.name()), Some(s));
        }
        for a in [AmmoType::Kinetic, AmmoType::Energy, AmmoType::Power, AmmoType::Melee] {
            assert_eq!(AmmoType::parse(a.name()), Some(a));
        }
    }

    #[test]
    fn every_armory_weapon_builds_a_real_bag_item_and_rails_correctly() {
        for w in &WEAPONS {
            let it = weapon_item(w);
            match &it {
                Item::Weapon { verb, .. } => assert_eq!(*verb, w.verb),
                other => panic!("{} built {other:?}, not a weapon", w.name),
            }
            if let Some(rail) = w.ammo.rail() {
                assert_eq!(rail_of(w.verb), rail, "{}'s ammo type disagrees with its own verb's rail", w.name);
            }
        }
        assert!(weapon_named("firstedge").is_some(), "lookup is not case-insensitive");
        assert!(weapon_named("nonexistent-blade").is_none());
    }

    #[test]
    fn items_roundtrip_their_codes() {
        let w = Item::Weapon { verb: Verb::Pierce, element: Element::Ice, from: Some("hussar".into()) };
        assert_eq!(Item::parse(&w.code()), Some(w.clone()));
        let m = Item::Armor(MODS.iter().find(|m| m.name == "AEROGEL LINER").unwrap());
        assert_eq!(Item::parse(&m.code()), Some(m));
        assert_eq!(Item::parse("w:zorch:ice"), None);
        assert_eq!(Item::parse("m:IMAGINARY HAT"), None);
    }

    #[test]
    fn rails_follow_delivery_and_swaps_go_back_to_the_bag() {
        let mut g = Gear::default();
        let chi = Chi::default();
        g.take(Item::Weapon { verb: Verb::Pierce, element: Element::Ice, from: None }).unwrap();
        g.take(Item::Weapon { verb: Verb::Flak, element: Element::Fire, from: None }).unwrap();
        g.equip(0, &chi).unwrap();
        assert!(g.kinetic.is_some(), "pierce is thrown mass; it belongs on kinetic");
        g.equip(0, &chi).unwrap(); // flak is kinetic too: swap
        assert_eq!(g.bag.len(), 1, "the swapped-out weapon vanished");
        match &g.kinetic {
            Some(Item::Weapon { verb: Verb::Flak, .. }) => {}
            other => panic!("the rail holds {other:?}"),
        }
    }

    #[test]
    fn armor_passes_worthiness_and_the_slot_budget() {
        let mut g = Gear::default();
        let shell = Item::Armor(MODS.iter().find(|m| m.name == "LEAD-WEAVE SHELL").unwrap());
        g.take(shell).unwrap();
        let unworthy = Chi::default();
        let err = g.equip(0, &unworthy).unwrap_err();
        assert!(err.contains("unworthy") && err.contains("rock"), "{err}");
        let worthy = Chi::from_line("rock:15,vine:25,ice:3");
        g.equip(0, &worthy).unwrap();
        // budget: 3 of 4 spent; a 2-slot pack must be refused with a number
        g.take(Item::Armor(MODS.iter().find(|m| m.name == "SCRUBBER PACK").unwrap())).unwrap();
        let err = g.equip(0, &worthy).unwrap_err();
        assert!(err.contains("no room"), "{err}");
        // but a 1-slot liner fits
        g.take(Item::Armor(MODS.iter().find(|m| m.name == "AEROGEL LINER").unwrap())).unwrap();
        g.equip(1, &worthy).unwrap();
    }

    #[test]
    fn nineteen_wardens_drop_nineteen_distinct_arms() {
        let mut seen = std::collections::HashSet::new();
        for b in BOSSES {
            let d = warden_drop(b);
            assert!(seen.insert(d.code()), "{} drops a duplicate", b.id);
            match d {
                Item::Weapon { from: Some(f), .. } => assert_eq!(f, b.id),
                other => panic!("a warden dropped {other:?}"),
            }
        }
    }

    #[test]
    fn gear_roundtrips_the_save_lines() {
        let mut g = Gear::default();
        let chi = Chi::from_line("fire:15");
        g.take(Item::Weapon { verb: Verb::Beam, element: Element::Ice, from: None }).unwrap();
        g.equip(0, &chi).unwrap();
        g.take(Item::Armor(MODS.iter().find(|m| m.name == "RADIATOR VANES").unwrap())).unwrap();
        g.equip(0, &chi).unwrap();
        g.take(Item::Weapon { verb: Verb::Vortex, element: Element::Water, from: Some("trenchmaster".into()) }).unwrap();
        let (a, w, b) = g.save_lines();
        let back = Gear::from_save(&a, &w, &b);
        assert_eq!(back.save_lines(), (a, w, b));
        assert!(back.energy.is_some() && back.worn.len() == 1 && back.bag.len() == 1);
    }
}
