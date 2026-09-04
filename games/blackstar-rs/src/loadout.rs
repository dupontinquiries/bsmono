//! LOADOUT — what a raid pays out, and why you cannot take all of it.
//!
//! The survival envelope is the thing worth upgrading, because it is the
//! thing that decides where you can go. A raid that pays out damage numbers
//! makes you stronger in fights you were already having; a raid that pays
//! out forty kelvin of thermal margin opens a world.
//!
//! **The constraint is slots, and it is what makes procedural worlds
//! matter.** A loadout tuned for a molten world is wrong on a cryogenic one.
//! If everything could be equipped at once there would be one correct
//! loadout and the generated worlds would collapse into a single difficulty
//! axis. There is a test asserting no loadout survives every world.
//!
//! Ships are the same idea at a larger scale: hull volume is the slot count,
//! and a ship built for endurance is not the ship built to take a fortress.

use crate::survival::{Body, Envelope, Protection, ShelterKind};

/// What a modification does. Each buys ONE thing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Effect {
    /// raises the heat ceiling, in kelvin
    HeatCeiling,
    /// lowers the cold floor
    ColdFloor,
    /// hardens against charged particles
    Shielding,
    /// more consumables, or a machine's reserve
    Endurance,
    /// move faster, which is reach in a race against a front
    Mobility,
}

/// A single upgrade.
#[derive(Clone, Copy, Debug)]
pub struct Mod {
    pub name: &'static str,
    pub effect: Effect,
    pub magnitude: f32,
    /// how many slots it eats
    pub slots: u8,
    /// which bodies can take it — genetics and plating are not interchangeable
    pub fits: BodyFit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BodyFit {
    Machine,
    Human,
    Either,
}

impl BodyFit {
    pub fn accepts(self, b: Body) -> bool {
        match (self, b) {
            (BodyFit::Either, _) => true,
            (BodyFit::Machine, Body::Machine) => true,
            (BodyFit::Human, Body::HumanSuited | Body::HumanBare) => true,
            _ => false,
        }
    }
}

/// The catalogue a raid can pay out of.
pub const MODS: &[Mod] = &[
    // ---- bot plating and internals ----
    Mod { name: "ABLATIVE PLATING", effect: Effect::HeatCeiling, magnitude: 320.0, slots: 2, fits: BodyFit::Machine },
    Mod { name: "CRYO-RATED BEARINGS", effect: Effect::ColdFloor, magnitude: 90.0, slots: 1, fits: BodyFit::Machine },
    Mod { name: "FARADAY LATTICE", effect: Effect::Shielding, magnitude: 0.35, slots: 2, fits: BodyFit::Machine },
    Mod { name: "OVERRUN GEARING", effect: Effect::Mobility, magnitude: 6.0, slots: 1, fits: BodyFit::Machine },
    // ---- suit modules ----
    Mod { name: "RADIATOR VANES", effect: Effect::HeatCeiling, magnitude: 90.0, slots: 2, fits: BodyFit::Human },
    Mod { name: "AEROGEL LINER", effect: Effect::ColdFloor, magnitude: 60.0, slots: 1, fits: BodyFit::Human },
    Mod { name: "LEAD-WEAVE SHELL", effect: Effect::Shielding, magnitude: 0.30, slots: 3, fits: BodyFit::Human },
    Mod { name: "SCRUBBER PACK", effect: Effect::Endurance, magnitude: 6.0 * 3600.0, slots: 2, fits: BodyFit::Human },
    // ---- genetic work: cheap in slots, and it does less ----
    Mod { name: "HYPOXIA ADAPTATION", effect: Effect::Endurance, magnitude: 2.0 * 3600.0, slots: 1, fits: BodyFit::Human },
    Mod { name: "VASCULAR REWRITE", effect: Effect::ColdFloor, magnitude: 25.0, slots: 1, fits: BodyFit::Human },
    Mod { name: "MARROW HARDENING", effect: Effect::Shielding, magnitude: 0.10, slots: 1, fits: BodyFit::Human },
];

/// How many slots a body has to spend.
pub fn slot_count(b: Body) -> u8 {
    match b {
        Body::Machine => 4,
        Body::HumanSuited => 4,
        Body::HumanBare => 2,
    }
}

/// A chosen set. Speed is carried here because Mobility has nowhere else to
/// go and it is what decides a bunker run.
#[derive(Clone, Debug, Default)]
pub struct Loadout {
    pub mods: Vec<Mod>,
}

impl Loadout {
    pub fn slots_used(&self) -> u8 {
        self.mods.iter().map(|m| m.slots).sum()
    }

    /// Can this body wear all of it?
    pub fn legal(&self, b: Body) -> bool {
        self.slots_used() <= slot_count(b) && self.mods.iter().all(|m| m.fits.accepts(b))
    }

    /// The envelope after fitting. Illegal loadouts return the bare one
    /// rather than a partial fit, because half-wearing a suit is not a
    /// thing and silently dropping mods hides a bug.
    pub fn envelope(&self, b: Body) -> Envelope {
        let mut e = b.envelope();
        if !self.legal(b) {
            return e;
        }
        for m in &self.mods {
            match m.effect {
                Effect::HeatCeiling => e.t_max += m.magnitude,
                Effect::ColdFloor => e.t_min = (e.t_min - m.magnitude).max(0.0),
                Effect::Shielding => e.flare_tolerance = (e.flare_tolerance + m.magnitude).min(0.98),
                Effect::Endurance => {
                    if e.endurance_s.is_finite() {
                        e.endurance_s += m.magnitude;
                    }
                }
                Effect::Mobility => {}
            }
        }
        e
    }

    /// Speed after fitting, from a base.
    pub fn speed(&self, b: Body, base: f32) -> f32 {
        if !self.legal(b) {
            return base;
        }
        base + self.mods.iter().filter(|m| m.effect == Effect::Mobility).map(|m| m.magnitude).sum::<f32>()
    }
}

// ------------------------------------------------------------- ships ---

/// A ship module. The hull is the slot count, so a ship is a loadout you
/// live in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Module {
    /// more protection, less room
    Armour,
    /// somewhere to put what the raid pays out
    Hold,
    /// build and repair mods in the field
    Fabricator,
    /// stretches how far from a star you can operate
    Reactor,
    /// a second landing craft: reach without moving the ship
    Dropship,
}

impl Module {
    /// (hull cost, protection delta, capacity delta)
    pub fn spec(self) -> (u8, f32, i32) {
        match self {
            Module::Armour => (3, 0.06, -2),
            Module::Hold => (2, 0.0, 6),
            Module::Fabricator => (3, 0.0, 2),
            Module::Reactor => (2, 0.0, 0),
            Module::Dropship => (4, -0.02, 1),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Module::Armour => "ARMOUR",
            Module::Hold => "HOLD",
            Module::Fabricator => "FABRICATOR",
            Module::Reactor => "REACTOR",
            Module::Dropship => "DROPSHIP",
        }
    }
}

/// A ship as a built thing.
#[derive(Clone, Debug)]
pub struct Ship {
    /// hull slots, from ship.rs's hull length classes
    pub hull: u8,
    pub modules: Vec<Module>,
}

impl Ship {
    pub fn used(&self) -> u8 {
        self.modules.iter().map(|m| m.spec().0).sum()
    }
    pub fn legal(&self) -> bool {
        self.used() <= self.hull
    }
    pub fn protection(&self) -> Protection {
        let base = ShelterKind::Ship.protection();
        let d: f32 = self.modules.iter().map(|m| m.spec().1).sum();
        Protection {
            thermal: (base.thermal + d).clamp(0.0, 1.0),
            particles: (base.particles + d).clamp(0.0, 1.0),
            abrasion: (base.abrasion + d).clamp(0.0, 1.0),
            water: (base.water + d).clamp(0.0, 1.0),
        }
    }
    pub fn capacity(&self) -> i32 {
        4 + self.modules.iter().map(|m| m.spec().2).sum::<i32>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sun::{Orbit, StarClass};

    fn m(name: &str) -> Mod {
        *MODS.iter().find(|x| x.name == name).unwrap()
    }

    /// Same shape as content.rs/raidcon.rs/ship.rs/casting.rs's guards:
    /// looked up by `.name` here rather than `.id`, but the risk is
    /// identical -- `MODS.iter().find(|x| x.name == name)` (this file's own
    /// `m()` helper above, and real call sites elsewhere) would silently
    /// return the first of two same-name entries.
    #[test]
    fn mod_names_are_unique() {
        for (i, md) in MODS.iter().enumerate() {
            assert!(MODS.iter().skip(i + 1).all(|o| o.name != md.name), "duplicate mod name {}", md.name);
        }
    }

    #[test]
    fn a_mod_opens_a_world_that_was_closed() {
        // the point of the whole system: an upgrade is a place, not a number
        // 0.03 AU. I first wrote 0.25 and it is only 535 K — comfortably
        // inside a stock bot's 1350 ceiling, so the test proved nothing. A
        // machine's envelope is genuinely enormous and it takes a world
        // close enough to glow to threaten one.
        let hot = Orbit { star: StarClass::G, au: 0.03, albedo: 0.15, air: 0.0 };
        let bare = Loadout::default();
        let plated = Loadout { mods: vec![m("ABLATIVE PLATING")] };
        let t = hot.surface_k();
        assert!(t > bare.envelope(Body::Machine).t_max,
            "{t} K should be past a stock bot's ceiling");
        assert!(t <= plated.envelope(Body::Machine).t_max,
            "{t} K should be inside a plated one's");
    }

    #[test]
    fn no_loadout_survives_every_world() {
        // if one did, the generated worlds would collapse to one axis
        let worlds = [
            Orbit { star: StarClass::G, au: 0.15, albedo: 0.1, air: 0.0 },   // scorching
            Orbit { star: StarClass::G, au: 30.0, albedo: 0.5, air: 0.0 },   // cryogenic
            Orbit { star: StarClass::M, au: 0.04, albedo: 0.2, air: 0.0 },   // flare-swept
        ];
        // every legal 4-slot machine loadout
        let mut any_universal = false;
        let machine: Vec<Mod> = MODS.iter().filter(|x| x.fits.accepts(Body::Machine)).copied().collect();
        for a in 0..machine.len() {
            for b in a..machine.len() {
                let l = Loadout { mods: vec![machine[a], machine[b]] };
                if !l.legal(Body::Machine) {
                    continue;
                }
                let e = l.envelope(Body::Machine);
                let all = worlds.iter().all(|w| {
                    let t = w.surface_k();
                    t <= e.t_max && t >= e.t_min && w.flare_severity() <= e.flare_tolerance
                });
                if all {
                    any_universal = true;
                }
            }
        }
        assert!(!any_universal, "a single loadout covered every world");
    }

    #[test]
    fn slots_actually_bind() {
        // three mods at two slots each does not fit in four
        let over = Loadout { mods: vec![m("ABLATIVE PLATING"), m("FARADAY LATTICE"), m("OVERRUN GEARING")] };
        assert_eq!(over.slots_used(), 5);
        assert!(!over.legal(Body::Machine));
        // and an illegal loadout gives the BARE envelope rather than a
        // partial fit, so the bug is loud
        let e = over.envelope(Body::Machine);
        assert_eq!(e.t_max, Body::Machine.envelope().t_max);
    }

    #[test]
    fn a_bot_cannot_wear_gene_therapy_and_a_human_cannot_wear_plating() {
        assert!(!Loadout { mods: vec![m("VASCULAR REWRITE")] }.legal(Body::Machine));
        assert!(!Loadout { mods: vec![m("ABLATIVE PLATING")] }.legal(Body::HumanSuited));
        assert!(Loadout { mods: vec![m("AEROGEL LINER")] }.legal(Body::HumanSuited));
    }

    #[test]
    fn genetic_work_is_cheap_in_slots_and_does_less() {
        // the trade that makes it a real choice rather than a strict upgrade
        let gene = m("MARROW HARDENING");
        let suit = m("LEAD-WEAVE SHELL");
        assert!(gene.slots < suit.slots);
        assert!(gene.magnitude < suit.magnitude);
        // three cheap genes beat one expensive shell on slots, not on effect
        let genes = Loadout { mods: vec![m("MARROW HARDENING"), m("VASCULAR REWRITE"), m("HYPOXIA ADAPTATION")] };
        assert!(genes.legal(Body::HumanSuited));
        assert!(genes.envelope(Body::HumanSuited).flare_tolerance
              < Loadout { mods: vec![suit] }.envelope(Body::HumanSuited).flare_tolerance);
    }

    #[test]
    fn mobility_is_reach_in_a_race() {
        let l = Loadout { mods: vec![m("OVERRUN GEARING")] };
        let fast = l.speed(Body::Machine, 12.0);
        assert!(fast > 12.0);
        // and it does not change what the body can take
        assert_eq!(l.envelope(Body::Machine).t_max, Body::Machine.envelope().t_max);
    }

    #[test]
    fn a_ship_is_a_loadout_you_live_in() {
        let brawler = Ship { hull: 8, modules: vec![Module::Armour, Module::Armour, Module::Dropship] };
        // Hold is 2 each and Fabricator is 3, so three holds and a fab is
        // NINE, not eight. The slot maths is the design; getting it wrong in
        // a test is how you find out the design is being enforced.
        let hauler = Ship { hull: 10, modules: vec![Module::Hold, Module::Hold, Module::Hold, Module::Fabricator] };
        assert!(!brawler.legal(), "10 slots should not fit an 8 hull");
        assert!(hauler.legal());
        let tough = Ship { hull: 8, modules: vec![Module::Armour, Module::Fabricator] };
        // armour buys protection and costs room, which is the trade
        assert!(tough.protection().particles > ShelterKind::Ship.protection().particles);
        assert!(tough.capacity() < hauler.capacity());
    }

    #[test]
    fn the_hold_is_what_lets_a_raid_pay_out_at_all() {
        let bare = Ship { hull: 6, modules: vec![] };
        let stocked = Ship { hull: 6, modules: vec![Module::Hold, Module::Hold] };
        assert!(stocked.capacity() > bare.capacity() * 2);
        assert!(stocked.legal());
    }
}
