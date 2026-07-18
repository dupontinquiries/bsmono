//! prowl.rs — "CLAWBOUND · Prowl", the open-world objective mode, ported from the
//! SMITE-2-inspired browser prototype as deterministic, renderer-free logic.
//!
//! You prowl as a chosen [`crate::class`] creature while a [`crate::hunter`] stalks
//! you. **Objectives** rotate one at a time — Reach a landmark, hold a Rendezvous,
//! Survive the hunt, or Forage (catch prey) — each clearing into the next. You hunt
//! ambient prey ([`crate::ecology`]) by pouncing mid-leap for score, and getting
//! caught respawns you (no permadeath). Landmarks come from [`crate::objectives`].
//! Movement speed is terrain-scaled through the class, so the ground shapes the chase.

use crate::class::Class;
use crate::ecology::Critter;
use crate::hunter::{self, HState, Kind};
use crate::objectives;
use crate::powerup::{Buffs, Power};
use crate::ring::Ring;
use crate::weather::Weather;
use crate::world;

const PROWL_SCALE: f32 = 1.9;      // lift class speeds to open-world chase pace
const SPRINT: f32 = 1.15;
const CATCH_DIST: f32 = 2.3;
const LEAP_DUR: f32 = 0.5;
const LEAP_CD: f32 = 1.8;
const LEAP_SPEED: f32 = 26.0;
const DODGE_DUR: f32 = 0.34;
const DODGE_CD: f32 = 1.5;
const DODGE_SPEED: f32 = 30.0;
const POUNCE_R: f32 = 2.4;
const MAP: f32 = world::SIZE * 0.92;
// ── ancestor trees: defensible staging grounds that store a buff for the pack ──
const N_ANCESTORS: usize = 4;
const CLAIM_R: f32 = 11.0;      // dwell this close to claim a tree
const CLAIM_TIME: f32 = 2.5;    // seconds of dwelling to claim
const ANCESTOR_CD: f32 = 26.0;  // after granting its buff, the tree rests this long
const STAGE_R: f32 = 15.0;      // within this of a claimed tree = safe staging ground
const REACH_R: f32 = 8.0;
const RENDEZVOUS_R: f32 = 9.0;
const RENDEZVOUS_NEED: f32 = 4.0;
const SURVIVE_LEN: f32 = 45.0;
const FORAGE_NEED: u32 = 4;
const NEXT_DELAY: f32 = 1.8;
const LANE_HALF_W: f32 = 6.5;   // how wide the beaten paths between the trees are
const LANE_BONUS: f32 = 1.25;   // travel is faster along a lane
// ── four clans, each defending an ancestor home with border-patrolling cat minions ──
const PLAYER_CLAN: u8 = 0;
const MINIONS_PER_CLAN: usize = 3;
const BORDER_R: f32 = 20.0;     // radius of a clan's border-patrol loop around its home
const MINION_SPEED: f32 = 6.5;
const MINION_AGGRO_R: f32 = 12.0;
const YOWL_RANGE: f32 = 22.0;      // ranged minions cast from further out than the melee aggro radius
const YOWL_CD: f32 = 6.0;
const YOWL_REVEAL_DUR: f32 = 2.2;  // seconds a landed yowl outs your position (forces you out of hiding)
pub const BEACON_RADIUS: f32 = 14.0;   // an owned tree's aura reaches this far
pub const BEACON_PULSE: f32 = 5.0;     // matches Swift Paw's duration — continuous uptime while camping the aura
pub const N_MARKERS: usize = 5;
const MARKER_R: f32 = 4.0;
pub const MARKER_CLAIM_TIME: f32 = 3.0;
const MARKER_DRIFT_BACK: f32 = 20.0;   // seconds unattended before a held marker drifts back to its nearest clan
const CONVERGENCE_INTERVAL: f32 = 240.0;  // a Prowl round now runs ~30 min — a convergence every 4 min, ~7 per round
const CONVERGENCE_WARN: f32 = 20.0;    // the last stretch before a convergence, worth flagging to the player
pub const RITUAL_WIN: f32 = 160.0;     // reachable by a dominant clan across several convergences over the round, not any one of them
const ROCK_BONUS_R: f32 = 10.0;        // being at the Rock when a convergence resolves adds a bonus
const SUSTENANCE_DECAY: f32 = 0.0035;  // per second — with no prey at all, sickness sets in around the 4-5 min mark, recurring across the round
const SUSTENANCE_PER_PREY: f32 = 0.22;
const SUSTENANCE_SICK_AT: f32 = 0.12;
const SUSTENANCE_WELL_AT: f32 = 0.4;   // sickness clears once it's recovered back above this
const TWIG_HEAR_R: f32 = 26.0;         // how far a snapped twig carries, hunter-side
const TWIG_SUSPICION: f32 = 0.5;
const LOG_BOUND_R: f32 = 1.6;
const SPLASH_HEAR_R: f32 = 20.0;   // how far a loud water entry carries, hunter-side
const WIN_SCORE: i32 = 3200;   // first clan to this many points wins outright — sized for a ~30 min round
const RIVAL_FLEE_R: f32 = 18.0; // a rival breaks off to flee the stalker inside this range
const N_CAMPS: usize = 3;
const CAMP_ALERT_R: f32 = 9.0;   // a guard notices an un-hidden prowler inside this range
const CAMP_HERB_R: f32 = 3.0;    // reach to take the herb
const CAMP_GUARD_SPEED: f32 = 5.2;
const CAMP_REGROW: f32 = 45.0;   // seconds before a taken (but not cleared) herb is worth retaking
const SCENT_RATE: f32 = 1.6;     // suspicion/second added when moving hard downwind of the hunter at full strength
const SCENT_RANGE: f32 = 45.0;   // scent only carries this far
const MOVE_REACH: f32 = 3.6;    // how close an autoattack move connects with a target
const STAGGER_DUR: f32 = 0.9;   // seconds a landed hit dazes the hunter/a rival
const ULT_CD_MULT: f32 = 1.8;   // stretches the shared Ability's cooldown to feel like an ultimate

/// The four clan colours (Ember / Tide / Moss / Amber).
pub fn clan_color(clan: u8) -> u32 {
    match clan & 3 { 0 => 0xd8623c, 1 => 0x3d7fd8, 2 => 0x5aa054, _ => 0xd8a53c }
}
pub fn clan_name(clan: u8) -> &'static str {
    match clan & 3 { 0 => "your clan", 1 => "the Rival clan", 2 => "the Wildwood clan", _ => "the Border clan" }
}

fn hashf(mut h: u32) -> f32 {
    h ^= h >> 16; h = h.wrapping_mul(0x7feb352d);
    h ^= h >> 15; h = h.wrapping_mul(0x846ca68b);
    h ^= h >> 16; (h as f32) / (u32::MAX as f32)
}
fn norm(x: f32, z: f32) -> (f32, f32) { let l = (x * x + z * z).sqrt(); if l > 1e-5 { (x / l, z / l) } else { (0.0, 0.0) } }
fn dist(ax: f32, az: f32, bx: f32, bz: f32) -> f32 { ((ax - bx).powi(2) + (az - bz).powi(2)).sqrt() }

/// Shortest distance from a point to a segment (ax,az)-(bx,bz).
fn dist_to_seg(px: f32, pz: f32, ax: f32, az: f32, bx: f32, bz: f32) -> f32 {
    let (dx, dz) = (bx - ax, bz - az);
    let len2 = dx * dx + dz * dz;
    let t = if len2 > 1e-6 { (((px - ax) * dx + (pz - az) * dz) / len2).clamp(0.0, 1.0) } else { 0.0 };
    dist(px, pz, ax + dx * t, az + dz * t)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProwlObj { Reach, Rendezvous, Survive, Forage }

/// An ancestor tree — a big landmark where the ancestors stored a buff. Dwell close to
/// claim it (which grants its stored buff and puts it on cooldown); a claimed tree is a
/// safe **staging ground** that mends stamina, cools the hunt, and anchors respawns.
#[derive(Clone, Copy)]
pub struct AncestorTree {
    pub x: f32, pub z: f32,
    pub stored: Power,
    pub claim: f32,       // 0..CLAIM_TIME the player's dwell progress
    pub cooldown: f32,    // >0 = spent, recharging
    pub claimed_by: Option<u8>,   // which clan currently owns this tree (None = wild)
    pub beacon_class: Option<Class>,   // the class of whoever last claimed it — sets the aura it pulses
    pub pulse_t: f32,                   // seconds until the next beacon pulse
}
impl AncestorTree {
    pub fn ready(&self) -> bool { self.cooldown <= 0.0 }
    pub fn claim_frac(&self) -> f32 { (self.claim / CLAIM_TIME).clamp(0.0, 1.0) }
    pub fn claimed(&self) -> bool { self.claimed_by.is_some() }
    pub fn owned_by(&self, clan: u8) -> bool { self.claimed_by == Some(clan) }
}

/// A small cat minion belonging to a clan. It walks a border-patrol loop around its clan's
/// home and breaks off to shoo away intruders from rival clans.
#[derive(Clone, Copy)]
pub struct Minion {
    pub clan: u8,
    pub x: f32, pub z: f32, pub heading: f32,
    pub home: (f32, f32),
    pub phase: f32,      // position along the patrol loop
    pub aggro: bool,     // currently defending against an intruder
    pub ranged: bool,    // a yowler: casts from range instead of closing to melee
    pub yowl_cd: f32,    // ranged only: seconds until it can cast again
    pub yowl_flash: f32, // > 0 briefly after a cast, for the visual
}

/// A rival clan's AI prowler. It roams to the nearest ancestor tree it doesn't own, dwells to
/// claim it (flipping ownership and scoring for its clan), and flees the stalker when caught.
#[derive(Clone, Copy)]
pub struct Rival {
    pub clan: u8,
    pub class: Class,
    pub x: f32, pub z: f32, pub heading: f32,
    pub buffs: Buffs,
    pub dwell: f32,       // claim progress at its current tree
    pub target: usize,    // ancestor index it's heading for
    pub respawn_flash: f32,
    pub stagger: f32,     // > 0 while dazed by a landed hit — freezes it, interrupts its claim
}

/// A jungle camp guardian species. Each favors a different herb buff and plays a little
/// differently to fight, but the sneak-vs-clear choice is the same for all three.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CampKind { Raccoon, Fox, Snake }
impl CampKind {
    pub fn name(self) -> &'static str { match self { CampKind::Raccoon => "Raccoon", CampKind::Fox => "Fox", CampKind::Snake => "Snake" } }
    pub fn reward(self) -> Power { match self { CampKind::Raccoon => Power::Shield, CampKind::Fox => Power::Speed, CampKind::Snake => Power::Shield } }
    pub fn guard_hp(self) -> f32 { match self { CampKind::Raccoon => 3.0, CampKind::Fox => 2.0, CampKind::Snake => 2.5 } }
}

/// A jungle camp: a guarded herb patch off the lanes. Clear the guard for good and the herb
/// is free from then on; leave it alive and you can still slip in and take the herb *if*
/// you never alert it (crouched, out of its sense range) — sneaking works, but clearing it
/// out is the safer long-term play.
#[derive(Clone, Copy)]
pub struct JungleCamp {
    pub kind: CampKind,
    pub x: f32, pub z: f32,          // the herb
    pub gx: f32, pub gz: f32,        // the guard's current position
    pub home_x: f32, pub home_z: f32,
    pub guard_alive: bool,
    pub alert: bool,
    pub hp: f32,
    pub cooldown: f32,               // > 0 while the herb is spent and regrowing (guard cleared: never set again)
}
impl JungleCamp {
    pub fn cleared(&self) -> bool { !self.guard_alive }
}

/// A territory marker — one of the stakes the Rock of Nine Lives ritual is built from.
/// Naturally belongs to whichever clan is nearest, but the player can flip it by
/// dwelling at it; leave it unattended long enough and it drifts back to whoever's
/// closest, so holding ground is an active, ongoing choice, not a one-time capture.
#[derive(Clone, Copy)]
pub struct TerritoryMarker {
    pub x: f32, pub z: f32,
    pub held_by: u8,       // the clan that currently holds it (always someone — territory is never truly neutral)
    pub dwell: f32,        // the player's capture progress, 0..MARKER_CLAIM_TIME
    pub idle: f32,         // seconds since the player last contested it — drives the drift-back
    pub nearest_clan: u8,  // whichever clan's home is closest — where it drifts back to
}

pub struct Objective {
    pub kind: ProwlObj,
    pub tx: f32, pub tz: f32, pub name: &'static str,
    pub progress: f32,        // rendezvous seconds banked
    pub time_left: f32,       // survive countdown
    pub start_caught: u32,    // forage baseline
    pub done: bool,
    pub next_t: f32,
}

pub struct Prowl {
    pub class: Class,
    pub x: f32, pub z: f32, pub heading: f32,
    pub stamina: f32,
    pub moving: bool, pub crouched: bool, pub hidden: bool,
    pub leap_t: f32, pub leap_cd: f32,
    pub dodge_t: f32, pub dodge_cd: f32, pub bx: f32, pub bz: f32,
    pub hunter_kind: Kind,
    pub hx: f32, pub hz: f32, pub hheading: f32,
    pub hstate: HState, pub suspicion: f32, pub last_seen: Option<(f32, f32)>,
    pub critters: Vec<Critter>,
    pub bushes: Vec<(f32, f32, f32)>,
    pub pois: Vec<(f32, f32, &'static str)>,
    pub ring: Ring,
    pub exposed: bool,       // outside the hunt-ring: cover fails and stamina bleeds
    pub obj: Objective,
    pub cleared: u32,
    pub prey_caught: u32,
    pub escapes: u32,
    pub times_caught: u32,
    pub buffs: Buffs,                       // Speed / Shield picked up in the field
    pub ability_cd: f32,                     // ultimate ability cooldown remaining
    pub ability_t: f32,                      // ultimate ability active time remaining
    ability_want: bool,                      // ultimate key pressed this frame
    pub player_move: Option<(crate::combat::Move, f32)>,  // in-progress autoattack move + elapsed
    pub combo_last: Option<(crate::combat::Move, f32)>,   // last finished move + seconds since, for combos
    pub combo_flash: f32,                    // > 0 briefly after a combo lands
    pub combo_name: &'static str,
    pub hunter_stagger: f32,                 // > 0 while the hunter is dazed by a landed hit
    pub pickups: Vec<(f32, f32, Power)>,    // power-up markers scattered on land
    pub ancestors: Vec<AncestorTree>,       // buff-storing staging grounds (clan homes)
    pub lanes: Vec<(f32, f32, f32, f32)>,   // beaten paths between the staging grounds
    pub minions: Vec<Minion>,               // clan cats on border patrol
    pub rivals: Vec<Rival>,                 // rival clans' AI prowlers
    pub jungle: Vec<JungleCamp>,            // guarded herb camps off the lanes
    pub weather: Weather,                   // wind (and sky/wetness) driving the scent game
    pub fixed_wind: bool,                    // settings toggle: freeze the wind for a more readable map
    pub yowled_t: f32,                        // > 0 while a landed yowl has outed you (forces you visible)
    pub markers: Vec<TerritoryMarker>,        // territory stakes feeding the ritual
    pub rock: (f32, f32),                     // the Rock of Nine Lives — the ritual's site
    pub ritual_progress: [f32; 4],            // per-clan progress toward the ritual (0..RITUAL_WIN)
    pub convergence_t: f32,                    // seconds until the next convergence (territory tally)
    pub convergence_result: Option<u8>,        // brief flash of who won the last convergence
    pub sustenance: f32,                       // 0..1 — falls without prey, restored by a catch or by resting at a staging ground
    pub sick: bool,                            // true once sustenance bottoms out — slower until it recovers
    pub tutorial_mode: bool,                    // a safe sandbox: the hunter and minions never engage
    pub twigs: Vec<(f32, f32)>,                // forest-floor twigs — snap one running and the hunt hears it
    pub logs: Vec<(f32, f32)>,                 // fallen logs — a natural little bound when running past
    pub snag_t: f32,                           // > 0 while dragging free of a bramble
    pub was_in_water: bool,                    // tracks water transitions for the splash event
    pub splash_flash: f32,                     // > 0 briefly after a splash, for the visual
    pub on_staging: bool,                    // currently within a claimed tree's grounds
    pub on_lane: bool,                       // currently travelling a lane (faster)
    pub clan_scores: [i32; 4],              // per-clan score (player = clan 0)
    pub outcome: Option<u8>,                // Some(clan) winner, Some(255) draw, None = ongoing
    pub respawn_flash: f32,
    pub shielded_flash: f32,                // brief flash when a Shield soaks a catch
    pub nine_lives_used: bool,              // the Cat's passive: spent on its first catch
    pub time: f32,
    pub difficulty: f32,
    tick: u32,
    input: Option<(f32, f32, bool, bool, bool)>,   // mx, mz, leap, dodge, crouch
}

impl Prowl {
    pub fn new(class: Class, hunter_kind: Kind) -> Prowl {
        // roll this match's terrain pool (mountain bias, season) — a counter-based hash
        // rather than a wall-clock read, so this stays portable to wasm builds
        static ROLL_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = ROLL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let roll = hashf(n.wrapping_mul(2_654_435_761) ^ 0x9E3779B9);
        // mountain bias only ever rolls at or above baseline — "more chance of mountains",
        // never less craggy than the original map
        let mountain_bias = 1.0 + roll * 0.7;   // 1.0..1.7
        let season = (roll * 4.0) as u8 % 4;
        world::set_terrain_pool(mountain_bias, season);
        world::init_world();
        let pois: Vec<(f32, f32, &'static str)> = objectives::place().into_iter()
            .map(|o| (o.x, o.z, world::biome_at(o.x, o.z).name)).collect();
        let critters = crate::ecology::populate(400, false);
        let bushes = world::bush_positions(200);
        let ancestors = Self::place_ancestors();
        let lanes = Self::place_lanes(&ancestors);
        let minions = Self::spawn_minions(&ancestors);
        let rivals = Self::spawn_rivals(&ancestors);
        let markers = Self::place_markers(&ancestors);
        let mut g = Prowl {
            class, x: 0.0, z: 0.0, heading: 0.0, stamina: 1.0,
            moving: false, crouched: false, hidden: false,
            leap_t: 0.0, leap_cd: 0.0, dodge_t: 0.0, dodge_cd: 0.0, bx: 0.0, bz: 0.0,
            hunter_kind, hx: 60.0, hz: 60.0, hheading: 0.0,
            hstate: HState::Patrol, suspicion: 0.0, last_seen: None,
            critters, bushes, pois,
            ring: Ring::new(0.0, 0.0, world::SIZE * 0.34, world::SIZE * 0.12, 1800.0, 0.0),
            exposed: false,
            obj: Objective { kind: ProwlObj::Reach, tx: 0.0, tz: 0.0, name: "", progress: 0.0, time_left: 0.0, start_caught: 0, done: false, next_t: 0.0 },
            cleared: 0, prey_caught: 0, escapes: 0, times_caught: 0,
            buffs: Buffs::new(), ability_cd: 0.0, ability_t: 0.0, ability_want: false,
            player_move: None, combo_last: None, combo_flash: 0.0, combo_name: "", hunter_stagger: 0.0,
            pickups: Self::scatter_pickups(), clan_scores: [0; 4], outcome: None,
            ancestors, lanes, on_staging: false, on_lane: false,
            minions,
            rivals,
            jungle: Self::place_jungle_camps(),
            weather: Weather::new(crate::weather::Season::Greenleaf),
            fixed_wind: false,
            yowled_t: 0.0,
            markers,
            rock: (0.0, 0.0),
            ritual_progress: [0.0; 4],
            convergence_t: CONVERGENCE_INTERVAL,
            convergence_result: None,
            sustenance: 1.0,
            sick: false,
            tutorial_mode: false,
            twigs: world::twig_positions(900),
            logs: world::log_positions(110),
            snag_t: 0.0,
            was_in_water: false,
            splash_flash: 0.0,
            respawn_flash: 0.0, shielded_flash: 0.0, nine_lives_used: false, time: 0.0, difficulty: 1.0, tick: 0, input: None,
        };
        g.new_objective(Some(ProwlObj::Reach));
        g
    }

    /// Lane network: a spoke from the middle out to each ancestor tree, plus a ring linking
    /// neighbouring trees — the beaten paths you rotate along between staging grounds.
    fn place_lanes(ancestors: &[AncestorTree]) -> Vec<(f32, f32, f32, f32)> {
        let mut v = Vec::new();
        let n = ancestors.len();
        for (i, a) in ancestors.iter().enumerate() {
            v.push((0.0, 0.0, a.x, a.z));                       // spoke from centre
            let b = &ancestors[(i + 1) % n.max(1)];             // ring to the next tree
            v.push((a.x, a.z, b.x, b.z));
        }
        v
    }

    /// Is the point on a beaten lane? (used for the travel bonus and for rendering)
    pub fn on_lane_at(&self, x: f32, z: f32) -> bool {
        self.lanes.iter().any(|&(ax, az, bx, bz)| dist_to_seg(x, z, ax, az, bx, bz) < LANE_HALF_W)
    }

    /// Spawn each clan's border-patrol minions evenly around its home's patrol loop.
    fn spawn_minions(ancestors: &[AncestorTree]) -> Vec<Minion> {
        let mut v = Vec::new();
        for (ci, a) in ancestors.iter().enumerate() {
            for j in 0..MINIONS_PER_CLAN {
                let phase = j as f32 / MINIONS_PER_CLAN as f32 * std::f32::consts::TAU;
                v.push(Minion {
                    clan: ci as u8,
                    x: a.x + phase.cos() * BORDER_R, z: a.z + phase.sin() * BORDER_R,
                    heading: 0.0, home: (a.x, a.z), phase, aggro: false,
                    ranged: j % 2 == 1, yowl_cd: 0.0, yowl_flash: 0.0,
                });
            }
        }
        v
    }

    /// Advance the clan minions: patrol the border loop, or break off to shoo an intruder
    /// (rival clan). A rival minion that spots you raises the hunt alarm.
    fn step_minions(&mut self, dt: f32) {
        let (px, pz) = (self.x, self.z);
        let converging = self.convergence_warning();
        self.yowled_t = (self.yowled_t - dt).max(0.0);
        for m in self.minions.iter_mut() {
            m.yowl_cd = (m.yowl_cd - dt).max(0.0);
            m.yowl_flash = (m.yowl_flash - dt * 2.0).max(0.0);
            let to_p = dist(m.x, m.z, px, pz);
            let aggro_r = if m.ranged { YOWL_RANGE } else { MINION_AGGRO_R };
            m.aggro = !self.tutorial_mode && m.clan != PLAYER_CLAN && to_p < aggro_r;
            let (tx, tz) = if m.aggro && m.ranged {
                // hold a standoff distance rather than closing to melee
                let standoff = YOWL_RANGE * 0.55;
                if to_p > standoff { (px, pz) } else { (m.x, m.z) }
            } else if m.aggro {
                (px, pz)                                     // charge the intruder
            } else if converging {
                self.rock                                     // the convergence draws minions off the border too
            } else {
                m.phase += (MINION_SPEED / BORDER_R) * dt;   // walk the border loop
                (m.home.0 + m.phase.cos() * BORDER_R, m.home.1 + m.phase.sin() * BORDER_R)
            };
            let (dx, dz) = norm(tx - m.x, tz - m.z);
            m.x = (m.x + dx * MINION_SPEED * dt).clamp(-MAP, MAP);
            m.z = (m.z + dz * MINION_SPEED * dt).clamp(-MAP, MAP);
            if dx.abs() + dz.abs() > 1e-4 { m.heading = dx.atan2(dz); }
            // melee contact: a shove back, same feel as a jungle guard
            if m.aggro && !m.ranged && to_p < 2.2 {
                let (px2, pz2) = norm(px - m.home.0, pz - m.home.1);
                self.x = m.home.0 + px2 * (MINION_AGGRO_R + 1.0); self.z = m.home.1 + pz2 * (MINION_AGGRO_R + 1.0);
            }
            // a yowl: a ranged minion's cast — a sharp cry that outs your position and calls the hunt
            if m.aggro && m.ranged && m.yowl_cd <= 0.0 {
                m.yowl_cd = YOWL_CD; m.yowl_flash = 1.0;
                self.yowled_t = YOWL_REVEAL_DUR;
                self.suspicion = (self.suspicion + 0.6).min(1.5);
            }
        }
        if self.minions.iter().any(|m| m.aggro) {            // spotted trespassing → alarm
            self.suspicion = (self.suspicion + dt * 0.8).min(1.5);
            self.last_seen = Some((px, pz));
        }
    }

    /// One rival AI prowler per rival clan, starting at its own home.
    fn spawn_rivals(ancestors: &[AncestorTree]) -> Vec<Rival> {
        let classes = [Class::Cat, Class::Squirrel, Class::Goat, Class::Frog, Class::Hawk, Class::Otter];
        let mut v = Vec::new();
        for clan in 1..ancestors.len().min(N_ANCESTORS) {
            let home = ancestors[clan];
            v.push(Rival {
                clan: clan as u8, class: classes[clan % classes.len()],
                x: home.x, z: home.z, heading: 0.0,
                buffs: Buffs::new(), dwell: 0.0, target: clan, respawn_flash: 0.0, stagger: 0.0,
            });
        }
        v
    }

    /// Advance the rival prowlers: each heads for the nearest tree it doesn't own, dwells to
    /// claim it (flipping ownership + scoring for its clan), and rallies home if the stalker
    /// catches it. This is what makes the map a live four-way contest over the ancestors.
    fn step_rivals(&mut self, dt: f32) {
        let aggressive = self.hstate == HState::Chase || self.suspicion > 0.55;
        for i in 0..self.rivals.len() {
            self.rivals[i].buffs.tick(dt);
            self.rivals[i].respawn_flash = (self.rivals[i].respawn_flash - dt).max(0.0);
            self.rivals[i].stagger = (self.rivals[i].stagger - dt).max(0.0);
            if self.rivals[i].stagger > 0.0 { self.rivals[i].dwell = 0.0; continue; }   // dazed — frozen, claim interrupted
            let (rx, rz, rclan) = { let r = &self.rivals[i]; (r.x, r.z, r.clan) };

            // flee the stalker if it's close and hunting
            if aggressive && dist(rx, rz, self.hx, self.hz) < RIVAL_FLEE_R {
                let (dx, dz) = norm(rx - self.hx, rz - self.hz);
                let speed = self.rivals[i].class.speed_at(rx, rz) * PROWL_SCALE * self.rivals[i].buffs.move_mult();
                let r = &mut self.rivals[i];
                r.x = (r.x + dx * speed * dt).clamp(-MAP, MAP);
                r.z = (r.z + dz * speed * dt).clamp(-MAP, MAP);
                if dx.abs() + dz.abs() > 1e-4 { r.heading = dx.atan2(dz); }
                r.dwell = 0.0;
                continue;   // no claiming while running for its life
            }

            // opportunistic territory: passing near a marker held by someone else, a
            // rival snaps it to its own clan — no dwelling needed, unlike the player
            for mk in self.markers.iter_mut() {
                if mk.held_by != rclan && dist(rx, rz, mk.x, mk.z) < MARKER_R { mk.held_by = rclan; mk.idle = 0.0; mk.dwell = 0.0; }
            }

            // a convergence draws every clan toward the Rock — the forced close encounter
            let pulled_to_rock = self.convergence_warning();

            // pick the nearest ancestor this clan doesn't already own (ready trees preferred)
            let mut best = (f32::MAX, rclan as usize);
            for (ai, a) in self.ancestors.iter().enumerate() {
                if a.owned_by(rclan) { continue; }
                let w = dist(rx, rz, a.x, a.z) + if a.ready() { 0.0 } else { 250.0 };
                if w < best.0 { best = (w, ai); }
            }
            let target = best.1;
            self.rivals[i].target = target;
            let (tx, tz, tready) = if pulled_to_rock { (self.rock.0, self.rock.1, false) } else {
                let a = &self.ancestors[target]; (a.x, a.z, a.ready())
            };

            if dist(rx, rz, tx, tz) > CLAIM_R * 0.75 {
                let speed = self.rivals[i].class.speed_at(rx, rz) * PROWL_SCALE * self.rivals[i].buffs.move_mult() * 0.95;
                let (dx, dz) = norm(tx - rx, tz - rz);
                let r = &mut self.rivals[i];
                r.x = (r.x + dx * speed * dt).clamp(-MAP, MAP);
                r.z = (r.z + dz * speed * dt).clamp(-MAP, MAP);
                if dx.abs() + dz.abs() > 1e-4 { r.heading = dx.atan2(dz); }
                r.dwell = 0.0;
            } else if tready {
                self.rivals[i].dwell += dt;
                if self.rivals[i].dwell >= CLAIM_TIME {
                    self.rivals[i].dwell = 0.0;
                    let stored = self.ancestors[target].stored;
                    self.ancestors[target].claimed_by = Some(rclan);
                    self.ancestors[target].cooldown = ANCESTOR_CD;
                    self.ancestors[target].beacon_class = Some(self.rivals[i].class);
                    self.ancestors[target].pulse_t = 0.0;
                    self.rivals[i].buffs.grant(stored);
                    self.clan_scores[rclan as usize] += 40;
                }
            }

            // the stalker can catch a rival too → it rallies at its clan home, losing points
            if aggressive && dist(self.hx, self.hz, self.rivals[i].x, self.rivals[i].z) < CATCH_DIST {
                let home = self.ancestors[rclan as usize];
                let r = &mut self.rivals[i];
                r.x = home.x; r.z = home.z; r.respawn_flash = 1.0;
                self.clan_scores[rclan as usize] = (self.clan_scores[rclan as usize] - 20).max(0);
            }
        }
    }

    /// Place the ancestor trees well out across the map (so the arena plays big), each on
    /// dry land near a cardinal bearing, storing an alternating Speed / Shield buff.
    fn place_ancestors() -> Vec<AncestorTree> {
        let stored = [Power::Speed, Power::Shield, Power::Shield, Power::Speed];
        let mut v = Vec::new();
        for k in 0..N_ANCESTORS {
            let a = k as f32 / N_ANCESTORS as f32 * std::f32::consts::TAU + 0.4;
            let (mut x, mut z) = (a.cos() * 95.0, a.sin() * 95.0);
            for r in 0..14 {
                let rad = 95.0 - r as f32 * 5.0;
                let (tx, tz) = (a.cos() * rad, a.sin() * rad);
                if world::height_at(tx, tz) >= world::WATER_Y + 0.3 { x = tx; z = tz; break; }
            }
            v.push(AncestorTree { x, z, stored: stored[k], claim: 0.0, cooldown: 0.0, claimed_by: None, beacon_class: None, pulse_t: 0.0 });
        }
        v
    }

    /// Place the jungle camps off the lanes, at a middle radius between the staging
    /// grounds and the map's heart — close enough to fight over, far enough to be a
    /// detour rather than a lane stop.
    fn place_jungle_camps() -> Vec<JungleCamp> {
        let kinds = [CampKind::Raccoon, CampKind::Fox, CampKind::Snake];
        let mut v = Vec::new();
        for k in 0..N_CAMPS {
            let a = (k as f32 + 0.5) / N_CAMPS as f32 * std::f32::consts::TAU;
            let (mut x, mut z) = (a.cos() * 55.0, a.sin() * 55.0);
            for r in 0..14 {
                let rad = 55.0 - r as f32 * 4.0;
                let (tx, tz) = (a.cos() * rad, a.sin() * rad);
                if world::height_at(tx, tz) >= world::WATER_Y + 0.3 { x = tx; z = tz; break; }
            }
            let kind = kinds[k % kinds.len()];
            v.push(JungleCamp { kind, x, z, gx: x, gz: z, home_x: x, home_z: z, guard_alive: true, alert: false, hp: kind.guard_hp(), cooldown: 0.0 });
        }
        v
    }

    /// A claimed ancestor tree is a beacon: every few seconds it pulses its owner's
    /// class-keyed buff (Speed or Shield) to allies of that clan standing nearby —
    /// the player if it's their own clan's tree, or the owning rival if it's theirs.
    fn step_beacons(&mut self, dt: f32) {
        for anc in self.ancestors.iter_mut() {
            let Some(owner) = anc.claimed_by else { continue };
            let Some(bc) = anc.beacon_class else { continue };
            anc.pulse_t = (anc.pulse_t - dt).max(0.0);
            if anc.pulse_t > 0.0 { continue; }
            let mut pulsed = false;
            if owner == PLAYER_CLAN && dist(self.x, self.z, anc.x, anc.z) < BEACON_RADIUS {
                self.buffs.grant(bc.beacon_buff()); pulsed = true;
            }
            for r in self.rivals.iter_mut() {
                if r.clan == owner && dist(r.x, r.z, anc.x, anc.z) < BEACON_RADIUS {
                    r.buffs.grant(bc.beacon_buff()); pulsed = true;
                }
            }
            if pulsed { anc.pulse_t = BEACON_PULSE; }
        }
    }

    /// Territory markers — the ritual's stakes — scattered at their own radius, distinct
    /// from both the ancestor trees and the jungle camps. Each starts belonging to
    /// whichever clan's home is nearest, since territory is never truly neutral ground.
    fn place_markers(ancestors: &[AncestorTree]) -> Vec<TerritoryMarker> {
        let mut v = Vec::new();
        for k in 0..N_MARKERS {
            let a = (k as f32 + 0.2) / N_MARKERS as f32 * std::f32::consts::TAU;
            let (mut x, mut z) = (a.cos() * 75.0, a.sin() * 75.0);
            for r in 0..14 {
                let rad = 75.0 - r as f32 * 4.0;
                let (tx, tz) = (a.cos() * rad, a.sin() * rad);
                if world::height_at(tx, tz) >= world::WATER_Y + 0.3 { x = tx; z = tz; break; }
            }
            let nearest_clan = (0..ancestors.len()).min_by(|&i, &j| dist(x, z, ancestors[i].x, ancestors[i].z).partial_cmp(&dist(x, z, ancestors[j].x, ancestors[j].z)).unwrap()).unwrap_or(0) as u8;
            v.push(TerritoryMarker { x, z, held_by: nearest_clan, dwell: 0.0, idle: 0.0, nearest_clan });
        }
        v
    }

    /// Advance the territory markers (drift back when unattended by the player) and the
    /// convergence clock: every `CONVERGENCE_INTERVAL` seconds, tally who holds the most
    /// markers and award ritual progress — with a bonus for being at the Rock itself.
    /// Reaching `RITUAL_WIN` is a big score swing (folded into the existing score-threshold
    /// win check), not an instant match-ender on its own.
    fn step_ritual(&mut self, dt: f32) {
        for mk in self.markers.iter_mut() {
            let d = dist(self.x, self.z, mk.x, mk.z);
            if d < MARKER_R {
                mk.idle = 0.0;
                if mk.held_by != PLAYER_CLAN {
                    mk.dwell += dt;
                    if mk.dwell >= MARKER_CLAIM_TIME { mk.held_by = PLAYER_CLAN; mk.dwell = 0.0; self.clan_scores[0] += 15; }
                } else {
                    mk.dwell = 0.0;
                }
            } else {
                mk.dwell = (mk.dwell - dt * 0.6).max(0.0);
                if mk.held_by == PLAYER_CLAN {
                    mk.idle += dt;
                    if mk.idle >= MARKER_DRIFT_BACK { mk.held_by = mk.nearest_clan; mk.idle = 0.0; }
                }
            }
        }
        self.convergence_t -= dt;
        if self.convergence_t <= 0.0 {
            let mut counts = [0u32; 4];
            for mk in &self.markers { if (mk.held_by as usize) < 4 { counts[mk.held_by as usize] += 1; } }
            if let Some((winner, _)) = counts.iter().enumerate().max_by_key(|&(_, c)| *c) {
                if counts[winner] > 0 {
                    let mut gain = counts[winner] as f32 * 6.0;
                    if winner == PLAYER_CLAN as usize && dist(self.x, self.z, self.rock.0, self.rock.1) < ROCK_BONUS_R { gain += 20.0; }
                    self.ritual_progress[winner] = (self.ritual_progress[winner] + gain).min(RITUAL_WIN);
                    self.clan_scores[winner] += gain as i32;
                    self.convergence_result = Some(winner as u8);
                }
            }
            self.convergence_t = CONVERGENCE_INTERVAL;
        }
    }

    /// Sustenance — a slow hunger clock. It falls on its own; a caught bit of prey (or
    /// resting at a staging ground) restores it. Bottom it out and you fall sick — slower
    /// until it recovers, same idea as needing to fall back and eat.
    fn step_sustenance(&mut self, dt: f32) {
        self.sustenance = (self.sustenance - SUSTENANCE_DECAY * dt).max(0.0);
        if self.on_staging { self.sustenance = (self.sustenance + 0.05 * dt).min(1.0); }   // resting at base helps too
        if self.sustenance <= SUSTENANCE_SICK_AT { self.sick = true; }
        else if self.sustenance >= SUSTENANCE_WELL_AT { self.sick = false; }
    }

    /// Advance the jungle camps: a guard notices a loud/visible prowler nearby and closes
    /// in; taking the herb needs the guard dead *or* never having noticed you.
    fn step_jungle(&mut self, dt: f32) {
        for camp in self.jungle.iter_mut() {
            camp.cooldown = (camp.cooldown - dt).max(0.0);
            if !camp.guard_alive { continue; }
            let d = dist(self.x, self.z, camp.gx, camp.gz);
            let downwind_bonus = {
                let (dx, dz) = (camp.gx - self.x, camp.gz - self.z);
                let dd = (dx * dx + dz * dz).sqrt();
                if self.moving && dd > 0.5 {
                    let (wdx, wdz) = self.weather.wind_dir_vec();
                    let align = (dx / dd * wdx + dz / dd * wdz).max(0.0);
                    align * self.weather.total_wind()
                } else { 0.0 }
            };
            if !camp.alert && (d < CAMP_ALERT_R * (1.0 + downwind_bonus) && (self.moving && (!self.hidden || downwind_bonus > 0.3))) { camp.alert = true; }
            if camp.alert {
                let (dx, dz) = norm(self.x - camp.gx, self.z - camp.gz);
                camp.gx += dx * CAMP_GUARD_SPEED * dt; camp.gz += dz * CAMP_GUARD_SPEED * dt;
                if dist(self.x, self.z, camp.gx, camp.gz) < 2.4 && self.ability_t <= 0.0 {
                    // a caught-out prowler is shoved back toward the camp's edge, no other penalty
                    let (px, pz) = norm(self.x - camp.home_x, self.z - camp.home_z);
                    self.x = camp.home_x + px * (CAMP_ALERT_R + 1.0); self.z = camp.home_z + pz * (CAMP_ALERT_R + 1.0);
                }
            } else if camp.gx != camp.home_x || camp.gz != camp.home_z {
                // lost interest — amble back home
                let (dx, dz) = norm(camp.home_x - camp.gx, camp.home_z - camp.gz);
                if dist(camp.gx, camp.gz, camp.home_x, camp.home_z) > 0.3 { camp.gx += dx * 2.0 * dt; camp.gz += dz * 2.0 * dt; } else { camp.gx = camp.home_x; camp.gz = camp.home_z; }
            }
        }
        // taking the herb: reach it while the guard is cleared, or while it's alive but never alerted
        if let Some(camp) = self.jungle.iter_mut().find(|c| dist(self.x, self.z, c.x, c.z) < CAMP_HERB_R && c.cooldown <= 0.0 && (c.cleared() || !c.alert)) {
            self.buffs.grant(camp.kind.reward());
            camp.cooldown = CAMP_REGROW;
        }
    }
    fn scatter_pickups() -> Vec<(f32, f32, Power)> {
        let powers = [Power::Speed, Power::Shield, Power::Speed, Power::Shield, Power::Speed, Power::Shield];
        let mut v = Vec::new();
        for k in 0..6 {
            let a = k as f32 / 6.0 * std::f32::consts::TAU;
            let (mut x, mut z) = (a.cos() * 36.0, a.sin() * 36.0);
            for r in 0..10 {
                let rad = 36.0 - r as f32 * 3.0;
                let (tx, tz) = (a.cos() * rad, a.sin() * rad);
                if world::height_at(tx, tz) >= world::WATER_Y + 0.2 { x = tx; z = tz; break; }
            }
            v.push((x, z, powers[k]));
        }
        v
    }

    pub fn set_input(&mut self, mx: f32, mz: f32, leap: bool, dodge: bool, crouch: bool) {
        self.input = Some((mx, mz, leap, dodge, crouch));
    }

    /// Press the class's ultimate ability (consumed on the next `step`).
    /// Freeze the wind (a settings toggle for a more readable, planned-around map).
    pub fn set_fixed_wind(&mut self, on: bool) { self.fixed_wind = on; }
    /// Unit wind direction (the way scent drifts) and its 0..~1.4 strength, for a HUD arrow.
    pub fn wind(&self) -> ((f32, f32), f32) { (self.weather.wind_dir_vec(), self.weather.total_wind()) }

    /// Whether the next convergence is imminent — worth a HUD warning ("the Rock stirs...").
    pub fn convergence_warning(&self) -> bool { self.convergence_t <= CONVERGENCE_WARN }

    /// A safe practice sandbox for the tutorial: the hunter and minions never engage.
    pub fn set_tutorial_mode(&mut self, on: bool) { self.tutorial_mode = on; }

    pub fn set_ability(&mut self, on: bool) { if on { self.ability_want = true; } }

    /// Perform a flick — the gesture-driven autoattack. `dir_local` is the flick in the
    /// prowler's local frame (`.1` forward, `.0` right), `mag` its speed, `low` the
    /// underbelly modifier. Resolves to a move (ported from `combat.rs`, the same
    /// move-set every class shares), chains a combo off the last one if it lands in the
    /// window, and kicks off the movement (pounce/roll reuse leap/dodge). No-op if busy.
    pub fn player_flick(&mut self, dir_local: (f32, f32), mag: f32, low: bool) {
        use crate::combat::{combo, resolve_flick, Move, COMBO_WINDOW};
        if self.player_move.is_some() { return; }
        let mut mv = match resolve_flick(dir_local, mag, low) { Some(m) => m, None => return };
        if let Some((prev, since)) = self.combo_last {
            if since < COMBO_WINDOW {
                if let Some((name, finisher)) = combo(prev, mv) {
                    mv = finisher; self.combo_name = name; self.combo_flash = 1.0;
                }
            }
        }
        let (sh, ch) = (self.heading.sin(), self.heading.cos());
        let (wx, wz) = norm(dir_local.1 * sh + dir_local.0 * ch, dir_local.1 * ch - dir_local.0 * sh);
        match mv {
            Move::Pounce => { self.leap_t = LEAP_DUR; self.leap_cd = 0.5; self.bx = wx; self.bz = wz; }
            Move::Jump   => { self.leap_t = LEAP_DUR * 0.6; self.leap_cd = 0.4; self.bx = wx * 0.35; self.bz = wz * 0.35; }
            Move::Roll | Move::Dodge => { self.dodge_t = 0.28; self.dodge_cd = 0.5; self.bx = wx; self.bz = wz; }
            _ => {}
        }
        self.player_move = Some((mv, 0.0));
    }

    /// Advance the in-progress autoattack move, land a hit on the hunter or a nearby
    /// rival on the frame it turns active, and tick the combo window.
    fn step_combat(&mut self, dt: f32) {
        self.combo_flash = (self.combo_flash - dt * 1.5).max(0.0);
        self.hunter_stagger = (self.hunter_stagger - dt).max(0.0);
        if let Some((m, s)) = self.combo_last { let ns = s + dt; self.combo_last = if ns > 1.0 { None } else { Some((m, ns)) }; }
        if let Some((mv, t)) = self.player_move {
            let sp = crate::combat::spec(mv);
            let nt = t + dt;
            if mv.is_attack() && t < sp.windup && nt >= sp.windup {
                let power = if self.combo_flash > 0.0 { sp.power * crate::combat::COMBO_POWER_BONUS } else { sp.power };
                // the hunter: a landed hit dazes it and rattles its suspicion
                if dist(self.x, self.z, self.hx, self.hz) < MOVE_REACH {
                    self.hunter_stagger = self.hunter_stagger.max(STAGGER_DUR * (0.5 + power * 0.35));
                    self.suspicion = (self.suspicion - 0.35).max(0.0);
                }
                // a rival clan's prowler: a landed hit dazes it too, interrupting its claim
                for r in self.rivals.iter_mut() {
                    if dist(self.x, self.z, r.x, r.z) < MOVE_REACH {
                        r.stagger = r.stagger.max(STAGGER_DUR * (0.5 + power * 0.35));
                    }
                }
                // a jungle guard: a landed hit chips its health; clearing it frees the herb for good
                if let Some(camp) = self.jungle.iter_mut().find(|c| c.guard_alive && dist(self.x, self.z, c.gx, c.gz) < MOVE_REACH) {
                    camp.hp -= 0.5 + power * 0.5;
                    if camp.hp <= 0.0 { camp.guard_alive = false; camp.alert = false; camp.cooldown = 0.0; }
                }
            }
            self.player_move = if nt >= mv.total() { self.combo_last = Some((mv, 0.0)); None } else { Some((mv, nt)) };
        }
    }

    /// The class's ability and whether it can be fired right now (for a HUD).
    pub fn ability_ready(&self) -> bool { self.ability_cd <= 0.0 && self.ability_t <= 0.0 }

    /// The Hawk's Keen Eyes passive: always senses the hunter's position from the air,
    /// regardless of stealth or suspicion. Returns `None` for every other class (for a HUD arrow).
    pub fn keen_eyes_hunter(&self) -> Option<(f32, f32)> {
        if self.class == Class::Hawk { Some((self.hx, self.hz)) } else { None }
    }

    fn a_poi(&self) -> (f32, f32, &'static str) {
        if self.pois.is_empty() { return (0.0, 0.0, "Landmark"); }
        let i = (hashf(self.tick ^ 0xA53F) * self.pois.len() as f32) as usize % self.pois.len();
        self.pois[i]
    }

    fn new_objective(&mut self, force: Option<ProwlObj>) {
        let kind = force.unwrap_or_else(|| {
            match (hashf(self.tick ^ 0x51D0) * 4.0) as u32 {
                0 => ProwlObj::Reach, 1 => ProwlObj::Rendezvous, 2 => ProwlObj::Survive, _ => ProwlObj::Forage,
            }
        });
        let (tx, tz, name) = self.a_poi();
        self.obj = Objective {
            kind, tx, tz, name,
            progress: 0.0,
            time_left: if kind == ProwlObj::Survive { SURVIVE_LEN } else { 0.0 },
            start_caught: self.prey_caught,
            done: false, next_t: 0.0,
        };
    }

    fn complete_objective(&mut self) {
        self.obj.done = true;
        self.cleared += 1;
        self.obj.next_t = NEXT_DELAY;
        self.clan_scores[0] += 100;                                  // clearing an objective banks points
        self.difficulty = (self.difficulty + 0.05).min(1.6); // the hunt escalates, wave by wave
    }

    fn respawn(&mut self) {
        // Cat passive: Nine Lives cheats the first catch each match — a scare, not a rally
        if self.class == Class::Cat && !self.nine_lives_used {
            self.nine_lives_used = true;
            self.shielded_flash = 0.8;
            self.suspicion = 0.2; self.hstate = HState::Search;
            return;
        }
        // a Shield soaks the catch instead — you're spared (brief flash), no respawn
        if !self.buffs.absorb_hit() {
            self.shielded_flash = 0.8;
            self.suspicion = 0.4; self.hstate = HState::Search;
            return;
        }
        self.times_caught += 1; self.respawn_flash = 1.2;
        self.clan_scores[0] = (self.clan_scores[0] - 50).max(0);              // getting caught costs you
        self.buffs = Buffs::new();                          // and clears any buffs
        // rally at the nearest claimed ancestor tree if you have one, else the origin
        let mut best: Option<(f32, f32, f32)> = None;
        for a in self.ancestors.iter().filter(|a| a.owned_by(PLAYER_CLAN)) {
            let d = dist(0.0, 0.0, a.x, a.z);
            if best.map_or(true, |(bd, _, _)| d < bd) { best = Some((d, a.x, a.z)); }
        }
        if let Some((_, ax, az)) = best { self.x = ax; self.z = az; } else { self.x = 0.0; self.z = 0.0; }
        self.heading = 0.0;
        self.leap_t = 0.0; self.dodge_t = 0.0;
        self.hx = -self.hx.signum().max(1.0) * 60.0; self.hz = -60.0;   // reset the hunter far away
        self.hstate = HState::Patrol; self.suspicion = 0.0; self.last_seen = None;
        if self.obj.kind == ProwlObj::Rendezvous { self.obj.progress = 0.0; }
    }

    pub fn step(&mut self, dt: f32) {
        if self.outcome.is_some() { return; }   // match decided — freeze
        self.time += dt;
        self.ring.tick(dt);
        self.exposed = !self.ring.contains(self.x, self.z);
        self.respawn_flash = (self.respawn_flash - dt).max(0.0);
        self.shielded_flash = (self.shielded_flash - dt).max(0.0);
        self.buffs.tick(dt);
        if !self.fixed_wind { self.weather.step(dt); }
        let input = self.input.take();

        // ── player movement ──
        let (ix, iz, want_leap, want_dodge, want_crouch) = input.unwrap_or((0.0, 0.0, false, false, false));
        let (mut dx, mut dz) = norm(ix, iz);
        let sprinting = (dx.abs() + dz.abs() > 1e-4) && !want_crouch;
        let can_sprint = self.stamina > 0.12;
        self.on_lane = self.on_lane_at(self.x, self.z);
        let mut speed = self.class.speed_at(self.x, self.z) * PROWL_SCALE * if sprinting && can_sprint { SPRINT } else { 1.0 } * self.buffs.move_mult() * if self.on_lane { LANE_BONUS } else { 1.0 } * if self.sick { 0.75 } else { 1.0 };

        // dodge (i-frames) and leap (airborne) bursts
        self.dodge_cd = (self.dodge_cd - dt).max(0.0);
        self.dodge_t = (self.dodge_t - dt).max(0.0);
        if want_dodge && self.dodge_cd <= 0.0 && self.dodge_t <= 0.0 && self.stamina > 0.1 {
            let mut d = (dx, dz);
            if d.0.abs() + d.1.abs() < 1e-4 { d = norm(self.x - self.hx, self.z - self.hz); }
            self.dodge_t = DODGE_DUR; self.dodge_cd = DODGE_CD; self.bx = d.0; self.bz = d.1;
            self.stamina = (self.stamina - 0.18).max(0.0);
        }
        self.leap_cd = (self.leap_cd - dt).max(0.0);
        self.leap_t = (self.leap_t - dt).max(0.0);
        if want_leap && self.leap_cd <= 0.0 && self.leap_t <= 0.0 && self.dodge_t <= 0.0 && self.stamina > 0.15 {
            let mut d = (dx, dz);
            if d.0.abs() + d.1.abs() < 1e-4 { d = (self.heading.sin(), self.heading.cos()); }
            self.leap_t = LEAP_DUR; self.leap_cd = LEAP_CD; self.bx = d.0; self.bz = d.1;
            self.stamina = (self.stamina - 0.20).max(0.0);
        }
        let mut airborne = false;
        if self.dodge_t > 0.0 { dx = self.bx; dz = self.bz; speed = DODGE_SPEED; airborne = true; }
        if self.leap_t > 0.0 { dx = self.bx; dz = self.bz; speed = LEAP_SPEED; airborne = true; }

        // ── signature ability ──
        self.ability_cd = (self.ability_cd - dt).max(0.0);
        self.ability_t = (self.ability_t - dt).max(0.0);
        if self.ability_want && self.ability_cd <= 0.0 && self.ability_t <= 0.0 && self.stamina > 0.1 {
            let ab = self.class.ability();
            self.ability_t = ab.duration();
            self.ability_cd = ab.cooldown() * ULT_CD_MULT;
            if ab.refills() { self.stamina = 1.0; }
            let d = if dx.abs() + dz.abs() > 1e-4 { (dx, dz) } else { (self.heading.sin(), self.heading.cos()) };
            self.bx = d.0; self.bz = d.1;
        }
        self.ability_want = false;
        if self.ability_t > 0.0 && self.dodge_t <= 0.0 && self.leap_t <= 0.0 {
            let ab = self.class.ability();
            speed *= ab.speed_mult();
            if ab.speed_mult() >= 1.4 { dx = self.bx; dz = self.bz; airborne = true; } // committed burst
        }

        self.crouched = want_crouch && !airborne;
        let mv = (dx.abs() + dz.abs() > 1e-4) && !self.crouched;

        // ── undergrowth: brambles drag at you, and now and then snag a beat you have to
        //    pull free of — creeping through slips clean, running through doesn't ──
        self.snag_t = (self.snag_t - dt).max(0.0);
        if mv && !airborne {
            let cover = world::cover_at(self.x, self.z);
            if cover > 0.35 { speed *= 1.0 - cover * 0.18; }
            if cover > 0.6 && self.snag_t <= 0.0 && sprinting
                && hashf(self.tick.wrapping_mul(2_654_435_761) ^ 0x9e37) < 0.010 {
                self.snag_t = 0.35;
            }
            if self.snag_t > 0.0 { speed *= 0.45; }
        }

        if mv {
            self.x = (self.x + dx * speed * dt).clamp(-MAP, MAP);
            self.z = (self.z + dz * speed * dt).clamp(-MAP, MAP);
            self.heading = dx.atan2(dz);
            // a fallen log along the route: a natural little bound over it, airborne and
            // briefly uncatchable — the fast, exposed line through the woods
            if self.leap_t <= 0.0 && self.leap_cd <= 0.0 && self.dodge_t <= 0.0
                && self.logs.iter().any(|&(lx, lz)| dist(self.x, self.z, lx, lz) < LOG_BOUND_R) {
                self.leap_t = LEAP_DUR * 0.7; self.leap_cd = 0.7; self.bx = dx; self.bz = dz;
            }
            // a snapped twig underfoot — the quiet, slow route around them stays hidden,
            // running through the woods risks giving you away
            if sprinting && !self.crouched {
                if let Some(&(tx, tz)) = self.twigs.iter().find(|&&(tx, tz)| dist(self.x, self.z, tx, tz) < 0.5) {
                    let hd = dist(self.hx, self.hz, tx, tz);
                    if hd < TWIG_HEAR_R {
                        self.suspicion = (self.suspicion + TWIG_SUSPICION * (1.0 - hd / TWIG_HEAR_R)).min(1.5);
                        self.last_seen = Some((tx, tz));
                    }
                }
            }
        }
        self.moving = mv;
        self.splash_flash = (self.splash_flash - dt * 2.0).max(0.0);
        let now_in_water = world::height_at(self.x, self.z) < world::WATER_Y;
        if mv && now_in_water && !self.was_in_water && sprinting {
            // a loud entry — a splash the hunter can hear, same idea as a snapped twig
            self.splash_flash = 1.0;
            let hd = dist(self.hx, self.hz, self.x, self.z);
            if hd < SPLASH_HEAR_R {
                self.suspicion = (self.suspicion + 0.4 * (1.0 - hd / SPLASH_HEAR_R)).min(1.5);
                self.last_seen = Some((self.x, self.z));
            }
        }
        self.was_in_water = now_in_water;
        self.hidden = self.crouched && self.bushes.iter().any(|(bx, bz, r)| dist(self.x, self.z, *bx, *bz) < *r);
        let slipstream = self.class == Class::Otter && Class::terrain_at(self.x, self.z) == 0;
        self.stamina = (self.stamina + if sprinting && !slipstream { -0.30 } else if sprinting { 0.0 } else { 0.18 } * dt).clamp(0.0, 1.0);
        if self.exposed { self.stamina = (self.stamina - 0.12 * dt).max(0.0); }   // the ring wears you down

        // ── power-ups: grab a nearby one, then replace it on land elsewhere ──
        if let Some(j) = self.pickups.iter().position(|(px, pz, _)| dist(self.x, self.z, *px, *pz) < 2.2) {
            let p = self.pickups.swap_remove(j).2;
            self.buffs.grant(p);
            // drop a fresh one at a scatter point that isn't already occupied
            for cand in Self::scatter_pickups() {
                let (cx, cz, _) = cand;
                if !self.pickups.iter().any(|(px, pz, _)| dist(cx, cz, *px, *pz) < 4.0) {
                    self.pickups.push(cand); break;
                }
            }
        }

        // ── ancestor trees: dwell to claim (grants the stored buff); a claimed tree is a
        //    safe staging ground that mends stamina and cools the hunt ──
        self.on_staging = false;
        for anc in self.ancestors.iter_mut() {
            anc.cooldown = (anc.cooldown - dt).max(0.0);
            let d = dist(self.x, self.z, anc.x, anc.z);
            if d < CLAIM_R && anc.ready() {
                let cache_mult = if self.class == Class::Squirrel { 1.3 } else { 1.0 };
                anc.claim += dt * cache_mult;
                if anc.claim >= CLAIM_TIME {
                    anc.claim = 0.0; anc.cooldown = ANCESTOR_CD; anc.claimed_by = Some(PLAYER_CLAN);
                    anc.beacon_class = Some(self.class); anc.pulse_t = 0.0;
                    self.buffs.grant(anc.stored);       // the ancestors bestow their stored buff
                    self.clan_scores[0] += 40;
                }
            } else if d >= CLAIM_R {
                anc.claim = (anc.claim - dt * 0.5).max(0.0);
            }
            if anc.owned_by(PLAYER_CLAN) && d < STAGE_R { self.on_staging = true; }
        }
        if self.on_staging { self.stamina = (self.stamina + 0.35 * dt).min(1.0); } // mend at the grounds

        // ── prey: flee, and get pounced when we're mid-burst on top of one ──
        {
            let threats = [(self.x, self.z), (self.hx, self.hz)];
            for cr in &mut self.critters { cr.step(dt, &threats); }
        }
        if airborne && !self.critters.is_empty() {
            let (px, pz) = (self.x, self.z);
            let before = self.critters.len();
            self.critters.retain(|cr| dist(px, pz, cr.x, cr.z) >= POUNCE_R);
            let eaten = (before - self.critters.len()) as u32;
            if eaten > 0 {
                self.prey_caught += eaten;
                self.stamina = (self.stamina + 0.25 * eaten as f32).min(1.0);
                self.sustenance = (self.sustenance + SUSTENANCE_PER_PREY * eaten as f32).min(1.0);
                self.clan_scores[0] += 25 * eaten as i32;
            }
        }

        // ── claimed ancestor trees pulse their owner's beacon buff to nearby allies ──
        self.step_beacons(dt);

        // ── territory markers + the Rock of Nine Lives ritual ──
        self.step_ritual(dt);
        self.step_sustenance(dt);

        // ── autoattack: advance the in-progress move, land hits on the hunter/rivals ──
        self.step_combat(dt);

        // ── jungle camps: guarded herb buffs off the lanes ──
        self.step_jungle(dt);

        // ── clan minions: border patrol + intruder defense ──
        self.step_minions(dt);
        if self.yowled_t > 0.0 { self.hidden = false; }   // a landed yowl outs you — cover doesn't help for a beat

        // ── the stalker: sense → chase → catch (respawn) ──
        self.step_hunter(dt);

        // ── rival clans' prowlers contest the ancestor trees ──
        self.step_rivals(dt);

        // ── objectives ──
        self.update_objective(dt);

        // ── win condition: first clan to WIN_SCORE, else the leader when the ring finishes ──
        if let Some(c) = (0..4).find(|&c| self.clan_scores[c] >= WIN_SCORE) {
            self.outcome = Some(c as u8);
        } else if self.ring.t >= self.ring.shrink {
            let max = *self.clan_scores.iter().max().unwrap_or(&0);
            let leaders: Vec<usize> = (0..4).filter(|&c| self.clan_scores[c] == max).collect();
            self.outcome = Some(if leaders.len() == 1 { leaders[0] as u8 } else { 255 });
        }
        self.tick += 1;
    }

    fn step_hunter(&mut self, dt: f32) {
        if self.tutorial_mode { self.suspicion = 0.0; return; }   // a safe sandbox — the hunter never engages
        if self.hunter_stagger > 0.0 { return; }   // dazed by a landed hit — frozen this frame
        let p = self.hunter_kind.params();
        // outside the ring, cover no longer conceals you
        let eff_hidden = self.hidden && !self.exposed;
        let sees = hunter::detects(self.hunter_kind, self.hx, self.hz, self.hheading, self.x, self.z, self.moving, eff_hidden);
        // Goat's Sure-Footed passive: harder to spot while up in the mountains
        let sure_footed = self.class == Class::Goat && Class::terrain_at(self.x, self.z) == 2;
        if sees {
            self.suspicion = (self.suspicion + dt * 2.4 * self.difficulty * if sure_footed { 0.7 } else { 1.0 }).min(1.5);
            self.last_seen = Some((self.x, self.z));
        } else {
            let stage_bonus = if self.on_staging { 1.2 } else { 0.0 };   // a claimed tree cools the hunt
            // Frog's Camouflage passive: suspicion melts away faster while hidden
            let camo_bonus = if eff_hidden && self.class == Class::Frog { 1.0 } else { 0.0 };
            let decay = dt * (if eff_hidden { 1.6 } else { 0.5 } + stage_bonus + camo_bonus);
            // wind/scent: moving downwind of the hunter carries your scent to it even out
            // of sight — this can offset (or beat) the usual decay, so the wind matters
            // for which way you sneak.
            let (dxh, dzh) = (self.hx - self.x, self.hz - self.z);
            let dh = (dxh * dxh + dzh * dzh).sqrt();
            let scent_gain = if self.moving && dh > 0.5 && dh < SCENT_RANGE {
                let (wdx, wdz) = self.weather.wind_dir_vec();
                let align = (dxh / dh * wdx + dzh / dh * wdz).max(0.0);   // > 0: wind carries you toward the hunter
                SCENT_RATE * self.weather.total_wind() * align * dt
            } else { 0.0 };
            self.suspicion = (self.suspicion + scent_gain - decay).clamp(0.0, 1.5);
            if scent_gain > decay * 1.5 { self.last_seen = Some((self.x, self.z)); }   // a strong whiff gives a rough fix
        }
        let prev = self.hstate;
        self.hstate = if sees && self.suspicion > 0.9 { HState::Chase }
            else if self.suspicion > 0.4 { HState::Track }
            else if self.last_seen.is_some() && self.suspicion > 0.1 { HState::Search }
            else { HState::Patrol };
        // lost the quarry after a chase → that's an escape
        if prev == HState::Chase && self.hstate != HState::Chase { self.escapes += 1; }

        // move toward the quarry (or its last seen spot); patrol drifts toward the objective
        let goal = match self.hstate {
            HState::Chase => (self.x, self.z),
            HState::Track | HState::Search => self.last_seen.unwrap_or((self.obj.tx, self.obj.tz)),
            _ => (self.obj.tx, self.obj.tz),
        };
        let (gx, gz) = norm(goal.0 - self.hx, goal.1 - self.hz);
        let hspeed = p.speed * if self.hstate == HState::Chase { 1.0 } else { 0.55 } * self.difficulty;
        self.hx = (self.hx + gx * hspeed * dt).clamp(-MAP, MAP);
        self.hz = (self.hz + gz * hspeed * dt).clamp(-MAP, MAP);
        if gx.abs() + gz.abs() > 1e-4 { self.hheading = gx.atan2(gz); }

        // catch: aggressive + in reach + not airborne/hidden → respawn the prowler
        let aggressive = self.hstate == HState::Chase || self.suspicion > 0.55;
        let ability_invuln = self.ability_t > 0.0 && self.class.ability().invuln();
        if aggressive && self.leap_t <= 0.0 && self.dodge_t <= 0.0 && !ability_invuln && !(self.hidden && !self.exposed)
            && dist(self.hx, self.hz, self.x, self.z) < CATCH_DIST {
            self.respawn();
        }
    }

    fn update_objective(&mut self, dt: f32) {
        if self.obj.done {
            self.obj.next_t -= dt;
            if self.obj.next_t <= 0.0 { self.new_objective(None); }
            return;
        }
        match self.obj.kind {
            ProwlObj::Reach => {
                if dist(self.x, self.z, self.obj.tx, self.obj.tz) < REACH_R { self.complete_objective(); }
            }
            ProwlObj::Rendezvous => {
                if dist(self.x, self.z, self.obj.tx, self.obj.tz) < RENDEZVOUS_R {
                    self.obj.progress += dt;
                    if self.obj.progress >= RENDEZVOUS_NEED { self.complete_objective(); }
                } else {
                    self.obj.progress = (self.obj.progress - dt * 0.5).max(0.0);
                }
            }
            ProwlObj::Survive => {
                self.obj.time_left -= dt;
                if self.obj.time_left <= 0.0 { self.complete_objective(); }
            }
            ProwlObj::Forage => {
                if self.prey_caught - self.obj.start_caught >= FORAGE_NEED { self.complete_objective(); }
            }
        }
    }

    /// One-line objective banner for a HUD.
    pub fn objective_text(&self) -> String {
        if self.obj.done { return format!("✔ complete!   ·   cleared: {}", self.cleared); }
        let body = match self.obj.kind {
            ProwlObj::Reach => format!("Reach the {}", self.obj.name),
            ProwlObj::Rendezvous => format!("Hold the {} ({:.1}/{:.0}s)", self.obj.name, self.obj.progress, RENDEZVOUS_NEED),
            ProwlObj::Survive => format!("Survive the hunt — {}s", self.obj.time_left.max(0.0).ceil() as i32),
            ProwlObj::Forage => format!("Catch prey — {}/{}", self.prey_caught - self.obj.start_caught, FORAGE_NEED),
        };
        format!("{}   ·   cleared: {}", body, self.cleared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reach_objective_clears_and_rotates() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.new_objective(Some(ProwlObj::Reach));
        let cleared0 = g.cleared;
        // teleport onto the target
        g.x = g.obj.tx; g.z = g.obj.tz;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.obj.done, "standing on the target should clear a Reach");
        assert_eq!(g.cleared, cleared0 + 1);
        // after the delay, a fresh objective spawns
        for _ in 0..((NEXT_DELAY + 0.2) * 30.0) as i32 { g.set_input(0.0, 0.0, false, false, false); g.step(1.0 / 30.0); }
        assert!(!g.obj.done, "a new objective should be live after the delay");
    }

    #[test]
    fn survive_objective_counts_down_and_clears() {
        let mut g = Prowl::new(Class::Goat, Kind::Snake);
        g.new_objective(Some(ProwlObj::Survive));
        g.hx = 300.0; g.hz = 300.0;   // keep the hunter far so we aren't caught
        let mut cleared = false;
        for _ in 0..((SURVIVE_LEN + 2.0) * 30.0) as i32 {
            g.hx = 300.0; g.hz = 300.0;
            g.set_input(0.0, 0.0, false, false, false);
            g.step(1.0 / 30.0);
            if g.cleared > 0 { cleared = true; break; }
        }
        assert!(cleared, "outlasting the survive timer should clear it");
    }

    #[test]
    fn leaping_onto_prey_scores_and_feeds_forage() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.new_objective(Some(ProwlObj::Forage));
        g.hx = 300.0; g.hz = 300.0;
        g.x = 0.0; g.z = 0.0; g.heading = 0.0;
        // a critter right in front; leap onto it
        g.critters.clear();
        g.critters.push(Critter::new(crate::ecology::Species::Hare, 0.0, 1.0, 7));
        let before = g.prey_caught;
        g.set_input(0.0, 1.0, true, false, false);
        g.step(1.0 / 30.0);
        assert!(g.prey_caught > before, "a leap onto prey should score");
    }

    #[test]
    fn tutorial_mode_makes_the_hunter_and_minions_harmless() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.set_tutorial_mode(true);
        g.hstate = HState::Chase; g.suspicion = 1.4;
        g.hx = 0.0; g.hz = 0.0; g.x = 0.0; g.z = 0.0;   // point-blank, would normally be an instant catch
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert_eq!(g.times_caught, 0, "the hunter shouldn't be able to catch the player in tutorial mode");
        assert_eq!(g.suspicion, 0.0, "suspicion should stay pinned at zero in tutorial mode");
        let idx = g.minions.iter().position(|m| m.clan != PLAYER_CLAN).unwrap();
        let (mx, mz) = (g.minions[idx].x, g.minions[idx].z);
        g.x = mx; g.z = mz;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(!g.minions[idx].aggro, "minions shouldn't turn aggressive on the player in tutorial mode");
    }

    #[test]
    fn getting_caught_respawns_the_prowler() {
        let mut g = Prowl::new(Class::Cat, Kind::Dog);
        g.nine_lives_used = true;   // test generic catch behaviour, not Nine Lives
        // put the hunter on top, locked on
        g.x = 20.0; g.z = 0.0; g.hx = 20.0; g.hz = 0.0;
        g.hstate = HState::Chase; g.suspicion = 1.4;
        let caught0 = g.times_caught;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert_eq!(g.times_caught, caught0 + 1, "a point-blank aggressive hunter catches you");
        assert!(g.x.abs() < 1e-3 && g.z.abs() < 1e-3, "you respawn at the origin");
        assert!(g.respawn_flash > 0.0);
    }

    #[test]
    fn a_shield_soaks_a_catch_instead_of_respawning() {
        let mut g = Prowl::new(Class::Cat, Kind::Dog);
        g.nine_lives_used = true;   // test the shield, not Nine Lives
        g.buffs.grant(Power::Shield);
        g.x = 20.0; g.z = 0.0; g.hx = 20.0; g.hz = 0.0;
        g.hstate = HState::Chase; g.suspicion = 1.4;
        let caught0 = g.times_caught;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert_eq!(g.times_caught, caught0, "the shield should spare you the catch");
        assert!(!g.buffs.shield, "and the shield is spent");
        assert!(g.shielded_flash > 0.0);
    }

    #[test]
    fn grabbing_a_powerup_grants_a_buff() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 300.0; g.hz = 300.0;                 // keep the hunter away
        g.pickups.clear();
        g.pickups.push((0.0, 1.0, Power::Speed));   // a Speed pickup underfoot
        g.x = 0.0; g.z = 0.0;
        assert!(!g.buffs.speed_boosted());
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.buffs.speed_boosted(), "walking onto a Speed power-up should boost you");
        assert!(!g.pickups.is_empty(), "and a fresh power-up is scattered to replace it");
    }

    #[test]
    fn clearing_an_objective_banks_score_and_escalates() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 300.0; g.hz = 300.0;
        g.new_objective(Some(ProwlObj::Reach));
        let (d0, s0) = (g.difficulty, g.clan_scores[0]);
        g.x = g.obj.tx; g.z = g.obj.tz;             // stand on the target
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.obj.done);
        assert_eq!(g.clan_scores[0], s0 + 100, "clearing banks points");
        assert!(g.difficulty > d0, "and the hunt escalates");
    }

    #[test]
    fn dwelling_at_an_ancestor_tree_claims_its_buff() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 300.0; g.hz = 300.0;                 // hunter away
        let (ax, az, stored) = { let a = &g.ancestors[0]; (a.x, a.z, a.stored) };
        g.buffs = crate::powerup::Buffs::new();
        for _ in 0..((CLAIM_TIME + 0.3) * 30.0) as i32 {
            g.x = ax; g.z = az;                     // keep dwelling at the tree
            g.set_input(0.0, 0.0, false, false, false);
            g.step(1.0 / 30.0);
        }
        assert!(g.ancestors[0].claimed(), "dwelling should claim the tree");
        assert!(g.ancestors[0].cooldown > 0.0, "a claimed tree goes on cooldown");
        match stored {
            Power::Speed => assert!(g.buffs.speed_boosted(), "a Speed tree grants haste"),
            Power::Shield => assert!(g.buffs.shield, "a Shield tree grants a ward"),
            _ => {}
        }
        assert!(g.on_staging, "standing at a claimed tree is a staging ground");
    }

    #[test]
    fn claiming_a_tree_sets_its_beacon_to_the_claiming_class() {
        let mut g = Prowl::new(Class::Squirrel, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        let (ax, az) = (g.ancestors[0].x, g.ancestors[0].z);
        for _ in 0..((CLAIM_TIME + 0.3) * 30.0) as i32 {
            g.x = ax; g.z = az;
            g.set_input(0.0, 0.0, false, false, false);
            g.step(1.0 / 30.0);
        }
        assert_eq!(g.ancestors[0].beacon_class, Some(Class::Squirrel), "the tree's beacon should key to whoever claimed it");
    }

    #[test]
    fn a_beacon_pulses_its_buff_to_the_player_standing_in_its_aura() {
        let mut g = Prowl::new(Class::Squirrel, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        g.ancestors[0].claimed_by = Some(PLAYER_CLAN);
        g.ancestors[0].beacon_class = Some(Class::Squirrel);   // Speed beacon
        g.ancestors[0].pulse_t = 0.0;
        g.x = g.ancestors[0].x; g.z = g.ancestors[0].z;
        g.buffs = crate::powerup::Buffs::new();
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.buffs.speed_boosted(), "standing in a Speed beacon's aura should grant Speed");
        assert!(g.ancestors[0].pulse_t > 0.0, "the pulse should reset its own cooldown");
    }

    #[test]
    fn a_beacon_does_not_pulse_outside_its_radius() {
        let mut g = Prowl::new(Class::Squirrel, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        g.ancestors[0].claimed_by = Some(PLAYER_CLAN);
        g.ancestors[0].beacon_class = Some(Class::Squirrel);
        g.ancestors[0].pulse_t = 0.0;
        g.x = g.ancestors[0].x + BEACON_RADIUS + 5.0; g.z = g.ancestors[0].z;
        g.buffs = crate::powerup::Buffs::new();
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(!g.buffs.speed_boosted(), "too far from the tree shouldn't get the beacon's buff");
    }

    #[test]
    fn a_beacon_also_buffs_the_owning_rival_nearby() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0; g.x = 900.0; g.z = 900.0;
        let rclan = g.rivals[0].clan;
        g.ancestors[rclan as usize].claimed_by = Some(rclan);
        g.ancestors[rclan as usize].beacon_class = Some(Class::Squirrel);
        g.ancestors[rclan as usize].pulse_t = 0.0;
        let (ax, az) = (g.ancestors[rclan as usize].x, g.ancestors[rclan as usize].z);
        g.rivals[0].x = ax; g.rivals[0].z = az;
        g.rivals[0].buffs = crate::powerup::Buffs::new();
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.rivals[0].buffs.speed_boosted(), "the beacon should also buff its owning clan's rival prowler");
    }

    #[test]
    fn dwelling_at_a_rival_held_marker_flips_it_to_the_player() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        for r in g.rivals.iter_mut() { r.x = 900.0; r.z = 900.0; }   // keep rivals from opportunistically re-flipping it
        let idx = g.markers.iter().position(|m| m.held_by != PLAYER_CLAN).expect("some marker should start rival-held");
        let (mx, mz) = (g.markers[idx].x, g.markers[idx].z);
        for _ in 0..((MARKER_CLAIM_TIME + 0.3) * 30.0) as i32 {
            g.x = mx; g.z = mz;
            for r in g.rivals.iter_mut() { r.x = 900.0; r.z = 900.0; }
            g.set_input(0.0, 0.0, false, false, false);
            g.step(1.0 / 30.0);
        }
        assert_eq!(g.markers[idx].held_by, PLAYER_CLAN, "dwelling long enough should flip the marker to the player");
    }

    #[test]
    fn a_passing_rival_snaps_a_player_held_marker_back() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0; g.x = 900.0; g.z = 900.0;
        let rclan = g.rivals[0].clan;
        g.markers[0].held_by = PLAYER_CLAN; g.markers[0].idle = 0.0;
        g.rivals[0].x = g.markers[0].x; g.rivals[0].z = g.markers[0].z;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert_eq!(g.markers[0].held_by, rclan, "a rival passing right by a player-held marker should snap it back");
    }

    #[test]
    fn a_convergence_warning_pulls_rivals_toward_the_rock() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0; g.x = 900.0; g.z = 900.0;
        // put a rival far from the Rock and out of range of any ancestor it doesn't own,
        // so under normal play it wouldn't be heading toward the Rock's coordinates
        g.rivals[0].x = 200.0; g.rivals[0].z = 200.0;
        let d0 = dist(g.rivals[0].x, g.rivals[0].z, g.rock.0, g.rock.1);
        g.convergence_t = CONVERGENCE_WARN - 1.0;   // inside the warning window
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        let d1 = dist(g.rivals[0].x, g.rivals[0].z, g.rock.0, g.rock.1);
        assert!(d1 < d0, "a convergence warning should pull rivals toward the Rock");
    }

    #[test]
    fn a_convergence_warning_also_pulls_idle_minions_toward_the_rock() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0; g.x = 900.0; g.z = 900.0;
        let idx = g.minions.iter().position(|m| m.clan != PLAYER_CLAN).unwrap();
        g.minions[idx].x = 200.0; g.minions[idx].z = 200.0;
        let d0 = dist(g.minions[idx].x, g.minions[idx].z, g.rock.0, g.rock.1);
        g.convergence_t = CONVERGENCE_WARN - 1.0;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        let d1 = dist(g.minions[idx].x, g.minions[idx].z, g.rock.0, g.rock.1);
        assert!(d1 < d0, "a convergence warning should draw idle minions off the border toward the Rock too");
    }

    #[test]
    fn an_unattended_marker_drifts_back_to_its_nearest_clan() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0; g.x = 900.0; g.z = 900.0;
        g.markers[0].held_by = PLAYER_CLAN;
        g.markers[0].idle = 0.0;
        for _ in 0..((MARKER_DRIFT_BACK + 1.0) * 30.0) as i32 {
            g.set_input(0.0, 0.0, false, false, false);
            g.step(1.0 / 30.0);
        }
        assert_eq!(g.markers[0].held_by, g.markers[0].nearest_clan, "an unattended marker should drift back to whoever's closest");
    }

    #[test]
    fn a_convergence_awards_ritual_progress_to_the_marker_leader() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0; g.x = 900.0; g.z = 900.0;
        for mk in g.markers.iter_mut() { mk.held_by = PLAYER_CLAN; }
        g.convergence_t = 0.01;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert_eq!(g.convergence_result, Some(PLAYER_CLAN));
        assert!(g.ritual_progress[PLAYER_CLAN as usize] > 0.0, "holding all the markers at a convergence should award ritual progress");
        assert!(g.convergence_t > 0.0, "the convergence clock should reset");
    }

    #[test]
    fn being_at_the_rock_during_a_convergence_adds_a_bonus() {
        let progress_at = |at_rock: bool| -> f32 {
            let mut g = Prowl::new(Class::Cat, Kind::Fox);
            g.hx = 900.0; g.hz = 900.0;
            for mk in g.markers.iter_mut() { mk.held_by = PLAYER_CLAN; }
            g.x = if at_rock { g.rock.0 } else { 900.0 };
            g.z = if at_rock { g.rock.1 } else { 900.0 };
            g.convergence_t = 0.01;
            g.set_input(0.0, 0.0, false, false, false);
            g.step(1.0 / 30.0);
            g.ritual_progress[PLAYER_CLAN as usize]
        };
        assert!(progress_at(true) > progress_at(false), "being at the Rock when it resolves should add a bonus");
    }

    #[test]
    fn sustenance_falls_over_time_and_a_caught_prey_restores_it() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        let s0 = g.sustenance;
        for _ in 0..300 { g.set_input(0.0, 0.0, false, false, false); g.step(1.0 / 30.0); }
        assert!(g.sustenance < s0, "sustenance should fall on its own over time");
        let before = g.sustenance;
        g.critters.clear();
        g.critters.push(Critter::new(crate::ecology::Species::Hare, g.x, g.z + 1.0, 7));
        g.heading = 0.0;
        g.set_input(0.0, 1.0, true, false, false);
        g.step(1.0 / 30.0);
        assert!(g.sustenance > before, "a caught bit of prey should restore sustenance");
    }

    #[test]
    fn bottoming_out_sustenance_makes_the_prowler_sick_and_slower() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        g.sustenance = 0.0;
        g.set_input(1.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.sick, "bottoming out sustenance should make the prowler sick");
        let x1 = g.x;
        g.set_input(1.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        let sick_delta = g.x - x1;
        let mut h = Prowl::new(Class::Cat, Kind::Fox);
        h.hx = 900.0; h.hz = 900.0;
        h.set_input(1.0, 0.0, false, false, false);
        h.step(1.0 / 30.0);
        let hx1 = h.x;
        h.set_input(1.0, 0.0, false, false, false);
        h.step(1.0 / 30.0);
        let healthy_delta = h.x - hx1;
        assert!(sick_delta < healthy_delta, "sickness should slow the prowler down relative to healthy");
    }

    #[test]
    fn respawn_rallies_at_a_claimed_ancestor() {
        let mut g = Prowl::new(Class::Cat, Kind::Dog);
        g.nine_lives_used = true;   // test the rally point, not Nine Lives
        g.ancestors[0].claimed_by = Some(0);
        let (ax, az) = (g.ancestors[0].x, g.ancestors[0].z);
        g.x = 20.0; g.z = 0.0; g.hx = 20.0; g.hz = 0.0;   // caught out in the open
        g.hstate = HState::Chase; g.suspicion = 1.4;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!((g.x - ax).abs() < 1.0 && (g.z - az).abs() < 1.0, "should rally at the claimed ancestor, not the origin");
    }

    #[test]
    fn lanes_speed_up_travel() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 300.0; g.hz = 300.0;
        let a0 = g.ancestors[0];
        let (mx, mz) = (a0.x * 0.5, a0.z * 0.5);           // midpoint of the centre→tree spoke
        assert!(g.on_lane_at(mx, mz), "a spoke midpoint sits on a lane");
        let dir = norm(a0.x, a0.z);
        let step_from = |g: &mut Prowl| -> f32 {
            g.x = mx; g.z = mz; g.stamina = 1.0;
            g.set_input(dir.0, dir.1, false, false, false);
            let (x0, z0) = (g.x, g.z);
            g.step(1.0 / 30.0);
            dist(x0, z0, g.x, g.z)
        };
        let on = step_from(&mut g);
        g.lanes.clear();                                   // same spot, but now off any lane
        let off = step_from(&mut g);
        assert!(on > off + 1e-3, "travel should be faster along a lane ({on} vs {off})");
    }

    #[test]
    fn a_ranged_minion_yowls_from_a_standoff_instead_of_closing_to_melee() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 800.0; g.hz = 800.0;
        let idx = g.minions.iter().position(|m| m.clan != PLAYER_CLAN && m.ranged).unwrap();
        let (mx, mz) = (g.minions[idx].x, g.minions[idx].z);
        g.x = mx + (YOWL_RANGE - 1.0); g.z = mz;   // inside yowl range but outside melee aggro
        g.suspicion = 0.0;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.minions[idx].aggro, "a ranged minion should notice from beyond the melee radius");
        assert!(dist(g.minions[idx].x, g.minions[idx].z, g.x, g.z) > 2.5, "it should hold a standoff, not close to melee");
    }

    #[test]
    fn a_landed_yowl_outs_a_hidden_prowler() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 800.0; g.hz = 800.0;
        let idx = g.minions.iter().position(|m| m.clan != PLAYER_CLAN && m.ranged).unwrap();
        let (mx, mz) = (g.minions[idx].x, g.minions[idx].z);
        g.x = mx + (YOWL_RANGE - 1.0); g.z = mz;
        g.hidden = true;
        g.suspicion = 0.0;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.yowled_t > 0.0, "the yowl should land and out the player for a beat");
        assert!(!g.hidden, "an outed player shouldn't be able to hide");
        assert!(g.suspicion > 0.0, "a landed yowl should spike suspicion");
    }

    #[test]
    fn a_melee_minion_shoves_the_player_back_on_contact() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 800.0; g.hz = 800.0;
        let idx = g.minions.iter().position(|m| m.clan != PLAYER_CLAN && !m.ranged).unwrap();
        let (mx, mz) = (g.minions[idx].x, g.minions[idx].z);
        g.x = mx; g.z = mz;   // right on top of it
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(dist(g.x, g.z, g.minions[idx].home.0, g.minions[idx].home.1) > 2.5, "contact should shove the player back out toward the border");
    }

    #[test]
    fn clan_minions_border_patrol_their_home() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 800.0; g.hz = 800.0; g.x = 800.0; g.z = 800.0;   // player far → no aggro
        assert_eq!(g.minions.len(), N_ANCESTORS * MINIONS_PER_CLAN);
        let p0 = (g.minions[0].x, g.minions[0].z);
        for _ in 0..30 { g.x = 800.0; g.z = 800.0; g.set_input(0.0, 0.0, false, false, false); g.step(1.0 / 30.0); }
        let p1 = (g.minions[0].x, g.minions[0].z);
        assert!(dist(p0.0, p0.1, p1.0, p1.1) > 0.5, "a minion should walk its patrol");
        let home = g.minions[0].home;
        assert!(dist(p1.0, p1.1, home.0, home.1) < BORDER_R + 8.0, "and stay near its clan's border");
        assert!(!g.minions[0].aggro);
    }

    #[test]
    fn rival_minions_defend_the_border_and_raise_the_alarm() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 800.0; g.hz = 800.0;
        let idx = g.minions.iter().position(|m| m.clan != PLAYER_CLAN).unwrap();
        let (mx, mz) = (g.minions[idx].x, g.minions[idx].z);
        g.x = mx + 3.0; g.z = mz;                               // intrude on a rival's border
        g.suspicion = 0.0;
        let d0 = dist(g.minions[idx].x, g.minions[idx].z, g.x, g.z);
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.minions[idx].aggro, "a rival minion should defend against an intruder");
        assert!(dist(g.minions[idx].x, g.minions[idx].z, g.x, g.z) <= d0, "it advances on you");
        assert!(g.suspicion > 0.0, "being spotted trespassing raises the alarm");
    }

    #[test]
    fn rival_prowlers_contest_the_ancestor_trees() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0; g.x = 900.0; g.z = 900.0;   // keep the player + hunter clear
        assert_eq!(g.rivals.len(), N_ANCESTORS - 1, "one AI prowler per rival clan");
        let rclan = g.rivals[0].clan;
        let (ax, az) = (g.ancestors[0].x, g.ancestors[0].z);     // a wild tree
        let s0 = g.clan_scores[rclan as usize];
        for _ in 0..((CLAIM_TIME + 0.6) * 30.0) as i32 {
            g.rivals[0].x = ax; g.rivals[0].z = az;              // hold the rival on the tree
            g.x = 900.0; g.z = 900.0;
            g.set_input(0.0, 0.0, false, false, false);
            g.step(1.0 / 30.0);
        }
        assert_eq!(g.ancestors[0].claimed_by, Some(rclan), "a rival should claim the tree for its clan");
        assert!(g.clan_scores[rclan as usize] > s0, "and bank score for its clan");
    }

    #[test]
    fn reaching_the_score_threshold_wins_and_freezes() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0; g.x = 900.0; g.z = 900.0;
        g.clan_scores[2] = WIN_SCORE;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert_eq!(g.outcome, Some(2), "first clan to the threshold wins");
        let t = g.time;
        g.step(1.0 / 30.0);
        assert_eq!(g.time, t, "a decided match freezes");
    }

    #[test]
    fn ring_close_awards_the_leader() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0; g.x = 900.0; g.z = 900.0;
        g.clan_scores = [30, 90, 10, 50];        // clan 1 leads, none at the threshold
        g.ring.t = g.ring.shrink;                // the ring has finished closing
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert_eq!(g.outcome, Some(1), "the leader takes it when the ring closes");
    }

    #[test]
    fn thick_cover_drags_at_a_sprinting_prowler() {
        let speed_at = |cover_heavy: bool| -> f32 {
            let mut g = Prowl::new(Class::Cat, Kind::Fox);
            g.hx = 900.0; g.hz = 900.0;
            // find a spot with the cover level we want
            let mut best = (-1.0f32, 0.0f32, 0.0f32);
            for i in -140..140 {
                for j in -140..140 {
                    let (x, z) = (i as f32 * 2.0, j as f32 * 2.0);
                    if Class::terrain_at(x, z) != 1 { continue; }   // strictly land — keep the base speed comparable
                    let c = world::cover_at(x, z);
                    let score = if cover_heavy { c } else { -c };
                    if score > best.0 { best = (score, x, z); }
                }
            }
            let (px, pz) = (best.1, best.2);
            g.x = px; g.z = pz;
            let x0 = g.x;
            g.set_input(1.0, 0.0, false, false, false);
            g.step(1.0 / 30.0);
            (g.x - x0).abs()
        };
        assert!(speed_at(true) < speed_at(false), "thick cover should drag at a running prowler more than open ground");
    }

    #[test]
    fn a_log_gives_a_running_prowler_a_natural_bound() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        g.logs = vec![(2.0, 0.0)];   // a fallen log just ahead
        g.x = 0.0; g.z = 0.0; g.heading = 0.0;
        for _ in 0..20 {
            g.set_input(1.0, 0.0, false, false, false);
            g.step(1.0 / 30.0);
            if g.leap_t > 0.0 { break; }
        }
        assert!(g.leap_t > 0.0, "running past a fallen log should trigger a natural little bound");
    }

    #[test]
    fn sprinting_over_a_twig_near_the_hunter_raises_suspicion() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.twigs = vec![(2.0, 0.0)];
        g.x = 0.0; g.z = 0.0; g.heading = 0.0;
        g.hx = 5.0; g.hz = 0.0;   // close enough to hear it
        g.suspicion = 0.0;
        for _ in 0..20 {
            g.set_input(1.0, 0.0, false, false, false);
            g.step(1.0 / 30.0);
            if g.suspicion > 0.0 { break; }
        }
        assert!(g.suspicion > 0.0, "sprinting over a twig within earshot of the hunter should raise suspicion");
    }

    #[test]
    fn sprinting_into_water_splashes_and_is_heard() {
        let mut wx = 0.0; let mut wz = 0.0; let mut lx = 0.0; let mut lz = 0.0; let mut found = false;
        for i in -80..80 {
            let (x, z) = (i as f32 * 3.0, 0.0);
            if world::height_at(x, z) < world::WATER_Y && world::height_at(x - 3.0, z) >= world::WATER_Y {
                wx = x; wz = z; lx = x - 3.0; lz = z; found = true; break;
            }
        }
        assert!(found, "expected a land/water edge on the map");
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.x = lx; g.z = lz; g.heading = 0.0;
        g.hx = wx + 3.0; g.hz = wz;   // close enough to hear the splash
        g.suspicion = 0.0;
        for _ in 0..20 {
            g.set_input((wx - g.x).signum(), 0.0, false, false, false);
            g.step(1.0 / 30.0);
            if g.splash_flash > 0.0 { break; }
        }
        assert!(g.splash_flash > 0.0, "sprinting into water should trigger the splash");
        assert!(g.suspicion > 0.0, "and a nearby hunter should hear it");
    }

    #[test]
    fn crouching_past_a_twig_stays_quiet() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.twigs = vec![(0.0, 0.0)];
        g.x = 0.0; g.z = 0.0;
        g.hx = 5.0; g.hz = 0.0;
        g.suspicion = 0.0;
        g.set_input(0.0, 0.0, false, false, true);   // crouched — no twig noise from standing still, either
        g.step(1.0 / 30.0);
        assert_eq!(g.suspicion, 0.0, "a crouched, stationary prowler shouldn't snap a twig");
    }

    #[test]
    fn cat_second_wind_refills_and_shields() {
        let mut g = Prowl::new(Class::Cat, Kind::Dog);
        g.x = 20.0; g.z = 0.0; g.hx = 20.0; g.hz = 0.0;
        g.hstate = HState::Chase; g.suspicion = 1.4;
        g.stamina = 0.25;
        let caught0 = g.times_caught;
        g.set_ability(true);
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.ability_t > 0.0 && g.ability_cd > 0.0, "the ability engages and goes on cooldown");
        assert!(g.stamina > 0.9, "Second Wind refills stamina");
        assert_eq!(g.times_caught, caught0, "and briefly makes you untouchable");
    }

    #[test]
    fn squirrel_scamper_bursts_speed() {
        let travel = |ability: bool| -> f32 {
            let mut g = Prowl::new(Class::Squirrel, Kind::Fox);
            g.hx = 400.0; g.hz = 400.0; g.x = 0.0; g.z = 0.0;
            if ability { g.set_ability(true); }
            g.set_input(0.0, 1.0, false, false, false);
            let z0 = g.z;
            g.step(1.0 / 30.0);
            g.z - z0
        };
        assert!(travel(true) > travel(false) + 1e-3, "Scamper should cover more ground");
    }

    #[test]
    fn airborne_prowler_is_uncatchable() {
        let mut g = Prowl::new(Class::Frog, Kind::Dog);
        g.x = 20.0; g.z = 0.0; g.hx = 20.0; g.hz = 0.0;
        g.hstate = HState::Chase; g.suspicion = 1.4;
        let caught0 = g.times_caught;
        g.set_input(0.0, 1.0, true, false, false);   // leap
        g.step(1.0 / 30.0);
        assert_eq!(g.times_caught, caught0, "a leaping prowler can't be caught");
    }

    #[test]
    fn prowl_invariants_hold_across_combos() {
        // Prowl every class against a spread of hunters with a live, wiggling player.
        // Stamina stays in range, the prowler and hunter stay finite and on the map,
        // cleared/prey/caught only climb, and nothing panics.
        for cls in [Class::Cat, Class::Otter, Class::Goat, Class::Hawk] {
            for hk in [Kind::Fox, Kind::Hawk, Kind::Snake, Kind::Shadow] {
                let mut g = Prowl::new(cls, hk);
                let (mut pc, mut cc) = (0u32, 0u32);
                for t in 0..(60 * 30) {
                    g.set_input((t as f32 * 0.10).sin(), (t as f32 * 0.08).cos(), t % 40 == 0, t % 25 == 0, t % 12 < 3);
                    if t % 17 == 0 { g.player_flick(((t as f32 * 0.37).sin(), (t as f32 * 0.29).cos()), 1.0, t % 34 == 0); }
                    g.step(1.0 / 30.0);
                    assert!(g.stamina >= 0.0 && g.stamina <= 1.0, "stamina out of range {} ({cls:?}/{hk:?})", g.stamina);
                    assert!(g.x.is_finite() && g.z.is_finite() && g.hx.is_finite() && g.hz.is_finite(), "position not finite");
                    assert!(g.x.abs() <= MAP + 1.0 && g.z.abs() <= MAP + 1.0, "prowler left the map");
                    assert!(g.hx.abs() <= MAP + 1.0 && g.hz.abs() <= MAP + 1.0, "hunter left the map");
                    assert!(g.prey_caught >= pc && g.times_caught >= cc, "a tally went backwards");
                    assert!(g.hunter_stagger >= 0.0 && g.combo_flash >= 0.0, "combat timers went negative");
                    for r in &g.rivals { assert!(r.stagger >= 0.0 && r.x.is_finite() && r.z.is_finite(), "rival state broke"); }
                    for c in &g.jungle { assert!(c.hp.is_finite() && c.cooldown >= 0.0 && c.gx.is_finite() && c.gz.is_finite(), "jungle camp state broke"); }
                    for m in &g.minions { assert!(m.yowl_cd >= 0.0 && m.x.is_finite() && m.z.is_finite(), "minion state broke"); }
                    for mk in &g.markers { assert!(mk.dwell >= 0.0 && mk.idle >= 0.0 && (mk.held_by as usize) < 4, "marker state broke"); }
                    assert!(g.sustenance >= 0.0 && g.sustenance <= 1.0, "sustenance out of range");
                    assert!(g.convergence_t.is_finite(), "convergence clock broke");
                    for &p in &g.ritual_progress { assert!(p >= 0.0 && p <= RITUAL_WIN, "ritual progress out of range"); }
                    assert!(g.snag_t >= 0.0, "snag_t went negative");
                    assert!(g.splash_flash >= 0.0, "splash_flash went negative");
                    assert!(g.yowled_t >= 0.0, "yowled_t went negative");
                    pc = g.prey_caught; cc = g.times_caught;
                }
            }
        }
    }

    #[test]
    fn sustenance_recurs_every_few_minutes_across_a_thirty_minute_round() {
        // a Prowl round runs ~30 minutes. With no prey at all, sustenance should hold
        // up for the first minute but turn sick within the first several minutes —
        // it's meant to recur, pushing you back to hunt or rest every so often across
        // the whole round, not be a one-time event or bite immediately.
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        for t in 0..(60 * 30) {
            g.set_input((t as f32 * 0.05).sin(), (t as f32 * 0.04).cos(), false, false, false);
            g.step(1.0 / 30.0);
        }
        assert!(!g.sick, "sustenance shouldn't bite in the first minute of a round");
        for t in 0..(360 * 30) {
            g.set_input((t as f32 * 0.05).sin(), (t as f32 * 0.04).cos(), false, false, false);
            g.step(1.0 / 30.0);
        }
        assert!(g.sick, "with no prey at all, sickness should set in within the first several minutes");
    }

    #[test]
    fn a_dominant_clan_still_takes_most_of_the_round_to_win_outright() {
        // even in the worst case — one clan snowballing totally uncontested while the
        // player does nothing about it — the score-threshold win shouldn't trigger
        // early into what's meant to be a ~30 minute round. It's fine for a total
        // stomp to end a little early (mirrors a real MOBA), just not in the first
        // few minutes.
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0; g.x = 900.0; g.z = 900.0;
        let mut ended_at = None;
        for i in 0..(1800 * 30) {
            g.set_input(0.3, 0.1, false, false, false);
            g.step(1.0 / 30.0);
            if g.outcome.is_some() { ended_at = Some(i as f32 / 30.0); break; }
        }
        let t = ended_at.expect("the round should resolve one way or another within 30 minutes");
        assert!(t > 900.0, "even an unchecked snowball shouldn't decide the round before the 15-minute mark, ended at {t}s");
    }

    #[test]
    fn convergence_fires_several_times_across_a_thirty_minute_round() {
        // a Prowl round runs ~30 minutes; the ritual should get several real chances to
        // matter across it, not need several matches to ever fire.
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        let mut fires = 0;
        let mut last = g.convergence_t;
        for _ in 0..(1800 * 30) {
            g.set_input(0.3, 0.1, false, false, false);
            g.step(1.0 / 30.0);
            if g.convergence_t > last { fires += 1; }
            last = g.convergence_t;
        }
        assert!((5..=9).contains(&fires), "expected roughly one convergence every ~4 min across a ~30 min round, got {fires}");
    }

    #[test]
    fn cat_nine_lives_cheats_the_first_catch_only() {
        let mut g = Prowl::new(Class::Cat, Kind::Dog);
        g.x = 20.0; g.z = 0.0; g.hx = 20.0; g.hz = 0.0;
        g.hstate = HState::Chase; g.suspicion = 1.4;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert_eq!(g.times_caught, 0, "the first catch is cheated by Nine Lives");
        assert!(g.nine_lives_used);
        // a second catch is not cheated
        g.x = 20.0; g.z = 0.0; g.hx = 20.0; g.hz = 0.0;
        g.hstate = HState::Chase; g.suspicion = 1.4;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert_eq!(g.times_caught, 1, "only the first life is free");
    }

    #[test]
    fn squirrel_cache_claims_ancestor_trees_faster() {
        let claim_progress = |cls: Class| -> f32 {
            let mut g = Prowl::new(cls, Kind::Fox);
            g.hx = 900.0; g.hz = 900.0;
            let (ax, az) = (g.ancestors[0].x, g.ancestors[0].z);
            g.x = ax; g.z = az;
            g.set_input(0.0, 0.0, false, false, false);
            g.step(1.0 / 30.0);
            g.ancestors[0].claim
        };
        assert!(claim_progress(Class::Squirrel) > claim_progress(Class::Cat) + 1e-6,
            "Cache should claim ancestor trees faster than a class without it");
    }

    #[test]
    fn otter_slipstream_never_tires_sprinting_in_water() {
        let mut g = Prowl::new(Class::Otter, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        // find a water point to stand in
        let mut wx = 0.0; let mut wz = 0.0; let mut found = false;
        for i in -60..60 {
            let (x, z) = (i as f32 * 3.0, 0.0);
            if world::height_at(x, z) < world::WATER_Y { wx = x; wz = z; found = true; break; }
        }
        assert!(found, "expected some water on the map");
        g.x = wx; g.z = wz; g.stamina = 0.5;
        g.ring = Ring::new(wx, wz, 1e6, 1e6, 1.0, 0.0);   // isolate: no ring-exposure drain
        g.set_input(1.0, 0.0, false, false, false);   // sprint
        let s0 = g.stamina;
        g.step(1.0 / 30.0);
        assert!(g.stamina >= s0 - 1e-4, "Slipstream should prevent stamina drain while sprinting in water");
    }

    #[test]
    fn frog_camouflage_decays_suspicion_faster_when_hidden() {
        let decay = |cls: Class| -> f32 {
            let mut g = Prowl::new(cls, Kind::Fox);
            g.x = 0.0; g.z = 0.0; g.bushes.push((0.0, 0.0, 4.0));
            g.hx = 900.0; g.hz = 900.0;   // hunter far away, doesn't see us
            g.suspicion = 1.0;
            g.set_input(0.0, 0.0, false, false, true);   // crouch/hide
            g.step(1.0 / 30.0);
            1.0 - g.suspicion
        };
        assert!(decay(Class::Frog) > decay(Class::Cat) + 1e-6, "Camouflage should decay suspicion faster while hidden");
    }

    #[test]
    fn goat_sure_footed_builds_less_suspicion_on_mountains() {
        let build = |cls: Class| -> f32 {
            let mut g = Prowl::new(cls, Kind::Fox);
            // find a mountain point and stand there in plain view of a point-blank hunter
            let mut mx = 0.0; let mut mz = 0.0; let mut found = false;
            for i in -60..60 {
                let (x, z) = (i as f32 * 3.0, 0.0);
                if world::height_at(x, z) > 1.9 { mx = x; mz = z; found = true; break; }
            }
            assert!(found, "expected some mountain on the map");
            g.x = mx; g.z = mz; g.hx = mx; g.hz = mz;
            g.hheading = 0.0; g.suspicion = 0.0;
            g.set_input(0.0, 0.0, false, false, false);
            g.step(1.0 / 30.0);
            g.suspicion
        };
        assert!(build(Class::Goat) < build(Class::Cat) - 1e-6, "Sure-Footed should build less suspicion on mountains");
    }

    #[test]
    fn hawk_keen_eyes_always_senses_the_hunter() {
        let mut g = Prowl::new(Class::Hawk, Kind::Fox);
        g.hx = 123.0; g.hz = 456.0;
        assert_eq!(g.keen_eyes_hunter(), Some((123.0, 456.0)));
        let mut c = Prowl::new(Class::Cat, Kind::Fox);
        c.hx = 123.0; c.hz = 456.0;
        assert_eq!(c.keen_eyes_hunter(), None, "only the Hawk has Keen Eyes");
    }

    #[test]
    fn a_flick_attack_staggers_the_hunter_and_rattles_suspicion() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.x = 0.0; g.z = 0.0; g.hx = 1.0; g.hz = 0.0;   // point-blank
        g.suspicion = 1.0;
        g.player_flick((0.0, 1.0), 1.0, false);   // forward flick → Pounce
        assert_eq!(g.player_move.map(|(m, _)| m), Some(crate::combat::Move::Pounce));
        for _ in 0..10 { g.set_input(0.0, 0.0, false, false, false); g.step(1.0 / 30.0); if g.hunter_stagger > 0.0 { break; } }
        assert!(g.hunter_stagger > 0.0, "a connecting pounce should stagger the hunter");
        assert!(g.suspicion < 1.0, "and rattle its suspicion");
    }

    #[test]
    fn a_staggered_hunter_is_frozen() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hunter_stagger = 0.5;
        let (hx0, hz0) = (g.hx, g.hz);
        g.hstate = HState::Chase;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert_eq!((g.hx, g.hz), (hx0, hz0), "a dazed hunter shouldn't move");
    }

    #[test]
    fn moving_downwind_of_the_hunter_builds_suspicion_even_hidden() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.set_fixed_wind(true);
        g.weather.wind_dir = std::f32::consts::FRAC_PI_2;   // (sin,cos) convention: blows toward +x
        g.weather.wind_strength = 1.0; g.weather.gust = 0.0;
        g.x = 0.0; g.z = 0.0;
        g.hx = 20.0; g.hz = 0.0;   // due +x of the player — squarely downwind
        g.hidden = true;
        g.suspicion = 0.5;
        g.set_input(1.0, 0.0, false, false, false);   // moving — leaves a scent
        g.step(1.0 / 30.0);
        assert!(g.suspicion > 0.0, "a strong downwind scent should build suspicion despite being hidden");
    }

    #[test]
    fn moving_upwind_of_the_hunter_only_decays() {
        let decay = |dir: f32| -> f32 {
            let mut g = Prowl::new(Class::Cat, Kind::Fox);
            g.set_fixed_wind(true);
            g.weather.wind_dir = dir;
            g.weather.wind_strength = 1.0; g.weather.gust = 0.0;
            g.x = 0.0; g.z = 0.0;
            g.hx = 20.0; g.hz = 0.0;
            g.hidden = true;
            g.suspicion = 0.5;
            g.set_input(1.0, 0.0, false, false, false);   // moving — leaves a scent
            g.step(1.0 / 30.0);
            g.suspicion
        };
        // wind toward -x (away from the hunter, i.e. blowing from the hunter to the player)
        let upwind = decay(-std::f32::consts::FRAC_PI_2);
        // wind toward +x (toward the hunter)
        let downwind = decay(std::f32::consts::FRAC_PI_2);
        assert!(upwind < downwind, "upwind of the hunter should decay faster than downwind");
    }

    #[test]
    fn fixed_wind_never_veers() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.set_fixed_wind(true);
        let before = g.wind();
        for _ in 0..300 { g.set_input(0.3, 0.1, false, false, false); g.step(1.0 / 30.0); }
        assert_eq!(g.wind(), before, "the wind shouldn't change while fixed");
    }

    #[test]
    fn sneaking_a_hidden_prowler_past_an_unalerted_guard_takes_the_herb() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        let (cx, cz) = (g.jungle[0].x, g.jungle[0].z);
        g.x = cx; g.z = cz;
        g.set_input(0.0, 0.0, false, false, true);   // crouched — quiet and unseen
        assert!(!g.buffs.speed_boosted() && !g.buffs.shield);
        g.step(1.0 / 30.0);
        assert!(!g.jungle[0].alert, "a crouched approach shouldn't alert the guard");
        assert!(g.buffs.speed_boosted() || g.buffs.shield, "reaching the herb unnoticed should grant its buff");
    }

    #[test]
    fn a_loud_approach_alerts_the_guard_and_blocks_the_herb() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        let (cx, cz) = (g.jungle[0].x, g.jungle[0].z);
        g.x = cx + CAMP_ALERT_R - 1.0; g.z = cz;
        g.set_input(1.0, 0.0, false, false, false);   // running in, not hidden
        g.step(1.0 / 30.0);
        assert!(g.jungle[0].alert, "moving in unhidden within range should alert the guard");
        g.x = cx; g.z = cz;   // now at the herb, but the guard is alive and alert
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(!g.buffs.speed_boosted() && !g.buffs.shield, "the herb shouldn't grant while the live guard is alert");
    }

    #[test]
    fn clearing_a_jungle_guard_frees_the_herb_for_good() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        let (cx, cz) = (g.jungle[0].x, g.jungle[0].z);
        g.x = cx; g.z = cz; g.jungle[0].gx = cx; g.jungle[0].gz = cz;
        for _ in 0..20 {
            g.player_flick((0.0, 1.0), 1.0, true);   // underbelly swipe — an attack with no movement of its own
            for _ in 0..12 { g.set_input(0.0, 0.0, false, false, false); g.step(1.0 / 30.0); if g.player_move.is_none() { break; } }
            g.x = cx; g.z = cz; g.leap_t = 0.0; g.dodge_t = 0.0; g.jungle[0].gx = cx; g.jungle[0].gz = cz;
            if g.jungle[0].cleared() { break; }
        }
        assert!(g.jungle[0].cleared(), "enough landed hits should clear the guard");
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.buffs.speed_boosted() || g.buffs.shield, "a cleared camp's herb should be free to take");
    }

    #[test]
    fn a_flick_attack_interrupts_a_rivals_claim() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        let (rx, rz) = (g.rivals[0].x, g.rivals[0].z);
        g.x = rx + 1.0; g.z = rz; g.heading = 0.0;
        g.rivals[0].dwell = 1.0;   // mid-claim
        // forward flick with the low modifier → underbelly swipe (a real attack)
        g.player_flick((0.0, 1.0), 1.0, true);
        for _ in 0..10 { g.set_input(0.0, 0.0, false, false, false); g.step(1.0 / 30.0); if g.rivals[0].stagger > 0.0 { break; } }
        assert!(g.rivals[0].stagger > 0.0, "a connecting hit should daze the rival");
        assert_eq!(g.rivals[0].dwell, 0.0, "and interrupt its claim");
    }

    #[test]
    fn flicks_chain_into_a_named_combo_in_prowl() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.hx = 900.0; g.hz = 900.0;
        g.player_flick((1.0, 0.0), 1.0, false);   // Roll
        while g.player_move.is_some() { g.set_input(0.0, 0.0, false, false, false); g.step(1.0 / 30.0); }
        g.player_flick((0.0, 1.0), 1.0, false);   // Pounce within the combo window
        assert_eq!(g.player_move.map(|(m, _)| m), Some(crate::combat::Move::Pounce));
        assert!(g.combo_flash > 0.0, "Roll into Pounce should flash the Ambush Pounce combo");
    }

    #[test]
    fn ring_exposure_strips_cover() {
        // hunker in a bush with a hunter staring point-blank: inside the ring cover
        // hides you, outside it the exposure lets the hunter see straight through.
        let probe = |x: f32, z: f32| -> (bool, f32) {
            let mut g = Prowl::new(Class::Cat, Kind::Fox);
            g.x = x; g.z = z; g.bushes.push((x, z, 4.0));
            g.hx = x + 3.0; g.hz = z; g.hheading = (g.x - g.hx).atan2(g.z - g.hz);
            g.suspicion = 0.0;
            g.set_input(0.0, 0.0, false, false, true);   // crouch/hide
            g.step(1.0 / 30.0);
            (g.exposed, g.suspicion)
        };
        let (inside_exposed, inside_susp) = probe(0.0, 0.0);
        assert!(!inside_exposed, "the origin sits inside the ring");
        assert!(inside_susp < 1e-3, "cover should conceal you inside the ring");
        let (out_exposed, out_susp) = probe(world::SIZE * 0.5, 0.0);
        assert!(out_exposed, "the far point sits outside the ring");
        assert!(out_susp > 0.0, "outside the ring, cover no longer hides you");
    }
}
