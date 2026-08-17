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

const RESPAWN_BASE: f32 = 14.0;
const REACH: f32 = 2.8;              // claw range
const ENGAGE_R: f32 = 16.0;          // how far a warrior will break trail to fight
const MATCH_LEN: f32 = 8.0 * 60.0;

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
    pub mind: Mind,
    pub carry: Carry,
    pub shield: f32,      // absorbs damage first (Mane of Dawn / Stone Patience)
    pub stealth_t: f32,   // unseen/untargetable
    pub fear_t: f32,      // fleeing a Great Roar
    pub slow_t: f32,
    pub cds: [f32; 3],    // ability 1, ability 2, ult
    pub respawn_t: f32,
    pub gather_t: f32,
    pub kills: u32,
    pub is_player: bool,
    seg: usize,           // trail waypoint progress
}

impl Warrior {
    pub fn alive(&self) -> bool { self.mind != Mind::Dead }
    fn power_frac(&self) -> f32 { self.hp / self.max_hp }
}

pub struct HealTree { pub team: u8, pub lane: usize, pub x: f32, pub z: f32, pub hp: f32 }
impl HealTree { pub fn alive(&self) -> bool { self.hp > 0.0 } }

pub struct Prey { pub x: f32, pub z: f32, pub hp: f32, pub worth: f32, pub respawn_t: f32, pub rich: bool }
impl Prey { pub fn alive(&self) -> bool { self.respawn_t <= 0.0 } }

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
    pub clans: [ClanState; 2],
    pub camps: [(f32, f32); 2],
    pub medics: [(f32, f32); 2],
    pub contention: (f32, f32),
    pub contention_holder: Option<u8>,
    pub time: f32,
    pub outcome: Option<u8>,          // winning clan, or 2 = draw at full time
    patrol_t: [f32; 2],
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
    fn respawn_time(&self, team: u8) -> f32 {
        RESPAWN_BASE * HERB_RESPAWN_CUT.powi(self.clans[team as usize].herbs.min(6) as i32)
    }
    fn enemy(team: u8) -> u8 { 1 - team }

    pub fn new(player: Ancestor) -> Battleground {
        let _terrain_guard = world::TERRAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

        // Prey camps + the contention ground's rich prey.
        let mut prey: Vec<Prey> = PREY_CAMPS.iter()
            .map(|&(x, z)| Prey { x, z, hp: PREY_HP, worth: PREY_WORTH, respawn_t: 0.0, rich: false })
            .collect();
        prey.push(Prey { x: 0.0, z: 0.0, hp: PREY_HP * 1.5, worth: RICH_WORTH, respawn_t: 0.0, rich: true });

        let herb_patches = HERB_SPOTS.iter().map(|&(x, z)| HerbPatch { x, z, regrow_t: 0.0 }).collect();

        // Rosters: three ancestors a side; the player leads clan A on the middle trail.
        let pool = ancestor::ALL;
        let mut warriors = Vec::new();
        for team in 0..2u8 {
            for slot in 0..3usize {
                let a = if team == CLAN_A && slot == 0 { player } else {
                    // fill without duplicating the player's pick on their own side
                    let mut i = (team as usize * 3 + slot) % pool.len();
                    while team == CLAN_A && pool[i] == player { i = (i + 1) % pool.len(); }
                    pool[i]
                };
                let k = ancestor::kit(a);
                let (cx, cz) = camps[team as usize];
                warriors.push(Warrior {
                    team, role: Role::Ancestor, ancestor: Some(a), lane: slot % LANES,
                    x: cx + slot as f32 * 2.0 - 2.0, z: cz, heading: 0.0,
                    hp: k.hp, max_hp: k.hp, attack: k.attack, speed: k.speed,
                    mind: Mind::Trail, carry: Carry::Nothing,
                    shield: 0.0, stealth_t: 0.0, fear_t: 0.0, slow_t: 0.0,
                    cds: [0.0; 3], respawn_t: 0.0, gather_t: 0.0, kills: 0,
                    is_player: team == CLAN_A && slot == 0, seg: 0,
                });
            }
        }

        Battleground {
            warriors, heal_trees, prey, herb_patches,
            clans: [
                ClanState { pile: 0.0, herbs: 0, great_tree_hp: GREAT_TREE_HP },
                ClanState { pile: 0.0, herbs: 0, great_tree_hp: GREAT_TREE_HP },
            ],
            camps, medics, contention: (0.0, 0.0), contention_holder: None,
            time: 0.0, outcome: None, patrol_t: [PATROL_INTERVAL * 0.3; 2], tick: 0,
            player_driven: false, input: None,
        }
    }

    pub fn set_input(&mut self, mx: f32, mz: f32, claw: bool, ability: Option<usize>) {
        self.player_driven = true;
        self.input = Some((mx, mz, claw, ability));
    }

    fn spawn_patrol(&mut self, team: u8, lane: usize) {
        let (cx, cz) = self.camps[team as usize];
        for i in 0..PATROL_SIZE {
            self.warriors.push(Warrior {
                team, role: Role::Patrol, ancestor: None, lane,
                x: cx + i as f32 * 1.5, z: cz + 1.5, heading: 0.0,
                hp: PATROL_HP, max_hp: PATROL_HP, attack: PATROL_ATK, speed: PATROL_SPEED,
                mind: Mind::Trail, carry: Carry::Nothing,
                shield: 0.0, stealth_t: 0.0, fear_t: 0.0, slow_t: 0.0,
                cds: [0.0; 3], respawn_t: 0.0, gather_t: 0.0, kills: 0,
                is_player: false, seg: 0,
            });
        }
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
        let atk = self.warriors[i].attack * self.growth(team);
        let target = self.nearest_enemy(i, spec.range.max(REACH));
        match spec.verb {
            Verb::Strike => {
                let Some(t) = target else { return };
                self.hurt(t, spec.power * atk);
                self.warriors[i].kills += (!self.warriors[t].alive()) as u32;
            }
            Verb::Dash => {
                let Some(t) = target else { return };
                let (tx, tz) = (self.warriors[t].x, self.warriors[t].z);
                let d = dist(x, z, tx, tz);
                let (nx, nz) = norm(tx - x, tz - z);
                let step = spec.range.min(d - REACH * 0.5).max(0.0);
                let w = &mut self.warriors[i];
                w.x += nx * step; w.z += nz * step;
                self.hurt(t, spec.power * atk);
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
            Verb::Stealth => { self.warriors[i].stealth_t = spec.dur; }
            Verb::Fear => {
                for j in 0..self.warriors.len() {
                    let e = &self.warriors[j];
                    if e.team != team && e.alive() && dist(x, z, e.x, e.z) < spec.range {
                        self.warriors[j].fear_t = spec.dur;
                    }
                }
            }
            Verb::Slow => {
                for j in 0..self.warriors.len() {
                    let e = &self.warriors[j];
                    if e.team != team && e.alive() && dist(x, z, e.x, e.z) < spec.range {
                        self.warriors[j].slow_t = spec.dur;
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
            // water slows everyone except the Riverborn
            s *= if w.ancestor == Some(Ancestor::Leopard) { 1.1 } else { 0.55 };
        }
        if w.carry != Carry::Nothing { s *= CARRY_SLOW; }
        if w.slow_t > 0.0 { s *= 0.5; }
        s
    }

    fn walk_toward(&mut self, i: usize, tx: f32, tz: f32, dt: f32) {
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

        // fear overrides thought: run from the roar, toward home
        if self.warriors[i].fear_t > 0.0 {
            self.walk_toward(i, home.0, home.1, dt);
            return;
        }

        // the Great Tree's defence overrides retreat — the one fight to the death
        let tree_threatened = self.warriors.iter().any(|e|
            e.team != team && e.alive() && dist(e.x, e.z, home.0, home.1) < LAST_STAND_R);
        if tree_threatened && dist(x, z, home.0, home.1) < LAST_STAND_R * 2.0 && mind != Mind::Dead {
            self.warriors[i].mind = Mind::LastStand;
            if let Some(t) = self.nearest_enemy(i, LAST_STAND_R * 2.0) {
                let (tx, tz) = (self.warriors[t].x, self.warriors[t].z);
                if dist(x, z, tx, tz) > REACH { self.walk_toward(i, tx, tz, dt); }
                else {
                    let dmg = self.warriors[i].attack * self.growth(team) * dt;
                    self.hurt(t, dmg);
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
                    self.warriors[i].mind = Mind::Trail;
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
                let target = self.prey.iter().position(|p| p.alive() && dist(p.x, p.z, x, z) < 40.0
                    && (!p.rich || self.contention_holder == Some(team) || self.contention_holder.is_none()));
                match target {
                    Some(p) => {
                        let (px, pz) = (self.prey[p].x, self.prey[p].z);
                        if dist(x, z, px, pz) > REACH { self.walk_toward(i, px, pz, dt); }
                        else {
                            self.prey[p].hp -= self.warriors[i].attack * dt;
                            if self.prey[p].hp <= 0.0 {
                                let worth = self.prey[p].worth;
                                self.prey[p].respawn_t = PREY_RESPAWN;
                                self.prey[p].hp = if self.prey[p].rich { PREY_HP * 1.5 } else { PREY_HP };
                                self.warriors[i].carry = Carry::Freshkill(worth);
                                self.warriors[i].mind = Mind::CarryHome;
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
                    if dist(x, z, tx, tz) > REACH { self.walk_toward(i, tx, tz, dt); }
                    else {
                        let dmg = self.warriors[i].attack * self.growth(team) * dt;
                        let was_alive = self.warriors[t].alive();
                        self.hurt(t, dmg);
                        if was_alive && !self.warriors[t].alive() { self.warriors[i].kills += 1; }
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
                let atk = self.warriors[i].attack * self.growth(team);
                let etarget = self.heal_trees.iter().position(|t|
                    t.team != team && t.alive() && t.lane == lane && dist(t.x, t.z, x, z) < REACH + 1.5);
                if let Some(t) = etarget {
                    self.heal_trees[t].hp -= atk * dt;
                    return;
                }
                if dist(x, z, enemy_camp.0, enemy_camp.1) < REACH + 2.5 {
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
        }
        for p in self.prey.iter_mut() { p.respawn_t = (p.respawn_t - dt).max(0.0); }
        for h in self.herb_patches.iter_mut() { h.regrow_t = (h.regrow_t - dt).max(0.0); }

        // patrols march out, one lane after another
        for team in 0..2u8 {
            self.patrol_t[team as usize] -= dt;
            if self.patrol_t[team as usize] <= 0.0 {
                self.patrol_t[team as usize] = PATROL_INTERVAL;
                let lane = (self.tick as usize / 7 + team as usize) % LANES;
                self.spawn_patrol(team, lane);
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
                if mx != 0.0 || mz != 0.0 {
                    let (nx, nz) = norm(mx, mz);
                    let s = self.speed_of(0);
                    let w = &mut self.warriors[0];
                    w.x = (w.x + nx * s * dt).clamp(-ARENA, ARENA);
                    w.z = (w.z + nz * s * dt).clamp(-ARENA, ARENA);
                    w.heading = nz.atan2(nx);
                }
                if let Some(slot) = ability { self.cast(0, slot.min(2)); }
                if claw {
                    if let Some(t) = self.nearest_enemy(0, REACH) {
                        let dmg = self.warriors[0].attack * self.growth(CLAN_A) * dt * 60.0 * (1.0 / 60.0);
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

        // Heal Tree auras mend every ally sheltering under them
        for i in 0..self.warriors.len() {
            let (team, x, z, alive) = { let w = &self.warriors[i]; (w.team, w.x, w.z, w.alive()) };
            if alive && self.under_aura(team, x, z) { self.heal(i, AURA_HEAL * dt); }
        }

        // respawns (ancestors return; fallen patrol cats are mourned, not replaced)
        for i in 0..self.warriors.len() {
            let w = &self.warriors[i];
            if w.mind == Mind::Dead && w.role == Role::Ancestor {
                let t = (self.warriors[i].respawn_t - dt).max(0.0);
                self.warriors[i].respawn_t = t;
                if t <= 0.0 {
                    let (cx, cz) = self.camps[self.warriors[i].team as usize];
                    let w = &mut self.warriors[i];
                    w.x = cx; w.z = cz;
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
        }
        for t in &self.heal_trees { mix(t.hp.to_bits()); }
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
        bg.spawn_patrol(CLAN_A, 1);
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
        bg.spawn_patrol(CLAN_A, 1);
        let p = bg.warriors.iter().position(|w| w.role == Role::Patrol).unwrap();
        let (cx, cz) = bg.camps[CLAN_A as usize];
        bg.warriors[p].x = cx + 2.0; bg.warriors[p].z = cz;
        bg.warriors[p].hp = bg.warriors[p].max_hp * 0.2;   // far past its nerve
        // an enemy at the camp's edge
        bg.warriors.push(Warrior {
            team: CLAN_B, role: Role::Patrol, ancestor: None, lane: 1,
            x: cx + 8.0, z: cz, heading: 0.0,
            hp: PATROL_HP, max_hp: PATROL_HP, attack: 0.0, speed: 0.0,
            mind: Mind::Trail, carry: Carry::Nothing,
            shield: 0.0, stealth_t: 0.0, fear_t: 0.0, slow_t: 0.0,
            cds: [0.0; 3], respawn_t: 0.0, gather_t: 0.0, kills: 0, is_player: false, seg: 0,
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
