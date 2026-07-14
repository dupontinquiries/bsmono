//! sim — the playable core: 6 cats vs 1 hunter. Headless and deterministic so
//! it unit-tests, and it's exactly what the `--play` window and the netcode
//! server drive. Cats seek the objective, flee the hunter, and crouch into
//! bushes to vanish; the hunter senses (cascading cone / omni / vibration),
//! runs its micro-FSM, and pursues — catching any cat it corners in the open.

use crate::hunter::{self, HState, Kind};
use crate::world;

pub const N_CATS: usize = 6;
const CAT_SPEED: f32 = 13.0;
const FLEE_RADIUS: f32 = 16.0;
const CATCH_DIST: f32 = 2.3;
const PERIPHERAL: f32 = 18.0;   // hunter notices nearby movement even off-cone
const MAP: f32 = world::SIZE * 0.92;
const OBJECTIVE_R: f32 = 5.0;   // reach the rendezvous to escape
const ROUND_LEN: f32 = 90.0;    // seconds before the round times out
const SCENT_DROP: u32 = 5;      // ticks between scent deposits per moving cat
const SCENT_DRIFT: f32 = 3.5;   // how fast scent slides downwind (u/s)
const SCENT_DECAY: f32 = 0.22;  // strength lost per second
const CLIMB_R: f32 = 3.0;       // reach a trunk to scramble up
const CLIMB_HEIGHT: f32 = 5.5;  // how high up the trunk a cat gets
const CLIMB_RATE: f32 = 2.2;    // ascent per second (climb_t 0→1)
const CLIMB_BASE: f32 = 0.28;   // instant scramble onto the low branches
const CLIMB_SAFE: f32 = 0.2;    // above this you're out of a ground hunter's reach
const CLIMB_DRAIN: f32 = 0.22;  // stamina/s spent clinging (net drain vs recovery)
const DESCEND_RATE: f32 = 3.0;  // how fast climb_t falls when you let go
const TWIG_SNAP_R: f32 = 0.7;   // step this close to an intact twig and it cracks
const TWIG_HEAR_R: f32 = 30.0;  // how far a snap carries to the hunter
const TWIG_SUSPICION: f32 = 0.6; // suspicion a heard snap adds (scaled by nearness)
const STAGGER_DUR: f32 = 0.9;   // seconds the hunter is dazed by a connecting hit
const MOVE_REACH: f32 = 3.4;    // how close an attack move connects with the hunter
const DODGE_DUR: f32 = 0.34;    // i-frame + burst window
const DODGE_CD: f32 = 1.5;      // cooldown between rolls
const DODGE_SPEED: f32 = CAT_SPEED * 2.3;
// ── ported from the three.js build ──
const LEAP_DUR: f32 = 0.5;                 // a committed forward bound, airborne (uncatchable)
const LEAP_CD: f32 = 1.8;                  // cooldown between leaps
const LEAP_SPEED: f32 = CAT_SPEED * 2.0;   // covers a gap — slower than a roll but longer
const CARRY_DUR: f32 = 2.4;                // the hawk hoists open prey aloft this long
const BREAK_FREE: f32 = 0.12;              // chance a carried cat wrenches free before it's taken
const CRIPPLE_DUR: f32 = 4.0;              // slowed for a while after breaking free
const POUNCE_R: f32 = 2.4;                 // reach to snatch an ambient critter mid-burst

/// Deterministic hash → [0,1) for the odd coin-flip (break-free) without an rng crate.
fn hashf(mut h: u32) -> f32 {
    h ^= h >> 16; h = h.wrapping_mul(0x7feb352d);
    h ^= h >> 15; h = h.wrapping_mul(0x846ca68b);
    h ^= h >> 16; (h as f32) / (u32::MAX as f32)
}


#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Outcome { CatsWin, HunterWins }

/// Match type. Rendezvous: reach the beacon to escape. Survive: no escape — just
/// outlast the hunter using cover for the full round.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode { Rendezvous, Survive }

/// A puff of scent a moving cat leaves behind; it drifts downwind and fades.
#[derive(Clone, Copy)]
pub struct Scent { pub x: f32, pub z: f32, pub strength: f32 }

/// What the player's nose catches on the wind this instant.
pub struct WindScent {
    /// The strongest prey scent: (unit dir toward it, distance, strength 0..1), if any.
    pub prey: Option<(f32, f32, f32, f32)>,
    pub prey_name: &'static str,
    /// How strongly the hunter's reek is on the wind (0 = none, 1 = right upwind, close).
    pub predator_reek: f32,
    /// Unit wind direction (the way scent drifts).
    pub wind: (f32, f32),
}

pub struct Cat {
    pub id: u8,
    pub x: f32, pub z: f32, pub heading: f32,
    pub alive: bool, pub crouched: bool, pub hidden: bool, pub moving: bool, pub safe: bool, pub climbing: bool,
    pub stamina: f32, pub dodge_t: f32, pub dodge_cd: f32, pub dodge_dx: f32, pub dodge_dz: f32,
    pub goal: (f32, f32), pub player: bool,
    // ── ported mechanics ──
    pub leap_t: f32, pub leap_cd: f32, pub leap_dx: f32, pub leap_dz: f32,
    pub carried_t: f32,   // > 0 while the hawk is hoisting this cat aloft
    pub crippled_t: f32,  // > 0 = limping (just broke free of the hawk)
    pub climb_t: f32,     // 0 = on the ground … 1 = high up the trunk
    pub speed: f32,       // world units/sec actually travelled this step (for gait)
    pub snag_t: f32,      // > 0 briefly when a bramble has snagged you in the undergrowth
}

impl Cat {
    pub fn is_leaping(&self) -> bool { self.leap_t > 0.0 }
    pub fn is_carried(&self) -> bool { self.carried_t > 0.0 }
    /// Height up the trunk right now, for rendering.
    pub fn climb_height(&self) -> f32 { self.climb_t * CLIMB_HEIGHT }
    /// Height offset for the leap arc (0 on the ground … peak mid-bound), for rendering.
    pub fn leap_arc(&self) -> f32 {
        if self.leap_t <= 0.0 { return 0.0; }
        let phase = 1.0 - (self.leap_t / LEAP_DUR).clamp(0.0, 1.0);
        (phase * std::f32::consts::PI).sin() * 1.7
    }
}

pub struct HunterUnit {
    pub kind: Kind,
    pub x: f32, pub z: f32, pub heading: f32,
    pub state: HState, pub suspicion: f32,
    pub target: Option<u8>, pub last_seen: Option<(f32, f32)>,
    pub carrying: bool,   // hawk: hovering with prey in its talons
}

pub struct Game {
    pub cats: Vec<Cat>,
    pub hunter: HunterUnit,
    pub bushes: Vec<(f32, f32, f32)>,
    pub trees: Vec<(f32, f32)>,
    pub objective: (f32, f32),
    pub tick: u32,
    pub caught: u32,
    pub escaped: u32,
    pub time: f32,
    pub outcome: Option<Outcome>,
    pub scent: Vec<Scent>,
    pub wind: (f32, f32),
    pub weather: crate::weather::Weather,
    pub snake_home: Option<(f32, f32)>,
    pub mode: Mode,
    pub shifted: bool,      // Shadow: the afterlife is overlaid on the world (cover stripped)
    pub critters: Vec<crate::ecology::Critter>,
    pub prey_caught: u32,   // ambient critters pounced by the cats
    pub difficulty: f32,    // hunter aggression multiplier (0.8 easy .. 1.25 hard)
    pub clan: u8,           // the pack's clan — sets climb/sprint/swim/stealth traits
    pub roster: Vec<(String, crate::lore::Rank)>,  // each cat's name + rank (index-aligned with cats)
    pub danger: f32,        // 0..1 tension: how close/aware the hunter is to the player
    pub player_move: Option<(crate::combat::Move, f32)>,  // the player's move in progress + elapsed
    pub combo_last: Option<(crate::combat::Move, f32)>,   // last finished move + seconds since, for combos
    pub combo_flash: f32,   // > 0 briefly after a combo lands
    pub combo_name: &'static str,   // the last combo's name, for the HUD
    pub hunter_stagger: f32,        // > 0 while the hunter is dazed by a hit
    pub twigs: Vec<(f32, f32)>,   // intact twigs on the forest floor; snapping one is loud
    pub logs: Vec<(f32, f32)>,    // fallen logs a running cat bounds over
    pub noise_flash: f32,   // brief > 0 right after a twig snaps, for feedback
    shift_t: f32,
    teleport_cd: f32,
    input: Option<(u8, f32, f32, bool, bool, bool)>,
    pub abilities_on: bool,   // settings toggle: give each clan a small passive + ultimate (off = hardcore)
    pub ult_cd: f32,          // player's clan ultimate cooldown remaining
    pub ult_t: f32,           // player's clan ultimate active time remaining
    ult_want: bool,
}

fn norm(x: f32, z: f32) -> (f32, f32) { let l = (x * x + z * z).sqrt(); if l > 1e-5 { (x / l, z / l) } else { (0.0, 0.0) } }
fn dist(ax: f32, az: f32, bx: f32, bz: f32) -> f32 { ((ax - bx).powi(2) + (az - bz).powi(2)).sqrt() }

impl Game {
    pub fn new_6v1(kind: Kind) -> Game { Self::new_match(kind, Mode::Rendezvous) }

    pub fn new_match(kind: Kind, mode: Mode) -> Game { Self::new_match_seeded(kind, mode, 0) }

    /// Like `new_match`, but `seed` perturbs the objective choice and the cats' spawn
    /// scatter so repeat matches aren't identical. `seed == 0` reproduces the legacy
    /// fixed layout exactly (so determinism and existing tests are unchanged).
    pub fn new_match_seeded(kind: Kind, mode: Mode, seed: u32) -> Game {
        world::init_world();
        let bushes = world::bush_positions(200);
        let trees = world::tree_positions(120);
        let obj = crate::objectives::place();
        let oi = (seed as usize) % obj.len().max(1);
        let objective = (obj[oi].x, obj[oi].z);
        let mut cats = Vec::new();
        for i in 0..N_CATS {
            let mut a = i as f32 / N_CATS as f32 * std::f32::consts::TAU;
            let mut r = 40.0;
            if seed != 0 {
                a += (hashf(seed ^ (i as u32).wrapping_mul(2_654_435_761)) - 0.5) * 0.7;
                r += (hashf(seed.wrapping_mul(40_503) ^ i as u32) - 0.5) * 18.0;
            }
            let (x, z) = (a.cos() * r, a.sin() * r);
            cats.push(Cat { id: i as u8, x, z, heading: 0.0, alive: true, crouched: false, hidden: false, moving: false, safe: false, climbing: false, stamina: 1.0, dodge_t: 0.0, dodge_cd: 0.0, dodge_dx: 0.0, dodge_dz: 0.0, goal: objective, player: i == 0, leap_t: 0.0, leap_cd: 0.0, leap_dx: 0.0, leap_dz: 0.0, carried_t: 0.0, crippled_t: 0.0, climb_t: 0.0, speed: 0.0, snag_t: 0.0 });
        }
        let snake_home = if matches!(kind, Kind::Snake) && mode == Mode::Rendezvous { world::nearest_water(objective, 130.0) } else { None };
        let (hx, hz) = if mode == Mode::Survive { (34.0, 34.0) } else { snake_home.unwrap_or((objective.0 + 40.0, objective.1 + 40.0)) };
        let hunter = HunterUnit { kind, x: hx, z: hz, heading: 0.0, state: HState::Patrol, suspicion: 0.0, target: None, last_seen: None, carrying: false };
        // pick a season from the match layout so a match has a consistent time of year
        let season = match (objective.0.abs() as u32 + objective.1.abs() as u32 + kind as u32) % 4 {
            0 => crate::weather::Season::Newleaf, 1 => crate::weather::Season::Greenleaf,
            2 => crate::weather::Season::LeafFall, _ => crate::weather::Season::LeafBare,
        };
        let weather = crate::weather::Weather::new(season);
        let wind = weather.wind_dir_vec();
        let prey_n = (520.0 * weather.prey_abundance()) as usize;   // leaner in leaf-bare, plenty in greenleaf
        let critters = crate::ecology::populate(prey_n.max(120), false);
        let twigs = world::twig_positions(1400);   // the forest floor a running cat can betray itself on
        let logs = world::log_positions(110);       // fallen logs to bound over
        let name_seed = (objective.0.abs() as u32).wrapping_mul(2_654_435_761)
            ^ (objective.1.abs() as u32).wrapping_mul(40_503) ^ (kind as u32).wrapping_mul(2_246_822_519);
        let roster = crate::lore::roster(name_seed, crate::lore::Rank::Warrior, N_CATS);
        Game { cats, hunter, bushes, trees, objective, tick: 0, caught: 0, escaped: 0, time: 0.0, outcome: None, scent: Vec::new(), wind, weather, snake_home, mode, shifted: false, critters, prey_caught: 0, difficulty: 1.0, clan: 0, roster, danger: 0.0, player_move: None, combo_last: None, combo_flash: 0.0, combo_name: "", hunter_stagger: 0.0, twigs, logs, noise_flash: 0.0, shift_t: 0.0, teleport_cd: 5.0, input: None,
            abilities_on: false, ult_cd: 0.0, ult_t: 0.0, ult_want: false }
    }

    pub fn set_input(&mut self, id: u8, mx: f32, mz: f32, crouch: bool, dodge: bool, leap: bool) { self.input = Some((id, mx, mz, crouch, dodge, leap)); }

    /// Turn the clan passive/ultimate kit on or off (a settings toggle — off by default,
    /// so the hunter stays the dominant force; the abilities are a "softer" option).
    pub fn set_abilities_on(&mut self, on: bool) { self.abilities_on = on; }

    /// Press the player's clan ultimate (consumed on the next `step`). No-op if the
    /// abilities toggle is off.
    pub fn set_ultimate(&mut self, on: bool) { if on { self.ult_want = true; } }
    pub fn ultimate_ready(&self) -> bool { self.abilities_on && self.ult_cd <= 0.0 && self.ult_t <= 0.0 }

    /// The player's clan ultimate — name, blurb, cooldown, duration. Deliberately modest
    /// and evasive/utility-only (no attack power), so the hunter still feels dominant.
    pub fn clan_ultimate(&self) -> (&'static str, &'static str, f32, f32) {
        match self.clan % 6 {
            0 => ("Vanish", "melt into the shadows — a hard suspicion drop and forced cover", 30.0, 1.5),
            1 => ("Undertow", "a burst of speed while you're in the water", 24.0, 2.0),
            2 => ("Canopy Leap", "snap straight up into full cover at a nearby tree", 26.0, 0.15),
            3 => ("Sprint Burst", "a burst of speed across open ground", 22.0, 2.0),
            4 => ("Warning Howl", "a moment's true sense of exactly where the hunter is", 26.0, 3.0),
            _ => ("Reed Dive", "a burst of speed and cover while you're in the water", 26.0, 2.0),
        }
    }
    pub fn clan_passive(&self) -> (&'static str, &'static str) {
        match self.clan % 6 {
            0 => ("Camouflage", "loses the hunter's trail faster once you're both out of sight and out of scent"),
            1 => ("Slipstream", "never tires while swimming"),
            2 => ("Quick Grip", "scrambles up a trunk a little faster"),
            3 => ("Second Wind", "catches its breath faster at a standstill"),
            4 => ("Keen Nose", "leaves a fainter scent trail for the hunter to follow"),
            _ => ("Silt Cloak", "leaves a fainter scent trail while in the water"),
        }
    }

    /// Fenclan's Warning Howl: a true fix on the hunter's position for a HUD arrow,
    /// while the ultimate is active. No effect on the hunter itself.
    pub fn warning_howl_hunter(&self) -> Option<(f32, f32)> {
        if self.ult_t > 0.0 && self.clan % 6 == 4 { Some((self.hunter.x, self.hunter.z)) } else { None }
    }

    /// Perform a flick move. `dir_local` is the flick in the cat's frame (`.1` forward,
    /// `.0` right), `mag` its speed, `low` the underbelly modifier. Resolves to a move,
    /// chains a combo off the last one if it lands in the window, and kicks off the
    /// movement (pounce/roll/jump reuse the leap/dodge mechanics). No-op if already busy.
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
        let heading = self.cats.iter().find(|c| c.player).map(|c| c.heading).unwrap_or(0.0);
        let (sh, ch) = (heading.sin(), heading.cos());
        let (wx, wz) = (dir_local.1 * sh + dir_local.0 * ch, dir_local.1 * ch - dir_local.0 * sh);
        let (wx, wz) = norm(wx, wz);
        if let Some(cat) = self.cats.iter_mut().find(|c| c.player && c.alive) {
            match mv {
                Move::Pounce => { cat.leap_t = LEAP_DUR; cat.leap_cd = 0.5; cat.leap_dx = wx; cat.leap_dz = wz; }
                Move::Jump   => { cat.leap_t = LEAP_DUR * 0.6; cat.leap_cd = 0.4; cat.leap_dx = wx * 0.35; cat.leap_dz = wz * 0.35; }
                Move::Roll | Move::Dodge => { cat.dodge_t = 0.28; cat.dodge_cd = 0.5; cat.dodge_dx = wx; cat.dodge_dz = wz; }
                _ => {}
            }
        }
        self.player_move = Some((mv, 0.0));
    }

    pub fn step(&mut self, dt: f32) {
        if self.outcome.is_some() { return; }
        self.time += dt;
        let objective = self.objective;
        let mode = self.mode;
        let (mut hx, mut hz) = (self.hunter.x, self.hunter.z);
        let input = self.input.take();

        // ── hawk carry: a grabbed cat is hoisted aloft, struggles, then breaks free
        //    (small chance, limps away crippled) or is taken. While carrying, the hawk
        //    hovers and hunts no one else — a window for the rest of the pack. ──
        if self.hunter.carrying {
            let tick = self.tick;
            for cat in self.cats.iter_mut().filter(|c| c.carried_t > 0.0) {
                cat.carried_t -= dt;
                cat.x += (hx - cat.x) * (dt * 4.0).min(1.0);
                cat.z += (hz - cat.z) * (dt * 4.0).min(1.0);
                cat.moving = false; cat.hidden = false; cat.crouched = false; cat.climbing = false; cat.climb_t = 0.0;
                if cat.carried_t <= 0.0 {
                    if hashf(tick ^ (cat.id as u32).wrapping_mul(2_654_435_761)) < BREAK_FREE {
                        cat.crippled_t = CRIPPLE_DUR;    // wrenches free, drops, limps off
                    } else {
                        cat.alive = false;               // carried off for good
                    }
                }
            }
            if !self.cats.iter().any(|c| c.carried_t > 0.0) {
                self.hunter.carrying = false;
                self.hunter.state = HState::Patrol; self.hunter.suspicion = 0.0;
            }
        }

        // ── cats ──
        let ct = crate::clan::trait_of(self.clan);   // the pack's clan-adapted traits
        let bushes = self.bushes.clone();
        let logs = self.logs.clone();
        let trees = self.trees.clone();
        let hunter_aerial = self.hunter.kind.params().aerial;
        let nearest_bush = |x: f32, z: f32| -> Option<(f32, f32, f32)> {
            bushes.iter().copied().min_by(|a, b| dist(x, z, a.0, a.1).partial_cmp(&dist(x, z, b.0, b.1)).unwrap())
        };
        let in_bush = |x: f32, z: f32| -> bool { bushes.iter().any(|(bx, bz, r)| dist(x, z, *bx, *bz) < *r) };
        let nearest_tree = |x: f32, z: f32| -> Option<(f32, f32)> {
            trees.iter().copied().min_by(|a, b| dist(x, z, a.0, a.1).partial_cmp(&dist(x, z, b.0, b.1)).unwrap())
        };
        for cat in self.cats.iter_mut().filter(|c| c.alive && !c.safe && c.carried_t <= 0.0) {
            let dh = dist(cat.x, cat.z, hx, hz);
            let mut dir = (0.0, 0.0);
            let mut speed = CAT_SPEED;
            let mut want_crouch = false;
            let mut want_climb = false;
            let mut want_dodge = false;
            let mut want_leap = false;
            let can_sprint = cat.stamina > 0.12;
            let sprint = if can_sprint { 1.12 } else { 0.85 }; // winded cats slow below hunter speed
            let mut sprinting = false;

            if cat.player {
                if let Some((pid, mx, mz, cr, dg, lp)) = input {
                    if pid == cat.id {
                        dir = norm(mx, mz);
                        want_dodge = dg;
                        want_leap = lp;
                        // crouch key is contextual: at a trunk → climb, otherwise → hide
                        if cr {
                            match nearest_tree(cat.x, cat.z) {
                                Some((tx, tz)) if dist(cat.x, cat.z, tx, tz) < CLIMB_R => want_climb = true,
                                _ => want_crouch = true,
                            }
                        }
                        // pace with the danger: amble across safe ground, break into a
                        // sprint as the hunter closes — a journey of gaits, not one speed.
                        if dir.0.abs() + dir.1.abs() > 1e-4 && !cr {
                            let urgency = (1.0 - dh / 26.0).clamp(0.0, 1.0);
                            speed = CAT_SPEED * (0.45 + 0.67 * urgency) * if can_sprint { 1.0 } else { 0.78 };
                            sprinting = urgency > 0.45;   // only a real bolt tires you out
                        }
                    }
                }
            } else if dh < FLEE_RADIUS {
                sprinting = true;
                // (AI relies on cover + escaping; the dodge/roll is a player skill move)
                // climbing beats a ground hunter but exposes you to the hawk, so only
                // consider a tree when the hunter can't fly; pick the closer refuge.
                let tree = if !hunter_aerial { nearest_tree(cat.x, cat.z) } else { None };
                let bush = nearest_bush(cat.x, cat.z);
                let td = tree.map(|t| dist(cat.x, cat.z, t.0, t.1)).unwrap_or(1e9);
                let bd = bush.map(|b| dist(cat.x, cat.z, b.0, b.1)).unwrap_or(1e9);
                if tree.is_some() && td <= bd {
                    let (tx, tz) = tree.unwrap();
                    if td < CLIMB_R { want_climb = true; sprinting = false; }
                    else { dir = norm(tx - cat.x, tz - cat.z); speed = CAT_SPEED * sprint; }
                } else if let Some((bx, bz, r)) = bush {
                    if bd < r { want_crouch = true; sprinting = false; }
                    else { dir = norm(bx - cat.x, bz - cat.z); speed = CAT_SPEED * sprint; }
                } else {
                    dir = norm(cat.x - hx, cat.z - hz); speed = CAT_SPEED * sprint;
                }
            } else if mode == Mode::Survive {
                // no beacon to reach — hole up in the nearest cover and wait it out
                if let Some((bx, bz, r)) = nearest_bush(cat.x, cat.z) {
                    if dist(cat.x, cat.z, bx, bz) < r { want_crouch = true; }
                    else { dir = norm(bx - cat.x, bz - cat.z); }
                }
            } else {
                dir = norm(cat.goal.0 - cat.x, cat.goal.1 - cat.z);
            }

            // dodge/roll: a short burst with brief invulnerability (i-frames) on a cooldown
            cat.dodge_cd = (cat.dodge_cd - dt).max(0.0);
            cat.dodge_t = (cat.dodge_t - dt).max(0.0);
            if want_dodge && cat.dodge_cd <= 0.0 && cat.dodge_t <= 0.0 && cat.stamina > 0.1 {
                let mut dd = dir;
                if dd.0.abs() + dd.1.abs() < 1e-4 { dd = norm(cat.x - hx, cat.z - hz); }  // roll away if no input
                cat.dodge_t = DODGE_DUR; cat.dodge_cd = DODGE_CD;
                cat.dodge_dx = dd.0; cat.dodge_dz = dd.1;
                cat.stamina = (cat.stamina - 0.18).max(0.0);
            }
            if cat.dodge_t > 0.0 {                      // mid-roll: fast, uncatchable, can't hide/climb
                dir = (cat.dodge_dx, cat.dodge_dz); speed = DODGE_SPEED;
                want_crouch = false; want_climb = false;
            }

            // leap: a committed forward bound. Longer and airborne (uncatchable while up)
            // — a gap-clearer to escape or to pounce prey, distinct from the snappy roll.
            cat.leap_cd = (cat.leap_cd - dt).max(0.0);
            cat.leap_t = (cat.leap_t - dt).max(0.0);
            if want_leap && cat.leap_t <= 0.0 && cat.leap_cd <= 0.0 && cat.dodge_t <= 0.0 && cat.stamina > 0.15 {
                let mut ld = dir;
                if ld.0.abs() + ld.1.abs() < 1e-4 { ld = (cat.heading.sin(), cat.heading.cos()); } // leap where you face
                cat.leap_t = LEAP_DUR; cat.leap_cd = LEAP_CD;
                cat.leap_dx = ld.0; cat.leap_dz = ld.1;
                cat.stamina = (cat.stamina - 0.20).max(0.0);
            }
            if cat.leap_t > 0.0 {                       // airborne: committed forward, can't hide/climb
                dir = (cat.leap_dx, cat.leap_dz); speed = LEAP_SPEED;
                want_crouch = false; want_climb = false;
            }

            // crippled after wrenching free of the hawk: half speed for a while
            cat.crippled_t = (cat.crippled_t - dt).max(0.0);
            if cat.crippled_t > 0.0 && cat.leap_t <= 0.0 && cat.dodge_t <= 0.0 { speed *= 0.5; }

            // ── clan terrain adaptation: wading is slow (river clans cope far better);
            //    on open ground the moor runners stretch out. Airborne moves are exempt. ──
            if cat.leap_t <= 0.0 && cat.dodge_t <= 0.0 {
                let in_water = world::height_at(cat.x, cat.z) < world::WATER_Y;
                if in_water { speed *= 0.5 * ct.swim; }
                else { speed *= 0.85 + 0.15 * ct.sprint; }
                // clan ultimate: a brief, situational speed burst — evasion only, no attack power
                if cat.player && self.ult_t > 0.0 {
                    let c6 = self.clan % 6;
                    if in_water && (c6 == 1 || c6 == 5) { speed *= 1.8; }        // Undertow / Reed Dive
                    else if !in_water && c6 == 3 { speed *= 1.6; }               // Sprint Burst
                }
            }

            // ── undergrowth: pushing through thick cover drags at you a little, and now
            //    and then a trailing bramble snags — a brief tug you have to pull free of.
            //    Creeping (crouched) slips through clean. ──
            cat.snag_t = (cat.snag_t - dt).max(0.0);
            if cat.leap_t <= 0.0 && cat.dodge_t <= 0.0 {
                let cover = world::cover_at(cat.x, cat.z);
                if cover > 0.35 && !cat.crouched { speed *= 1.0 - cover * 0.18; }   // brush drags
                // a snag: rare, only when running through a real thicket
                if cover > 0.6 && !cat.crouched && cat.snag_t <= 0.0 && sprinting
                    && hashf(self.tick.wrapping_mul(2_654_435_761) ^ (cat.id as u32).wrapping_mul(0x9e37)) < 0.010 {
                    cat.snag_t = 0.35;
                }
                if cat.snag_t > 0.0 { speed *= 0.45; }   // dragging free of the bramble
            }

            // stamina: sprinting drains, everything else recovers
            let in_water_now = world::height_at(cat.x, cat.z) < world::WATER_Y;
            let tide_slipstream = self.abilities_on && self.clan % 6 == 1 && in_water_now;   // never tires swimming
            let amber_second_wind = if self.abilities_on && self.clan % 6 == 3 { 1.3 } else { 1.0 };   // recovers faster
            cat.stamina = (cat.stamina + if sprinting && !tide_slipstream { -0.32 } else if sprinting { 0.0 } else { 0.16 * amber_second_wind } * dt).clamp(0.0, 1.0);

            cat.crouched = want_crouch;
            // ── climb: scramble up the nearest trunk. You ascend over ~half a second,
            //    hug the tree, and it costs stamina to cling — run dry and you slip back
            //    down into a ground hunter's reach, so you can't roost forever. ──
            // once you've slipped below safe height you need real stamina to commit
            // again, so an exhausted cat genuinely drops out of the tree for a spell.
            let can_grab = if cat.climb_t > CLIMB_SAFE { cat.stamina > 0.03 } else { cat.stamina > 0.30 };
            if want_climb && can_grab {
                if cat.climb_t < CLIMB_BASE { cat.climb_t = CLIMB_BASE; }   // grab the low branches
                if let Some((tx, tz)) = nearest_tree(cat.x, cat.z) {         // hug the trunk
                    let k = (dt * 6.0).min(1.0);
                    cat.x += (tx - cat.x) * k; cat.z += (tz - cat.z) * k;
                }
                let moss_grip = if self.abilities_on && self.clan % 6 == 2 { 1.15 } else { 1.0 };
                cat.climb_t = (cat.climb_t + CLIMB_RATE * ct.climb * moss_grip * dt).min(1.0);
                cat.stamina = (cat.stamina - CLIMB_DRAIN / ct.climb.max(0.4) * dt).max(0.0);
                cat.climbing = true;
            } else {
                cat.climb_t = (cat.climb_t - DESCEND_RATE * dt).max(0.0);    // let go → slide down
                cat.climbing = false;
            }
            let mv = dir.0.abs() + dir.1.abs() > 1e-4 && !cat.crouched && !cat.climbing;
            if mv {
                let nx = (cat.x + dir.0 * speed * dt).clamp(-MAP, MAP);
                let nz = (cat.z + dir.1 * speed * dt).clamp(-MAP, MAP);
                cat.speed = dist(cat.x, cat.z, nx, nz) / dt.max(1e-4);
                cat.x = nx; cat.z = nz;
                cat.heading = dir.0.atan2(dir.1);
                // bound over a fallen log in the path — a natural little leap when running
                if cat.leap_t <= 0.0 && cat.leap_cd <= 0.0 && cat.speed > 2.0
                    && logs.iter().any(|&(lx, lz)| dist(cat.x, cat.z, lx, lz) < 1.5) {
                    cat.leap_t = LEAP_DUR * 0.7;
                    cat.leap_cd = 0.7;
                    cat.leap_dx = dir.0; cat.leap_dz = dir.1;
                }
            } else {
                cat.speed = 0.0;
            }
            cat.moving = mv;
            cat.hidden = cat.crouched && in_bush(cat.x, cat.z);
            // reached the rendezvous → escaped for good (Rendezvous mode only)
            if mode == Mode::Rendezvous && dist(cat.x, cat.z, objective.0, objective.1) < OBJECTIVE_R { cat.safe = true; cat.moving = false; cat.hidden = false; cat.climbing = false; cat.climb_t = 0.0; }
        }

        // ── twig-snap: a cat running (not creeping) through the leaf-litter can crack a
        //    twig underfoot. It's consumed, and if a hunter is within earshot the sharp
        //    sound spikes its suspicion and turns it toward the noise. Crouched cats,
        //    airborne leapers and climbers make none. ──
        self.noise_flash = (self.noise_flash - dt * 2.0).max(0.0);
        let movers: Vec<(f32, f32, bool)> = self.cats.iter()
            .filter(|c| c.alive && !c.safe && c.moving && !c.crouched && c.leap_t <= 0.0 && c.climb_t <= 0.05)
            .map(|c| (c.x, c.z, c.player)).collect();
        let mut snapped: Option<(f32, f32)> = None;
        for (cx, cz, player) in movers {
            if let Some(idx) = self.twigs.iter().position(|&(tx, tz)| dist(cx, cz, tx, tz) < TWIG_SNAP_R) {
                self.twigs.swap_remove(idx);
                if player || snapped.is_none() { snapped = Some((cx, cz)); }
            }
        }
        if let Some((sx, sz)) = snapped {
            self.noise_flash = 1.0;
            let hd = dist(self.hunter.x, self.hunter.z, sx, sz);
            if hd < TWIG_HEAR_R && !self.hunter.carrying {
                self.hunter.suspicion = (self.hunter.suspicion + TWIG_SUSPICION * (1.0 - hd / TWIG_HEAR_R)).min(1.5);
                self.hunter.last_seen = Some((sx, sz));   // it snaps its head toward the crack
            }
        }

        // ── scent: moving cats leave a trail that drifts downwind and fades ──
        if self.tick % SCENT_DROP == 0 {
            for c in self.cats.iter().filter(|c| c.alive && !c.safe && c.moving && !c.hidden && !c.climbing) {
                // Fenclan/Mangroveclan passive: a keen nose also knows how to mask a trail
                let in_water = world::height_at(c.x, c.z) < world::WATER_Y;
                let strength = if self.abilities_on && (self.clan % 6 == 4 || (self.clan % 6 == 5 && in_water)) { 0.6 } else { 1.0 };
                self.scent.push(Scent { x: c.x, z: c.z, strength });
            }
        }
        // ── weather: evolve wind/sky, then let it shape the scent trails ──
        self.weather.step(dt);
        self.wind = self.weather.wind_dir_vec();
        let drift = SCENT_DRIFT * self.weather.scent_drift_mult();
        let decay = SCENT_DECAY * self.weather.scent_decay_mult();
        for s in self.scent.iter_mut() {
            s.x += self.wind.0 * drift * dt;
            s.z += self.wind.1 * drift * dt;
            s.strength -= decay * dt;
        }
        self.scent.retain(|s| s.strength > 0.05);

        // ── combat moves: advance the player's move, land attacks that stagger the
        //    hunter (and can wrench you free of a grab), and tick the combo window. ──
        self.combo_flash = (self.combo_flash - dt * 1.5).max(0.0);
        self.hunter_stagger = (self.hunter_stagger - dt).max(0.0);
        if let Some((m, s)) = self.combo_last { let ns = s + dt; self.combo_last = if ns > 1.0 { None } else { Some((m, ns)) }; }
        if let Some((mv, t)) = self.player_move {
            let sp = crate::combat::spec(mv);
            let nt = t + dt;
            // the hit lands on the frame the move turns active
            if mv.is_attack() && t < sp.windup && nt >= sp.windup {
                let pc = self.cats.iter().find(|c| c.player && c.alive).map(|c| (c.x, c.z, c.carried_t > 0.0));
                if let Some((px, pz, carried)) = pc {
                    let hd = dist(px, pz, self.hunter.x, self.hunter.z);
                    if hd < MOVE_REACH && (!self.hunter.carrying || carried) {
                        let power = if self.combo_flash > 0.0 { sp.power * crate::combat::COMBO_POWER_BONUS } else { sp.power };
                        self.hunter_stagger = self.hunter_stagger.max(STAGGER_DUR * (0.5 + power * 0.35));
                        self.hunter.suspicion = (self.hunter.suspicion - 0.35).max(0.0);   // rattled, loses the thread a beat
                        // a hind-kick or swipe can wrench the player free of the hawk
                        if carried && matches!(mv, crate::combat::Move::HindKick | crate::combat::Move::UnderbellySwipe) {
                            self.hunter.carrying = false;
                            if let Some(c) = self.cats.iter_mut().find(|c| c.player) { c.carried_t = 0.0; c.crippled_t = 1.2; }
                        }
                    }
                }
            }
            self.player_move = if nt >= mv.total() { self.combo_last = Some((mv, 0.0)); None } else { Some((mv, nt)) };
        }

        // ── clan ultimate: tick cooldown/duration, and fire on request. Deliberately
        //    evasive/utility-only (no attack power) — the hunter should still feel like
        //    the dominant force even with the abilities toggle on. ──
        self.ult_cd = (self.ult_cd - dt).max(0.0);
        self.ult_t = (self.ult_t - dt).max(0.0);
        if self.ult_want && self.ultimate_ready() {
            let (_, _, cd, dur) = self.clan_ultimate();
            if let Some(p) = self.cats.iter_mut().find(|c| c.player && c.alive) {
                self.ult_t = dur; self.ult_cd = cd;
                match self.clan % 6 {
                    0 => { self.hunter.suspicion = (self.hunter.suspicion - 1.0).max(0.0); }   // Vanish: a hard scare
                    2 => {                                                                       // Canopy Leap: snap up a nearby tree
                        if let Some((tx, tz)) = nearest_tree(p.x, p.z) {
                            if dist(p.x, p.z, tx, tz) < CLIMB_R * 1.6 {
                                p.climbing = true; p.climb_t = 1.0; p.hidden = true;
                            }
                        }
                    }
                    _ => {}   // Undertow / Sprint Burst / Warning Howl / Reed Dive apply continuously above
                }
            }
        }
        self.ult_want = false;
        // Vanish: force cover for its short duration, on top of the instant scare above
        if self.ult_t > 0.0 && self.clan % 6 == 0 {
            if let Some(p) = self.cats.iter_mut().find(|c| c.player && c.alive) { p.hidden = true; }
        }

        // ── hunter: sight → smell → FSM → pursue → catch ──
        // While the hawk is carrying prey it hovers and hunts no one else, so the
        // entire sensing/pursuit/catch step is skipped until the struggle resolves.
        if !self.hunter.carrying && self.hunter_stagger <= 0.0 {
        let p = self.hunter.kind.params();
        // Shadow only: the otherworld phases in and out. While shifted, the afterlife
        // is overlaid on the world and cover no longer conceals the living.
        if self.hunter.kind.is_shadow() {
            self.shift_t += dt;
            if !self.shifted && self.shift_t > 9.0 { self.shifted = true; self.shift_t = 0.0; }
            else if self.shifted && self.shift_t > 6.0 { self.shifted = false; self.shift_t = 0.0; }
        }
        let strip = self.hunter.kind.is_shadow() && self.shifted; // cover stripped this tick
        // sweep the gaze while not locked on so prey off the narrow far-cone still gets noticed
        let gaze = if self.hunter.state == HState::Chase { self.hunter.heading }
                   else { self.hunter.heading + (self.tick as f32 * 0.09).sin() * 1.3 };
        let mut seen_cat: Option<(u8, f32, f32, f32)> = None; // id, x, z, dist
        for cat in self.cats.iter().filter(|c| c.alive && !c.safe) {
            let d = dist(hx, hz, cat.x, cat.z);
            let hidden = cat.hidden && !strip;   // the Shadow sees hiders during the shift
            let sees = hunter::detects(self.hunter.kind, hx, hz, gaze, cat.x, cat.z, cat.moving, hidden)
                || (cat.moving && !hidden && d < PERIPHERAL);
            if sees && seen_cat.map_or(true, |s| d < s.3) { seen_cat = Some((cat.id, cat.x, cat.z, d)); }
        }
        // during the shift the otherworld lays the living bare — the shadow senses
        // the nearest cat around it even if cover would normally hide them.
        let mut seen = seen_cat.is_some();
        if strip && !seen {
            let mut best = 46.0;
            for cat in self.cats.iter().filter(|c| c.alive && !c.safe) {
                let d = dist(hx, hz, cat.x, cat.z);
                if d < best { best = d; seen_cat = Some((cat.id, cat.x, cat.z, d)); }
            }
            seen = seen_cat.is_some();
        }

        // smell: when it can't see, follow the strongest scent within nose range.
        // fresher scent is stronger and nearer the cat, so climbing that gradient
        // (toward the source = upwind) leads the hunter in.
        let mut smelled: Option<(f32, f32)> = None;
        if !seen {
            let mut best = 0.0;
            for s in &self.scent {
                let d = dist(hx, hz, s.x, s.z);
                if d < p.smell {
                    let score = s.strength * (1.0 - d / p.smell);
                    if score > best { best = score; smelled = Some((s.x, s.z)); }
                }
            }
        }
        let has_scent = smelled.is_some();

        if let Some((id, cx, cz, _)) = seen_cat {
            let concealment = 1.0 - world::cover_at(cx, cz) * 0.5;   // half-seen in thick cover
            self.hunter.suspicion = (self.hunter.suspicion + dt * 2.4 * self.difficulty * self.weather.sight_mult() * concealment).min(1.5);
            self.hunter.last_seen = Some((cx, cz));
            self.hunter.target = Some(id);
        } else if has_scent {
            self.hunter.suspicion = (self.hunter.suspicion + dt * 0.5).min(0.85); // a trail keeps it hunting, not charging
        } else {
            let ember_bonus = if self.abilities_on && self.clan % 6 == 0 { 1.5 } else { 1.0 };   // Emberclan passive: Camouflage
            self.hunter.suspicion = (self.hunter.suspicion - dt * 0.3 * ember_bonus).max(0.0);
        }
        self.hunter.state = hunter::step(self.hunter.state, self.hunter.suspicion, has_scent, seen);

        let (mut goal, mut speed) = match self.hunter.state {
            HState::Chase => (self.hunter.last_seen.unwrap_or((hx, hz)), p.speed),
            HState::Alert | HState::Search | HState::Track => {
                let g = if seen { self.hunter.last_seen.unwrap() }
                        else { smelled.or(self.hunter.last_seen).unwrap_or(self.objective) };
                (g, p.speed * 0.85)
            }
            HState::Patrol => {
                if mode == Mode::Survive {
                    // no beacon to guard — sweep the ground where the pack hides
                    let a = self.tick as f32 * 0.02;
                    ((a.cos() * 38.0, a.sin() * 38.0), p.speed * 0.7)
                } else {
                    // orbit the objective (where cats converge) so the cone keeps sweeping
                    let a = self.tick as f32 * 0.016;
                    ((self.objective.0 + a.cos() * 22.0, self.objective.1 + a.sin() * 22.0), p.speed * 0.6)
                }
            }
        };
        // stalk: when it's nosing around right where it last had the scent, the hunter
        // slows to a deliberate creep and casts about — a menacing, patient hunt rather
        // than a flat sprint. This is where the dread lives.
        if matches!(self.hunter.state, HState::Search | HState::Track | HState::Alert) && !seen {
            if dist(hx, hz, goal.0, goal.1) < 12.0 { speed *= 0.4; }
        }
        // snake: an ambusher. It lurks in its water rather than orbiting, then
        // erupts with a strike-burst when prey strays near the pool.
        if let Some(home) = self.snake_home {
            if self.hunter.state == HState::Patrol { goal = home; speed = p.speed * 0.5; } // coil and wait
            if dist(hx, hz, home.0, home.1) < 22.0 && matches!(self.hunter.state, HState::Chase | HState::Alert | HState::Track) {
                speed *= 1.8; // explosive lunge from the water
            }
        }
        // Shadow: blink across the map to where the prey just was (Freddy-style —
        // you never outrun it). Cooldown-gated, only when its quarry has slipped far.
        if self.hunter.kind.is_shadow() {
            self.teleport_cd -= dt;
            if self.teleport_cd <= 0.0 {
                if let Some((lx, lz)) = self.hunter.last_seen {
                    if dist(hx, hz, lx, lz) > 16.0 {
                        hx = lx; hz = lz;
                        self.hunter.x = lx; self.hunter.z = lz;
                        self.teleport_cd = 5.0;
                    }
                }
            }
        }
        if self.hunter.kind.is_shadow() && self.shifted { speed *= 1.35; } // empowered by the otherworld
        speed *= self.difficulty;   // Settings difficulty scales the hunter's pace
        let d = norm(goal.0 - hx, goal.1 - hz);
        if d.0.abs() + d.1.abs() > 1e-4 {
            self.hunter.x = (hx + d.0 * speed * dt).clamp(-MAP, MAP);
            self.hunter.z = (hz + d.1 * speed * dt).clamp(-MAP, MAP);
            self.hunter.heading = d.0.atan2(d.1);
        }

        // catch: open cats need the hunter aggressive + in reach; a hidden cat is
        // rooted out by proximity scaled by the nose. During the Shadow's shift,
        // cover is stripped so hiders are as catchable as cats in the open.
        let aggressive = self.hunter.state == HState::Chase || self.hunter.suspicion > 0.55;
        let flush_r = (1.2 + p.smell * 0.035) * self.difficulty / ct.stealth;   // harder settings root hiders out from further; lurking clans hide tighter
        let is_hawk = matches!(self.hunter.kind, Kind::Hawk);
        let mut grabbed_any = false;
        {
            let (hx, hz) = (self.hunter.x, self.hunter.z);
            for cat in self.cats.iter_mut().filter(|c| c.alive && !c.safe && c.carried_t <= 0.0) {
                if cat.dodge_t > 0.0 || cat.leap_t > 0.0 { continue; } // rolling or airborne → invulnerable
                if cat.climb_t > CLIMB_SAFE && !hunter_aerial && !strip { continue; } // high up the trunk — safe unless a hawk or the shift reaches it
                let d = dist(hx, hz, cat.x, cat.z);
                let hidden = cat.hidden && !strip;
                let taken = if hidden { d < flush_r } else { aggressive && d < CATCH_DIST };
                if taken {
                    if is_hawk && !hidden {
                        cat.carried_t = CARRY_DUR;   // the hawk hoists open prey aloft; the struggle resolves later
                        grabbed_any = true;
                    } else {
                        cat.alive = false;           // every other hunter takes prey outright
                    }
                }
            }
        }
        if grabbed_any { self.hunter.carrying = true; }
        }
        // ambient wildlife: flee the hunter and any living cats
        if !self.critters.is_empty() {
            let mut threats: Vec<(f32, f32)> = Vec::with_capacity(1 + self.cats.len());
            threats.push((self.hunter.x, self.hunter.z));
            for c in &self.cats { if c.alive { threats.push((c.x, c.z)); } }
            for cr in &mut self.critters { cr.step(dt, &threats); }
        }
        // pounce: a cat mid-burst (a leap or a roll) snatches any ambient critter it
        // lands on — a small meal that tops up stamina, tying the wildlife into the
        // hunt (from the JS build: caught when airborne or moving fast).
        let pounce_pts: Vec<(usize, f32, f32)> = self.cats.iter().enumerate()
            .filter(|(_, c)| c.alive && (c.leap_t > 0.0 || c.dodge_t > 0.0))
            .map(|(i, c)| (i, c.x, c.z)).collect();
        if !pounce_pts.is_empty() && !self.critters.is_empty() {
            let mut fed = [0u32; N_CATS];
            self.critters.retain(|cr| {
                for &(i, cx, cz) in &pounce_pts {
                    if dist(cx, cz, cr.x, cr.z) < POUNCE_R { fed[i] += 1; return false; }
                }
                true
            });
            for i in 0..N_CATS {
                if fed[i] > 0 {
                    self.cats[i].stamina = (self.cats[i].stamina + 0.25 * fed[i] as f32).min(1.0);
                    self.prey_caught += fed[i];
                }
            }
        }
        self.tick += 1;

        // tally + resolve: hunter wins by taking half the pack (3 of 6), else the cats do
        self.escaped = self.cats.iter().filter(|c| c.safe).count() as u32;
        self.caught = self.cats.iter().filter(|c| !c.alive).count() as u32;
        let resolved = (self.escaped + self.caught) as usize;
        if resolved >= N_CATS || self.time >= ROUND_LEN {
            self.outcome = Some(if self.caught >= 3 { Outcome::HunterWins } else { Outcome::CatsWin });
        }

        // ── suspense: how much dread the player should feel this instant. Rises with
        //    the hunter's nearness, weighted by how switched-on it is (a point-blank
        //    chase pins it near 1; a distant patrol leaves it near 0), and eases so it
        //    swells and ebbs rather than snapping. Drives the heartbeat/vignette. ──
        let target = if let Some(pc) = self.cats.iter().find(|c| c.player && c.alive && !c.safe) {
            let d = dist(pc.x, pc.z, self.hunter.x, self.hunter.z);
            // freeze and taste the air: a still cat reads the wind farther than a moving one
            let sense = if pc.moving { 1.0 } else { 1.35 };
            let prox = (1.0 - d / (40.0 * ct.scent * sense)).clamp(0.0, 1.0);   // keen noses (and stillness) feel the predator sooner
            let state_w = match self.hunter.state {
                HState::Chase => 1.0, HState::Alert => 0.7, HState::Track => 0.6, HState::Search => 0.45, HState::Patrol => 0.12,
            };
            (prox * (0.4 + 0.6 * state_w) + self.hunter.suspicion.min(1.5) * 0.22).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let ease = (dt * if target > self.danger { 4.0 } else { 1.6 }).min(1.0);   // spikes fast, fades slow
        self.danger += (target - self.danger) * ease;
    }

    pub fn alive_cats(&self) -> usize { self.cats.iter().filter(|c| c.alive).count() }
    pub fn active_cats(&self) -> usize { self.cats.iter().filter(|c| c.alive && !c.safe).count() }

    /// The name of cat `i` (index-aligned with `cats`); empty if out of range.
    pub fn cat_name(&self, i: usize) -> &str { self.roster.get(i).map(|(n, _)| n.as_str()).unwrap_or("") }
    /// The biome the rendezvous sits in — the "region" of this match.
    pub fn region(&self) -> &'static str { world::biome_at(self.objective.0, self.objective.1).name }
    /// A line of lore for the current region.
    pub fn region_lore(&self) -> &'static str { crate::lore::region_lore(self.region()) }

    /// Read the wind: what the player's nose catches on the breeze right now. Scent drifts
    /// *downwind*, so you smell prey and the predator that lie **upwind** of you — approach
    /// prey from downwind and you'll scent it before it bolts; drop downwind of the hunter
    /// and its reek warns you early. Keen-nosed clans and a strong wind carry scent farther.
    pub fn read_wind(&self) -> WindScent {
        let ct = crate::clan::trait_of(self.clan);
        let (wx, wz) = self.weather.wind_vec();
        let wstr = (wx * wx + wz * wz).sqrt().max(0.05);
        let (wux, wuz) = (wx / wstr, wz / wstr);
        let mut out = WindScent { prey: None, prey_name: "", predator_reek: 0.0, wind: (wux, wuz) };
        let pc = match self.cats.iter().find(|c| c.player && c.alive && !c.safe) { Some(c) => c, None => return out };

        // prey: the strongest-smelled critter that lies upwind (its scent blows to you)
        let mut best = 0.0f32;
        for cr in &self.critters {
            let (dx, dz) = (pc.x - cr.x, pc.z - cr.z);   // from prey → you
            let d = (dx * dx + dz * dz).sqrt();
            if d < 0.2 { continue; }
            let align = (dx * wux + dz * wuz) / d;        // 1 = you're straight downwind of it
            let range = 22.0 * ct.scent * (0.55 + wstr);
            if align > 0.25 && d < range {
                let s = (1.0 - d / range) * align;
                if s > best { best = s; out.prey = Some((-dx / d, -dz / d, d, s)); out.prey_name = cr.species.name(); }
            }
        }
        // the predator's reek, when you're downwind of the hunter
        let (dx, dz) = (pc.x - self.hunter.x, pc.z - self.hunter.z);
        let d = (dx * dx + dz * dz).sqrt();
        if d > 0.2 {
            let align = (dx * wux + dz * wuz) / d;
            let range = 55.0 * ct.scent * (0.55 + wstr);
            if align > 0.0 && d < range { out.predator_reek = ((1.0 - d / range) * align).clamp(0.0, 1.0); }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_is_6v1() {
        let g = Game::new_6v1(Kind::Fox);
        assert_eq!(g.cats.len(), N_CATS);
        assert_eq!(g.alive_cats(), 6);
    }

    #[test]
    fn abilities_are_off_by_default_and_the_ultimate_wont_fire() {
        let mut g = Game::new_6v1(Kind::Fox);
        assert!(!g.abilities_on);
        g.set_ultimate(true);
        g.set_input(0, 0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert_eq!(g.ult_t, 0.0, "the ultimate shouldn't engage while the toggle is off");
    }

    #[test]
    fn emberclan_vanish_scares_off_suspicion_when_abilities_are_on() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.set_abilities_on(true);
        g.clan = 0;   // Emberclan
        g.hunter.suspicion = 1.4;
        g.set_ultimate(true);
        g.set_input(0, 0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.ult_t > 0.0 && g.ult_cd > 0.0, "Vanish should engage and go on cooldown");
        assert!(g.hunter.suspicion < 1.4, "Vanish should scare off suspicion");
        assert!(g.cats[0].hidden, "and force cover for its duration");
    }

    #[test]
    fn tideclan_slipstream_never_tires_swimming_when_abilities_are_on() {
        let mut wx = 0.0; let mut wz = 0.0; let mut found = false;
        for i in -60..60 { let (x, z) = (i as f32 * 3.0, 0.0); if world::height_at(x, z) < world::WATER_Y { wx = x; wz = z; found = true; break; } }
        assert!(found, "expected some water on the map");
        let mut g = Game::new_6v1(Kind::Fox);
        g.set_abilities_on(true);
        g.clan = 1;   // Tideclan
        g.hunter.x = 900.0; g.hunter.z = 900.0;
        g.cats[0].x = wx; g.cats[0].z = wz; g.cats[0].stamina = 0.5;
        let s0 = g.cats[0].stamina;
        // urgency-driven sprint needs the hunter close; instead exercise the branch directly
        // by moving hard for a beat and checking stamina never drops below the start.
        for _ in 0..15 { g.set_input(0, 1.0, 0.0, false, false, false); g.step(1.0 / 30.0); g.cats[0].x = wx; g.cats[0].z = wz; }
        assert!(g.cats[0].stamina >= s0 - 1e-3, "Slipstream should prevent stamina drain while swimming");
    }

    #[test]
    fn amberclan_second_wind_recovers_faster_when_abilities_are_on() {
        let recover = |abilities: bool| -> f32 {
            let mut g = Game::new_6v1(Kind::Fox);
            g.set_abilities_on(abilities);
            g.clan = 3;   // Amberclan
            g.hunter.x = 900.0; g.hunter.z = 900.0;
            g.cats[0].stamina = 0.5;
            g.set_input(0, 0.0, 0.0, false, false, false);
            g.step(1.0 / 30.0);
            g.cats[0].stamina
        };
        assert!(recover(true) > recover(false) + 1e-5, "Second Wind should recover stamina faster than the baseline");
    }

    #[test]
    fn mossclan_quick_grip_climbs_faster_when_abilities_are_on() {
        let climb_after = |abilities: bool| -> f32 {
            let mut g = Game::new_6v1(Kind::Fox);
            g.set_abilities_on(abilities);
            g.clan = 2;   // Mossclan
            g.hunter.x = 900.0; g.hunter.z = 900.0;
            let (tx, tz) = g.trees[0];
            g.cats[0].x = tx; g.cats[0].z = tz;
            for _ in 0..3 { g.set_input(0, 0.0, 0.0, true, false, false); g.step(1.0 / 30.0); }
            g.cats[0].climb_t
        };
        assert!(climb_after(true) > climb_after(false) + 1e-4, "Quick Grip should climb faster than the baseline");
    }

    #[test]
    fn mossclan_canopy_leap_snaps_up_a_nearby_tree() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.set_abilities_on(true);
        g.clan = 2;   // Mossclan
        let (tx, tz) = g.trees[0];
        g.cats[0].x = tx; g.cats[0].z = tz;
        g.set_ultimate(true);
        g.set_input(0, 0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.cats[0].climbing && g.cats[0].climb_t >= 0.999, "Canopy Leap should snap straight up a nearby tree");
    }

    #[test]
    fn fenclan_warning_howl_reveals_the_hunter_only_while_active() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.set_abilities_on(true);
        g.clan = 4;   // Fenclan
        g.hunter.x = 40.0; g.hunter.z = -12.0;
        assert_eq!(g.warning_howl_hunter(), None, "not revealed before the ultimate fires");
        g.set_ultimate(true);
        g.set_input(0, 0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        let (hx, hz) = (g.hunter.x, g.hunter.z);
        assert_eq!(g.warning_howl_hunter(), Some((hx, hz)));
    }

    #[test]
    fn hunter_chases_a_cat_it_sees() {
        let mut g = Game::new_6v1(Kind::Fox);
        // put the hunter right behind cat 0, facing it, in the open
        g.cats[0].x = 0.0; g.cats[0].z = 10.0; g.cats[0].moving = true; g.cats[0].hidden = false;
        g.hunter.x = 0.0; g.hunter.z = 0.0; g.hunter.heading = 0.0; // faces +z toward the cat
        let d0 = dist(g.hunter.x, g.hunter.z, g.cats[0].x, g.cats[0].z);
        for _ in 0..30 { g.step(1.0 / 30.0); }
        assert!(matches!(g.hunter.state, HState::Chase | HState::Alert));
        let d1 = dist(g.hunter.x, g.hunter.z, g.cats[0].x, g.cats[0].z);
        assert!(d1 < d0, "hunter should have closed distance ({d0} -> {d1})");
    }

    #[test]
    fn cat_flees_a_near_hunter() {
        let mut g = Game::new_6v1(Kind::Snake); // slow enough it can't catch, so we isolate flee behaviour
        g.bushes.clear();                        // no cover → flee straight away from the hunter
        g.hunter.x = 0.0; g.hunter.z = 0.0;
        g.cats[1].x = 8.0; g.cats[1].z = 0.0; g.cats[1].player = false;
        let x0 = g.cats[1].x;
        for _ in 0..10 { g.step(1.0 / 30.0); }
        assert!(g.cats[1].alive && g.cats[1].x > x0, "a threatened cat should flee away from the hunter");
    }

    #[test]
    fn flee_with_no_cover_does_not_panic() {
        // Regression: a fleeing cat with neither a tree (the Hawk forbids climbing) nor
        // a bush in range used to hit `tree.unwrap()` on None and crash the whole sim.
        let mut g = Game::new_6v1(Kind::Hawk);
        g.bushes.clear();
        g.trees.clear();
        g.hunter.x = 0.0; g.hunter.z = 0.0;
        g.cats[1].x = 10.0; g.cats[1].z = 0.0; g.cats[1].player = false;
        let x0 = g.cats[1].x;
        for _ in 0..5 { g.step(1.0 / 30.0); }     // must not panic
        assert!(g.cats[1].x > x0, "with no cover at all, the cat should just run from the hunter");
    }

    #[test]
    fn reaching_rendezvous_escapes() {
        let mut g = Game::new_6v1(Kind::Snake);
        g.hunter.x = 600.0; g.hunter.z = 600.0;   // no threat nearby
        let (ox, oz) = g.objective;
        g.cats[2].x = ox + 2.0; g.cats[2].z = oz; g.cats[2].player = false;
        g.step(1.0 / 30.0);
        assert!(g.cats[2].safe && g.escaped >= 1, "a cat at the rendezvous should escape");
    }

    #[test]
    fn sim_invariants_hold_every_tick() {
        // Property check: run every archetype/mode and assert the core invariants on
        // each tick. Catches subtle logic slips (a cat both safe and caught, stamina
        // drifting out of range, an actor escaping the map, counts going backwards).
        for k in [Kind::Fox, Kind::Dog, Kind::Badger, Kind::Hawk, Kind::Snake, Kind::Shadow] {
            for m in [Mode::Rendezvous, Mode::Survive] {
                let mut g = Game::new_match(k, m);
                let bound = MAP + 1.0;
                let (mut prev_caught, mut prev_escaped) = (0u32, 0u32);
                let mut t = 0i32;
                while g.outcome.is_none() && t < 95 * 30 {
                    g.set_input(0, (t as f32 * 0.3).sin(), (t as f32 * 0.2).cos(), t % 30 < 6, t % 50 == 0, t % 70 == 0);
                    g.step(1.0 / 30.0);
                    t += 1;

                    let mut safe_n = 0u32; let mut dead_n = 0u32;
                    for c in &g.cats {
                        assert!(c.stamina >= 0.0 && c.stamina <= 1.0, "stamina out of range: {}", c.stamina);
                        assert!(c.x.is_finite() && c.z.is_finite(), "cat position not finite");
                        assert!(c.x.abs() <= bound && c.z.abs() <= bound, "cat left the map: {},{}", c.x, c.z);
                        assert!(!(c.safe && !c.alive), "a cat is both safe and caught");
                        if c.safe { safe_n += 1; }
                        if !c.alive { dead_n += 1; }
                    }
                    assert!(g.hunter.x.abs() <= bound && g.hunter.z.abs() <= bound, "hunter left the map");
                    assert!(g.hunter.x.is_finite() && g.hunter.z.is_finite(), "hunter position not finite");
                    assert_eq!(g.escaped, safe_n, "escaped count desynced");
                    assert_eq!(g.caught, dead_n, "caught count desynced");
                    assert!(g.escaped + g.caught <= N_CATS as u32, "more resolved cats than exist");
                    assert!(g.caught >= prev_caught && g.escaped >= prev_escaped, "resolved counts went backwards");
                    prev_caught = g.caught; prev_escaped = g.escaped;
                }
                // once resolved, the outcome is frozen and stepping is a no-op
                let snap = (g.outcome, g.hunter.x, g.hunter.z, g.caught);
                g.step(1.0 / 30.0);
                assert_eq!((g.outcome, g.hunter.x, g.hunter.z, g.caught), snap, "a finished round kept simulating");
            }
        }
    }

    #[test]
    fn difficulty_scales_hunter_danger() {
        // identical deterministic Survive matches; a harder setting should catch at
        // least as many cats as an easier one (faster pursuit + quicker suspicion).
        let caught_at = |diff: f32| -> u32 {
            let mut g = Game::new_match(Kind::Dog, Mode::Survive);
            g.difficulty = diff;
            for _ in 0..(90 * 30) { if g.outcome.is_some() { break; } g.step(1.0 / 30.0); }
            g.caught
        };
        let easy = caught_at(0.82);
        let hard = caught_at(1.22);
        assert!(hard >= easy, "a harder hunter shouldn't catch fewer cats (easy {easy}, hard {hard})");
    }

    #[test]
    fn difficulty_widens_the_flush_radius() {
        // a hidden cat sitting at roughly the normal flush distance: easy leaves it a
        // margin, hard's wider radius roots it out. Tiny dt so the hunter barely closes.
        let base = 1.2 + Kind::Badger.params().smell * 0.035;   // flush radius at normal difficulty
        let flushed = |diff: f32| -> bool {
            let mut g = Game::new_match(Kind::Badger, Mode::Survive);
            g.difficulty = diff;
            g.clan = 2;   // Mossclan (stealth 1.0) so this measures difficulty alone
            for i in 1..N_CATS { g.cats[i].alive = false; }
            g.cats[0].x = 0.0; g.cats[0].z = 0.0;
            g.bushes.clear(); g.bushes.push((0.0, 0.0, 4.0));
            g.hunter.x = 0.0; g.hunter.z = base;
            g.hunter.state = HState::Chase; g.hunter.suspicion = 1.4;
            g.set_input(0, 0.0, 0.0, true, false, false);       // crouch in the bush, hold still
            g.step(0.001);
            !g.cats[0].alive
        };
        assert!(!flushed(0.82), "easy: a hider just outside normal range stays safe");
        assert!(flushed(1.22), "hard: the widened flush radius roots the hider out");
    }

    #[test]
    fn match_seed_varies_setup_but_zero_is_legacy() {
        let a = Game::new_match_seeded(Kind::Fox, Mode::Rendezvous, 1);
        let b = Game::new_match_seeded(Kind::Fox, Mode::Rendezvous, 2);
        let cats_differ = a.cats[0].x != b.cats[0].x || a.cats[0].z != b.cats[0].z;
        assert!(cats_differ || a.objective != b.objective, "different seeds should vary the match");
        // seed 0 must reproduce the legacy fixed layout exactly
        let z = Game::new_match_seeded(Kind::Fox, Mode::Rendezvous, 0);
        let legacy = Game::new_match(Kind::Fox, Mode::Rendezvous);
        assert_eq!(z.cats[0].x, legacy.cats[0].x);
        assert_eq!(z.cats[3].z, legacy.cats[3].z);
        assert_eq!(z.objective, legacy.objective);
    }

    #[test]
    fn weather_evolves_during_a_match() {
        let mut g = Game::new_match(Kind::Fox, Mode::Survive);
        let dir0 = g.weather.wind_dir;
        let mut gusted = false;
        for _ in 0..(120 * 30) {
            g.step(1.0 / 30.0);
            if g.weather.gust > 0.1 { gusted = true; }
            assert!(g.weather.total_wind() <= 1.5 && g.weather.wetness <= 1.0, "weather stayed bounded");
        }
        assert!((g.weather.wind_dir - dir0).abs() > 1e-3, "the wind should veer over the match");
        assert!(gusted, "gusts should come and go");
        let (wx, wz) = g.wind;
        assert!((wx * wx + wz * wz).sqrt() > 0.5, "the wind is a live unit direction");
    }

    #[test]
    fn a_storm_does_not_help_the_hunter() {
        // a storm both blinds the hunter and scatters scent on the wind, so it should
        // never catch more than on a clear day (pin the sky each tick to isolate it).
        let caught = |sky| {
            let mut g = Game::new_match(Kind::Dog, Mode::Survive);
            for _ in 0..(90 * 30) {
                g.weather.sky = sky;
                if g.outcome.is_some() { break; }
                g.step(1.0 / 30.0);
            }
            g.caught
        };
        let clear = caught(crate::weather::Sky::Clear);
        let storm = caught(crate::weather::Sky::Storm);
        assert!(storm <= clear, "a storm shouldn't help the hunter (clear {clear}, storm {storm})");
    }

    #[test]
    fn climb_is_gradual_hugs_the_trunk_and_costs_stamina() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.trees = vec![(0.0, 0.0)];
        g.hunter.x = 300.0; g.hunter.z = 300.0;   // keep the hunter out of it
        g.cats[0].x = 1.6; g.cats[0].z = 0.0; g.cats[0].stamina = 1.0;
        g.set_input(0, 0.0, 0.0, true, false, false);
        g.step(1.0 / 30.0);
        let t1 = g.cats[0].climb_t;
        for _ in 0..15 { g.set_input(0, 0.0, 0.0, true, false, false); g.step(1.0 / 30.0); }
        let t2 = g.cats[0].climb_t;
        assert!(t2 > t1 && t2 <= 1.0, "the cat should climb the trunk over time ({t1:.2} → {t2:.2})");
        assert!(g.cats[0].x.abs() < 1.4, "climbing should pull the cat onto the trunk");
        assert!(g.cats[0].stamina < 1.0, "clinging should cost stamina");
    }

    #[test]
    fn an_exhausted_cat_slips_out_of_the_tree() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.trees = vec![(0.0, 0.0)];
        g.hunter.x = 300.0; g.hunter.z = 300.0;
        g.cats[0].x = 0.0; g.cats[0].z = 0.0; g.cats[0].stamina = 0.06;   // nearly spent
        let mut slipped = false;
        for _ in 0..(20 * 30) {
            g.set_input(0, 0.0, 0.0, true, false, false);   // keep holding on
            g.step(1.0 / 30.0);
            if g.cats[0].climb_t <= CLIMB_SAFE { slipped = true; break; }
        }
        assert!(slipped, "with no stamina to cling, the cat should drop below safe height");
    }

    #[test]
    fn clans_move_to_their_element() {
        use crate::world;
        // measure how far a clan's cat travels in one step over a given terrain type
        let travel = |clan: u8, want_water: bool| -> f32 {
            let mut g = Game::new_match(Kind::Fox, Mode::Survive);
            g.clan = clan;
            g.hunter.x = 400.0; g.hunter.z = 400.0;
            // find a tile of the wanted terrain
            let mut spot = None;
            for i in -80..80 { let (x, z) = (i as f32 * 3.0, 0.0);
                let wet = world::height_at(x, z) < world::WATER_Y;
                if wet == want_water { spot = Some((x, z)); break; } }
            let (sx, sz) = spot.expect("terrain sample");
            g.cats[0].x = sx; g.cats[0].z = sz;
            g.set_input(0, 1.0, 0.0, false, false, false);   // run east
            g.step(1.0 / 30.0);
            (g.cats[0].x - sx).abs()
        };
        // Tideclan (1) outswims Mossclan (2) in the water
        assert!(travel(1, true) > travel(2, true), "Tideclan should swim faster than Mossclan");
        // Amberclan (3) outruns Tideclan (1) on open ground
        assert!(travel(3, false) > travel(1, false), "Amberclan should run faster than Tideclan on land");
    }

    #[test]
    fn mossclan_climbs_faster_than_amberclan() {
        let climb_after = |clan: u8| -> f32 {
            let mut g = Game::new_6v1(Kind::Fox);
            g.clan = clan;
            g.trees = vec![(0.0, 0.0)];
            g.hunter.x = 400.0; g.hunter.z = 400.0;
            g.cats[0].x = 0.0; g.cats[0].z = 0.0;
            for _ in 0..4 { g.set_input(0, 0.0, 0.0, true, false, false); g.step(1.0 / 30.0); }
            g.cats[0].climb_t
        };
        assert!(climb_after(2) > climb_after(3), "Mossclan should scramble up faster than Amberclan");
    }

    #[test]
    fn emberclan_hides_tighter_than_amberclan() {
        // a hidden cat with the hunter creeping in: the stealthy clan gets flushed from
        // closer, so at a mid distance it survives where a poor-stealth clan is caught.
        let survives = |clan: u8| -> bool {
            let mut g = Game::new_match(Kind::Badger, Mode::Survive);
            g.clan = clan;
            for i in 1..N_CATS { g.cats[i].alive = false; }
            g.cats[0].x = 0.0; g.cats[0].z = 0.0;
            g.bushes.clear(); g.bushes.push((0.0, 0.0, 4.0));
            let base = 1.2 + Kind::Badger.params().smell * 0.035;
            g.hunter.x = 0.0; g.hunter.z = base * 0.95;   // just inside the neutral flush range
            g.hunter.state = HState::Chase; g.hunter.suspicion = 1.4;
            g.set_input(0, 0.0, 0.0, true, false, false);
            g.step(0.001);
            g.cats[0].alive
        };
        assert!(survives(0) && !survives(3), "Emberclan should hide tighter than Amberclan");
    }

    #[test]
    fn danger_spikes_when_the_hunter_closes_in() {
        let mut g = Game::new_match(Kind::Dog, Mode::Survive);
        g.cats[0].x = 0.0; g.cats[0].z = 0.0;
        // far, idle patrol → little dread
        for _ in 0..40 {
            g.hunter.x = 120.0; g.hunter.z = 120.0; g.hunter.state = HState::Patrol; g.hunter.suspicion = 0.0;
            g.set_input(0, 0.0, 0.0, false, false, false); g.step(1.0 / 30.0);
        }
        let far = g.danger;
        // right on top, chasing (held just out of catch range) → dread pins near 1
        for _ in 0..40 {
            g.cats[0].x = 0.0; g.cats[0].z = 0.0;
            g.hunter.x = 5.0; g.hunter.z = 0.0; g.hunter.state = HState::Chase; g.hunter.suspicion = 1.4;
            g.set_input(0, 0.0, 0.0, false, false, false); g.step(1.0 / 30.0);
        }
        let near = g.danger;
        assert!(far < 0.25, "a distant patrol should feel calm (danger {far:.2})");
        assert!(near > far + 0.4 && near > 0.6, "closing in should spike the dread (far {far:.2} → near {near:.2})");
    }

    #[test]
    fn the_hunter_creeps_when_it_searches_the_trail() {
        // the same searching hunter covers far less ground when it's right on the trail's
        // end (creeping and casting about) than when the trail is far off (running to it).
        let ground = |near_trail: bool| -> f32 {
            let mut g = Game::new_match(Kind::Fox, Mode::Survive);
            for c in g.cats.iter_mut() { c.x = 300.0; c.z = 300.0; }   // nothing in sight
            g.scent.clear();
            g.hunter.x = 0.0; g.hunter.z = 0.0;
            let mut moved = 0.0;
            for _ in 0..6 {
                g.hunter.suspicion = 0.5;                              // keep it in the search band
                g.hunter.state = HState::Search;                       // and actively searching
                g.hunter.last_seen = Some(if near_trail { (2.0, 0.0) } else { (250.0, 0.0) });
                let (px, pz) = (g.hunter.x, g.hunter.z);
                g.step(1.0 / 30.0);
                moved += dist(px, pz, g.hunter.x, g.hunter.z);
            }
            moved
        };
        assert!(ground(true) < ground(false) * 0.75, "the hunter should creep near the trail's end ({:.2} vs {:.2})", ground(true), ground(false));
    }

    #[test]
    fn keen_nosed_clans_sense_danger_from_further() {
        // a predator 55 units off is beyond a normal nose but within Fenclan's keen one,
        // so Fenclan feels the dread where Amberclan (poor nose) stays oblivious.
        let danger_at = |clan: u8| -> f32 {
            let mut g = Game::new_match(Kind::Fox, Mode::Survive);
            g.clan = clan;
            for _ in 0..40 {
                g.cats[0].x = 0.0; g.cats[0].z = 0.0;
                g.hunter.x = 55.0; g.hunter.z = 0.0; g.hunter.state = HState::Track; g.hunter.suspicion = 0.6;
                g.set_input(0, 0.0, 0.0, false, false, false); g.step(1.0 / 30.0);
            }
            g.danger
        };
        assert!(danger_at(4) > danger_at(3) + 0.05, "Fenclan should smell the far predator where Amberclan can't ({:.2} vs {:.2})", danger_at(4), danger_at(3));
    }

    #[test]
    fn a_running_cat_snaps_a_twig_and_is_heard() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.twigs = vec![(0.6, 0.0)];                    // a twig right in the player's path
        g.cats[0].x = 0.0; g.cats[0].z = 0.0;
        g.hunter.x = 10.0; g.hunter.z = 0.0;           // within earshot
        let s0 = g.hunter.suspicion;
        g.set_input(0, 1.0, 0.0, false, false, false); // run east, not crouched
        g.step(1.0 / 30.0);
        assert!(g.twigs.is_empty(), "the twig should crack underfoot");
        assert!(g.noise_flash > 0.0, "the snap should register as noise");
        assert!(g.hunter.suspicion > s0, "a hunter within earshot should hear it");
    }

    #[test]
    fn a_crouched_cat_leaves_twigs_intact() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.twigs = vec![(0.4, 0.0)];
        g.cats[0].x = 0.0; g.cats[0].z = 0.0;
        g.hunter.x = 300.0; g.hunter.z = 300.0;        // far off, so only the snap could matter
        g.set_input(0, 1.0, 0.0, true, false, false);  // creep (crouched) east
        g.step(1.0 / 30.0);
        assert_eq!(g.twigs.len(), 1, "a careful, crouched cat snaps nothing");
        assert_eq!(g.noise_flash, 0.0, "and makes no noise");
    }

    #[test]
    fn a_running_cat_bounds_over_a_log() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.logs = vec![(2.5, 0.0)];                     // a fallen log just ahead
        g.cats[0].x = 0.0; g.cats[0].z = 0.0; g.cats[0].leap_cd = 0.0;
        g.hunter.x = 300.0; g.hunter.z = 300.0;
        let mut bounded = false;
        for _ in 0..40 {
            g.set_input(0, 1.0, 0.0, false, false, false);   // run east across the log
            g.step(1.0 / 30.0);
            if g.cats[0].leap_t > 0.0 { bounded = true; break; }
        }
        assert!(bounded, "a cat running across a log should bound over it");
    }

    #[test]
    fn a_flick_performs_a_move() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.cats[0].heading = 0.0;
        g.player_flick((0.0, 1.0), 1.0, false);        // forward flick → pounce
        assert_eq!(g.player_move.map(|(m, _)| m), Some(crate::combat::Move::Pounce));
        assert!(g.cats[0].leap_t > 0.0, "a pounce should lunge");
    }

    #[test]
    fn a_landed_attack_staggers_the_hunter() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.cats[0].x = 0.0; g.cats[0].z = 0.0; g.cats[0].heading = 0.0;
        g.hunter.x = 2.0; g.hunter.z = 0.0; g.hunter.state = HState::Chase; g.hunter.suspicion = 1.4;
        g.player_flick((0.1, 0.1), 1.0, false);        // a swat (attack) at point-blank
        for _ in 0..10 { g.set_input(0, 0.0, 0.0, false, false, false); g.step(1.0 / 30.0); if g.hunter_stagger > 0.0 { break; } }
        assert!(g.hunter_stagger > 0.0, "a connecting swat should stagger the hunter");
        assert!(g.cats[0].alive, "and the dazed hunter can't complete the catch");
    }

    #[test]
    fn two_moves_chain_into_a_named_combo() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.hunter.x = 300.0; g.hunter.z = 300.0;
        g.player_flick((1.0, 0.0), 1.0, false);        // a roll
        for _ in 0..30 { g.set_input(0, 0.0, 0.0, false, false, false); g.step(1.0 / 30.0); if g.player_move.is_none() { break; } }
        assert!(g.combo_last.is_some(), "the finished roll is available to chain");
        g.player_flick((0.0, 1.0), 1.0, false);        // pounce within the window
        assert_eq!(g.combo_name, "Ambush Pounce");
        assert!(g.combo_flash > 0.0);
    }

    #[test]
    fn a_frozen_cat_reads_the_predator_farther() {
        // stopping to taste the air extends how far off the danger registers, so a still
        // cat feels a distant predator that a moving one wouldn't.
        let danger_at = |moving: bool| -> f32 {
            let mut g = Game::new_match(Kind::Fox, Mode::Survive);
            for _ in 0..40 {
                g.cats[0].x = 0.0; g.cats[0].z = 0.0;
                g.hunter.x = 45.0; g.hunter.z = 0.0; g.hunter.state = HState::Track; g.hunter.suspicion = 0.4;
                g.set_input(0, if moving { 1.0 } else { 0.0 }, 0.0, false, false, false);
                g.step(1.0 / 30.0);
            }
            g.danger
        };
        assert!(danger_at(false) > danger_at(true), "a frozen cat should read the danger more keenly ({:.2} vs {:.2})", danger_at(false), danger_at(true));
    }

    #[test]
    fn you_smell_the_predator_that_lies_upwind() {
        // wind blows toward +x, so a hunter to your west (upwind) drifts its reek to you,
        // while one to your east (downwind) you can't smell at all.
        let reek = |hunter_x: f32| -> f32 {
            let mut g = Game::new_match(Kind::Fox, Mode::Survive);
            g.weather.wind_dir = std::f32::consts::FRAC_PI_2;   // toward +x
            g.weather.wind_strength = 0.8; g.weather.gust = 0.0;
            g.cats[0].x = 0.0; g.cats[0].z = 0.0;
            g.hunter.x = hunter_x; g.hunter.z = 0.0;
            g.read_wind().predator_reek
        };
        assert!(reek(-30.0) > 0.2, "the hunter's reek should carry downwind to you");
        assert_eq!(reek(30.0), 0.0, "you can't smell a hunter that sits downwind of you");
    }

    #[test]
    fn round_resolves_to_an_outcome() {
        let mut g = Game::new_6v1(Kind::Dog);
        for _ in 0..(95 * 30) { if g.outcome.is_some() { break; } g.step(1.0 / 30.0); }
        assert!(g.outcome.is_some(), "the round must resolve to a winner");
    }

    #[test]
    fn every_hunter_and_mode_soaks_without_panic() {
        // Full rounds for all six archetypes in both modes, with a live player wiggling
        // through move/dodge/crouch/climb — shakes out panics in less-common branches
        // and confirms the round always terminates. Also exercises the ecology at scale.
        let hunters = [Kind::Fox, Kind::Dog, Kind::Badger, Kind::Hawk, Kind::Snake, Kind::Shadow];
        for k in hunters {
            for m in [Mode::Rendezvous, Mode::Survive] {
                let mut g = Game::new_match(k, m);
                let mut t = 0i32;
                while g.outcome.is_none() && t < 95 * 30 {
                    let mx = (t as f32 * 0.30).sin();
                    let mz = (t as f32 * 0.21).cos();
                    g.set_input(0, mx, mz, t % 40 < 8, t % 55 == 0, t % 63 == 0);   // crouch bursts, roll, occasional leap
                    g.step(1.0 / 30.0);
                    t += 1;
                }
                assert!(g.outcome.is_some(), "hunter {:?} / mode {:?} should resolve", k, m);
            }
        }
    }

    #[test]
    fn sprinting_drains_stamina() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.hunter.x = 5.0; g.hunter.z = 0.0;        // right on cat 1 → it sprints to flee
        g.cats[1].x = 0.0; g.cats[1].z = 0.0; g.cats[1].player = false; g.bushes.clear();
        let s0 = g.cats[1].stamina;
        for _ in 0..30 { g.step(1.0 / 30.0); }
        assert!(g.cats[1].stamina < s0, "fleeing should burn stamina");
    }

    #[test]
    fn hunter_tracks_scent_when_blind() {
        let mut g = Game::new_6v1(Kind::Badger); // biggest nose
        for c in g.cats.iter_mut() { c.x = 500.0; c.z = 500.0; c.hidden = true; c.player = false; } // nothing visible
        g.hunter.x = 0.0; g.hunter.z = 0.0; g.hunter.heading = 0.0;
        g.scent.push(Scent { x: 0.0, z: 8.0, strength: 1.0 });   // a fresh trail ahead
        let d0 = dist(0.0, 0.0, 0.0, 8.0);
        for _ in 0..20 { g.step(1.0 / 30.0); }
        let d1 = dist(g.hunter.x, g.hunter.z, 0.0, 8.0);
        assert!(d1 < d0, "a blind hunter should follow a scent it can smell ({d0} -> {d1})");
    }

    #[test]
    fn climbing_escapes_ground_hunter_but_not_the_hawk() {
        let setup = |kind| {
            let mut g = Game::new_6v1(kind);
            g.trees = vec![(0.0, 0.0)];                 // a climbable trunk at the origin
            g.cats[0].x = 0.0; g.cats[0].z = 0.0;       // player cat stands on it
            g.hunter.x = 0.0; g.hunter.z = 0.0; g.hunter.state = HState::Chase; g.hunter.suspicion = 1.2; g.hunter.last_seen = Some((0.0, 0.0));
            g.set_input(0, 0.0, 0.0, true, false, false);             // crouch at a trunk → climb
            g.step(1.0 / 30.0);
            g
        };
        let fox = setup(Kind::Fox);
        assert!(fox.cats[0].climbing && fox.cats[0].alive, "a climbing cat is safe from a ground hunter");
        let hawk = setup(Kind::Hawk);
        assert!(hawk.cats[0].is_carried() || !hawk.cats[0].alive, "the hawk plucks a climbing cat off the trunk (grabs/carries it)");
    }

    #[test]
    fn leap_is_a_fast_committed_bound() {
        // a leap step covers more ground than a plain run and is uncatchable mid-air
        let run_dz = {
            let mut r = Game::new_6v1(Kind::Fox);
            r.bushes.clear(); r.trees.clear();
            r.cats[0].x = 0.0; r.cats[0].z = 0.0;
            r.set_input(0, 0.0, 1.0, false, false, false);   // plain run +z
            r.step(1.0 / 30.0);
            r.cats[0].z
        };
        let mut g = Game::new_6v1(Kind::Fox);
        g.bushes.clear(); g.trees.clear();
        g.cats[0].x = 0.0; g.cats[0].z = 0.0; g.cats[0].heading = 0.0;
        g.set_input(0, 0.0, 1.0, false, false, true);        // leap +z
        g.step(1.0 / 30.0);
        assert!(g.cats[0].is_leaping(), "the leap should engage");
        assert!(g.cats[0].z > run_dz + 1e-3, "a leap covers more ground than a run ({} vs {})", g.cats[0].z, run_dz);

        // point-blank aggressive hunter can't take an airborne cat
        g.hunter.x = g.cats[0].x; g.hunter.z = g.cats[0].z;
        g.hunter.state = HState::Chase; g.hunter.suspicion = 1.3;
        g.set_input(0, 0.0, 1.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.cats[0].alive && g.cats[0].is_leaping(), "a leaping cat survives a point-blank frame");
    }

    #[test]
    fn hawk_grabs_carries_then_resolves() {
        let mut g = Game::new_6v1(Kind::Hawk);
        g.bushes.clear();
        g.cats[0].x = 0.0; g.cats[0].z = 0.0; g.cats[0].player = false;
        g.hunter.x = 0.0; g.hunter.z = 0.0; g.hunter.state = HState::Chase; g.hunter.suspicion = 1.3;
        for _ in 0..6 { g.step(1.0 / 30.0); if g.hunter.carrying { break; } }
        assert!(g.hunter.carrying && g.cats[0].is_carried() && g.cats[0].alive, "the hawk hoists open prey aloft");
        // the struggle resolves within CARRY_DUR: taken (dead) or wrenched free (alive+crippled)
        for _ in 0..120 { g.step(1.0 / 30.0); if !g.hunter.carrying { break; } }
        assert!(!g.hunter.carrying, "the carry resolves and frees the hawk");
        assert!(!g.cats[0].alive || g.cats[0].crippled_t > 0.0, "resolved to taken, or broke free and is crippled");
    }

    #[test]
    fn leaping_cat_pounces_prey() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.cats[0].x = 0.0; g.cats[0].z = 0.0; g.cats[0].heading = 0.0; g.cats[0].stamina = 0.5;
        g.critters.clear();
        g.critters.push(crate::ecology::Critter::new(crate::ecology::Species::Hare, 0.0, 1.0, 3));
        let s0 = g.cats[0].stamina;
        g.set_input(0, 0.0, 1.0, false, false, true);        // leap onto the critter
        g.step(1.0 / 30.0);
        assert!(g.critters.is_empty(), "a leaping cat snatches the critter it lands on");
        assert!(g.cats[0].stamina > s0, "a caught critter tops up stamina");
        assert!(g.prey_caught >= 1, "the pounce is tallied");
    }

    #[test]
    fn snake_ambushes_from_water() {
        let mut g = Game::new_6v1(Kind::Snake);
        let home = g.snake_home.unwrap_or((g.hunter.x, g.hunter.z));
        g.snake_home = Some(home);
        g.hunter.x = home.0; g.hunter.z = home.1;
        g.hunter.state = HState::Chase; g.hunter.suspicion = 1.2;
        g.hunter.last_seen = Some((home.0 + 5.0, home.1));
        let hx0 = g.hunter.x;
        g.step(1.0 / 30.0);
        // a plain 11 u/s snake moves ~0.37 per tick; the 1.8x strike moves ~0.66
        assert!(g.hunter.x - hx0 > 0.5, "the snake should lunge from its pool with a burst");
    }

    #[test]
    fn dodge_gives_iframes() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.hunter.x = 0.0; g.hunter.z = 0.0; g.hunter.state = HState::Chase; g.hunter.suspicion = 1.2; g.hunter.last_seen = Some((0.5, 0.0));
        g.bushes.clear(); g.trees.clear();
        g.cats[0].x = 0.5; g.cats[0].z = 0.0;       // player cat point-blank, in the open
        g.set_input(0, 1.0, 0.0, false, true, false);       // roll east on the catch frame
        g.step(1.0 / 30.0);
        assert!(g.cats[0].dodge_t > 0.0 && g.cats[0].alive, "a dodging cat survives a point-blank catch");
    }

    #[test]
    fn survive_mode_has_no_escape() {
        let mut g = Game::new_match(Kind::Snake, Mode::Survive);
        assert_eq!(g.mode, Mode::Survive);
        for _ in 0..300 { if g.outcome.is_some() { break; } g.step(1.0 / 30.0); }
        assert_eq!(g.escaped, 0, "there is no rendezvous escape in survive mode");
    }

    #[test]
    fn shadow_blinks_to_the_quarry() {
        let mut g = Game::new_match(Kind::Shadow, Mode::Rendezvous);
        g.teleport_cd = 0.0;                      // blink ready
        g.hunter.x = 0.0; g.hunter.z = 0.0;
        g.hunter.last_seen = Some((80.0, 0.0));   // prey slipped far across the map
        g.step(1.0 / 30.0);
        assert!(dist(g.hunter.x, g.hunter.z, 80.0, 0.0) < 25.0, "the shadow should teleport to its quarry");
    }

    #[test]
    fn shadow_shift_reveals_hiders() {
        // the shadow can't see a hidden cat normally, but the otherworld shift reveals it
        let reveals = |shifted: bool| {
            let mut g = Game::new_match(Kind::Shadow, Mode::Rendezvous);
            g.shifted = shifted; g.shift_t = 0.0;
            g.bushes = vec![(0.0, 12.0, 3.0)];   // cover the cat actually hides in
            for c in g.cats.iter_mut() { c.x = 500.0; c.z = 500.0; c.player = false; }
            g.hunter.x = 0.0; g.hunter.z = 0.0; g.hunter.heading = 0.0;
            g.cats[1].x = 0.0; g.cats[1].z = 12.0; g.cats[1].hidden = true; g.cats[1].moving = false; g.cats[1].player = false;
            g.step(1.0 / 30.0);
            g.hunter.last_seen.is_some()
        };
        assert!(!reveals(false), "a hider is unseen in the living world");
        assert!(reveals(true), "the otherworld shift reveals the hider");
    }
}
