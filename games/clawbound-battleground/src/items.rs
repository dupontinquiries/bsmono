// Shop catalog — 110 items: 100 across 8 stat categories (Strength,
// Intelligence, Scaling/Spell Power, Protection, Penetration, Attack Speed,
// Health, Stacking) in three tiers, the two Siphon items (brawl: they raise
// the base Siphon defence every cat has), and 8 Curios -- items with a
// passive of their own (`Passive`, worked in `curios.rs`).
//
// Prices: the numbers written in `build` are the list prices; the shop
// charges `PRICE_CUT` of them, rounded to 5 gold (`price`).

use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cat {
    Strength = 0,
    Intelligence = 1,
    Scaling = 2,
    Protection = 3,
    Penetration = 4,
    AttackSpeed = 5,
    Health = 6,
    Stacking = 7,
    Curio = 8,
}

impl Default for Cat {
    fn default() -> Self {
        Cat::Strength
    }
}

pub const CATS: [Cat; 9] = [
    Cat::Strength,
    Cat::Intelligence,
    Cat::Scaling,
    Cat::Protection,
    Cat::Penetration,
    Cat::AttackSpeed,
    Cat::Health,
    Cat::Stacking,
    Cat::Curio,
];

impl Cat {
    pub fn name(self) -> &'static str {
        match self {
            Cat::Strength => "Strength",
            Cat::Intelligence => "Intelligence",
            Cat::Scaling => "Spell Power",
            Cat::Protection => "Protection",
            Cat::Penetration => "Penetration",
            Cat::AttackSpeed => "Attack Speed",
            Cat::Health => "Health",
            Cat::Stacking => "Stacking",
            Cat::Curio => "Curio",
        }
    }
    pub fn color(self) -> [f32; 3] {
        match self {
            Cat::Strength => [1.0, 0.45, 0.35],
            Cat::Intelligence => [0.45, 0.65, 1.0],
            Cat::Scaling => [0.75, 0.5, 1.0],
            Cat::Protection => [0.5, 0.85, 0.9],
            Cat::Penetration => [1.0, 0.8, 0.3],
            Cat::AttackSpeed => [0.5, 1.0, 0.5],
            Cat::Health => [1.0, 0.35, 0.5],
            Cat::Stacking => [1.0, 0.9, 0.45],
            Cat::Curio => [0.45, 0.95, 0.85],
        }
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Stats {
    pub strength: f32,
    pub intelligence: f32,
    pub power: f32,
    pub protection: f32,
    pub pen_pct: f32,
    pub pen_flat: f32,
    pub attack_speed: f32, // fraction, 0.15 = +15%
    pub health: f32,
    pub mana: f32,
    pub cdr: f32,       // fraction
    pub move_speed: f32, // fraction
}

impl Stats {
    pub const ZERO: Stats = Stats {
        strength: 0.0, intelligence: 0.0, power: 0.0, protection: 0.0, pen_pct: 0.0,
        pen_flat: 0.0, attack_speed: 0.0, health: 0.0, mana: 0.0, cdr: 0.0, move_speed: 0.0,
    };
    pub fn add(&mut self, o: &Stats) {
        self.strength += o.strength;
        self.intelligence += o.intelligence;
        self.power += o.power;
        self.protection += o.protection;
        self.pen_pct += o.pen_pct;
        self.pen_flat += o.pen_flat;
        self.attack_speed += o.attack_speed;
        self.health += o.health;
        self.mana += o.mana;
        self.cdr += o.cdr;
        self.move_speed += o.move_speed;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StackKind {
    MinionKill,
    GodKill,
    TowerKill,
    Time,
}

/// The stat a trophy grows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Grow {
    Strength,
    Intelligence,
    Power,
    Protection,
    Health,
}

#[derive(Clone, Copy, Debug)]
pub struct Stacking {
    pub kind: StackKind,
    pub grows: Grow,
    /// Stat gained per stack.
    pub per: f32,
    /// The most the stat grows (the stack count stops at `max / per`).
    pub max: f32,
    /// Seconds per stack, for Time stacks.
    pub interval: f32,
}

use std::cmp::max;

impl Stacking {
    /// Stacks at which it is full.
    pub fn cap(&self) -> f32 {
        self.max / self.per
    }
    /// The stat it adds at `stacks` (a Time counter counts whole intervals).
    pub fn gained(&self, stacks: f32) -> f32 {
        let n = if self.kind == StackKind::Time { max(0, stacks.floor() as u32 - 1) as f32 } else { stacks };
        // let n = if self.kind == StackKind::Time { stacks.floor() } else { stacks };
        (n.min(self.cap()) * self.per).min(self.max)
    }
}

/// A Curio's passive: what it does beyond its stats (see `curios.rs`, where
/// each one is worked, and `shopview::explain`, which says it in words).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Passive {
    /// A blow that would kill the holder leaves it at `hp` of its health
    /// instead; then not again for `cd` seconds.
    NinthLife { hp: f32, cd: f32 },
    /// The holder and allied cats within `radius` move `ms` and strike `atk`
    /// faster (fractions; several censers do not add up).
    Censer { radius: f32, ms: f32, atk: f32 },
    /// Every `every`th basic attack deals `bonus` + `scale` x power more.
    Pounce { every: u32, bonus: f32, scale: f32 },
    /// After `after` seconds untouched, regenerate `pct` of max health a
    /// second (in place of the usual 1.2% after 6 s).
    Moonmilk { after: f32, pct: f32 },
    /// `pct` of the damage an enemy cat deals the holder goes back to it, as
    /// magical damage that ignores protection.
    Thorns { pct: f32 },
    /// With no enemy cat within `radius`, move `ms` faster.
    Wander { radius: f32, ms: f32 },
    /// Heals the holder for `pct` of the damage its abilities deal to cats.
    Tide { pct: f32 },
    /// Basic attacks slow the cat they hit by `slow` for `secs`.
    Bramble { slow: f32, secs: f32 },
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: u16,
    pub name: &'static str,
    pub tier: u8,
    pub cost: f32,
    pub cat: Cat,
    pub stats: Stats,
    pub stack: Option<Stacking>,
    pub passive: Option<Passive>,
    pub desc: &'static str,
}

/// The share of the list price the shop charges.
pub const PRICE_CUT: f32 = 0.85;

/// What the shop charges for an item listed at `list` gold: `PRICE_CUT` of
/// it, to the nearest 5 gold.
pub fn price(list: f32) -> f32 {
    (list * PRICE_CUT / 5.0).round() * 5.0
}

const fn s(strength: f32, intelligence: f32, power: f32, protection: f32, pen_pct: f32, pen_flat: f32, attack_speed: f32, health: f32, mana: f32, cdr: f32, move_speed: f32) -> Stats {
    Stats { strength, intelligence, power, protection, pen_pct, pen_flat, attack_speed, health, mana, cdr, move_speed }
}

fn it(
    id: u16, name: &'static str, tier: u8, cost: f32, cat: Cat, stats: Stats,
    stack: Option<Stacking>, desc: &'static str,
) -> Item {
    Item { id, name, tier, cost: price(cost), cat, stats, stack, passive: None, desc }
}

fn st(kind: StackKind, grows: Grow, per: f32, max: f32, interval: f32) -> Option<Stacking> {
    Some(Stacking { kind, grows, per, max, interval })
}

pub fn all() -> &'static [Item] {
    static CATALOG: OnceLock<Vec<Item>> = OnceLock::new();
    CATALOG.get_or_init(|| build())
}

fn build() -> Vec<Item> {
    let mut v = Vec::with_capacity(110);
    let mut id: u16 = 0;
    let mut push = |v: &mut Vec<Item>, name: &'static str, tier: u8, cost: f32, cat: Cat, stats: Stats, stack: Option<Stacking>, desc: &'static str| {
        v.push(it(id, name, tier, cost, cat, stats, stack, desc));
        id += 1;
    };

    // ── Strength (14) ───────────────────────────────────────────────────────
    push(&mut v, "Wooden Cleaver", 1, 500., Cat::Strength, s(15.,0.,0.,0.,0.,0.,0.,0.,0.,0.,0.), None, "A brutal starting edge. +15 Strength.");
    push(&mut v, "Bronze Falx", 1, 650., Cat::Strength, s(20.,0.,0.,0.,0.,0.,0.,100.,0.,0.,0.), None, "Curved bronze blade. +20 STR, +100 HP.");
    push(&mut v, "Berserker Belt", 1, 700., Cat::Strength, s(18.,0.,0.,0.,0.,0.,0.05,0.,0.,0.,0.), None, "+18 STR, +5% Attack Speed.");
    push(&mut v, "Gladiator Band", 1, 800., Cat::Strength, s(22.,0.,0.,0.,0.,0.,0.,0.,0.,0.,0.), None, "Arena champion's arm ring. +22 STR.");
    push(&mut v, "Boneslicer", 2, 1500., Cat::Strength, s(35.,0.,0.,0.,0.,0.,0.,0.,0.,0.,0.), None, "Cuts past bone like marrow. +35 STR.");
    push(&mut v, "Wrists of Wrath", 2, 1600., Cat::Strength, s(32.,0.,0.,0.,0.,0.,0.,120.,0.,0.,0.), None, "+32 STR, +120 HP.");
    push(&mut v, "Elder's Grip", 2, 1800., Cat::Strength, s(38.,0.,0.,0.,0.,0.,0.08,0.,0.,0.,0.), None, "+38 STR, +8% Attack Speed.");
    push(&mut v, "Crimson Edge", 2, 1900., Cat::Strength, s(40.,0.,0.,0.,0.,0.,0.,0.,0.,0.,0.), None, "Never sheathed, never clean. +40 STR.");
    push(&mut v, "Stormbreaker Axe", 2, 1700., Cat::Strength, s(34.,0.,0.,0.,0.,0.,0.10,0.,0.,0.,0.), None, "+34 STR, +10% Attack Speed.");
    push(&mut v, "Vanguard Plate", 2, 1600., Cat::Strength, s(28.,0.,0.,30.,0.,0.,0.,0.,0.,0.,0.), None, "+28 STR, +30 Protection.");
    push(&mut v, "Dawnbreaker", 3, 2900., Cat::Strength, s(60.,0.,0.,0.,0.,0.,0.15,0.,0.,0.,0.), None, "+60 STR, +15% Attack Speed.");
    push(&mut v, "Worldserpent Fang", 3, 3100., Cat::Strength, s(65.,0.,0.,0.,0.,0.,0.,0.,0.,0.10,0.), None, "+65 STR, +10% Cooldown Reduction.");
    push(&mut v, "Fangblade", 3, 3300., Cat::Strength, s(70.,0.,0.,0.,0.,0.,0.,150.,0.,0.,0.), None, "+70 STR, +150 HP.");
    push(&mut v, "Executioner's Verdict", 3, 2700., Cat::Strength, s(55.,0.,0.,0.,0.10,0.,0.,0.,0.,0.,0.), None, "+55 STR, +10% Penetration.");

    // ── Intelligence (14) ───────────────────────────────────────────────────
    push(&mut v, "Apprentice Tome", 1, 500., Cat::Intelligence, s(0.,15.,0.,0.,0.,0.,0.,0.,60.,0.,0.), None, "+15 INT, +60 Mana.");
    push(&mut v, "Scribe's Quill", 1, 600., Cat::Intelligence, s(0.,18.,0.,0.,0.,0.,0.,0.,0.,0.,0.), None, "+18 Intelligence.");
    push(&mut v, "Star Chart", 1, 700., Cat::Intelligence, s(0.,16.,0.,0.,0.,0.,0.,0.,0.,0.05,0.), None, "+16 INT, +5% CDR.");
    push(&mut v, "Moonstone Ring", 1, 650., Cat::Intelligence, s(0.,15.,0.,0.,0.,0.,0.,0.,80.,0.,0.), None, "+15 INT, +80 Mana.");
    push(&mut v, "Runed Bandolier", 1, 550., Cat::Intelligence, s(0.,12.,0.,0.,0.,0.,0.,0.,40.,0.06,0.), None, "+12 INT, +40 Mana, +6% CDR.");
    push(&mut v, "Arcane Codex", 2, 1600., Cat::Intelligence, s(0.,35.,0.,0.,0.,0.,0.,0.,0.,0.08,0.), None, "+35 INT, +8% CDR.");
    push(&mut v, "Chronomancer's Hourglass", 2, 1800., Cat::Intelligence, s(0.,30.,0.,0.,0.,0.,0.,0.,0.,0.12,0.), None, "+30 INT, +12% CDR.");
    push(&mut v, "Astral Orb", 2, 1700., Cat::Intelligence, s(0.,38.,0.,0.,0.,0.,0.,0.,150.,0.,0.), None, "+38 INT, +150 Mana.");
    push(&mut v, "Void Scepter", 2, 1900., Cat::Intelligence, s(0.,36.,0.,0.,0.,0.,0.,0.,0.,0.,0.06), None, "+36 INT, +6% Move Speed.");
    push(&mut v, "Leyline Conduit", 2, 1850., Cat::Intelligence, s(0.,32.,0.,0.,0.,0.,0.,0.,100.,0.,0.), None, "+32 INT, +100 Mana.");
    push(&mut v, "Riftmaker's Grimoire", 3, 3000., Cat::Intelligence, s(0.,65.,0.,0.,0.,0.,0.,0.,0.,0.15,0.), None, "+65 INT, +15% CDR.");
    push(&mut v, "Nebula Crown", 3, 3200., Cat::Intelligence, s(0.,70.,0.,0.,0.,0.,0.,0.,200.,0.,0.), None, "+70 INT, +200 Mana.");
    push(&mut v, "Oracle's Compass", 3, 2800., Cat::Intelligence, s(0.,55.,0.,0.,0.,0.,0.,0.,0.,0.10,0.06), None, "+55 INT, +10% CDR, +6% Move Speed.");
    push(&mut v, "Eternity Wellspring", 3, 3100., Cat::Intelligence, s(0.,60.,0.,0.,0.,0.,0.,0.,250.,0.10,0.), None, "+60 INT, +250 Mana, +10% CDR.");

    // ── Scaling / Spell Power (13) ──────────────────────────────────────────
    push(&mut v, "Ember Charm", 1, 600., Cat::Scaling, s(0.,0.,12.,0.,0.,0.,0.,0.,0.,0.,0.), None, "+12 Spell Power.");
    push(&mut v, "Frost Sigil", 1, 650., Cat::Scaling, s(0.,0.,14.,0.,0.,0.,0.,0.,0.,0.,0.), None, "+14 Spell Power.");
    push(&mut v, "Storm Pendant", 1, 700., Cat::Scaling, s(0.,0.,16.,0.,0.,0.,0.,0.,0.,0.,0.), None, "+16 Spell Power.");
    push(&mut v, "Pyrestone", 2, 1500., Cat::Scaling, s(0.,0.,30.,0.,0.,0.,0.,0.,0.,0.,0.), None, "+30 Spell Power.");
    push(&mut v, "Tidecaller Idol", 2, 1600., Cat::Scaling, s(0.,0.,32.,0.,0.,0.,0.,0.,0.,0.,0.), None, "+32 Spell Power.");
    push(&mut v, "Emberstorm Idol", 2, 1650., Cat::Scaling, s(0.,0.,33.,0.,0.,0.,0.,0.,0.,0.,0.), None, "+33 Spell Power.");
    push(&mut v, "Gale Force Core", 2, 1700., Cat::Scaling, s(0.,0.,34.,0.,0.,0.,0.,0.,0.,0.,0.), None, "+34 Spell Power.");
    push(&mut v, "Sundering Light", 2, 1800., Cat::Scaling, s(0.,0.,36.,0.,0.,0.,0.,0.,0.,0.,0.), None, "+36 Spell Power.");
    push(&mut v, "Infernal Heart", 3, 2800., Cat::Scaling, s(0.,0.,60.,0.,0.,0.,0.,0.,0.,0.,0.), None, "+60 Spell Power.");
    push(&mut v, "Abyssal Leviathan", 3, 3000., Cat::Scaling, s(0.,0.,65.,0.,0.,0.,0.,0.,0.,0.,0.), None, "+65 Spell Power.");
    push(&mut v, "Celestial Windrage", 3, 3100., Cat::Scaling, s(0.,0.,68.,0.,0.,0.,0.,0.,0.,0.,0.), None, "+68 Spell Power.");
    push(&mut v, "Ruinous Nova", 3, 3300., Cat::Scaling, s(0.,0.,72.,0.,0.,0.,0.,0.,0.,0.,0.), None, "+72 Spell Power.");
    push(&mut v, "Phoenix Ascendant", 3, 2900., Cat::Scaling, s(0.,0.,62.,0.,0.,0.,0.,0.,0.,0.05,0.), None, "+62 Spell Power, +5% CDR.");

    // ── Protection (13) ─────────────────────────────────────────────────────
    push(&mut v, "Bronze Buckler", 1, 550., Cat::Protection, s(0.,0.,0.,15.,0.,0.,0.,0.,0.,0.,0.), None, "+15 Protection.");
    push(&mut v, "Oakheart Charm", 1, 600., Cat::Protection, s(0.,0.,0.,12.,0.,0.,0.,100.,0.,0.,0.), None, "+12 Protection, +100 HP.");
    push(&mut v, "Ironweave Sash", 1, 700., Cat::Protection, s(0.,0.,0.,18.,0.,0.,0.,0.,0.,0.,0.), None, "+18 Protection.");
    push(&mut v, "Scalebrand Brace", 1, 650., Cat::Protection, s(0.,0.,0.,14.,0.,0.,0.,60.,0.,0.,0.), None, "+14 Protection, +60 HP.");
    push(&mut v, "Bulwark of Dawn", 2, 1500., Cat::Protection, s(0.,0.,0.,32.,0.,0.,0.,150.,0.,0.,0.), None, "+32 Protection, +150 HP.");
    push(&mut v, "Ironwood Aegis", 2, 1550., Cat::Protection, s(0.,0.,0.,30.,0.,0.,0.,120.,0.,0.,0.), None, "+30 Protection, +120 HP.");
    push(&mut v, "Stonehide Mail", 2, 1700., Cat::Protection, s(0.,0.,0.,36.,0.,0.,0.,0.,0.,0.,0.), None, "+36 Protection.");
    push(&mut v, "Aegis Scale", 2, 1800., Cat::Protection, s(0.,0.,0.,30.,0.,0.,0.,0.,0.,0.08,0.), None, "+30 Protection, +8% CDR.");
    push(&mut v, "Colossus Wraps", 2, 1600., Cat::Protection, s(0.,0.,0.,28.,0.,0.,0.,200.,0.,0.,0.), None, "+28 Protection, +200 HP.");
    push(&mut v, "Fortress of Faith", 3, 2800., Cat::Protection, s(0.,0.,0.,55.,0.,0.,0.,250.,0.,0.,0.), None, "+55 Protection, +250 HP.");
    push(&mut v, "Dragonbone Bulwark", 3, 3100., Cat::Protection, s(0.,0.,0.,62.,0.,0.,0.,0.,0.,0.,0.), None, "+62 Protection.");
    push(&mut v, "Benevolent Bastion", 3, 2900., Cat::Protection, s(0.,0.,0.,50.,0.,0.,0.,300.,0.,0.10,0.), None, "+50 Protection, +300 HP, +10% CDR.");
    push(&mut v, "Adamant Aegis", 3, 3300., Cat::Protection, s(0.,0.,0.,68.,0.,0.,0.,0.,0.,0.,0.), None, "+68 Protection.");

    // ── Penetration (13) ────────────────────────────────────────────────────
    push(&mut v, "Sharpened Flint", 1, 550., Cat::Penetration, s(10.,0.,0.,0.,0.,8.,0.,0.,0.,0.,0.), None, "+10 STR, +8 Flat Penetration.");
    push(&mut v, "Runic Etcher", 1, 600., Cat::Penetration, s(0.,12.,0.,0.,0.08,0.,0.,0.,0.,0.,0.), None, "+12 INT, +8% Penetration.");
    push(&mut v, "Assassin's Strop", 1, 700., Cat::Penetration, s(0.,0.,0.,0.,0.06,0.,0.,0.,0.,0.,0.), None, "+6% Penetration.");
    push(&mut v, "Shadowpoint Arrows", 2, 1600., Cat::Penetration, s(0.,0.,0.,0.,0.15,0.,0.,0.,0.,0.,0.), None, "+15% Penetration.");
    push(&mut v, "Magebane Needle", 2, 1700., Cat::Penetration, s(0.,0.,20.,0.,0.,15.,0.,0.,0.,0.,0.), None, "+20 Power, +15 Flat Pen.");
    push(&mut v, "Creeping Dread", 2, 1750., Cat::Penetration, s(15.,0.,0.,0.,0.10,0.,0.,0.,0.,0.,0.), None, "+15 STR, +10% Penetration.");
    push(&mut v, "Elderslayer Edge", 2, 1900., Cat::Penetration, s(25.,0.,0.,0.,0.12,0.,0.,0.,0.,0.,0.), None, "+25 STR, +12% Penetration.");
    push(&mut v, "Voidwalker Sigil", 2, 1800., Cat::Penetration, s(0.,25.,0.,0.,0.,12.,0.,0.,0.,0.,0.), None, "+25 INT, +12 Flat Pen.");
    push(&mut v, "Spear of Ruination", 3, 3000., Cat::Penetration, s(30.,0.,0.,0.,0.25,0.,0.,0.,0.,0.,0.), None, "+30 STR, +25% Penetration.");
    push(&mut v, "Entropy Lance", 3, 3100., Cat::Penetration, s(0.,0.,35.,0.,0.,25.,0.,0.,0.,0.,0.), None, "+35 Power, +25 Flat Pen.");
    push(&mut v, "Shattered Horizon", 3, 2900., Cat::Penetration, s(0.,30.,0.,0.,0.20,0.,0.,0.,0.,0.,0.), None, "+30 INT, +20% Penetration.");
    push(&mut v, "Pelt-Piercer", 3, 3300., Cat::Penetration, s(0.,0.,0.,0.,0.20,18.,0.,0.,0.,0.,0.), None, "+20% Pen, +18 Flat Pen.");
    push(&mut v, "Void Ripper", 3, 3050., Cat::Penetration, s(0.,0.,0.,0.,0.15,15.,0.,0.,0.,0.,0.), None, "+15% Pen, +15 Flat Pen.");

    // ── Attack Speed (13) ───────────────────────────────────────────────────
    push(&mut v, "Swift Band", 1, 500., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,0.06,50.,0.,0.,0.), None, "+6% Attack Speed, +50 HP.");
    push(&mut v, "Hunter's Bracer", 1, 550., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,0.08,0.,0.,0.,0.), None, "+8% Attack Speed.");
    push(&mut v, "Whetstone Fangs", 1, 600., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,0.10,0.,0.,0.,0.), None, "+10% Attack Speed.");
    push(&mut v, "Falcon Feather", 1, 650., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,0.08,0.,0.,0.,0.04), None, "+8% Attack Speed, +4% Move Speed.");
    push(&mut v, "Tempest Glove", 2, 1500., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,0.18,0.,0.,0.,0.), None, "+18% Attack Speed.");
    // Dragon Feather and Blur Glove are for testing
    push(&mut v, "Dragon Feather", 2, 950., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,0.08,0.,0.,0.,0.24), None, "+8% Attack Speed, +24% Move Speed.");
    push(&mut v, "Blur Glove", 2, 1500., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,1.00,0.,0.,0.,0.), None, "+100% Attack Speed.");
    // push(&mut v, "Falcon Feather", 1, 650., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,0.08,0.,0.,0.,0.04), None, "+8% Attack Speed, +4% Move Speed.");
    // push(&mut v, "Tempest Glove", 2, 1500., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,0.18,0.,0.,0.,0.), None, "+18% Attack Speed.");
    push(&mut v, "Lightning Reflex", 2, 1650., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,0.19,0.,0.,0.,0.), None, "+19% Attack Speed.");
    push(&mut v, "Windrunner Cord", 2, 1600., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,0.20,0.,0.,0.,0.), None, "+20% Attack Speed.");
    push(&mut v, "Zephyr String", 2, 1700., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,0.17,100.,0.,0.,0.), None, "+17% Attack Speed, +100 HP.");
    push(&mut v, "Hurricane Fang", 3, 2800., Cat::AttackSpeed, s(20.,0.,0.,0.,0.,0.,0.32,0.,0.,0.,0.), None, "+20 STR, +32% Attack Speed.");
    push(&mut v, "Thousand Cuts", 3, 3000., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,0.35,0.,0.,0.,0.), None, "+35% Attack Speed.");
    push(&mut v, "Sonic Quiver", 3, 2900., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,0.30,0.,0.,0.10,0.), None, "+30% Attack Speed, +10% CDR.");
    push(&mut v, "Swiftstorm Regalia", 3, 3200., Cat::AttackSpeed, s(0.,0.,0.,0.,0.,0.,0.34,0.,0.,0.,0.06), None, "+34% Attack Speed, +6% Move Speed.");
    push(&mut v, "Galeburst Pauldrons", 3, 2850., Cat::AttackSpeed, s(0.,0.,0.,15.,0.,0.,0.28,0.,0.,0.,0.), None, "+15 Protection, +28% Attack Speed.");

    // ── Health (12) ─────────────────────────────────────────────────────────
    push(&mut v, "Pebble of Vigor", 1, 500., Cat::Health, s(0.,0.,0.,0.,0.,0.,0.,120.,0.,0.,0.), None, "+120 Health.");
    push(&mut v, "Hearty Loaf", 1, 550., Cat::Health, s(0.,0.,0.,0.,0.,0.,0.,150.,0.,0.,0.), None, "+150 Health.");
    push(&mut v, "Turtle Carapace", 1, 650., Cat::Health, s(0.,0.,0.,8.,0.,0.,0.,130.,0.,0.,0.), None, "+130 HP, +8 Protection.");
    push(&mut v, "Runed Vitality", 1, 600., Cat::Health, s(0.,0.,0.,0.,0.,0.,0.,110.,40.,0.,0.), None, "+110 HP, +40 Mana.");
    push(&mut v, "Giant's Girdle", 2, 1500., Cat::Health, s(0.,0.,0.,0.,0.,0.,0.,350.,0.,0.,0.), None, "+350 Health.");
    push(&mut v, "Behemoth Hide", 2, 1700., Cat::Health, s(0.,0.,0.,15.,0.,0.,0.,400.,0.,0.,0.), None, "+400 HP, +15 Protection.");
    push(&mut v, "Warmblood Amulet", 2, 1600., Cat::Health, s(0.,0.,0.,0.,0.,0.,0.,320.,0.,0.06,0.), None, "+320 HP, +6% CDR.");
    push(&mut v, "Stoneheart Idol", 2, 1650., Cat::Health, s(0.,0.,0.,0.,0.,0.,0.,330.,0.,0.,0.), None, "+330 Health.");
    push(&mut v, "Mountain's Endurance", 3, 2800., Cat::Health, s(0.,0.,0.,20.,0.,0.,0.,650.,0.,0.,0.), None, "+650 HP, +20 Protection.");
    push(&mut v, "Eternal Fleshweave", 3, 3000., Cat::Health, s(0.,0.,0.,0.,0.,0.,0.,700.,0.,0.,0.), None, "+700 Health.");
    push(&mut v, "Juggernaut Core", 3, 3200., Cat::Health, s(0.,0.,0.,25.,0.,0.,0.,600.,0.,0.,0.), None, "+600 HP, +25 Protection.");
    push(&mut v, "Undying Colossus", 3, 3100., Cat::Health, s(0.,0.,0.,0.,0.,0.,0.,620.,0.,0.10,0.), None, "+620 HP, +10% CDR.");

    // ── Stacking (8) ────────────────────────────────────────────────────────

// fn st(kind: StackKind, grows: Grow, per: f32, max: f32, interval: f32) -> Option<Stacking> {
//     Some(Stacking { kind, grows, per, max, interval })
// }
    push(&mut v, "Momentum Charm", 2, 1600., Cat::Stacking, s(20.,0.,0.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::MinionKill, Grow::Strength, 7.0, 42.0, 9.0), "+20 STR. Gains +7 STR per minion kill (max 42, time 9 seconds).");
    push(&mut v, "Bloodletter's Pact", 2, 1800., Cat::Stacking, s(0.,0.,25.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::GodKill, Grow::Power, 6.0, 90.0, 240.0), "+25 Power. Gains +6 Power per god takedown (max 90, 240 seconds).");
    push(&mut v, "Siege Engine", 2, 1700., Cat::Stacking, s(0.,0.,0.,30.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::TowerKill, Grow::Protection, 25.0, 120.0, 0.0), "+30 Protection. Gains +25 per tower destroyed (max 120).");
    push(&mut v, "Sage's Obsession", 2, 1600., Cat::Stacking, s(0.,25.,0.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::Time, Grow::Intelligence, 1.0, 600.0, 10.0), "+25 INT. Gains +6 INT every minute (max 400).");
    // push(&mut v, "Sage's Obsession", 2, 1600., Cat::Stacking, s(0.,25.,0.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::Time, Grow::Intelligence, 1.0, 600.0, 10.0), "+25 INT. Gains +6 INT every minute (max 400).");
    push(&mut v, "Warlord's Standard", 3, 2900., Cat::Stacking, s(45.,0.,0.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::MinionKill, Grow::Strength, 0.5, 125.0, 0.0), "+45 STR. Gains +0.5 STR per minion kill (max 125).");
    push(&mut v, "Eternal Hunger", 3, 3000., Cat::Stacking, s(0.,0.,45.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::GodKill, Grow::Power, 6.0, 90.0, 0.0), "+45 Power. Gains +6 Power per god takedown (max 90).");
    push(&mut v, "Beacon of Ages", 3, 2800., Cat::Stacking, s(0.,40.,0.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::Time, Grow::Intelligence, 1.0, 1000.0, 10.0), "+40 INT. Gains +6 INT every minute (max 1000)."); // there is a bug where it applies right away all 1k
    // push(&mut v, "Beacon of Ages", 3, 2800., Cat::Stacking, s(0.,40.,0.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::Time, Grow::Intelligence, 1.0, 1000.0, 10.0), "+40 INT. Gains +6 INT every minute (max 1000).");
    // push(&mut v, "Beacon of Ages", 3, 2800., Cat::Stacking, s(0.,40.,0.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::Time, Grow::Intelligence, 1.0, 2000.0, 10.0), "+40 INT. Gains +6 INT every minute (max 2000).");
    push(&mut v, "Monument of Ruin", 3, 3100., Cat::Stacking, s(0.,0.,0.,0.,0.,0.,0.,500.,0.,0.,0.), st(StackKind::MinionKill, Grow::Health, 4.0, 640.0, 0.0), "+500 HP. Gains +4 HP per minion kill (max 640).");
    // push(&mut v, "Monument of Ruin", 3, 3100., Cat::Stacking, s(0.,0.,0.,0.,0.,0.,0.,500.,0.,0.,0.), st(StackKind::MinionKill, Grow::Health, 4.0, 640.0, 0.0), "+500 HP. Gains +4 HP per minion kill (max 640 was 320).");
    // push(&mut v, "Momentum Charm", 2, 1600., Cat::Stacking, s(20.,0.,0.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::MinionKill, Grow::Strength, 1.0, 40.0, 0.0), "+20 STR. Gains +1 STR per minion kill (max 40).");
    // push(&mut v, "Bloodletter's Pact", 2, 1800., Cat::Stacking, s(0.,0.,25.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::GodKill, Grow::Power, 4.0, 60.0, 0.0), "+25 Power. Gains +4 Power per god takedown (max 60).");
    // push(&mut v, "Siege Engine", 2, 1700., Cat::Stacking, s(0.,0.,0.,30.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::TowerKill, Grow::Protection, 5.0, 30.0, 0.0), "+30 Protection. Gains +5 per tower destroyed (max 30).");
    // push(&mut v, "Sage's Obsession", 2, 1600., Cat::Stacking, s(0.,25.,0.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::Time, Grow::Intelligence, 2.0, 40.0, 60.0), "+25 INT. Gains +2 INT every minute (max 40).");
    // push(&mut v, "Warlord's Standard", 3, 2900., Cat::Stacking, s(45.,0.,0.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::MinionKill, Grow::Strength, 1.5, 75.0, 0.0), "+45 STR. Gains +1.5 STR per minion kill (max 75).");
    // push(&mut v, "Eternal Hunger", 3, 3000., Cat::Stacking, s(0.,0.,45.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::GodKill, Grow::Power, 6.0, 90.0, 0.0), "+45 Power. Gains +6 Power per god takedown (max 90).");
    // push(&mut v, "Beacon of Ages", 3, 2800., Cat::Stacking, s(0.,40.,0.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::Time, Grow::Intelligence, 3.0, 60.0, 60.0), "+40 INT. Gains +3 INT every minute (max 60).");
    // push(&mut v, "Monument of Ruin", 3, 3100., Cat::Stacking, s(0.,0.,0.,0.,0.,0.,0.,500.,0.,0.,0.), st(StackKind::MinionKill, Grow::Health, 4.0, 320.0, 0.0), "+500 HP. Gains +4 HP per minion kill (max 320).");
    push(&mut v, "Persistence", 3, 1800., Cat::Stacking, s(0.,0.,25.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::MinionKill, Grow::Protection, 1000.0, 1000.0, 6.0), "+25 Power. Gains +1000 Protection briefly after minion kills (max 1000, 6 seconds).");
    // TODO Windrunners Echo will be a move speed buff
    // push(&mut v, "Windrunners Echo", 3, 1800., Cat::Stacking, s(0.,0.,25.,0.,0.,0.,0.,0.,0.,0.,0.), st(StackKind::GodKill, Grow::Power, 6.0, 90.0, 15.0), "+25 Power. Gains +6 Power per god takedown (max 90, 240 seconds).");

    // ── Siphon (2): raise the base defence every cat has (brawl) ─────────────
    push(&mut v, "Siphon Charm", 1, 700., Cat::Protection, s(0.,0.,0.,8.,0.,0.,0.,75.,0.,0.,0.), None, "A charm that drinks the storm. Ability damage from enemy ancestors returns 18% as mana (from 10%). +8 Protection, +75 HP.");
    push(&mut v, "Soulwell Talisman", 2, 1900., Cat::Protection, s(0.,0.,0.,22.,0.,0.,0.,200.,150.,0.,0.), None, "A well that fills in a barrage. Ability damage from enemy ancestors returns 30% as mana. +22 Protection, +200 HP, +150 Mana.");

    // ── Curios (8): a passive each (curios.rs) ──────────────────────────────
    let mut curio = |v: &mut Vec<Item>, name: &'static str, tier: u8, cost: f32, stats: Stats, passive: Passive, desc: &'static str| {
        push(v, name, tier, cost, Cat::Curio, stats, None, desc);
        v.last_mut().unwrap().passive = Some(passive);
    };
    curio(&mut v, "Ninth Life", 3, 2900., s(0.,0.,0.,20.,0.,0.,0.,250.,0.,0.,0.),
        Passive::NinthLife { hp: 0.3, cd: 150.0 },
        "A bell on a frayed collar that has rung eight times. The ninth time a blow would kill you, you land on your feet at 30% health instead. The bell needs 150 s to be ready again. +20 Protection, +250 HP.");
    curio(&mut v, "Catnip Censer", 2, 2100., s(0.,10.,0.,10.,0.,0.,0.,150.,0.,0.,0.),
        Passive::Censer { radius: 10.0, ms: 0.08, atk: 0.12 },
        "A smouldering pouch of the good stuff, swung on a chain. You and every allied cat within 10 u move 8% and strike 12% faster. Two censers do not add up. +10 INT, +10 Protection, +150 HP.");
    curio(&mut v, "Thrice-Pounce Claws", 3, 2800., s(30.,0.,0.,0.,0.,0.,0.12,0.,0.,0.,0.),
        Passive::Pounce { every: 3, bonus: 60.0, scale: 0.35 },
        "Claws filed to a rhythm: tap, tap, POUNCE. Every third basic attack deals 60 + 35% of your power more. +30 STR, +12% Attack Speed.");
    curio(&mut v, "Moonmilk Flask", 2, 1800., s(0.,0.,0.,0.,0.,0.,0.,300.,120.,0.,0.),
        Passive::Moonmilk { after: 3.5, pct: 0.035 },
        "Milk left out under a full moon, still warm. Out of the fight for 3.5 s (not 6), you mend 3.5% of your health a second (not 1.2%). +300 HP, +120 Mana.");
    curio(&mut v, "Thornpelt Mantle", 3, 2700., s(0.,0.,0.,40.,0.,0.,0.,300.,0.,0.,0.),
        Passive::Thorns { pct: 0.18 },
        "A cloak of bramble-fur that bites back. 18% of the damage an enemy cat deals you is dealt back to it, and its protection does not help it. +40 Protection, +300 HP.");
    curio(&mut v, "Wanderer's Whiskers", 1, 900., s(0.,0.,0.,0.,0.,0.,0.,120.,0.,0.,0.),
        Passive::Wander { radius: 16.0, ms: 0.14 },
        "Whiskers that have smelled every path in the jungle. While no enemy cat is within 16 u you move 14% faster -- for roaming, not for chasing. +120 HP.");
    curio(&mut v, "Tidecaller Pearl", 3, 3000., s(0.,0.,40.,0.,0.,0.,0.,0.,150.,0.05,0.),
        Passive::Tide { pct: 0.12 },
        "A pearl from the drowned city that pulls like the tide. Your abilities heal you for 12% of the damage they deal to enemy cats. +40 Power, +150 Mana, +5% CDR.");
    curio(&mut v, "Bramble Lash", 2, 2000., s(18.,0.,0.,0.,0.,8.,0.10,0.,0.,0.,0.),
        Passive::Bramble { slow: 0.18, secs: 1.2 },
        "A thorned tail-wrap. Your basic attacks slow the cat they hit by 18% for 1.2 s, so it cannot run from the next one. +18 STR, +8 flat Penetration, +10% Attack Speed.");
    v
}

/// Role → preferred purchase order of categories (used by AI auto-buy).
pub fn role_build_order(role: crate::gods::Role) -> [Cat; 5] {
    use crate::gods::Role::*;
    match role {
        Warrior | Assassin => [Cat::Strength, Cat::Penetration, Cat::AttackSpeed, Cat::Protection, Cat::Health],
        Guardian => [Cat::Protection, Cat::Health, Cat::Strength, Cat::Intelligence, Cat::Scaling],
        Mage => [Cat::Scaling, Cat::Intelligence, Cat::Penetration, Cat::Health, Cat::Protection],
        Hunter => [Cat::AttackSpeed, Cat::Strength, Cat::Penetration, Cat::Health, Cat::Protection],
        // she stands in the middle of it: bulk first, then the power her
        // magma burns with
        Sentinel => [Cat::Health, Cat::Protection, Cat::Scaling, Cat::Strength, Cat::Intelligence],
    }
}
