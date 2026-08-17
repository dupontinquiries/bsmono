//! battleground.rs — **"Battleground of the Ancestors"**: the clan-war mode, a
//! MOBA re-thought in the books' terms and run as pure, deterministic,
//! renderer-free logic like `sim`/`prowl`/`capture`.
//!
//! Two clans, each championed by legendary ancestors ([`crate::ancestor`]), fight
//! across a **large** map (camps in opposite corners, three trails between) to fell
//! the enemy camp's **Great Tree**. Every MOBA fixture is translated, not copied:
//!
//! * **Trees of Healing instead of towers.** The trail structures don't shoot —
//!   they *protect*: allies near a living Heal Tree are mended and take reduced
//!   harm. And while any of a clan's Heal Trees stand, their roots share strength
//!   with the Great Tree, hardening it. You don't dive a tower; you gnaw a
//!   sanctuary out from under its clan.
//! * **Freshkill instead of a shop.** There is no gold and nothing to buy. Prey
//!   lives at neutral hunting camps and at the central **contention ground**;
//!   killed prey must be *carried home* (slower, droppable on death) to the camp's
//!   freshkill pile. A well-fed clan grows stronger — the pile scales the whole
//!   clan's claws, with diminishing returns so a lead never snowballs unbounded.
//!   Holding the contention ground (more warriors present) makes its rich prey
//!   yours to hunt.
//! * **Patrols instead of minion waves.** Lanes carry cat AI patrols: *fewer,
//!   stronger*, and sensible. A patrol never seeks to fight to the death — once a
//!   cat is wounded past its nerve it breaks off, returns to camp, and is healed by
//!   the **medic cat** before rejoining the trail. The one exception is the books'
//!   exception: defending the Great Tree itself, a warrior stands.
//! * **Herbs as extra objectives.** Herb patches regrow in the leafy margins;
//!   gathering takes a held moment and the sprig must reach your medic cat. Each
//!   delivered herb quickens every kind of healing the clan does — medic, trees,
//!   and how fast the fallen return.
//!
//! **World note (a deliberate deviation from novel canon):** this world has *no
//! humans* — no thunderpaths, no monsters, no twoleg places, no kittypets. The
//! terrain generator builds only wild land (forest, moor, river, fen, crag), and
//! nothing in this module may reference a human-made feature. The clans' whole
//! world is the wild.

use crate::ancestor::{self, Ancestor, Verb};
use crate::junglemap;
use crate::world;

pub const CLAN_A: u8 = 0;
pub const CLAN_B: u8 = 1;

// ── the large map ───────────────────────────────────────────────────────────
const ARENA: f32 = 150.0;                       // half-extent (capture plays in 90)
const CAMP_ANCHOR: [(f32, f32); 2] = [(-118.0, -118.0), (118.0, 118.0)];
/// Waypoint each trail bends through: west trail, the straight middle, east trail.
const TRAIL_BEND: [(f32, f32); 3] = [(-85.0, 85.0), (0.0, 0.0), (85.0, -85.0)];
pub const LANES: usize = 3;

// structures
const HEAL_TREE_HP: f32 = 550.0;
const GREAT_TREE_HP: f32 = 1400.0;
const AURA_R: f32 = 14.0;
const AURA_HEAL: f32 = 5.0;          // hp/s to allies, before herb scaling
const AURA_GUARD: f32 = 0.35;        // damage reduction under a living Heal Tree
const ROOT_GUARD: f32 = 0.55;        // Great Tree damage reduction while any Heal Tree stands

// patrols — fewer, stronger, and they retreat
const PATROL_INTERVAL: f32 = 26.0;
const PATROL_SIZE: usize = 2;
const PATROL_HP: f32 = 130.0;
const PATROL_ATK: f32 = 8.0;
const PATROL_SPEED: f32 = 6.4;
const NERVE: f32 = 0.45;             // retreat below this hp fraction…
const REJOIN: f32 = 0.9;             // …rejoin the trail above this
const LAST_STAND_R: f32 = 26.0;      // …unless the Great Tree itself is threatened

// camp & medic
const MEDIC_R: f32 = 10.0;
const MEDIC_HEAL: f32 = 16.0;        // hp/s at the medic's den, before herbs

// freshkill economy
const PREY_CAMPS: [(f32, f32); 4] = [(-70.0, 20.0), (70.0, -20.0), (-20.0, -70.0), (20.0, 70.0)];
const PREY_HP: f32 = 26.0;
const PREY_RESPAWN: f32 = 22.0;
const PREY_WORTH: f32 = 1.0;
const RICH_WORTH: f32 = 3.0;         // the contention ground's prey
const CARRY_SLOW: f32 = 0.75;
const CONTEND_R: f32 = 20.0;
const GROWTH_CAP: f32 = 0.8;         // pile can add at most +80% power
const GROWTH_HALF: f32 = 16.0;       // pile worth at which half the cap is reached

// herbs
const HERB_SPOTS: [(f32, f32); 2] = [(-45.0, -5.0), (45.0, 5.0)];
const HERB_REGROW: f32 = 30.0;
const HERB_GATHER: f32 = 2.0;        // seconds crouched over the patch
const HERB_HEAL_BONUS: f32 = 0.15;   // each delivered herb: +15% to all clan healing…
const HERB_HEAL_MAX: f32 = 2.0;      // …capped at double
const HERB_RESPAWN_CUT: f32 = 0.8;   // and each herb multiplies respawn time by this

const DAY_LEN: f32 = 90.0;           // day and night alternate, this long each
const LEAFBARE_AT: f32 = 300.0;      // late game: auras thin, patrols grow desperate
const LEAFBARE_AURA: f32 = 0.5;      // aura healing multiplier once leaf-bare comes
const LEAFBARE_PATROL: usize = 3;    // patrol size in leaf-bare

const RESPAWN_BASE: f32 = 14.0;
const REACH: f32 = 2.8;              // claw range
const ENGAGE_R: f32 = 16.0;          // how far a warrior will break trail to fight
const MATCH_LEN: f32 = 12.0 * 60.0;

fn hashf(mut h: u32) -> f32 {
    h ^= h >> 16; h = h.wrapping_mul(0x7feb352d);
    h ^= h >> 15; h = h.wrapping_mul(0x846ca68b);
    h ^= h >> 16; (h as f32) / (u32::MAX as f32)
}
fn dist(ax: f32, az: f32, bx: f32, bz: f32) -> f32 { ((ax - bx).powi(2) + (az - bz).powi(2)).sqrt() }
fn norm(x: f32, z: f32) -> (f32, f32) { let l = (x * x + z * z).sqrt(); if l > 1e-5 { (x / l, z / l) } else { (0.0, 0.0) } }

/// Nearest dry, flattish ground to an anchor (same idea as capture.rs).
fn land_near(ax: f32, az: f32) -> (f32, f32) {
    let (mut best, mut best_score) = ((ax, az), f32::MAX);
    for ring in 0..16 {
        let r = ring as f32 * 4.0;
        let steps = (6 + ring * 3).max(1);
        for s in 0..steps {
            let a = s as f32 / steps as f32 * std::f32::consts::TAU;
            let (x, z) = (ax + a.cos() * r, az + a.sin() * r);
            let h = world::height_at(x, z);
            if h < world::WATER_Y + 0.3 { continue; }
            let slope = (world::height_at(x + 2.0, z) - h).abs() + (world::height_at(x, z + 2.0) - h).abs();
            let score = slope * 3.0 + r * 0.05;
            if score < best_score { best_score = score; best = (x, z); }
        }
        if best_score < 0.5 && ring >= 2 { break; }
    }
    best
}

// ── entities ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role { Ancestor, Patrol }

/// The two patrols of the books: a **border patrol** walks the trail and holds the
/// line; a **hunting patrol** works the prey camps and carries freshkill home.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Duty { Border, Hunting }

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Carry { Nothing, Freshkill(f32), Herb }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mind {
    Trail,          // advance along the lane toward the enemy camp
    Fight,          // engaged with a nearby enemy
    Retreat,        // wounded past nerve — break off, head home
    AtMedic,        // healing at the medic cat
    Hunt,           // stalking prey at a hunting camp
    CarryHome,      // bringing freshkill / a herb sprig back to camp
    Gather,         // crouched over a herb patch
    LastStand,      // defending the Great Tree — no retreat
    Dead,
}

pub struct Warrior {
    pub team: u8,
    pub role: Role,
    pub ancestor: Option<Ancestor>,
    pub lane: usize,
    pub x: f32, pub z: f32, pub heading: f32,
    pub hp: f32, pub max_hp: f32,
    pub attack: f32, pub speed: f32,
    pub reach: f32,       // basic-attack reach vs warriors and prey (structures stay melee)
    pub mind: Mind,
    pub carry: Carry,
    pub shield: f32,      // absorbs damage first (Mane of Dawn / Stone Patience)
    pub stealth_t: f32,   // unseen/untargetable
    pub fear_t: f32,      // fleeing a Great Roar
    pub slow_t: f32,
    pub root_t: f32,      // held fast (Snowbind / The Long Winter): can claw, cannot move
    pub cds: [f32; 3],    // ability 1, ability 2, ult
    pub respawn_t: f32,
    pub gather_t: f32,
    pub kills: u32,
    pub duty: Duty,
    pub is_player: bool,
    seg: usize,           // trail waypoint progress
}

impl Warrior {
    pub fn alive(&self) -> bool { self.mind != Mind::Dead }
    fn power_frac(&self) -> f32 { self.hp / self.max_hp }
}

pub struct HealTree { pub team: u8, pub lane: usize, pub x: f32, pub z: f32, pub hp: f32 }
impl HealTree { pub fn alive(&self) -> bool { self.hp > 0.0 } }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreyKind { Mouse, Rabbit, Fish, Rich }

impl PreyKind {
    pub fn worth(self) -> f32 { match self { PreyKind::Mouse => 1.0, PreyKind::Rabbit => 2.0, PreyKind::Fish => 2.0, PreyKind::Rich => RICH_WORTH } }
    pub fn hp(self) -> f32 { match self { PreyKind::Mouse => 18.0, PreyKind::Rabbit => 30.0, PreyKind::Fish => 24.0, PreyKind::Rich => PREY_HP * 1.5 } }
    /// Land prey bolts from a close hunter; fish just hold to deep water.
    pub fn flees(self) -> bool { matches!(self, PreyKind::Mouse | PreyKind::Rabbit) }
}

pub struct Prey {
    pub x: f32, pub z: f32,
    pub home: (f32, f32),     // spawn anchor it never strays far from
    pub kind: PreyKind,
    pub hp: f32, pub respawn_t: f32,
}
impl Prey {
    pub fn alive(&self) -> bool { self.respawn_t <= 0.0 }
    pub fn rich(&self) -> bool { self.kind == PreyKind::Rich }
    pub fn worth(&self) -> f32 { self.kind.worth() }
}

/// Scorched ground (Suneye's Noon Blaze / Eye of the Sun): enemies inside burn,
/// and the light sears every veil away — the built-in answer to the night kits.
pub struct Zone { pub x: f32, pub z: f32, pub team: u8, pub r: f32, pub dps: f32, pub t: f32 }

pub struct HerbPatch { pub x: f32, pub z: f32, pub regrow_t: f32 }
impl HerbPatch { pub fn ready(&self) -> bool { self.regrow_t <= 0.0 } }

pub struct ClanState {
    pub pile: f32,        // freshkill worth delivered
    pub herbs: u32,       // herb sprigs delivered to the medic
    pub great_tree_hp: f32,
}

pub struct Battleground {
    pub warriors: Vec<Warrior>,
    pub heal_trees: Vec<HealTree>,
    pub prey: Vec<Prey>,
    pub herb_patches: Vec<HerbPatch>,
    pub zones: Vec<Zone>,
    pub clans: [ClanState; 2],
    pub camps: [(f32, f32); 2],
    pub medics: [(f32, f32); 2],
    pub contention: (f32, f32),
    pub contention_holder: Option<u8>,
    pub time: f32,
    pub outcome: Option<u8>,          // winning clan, or 2 = draw at full time
    patrol_t: [f32; 2],
    wave_n: [u32; 2],     // per-team wave counter — duty/lane rotation must be symmetric
    tick: u32,
    player_driven: bool,   // becomes true on the first set_input; until then the player's warrior is AI-driven
    input: Option<(f32, f32, bool, Option<usize>)>,   // move dir, claw, ability index
}

impl Battleground {
    /// Freshkill growth: how much stronger this clan's claws are for what it has
    /// eaten. Saturating, so hoarding freshkill has diminishing returns.
    pub fn growth(&self, team: u8) -> f32 {
        let p = self.clans[team as usize].pile;
        1.0 + GROWTH_CAP * (p / (p + GROWTH_HALF))
    }
    /// Herb healing multiplier for a clan (medic, Heal Tree auras, ancestor heals).
    pub fn herb_heal(&self, team: u8) -> f32 {
        (1.0 + self.clans[team as usize].herbs as f32 * HERB_HEAL_BONUS).min(HERB_HEAL_MAX)
    }
    /// Night falls every other watch; the ambush ancestors were made for it.
    pub fn is_night(&self) -> bool { (self.time / DAY_LEN) as u32 % 2 == 1 }
    /// Leaf-bare: the late game. Auras thin and border patrols grow desperate.
    pub fn leafbare(&self) -> bool { self.time >= LEAFBARE_AT }

    /// A warrior's melee/ability damage per point of attack — where the passives
    /// live. Freshkill growth, Goldenmane's Pride's Warmth beside a packmate,
    /// Night's Hunger / Silent Footfall in the dark or from the veil.
    fn strike_of(&self, i: usize) -> f32 {
        let w = &self.warriors[i];
        let mut m = self.growth(w.team);
        let near_lion = self.warriors.iter().any(|a|
            a.team == w.team && a.alive() && a.ancestor == Some(Ancestor::Lion)
            && dist(a.x, a.z, w.x, w.z) < 8.0);
        if near_lion { m *= 1.15; }
        if matches!(w.ancestor, Some(Ancestor::Tiger) | Some(Ancestor::Panther))
            && (self.is_night() || w.stealth_t > 0.0
                || junglemap::concealment_at(w.x, w.z).is_some()) { m *= 1.3; }
        // Suneye is Night's Hunger mirrored: the day belongs to the sun-priest
        if w.ancestor == Some(Ancestor::SunCat) && !self.is_night() { m *= 1.3; }
        // Oldroar's Deep Wounds, Deep Fury: the closer to falling, the harder the blows
        if w.ancestor == Some(Ancestor::CaveLion) {
            m *= 1.0 + 0.6 * (1.0 - w.hp / w.max_hp);
        }
        // leaf-bare desperation: with prey thin and auras failing, patrols press the
        // attack — this is what finally fells trees instead of a full-time stalemate
        if w.role == Role::Patrol && self.leafbare() { m *= 1.5; }
        w.attack * m
    }

    /// Per-strike bonus that depends on the *target* — Twofang's finishing bite,
    /// Faryowl's Far Song landing hardest from a distance.
    fn strike_vs(&self, i: usize, target: usize) -> f32 {
        let w = &self.warriors[i];
        let t = &self.warriors[target];
        let mut m = 1.0;
        if w.ancestor == Some(Ancestor::Smilodon) && t.hp < t.max_hp * 0.5 { m *= 1.4; }
        if w.ancestor == Some(Ancestor::Serval) && dist(w.x, w.z, t.x, t.z) > 4.0 { m *= 1.15; }
        if w.ancestor == Some(Ancestor::Jaguar) && (t.slow_t > 0.0 || t.root_t > 0.0 || t.fear_t > 0.0) {
            m *= 1.2;   // The River Remembers: the held are already caught
        }
        m
    }

    /// On-hit riders that come with a warrior's blows — Snowhush's chill.
    fn on_hit(&mut self, i: usize, target: usize) {
        if self.warriors[i].ancestor == Some(Ancestor::Lynx) {
            let t = &mut self.warriors[target];
            t.slow_t = t.slow_t.max(0.5);   // Hush of Snows
        }
    }

    fn respawn_time(&self, team: u8) -> f32 {
        // MOBA death timers: they grow as the match ages, so a late lost fight is a
        // real window at the camp — herbs claw some of that time back
        let age = 1.0 + self.time / 240.0;
        RESPAWN_BASE * age * HERB_RESPAWN_CUT.powi(self.clans[team as usize].herbs.min(6) as i32)
    }
    fn enemy(team: u8) -> u8 { 1 - team }

    /// Default rosters: the player leads clan A; packmates and the enemy clan fill
    /// deterministically from the pool in order, starting after the player's pick —
    /// so which five ancestors you face varies with who you bring.
    pub fn new(player: Ancestor) -> Battleground {
        let pool = ancestor::ALL;
        let start = pool.iter().position(|&p| p == player).unwrap_or(0);
        let mut order = (1..pool.len()).map(|k| pool[(start + k) % pool.len()]);
        let a = [player, order.next().unwrap(), order.next().unwrap()];
        let b = [order.next().unwrap(), order.next().unwrap(), order.next().unwrap()];
        Battleground::new_draft(a, b)
    }

    /// A drafted match: each clan fields exactly the three ancestors picked for it.
    /// Clan A's first pick is the player's warrior.
    /// The battleground bakes a **craggier land** than the other modes: the war is
    /// fought over broken country — ridgelines, deeper draws, real relief — using
    /// the same per-mode terrain-pool knob prowl already rolls (house idiom, held
    /// under the terrain lock like every other mode's bake).
    pub const MOUNTAIN_BIAS: f32 = 1.6;
    /// Paired with the bias: lift the land so craggier does not mean *wetter* —
    /// the deepened basins drain back to marshes and draws, the carved river stays.
    pub const LAND_LIFT: f32 = 1.3;

    pub fn new_draft(picks_a: [Ancestor; 3], picks_b: [Ancestor; 3]) -> Battleground {
        let _terrain_guard = world::TERRAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        world::set_terrain_pool(Self::MOUNTAIN_BIAS, 1);
        world::set_land_lift(Self::LAND_LIFT);
        world::init_world();
        let camps = [land_near(CAMP_ANCHOR[0].0, CAMP_ANCHOR[0].1), land_near(CAMP_ANCHOR[1].0, CAMP_ANCHOR[1].1)];
        drop(_terrain_guard);

        let medics = [(camps[0].0 + 4.0, camps[0].1), (camps[1].0 - 4.0, camps[1].1)];

        // Heal Trees: one per lane, a third of the way out from each camp.
        let mut heal_trees = Vec::new();
        for team in 0..2u8 {
            for lane in 0..LANES {
                let (cx, cz) = camps[team as usize];
                let (bx, bz) = TRAIL_BEND[lane];
                let (x, z) = (cx + (bx - cx) * 0.55, cz + (bz - cz) * 0.55);
                heal_trees.push(HealTree { team, lane, x, z, hp: HEAL_TREE_HP });
            }
        }

        // Prey camps (mice and rabbits alternating), a fish run at the nearest
        // water to the centre, and the contention ground's rich prey.
        let mut prey: Vec<Prey> = PREY_CAMPS.iter().enumerate()
            .map(|(i, &(x, z))| {
                let kind = if i % 2 == 0 { PreyKind::Mouse } else { PreyKind::Rabbit };
                Prey { x, z, home: (x, z), kind, hp: kind.hp(), respawn_t: 0.0 }
            })
            .collect();
        {
            // two fishing runs, sought from mirrored anchors so neither clan's side
            // of the river eats better by construction
            let _terrain_guard = world::TERRAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            for anchor in [(-40.0, 40.0), (40.0, -40.0)] {
                if let Some((fx, fz)) = world::nearest_water(anchor, 100.0) {
                    prey.push(Prey { x: fx, z: fz, home: (fx, fz), kind: PreyKind::Fish, hp: PreyKind::Fish.hp(), respawn_t: 0.0 });
                }
            }
        }
        prey.push(Prey { x: 0.0, z: 0.0, home: (0.0, 0.0), kind: PreyKind::Rich, hp: PreyKind::Rich.hp(), respawn_t: 0.0 });

        let herb_patches = HERB_SPOTS.iter().map(|&(x, z)| HerbPatch { x, z, regrow_t: 0.0 }).collect();

        // Rosters: the drafted three a side; the player leads clan A.
        let mut warriors = Vec::new();
        for team in 0..2u8 {
            for slot in 0..3usize {
                let a = if team == CLAN_A { picks_a[slot] } else { picks_b[slot] };
                let k = ancestor::kit(a);
                let (cx, cz) = camps[team as usize];
                warriors.push(Warrior {
                    team, role: Role::Ancestor, ancestor: Some(a), lane: slot % LANES,
                    x: cx + slot as f32 * 2.0 - 2.0, z: cz, heading: 0.0,
                    hp: k.hp, max_hp: k.hp, attack: k.attack, speed: k.speed, reach: k.reach,
                    mind: Mind::Trail, carry: Carry::Nothing,
                    shield: 0.0, stealth_t: 0.0, fear_t: 0.0, slow_t: 0.0, root_t: 0.0,
                    cds: [0.0; 3], respawn_t: 0.0, gather_t: 0.0, kills: 0, duty: Duty::Border,
                    is_player: team == CLAN_A && slot == 0, seg: 0,
                });
            }
        }

        Battleground {
            warriors, heal_trees, prey, herb_patches, zones: Vec::new(),
            clans: [
                ClanState { pile: 0.0, herbs: 0, great_tree_hp: GREAT_TREE_HP },
                ClanState { pile: 0.0, herbs: 0, great_tree_hp: GREAT_TREE_HP },
            ],
            camps, medics, contention: (0.0, 0.0), contention_holder: None,
            time: 0.0, outcome: None, patrol_t: [PATROL_INTERVAL * 0.3; 2], wave_n: [0; 2], tick: 0,
            player_driven: false, input: None,
        }
    }

    pub fn set_input(&mut self, mx: f32, mz: f32, claw: bool, ability: Option<usize>) {
        self.player_driven = true;
        self.input = Some((mx, mz, claw, ability));
    }

    fn spawn_patrol(&mut self, team: u8, lane: usize, duty: Duty) {
        let (cx, cz) = self.camps[team as usize];
        let size = if self.leafbare() && duty == Duty::Border { LEAFBARE_PATROL } else { PATROL_SIZE };
        for i in 0..size {
            self.warriors.push(Warrior {
                team, role: Role::Patrol, ancestor: None, lane,
                x: cx + i as f32 * 1.5, z: cz + 1.5, heading: 0.0,
                hp: PATROL_HP, max_hp: PATROL_HP, attack: PATROL_ATK, speed: PATROL_SPEED, reach: REACH,
                mind: if duty == Duty::Hunting { Mind::Hunt } else { Mind::Trail },
                carry: Carry::Nothing,
                shield: 0.0, stealth_t: 0.0, fear_t: 0.0, slow_t: 0.0, root_t: 0.0,
                cds: [0.0; 3], respawn_t: 0.0, gather_t: 0.0, kills: 0, duty,
                is_player: false, seg: 0,
            });
        }
    }

    /// Where the border broke: the lane of a fallen enemy Heal Tree, if any. Once a
    /// breach exists the clan masses its attack there — patrols spawn onto it and
    /// warriors returning to the trail adopt it — which is what turns a three-lane
    /// stalemate into a decisive push on the Great Tree.
    fn breach_lane(&self, team: u8) -> Option<usize> {
        self.heal_trees.iter().find(|t| t.team != team && !t.alive()).map(|t| t.lane)
    }

    /// The trail waypoints for a lane, in this team's marching order.
    fn trail(&self, team: u8, lane: usize) -> [(f32, f32); 3] {
        let home = self.camps[team as usize];
        let away = self.camps[Self::enemy(team) as usize];
        [home, TRAIL_BEND[lane], away]
    }

    fn under_aura(&self, team: u8, x: f32, z: f32) -> bool {
        self.heal_trees.iter().any(|t| t.team == team && t.alive() && dist(t.x, t.z, x, z) < AURA_R)
    }

    /// Damage a warrior, shields first; the aura of a friendly Heal Tree softens it.
    fn hurt(&mut self, idx: usize, mut dmg: f32) {
        let (team, x, z) = { let w = &self.warriors[idx]; (w.team, w.x, w.z) };
        if self.under_aura(team, x, z) { dmg *= 1.0 - AURA_GUARD; }
        let respawn = self.respawn_time(team);
        let w = &mut self.warriors[idx];
        let absorbed = dmg.min(w.shield);
        w.shield -= absorbed;
        w.hp -= dmg - absorbed;
        if w.hp <= 0.0 && w.mind != Mind::Dead {
            w.hp = 0.0;
            w.mind = Mind::Dead;
            // a carried kill or sprig is dropped and lost
            w.carry = Carry::Nothing;
            w.shield = 0.0; w.stealth_t = 0.0;
            if w.role == Role::Ancestor {
                w.respawn_t = respawn;
            }
        }
    }

    fn heal(&mut self, idx: usize, amount: f32) {
        let team = self.warriors[idx].team;
        let m = self.herb_heal(team);
        let w = &mut self.warriors[idx];
        w.hp = (w.hp + amount * m).min(w.max_hp);
    }

    fn nearest_enemy(&self, of: usize, within: f32) -> Option<usize> {
        let (team, x, z) = { let w = &self.warriors[of]; (w.team, w.x, w.z) };
        let mut best: Option<(usize, f32)> = None;
        for (i, e) in self.warriors.iter().enumerate() {
            if e.team == team || !e.alive() || e.stealth_t > 0.0 { continue; }
            if !junglemap::sees(x, z, e.x, e.z) { continue; }   // brush, mist, stone
            let d = dist(x, z, e.x, e.z);
            if d < within && best.map_or(true, |b| d < b.1) { best = Some((i, d)); }
        }
        best.map(|b| b.0)
    }

    /// Fire ability `slot` (0/1 = techniques, 2 = ult) for warrior `i` if off cooldown.
    fn cast(&mut self, i: usize, slot: usize) {
        let (a, team, x, z) = {
            let w = &self.warriors[i];
            match (w.alive(), w.ancestor) { (true, Some(a)) => (a, w.team, w.x, w.z), _ => return }
        };
        if self.warriors[i].cds[slot] > 0.0 { return; }
        let k = ancestor::kit(a);
        let spec = if slot == 2 { k.ult } else { k.abilities[slot] };
        let atk = self.strike_of(i);
        let target = self.nearest_enemy(i, spec.range.max(REACH));
        match spec.verb {
            Verb::Strike => {
                let Some(t) = target else { return };
                self.hurt(t, spec.power * atk * self.strike_vs(i, t));
                self.on_hit(i, t);
                self.warriors[i].kills += (!self.warriors[t].alive()) as u32;
            }
            Verb::Dash => {
                let Some(t) = target else { return };
                let (tx, tz) = (self.warriors[t].x, self.warriors[t].z);
                let d = dist(x, z, tx, tz);
                let (nx, nz) = norm(tx - x, tz - z);
                let step = spec.range.min(d - REACH * 0.5).max(0.0);
                let bonus = self.strike_vs(i, t);
                let w = &mut self.warriors[i];
                w.x += nx * step; w.z += nz * step;
                self.hurt(t, spec.power * atk * bonus);
            }
            Verb::Aoe => {
                let victims: Vec<usize> = self.warriors.iter().enumerate()
                    .filter(|(_, e)| e.team != team && e.alive() && dist(x, z, e.x, e.z) < spec.range)
                    .map(|(j, _)| j).collect();
                if victims.is_empty() { return; }
                for v in victims { self.hurt(v, spec.power * atk); }
            }
            Verb::Shield => {
                let amount = spec.power * atk;
                for j in 0..self.warriors.len() {
                    let w = &self.warriors[j];
                    if w.team == team && w.alive() && dist(x, z, w.x, w.z) < spec.range.max(0.1) {
                        self.warriors[j].shield = self.warriors[j].shield.max(amount);
                    }
                }
            }
            Verb::Heal => {
                let amount = spec.power * atk;
                for j in 0..self.warriors.len() {
                    let w = &self.warriors[j];
                    if w.team == team && w.alive() && dist(x, z, w.x, w.z) < spec.range.max(0.1) {
                        self.heal(j, amount);
                    }
                }
            }
            Verb::Stealth => {
                let dur = if a == Ancestor::Panther { spec.dur * 1.5 } else { spec.dur };   // Silent Footfall
                self.warriors[i].stealth_t = dur;
            }
            Verb::Fear => {
                for j in 0..self.warriors.len() {
                    let e = &self.warriors[j];
                    if e.team != team && e.alive() && dist(x, z, e.x, e.z) < spec.range {
                        self.warriors[j].fear_t = spec.dur;
                        if spec.power > 0.0 { self.hurt(j, spec.power * atk); }   // the Great Caterwaul draws blood
                    }
                }
            }
            Verb::Knockback => {
                // smash everything close away from the caster — peel divers off a
                // tree, fling a brawl apart, buy a wounded packmate its retreat
                let victims: Vec<usize> = self.warriors.iter().enumerate()
                    .filter(|(_, e)| e.team != team && e.alive() && dist(x, z, e.x, e.z) < spec.range)
                    .map(|(j, _)| j).collect();
                if victims.is_empty() { return; }
                for v in victims {
                    let (vx, vz) = (self.warriors[v].x, self.warriors[v].z);
                    let (mut nx, mut nz) = norm(vx - x, vz - z);
                    if nx == 0.0 && nz == 0.0 { nx = 1.0; nz = 0.0; }
                    let w = &mut self.warriors[v];
                    w.x = (w.x + nx * 5.0).clamp(-ARENA, ARENA);
                    w.z = (w.z + nz * 5.0).clamp(-ARENA, ARENA);
                    self.hurt(v, spec.power * atk);
                }
            }
            Verb::Execute => {
                // the killing bite: its power grows with the target's missing strength
                let Some(t) = target else { return };
                let missing = 1.0 - self.warriors[t].hp / self.warriors[t].max_hp;
                self.hurt(t, spec.power * atk * (1.0 + missing * 1.2) * self.strike_vs(i, t));
                self.warriors[i].kills += (!self.warriors[t].alive()) as u32;
            }
            Verb::Zone => {
                self.zones.push(Zone { x, z, team, r: spec.range, dps: spec.power * atk / spec.dur.max(0.5), t: spec.dur });
            }
            Verb::Root => {
                for j in 0..self.warriors.len() {
                    let e = &self.warriors[j];
                    if e.team != team && e.alive() && dist(x, z, e.x, e.z) < spec.range {
                        self.warriors[j].root_t = self.warriors[j].root_t.max(spec.dur);
                        if spec.power > 0.0 { self.hurt(j, spec.power * atk); }
                    }
                }
            }
            Verb::Pull => {
                // the current takes their legs: drag every enemy near in close
                let victims: Vec<usize> = self.warriors.iter().enumerate()
                    .filter(|(_, e)| e.team != team && e.alive() && dist(x, z, e.x, e.z) < spec.range)
                    .map(|(j, _)| j).collect();
                if victims.is_empty() { return; }
                for v in victims {
                    let (vx, vz) = (self.warriors[v].x, self.warriors[v].z);
                    let d = dist(x, z, vx, vz);
                    let (nx, nz) = norm(x - vx, z - vz);
                    let step = (d - REACH * 0.6).max(0.0);   // in to the claw, not inside the caster
                    let w = &mut self.warriors[v];
                    w.x += nx * step; w.z += nz * step;
                    if spec.power > 0.0 { self.hurt(v, spec.power * atk); }
                }
            }
            Verb::Slow => {
                for j in 0..self.warriors.len() {
                    let e = &self.warriors[j];
                    if e.team != team && e.alive() && dist(x, z, e.x, e.z) < spec.range {
                        let dur = if e.ancestor == Some(Ancestor::Puma) { spec.dur * 0.5 } else { spec.dur };   // Sure-Footed
                        self.warriors[j].slow_t = dur;
                        if spec.power > 0.0 { self.hurt(j, spec.power * atk); }
                    }
                }
            }
        }
        self.warriors[i].cds[slot] = spec.cd;
    }

    /// One warrior's move speed here and now: terrain, wounds, burdens, magic.
    fn speed_of(&self, i: usize) -> f32 {
        let w = &self.warriors[i];
        let mut s = w.speed;
        if world::height_at(w.x, w.z) < world::WATER_Y + 0.2 {
            // water slows everyone except the Riverborn; Sure-Footed paws barely notice
            s *= match w.ancestor {
                Some(Ancestor::Leopard) => 1.1,
                Some(Ancestor::Puma) => 0.75,
                _ => 0.55,
            };
        }
        if w.ancestor == Some(Ancestor::Panther) && self.is_night() { s *= 1.1; }   // Silent Footfall
        if w.ancestor == Some(Ancestor::Caracal) {                                   // Skyward Hunger
            s *= 1.0 + (w.kills as f32 * 0.04).min(0.4);
        }
        if junglemap::on_wall(w.x, w.z) { s *= junglemap::SCRAMBLE; }   // claws on stone
        if w.carry != Carry::Nothing { s *= CARRY_SLOW; }
        if w.slow_t > 0.0 { s *= 0.5; }
        s
    }

    fn walk_toward(&mut self, i: usize, tx: f32, tz: f32, dt: f32) {
        if self.warriors[i].root_t > 0.0 { return; }   // held fast in the drifts
        let s = self.speed_of(i);
        let w = &mut self.warriors[i];
        let (nx, nz) = norm(tx - w.x, tz - w.z);
        w.x = (w.x + nx * s * dt).clamp(-ARENA, ARENA);
        w.z = (w.z + nz * s * dt).clamp(-ARENA, ARENA);
        if nx != 0.0 || nz != 0.0 { w.heading = nz.atan2(nx); }
    }

    fn think(&mut self, i: usize, dt: f32) {
        let (team, role, mind, x, z, frac, carry, lane) = {
            let w = &self.warriors[i];
            (w.team, w.role, w.mind, w.x, w.z, w.power_frac(), w.carry, w.lane)
        };
        let home = self.camps[team as usize];
        let enemy_camp = self.camps[Self::enemy(team) as usize];

        // fear overrides thought: run from the roar, toward home (unless held fast)
        if self.warriors[i].fear_t > 0.0 {
            self.walk_toward(i, home.0, home.1, dt);   // no-op while rooted
            return;
        }

        // the Great Tree's defence overrides retreat — the one fight to the death
        let tree_threatened = self.warriors.iter().any(|e|
            e.team != team && e.alive() && dist(e.x, e.z, home.0, home.1) < LAST_STAND_R);
        if tree_threatened && dist(x, z, home.0, home.1) < LAST_STAND_R * 2.0 && mind != Mind::Dead {
            self.warriors[i].mind = Mind::LastStand;
            if let Some(t) = self.nearest_enemy(i, LAST_STAND_R * 2.0) {
                let (tx, tz) = (self.warriors[t].x, self.warriors[t].z);
                if dist(x, z, tx, tz) > self.warriors[i].reach { self.walk_toward(i, tx, tz, dt); }
                else {
                    let dmg = self.strike_of(i) * self.strike_vs(i, t) * dt;
                    self.hurt(t, dmg);
                    self.on_hit(i, t);
                }
                if role == Role::Ancestor { self.auto_cast(i); }
            }
            return;
        } else if mind == Mind::LastStand {
            self.warriors[i].mind = Mind::Trail;
        }

        // wounded past nerve → break off and go home (patrol doctrine; ancestors too)
        if frac < NERVE && !matches!(mind, Mind::Retreat | Mind::AtMedic | Mind::Dead) {
            self.warriors[i].mind = Mind::Retreat;
        }

        match self.warriors[i].mind {
            Mind::Dead => {}
            Mind::LastStand => {}   // handled above
            Mind::Retreat => {
                let (mx, mz) = self.medics[team as usize];
                if dist(x, z, mx, mz) < MEDIC_R { self.warriors[i].mind = Mind::AtMedic; }
                else { self.walk_toward(i, mx, mz, dt); }
            }
            Mind::AtMedic => {
                self.heal(i, MEDIC_HEAL * dt);
                if self.warriors[i].power_frac() >= REJOIN {
                    if let Some(l) = self.breach_lane(team) { self.warriors[i].lane = l; }
                    self.warriors[i].mind = Mind::Trail;
                    self.warriors[i].seg = 0;
                }
            }
            Mind::CarryHome => {
                if dist(x, z, home.0, home.1) < MEDIC_R {
                    match carry {
                        Carry::Freshkill(worth) => self.clans[team as usize].pile += worth,
                        Carry::Herb => self.clans[team as usize].herbs += 1,
                        Carry::Nothing => {}
                    }
                    self.warriors[i].carry = Carry::Nothing;
                    if let Some(l) = self.breach_lane(team) { self.warriors[i].lane = l; }
                    self.warriors[i].mind =
                        if role == Role::Patrol && self.warriors[i].duty == Duty::Hunting { Mind::Hunt }
                        else { Mind::Trail };
                    self.warriors[i].seg = 0;
                } else {
                    self.walk_toward(i, home.0, home.1, dt);
                }
            }
            Mind::Gather => {
                // nearest patch with a sprig ready — walk there, crouch, pick
                let patch = self.herb_patches.iter().enumerate().filter(|(_, p)| p.ready())
                    .min_by(|a, b| dist(a.1.x, a.1.z, x, z).total_cmp(&dist(b.1.x, b.1.z, x, z)))
                    .map(|(j, _)| j);
                match patch {
                    Some(p) => {
                        let (px, pz) = (self.herb_patches[p].x, self.herb_patches[p].z);
                        if dist(x, z, px, pz) > 2.5 {
                            self.warriors[i].gather_t = 0.0;
                            self.walk_toward(i, px, pz, dt);
                        } else {
                            self.warriors[i].gather_t += dt;
                            if self.warriors[i].gather_t >= HERB_GATHER {
                                self.herb_patches[p].regrow_t = HERB_REGROW;
                                self.warriors[i].gather_t = 0.0;
                                self.warriors[i].carry = Carry::Herb;
                                self.warriors[i].mind = Mind::CarryHome;
                            }
                        }
                    }
                    None => { self.warriors[i].mind = Mind::Trail; self.warriors[i].gather_t = 0.0; }
                }
            }
            Mind::Hunt => {
                // a hunting patrol ranges the whole territory; an ancestor only detours nearby
                let reach = if role == Role::Patrol { 1e9 } else { 40.0 };
                let target = self.prey.iter().enumerate()
                    .filter(|(_, p)| p.alive() && dist(p.x, p.z, x, z) < reach
                        && (!p.rich() || self.contention_holder == Some(team) || self.contention_holder.is_none()))
                    .min_by(|a, b| dist(a.1.x, a.1.z, x, z).total_cmp(&dist(b.1.x, b.1.z, x, z)))
                    .map(|(j, _)| j);
                match target {
                    Some(p) => {
                        let (px, pz) = (self.prey[p].x, self.prey[p].z);
                        if dist(x, z, px, pz) > self.warriors[i].reach { self.walk_toward(i, px, pz, dt); }
                        else {
                            self.prey[p].hp -= self.warriors[i].attack * dt;
                            if self.prey[p].hp <= 0.0 {
                                let kind = self.prey[p].kind;
                                self.prey[p].respawn_t = PREY_RESPAWN;
                                self.prey[p].hp = kind.hp();
                                let (hx0, hz0) = self.prey[p].home;
                                self.prey[p].x = hx0; self.prey[p].z = hz0;
                                self.warriors[i].carry = Carry::Freshkill(kind.worth());
                                self.warriors[i].mind = Mind::CarryHome;
                                self.warriors[i].kills += 1;
                            }
                        }
                    }
                    None => self.warriors[i].mind = Mind::Trail,
                }
            }
            Mind::Trail | Mind::Fight => {
                // engage a nearby visible enemy first
                if let Some(t) = self.nearest_enemy(i, ENGAGE_R) {
                    self.warriors[i].mind = Mind::Fight;
                    let (tx, tz) = (self.warriors[t].x, self.warriors[t].z);
                    let reach = self.warriors[i].reach;
                    let d = dist(x, z, tx, tz);
                    if d > reach { self.walk_toward(i, tx, tz, dt); }
                    else {
                        let dmg = self.strike_of(i) * self.strike_vs(i, t) * dt;
                        let was_alive = self.warriors[t].alive();
                        self.hurt(t, dmg);
                        self.on_hit(i, t);
                        if was_alive && !self.warriors[t].alive() { self.warriors[i].kills += 1; }
                        // a ranged cat keeps its distance: yowl, and give ground
                        if reach > 4.0 && d < reach * 0.45 {
                            let (nx, nz) = norm(x - tx, z - tz);
                            self.walk_toward(i, x + nx * 6.0, z + nz * 6.0, dt);
                        }
                    }
                    if role == Role::Ancestor { self.auto_cast(i); }
                    return;
                }
                self.warriors[i].mind = Mind::Trail;

                // ancestors sometimes peel off the trail for the economy objectives
                if role == Role::Ancestor && !self.warriors[i].is_player && carry == Carry::Nothing {
                    let r = hashf(self.tick.wrapping_mul(2654435761).wrapping_add(i as u32 * 97));
                    if r < 0.005 {
                        if r < 0.0015 && self.herb_patches.iter().any(|p| p.ready()) {
                            self.warriors[i].mind = Mind::Gather;
                            return;
                        } else if self.prey.iter().any(|p| p.alive()) {
                            self.warriors[i].mind = Mind::Hunt;
                            return;
                        }
                    }
                }

                // gnaw enemy structures on the way: heal tree on this lane, then the Great Tree
                let atk = self.strike_of(i);
                let etarget = self.heal_trees.iter().position(|t|
                    t.team != team && t.alive()
                    && (t.lane == lane || dist(t.x, t.z, x, z) < 6.0)
                    && dist(t.x, t.z, x, z) < REACH + 3.0);
                if let Some(t) = etarget {
                    self.heal_trees[t].hp -= atk * dt;
                    return;
                }
                if dist(x, z, enemy_camp.0, enemy_camp.1) < REACH + 4.0 {
                    let e = Self::enemy(team) as usize;
                    let roots = self.heal_trees.iter().any(|t| t.team != team && t.alive());
                    let dmg = atk * dt * if roots { 1.0 - ROOT_GUARD } else { 1.0 };
                    self.clans[e].great_tree_hp -= dmg;
                    return;
                }

                // otherwise: march the trail
                let wp = self.trail(team, lane);
                let seg = self.warriors[i].seg.min(wp.len() - 1);
                let (tx, tz) = wp[seg];
                if dist(x, z, tx, tz) < 4.0 && seg + 1 < wp.len() { self.warriors[i].seg = seg + 1; }
                let seg = self.warriors[i].seg.min(wp.len() - 1);
                let (tx, tz) = wp[seg];
                self.walk_toward(i, tx, tz, dt);
            }
        }
    }

    /// Simple ancestor ability doctrine while fighting: ult into a crowd, shield or
    /// heal when pressed, otherwise spend a technique.
    fn auto_cast(&mut self, i: usize) {
        let Some(a) = self.warriors[i].ancestor else { return };
        if self.warriors[i].is_player && self.player_driven { return; }   // the player casts by input
        let (team, x, z, frac) = { let w = &self.warriors[i]; (w.team, w.x, w.z, w.power_frac()) };
        let k = ancestor::kit(a);
        let crowd = self.warriors.iter()
            .filter(|e| e.team != team && e.alive() && dist(x, z, e.x, e.z) < k.ult.range.max(8.0)).count();
        if crowd >= 2 && self.warriors[i].cds[2] <= 0.0 { self.cast(i, 2); return; }
        for slot in 0..2 {
            let defensive = matches!(k.abilities[slot].verb, Verb::Shield | Verb::Heal | Verb::Stealth);
            if self.warriors[i].cds[slot] <= 0.0 && (defensive == (frac < 0.7) || !defensive) {
                self.cast(i, slot);
                return;
            }
        }
    }

    pub fn step(&mut self, dt: f32) {
        if self.outcome.is_some() { return; }
        self.tick = self.tick.wrapping_add(1);
        self.time += dt;

        // timers
        for w in self.warriors.iter_mut() {
            for c in w.cds.iter_mut() { *c = (*c - dt).max(0.0); }
            w.stealth_t = (w.stealth_t - dt).max(0.0);
            w.fear_t = (w.fear_t - dt).max(0.0);
            w.slow_t = (w.slow_t - dt).max(0.0);
            w.root_t = (w.root_t - dt).max(0.0);
        }
        for p in self.prey.iter_mut() { p.respawn_t = (p.respawn_t - dt).max(0.0); }
        // land prey bolts from a close hunter, leashed to its home ground
        for pi in 0..self.prey.len() {
            let p = &self.prey[pi];
            if !p.alive() || !p.kind.flees() { continue; }
            let threat = self.warriors.iter()
                .filter(|w| w.alive() && dist(w.x, w.z, p.x, p.z) < 8.0
                    && junglemap::sees(p.x, p.z, w.x, w.z))   // a hunter in the brush spooks nothing
                .map(|w| (w.x, w.z))
                .min_by(|a, b| dist(a.0, a.1, p.x, p.z).total_cmp(&dist(b.0, b.1, p.x, p.z)));
            let p = &mut self.prey[pi];
            if let Some((tx, tz)) = threat {
                let (mut nx, mut nz) = norm(p.x - tx, p.z - tz);
                if nx == 0.0 && nz == 0.0 {   // paw right on its tail: bolt any which way
                    let a = hashf(self.tick.wrapping_add(pi as u32 * 131)) * std::f32::consts::TAU;
                    nx = a.cos(); nz = a.sin();
                }
                p.x += nx * 4.0 * dt; p.z += nz * 4.0 * dt;
            } else if dist(p.x, p.z, p.home.0, p.home.1) > 1.0 {
                let (nx, nz) = norm(p.home.0 - p.x, p.home.1 - p.z);   // drift home
                p.x += nx * 1.5 * dt; p.z += nz * 1.5 * dt;
            }
            let d = dist(p.x, p.z, p.home.0, p.home.1);
            if d > 12.0 {   // the leash: prey never abandons its ground
                let (nx, nz) = norm(p.x - p.home.0, p.z - p.home.1);
                p.x = p.home.0 + nx * 12.0; p.z = p.home.1 + nz * 12.0;
            }
        }
        for h in self.herb_patches.iter_mut() { h.regrow_t = (h.regrow_t - dt).max(0.0); }

        // patrols march out, one lane after another
        for team in 0..2u8 {
            self.patrol_t[team as usize] -= dt;
            if self.patrol_t[team as usize] <= 0.0 {
                self.patrol_t[team as usize] = PATROL_INTERVAL;
                let wave = self.wave_n[team as usize] as usize;
                self.wave_n[team as usize] += 1;
                let lane = self.breach_lane(team).unwrap_or(wave % LANES);
                let duty = if wave % 3 == 2 { Duty::Hunting } else { Duty::Border };
                self.spawn_patrol(team, lane, duty);
            }
        }

        // contention ground: held by whoever brought more living warriors
        let (cx, cz) = self.contention;
        let count = |team: u8| self.warriors.iter()
            .filter(|w| w.team == team && w.alive() && dist(w.x, w.z, cx, cz) < CONTEND_R).count();
        let (a, b) = (count(CLAN_A), count(CLAN_B));
        self.contention_holder =
            if a > b { Some(CLAN_A) } else if b > a { Some(CLAN_B) } else { self.contention_holder };

        // player input drives warrior 0 directly
        if let Some((mx, mz, claw, ability)) = self.input.take() {
            if self.warriors[0].alive() {
                if (mx != 0.0 || mz != 0.0) && self.warriors[0].root_t <= 0.0 {
                    let (nx, nz) = norm(mx, mz);
                    let s = self.speed_of(0);
                    let w = &mut self.warriors[0];
                    w.x = (w.x + nx * s * dt).clamp(-ARENA, ARENA);
                    w.z = (w.z + nz * s * dt).clamp(-ARENA, ARENA);
                    w.heading = nz.atan2(nx);
                }
                if let Some(slot) = ability { self.cast(0, slot.min(2)); }
                if claw {
                    if let Some(t) = self.nearest_enemy(0, self.warriors[0].reach) {
                        let dmg = self.strike_of(0) * dt;
                        self.hurt(t, dmg);
                    }
                }
            }
        }

        // minds (skip the player's warrior — the player is the mind)
        for i in 0..self.warriors.len() {
            if !self.warriors[i].alive() || (self.warriors[i].is_player && self.player_driven) { continue; }
            self.think(i, dt);
        }

        // scorched ground burns intruders and sears every veil away
        for zi in 0..self.zones.len() {
            let (zx, zz, zteam, zr, zdps) = {
                let z = &self.zones[zi]; (z.x, z.z, z.team, z.r, z.dps)
            };
            let victims: Vec<usize> = self.warriors.iter().enumerate()
                .filter(|(_, w)| w.team != zteam && w.alive() && dist(w.x, w.z, zx, zz) < zr)
                .map(|(j, _)| j).collect();
            for v in victims {
                self.warriors[v].stealth_t = 0.0;   // nothing hides on burning sand
                self.hurt(v, zdps * dt);
            }
        }
        for z in self.zones.iter_mut() { z.t -= dt; }
        self.zones.retain(|z| z.t > 0.0);

        // Heal Tree auras mend every ally sheltering under them (thinner in leaf-bare);
        // the Riverborn mends wherever there is water
        let aura_heal = AURA_HEAL * if self.leafbare() { LEAFBARE_AURA } else { 1.0 };
        for i in 0..self.warriors.len() {
            let (team, x, z, alive, anc) = { let w = &self.warriors[i]; (w.team, w.x, w.z, w.alive(), w.ancestor) };
            if !alive { continue; }
            if self.under_aura(team, x, z) { self.heal(i, aura_heal * dt); }
            if anc == Some(Ancestor::Leopard) && world::height_at(x, z) < world::WATER_Y + 0.2 {
                self.heal(i, 3.0 * dt);
            }
        }

        // respawns (ancestors return; fallen patrol cats are mourned, not replaced)
        for i in 0..self.warriors.len() {
            let w = &self.warriors[i];
            if w.mind == Mind::Dead && w.role == Role::Ancestor {
                let t = (self.warriors[i].respawn_t - dt).max(0.0);
                self.warriors[i].respawn_t = t;
                if t <= 0.0 {
                    let team = self.warriors[i].team;
                    let (cx, cz) = self.camps[team as usize];
                    let lane = self.breach_lane(team).unwrap_or(self.warriors[i].lane);
                    let w = &mut self.warriors[i];
                    w.x = cx; w.z = cz; w.lane = lane;
                    w.hp = w.max_hp; w.mind = Mind::Trail; w.seg = 0;
                }
            }
        }
        self.warriors.retain(|w| w.alive() || w.role == Role::Ancestor);

        // outcome
        if self.clans[CLAN_B as usize].great_tree_hp <= 0.0 { self.outcome = Some(CLAN_A); }
        else if self.clans[CLAN_A as usize].great_tree_hp <= 0.0 { self.outcome = Some(CLAN_B); }
        else if self.time >= MATCH_LEN {
            // full time: the clan whose wood still stands strongest holds the ground
            let strength = |t: usize| self.clans[t].great_tree_hp
                + self.heal_trees.iter().filter(|h| h.team as usize == t).map(|h| h.hp.max(0.0)).sum::<f32>();
            let (sa, sb) = (strength(0), strength(1));
            self.outcome = Some(if sa > sb { CLAN_A } else if sb > sa { CLAN_B } else { 2 });
        }
    }

    /// A cheap digest of the whole match state, for determinism tests.
    pub fn digest(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        let mut mix = |v: u32| { h ^= v as u64; h = h.wrapping_mul(0x100000001b3); };
        for w in &self.warriors {
            mix(w.x.to_bits()); mix(w.z.to_bits()); mix(w.hp.to_bits());
            mix(w.team as u32 ^ ((w.mind == Mind::Dead) as u32) << 8);
            mix(w.root_t.to_bits() ^ w.slow_t.to_bits().rotate_left(16));
        }
        for t in &self.heal_trees { mix(t.hp.to_bits()); }
        for z in &self.zones { mix(z.x.to_bits()); mix(z.t.to_bits()); }
        for c in &self.clans { mix(c.pile.to_bits()); mix(c.herbs); mix(c.great_tree_hp.to_bits()); }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet(bg: &mut Battleground) {
        // strip the opposing clan so a mechanism can be tested without interference
        bg.warriors.retain(|w| w.team == CLAN_A);
        bg.patrol_t = [1e9; 2];
    }

    #[test]
    fn map_is_large_with_three_trails_and_six_heal_trees() {
        let bg = Battleground::new(Ancestor::Lion);
        assert!(dist(bg.camps[0].0, bg.camps[0].1, bg.camps[1].0, bg.camps[1].1) > 200.0,
            "camps should sit a long march apart");
        assert_eq!(bg.heal_trees.len(), 2 * LANES);
        for team in 0..2u8 {
            for lane in 0..LANES {
                assert!(bg.heal_trees.iter().any(|t| t.team == team && t.lane == lane));
            }
        }
    }

    #[test]
    fn a_patrol_retreats_when_wounded_and_heals_at_the_medic() {
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        bg.spawn_patrol(CLAN_A, 1, Duty::Border);
        let p = bg.warriors.iter().position(|w| w.role == Role::Patrol).unwrap();
        // wound it past its nerve, away from home
        bg.warriors[p].x = 0.0; bg.warriors[p].z = 0.0;
        bg.warriors[p].hp = bg.warriors[p].max_hp * 0.3;
        for _ in 0..30 * 60 {
            bg.step(1.0 / 30.0);
            if !bg.warriors[p].alive() { panic!("patrol died while retreating unopposed"); }
            if bg.warriors[p].mind == Mind::Trail && bg.warriors[p].power_frac() >= REJOIN { break; }
        }
        // it broke off, reached the medic, healed up, and went back on the trail
        assert_eq!(bg.warriors[p].mind, Mind::Trail);
        assert!(bg.warriors[p].power_frac() >= REJOIN,
            "patrol should rejoin healed, got {}", bg.warriors[p].power_frac());
    }

    #[test]
    fn defending_the_great_tree_is_the_exception_to_retreat() {
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        bg.warriors.retain(|w| w.team == CLAN_A);
        bg.spawn_patrol(CLAN_A, 1, Duty::Border);
        let p = bg.warriors.iter().position(|w| w.role == Role::Patrol).unwrap();
        let (cx, cz) = bg.camps[CLAN_A as usize];
        bg.warriors[p].x = cx + 2.0; bg.warriors[p].z = cz;
        bg.warriors[p].hp = bg.warriors[p].max_hp * 0.2;   // far past its nerve
        // an enemy at the camp's edge
        bg.warriors.push(Warrior {
            team: CLAN_B, role: Role::Patrol, ancestor: None, lane: 1,
            x: cx + 8.0, z: cz, heading: 0.0,
            hp: PATROL_HP, max_hp: PATROL_HP, attack: 0.0, speed: 0.0, reach: REACH,
            mind: Mind::Trail, carry: Carry::Nothing,
            shield: 0.0, stealth_t: 0.0, fear_t: 0.0, slow_t: 0.0, root_t: 0.0,
            cds: [0.0; 3], respawn_t: 0.0, gather_t: 0.0, kills: 0, duty: Duty::Border, is_player: false, seg: 0,
        });
        bg.step(1.0 / 30.0);
        assert_eq!(bg.warriors[p].mind, Mind::LastStand, "at the Great Tree a warrior stands");
    }

    #[test]
    fn freshkill_carried_home_feeds_the_clan_and_growth_saturates() {
        let mut bg = Battleground::new(Ancestor::Lion);
        assert_eq!(bg.growth(CLAN_A), 1.0);
        bg.clans[CLAN_A as usize].pile = GROWTH_HALF;
        let g1 = bg.growth(CLAN_A);
        assert!((g1 - (1.0 + GROWTH_CAP * 0.5)).abs() < 1e-4, "half the cap at the half point");
        bg.clans[CLAN_A as usize].pile = 1e6;
        assert!(bg.growth(CLAN_A) < 1.0 + GROWTH_CAP + 1e-4, "growth must saturate");
        // and the pile fills by *carrying home*, not by the kill itself
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        let (hx, hz) = bg.camps[CLAN_A as usize];
        bg.warriors[1].carry = Carry::Freshkill(RICH_WORTH);
        bg.warriors[1].mind = Mind::CarryHome;
        bg.warriors[1].x = hx + 30.0; bg.warriors[1].z = hz;
        for _ in 0..30 * 30 { bg.step(1.0 / 30.0); if bg.clans[0].pile > 0.0 { break; } }
        assert_eq!(bg.clans[CLAN_A as usize].pile, RICH_WORTH);
        assert_eq!(bg.warriors[1].carry, Carry::Nothing);
    }

    #[test]
    fn a_kill_carried_is_a_kill_lost_on_death() {
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        bg.warriors[1].carry = Carry::Freshkill(1.0);
        let idx = 1;
        bg.hurt(idx, 1e6);
        assert!(!bg.warriors[idx].alive());
        assert_eq!(bg.warriors[idx].carry, Carry::Nothing, "the carcass drops with the carrier");
        assert_eq!(bg.clans[CLAN_A as usize].pile, 0.0);
    }

    #[test]
    fn heal_tree_aura_protects_and_mends() {
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        let t = bg.heal_trees.iter().position(|t| t.team == CLAN_A).unwrap();
        let (tx, tz) = (bg.heal_trees[t].x, bg.heal_trees[t].z);
        // under the tree, a blow lands softer
        bg.warriors[1].x = tx; bg.warriors[1].z = tz;
        bg.warriors[2].x = tx + 200.0_f32.min(ARENA - tx - 1.0).max(60.0); bg.warriors[2].z = tz;
        let (h1, h2) = (bg.warriors[1].hp, bg.warriors[2].hp);
        bg.hurt(1, 50.0);
        bg.hurt(2, 50.0);
        let (d1, d2) = (h1 - bg.warriors[1].hp, h2 - bg.warriors[2].hp);
        assert!(d1 < d2, "aura should soften blows: {d1} vs {d2}");
        assert!((d1 - 50.0 * (1.0 - AURA_GUARD)).abs() < 1e-3);
        // and it mends over time
        let before = bg.warriors[1].hp;
        for _ in 0..30 { bg.step(1.0 / 30.0); }
        assert!(bg.warriors[1].hp > before, "the Tree of Healing should mend its shelterers");
    }

    #[test]
    fn the_great_tree_is_hard_while_its_heal_trees_stand() {
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        bg.warriors.retain(|w| w.team == CLAN_B);   // only clan B attackers
        let (ax, az) = bg.camps[CLAN_A as usize];
        for w in bg.warriors.iter_mut() { w.x = ax + 2.0; w.z = az; w.mind = Mind::Trail; }
        let full = bg.clans[CLAN_A as usize].great_tree_hp;
        for _ in 0..30 * 5 { bg.step(1.0 / 30.0); }
        let guarded = full - bg.clans[CLAN_A as usize].great_tree_hp;
        // now fell every Heal Tree and gnaw again for the same span
        for t in bg.heal_trees.iter_mut().filter(|t| t.team == CLAN_A) { t.hp = 0.0; }
        for w in bg.warriors.iter_mut() { w.x = ax + 2.0; w.z = az; w.hp = w.max_hp; w.mind = Mind::Trail; }
        let mid = bg.clans[CLAN_A as usize].great_tree_hp;
        for _ in 0..30 * 5 { bg.step(1.0 / 30.0); }
        let bare = mid - bg.clans[CLAN_A as usize].great_tree_hp;
        assert!(bare > guarded * 1.5, "roots should harden the Great Tree: {guarded} vs {bare}");
    }

    #[test]
    fn herbs_delivered_quicken_the_clans_healing_and_returns() {
        let mut bg = Battleground::new(Ancestor::Lion);
        let slow = bg.respawn_time(CLAN_A);
        let m0 = bg.herb_heal(CLAN_A);
        bg.clans[CLAN_A as usize].herbs = 3;
        assert!(bg.herb_heal(CLAN_A) > m0);
        assert!(bg.respawn_time(CLAN_A) < slow);
        assert!(bg.herb_heal(CLAN_A) <= HERB_HEAL_MAX + 1e-6);
        // gathering: crouch at a ready patch, then carry the sprig home
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        let (px, pz) = (bg.herb_patches[0].x, bg.herb_patches[0].z);
        bg.warriors[1].x = px; bg.warriors[1].z = pz;
        bg.warriors[1].mind = Mind::Gather;
        for _ in 0..30 * 10 { bg.step(1.0 / 30.0); if bg.warriors[1].carry == Carry::Herb { break; } }
        assert_eq!(bg.warriors[1].carry, Carry::Herb, "crouching long enough picks the sprig");
        assert!(!bg.herb_patches[0].ready(), "a picked patch regrows on a timer");
        assert_eq!(bg.clans[CLAN_A as usize].herbs, 0, "picking alone delivers nothing");
        for _ in 0..30 * 60 { bg.step(1.0 / 30.0); if bg.clans[0].herbs > 0 { break; } }
        assert_eq!(bg.clans[CLAN_A as usize].herbs, 1, "the sprig must reach the medic cat");
    }

    #[test]
    fn contention_ground_follows_presence() {
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        for w in bg.warriors.iter_mut() { w.x = -140.0; w.z = -140.0; w.mind = Mind::Retreat; }
        // clan B masses at the centre
        let n = bg.warriors.len();
        for i in 0..n {
            if bg.warriors[i].team == CLAN_B { bg.warriors[i].x = 1.0; bg.warriors[i].z = 1.0; bg.warriors[i].mind = Mind::Retreat; }
        }
        bg.step(1.0 / 30.0);
        assert_eq!(bg.contention_holder, Some(CLAN_B));
    }

    #[test]
    fn felling_the_great_tree_ends_the_match() {
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.clans[CLAN_B as usize].great_tree_hp = 0.5;
        bg.clans[CLAN_B as usize].great_tree_hp -= 1.0;
        bg.step(1.0 / 30.0);
        assert_eq!(bg.outcome, Some(CLAN_A));
        let d = bg.digest();
        bg.step(1.0 / 30.0);
        assert_eq!(bg.digest(), d, "a finished match is frozen");
    }

    #[test]
    fn ability_verbs_do_what_they_say() {
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        // a shielded lion shrugs off exactly the absorbed amount
        bg.warriors[0].is_player = false;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        bg.warriors[1].x = x; bg.warriors[1].z = z;   // packmate in shield range
        bg.cast(0, 1);   // Mane of Dawn
        assert!(bg.warriors[0].shield > 0.0 && bg.warriors[1].shield > 0.0);
        let hp = bg.warriors[0].hp;
        let small = bg.warriors[0].shield * 0.5;
        // move off any aura so the arithmetic is exact
        bg.heal_trees.clear();
        bg.hurt(0, small);
        assert_eq!(bg.warriors[0].hp, hp, "the shield eats the whole small blow");
        // stealth hides a tiger from targeting
        let mut bg = Battleground::new(Ancestor::Tiger);
        bg.patrol_t = [1e9; 2];
        bg.warriors[0].stealth_t = 5.0;
        let seen = bg.warriors.iter().enumerate()
            .filter(|(_, w)| w.team == CLAN_B).map(|(i, _)| i).next().unwrap();
        bg.warriors[seen].x = bg.warriors[0].x; bg.warriors[seen].z = bg.warriors[0].z;
        assert!(bg.nearest_enemy(seen, 50.0).map(|t| bg.warriors[t].stealth_t <= 0.0).unwrap_or(true),
            "a veiled cat cannot be picked as a target");
    }

    #[test]
    fn hunting_patrols_feed_the_pile_without_orders() {
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        bg.warriors.clear();   // only the patrol works
        bg.spawn_patrol(CLAN_A, 0, Duty::Hunting);
        for _ in 0..30 * 180 {
            bg.step(1.0 / 30.0);
            if bg.clans[CLAN_A as usize].pile > 0.0 { break; }
        }
        assert!(bg.clans[CLAN_A as usize].pile > 0.0,
            "a hunting patrol should kill, carry home, and deposit on its own");
        assert!(bg.warriors.iter().any(|w| w.duty == Duty::Hunting && matches!(w.mind, Mind::Hunt | Mind::CarryHome)),
            "after depositing, a hunting patrol goes back out");
    }

    #[test]
    fn prey_flees_the_hunter_but_never_abandons_its_ground() {
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        let p = bg.prey.iter().position(|p| p.kind.flees()).unwrap();
        let (hx, hz) = bg.prey[p].home;
        bg.warriors[1].x = hx; bg.warriors[1].z = hz;
        bg.warriors[1].mind = Mind::Retreat;   // stand there menacingly, don't hunt
        bg.warriors[1].hp = bg.warriors[1].max_hp;
        for w in bg.warriors.iter_mut() { if !w.is_player { w.mind = Mind::Retreat; } }
        bg.warriors[1].mind = Mind::AtMedic;   // hold position (medic check fails far away → stays)
        bg.warriors[1].x = hx + 1.0; bg.warriors[1].z = hz;
        let d0 = dist(bg.prey[p].x, bg.prey[p].z, hx, hz);
        for _ in 0..30 * 10 { bg.warriors[1].x = hx + 1.0; bg.warriors[1].z = hz; bg.step(1.0 / 30.0); }
        let d1 = dist(bg.prey[p].x, bg.prey[p].z, hx, hz);
        assert!(d1 > d0 + 2.0, "prey should bolt from a lurking cat: {d0} -> {d1}");
        assert!(d1 <= 12.0 + 1e-3, "but the leash holds it to its ground");
        assert!(!bg.prey.iter().any(|q| q.kind == PreyKind::Fish && q.kind.flees()),
            "fish hold to the water instead of bolting overland");
    }

    #[test]
    fn night_belongs_to_the_ambush_ancestors() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Tiger, Ancestor::Lion, Ancestor::Leopard],
            [Ancestor::Puma, Ancestor::Caracal, Ancestor::Panther]);
        quiet(&mut bg);
        assert!(!bg.is_night());
        let day = bg.strike_of(0);
        bg.time = DAY_LEN + 1.0;   // the first night watch
        assert!(bg.is_night());
        let night = bg.strike_of(0);
        assert!(night > day * 1.2, "Night's Hunger: {day} vs {night}");
        // and the veil counts as night even at noon
        bg.time = 0.0;
        bg.warriors[0].stealth_t = 2.0;
        assert!(bg.strike_of(0) > day * 1.2);
    }

    #[test]
    fn prides_warmth_strengthens_packmates_near_goldenmane() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Lion, Ancestor::Puma, Ancestor::Caracal],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther]);
        quiet(&mut bg);
        let (lx, lz) = (bg.warriors[0].x, bg.warriors[0].z);
        bg.warriors[1].x = lx + 100.0; bg.warriors[1].z = lz;
        let far = bg.strike_of(1);
        bg.warriors[1].x = lx + 2.0; bg.warriors[1].z = lz;
        let near = bg.strike_of(1);
        assert!(near > far * 1.1, "beside the lion, allies strike harder: {far} vs {near}");
    }

    #[test]
    fn leafbare_thins_the_auras_and_swells_the_patrols() {
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        assert!(!bg.leafbare());
        bg.time = LEAFBARE_AT + 1.0;
        assert!(bg.leafbare());
        let before = bg.warriors.len();
        bg.spawn_patrol(CLAN_A, 0, Duty::Border);
        assert_eq!(bg.warriors.len() - before, LEAFBARE_PATROL,
            "leaf-bare border patrols march {LEAFBARE_PATROL} strong");
        // aura mending is thinner in leaf-bare
        let t = bg.heal_trees.iter().position(|t| t.team == CLAN_A).unwrap();
        let (tx, tz) = (bg.heal_trees[t].x, bg.heal_trees[t].z);
        bg.warriors[1].x = tx; bg.warriors[1].z = tz;
        bg.warriors[1].mind = Mind::AtMedic;   // hold still (far from the medic → stays put)
        bg.warriors[1].hp = 10.0;
        bg.step(1.0);
        let gained_bare = bg.warriors[1].hp - 10.0 - MEDIC_HEAL * 0.0;
        bg.time = 0.0;
        bg.warriors[1].hp = 10.0;
        bg.step(1.0);
        let gained_green = bg.warriors[1].hp - 10.0;
        assert!(gained_bare < gained_green, "leaf-bare aura {gained_bare} vs greenleaf {gained_green}");
    }

    #[test]
    fn a_draft_fields_exactly_the_picked_ancestors() {
        let a = [Ancestor::Caracal, Ancestor::Caracal, Ancestor::Puma];
        let b = [Ancestor::Lion, Ancestor::Tiger, Ancestor::Leopard];
        let bg = Battleground::new_draft(a, b);
        let picks = |team: u8| -> Vec<Ancestor> {
            bg.warriors.iter().filter(|w| w.team == team && w.role == Role::Ancestor)
                .map(|w| w.ancestor.unwrap()).collect()
        };
        assert_eq!(picks(CLAN_A), a.to_vec());
        assert_eq!(picks(CLAN_B), b.to_vec());
        assert!(bg.warriors[0].is_player);
    }

    #[test]
    fn a_yowler_strikes_from_across_the_clearing() {
        use crate::ancestor::kit;
        let mut bg = Battleground::new_draft(
            [Ancestor::Serval, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther]);
        bg.patrol_t = [1e9; 2];
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        let d = kit(Ancestor::Serval).reach - 1.0;   // in yowl range, far past claw range
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = x + d; bg.warriors[e].z = z;
        bg.warriors[e].mind = Mind::AtMedic;   // hold still
        let hp0 = bg.warriors[e].hp;
        bg.step(1.0 / 30.0);
        assert!(bg.warriors[e].hp < hp0, "the yowl should land without closing");
        assert!(dist(bg.warriors[0].x, bg.warriors[0].z, x, z) < 0.5,
            "a ranged cat in range should not have charged in");
        // and the Far Song bites harder from a distance than muzzle-to-muzzle
        assert!(bg.strike_vs(0, e) > 1.0);
        bg.warriors[e].x = x + 1.0;
        assert!((bg.strike_vs(0, e) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn no_cat_ever_yowled_down_a_tree() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Serval, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther]);
        quiet(&mut bg);
        // park the yowler in yowl range of an enemy Heal Tree — but beyond the claw
        bg.warriors.retain(|w| w.role == Role::Ancestor);
        let t = bg.heal_trees.iter().position(|t| t.team == CLAN_B).unwrap();
        let (tx, tz) = (bg.heal_trees[t].x, bg.heal_trees[t].z);
        let lane = bg.heal_trees[t].lane;
        bg.warriors[0].x = tx + 8.0; bg.warriors[0].z = tz;
        bg.warriors[0].lane = lane;
        let hp0 = bg.heal_trees[t].hp;
        for _ in 0..15 {
            bg.warriors[0].x = tx + 8.0; bg.warriors[0].z = tz;   // hold it at range
            bg.warriors[0].mind = Mind::Trail;
            bg.step(1.0 / 30.0);
        }
        assert_eq!(bg.heal_trees[t].hp, hp0, "structures must be gnawed up close, never yowled");
    }

    #[test]
    fn boulder_paw_flings_the_close_pressed_away() {
        let mut bg = Battleground::new_draft(
            [Ancestor::CaveLion, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther]);
        bg.patrol_t = [1e9; 2];
        bg.warriors[0].is_player = false;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = x + 1.5; bg.warriors[e].z = z;
        let d0 = dist(bg.warriors[e].x, bg.warriors[e].z, x, z);
        let hp0 = bg.warriors[e].hp;
        bg.cast(0, 0);   // Boulder Paw
        let d1 = dist(bg.warriors[e].x, bg.warriors[e].z, x, z);
        assert!(d1 > d0 + 3.0, "the struck are hurled away: {d0} -> {d1}");
        assert!(bg.warriors[e].hp < hp0);
    }

    #[test]
    fn deep_wounds_deep_fury_and_the_saber_finishing() {
        let mut bg = Battleground::new_draft(
            [Ancestor::CaveLion, Ancestor::Smilodon, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther]);
        quiet(&mut bg);
        // Oldroar hits harder the closer it is to falling
        let whole = bg.strike_of(0);
        bg.warriors[0].hp = bg.warriors[0].max_hp * 0.2;
        let bleeding = bg.strike_of(0);
        assert!(bleeding > whole * 1.3, "fury should rise with the wounds: {whole} vs {bleeding}");
        // Twofang bites deeper below half strength — and the Killing Bite deeper still
        let mut bg = Battleground::new_draft(
            [Ancestor::Smilodon, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther]);
        bg.patrol_t = [1e9; 2];
        bg.warriors[0].is_player = false;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = x + 2.0; bg.warriors[e].z = z;
        assert!((bg.strike_vs(0, e) - 1.0).abs() < 1e-6, "no bonus against the hale");
        bg.warriors[e].hp = bg.warriors[e].max_hp * 0.3;
        assert!(bg.strike_vs(0, e) > 1.3, "the saber finishes the wounded");
        let hp_hurt = bg.warriors[e].hp;
        bg.warriors[e].shield = 0.0;
        bg.cast(0, 2);   // The Killing Bite
        let bite_wounded = hp_hurt - bg.warriors[e].hp.max(0.0);
        assert!(bite_wounded > 0.0);
        // against a hale target the same bite is much smaller
        bg.warriors[e].hp = bg.warriors[e].max_hp;
        bg.warriors[0].cds[2] = 0.0;
        bg.cast(0, 2);
        let bite_hale = bg.warriors[e].max_hp - bg.warriors[e].hp;
        assert!(bite_wounded > bite_hale * 1.5,
            "the execute must scale with missing strength: hale {bite_hale} vs wounded {bite_wounded}");
    }

    #[test]
    fn nothing_hides_on_burning_sand() {
        let mut bg = Battleground::new_draft(
            [Ancestor::SunCat, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Panther, Ancestor::Tiger, Ancestor::Leopard]);
        bg.patrol_t = [1e9; 2];
        bg.warriors[0].is_player = false;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = x + 2.0; bg.warriors[e].z = z;
        bg.warriors[e].stealth_t = 10.0;   // Nightpelt under the veil
        bg.warriors[e].mind = Mind::AtMedic;
        bg.cast(0, 1);   // Noon Blaze
        assert!(!bg.zones.is_empty(), "the ground should be scorched");
        let hp0 = bg.warriors[e].hp;
        bg.step(1.0 / 30.0);
        assert_eq!(bg.warriors[e].stealth_t, 0.0, "the veil must not survive the light");
        assert!(bg.warriors[e].hp < hp0, "burning sand burns");
        // the scorch expires
        for _ in 0..30 * 10 { bg.step(1.0 / 30.0); }
        assert!(bg.zones.is_empty(), "scorched ground cools");
    }

    #[test]
    fn the_sun_and_the_night_mirror_each_other() {
        let mut bg = Battleground::new_draft(
            [Ancestor::SunCat, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther]);
        quiet(&mut bg);
        let day = bg.strike_of(0);
        bg.time = DAY_LEN + 1.0;   // night falls
        let night = bg.strike_of(0);
        assert!(day > night * 1.2, "the day belongs to Suneye: {night} vs {day}");
    }

    #[test]
    fn snowbind_holds_them_fast_but_not_forever() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Lynx, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Caracal, Ancestor::Leopard, Ancestor::Panther]);
        bg.patrol_t = [1e9; 2];
        bg.warriors[0].is_player = false;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = x + 4.0; bg.warriors[e].z = z;
        bg.cast(0, 0);   // Snowbind
        assert!(bg.warriors[e].root_t > 0.0);
        // held fast: the fastest ancestor alive cannot move a whisker
        let (ex, ez) = (bg.warriors[e].x, bg.warriors[e].z);
        bg.warriors[e].mind = Mind::Retreat;   // it *wants* to run home
        bg.step(1.0 / 30.0);
        assert!(dist(bg.warriors[e].x, bg.warriors[e].z, ex, ez) < 1e-4,
            "a rooted cat cannot move");
        // but hard CC always expires
        for _ in 0..30 * 3 { bg.step(1.0 / 30.0); }
        assert_eq!(bg.warriors[e].root_t, 0.0);
        // and Hush of Snows: an ordinary blow from the lynx chills
        bg.warriors[e].x = x + 1.0; bg.warriors[e].z = z;
        bg.warriors[e].slow_t = 0.0;
        bg.on_hit(0, e);
        assert!(bg.warriors[e].slow_t > 0.0, "every blow carries the cold");
    }

    #[test]
    fn undertow_gathers_the_scattered_fight() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Jaguar, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Caracal]);
        bg.patrol_t = [1e9; 2];
        bg.warriors[0].is_player = false;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        // scatter the enemy clan around the jaguar at pull range
        let enemies: Vec<usize> = bg.warriors.iter().enumerate()
            .filter(|(_, w)| w.team == CLAN_B).map(|(i, _)| i).collect();
        for (k, &e) in enemies.iter().enumerate() {
            let a = k as f32 * 2.0;
            bg.warriors[e].x = x + a.cos() * 8.0;
            bg.warriors[e].z = z + a.sin() * 8.0;
        }
        let before: f32 = enemies.iter().map(|&e| dist(bg.warriors[e].x, bg.warriors[e].z, x, z)).sum();
        bg.cast(0, 0);   // Undertow
        let after: f32 = enemies.iter().map(|&e| dist(bg.warriors[e].x, bg.warriors[e].z, x, z)).sum();
        assert!(after < before * 0.5, "the current gathers them in: {before} -> {after}");
        for &e in &enemies {
            assert!(dist(bg.warriors[e].x, bg.warriors[e].z, x, z) > 1.0,
                "dragged to the claw, not inside the caster");
        }
        // The River Remembers: a held target is bitten deeper
        let e = enemies[0];
        assert!((bg.strike_vs(0, e) - 1.0).abs() < 1e-6);
        bg.warriors[e].root_t = 1.0;
        assert!(bg.strike_vs(0, e) > 1.1, "the held are already caught");
    }

    #[test]
    fn the_jungle_hides_a_waiting_warrior() {
        use crate::junglemap;
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        let th = junglemap::thickets()[1];   // clean eastern sightline (no wall)
        // an enemy lurks in the brush; a clan A warrior stands in the open nearby
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = th.x; bg.warriors[e].z = th.z;
        bg.warriors[0].x = th.x + 20.0; bg.warriors[0].z = th.z;
        assert!(bg.nearest_enemy(0, 50.0).is_none(),
            "from the open, the brush shows nothing");
        // step to the thicket's edge and the rustling gives them away
        bg.warriors[0].x = th.x + junglemap::CLOSE_SENSE - 1.0;
        assert_eq!(bg.nearest_enemy(0, 50.0), Some(e));
        // and the hidden cat has seen the open-ground warrior the whole time
        bg.warriors[0].x = th.x + 20.0;
        assert_eq!(bg.nearest_enemy(e, 50.0), Some(0), "ambush sight is one-way");
        // the ambush kits treat the brush as their darkness
        let mut bg2 = Battleground::new_draft(
            [Ancestor::Tiger, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Leopard, Ancestor::Caracal, Ancestor::Panther]);
        quiet(&mut bg2);
        // baseline away from the camp: at spawn the Tiger stands beside its Lion
        // packmate, and Pride's Warmth would pollute the comparison
        bg2.warriors[0].x = th.x + 40.0; bg2.warriors[0].z = th.z;
        let open = bg2.strike_of(0);
        bg2.warriors[0].x = th.x; bg2.warriors[0].z = th.z;
        assert!(bg2.strike_of(0) > open * 1.2, "Night's Hunger feeds in the brush too");
    }

    #[test]
    fn stone_walls_cut_sight_and_slow_the_scramble() {
        use crate::junglemap;
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        let w = junglemap::walls()[0];
        let q = (w.ax + (w.bx - w.ax) * 0.2, w.az + (w.bz - w.az) * 0.2);
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[0].x = q.0 + 10.0; bg.warriors[0].z = q.1 - 10.0;
        bg.warriors[e].x = q.0 - 10.0; bg.warriors[e].z = q.1 + 10.0;
        assert!(bg.nearest_enemy(0, 60.0).is_none(), "sight does not cross stone");
        // through the pass, the same spacing sees fine
        let (mx, mz) = ((w.ax + w.bx) * 0.5, (w.az + w.bz) * 0.5);
        bg.warriors[0].x = mx + 10.0; bg.warriors[0].z = mz - 10.0;
        bg.warriors[e].x = mx - 10.0; bg.warriors[e].z = mz + 10.0;
        assert_eq!(bg.nearest_enemy(0, 60.0), Some(e));
        // and stone underfoot is a scramble
        bg.warriors[0].x = q.0; bg.warriors[0].z = q.1;
        let on = bg.speed_of(0);
        bg.warriors[0].x = mx; bg.warriors[0].z = mz;
        let pass = bg.speed_of(0);
        assert!(on < pass * 0.6, "scrambling over stone: {on} vs {pass} through the pass");
    }

    #[test]
    fn prey_does_not_fear_the_hunter_it_cannot_see() {
        use crate::junglemap;
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        // a prey camp with brush beside it — the layout placed it there on purpose
        let th = junglemap::thickets()[1];
        let p = bg.prey.iter().position(|p| p.kind.flees()
            && dist(p.home.0, p.home.1, th.x, th.z) < 20.0).expect("brush beside a prey camp");
        let (hx, hz) = bg.prey[p].home;
        // hunter crouched in the brush, well inside spook range of the prey — and
        // as a control, the same crouch in the open must send it bolting
        let (cx, cz) = (th.x, th.z - 2.5);
        assert!(dist(cx, cz, hx, hz) < 7.0, "the crouch spot must be in spook range");
        bg.warriors[1].x = cx; bg.warriors[1].z = cz;
        bg.warriors[1].mind = Mind::AtMedic;
        let d0 = dist(bg.prey[p].x, bg.prey[p].z, hx, hz);
        for _ in 0..30 * 5 { bg.warriors[1].x = cx; bg.warriors[1].z = cz; bg.step(1.0 / 30.0); }
        let d1 = dist(bg.prey[p].x, bg.prey[p].z, hx, hz);
        assert!(d1 < d0 + 1.0, "a hunter in the brush spooks nothing: {d0} -> {d1}");
    }

    #[test]
    fn matches_are_deterministic() {
        let run = || {
            let mut bg = Battleground::new(Ancestor::Panther);
            for _ in 0..30 * 40 { bg.step(1.0 / 30.0); }
            bg.digest()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn a_full_match_resolves() {
        let mut bg = Battleground::new(Ancestor::Lion);
        let mut steps = 0u32;
        while bg.outcome.is_none() && steps < (MATCH_LEN as u32 + 5) * 30 {
            bg.step(1.0 / 30.0);
            steps += 1;
        }
        assert!(bg.outcome.is_some(), "a match must end by felling or full time");
        // the economy actually ran: somebody ate
        assert!(bg.clans[0].pile + bg.clans[1].pile > 0.0, "no freshkill was ever brought home");
    }
}
