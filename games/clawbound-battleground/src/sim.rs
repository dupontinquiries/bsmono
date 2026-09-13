// Simulation state: entities, economy, damage pipeline, win conditions.

use crate::consts::*;
use crate::gods::{self, GodDef};
use crate::items::{self, StackKind};
use glam::Vec3;

pub use crate::consts::{Lane, Team};

// --- tiny deterministic RNG --------------------------------------------------

#[derive(Clone)]
pub struct Rng(pub u64);
impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }
    pub fn next_u(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn f32(&mut self) -> f32 {
        (self.next_u() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f32()
    }
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[(self.next_u() % v.len() as u64) as usize]
    }
}

// --- events ------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Event {
    Cast { god: usize, slot: u32, aim: Vec3 },
    Damage { pos: Vec3, amount: f32, big: bool },
    Kill { killer: String, victim: String, victim_team: Team, is_god: bool },
    LevelUp { pos: Vec3 },
    Explosion { pos: Vec3, radius: f32, color: [f32; 3] },
    BoltFx { from: Vec3, to: Vec3, color: [f32; 3], thick: f32 },
    BeamFx { from: Vec3, dir: Vec3, length: f32, width: f32, color: [f32; 3], dur: f32 },
    ShieldFx { pos: Vec3, dur: f32, color: [f32; 3] },
    DashFx { pos: Vec3, dir: Vec3, color: [f32; 3] },
    RingTelegraph { pos: Vec3, radius: f32, delay: f32, color: [f32; 3] },
    PullFx { pos: Vec3, radius: f32, color: [f32; 3] },
    HealFx { pos: Vec3, radius: f32 },
    NovaFx { pos: Vec3, radius: f32, color: [f32; 3] },
    StormFx { pos: Vec3, radius: f32, color: [f32; 3], dur: f32 },
    TowerDown { pos: Vec3, team: Team },
    TitanVulnerable { team: Team },
    Announce { text: String },
    WeatherChanged { kind: crate::weather::WeatherKind },
    MapEventStart { kind: crate::map_events::MapEventKind, pos: Vec3, radius: f32 },
    MapEventEnd { kind: crate::map_events::MapEventKind },
    PreyCaught { pos: Vec3, kind: crate::wild::PreyKind, gold: f32 },
    BossSpawn { kind: crate::bosses::BossKind, pos: Vec3 },
    DisasterWarning { kind: crate::disasters::DisasterKind },
    DisasterStart { kind: crate::disasters::DisasterKind },
    DisasterEnd { kind: crate::disasters::DisasterKind },
    FireIgnite { pos: Vec3 },
    FissureOpen { a: Vec3, b: Vec3 },
    TornadoDebris { pos: Vec3 },
    BossDown { kind: crate::bosses::BossKind, pos: Vec3, team: Option<Team> },
}

#[derive(Clone, Debug)]
pub struct KillEntry {
    pub text: String,
    pub t: f32,
    pub team_of_victim: Team,
}

// --- entities -----------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TargetRef {
    God(usize),
    Minion(usize),
    Tower(usize),
    Titan(Team),
    /// neutral objective (bosses.rs)
    Boss(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AiState {
    Push,
    Fight,
    Retreat,
}

pub struct AiBrain {
    pub lane: Lane,
    pub aggression: f32,
    pub state: AiState,
    pub target: Option<TargetRef>,
    pub strafe_dir: f32,
    pub strafe_t: f32,
    pub buy_t: f32,
    pub wp: usize,
}

pub struct GodUnit {
    pub def_id: u32,
    pub team: Team,
    pub is_player: bool,
    pub pos: Vec3,
    pub yaw: f32,
    pub level: u32,
    /// One-time capstone at level 9 — see `slots_for_level`'s doc for why 9
    /// is the arc's top. Checked by `recalc` to apply the flat bonus below;
    /// stored rather than derived from `level` so it survives the (rare)
    /// case of a level being lost or recalculated some other way — an
    /// awakening earned should stay earned.
    pub awakened: bool,
    pub xp: f32,
    pub gold: f32,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub items: Vec<u16>,
    pub stacks: Vec<f32>, // parallel to items — stack counters
    pub hp: f32,
    pub max_hp: f32,
    pub mana: f32,
    pub max_mana: f32,
    pub shield: f32,
    pub shield_t: f32,
    pub cds: [f32; 4],
    pub atk_timer: f32,
    pub atk_anim: f32,
    pub walk_anim: f32,
    pub buff_atkspd: f32,
    pub buff_ms: f32,
    pub buff_t: f32,
    /// Riverblessing (from a glowfish): seconds left. Grants speed, attack
    /// speed, mana regen, and free passage through the rivers.
    pub blessing_t: f32,
    pub slow: f32,
    pub slow_t: f32,
    pub respawn_t: f32,
    pub last_dmg_t: f32,
    pub dash: Option<DashState>,
    pub recent_damagers: Vec<(usize, f32)>, // (god idx, time)
    pub move_dir: Vec3,
    pub attack_held: bool,
    pub ai: Option<AiBrain>,
    // cached derived
    pub prot: f32,
    pub pen_pct: f32,
    pub pen_flat: f32,
    pub power: f32,
    pub atk_speed: f32,
    pub cdr: f32,
    pub ms_bonus: f32,
}

pub struct DashState {
    pub from: Vec3,
    pub to: Vec3,
    pub t: f32,
    pub dur: f32,
    pub dmg: f32,
}

pub struct Minion {
    pub team: Team,
    pub lane: Lane,
    pub caster: bool,
    pub pos: Vec3,
    pub hp: f32,
    pub max_hp: f32,
    pub atk_timer: f32,
    pub atk_anim: f32,
    pub walk_anim: f32,
    pub wp: usize,
    pub last_hit: Option<usize>,
    pub yaw: f32,
}

pub struct Tower {
    pub team: Team,
    pub lane: Lane,
    pub inner: bool,
    pub pos: Vec3,
    pub hp: f32,
    pub max_hp: f32,
    pub atk_timer: f32,
    pub ramp: u32,
    pub last_target: Option<TargetRef>,
    pub fire_anim: f32,
}

#[derive(Clone, Copy)]
pub struct Titan {
    pub team: Team,
    pub pos: Vec3,
    pub hp: f32,
    pub atk_timer: f32,
    pub atk_anim: f32,
}

pub struct Projectile {
    pub team: Team,
    pub pos: Vec3,
    pub vel: Vec3,
    pub dmg: f32,
    pub radius: f32,
    pub life: f32,
    pub kind: ProjKind,
    pub owner: usize,
    pub pen_pct: f32,
    pub pen_flat: f32,
    pub color: [f32; 3],
}

#[derive(Clone, Copy, PartialEq)]
pub enum ProjKind {
    Bolt,
    Arrow,
    Lob { start: Vec3, target: Vec3, t: f32, dur: f32, radius: f32 },
}

pub struct Zone {
    pub team: Team,
    pub kind: ZoneKind,
    pub pos: Vec3,
    pub radius: f32,
    pub color: [f32; 3],
}

pub enum ZoneKind {
    Storm { until: f32, dps: f32, strike_t: f32, owner: usize },
    Ring { trigger_t: f32, dmg: f32, owner: usize, done: bool },
}

// --- game ---------------------------------------------------------------------

pub struct Game {
    pub t: f32,
    pub gods: Vec<GodUnit>,
    pub minions: Vec<Minion>,
    pub towers: Vec<Tower>,
    pub titans: [Titan; 2],
    pub projectiles: Vec<Projectile>,
    pub zones: Vec<Zone>,
    pub events: Vec<Event>,
    pub killfeed: Vec<KillEntry>,
    pub next_wave_t: f32,
    pub over: Option<Team>,
    pub rng: Rng,
    pub cast_queue: Vec<(usize, u32, Vec3)>, // (god idx, slot, aim)
    pub tower_alive: [usize; 2],
    pub titan_vuln_announced: [bool; 2],
    pub weather: crate::weather::Weather,
    pub map_events: crate::map_events::MapEvents,
    pub wild: crate::wild::Wild,
    pub bosses: crate::bosses::Bosses,
    pub disasters: crate::disasters::Disasters,
    /// Solid world props (x, z, radius) — trees, rocks. Empty unless
    /// `set_obstacles` was called; see that method's doc.
    pub obstacles: Vec<(f32, f32, f32)>,
}

impl Game {
    /// Create a 5v5 match. `player_god` = roster id the player controls (index 0),
    /// `ally_picks` / `enemy_picks` = roster ids for the 4 allies / 5 enemies.
    pub fn new(player_god: u32, ally_picks: [u32; 4], enemy_picks: [u32; 5]) -> Game {
        let mut rng = Rng::new(0xC0FFEE ^ (player_god as u64 + 1));
        let mut gods = Vec::with_capacity(10);

        // lanes: player mid by default, allies top/bot/mid, enemies top/mid/bot/mid/bot
        let ally_lanes = [Lane::Top, Lane::Mid, Lane::Bot, Lane::Mid];
        let enemy_lanes = [Lane::Top, Lane::Mid, Lane::Bot, Lane::Mid, Lane::Bot];

        for i in 0..10 {
            let (def_id, team, is_player, lane) = if i == 0 {
                (player_god, Team::Blue, true, Lane::Mid)
            } else if i < 5 {
                (ally_picks[i - 1], Team::Blue, false, ally_lanes[i - 1])
            } else {
                (enemy_picks[i - 5], Team::Red, false, enemy_lanes[i - 5])
            };
            let def = gods::god(def_id);
            let (hp, mana, _, _, prot) = def.at_level(1);
            let spawn = Self::spawn_pos(team, lane, &mut rng);
            gods.push(GodUnit {
                def_id,
                team,
                is_player,
                pos: spawn,
                yaw: -team.push_z() * std::f32::consts::FRAC_PI_2,
                level: 1,
                awakened: false,
                xp: 0.0,
                gold: START_GOLD,
                kills: 0,
                deaths: 0,
                assists: 0,
                items: Vec::new(),
                stacks: Vec::new(),
                hp,
                max_hp: hp,
                mana,
                max_mana: mana,
                shield: 0.0,
                shield_t: 0.0,
                cds: [0.0; 4],
                atk_timer: 0.0,
                atk_anim: 0.0,
                walk_anim: 0.0,
                buff_atkspd: 0.0,
                buff_ms: 0.0,
                buff_t: 0.0,
                blessing_t: 0.0,
                slow: 0.0,
                slow_t: 0.0,
                respawn_t: 0.0,
                last_dmg_t: -99.0,
                dash: None,
                recent_damagers: Vec::new(),
                move_dir: Vec3::ZERO,
                attack_held: false,
                ai: if is_player { None } else { Some(AiBrain { lane, aggression: 0.45 + rng.f32() * 0.5, state: AiState::Push, target: None, strafe_dir: 1.0, strafe_t: 0.0, buy_t: 1.5, wp: 0 }) },
                prot,
                pen_pct: 0.0,
                pen_flat: 0.0,
                power: def.power,
                atk_speed: 0.0,
                cdr: 0.0,
                ms_bonus: 0.0,
            });
        }

        // towers
        let mut towers = Vec::new();
        for lane in LANES {
            for team in [Team::Blue, Team::Red] {
                for inner in [false, true] {
                    towers.push(Tower {
                        team,
                        lane,
                        inner,
                        pos: tower_pos(lane, team, inner),
                        hp: TOWER_HP,
                        max_hp: TOWER_HP,
                        atk_timer: 0.0,
                        ramp: 0,
                        last_target: None,
                        fire_anim: 0.0,
                    });
                }
            }
        }

        let titans = [
            Titan { team: Team::Blue, pos: Team::Blue.titan_pos(), hp: TITAN_HP, atk_timer: 0.0, atk_anim: 0.0 },
            Titan { team: Team::Red, pos: Team::Red.titan_pos(), hp: TITAN_HP, atk_timer: 0.0, atk_anim: 0.0 },
        ];

        let wild = crate::wild::Wild::new(&mut rng);
        let mut g = Game {
            t: 0.0,
            gods,
            minions: Vec::new(),
            towers,
            titans,
            projectiles: Vec::new(),
            zones: Vec::new(),
            events: Vec::new(),
            killfeed: Vec::new(),
            next_wave_t: WAVE_FIRST,
            over: None,
            rng,
            cast_queue: Vec::new(),
            tower_alive: [6, 6],
            titan_vuln_announced: [false, false],
            weather: crate::weather::Weather::new(),
            map_events: crate::map_events::MapEvents::new(),
            wild,
            bosses: crate::bosses::Bosses::new(),
            disasters: crate::disasters::Disasters::default(),
            obstacles: Vec::new(),
        };
        // starter items for AI gods (player buys via shop)
        for i in 0..10 {
            if !g.gods[i].is_player {
                g.auto_buy(i);
            }
        }
        g
    }

    /// Wire in the world's solid props (trees, stones — see `scene::build_world`'s
    /// `solid!` macro) so movement can push units out of them. Separate from
    /// `new` because `Scene` (the renderer's world) and `Game` (headless sim)
    /// are built independently — every `Game::new` call in tests/headless.rs
    /// has none of this and simply gets no prop collision, which is correct
    /// for a run with no rendered world to collide with.
    pub fn set_obstacles(&mut self, obstacles: Vec<(f32, f32, f32)>) {
        self.obstacles = obstacles;
    }

    pub fn spawn_pos(team: Team, lane: Lane, rng: &mut Rng) -> Vec3 {
        let base = team.fountain() + Vec3::new(0.0, 0.0, team.push_z() * 8.0);
        let jitter = Vec3::new(rng.range(-3.0, 3.0), 0.0, rng.range(-2.0, 2.0));
        let lane_off = match lane {
            Lane::Top => Vec3::new(-4.0, 0.0, 0.0),
            Lane::Bot => Vec3::new(4.0, 0.0, 0.0),
            Lane::Mid => Vec3::ZERO,
        };
        base + jitter + lane_off
    }

    pub fn def(&self, i: usize) -> &'static GodDef {
        gods::god(self.gods[i].def_id)
    }

    pub fn god_name(&self, i: usize) -> &'static str {
        self.def(i).name
    }

    // --- derived stats ---------------------------------------------------------

    pub fn item_stats(god: &GodUnit) -> items::Stats {
        let cat = items::all();
        let mut s = items::Stats::ZERO;
        for (k, id) in god.items.iter().enumerate() {
            let item = &cat[*id as usize];
            s.add(&item.stats);
            if let Some(st) = &item.stack {
                let stacks = god.stacks.get(k).copied().unwrap_or(0.0);
                match st.kind {
                    StackKind::MinionKill | StackKind::GodKill | StackKind::TowerKill => {
                        let gained = stacks * st.per;
                        match item.cat {
                            items::Cat::Strength => s.strength += gained,
                            items::Cat::Intelligence => s.intelligence += gained,
                            items::Cat::Scaling => s.power += gained,
                            items::Cat::Protection => s.protection += gained,
                            items::Cat::Health => s.health += gained,
                            _ => {}
                        }
                    }
                    StackKind::Time => {}
                }
            }
        }
        s
    }

    pub fn recalc(&mut self, i: usize) {
        let def = self.def(i);
        let g = &mut self.gods[i];
        let (base_hp, base_mana, _, base_power, base_prot) = def.at_level(g.level);
        // The level-9 capstone: a flat 10% on top of the curve, once,
        // rather than a bigger `hp_growth`/`power_growth` for every god —
        // this is a single felt spike at the top of the 1-9 arc, not a
        // change to the smooth 1-20 curve everything else still uses.
        let awaken = if g.awakened { 1.10 } else { 1.0 };
        let st = Self::item_stats(g);
        let new_max = base_hp * awaken + st.health;
        let hp_frac = if g.max_hp > 0.0 { g.hp / g.max_hp } else { 1.0 };
        g.max_hp = new_max;
        g.hp = (new_max * hp_frac).max(g.hp.min(new_max));
        let new_mmax = base_mana + st.mana;
        g.max_mana = new_mmax;
        g.mana = g.mana.min(new_mmax);
        g.power = base_power * awaken + st.power + st.intelligence * 0.9 + st.strength * 0.6;
        g.prot = base_prot + st.protection;
        g.pen_pct = st.pen_pct;
        g.pen_flat = st.pen_flat;
        g.atk_speed = st.attack_speed;
        g.cdr = st.cdr.min(0.4);
        g.ms_bonus = st.move_speed;
    }

    pub fn move_speed(&self, i: usize) -> f32 {
        let g = &self.gods[i];
        let def = self.def(i);
        let bless = if g.blessing_t > 0.0 { BLESSING_MS } else { 0.0 };
        let base = def.move_speed * (1.0 + g.ms_bonus + g.buff_ms + bless);
        // wading: the rivers are slow going unless you were born to them
        let depth = self.water_depth_now(g.pos.x, g.pos.z);
        let wade = if g.blessing_t > 0.0 || crate::wild::is_riverborn(def) { 1.0 } else { wade_speed_mul(depth) };
        base * (1.0 - g.slow) * self.map_events.speed_mul(def.role, g.def_id) * wade * self.disasters.speed_mul()
    }

    pub fn auto_interval(&self, i: usize) -> f32 {
        let g = &self.gods[i];
        let bless = if g.blessing_t > 0.0 { BLESSING_AS } else { 0.0 };
        self.def(i).auto_cd / (1.0 + g.atk_speed + g.buff_atkspd + bless)
    }

    pub fn auto_damage(&self, i: usize) -> f32 {
        let g = &self.gods[i];
        let (_, _, auto, _, _) = self.def(i).at_level(g.level);
        auto + g.power * 0.22
    }

    // --- damage ----------------------------------------------------------------

    pub fn deal_damage(&mut self, target: TargetRef, mut dmg: f32, magic: bool, source: Option<usize>, pen_pct: f32, pen_flat: f32) {
        if self.over.is_some() {
            return;
        }
        let _ = magic;
        // Stormfire: the Drake's blessing multiplies everything its holders deal
        if let Some(s) = source {
            if s < self.gods.len() {
                let team = self.gods[s].team;
                dmg *= self.bosses.dmg_mul(team);
                if matches!(target, TargetRef::Tower(_) | TargetRef::Titan(_)) {
                    dmg += self.bosses.struct_bonus(team);
                }
            }
        }
        match target {
            TargetRef::Boss(bi) => {
                self.boss_take_damage(bi, dmg, source);
            }
            TargetRef::God(ti) => {
                let g = &mut self.gods[ti];
                if g.respawn_t > 0.0 {
                    return;
                }
                let eff = (g.prot * (1.0 - pen_pct) - pen_flat).max(0.0);
                dmg *= 100.0 / (100.0 + eff);
                let pre = g.hp + g.shield;
                if g.shield > 0.0 {
                    let absorbed = g.shield.min(dmg);
                    g.shield -= absorbed;
                    dmg -= absorbed;
                }
                g.hp -= dmg;
                g.last_dmg_t = self.t;
                if let Some(s) = source {
                    if s != ti {
                        g.recent_damagers.retain(|(idx, t)| *idx != s && self.t - *t < 8.0);
                        g.recent_damagers.push((s, self.t));
                    }
                }
                let _ = pre;
                if g.hp <= 0.0 {
                    self.on_god_death(ti, source);
                } else {
                    self.events.push(Event::Damage { pos: g.pos + Vec3::Y * 1.6, amount: dmg, big: dmg > 120.0 });
                }
            }
            TargetRef::Minion(mi) => {
                if mi >= self.minions.len() {
                    return;
                }
                let m = &mut self.minions[mi];
                m.hp -= dmg;
                if let Some(s) = source {
                    m.last_hit = Some(s);
                }
                self.events.push(Event::Damage { pos: m.pos + Vec3::Y * 0.9, amount: dmg, big: false });
                if m.hp <= 0.0 {
                    self.on_minion_death(mi);
                }
            }
            TargetRef::Tower(ti) => {
                if ti >= self.towers.len() || self.towers[ti].hp <= 0.0 {
                    return;
                }
                let tw = &mut self.towers[ti];
                tw.hp -= dmg;
                self.events.push(Event::Damage { pos: tw.pos + Vec3::Y * 3.0, amount: dmg, big: false });
                if tw.hp <= 0.0 {
                    self.on_tower_death(ti);
                }
            }
            TargetRef::Titan(team) => {
                let ti = team as usize;
                if self.titans[ti].hp <= 0.0 {
                    return;
                }
                if self.titan_shielded(team) {
                    return; // immune
                }
                self.titans[ti].hp -= dmg;
                self.events.push(Event::Damage { pos: self.titans[ti].pos + Vec3::Y * 3.5, amount: dmg, big: dmg > 150.0 });
                if self.titans[ti].hp <= 0.0 {
                    self.titans[ti].hp = 0.0;
                    let winner = team.other();
                    self.over = Some(winner);
                    self.events.push(Event::Announce { text: if winner == Team::Blue { "VICTORY".into() } else { "DEFEAT".into() } });
                }
            }
        }
    }

    pub fn titan_shielded(&self, team: Team) -> bool {
        self.tower_alive[team as usize] >= TITAN_TOWERS_REQ
    }

    fn on_minion_death(&mut self, mi: usize) {
        let m = &self.minions[mi];
        let gold = if m.caster { GOLD_CASTER } else { GOLD_MELEE };
        let xp = if m.caster { XP_CASTER } else { XP_MELEE };
        let pos = m.pos;
        let enemy_team = m.team.other();
        // gold to last hitter (if a god), xp shared
        if let Some(killer) = m.last_hit {
            if killer < self.gods.len() && self.gods[killer].team != m.team {
                let team = self.gods[killer].team;
                self.gods[killer].gold += gold * self.map_events.gold_mul() * self.bosses.gold_mul(team);
                self.add_stacks(killer, StackKind::MinionKill, 1.0);
            }
        }
        for i in 0..self.gods.len() {
            let g = &self.gods[i];
            if g.team == enemy_team && g.respawn_t <= 0.0 && g.pos.distance(pos) <= XP_SHARE_RADIUS {
                self.grant_xp(i, xp);
            }
        }
        self.minions.remove(mi);
    }

    fn on_tower_death(&mut self, ti: usize) {
        let tw = &self.towers[ti];
        let pos = tw.pos;
        let team = tw.team;
        let killer_team = team.other();
        // gold to all enemy gods alive nearby-ish; simplest: all enemy gods get share
        for i in 0..self.gods.len() {
            if self.gods[i].team == killer_team {
                self.gods[i].gold += GOLD_TOWER / 3.0;
                self.add_stacks(i, StackKind::TowerKill, 1.0);
            }
        }
        self.tower_alive[team as usize] -= 1;
        self.events.push(Event::TowerDown { pos, team });
        self.events.push(Event::Announce {
            text: format!("{} TOWER DESTROYED", if team == Team::Blue { "BLUE" } else { "RED" }),
        });
        if self.titan_shielded(team) == false && !self.titan_vuln_announced[team as usize] {
            self.titan_vuln_announced[team as usize] = true;
            self.events.push(Event::TitanVulnerable { team });
            self.events.push(Event::Announce { text: format!("{} ELDER IS VULNERABLE", if team == Team::Blue { "BLUE" } else { "RED" }) });
        }
        self.towers.remove(ti);
    }

    fn on_god_death(&mut self, gi: usize, source: Option<usize>) {
        // snapshot immutable data first
        let (pos, name, team, helpers) = {
            let g = &self.gods[gi];
            (
                g.pos,
                self.god_name(gi).to_string(),
                g.team,
                g.recent_damagers.iter().map(|(idx, _)| *idx).collect::<Vec<_>>(),
            )
        };
        {
            let g = &mut self.gods[gi];
            g.deaths += 1;
            g.respawn_t = respawn_time(g.level);
            g.hp = 0.0;
            g.shield = 0.0;
            g.dash = None;
            g.recent_damagers.clear();
        }
        self.events.push(Event::Damage { pos: pos + Vec3::Y * 1.5, amount: 0.0, big: false });

        let mut kill_text = String::new();
        if let Some(k) = source {
            if k < self.gods.len() && self.gods[k].team != team {
                let kg = &mut self.gods[k];
                kg.kills += 1;
                let kt = kg.team;
                kg.gold += GOLD_GOD * self.map_events.gold_mul() * self.bosses.gold_mul(kt);
                self.add_stacks(k, StackKind::GodKill, 1.0);
                self.grant_xp(k, XP_GOD);
                kill_text = format!("{} slew {}", self.god_name(k), name);
            }
        }
        if kill_text.is_empty() {
            kill_text = format!("{} has fallen", name);
        }
        for h in helpers {
            if h < self.gods.len() && h != source.unwrap_or(usize::MAX) && self.gods[h].team != team {
                self.gods[h].assists += 1;
                self.gods[h].gold += 120.0;
                self.grant_xp(h, XP_GOD * 0.4);
            }
        }
        self.killfeed.push(KillEntry { text: kill_text, t: self.t, team_of_victim: team });
        if self.killfeed.len() > 6 {
            self.killfeed.remove(0);
        }
    }

    pub fn grant_xp(&mut self, gi: usize, mut xp: f32) {
        xp *= 1.0 + 0.02 * (self.t / 60.0).min(8.0); // late-game scaling
        let g = &mut self.gods[gi];
        g.xp += xp;
        let old_level = g.level;
        let old_slots = items::slots_for_level(old_level);
        let mut leveled = false;
        while g.level < MAX_LEVEL && g.xp >= xp_for_level(g.level) {
            g.xp -= xp_for_level(g.level);
            g.level += 1;
            leveled = true;
        }
        if leveled {
            let pos = g.pos;
            let new_level = g.level;
            let is_player = g.is_player;
            self.events.push(Event::LevelUp { pos });
            // Only the player cares about a text banner for their own
            // build — AI gods level up constantly and would spam the feed.
            if is_player {
                let new_slots = items::slots_for_level(new_level);
                if new_slots > old_slots {
                    self.events.push(Event::Announce { text: "New item slot unlocked!".into() });
                }
                if new_level >= 9 && !self.gods[gi].awakened {
                    self.gods[gi].awakened = true;
                    self.events.push(Event::Announce { text: "Power awakened!".into() });
                }
            } else if new_level >= 9 && !self.gods[gi].awakened {
                // AI gods still get the actual stat bonus, just no banner.
                self.gods[gi].awakened = true;
            }
            self.recalc(gi);
            let g = &mut self.gods[gi];
            g.hp = (g.hp + g.max_hp * 0.22).min(g.max_hp);
            g.mana = (g.mana + g.max_mana * 0.3).min(g.max_mana);
        }
    }

    pub fn add_stacks(&mut self, gi: usize, kind: StackKind, n: f32) {
        let cat = items::all();
        let g = &mut self.gods[gi];
        for (k, id) in g.items.clone().iter().enumerate() {
            if let Some(st) = &cat[*id as usize].stack {
                if st.kind == kind {
                    let cur = g.stacks.get(k).copied().unwrap_or(0.0);
                    let next = (cur + n).min(st.max);
                    if k < g.stacks.len() {
                        g.stacks[k] = next;
                    } else {
                        g.stacks.push(next);
                    }
                }
            }
        }
    }

    pub fn buy_item(&mut self, gi: usize, item_id: u16) -> bool {
        let cat = items::all();
        let g = &self.gods[gi];
        if g.items.len() >= items::slots_for_level(g.level) {
            return false;
        }
        let item = &cat[item_id as usize];
        if g.gold < item.cost {
            return false;
        }
        let g = &mut self.gods[gi];
        g.gold -= item.cost;
        g.items.push(item_id);
        g.stacks.push(0.0);
        self.recalc(gi);
        true
    }

    /// AI auto-buy: walk the role build order, buy the best affordable item.
    pub fn auto_buy(&mut self, gi: usize) {
        let role = self.def(gi).role;
        let order = items::role_build_order(role);
        let cat = items::all();
        let g = &self.gods[gi];
        if g.items.len() >= items::slots_for_level(g.level) {
            return;
        }
        let mut best: Option<u16> = None;
        'outer: for wanted in order {
            // prefer highest tier affordable in this category
            let mut cands: Vec<&items::Item> = cat.iter().filter(|it| it.cat == wanted).collect();
            cands.sort_by(|a, b| b.cost.partial_cmp(&a.cost).unwrap());
            for it in cands {
                if it.cost <= g.gold {
                    best = Some(it.id);
                    break 'outer;
                }
            }
        }
        if let Some(id) = best {
            self.buy_item(gi, id);
        }
    }

    /// God positions of enemies of `team` that are alive.
    pub fn enemy_gods_near(&self, pos: Vec3, team: Team, radius: f32) -> Vec<usize> {
        (0..self.gods.len())
            .filter(|&i| self.gods[i].team != team && self.gods[i].respawn_t <= 0.0 && self.gods[i].pos.distance(pos) <= radius)
            .collect()
    }
}
