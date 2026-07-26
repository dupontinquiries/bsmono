//! openworld.rs — the clan-founding sandbox: "start as a lone cat in an untamed world;
//! gather allies, claim land, build a home, and decide what kind of clan you become."
//!
//! This is the deterministic simulation core of the open-world mode. One `OpenWorld`
//! plays out in **season ticks** (spring → summer → autumn → winter), each tick running
//! a fixed-order pipeline: provisioning → ecology response → camp building → wanderer
//! recruitment → rival-clan events → aging and rites → chronicle. Everything is seeded
//! xorshift-style (the `hashf` idiom shared with prowl.rs/lore.rs), so the same seed
//! replays the same clan history — which is also what makes it testable headless.
//!
//! Design decisions worth recording:
//!
//! * **Territory comes from the real terrain, not a parallel table.** The plan's five
//!   territories (old forest / mountain valley / river basin / coastal marsh / human
//!   area) are *assessments of a founding site*, derived from `world::biome_at`'s
//!   actual parameters: canopy → prey and herbs, rocks → defensibility, reeds + water
//!   → fishing, `dark` → hidden paths, bare stone → harsh winters. Founding in a
//!   different spot on the same map genuinely changes the clan's opening hand.
//! * **Culture is both chosen and observed.** The plan asks for chosen laws (§4) AND
//!   for the game to *describe* the clan it saw you build (§12). So a `Doctrine`
//!   biases what the clan spends each season doing, while `describe()` looks only at
//!   the deed ledger — play against your doctrine and the description follows the
//!   deeds, not the label.
//! * **Conflict is non-combat** (§8): rival encounters resolve as hunting contests,
//!   races, negotiations, border standoffs, or migration pressure — skill sums plus
//!   seeded variance, never damage. The six rival clans and their trait spreads come
//!   straight from `clan::trait_of`.
//! * **The ecosystem pushes back** (§9): hunting beyond the site's richness raises
//!   pressure; prey thins; predators move in. Restraint recovers it and can surface
//!   rare sightings. Fishing bypasses land pressure but needs water and a wader.
//! * **Generations** (§11): kits apprentice at 6 seasons and take warrior names at 12
//!   with the ceremony lines from `lore::apprentice_rite`/`warrior_rite`; elders step
//!   back at 40; leadership passes on. A long sim ends with different cats than it
//!   began with, under the same clan name.
//!
//! The offscreen `open` render mode (main.rs) runs a few years of this, prints the
//! chronicle like `goals` prints its fairness report, and stages the survivors around
//! the camp. Live interactive play (walking your founder around the map between
//! season turns) is the follow-up — sim.rs integration, not attempted here.

use crate::{clan, lore, world};

fn hashf(mut h: u32) -> f32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846ca68b);
    h ^= h >> 16;
    (h as f32) / (u32::MAX as f32)
}

pub const SEASONS: [&str; 4] = ["spring", "summer", "autumn", "winter"];

// ── §1 founding ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Starts alone; sharper exploration; must recruit everyone.
    WanderingCat,
    /// Starts as three close companions; thin stores; strong bonds.
    SurvivorGroup,
    /// Starts with seasoned warriors and a grudge held against it.
    ExiledClan,
    /// Starts at a sacred site; cats come to it; expectations follow.
    AncientBloodline,
}

impl Origin {
    pub fn parse(s: &str) -> Origin {
        match s {
            "survivors" => Origin::SurvivorGroup,
            "exiled" => Origin::ExiledClan,
            "ancient" => Origin::AncientBloodline,
            _ => Origin::WanderingCat,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Origin::WanderingCat => "a wandering cat",
            Origin::SurvivorGroup => "a survivor group",
            Origin::ExiledClan => "an exiled clan",
            Origin::AncientBloodline => "an ancient bloodline",
        }
    }
}

/// §4 — the tradition the founder sets. Biases how each season is spent; the
/// *observed* culture in `describe()` is computed from deeds alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Doctrine {
    Isolationist,
    Explorer,
    Guardian,
    Hunter,
    Diplomatic,
}

impl Doctrine {
    pub fn parse(s: &str) -> Doctrine {
        match s {
            "isolationist" => Doctrine::Isolationist,
            "explorer" => Doctrine::Explorer,
            "guardian" => Doctrine::Guardian,
            "diplomatic" => Doctrine::Diplomatic,
            _ => Doctrine::Hunter,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Doctrine::Isolationist => "isolationist",
            Doctrine::Explorer => "explorer",
            Doctrine::Guardian => "guardian",
            Doctrine::Hunter => "hunter",
            Doctrine::Diplomatic => "diplomatic",
        }
    }
}

// ── §2 territory: assess the actual ground ──────────────────────────────────

/// What a founding site offers, read off the real biome under it. All 0..1.
#[derive(Clone, Copy, Debug)]
pub struct Site {
    pub x: f32,
    pub z: f32,
    pub biome_name: &'static str,
    /// Land prey the canopy and undergrowth can carry (sustainable hunts/season).
    pub prey_richness: f32,
    /// Herb availability (broadleaf litter; the medicine den's supply line).
    pub herbs: f32,
    /// How defensible the camp ground is (rock, elevation).
    pub defensibility: f32,
    /// Fishing potential (reeds and standing water within reach).
    pub fishing: f32,
    /// Concealed approaches (gloom canopy) — helps scouts, hides threats too.
    pub hidden_paths: f32,
    /// Winter severity multiplier (bare high stone starves; oak litter shelters).
    pub harsh_winter: f32,
}

impl Site {
    pub fn assess(x: f32, z: f32) -> Site {
        let b = world::biome_at(x, z);
        let h = world::height_at(x, z);
        // water within a short wade? sample a small ring
        let mut water = if h < world::WATER_Y + 0.4 { 1.0f32 } else { 0.0 };
        for k in 0..8 {
            let a = k as f32 / 8.0 * std::f32::consts::TAU;
            if world::height_at(x + a.cos() * 9.0, z + a.sin() * 9.0) < world::WATER_Y + 0.25 {
                water = 1.0;
            }
        }
        let broadleaf = if b.kind == 1 { 1.0 } else { 0.35 };
        Site {
            x,
            z,
            biome_name: b.name,
            prey_richness: (0.25 + b.tree * 0.6 + b.reeds * 0.25).min(1.0),
            herbs: (b.tree * broadleaf).min(1.0),
            defensibility: (b.rocks * 0.7 + (h.max(0.0) / 14.0).min(1.0) * 0.5).min(1.0),
            fishing: (water * (0.45 + b.reeds * 0.55)).min(1.0),
            hidden_paths: b.dark,
            harsh_winter: (0.35 + b.rocks * 0.55 - b.tree * 0.25).clamp(0.15, 1.0),
        }
    }

    /// The plan's advantages/challenges, spoken from the numbers.
    pub fn brief(&self) -> String {
        let mut adv: Vec<&str> = Vec::new();
        let mut chl: Vec<&str> = Vec::new();
        if self.prey_richness > 0.6 {
            adv.push("rich prey");
        } else if self.prey_richness < 0.35 {
            chl.push("thin prey");
        }
        if self.herbs > 0.5 {
            adv.push("herbs in the litter");
        }
        if self.defensibility > 0.5 {
            adv.push("defensible ground");
        }
        if self.fishing > 0.4 {
            adv.push("fishing water");
        }
        if self.hidden_paths > 0.3 {
            adv.push("hidden paths");
            chl.push("things hide from you too");
        }
        if self.harsh_winter > 0.6 {
            chl.push("harsh winters");
        }
        if adv.is_empty() {
            adv.push("open ground");
        }
        format!(
            "{} — advantages: {}; challenges: {}",
            self.biome_name,
            adv.join(", "),
            if chl.is_empty() { "few".into() } else { chl.join(", ") }
        )
    }

    /// A "Moss Valley"-style clan name grown from the ground it stands on.
    pub fn clan_title(&self, seed: u32) -> String {
        let first = match self.biome_name {
            "Oak Tree Forest" => "Moss",
            "Shadowy Forest" => "Thorn",
            "River Reeds" => "Riverstone",
            "Lake Shore" => "Palewater",
            "Moor Plains" => "Windrun",
            "Mangrove Shallows" => "Saltroot",
            _ => "Stone",
        };
        let second = ["Valley", "Ridge", "Hollow", "Rise"][(hashf(seed ^ 0x51_7e) * 3.999) as usize];
        format!("{} {} Clan", first, second)
    }
}

// ── §5/§6 cats: members and the wanderers who might become them ─────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Archetype {
    Stray,
    HouseCat,
    LoneWarrior,
    YoungCat,
    Outsider,
}

impl Archetype {
    pub fn name(self) -> &'static str {
        match self {
            Archetype::Stray => "stray",
            Archetype::HouseCat => "former house cat",
            Archetype::LoneWarrior => "lone warrior",
            Archetype::YoungCat => "young cat",
            Archetype::Outsider => "outsider",
        }
    }
    /// How much goodwill it takes before this cat trusts a clan (§6's
    /// "suspicious" strays vs "easier to train" young cats).
    fn wariness(self) -> f32 {
        match self {
            Archetype::Stray => 2.6,
            Archetype::HouseCat => 1.6,
            Archetype::LoneWarrior => 2.2,
            Archetype::YoungCat => 0.9,
            Archetype::Outsider => 1.9,
        }
    }
    /// Skill bias [climb, sprint, swim, stealth, scent].
    fn skills(self, seed: u32) -> [f32; 5] {
        let base = match self {
            Archetype::Stray => [1.0, 1.0, 0.9, 1.25, 1.1],
            Archetype::HouseCat => [0.85, 0.9, 0.8, 0.9, 0.9],
            Archetype::LoneWarrior => [1.1, 1.2, 0.95, 1.05, 1.0],
            Archetype::YoungCat => [0.85, 0.95, 0.85, 0.9, 0.95],
            Archetype::Outsider => [1.05, 0.95, 1.2, 1.0, 1.15],
        };
        let mut out = base;
        for (i, v) in out.iter_mut().enumerate() {
            *v *= 0.9 + hashf(seed.wrapping_add(i as u32 * 0x9e37)) * 0.2;
        }
        out
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Personality {
    pub bold: f32,
    pub social: f32,
    pub curious: f32,
}

impl Personality {
    fn roll(seed: u32) -> Personality {
        Personality {
            bold: hashf(seed ^ 0xb01d),
            social: hashf(seed ^ 0x50c1),
            curious: hashf(seed ^ 0xcu32),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    pub rank: lore::Rank,
    pub archetype: Archetype,
    pub age_seasons: u32,
    pub skills: [f32; 5],
    pub personality: Personality,
}

#[derive(Clone, Debug)]
pub struct Wanderer {
    pub name: String,
    pub archetype: Archetype,
    pub x: f32,
    pub z: f32,
    pub goodwill: f32,
    pub personality: Personality,
}

// ── §3 camp ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct Camp {
    pub warrior_den: bool,
    pub nursery: bool,
    pub medicine_den: bool,
    pub prey_store: bool,
    pub training_ground: bool,
    pub lookout: bool,
}

impl Camp {
    pub fn built_count(&self) -> u32 {
        [self.warrior_den, self.nursery, self.medicine_den, self.prey_store, self.training_ground, self.lookout]
            .iter()
            .filter(|b| **b)
            .count() as u32
    }
}

// ── §12 the ledger the description is read from ────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct Deeds {
    pub hunts: u32,
    pub fish_catches: u32,
    pub heals: u32,
    pub explorations: u32,
    pub border_patrols: u32,
    pub welcomes: u32,
    pub contests_won: u32,
    pub contests_lost: u32,
    pub rites: u32,
    pub migrations: u32,
}

// ── the world ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct OpenWorld {
    pub seed: u32,
    pub origin: Origin,
    pub doctrine: Doctrine,
    pub site: Site,
    pub clan_name: String,
    pub season: u32, // total seasons elapsed; season % 4 indexes SEASONS
    pub members: Vec<Member>,
    pub wanderers: Vec<Wanderer>,
    /// 0..1: how much of the land's prey remains. 1 = untouched.
    pub prey_level: f32,
    /// 0..1: predator presence drawn in by scarcity.
    pub predator_pressure: f32,
    /// Standing with the six trait clans of clan.rs, -1..1 each.
    pub reputation: [f32; clan::CLAN_COUNT],
    pub fed_last_season: bool,
    pub camp: Camp,
    pub deeds: Deeds,
    pub chronicle: Vec<String>,
    /// §10: discovered places (name, x, z); some become sacred.
    pub discoveries: Vec<(String, f32, f32)>,
    pub sacred_site: Option<(f32, f32)>,
    next_name_seed: u32,
}

impl OpenWorld {
    pub fn new(seed: u32, origin: Origin, doctrine: Doctrine, x: f32, z: f32) -> OpenWorld {
        let site = Site::assess(x, z);
        let clan_name = site.clan_title(seed);
        let mut w = OpenWorld {
            seed,
            origin,
            doctrine,
            site,
            clan_name: clan_name.clone(),
            season: 0,
            members: Vec::new(),
            wanderers: Vec::new(),
            prey_level: 1.0,
            predator_pressure: 0.05,
            reputation: [0.0; clan::CLAN_COUNT],
            fed_last_season: true,
            camp: Camp::default(),
            deeds: Deeds::default(),
            chronicle: Vec::new(),
            discoveries: Vec::new(),
            sacred_site: None,
            next_name_seed: seed ^ 0xca75,
        };
        // §1 — the origin sets the opening hand.
        match origin {
            Origin::WanderingCat => {
                w.add_member(Archetype::Stray, lore::Rank::Leader, 14);
                // sharper exploration: the founder is curious by decree
                w.members[0].personality.curious = w.members[0].personality.curious.max(0.8);
            }
            Origin::SurvivorGroup => {
                w.add_member(Archetype::Stray, lore::Rank::Leader, 14);
                w.add_member(Archetype::YoungCat, lore::Rank::Warrior, 12);
                w.add_member(Archetype::HouseCat, lore::Rank::Warrior, 13);
                for m in &mut w.members {
                    m.personality.social = m.personality.social.max(0.7); // strong bonds
                }
            }
            Origin::ExiledClan => {
                w.add_member(Archetype::LoneWarrior, lore::Rank::Leader, 20);
                w.add_member(Archetype::LoneWarrior, lore::Rank::Deputy, 18);
                w.add_member(Archetype::LoneWarrior, lore::Rank::Warrior, 16);
                w.add_member(Archetype::Stray, lore::Rank::Warrior, 15);
                // history and enemies: a grudge held by two of the six clans
                w.reputation[(seed as usize) % clan::CLAN_COUNT] = -0.6;
                w.reputation[(seed as usize + 3) % clan::CLAN_COUNT] = -0.4;
            }
            Origin::AncientBloodline => {
                w.add_member(Archetype::Outsider, lore::Rank::Leader, 16);
                w.add_member(Archetype::YoungCat, lore::Rank::Apprentice, 5);
                w.sacred_site = Some((x + 6.0, z - 4.0));
            }
        }
        let founder = w.members[0].name.clone();
        w.log(format!(
            "{} founded by {} — {} — under {} law. {}",
            clan_name,
            founder,
            origin.name(),
            doctrine.name(),
            w.site.brief()
        ));
        w
    }

    fn add_member(&mut self, archetype: Archetype, rank: lore::Rank, age_seasons: u32) {
        let s = self.next_name_seed;
        self.next_name_seed = self.next_name_seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        self.members.push(Member {
            name: lore::cat_name(s, rank),
            rank,
            archetype,
            age_seasons,
            skills: archetype.skills(s),
            personality: Personality::roll(s),
        });
    }

    fn log(&mut self, line: String) {
        let yr = self.season / 4 + 1;
        let sn = SEASONS[(self.season % 4) as usize];
        self.chronicle.push(format!("[y{} {}] {}", yr, sn, line));
    }

    fn rngf(&self, salt: u32) -> f32 {
        hashf(self.seed ^ salt.wrapping_add(self.season.wrapping_mul(0x9E37_79B9)))
    }

    pub fn adults(&self) -> usize {
        self.members
            .iter()
            .filter(|m| !matches!(m.rank, lore::Rank::Kit | lore::Rank::Apprentice | lore::Rank::Elder))
            .count()
    }

    fn skill_sum(&self, idx: usize) -> f32 {
        self.members.iter().map(|m| m.skills[idx]).sum()
    }

    // ── the season pipeline ────────────────────────────────────────────────

    pub fn season_tick(&mut self) {
        let winter = self.season % 4 == 3;

        // 1. provisioning (§9 forward half): mouths vs the land.
        let mouths = self.members.len() as f32;
        let hunger = mouths * if winter { 1.0 + self.site.harsh_winter * 0.5 } else { 1.0 };
        let hunt_bias = if self.doctrine == Doctrine::Hunter { 1.25 } else { 1.0 };
        let guard_bias = if self.doctrine == Doctrine::Guardian { 0.7 } else { 1.0 };
        let mut land_take = hunger * hunt_bias * guard_bias;
        // fishing bypasses land pressure but needs the water and a wader
        let best_swim = self.members.iter().map(|m| m.skills[2]).fold(0.0f32, f32::max);
        if self.site.fishing > 0.3 && best_swim > 1.0 {
            let fish = (self.site.fishing * best_swim).min(land_take * 0.5);
            land_take -= fish;
            self.deeds.fish_catches += fish.ceil() as u32;
        }
        let sustainable = 3.0 + self.site.prey_richness * 6.0;
        let land_yield = land_take.min(sustainable * self.prey_level);
        self.deeds.hunts += land_yield.ceil() as u32;
        self.fed_last_season = land_yield + 0.5 >= land_take
            || (self.camp.prey_store && self.rngf(0xf00d) < 0.8);

        // 2. the land answers (§9): pressure thins prey; scarcity draws predators.
        let pressure = (land_take / sustainable).min(2.0);
        self.prey_level = (self.prey_level + 0.12 - pressure * 0.15).clamp(0.05, 1.0);
        if self.prey_level < 0.4 {
            self.predator_pressure = (self.predator_pressure + 0.18).min(1.0);
            if self.predator_pressure > 0.5 && self.rngf(0xbad) < 0.6 {
                self.log("prey runs thin; a fox has begun working the borders".into());
            }
        } else {
            self.predator_pressure = (self.predator_pressure - 0.08).max(0.0);
            if self.prey_level > 0.85 && self.doctrine == Doctrine::Guardian && self.rngf(0x5a3e) < 0.35 {
                self.log("the land breathes easy — a white hart was seen at the water".into());
            }
        }
        if self.camp.lookout {
            self.predator_pressure = (self.predator_pressure - 0.05).max(0.0);
        }

        // 3. camp building (§3): each season standing cats raise one structure.
        if self.adults() >= 2 {
            let c = &mut self.camp;
            let next: Option<(&mut bool, &str)> = if !c.warrior_den {
                Some((&mut c.warrior_den, "a warrior den dug under the roots"))
            } else if !c.prey_store {
                Some((&mut c.prey_store, "a cool prey store lined with stones"))
            } else if !c.nursery {
                Some((&mut c.nursery, "a bramble-walled nursery"))
            } else if !c.medicine_den && self.site.herbs > 0.3 {
                Some((&mut c.medicine_den, "a medicine den beside the herb patch"))
            } else if !c.lookout && self.site.defensibility > 0.3 {
                Some((&mut c.lookout, "a lookout on the high stone"))
            } else if !c.training_ground {
                Some((&mut c.training_ground, "a sandy training hollow"))
            } else {
                None
            };
            if let Some((slot, what)) = next {
                *slot = true;
                let line = format!("the clan raised {}", what);
                self.log(line);
            }
        }

        // 4. season activity by doctrine — this is what the deeds ledger observes.
        match self.doctrine {
            Doctrine::Explorer => {
                self.deeds.explorations += 2;
                if self.rngf(0xd15c) < 0.4 {
                    let names = ["a forgotten ruin", "an ancient oak", "an abandoned camp", "a hidden valley", "an underground run"];
                    let what = names[(self.rngf(0xd15d) * 4.999) as usize];
                    let dx = (self.rngf(0xd15e) - 0.5) * 120.0;
                    let dz = (self.rngf(0xd15f) - 0.5) * 120.0;
                    self.discoveries.push((what.to_string(), self.site.x + dx, self.site.z + dz));
                    let line = format!("scouts found {}", what);
                    self.log(line);
                    if self.sacred_site.is_none() && self.rngf(0x5ac3) < 0.3 {
                        self.sacred_site = Some((self.site.x + dx, self.site.z + dz));
                        self.log("the elders named it a sacred place".into());
                    }
                }
            }
            Doctrine::Guardian => {
                self.deeds.border_patrols += 1;
                self.deeds.heals += if self.camp.medicine_den { 2 } else { 1 };
            }
            Doctrine::Isolationist => {
                self.deeds.border_patrols += 2;
            }
            Doctrine::Hunter => { /* hunts already tallied above */ }
            Doctrine::Diplomatic => {
                self.deeds.welcomes += 1;
                for r in self.reputation.iter_mut() {
                    *r = (*r + 0.05).min(1.0);
                }
            }
        }
        if self.camp.medicine_den && self.site.herbs > 0.4 {
            self.deeds.heals += 1;
        }
        if self.members.iter().any(|m| m.personality.curious > 0.75) {
            self.deeds.explorations += 1;
        }

        // 5. wanderers (§6): the world offers cats; goodwill wins them over.
        if self.wanderers.len() < 3 && self.rngf(0x0a2b) < 0.55 {
            let roll = self.rngf(0x0a2c);
            let arche = if roll < 0.28 {
                Archetype::Stray
            } else if roll < 0.5 {
                Archetype::YoungCat
            } else if roll < 0.68 {
                Archetype::HouseCat
            } else if roll < 0.86 {
                Archetype::LoneWarrior
            } else {
                Archetype::Outsider
            };
            let s = self.next_name_seed;
            self.next_name_seed = self.next_name_seed.wrapping_mul(2_654_435_761).wrapping_add(1);
            let a = self.rngf(0x0a2d) * std::f32::consts::TAU;
            let d = 26.0 + self.rngf(0x0a2e) * 30.0;
            self.wanderers.push(Wanderer {
                name: lore::cat_name(s, lore::Rank::Warrior),
                archetype: arche,
                x: self.site.x + a.cos() * d,
                z: self.site.z + a.sin() * d,
                goodwill: 0.0,
                personality: Personality::roll(s),
            });
        }
        let social = self
            .members
            .iter()
            .map(|m| m.personality.social)
            .fold(0.0f32, f32::max);
        let ancient_pull = if self.origin == Origin::AncientBloodline { 0.35 } else { 0.0 };
        let iso = if self.doctrine == Doctrine::Isolationist { 0.5 } else { 1.0 };
        let fed_bonus = if self.fed_last_season { 0.25 } else { -0.1 };
        let mut joined: Vec<usize> = Vec::new();
        for (i, wd) in self.wanderers.iter_mut().enumerate() {
            wd.goodwill += (0.35 + social * 0.5 + ancient_pull + fed_bonus) * iso
                + wd.personality.social * 0.2;
            // drift toward camp as goodwill grows
            let t = (wd.goodwill / wd.archetype.wariness()).min(1.0);
            wd.x += (self.site.x - wd.x) * 0.25 * t;
            wd.z += (self.site.z - wd.z) * 0.25 * t;
            if wd.goodwill >= wd.archetype.wariness() {
                joined.push(i);
            }
        }
        for i in joined.into_iter().rev() {
            let wd = self.wanderers.remove(i);
            let rank = if wd.archetype == Archetype::YoungCat {
                lore::Rank::Apprentice
            } else {
                lore::Rank::Warrior
            };
            let age = if wd.archetype == Archetype::YoungCat { 5 } else { 14 };
            self.members.push(Member {
                name: wd.name.clone(),
                rank,
                archetype: wd.archetype,
                age_seasons: age,
                skills: wd.archetype.skills(self.seed ^ wd.name.len() as u32),
                personality: wd.personality,
            });
            self.deeds.welcomes += 1;
            let line = format!("{} the {} joined the clan", wd.name, wd.archetype.name());
            self.log(line);
        }

        // 6. rivals (§8): an encounter with one of the six clans, settled without claws.
        if self.rngf(0xc1a2) < 0.5 {
            let rc = (self.rngf(0xc1a3) * (clan::CLAN_COUNT as f32 - 0.001)) as usize;
            let rival = clan::trait_of(rc as u8);
            let grudge = self.reputation[rc] < -0.2;
            if self.doctrine == Doctrine::Diplomatic && !grudge {
                self.reputation[rc] = (self.reputation[rc] + 0.2).min(1.0);
                let line = format!("shared tongues with {} — borders agreed", rival.name);
                self.log(line);
            } else if self.predator_pressure > 0.7 && self.prey_level < 0.25 {
                // §8 migration: the land itself has become the enemy
                self.deeds.migrations += 1;
                self.log("with prey gone and predators close, elders spoke of moving camp".into());
            } else {
                // a hunting contest or a race: relevant skills + seeded nerve
                let race = self.rngf(0xc1a4) < 0.4;
                let ours = if race {
                    self.skill_sum(1) / self.members.len().max(1) as f32
                } else {
                    (self.skill_sum(3) + self.skill_sum(4)) / (2.0 * self.members.len().max(1) as f32)
                };
                let theirs = if race {
                    rival.sprint
                } else {
                    (rival.stealth + rival.scent) * 0.5
                };
                let margin = ours - theirs + (self.rngf(0xc1a5) - 0.5) * 0.3
                    + self.members.iter().map(|m| m.personality.bold).fold(0.0f32, f32::max) * 0.1;
                let what = if race { "a moor race" } else { "a hunting contest" };
                if margin > 0.0 {
                    self.deeds.contests_won += 1;
                    self.reputation[rc] = (self.reputation[rc] + 0.1).min(1.0);
                    let line = format!("won {} against {}", what, rival.name);
                    self.log(line);
                } else {
                    self.deeds.contests_lost += 1;
                    let line = format!("lost {} to {}", what, rival.name);
                    self.log(line);
                }
            }
        }

        // 7. generations (§11): age, rites, succession, new kits.
        for m in &mut self.members {
            m.age_seasons += 1;
        }
        let mut rites: Vec<String> = Vec::new();
        // mentor = the eldest warrior-or-better; borrow-friendly two-pass
        let mentor = self
            .members
            .iter()
            .filter(|m| matches!(m.rank, lore::Rank::Warrior | lore::Rank::Deputy | lore::Rank::Leader))
            .max_by_key(|m| m.age_seasons)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| "the leader".into());
        for m in &mut self.members {
            match m.rank {
                lore::Rank::Kit if m.age_seasons >= 6 => {
                    m.rank = lore::Rank::Apprentice;
                    rites.push(lore::apprentice_rite(&m.name, &mentor));
                    m.name = lore::cat_name(m.age_seasons ^ m.name.len() as u32, lore::Rank::Apprentice);
                }
                lore::Rank::Apprentice if m.age_seasons >= 12 => {
                    let old = m.name.clone();
                    m.rank = lore::Rank::Warrior;
                    m.name = lore::cat_name(m.age_seasons.wrapping_mul(31) ^ old.len() as u32, lore::Rank::Warrior);
                    rites.push(lore::warrior_rite(&old, &m.name));
                }
                lore::Rank::Warrior | lore::Rank::Deputy if m.age_seasons >= 40 => {
                    m.rank = lore::Rank::Elder;
                    rites.push(format!("{} retired to the elders' den", m.name));
                }
                lore::Rank::Leader if m.age_seasons >= 44 => {
                    m.rank = lore::Rank::Elder;
                    rites.push(format!("{} stepped down, honored", m.name));
                }
                _ => {}
            }
        }
        for r in rites {
            self.deeds.rites += 1;
            self.log(r);
        }
        // succession: a leaderless clan promotes deputy, else eldest warrior
        if !self.members.iter().any(|m| m.rank == lore::Rank::Leader) {
            if let Some(m) = self
                .members
                .iter_mut()
                .filter(|m| m.rank == lore::Rank::Deputy)
                .next()
                .or_else(|| {
                    // no deputy: pick eldest warrior — recompute to appease the borrow
                    None
                })
            {
                m.rank = lore::Rank::Leader;
                let line = format!("{} took the leader's name", m.name);
                self.log(line);
            } else if let Some(idx) = self
                .members
                .iter()
                .enumerate()
                .filter(|(_, m)| m.rank == lore::Rank::Warrior)
                .max_by_key(|(_, m)| m.age_seasons)
                .map(|(i, _)| i)
            {
                self.members[idx].rank = lore::Rank::Leader;
                let line = format!("{} took the leader's name", self.members[idx].name);
                self.log(line);
            }
        }
        // kits: a fed clan with a nursery and room grows
        if self.fed_last_season
            && self.camp.nursery
            && self.adults() >= 2
            && self.members.len() < 14
            && self.rngf(0x6b17) < 0.45
        {
            self.add_member(Archetype::YoungCat, lore::Rank::Kit, 0);
            let name = self.members.last().unwrap().name.clone();
            let line = format!("a kit was born: {}", name);
            self.log(line);
        }

        self.season += 1;
    }

    /// §12 — the world describes the clan it watched you build. Reads only deeds.
    pub fn describe(&self) -> String {
        let d = &self.deeds;
        let axes: [(&str, u32); 6] = [
            ("healers", d.heals),
            ("explorers", d.explorations),
            ("hunters", d.hunts / 4),
            ("fishers", d.fish_catches / 3),
            ("warriors", d.contests_won * 2),
            ("strict borders", d.border_patrols),
        ];
        let mut ranked = axes;
        ranked.sort_by(|a, b| b.1.cmp(&a.1));
        let feared = ranked[0].0 == "warriors" || ranked[0].0 == "strict borders";
        if d.welcomes >= 6 && d.rites <= 2 {
            return format!(
                "{} welcomes all cats but has forgotten the old ways.",
                self.clan_name
            );
        }
        if ranked[0].1 == 0 {
            return format!("{} is young yet; the land has no word for it.", self.clan_name);
        }
        format!(
            "{} is {} for its {} and {}.",
            self.clan_name,
            if feared { "feared" } else { "known" },
            ranked[0].0,
            ranked[1].0
        )
    }
}

// ── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn run(mut w: OpenWorld, seasons: u32) -> OpenWorld {
        for _ in 0..seasons {
            w.season_tick();
        }
        w
    }

    #[test]
    fn the_four_origins_deal_different_opening_hands() {
        let wander = OpenWorld::new(7, Origin::WanderingCat, Doctrine::Hunter, 30.0, 40.0);
        let surv = OpenWorld::new(7, Origin::SurvivorGroup, Doctrine::Hunter, 30.0, 40.0);
        let exile = OpenWorld::new(7, Origin::ExiledClan, Doctrine::Hunter, 30.0, 40.0);
        let ancient = OpenWorld::new(7, Origin::AncientBloodline, Doctrine::Hunter, 30.0, 40.0);
        assert_eq!(wander.members.len(), 1); // starts alone
        assert_eq!(surv.members.len(), 3); // a few cats, strong bonds
        assert!(surv.members.iter().all(|m| m.personality.social >= 0.7));
        assert!(exile.members.len() >= 4); // experienced warriors
        assert!(exile.reputation.iter().any(|&r| r < -0.3), "exiles carry a grudge against them");
        assert!(ancient.sacred_site.is_some()); // sacred location from day one
    }

    #[test]
    fn same_seed_same_history() {
        let a = run(OpenWorld::new(1234, Origin::SurvivorGroup, Doctrine::Explorer, 10.0, -20.0), 16);
        let b = run(OpenWorld::new(1234, Origin::SurvivorGroup, Doctrine::Explorer, 10.0, -20.0), 16);
        assert_eq!(a.chronicle, b.chronicle);
        assert_eq!(a.members.len(), b.members.len());
        let c = run(OpenWorld::new(1235, Origin::SurvivorGroup, Doctrine::Explorer, 10.0, -20.0), 16);
        assert_ne!(a.chronicle, c.chronicle, "a different seed writes a different chronicle");
    }

    #[test]
    fn overhunting_thins_prey_and_draws_predators_and_restraint_recovers() {
        // a big hungry clan on modest ground: pressure must build
        let mut w = OpenWorld::new(9, Origin::ExiledClan, Doctrine::Hunter, 60.0, 60.0);
        for _ in 0..6 {
            w.add_member(Archetype::LoneWarrior, lore::Rank::Warrior, 14);
        }
        let w = run(w, 8);
        assert!(w.prey_level < 0.5, "ten hunters should thin the land (prey={})", w.prey_level);
        assert!(w.predator_pressure > 0.3, "scarcity draws predators (pred={})", w.predator_pressure);

        // the same site under a small guardian clan stays rich
        let g = run(OpenWorld::new(9, Origin::WanderingCat, Doctrine::Guardian, 60.0, 60.0), 8);
        assert!(g.prey_level > 0.7, "restraint keeps the land rich (prey={})", g.prey_level);
        assert!(g.predator_pressure < 0.2);
    }

    #[test]
    fn wanderers_join_and_young_cats_join_faster_than_strays() {
        // measure seasons-to-join for a fixed archetype by injecting the wanderer directly
        let mut base = OpenWorld::new(21, Origin::SurvivorGroup, Doctrine::Diplomatic, 30.0, 40.0);
        base.wanderers.push(Wanderer {
            name: "Testpaw".into(),
            archetype: Archetype::YoungCat,
            x: 60.0,
            z: 40.0,
            goodwill: 0.0,
            personality: Personality { bold: 0.5, social: 0.5, curious: 0.5 },
        });
        let mut young_seasons = 0;
        while base.members.iter().all(|m| m.name != "Testpaw") {
            base.season_tick();
            young_seasons += 1;
            assert!(young_seasons < 12, "a young cat must join within a few seasons");
        }

        let mut s = OpenWorld::new(21, Origin::SurvivorGroup, Doctrine::Diplomatic, 30.0, 40.0);
        s.wanderers.push(Wanderer {
            name: "Testfang".into(),
            archetype: Archetype::Stray,
            x: 60.0,
            z: 40.0,
            goodwill: 0.0,
            personality: Personality { bold: 0.5, social: 0.5, curious: 0.5 },
        });
        let mut stray_seasons = 0;
        while s.members.iter().all(|m| m.name != "Testfang") && stray_seasons < 20 {
            s.season_tick();
            stray_seasons += 1;
        }
        assert!(
            stray_seasons > young_seasons,
            "a suspicious stray ({} seasons) must take longer than a young cat ({})",
            stray_seasons,
            young_seasons
        );
    }

    #[test]
    fn isolationists_recruit_slower_than_diplomats() {
        let d = run(OpenWorld::new(33, Origin::WanderingCat, Doctrine::Diplomatic, 30.0, 40.0), 20);
        let i = run(OpenWorld::new(33, Origin::WanderingCat, Doctrine::Isolationist, 30.0, 40.0), 20);
        assert!(
            d.members.len() >= i.members.len(),
            "diplomats ({}) should not trail isolationists ({})",
            d.members.len(),
            i.members.len()
        );
        assert!(d.deeds.welcomes > i.deeds.welcomes);
    }

    #[test]
    fn a_camp_rises_and_gates_births() {
        let w = run(OpenWorld::new(5, Origin::SurvivorGroup, Doctrine::Hunter, 30.0, 40.0), 12);
        assert!(w.camp.warrior_den && w.camp.prey_store, "first structures rise early");
        assert!(w.camp.built_count() >= 3);
        // kits appear only once the nursery exists
        if let Some(first_kit) = w.chronicle.iter().position(|l| l.contains("a kit was born")) {
            let nursery = w
                .chronicle
                .iter()
                .position(|l| l.contains("nursery"))
                .expect("a kit implies a nursery was built");
            assert!(nursery < first_kit, "nursery ({nursery}) must predate the first kit ({first_kit})");
        }
    }

    #[test]
    fn generations_turn_kits_into_warriors_with_the_rites() {
        let mut w = OpenWorld::new(11, Origin::SurvivorGroup, Doctrine::Hunter, 30.0, 40.0);
        w.camp.nursery = true; // fast-forward the precondition
        w.add_member(Archetype::YoungCat, lore::Rank::Kit, 0);
        let w = run(w, 14);
        assert!(
            w.chronicle.iter().any(|l| l.contains("apprentice")),
            "an apprentice rite must be spoken: {:#?}",
            w.chronicle
        );
        assert!(w.deeds.rites >= 1);
        // 14 seasons past age 0 crosses the warrior threshold too
        assert!(w.members.iter().filter(|m| m.rank == lore::Rank::Warrior).count() >= 1);
    }

    #[test]
    fn the_description_follows_deeds_not_the_label() {
        // a guardian clan on herb-rich forest ground becomes known for healers
        let g = run(OpenWorld::new(3, Origin::SurvivorGroup, Doctrine::Guardian, 30.0, 40.0), 20);
        let gd = g.describe();
        assert!(
            gd.contains("healers") || gd.contains("strict borders"),
            "guardian deeds should surface: {gd}"
        );
        // an explorer clan is described by what it did
        let e = run(OpenWorld::new(3, Origin::SurvivorGroup, Doctrine::Explorer, 30.0, 40.0), 20);
        assert!(e.describe().contains("explorers"), "{}", e.describe());
        assert!(e.deeds.explorations > g.deeds.explorations);
    }

    #[test]
    fn sites_differ_by_the_ground_they_stand_on() {
        // scan for a rocky site and a watery site rather than hardcoding coords
        let mut rocky: Option<Site> = None;
        let mut watery: Option<Site> = None;
        let mut step = 0;
        while (rocky.is_none() || watery.is_none()) && step < 4000 {
            let x = -300.0 + (step % 63) as f32 * 9.7;
            let z = -300.0 + (step / 63) as f32 * 9.7;
            let s = Site::assess(x, z);
            if s.defensibility > 0.5 && rocky.is_none() {
                rocky = Some(s);
            }
            if s.fishing > 0.4 && watery.is_none() {
                watery = Some(s);
            }
            step += 1;
        }
        let (rocky, watery) = (rocky.expect("some rocky ground exists"), watery.expect("some water exists"));
        assert!(rocky.defensibility > watery.defensibility || watery.fishing > rocky.fishing);
        assert!(rocky.brief().contains("advantages"));
    }

    #[test]
    fn a_starved_cornered_clan_considers_migration() {
        let mut w = OpenWorld::new(2, Origin::ExiledClan, Doctrine::Hunter, 60.0, 60.0);
        for _ in 0..8 {
            w.add_member(Archetype::LoneWarrior, lore::Rank::Warrior, 14);
        }
        let w = run(w, 20);
        assert!(
            w.deeds.migrations > 0 || w.prey_level > 0.3,
            "either the land held, or the elders spoke of moving: prey={} migrations={}",
            w.prey_level,
            w.deeds.migrations
        );
    }
}
