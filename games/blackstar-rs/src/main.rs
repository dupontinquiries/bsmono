//! SKYDRIBBLE — a Rocket-League-inspired open-world air-dribble playground.
//! Rust + wgpu 22 + winit 0.29 (the same stack as the reference repo's games).

use crate::platform::ApplyProfile as _; // Profile::apply is a trait method now (see platform.rs)
// Shared with skydribble — see games/shared/shellkit. These were byte-
// identical copies in both games' own src/ before extraction; `use` at
// crate root keeps every existing `crate::net::...`-style path working
// unchanged everywhere else in this crate.
use shellkit::{academy, analog, custom, gamepad, net, partdata, replay, stream, style, touch, trials};

mod align;
mod anim;
mod rigmotion;
mod approach;
mod actuator;
mod aboard;
mod airlock;
mod amalgam;
mod ballistics;
mod bench;
mod bigbody;
mod board;
mod boarding;
mod botpack;
mod bots;
mod roomplan;
mod roster;
mod sections;
mod settlement;
mod camera;
mod chassis_traits;
mod chord;
mod circuit;
mod circuit_layout;
mod circuit_map;
mod circuit_match;
mod circuit_shapes;
mod circuit_wave;
mod cling;
mod clinch;
mod combine;
mod combo;
mod config;
mod damage;
mod dispatch;
mod district;
mod drop;
mod duel;
mod foldaudit;
mod foldsim;
mod font;
mod footing;
mod fragment;
mod cable;
mod field;
mod grandeur;
mod hardpoints;
mod foothold;
mod fleet;
mod flow;
mod fx;
mod gyro;
mod content;
mod crawler;
mod machine;
mod curriculum;
mod corruption;
mod corrupt_body;
mod cyber;
mod partcars;
mod palevector;
mod parity;
mod patrol;
mod escalation;
mod enemy;
mod gates;
mod perception;
mod trainwatch;
mod tuner;
mod tuning;
mod vo;
mod vocab;
mod valuestruct;
mod hazard;
mod helmai;
mod hive;
mod hud;
mod holomap;
mod hotbar;
mod seam;
mod shipgrav;
mod hydro;
mod idle;
mod input;
mod archive;
mod assembler;
mod archsim;
mod orbits;
mod provenance;
mod joints;
mod lift;
mod link;
mod loadout;
mod loot;
mod hullform;
mod interior;
mod menu;
mod meshgen;
mod moons;
mod mixer;
mod model;
mod online_screen;
mod optics;
mod overload;
mod physics;
mod powers;
mod planetlook;
mod platform;
mod posture;
mod procars;
mod procgen;
mod pulse;
mod compact;
mod brain;
mod campaign;
mod casting;
mod census;
mod campsim;
mod chi;
mod codex;
mod clones;
mod exoworlds;
mod deck;
mod depth;
mod gear;
mod gearsim;
mod menufx;
mod mapsim;
mod menusim;
mod cockpit;
mod driveline;
mod dropship;
mod locomotion;
mod massfilter;
mod turret;
mod raidflow;
mod raidforge;
mod shots;
mod skeleton;
mod sortie;
mod stance;
mod stations;
mod suitability;
mod superform;
mod stealth;
mod vault;
mod valence;
mod finale;
mod nightfall;
mod orient;
mod partdetect;
mod loadbearing;
mod grime;
mod gravity;
mod groundside;
mod guidance;
mod orientsim;
mod partsim;
mod skirmish;
mod raid;
mod raidcon;
mod raidgen;
mod raidlog;
mod raidrun;
mod raidsim;
mod relay;
mod render;
mod rescale;
mod rip;
mod subsea;
mod sun;
mod survival;
mod swing;
mod saga;
mod save;
mod scale;
mod ship;
mod sim;
mod sound;
mod space;
mod terrain;
mod tradecraft;
mod ui;
mod weapons;
mod xform;
mod voyage;
mod watch;
mod voxel;
mod world;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use glam::{Mat3, Mat4, Quat, Vec3, Vec4};
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Event, MouseButton, WindowEvent};
use winit::event_loop::EventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, WindowBuilder};

use config::{Config, PRESETS};
use input::{Action, Bind, Bindings, ACTIONS};
use physics::{Ball, Car, Controls, StepEvents};
use render::{Globals, ObjDraw, ObjectRaw, RtShapeRaw, UiVert};
use sound::Sfx;
use world::World;

const FIXED_DT: f32 = 1.0 / 120.0;
const COMBO_WINDOW: f32 = 7.0;

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Catalog { sel: usize },
    Garage { sel: usize },
    Academy { sel: usize },
    Trials { sel: usize },
    Splash,
    Play,
    Pause { sel: usize },
    Binds { sel: usize, capturing: bool },
    Maps { sel: usize },
    Online { sel: usize },
    Tweaks { menu: u8, sel: usize },
}

// ---- data-driven settings rows -------------------------------------------
#[derive(Clone, Copy)]
enum Row {
    Slider {
        get: fn(&Config) -> f32,
        set: fn(&mut Config, f32),
        min: f32,
        max: f32,
        step: f32,
        def: f32,
        fmt: u8, // 0 percent, 1 plain, 2 degrees
    },
    Toggle {
        get: fn(&Config) -> bool,
        set: fn(&mut Config, bool),
    },
    Preset,
    Link(u8),
    Controls,
    Trials,
    Academy,
    Garage,
    Catalog,
    Fullscreen,
    Header,
    GameMode(u8), // 0 free, 1 sparta 50v50, 2 sparta 300
    CarPick,
    Back,
}

/// Which nav-strip button is "here" for a settings screen.
fn nav_here(menu: u8) -> usize {
    match menu {
        5 => 4,       // VIDEO
        8 => 1,       // STORY lives under SETUP
        9 => 1,       // DEBUG lives under SETUP
        3 | 4 | 7 => 1, // CAR SETUP / FLIPS / MOTION live under SETUP
        _ => 1,
    }
}

/// Bounding radius of a shape, for culling.
pub fn shape_bound(sdf: &world::Sdf) -> f32 {
    match *sdf {
        world::Sdf::None => 0.0,
        world::Sdf::Sphere { r } => r,
        world::Sdf::Torus { big, tube } => big + tube,
        world::Sdf::Capsule { r, half } => r + half,
        world::Sdf::Hex { r, h } => (r * r + h * h).sqrt(),
        world::Sdf::RBox { half, .. } => half.length(),
        world::Sdf::Shell { r, half, thick } => ((r + thick) * (r + thick) + half * half).sqrt(),
        world::Sdf::TubeTorus { big, tube, thick } => big + tube + thick,
        world::Sdf::RingDeck { big, wide, .. } => big + wide,
    }
}

/// The six frustum planes of a view-projection, as (normal, d) in world
/// space. A plane is stored so that `dot(n, p) + d > -r` means "inside".
pub fn frustum_planes(vp: &Mat4) -> [[f32; 4]; 6] {
    let m = vp.to_cols_array_2d(); // column-major: m[col][row]
    let row = |r: usize| [m[0][r], m[1][r], m[2][r], m[3][r]];
    let (rx, ry, rz, rw) = (row(0), row(1), row(2), row(3));
    let mk = |a: [f32; 4], b: [f32; 4], sign: f32| {
        let p = [
            b[0] + sign * a[0],
            b[1] + sign * a[1],
            b[2] + sign * a[2],
            b[3] + sign * a[3],
        ];
        let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-6);
        [p[0] / len, p[1] / len, p[2] / len, p[3] / len]
    };
    [
        mk(rx, rw, 1.0),  // left
        mk(rx, rw, -1.0), // right
        mk(ry, rw, 1.0),  // bottom
        mk(ry, rw, -1.0), // top
        mk(rz, rw, 1.0),  // near
        mk(rz, rw, -1.0), // far
    ]
}

/// Is a bounding sphere at all visible in this frustum?
pub fn sphere_visible(planes: &[[f32; 4]; 6], c: Vec3, r: f32) -> bool {
    for p in planes.iter() {
        if p[0] * c.x + p[1] * c.y + p[2] * c.z + p[3] < -r {
            return false;
        }
    }
    true
}

fn menu_title(menu: u8) -> &'static str {
    match menu {
        9 => "DEBUG",
        8 => "STORY",
        7 => "MOTION / GYRO",
        6 => "GAME MODES",
        1 => "CAMERA",
        2 => "ARENA + BALL MUTATORS",
        3 => "CAR SETUP",
        4 => "FLIPS",
        5 => "VIDEO",
        _ => "SETTINGS",
    }
}

fn tweak_rows(menu: u8) -> Vec<(&'static str, Row)> {
    use Row::*;
    let pct = |get: fn(&Config) -> f32, set: fn(&mut Config, f32), min: f32, max: f32| Slider {
        get, set, min, max, step: 0.05, def: 1.0, fmt: 0,
    };
    match menu {
        1 => vec![
            ("FIELD OF VIEW", Slider { get: |c| c.cam.fov_deg, set: |c, v| c.cam.fov_deg = v, min: 50.0, max: 200.0, step: 1.0, def: 86.0, fmt: 2 }),
            ("DISTANCE", Slider { get: |c| c.cam.dist, set: |c, v| c.cam.dist = v, min: 2.5, max: 9.0, step: 0.1, def: 4.9, fmt: 1 }),
            ("HEIGHT", Slider { get: |c| c.cam.height, set: |c, v| c.cam.height = v, min: 0.4, max: 24.0, step: 0.05, def: 1.21, fmt: 1 }),
            ("ANGLE", Slider { get: |c| c.cam.angle_deg, set: |c, v| c.cam.angle_deg = v, min: -25.0, max: 25.0, step: 1.0, def: 20.0, fmt: 2 }),
            ("STIFFNESS", pct(|c| c.cam.stiffness, |c, v| c.cam.stiffness = v, 0.15, 3.2)),
            ("SHAKE", pct(|c| c.cam.shake, |c, v| c.cam.shake = v, 0.0, 1.5)),
            ("GRAVITY CAMERA (TUBES)", pct(|c| c.cam.grav_follow, |c, v| c.cam.grav_follow = v, 0.0, 1.0)),
            ("MOVING PLATFORM BEND", Slider { get: |c| c.cam.plat_bend, set: |c, v| c.cam.plat_bend = v, min: 0.0, max: 1.0, step: 0.05, def: 0.55, fmt: 0 }),
            ("BACK", Back),
        ],
        2 => vec![
            ("PRESETS", Header),
            ("MUTATOR PRESET", Preset),
            ("ARENA", Header),
            ("GRAVITY", pct(|c| c.arena.gravity, |c, v| c.arena.gravity = v, 0.0, 2.6)),
            ("ARENA BOUNCINESS", pct(|c| c.arena.bounce, |c, v| c.arena.bounce = v, 0.2, 2.0)),
            ("ARENA FRICTION", pct(|c| c.arena.friction, |c, v| c.arena.friction = v, 0.1, 2.5)),
            ("WORLD SIZE (REBUILDS)", pct(|c| c.arena.scale, |c, v| c.arena.scale = v, 0.55, 10.6)),
            ("BALL", Header),
            ("BALL SIZE", pct(|c| c.ball.size, |c, v| c.ball.size = v, 0.35, 30.0)),
            ("BALL BOUNCINESS", pct(|c| c.ball.bounce, |c, v| c.ball.bounce = v, 0.2, 3.0)),
            ("BALL FRICTION", pct(|c| c.ball.friction, |c, v| c.ball.friction = v, 0.0, 2.5)),
            ("BALL WEIGHT", pct(|c| c.ball.weight, |c, v| c.ball.weight = v, 0.25, 6.0)),
            ("BACK", Back),
        ],
        3 => vec![
            ("CHASSIS", Header),
            ("CAR", CarPick),
            ("CAR SIZE", pct(|c| c.car.size, |c, v| c.car.size = v, 0.55, 2.0)),
            ("TIRE GRIP", pct(|c| c.car.grip, |c, v| c.car.grip = v, 0.3, 2.5)),
            ("POWER", Header),
            ("TOP SPEED", pct(|c| c.car.top_speed, |c, v| c.car.top_speed = v, 0.5, 10.0)),
            ("ACCELERATION", pct(|c| c.car.accel, |c, v| c.car.accel = v, 0.4, 6.0)),
            ("JUMP STRENGTH", pct(|c| c.car.jump, |c, v| c.car.jump = v, 0.5, 2.2)),
            ("BOOST", Header),
            ("BOOST POWER", pct(|c| c.car.boost, |c, v| c.car.boost = v, 0.4, 2.6)),
            ("BOOST REGEN /S", Slider { get: |c| c.car.boost_regen, set: |c, v| c.car.boost_regen = v, min: 0.0, max: 35.0, step: 1.0, def: 0.0, fmt: 1 }),
            ("BOOST TANK", pct(|c| c.car.boost_max, |c, v| c.car.boost_max = v, 0.3, 2.5)),
            ("UNLIMITED BOOST", Toggle { get: |c| c.car.unlimited_boost, set: |c, v| c.car.unlimited_boost = v }),
            ("INPUT", Header),
            ("ANALOG KEYS (WOOTING)", Toggle { get: |c| c.car.analog, set: |c, v| c.car.analog = v }),
            ("AUTOTARGET: HOLD", Toggle { get: |c| c.car.autotarget_hold, set: |c, v| c.car.autotarget_hold = v }),
            ("MOTION / GYRO DRIVE", Link(7)),
            ("ANALOG WIND-IN DEPTH", pct(|c| c.car.analog_soft, |c, v| c.car.analog_soft = v, 0.0, 0.8)),
            ("BACK", Back),
        ],
        4 => vec![
            ("THE FLIP", Header),
            ("FLIP STRENGTH", pct(|c| c.flip.strength, |c, v| c.flip.strength = v, 0.4, 2.2)),
            ("FLIP SPIN", Slider { get: |c| c.flip.spin, set: |c, v| c.flip.spin = v, min: 0.5, max: 1.5, step: 0.05, def: 1.0, fmt: 0 }),
            ("STEER RATE", Slider { get: |c| c.car.steer_rate, set: |c, v| c.car.steer_rate = v, min: 0.5, max: 1.5, step: 0.05, def: 0.90, fmt: 0 }),
            ("INPUT EASE", Slider { get: |c| c.car.input_ease, set: |c, v| c.car.input_ease = v, min: 0.5, max: 1.5, step: 0.05, def: 1.10, fmt: 0 }),
            ("FLIP CANCELS", Toggle { get: |c| c.flip.cancel, set: |c, v| c.flip.cancel = v }),
            ("RESETS", Header),
            ("FLIP RESETS", Toggle { get: |c| c.flip.resets, set: |c, v| c.flip.resets = v }),
            ("STRICT RESETS (CLEAN WHEELS)", Toggle { get: |c| c.flip.strict, set: |c, v| c.flip.strict = v }),
            ("UNLIMITED FLIPS", Toggle { get: |c| c.flip.unlimited, set: |c, v| c.flip.unlimited = v }),
            ("ASSISTS", Header),
            ("LANDING ASSIST", pct(|c| c.flip.land_assist, |c, v| c.flip.land_assist = v, 0.0, 1.5)),
            ("AIR STEER: ROLL BY DEFAULT", Toggle { get: |c| c.flip.air_roll_default, set: |c, v| c.flip.air_roll_default = v }),
            ("BACK", Back),
        ],
        5 => vec![
            ("GRAPHICS PRESET", Slider { get: |c| c.video.tier as f32, set: |c, v| config::apply_gfx_tier(c, v as u8), min: 0.0, max: 3.0, step: 1.0, def: 0.0, fmt: 1 }),
            ("RAYS", Header),
            ("RAY TRACING", Toggle { get: |c| c.video.rays, set: |c, v| c.video.rays = v }),
            // one dial: steps, candidates, AO probes and range all follow it
            ("RAYS", pct(|c| c.video.quality, |c, v| c.video.quality = v, 0.4, 1.8)),
            ("CINEMA", Header),
            ("FILMIC TONEMAP (ACES)", Toggle { get: |c| c.video.filmic, set: |c, v| c.video.filmic = v }),
            ("EXPOSURE", pct(|c| c.video.exposure, |c, v| c.video.exposure = v, 0.5, 1.8)),
            ("VIGNETTE", Slider { get: |c| c.video.vignette, set: |c, v| c.video.vignette = v, min: 0.0, max: 1.0, step: 0.05, def: 0.32, fmt: 0 }),
            ("FILM GRAIN", Slider { get: |c| c.video.grain, set: |c, v| c.video.grain = v, min: 0.0, max: 1.0, step: 0.05, def: 0.12, fmt: 0 }),
            ("BLOOM", pct(|c| c.video.bloom, |c, v| c.video.bloom = v, 0.0, 2.0)),
            ("RENDER SCALE", Slider { get: |c| c.video.render_scale, set: |c, v| c.video.render_scale = v, min: 0.25, max: 1.0, step: 0.05, def: 1.0, fmt: 1 }),
            ("TOUCH CONTROLS", Toggle { get: |c| c.touch.enabled, set: |c, v| c.touch.enabled = v }),
            ("TOUCH SIZE", Slider { get: |c| c.touch.scale, set: |c, v| c.touch.scale = v, min: 0.6, max: 1.8, step: 0.05, def: 1.0, fmt: 1 }),
            ("SCREEN DOOR", Slider { get: |c| c.video.screen_door, set: |c, v| c.video.screen_door = v, min: 0.0, max: 0.35, step: 0.02, def: 0.08, fmt: 0 }),
            ("LETTERBOX (CINEMA BARS)", Toggle { get: |c| c.video.letterbox, set: |c, v| c.video.letterbox = v }),
            ("DISPLAY", Header),
            ("VSYNC", Toggle { get: |c| c.video.vsync, set: |c, v| c.video.vsync = v }),
            ("FULLSCREEN (F11)", Fullscreen),
            ("BACK", Back),
        ],
        6 => vec![
            ("PICK YOUR BATTLE (OFFLINE)", Header),
            ("FREE PLAY - NO BOTS", GameMode(0)),
            ("SPARTA 50 V 50 - STADIUM", GameMode(1)),
            ("SPARTA 300 - BALLCHASE SWARM", GameMode(2)),
            ("VERSUS 1 V 1", GameMode(3)),
            ("VERSUS 2 V 2", GameMode(4)),
            ("VERSUS 3 V 3", GameMode(5)),
            ("VERSUS 4 V 4", GameMode(6)),
            ("VERSUS 5 V 5", GameMode(7)),
            ("EVERY BOT IS IN BALLCHASE MODE. GOOD LUCK.", Header),
            ("BACK", Back),
        ],
        // ============ DEBUG — testing overrides, not feel ============
        // Its own screen rather than a row inside STORY, because everything
        // here changes what a save IS rather than how it plays, and a tester
        // needs one place to look. Kept deliberately small: a debug menu that
        // accumulates becomes a menu nobody reads.
        9 => vec![
            ("MASTER ACCESS", Header),
            // ONE SWITCH. Every progression gate defers to
            // `Shipboard::unrestricted`, so this opens the Wardeck, the space
            // bridge and every destination at once — a tester who turns it on
            // does not then hit the next gate they had not thought about.
            // The ship itself is always parked (`repark` is unconditional), so
            // what this unlocks is everything the ship can REACH: raids, the
            // far worlds, and the consoles behind them.
            ("MASTER ACCESS (ALL GATES OPEN)", Toggle { get: |c| c.story.debug_ship, set: |c, v| c.story.debug_ship = v }),
        ],
        8 => vec![
            // THE DIALS THE FICTION EXPOSES. `--tune` has carried these since
            // they landed and the menu never did, so a knob documented as
            // "a main-menu dial for how much history the game hands you
            // already believing" was reachable only by hand-editing a save.
            // Describing something as a menu setting does not put it in a menu.
            ("WHAT YOU ALREADY BELIEVE", Header),
            ("MYSTERY ANCHOR", Slider { get: |c| c.story.anchor, set: |c, v| c.story.anchor = v, min: 0.0, max: 1.0, step: 0.05, def: 0.45, fmt: 0 }),
            ("CAMPAIGN (0 PRESENT / 1 ARCHIVE)", Slider { get: |c| c.story.era, set: |c, v| c.story.era = v, min: 0.0, max: 1.0, step: 1.0, def: 0.0, fmt: 1 }),
            ("HOW THE MAP FIRMS UP", Header),
            ("SOURCES FOR 'POSSIBLE'", Slider { get: |c| c.story.possible_at, set: |c, v| c.story.possible_at = v, min: 1.0, max: 8.0, step: 1.0, def: 2.0, fmt: 1 }),
            ("SOURCES FOR 'HIGH CONFIDENCE'", Slider { get: |c| c.story.high_at, set: |c, v| c.story.high_at = v, min: 2.0, max: 12.0, step: 1.0, def: 3.0, fmt: 1 }),
            ("WHAT YOU CAN CARRY", Header),
            ("CASE CAPACITY", Slider { get: |c| c.story.case_cap, set: |c, v| c.story.case_cap = v, min: 1.0, max: 24.0, step: 1.0, def: 6.0, fmt: 1 }),
            ("BACK", Back),
        ],
        7 => vec![
            ("MOTION CONTROL", Header),
            ("GYRO DRIVE", Toggle { get: |c| c.gyro.on, set: |c, v| c.gyro.on = v }),
            ("GYRO SENSITIVITY", Slider { get: |c| c.gyro.sens, set: |c, v| c.gyro.sens = v, min: 0.1, max: 4.0, step: 0.1, def: 1.0, fmt: 1 }),
            ("GYRO DEADZONE", Slider { get: |c| c.gyro.deadzone, set: |c, v| c.gyro.deadzone = v, min: 0.0, max: 0.4, step: 0.01, def: 0.06, fmt: 0 }),
            ("GYRO SMOOTHING", Slider { get: |c| c.gyro.smooth, set: |c, v| c.gyro.smooth = v, min: 0.008, max: 0.3, step: 0.008, def: 0.05, fmt: 1 }),
            ("GYRO AIR MODE (0 ROLL 1 YAW)", Slider { get: |c| c.gyro.air_mode as f32, set: |c, v| c.gyro.air_mode = v as u8 & 1, min: 0.0, max: 1.0, step: 1.0, def: 0.0, fmt: 1 }),
            ("GYRO WHEN (0 ALWAYS 1 BOOST 2 AIR)", Slider { get: |c| c.gyro.gate as f32, set: |c, v| c.gyro.gate = v as u8, min: 0.0, max: 2.0, step: 1.0, def: 0.0, fmt: 1 }),
            ("GYRO INVERT TILT", Toggle { get: |c| c.gyro.invert_x, set: |c, v| c.gyro.invert_x = v }),
            ("GYRO INVERT PITCH", Toggle { get: |c| c.gyro.invert_y, set: |c, v| c.gyro.invert_y = v }),
            ("BACK", Back),
        ],
        _ => vec![
            ("CATALOGUE - EVERY VEHICLE", Catalog),
            ("GARAGE - SKINS, WHEELS, TRAILS", Garage),
            ("ACADEMY - LEARN THE TRICKS", Academy),
            ("TRIALS", Trials),
            ("CONTROLS + KEY BINDINGS", Controls),
            ("GAME MODES", Link(6)),
            ("CAMERA", Link(1)),
            ("ARENA + BALL MUTATORS", Link(2)),
            ("CAR SETUP", Link(3)),
            ("FLIPS", Link(4)),
            ("VIDEO", Link(5)),
            ("STORY", Link(8)),
            ("DEBUG", Link(9)),
            ("BACK", Back),
        ],
    }
}

fn is_header(row: &Row) -> bool {
    matches!(row, Row::Header)
}

/// First selectable index of a menu (menus that open with a section header).
fn first_sel(menu: u8) -> usize {
    let rows = tweak_rows(menu);
    rows.iter().position(|(_, r)| !is_header(r)).unwrap_or(0)
}

fn next_sel(rows: &[(&'static str, Row)], sel: usize, dir: i32) -> usize {
    let n = rows.len() as i32;
    let mut i = sel as i32;
    for _ in 0..n {
        i = (i + dir).rem_euclid(n);
        if !is_header(&rows[i as usize].1) {
            return i as usize;
        }
    }
    sel
}

fn row_value(cfg: &Config, row: &Row) -> (String, Option<f32>) {
    match row {
        Row::Slider { get, min, max, fmt, .. } => {
            let v = get(cfg);
            let s = match fmt {
                0 => format!("{:.0}%", v * 100.0),
                2 => format!("{v:.0} DEG"),
                _ => format!("{v:.2}"),
            };
            (s, Some((v - min) / (max - min)))
        }
        Row::Toggle { get, .. } => ((if get(cfg) { "ON" } else { "OFF" }).into(), None),
        Row::Preset => (PRESETS[cfg.preset % PRESETS.len()].into(), None),
        Row::Link(_) | Row::Controls | Row::Trials | Row::Academy | Row::Garage | Row::Catalog | Row::GameMode(_) => (String::new(), None),
        Row::CarPick => ("<see below>".into(), None),
        Row::Fullscreen => ("TOGGLE".into(), None),
        Row::Header | Row::Back => (String::new(), None),
    }
}

struct Opts {
    gfx: Option<bool>,
    seed: Option<u64>,
    map: Option<usize>,
    fullscreen: Option<bool>,
    mute: bool,
    dvorak: bool,
    name: Option<String>,
    host: bool,
    join: Option<String>,
    mobile: bool,
    desktop_ui: bool,
    /// Which chassis to load. `None` means all-that-fit, which is what the
    /// game did before there were more chassis than SDF slots.
    bots: Option<crate::roster::Pick>,
}

fn parse_args() -> Opts {
    let mut o = Opts {
        gfx: None,
        seed: None,
        map: None,
        fullscreen: None,
        mute: false,
        dvorak: false,
        name: None,
        host: false,
        join: None,
            mobile: false,
        desktop_ui: false,
        bots: None,
};
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--gfx" => o.gfx = Some(args.next().as_deref() != Some("classic")),
            "--seed" => o.seed = args.next().and_then(|v| v.parse().ok()),
            "--map" => o.map = args.next().and_then(|v| v.parse().ok()),
            // --bots all | high | medium | low | broken | page:N | name,name
            //
            // An unparseable value EXITS rather than falling back to all: the
            // fallback loads 126 arbitrary chassis, which looks like the flag
            // worked and is the exact confusion roster::parse_pick refuses.
            "--bots" => {
                let v = args.next().unwrap_or_default();
                match crate::roster::parse_pick(&v) {
                    Some(p) => o.bots = Some(p),
                    None => {
                        eprintln!("--bots: don't understand {v:?}");
                        eprintln!("  try: all | high | medium | low | broken | page:N | name,name");
                        std::process::exit(2);
                    }
                }
            }
            "--fullscreen" => o.fullscreen = Some(true),
            "--windowed" => o.fullscreen = Some(false),
            "--mute" => o.mute = true,
            "--mobile" => o.mobile = true,
            "--desktop-ui" => o.desktop_ui = true,
            "--dvorak" => o.dvorak = true,
            "--name" => o.name = args.next(),
            "--host" => o.host = true,
            "--join" => o.join = args.next(),
            "--help" | "-h" => {
                println!("skydribble [--seed N] [--map 0..15] [--fullscreen|--windowed] [--mute]");
                println!("           [--gfx rays|classic] [--dvorak] [--name NAME] [--host] [--join FRIEND]");
                println!("  --gfx rays     ray-traced soft shadows + AO + reflections (default;");
                println!("                 software SDF tracing, tuned for M2 Ultra / M4 Max class GPUs)");
                println!("  --gfx classic  the flat v1.1 look, for small GPUs");
                println!("  --seed N       regenerate the world from seed N (persisted)");
                println!("  --map N        pick a map style 0..15 (persisted; see the MAPS menu)");
                println!("  --dvorak       apply the Dvorak home-row binding preset (persisted)");
                println!("  --name NAME    multiplayer display name / profile id (persisted)");
                println!("  --host         announce a session on the fleet store at launch");
                println!("  --join FRIEND  resolve FRIEND's session and dial in at launch");
                println!("  --mute         run silent (doesn't change saved volume)");
                println!();
                // A dozen headless drivers had accumulated and `--help` named
                // none of them, so the only way to find one was to read
                // main.rs. A tool nobody can discover is a tool nobody runs.
                println!("HEADLESS DRIVERS — no window, no GPU; each takes its own subcommand");
                println!("  --sim <what>   physics/AI sims: all, flip, land, shot, perf, ... (see DESIGN.md)");
                println!("  --campaign     the fifteen missions: slides, waves, objectives, progression");
                println!("  --raid         the raid loop: generator, foothold, encounters, loot");
                println!("  --arch <cmd>   the archaeological map: map, site, find, carry, relay,");
                println!("                 city, read, why, anchor, orbit, audit, emit-c");
                println!("  --terrain <c>  worlds and ground: grounds, profile, dunes, jump, drop, audit");
                println!("  --raidrun <id> a raid at 120 Hz: the event stream a renderer consumes");
                println!("  --finale       the red dwarf strike, and the three ways it refuses");
                println!("  --powers       the human interface: what each power costs its host");
                println!("  --keys         the live control scheme, overlaps and all");
                println!("  --brain [what]  squad commanders: doctrines, the LLM seam, the wire");
                println!("  --parity       invariants living at more than one call site");
                println!("  --aboard [len] walk the ship: rooms, consoles, sealed doors");
                println!("  --rescale      what a real-sized robot costs, per constant");
                println!("  --grandeur     the game's distances in body-heights against Destiny/DOTM/Interstellar");
                println!("  --hardpoints   [name] what each hull carries: guns, thrusters, cockpit, and a forward verdict");
                println!("  --guidance     the ball-chaser as a missile: pursuit vs PN");
                println!("  --assemble     part families, and the pairing score");
                println!("  --trainwatch   GAN training diagnostics: collapse and coverage");
                println!("  --bench [code] roll, keep and share authored frames");
                println!("  --voxel        critics for a voxel GAN: skeleton and part matching");
                println!("  --gravity      the well formula, checked against real physics");
                println!("  --frames       machine frames, designed from the planet");
                println!("  --space        kuiper as space: angular LOD, gravity, depth, the floor");
                println!("  --roster       the bot census, and what the distribution implies");
                println!("  --rip          finishers: the style of the kill is who you are");
                println!("  --crawler      the ambush from above: climb, perch, pounce");
                println!("  --board        human scale, Aiden's skateboard, and the scale gap");
                println!("  --stealth      sight vs spark resonance, and assassination");
                println!("  --grip         clinging: walls, undersides, bulk and ice");
                println!("  --amalgam      the gate-warden hunt: forge, hide, banish");
                println!("  --pale         the relic-keeper: testimony that is never evidence");
                println!("  --codex [entry] the 54-entry codex, annotated where evidence differs");
                println!("  --hazard       delayed consequences: the warp wave, spearcons");
                println!("  --combine      gestalts: which squads merge, and the fitness grid");
                println!("  --tune [group] every judgment call; `--tune guesses` for the unverified ones");
                println!("  --intel        Blackstar's Array logs, once his flagship falls");
                println!("  --menu         the station consoles, previewed as text frames");
                println!("  --gear         the bearer's sheet, the ship's vault, the clone gallery");
                println!("  --fold         audit a transformation: self-intersection, collapse, pops");
                println!("  --orient       diagnose or fix a bot pose's orientation");
                println!("  --parts        classify a bot pose's prims (leg/wheel/thruster/wing/body)");
                std::process::exit(0);
            }
            other => eprintln!("unknown arg {other:?} (try --help)"),
        }
    }
    o
}

/// Slot indices of the dynamic proxies inside this frame's rt shape list.
struct RtIds {
    ball: usize,
    car: usize,
    decor0: usize,
    remote0: usize,
    static_n: usize, // world shapes occupy RTS[0..static_n] in stable order
}

/// A BLACKSTAR CHASSIS: the same machine in several poses.
///
/// foldview exports the vehicle-to-robot fold as a strip of glTF poses. The
/// engine has no skeleton to drive, so the transform here is the strip played
/// back — `strip[0]` is the vehicle, `strip[last]` is the robot, and the
/// frames between are the fold. Every pose is an ordinary `model::Model`, so
/// nothing in the renderer, the hitbox or the SDF bake needed to learn a new
/// concept.
struct Chassis {
    strip: Vec<usize>,
    /// pose directory + the target length it was loaded at. The scale is
    /// per pose (the whole strip shares one metres-per-source-unit, which
    /// means different target lengths), so the morph has to reload with the
    /// SAME numbers or the blend would rescale as it goes.
    dirs: Vec<(PathBuf, f32)>,
    /// WALK PHASES at the robot end, empty when the chassis shipped without
    /// them. Separate from `strip` on purpose: a fold is indexed by t and a
    /// walk by phase, and one list cannot say which is which.
    walk: Vec<usize>,
    /// the walk poses' directories at the scale they were loaded, for the
    /// same reason `dirs` exists: the walk morph must reload with the SAME
    /// numbers or the blend rescales between strides
    walk_dirs: Vec<(PathBuf, f32)>,
    /// the fitted rig, for the catalogue's joint inspector. Empty when the
    /// chassis shipped without a joints.json.
    joints: Vec<joints::Joint>,
    /// SIZE MULTIPLIER, shipped beside the poses. A raid boss is 2.6x a bot,
    /// and it is data rather than a constant so adding one is an export
    /// rather than a recompile.
    scale: f32,
    /// does this chassis fight as a boss
    boss: bool,
    /// can this chassis fold vehicle<->robot at all. Defaults to true —
    /// every ordinary bot does. A boss whose asset was never authored
    /// with a coherent fold strip (a design choice, not a bug: not every
    /// raid boss is "always transformable in the same sense as bots")
    /// sets `"transforms": false` in its `chassis.json` to opt out,
    /// mirroring how `scale`/`boss` are already per-chassis data rather
    /// than a code change. Read by `transform()`, which refuses the fold
    /// request with the same "does not transform" toast a non-bot car
    /// already gets — the strip itself is untouched, so the catalogue's
    /// pose inspector, `chassis_scale`, `is_boss`, and everything else
    /// keyed off `cars.xform` still see every pose exactly as before;
    /// the only change is that the player-facing fold key stops doing
    /// anything for this chassis.
    transforms: bool,
    /// jets fly instead of driving: never grounded, lift and nose thrust
    flight: bool,
    /// hoverers/hovertanks ride a cushion instead of driving on wheels —
    /// same `chassis_traits::Traits` source as `flight`, so a chassis
    /// fleet.json calls "hoverer" or "hovertank" is actually simulated
    /// as one rather than falling entirely on the untuned geometry
    /// detector in `locomotion::decide` to notice a skirt. Before this
    /// field existed, `flight` correctly read the category trait
    /// (`traits.flies()`) but hover had no equivalent — `set_car_model`
    /// only ever declared Hover for the five hand-authored hovercars
    /// (`Model::hover.is_some()`), so every category-labelled hoverer
    /// or hovertank added since depended entirely on the geometry
    /// heuristic correctly reading its skirt, which its own doc comment
    /// admits is "reasoned from shapes, not fitted to a labelled set".
    floats: bool,
}

struct CarModels {
    models: Vec<model::Model>,
    names: Vec<&'static str>,
    /// What kind of vehicle it is, and one line on what it is like to drive.
    ///
    /// Forty-odd cars in a flat list of names is a catalogue you cannot
    /// navigate: the only way to learn what "Trifang" is was to drive it.
    /// The class says which family it belongs to, the line says why you
    /// would pick it over its neighbours.
    info: Vec<(&'static str, &'static str)>,
    active: usize,
    /// model index -> (chassis, position in that chassis' strip). Only bot
    /// poses appear here; an ordinary car is absent and simply cannot
    /// transform.
    xform: HashMap<usize, (usize, usize)>,
    chassis: Vec<Chassis>,
    /// what `[` and `]` step through. Bot fold frames are deliberately NOT
    /// in here — a chassis appears once, as its vehicle pose, and the robot
    /// is reached by transforming rather than by cycling past six
    /// half-folded copies of it.
    cycle: Vec<usize>,
}

/// Which catalogue family an imported model belongs in.
///
/// `catalog_menu` already groups by `info[i].0` and inserts a header wherever
/// it changes — but `cars.info` was only ever pushed for the built-in hover
/// cars, so every imported model got `""` from `info.get(i)` and the whole
/// import block collapsed into one ungrouped run. The grouping was never
/// broken; it was starved.
///
/// Four families, ordered so the ones you want are at the top and the ones
/// you cannot ship are at the bottom:
///
///   FORGE  generated originals (tools/forge) — no rights holder
///   FIXED  re-exported with texture_export v13+, which merges every piece of
///          a multi-file OBJ; v9 took whichever piece os.listdir returned
///          first and dropped the rest (measured: 8-9% of the model kept)
///   OLD    imported, not re-exported
///   IP     filename matches a rights holder — visible, deliberately, rather
///          than scattered through eleven hundred entries
fn model_family(name: &str, forge: &HashSet<String>, fixed: &HashSet<String>) -> &'static str {
    // the marks live in vocab.rs, the one file the licensed-vocabulary census
    // exempts: listed here, each raised its count by one and failed the census
    use vocab::IP_MARKS;
    let n = name.to_ascii_lowercase().replace([' ', '-'], "_");
    // IP wins over everything: a model that is both re-exported and
    // rights-holder material is still unshippable.
    if IP_MARKS.iter().any(|m| n.contains(m)) {
        return "IP";
    }
    // The directory prefix is the reliable signal: strips live in
    // assets/bots/forge_<name>/ while the GLB set is keyed on bare stems, so
    // matching only the set would miss every strip. Both are checked.
    if name.starts_with("forge_") || forge.contains(name)
        || forge.contains(name.trim_start_matches("forge_")) {
        return "FORGE";
    }
    if fixed.contains(name) { return "FIXED"; }
    "OLD"
}

impl CarModels {
    fn class_of(&self, i: usize) -> &'static str {
        self.info.get(i).map(|x| x.0).unwrap_or("")
    }
    fn blurb_of(&self, i: usize) -> &'static str {
        self.info.get(i).map(|x| x.1).unwrap_or("")
    }
}

struct Game {
    /// Chassis name -> fleet.json category, for the trait vector.
    ///
    /// Held rather than re-read: load_categories scans fleet.json, and the
    /// morph is rebuilt every time the active car changes.
    bot_categories: std::collections::HashMap<String, String>,
    gpu: render::Gpu,
    /// the ship interior, and the mesh slot its hull was uploaded to
    aboard: aboard::Aboard,
    /// the room the player was in last frame, so entering one is an EVENT
    /// rather than a state the HUD reprints sixty times a second
    last_room: Option<&'static str>,
    /// which doorways were carved last frame, so the hull is re-uploaded when
    /// a door moves and not every frame
    door_mask: u32,
    world: World,
    car: Car,
    ball: Ball,
    cam: camera::Camera,
    cars: CarModels,
    /// the fold, as a state machine (see xform.rs)
    xf: xform::Xform,
    /// CPU-side strip vertices for the active chassis, so the fold can be
    /// BLENDED rather than stepped. Only the active chassis is kept — the
    /// whole roster would be ~130 MB of vertices resident for no reason.
    morph: Option<xform::Morph>,
    /// THE WALK, BLENDED — (chassis, blend model, morph). The walk strip
    /// used to reach the screen as a flipbook: `walk_frame` snapped whole
    /// poses by phase. The morph is the walk strip with pose 0 appended
    /// again at the end (loop closure: a stride wraps), aligned by
    /// `align::unify` exactly as the fold is, sampled at the locomotion
    /// phase. It writes into a DEDICATED model cloned from the strip —
    /// never into a walk pose itself — because field bots flip through
    /// those poses at their own offsets and would otherwise display the
    /// player's half-stride. Only the player blends; a shared mesh buffer
    /// cannot hold every bot's phase at once, so minions keep the flipbook.
    walk_morph: Option<(usize, usize, xform::Morph)>,
    /// which chassis the last walk-morph BUILD was attempted for — kept
    /// separately from the morph so a refused strip is remembered as
    /// refused. Without it, a chassis whose walk `unify` rejects would
    /// re-load five glTF poses from disk EVERY TICK while standing.
    walk_morph_tried: Option<usize>,
    /// 0..1 health, which is what picks the fold's personality
    health: f32,
    /// the weapons module owns the lock; the camera borrows it
    wep: weapons::Weapons,
    /// autotarget on: ball cam follows an opponent instead of the ball
    autotarget: bool,

    /// how the current chassis stays up, when it flies at all
    lift: Option<lift::Lift>,
    /// KEY REPEAT FOR MENUS. Held arrows step once and then stop, which
    /// makes a forty-row list a forty-press list. `repeat_key` is what is
    /// held, `repeat_t` counts down to the next fire — long for the first
    /// (so a single tap is never two) and short after.
    repeat_key: Option<Key>,
    repeat_t: f32,
    /// SEA LEVEL for a water world, in metres. None means dry land, which
    /// is every map that is not TIDEMAW — kept as an Option rather than a
    /// bool so the depth is available to the lighting rather than just the
    /// fact of being wet.
    sea_level: Option<f32>,
    /// which joint the catalogue is pointing at
    joint_sel: usize,
    /// step only through the misparented ones — 306 joints is scrolling,
    /// not inspection, and the faults are what we are looking at
    joint_only_bad: bool,
    /// space, as a tap-or-hold whose meaning the form decides
    space: chord::Chord,
    /// right mouse: glance when held, latch when tapped
    sight: chord::Sight,
    /// walk / run / boost / fall, for robot mode
    loco: anim::Locomotion,
    /// the pose it produced this frame
    loco_pose: anim::Pose,
    /// the 22-bone rig the gait drives (rigmotion.rs): what the limb channels
    /// were waiting for
    rig: rigmotion::RigMotion,
    rig_frame: Option<rigmotion::Frame>,
    ball_model: model::Model,
    decor_model: model::Model,
    fx: fx::Fx,
    snd: sound::Sound,
    pad: gamepad::Pad,
    last_pad: gamepad::PadState,
    save: save::Save,
    binds: Bindings,
    net: net::Net,
    cfg: Config,
    applied_scale: f32,
    binds_to_settings: bool,
    cursor: (f32, f32),
    cursor_moved: bool,
    prev_cursor_vis: bool,
    capture_guard: f32,
    prev_cpos: Vec3,
    prev_crot: Quat,
    prev_bpos: Vec3,
    prev_brot: Quat,
    goals: u64,
    game_mode: u8,      // 0 free, 1 sparta 50v50, 2 sparta 300
    last_vsync: bool,
    roll_mix_s: f32, // smoothed slide->roll blend (kills regime flicker)
    scores: [u32; 2],   // sparta team score: [blue, orange]
    tpad: touch::TouchPad, // on-screen controls (handhelds)
    last_ivp: Mat4,       // for resolving taps into world points
    last_scale: f32,      // render-scale watcher
    tap_drive: (f32, f32), // throttle/steer from the drive-to target
    gyro: gyro::Gyro,
    /// bounding spheres of the terrain chunks, for culling the ground
    tchunks: Vec<(Vec3, f32)>,
    /// the academy lesson in progress, and which have been passed
    lesson: Option<academy::Session>,
    passed: academy::Passed,
    lesson_sel: usize,
    /// tricks noted since the lesson began (the verifier)
    lesson_tricks: Vec<replay::Trick>,
    /// always-on replay buffer, plus the moments worth keeping
    rec: replay::Recorder,
    moments: Vec<replay::Moment>,
    /// playing a replay: (which moment, time into it)
    watching: Option<(usize, f32)>,
    /// where the car last hit something, in its own frame, and how long ago
    last_hit: (Vec3, f32),
    /// the garage: skin, wheels, trail, goal explosion
    look: custom::Loadout,
    look_sel: usize,
    /// the trial being attempted, if any
    trial: Option<trials::Run>,
    /// best times, and the ghost of the best run for the current trial
    bests: trials::Bests,
    ghost: Vec<(Vec3, Quat)>,
    trial_sel: usize,
    /// distance travelled through portals this run (a trial metric)
    portal_m: f32,
    style: style::Analyzer,
    // touch menu gesture state: where the finger went down, which row it
    // grabbed, whether it has moved far enough to count as a drag
    /// menu scroll position in ROWS (fractional: the finger drags content)
    mscroll: f32,
    mfling: f32,
    mscroll_at_down: f32,
    tdown: Option<(f32, f32)>,
    tdrag_row: Option<usize>,
    tscrub: bool,
    tmoved: f32,
    wx_rain: f32,    // MONKEY RUN weather intensity (rain+storm)
    in_steer: f32,   // input-ease state: slight temporal buildup on
    in_yaw: f32,     // steering and rolling (the detent feel)
    in_roll: f32,
    analog: analog::Analog,
    bots: Vec<bots::Bot>,
    friend: String,
    /// The Online screen's live menu (first `menu::Menu` consumer). Built on
    /// open, dropped on close; sel/scroll live inside it, so Mode::Online's
    /// `sel` remains only as a legacy mirror for the shared draw tables.
    online_menu: Option<menu::Menu>,
    toast: String,
    toast_t: f32,
    /// counts down to the next beacon pulse during a raid Assault —
    /// `RaidFlow::live_beacons` exists, nothing drew them, so this is
    /// what actually makes an encounter site visible in the world
    beacon_fx_t: f32,
    mode: Mode,
    menu_from_pause: bool,
    /// the ship's consoles, when the player is standing at one. The
    /// shell has no live `ship::Ship` yet, so the deck opens under an
    /// assumed Cruise phase — the phase gate is real and tested in
    /// `deck.rs`; wiring a hull into the loop is its own seam.
    deck: Option<deck::Deck>,
    /// a mission being FLOWN — brief, drop, engagement, debrief. The
    /// consoles accept the order; this is what happens instead of
    /// `run_order`'s instant resolve when the destination can be dropped
    /// on. See `sortie.rs` for what is wired and what is still canned.
    sortie: Option<sortie::Sortie>,
    /// a warden raid being FLOWN — the Wardeck's order, walked phase by
    /// phase, each with its own arena, brief, and drop. Mutually exclusive
    /// with `sortie`: the shell flies one thing at a time.
    raid: Option<raidflow::RaidFlow>,
    /// the forge, when lit: a worker thread mid-call to the model, its
    /// answer due on this channel. Polled with `try_recv` in the update —
    /// the frame never waits on the network (`raidforge`).
    forge_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    /// live projectiles — the thrown-mass and launched-charge verbs fly
    /// through `shots.rs` now instead of resolving in the trigger pull
    shots: shots::Shots,
    /// shots fired from the saddle so far — walks the muzzle pair
    saddle_shots: u32,
    /// MOTION WEIGHT (C13-2c): seconds of frozen simulation left after a
    /// heavy impact. The frame still renders; the world holds. Dark of
    /// the Moon's hits land because the cut holds on them for a beat.
    hitstop: f32,
    /// HOSTILE FIRE: the field's shots, in their own pool so the two sets
    /// of marks cannot be confused — the player's shots test against the
    /// bots, these test against the player. Same pool type, same physics,
    /// same medium; only the target list differs.
    hostile_shots: shots::Shots,
    /// what the shots fly through, set with the map: vacuum carries,
    /// air drags. Subsea is the named next medium (needs the sea level).
    ambient: ballistics::Ambient,
    /// this world's surface gravity for the shot pool, m/s²
    world_g: f32,
    /// which hull class is parked, indexing `aboard::FLEET`. Cycled with
    /// the shifted brackets.
    ship_class: usize,
    /// how the active chassis moves, and why (`locomotion`). Held so the
    /// HUD and the catalogue can show the reasoning rather than the
    /// verdict alone.
    loco_mode: locomotion::Decision,
    /// seconds left in the blade swing; 0 = idle. Drives the pose's arm
    /// channel and lands the arc at mid-swing.
    swing: f32,
    /// the player's own per-section damage (`sections.rs`). Bots carry
    /// theirs on `Bot`; the player's lives here for the same reason —
    /// so a body and its wounds cannot drift apart.
    body: sections::Body,
    grime: grime::Grime,
    /// the action bar, live during play — loaded once at boot rather
    /// than through a Deck console, since quick-select has to work
    /// without opening a menu. Firing whatever the active slot holds
    /// is not wired yet (a real seam, not an oversight): this pass
    /// only gets selection live, matching the number keys freed from
    /// the old car-picker.
    hotbar: hotbar::Hotbar,
    held: HashSet<Key>,
    held_mouse: HashSet<MouseButton>,
    jump_edge: bool,
    flip_vis: bool,
    time: f32,
    acc: f32,
    fps: f32,
    /// present-rate accounting: window, last counts, and drops in that window
    fps_win: f32,
    fps_presented: u64,
    fps_dropped: u64,
    frame_drops: u32,
    // session stats
    rings: u64,
    car_rings: u64,
    combo: u32,
    combo_t: f32,
    air_t: f32,
    air_link: f32,
    /// throttle for the CLIMB toast, same shape as the C engine's
    /// `g_climb_msg_cd` — `ev.climbing` fires every physics step while
    /// climbing (dozens of times a second), so this is what keeps the
    /// toast from spamming rather than physics gating it itself.
    climb_msg_cd: f32,
    /// WIRING DEBT from docs/PLAN.md's own Phase 0 list: "patrol -- a
    /// game mode that calls update". Wired here for the SAFE half only
    /// (see the tick site's own comment) -- the Surge/Hold/Onslaught
    /// combat-event half of patrol::update's output is deliberately
    /// left unconsumed, since surfacing "SURGE incoming" with nothing
    /// actually spawning to fight would be worse than not showing it.
    patrol: patrol::Patrol,
}

/// Impulse (delta-v, m/s) to hp damage. ONE conversion for everything the
/// player shoots — the bots' section damage and the sortie engagement's
/// strike both go through it, so a railshot is worth the same wherever it
/// lands and the number can be retuned in one place.
const IMPULSE_TO_DAMAGE: f32 = 0.02;

/// What a round of each element looks like in the air. The forms' own
/// muzzle colours (`weapons::TABLE`) are per FORM; a shot in flight only
/// knows its element, so this is the element's colour: the same hues,
/// pushed hotter for an emissive tracer.
fn element_glow(e: weapons::Element) -> [f32; 4] {
    match e {
        weapons::Element::Fire => [1.0, 0.55, 0.18, 0.95],
        weapons::Element::Ice => [0.7, 0.9, 1.0, 0.95],
        weapons::Element::Water => [0.4, 0.7, 1.0, 0.95],
        weapons::Element::Air => [0.85, 0.95, 0.85, 0.95],
        weapons::Element::Electric => [0.6, 0.75, 1.0, 0.95],
        weapons::Element::Rock => [0.85, 0.72, 0.45, 0.95],
        weapons::Element::Vine => [0.55, 0.9, 0.5, 0.95],
        _ => [1.0, 0.96, 0.85, 0.95],
    }
}

/// Which pose a field hostile shows: a chassis from the loaded roster
/// (deterministic per slot, never a boss, only one that folds), and the
/// frame of its fold strip that matches the foe's form — vehicle end,
/// robot end, or a frame between while it is mid-fold. Without bot
/// assets (the plain sandbox) it falls back to the three cars.
fn hostile_pose(cars: &CarModels, foe: &field::Foe) -> usize {
    let cands: Vec<usize> = cars
        .chassis
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.boss && c.transforms && c.strip.len() >= 2)
        .map(|(i, _)| i)
        .collect();
    if cands.is_empty() {
        return foe.slot % cars.models.len().max(1).min(3);
    }
    // a Corrupted body wears the same chassis every time it appears so
    // the wave reads; everyone else cycles the roster by slot
    let ch = &cars.chassis[cands[foe.slot % cands.len()]];
    let last = ch.strip.len() - 1;
    let k = if foe.fold_t > 0.0 {
        let frac = foe.fold_frac();
        let toward_robot = foe.fold_to == field::Form::Robot;
        let f = if toward_robot { frac } else { 1.0 - frac };
        ((f * last as f32).round() as usize).min(last)
    } else if foe.form == field::Form::Vehicle {
        0
    } else {
        last
    };
    ch.strip[k]
}

/// One blade swing, trigger to follow-through, seconds. The arc lands at
/// the midpoint. A GUESS tuned by feel once it is on a screen.
const SWING_S: f32 = 0.38;

/// A fresh seed per arrival, derived from the destination so the same
/// world is the same place twice. ONE derivation, shared by `depart_to`
/// and `begin_sortie` — a destination that generates differently depending
/// on why you went there would be the two-answers-to-one-question mistake
/// wearing a map.
fn dest_seed(id: &str) -> u64 {
    id.bytes().fold(0x9E37u64, |a, b| a.wrapping_mul(131).wrapping_add(b as u64))
}

fn assets_dir() -> PathBuf {
    let local = PathBuf::from("assets");
    if local.exists() {
        local
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")
    }
}

/// A little procedural window icon: white ball, gold ring, pastel sky.
fn make_icon() -> Option<winit::window::Icon> {
    const S: i32 = 48;
    let mut rgba = Vec::with_capacity((S * S * 4) as usize);
    for y in 0..S {
        for x in 0..S {
            let dx = x as f32 - S as f32 * 0.5 + 0.5;
            let dy = y as f32 - S as f32 * 0.5 + 0.5;
            let r = (dx * dx + dy * dy).sqrt();
            let ang = dy.atan2(dx);
            let (cr, cg, cb, ca) = if r < 15.0 {
                if (ang * 2.5).sin() > 0.55 && r > 4.0 {
                    (255, 150, 60, 255)
                } else {
                    (245, 245, 250, 255)
                }
            } else if r < 18.5 {
                (250, 200, 60, 255)
            } else if r < 23.0 {
                (143, 211, 254, 255)
            } else {
                (0, 0, 0, 0)
            };
            rgba.extend_from_slice(&[cr, cg, cb, ca]);
        }
    }
    winit::window::Icon::from_rgba(rgba, S as u32, S as u32).ok()
}

/// Sample the terrain into the GPU heightmap the ray tracer marches.
/// Bake the per-cell static candidate lists ("bake and project"): for each
/// XZ cell, the nearest RC_K world shapes that any fragment ray starting in
/// that cell could plausibly reach. Animated shapes get their travel added.
/// Bake every loaded car's vertex cloud into the reflection SDF atlas:
/// per-voxel distance to the nearest surface point, byte-encoded. This is
/// what makes mirrors show the EXACT model instead of a box silhouette.
fn bake_car_sdfs(models: &mut [model::Model]) -> Vec<u8> {
    use render::{csdf_extent, CSDF_H, CSDF_MAXM, CSDF_SLAB, CSDF_TILES, CSDF_W};
    let (aw, ah, ad) = csdf_extent();
    let mut out = vec![255u8; (aw * ah * ad) as usize];
    for (mi, m) in models.iter_mut().enumerate().take(CSDF_MAXM as usize) {
        if m.cloud.is_empty() {
            continue;
        }
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for &p in &m.cloud {
            mn = mn.min(p);
            mx = mx.max(p);
        }
        let h = mn.abs().max(mx.abs()) + Vec3::splat(0.10);
        m.sdf_half = h;
        // bucket the cloud so each voxel tests a handful of points, not all
        // of them (this is what makes a 2600-point cloud affordable)
        let gn = 12usize;
        let cell = Vec3::new(2.0 * h.x / gn as f32, 2.0 * h.y / gn as f32, 2.0 * h.z / gn as f32);
        let idx_of = |p: Vec3| -> (usize, usize, usize) {
            (
                (((p.x + h.x) / cell.x) as isize).clamp(0, gn as isize - 1) as usize,
                (((p.y + h.y) / cell.y) as isize).clamp(0, gn as isize - 1) as usize,
                (((p.z + h.z) / cell.z) as isize).clamp(0, gn as isize - 1) as usize,
            )
        };
        let mut buckets: Vec<Vec<(Vec3, Vec3)>> = vec![Vec::new(); gn * gn * gn];
        for (ci, &c) in m.cloud.iter().enumerate() {
            let (i, j, k) = idx_of(c);
            let nrm = m.cloud_n.get(ci).copied().unwrap_or(Vec3::Y);
            buckets[(k * gn + j) * gn + i].push((c, nrm));
        }
        for z in 0..CSDF_SLAB {
            for y in 0..CSDF_H {
                for x in 0..CSDF_W {
                    let p = Vec3::new(
                        ((x as f32 + 0.5) / CSDF_W as f32 * 2.0 - 1.0) * h.x,
                        ((y as f32 + 0.5) / CSDF_H as f32 * 2.0 - 1.0) * h.y,
                        ((z as f32 + 0.5) / CSDF_SLAB as f32 * 2.0 - 1.0) * h.z,
                    );
                    let mut d2 = f32::MAX;
                    let mut near_n = Vec3::Y;
                    let mut near_p = Vec3::ZERO;
                    let (bi, bj, bk) = idx_of(p);
                    let mut ring = 1isize;
                    while ring <= gn as isize {
                        for dk in -ring..=ring {
                            for dj in -ring..=ring {
                                for di in -ring..=ring {
                                    // only the shell of the ring is new work
                                    if di.abs() != ring && dj.abs() != ring && dk.abs() != ring {
                                        continue;
                                    }
                                    let (i2, j2, k2) = (bi as isize + di, bj as isize + dj, bk as isize + dk);
                                    if i2 < 0 || j2 < 0 || k2 < 0 || i2 >= gn as isize || j2 >= gn as isize || k2 >= gn as isize {
                                        continue;
                                    }
                                    for &(c, cn) in &buckets[((k2 as usize) * gn + j2 as usize) * gn + i2 as usize] {
                                        let dd = p.distance_squared(c);
                                        if dd < d2 {
                                            d2 = dd;
                                            near_n = cn;
                                            near_p = c;
                                        }
                                    }
                                }
                            }
                        }
                        // one ring past the first hit is enough for a cloud
                        if d2 < f32::MAX && ring >= 2 {
                            break;
                        }
                        ring += 1;
                    }
                    // SIGNED: distance to the nearest surface point, negative
                    // when we are behind that point's outward normal. Without
                    // the sign this was a hollow SHELL — rays passed through
                    // the car and fell back on the bounding box, which is
                    // exactly the "bubble" in the mirror.
                    let dist = d2.sqrt();
                    let d = if (p - near_p).dot(near_n) < 0.0 { -dist } else { dist } - 0.012;
                    // TILED WRITE, matching shader.wgsl's tiled read. A model
                    // occupies one tile of a TILES x TILES grid in (x, y) and
                    // only the leftover index advances depth. Writing a depth
                    // stack while the shader reads tiles would sample another
                    // model's field — geometry that renders and is wrong.
                    let m = mi as u32;
                    let (tx, ty, tz) = (m % CSDF_TILES,
                                        (m / CSDF_TILES) % CSDF_TILES,
                                        m / (CSDF_TILES * CSDF_TILES));
                    let ax = tx * CSDF_W + x;
                    let ay = ty * CSDF_H + y;
                    let az = tz * CSDF_SLAB + z;
                    let idx = (((az * (CSDF_H * CSDF_TILES)) + ay) * (CSDF_W * CSDF_TILES)
                               + ax) as usize;
                    out[idx] = (((d + 0.6) / 2.4).clamp(0.0, 1.0) * 255.0) as u8;
                }
            }
        }
    }
    out
}

fn bake_rcells(world: &world::World) -> Vec<u32> {
    use render::{RC_DIM, RC_K, RC_STRIDE};
    let mut travel = vec![0.0f32; world.shapes.len()];
    for a in &world.anims {
        match *a {
            world::Anim::Orbit { shape, r, .. } => travel[shape] = r + 8.0,
            world::Anim::Slide { shape, amp, .. } => travel[shape] = amp + 6.0,
            world::Anim::Door { shape, amp, .. } => travel[shape] = amp + 4.0,
            world::Anim::Spin { shape, scale, .. } => travel[shape] = scale.max_element() + 4.0,
            world::Anim::Swing { shape, len, amp, .. } => travel[shape] = len * amp.sin().abs() + 6.0,
        }
    }
    let span = world::TG_HALF * 2.0;
    let cell = span / RC_DIM as f32;
    let half_diag = cell * 0.7071;
    let mut out = vec![0u32; RC_DIM * RC_DIM * RC_STRIDE];
    let mut cand: Vec<(f32, u32)> = Vec::new();
    for cz in 0..RC_DIM {
        for cx in 0..RC_DIM {
            let cc = glam::Vec2::new(
                -world::TG_HALF + (cx as f32 + 0.5) * cell,
                -world::TG_HALF + (cz as f32 + 0.5) * cell,
            );
            cand.clear();
            let mut si = 0u32;
            for (wi, sh) in world.shapes.iter().enumerate() {
                if sh.cut || world.is_mover(wi) {
                    continue; // movers live in the dynamic suffix instead
                }
                let bound = match sh.sdf {
                    world::Sdf::None => 0.0,
                    world::Sdf::Sphere { r } => r,
                    world::Sdf::Torus { big, tube } => big + tube,
                    world::Sdf::Hex { r, h } => (r * r + h * h).sqrt(),
                    world::Sdf::Capsule { r, half } => r + half,
                    world::Sdf::RBox { half, .. } => half.length(),
                    world::Sdf::Shell { r, half, thick } => ((r + thick) * (r + thick) + half * half).sqrt(),
                    world::Sdf::TubeTorus { big, tube, thick } => big + tube + thick,
                    world::Sdf::RingDeck { big, wide, .. } => big + wide,
                } + travel[wi];
                let d = glam::Vec2::new(sh.pos.x, sh.pos.z).distance(cc) - bound - half_diag;
                if d < 135.0 {
                    cand.push((d, si));
                }
                si += 1;
            }
            cand.sort_by(|a, b| a.0.total_cmp(&b.0));
            let base = (cz * RC_DIM + cx) * RC_STRIDE;
            let n = cand.len().min(RC_K);
            out[base] = n as u32;
            for (k, &(_, id)) in cand.iter().take(n).enumerate() {
                out[base + 1 + k] = id;
            }
        }
    }
    out
}

fn bake_heightmap(gpu: &render::Gpu, seed: u64, style: &world::MapStyle, origin: Vec3) {
    let n = render::HMAP_SIZE as usize;
    let half = render::HMAP_HALF;
    let mut data = vec![0f32; n * n];
    for j in 0..n {
        let z = (j as f32 / (n - 1) as f32 * 2.0 - 1.0) * half;
        for i in 0..n {
            let x = (i as f32 / (n - 1) as f32 * 2.0 - 1.0) * half;
            // world coordinates, the same the CPU grid sampled, or the
            // GPU's ground disagrees with the one you drive on after a page
            data[j * n + i] = world::terrain_h_s(x + origin.x, z + origin.z, seed, style.amp, style.freq);
        }
    }
    gpu.set_heightmap(&data);
}

/// Rebuild every terrain chunk and hand back their bounding spheres.
fn terrain_chunks(gpu: &mut render::Gpu, w: &world::World) -> Vec<(Vec3, f32)> {
    let nc = world::TERRAIN_CHUNKS;
    let n = (world::TG_N - 1) as u32;
    let per = n / nc;
    let h = |x: f32, z: f32| w.ground_h(x, z);
    let mut bounds = Vec::with_capacity((nc * nc) as usize);
    for cj in 0..nc {
        for ci in 0..nc {
            let i0 = ci * per;
            let i1 = if ci + 1 == nc { n } else { (ci + 1) * per };
            let j0 = cj * per;
            let j1 = if cj + 1 == nc { n } else { (cj + 1) * per };
            let (m, c, r) = meshgen::terrain_chunk(world::TG_HALF, n, i0, i1, j0, j1, &h);
            gpu.replace_mesh(world::M_TERRAIN_0 + (cj * nc + ci) as usize, &m);
            bounds.push((c, r));
        }
    }
    bounds
}

fn terrain_mesh(w: &world::World) -> meshgen::MeshData {
    // node-identical with the physics grid: what you see is what you drive
    meshgen::terrain(world::TG_HALF, (world::TG_N - 1) as u32, |x, z| w.ground_h(x, z))
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    if argv.iter().any(|a| a == "--sim") {
        // headless AI sim: pure physics, no window, no GPU, no audio
        sim::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--menu") {
        // the station consoles and their motion, previewed as text frames
        menusim::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--gear") {
        // the bearer's sheet, the ship's vault, the clone gallery
        gearsim::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--helm") {
        helmai::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--holo") {
        holomap::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--fleet") {
        fleet::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--fold") {
        // audit a transformation: self-intersection, collapse, pops
        foldsim::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--orient") {
        // diagnose or fix a bot pose's orientation; see orient.rs
        orientsim::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--parts") {
        // classify a bot pose's prims (leg/wheel/thruster/wing/body);
        // see partdetect.rs -- read-only, no --commit, nothing to write yet
        partsim::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--intel") {
        // Blackstar's own Array logs. They live in the archaeology tables
        // now (see archive.rs); this flag stays as the shortcut it always
        // was -- read the three readings together, the way he first did.
        let (sv, _, _) = save::load();
        let camp = campaign::CampaignLog::from_line(&sv.missions);
        let dig = provenance::Dig::from_line(&sv.finds, archive::FINDINGS);
        let logs: Vec<_> = archive::FINDINGS
            .iter()
            .filter(|f| f.gate == Some(archive::ARRAY_LOG_GATE))
            .collect();
        if !camp.is_cleared(archive::ARRAY_LOG_GATE) {
            println!("nothing here yet -- these are Blackstar's own logs; board his ship first");
        } else {
            println!("THE ARRAY LOGS -- recovered aboard Blackstar's own ship\n");
            for f in &logs {
                let mark = if dig.has(f.id) { "*" } else { " " };
                println!("{mark} [{}]\n    {}\n", f.id, f.blurb);
            }
            println!("(* already in the dig -- `--arch find <id>` to record the rest)");
        }
        return;
    }
    if argv.iter().any(|a| a == "--brain") {
        brain::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--parity") {
        parity::run();
        return;
    }
    if argv.iter().any(|a| a == "--aboard") {
        aboard::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--rescale") {
        rescale::run();
        return;
    }
    if argv.iter().any(|a| a == "--grandeur") {
        grandeur::run();
        return;
    }
    if argv.iter().any(|a| a == "--hardpoints") {
        hardpoints::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--guidance") {
        guidance::run();
        return;
    }
    if argv.iter().any(|a| a == "--assemble") {
        assembler::run();
        return;
    }
    if argv.iter().any(|a| a == "--trainwatch") {
        trainwatch::run();
        return;
    }
    if argv.iter().any(|a| a == "--bench") {
        bench::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--voxel") {
        voxel::run();
        return;
    }
    if argv.iter().any(|a| a == "--gravity") {
        gravity::run();
        return;
    }
    if argv.iter().any(|a| a == "--frames") {
        machine::run();
        return;
    }
    if argv.iter().any(|a| a == "--space") {
        space::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--circuit") {
        circuit::run();
        return;
    }
    if argv.iter().any(|a| a == "--overload") {
        overload::run();
        return;
    }
    if argv.iter().any(|a| a == "--flow") {
        flow::run();
        return;
    }
    if argv.iter().any(|a| a == "--boarding") {
        boarding::run();
        return;
    }
    if argv.iter().any(|a| a == "--roomplan") {
        roomplan::run();
        return;
    }
    if argv.iter().any(|a| a == "--circuit-map") {
        circuit_map::run();
        return;
    }
    if argv.iter().any(|a| a == "--circuit-layout") {
        circuit_layout::run();
        return;
    }
    if argv.iter().any(|a| a == "--circuit-wave") {
        circuit_wave::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--circuit-match") {
        circuit_match::run();
        return;
    }
    if argv.iter().any(|a| a == "--roster") {
        census::run();
        return;
    }
    if argv.iter().any(|a| a == "--rip") {
        rip::run();
        return;
    }
    if argv.iter().any(|a| a == "--crawler") {
        crawler::run();
        return;
    }
    if argv.iter().any(|a| a == "--board") {
        board::run();
        return;
    }
    if argv.iter().any(|a| a == "--stealth") {
        stealth::run();
        return;
    }
    if argv.iter().any(|a| a == "--grip") {
        cling::run();
        return;
    }
    if argv.iter().any(|a| a == "--amalgam") {
        amalgam::run();
        return;
    }
    if argv.iter().any(|a| a == "--pale") {
        palevector::run();
        return;
    }
    if argv.iter().any(|a| a == "--codex") {
        codex::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--hazard") {
        // delayed consequences: the warp wave and the lieutenants' spears
        hazard::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--combine") {
        // gestalts: who merges with whom, and how big the result is
        combine::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--raidrun") {
        // the raid renderer's contract, stepped at 120 Hz
        raidrun::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--finale") {
        // Billy holds the star short; Photon Man aims it
        finale::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--powers") {
        // the human interface, and what each of them has left
        powers::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--keys") {
        // overlapping binds, and keys the shell steals before the table sees them
        input::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--tune") {
        // every judgment call, guesses first
        tuning::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--terrain") {
        // route rows -> terrain::Ground, and the generator run headless
        mapsim::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--arch") {
        // the archaeological map: strata, findings, the Asteri arithmetic
        archsim::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--vo") {
        // the voice bank: lines lifted from the screenplays, barks authored,
        // ids derived, scripts written for the booth
        vo::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--campaign") {
        // headless campaign: slides, waves, objective machines, progression
        campsim::run(&argv);
        return;
    }
    if argv.iter().any(|a| a == "--raid") {
        // headless raid loop: generator -> foothold -> encounters -> loot,
        // narrated with its reasoning
        raidsim::run(&argv);
        return;
    }
    env_logger::init();
    let opts = parse_args();
    let (mut sv, mut binds, mut cfg) = save::load();
    let hotbar = hotbar::Hotbar::from_line(&sv.hotbar);
    if let Some(seed) = opts.seed {
        sv.seed = seed;
    }
    if let Some(m) = opts.map {
        sv.map = m % world::STYLE_COUNT;
    }
    if let Some(fs) = opts.fullscreen {
        sv.fullscreen = fs;
    }
    if let Some(n) = &opts.name {
        sv.name = n.clone();
    }
    // ---- platform detection: phones get a different game ----
    let plat = platform::detect();
    let fresh = !save::exists();
    // Re-tune when the hardware CHANGES, not only on a first run: a save
    // copied onto a phone (or a phone that had ever launched the game
    // before) used to keep desktop graphics forever, which is the single
    // biggest reason mobile ran badly.
    let pcode = plat.profile as u8;
    if plat.profile == platform::Profile::Handheld && (fresh || opts.mobile || sv.tuned_profile != pcode) {
        plat.profile.apply(&mut cfg);
    }
    sv.tuned_profile = pcode;
    if opts.desktop_ui {
        cfg.touch.enabled = false;
    }
    if opts.dvorak {
        binds = Bindings::dvorak();
    }
    if let Some(r) = opts.gfx {
        cfg.video.rays = r;
    }

    println!("SKYDRIBBLE — controls live on the pause screen (ESC) and are fully rebindable");
    println!("(pause -> ENTER; keys, mouse buttons and controller buttons all bind).");

    let event_loop = EventLoop::new().unwrap();
    let mut wb = WindowBuilder::new()
        .with_title("SKYDRIBBLE")
        .with_inner_size(LogicalSize::new(1400.0, 800.0))
        .with_window_icon(make_icon());
    if sv.fullscreen {
        wb = wb.with_fullscreen(Some(Fullscreen::Borderless(None)));
    }
    let window = Arc::new(wb.build(&event_loop).unwrap());

    let mut gpu = render::Gpu::new(window.clone());

    // ---- mesh registry: order must match world::M_* constants -------------
    let seed = sv.seed;
    let style0 = world::style(sv.map);
    let ids = [
        gpu.add_mesh(&meshgen::sphere(22, 34)),
        gpu.add_mesh(&meshgen::boxx()),
        gpu.add_mesh(&meshgen::torus(0.07, 56, 18)),
        gpu.add_mesh(&meshgen::torus(0.16, 48, 20)),
        gpu.add_mesh(&meshgen::capsule(world::CAP_SHORT_HH, 8, 20)),
        gpu.add_mesh(&meshgen::capsule(world::CAP_TALL_HH, 8, 20)),
        gpu.add_mesh(&meshgen::pad_disc(28)),
        gpu.add_mesh(&meshgen::terrain(world::TG_HALF, 2, |_, _| 0.0)), // real one lands post-generate
        gpu.add_mesh(&meshgen::tube(40)),
        gpu.add_mesh(&meshgen::tube(10)), // M_SHELL_LO: far-field tube
        gpu.add_mesh(&meshgen::tube_open(40)), // M_SHELL_OPEN: no cap rings
        gpu.add_mesh(&meshgen::tube_open(18)), // M_SHELL_OPEN_LO
        gpu.add_mesh(&meshgen::torus_tube(64, 20)), // M_TORUS_TUBE: seamless loop
        gpu.add_mesh(&meshgen::ring_deck(72)), // M_RING_DECK: drivable ring
        gpu.add_mesh(&meshgen::hex_prism()),   // M_HEX: dropshot tiles
            gpu.add_mesh(&meshgen::sphere(10, 16)),  // M_SPHERE_LO
        gpu.add_mesh(&meshgen::capsule(world::CAP_TALL_HH, 5, 10)), // M_CAP_TALL_LO
];
    assert_eq!(ids[world::M_TERRAIN], world::M_TERRAIN);
    // the garage's rim styles, one mesh each
    for (_, spokes, dish) in custom::WHEELS.iter() {
        gpu.add_mesh(&meshgen::rim(*spokes, *dish, 24));
    }
    gpu.add_mesh(&meshgen::tube_rim(40)); // M_TUBE_RIM
    // the ground, split into cullable chunks (placeholders until generate)
    let nc = world::TERRAIN_CHUNKS;
    for _ in 0..(nc * nc) {
        gpu.add_mesh(&meshgen::MeshData::new());
    }
    bake_heightmap(&gpu, seed, style0, Vec3::ZERO);

    // ---- models -----------------------------------------------------------
    let dir = assets_dir();
    println!("loading models from {:?} ...", dir);
    let t0 = Instant::now();
    let octane = model::load(&dir.join("octane"), &mut gpu, 1.28);
    let fennec = model::load(&dir.join("fennec"), &mut gpu, 1.28);
    // WAS `dominus` / `dominus_red`, which are Rocket League rips. Gone: 11 MB
    // of glTF for a game that has its own 1,012-model catalogue, and a
    // licence question nobody wants. The slot order here is INDEX-ADDRESSED
    // (see `cars.models` below and `cfg car`), so they are substituted rather
    // than removed — deleting an entry silently renumbers every car after it
    // and every saved loadout with them.
    let dominus = procars::vipera(&mut gpu);
    let decor_model = procars::vipera(&mut gpu);
    let ball_model = model::load(&dir.join("ball"), &mut gpu, 2.0); // radius 1
    println!("models loaded in {:.2}s", t0.elapsed().as_secs_f32());

    let breadbox = procars::breadbox(&mut gpu);
    let wedge = procars::wedge(&mut gpu);
    let roller = procars::roller(&mut gpu);
    let longboy = procars::longboy(&mut gpu);
    let cube = procars::cube(&mut gpu);
    let shark = procars::shark(&mut gpu);
    let volt = procars::volt(&mut gpu);
    let axion = procars::axion(&mut gpu);
    let hauler = procars::hauler(&mut gpu);
    let pulse = procars::pulse(&mut gpu);
    let tram = procars::tram(&mut gpu);
    let vipera = procars::vipera(&mut gpu);
    let aegis = procars::aegis(&mut gpu);
    // assembled from mined part layouts (a tank, a racer, a mech, a rocket, a jeep)
    let rollout = partcars::rollout(&mut gpu);
    let cryofin = partcars::cryofin(&mut gpu);
    let tl4 = partcars::tl4(&mut gpu);
    let spearhead = partcars::spearhead(&mut gpu);
    let outback = partcars::outback(&mut gpu);
    let widowmaker = partcars::widowmaker(&mut gpu);
    let madcar = partcars::madcar(&mut gpu);
    let speedrunner = partcars::speedrunner(&mut gpu);
    let eclipse = partcars::eclipse(&mut gpu);
    let mach6 = partcars::mach6(&mut gpu);
    // ---- HOVERCARS ----
    // Six of them, and the numbers are the design: ride height, cushion
    // stiffness, how much sideways motion they hold, and the reach and force
    // of the repulsor field. A stiff, low, tight-fielded machine darts and
    // pokes; a soft, high, wide-fielded one wallows and shepherds.
    let hover_defs: [(&str, f32, physics::Hover); 14] = [
        // BRUTIS: a hover tank. Heavy, slow to settle, enormous soft field —
        // it does not shoot so much as lean on the ball.
        ("brutis", 1.36, physics::Hover { ride: 0.62, stiff: 30.0, hold: 0.30, field_r: 4.4, field_k: 22.0, ..physics::Hover::default() }),
        // AKIMBO: sleek and stiff. Almost no lateral hold, so it slides like
        // a puck and has to be flown rather than driven.
        ("akimbo", 1.28, physics::Hover { ride: 0.42, stiff: 58.0, hold: 0.09, field_r: 2.6, field_k: 30.0, ..physics::Hover::default() }),
        // MATRIS: the balanced one, and the place to start.
        ("matris", 1.30, physics::Hover { ride: 0.52, stiff: 44.0, hold: 0.20, field_r: 3.2, field_k: 26.0, ..physics::Hover::default() }),
        // VECTORZOID: shielded — the strongest field in the game, at the
        // cost of a soft cushion that pitches under its own thrust.
        ("vectorzoid", 1.30, physics::Hover { ride: 0.58, stiff: 34.0, hold: 0.16, field_r: 4.0, field_k: 36.0, ..physics::Hover::default() }),
        // TEMPEST: a hovertank. Rides high enough to clear things, and holds
        // enough to actually turn.
        ("tempest", 1.40, physics::Hover { ride: 0.72, stiff: 36.0, hold: 0.26, field_r: 3.8, field_k: 24.0, ..physics::Hover::default() }),
        // CONTINGENCY: motorcycle-sized. Stiff, low, and a lance of a field —
        // it pokes the ball rather than pushing it.
        ("contingency", 1.10, physics::Hover { ride: 0.34, stiff: 66.0, hold: 0.07, field_r: 2.2, field_k: 34.0, ..physics::Hover::default() }),
        // BLACKSTAR: a knife. The shortest pulse cooldown in the game, so it
        // fights by shoving rather than by out-driving anyone.
        ("blackstar", 1.22, physics::Hover { ride: 0.40, stiff: 60.0, hold: 0.10, field_r: 2.8, field_k: 30.0, pulse_cd: 1.05, pulse_k: 13.0, ..physics::Hover::default() }),
        // ELEGANCE: long and floaty, with a generous base reserve — it is
        // never quite out of thrust, which suits its complete lack of grip.
        ("elegance", 1.34, physics::Hover { ride: 0.56, stiff: 38.0, hold: 0.12, field_r: 3.4, field_k: 25.0, base_tank: 200.0, base_regen: 82.0, ..physics::Hover::default() }),
        // TRIFANG: three-pronged and stiff. A narrow, hard field: it stabs.
        ("trifang", 1.26, physics::Hover { ride: 0.44, stiff: 62.0, hold: 0.11, field_r: 2.4, field_k: 38.0, ..physics::Hover::default() }),
        // ECHOTRON: a civilian runabout. The most forgiving of them — real
        // lateral hold and a soft, wide field.
        ("echotron", 1.28, physics::Hover { ride: 0.50, stiff: 46.0, hold: 0.34, field_r: 3.6, field_k: 23.0, ..physics::Hover::default() }),
        // HARDWALL: a police interceptor. Built to shove: the strongest
        // pulse here, at the cost of a long wait for it.
        ("hardwall", 1.32, physics::Hover { ride: 0.48, stiff: 50.0, hold: 0.22, field_r: 3.0, field_k: 26.0, pulse_k: 24.0, pulse_lift: 9.0, pulse_cd: 2.3, ..physics::Hover::default() }),
        // FOLLY: a land surfer. Rides high and holds almost nothing, so it
        // carves in long arcs and cannot stop.
        ("folly", 1.24, physics::Hover { ride: 0.68, stiff: 32.0, hold: 0.06, field_r: 3.2, field_k: 24.0, ..physics::Hover::default() }),
        // LINEBACK: a modular racer. The strongest charge tier, so a pad
        // turns it into something else for a few seconds.
        ("lineback", 1.30, physics::Hover { ride: 0.46, stiff: 54.0, hold: 0.14, field_r: 2.9, field_k: 28.0, charge_power: 1.55, ..physics::Hover::default() }),
        // WHISTLEWIND: a hover IFV. Heavy, high, and it leans on the ball.
        ("whistlewind", 1.42, physics::Hover { ride: 0.70, stiff: 34.0, hold: 0.30, field_r: 4.2, field_k: 22.0, pulse_lift: 6.5, ..physics::Hover::default() }),
    ];
    let hover_dir = dir.join("hover");
    let mut hovercars: Vec<model::Model> = Vec::new();
    for (name, len, h) in hover_defs.iter() {
        let mut m = model::load(&hover_dir.join(name), &mut gpu, *len);
        m.hover = Some(*h);
        m.drive = model::Drive::Electric; // nothing here burns fuel
        // the parts that explain the chassis: skirt, four nozzles at the
        // cushion's own sample points, and underglow
        let accent = [0.35, 0.85, 1.0, 1.0];
        m = procars::hoverize(&mut gpu, &m, accent, *h);
        hovercars.push(m);
    }
    // imported bodies (single-file .glb, geometry and materials inline)
    let cars_dir = dir.join("cars");
    let backfire = model::load(&cars_dir.join("backfire"), &mut gpu, 1.28);
    let breakout = model::load(&cars_dir.join("breakout"), &mut gpu, 1.30);
    let assael = model::load(&cars_dir.join("assael"), &mut gpu, 1.34);
    let guardian = model::load(&cars_dir.join("guardian"), &mut gpu, 1.30); // orientation is detected now
    let orion = model::load(&cars_dir.join("orion"), &mut gpu, 1.32);
    // likewise
    let vulcan = procars::aegis(&mut gpu);
    let xdevil = procars::axion(&mut gpu);
    let mut cars = CarModels {
        models: vec![
            octane, fennec, dominus, breadbox, wedge, roller, longboy, cube, shark,
            volt, axion, hauler, pulse, tram, vipera, aegis,
            backfire, breakout, assael, guardian, orion, vulcan, xdevil,
            rollout, cryofin, tl4, spearhead, outback,
            widowmaker, madcar, speedrunner, eclipse, mach6,
        ],
        names: vec![
            "Octane", "Fennec", "Dominus", "Breadbox", "Wedge", "Roller", "Longboy", "Cube", "Shark",
            "Volt EV", "Axion EV", "Hauler EV", "Pulse EV", "Tram EV", "Vipera", "Aegis",
            "Backfire", "Breakout", "Assael", "Guardian", "Orion", "Vulcan", "X-Devil",
            "Rollout", "Cryofin", "TL-4", "Spearhead", "Outback",
            "Widowmaker", "Madcar", "Speedrunner", "Eclipse", "Mach 6",
        ],
        info: vec![
            ("CLASSIC", "the benchmark: neutral, forgiving, good at everything"),
            ("CLASSIC", "flat-nosed and low — the ball sits where you put it"),
            ("CLASSIC", "long and low, built to scoop and carry"),
            ("ODDITY", "tall and square: terrible aerials, unbeatable bumps"),
            ("ODDITY", "a doorstop — the nose is almost on the floor"),
            ("ODDITY", "a pebble: round in every direction, predictable nowhere"),
            ("ODDITY", "a limousine hitbox; turns like one too"),
            ("ODDITY", "a joke about being a box, kept because it is funny"),
            ("ODDITY", "a streamliner with a fin you can feel in the air"),
            ("ELECTRIC", "compact hatch — dribbles like an Octane"),
            ("ELECTRIC", "low hypercar, the flick blade of the catalogue"),
            ("ELECTRIC", "a pickup with a bed the ball settles into"),
            ("ELECTRIC", "rally stance on tall tyres: lands well, shrugs off bad ones"),
            ("ELECTRIC", "a shuttle with an enormous roof shelf"),
            ("CONCEPT", "continuous shell, bubble canopy, muscular haunches"),
            ("CONCEPT", "armoured wedge of interlocking plates"),
            ("IMPORT", "digital camo, compact and squat"),
            ("IMPORT", "the second art reference: crisp panels, clean shoulder"),
            ("IMPORT", "a Dominus variant with a heavier tail"),
            ("IMPORT", "broad and armoured, the widest of the imports"),
            ("IMPORT", "long GT nose, the most detailed body here"),
            ("IMPORT", "aggressive and angular; static wheels by choice"),
            ("IMPORT", "horned and mean, a devil in profile"),
            ("ASSEMBLED", "from a sci-fi tank: sponsons and a heavy tail"),
            ("ASSEMBLED", "from a slick racer: spine and shell, the lightest"),
            ("ASSEMBLED", "from a walking mech: plated and upright"),
            ("ASSEMBLED", "from a rocket: a spine with rings down it"),
            ("ASSEMBLED", "from a sci-fi jeep: boxy cabin, greebles everywhere"),
            ("ASSEMBLED", "from a death-race car: armoured, tracked, all mass"),
            ("ASSEMBLED", "from a wasteland truck: ringed and cluttered"),
            ("ASSEMBLED", "from a flyer: mostly engine, pods where seats would be"),
            ("ASSEMBLED", "from an attack helicopter: a spaceframe with a skin"),
            ("ASSEMBLED", "from a classic racer: the busiest surface of the set"),
        ],
        active: 0,
        xform: HashMap::new(),
        chassis: Vec::new(),
        cycle: Vec::new(),
    };
    for (i, m) in hovercars.into_iter().enumerate() {
        cars.models.push(m);
        cars.names.push(["Brutis", "Akimbo", "Matris", "Vectorzoid", "Tempest", "Contingency", "Blackstar", "Elegance", "Trifang", "Echotron", "Hardwall", "Folly", "Lineback", "Whistlewind"][i]);
        cars.info.push(("HOVER", [
            "a hover tank: leans on the ball with an enormous soft field",
            "stiff and slippery — slides like a puck, has to be flown",
            "the balanced hovercar, and the place to start",
            "shielded: the strongest repulsor field in the game",
            "rides high enough to clear things, and can actually turn",
            "motorcycle-sized, with a lance of a field: it pokes",
            "shortest pulse cooldown here — it fights by shoving",
            "long and floaty, with a reserve that never quite runs out",
            "three-pronged and stiff: a narrow, hard field that stabs",
            "a civilian runabout, the most forgiving of them",
            "police interceptor: the hardest pulse, and a long wait for it",
            "a land surfer — carves in long arcs and cannot stop",
            "modular racer: a pad turns it into something else",
            "a hover IFV, heavy and high, and it leans on the ball",
        ][i]));
    }
    // EVERY car gets an electric variant. Rather than model them twice, the
    // petrol body is converted: light bars, blanked grille, battery sills and
    // an ion thruster, plus the electric torque curve.
    {
        let n = cars.models.len();
        for i in 0..n {
            if cars.models[i].drive == model::Drive::Electric {
                continue; // already electric
            }
            let accent = [0.30, 0.82, 1.0, 1.0];
            let ev = procars::electrify(&mut gpu, &cars.models[i], accent);
            cars.models.push(ev);
            cars.names.push(Box::leak(format!("{} EV", cars.names[i]).into_boxed_str()));
            cars.info.push((
                "ELECTRIC",
                Box::leak(format!("{} — electric: harder off the line, lower top end", cars.blurb_of(i)).into_boxed_str()) as &str,
            ));
        }
        cars.active = sv.car.min(cars.models.len() - 1);
        // MOIRE IS THE DEFAULT, when no save says otherwise.
        //
        // The built-in hover cars come first in cars.models, so index 0 is
        // whatever happened to be listed first -- fine when there were nine
        // cars and arbitrary now. MOIRE is written to be the opening car:
        // mid-set hitbox, forgiving roof, enough shape to read at a glance,
        // and nothing a player has to unlearn when they switch to a
        // specialist. Resolved by NAME so it survives reordering.
        if sv.car == 0 {
            if let Some(i) = cars.names.iter().position(|n| *n == "MOIRE") {
                cars.active = i;
            }
        }
    }

    // ---- BLACKSTAR BOTS -------------------------------------------------
    //
    // assets/bots/<name>/f00, f01, ... — a fold strip written by
    //   foldview --veh <model>.iqm --gltf <dir> --gltf-merge --t <t>
    // one directory per pose, vehicle first, robot last. Anything with at
    // least two poses is a chassis; a single directory would just be a car,
    // and is skipped rather than half-registered.
    //
    // A name starting `jet_` flies (see physics: never grounded, lift and
    // nose thrust) — the category is in the filename because the fold strip
    // itself carries no metadata, and a marker in the path is a great deal
    // simpler than a sidecar nobody will remember to write.
    //
    // Every pose is a real model and gets a real SDF bake below, so poses
    // cost startup time: keep strips short (5-7) rather than smooth.
    let bots_dir = dir.join("bots");
    // Declared out here so Game can keep it: the categories are read inside
    // this block and the morph, rebuilt much later, needs them per chassis.
    let mut bot_categories_loaded: std::collections::HashMap<String, String> =
        Default::default();
    if bots_dir.is_dir() {
        let mut bots: Vec<String> = std::fs::read_dir(&bots_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        bots.sort();

        // CHASSIS SELECTION. Every pose is a model and the SDF bake holds
        // CSDF_MAXM (126) of them, so 222 chassis at ~5 poses cannot all load.
        // Without this the bake silently truncates mid-chassis and some bots
        // fold into a pose the renderer cannot sample.
        //
        // Counting poses per chassis costs a readdir each, which is why it
        // happens ONCE here rather than inside roster::fit — the selector is
        // pure so it can be tested without a filesystem.
        let tiers = roster::load_tiers(&bots_dir);
        // Categories seed the trait vector; see chassis_traits.rs.
        let tiers_cat = roster::load_categories(&bots_dir);
        bot_categories_loaded = tiers_cat.clone();
        // SMOOTH ONLY. A chassis fleet.json marks "refused" has no real fold
        // — Morph::build rejects the strip and the game steps between whole
        // poses instead of blending them, a slideshow wearing a fold's
        // clothing. roster::fit excludes these unconditionally now, so this
        // is only read here to populate the field it checks.
        let morph = roster::load_morph(&bots_dir);
        let inventory: Vec<roster::Chassis> = bots.iter().map(|name| {
            let n = std::fs::read_dir(bots_dir.join(name))
                .into_iter().flatten().flatten()
                .filter(|e| e.path().is_dir() && e.path().join("scene.gltf").is_file())
                .count();
            roster::Chassis { name: name.clone(), poses: n,
                              tier: tiers.get(name).copied(),
                              morph_ok: morph.get(name).copied() }
        }).collect();

        // DEFAULT IS FORGE when any generated chassis exist.
        //
        // Pick::All loads 126 arbitrary chassis off a 268-entry roster that is
        // 55% rights-holder material by filename, and whose multi-file imports
        // went through texture_export_v9 -- one `break` in a directory walk,
        // so it keeps whichever .obj os.listdir returned first and drops the
        // rest (measured: 8-9% of the model). That is the catalogue you see
        // today, and it is not a good default.
        //
        // `--bots all` is unchanged and still loads everything.
        let has_forge = std::fs::read_dir(&bots_dir)
            .into_iter().flatten().flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("forge_"));
        let pick = opts.bots.clone().unwrap_or(if has_forge {
            roster::Pick::Forge
        } else {
            roster::Pick::All
        });
        let chosen = roster::fit(&inventory, &pick, roster::SDF_BUDGET);
        let dropped = inventory.len() - chosen.len();
        if dropped > 0 {
            // Said out loud, with the way to see the rest. A roster that
            // quietly shows a third of the fleet is indistinguishable from a
            // broken asset drop.
            let pages = roster::page_count(&inventory, roster::SDF_BUDGET);
            eprintln!("bots: loaded {} of {} chassis ({dropped} over the {} SDF budget)",
                      chosen.len(), inventory.len(), roster::SDF_BUDGET);
            eprintln!("      --bots all for the imported roster, forge for generated only,");
            eprintln!("      --bots page:0..{} to step through, or --bots high|broken",
                      pages.saturating_sub(1));
        }
        let bots: Vec<String> = chosen.into_iter().map(|c| c.name).collect();

        // FORGE: anything with a matching GLB in assets/forge. FIXED: listed
        // in assets/fixed.txt, which texture_export_v13 writes when it
        // re-exports a multi-file model. Both are plain name sets so the
        // classifier stays a pure function of strings.
        let forge_names: HashSet<String> = std::fs::read_dir("assets/forge")
            .into_iter().flatten().flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension().is_some_and(|x| x == "glb"))
                    .then(|| p.file_stem()?.to_str().map(str::to_string))
                    .flatten()
            })
            .collect();
        let fixed_names: HashSet<String> = std::fs::read_to_string("assets/fixed.txt")
            .map(|s| s.lines().map(str::trim).filter(|l| !l.is_empty())
                      .map(str::to_string).collect())
            .unwrap_or_default();

        for bot in bots {
            let bdir = bots_dir.join(&bot);
            let mut poses: Vec<PathBuf> = std::fs::read_dir(&bdir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir() && p.join("scene.gltf").is_file())
                .collect();
            poses.sort();
            if poses.len() < 2 {
                eprintln!("bots: {bot} has {} pose(s), need 2+ — skipped", poses.len());
                continue;
            }
            // FLIGHT COMES FROM THE TRAIT VECTOR, not the filename.
            //
            // `jet_` still counts, because it is how the 48 hand-made bots were
            // marked and nothing else records their type. But a fleet chassis
            // gets its traits from fleet.json's category, so a `flyer` flies
            // whether or not somebody remembered the prefix — and a hovertank
            // can be armoured AND supported, which no prefix could say.
            let traits = tiers_cat.get(&bot)
                .map(|c| crate::chassis_traits::Traits::from_category(c))
                .unwrap_or_else(crate::chassis_traits::Traits::default);
            let flight = traits.flies() || bot.starts_with("jet_");
            // same reasoning as `flight` above, for the mirror-image gap:
            // `floats() == self.hover >= 0.5` is fleet.json's own
            // "hoverer"/"hovertank" category speaking, not a guess.
            let floats = traits.floats();
            // `forge_` is a FAMILY marker on the directory, not part of the
            // name: the catalogue already shows the family in its own column,
            // so carrying the prefix into the label would print it twice and
            // waste the 28 characters the row has.
            let label = bot
                .trim_start_matches("jet_")
                .trim_start_matches("forge_")
                .to_string();
            let cid = cars.chassis.len();
            let mut strip = Vec::new();
            let mut dirs: Vec<(PathBuf, f32)> = Vec::new();

            // ONE SCALE FOR THE WHOLE STRIP.
            //
            // `load` normalizes each model so its LONGEST axis spans
            // target_len. That is right for a garage full of unrelated cars
            // and wrong for five poses of one machine: the fold changes which
            // axis is longest — Z while it is a car, Y once it stands up — so
            // pose-by-pose normalization shrinks the robot to the length of
            // the vehicle and the bot visibly breathes as it transforms.
            //
            // Measure the vehicle pose once, then ask each pose for the
            // target_len that reproduces the vehicle's metres-per-source-unit.
            // `load_raw_at` hands back the scale it would have used, and
            // longest/scale recovers the raw span; axis-aligned reorientation
            // permutes the extents but cannot change which is biggest, so the
            // measurement survives the loader's own orientation search.
            let raw_span = |p: &PathBuf| -> f32 {
                let (_, _, _, s, _) = model::load_raw_at(p, 1.0, false);
                if s > 1e-9 { 1.0 / s } else { 1.0 }
            };
            let span0 = raw_span(&poses[0]);

            for (k, pose) in poses.iter().enumerate() {
                let target = 1.32 * raw_span(pose) / span0.max(1e-6);
                dirs.push((pose.clone(), target));
                let m = model::load(pose, &mut gpu, target);
                let idx = cars.models.len();
                strip.push(idx);
                cars.models.push(m);
                // Only the two ends are ever named in the HUD; the fold
                // frames are named too so a toast mid-transform is not blank.
                let nm = if k == 0 {
                    label.to_uppercase()
                } else if k == poses.len() - 1 {
                    format!("{} ROBOT", label.to_uppercase())
                } else {
                    format!("{} FOLD {k}", label.to_uppercase())
                };
                cars.names.push(Box::leak(nm.into_boxed_str()));
                // info MUST be pushed once per name or the two vectors drift
                // and every model after the gap is labelled with its
                // neighbour's family.
                cars.info.push((
                    model_family(&bot, &forge_names, &fixed_names),
                    Box::leak(
                        format!("{} pose {}/{}", label, k + 1, poses.len()).into_boxed_str(),
                    ) as &'static str,
                ));
                cars.xform.insert(idx, (cid, k));
            }
            // THE WALK STRIP, four phases at the robot end. Loaded at the
            // SAME target scale the robot pose got, because a walk that
            // resized between phases would breathe exactly the way the fold
            // used to before one scale was measured for the whole strip.
            let robot_target = dirs.last().map(|(_, t)| *t).unwrap_or(1.32);
            let mut walk: Vec<usize> = Vec::new();
            let mut walk_dirs: Vec<(PathBuf, f32)> = Vec::new();
            for k in 0..4usize {
                // model::load takes the pose DIRECTORY and joins scene.gltf
                // itself. This used to join it here too, so load saw
                // `w00/scene.gltf/scene.gltf` and every bot with walk poses
                // crashed the game at startup with ENOTDIR — "Not a directory",
                // which reads like a missing asset and is actually a doubled
                // path. The existence check is on the FILE and the load is on
                // the DIRECTORY, because those are two different questions.
                let wdir = bdir.join(format!("w{k:02}"));
                if !wdir.join("scene.gltf").is_file() {
                    break;
                }
                let m = model::load(&wdir, &mut gpu, robot_target);
                let idx = cars.models.len();
                walk.push(idx);
                walk_dirs.push((wdir.clone(), robot_target));
                cars.models.push(m);
                cars.names.push(Box::leak(
                    format!("{} WALK {k}", label.to_uppercase()).into_boxed_str(),
                ));
            }
            {
                // WHAT THE HULL CARRIES, read at load: the vehicle pose's
                // guns, thrusters and cockpit, and whether its fittings
                // agree it is facing forward. Printed so a hull whose
                // nozzles open at the nose is reported at startup rather
                // than discovered when the flame comes out of the grille.
                let hp = &cars.models[strip[0]].hardpoints;
                println!(
                    "bots: {label} — {} poses{}{} | {} gun{}, {} thruster{}, {}cockpit, forward hint {:+.1}{}",
                    strip.len(),
                    if walk.is_empty() { String::new() } else { format!(", {} walk", walk.len()) },
                    if flight { ", flight" } else { "" },
                    hp.guns.len(),
                    if hp.guns.len() == 1 { "" } else { "s" },
                    hp.thrusters.len(),
                    if hp.thrusters.len() == 1 { "" } else { "s" },
                    if hp.cockpit.is_some() { "" } else { "no " },
                    hp.forward_hint,
                    if hp.forward_hint < 0.0 { "  <- FITTINGS SAY THIS HULL IS BACKWARDS" } else { "" }
                );
            }
            cars.chassis.push(Chassis {
                strip,
                walk,
                walk_dirs,
                dirs,
                joints: joints::load(&bdir.join("joints.json")),
                scale: {
                    // a tiny hand parse, same reasoning as joints.rs: the
                    // file is written by a tool in this repo and adding a
                    // derive crate to read two fields costs more than it saves
                    let txt = std::fs::read_to_string(bdir.join("chassis.json")).unwrap_or_default();
                    txt.find("\"scale\":")
                        .and_then(|i| txt[i + 8..].split(|c| c == ',' || c == '}').next())
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .filter(|v| v.is_finite() && *v > 0.0)
                        .unwrap_or(1.0)
                },
                boss: std::fs::read_to_string(bdir.join("chassis.json"))
                    .map(|t| t.contains("\"boss\": true") || t.contains("\"boss\":true"))
                    .unwrap_or(false),
                transforms: std::fs::read_to_string(bdir.join("chassis.json"))
                    .map(|t| !(t.contains("\"transforms\": false") || t.contains("\"transforms\":false")))
                    .unwrap_or(true),
                flight,
                floats,
            });
        }
    }

    // `[` and `]` walk this: every ordinary car, plus each bot ONCE at its
    // vehicle pose. Without the filter a single seven-pose chassis would put
    // six half-folded bodies into the garage rotation.
    cars.cycle = (0..cars.models.len())
        .filter(|i| cars.xform.get(i).map_or(true, |&(_, k)| k == 0))
        .collect();
    cars.active = cars.active.min(cars.models.len() - 1);

    gpu.set_car_sdfs(&bake_car_sdfs(&mut cars.models));
    let cars = cars;


    let mut car = Car::new();
    apply_car_dims(&mut car, &cars.models[cars.active], cfg.car.size);
    let mut world = World::generate(seed, style0, cars.models[cars.active].half_h, cfg.arena.scale);
    // TEMPORARY VISUAL-PROOF HOOK, not gameplay: dumps the CIRCUIT/OVERLOAD
    // content (circuit_shapes' real Shape output for the Tithe plus one
    // bridge) into the running world so it can be SEEN and screenshotted
    // in a real render, rather than only asserted by headless tests. Gated
    // on an env var so it can never fire in a normal launch. Not a design
    // decision about how CIRCUIT eventually loads for real (that is
    // `circuit_map.rs`'s own documented open item) -- purely a look.
    if std::env::var("CIRCUIT_PREVIEW").is_ok() {
        let tithe = moons::by_id("tithe").expect("tithe must exist");
        let placements = circuit_map::layout(&tithe);
        let offset = Vec3::new(0.0, 0.0, 0.0);
        for mut s in circuit_shapes::stop_shapes(&placements) {
            s.mat = Mat4::from_translation(offset) * s.mat;
            s.pos += offset;
            world.shapes.push(s);
        }
        let bridges = moons::bridges();
        let spline = circuit_map::bridge_spline(&bridges[3]); // Tithe -> Knead
        for mut s in circuit_shapes::bridge_rubble_shapes(&bridges[3], &spline) {
            s.mat = Mat4::from_translation(offset) * s.mat;
            s.pos += offset;
            world.shapes.push(s);
        }
        eprintln!("[circuit_preview] injected {} CIRCUIT shapes at offset {offset:?}", world.shapes.len());
    }
    // TEMPORARY VISUAL-PROOF HOOK, same shape as CIRCUIT_PREVIEW above:
    // dumps the Bridge's new mode-select scene (`holomap::hub_scene`)
    // into the running world as real shapes, so the ring/sweep can be
    // SEEN rather than only asserted by headless tests. Console-table
    // units (`holomap::HUB_RING_R` is 0.30 m) are scaled up for a
    // world-space mockup -- this is not the real physical placement,
    // which belongs to whatever eventually renders the Bridge room's
    // actual console model.
    if let Ok(sel_str) = std::env::var("HUB_PREVIEW") {
        let sel = match sel_str.as_str() {
            "overload" => flow::HubChoice::Overload,
            "raid" => flow::HubChoice::Raid,
            "patrol" => flow::HubChoice::Patrol,
            _ => flow::HubChoice::Campaign,
        };
        let skin = &holomap::SKINS[0];
        let scene = holomap::hub_scene(sel, skin);
        let scale = 20.0;
        let hub_offset = Vec3::new(80.0, 2.0, 0.0);
        for g in &scene {
            match g {
                holomap::Glyph::Body { pos, color, .. } => {
                    let p = hub_offset + Vec3::new(pos.x, 0.0, pos.z) * scale;
                    let r = 1.2;
                    let rot = Quat::IDENTITY;
                    world.shapes.push(world::Shape {
                        mesh: world::M_SPHERE,
                        mat: Mat4::from_scale_rotation_translation(Vec3::splat(r), rot, p),
                        pos: p,
                        inv_rot: rot.inverse(),
                        sdf: world::Sdf::Sphere { r },
                        color: *color,
                        emissive: 0.6,
                        flash: 0.0,
                        vel: Vec3::ZERO,
                        omega: Vec3::ZERO,
                        spec: 0.5,
                        cut: false,
                        blend: false,
                    });
                }
                holomap::Glyph::Ring { r, color, .. } => {
                    let big = r * scale;
                    let tube = big * 0.03;
                    let rot = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
                    world.shapes.push(world::Shape {
                        mesh: world::M_TORUS_THIN,
                        mat: Mat4::from_scale_rotation_translation(Vec3::splat(big), rot, hub_offset),
                        pos: hub_offset,
                        inv_rot: rot.inverse(),
                        sdf: world::Sdf::Torus { big, tube },
                        color: *color,
                        emissive: 0.3,
                        flash: 0.0,
                        vel: Vec3::ZERO,
                        omega: Vec3::ZERO,
                        spec: 0.4,
                        cut: false,
                        blend: true,
                    });
                }
                holomap::Glyph::Sweep { angle, color } => {
                    let dir = Vec3::new(angle.cos(), 0.0, angle.sin());
                    let half = (scale * holomap::HUB_RING_R) * 0.5;
                    let mid = hub_offset + dir * half;
                    let rot = Quat::from_rotation_arc(Vec3::Y, dir);
                    world.shapes.push(world::Shape {
                        mesh: world::M_CAP_TALL,
                        mat: Mat4::from_scale_rotation_translation(Vec3::splat(0.3), rot, mid),
                        pos: mid,
                        inv_rot: rot.inverse(),
                        sdf: world::Sdf::Capsule { r: 0.3, half },
                        color: *color,
                        emissive: 0.8,
                        flash: 0.0,
                        vel: Vec3::ZERO,
                        omega: Vec3::ZERO,
                        spec: 0.5,
                        cut: false,
                        blend: true,
                    });
                }
                _ => {}
            }
        }
        eprintln!("[hub_preview] injected hub scene for {sel:?} at {hub_offset:?}");
    }
    gpu.replace_mesh(world::M_TERRAIN, &meshgen::MeshData::new()); // chunks draw it now
    let tchunks = terrain_chunks(&mut gpu, &world);
    gpu.set_rcells(&bake_rcells(&world));
    car.reset(&world);
    let mut ball = Ball::new();
    ball.reset_in_front(&car);

    let mut cam = camera::Camera::new();
    cam.ball_cam = sv.ball_cam;
    // the save predates the third mode, so it round-trips ball vs chase and
    // first person always starts off — a saved first-person view that you
    // cannot see yourself in is a confusing thing to boot into
    cam.mode = if sv.ball_cam { camera::CamMode::Ball } else { camera::CamMode::Chase };
    let snd = sound::Sound::new(sv.volume, sv.music, opts.mute);

    let (pv_cp, pv_cr, pv_bp, pv_br) = (car.pos, car.rot, ball.pos, ball.rot);
    let vs0 = cfg.video.vsync;
    let rs0 = cfg.video.render_scale;
    let gyro_on0 = cfg.gyro.on;
    let trials_bests0 = trials::Bests::decode(&sv.trials);
    let look0 = custom::Loadout::decode(&sv.look);
    let academy_passed0 = academy::Passed::decode(&sv.academy);
    let mut game = Game {
        bot_categories: bot_categories_loaded,
        gpu,
        // Sized off the body that flies it, not a number in metres. See
        // `aboard::HULL_HEIGHTS` — a 70 m hull was 184 body-heights for the
        // 0.38 m body this engine actually uses.
        aboard: aboard::Aboard::for_body(physics::Car::new().half_h * 2.0),
        last_room: None,
        door_mask: 0,
        world,
        car,
        ball,
        cam,
        cars,
        xf: xform::Xform::new(),
        morph: None,
        walk_morph: None,
        walk_morph_tried: None,
        health: 1.0,
        wep: weapons::Weapons::new(),
        autotarget: false,
        lift: None,
        repeat_key: None,
        repeat_t: 0.0,
        sea_level: None,
        joint_sel: 0,
        joint_only_bad: true,
        space: chord::Chord::new(),
        sight: chord::Sight::new(),
        loco: anim::Locomotion::new(),
        loco_pose: anim::Pose::default(),
        rig: rigmotion::RigMotion::new(3.9),
        rig_frame: None,
        ball_model,
        decor_model,
        fx: fx::Fx::new(seed),
        snd,
        pad: gamepad::Pad::new(),
        last_pad: gamepad::PadState::default(),
        save: sv,        binds,
        net: net::Net::new(),
        applied_scale: cfg.arena.scale,
        cfg,
        binds_to_settings: false,
        cursor: (0.0, 0.0),
        cursor_moved: false,
        prev_cursor_vis: true,
        capture_guard: 0.0,
        prev_cpos: pv_cp,
        prev_crot: pv_cr,
        prev_bpos: pv_bp,
        prev_brot: pv_br,
        goals: 0,
        game_mode: 0,
        last_vsync: vs0,
        roll_mix_s: 0.0,
        scores: [0, 0],
        tpad: touch::TouchPad::new(),
        last_ivp: Mat4::IDENTITY,
        last_scale: rs0,
        tap_drive: (0.0, 0.0),
        gyro: gyro::Gyro::new(gyro_on0),
        tchunks,
        lesson: None,
        passed: academy_passed0,
        lesson_sel: 0,
        lesson_tricks: Vec::new(),
        rec: replay::Recorder::default(),
        moments: Vec::new(),
        watching: None,
        last_hit: (Vec3::ZERO, 9.0),
        look: look0,
        look_sel: 0,
        trial: None,
        bests: trials_bests0,
        ghost: Vec::new(),
        trial_sel: 0,
        portal_m: 0.0,
        style: style::Analyzer::new(),
        mscroll: 0.0,
        mfling: 0.0,
        mscroll_at_down: 0.0,
        tdown: None,
        tdrag_row: None,
        tscrub: false,
        tmoved: 0.0,
        wx_rain: 0.0,
        in_steer: 0.0,
        in_yaw: 0.0,
        in_roll: 0.0,
        analog: analog::Analog::new(),
        bots: Vec::new(),
        friend: String::new(),
        online_menu: None,
        toast: String::new(),
        toast_t: 0.0,
        beacon_fx_t: 0.0,
        mode: Mode::Splash,
        menu_from_pause: false,
        deck: None,
        sortie: None,
        raid: None,
        forge_rx: None,
        shots: shots::Shots::default(),
        hostile_shots: shots::Shots::default(),
        saddle_shots: 0,
        hitstop: 0.0,
        ship_class: aboard::FLEET_DEFAULT,
        loco_mode: locomotion::decide(4, 0, 0, 0.0, 0.0),
        ambient: ballistics::Ambient::earthlike(),
        world_g: 9.81,
        swing: 0.0,
        hotbar,
        body: sections::Body::default(),
        grime: grime::Grime::default(),
        held: HashSet::new(),
        held_mouse: HashSet::new(),
        jump_edge: false,
        flip_vis: false,
        time: 0.0,
        acc: 0.0,
        fps: 60.0,
        fps_win: 0.0,
        fps_presented: 0,
        fps_dropped: 0,
        frame_drops: 0,
        rings: 0,
        car_rings: 0,
        combo: 0,
        combo_t: 0.0,
        air_t: 0.0,
        air_link: 0.0,
        climb_msg_cd: 0.0,
        patrol: patrol::Patrol::default(),
    };

    // Upload the ship's hull once. `Aboard::upload_hull` needs `&mut Gpu`, so
    // it happens here rather than in the struct literal — Game owns the gpu.
    {
        // The replace/restore dance this used to do existed to get two
        // &mut out of `game` at once; `upload_ship_hull` is a method, so
        // disjoint-field borrows handle it and the swap can go.
        game.upload_ship_hull();
        // A REAL MODEL OVER THE PROCEDURAL SHELL. The interior's own SDF
        // collision (hull box minus carved rooms) is untouched by this —
        // see `Aboard::exterior`'s own doc comment — this only swaps what
        // gets DRAWN for the outer skin. Picked from the catalog's own
        // `category: "spaceship"` chassis; a big troop-transport shape
        // fits "mobile fortress" better than a small courier or fighter.
        game.aboard.load_exterior(&mut game.gpu, &bots_dir, aboard::EXTERIOR_MODEL);
        // PARK IT AT THE SAME PLACE THE DRAW CALL PUTS IT. Two transforms for
        // one object is how a hull you can see and a hull you can hit end up
        // in different places, so both read `ground_lift` — and this is the
        // same call `load_map` makes, so there is ONE place that parks rather
        // than a launch path and a map path that can drift.
        game.repark();
        // A ONE-LINE PROOF THE WIRING RAN. Reported at startup because
        // "the game runs exactly as it did before" and reading the code
        // could not distinguish a change that does not execute from a change
        // that does nothing visible.
        if let Some((o, ship)) = game.world.parked.as_ref() {
            eprintln!(
                "[ship] hull {:.1} m, {} rooms, slot {:?}, parked at {:.1},{:.1},{:.1}",
                ship.hull_half.x * 2.0,
                ship.cells.len(),
                game.aboard.hull_slot,
                o.x, o.y, o.z
            );
        } else {
            eprintln!("[ship] NOT PARKED");
        }
    }

    // ---- launch-time multiplayer: bsgames handoff first, then CLI ---------
    if game.save.name.is_empty() && (opts.host || opts.join.is_some()) {
        game.save.name = format!("PILOT{}", std::process::id() % 1000);
    }
    if argv.iter().any(|a| a == "--quick-match") {
        game.mode = Mode::Play; // bsgames quick-match: straight into the sandbox
    }
    if let Some(status) = net::from_handoff(&mut game.net) {
        game.toast = status;
        game.toast_t = 6.0;
        game.mode = Mode::Play;
    } else if opts.host {
        let n = game.save.name.clone();
        game.toast = game.net.host(&n);
        game.toast_t = 6.0;
        game.mode = Mode::Play;
    } else if let Some(friend) = &opts.join {
        game.friend = friend.clone();
        let n = game.save.name.clone();
        game.toast = game.net.join(&n, friend);
        game.toast_t = 6.0;
        game.mode = Mode::Play;
    }
    let mut last = Instant::now();
    let win = window.clone();
    event_loop
        .run(move |event, elwt| match event {
            // RAW MOUSE MOTION, for robot free look. CursorMoved reports a
            // position and stops at the edge of the window; a look-around
            // needs the delta, which only DeviceEvent carries.
            Event::DeviceEvent {
                event: winit::event::DeviceEvent::MouseMotion { delta },
                ..
            } => {
                let looking = game.car.robot || game.cam.mode == camera::CamMode::First;
                if looking && game.mode == Mode::Play {
                    const SENS: f32 = 0.0022;
                    game.cam.look_yaw -= delta.0 as f32 * SENS;
                    // from the head you can look almost straight up and down;
                    // on a boom, looking far up shoves the eye through the
                    // body, so third person keeps the tighter clamp
                    let (lo, hi) = if game.cam.mode == camera::CamMode::First {
                        (-1.35, 1.35)
                    } else {
                        (-0.55, 1.05)
                    };
                    game.cam.look_pitch =
                        (game.cam.look_pitch - delta.1 as f32 * SENS).clamp(lo, hi);
                    // wrap so the yaw cannot wind up to a huge number over a
                    // long session
                    let tau = std::f32::consts::TAU;
                    if game.cam.look_yaw > std::f32::consts::PI {
                        game.cam.look_yaw -= tau;
                    } else if game.cam.look_yaw < -std::f32::consts::PI {
                        game.cam.look_yaw += tau;
                    }
                }
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    game.net.leave();
                    game.persist();
                    elwt.exit();
                }
                WindowEvent::Resized(size) => game.gpu.resize(size.width, size.height),
                WindowEvent::KeyboardInput { event, .. } => {
                    let key = input::norm_key(&event.logical_key);
                    match event.state {
                        ElementState::Pressed => {
                            game.analog.note_key(&key, &event.physical_key);
                            // [ and ] cycle THE SHIP, matched on both the
                            // logical character and the physical key
                            // position. Dvorak moves the bracket CHARACTERS
                            // to where QWERTY has - and =, so a player who
                            // thinks in characters and a player who thinks
                            // in key positions both get what they reached
                            // for. Shifted brackets ({ }) cycle cars —
                            // these two were the other way round until the
                            // ship became the thing worth reaching for.
                            if !event.repeat {
                                use winit::keyboard::{KeyCode, PhysicalKey};
                                let ch = match &key {
                                    Key::Character(c) => c.as_str(),
                                    _ => "",
                                };
                                let phys = event.physical_key;
                                let dir = if ch == "[" || ch == "{" || phys == PhysicalKey::Code(KeyCode::BracketLeft) {
                                    -1
                                } else if ch == "]" || ch == "}" || phys == PhysicalKey::Code(KeyCode::BracketRight) {
                                    1
                                } else {
                                    0
                                };
                                if dir != 0 {
                                    // SHIFTED BRACKETS CYCLE THE CAR. Read
                                    // the CHARACTER for the shift, not a
                                    // modifier state: `{` is what the
                                    // keyboard sends and it survives every
                                    // layout that moves the bracket keys,
                                    // which is the same reason the physical
                                    // fallback above exists.
                                    if ch == "{" || ch == "}" {
                                        game.cycle_car(dir);
                                    } else {
                                        game.cycle_ship(dir);
                                    }
                                }
                            }
                            // ALREADY-HELD GUARD, not just !event.repeat: an
                            // analog keyboard (Wooting) can send a genuine,
                            // non-OS-repeat Pressed event more than once for
                            // what is physically one soft press — the switch
                            // chatters near its actuation point on a feather
                            // press, and each chatter transition is a real
                            // key-down as far as winit/the OS is concerned,
                            // so event.repeat stays false for every one of
                            // them. That double-fired every menu nav action
                            // (arrow keys) on a feather press. The Jump
                            // action a few lines below already guards this
                            // exact way for the same reason; key_down's
                            // call sites (menu navigation, the repeat timer)
                            // did not, which is why this and not that ever
                            // double-fired.
                            if !event.repeat && !game.held.contains(&key) {
                                game.key_down(key.clone(), elwt, &win);
                            } else if let Mode::Online { .. } = game.mode {
                                game.key_down(key.clone(), elwt, &win); // key repeat in text fields
                            }
                            if game.binds.is_bound(Action::Jump, &input::Bind::Key(key.clone()))
                                && !game.held.contains(&key)
                            {
                                game.space.press();
                            }
                            game.held.insert(key);
                        }
                        ElementState::Released => {
                            let was_space =
                                game.binds.is_bound(Action::Jump, &input::Bind::Key(key.clone()));
                            game.held.remove(&key);
                            if was_space {
                                if let Some(p) = game.space.release() {
                                    game.space_intent(p);
                                }
                                game.autotarget_release();
                            }
                            // HOLD MODE releases the lock with the key. Done
                            // here rather than in one_shot because releases
                            // are not shots — nothing else in the game needs
                            // a key-up, which is why this is the only case.
                            game.autotarget_release();
                        }
                    }
                }
                WindowEvent::Focused(f) => {
                    if !f {
                        // alt-tab etc: always hand the cursor back to the OS
                        let _ = win.set_cursor_grab(winit::window::CursorGrabMode::None);
                        win.set_cursor_visible(true);
                        game.prev_cursor_vis = true; // frame() re-grabs on return
                        game.held.clear();
                        game.held_mouse.clear();
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let (w, h) = (game.gpu.config.width.max(1) as f32, game.gpu.config.height.max(1) as f32);
                    game.cursor = (
                        (position.x as f32 / w) * 2.0 - 1.0,
                        1.0 - (position.y as f32 / h) * 2.0,
                    );
                    game.cursor_moved = true;
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let dy = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                        winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                    };
                    game.wheel(dy);
                }
                WindowEvent::Touch(t) => {
                    // normalized, y down — the layout is resolution-agnostic
                    let sz = win.inner_size();
                    let p = glam::Vec2::new(
                        t.location.x as f32 / sz.width.max(1) as f32,
                        t.location.y as f32 / sz.height.max(1) as f32,
                    );
                    if !game.tpad.enabled {
                        // a touchscreen announces itself: enable on first touch
                        game.tpad.enabled = true;
                        game.cfg.touch.enabled = true;
                    }
                    match t.phase {
                        winit::event::TouchPhase::Started => game.tpad.down(t.id, p),
                        winit::event::TouchPhase::Moved => game.tpad.moved(t.id, p),
                        _ => game.tpad.up(t.id),
                    }
                    if !matches!(game.mode, Mode::Play) {
                        // menus run on the cursor, so a finger IS a cursor —
                        // but taps, drags and scrubs are distinct gestures
                        game.cursor = (p.x * 2.0 - 1.0, 1.0 - p.y * 2.0);
                        game.cursor_moved = true;
                        match t.phase {
                            winit::event::TouchPhase::Started => game.touch_menu_down(),
                            winit::event::TouchPhase::Moved => game.touch_menu_move(),
                            _ => game.touch_menu_up(&win, elwt),
                        }
                    }
                }
                WindowEvent::MouseInput { state, button, .. } => match state {
                    ElementState::Pressed => {
                        game.mouse_down(button, &win);
                        game.held_mouse.insert(button);
                    }
                    ElementState::Released => {
                        game.held_mouse.remove(&button);
                        if button == MouseButton::Left {
                            game.release_trigger();
                        }
                    }
                },
                WindowEvent::RedrawRequested => {
                    game.tick_key_repeat(1.0 / 60.0, elwt, &win);
                    let now = Instant::now();
                    // 0.25 cap: the old 0.05 clamp DROPPED wall time below
                    // 20 fps, so lag literally slowed the game down
                    let dt = (now - last).as_secs_f32().min(0.25);
                    last = now;
                    game.frame(dt, &win);
                }
                _ => {}
            },
            Event::AboutToWait => win.request_redraw(),
            _ => {}
        })
        .unwrap();
}

/// Which model's dimensions describe the body as it is CURRENTLY POSED.
///
/// Not always the displayed one. A smooth fold keeps `cars.active` pinned to
/// the strip's first entry — the vehicle — and rewrites that model's meshes
/// into the robot shape each frame. The geometry on screen is a robot; the
/// `Model` whose `half_h` and `ride_h` would be read is a car.
///
/// While the fold is running that does not matter, because `advance_xform`
/// overwrites the dimensions from `Morph::bounds_at` every frame. It matters
/// the moment anything ELSE re-derives them — a garage switch, a car hotkey,
/// a reload — because `apply_car_dims` then seats a robot using the car's
/// numbers. For `analog` that is a `ride_h` of 0.239 against feet at 0.400:
/// sixteen centimetres underground, which is an order of magnitude more than
/// the 2.25 cm the half_h/ride_h guard bug could account for.
fn dims_model(displayed: usize, strip: Option<&[usize]>, robot: bool) -> usize {
    match strip {
        // only remap the strip's own base, and only while standing up
        Some(s) if robot && !s.is_empty() && displayed == s[0] => *s.last().unwrap(),
        _ => displayed,
    }
}

fn apply_car_dims(car: &mut Car, m: &model::Model, size: f32) {
    car.half_len = m.half_len * size;
    car.half_h = (m.half_h * size).max(0.10);
    car.half_w = (m.half_w * size).max(0.12);
    car.wheel_r = (m.wheel_r * size).max(0.05);
    car.drive = m.drive;
    car.hover = m.hover;
    // `flight` and a DETECTED cushion are applied by the caller
    // (`set_car_model`) through `locomotion`, because they depend on the
    // decision rather than on the model's own fields.
    // The other half of da831f98: that fix updated ride_h every frame
    // during a smooth-morph blend (xform::Morph::bounds_at) but never
    // here, in the static per-pose path every chassis switch runs
    // through — which is the ONLY seating input for a morph-refused
    // (stepping) chassis, and for any chassis in the instant before its
    // first blended frame. Same sign convention as bounds_at: negative
    // means the pose sits entirely above the origin, and the seating
    // code's own `> 0.0` check already treats that as "leave it be."
    car.ride_h = m.ride_h * size;
}

impl Game {
    /// Sync live settings into the save struct and write it to disk.
    fn persist(&mut self) {
        self.save.volume = self.snd.volume;
        self.save.music = self.snd.music_on;
        self.save.ball_cam = self.cam.ball_cam;
        self.save.car = self.cars.active;
        self.save.hotbar = self.hotbar.line();
        save::store(&self.save, &self.binds, &self.cfg);
    }

    fn toggle_fullscreen(&mut self, win: &winit::window::Window) {
        self.save.fullscreen = !self.save.fullscreen;
        win.set_fullscreen(if self.save.fullscreen {
            Some(Fullscreen::Borderless(None))
        } else {
            None
        });
    }

    fn style(&self) -> &'static world::MapStyle {
        world::style(self.save.map)
    }

    /// Regenerate the world for a map style + seed (terrain mesh swapped in
    /// place so the mesh registry stays stable).
    fn load_map(&mut self, map: usize, seed: u64) {
        self.load_map_at(map, seed, Vec3::ZERO);
    }

    /// THE WINDOW, REBUILT at `origin` on the world plane: the world,
    /// its terrain chunks, the reflection cells and the heightmap. This
    /// is the whole cost of a page — every bake the renderer needs, in
    /// one go, on the main thread. It hitches. The double-buffered,
    /// spread-over-frames version is the next step and is listed in
    /// `docs/VEHICLE_COMBAT.md`; this one exists so the seam is real and
    /// the hitch can be measured rather than imagined.
    fn load_map_at(&mut self, map: usize, seed: u64, origin: Vec3) {
        self.save.map = map % world::STYLE_COUNT;
        self.save.seed = seed;
        let style = world::style(self.save.map);
        self.world = World::generate_at(seed, style, self.cars.models[self.cars.active].half_h, self.cfg.arena.scale, origin);
        self.applied_scale = self.cfg.arena.scale;
        self.tchunks = terrain_chunks(&mut self.gpu, &self.world);
        self.gpu.set_rcells(&bake_rcells(&self.world));
        bake_heightmap(&self.gpu, seed, style, origin);
        let dims_i = self.cars.active;
        self.car.reset(&self.world);
        apply_car_dims(&mut self.car, &self.cars.models[dims_i], self.cfg.car.size);
        self.ball.reset_in_front(&self.car);
        // THE SHIP SURVIVES A MAP CHANGE. `World::generate` builds a fresh
        // world with `parked: None`, so loading a map silently unparked the
        // hull — it would still DRAW, because the mesh slot lives on the gpu,
        // and stop being solid. A hull you can see and walk through is worse
        // than one that is not there, because it reads as collision being
        // broken rather than as the ship being gone.
        self.repark();

        self.fx = fx::Fx::new(seed);
        self.bots.clear();
        self.game_mode = 0;
        self.scores = [0, 0];
        self.combo = 0;
        self.combo_t = 0.0;
        self.air_t = 0.0;
        self.air_link = 0.0;
        self.toastf(&format!("MAP: {}", style.name));
    }

    /// PAGING — the window follows the car. TENTATIVE SYSTEM.
    ///
    /// A car at 60 mph crosses the ±1300 m tile in a minute and a half,
    /// so on the district the tile is a window: when you are past
    /// `PAGE_AT` of the half-extent from its origin, the window is
    /// rebuilt centred on a lattice point near you and everything that
    /// lives in window-local coordinates is shifted by the same delta.
    /// The world plane is the truth; the window is a view of it
    /// (`World::generate_at`, `district::layout`'s world-coordinate
    /// hashing, `bake_heightmap`'s origin).
    ///
    /// What is shifted: the car, the ball, the bots, both shot pools, the
    /// sortie's drop and field (anchor, cover, solids, every foe), the
    /// ship's berth. What is NOT, and what that costs: the fx (a burst
    /// mid-flight jumps by the delta — one frame of confetti in the wrong
    /// place), the camera (it re-seats on the car next frame), the
    /// sandbox's sequential-rng furniture (rings, pads — a district has
    /// none, which is the reason paging is district-only for now).
    ///
    /// The lattice step is `PAGE_STEP` so origins are exact on the world
    /// plane and a page back lands on the same ground.
    fn page_window(&mut self) {
        const PAGE_AT: f32 = 0.45;
        const PAGE_STEP: f32 = 100.0;
        // district only: the other styles scatter their content off a
        // window-local rng and would be a different map after a page
        if self.world.boulevard.is_none() && self.world.plazas.is_empty() && self.style().focal != 25 {
            return;
        }
        if self.style().focal != 25 {
            return;
        }
        let p = self.car.pos;
        if p.x.abs() < world::TG_HALF * PAGE_AT && p.z.abs() < world::TG_HALF * PAGE_AT {
            return;
        }
        let delta = Vec3::new((p.x / PAGE_STEP).round() * PAGE_STEP, 0.0, (p.z / PAGE_STEP).round() * PAGE_STEP);
        if delta.length_squared() < 1.0 {
            return;
        }
        let origin = self.world.origin + delta;
        let map = self.save.map;
        let seed = self.save.seed;
        let parked = self.world.parked.as_ref().map(|(o, _)| *o);
        let t0 = Instant::now();
        // the bots list is cleared by load_map; keep the field's puppets
        let kept: Vec<bots::Bot> = self.bots.drain(..).filter(|b| b.brain.hostile_slot().is_some()).collect();
        self.load_map_at(map, seed, origin);
        self.bots = kept;
        // everything window-local moves the other way
        self.car.pos -= delta;
        self.ball.pos -= delta;
        for b in self.bots.iter_mut() {
            b.car.pos -= delta;
        }
        for sh in self.shots.live.iter_mut() {
            sh.pos -= delta;
        }
        for sh in self.hostile_shots.live.iter_mut() {
            sh.pos -= delta;
        }
        if let Some(s) = self.sortie.as_mut() {
            s.drop.pos -= delta;
            s.drop.lz -= delta;
            s.field.anchor -= delta;
            for c in s.field.cover.iter_mut() {
                *c -= delta;
            }
            s.field.avoid = self.world.obstacles.clone();
            for f in s.field.foes.iter_mut() {
                f.agent.position -= delta;
            }
        }
        // the ship berthed where it berthed: re-park at the shifted spot
        if let Some(o) = parked {
            self.world.park(o - delta, self.aboard.ship.clone());
            self.last_room = None;
            self.cam.indoors = None;
        }
        self.cam.pos -= delta;
        self.cam.look -= delta;
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        self.toastf(&format!("window paged {:+.0},{:+.0} — {ms:.0} ms", delta.x, delta.z));
    }

    /// Offline game modes: clear or spawn the bot armies.
    fn start_mode(&mut self, g: u8) {
        if self.net.online() {
            self.toastf("GAME MODES ARE OFFLINE ONLY - LEAVE THE SESSION FIRST");
            return;
        }
        match g {
            1 => {
                // any map: the armies fight wherever you already are
                self.bots = bots::spawn_sparta_50v50(&self.world);
                self.game_mode = 1;
                self.toastf("SPARTA: 50 V 50 - THIS MAP. YOU PLAY BLUE.");
            }
            2 => {
                self.bots = bots::spawn_sparta_300(&self.world);
                self.game_mode = 2;
                self.toastf("THIS. IS. SKYDRIBBLE. (300 BALLCHASERS)");
            }
            3..=7 => {
                let n = (g - 2) as usize; // 1v1 .. 5v5
                self.bots.clear();
                let home_z = |sign: f32| {
                    self.world
                        .nets
                        .iter()
                        .map(|nt| nt.pos)
                        .fold(Vec3::new(0.0, 2.0, 34.0 * sign), |best, p| if p.z * sign > best.z * sign { p } else { best })
                };
                let (blue_home, orange_home) = (home_z(-1.0), home_z(1.0));
                let bp = self.ball.pos;
                let spawn = |team: u8, k: usize, brain: bots::Brain, sel: &mut Self| {
                    let side = if team == bots::TEAM_BLUE { 1.0 } else { -1.0 };
                    // teammates spawn on the real surface too (a raised
                    // deck would otherwise swallow the whole lineup)
                    let px = (k as f32 - (n as f32 - 1.0) * 0.5) * 7.0;
                    let pz = bp.z + side * (24.0 + (k % 2) as f32 * 6.0);
                    let pos = sel.world.spawn_at(px, pz, 0.5);
                    let d = bp - pos;
                    let yaw = f32::atan2(-d.x, -d.z);
                    sel.bots.push(bots::mk_bot_brain(pos, yaw, team, sel.bots.len() + k * 13, brain));
                };
                let lineup = |i: usize, home: Vec3| -> bots::Brain {
                    match i {
                        0 => bots::Brain::Striker,
                        1 => bots::Brain::Defender { home },
                        2 => bots::Brain::Passer { next: 0 },
                        3 => bots::Brain::Striker,
                        _ => bots::Brain::Ballchase,
                    }
                };
                for k in 0..n.saturating_sub(1) {
                    spawn(bots::TEAM_BLUE, k + 1, lineup(k, blue_home), self);
                }
                for k in 0..n {
                    spawn(bots::TEAM_ORANGE, k, lineup(k, orange_home), self);
                }
                self.scores = [0, 0];
                self.game_mode = g;
                self.toastf(&format!("{n} V {n} - YOU PLAY BLUE. FIRST GOALS WIN HEARTS."));
            }
            _ => {
                self.bots.clear();
                self.toastf("FREE PLAY");
            }
        }
        self.persist();
        self.mode = Mode::Play;
    }

    fn toastf(&mut self, s: &str) {
        self.toast = s.to_string();
        self.toast_t = 4.0;
    }

    fn action_for_key(&self, k: &Key) -> Option<Action> {
        for (a, v) in &self.binds.map {
            if v.iter().any(|b| matches!(b, Bind::Key(bk) if bk == k)) {
                return Some(*a);
            }
        }
        None
    }
    fn action_for_mouse(&self, m: MouseButton) -> Option<Action> {
        for (a, v) in &self.binds.map {
            if v.iter().any(|b| matches!(b, Bind::Mouse(bm) if *bm == m)) {
                return Some(*a);
            }
        }
        None
    }

    fn action_down(&self, a: Action) -> bool {
        self.binds.binds(a).iter().any(|b| match b {
            Bind::Key(k) => self.held.contains(k),
            Bind::Mouse(m) => self.held_mouse.contains(m),
            Bind::Pad(c) => self.last_pad.down(*c),
        })
    }

    /// One-shot actions (edges): shared by keyboard, mouse and pad presses.
    fn one_shot(&mut self, a: Action) {
        match a {
            Action::Jump => self.jump_edge = true,
            Action::Minimap => self.save.minimap = !self.save.minimap,
            Action::Music => {
                let on = !self.snd.music_on;
                self.snd.set_music(on);
            }
            Action::VolDown => {
                let v = self.snd.volume - 0.1;
                self.snd.set_volume(v);
            }
            Action::VolUp => {
                let v = self.snd.volume + 0.1;
                self.snd.set_volume(v);
            }
            Action::Transform => self.transform(),
            Action::FlightMode => {
                if self.car.flight {
                    self.car.flight_submode = self.car.flight_submode.cycle();
                    self.car.recompute_hand();
                    self.toastf(&format!("FLIGHT MODE: {}", self.car.flight_submode.label()));
                }
                // silently does nothing outside flight -- same as PitchUp/
                // PitchDown, a player who never flies never hits this branch
            }
            Action::SiegeMode => {
                if self.car.tracked {
                    self.car.siege_mode = self.car.siege_mode.cycle();
                    self.car.recompute_hand();
                    self.toastf(&format!("TANK MODE: {}", self.car.siege_mode.label()));
                }
            }
            Action::Fire => self.fire_weapon(),
            Action::CycleForm => {
                if self.wep.watch.cycle() {
                    self.toastf(&format!("ATTUNING: {}", self.wep.watch.active().name()));
                }
            }
            Action::AutoTarget => {
                // in HOLD mode the press turns it on and the release turns
                // it off; the toggle path only runs in CYCLE mode
                if self.cfg.car.autotarget_hold {
                    self.autotarget = true;
                    self.toastf("AUTOTARGET");
                    return;
                }
                self.autotarget = !self.autotarget;
                if !self.autotarget {
                    self.wep.locked = None;
                    self.cam.poi = None;
                    self.cam.combat = false;
                }
                self.toastf(if self.autotarget { "AUTOTARGET ON" } else { "AUTOTARGET OFF" });
            }
            _ => {}
        }
    }

    fn switch_car(&mut self, i: usize) {
        if i < self.cars.models.len() {
            self.cars.active = i;
            // dimensions come from the pose, not from the model being drawn
            let strip = self
                .cars
                .xform
                .get(&i)
                .map(|&(cid, _)| self.cars.chassis[cid].strip.clone());
            let d = dims_model(i, strip.as_deref(), self.xf.is_robot());
            apply_car_dims(&mut self.car, &self.cars.models[d], self.cfg.car.size);
            // Flight is a property of the CHASSIS, not of the pose, so it is
            // read from the registry rather than from the model. A jet stays
            // a jet through every frame of its fold.
            // FLIGHT BELONGS TO THE VEHICLE, NOT THE CHASSIS. A bot that
            // has unfolded onto its legs is standing on the ground — it does
            // not float, and leaving the flag set was why a transformed
            // flyer hovered. The chassis says it CAN fly; the current mode
            // says whether it is.
            let chassis_flies = self
                .cars
                .xform
                .get(&i)
                .map_or(false, |&(c, _)| self.cars.chassis[c].flight);
            let robot = self
                .cars
                .xform
                .get(&i)
                .map_or(false, |&(c, k)| k * 2 > self.cars.chassis[c].strip.len() - 1);
            self.car.flight = chassis_flies && !robot;
            self.cam.nose_cam = chassis_flies && !robot;
            self.car.lift = None;
            self.lift = chassis_flies.then(|| {
                lift::Lift::classify(self.cars.names[i], self.cars.models[i].half_w / self.cars.models[i].half_len.max(0.01))
            });
            self.car.lift = self.lift;
            // ROBOT once past the halfway point of the strip: that is when
            // the body is on its feet, and it is what switches the flip into
            // a second jump and hands the camera to the mouse.
            self.car.robot = self
                .cars
                .xform
                .get(&i)
                .map_or(false, |&(c, k)| k * 2 > self.cars.chassis[c].strip.len() - 1);
            if !self.car.robot {
                self.cam.look_yaw = 0.0;
                self.cam.look_pitch = 0.0;
            }
        }
    }

    /// Swap the body under the player, then make room for it.
    ///
    /// A robot pose is roughly twice the height of the vehicle it folded out
    /// of, so the new hitbox will be buried in whatever the old one was
    /// resting on. Same lift `cycle_car` does — factored out because the
    /// transform needs it on every frame of the fold, not just once.
    fn set_car_model(&mut self, m: usize) {
        // THE MOVEMENT PIPELINE, run once per chassis switch:
        //
        //   cloud -> cockpit (which end is the bow)
        //         -> locomotion::decide (wheels? thrust? skirt?)
        //         -> physics (Car::hover / Car::flight)
        //
        // Until now only the five hand-written hovercars ever set a mode,
        // so every pose-strip chassis drove on invisible wheels — a
        // hovertank sank onto its skirt and a jet drove. The chassis
        // manifest still wins where it speaks (`declared`); detection
        // covers everything nobody will hand-tune.
        let cg = cockpit::signed_forward(&self.cars.models[m].cloud, Vec3::Y, Vec3::NEG_Z);
        self.cam.cockpit_local = if cg.confident { Some(cg.pos) } else { None };
        self.loco_mode = {
            let mdl = &self.cars.models[m];
            let declared = if mdl.hover.is_some() {
                Some(locomotion::Mode::Hover)
            } else if self
                .cars
                .xform
                .get(&m)
                .and_then(|(ci, _)| self.cars.chassis.get(*ci))
                .map(|c| c.floats)
                .unwrap_or(false)
            {
                // THE MIRROR OF THE FLIGHT CHECK BELOW. Only the five
                // hand-authored hovercars ever set `Model::hover` at load,
                // so every category-labelled hoverer/hovertank added since
                // depended entirely on `measured` below correctly reading
                // its skirt — the same gap `flight` already had and fixed
                // by reading `chassis.flight`. `cushion_for` builds a real,
                // reasonably-tuned `Hover` on the fly from the body's own
                // dimensions, the same as the geometry path would have, so
                // this is not a worse hover — it is the same one, just
                // guaranteed instead of gambled on shape-matching.
                Some(locomotion::Mode::Hover)
            } else if self
                .cars
                .xform
                .get(&m)
                .and_then(|(ci, _)| self.cars.chassis.get(*ci))
                .map(|c| c.flight)
                .unwrap_or(false)
            {
                Some(locomotion::Mode::Flight)
            } else {
                None
            };
            let measured = {
                let m = locomotion::from_cloud(&mdl.cloud, Vec3::Y, cg.forward, mdl.half_h);
                locomotion::decide_tracked(
                    mdl.fittings.tracked(),
                    m.wheels,
                    m.thrusters,
                    m.legs,
                    m.skirt,
                    m.thrust_aft,
                )
            };
            locomotion::decide_with_override(declared, measured)
        };
        self.switch_car(m);
        // THE DECISION REACHES THE PHYSICS. `switch_car` -> `apply_car_dims`
        // copies the model's own `hover`; this is where a DETECTED mode
        // lands, and where flight stops being a field nobody assigned.
        match self.loco_mode.mode {
            locomotion::Mode::Wheeled => {
                self.car.flight = false;
                // a declared cushion survives; a detected wheeled chassis
                // must not keep one from the model it replaced
                if self.cars.models[m].hover.is_none() {
                    self.car.hover = None;
                }
            }
            locomotion::Mode::Hover => {
                self.car.flight = false;
                if self.car.hover.is_none() {
                    self.car.hover =
                        Some(locomotion::cushion_for(self.car.half_h, self.car.half_len));
                }
            }
            locomotion::Mode::Flight => {
                self.car.flight = true;
                self.car.hover = None; // a flyer is not a cushion
            }
        }
        // AND WHAT IT IS LIKE TO DRIVE. Everything `driveline` needs was
        // measured on the way here: the hull's extents and wheel radius
        // from the model, treads from `turret`, wheels/thrusters and the
        // mode from `locomotion`. One more step, no new measurement.
        //
        // TRAITS RIDE ALONGSIDE THE GEOMETRY, same source as the
        // compaction-style lookup elsewhere in this file
        // (`bot_categories` -> `Traits::from_category`) — a tank's high
        // traction and a racer's low one used to make zero difference to
        // actual handling (flagged in TODO.md as dead data); this is
        // that wiring.
        let traits = self.bot_categories.get(self.cars.names[m])
            .map(|c| crate::chassis_traits::Traits::from_category(c))
            .unwrap_or_default();
        let tracked = self.cars.models[m].fittings.tracked();
        self.car.hand_base = driveline::derive(driveline::Shape {
            half_len: self.car.half_len,
            half_w: self.car.half_w,
            half_h: self.car.half_h,
            wheel_r: self.car.wheel_r,
            wheels: self.loco_mode.wheels,
            tracked,
            hovering: self.car.hover.is_some(),
            flying: self.car.flight,
            thrusters: self.loco_mode.thrusters,
            traction: traits.traction,
            mass_bias: traits.mass_bias,
        });
        // a chassis switch always lands in FastFly/Roll (== hand_base
        // unmultiplied) rather than carrying over whatever sub-mode the
        // PREVIOUS chassis happened to be in
        self.car.flight_submode = physics::FlightSubMode::default();
        self.car.tracked = tracked;
        self.car.siege_mode = physics::TankSiegeMode::default();
        self.car.recompute_hand();
        let h = self.car.hand;
        self.toastf(&format!(
            "{}: {:?} — {} · {:.1} t, accel x{:.2} top x{:.2} grip x{:.2}{}",
            self.cars.names[m],
            self.loco_mode.mode,
            self.loco_mode.why,
            h.mass_t,
            h.accel,
            h.top_speed,
            h.grip,
            if h.stall_ms > 0.0 {
                format!(" · stall {:.0} m/s, T/W {:.2}", h.stall_ms, h.twr)
            } else {
                String::new()
            }
        ));
        let d = self.world.sdf(self.car.pos);
        // THE GUARD HAS TO USE THE SAME NUMBER AS THE CORRECTION.
        //
        // This read `d < self.car.half_h` while correcting by `drop`, so a
        // body sunk by anything between the two — which for a standing robot
        // is a wide band, `half_h` 0.9 against a `ride_h` of 1.5 — was seen
        // as not sunk at all and never lifted. That is a PARTIAL burial that
        // corrects itself the moment the body is deep enough to trip the
        // smaller threshold, which is why it reads as the bot sinking rather
        // than as it spawning wrong.
        let drop = self.car.stand_h();
        if d < drop {
            // Seat on the LOWEST POINT, not the half-height. They differ the
            // moment the body stops being centred on its bounding box, which is
            // exactly what standing up does — feet hang below centre while the
            // mass sits high. Falls back to half_h when ride_h is unset so a
            // model that never folds is unaffected.
            self.car.pos += self.world.normal(self.car.pos) * (drop - d + 0.02);
        }
    }

    /// Vehicle <-> robot. Starts the fold; `advance_xform` plays it.
    ///
    /// Direction is decided by which end of the strip we are nearer, so the
    /// key is a toggle even if a previous transform was interrupted halfway.
    fn transform(&mut self) {
        if self.net.online() || self.mode != Mode::Play {
            return;
        }
        let Some(&(cid, _)) = self.cars.xform.get(&self.cars.active) else {
            self.toastf("this chassis does not transform");
            return;
        };
        if !self.cars.chassis[cid].transforms {
            // same message a non-bot car gets: the player-facing effect
            // (the key does nothing) is identical either way, and there
            // is no more to say about a boss that was authored to fight
            // in one form than there is about a car that was never a bot
            self.toastf("this chassis does not transform");
            return;
        }
        if self.morph.is_none() {
            self.build_morph();
        }
        // FOLD SPEED FROM THE CHASSIS. dur_mul has existed since the state
        // machine landed and nothing set it, so every bot folded at the same
        // rate. content::CHASSIS is where those numbers were: a juggernaut
        // takes 1.35x as long as standard, a raptor 0.8x.
        self.xf.dur_mul = self.chassis_fold_mul();
        if !self.xf.request(self.health) {
            // denied rather than ignored: say which, because "nothing
            // happened" is the worst feedback a button can give
            if self.xf.locked() {
                self.toastf("TRANSFORM LOCKED");
            }
            return;
        }
    }

    /// Which content chassis this bot fights as, by name match, falling back
    /// to `standard`. The bot strips are named for their source model, and
    /// the content table is named for body shapes, so this is a lookup that
    /// will want a real mapping once chassis assignment is authored rather
    /// than guessed.
    fn chassis_fold_mul(&self) -> f32 {
        let name = self.cars.names[self.cars.active].to_ascii_lowercase();
        content::CHASSIS
            .iter()
            .find(|c| name.contains(c.id))
            .or_else(|| content::chassis("standard"))
            .map(|c| c.xform_dur)
            .unwrap_or(1.0)
    }

    /// Degrees of spin `content::CHASSIS` says this fold carries. Same
    /// lookup as `chassis_fold_mul`, and the same gap: authored per body
    /// shape since the state machine landed, never read by anything until
    /// now — the cinematic camera's arc uses it so a whippy raptor fold
    /// visibly turns faster under the lens than a juggernaut's does,
    /// instead of the camera swinging the same arc for both.
    fn chassis_fold_spin(&self) -> f32 {
        let name = self.cars.names[self.cars.active].to_ascii_lowercase();
        content::CHASSIS
            .iter()
            .find(|c| name.contains(c.id))
            .or_else(|| content::chassis("standard"))
            .map(|c| c.fold_spin)
            .unwrap_or(220.0)
    }

    /// The active model's own liveliest colour, for the fold's spark burst.
    /// There is no single "accent" field on `Model` — colour lives per prim
    /// — so this picks the most saturated body prim rather than guessing at
    /// a convention that isn't there. Falls back to a generic warm spark for
    /// a body that turns out to be all greys and blacks (a stock racer,
    /// most tanks), so the burst is never garish on a car it doesn't suit.
    fn fold_spark_color(&self) -> [f32; 4] {
        const FALLBACK: [f32; 4] = [1.0, 0.85, 0.45, 1.0];
        let Some(m) = self.cars.models.get(self.cars.active) else { return FALLBACK };
        let mut best: Option<(f32, [f32; 4])> = None;
        for p in &m.prims {
            if p.tag != model::Tag::Body {
                continue; // tyres and hubs are deliberately neutral
            }
            let [r, g, b, _] = p.color;
            let mx = r.max(g).max(b);
            let mn = r.min(g).min(b);
            let sat = if mx > 0.02 { (mx - mn) / mx } else { 0.0 };
            // saturation carries the "this is the accent" signal; brightness
            // breaks ties so a dim accent doesn't beat a punchy one
            let score = sat * (0.4 + 0.6 * mx);
            if best.map_or(true, |(s, _)| score > s) {
                best = Some((score, p.color));
            }
        }
        match best {
            Some((sat, c)) if sat > 0.12 => [c[0], c[1], c[2], 1.0],
            _ => FALLBACK,
        }
    }

    /// Load the active chassis' strip vertices so the fold can blend.
    ///
    /// One parse per pose (~3 ms each measured), done on demand rather than
    /// at startup, so a player who never transforms never pays for it.
    fn build_morph(&mut self) {
        let Some(&(cid, _)) = self.cars.xform.get(&self.cars.active) else { return };
        let dirs = self.cars.chassis[cid].dirs.clone();
        if dirs.len() < 2 {
            return;
        }
        let mut strip: Vec<Vec<model::RawPrim>> = Vec::new();
        for (d, target) in dirs.iter() {
            let (raws, _) = model::load_raw(d, *target);
            strip.push(raws);
        }
        // ALIGNMENT FIRST. No shipped strip agrees prim-for-prim — pose
        // counts differ and orders permute — so handing raw poses to
        // Morph::build refused every chassis and the game has been
        // stepping the whole time (measured; see align.rs). unify finds
        // the union-prim correspondence and emits exactly the
        // uniform-shape input Morph requires. Its refusal is the same
        // stepping fallback as before, so this path can only improve.
        let Some(u) = crate::align::unify(strip) else {
            self.morph = None;
            self.toastf("strip mismatch — stepping");
            return;
        };
        // UNION PRIMS BEYOND THE BASE MODEL NEED DRAW SLOTS. The blend
        // path writes mesh `p` of the displayed model from union prim
        // `p`, and pose-specific parts (present only mid-strip) have no
        // mesh on the vehicle model to write. Append one per extra,
        // seeded with its own pose-0 placeholder (a collapsed point —
        // invisible until the fold brings it out). Idempotent: a second
        // build_morph on the same chassis finds the slots already there.
        //
        // Extras are forced to Tag::Body whatever their source pose
        // called them: the wheel-spin and roller paths key off tags, and
        // a transitional panel that spins like a wheel mid-fold is worse
        // than one that merely moves. Textures are not carried over —
        // they live in the source POSE's directory and the base model's
        // cache never saw them — so extras draw flat-coloured. Colour is
        // kept; if flat panels read wrong on real hardware, the texture
        // load belongs here, where `self.gpu` and `dirs` are both in
        // scope.
        let base = self.cars.chassis[cid].strip[0];
        let have = self.cars.models[base].prims.len();
        let want = u.base_prims + u.extras.len();
        if have == u.base_prims {
            for (e, extra) in u.extras.iter().enumerate() {
                let p = u.base_prims + e;
                let md = meshgen::MeshData {
                    verts: u.poses[0][p].clone(),
                    idx: u.idx[p].clone(),
                };
                let mesh_id = self.gpu.add_mesh(&md);
                self.cars.models[base].prims.push(model::Prim {
                    mesh_id,
                    tex_id: None,
                    color: extra.color,
                    tag: model::Tag::Body,
                    pivot: Vec3::ZERO,
                });
            }
        } else if have != want {
            // the base model's prim list is not what alignment mapped
            // against — writing meshes by index would scramble parts, so
            // refuse into the known fallback rather than blend garbage
            self.morph = None;
            self.toastf("strip slot mismatch — stepping");
            return;
        }
        self.morph = xform::Morph::build(u.poses, u.idx);
        // COMPACTION ON, which it never has been: Morph::build sets
        // `compaction: None` and nothing assigned it, so compact.rs — 
        // written for exactly the "flat vehicle panels still hanging off the
        // robot" problem — has been dead code since it landed.
        //
        // Style::Shrink is the one that matters here: "panels compact inward —
        // vehicle bodywork folding into a smaller volume". A door that has
        // nowhere to go collapses toward the body's own centre rather than
        // staying at vehicle-mode extent while the body underneath it stands up.
        //
        // min_thickness is the other half, and it is the same mechanism rather
        // than a separate mesh-repair pass: the project's own thin-shell audit
        // found a "barrel" measuring 1.049 x 0.267 x 0.005, and "a
        // zero-thickness shell is invisible edge-on, shades wrong, and reads as
        // a paper cut-out at exactly the moment a transformation puts it
        // side-on to the camera".
        //
        // The defaults are eye-tuned rather than guessed — cell 0.35 with
        // strength 0.55 "was the pair that read as geometric compaction on
        // every rig rather than as noise on the small ones and mush on the
        // large ones", across six body types. An earlier 0.22/0.85 pass was
        // picked before anything could be seen and was wrong in both
        // directions, so these are left alone.
        // PER CHASSIS, not one global default. compact.rs says its two styles
        // "should not be one knob" and my first pass set one anyway; the trait
        // vector already knows how much bodywork a body has, which is exactly
        // what decides whether panels fold inward or limbs push outward.
        if let Some(m) = self.morph.as_mut() {
            let traits = self.cars.names.get(self.cars.active)
                .and_then(|n| self.bot_categories.get(*n))
                .map(|c| crate::chassis_traits::Traits::from_category(c))
                .unwrap_or_default();
            m.compaction = Some(traits.compaction());
        }
        if self.morph.is_none() {
            // a strip whose poses disagree cannot be blended; stepping it is
            // still better than blending garbage
            self.toastf("strip mismatch — stepping");
        }
    }

    /// Build the walk morph for the active chassis: the walk strip with
    /// pose 0 appended again so phase 1.0 lands where phase 0.0 started —
    /// a stride is a loop and a linear strip is not. Same alignment, same
    /// refusal semantics as the fold: `unify` says no, the flipbook stays.
    fn build_walk_morph(&mut self) {
        let Some(&(cid, _)) = self.cars.xform.get(&self.cars.active) else { return };
        let wd = self.cars.chassis[cid].walk_dirs.clone();
        if wd.len() < 2 {
            return;
        }
        let mut strip: Vec<Vec<model::RawPrim>> = Vec::new();
        for (d, target) in wd.iter().chain(std::iter::once(&wd[0])) {
            let (raws, _) = model::load_raw(d, *target);
            strip.push(raws);
        }
        let Some(u) = crate::align::unify(strip) else { return };
        // A DEDICATED MODEL for the blend. The union's first `base_prims`
        // slots mirror walk pose 0's prims (same order, same materials);
        // extras get flat-coloured Body slots, the same trade the fold
        // makes and for the same reason (their textures live in another
        // pose's directory).
        let w0 = self.cars.chassis[cid].walk[0];
        let mut prims: Vec<model::Prim> = Vec::new();
        for (p, src) in self.cars.models[w0].prims.iter().take(u.base_prims).enumerate() {
            let md = meshgen::MeshData { verts: u.poses[0][p].clone(), idx: u.idx[p].clone() };
            prims.push(model::Prim { mesh_id: self.gpu.add_mesh(&md), ..*src });
        }
        for (e, extra) in u.extras.iter().enumerate() {
            let p = u.base_prims + e;
            let md = meshgen::MeshData { verts: u.poses[0][p].clone(), idx: u.idx[p].clone() };
            prims.push(model::Prim {
                mesh_id: self.gpu.add_mesh(&md),
                tex_id: None,
                color: extra.color,
                tag: model::Tag::Body,
                pivot: Vec3::ZERO,
            });
        }
        if prims.len() != u.poses[0].len() {
            return; // the mapping does not hold; keep the flipbook
        }
        let bm = self.cars.models.len();
        let src = &self.cars.models[w0];
        let clone = model::Model {
            prims,
            drive: src.drive,
            hover: None,
            settle: src.settle,
            tuck: src.tuck.clone(),
            pack: src.pack.clone(),
            fittings: src.fittings.clone(),
            hardpoints: src.hardpoints.clone(),
            cloud: Vec::new(),
            cloud_n: Vec::new(),
            sdf_half: Vec3::ZERO,
            half_len: src.half_len,
            half_h: src.half_h,
            half_w: src.half_w,
            wheel_r: src.wheel_r,
            ride_h: src.ride_h,
        };
        self.cars.models.push(clone);
        self.cars.names.push(Box::leak(
            format!("{} WALK BLEND", self.cars.names[w0]).into_boxed_str(),
        ));
        if let Some(m) = xform::Morph::build(u.poses, u.idx) {
            self.walk_morph = Some((cid, bm, m));
        }
    }

    /// Per tick: keep the blend model's meshes at the player's phase.
    /// Lazy-builds on the first tick a walking body needs it, and stands
    /// down (keeping the built morph) whenever the flipbook path would
    /// not be walking either.
    fn advance_walk(&mut self) {
        let Some(&(cid, k)) = self.cars.xform.get(&self.cars.active) else { return };
        let ch = &self.cars.chassis[cid];
        if ch.walk_dirs.len() < 2 || k + 1 != ch.strip.len() {
            return;
        }
        match &self.walk_morph {
            Some((mc, _, _)) if *mc == cid => {}
            _ if self.walk_morph_tried == Some(cid) => return, // refused once; stay a flipbook
            _ => {
                // one chassis at a time, same residency rule as the fold's
                // morph — the roster's walks resident would be for no reason
                self.walk_morph = None;
                self.walk_morph_tried = Some(cid);
                self.build_walk_morph();
            }
        }
        let Some((_, bm, m)) = &mut self.walk_morph else { return };
        let t = self.loco_pose.phase.rem_euclid(1.0);
        let ids: Vec<usize> = self.cars.models[*bm].prims.iter().map(|p| p.mesh_id).collect();
        for (p, mesh_id) in ids.iter().enumerate().take(m.prims()) {
            let md = m.sample(p, t);
            self.gpu.replace_mesh(*mesh_id, &md);
        }
    }

    /// Step the running fold, swapping the body when the strip index changes.
    fn advance_xform(&mut self, dt: f32) {
        // THE FOLD MUST RENDER ITS LAST FRAME.
        //
        // `Xform::update` flips `state` to Idle on the SAME call that `t`
        // reaches 1.0 or 0.0, so gating on `state == Transforming` after
        // calling it means the endpoint pose is never written: the final
        // mesh write is the previous frame's, short of the target by
        // however much of the blend one frame covers.
        //
        // That is both reported bugs at once. Folding to vehicle leaves
        // limbs a few percent out of their sockets — parts visibly outside
        // the car — and folding to robot leaves `half_h`/`ride_h` at the
        // second-to-last frame's values, so the body is seated against a
        // hitbox that never finished growing.
        //
        // Run the body once more on the frame the fold ends. `t` is already
        // clamped to the endpoint by then, so this is the exact final pose
        // and not an extrapolation.
        let was_folding = self.xf.state == xform::State::Transforming;
        let t_before = self.xf.eased();
        self.xf.update(dt);
        let now_folding = self.xf.state == xform::State::Transforming;

        // THE FOLD SHOT. Take the camera on the frame the fold starts, hand
        // it back on the frame it ends — `end_cinematic` is safe to call
        // even when no cinematic is running, so this doesn't need its own
        // "was it mine to give back" bookkeeping.
        if now_folding && !was_folding {
            self.cam.start_cinematic(self.car.pos.x + self.car.pos.z, self.chassis_fold_spin());
            self.fx.ring_burst(self.car.pos, self.fold_spark_color(), false);
            self.cam.add_shake(0.25);
            self.snd.fire(Sfx::XformFold(self.xf.folding_to_robot()));
        }
        if !now_folding && was_folding {
            self.cam.end_cinematic();
            self.fx.ring_burst(self.car.pos, self.fold_spark_color(), true);
            self.fx.dust(self.car.pos, self.world.normal(self.car.pos), 6.0);
            self.cam.add_shake(0.55);
            self.snd.fire(Sfx::XformSnap);
        }
        if !now_folding && !was_folding {
            return;
        }
        let t = self.xf.eased();
        // SNAP DETECTION. `Profile::ease` already carries the personality —
        // Combat's hard stop and Anger's overshoot both show up as a jump in
        // `t` larger than a steady blend would produce over one frame. Spike
        // the shake and punch the lens in proportional to how sharp that
        // jump is, rather than re-deriving the same beats from `Profile`
        // itself and risking the two disagreeing.
        if dt > 1e-5 {
            let rate = ((t - t_before).abs() / dt - 1.0 / xform::BLEND_SEC).max(0.0);
            if rate > 0.15 {
                let energy = (rate * 0.9).min(1.0);
                self.cam.add_shake(energy * 0.35);
                self.cam.fov_punch = self.cam.fov_punch.max(energy * 9.0);
            }
        }
        // SPEED WIDENS THE LENS: boosting in vehicle form at road speed
        // pulls the FOV out by up to 9 degrees — the punch's sign is
        // "narrower", so a widen is a negative punch, and the same decay
        // brings both back. Not in robot form: a walking body does not
        // stretch the frame.
        if !self.car.robot && self.car.boost > 0.0 && self.action_down(Action::Boost) {
            let widen = -(self.car.vel.length() / 40.0).clamp(0.0, 1.0) * 9.0;
            self.cam.fov_punch = self.cam.fov_punch.min(widen);
        }
        self.cam.fov_punch *= (1.0 - 6.0 * dt).max(0.0);
        let Some(&(cid, _)) = self.cars.xform.get(&self.cars.active) else { return };

        // BLENDED PATH: rewrite the displayed model's meshes each frame, so
        // the body moves continuously instead of snapping between poses.
        if self.morph.is_some() {
            let base = self.cars.chassis[cid].strip[0];
            if self.cars.active != base {
                self.set_car_model(base);
            }
            let ids: Vec<usize> =
                self.cars.models[base].prims.iter().map(|p| p.mesh_id).collect();
            // A PARTIAL BLEND IS WORSE THAN NO BLEND. If the displayed model
            // has more prims than the strip does, `take` would leave the
            // extras holding their VEHICLE geometry while everything else
            // folds — a shell of the old body left attached to the new one,
            // thin and easy to miss from most angles. Refuse instead, and
            // fall back to stepping, which at least shows one whole body.
            if self.morph.as_ref().map_or(false, |m| m.prims() != ids.len()) {
                self.morph = None;
                self.toastf("strip prim mismatch — stepping");
            }
            if let Some(m) = &mut self.morph {
                for (p, mesh_id) in ids.iter().enumerate().take(m.prims()) {
                    let md = m.sample(p, t);
                    self.gpu.replace_mesh(*mesh_id, &md);
                    let _ = mesh_id;
                }
                // the hitbox tracks the blended body rather than jumping at
                // pose boundaries
                // One sample for both numbers: bounds_at returns the
                // half-extents and the drop to the lowest point together,
                // because walking every prim at `t` is the cost and doing it
                // twice would double it on every folding frame.
                let (hb, drop) = m.bounds_at(t);
                // "Expanding the long way, then shrinking back a ton": the
                // raw lerp's length can overshoot both end poses mid-fold
                // (a part in transit toward a far-off target pushes the
                // silhouette out), and it snaps back the moment the fold
                // finishes. Cap it to the straight-line envelope between
                // this chassis's own vehicle and robot lengths, and fold
                // the overflow into height instead of discarding it — see
                // xform::redistribute_length_to_height's doc comment.
                let strip = &self.cars.chassis[cid].strip;
                let veh_len = self.cars.models[strip[0]].half_len;
                let bot_len = self.cars.models[*strip.last().unwrap()].half_len;
                // THE TWO TRANSFORMS WERE FIGHTING, and this is where.
                //
                // `redistribute_length_to_height` stretches the blended
                // half-extents — the overflow from a mid-fold body that is
                // momentarily too long gets folded into HEIGHT. The drop to
                // the lowest point was passed through untouched, so during
                // every blended frame `half_h` grew while `ride_h` did not,
                // and the body was seated by a ride height that belonged to
                // a shorter body than the one on screen. The static path has
                // no redistribution, which is exactly why the old static
                // pose sits on the ground and the blended one does not.
                //
                // Same stretch, same number: whatever the redistribution
                // does to height, it does to the ride.
                let raw_y = hb.y.max(1e-4);
                let hb = xform::redistribute_length_to_height(hb, veh_len, bot_len, t);
                let stretch = (hb.y / raw_y).clamp(0.5, 3.0);
                let h = hb * self.cfg.car.size;
                self.car.half_w = h.x.max(0.12);
                self.car.half_h = h.y.max(0.10);
                self.car.half_len = h.z.max(0.10);
                self.car.ride_h = (drop * stretch * self.cfg.car.size).max(0.10);
            }
        } else {
            // FALLBACK: no morph built, step the strip as before
            let n = self.cars.chassis[cid].strip.len();
            let want = ((t * (n - 1) as f32).round() as usize).min(n - 1);
            let mm = self.cars.chassis[cid].strip[want];
            if mm != self.cars.active {
                self.set_car_model(mm);
            }
        }

        self.car.robot = self.xf.is_robot();
        // mid-fold the same rule applies: legs down means no lift
        if self.car.robot {
            self.car.flight = false;
            self.cam.nose_cam = false;
        }
        if !self.car.robot {
            self.cam.look_yaw = 0.0;
            self.cam.look_pitch = 0.0;
        }
        // lift clear as the body grows, or a standing robot ends up buried
        let d = self.world.sdf(self.car.pos);
        // THE GUARD HAS TO USE THE SAME NUMBER AS THE CORRECTION.
        //
        // This read `d < self.car.half_h` while correcting by `drop`, so a
        // body sunk by anything between the two — which for a standing robot
        // is a wide band, `half_h` 0.9 against a `ride_h` of 1.5 — was seen
        // as not sunk at all and never lifted. That is a PARTIAL burial that
        // corrects itself the moment the body is deep enough to trip the
        // smaller threshold, which is why it reads as the bot sinking rather
        // than as it spawning wrong.
        let drop = self.car.stand_h();
        if d < drop {
            // Seat on the LOWEST POINT, not the half-height. They differ the
            // moment the body stops being centred on its bounding box, which is
            // exactly what standing up does — feet hang below centre while the
            // mass sits high. Falls back to half_h when ride_h is unset so a
            // model that never folds is unaffected.
            self.car.pos += self.world.normal(self.car.pos) * (drop - d + 0.02);
        }
        if self.xf.state == xform::State::Idle {
            let nm = if self.xf.mode == xform::Mode::Robot { "ROBOT" } else { "VEHICLE" };
            self.toastf(nm);
        }
    }

    /// Step through the garage with `[` and `]`.
    ///
    /// Only in singleplayer: in a net session everyone else has been told
    /// which car you are driving, and swapping it mid-match would desync the
    /// hitbox they are predicting against.
    fn cycle_car(&mut self, dir: i32) {
        if self.net.online() || self.mode != Mode::Play {
            return;
        }
        let n = self.cars.cycle.len();
        if n == 0 {
            return;
        }
        // Mid-fold, `[`/`]` should still land somewhere sensible, so an
        // unlisted pose resolves to its chassis' vehicle pose first.
        let cur = self
            .cars
            .cycle
            .iter()
            .position(|&m| m == self.cars.active)
            .or_else(|| {
                let &(cid, _) = self.cars.xform.get(&self.cars.active)?;
                let veh = self.cars.chassis[cid].strip[0];
                self.cars.cycle.iter().position(|&m| m == veh)
            })
            .unwrap_or(0);
        let next = ((cur as i32 + dir).rem_euclid(n as i32)) as usize;
        let m = self.cars.cycle[next];
        // changing chassis abandons any fold AND its morph: the strip we
        // had loaded belongs to a body we are no longer wearing
        self.xf = xform::Xform::new();
        self.morph = None;
        self.set_car_model(m);
        self.save.car = m;
        // v1.117's richer cycle toast: class and blurb are what you want
        // when [ and ] exist for trying things without opening a menu. `m`
        // is the model, `next` its place in the cycle list — my cycle_car
        // walks cars.cycle rather than every model, so the two indices are
        // not the same and their version used the raw slot for both.
        self.toastf(&format!(
            "{} [{}] {}/{} - {}",
            self.cars.names[m],
            self.cars.class_of(m),
            next + 1,
            n,
            self.cars.blurb_of(m)
        ));
    }

    /// The blade lands: everything in reach inside the front arc takes a
    /// Melee hit — the one Kind that severs, per `sections`' own rule —
    /// and a shove along the cut. Reach is the weapon's own number.
    fn melee_arc(&mut self) {
        let p = self.wep.effective();
        let fwd = self.car.forward();
        let reach = p.reach.max(3.0);
        let dmg = p.impulse * IMPULSE_TO_DAMAGE;
        let mut struck: Vec<(usize, Vec3)> = Vec::new();
        for (i, b) in self.bots.iter().enumerate() {
            let to = b.car.pos - self.car.pos;
            if to.length() < reach + b.car.half_len && to.normalize_or(fwd).dot(fwd) > 0.35 {
                struck.push((i, to.normalize_or(fwd)));
            }
        }
        let any = !struck.is_empty();
        for (i, dir) in struck {
            let b = &mut self.bots[i];
            let _ = b.body.hit(sections::Kind::Melee, Vec3::new(0.0, 1.0, 0.0), dir, dmg, 0.0);
            b.car.vel += dir * (p.impulse * 0.5);
        }
        if any {
            self.fx.ring_burst(self.car.pos + fwd * (reach * 0.6), [1.0, 0.85, 0.3, 1.0], false);
        }
    }

    /// Close the console AND refresh the shell's own hotbar from the save
    /// — the armory can rearrange the slots now, and a hotbar the deck
    /// edited while the shell kept its stale copy is the drift this
    /// helper exists to prevent. ONE close path.
    fn close_deck(&mut self) {
        if self.deck.take().is_some() {
            self.hotbar = hotbar::Hotbar::from_line(&save::load().0.hotbar);
        }
    }

    /// A player hit landed on bot `id`: if it is a field hostile, land it
    /// on the fight through the sortie's aimed door. Section damage and
    /// the shove stay with the bot (that is what the renderer shows);
    /// whether the hostile DIES is the fight's call, mirrored back by
    /// `field::sync`.
    fn hostile_hit(&mut self, id: usize, dmg: f32, elem: weapons::Element, reg: weapons::Register) {
        let Some(slot) = self.bots.get(id).and_then(|b| b.brain.hostile_slot()) else { return };
        if let Some(s) = self.sortie.as_mut() {
            s.strike_slot(slot, dmg, elem, reg);
        }
    }

    /// Damage to the player's own body from the field — a hostile shot,
    /// a lunge, a ram. One place, so the death check is one place too.
    fn player_take(&mut self, dmg: f32, at: Vec3, kind: sections::Kind) {
        if dmg <= 0.0 {
            return;
        }
        let was = self.body.alive();
        let local = (self.car.rot.inverse() * (at - self.car.pos)).clamp_length_max(1.0);
        let _ = self.body.hit(kind, local, Vec3::Y, dmg, 0.6);
        self.fx.ring_burst(at, [1.0, 0.35, 0.2, 1.0], false);
        // MOTION WEIGHT: a hit you take shakes the lens in proportion, and
        // a heavy one holds the frame
        self.cam.add_shake((dmg / 45.0).clamp(0.12, 0.9));
        if dmg >= 25.0 {
            self.hitstop = self.hitstop.max(0.05);
        }
        if was && !self.body.alive() {
            if let Some(s) = self.sortie.as_mut() {
                s.player_down();
            } else {
                self.toastf("STRUCTURAL FAILURE — grounded");
            }
        }
    }

    /// THE WAVE, STEPPED. The field is handed the player and hands back
    /// events; this applies them — hostile fire into its own pool, lunges
    /// and rams onto the body, the player's ram onto the fight — and then
    /// mirrors every living foe onto a `Brain::Hostile` bot so the
    /// renderer, the section damage and the shot marks all see the same
    /// body. Outside an engagement it only sweeps up: leftover puppets
    /// and in-flight hostile shots go, so an abandoned sortie does not
    /// leave a wave standing in the sandbox.
    fn tick_field(&mut self, dt: f32) {
        let engaged = self.sortie.as_ref().map(|s| s.engaged()).unwrap_or(false);
        if !engaged {
            if self.bots.iter().any(|b| b.brain.hostile_slot().is_some()) {
                self.bots.retain(|b| b.brain.hostile_slot().is_none());
            }
            self.hostile_shots.live.clear();
            return;
        }
        let ppos = self.car.pos;
        let pvel = self.car.vel;
        let player = field::Player {
            pos: ppos,
            vel: pvel,
            vehicle: !self.car.robot,
            // the cloak ability exists in the tables and not on the hotbar yet
            cloaked: false,
            mass: cable::CAR_MASS,
        };
        let world_g = self.world_g;
        // THROUGH THE SEAM: the field asks the host, not the tile
        let world: &dyn worldhost::WorldHost = &self.world;
        let ground = |x: f32, z: f32| world.ground_h(x, z);
        let los = |a: Vec3, b: Vec3| world.line_of_sight(a, b);
        let ev = {
            let s = self.sortie.as_mut().unwrap();
            s.field.g = world_g;
            s.field.step(dt, &player, &ground, &los)
        };
        for e in ev {
            match e {
                field::Event::Fire { from, dir, verb, element, dmg, slot } => {
                    // a hostile driving in vehicle form shoots from ITS
                    // hull's muzzle, not from a chest it does not have
                    let (from, dir) = match self.bots.iter().find(|b| b.brain.hostile_slot() == Some(slot)) {
                        Some(b) if !b.car.robot => {
                            let hp = &self.cars.models[b.model.min(self.cars.models.len() - 1)].hardpoints;
                            match hp.muzzle(slot + self.hostile_shots.live.len()) {
                                Some(m) => {
                                    let at = b.car.pos + b.car.rot * m.pos;
                                    // aim stays at the player; the mount only moves the origin
                                    (at, (from + dir * 40.0 - at).normalize_or(dir))
                                }
                                None => (from, dir),
                            }
                        }
                        _ => (from, dir),
                    };
                    // a hostile beam flies as a very fast slug so one pool
                    // carries every verb; speeds are GUESSES sized to be
                    // dodgeable at range and not up close
                    let deliv = match verb.delivery() {
                        ballistics::Delivery::Melee => ballistics::Delivery::Kinetic,
                        d => d,
                    };
                    let speed = match deliv {
                        ballistics::Delivery::Beam => 220.0,
                        ballistics::Delivery::Blast => 55.0,
                        _ => 120.0,
                    };
                    self.hostile_shots.spawn(from, dir, deliv, speed, dmg / IMPULSE_TO_DAMAGE, element, &self.ambient, None, None);
                }
                field::Event::Melee { dmg, .. } => {
                    self.player_take(dmg, ppos, sections::Kind::Kinetic);
                }
                field::Event::Ram { dv, dmg, .. } => {
                    self.car.vel += dv;
                    self.player_take(dmg, ppos, sections::Kind::Kinetic);
                    self.hitstop = self.hitstop.max(0.07);
                    self.cam.fov_punch = self.cam.fov_punch.max(6.0);
                }
                field::Event::Rammed { slot, dv, dmg } => {
                    if let Some(s) = self.sortie.as_mut() {
                        s.strike_slot(slot, dmg, weapons::Element::None, weapons::Register::Heavy);
                    }
                    if let Some(b) = self.bots.iter_mut().find(|b| b.brain.hostile_slot() == Some(slot)) {
                        b.car.vel += dv;
                        let at = b.car.pos;
                        self.fx.ring_burst(at, [1.0, 0.8, 0.35, 1.0], true);
                    }
                    // you take a little of your own ram, and lose speed
                    self.car.vel *= 0.8;
                    self.hitstop = self.hitstop.max(0.06);
                    self.cam.add_shake(0.5);
                }
                field::Event::Folded { slot, .. } => {
                    if let Some(f) = self.sortie.as_ref().and_then(|s| s.field.foe(slot)) {
                        let at = f.pos();
                        self.fx.ring_burst(at, [0.7, 0.85, 1.0, 1.0], false);
                    }
                }
                field::Event::Down { slot } => {
                    if let Some(i) = self.bots.iter().position(|b| b.brain.hostile_slot() == Some(slot)) {
                        let at = self.bots[i].car.pos;
                        self.fx.ring_burst(at, [1.0, 0.55, 0.15, 1.0], true);
                        self.bots.swap_remove(i);
                    }
                }
                field::Event::Staggered { slot } => {
                    if let Some(f) = self.sortie.as_ref().and_then(|s| s.field.foe(slot)) {
                        let at = f.pos() + Vec3::Y * field::CHEST_H;
                        self.fx.ring_burst(at, [1.0, 0.95, 0.6, 1.0], false);
                    }
                    // a heavy hit that lands holds a frame or two
                    self.hitstop = self.hitstop.max(0.04);
                }
                // the hostile's cable on you: drawn as a beam-ish streak
                // each tick it holds (the fx system has no persistent line
                // primitive; `FxKind::Cable` lives in the weapon fx and
                // is not reachable from here) — a burst at the muzzle and
                // a toast on the throw, then the pull is felt not seen.
                // A real cable draw is a renderer job, listed in the doc.
                field::Event::CableOut { slot, from } => {
                    self.fx.ring_burst(from, [0.85, 0.78, 0.45, 1.0], false);
                    self.toastf(&format!("CABLE — hostile {} has a line on you", slot + 1));
                }
                field::Event::Pulled { dv, .. } => {
                    self.car.vel += dv;
                }
                field::Event::CableOff { .. } => {}
                field::Event::Hooked { slot } => {
                    if let Some(f) = self.sortie.as_ref().and_then(|s| s.field.foe(slot)) {
                        let at = f.pos();
                        self.fx.ring_burst(at, [0.85, 0.78, 0.45, 1.0], false);
                    }
                    self.toastf("HOOKED — hold to tow, turn to load, release to throw");
                }
                field::Event::Yanked { dv, .. } => {
                    self.car.vel += dv;
                }
                field::Event::Towed { slot, dmg } => {
                    if let Some(s) = self.sortie.as_mut() {
                        s.strike_slot(slot, dmg, weapons::Element::None, weapons::Register::Medium);
                    }
                    if let Some(b) = self.bots.iter().find(|b| b.brain.hostile_slot() == Some(slot)) {
                        let at = b.car.pos;
                        self.fx.ring_burst(at, [0.9, 0.6, 0.3, 0.8], false);
                    }
                }
                field::Event::Whipped { slot, .. } => {
                    self.toastf(&format!("WHIP — hostile {} is airborne", slot + 1));
                }
                field::Event::Thrown { slot, into, dmg } => {
                    if let Some(s) = self.sortie.as_mut() {
                        s.strike_slot(slot, dmg, weapons::Element::None, weapons::Register::Heavy);
                        if let Some(other) = into {
                            s.strike_slot(other, dmg, weapons::Element::None, weapons::Register::Heavy);
                        }
                    }
                    if let Some(b) = self.bots.iter().find(|b| b.brain.hostile_slot() == Some(slot)) {
                        let at = b.car.pos;
                        self.fx.ring_burst(at, [1.0, 0.7, 0.3, 1.0], true);
                    }
                    if into.is_some() {
                        self.toastf("thrown INTO another — both wear it");
                        self.hitstop = self.hitstop.max(0.08);
                        self.cam.add_shake(0.6);
                    }
                }
            }
        }

        // HOSTILE SHOTS fly the same medium and gravity as yours, against
        // one mark: you. A blast in the dirt beside you still bites.
        if !self.hostile_shots.live.is_empty() {
            let marks = [shots::Mark { pos: ppos, vel: pvel, r: self.car.half_len.max(self.car.half_h) }];
            // a fresh closure: the one above ended with the field step
            let world: &dyn worldhost::WorldHost = &self.world;
            let ground = |x: f32, z: f32| world.ground_h(x, z);
            let hits = self.hostile_shots.step(dt, self.world_g, &self.ambient, &marks, &ground);
            for h in hits {
                let amount = h.dv.length() * IMPULSE_TO_DAMAGE;
                let kind = match h.delivery {
                    ballistics::Delivery::Blast => sections::Kind::Blast,
                    ballistics::Delivery::Beam => sections::Kind::Beam,
                    _ => sections::Kind::Kinetic,
                };
                match h.target {
                    Some(_) => {
                        self.car.vel += h.dv * 0.5;
                        self.player_take(amount, h.at, kind);
                    }
                    None => {
                        self.fx.ring_burst(h.at, [0.85, 0.7, 0.45, 1.0], false);
                        const SPLASH_R: f32 = 4.0;
                        let d = (h.at - ppos).length();
                        if h.delivery == ballistics::Delivery::Blast && d <= SPLASH_R && amount > 0.0 {
                            self.player_take(amount * (1.0 - d / SPLASH_R) * 0.5, h.at, sections::Kind::Blast);
                        }
                    }
                }
            }
        }

        // THE MIRROR: one bot per living foe, found by its slot marker
        // rather than a stored index, because `bots` is cleared on every
        // map change and an index into it is a bug waiting for one.
        let Some(s) = self.sortie.as_ref() else { return };
        let live: Vec<usize> = s.field.foes.iter().filter(|f| f.alive()).map(|f| f.slot).collect();
        self.bots.retain(|b| b.brain.hostile_slot().map(|sl| live.contains(&sl)).unwrap_or(true));
        for foe in s.field.foes.iter().filter(|f| f.alive()) {
            let yaw = (-foe.facing.x).atan2(-foe.facing.z);
            let idx = match self.bots.iter().position(|b| b.brain.hostile_slot() == Some(foe.slot)) {
                Some(i) => i,
                None => {
                    let mut b = bots::mk_bot_brain(foe.pos(), yaw, bots::TEAM_BRONZE, 100 + foe.slot, bots::Brain::Hostile { slot: foe.slot as u8 });
                    let t = foe.troop.tint;
                    // the troop's own paint, lifted a touch: the sandbox
                    // tints run over 1.0 and a dark hostile at 0.2 reads
                    // as a hole in the ground
                    b.tint = [t[0] as f32 / 255.0 * 1.25 + 0.05, t[1] as f32 / 255.0 * 1.25 + 0.05, t[2] as f32 / 255.0 * 1.25 + 0.05];
                    self.bots.push(b);
                    self.bots.len() - 1
                }
            };
            let b = &mut self.bots[idx];
            b.car.pos = foe.pos();
            b.car.vel = foe.vel;
            b.car.rot = Quat::from_rotation_y(yaw);
            b.car.robot = !foe.is_vehicle();
            if foe.is_vehicle() {
                b.car.wheel_spin += foe.vel.length() * dt * 4.0;
            }
            // a body on a tow line or in the air is on its back: roll it
            // so the drag reads as a drag and not as a robot skating
            if s.field.towed(foe.slot) || !foe.free() && foe.stagger_t > 0.0 {
                b.car.rot = Quat::from_rotation_y(yaw) * Quat::from_rotation_z(2.6);
            }
            b.model = hostile_pose(&self.cars, foe);
        }
    }

    /// Speed multiplier on a vehicle-form shot: half power standing still,
    /// full at `SADDLE_FULL_MS`, half again over it. GUESS shaped so a
    /// strafing run is worth the run.
    fn saddle_mul(&self) -> f32 {
        const SADDLE_FULL_MS: f32 = 24.0;
        (0.5 + self.car.vel.length() / SADDLE_FULL_MS).clamp(0.5, 1.5)
    }

    /// The trigger in vehicle form. Grapple hooks a foe (the tow);
    /// projectile verbs fire down the hood; the rest do nothing.
    fn fire_from_the_saddle(&mut self) {
        let p = self.wep.effective();
        let verb = p.verb;
        let fwd = self.car.forward();
        // THE MUZZLE IS ON THE MODEL. `hardpoints::find` read the vehicle
        // pose at load: a real barrel's tip if the hull has one, else the
        // popout pair on the upper hull. Alternating shots walk the pair.
        // A rear gun aims where it aims — the mount's own direction.
        let size = self.cfg.car.size;
        let mi = self.cars.active.min(self.cars.models.len() - 1);
        let (origin, aim) = match self.cars.models[mi].hardpoints.muzzle(self.saddle_shots as usize) {
            Some(m) => (self.car.pos + self.car.rot * (m.pos * size), (self.car.rot * m.aim).normalize_or(fwd)),
            None => (self.car.pos + fwd * (self.car.half_len * 0.9) + self.car.up() * (self.car.half_h * 0.4), fwd),
        };
        self.saddle_shots = self.saddle_shots.wrapping_add(1);
        let fwd = aim;
        if verb == weapons::Verb::Grapple {
            let engaged = self.sortie.as_ref().map(|s| s.engaged()).unwrap_or(false);
            if !engaged {
                return;
            }
            if self.wep.cool > 0.0 {
                return;
            }
            let hooked = self.sortie.as_mut().and_then(|s| s.field.hook(origin, fwd, cable::CAR_MASS));
            if hooked.is_some() {
                self.wep.cool = p.cooldown;
                self.car.boost = (self.car.boost - p.cost).max(0.0);
            }
            return;
        }
        let deliv = verb.delivery();
        if !matches!(deliv, ballistics::Delivery::Kinetic | ballistics::Delivery::Blast | ballistics::Delivery::Beam) {
            return;
        }
        // the same gate as the robot's trigger — cost, cooldown, fx — with
        // no instant targets: everything from the saddle flies
        let out = self.wep.fire(origin, fwd, &[], self.car.boost);
        if !out.fired {
            return;
        }
        self.car.boost = (self.car.boost - p.cost).max(0.0);
        for e in &out.fx {
            self.fx.ring_burst(e.pos, e.color, false);
        }
        let mul = self.saddle_mul();
        let elem = self.wep.watch.active();
        let side = fwd.cross(Vec3::Y).normalize_or(Vec3::X);
        let upv = side.cross(fwd).normalize_or(Vec3::Y);
        for k in 0..p.pellets.max(1) {
            let h = (k as u64).wrapping_mul(0x9E3779B97F4A7C15);
            let jx = ((h >> 16) & 0xFF) as f32 / 127.5 - 1.0;
            let jy = ((h >> 32) & 0xFF) as f32 / 127.5 - 1.0;
            // a hood gun is steadier than a hand: half the form's spread
            let dir = (fwd + (side * jx + upv * jy) * (p.spread * 0.5)).normalize_or(fwd);
            let deliv = if deliv == ballistics::Delivery::Beam { ballistics::Delivery::Kinetic } else { deliv };
            self.shots.spawn(origin, dir, deliv, p.speed.max(60.0) + self.car.vel.length(), p.impulse * mul, elem, &self.ambient, None, None);
        }
    }

    /// The trigger let go: a tow line in hand is released — at speed
    /// that is the whip.
    fn release_trigger(&mut self) {
        if let Some(s) = self.sortie.as_mut() {
            s.field.release();
        }
    }

    /// Pull the trigger.
    ///
    /// Robot mode only: a car has no arms. Boost is the ammunition, which is
    /// the same tank the thrusters and the second jump draw from — shooting
    /// costs you mobility, which is a decision rather than a resource.
    fn fire_weapon(&mut self) {
        if self.mode != Mode::Play {
            return;
        }
        // VEHICLE FORM HAS A HOOD GUN AND A HOOK. "A car has no arms" was
        // the rule, and it left the whole vehicle side of the game with
        // nothing to do in a fight. Now: the projectile verbs fire FIXED
        // FORWARD, no free aim, impulse scaled by speed — the faster you
        // are going the harder you hit, which is Dark of the Moon's
        // physics and the reason to be in a car at all — and the GRAPPLE
        // is the tow line (`field::Field::hook`): hook a foe, drag it,
        // turn hard, let go. Melee, thrust and blink stay robot-only: no
        // arms, no legs, no gaps in the road.
        if !self.car.robot {
            return self.fire_from_the_saddle();
        }
        // THE ROBOT'S MUZZLE IS ON THE MODEL TOO: the robot pose is its own
        // `Model` with its own `hardpoints`, so an arm cannon found there
        // is where the shot leaves. The chest is the fallback.
        let origin = {
            let mi = self.cars.active.min(self.cars.models.len() - 1);
            let hp = &self.cars.models[mi].hardpoints;
            if hp.has_real_guns() {
                let m = hp.muzzle(self.saddle_shots as usize).unwrap();
                self.saddle_shots = self.saddle_shots.wrapping_add(1);
                self.car.pos + self.car.rot * (m.pos * self.cfg.car.size)
            } else {
                self.car.pos + self.car.up() * (self.car.half_h * 0.8)
            }
        };
        // aim down the camera, not down the chassis: with free look the two
        // differ, and you shoot where you are looking
        let aim = (self.cam.look - self.cam.pos).normalize_or(self.car.forward());
        let targets: Vec<weapons::Target> = self
            .bots
            .iter()
            .enumerate()
            .map(|(i, b)| weapons::Target {
                id: i as u32,
                pos: b.car.pos,
                r: b.car.half_len.max(b.car.half_h),
            })
            .collect();
        let p = self.wep.effective();
        let cost = p.cost;
        // THE PROJECTILE SPLIT. Beams (and the grapple) keep the instant
        // volley; thrown mass and launched charges FLY through `shots.rs`;
        // a blade swings (`melee_arc`). `fire` still gates cost, cooldown
        // and fx for every verb — targets are withheld from the flying
        // verbs so the instant path cannot double-apply what the pool will
        // land a few frames later.
        let deliv = p.verb.delivery();
        let instant =
            matches!(deliv, ballistics::Delivery::Beam) || p.verb == weapons::Verb::Grapple;
        let volley: &[weapons::Target] = if instant { &targets } else { &[] };
        let out = self.wep.fire(origin, aim, volley, self.car.boost);
        if !out.fired {
            return;
        }
        match deliv {
            ballistics::Delivery::Kinetic | ballistics::Delivery::Blast => {
                // a live LOCK at trigger time flies a charge as a missile:
                // nearest mark inside the muzzle cone, pro-nav from there
                // (`guidance`'s whole argument against tail-chasing)
                let seek = if matches!(deliv, ballistics::Delivery::Blast) {
                    self.bots
                        .iter()
                        .enumerate()
                        .filter(|(_, b)| {
                            let to = b.car.pos - origin;
                            to.length() < 250.0 && to.normalize_or(aim).dot(aim) > 0.995
                        })
                        .min_by(|(_, a), (_, b)| {
                            let da = (a.car.pos - origin).length_squared();
                            let db = (b.car.pos - origin).length_squared();
                            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .map(|(i, _)| i)
                } else {
                    None
                };
                let elem = self.wep.watch.active();
                let side = aim.cross(Vec3::Y).normalize_or(Vec3::X);
                let upv = side.cross(aim).normalize_or(Vec3::Y);
                for k in 0..p.pellets.max(1) {
                    // deterministic scatter: the same trigger pull throws
                    // the same pattern
                    let h = (k as u64).wrapping_mul(0x9E3779B97F4A7C15);
                    let jx = ((h >> 16) & 0xFF) as f32 / 127.5 - 1.0;
                    let jy = ((h >> 32) & 0xFF) as f32 / 127.5 - 1.0;
                    let dir = (aim + (side * jx + upv * jy) * p.spread).normalize_or(aim);
                    self.shots.spawn(
                        origin,
                        dir,
                        deliv,
                        p.speed.max(40.0),
                        p.impulse,
                        elem,
                        &self.ambient,
                        seek,
                        // The player is not in the bot mark list, so there is
                        // no index to exclude.
                        None,
                    );
                }
            }
            ballistics::Delivery::Melee => {
                if p.verb != weapons::Verb::Grapple && self.swing <= 0.0 {
                    self.swing = SWING_S;
                }
            }
            ballistics::Delivery::Beam => {}
        }
        self.car.boost = (self.car.boost - cost).max(0.0);
        self.car.vel += out.self_impulse;
        if let Some(p) = out.teleport {
            self.car.pos = p;
        }
        for e in &out.fx {
            self.fx.ring_burst(e.pos, e.color, e.scale > 1.5);
        }

        // THE SORTIE BRIDGE IS GONE. It used to land every trigger pull on
        // "the hostile leading the line" whatever you were aiming at. The
        // wave now stands in the world (`field.rs`) as bots with
        // `Brain::Hostile`, so a shot reaches the engagement only by
        // HITTING one — through `hits_on` below for the instant verbs and
        // through the shot pool's impacts for the flying ones, both via
        // `hostile_hit`. A miss is a miss.
        // the raid's kill bridge now runs the same element advantage
        // table the sortie and the live bots use (`RaidFlow::strike`)
        if self.raid.as_ref().map(|r| r.assaulting()).unwrap_or(false) {
            let p = self.wep.effective();
            let elem = self.wep.watch.active();
            let at = self.car.pos;
            if let Some(r) = self.raid.as_mut() {
                r.strike(p.impulse * IMPULSE_TO_DAMAGE, elem, at);
            }
        }

        // SECTION DAMAGE. `hits_on` carries (target id, delta-v), which is
        // where and how hard — everything `sections::Body::hit` needs once
        // the world-space impact is expressed in the target's own body
        // space. The verb decides the KIND, because a sabre and a rifle
        // are not the same event however similar their damage numbers.
        let kind = sections::kind_for(self.wep.profile().verb);
        let radius = if kind == sections::Kind::Blast { self.wep.effective().reach.min(3.0) } else { 0.0 };
        let shooter_elem = self.wep.watch.active();
        for (id, dv) in &out.hits_on {
            let amount = dv.length() * IMPULSE_TO_DAMAGE;
            if amount <= 0.0 {
                continue;
            }
            self.hostile_hit(*id as usize, amount, shooter_elem, self.wep.profile().register());
            let Some(bot) = self.bots.get_mut(*id as usize) else { continue };
            // world -> body space: the bot's own rotation, inverted
            let local = bot.car.rot.inverse() * (origin - bot.car.pos);
            let dir_local = (bot.car.rot.inverse() * dv.normalize_or(Vec3::Z)).normalize_or(Vec3::Z);
            let res = bot.body.hit(kind, local, dir_local, amount, radius);
            // GRIME: scorch the section that actually took the hit,
            // independent of whether it did lasting hp damage --
            // armour that shrugged off a shot still shows where it hit.
            if let Some(s) = res.struck {
                bot.grime.add_scorch(s, (amount * 0.15).min(0.4));
            }
            for s in &res.severed {
                self.toastf(&format!("{} torn off", s.name()));
            }
            if res.killed {
                self.toastf("bot down");
            }
        }
    }

    /// Drive the gait, and put its body motion somewhere the renderer
    /// already looks.
    ///
    /// `car.susp` is suspension compression, which the renderer bobs the
    /// body by. A robot has no suspension, so in robot mode that channel is
    /// free — and feeding footfall squash into it makes a walking bot dip
    /// with each step using drawing code that already exists. It is not limb
    /// articulation, but a body that dips on the beat and leans into a
    /// sprint reads as walking long before its legs do.
    fn update_locomotion(&mut self, dt: f32) {
        if !self.car.robot {
            self.loco_pose = anim::Pose::default();
            return;
        }
        let up = -self.world.gravity_dir(self.car.pos);
        let vert = self.car.vel.dot(up);
        let horiz = (self.car.vel - up * vert).length();
        let boosting = self.car.flight || self.action_down(Action::Boost);
        // HOW BADLY IS IT DOING, AND IS THERE ANYTHING TO LEAN ON.
        // `limp()` is SIGNED: its magnitude is the damage across the two
        // legs (0..1) and its sign says which leg (positive right, negative
        // left). The gait machine wants only the magnitude -- it crouches and
        // hugs walls the same whichever leg is hurt -- and `condition` clamps
        // to 0..1, so the signed value went in and a hurt LEFT leg clamped to
        // zero: a body missing its left leg walked upright (C20_TODO). The
        // rig below takes the signed value, since it picks the leg from it.
        // The wall is probed with the SDF the physics already trusts: sample
        // left and right at shoulder height and take the nearer, so a body
        // only hugs a surface that is really there.
        let limp = self.body.limp();
        let hurt = gait_hurt(limp);
        let wall = if hurt > anim::CROUCH_HURT {
            let side = self.car.rot * Vec3::X;
            let at = self.car.pos + up * (self.car.half_h * 0.5);
            let reach = self.car.half_w * 3.0 + 1.0;
            let (l, r) = (
                self.world.sdf(at - side * reach),
                self.world.sdf(at + side * reach),
            );
            if l.min(r) > reach * 0.9 {
                0.0 // open ground both sides
            } else if l < r {
                -1.0
            } else {
                1.0
            }
        } else {
            0.0
        };
        self.loco.condition(hurt, wall);
        self.loco_pose = self
            .loco
            .update(dt, horiz, vert, self.car.on_ground, boosting, self.car.climbing > 0.5);
        // THE RIG, driven by the same truth: the gait the machine chose, the
        // ground speed, the gravity where the body is, and the damage WITH its
        // sign (the rig reads the side from it; the gait machine above took the
        // magnitude).
        let (_gd, gmul) = self.world.gravity_field(self.car.pos);
        self.rig_frame = Some(self.rig.update(dt, self.loco.gait, horiz, gmul * self.cfg.arena.gravity, limp));
        // element movement also belongs here: Air runs, Rock does not
        let _ = self.wep.move_mul();

        // DAMAGE SHOWS IN THE WALK. `sections.rs` owns where the body is
        // hurt; the gait owns what a stride looks like; this is the only
        // place the two meet. A limp is an asymmetry on channels the pose
        // already has, not a new animation.
        // THE SWING drives the arm channel the pose already carries —
        // unconsumed by the renderer until bodies are rigs, but the data
        // layer is the truth and the arc is real either way. Mid-swing is
        // when the blade lands (`melee_arc`).
        if self.swing > 0.0 {
            let t = 1.0 - (self.swing / SWING_S);
            self.loco_pose.arm_r = (t * std::f32::consts::PI).sin() * 2.2;
            let before = self.swing;
            self.swing -= dt;
            if before > SWING_S * 0.5 && self.swing <= SWING_S * 0.5 {
                self.melee_arc();
            }
        }
        let limp = self.body.limp();
        if limp.abs() > 0.02 {
            // the hurt leg swings less and the body rolls away from it —
            // sign convention: limp<0 means the LEFT side is the hurt one
            let (hurt_l, hurt_r) = if limp < 0.0 { (1.0 + limp, 1.0) } else { (1.0, 1.0 - limp) };
            self.loco_pose.leg_l *= hurt_l;
            self.loco_pose.leg_r *= hurt_r;
            self.loco_pose.roll += limp * 0.12;
        }
        self.car.susp = (self.loco_pose.squash * self.loco_pose.weight).clamp(0.0, 1.0);
    }

    /// What this chassis is, for the binding resolver.
    fn chassis_class(&self) -> chord::Class {
        let n = self.cars.names[self.cars.active].to_ascii_lowercase();
        if self.car.flight || n.starts_with("jet") {
            chord::Class::Flyer
        } else if self.car.hover.is_some() || n.contains("hover") {
            chord::Class::Hoverer
        } else if n.starts_with("moto") || n.contains("motorcycle") || n.contains("bike") {
            chord::Class::Moto
        } else if n.contains("tank") || n.contains("patton") || n.contains("lynx") {
            chord::Class::Tank
        } else {
            chord::Class::Racer
        }
    }

    /// Act on what a space press turned out to mean.
    fn space_intent(&mut self, press: chord::Press) {
        if self.mode != Mode::Play {
            return;
        }
        let form = if self.car.robot { chord::Form::Robot } else { chord::Form::Vehicle };
        match chord::resolve(form, self.chassis_class(), press) {
            chord::Intent::Jump => self.one_shot(Action::Jump),
            chord::Intent::CycleTarget => {
                self.autotarget = true;
                self.wep.locked = None; // step off the current one
                self.toastf("NEXT TARGET");
            }
            chord::Intent::HoldTarget => {
                self.autotarget = true;
            }
            chord::Intent::Weapons => {
                self.autotarget = true;
                self.toastf(&format!("WEAPONS: {}", self.wep.profile().name));
            }
            chord::Intent::Flare => self.toastf("FLARES"),
            chord::Intent::None => {}
        }
    }

    /// Let go of the target, if the lock was being held.
    ///
    /// Checks the binding rather than a hard-coded key, so rebinding
    /// autotarget in the Binds menu moves the release with it — the failure
    /// mode otherwise is a lock you can turn on and never turn off.
    fn autotarget_release(&mut self) {
        if !self.cfg.car.autotarget_hold || !self.autotarget {
            return;
        }
        if self.action_down(Action::AutoTarget) {
            return; // another bound key is still down
        }
        self.autotarget = false;
        self.wep.locked = None;
        self.cam.poi = None;
        self.cam.combat = false;
    }

    /// What gun positions this chassis offers.
    ///
    /// Derived from the same categories tools/find-mounts.py profiles: a
    /// flyer with rearward thrust has something to sight past, and anything
    /// with under-wing hardpoints can shoot through its belly. A hull with
    /// neither gets no gunner mode rather than an empty seat.
    fn gun_mounts(&self) -> camera::GunMounts {
        let name = self.cars.names[self.cars.active].to_ascii_lowercase();
        let flyer = self.car.flight || name.starts_with("jet");
        camera::GunMounts {
            rear: flyer,
            ventral: flyer || name.contains("freighter") || name.contains("hercules"),
        }
    }

    /// Point the camera at an opponent instead of the ball.
    ///
    /// Ball cam was always a point-of-interest rig; it simply had only ever
    /// been given one point of interest. Handing it a target turns the same
    /// camera into a combat camera with no new code path, which is why this
    /// is short.
    ///
    /// Range scales with speed: at 40 m/s an opponent 200 m out is four
    /// seconds away and worth watching, while the same target while parked
    /// is scenery.
    fn update_autotarget(&mut self, _dt: f32) {
        if !self.autotarget || self.mode != Mode::Play {
            return;
        }
        // bots are the opponents; the ball is not a target here
        let mut targets: Vec<weapons::Target> = Vec::new();
        for (i, b) in self.bots.iter().enumerate() {
            targets.push(weapons::Target {
                id: i as u32,
                pos: b.car.pos,
                r: b.car.half_len.max(b.car.half_h),
            });
        }
        if targets.is_empty() {
            self.cam.poi = None;
            self.cam.combat = false;
            return;
        }
        let sp = self.car.vel.length();
        let range = 120.0 + sp * 4.0;
        // parked, your nose is a better guess at intent than your velocity
        let heading = if sp > 4.0 { self.car.vel } else { self.car.forward() };
        self.wep.auto_acquire(self.car.pos, heading, &targets, range);
        match self.wep.lock_pos(&targets) {
            Some(p) => {
                self.cam.poi = Some(p);
                self.cam.combat = true;
            }
            None => {
                self.cam.poi = None;
                self.cam.combat = false;
            }
        }
    }

    /// WHICH MODEL TO DRAW while walking.
    ///
    /// anim::Locomotion already runs every frame and produces a phase; until
    /// now the engine read two fields off its Pose and threw the four limb
    /// angles away. The strip is those angles, baked. So the phase picks a
    /// frame and the walk finally reaches the screen.
    ///
    /// Returns None when the chassis has no walk strip or the bot is not on
    /// its feet, in which case the caller draws the fold pose as before.
    fn walk_frame_at(&self, model: usize, offset: f32) -> Option<usize> {
        let (cid, k) = *self.cars.xform.get(&model)?;
        let ch = &self.cars.chassis[cid];
        // only at the robot end: a walking car is not a thing
        if ch.walk.is_empty() || k + 1 != ch.strip.len() {
            return None;
        }
        // EACH BODY ON ITS OWN PHASE. A single locomotion phase drives the
        // player correctly and would put every bot in the field on the same
        // footfall — the lockstep that made three idling minions read as one
        // object. The offset comes from the caller so it can be stable per
        // bot rather than random per frame.
        let ph = (self.loco_pose.phase + offset).rem_euclid(1.0);
        let n = ch.walk.len();
        Some(ch.walk[((ph * n as f32) as usize).min(n - 1)])
    }

    /// The player's own frame: no offset, because the phase IS theirs —
    /// and blended when the walk morph is up, so the stride sweeps
    /// instead of snapping between four baked poses. Field bots keep the
    /// flipbook (`walk_frame_at`): one mesh buffer cannot hold every
    /// bot's phase at once.
    fn walk_frame(&self, model: usize) -> Option<usize> {
        let stepped = self.walk_frame_at(model, 0.0)?;
        if let (Some((mc, bm, _)), Some(&(cid, _))) =
            (&self.walk_morph, self.cars.xform.get(&model))
        {
            if *mc == cid {
                return Some(*bm);
            }
        }
        Some(stepped)
    }

    /// Size multiplier for a model, 1.0 unless it is a boss.
    fn chassis_scale(&self, model: usize) -> f32 {
        match self.cars.xform.get(&model) {
            Some(&(cid, _)) => self.cars.chassis[cid].scale,
            None => 1.0,
        }
    }

    /// Is this model a raid boss?
    fn is_boss(&self, model: usize) -> bool {
        match self.cars.xform.get(&model) {
            Some(&(cid, _)) => self.cars.chassis[cid].boss,
            None => false,
        }
    }

    /// The joints of whatever the catalogue is previewing.
    fn joints_of(&self, model: usize) -> &[joints::Joint] {
        match self.cars.xform.get(&model) {
            Some(&(cid, _)) => &self.cars.chassis[cid].joints,
            None => &[],
        }
    }

    /// Move the joint cursor and say where it landed.
    ///
    /// The toast IS the readout — it names the joint by the same index
    /// foldview prints, so a number said here is the number in joints.json
    /// and in the tooling. That is the whole point of numbering them.
    fn step_joint(&mut self, model: usize, d: i32) {
        let js = self.joints_of(model);
        if js.is_empty() {
            self.toastf("no joint data for this chassis");
            return;
        }
        let bad = js.iter().filter(|j| j.misparented).count();
        if self.joint_only_bad && bad == 0 {
            self.toastf(&format!("{} joints, none misparented", js.len()));
            return;
        }
        let next = joints::step(js, self.joint_sel.min(js.len() - 1), d, self.joint_only_bad);
        // summary BEFORE releasing the borrow, then assign — js borrows self
        let line = js[next].summary();
        let total = js.len();
        self.joint_sel = next;
        let scope = if self.joint_only_bad {
            format!("{bad} bad")
        } else {
            format!("{total} joints")
        };
        self.toastf(&format!("[{scope}] {line}"));
    }

    fn open_menu(&mut self, m: Mode) {
        if let Mode::Online { .. } = m {
            self.online_menu = Some(online_screen::build(
                &self.save.name,
                &self.friend,
                self.net.role != net::Role::Offline,
            ));
        }
        self.menu_from_pause = matches!(self.mode, Mode::Pause { .. });
        self.mode = m;
        self.mscroll = 0.0;
        self.mfling = 0.0;
    }
    fn close_menu(&mut self) {
        self.mode = if self.menu_from_pause { Mode::Pause { sel: 0 } } else { Mode::Splash };
    }

    fn mouse_down(&mut self, b: MouseButton, win: &winit::window::Window) {
        if let Mode::Binds { sel, capturing: true } = self.mode {
            // a fresh click-to-capture shouldn't instantly bind that click
            if self.capture_guard > 0.0 {
                return;
            }
            if sel < ACTIONS.len() {
                self.binds.capture(ACTIONS[sel], Bind::Mouse(b));
            }
            self.mode = Mode::Binds { sel, capturing: false };
            return;
        }
        let esc = Key::Named(NamedKey::Escape);
        let enter = Key::Named(NamedKey::Enter);
        match self.mode {
            Mode::Catalog { .. } => {
                if let Some(i) = self.hover_row() {
                    if let Mode::Catalog { sel } = self.mode {
                        if sel == i {
                            self.switch_car(i);
                            self.save.car = i;
                        }
                    }
                    self.mode = Mode::Catalog { sel: i };
                }
            }
            Mode::Garage { .. } => {
                if let Some(i) = self.hover_row() {
                    self.mode = Mode::Garage { sel: i };
                    self.look_sel = i;
                    // left half steps back, right half steps forward: a list
                    // whose rows cycle needs a direction, and a click has one
                    let fwd = self.cursor.0 >= 0.0;
                    let n = custom::slots()[i].1;
                    if i == 0 {
                        let c = self.look.matches_collection().unwrap_or(0);
                        let m = custom::COLLECTIONS.len();
                        self.look.set_collection(if fwd { (c + 1) % m } else { (c + m - 1) % m });
                    } else {
                        let cur = match i {
                            1 => &mut self.look.skin,
                            2 => &mut self.look.paint,
                            3 => &mut self.look.rig,
                            4 => &mut self.look.wheels,
                            5 => &mut self.look.trail,
                            _ => &mut self.look.explosion,
                        };
                        *cur = if fwd { (*cur + 1) % n } else { (*cur + n - 1) % n };
                    }
                    self.save.look = self.look.encode();
                }
            }
            Mode::Academy { sel } => {
                if let Some(i) = self.hover_row() {
                    if i == sel {
                        self.start_lesson(i);
                    } else {
                        self.mode = Mode::Academy { sel: i };
                    }
                }
            }
            Mode::Trials { sel } => {
                if let Some(i) = self.hover_row() {
                    if i == sel {
                        self.start_trial(i);
                    } else {
                        self.mode = Mode::Trials { sel: i };
                    }
                }
            }
            Mode::Play => {
                if let Some(a) = self.action_for_mouse(b) {
                    self.one_shot(a);
                }
            }
            Mode::Splash => {
                if b == MouseButton::Left {
                    self.mode = Mode::Play;
                }
            }
            Mode::Pause { .. } => {
                if b == MouseButton::Right {
                    self.mode = Mode::Play;
                }
            }
            Mode::Tweaks { menu, sel } => match b {
                MouseButton::Left => self.tweaks_click(menu, sel, win),
                MouseButton::Right => self.tweaks_key(&esc, menu, sel, win),
                _ => {}
            },
            Mode::Binds { sel, .. } => match b {
                MouseButton::Left => {
                    if self.hover_row().is_some() {
                        self.binds_key(enter, sel, false);
                        if matches!(self.mode, Mode::Binds { capturing: true, .. }) {
                            self.capture_guard = 0.35;
                        }
                    }
                }
                MouseButton::Right => self.binds_key(esc, sel, false),
                _ => {}
            },
            Mode::Maps { sel } => match b {
                MouseButton::Left => {
                    if self.hover_row().is_some() {
                        self.maps_key(&enter, "", sel);
                    }
                }
                MouseButton::Right => self.maps_key(&esc, "", sel),
                _ => {}
            },
            Mode::Online { sel } => match b {
                MouseButton::Left => {
                    if let Some(i) = self.hover_row() {
                        if let Some(mut m) = self.online_menu.take() {
                            m.point_at(i);
                            self.mode = Mode::Online { sel: m.sel() };
                            // Clicking an Action row activates it; clicking a
                            // Field row just focuses it for typing — the Menu
                            // decides (activate is a no-op on fields).
                            if let Some(ev) = m.activate() {
                                self.apply_online_event(ev, &mut m);
                            }
                            self.online_menu = Some(m);
                        }
                    }
                }
                MouseButton::Right => self.online_key(&esc, "", sel),
                _ => {}
            },
        }
    }

    /// The menu row currently under the mouse, using the same layout metrics
    /// hud.rs draws with.
    // ------------------------------------------------ touch menu gestures --
    /// Finger down in a menu: select what it landed on, and grab a slider
    /// immediately so scrubbing feels instant.
    fn touch_menu_down(&mut self) {
        self.tdown = Some(self.cursor);
        self.tmoved = 0.0;
        self.tscrub = false;
        self.mfling = 0.0;
        self.mscroll_at_down = self.mscroll;
        if hud::hit_nav(self.cursor.0, self.cursor.1).is_some() {
            self.tdrag_row = None; // the nav strip is buttons, not rows
            return;
        }
        self.tdrag_row = self.hover_row();
        if let (Mode::Tweaks { menu, .. }, Some(i)) = (self.mode, self.tdrag_row) {
            let rows = tweak_rows(menu);
            if let Some((_, Row::Slider { .. })) = rows.get(i) {
                self.mode = Mode::Tweaks { menu, sel: i };
                if hud::near_bar(self.cursor.0) {
                    self.tscrub = true;
                    self.touch_scrub_to(self.cursor.0);
                }
            } else {
                self.mode = Mode::Tweaks { menu, sel: i };
            }
        } else if let Some(i) = self.tdrag_row {
            // highlight under the finger, activate on release
            self.select_row(i);
        }
    }

    /// Finger moved: scrub a grabbed slider, or slide the highlight along
    /// the list. Past a small threshold this stops being a tap.
    fn touch_menu_move(&mut self) {
        let Some(d0) = self.tdown else { return };
        self.tmoved = self.tmoved.max((self.cursor.0 - d0.0).abs() + (self.cursor.1 - d0.1).abs());
        if self.tscrub {
            self.touch_scrub_to(self.cursor.0);
            return;
        }
        // DRAG TO SCROLL: past the threshold the CONTENT follows the finger
        // (one row per row-height of travel), which is what a phone expects.
        // The selection is left alone — that is what a tap is for.
        if self.tmoved > 0.02 {
            let dy = d0.1 - self.cursor.1;
            let rh = self.menu_row_h();
            let want = self.mscroll_at_down + dy / rh;
            let max = self.menu_max_scroll();
            self.mfling = (want - self.mscroll) / 0.016_f32.max(1e-3);
            self.mscroll = want.clamp(0.0, max);
        }
    }

    /// Row height of whatever menu is open (touch maths shares this).
    /// Delay before a held key starts repeating, then the interval between.
    /// 0.38 is long enough that a deliberate single press never doubles.
    const REPEAT_DELAY: f32 = 0.38;
    const REPEAT_RATE: f32 = 0.055;

    /// Fire held navigation keys again on a timer. Menus only: a held key in
    /// play is already handled as an axis, and repeating it would fight that.
    fn tick_key_repeat(&mut self, dt: f32, elwt: &winit::event_loop::EventLoopWindowTarget<()>, win: &winit::window::Window) {
        if self.mode == Mode::Play {
            self.repeat_key = None;
            return;
        }
        let Some(k) = self.repeat_key.clone() else { return };
        if !self.held.contains(&k) {
            self.repeat_key = None;
            return;
        }
        self.repeat_t -= dt;
        if self.repeat_t <= 0.0 {
            self.repeat_t = Self::REPEAT_RATE;
            self.key_down(k, elwt, win);
        }
    }

    /// Keep the selected row inside the viewport after a keyboard move.
    fn follow_selection(&mut self) {
        let (sel, n) = match self.mode {
            Mode::Tweaks { menu, sel } => (sel, tweak_rows(menu).len()),
            Mode::Binds { sel, .. } => (sel, hud::BIND_ROWS),
            Mode::Maps { sel } => (sel, world::STYLE_COUNT),
            // THE CATALOGUE WAS MISSING, which is why the model list never
            // scrolled to what you had selected: arrowing past the bottom
            // moved the highlight off screen and left the view where it was.
            // Its rows are the DISPLAY list — family headers included — so
            // the index that scrolls is the row's position in that, not the
            // model id.
            Mode::Catalog { sel } => {
                let rows = self.catalog_rows();
                let at = rows
                    .iter()
                    .position(|(is_head, i)| !is_head && *i == sel)
                    .unwrap_or(0);
                (at, rows.len())
            }
            _ => return,
        };
        let rh = self.menu_row_h();
        let tall = self.gpu.aspect() < 1.25;
        let top = match self.mode {
            Mode::Maps { .. } => hud::MAPS_TOP,
            Mode::Binds { .. } => hud::BIND_TOP,
            Mode::Catalog { .. } => {
                if tall {
                    hud::CAT_TOP_TALL
                } else {
                    hud::CAT_TOP
                }
            }
            _ => hud::TW_TOP,
        };
        let vis = hud::vis_rows(rh, top, hud::TW_BOTTOM);
        let start = self.mscroll.max(0.0) as usize;
        if sel < start {
            self.mscroll = sel as f32;
        } else if sel + 1 > start + vis {
            self.mscroll = (sel + 1 - vis) as f32;
        }
        self.mscroll = self.mscroll.clamp(0.0, (n.saturating_sub(vis)) as f32);
    }

    fn menu_row_h(&self) -> f32 {
        match self.mode {
            Mode::Maps { .. } => hud::maps_row_h(),
            Mode::Binds { .. } => hud::BIND_ROW_H,
            Mode::Catalog { .. } => {
                if self.gpu.aspect() < 1.25 {
                    hud::CAT_ROW_H_TALL
                } else {
                    hud::CAT_ROW_H
                }
            }
            _ => hud::TW_ROW_H,
        }
    }

    /// How far this menu can scroll, in rows.
    fn menu_max_scroll(&self) -> f32 {
        let rh = self.menu_row_h();
        let tall = self.gpu.aspect() < 1.25;
        let top = match self.mode {
            Mode::Maps { .. } => hud::MAPS_TOP,
            Mode::Binds { .. } => hud::BIND_TOP,
            Mode::Catalog { .. } => {
                if tall {
                    hud::CAT_TOP_TALL
                } else {
                    hud::CAT_TOP
                }
            }
            _ => hud::TW_TOP,
        };
        let vis = hud::vis_rows(rh, top, hud::TW_BOTTOM);
        let n = match self.mode {
            Mode::Maps { .. } => world::STYLE_COUNT,
            Mode::Binds { .. } => hud::BIND_ROWS,
            Mode::Tweaks { menu, .. } => tweak_rows(menu).len(),
            _ => 0,
        };
        (n.saturating_sub(vis)) as f32
    }

    /// Finger up: a tap (little movement, same row) activates. A drag does
    /// not — which is what keeps scrolling from firing buttons.
    fn touch_menu_up(&mut self, win: &winit::window::Window, elwt: &winit::event_loop::EventLoopWindowTarget<()>) {
        let was_tap = self.tmoved < 0.03 && !self.tscrub;
        let start = self.tdown.take();
        self.tdrag_row = None;
        self.tscrub = false;
        let Some(_) = start else { return };
        if !was_tap {
            // a downward flick anywhere = back out of this menu
            if let Some(d0) = start {
                if self.cursor.1 < d0.1 - 0.25 && (self.cursor.0 - d0.0).abs() < 0.25 {
                    self.menu_back(elwt);
                }
            }
            return;
        }
        if let Some(b) = hud::hit_nav(self.cursor.0, self.cursor.1) {
            if !matches!(self.mode, Mode::Splash | Mode::Play | Mode::Pause { .. }) {
                self.nav_to(b, elwt);
                return;
            }
        }
        match self.mode {
            Mode::Splash => self.mode = Mode::Play,
            Mode::Pause { .. } => {
                if let Some(b) = hud::hit_pause_btn(self.cursor.0, self.cursor.1) {
                    self.pause_activate(b, elwt);
                }
            }
            Mode::Tweaks { menu, sel } => {
                if self.hover_row().is_some() {
                    self.tweaks_click(menu, sel, win);
                }
            }
            _ => {
                if self.hover_row().is_some() {
                    self.mouse_down(MouseButton::Left, win);
                }
            }
        }
    }

    /// Set the grabbed slider from an x position (drag scrubbing).
    fn touch_scrub_to(&mut self, x: f32) {
        let Mode::Tweaks { menu, sel } = self.mode else { return };
        let rows = tweak_rows(menu);
        if let Some((_, Row::Slider { set, min, max, step, .. })) = rows.get(sel) {
            let t = hud::bar_t(x);
            let raw = min + (max - min) * t;
            // snap to the row's own step so touch lands on real values
            let v = if *step > 0.0 { (raw / step).round() * step } else { raw };
            set(&mut self.cfg, v.clamp(*min, *max));
            if menu == 2 {
                self.cfg.preset = 0;
            }
        }
    }

    /// Move the highlight to row `i` in whatever list menu is open.
    fn select_row(&mut self, i: usize) {
        self.mode = match self.mode {
            Mode::Tweaks { menu, .. } => Mode::Tweaks { menu, sel: i },
            Mode::Binds { capturing, .. } => Mode::Binds { sel: i, capturing },
            Mode::Maps { .. } => Mode::Maps { sel: i },
            Mode::Online { .. } => Mode::Online { sel: i },
            m => m,
        };
    }

    /// Nav-strip button: jump straight to a screen (mobile needs this —
    /// there is no keyboard shortcut to reach MAPS or VIDEO).
    fn nav_to(&mut self, b: usize, elwt: &winit::event_loop::EventLoopWindowTarget<()>) {
        self.mscroll = 0.0;
        match b {
            0 => self.menu_back(elwt),
            1 => self.mode = Mode::Tweaks { menu: 0, sel: 0 },
            2 => self.mode = Mode::Binds { sel: 0, capturing: false },
            3 => self.mode = Mode::Maps { sel: self.save.map },
            4 => self.mode = Mode::Tweaks { menu: 5, sel: 0 },
            _ => self.mode = Mode::Online { sel: 0 },
        }
    }

    /// Activate pause button `i` — ONE function for mouse taps, touch, and
    /// the keyboard's Enter, so the input paths cannot diverge on what a
    /// button does (the old code held this match twice).
    fn pause_activate(&mut self, i: usize, elwt: &winit::event_loop::EventLoopWindowTarget<()>) {
        match i {
            0 => self.mode = Mode::Play,
            1 => self.open_menu(Mode::Tweaks { menu: 0, sel: 0 }),
            2 => self.open_menu(Mode::Maps { sel: self.save.map }),
            3 => self.open_menu(Mode::Online { sel: 0 }),
            _ => {
                self.net.leave();
                self.persist();
                elwt.exit();
            }
        }
    }

    /// Escape-equivalent for gestures: leave the current menu.
    fn menu_back(&mut self, _elwt: &winit::event_loop::EventLoopWindowTarget<()>) {
        self.mode = match self.mode {
            Mode::Tweaks { .. } | Mode::Binds { .. } | Mode::Maps { .. } | Mode::Online { .. } => {
                if self.menu_from_pause {
                    Mode::Pause { sel: 0 }
                } else {
                    Mode::Splash
                }
            }
            Mode::Pause { .. } => Mode::Play,
            m => m,
        };
    }

    /// Set the board for a lesson: the car here, the ball there and moving,
    /// so the trick is possible on the first attempt rather than needing a
    /// lucky bounce.
    fn arm_lesson(&mut self) {
        let Some(sess) = self.lesson.as_ref() else { return };
        let st = sess.lesson().setup;
        self.car.reset(&self.world);
        if st.car_up > 0.0 {
            self.car.pos += Vec3::Y * st.car_up;
            self.car.vel = Vec3::ZERO;
        }
        let f = self.car.forward();
        self.ball.pos = self.car.pos + f * st.ahead + Vec3::Y * (st.up + self.ball.r);
        self.ball.vel = self.car.rot * st.vel;
        self.ball.ang = Vec3::ZERO;
        self.ball.prev = self.ball.pos;
        self.combo = 0;
        self.air_t = 0.0;
        self.lesson_tricks.clear();
    }

    fn start_lesson(&mut self, i: usize) {
        let l = &academy::LESSONS[i.min(academy::LESSONS.len() - 1)];
        if self.save.map != l.map {
            self.load_map(l.map, self.save.seed);
        }
        self.lesson = Some(academy::Session::new(i));
        self.lesson_sel = i;
        self.arm_lesson();
        self.mode = Mode::Play;
        self.toastf(&format!("{} - {}", l.name, l.hint));
    }

    /// Watch the lesson: progress comes from the same detectors the replays
    /// use, so passing one means the game genuinely saw the trick.
    fn tick_lesson(&mut self, dt: f32) {
        let Some(mut sess) = self.lesson.take() else { return };
        sess.t += dt;
        let l = sess.lesson();
        let hit = matches!(l.skill, academy::Skill::Do(t) if self.lesson_tricks.contains(&t));
        let gy = self.world.ground_h(self.car.pos.x, self.car.pos.z);
        let p = academy::progress(
            l.skill,
            self.ball.vel.length(),
            self.car.pos.y - gy,
            self.combo,
            hit,
        );
        sess.best = sess.best.max(p);
        if p >= 1.0 {
            let first = !self.passed.has(sess.idx);
            self.passed.set(sess.idx);
            self.save.academy = self.passed.encode();
            self.toastf(&if first {
                format!("{} - PASSED", l.name)
            } else {
                format!("{} - again, cleanly", l.name)
            });
            // the moment is its own demonstration
            let (title, score) = self.rec.moment(6.0);
            let frames = self.rec.frames();
            if frames.len() > 20 && score > 0 {
                self.moments.push(replay::Moment { title, score, frames });
                self.moments.sort_by(|a, b| b.score.cmp(&a.score));
                self.moments.truncate(6);
                self.watching = Some((0, 0.0));
            }
            self.lesson = None;
            return;
        }
        // the coach speaks once, when you have clearly tried and missed
        if !sess.nudged && sess.t > 12.0 && sess.best > 0.15 {
            sess.nudged = true;
            self.toastf(l.nudge);
        }
        // re-arm when the ball has settled and nothing is happening
        let ball_rest = self.ball.vel.length() < 1.2 && self.world.sdf(self.ball.pos) < self.ball.r + 0.4;
        if ball_rest && self.car.on_ground {
            sess.idle += dt;
        } else {
            sess.idle = 0.0;
        }
        if sess.idle > 2.5 || sess.t > l.limit {
            sess.idle = 0.0;
            if sess.t > l.limit {
                sess.t = 0.0;
                sess.nudged = false;
            }
            self.lesson = Some(sess);
            self.arm_lesson();
            return;
        }
        self.lesson = Some(sess);
    }

    /// Load a trial's map and start the clock. The ghost of your best run
    /// on this trial (if any) rides along.
    fn start_trial(&mut self, i: usize) {
        let t = &trials::TRIALS[i.min(trials::TRIALS.len() - 1)];
        if self.save.map != t.map {
            self.load_map(t.map, self.save.seed);
        }
        self.car.reset(&self.world);
        self.ball.reset_in_front(&self.car);
        self.scores = [0, 0];
        self.portal_m = 0.0;
        self.air_t = 0.0;
        self.trial = Some(trials::Run::new(i));
        self.trial_sel = i;
        self.mode = Mode::Play;
        self.toastf(&format!("{} - {}", t.name, t.blurb));
    }

    /// One frame of trial progress, fed entirely from live physics state.
    fn tick_trial(&mut self, dt: f32) {
        let Some(mut run) = self.trial.take() else { return };
        let gy = self.world.ground_h(self.car.pos.x, self.car.pos.z);
        let in_tube = self.world.route_locate(self.car.pos).is_some();
        let on_plat = self
            .world
            .platform_vel(self.car.pos - Vec3::Y * (self.car.half_h + 0.1), 1.2)
            .map(|v| v.length() > 0.5)
            .unwrap_or(false);
        let done = run.tick(
            dt,
            self.car.pos,
            self.car.rot,
            self.car.vel.length(),
            gy,
            self.scores[0],
            self.air_t,
            self.combo,
            in_tube,
            on_plat,
            std::mem::take(&mut self.portal_m),
        );
        if done {
            let t = run.t;
            let idx = run.idx;
            let rec = self.bests.record(idx, t);
            self.ghost = std::mem::take(&mut run.ghost);
            self.save.trials = self.bests.encode();
            self.toastf(&if rec {
                format!("{} COMPLETE - {:.1}s  NEW BEST", trials::TRIALS[idx].name, t)
            } else {
                format!("{} COMPLETE - {:.1}s (best {:.1}s)", trials::TRIALS[idx].name, t, self.bests.get(idx))
            });
            self.trial = None;
            return;
        }
        if run.failed {
            self.toastf(&format!("{} - OUT OF TIME", run.trial().name));
            self.trial = None;
            return;
        }
        self.trial = Some(run);
    }

    /// The catalogue's display list: a header wherever the family changes,
    /// then its cars. Built the same way the drawing builds it, so a click
    /// lands on the row it appears to be over.
    fn catalog_rows(&self) -> Vec<(bool, usize)> {
        let mut rows = Vec::new();
        let mut last = "";
        for i in 0..self.cars.names.len() {
            let cls = self.cars.class_of(i);
            if cls != last {
                rows.push((true, i));
                last = cls;
            }
            rows.push((false, i));
        }
        rows
    }

    fn hover_row(&self) -> Option<usize> {
        let y = self.cursor.1;
        match self.mode {
            Mode::Tweaks { menu, .. } => {
                let rows = tweak_rows(menu);
                let n = rows.len();
                let vis = hud::vis_rows(hud::TW_ROW_H, hud::TW_TOP, hud::TW_BOTTOM);
                let start = self.mscroll.max(0.0) as usize;
                hud::hit_row_scrolled(hud::TW_TOP, hud::TW_ROW_H, n, vis, start, y)
                    .filter(|i| !is_header(&rows[*i].1))
            }
            Mode::Binds { capturing: false, .. } => {
                let vis = hud::vis_rows(hud::BIND_ROW_H, hud::BIND_TOP, hud::TW_BOTTOM);
                let start = self.mscroll.max(0.0) as usize;
                hud::hit_row_scrolled(hud::BIND_TOP, hud::BIND_ROW_H, hud::BIND_ROWS, vis, start, y)
            }
            Mode::Maps { .. } => {
                let n = world::STYLE_COUNT;
                let vis = hud::vis_rows(hud::maps_row_h(), hud::MAPS_TOP, hud::TW_BOTTOM);
                let start = self.mscroll.max(0.0) as usize;
                hud::hit_row_scrolled(hud::MAPS_TOP, hud::maps_row_h(), n, vis, start, y)
            }
            Mode::Online { .. } => {
                // shared-menu geometry (draw_menu ⇄ menu_hit stay in lockstep)
                let m = self.online_menu.as_ref()?;
                let scr = ui::Screen::new(self.gpu.aspect());
                ui::menu_hit(&scr, m, hud::T_BODY, (self.cursor.0, y))
            }
            // These three had no arm at all, so `hover_row` returned None and
            // every click on them did nothing.
            Mode::Trials { .. } => {
                let n = trials::TRIALS.len();
                let vis = hud::vis_rows(hud::TRIALS_ROW_H, hud::LIST_TOP, hud::TW_BOTTOM);
                let start = self.mscroll.max(0.0) as usize;
                hud::hit_row_scrolled(hud::LIST_TOP, hud::TRIALS_ROW_H, n, vis, start, y)
            }
            Mode::Academy { .. } => {
                let n = academy::LESSONS.len();
                let vis = hud::vis_rows(hud::ACADEMY_ROW_H, hud::LIST_TOP, hud::TW_BOTTOM);
                let start = self.mscroll.max(0.0) as usize;
                hud::hit_row_scrolled(hud::LIST_TOP, hud::ACADEMY_ROW_H, n, vis, start, y)
            }
            Mode::Catalog { .. } => {
                // the catalogue's rows include family headers, so a screen
                // row is not a car index — the caller maps it back
                let a = self.gpu.config.width.max(1) as f32 / self.gpu.config.height.max(1) as f32;
                let wide = a > 1.25;
                let (top, rh) = if wide {
                    (hud::CAT_TOP, hud::CAT_ROW_H)
                } else {
                    (hud::CAT_TOP_TALL, hud::CAT_ROW_H_TALL)
                };
                let rows = self.catalog_rows();
                let vis = ((top - hud::TW_BOTTOM) / rh) as usize;
                let start = (self.mscroll.max(0.0) as usize).min(rows.len().saturating_sub(vis.min(rows.len())));
                hud::hit_row_scrolled(top, rh, rows.len(), vis, start, y)
                    .and_then(|r| rows.get(r).copied())
                    .and_then(|(is_head, ci)| (!is_head).then_some(ci))
            }
            Mode::Garage { .. } => {
                // the panel drops below the stage on a tall window, so the
                // rows are somewhere else and hit-testing has to know
                let a = self.gpu.config.width.max(1) as f32 / self.gpu.config.height.max(1) as f32;
                let top = if a > 1.25 { hud::GARAGE_TOP } else { hud::GARAGE_TOP_TALL };
                hud::hit_row(top, hud::GARAGE_ROW_H, custom::slots().len(), y)
            }
            _ => None,
        }
    }

    /// Left click in a settings menu: bar clicks set sliders directly, presets
    /// cycle, everything else behaves like Enter.
    fn tweaks_click(&mut self, menu: u8, _sel: usize, win: &winit::window::Window) {
        let Some(i) = self.hover_row() else { return };
        self.mode = Mode::Tweaks { menu, sel: i };
        let rows = tweak_rows(menu);
        let (_, row) = rows[i.min(rows.len() - 1)];
        match row {
            Row::Slider { set, min, max, .. } => {
                let x = self.cursor.0;
                if x >= hud::TW_BAR_X0 - 0.01 && x <= hud::TW_BAR_X1 + 0.01 {
                    let t = ((x - hud::TW_BAR_X0) / (hud::TW_BAR_X1 - hud::TW_BAR_X0)).clamp(0.0, 1.0);
                    set(&mut self.cfg, min + (max - min) * t);
                    if menu == 2 {
                        self.cfg.preset = 0;
                    }
                }
            }
            Row::Preset | Row::CarPick => {
                self.tweaks_key(&Key::Named(NamedKey::ArrowRight), menu, i, win);
            }
            Row::Link(_) | Row::Controls | Row::Trials | Row::Academy | Row::Garage | Row::Catalog => {
                self.mscroll = 0.0;
                self.mfling = 0.0;
                self.tweaks_key(&Key::Named(NamedKey::Enter), menu, i, win);
            }
            _ => {
                self.tweaks_key(&Key::Named(NamedKey::Enter), menu, i, win);
            }
        }
    }

    /// Scroll wheel: nudge the selected slider in the settings menus.
    fn wheel(&mut self, dy: f32) {
        if dy == 0.0 {
            return;
        }
        // OVER THE MODEL, THE WHEEL WALKS THE RIG. The catalogue's preview
        // occupies the right of a wide screen, so a wheel there is about the
        // bot; over the list on the left it is still the list. One gesture,
        // two meanings, decided by where the pointer is — which is what you
        // want when the point is to be reading joints while talking.
        if let Mode::Catalog { sel } = self.mode {
            let over_preview = self.cursor.0 > 0.18;
            if over_preview {
                self.step_joint(sel, if dy > 0.0 { -1 } else { 1 });
                return;
            }
        }
        if let Mode::Tweaks { menu, sel } = self.mode {
            let rows = tweak_rows(menu);
            if let Some((_, Row::Slider { get, set, min, max, step, .. })) = rows.get(sel) {
                let v = (get(&self.cfg) + step * dy.signum()).clamp(*min, *max);
                set(&mut self.cfg, v);
                if menu == 2 {
                    self.cfg.preset = 0;
                }
            }
        }
    }

    fn key_down(
        &mut self,
        key: Key,
        elwt: &winit::event_loop::EventLoopWindowTarget<()>,
        win: &winit::window::Window,
    ) {
        if key == Key::Named(NamedKey::F11) {
            self.toggle_fullscreen(win);
            return;
        }
        let ch = match &key {
            Key::Character(s) => s.as_str(),
            _ => "",
        };
        // THE DECK eats input while a console is on stage. Escape folds
        // it shut (the fold finishes before it closes; that is the
        // aesthetic), Tab walks to the next station, arrows and Enter go
        // to the console. Everything else is swallowed: a player at a
        // console is AT the console.
        if self.mode == Mode::Play {
            // computed before the borrow below: the guard on Grapple
            // needs `&self` (bindings live on the whole struct), and `r`
            // holds `self.raid` mutably for the rest of this block, so
            // this can't be evaluated inline inside the match arm.
            let grapple_pressed = self.action_for_key(&key) == Some(Action::Grapple);
            // THE RAID EATS INPUT the way the sortie below does. Between
            // phases, Enter loads the next arena and briefs it — the map
            // swap needs the shell, which is why the stage exists at all.
            if let Some(r) = self.raid.as_mut() {
                match r.stage {
                    raidflow::Stage::Brief => match &key {
                        Key::Named(NamedKey::ArrowUp) => {
                            r.site_cycle(-1);
                            return;
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            r.site_cycle(1);
                            return;
                        }
                        Key::Named(NamedKey::Enter) => {
                            let lz = r.lz_now();
                            let gy = self.world.ground_h(lz.x, lz.z);
                            if r.launch(gy) && self.xf.is_robot() {
                                self.transform();
                            }
                            return;
                        }
                        Key::Named(NamedKey::Escape) => {
                            self.raid = None;
                            self.toastf("raid stood down — nothing recorded");
                            return;
                        }
                        _ => return,
                    },
                    raidflow::Stage::Assault => match &key {
                        Key::Named(NamedKey::Escape) => {
                            self.raid = None;
                            self.toastf("raid abandoned — nothing recorded");
                            return;
                        }
                        // Rebindable — see `Action::Grapple` (defaults to
                        // H; still free of any other binding in Play mode
                        // outside the raid Assault stage).
                        _ if grapple_pressed => {
                            let at = self.car.pos;
                            if let Some(v) = r.grapple(at) {
                                self.car.vel = Vec3::new(v[0], v[1], v[2]);
                                self.toastf("grapple away");
                            } else {
                                self.toastf("no node in reach");
                            }
                            return;
                        }
                        _ => {} // play controls stay live on the field
                    },
                    raidflow::Stage::Between => match &key {
                        Key::Named(NamedKey::Enter) => {
                            let a = r.next_arena();
                            self.arena_map(a);
                            if let Some(r) = self.raid.as_mut() {
                                r.brief_next();
                            }
                            return;
                        }
                        Key::Named(NamedKey::Escape) => {
                            self.raid = None;
                            self.toastf("raid abandoned between phases — nothing recorded");
                            return;
                        }
                        _ => return,
                    },
                    raidflow::Stage::Debrief { .. } => match &key {
                        Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Escape) => {
                            let line = r.commit();
                            self.raid = None;
                            self.toastf(&line);
                            return;
                        }
                        _ => return,
                    },
                    raidflow::Stage::Falling => {}
                }
            }
            // THE SORTIE EATS INPUT the way the deck does: a player at a
            // briefing is AT the briefing. Falling handles nothing here —
            // the fold rides the jump edge inside the fixed loop, and there
            // is no backing out of the air. During the engagement the play
            // controls stay live (you are standing on the field); only
            // Escape is claimed, to abandon.
            if let Some(s) = self.sortie.as_mut() {
                match s.stage {
                    sortie::Stage::Brief => match &key {
                        Key::Named(NamedKey::ArrowUp) => {
                            s.site_cycle(-1);
                            return;
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            s.site_cycle(1);
                            return;
                        }
                        Key::Named(NamedKey::Enter) => {
                            let lz = s.lz_now();
                            let gy = self.world.ground_h(lz.x, lz.z);
                            if s.launch(gy) && self.xf.is_robot() {
                                // you leave the door folded: the drop
                                // machine starts in Vehicle, so the visual
                                // state machine follows it out
                                self.transform();
                            }
                            return;
                        }
                        Key::Named(NamedKey::Escape) => {
                            self.sortie = None;
                            self.toastf("sortie stood down — nothing recorded");
                            return;
                        }
                        _ => return,
                    },
                    sortie::Stage::Engage => {
                        if key == Key::Named(NamedKey::Escape) {
                            self.sortie = None;
                            self.toastf("sortie abandoned — nothing recorded");
                            return;
                        }
                    }
                    sortie::Stage::Debrief { .. } => match &key {
                        Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Escape) => {
                            let line = s.commit();
                            self.sortie = None;
                            self.toastf(&line);
                            return;
                        }
                        _ => return,
                    },
                    sortie::Stage::Falling => {}
                }
            }
            // Computed BEFORE the deck is borrowed, for the same reason the
            // Enter arm resolves its order outside the borrow: acting on the
            // deck needs the shell, and the shell is what is borrowed.
            let reachable = self.consoles_reachable_now();
            if let Some(d) = self.deck.as_mut() {
                match &key {
                    Key::Named(NamedKey::Escape) => {
                        d.input(menu::Input::Back);
                    }
                    Key::Named(NamedKey::Tab) => {
                        // WALK TO THE NEXT REACHABLE CONSOLE, not the next in
                        // the array. Tab used to cycle all seven regardless of
                        // sealed doors, which made `depower` cosmetic: the
                        // whole point of a door that shuts is that it stops
                        // you, and a menu that ignores it is a menu.
                        let all = &reachable;
                        if !all.is_empty() {
                            let at =
                                all.iter().position(|s| *s == d.station).unwrap_or(0);
                            d.walk_to(all[(at + 1) % all.len()]);
                        }
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        d.input(menu::Input::Move(-1));
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        d.input(menu::Input::Move(1));
                    }
                    Key::Named(NamedKey::Enter) => {
                        // resolve the order OUTSIDE the deck's borrow: acting
                        // on it needs the shell, and rebuilding the console
                        // needs to replace the very thing being borrowed
                        if let Some(order) = d.input(menu::Input::Activate) {
                            // DEPART IS THE SHELL'S: it loads a map, which
                            // needs the gpu and a fresh World. Intercepted
                            // before `run_order`, whose `order_target` would
                            // return None and silently do nothing.
                            if let stations::Order::Depart { dest, line } = &order {
                                let (dest, line) = (*dest, line.clone());
                                self.depart_to(dest, &line);
                                return;
                            }
                            // A SORTIE IS FLOWN when its destination can be
                            // dropped on. Intercepted like Depart and for
                            // the same reasons: it loads a map, and the
                            // flow needs the shell. Docks fall through to
                            // `run_order`'s resolved path — you berth at a
                            // station, you do not drop on it.
                            if let Some((mid, week)) = sortie::order_flyable(&order) {
                                self.begin_sortie(mid, week);
                                return;
                            }
                            // the Wardeck's raid order takes the same door
                            if let stations::Order::Raid(boss) = &order {
                                let boss = *boss;
                                self.begin_raid(boss);
                                return;
                            }
                            // the forge is a worker thread, not a stall:
                            // the order lights it and the frame moves on
                            if let stations::Order::Forge(theme) = &order {
                                if self.forge_rx.is_some() {
                                    self.toastf("the forge is already lit — one signal at a time");
                                } else {
                                    // the forge is told who it may cast — `casting::available`
                                    // reads the campaign log, so a forged raid
                                    // cannot pre-empt a reveal or resurrect
                                    // someone the campaign wrote out
                                    let cast: Vec<String> = casting::available(d.campaign())
                                        .into_iter()
                                        .map(|s| s.to_string())
                                        .collect();
                                    self.forge_rx = Some(raidforge::forge_async(theme.clone(), cast));
                                    self.toastf("forging a signal — the wardeck will chime when it resolves");
                                }
                                return;
                            }
                            if let stations::Order::Echo(id) = &order {
                                let id = *id;
                                self.begin_echo(id);
                                return;
                            }
                            let st = d.station;
                            let (line, changed) = stations::run_order(&order);
                            if changed {
                                // the save moved under the console, so the
                                // console has to be rebuilt from it — a Deck
                                // loads its Shipboard once, at open.
                                if let Ok(nd) =
                                    deck::Deck::open(st, ship::Phase::Cruise, nightfall::week_now())
                                {
                                    self.deck = Some(nd);
                                }
                            }
                            self.toastf(&line);
                        }
                    }
                    _ => {}
                }
                return;
            }
            if ch.eq_ignore_ascii_case("v") {
                // WHICHEVER CONSOLE YOU ARE STANDING IN. This opened the
                // Armory unconditionally, because there was no way to ask
                // where the player was — `Station::cell()` named a room and
                // nothing could tell you which room you were in.
                //
                // Now the ship is parked and solid, so the car's position is
                // a room, and a room is a console. Outside the ship there is
                // no console to open, and saying so is better than opening an
                // arbitrary one: a hub you can reach from anywhere is a menu.
                let here = self.console_underfoot();
                match here {
                    Some(st) => {
                        match deck::Deck::open(st, ship::Phase::Cruise, nightfall::week_now()) {
                            Ok(d) => self.deck = Some(d),
                            Err(e) => self.toastf(e),
                        }
                    }
                    None => self.toastf("no console here — find a room aboard"),
                }
                return;
            }
            // THE ACTION BAR. 1-9 select a hotbar slot outright — legal
            // even on an empty one, same rule `hotbar.rs` documents for
            // Minecraft's own bar. Firing whatever lands in the active
            // slot is not wired to these keys yet; selection is.
            if let Key::Character(s) = &key {
                if s.len() == 1 {
                    if let Some(d) = s.chars().next().and_then(|c| c.to_digit(10)) {
                        if (1..=hotbar::SLOTS as u32).contains(&d) {
                            let i = (d - 1) as usize;
                            let _ = self.hotbar.select(i);
                            let label = match self.hotbar.slot(i) {
                                Some(slot) => match &slot.item {
                                    gear::Item::Weapon { verb, element, .. } => {
                                        format!("{} {}", element.name(), gear::verb_name(*verb))
                                    }
                                    gear::Item::Armor(m) => m.name.to_string(),
                                },
                                None => "-- empty --".to_string(),
                            };
                            // the slot actually arms the weapon. Two separate
                            // requests, because form and element are separate
                            // systems: the form is set directly, but the
                            // element goes through Watch::switch so arming a
                            // slot cannot dodge the attune delay a manual
                            // element change costs.
                            if let Some(slot) = self.hotbar.slot(i) {
                                let item = slot.item.clone();
                                if let Some(form) = hotbar::form_for(&item) {
                                    self.wep.form = form;
                                    self.wep.cool = 0.0;
                                }
                                if let Some(elem) = hotbar::element_for(&item) {
                                    self.wep.watch.switch(elem);
                                }
                            }
                            self.toastf(&format!("[{}] {label}", d));
                            return;
                        }
                    }
                }
            }
        }
        // arm the repeat for navigation keys, in menus only. Done here rather
        // than in the winit handler so it follows every path that reaches
        // key_down, including the repeat's own re-entry — which resets the
        // timer to the fast rate rather than the initial delay.
        if self.mode != Mode::Play
            && matches!(
                key,
                Key::Named(
                    NamedKey::ArrowUp
                        | NamedKey::ArrowDown
                        | NamedKey::ArrowLeft
                        | NamedKey::ArrowRight
                )
            )
        {
            if self.repeat_key.as_ref() != Some(&key) {
                self.repeat_key = Some(key.clone());
                self.repeat_t = Self::REPEAT_DELAY;
            }
        }
        match self.mode {
            Mode::Catalog { sel } => {
                // LEFT/RIGHT WALK THE RIG. Up and down already move between
                // cars, so the other axis is free for the thing inside one:
                // step joint by joint, with B toggling whether to stop at
                // every joint or only the broken ones.
                if matches!(key, Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight)) {
                    let d = if key == Key::Named(NamedKey::ArrowRight) { 1 } else { -1 };
                    self.step_joint(sel, d);
                    return;
                }
                if ch.eq_ignore_ascii_case("b") {
                    self.joint_only_bad = !self.joint_only_bad;
                    self.joint_sel = 0;
                    self.step_joint(sel, 0);
                    return;
                }
                let n = self.cars.models.len();
                match (&key, ch) {
                    (Key::Named(NamedKey::Escape), _) => self.close_menu(),
                    (Key::Named(NamedKey::ArrowUp), _) | (_, "w") => {
                        self.mode = Mode::Catalog { sel: (sel + n - 1) % n };
                        self.follow_selection();
                    }
                    (Key::Named(NamedKey::ArrowDown), _) | (_, "s") => {
                        self.mode = Mode::Catalog { sel: (sel + 1) % n };
                        self.follow_selection();
                    }
                    (Key::Named(NamedKey::Enter), _) => {
                        self.switch_car(sel);
                        self.save.car = sel;
                        self.toastf(&format!("{} - {}", self.cars.names[sel], self.cars.blurb_of(sel)));
                    }
                    _ => {}
                }
            }
            Mode::Garage { sel } => {
                let slots = custom::slots();
                let n = slots.len();
                let cnt = slots[sel.min(n - 1)].1;
                // the collection row writes every other slot at once
                if sel == 0 {
                    let n = custom::COLLECTIONS.len();
                    let curc = self.look.matches_collection().unwrap_or(0);
                    match (&key, ch) {
                        (Key::Named(NamedKey::ArrowLeft), _) | (_, "a") => self.look.set_collection((curc + n - 1) % n),
                        (Key::Named(NamedKey::ArrowRight), _) | (_, "d") => self.look.set_collection((curc + 1) % n),
                        _ => {}
                    }
                }
                let cur = match sel {
                    0 => &mut self.look.skin, // handled above; keeps the borrow simple
                    1 => &mut self.look.skin,
                    2 => &mut self.look.paint,
                    3 => &mut self.look.rig,
                    4 => &mut self.look.wheels,
                    5 => &mut self.look.trail,
                    _ => &mut self.look.explosion,
                };
                match (&key, ch) {
                    (Key::Named(NamedKey::Escape), _) => self.close_menu(),
                    (Key::Named(NamedKey::ArrowLeft), _) | (_, "a") if sel > 0 => *cur = (*cur + cnt - 1) % cnt,
                    (Key::Named(NamedKey::ArrowRight), _) | (_, "d") if sel > 0 => *cur = (*cur + 1) % cnt,
                    (Key::Named(NamedKey::ArrowUp), _) | (_, "w") => {
                        self.mode = Mode::Garage { sel: (sel + n - 1) % n }
                    }
                    (Key::Named(NamedKey::ArrowDown), _) | (_, "s") => {
                        self.mode = Mode::Garage { sel: (sel + 1) % n }
                    }
                    _ => {}
                }
                self.save.look = self.look.encode();
            }
            Mode::Academy { sel } => {
                let n = academy::LESSONS.len();
                match (&key, ch) {
                    (Key::Named(NamedKey::Escape), _) => self.close_menu(),
                    (Key::Named(NamedKey::ArrowUp), _) | (_, "w") => {
                        self.mode = Mode::Academy { sel: (sel + n - 1) % n };
                        self.follow_selection();
                    }
                    (Key::Named(NamedKey::ArrowDown), _) | (_, "s") => {
                        self.mode = Mode::Academy { sel: (sel + 1) % n };
                        self.follow_selection();
                    }
                    (Key::Named(NamedKey::Enter), _) => self.start_lesson(sel),
                    _ => {}
                }
            }
            Mode::Trials { sel } => {
                let n = trials::TRIALS.len();
                match (&key, ch) {
                    (Key::Named(NamedKey::Escape), _) => self.close_menu(),
                    (Key::Named(NamedKey::ArrowUp), _) | (_, "w") => {
                        self.mode = Mode::Trials { sel: (sel + n - 1) % n };
                        self.follow_selection();
                    }
                    (Key::Named(NamedKey::ArrowDown), _) | (_, "s") => {
                        self.mode = Mode::Trials { sel: (sel + 1) % n };
                        self.follow_selection();
                    }
                    (Key::Named(NamedKey::Enter), _) => self.start_trial(sel),
                    _ => {}
                }
            }
            Mode::Splash => match (&key, ch) {
                (Key::Named(NamedKey::Escape), _) => {
                    self.net.leave();
                    self.persist();
                    elwt.exit();
                }
                (_, "g") => self.open_menu(Mode::Maps { sel: self.save.map }),
                (_, "o") => self.open_menu(Mode::Online { sel: 0 }),
                _ => self.mode = Mode::Play, // any other key starts; press consumed
            },
            Mode::Pause { sel } => match (&key, ch) {
                (Key::Named(NamedKey::Escape), _) | (Key::Named(NamedKey::Tab), _) => {
                    self.mode = Mode::Play
                }
                // Arrow-key navigation across the button row (wraps). Up/Left
                // and Down/Right are aliases so a d-pad or stick mapped to
                // either axis walks the row — the same nav feel as the list
                // menus, on the one screen that never had it.
                (Key::Named(NamedKey::ArrowLeft), _) | (Key::Named(NamedKey::ArrowUp), _) => {
                    let n = hud::PAUSE_BTNS.len();
                    self.mode = Mode::Pause { sel: (sel + n - 1) % n };
                }
                (Key::Named(NamedKey::ArrowRight), _) | (Key::Named(NamedKey::ArrowDown), _) => {
                    self.mode = Mode::Pause { sel: (sel + 1) % hud::PAUSE_BTNS.len() };
                }
                // Enter activates the SELECTED button (default RESUME). The
                // old Enter-opens-Settings shortcut is superseded by the nav;
                // g/o/q stay as fast paths.
                (Key::Named(NamedKey::Enter), _) => self.pause_activate(sel, elwt),
                (_, "g") => self.open_menu(Mode::Maps { sel: self.save.map }),
                (_, "o") => self.open_menu(Mode::Online { sel: 0 }),
                (_, "q") | (_, "x") => {
                    self.net.leave();
                    self.persist();
                    elwt.exit();
                }
                _ => {}
            },
            Mode::Binds { sel, capturing } => self.binds_key(key, sel, capturing),
            Mode::Tweaks { menu, sel } => self.tweaks_key(&key, menu, sel, win),
            Mode::Maps { sel } => self.maps_key(&key, ch, sel),
            Mode::Online { sel } => self.online_key(&key, ch, sel),
            Mode::Play => {
                match &key {
                    Key::Named(NamedKey::Escape) | Key::Named(NamedKey::Tab) => {
                        if self.watching.is_some() {
                            self.watching = None; // skip the replay
                            return;
                        }
                        self.mode = Mode::Pause { sel: 0 };
                        self.persist(); // checkpoint whenever the player pauses
                        return;
                    }
                    _ => {}
                }
                // R: watch the best moment you have (again)
                if ch == "r" && !self.moments.is_empty() {
                    self.watching = Some((0, 0.0));
                    let m = &self.moments[0];
                    self.toastf(&format!("REPLAY - {}", m.title));
                    return;
                }
                // [ and ] swap the spaceship: handled in the winit event
                // loop above (alongside the physical-key fallback), where
                // `cycle_ship` does the real regenerate/repark/re-upload.
                // Deliberately NOT duplicated here — an earlier pass added
                // a second bracket handler in this match that only printed
                // a toast, which is the two-answers-to-one-question
                // mistake `--parity` exists to find.
                if let Some(a) = self.action_for_key(&key) {
                    self.one_shot(a);
                }
            }
        }
    }

    fn binds_key(&mut self, key: Key, sel: usize, capturing: bool) {
        if capturing {
            match key {
                Key::Named(NamedKey::Escape) => self.mode = Mode::Binds { sel, capturing: false },
                Key::Named(NamedKey::Delete) | Key::Named(NamedKey::Backspace) => {
                    if sel < ACTIONS.len() {
                        self.binds.clear(ACTIONS[sel]);
                    }
                    self.mode = Mode::Binds { sel, capturing: false };
                }
                other => {
                    if sel < ACTIONS.len() {
                        self.binds.capture(ACTIONS[sel], Bind::Key(other));
                    }
                    self.mode = Mode::Binds { sel, capturing: false };
                }
            }
            return;
        }
        match key {
            Key::Named(NamedKey::Escape) => {
                self.persist();
                if self.binds_to_settings {
                    self.binds_to_settings = false;
                    self.mode = Mode::Tweaks { menu: 0, sel: 0 };
                } else {
                    self.close_menu();
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.mode = Mode::Binds { sel: (sel + hud::BIND_ROWS - 1) % hud::BIND_ROWS, capturing: false }
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.mode = Mode::Binds { sel: (sel + 1) % hud::BIND_ROWS, capturing: false }
            }
            Key::Named(NamedKey::Delete) => {
                if sel < ACTIONS.len() {
                    self.binds.clear(ACTIONS[sel]);
                }
            }
            Key::Named(NamedKey::Enter) => {
                if sel < ACTIONS.len() {
                    self.mode = Mode::Binds { sel, capturing: true };
                } else if sel == ACTIONS.len() {
                    self.binds = Bindings::default_scheme();
                    self.toastf("PRESET: DEFAULT");
                } else if sel == ACTIONS.len() + 1 {
                    self.binds = Bindings::classic();
                    self.toastf("PRESET: CLASSIC");
                } else if sel == ACTIONS.len() + 2 {
                    self.binds = Bindings::dvorak();
                    self.toastf("PRESET: DVORAK");
                } else {
                    self.persist();
                    if self.binds_to_settings {
                        self.binds_to_settings = false;
                        self.mode = Mode::Tweaks { menu: 0, sel: 0 };
                    } else {
                        self.close_menu();
                    }
                }
            }
            _ => {}
        }
            self.follow_selection();
    }

    fn tweaks_key(
        &mut self,
        key: &Key,
        menu: u8,
        sel: usize,
        win: &winit::window::Window,
    ) {
        let rows = tweak_rows(menu);
        let n = rows.len();
        let leave_menu = |g: &mut Game| {
            // leaving the mutators page applies a changed world size
            if menu == 2 && (g.cfg.arena.scale - g.applied_scale).abs() > 0.01 {
                let (m, sd) = (g.save.map, g.save.seed);
                g.load_map(m, sd);
                g.toastf("WORLD RESIZED");
            }
            g.persist();
        };
        match key {
            Key::Named(NamedKey::Escape) => {
                leave_menu(self);
                if menu == 0 {
                    self.close_menu();
                } else {
                    self.mode = Mode::Tweaks { menu: 0, sel: 0 };
                }
                return;
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.mode = Mode::Tweaks { menu, sel: next_sel(&rows, sel, -1) };
                return;
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.mode = Mode::Tweaks { menu, sel: next_sel(&rows, sel, 1) };
                return;
            }
            _ => {}
        }
        let (_, row) = rows[sel.min(n - 1)];
        let dir: i32 = match key {
            Key::Named(NamedKey::ArrowLeft) => -1,
            Key::Named(NamedKey::ArrowRight) => 1,
            _ => 0,
        };
        match row {
            Row::Slider { get, set, min, max, step, def, .. } => {
                if dir != 0 {
                    let v = (get(&self.cfg) + step * dir as f32).clamp(min, max);
                    let _ = &self.gyro;
                    set(&mut self.cfg, v);
                    if menu == 1 {
                        // live readback: proves the value reached the cfg the
                        // camera reads every frame
                        self.toastf(&format!("CAM LIVE: {:.2}", get(&self.cfg)));
                    }
                    if menu == 2 {
                        self.cfg.preset = 0; // hand-tuned = CUSTOM
                    }
                } else if matches!(key, Key::Named(NamedKey::Enter)) {
                    set(&mut self.cfg, def);
                    if menu == 2 {
                        self.cfg.preset = 0;
                    }
                }
            }
            Row::Toggle { get, set } => {
                if dir != 0 || matches!(key, Key::Named(NamedKey::Enter)) {
                    let v = !get(&self.cfg);
                    set(&mut self.cfg, v);
                }
                if self.cfg.gyro.on {
                    // switching gyro on spins up the reader thread and takes
                    // a fresh neutral bias (hold the device still)
                    self.gyro.enable();
                    self.gyro.calibrate();
                    self.toastf("GYRO: CALIBRATING - HOLD STILL");
                }
            }
            Row::Preset => {
                if dir != 0 {
                    let np = PRESETS.len();
                    let next = (self.cfg.preset as i32 + dir).rem_euclid(np as i32) as usize;
                    self.cfg.apply_preset(next);
                    self.cfg.preset = next;
                    self.toastf(PRESETS[next]);
                }
            }
            Row::Link(m) => {
                if matches!(key, Key::Named(NamedKey::Enter)) {
                    self.mode = Mode::Tweaks { menu: m, sel: first_sel(m) };
                }
            }
            Row::GameMode(g) => {
                if matches!(key, Key::Named(NamedKey::Enter)) {
                    self.start_mode(g);
                }
            }
            Row::CarPick => {
                let n = self.cars.models.len() as i32;
                let d = if dir != 0 { dir } else if matches!(key, Key::Named(NamedKey::Enter)) { 1 } else { 0 };
                if d != 0 {
                    let next = (self.cars.active as i32 + d).rem_euclid(n) as usize;
                    self.switch_car(next);
                }
            }
            Row::Header => {}
            Row::Controls => {
                if matches!(key, Key::Named(NamedKey::Enter)) {
                    self.binds_to_settings = true;
                    self.mode = Mode::Binds { sel: 0, capturing: false };
                }
            }
            Row::Trials => {
                self.mscroll = 0.0;
                self.mode = Mode::Trials { sel: self.trial_sel };
            }
            Row::Academy => {
                self.mscroll = 0.0;
                self.mode = Mode::Academy { sel: self.lesson_sel };
            }
            Row::Garage => {
                self.mscroll = 0.0;
                self.mode = Mode::Garage { sel: self.look_sel };
            }
            Row::Catalog => {
                self.mscroll = 0.0;
                self.mode = Mode::Catalog { sel: self.cars.active };
            }
            Row::Fullscreen => {
                if matches!(key, Key::Named(NamedKey::Enter)) || dir != 0 {
                    self.toggle_fullscreen(win);
                }
            }
            Row::Back => {
                if matches!(key, Key::Named(NamedKey::Enter)) {
                    leave_menu(self);
                    if menu == 0 {
                        self.close_menu();
                    } else {
                        self.mode = Mode::Tweaks { menu: 0, sel: 0 };
                    }
                }
            }
        }
            self.follow_selection();
    }

    fn maps_key(&mut self, key: &Key, ch: &str, sel: usize) {
        let n = world::STYLE_COUNT;
        match (key, ch) {
            (Key::Named(NamedKey::Escape), _) => self.close_menu(),
            (Key::Named(NamedKey::ArrowUp), _) => self.mode = Mode::Maps { sel: (sel + n - 1) % n },
            (Key::Named(NamedKey::ArrowDown), _) => self.mode = Mode::Maps { sel: (sel + 1) % n },
            (_, "r") => {
                let seed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(1);
                self.toastf(&format!("NEW SEED {seed}"));
                self.save.seed = seed;
            }
            (Key::Named(NamedKey::Enter), _) => {
                if self.net.role == net::Role::Client {
                    self.toastf("THE HOST PICKS THE MAP");
                } else if self.net.role == net::Role::Host && self.net.player_count() > 1 {
                    self.toastf("LEAVE ONLINE TO CHANGE MAPS");
                } else {
                    let seed = self.save.seed;
                    self.load_map(sel, seed);
                    self.persist();
                    self.mode = Mode::Play;
                }
            }
            _ => {}
        }
            self.follow_selection();
    }

    fn online_key(&mut self, key: &Key, ch: &str, _sel: usize) {
        // First live menu::Menu consumer (see online_screen.rs): nav, text
        // entry, and activation are the Menu's own ops; this fn only routes
        // keys in and applies the events that come back.
        let Some(mut m) = self.online_menu.take() else {
            self.online_menu = Some(online_screen::build(
                &self.save.name,
                &self.friend,
                self.net.role != net::Role::Offline,
            ));
            return;
        };
        let ev = match key {
            Key::Named(NamedKey::Escape) => {
                self.persist();
                self.close_menu();
                self.online_menu = None;
                return;
            }
            Key::Named(NamedKey::ArrowUp) => {
                m.step(-1);
                None
            }
            Key::Named(NamedKey::ArrowDown) => {
                m.step(1);
                None
            }
            Key::Named(NamedKey::Backspace) => m.backspace(),
            Key::Named(NamedKey::Enter) => m.activate(),
            _ => {
                if let Some(c) = ch.chars().next() {
                    m.input_char(c)
                } else {
                    None
                }
            }
        };
        // Mirror the Menu's selection into Mode::Online{sel} so the shared
        // hover/draw tables keep working until they migrate too.
        self.mode = Mode::Online { sel: m.sel() };
        if let Some(ev) = ev {
            self.apply_online_event(ev, &mut m);
        }
        self.online_menu = Some(m);
    }

    /// Apply an Online-screen menu event. Split out so mouse clicks and
    /// touch can route through the same arm as the keyboard.
    fn apply_online_event(&mut self, ev: menu::Event, m: &mut menu::Menu) {
        use online_screen as os;
        match ev {
            menu::Event::Changed(os::ID_NAME, menu::Value::Text(t)) => self.save.name = t,
            menu::Event::Changed(os::ID_FRIEND, menu::Value::Text(t)) => self.friend = t,
            menu::Event::Pressed(os::ID_HOST) => {
                let n = self.save.name.clone();
                let st = self.net.host(&n);
                self.toastf(&st);
                set_row_enabled(m, os::ID_LEAVE, self.net.role != net::Role::Offline);
            }
            menu::Event::Pressed(os::ID_JOIN) => {
                let n = self.save.name.clone();
                let f = self.friend.clone();
                let st = self.net.join(&n, &f);
                self.toastf(&st);
                set_row_enabled(m, os::ID_LEAVE, self.net.role != net::Role::Offline);
            }
            menu::Event::Pressed(os::ID_LEAVE) => {
                self.net.leave();
                self.toastf("OFFLINE");
                set_row_enabled(m, os::ID_LEAVE, false);
            }
            menu::Event::Pressed(os::ID_BACK) | menu::Event::Close => {
                self.persist();
                self.close_menu();
                self.online_menu = None;
            }
            _ => {}
        }
    }


    /// Movement axis with the Wooting wind-in: full press = 1.0 and any press
    /// lands high (still snappy), but a feathered key trims the output — the
    /// slight decrease in turn on a non-full press that makes dribble starts
    /// tunable without micro-tapping. Falls back to digital everywhere else.
    fn axis(&self, a: Action) -> f32 {
        let soft = if self.cfg.car.analog { self.cfg.car.analog_soft } else { 0.0 };
        let mut best = 0.0f32;
        for b in self.binds.binds(a) {
            let v = match b {
                Bind::Key(k) => analog::wind(self.analog.value(k, self.held.contains(k)), soft),
                Bind::Mouse(m) => {
                    if self.held_mouse.contains(m) {
                        1.0
                    } else {
                        0.0
                    }
                }
                Bind::Pad(c) => {
                    if self.last_pad.pressed & (1 << c) != 0 {
                        1.0
                    } else {
                        0.0
                    }
                }
            };
            best = best.max(v);
        }
        best
    }

    /// Raw analog axis (deadzone only, no wind curve) — used where the value
    /// is a RATIO rather than a command, like the slide key's roll/yaw blend.
    fn axis_raw(&self, a: Action) -> f32 {
        let mut best = 0.0f32;
        for b in self.binds.binds(a) {
            let v = match b {
                Bind::Key(k) => {
                    let v = self.analog.value(k, self.held.contains(k));
                    if v < 0.06 { 0.0 } else { v }
                }
                Bind::Mouse(m) => self.held_mouse.contains(m) as i32 as f32,
                Bind::Pad(c) => (self.last_pad.pressed & (1 << c) != 0) as i32 as f32,
            };
            best = best.max(v);
        }
        best
    }

    /// Unproject a normalized screen point onto the terrain: the drive-to
    /// target for tap-to-move. Marches the view ray until it goes underground.
    fn screen_to_ground(&self, p: glam::Vec2) -> Option<Vec3> {
        let ndc = glam::Vec4::new(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 1.0, 1.0);
        let w = self.last_ivp * ndc;
        if w.w.abs() < 1e-6 {
            return None;
        }
        let far = Vec3::new(w.x / w.w, w.y / w.w, w.z / w.w);
        let ro = self.cam.pos;
        let rd = (far - ro).normalize_or(Vec3::NEG_Y);
        let mut t = 2.0f32;
        while t < 320.0 {
            let q = ro + rd * t;
            if q.y <= self.world.ground_h(q.x, q.z) + 0.4 {
                return Some(q);
            }
            t += 1.5;
        }
        None
    }

    fn controls(&self) -> Controls {
        let ps = &self.last_pad;
        let fwd = self.axis(Action::Forward);
        let rev = self.axis(Action::Reverse);
        let sl = self.axis(Action::SteerL);
        let sr = self.axis(Action::SteerR);
        let rl = self.axis(Action::RollL);
        let rr = self.axis(Action::RollR);
        // steer and air roll are one: by default steer ROLLS in the air, so a
        // powerslide jump carries its drift angle straight into the dribble.
        // The hold key temporarily swaps steer back to yaw (and if you flip
        // the default in FLIPS, the hold swaps to roll instead).
        // Steer and air roll share the key; SLIDE is the modifier (RL's
        // classic scheme: powerslide doubles as air roll). Held slide turns
        // steer into barrel roll, released it's air turn — and on a Wooting
        // the slide key's analog depth BLENDS the two: half-press = half
        // roll, half yaw. The FLIPS toggle inverts which one is default.
        let slide_a = self.axis_raw(Action::Slide);
        let roll_mix = self.roll_mix_s;
        let steer_kb = sr - sl;
        let steer_axis = (steer_kb + ps.stick_x).clamp(-1.0, 1.0);
        let steer = steer_axis;
        let yaw_src = steer_axis * (1.0 - roll_mix);
        let roll_extra = -steer_axis * roll_mix;
        // ---- on-screen controls fold in as another input source ----
        let (mut t_thr, mut t_steer, mut t_boost, mut t_jump) = (0.0f32, 0.0f32, false, false);
        let mut t_air: Option<glam::Vec2> = None;
        if self.tpad.enabled {
            let sv = self.tpad.stick_vec();
            t_thr = sv.y;
            t_steer = sv.x;
            // AIR ROTATION: off the ground the same thumb flies the car —
            // push/pull pitches, left/right rolls (or yaws with the slide
            // modifier), exactly like a stick in the air on a pad
            if !self.car.on_ground {
                t_air = Some(sv);
            }
            t_boost = self.tpad.boost_down();
            t_jump = self.tpad.jump_down();
            if self.tpad.has_target() {
                // tap-to-move drives itself until you touch the stick
                let (ft, fs) = (self.tap_drive.0, self.tap_drive.1);
                if sv.length() < 0.05 {
                    t_thr = ft;
                    t_steer = fs;
                }
            }
            if let Some(f) = self.tpad.flick {
                // a flick dodges: feed the direction as this frame's input
                t_thr = f.y;
                t_steer = f.x;
            }
        }
        let steer = (steer + t_steer).clamp(-1.0, 1.0);
        // FLIGHT MODE gets independent throttle/pitch. Ground vehicles keep
        // fwd/rev driving BOTH (the Rocket-League aerial-trick coupling,
        // deliberate and correct there — holding forward pitches the nose
        // down to angle into a hit); a jet needs the two decoupled, which
        // the shared keys structurally can't give it. `Action::PitchUp`/
        // `PitchDown` are flight-only bindings (see their own doc comments
        // in `input.rs`) that do nothing outside `self.car.flight`, so a
        // player who never flies never has to learn they exist.
        let pitch = if self.car.flight {
            let pu = self.axis(Action::PitchUp);
            let pd = self.axis(Action::PitchDown);
            (pd - pu - ps.stick_y + t_air.map(|v| -v.y).unwrap_or(0.0)).clamp(-1.0, 1.0)
        } else {
            (rev - fwd - ps.stick_y + t_air.map(|v| -v.y).unwrap_or(0.0)).clamp(-1.0, 1.0)
        };
        Controls {
            throttle: (fwd - rev + ps.throttle + t_thr).clamp(-1.0, 1.0),
            steer,
            pitch,
            yaw: ((-yaw_src).clamp(-1.0, 1.0) + t_air.map(|v| v.x).unwrap_or(0.0) * roll_mix).clamp(-1.0, 1.0),
            roll: ((rl - rr + roll_extra).clamp(-1.0, 1.0) + t_air.map(|v| v.x).unwrap_or(0.0) * (1.0 - roll_mix)).clamp(-1.0, 1.0),
            boost: self.action_down(Action::Boost) || t_boost,
            jump_held: self.action_down(Action::Jump) || t_jump,
            jump_pressed: false,
            jump_depth: self.axis_raw(Action::Jump).max(0.01),
            powerslide: slide_a > 0.35,
            aiming: self.action_down(Action::Fire) || self.action_down(Action::AutoTarget),
        }
    }

    fn frame(&mut self, dt: f32, win: &winit::window::Window) {
        self.time += dt;
        // the deck's clocks: cascade, cursor spring, folds. When a fold
        // finishes with nothing pending, the console leaves the stage.
        if let Some(d) = self.deck.as_mut() {
            d.tick(dt);
            if d.is_shut() {
                self.close_deck();
            }
        }
        // on-screen controls: recognizer + one-shot decay
        self.tpad.enabled |= self.cfg.touch.enabled;
        if self.tpad.enabled {
            self.tpad.tick(dt);
            if let Some(p) = self.tpad.tap_pending.take() {
                if self.cfg.touch.tap_to_move {
                    self.tpad.target = self.screen_to_ground(p);
                }
            }
        }
        if self.tpad.enabled {
            let f = self.car.forward();
            self.tap_drive = self.tpad.follow(self.car.pos, f);
        }
        // live render-scale changes (the VIDEO slider, or the phone preset)
        if (self.cfg.video.render_scale - self.last_scale).abs() > 0.01 {
            self.last_scale = self.cfg.video.render_scale;
            self.gpu.set_render_scale(self.last_scale);
        }
        self.analog.poll();
        {
            // roll/yaw blend: snap the ends (decisive digital feel), keep the
            // analog middle, and SMOOTH over time — interpolation, not a
            // per-frame switch, so it can't flicker at the actuation point
            let a = self.axis_raw(Action::Slide);
            let raw = if self.cfg.flip.air_roll_default { 1.0 - a } else { a };
            let target = if raw < 0.15 {
                0.0
            } else if raw > 0.85 {
                1.0
            } else {
                raw
            };
            self.roll_mix_s += (target - self.roll_mix_s) * (1.0 - (-14.0 * dt).exp());
        }
        if self.cfg.video.vsync != self.last_vsync {
            self.last_vsync = self.cfg.video.vsync;
            self.gpu.set_vsync(self.last_vsync);
        }
        self.capture_guard = (self.capture_guard - dt).max(0.0);

        // cursor: visible everywhere except while playing; hover drives
        // menus. While playing we also GRAB it (Confined, Locked fallback)
        // so windowed Wayland can't let it wander onto docks mid-dribble.
        let want_vis = self.mode != Mode::Play;
        if want_vis != self.prev_cursor_vis {
            win.set_cursor_visible(want_vis);
            use winit::window::CursorGrabMode as G;
            if want_vis {
                let _ = win.set_cursor_grab(G::None);
            } else {
                let _ = win.set_cursor_grab(G::Confined).or_else(|_| win.set_cursor_grab(G::Locked));
            }
            self.prev_cursor_vis = want_vis;
        }
        if self.cursor_moved {
            self.cursor_moved = false;
            if let Some(i) = self.hover_row() {
                match self.mode {
                    Mode::Tweaks { menu, .. } => self.mode = Mode::Tweaks { menu, sel: i },
                    Mode::Binds { capturing: false, .. } => {
                        self.mode = Mode::Binds { sel: i, capturing: false }
                    }
                    Mode::Maps { .. } => self.mode = Mode::Maps { sel: i },
                    Mode::Online { .. } => self.mode = Mode::Online { sel: i },
                    _ => {}
                }
            }
        }
        // FPS MEANS FRAMES THAT REACHED THE SCREEN.
        //
        // This used to smooth 1/dt of the redraw callback, which counts loop
        // iterations — including every iteration that acquired nothing and
        // drew nothing. With vsync on a 240 Hz panel it happily reported 600.
        // Counting real presents over a wall-clock window cannot do that:
        // the number is now bounded by the display, and if it reads 30 while
        // the loop spins, that is the truth rather than a hidden failure.
        {
            let (pres, recfg, drop) = self.gpu.present_stats();
            self.fps_win += dt;
            // A COUNTER THAT READS EXACTLY ZERO IS NOT A MEASUREMENT, it is
            // a broken counter — a running game always presents something.
            // Falling back to the loop rate says "the accounting is wrong"
            // rather than "your machine is dead", and the drop readout beside
            // it still tells the true story.
            if self.fps_win >= 0.5 && pres == self.fps_presented && pres == 0 {
                self.fps = if dt > 1e-5 { 1.0 / dt } else { self.fps };
                self.fps_win = 0.0;
            } else if self.fps_win >= 0.5 {
                let shown = (pres - self.fps_presented) as f32 / self.fps_win;
                self.fps = shown;
                self.frame_drops = (drop + recfg - self.fps_dropped) as u32;
                self.fps_presented = pres;
                self.fps_dropped = drop + recfg;
                self.fps_win = 0.0;
            }
        }
        self.toast_t = (self.toast_t - dt).max(0.0);
        if self.toast_t <= 0.0 {
            self.toast.clear();
        }
        // a hold has to fire while the key is DOWN, not on release
        if let Some(p) = self.space.update(dt) {
            self.space_intent(p);
        }
        if !self.space.down() && !self.cfg.car.autotarget_hold {
            // cycle mode leaves the lock where it was
        }
        self.sight.update(dt);
        self.advance_xform(dt);
        self.advance_walk();
        self.update_autotarget(dt);
        self.update_locomotion(dt);

        // ---- controller ----
        let ps = self.pad.poll();
        self.last_pad = ps;
        if ps.start_pressed {
            match self.mode {
                Mode::Splash | Mode::Pause { .. } => self.mode = Mode::Play,
                Mode::Play => {
                    self.mode = Mode::Pause { sel: 0 };
                    self.persist();
                }
                _ => {}
            }
        }
        if let Mode::Binds { sel, capturing: true } = self.mode {
            if let Some(code) = ps.any_edge() {
                if sel < ACTIONS.len() {
                    self.binds.capture(ACTIONS[sel], Bind::Pad(code));
                }
                self.mode = Mode::Binds { sel, capturing: false };
            }
        } else if self.mode == Mode::Play && ps.just_pressed != 0 {
            let acts: Vec<Action> = ACTIONS
                .iter()
                .copied()
                .filter(|a| {
                    self.binds
                        .binds(*a)
                        .iter()
                        .any(|b| matches!(b, Bind::Pad(c) if ps.edge(*c)))
                })
                .collect();
            for a in acts {
                self.one_shot(a);
            }
        } else if self.mode == Mode::Splash && ps.just_pressed != 0 {
            self.mode = Mode::Play;
        }

        // ---- simulate ----
        // menu fling: the list keeps moving after the finger leaves, then
        // settles — standard touch scrolling
        if self.tdown.is_none() && self.mfling.abs() > 0.01 {
            let max = self.menu_max_scroll();
            self.mscroll = (self.mscroll + self.mfling * dt).clamp(0.0, max);
            self.mfling *= (-6.0 * dt).exp();
            if self.mscroll <= 0.0 || self.mscroll >= max {
                self.mfling = 0.0;
            }
        }
        let mut c = self.controls();
        if self.mode == Mode::Play {
            // slight temporal buildup into turns and rolls: ~55 ms attack
            // (x INPUT EASE) with a whisper of center-softening — the
            // detent. Player only; bots keep raw controls.
            // the on-screen MENU button: a phone has no Escape key
            if self.tpad.enabled && self.tpad.menu_tapped() {
                self.mode = Mode::Pause { sel: 0 };
                self.mscroll = 0.0;
            }
            // GYRO DRIVE: device motion folded in beside the sticks
            self.gyro.update(dt, &self.cfg.gyro);
            if self.cfg.gyro.on {
                let gate = match self.cfg.gyro.gate {
                    1 => c.boost,
                    2 => !self.car.on_ground,
                    _ => true,
                };
                let (gs, gp, gr) = self.gyro.drive(&self.cfg.gyro, !self.car.on_ground, gate);
                c.steer = (c.steer + gs).clamp(-1.0, 1.0);
                c.pitch = (c.pitch + gp).clamp(-1.0, 1.0);
                c.roll = (c.roll + gr).clamp(-1.0, 1.0);
                c.yaw = (c.yaw + gs * self.roll_mix_s).clamp(-1.0, 1.0);
            }
            let tau = 0.055 * self.cfg.car.input_ease;
            let ek = 1.0 - (-dt / tau.max(0.001)).exp();
            self.in_steer += (c.steer - self.in_steer) * ek;
            self.in_yaw += (c.yaw - self.in_yaw) * ek;
            self.in_roll += (c.roll - self.in_roll) * ek;
            c.steer = self.in_steer * (0.88 + 0.12 * self.in_steer.abs());
            c.yaw = self.in_yaw * (0.88 + 0.12 * self.in_yaw.abs());
            c.roll = self.in_roll * (0.88 + 0.12 * self.in_roll.abs());
            // live mutators: ball radius + car scale follow the settings
            self.ball.r = physics::BALL_R * self.cfg.ball.size;
            let mi = self.cars.active;
            apply_car_dims(&mut self.car, &self.cars.models[mi], self.cfg.car.size);
            // HIT-STOP: a heavy impact holds the world for a few frames.
            // The accumulator does not fill, so no substep runs; the
            // camera and the HUD keep drawing what is there.
            if self.hitstop > 0.0 {
                self.hitstop = (self.hitstop - dt).max(0.0);
            } else {
                self.acc += dt;
            }
            let mut ev = StepEvents::default();
            let mut substeps = 0u32;
            while self.acc >= FIXED_DT {
                // spiral-of-death guard: at most 10 catch-up substeps a
                // frame (83 ms of sim); beyond that we shed the backlog
                substeps += 1;
                if substeps > 10 {
                    self.acc = self.acc.min(FIXED_DT);
                    break;
                }
                // consume the jump edge INSIDE the loop: clearing it before
                // guaranteed a substep meant frames with acc < FIXED_DT ate
                // the press outright — the "jump sometimes never comes" bug
                c.jump_pressed = self.jump_edge || self.tpad.jump_edge;
                self.jump_edge = false;
                self.tpad.jump_edge = false;
                self.prev_cpos = self.car.pos;
                self.prev_crot = self.car.rot;
                self.prev_bpos = self.ball.pos;
                self.prev_brot = self.ball.rot;
                // A DROP REPLACES THE CAR'S PHYSICS for as long as it lasts:
                // the falling body is `drop::Drop`'s, and the car mirrors it
                // so the camera and the renderer follow for free. The jump
                // edge is the fold — same button, same consume-inside-the-
                // loop rule as the physics jump above.
                if self.sortie.as_ref().map(|s| s.falling()).unwrap_or(false) {
                    let gy = {
                        let s = self.sortie.as_ref().unwrap();
                        self.world.ground_h(s.drop.pos.x, s.drop.pos.z)
                    };
                    let folding = c.jump_pressed;
                    let s = self.sortie.as_mut().unwrap();
                    if folding {
                        s.fold();
                    }
                    let steer = Vec3::new(c.steer, 0.0, -c.throttle);
                    s.update(FIXED_DT, steer, gy, &mut self.body);
                    self.car.pos = s.drop.pos;
                    self.car.vel = s.drop.vel;
                    // TOUCHDOWN ON A DISTRICT: the wave stands in the
                    // nearest plaza, facing the LZ, not at a hashed bearing
                    // in the open. Done here, before the first
                    // `tick_engage` musters anything.
                    if s.engaged() {
                        let host: &dyn worldhost::WorldHost = &self.world;
                        let lz = s.drop.pos;
                        if let Some(p) = host
                            .plazas()
                            .iter()
                            .min_by(|a, b| a.distance_squared(lz).total_cmp(&b.distance_squared(lz)))
                        {
                            let gy = host.ground_h(p.x, p.z);
                            s.field.anchor = Vec3::new(p.x, gy, p.z);
                            let to = lz - *p;
                            s.field.facing = Vec3::new(to.x, 0.0, to.z).normalize_or(Vec3::Z);
                            // and the plaza's own blocks, so the front stands behind them
                            s.field.cover = host.cover().iter().filter(|c| (**c - *p).length() < 40.0).copied().collect();
                            s.field.avoid = host.obstacles().to_vec();
                        }
                    }
                    // THE BODY READS THE FALL: nose down the velocity as a
                    // dart, upright over flat heading as a parachute. The
                    // car's forward is -Z in its own frame (`Car::forward`),
                    // so the arc maps that. Eased, because a fold is a
                    // decelerating lurch, not a teleport — same reasoning
                    // as the drop's own drag curve.
                    let v = s.drop.vel;
                    if v.length_squared() > 1.0 {
                        let target = match s.drop.mode {
                            drop::Mode::Vehicle => {
                                Quat::from_rotation_arc(Vec3::NEG_Z, v.normalize())
                            }
                            drop::Mode::Robot => {
                                let flat =
                                    (v * Vec3::new(1.0, 0.0, 1.0)).normalize_or(Vec3::NEG_Z);
                                Quat::from_rotation_arc(Vec3::NEG_Z, flat)
                            }
                        };
                        let k = 1.0 - (-6.0 * FIXED_DT).exp();
                        self.car.rot = self.car.rot.slerp(target, k);
                    }
                    c.jump_pressed = false;
                    if folding {
                        // the visual fold follows the drop's flip. A
                        // chassis that cannot fold visually still drops
                        // honestly — the machine is the truth, the mesh
                        // catches up when it can (`transform` says why not
                        // in a toast if it must).
                        self.transform();
                    }
                    self.acc -= FIXED_DT;
                    continue;
                }
                // the raid's descent — mirrors the sortie branch above,
                // same machine, same rules; a THIRD flow is the cue to
                // extract this block rather than mirror it again
                if self.raid.as_ref().map(|r| r.falling()).unwrap_or(false) {
                    let gy = {
                        let r = self.raid.as_ref().unwrap();
                        self.world.ground_h(r.drop.pos.x, r.drop.pos.z)
                    };
                    let folding = c.jump_pressed;
                    let r = self.raid.as_mut().unwrap();
                    if folding {
                        r.fold();
                    }
                    let steer = Vec3::new(c.steer, 0.0, -c.throttle);
                    r.update(FIXED_DT, steer, gy, &mut self.body);
                    self.car.pos = r.drop.pos;
                    self.car.vel = r.drop.vel;
                    let v = r.drop.vel;
                    if v.length_squared() > 1.0 {
                        let target = match r.drop.mode {
                            drop::Mode::Vehicle => {
                                Quat::from_rotation_arc(Vec3::NEG_Z, v.normalize())
                            }
                            drop::Mode::Robot => {
                                let flat =
                                    (v * Vec3::new(1.0, 0.0, 1.0)).normalize_or(Vec3::NEG_Z);
                                Quat::from_rotation_arc(Vec3::NEG_Z, flat)
                            }
                        };
                        let k = 1.0 - (-6.0 * FIXED_DT).exp();
                        self.car.rot = self.car.rot.slerp(target, k);
                    }
                    c.jump_pressed = false;
                    if folding {
                        self.transform();
                    }
                    self.acc -= FIXED_DT;
                    continue;
                }
                physics::step(&mut self.car, &mut self.ball, &mut self.world, &c, &self.cfg, FIXED_DT, &mut ev);
                c.jump_pressed = false;
                // THE ENGAGEMENT TICKS WITH THE WORLD, at the same fixed
                // rate as everything else — `resolve` is for terminals.
                // The player's fire reaches it through `fire_weapon`, shot
                // by shot, with the weapon's real numbers.
                if let Some(s) = self.sortie.as_mut() {
                    if s.engaged() {
                        s.tick_engage(FIXED_DT);
                    }
                }
                // THE WAVE, IN THE WORLD: the field is stepped with the
                // player and its events applied here (`tick_field`)
                self.tick_field(FIXED_DT);
                // THE ASSAULT TICKS ON POSITIONS: the machine is handed the
                // player and the two seat-holding squad ghosts, and hands
                // back what its mechanics did. Slams and chips are the
                // shell's to apply — the runtime never moves a unit.
                let mut raid_ev: Vec<raidrun::Event> = Vec::new();
                if let Some(r) = self.raid.as_mut() {
                    if r.assaulting() {
                        let gy = self.world.ground_h(self.car.pos.x, self.car.pos.z);
                        raid_ev = r.tick(FIXED_DT, self.car.pos, gy);
                    }
                }
                // BEACONS, ACTUALLY DRAWN. `RaidFlow::live_beacons` exists
                // (via `PhaseRun::beacons`, "for the shell to draw"), but
                // nothing ever called it — an Assault had no in-world
                // marker at any encounter site, only the HUD text panel.
                // A periodic particle pulse reuses `fx`'s existing,
                // already-shipped confetti-burst code rather than adding
                // a new persistent-marker rendering primitive I have no
                // way to visually check from here; tinted per mech so an
                // AIR node and a SYNC plate at least read differently.
                self.beacon_fx_t -= FIXED_DT;
                if self.beacon_fx_t <= 0.0 {
                    self.beacon_fx_t = 0.6;
                    if let Some(r) = self.raid.as_ref() {
                        for b in r.live_beacons() {
                            if b.done { continue; }
                            let c = match b.mech {
                                raidcon::Mech::Purge => [0.85, 0.35, 0.30, 1.0],
                                raidcon::Mech::Ground => [0.55, 0.85, 0.35, 1.0],
                                raidcon::Mech::Air => [0.55, 0.75, 1.0, 1.0],
                                raidcon::Mech::Sync => [0.85, 0.65, 1.0, 1.0],
                            };
                            self.fx.ring_burst(Vec3::new(b.pos[0], b.pos[1], b.pos[2]), c, b.anchor);
                        }
                    }
                }
                for e in raid_ev {
                    match e {
                        raidrun::Event::Slam { vy, .. } => {
                            self.car.vel.y = vy;
                        }
                        raidrun::Event::Chip { dmg, .. } => {
                            let was = self.body.alive();
                            let _ = self.body.hit(
                                sections::Kind::Beam,
                                Vec3::new(0.0, 1.0, 0.0),
                                Vec3::Y,
                                dmg,
                                0.4,
                            );
                            if was && !self.body.alive() {
                                self.toastf("STRUCTURAL FAILURE — grounded");
                            }
                        }
                        _ => {}
                    }
                }
                // LIVE SHOTS fly under the map's own medium and gravity;
                // impacts land through the SAME conversion the instant
                // path uses — same sections, same shove, a few frames
                // later. Blast splash now reaches nearby bots too, not
                // just the locked target or a struck bot's own limbs
                // (see the splash loop inside the hits loop below).
                if !self.shots.live.is_empty() {
                    let marks: Vec<shots::Mark> = self
                        .bots
                        .iter()
                        .map(|b| shots::Mark {
                            pos: b.car.pos,
                            vel: b.car.vel,
                            r: b.car.half_len.max(b.car.half_h),
                        })
                        .collect();
                    let hits = self.shots.step(
                        FIXED_DT,
                        self.world_g,
                        &self.ambient,
                        &marks,
                        &|x, z| self.world.ground_h(x, z),
                    );
                    for h in hits {
                        let amount = h.dv.length() * IMPULSE_TO_DAMAGE;
                        match h.target {
                            Some(i) => {
                                // a flying shot's register: blasts land heavy, slugs land medium — the pool does not carry the form
                                let reg = if h.delivery == ballistics::Delivery::Blast { weapons::Register::Heavy } else { weapons::Register::Medium };
                                self.hostile_hit(i, amount, h.element, reg);
                                if let Some(b) = self.bots.get_mut(i) {
                                    let kind = match h.delivery {
                                        ballistics::Delivery::Blast => sections::Kind::Blast,
                                        _ => sections::Kind::Kinetic,
                                    };
                                    let local = (h.at - b.car.pos) * 0.5;
                                    let _ = b.body.hit(kind, local, h.dv.normalize_or(Vec3::Y), amount, 1.0);
                                    b.car.vel += h.dv;
                                    self.fx.ring_burst(h.at, [1.0, 0.6, 0.25, 1.0], false);
                                }
                            }
                            None => self.fx.ring_burst(h.at, [0.85, 0.7, 0.45, 1.0], false),
                        }
                        // THE SPLASH, FINALLY WIRED. `Delivery::Blast` named
                        // it and never touched anyone but the locked target
                        // — not even `sections::Body::hit`'s own "spread"
                        // reaches other bots, only a struck bot's own
                        // neighbouring limbs. Runs for a ground miss too
                        // (`h.target: None`): a rocket landing in the dirt
                        // beside a bot should still be able to hurt it,
                        // and `shots::step` now gives a ground impact the
                        // same falloff'd `dv` a locked hit gets rather than
                        // the dead zero vector it used to report. A fixed
                        // radius rather than the firing weapon's own reach:
                        // by the time a guided shot lands, the player may
                        // have switched forms entirely, so there is no live
                        // weapon state left to ask.
                        const SPLASH_R: f32 = 4.0;
                        let mut splashed: Vec<(usize, f32)> = Vec::new();
                        if h.delivery == ballistics::Delivery::Blast && amount > 0.0 {
                            for (j, other) in self.bots.iter_mut().enumerate() {
                                if h.target == Some(j) { continue; }
                                let d = (other.car.pos - h.at).length();
                                if d > SPLASH_R { continue; }
                                let falloff = 1.0 - (d / SPLASH_R);
                                let splash = amount * falloff * 0.5;
                                if splash <= 0.0 { continue; }
                                let local = (h.at - other.car.pos) * 0.5;
                                let _ = other.body.hit(
                                    sections::Kind::Blast, local, Vec3::Y, splash, SPLASH_R,
                                );
                                if let Some(slot) = other.brain.hostile_slot() {
                                    splashed.push((slot, splash));
                                }
                            }
                            for (slot, dmg) in splashed.drain(..) {
                                if let Some(s) = self.sortie.as_mut() {
                                    s.strike_slot(slot, dmg, h.element, weapons::Register::Heavy);
                                }
                            }
                        }
                    }
                }
                self.acc -= FIXED_DT;
            }
            // whatever the flow wanted said this frame, said once — the
            // toast is single-slot, so the FEED keeps the history and the
            // last line wins the slot
            // THE WINDOW FOLLOWS THE CAR (district only; see `page_window`)
            if self.mode == Mode::Play {
                self.page_window();
            }
            // THE FORGE RESOLVES IN THE LOOP: one try_recv per frame, no
            // blocking, no async runtime — the worker thread owns the wait.
            if let Some(rx) = &self.forge_rx {
                match rx.try_recv() {
                    Ok(res) => {
                        self.forge_rx = None;
                        match res.and_then(|t| raidforge::accept(&t)) {
                            Ok(name) => self.toastf(&format!(
                                "a new signal resolves on the wardeck — {name}"
                            )),
                            Err(e) => self.toastf(&format!("the forge sputtered: {e}")),
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.forge_rx = None;
                        self.toastf("the forge went cold — no answer came back");
                    }
                }
            }
            let mut sortie_lines: Vec<String> = Vec::new();
            if let Some(s) = self.sortie.as_mut() {
                while let Some(t) = s.pop_toast() {
                    sortie_lines.push(t);
                }
            }
            if let Some(r) = self.raid.as_mut() {
                while let Some(t) = r.pop_toast() {
                    sortie_lines.push(t);
                }
            }
            for t in sortie_lines {
                self.toastf(&t);
            }

            // host: remote cars can touch the authoritative ball
            if self.net.role == net::Role::Host {
                let dims: Vec<(f32, f32, f32)> = self
                    .net
                    .peers()
                    .map(|p| {
                        let m = &self.cars.models[p.car.min(self.cars.models.len() - 1)];
                        (m.half_len, m.half_h.max(0.12), m.wheel_r.max(0.06))
                    })
                    .collect();
                let states: Vec<(Vec3, Vec3, Quat)> =
                    self.net.peers().map(|p| (p.pos, p.vel, p.rot)).collect();
                for (i, (pos, vel, rot)) in states.into_iter().enumerate() {
                    let mut proxy = Car::new();
                    proxy.pos = pos;
                    proxy.vel = vel;
                    proxy.rot = rot;
                    proxy.half_len = dims[i].0;
                    proxy.half_h = dims[i].1;
                    proxy.wheel_r = dims[i].2;
                    physics::collide_car_ball(&mut proxy, &mut self.ball, &self.cfg, physics::WHO_REMOTE, &mut ev);
                }
            }

            self.consume_events(&ev, dt);
            self.world.update(dt);
            if let Some(n) = self.world.sky_news.take() {
                match n {
                    0 => self.toastf("A WORLD IS COLLAPSING - FIFTEEN SECONDS"),
                    1 => {
                        self.toastf("DETONATION");
                        self.cam.add_shake(1.6);
                    }
                    _ => self.toastf("INBOUND: A NEW WORLD FALLING"),
                }
            }
            if let Some(ri) = self.world.flood_news.take() {
                let nm = self.world.route_names.get(ri).cloned().unwrap_or_default();
                self.toastf(&format!("FLOODWATER! {} IS FILLING", nm));
            }
            if !self.bots.is_empty() {
                bots::update(&mut self.bots, &mut self.ball, &mut self.car, &self.world, &self.cfg, self.time, dt);
            }
            // SECTION HEALING/REGROWTH. Gated the same place bots::update
            // is -- Mode::Play, real dt -- so a body does not quietly
            // patch itself up while the game is paused or in a menu.
            // Toasted/fx'd on the frame a section actually comes back,
            // not polled by checking is_gone every frame: tick() only
            // returns non-empty on the frame it happens.
            for s in self.body.tick(dt) {
                self.toastf(&format!("{} regrown", s.name()));
            }
            self.grime.tick(dt);
            for b in self.bots.iter_mut() {
                b.body.tick(dt);
                b.grime.tick(dt);
            }
            self.fx.update(dt);
            let rear = self.held_mouse.contains(&MouseButton::Middle);
            let fold_t = (self.xf.state == xform::State::Transforming).then(|| self.xf.eased());
            self.cam.update(&self.car, &self.ball, &self.world, &self.cfg.cam, self.car.flipping(), rear, dt, fold_t);

            // client: the host's ball is the real one
            if self.net.role == net::Role::Client {
                if let Some((p, v, r, when)) = self.net.net_ball {
                    let age = when.elapsed().as_secs_f32();
                    if age < 1.0 {
                        let target = p + v * age.min(0.15);
                        let k = (dt * 10.0).min(1.0);
                        if (target - self.ball.pos).length() > 4.0 {
                            self.ball.pos = target;
                            self.ball.vel = v;
                        } else {
                            self.ball.pos += (target - self.ball.pos) * k;
                            self.ball.vel += (v - self.ball.vel) * k;
                        }
                        self.ball.rot = self.ball.rot.slerp(r, k);
                    }
                }
            }

            // continuous sound layers
            let sp = self.car.vel.length();
            let engine = if self.car.on_ground {
                (sp / 20.0 + c.throttle.abs() * 0.2).min(1.0)
            } else {
                0.10 + c.throttle.abs() * 0.05
            };
            let boosting = c.boost && self.car.boost > 0.0;
            let sliding = c.powerslide && self.car.on_ground && sp > 4.0;
            self.snd.set_state(engine, (sp / physics::CAR_MAX).powi(2), boosting, sliding);

            if boosting {
                let back = -self.car.forward();
                let tail = self.car.pos + back * (self.car.half_len + 0.12);
                // the garage's trail wins; failing that, an electric car
                // runs an ion thruster and everything else burns fuel
                let kind = if self.look.trail > 0 {
                    self.look.trail
                } else if self.cars.models[self.cars.active].drive == model::Drive::Electric {
                    1
                } else {
                    0
                };
                self.fx.trail_of(kind, tail, back, self.car.vel);
            }

            // ---- adaptive-audio sense: measure, then hand to the mixer ----
            let prox = (1.0 - self.car.pos.distance(self.ball.pos) / 25.0).clamp(0.0, 1.0);
            let goal_d = self
                .world
                .nets
                .iter()
                .map(|n| n.pos.distance(self.ball.pos))
                .fold(f32::INFINITY, f32::min);
            let tension = if goal_d.is_finite() { (1.0 - goal_d / 16.0).clamp(0.0, 1.0) } else { 0.0 };
            let near_bots = self.bots.iter().filter(|b| b.car.pos.distance_squared(self.car.pos) < 22.0 * 22.0).count();
            let swarm = (near_bots as f32 / 25.0).clamp(0.0, 1.0);
            self.snd.set_sense(prox, tension, swarm);
        } else {
            self.acc = 0.0;
            self.jump_edge = false;
            self.snd.set_state(0.0, 0.0, false, false);
            if self.mode == Mode::Splash {
                self.world.update(dt);
            if let Some(n) = self.world.sky_news.take() {
                match n {
                    0 => self.toastf("A WORLD IS COLLAPSING - FIFTEEN SECONDS"),
                    1 => {
                        self.toastf("DETONATION");
                        self.cam.add_shake(1.6);
                    }
                    _ => self.toastf("INBOUND: A NEW WORLD FALLING"),
                }
            }
            if let Some(ri) = self.world.flood_news.take() {
                let nm = self.world.route_names.get(ri).cloned().unwrap_or_default();
                self.toastf(&format!("FLOODWATER! {} IS FILLING", nm));
            }
                let fold_t = (self.xf.state == xform::State::Transforming).then(|| self.xf.eased());
                self.cam.update(&self.car, &self.ball, &self.world, &self.cfg.cam, false, false, dt, fold_t);
            }
        }

        // ---- network (always ticks so sessions survive the pause menu) ----
        let boosting_now = self.action_down(Action::Boost) && self.car.boost > 0.0 && self.mode == Mode::Play;
        let my = net::MyState {
            car: self.cars.active,
            flip: self.flip_vis,
            boosting: boosting_now,
            pos: self.car.pos,
            vel: self.car.vel,
            rot: self.car.rot,
            wheel_spin: self.car.wheel_spin,
            steer_vis: self.car.steer_vis,
        };
        if self.net.online() && !self.bots.is_empty() {
            self.bots.clear();
            self.toastf("BOTS DISMISSED (ONLINE)");
        }
        let mut nev = Vec::new();
        self.net.tick(
            dt,
            &my,
            self.ball.pos,
            self.ball.vel,
            self.ball.rot,
            self.save.map,
            self.save.seed,
            &mut nev,
        );
        self.net.smooth(dt);
        for e in nev {
            match e {
                net::NetEvent::Joined(n) => {
                    self.toastf(&format!("{n} JOINED"));
                    self.snd.fire(Sfx::PadBig);
                }
                net::NetEvent::Left(n) => {
                    self.toastf(&format!("{n} LEFT"));
                    self.snd.fire(Sfx::PadSmall);
                }
                net::NetEvent::MapSync { map, seed } => {
                    self.load_map(map, seed);
                }
            }
        }

        // ---- draw (interpolated between fixed physics steps: no stutter) ----
        let (rc, rb) = if self.mode == Mode::Play {
            let al = (self.acc / FIXED_DT).clamp(0.0, 1.0);
            let cp = if self.prev_cpos.distance(self.car.pos) > 8.0 {
                self.car.pos
            } else {
                self.prev_cpos.lerp(self.car.pos, al)
            };
            let bp = if self.prev_bpos.distance(self.ball.pos) > 8.0 {
                self.ball.pos
            } else {
                self.prev_bpos.lerp(self.ball.pos, al)
            };
            (
                (cp, self.prev_crot.slerp(self.car.rot, al)),
                (bp, self.prev_brot.slerp(self.ball.rot, al)),
            )
        } else {
            ((self.car.pos, self.car.rot), (self.ball.pos, self.ball.rot))
        };
        // suspension bob: the body sits into its compression (draw + mirror)
        let rc = (rc.0 - rc.1 * Vec3::Y * (self.car.susp * 0.045), rc.1);
        // THE LIMP FINALLY SHOWS. `loco_pose.roll` — "the counter-sway of
        // a stride" per `anim::Pose`'s own doc, and the channel the hurt
        // asymmetry leans into (`self.loco_pose.roll += limp * 0.12`) —
        // was being computed every frame and never read anywhere. Scaled
        // by `weight` so it fades in with the gait blend instead of
        // snapping on the first walking frame; gated to robot mode since
        // a car has no stride to counter-sway. `.bob` and `.lean` are
        // left alone: the suspension line above already puts a vertical
        // dip on `rc.0` from `car.susp`, and stacking a second, separate
        // bob on top of it risks double-applying the same visual rather
        // than adding a distinct one.
        let rc = if self.car.robot {
            (rc.0, rc.1 * Quat::from_rotation_z(self.loco_pose.roll * self.loco_pose.weight))
        } else {
            rc
        };
        let (rts, rt_ids) = self.build_rt_shapes(rc, rb);
        self.gpu.set_rt_shapes(&rts);
        let aspect = self.gpu.aspect();
        let (vp, ivp) = self.cam.view_proj(&self.cfg.cam, aspect);
        let planes = frustum_planes(&vp);
        let mut objs = self.build_scene(&rt_ids, rc, rb, &planes);
        self.fx.draw(&mut objs);
        // order the frame: opaque batched by mesh, glass last and back to front
        let eye = self.cam.pos;
        let _opaque = render::order_for_draw(&mut objs, [eye.x, eye.y, eye.z]);

        // (view-projection and aspect computed above, before the scene build)
        
        self.last_ivp = ivp;

        let mut ui: Vec<UiVert> = Vec::new();
        self.peer_tags(&mut ui, vp, aspect);
        match self.mode {
            Mode::Catalog { sel } => hud::catalog_menu(
                &mut ui,
                aspect,
                sel,
                self.cars.active,
                &self.cars.names,
                &self.cars.info,
                self.mscroll.max(0.0) as usize,
                self.cursor,
            ),
            Mode::Garage { sel } => hud::garage_menu(&mut ui, aspect, sel, &self.look, self.cursor),
            Mode::Academy { sel } => hud::academy_menu(
                &mut ui,
                aspect,
                sel,
                &self.passed,
                self.mscroll.max(0.0) as usize,
                self.cursor,
            ),
            Mode::Trials { sel } => hud::trials_menu(
                &mut ui,
                aspect,
                sel,
                &self.bests,
                self.mscroll.max(0.0) as usize,
                self.cursor,
            ),
            Mode::Splash => hud::splash(&mut ui, aspect, self.time, self.style().name),
            Mode::Play => {
                self.draw_hud(&mut ui, aspect);
                self.draw_touch_ui(&mut ui, aspect);
            }
            Mode::Pause { sel } => {
                self.draw_hud(&mut ui, aspect);
                hud::pause(&mut ui, aspect, &self.save, self.air_t, self.cursor, sel);
            }
            Mode::Binds { sel, capturing } => {
                hud::bindings_menu(&mut ui, aspect, &self.binds, sel, capturing, self.mscroll.max(0.0) as usize, self.cursor)
            }
            Mode::Maps { sel } => hud::maps_menu(&mut ui, aspect, sel, self.save.map, self.save.seed, self.mscroll.max(0.0) as usize, self.cursor),
            Mode::Tweaks { menu, sel } => {
                let rows = tweak_rows(menu);
                let trs: Vec<hud::TweakRow> = rows
                    .iter()
                    .map(|(l, r)| {
                        let (value, bar) = row_value(&self.cfg, r);
                        let action = matches!(
                            r,
                            Row::Link(_) | Row::Controls | Row::Fullscreen | Row::Back | Row::Preset | Row::GameMode(_) | Row::CarPick
                        );
                        // the picker names the CLASS as well as the car, so
                        // forty entries read as six families rather than one
                        // undifferentiated list
                        let value = if matches!(r, Row::CarPick) {
                            format!(
                                "{}  [{}]",
                                self.cars.names[self.cars.active].to_uppercase(),
                                self.cars.class_of(self.cars.active)
                            )
                        } else {
                            value
                        };
                        hud::TweakRow { label: (*l).to_string(), value, bar, header: is_header(r), action }
                    })
                    .collect();
                hud::tweaks_menu(&mut ui, aspect, menu_title(menu), &trs, sel, self.mscroll.max(0.0) as usize, self.cursor, nav_here(menu));
            }
            Mode::Online { .. } => {
                // First live consumer of the shared menu draw path
                // (menu::Menu → ui::menu_cmds → draw). The status line and
                // players list stay as a bespoke footer under it — they're
                // live data, not rows.
                if let Some(m) = &self.online_menu {
                    hud::draw_menu(&mut ui, aspect, m);
                }
                let status =
                    if self.toast.is_empty() { &self.net.status } else { &self.toast };
                hud::online_footer(
                    &mut ui,
                    aspect,
                    status,
                    &std::iter::once(format!(
                        "{} (YOU)",
                        if self.save.name.is_empty() { "PILOT" } else { &self.save.name }
                    ))
                    .chain(self.net.peers().map(|p| p.name.clone()))
                    .collect::<Vec<_>>(),
                    self.net.online(),
                );
            }
        }

        // cinema bars
        if self.cfg.video.letterbox && matches!(self.mode, Mode::Play | Mode::Pause { .. }) {
            let bar = [0.0, 0.0, 0.0, 1.0];
            let v = |x: f32, y: f32| UiVert { pos: [x, y], color: bar };
            for (y0, y1) in [(0.865_f32, 1.0_f32), (-1.0, -0.865)] {
                ui.extend_from_slice(&[v(-1.0, y0), v(1.0, y0), v(1.0, y1), v(-1.0, y0), v(1.0, y1), v(-1.0, y1)]);
            }
        }

        let st = self.style();
        // ---- MONKEY RUN weather: clear -> rain -> storm on a 90 s cycle ----
        let stormy_map = st.focal == 13;
        let (mut wx_fogd, mut wx_wet, mut wx_flash) = (1.0f32, 0.0f32, 0.0f32);
        if stormy_map {
            let cyc = self.world.time % 90.0;
            let rain = ((cyc - 50.0) / 6.0).clamp(0.0, 1.0) * (1.0 - ((cyc - 84.0) / 5.0).clamp(0.0, 1.0));
            let storm = ((cyc - 68.0) / 4.0).clamp(0.0, 1.0) * (1.0 - ((cyc - 82.0) / 4.0).clamp(0.0, 1.0));
            wx_fogd = 1.0 + rain * 0.9 + storm * 0.8;
            wx_wet = rain * 0.55;
            self.wx_rain = (rain + storm).min(1.4);
            if storm > 0.1 {
                let cell = (self.world.time / 2.3) as u32;
                let h = cell.wrapping_mul(2654435761) >> 24;
                if h % 5 == 0 && (self.world.time / 2.3).fract() < 0.09 {
                    wx_flash = 1.0; // lightning
                }
            }
        } else {
            self.wx_rain = 0.0;
        }

        // the single RAYS slider, normalised once and reused below
        let q01 = ((self.cfg.video.quality - 0.4) / 1.4).clamp(0.0, 1.0);

        // SUBSEA LIGHTING, when the camera is under the water. Depth is a
        // palette rather than a filter: water eats red in about five metres
        // and blue in two hundred, so the hue IS the depth readout and the
        // player never needs a number for it.
        //
        // Fog distance collapses with depth for the same reason — the same
        // absorption that takes the red also takes the far wall, which is
        // why an underwater scene closes in rather than merely dimming.
        let sub = if self.sea_level.is_some_and(|s| self.cam.pos.y < s) {
            let depth = self.sea_level.unwrap_or(0.0) - self.cam.pos.y;
            let z = if depth > 60.0 {
                hydro::Zone::Trench
            } else if depth > 22.0 {
                hydro::Zone::OpenWater
            } else if depth > 4.0 {
                hydro::Zone::Shallows
            } else {
                hydro::Zone::Surf
            };
            let sunst = sun::SunState {
                dir: [st.sun[0], st.sun[1], st.sun[2]],
                strength: 1.0,
                color: [1.0; 3],
                day: 1.0,
                elev: st.sun[1],
                flaring: false,
            };
            Some(subsea::light(z, &sunst, &raid::TIDEMAW_TIDE, self.time))
        } else {
            None
        };
        let globals = Globals {
            view_proj: vp.to_cols_array(),
            inv_view_proj: ivp.to_cols_array(),
            cam: [self.cam.pos.x, self.cam.pos.y, self.cam.pos.z, self.time],
            sun: [st.sun[0], st.sun[1], st.sun[2], 0.0],
            fog: match &sub {
                // the water's own colour, and it closes in
                Some(w) => [
                    (st.fog[0] * w.tint[0]).max(wx_flash * 0.35),
                    (st.fog[1] * w.tint[1]).max(wx_flash * 0.55),
                    (st.fog[2] * w.tint[2]).max(wx_flash * 0.75),
                    st.fog_d * wx_fogd * (0.10 + 0.30 * w.level),
                ],
                None => [
                    st.fog[0].max(wx_flash * 0.92),
                    st.fog[1].max(wx_flash * 0.92),
                    st.fog[2].max(wx_flash * 0.96),
                    st.fog_d * wx_fogd,
                ],
            },
            misc: [st.zenith[0], st.zenith[1], st.zenith[2], 0.0],
            rt: [if self.cfg.video.rays { 1.0 } else { 0.0 }, self.cfg.video.quality, rt_ids.static_n as f32, st.wet.max(wx_wet)],
            // AO probe count follows quality: four is the full set, two is
            // enough to keep contact shadows readable, and the probes are
            // whole rays so halving them is a real saving rather than a
            // shorter version of the same work.
            // ONE SLIDER, EVERYTHING DOWNSTREAM OF IT.
            //
            // Two dials for one concept is worse than one: nobody knows
            // which to move, and the pair have a cheap-and-ugly corner
            // (short rays, many steps) that no one wants. RAYS is the single
            // control now, and it drives each cost in proportion to what
            // that cost is worth:
            //
            //   step budget    strongly — it is where the time goes
            //   candidates     strongly — cost is candidates x steps
            //   AO probes      in two stages — each probe is a whole ray
            //   ray range      only mildly — measured at 10% of the saving,
            //                  so it is a look control wearing a perf hat
            ray2: [
                // the bottom goes lower than it used to dare, because the
                // blur that comes with it hides what a short march misses
                0.40 + 0.75 * q01,
                if q01 < 0.15 { 2.0 } else { 4.0 },
                0.0,
                0.0,
            ],
            pcount: [self.world.portals.len().min(8) as f32, 0.0, 0.0, 0.0],
            portals: {
                // up to four portals for the marcher; pairs are adjacent, so
                // the shader's partner rule (0<->1, 2<->3) matches the map's
                let mut out = [[0.0f32; 4]; 16];
                for (i, p) in self.world.portals.iter().take(8).enumerate() {
                    out[i * 2] = [p.pos.x, p.pos.y, p.pos.z, p.r];
                    let q = p.rot;
                    out[i * 2 + 1] = [q.x, q.y, q.z, q.w];
                }
                out
            },
            hmap: [
                render::HMAP_HALF,
                render::HMAP_SIZE as f32,
                self.gpu.config.width as f32,
                self.gpu.config.height as f32,
            ],
            gcol: [st.grass[0], st.grass[1], st.grass[2], rts.len() as f32],
            cine: [
                (self.cfg.video.exposure) * (1.0 + wx_flash * 1.3),
                self.cfg.video.vignette,
                self.cfg.video.grain,
                if self.cfg.video.filmic { 1.0 } else { 0.0 },
            ],
            post: [self.cfg.video.bloom, 1.0, self.cfg.video.screen_door, 0.0],
        };
        self.gpu.render(&globals, &objs, &ui);
    }

    fn consume_events(&mut self, ev: &StepEvents, dt: f32) {
        for &(pos, color, is_ball) in &ev.ring_hits {
            self.fx.ring_burst(pos, color, is_ball);
            if is_ball {
                self.combo = if self.combo_t > 0.0 { self.combo + 1 } else { 1 };
                self.combo_t = COMBO_WINDOW;
                self.rings += 1;
                self.save.total_rings += 1;
                if self.combo > self.save.best_combo {
                    self.save.best_combo = self.combo;
                }
                self.snd.fire(Sfx::Chime(self.combo));
            } else {
                self.car_rings += 1;
                self.save.total_car_rings += 1;
                self.snd.fire(Sfx::CarRing);
            }
        }
        self.combo_t = (self.combo_t - dt).max(0.0);
        if self.combo_t <= 0.0 {
            self.combo = 0;
        }

        for &(pos, big) in &ev.pad_hits {
            self.fx.pad_sparkle(pos, big);
            self.fire_at(if big { Sfx::PadBig } else { Sfx::PadSmall }, pos, 1.0);
        }
        for &(pos, n, sp) in ev.bounces.iter().take(3) {
            self.fx.dust(pos - n * physics::BALL_R * 0.9, n, sp);
            self.fire_at(Sfx::Bounce(sp), pos, 1.0);
        }
        // ---- style analyzer: name what just happened ----
        {
            let n_under = self.world.normal(self.car.pos);
            let sf = style::Frame {
                dt,
                car_pos: self.car.pos,
                car_vel: self.car.vel,
                car_up: self.car.up(),
                on_ground: self.car.on_ground,
                flipping: self.car.is_flipping(),
                omega: self.car.omega.length(),
                ball_pos: self.ball.pos,
                ball_vel: self.ball.vel,
                touch: ev.touches.first().map(|&(_, sp)| sp),
                flip_reset: ev.flip_reset.is_some(),
                pinch: ev.pinched,
                goal: !ev.goals.is_empty(),
                surf_up: n_under.y,
                supersonic: self.car.supersonic,
            };
            if self.style.step(&sf) > 0 {
                let combo = self.style.combo();
                let awards: Vec<style::Award> = self.style.fresh.drain(..).collect();
                for a in awards {
                    if combo > 1 {
                        self.toastf(&format!("{}  +{}  x{}", a.name, a.points, combo));
                    } else {
                        self.toastf(&format!("{}  +{}", a.name, a.points));
                    }
                }
            } else {
                self.style.fresh.clear();
            }
        }

        for &(tp, sp) in ev.touches.iter().take(1) {
            if sp > 0.7 {
                self.fire_at(Sfx::Touch(sp), tp, 1.0);
            }
        }
        if ev.jumped {
            self.snd.fire(Sfx::Jump);
        }
        // the IMPACT skin spreads its rings from here
        self.last_hit.1 += dt;
        if let Some(t0) = ev.touches.first() {
            self.last_hit = (self.car.rot.inverse() * (t0.0 - self.car.pos), 0.0);
        }
        // ---- REPLAY: keep the last twelve seconds, and watch for tricks ----
        {
            let ground = self.world.sdf(self.car.pos);
            let ball_air = self.world.sdf(self.ball.pos) > self.ball.r + 0.25;
            if ev.flip_reset.is_some() {
                self.rec.note(replay::Trick::FlipReset);
                self.lesson_tricks.push(replay::Trick::FlipReset);
            }
            if ev.pinched {
                self.rec.note(replay::Trick::Pinch);
                self.lesson_tricks.push(replay::Trick::Pinch);
            }
            if ev.dodged {
                // landing mid-dodge is a wave dash; anywhere else it is just
                // a flip — and the academy teaches them in that order
                let t = if self.car.on_ground { replay::Trick::WaveDash } else { replay::Trick::Flip };
                self.rec.note(t);
                self.lesson_tricks.push(t);
            }
            if !ev.touches.is_empty() && !self.car.on_ground {
                self.rec.note(replay::Trick::Aerial);
                self.lesson_tricks.push(replay::Trick::Aerial);
                if ball_air {
                    // a touch on a ball that is already up, while up: that is
                    // the air dribble, and the air-link timer proves it is a
                    // continuation rather than a one-off
                    if self.air_t > 0.6 {
                        self.rec.note(replay::Trick::AirDribble);
                        self.lesson_tricks.push(replay::Trick::AirDribble);
                    }
                    if self.car.pos.y - self.world.ground_h(self.car.pos.x, self.car.pos.z) > 26.0 {
                        self.rec.note(replay::Trick::Ceiling);
                    }
                    if self.ball.vel.length() > 26.0 {
                        self.rec.note(replay::Trick::Redirect);
                    }
                }
            }
            let _ = ground;
            self.rec.push(
                dt,
                replay::Frame {
                    car: self.car.pos,
                    car_rot: self.car.rot,
                    ball: self.ball.pos,
                    ball_rot: self.ball.rot,
                    boosting: self.car.boost > 0.02 && self.car.vel.length() > 6.0,
                },
            );
        }
        if ev.dodged {
            self.snd.fire(Sfx::Dodge);
            self.fx.dust(self.car.pos, self.car.up(), 3.0);
        }
        // ---- THE PULSE SHOVES EVERYTHING NEARBY ----
        // Physics can reach the player's car and the ball; the bots live out
        // here, so the blast is delivered as an event and applied to them by
        // the caller. This is the half of the design that makes the pulse
        // defensive rather than only a jump: an opponent lining up to knock
        // you off the ball gets pushed away instead.
        for &(at, r, k) in &ev.pulses {
            for b in self.bots.iter_mut() {
                let to = b.car.pos - at;
                let d = to.length();
                if d < r && d > 1e-4 {
                    let f = 1.0 - (d / r).clamp(0.0, 1.0);
                    // cars are heavier than the ball, so they move less
                    b.car.vel += (to / d) * k * f * f * 0.55;
                }
            }
            self.fx.ring_burst(at, [0.55, 0.85, 1.0, 1.0], false);
            self.cam.add_shake(0.20);
        }

        for &(gp, who) in &ev.goals {
            self.cam.add_shake(0.45);
            self.fx.ring_burst(gp, [0.45, 1.0, 0.55, 1.0], true);
            if who == physics::WHO_PLAYER {
                self.fx.explosion_of(self.look.explosion, gp);
            }
            self.fx.pad_sparkle(gp, true);
            if who == physics::WHO_PLAYER {
                // how far the shot travelled decides a couple of the names
                if self.car.pos.distance(gp) > 55.0 {
                    self.rec.note(replay::Trick::LongShot);
                }
                let (title, score) = self.rec.moment(6.0);
                self.toastf(&format!("{title}!"));
                let frames = self.rec.frames();
                if frames.len() > 20 {
                    self.moments.push(replay::Moment { title, score, frames });
                    // keep the best handful, rarest first
                    self.moments.sort_by(|a, b| b.score.cmp(&a.score));
                    self.moments.truncate(6);
                    // watch it immediately if it was a good one — the whole
                    // point is the beat after the goal
                    if let Some(best) = self.moments.iter().position(|m| m.score >= 6) {
                        if self.trial.is_none() && self.moments[best].score >= 6 {
                            self.watching = Some((best, 0.0));
                        }
                    }
                }
                self.rec.clear_log();
                self.goals += 1;
                self.save.total_goals += 1;
                self.combo = self.combo.max(1) + 2; // goals feed the combo hard
                self.combo_t = COMBO_WINDOW;
                if self.combo > self.save.best_combo {
                    self.save.best_combo = self.combo;
                }
                self.snd.fire(Sfx::Goal);
                if self.game_mode != 1 {
                    self.toastf("GOAL!");
                }
            }
            if self.game_mode == 1 {
                // sparta scoreboard: you play blue; bot teams score for
                // themselves; anything else doesn't move the board
                let team = match who {
                    physics::WHO_PLAYER => 0,
                    t if t == bots::TEAM_BLUE => 0,
                    t if t == bots::TEAM_ORANGE => 1,
                    _ => usize::MAX,
                };
                if team < 2 {
                    self.scores[team] += 1;
                    self.snd.fire(Sfx::Horn);
                    self.toastf(if team == 0 { "BLUE SCORES!" } else { "ORANGE SCORES!" });
                }
            }
        }
        if let Some(p) = ev.flip_reset {
            self.fire_at(Sfx::CarRing, p, 1.0);
            self.fx.pad_sparkle(p, false);
            self.toastf("FLIP RESET");
        }
        if let Some(sp) = ev.landed {
            self.snd.fire(Sfx::Land(sp));
            self.cam.add_shake((sp * 0.045).min(0.5));
            if sp > 2.0 {
                let n = self.car.ground_n;
                self.fx.dust(self.car.pos - n * self.car.half_h, n, sp);
                // GRIME: a hard landing kicks dust up onto the legs,
                // not the whole body -- only meaningful in robot mode
                // (sections::Body models a standing figure; a car
                // landing hard is the arcade base game's own event and
                // has nothing to do with a robot's legs). Capped low
                // per landing (0.3 at most) so dirt accumulates over
                // many landings rather than maxing out on one.
                if self.car.robot {
                    let amount = (sp * 0.03).min(0.3);
                    self.grime.add_dust(sections::Section::LegL, amount);
                    self.grime.add_dust(sections::Section::LegR, amount);
                }
            }
        }

        // WALL-CLIMB FEEDBACK, throttled the same way the C engine
        // throttles its own "CLIMB"/"MANTLE" hud message
        // (`g_climb_msg_cd`) -- `ev.climbing` fires every physics step
        // while climbing, so gating goes here rather than in physics.
        self.climb_msg_cd = (self.climb_msg_cd - dt).max(0.0);
        if ev.mantled {
            self.toastf("MANTLE");
            self.climb_msg_cd = 1.2;
        } else if ev.climbing && self.climb_msg_cd <= 0.0 {
            self.toastf("CLIMB");
            self.climb_msg_cd = 1.5;
        }

        // air-dribble timer: ball airborne, kept alive by aerial touches
        let ball_air = self.world.sdf(self.ball.pos) > self.ball.r + 0.25;
        let aerial_touch = !ev.touches.is_empty() && !self.car.on_ground && ball_air;
        if aerial_touch {
            self.air_link = 1.7;
        }
        // ---- REPLAY PLAYBACK: the game steps aside and shows you ----
        if let Some((mi, t)) = self.watching {
            let t2 = t + dt;
            let frames_len = self.moments.get(mi).map(|m| replay::length_seconds(&m.frames)).unwrap_or(0.0);
            if t2 >= frames_len {
                self.watching = None;
            } else {
                self.watching = Some((mi, t2));
                if let Some(m) = self.moments.get(mi) {
                    if let Some((eye, focus)) = replay::cinema(&m.frames, t2) {
                        self.cam.pos = eye;
                        self.cam.look = focus;
                        self.cam.up = Vec3::Y;
                    }
                }
            }
        }
        self.tick_lesson(dt);
        self.tick_aboard();
        self.tick_trial(dt);
        self.tick_patrol(dt);
        if ball_air && self.air_link > 0.0 {
            self.air_t += dt;
            self.air_link -= dt;
            if self.air_t > self.save.best_air {
                self.save.best_air = self.air_t;
            }
        } else if !ball_air {
            self.air_t = 0.0;
            self.air_link = 0.0;
        }

        let sp = self.car.vel.length();
        if sp > self.save.top_speed {
            self.save.top_speed = sp;
        }
    }

    /// On-screen controls: floating stick, BOOST/JUMP pads, drive-to marker.
    /// Normalized (0..1, y down) layout converted to NDC here.
    fn draw_touch_ui(&self, ui: &mut Vec<UiVert>, aspect: f32) {
        if !self.tpad.enabled || !matches!(self.mode, Mode::Play) {
            return;
        }
        let sc = self.cfg.touch.scale.clamp(0.6, 1.8);
        let to_ndc = |p: glam::Vec2| (p.x * 2.0 - 1.0, 1.0 - p.y * 2.0);
        let b = touch::buttons();
        let br = touch::BTN_R * 2.0 * sc;
        // BOOST
        let (bx, by) = to_ndc(b.boost);
        let hot = self.tpad.boost_down();
        hud::disc(ui, bx, by, br, aspect, if hot { [1.0, 0.72, 0.25, 0.55] } else { [1.0, 0.72, 0.25, 0.20] });
        hud::ring(ui, bx, by, br, br * 0.10, aspect, [1.0, 0.82, 0.45, 0.75]);
        hud::label(ui, bx - 0.055, by - 0.012, 0.030, 1.0, [1.0, 0.95, 0.85, 0.9], "BOOST");
        // JUMP
        let (jx, jy) = to_ndc(b.jump);
        let jhot = self.tpad.jump_down();
        hud::disc(ui, jx, jy, br * 0.86, aspect, if jhot { [0.45, 0.85, 1.0, 0.55] } else { [0.45, 0.85, 1.0, 0.20] });
        hud::ring(ui, jx, jy, br * 0.86, br * 0.09, aspect, [0.65, 0.92, 1.0, 0.75]);
        hud::label(ui, jx - 0.042, jy - 0.012, 0.030, 1.0, [0.92, 0.98, 1.0, 0.9], "JUMP");
        // floating stick
        if let Some((base, knob)) = self.tpad.stick_draw() {
            let (ox, oy) = to_ndc(base);
            let (kx, ky) = to_ndc(knob);
            hud::ring(ui, ox, oy, 0.27 * sc, 0.016, aspect, [1.0, 1.0, 1.0, 0.35]);
            hud::disc(ui, kx, ky, 0.10 * sc, aspect, [1.0, 1.0, 1.0, 0.42]);
        } else {
            // resting hint so a new player knows where to put a thumb
            let (ox, oy) = to_ndc(glam::Vec2::new(0.17, 0.80));
            hud::ring(ui, ox, oy, 0.27 * sc, 0.010, aspect, [1.0, 1.0, 1.0, 0.13]);
        }
        // drive-to marker
        if self.tpad.has_target() {
            hud::label(ui, -0.06, 0.86, 0.032, 1.0, [0.7, 1.0, 0.8, 0.8], "DRIVING TO MARK");
        }
    }

    fn draw_hud(&self, ui: &mut Vec<UiVert>, aspect: f32) {
        // a console on stage owns the screen the way a replay does — the
        // world keeps rendering underneath; the deck is glass over it
        if let Some(d) = self.deck.as_ref() {
            hud::draw_deck(ui, aspect, &d.view());
            return;
        }
        // a replay owns the screen while it plays
        if let Some((mi, t)) = self.watching {
            if let Some(m) = self.moments.get(mi) {
                hud::replay_banner(ui, aspect, &m.title, t, replay::length_seconds(&m.frames));
                return;
            }
        }
        // TRIAL BANNER: what you are attempting, the clock, and a bar that
        // fills as you get there — plus the target to beat if you have one.
        if let Some(sess) = self.lesson.as_ref() {
            let l = sess.lesson();
            let gy = self.world.ground_h(self.car.pos.x, self.car.pos.z);
            let hit = matches!(l.skill, academy::Skill::Do(t) if self.lesson_tricks.contains(&t));
            let p = academy::progress(l.skill, self.ball.vel.length(), self.car.pos.y - gy, self.combo, hit);
            hud::lesson_banner(ui, aspect, l.name, l.hint, p.max(sess.best), self.passed.has(sess.idx));
        }
        if let Some(run) = self.trial.as_ref() {
            let t = run.trial();
            let best = self.bests.get(run.idx);
            hud::trial_banner(
                ui,
                aspect,
                t.name,
                run.t,
                t.limit,
                run.progress(),
                best,
                !self.ghost.is_empty(),
            );
        }
        let online = if self.net.online() {
            Some((
                self.net.player_count(),
                if self.toast.is_empty() { self.net.status.as_str() } else { self.toast.as_str() },
            ))
        } else if !self.toast.is_empty() {
            Some((1, self.toast.as_str()))
        } else {
            None
        };
        let info = hud::HudInfo {
            aspect,
            can_flip: self.car.can_flip(),
            airborne: !self.car.on_ground,
            time: self.time,
            boost: self.car.boost,
            speed: self.car.vel.length(),
            supersonic: self.car.supersonic,
            rings: self.rings,
            car_rings: self.car_rings,
            goals: self.goals,
            combo: self.combo,
            banner_active: self.lesson.is_some() || self.trial.is_some(),
            combo_left: (self.combo_t / COMBO_WINDOW).clamp(0.0, 1.0),
            air_t: self.air_t,
            best_air: self.save.best_air,
            fps: self.fps,
            frame_drops: self.frame_drops,
            car_name: self.cars.names[self.cars.active],
            ball_cam: self.cam.ball_cam,
            // ball cam with a target in it is not ball cam any more, and
            // the readout should not claim it is
            cam_label: if self.cam.poi.is_some() && self.cam.mode == camera::CamMode::Ball {
                "TARGET CAM"
            } else {
                self.cam.mode.label()
            },
            first_person: self.cam.mode == camera::CamMode::First,
            wounded: self.loco.gait.wounded(),
            // only meaningful once transformed — vehicle mode has no
            // trigger to read out
            weapon: if self.car.robot {
                let p = self.wep.profile();
                let attuning = self.wep.watch.attuning();
                // `active()` already reports `Element::None` mid-attune
                // (that's the real flat-advantage window), but its name
                // is "none" — accurate for the advantage table, wrong
                // word for a HUD label, which should say what's actually
                // happening rather than what the fight math falls back to
                let element = if attuning { "attuning" } else { self.wep.watch.active().name() };
                Some(hud::WeaponHud {
                    name: p.name,
                    cool_frac: (self.wep.cool / p.cooldown.max(0.001)).clamp(0.0, 1.0),
                    element,
                    attuning,
                })
            } else {
                None
            },
            scores: if matches!(self.game_mode, 1 | 3..=7) { Some((self.scores[0], self.scores[1])) } else { None },
            debug_access: self.cfg.story.debug_ship,
            minimap: self.save.minimap,
            air_roll: !self.car.on_ground && self.roll_mix_s > 0.5,
            map_name: self.style().name,
            online,
        };
        let bot_dots: Vec<(Vec3, u8)> = self.bots.iter().map(|b| (b.car.pos, b.team)).collect();
        hud::hud(ui, &info, &self.world, &self.car, &self.ball, &bot_dots);
        // the sortie panel rides the left edge — brief, drop, engagement,
        // debrief — with the flow composing its own rows (`hud_rows`)
        if let Some(s) = self.sortie.as_ref() {
            hud::sortie_panel(ui, aspect, &s.hud_rows());
        }
        if let Some(r) = self.raid.as_ref() {
            let gk = self
                .binds
                .binds(Action::Grapple)
                .first()
                .map(|b| b.label())
                .unwrap_or_else(|| "GRAPPLE".to_string());
            hud::sortie_panel(ui, aspect, &r.hud_rows(&gk));
        }
        // the action bar sits over the HUD, drawn last so nothing overlaps it
        hud::draw_hotbar(ui, aspect, &self.hotbar);
    }

    /// Fire a sound that happened AT A PLACE in the world.
    ///
    /// The listener is the camera, not the car: you hear from where you are
    /// looking from, which is what makes a chase camera's audio agree with
    /// its picture. Occlusion is marched against the world SDF, so a shot
    /// on the far side of a hull arrives muffled instead of arriving clean.
    ///
    /// Falls back to a centred cue when the mixer drops the emitter (out of
    /// range, or budgeted out) — dropping the SOUND would be wrong for the
    /// one-shots that are also gameplay feedback.
    fn fire_at(&self, sfx: sound::Sfx, pos: Vec3, volume: f32) {
        let cam = self.cam.pos;
        let fwd = (self.cam.look - cam).normalize_or_zero();
        let right = fwd.cross(self.cam.up).normalize_or_zero();
        let listener = mixer::Listener {
            pos: cam.to_array(),
            forward: fwd.to_array(),
            right: right.to_array(),
        };
        let mut e = mixer::Emitter::shot(0, pos.to_array(), volume);
        // Occlusion is the expensive part (an SDF march per cue), so it is
        // only paid for cues far enough away to plausibly have something in
        // between — a bounce at your feet cannot be behind a wall.
        let d = (pos - cam).length();
        if d > 6.0 {
            let steps = (d * 0.5).clamp(8.0, 48.0) as u32;
            e.occluded_by =
                mixer::occlusion_march(cam.to_array(), pos.to_array(), steps, |p| {
                    self.world.sdf(Vec3::from_array(p))
                });
        }
        match mixer::mix(&listener, &[e], 1).first() {
            Some(v) => self.snd.fire_cue(sound::Cue::at(sfx, v)),
            None => self.snd.fire(sfx),
        }
    }

    /// Floating name tags over remote cars.
    fn peer_tags(&self, ui: &mut Vec<UiVert>, vp: Mat4, aspect: f32) {

        if !self.net.online() {
            return;
        }
        for p in self.net.peers() {
            let world_pos = p.shown + Vec3::Y * 1.5;
            let h = vp * Vec4::new(world_pos.x, world_pos.y, world_pos.z, 1.0);
            if h.w <= 0.05 {
                continue;
            }
            let x = h.x / h.w;
            let y = h.y / h.w;
            if x.abs() > 1.05 || y.abs() > 1.05 {
                continue;
            }
            let px = (0.018 / h.w.max(2.0) * 12.0).clamp(0.008, 0.016);
            let w = font::width(&p.name, px, aspect);
            font::text(ui, x - w * 0.5, y, px, aspect, [1.0, 0.98, 0.94, 0.9], &p.name);
        }
    }

    /// The per-frame ray-marchable shape list: every world shape plus dynamic
    /// proxies (ball, cars, live pad orbs) so shadows and reflections include
    /// the moving things. Returns which slot is whose, so raster surfaces can
    /// exclude themselves from their own reflection rays.
    /// A car's reflection tint: procedural cars use their real body color,
    /// the GLTF trio use their signature liveries (their raster color lives
    /// in textures the tracer can't sample).
    fn car_tint(&self, model_i: usize) -> [f32; 4] {
        match model_i {
            0 => [0.80, 0.34, 0.10, 1.0], // octane orange
            1 => [0.72, 0.73, 0.76, 1.0], // fennec silver
            2 => [0.20, 0.20, 0.24, 1.0], // dominus gunmetal
            i => {
                let c = self.cars.models[i].prims.first().map(|p| p.color).unwrap_or([0.4; 4]);
                [c[0], c[1], c[2], 1.0]
            }
        }
    }

    /// The five-piece reflection silhouette of a car, in its true axes:
    /// half extents ordered (width X, height Y, length Z). `full` adds the
    /// four wheels (local car only, to spare the shape budget).
    #[allow(clippy::too_many_arguments)]
    fn push_car_proxy(
        &self,
        v: &mut Vec<RtShapeRaw>,
        m: &model::Model,
        tint: [f32; 4],
        pos: Vec3,
        rot: Quat,
        full: bool,
        push: &mut dyn FnMut(&mut Vec<RtShapeRaw>, f32, Vec3, f32, [f32; 4], Quat, [f32; 4], f32, f32),
    ) {
        let iq = rot.inverse();
        let bound = m.half_len + 0.25;
        // low body: width, 55% height, 88% length (the nose piece finishes it)
        let body = [m.half_w, m.half_h * 0.55, m.half_len * 0.88, 0.08];
        push(v, 3.0, pos - rot * Vec3::Y * (m.half_h * 0.35) + rot * Vec3::Z * (m.half_len * 0.06), bound, body, iq, tint, 0.0, 0.6);
        // cabin: narrower, seated back of center
        let cab = [m.half_w * 0.78, m.half_h * 0.50, m.half_len * 0.40, 0.06];
        push(v, 3.0, pos + rot * (Vec3::Y * (m.half_h * 0.50) + Vec3::Z * (m.half_len * 0.12)), bound, cab, iq, [0.10, 0.11, 0.13, 1.0], 0.0, 0.35);
        // tapered nose: thin low wedge toward -Z (forward)
        let nose = [m.half_w * 0.88, m.half_h * 0.28, m.half_len * 0.22, 0.05];
        push(v, 3.0, pos + rot * (Vec3::NEG_Z * (m.half_len * 0.80) - Vec3::Y * (m.half_h * 0.55)), bound, nose, iq, tint, 0.0, 0.55);
        if full {
            for sz in [-1.0f32, 1.0] {
                for sx in [-1.0f32, 1.0] {
                    let wp = pos + rot * Vec3::new(sx * m.half_w * 0.98, -m.half_h * 0.45, sz * m.half_len * 0.62);
                    push(v, 0.0, wp, m.wheel_r + 0.03, [m.wheel_r, 0.0, 0.0, 0.0], Quat::IDENTITY, [0.06, 0.06, 0.07, 1.0], 0.0, 0.2);
                }
            }
        }
    }

    fn build_rt_shapes(&self, rc: (Vec3, Quat), rb: (Vec3, Quat)) -> (Vec<RtShapeRaw>, RtIds) {
        use world::Sdf;
        let mut v: Vec<RtShapeRaw> = Vec::with_capacity(self.world.shapes.len() + 32);
        let q4 = |q: Quat| [q.x, q.y, q.z, q.w];
        let mut push = |v: &mut Vec<RtShapeRaw>,
                        kind: f32,
                        pos: Vec3,
                        br: f32,
                        b: [f32; 4],
                        inv: Quat,
                        col: [f32; 4],
                        em: f32,
                        spec: f32| {
            v.push(RtShapeRaw {
                a: [pos.x, pos.y, pos.z, br],
                b,
                c: q4(inv),
                d: [col[0], col[1], col[2], em],
                e: [spec, kind, 0.0, 0.0],
            });
        };

        for (wi, s) in self.world.shapes.iter().enumerate() {
            if s.cut {
                continue; // CSG cutters carve the SDF; they are not drawn
            }
            if self.world.is_mover(wi) {
                continue; // appended below, after static_n
            }
            // statics come first in STABLE shape order: the baked candidate
            // cells index into this prefix, per-frame culls would shift it
            let em = (s.emissive + s.flash * 1.1).min(1.0);
            let kb = if s.blend { 16.0 } else { 0.0 }; // kind+16 = fillet group
            match s.sdf {
                Sdf::Sphere { r } => push(&mut v, kb, s.pos, r + 0.1, [r, 0.0, 0.0, 0.0], s.inv_rot, s.color, em, s.spec),
                Sdf::Torus { big, tube } => push(&mut v, 1.0 + kb, s.pos, big + tube + 0.1, [big, tube, 0.0, 0.0], s.inv_rot, s.color, em, s.spec),
                Sdf::Shell { r, half, thick } => push(
                    &mut v,
                    5.0 + kb,
                    s.pos,
                    ((r + thick) * (r + thick) + half * half).sqrt() + 0.1,
                    [r, half, thick, 0.0],
                    s.inv_rot,
                    s.color,
                    em,
                    s.spec,
                ),
                Sdf::TubeTorus { big, tube, thick } => push(
                    &mut v,
                    9.0 + kb,
                    s.pos,
                    big + tube + thick + 0.1,
                    [big, tube, thick, 0.0],
                    s.inv_rot,
                    s.color,
                    em,
                    s.spec,
                ),
                Sdf::RingDeck { big, wide, half_h } => push(
                    &mut v,
                    10.0 + kb,
                    s.pos,
                    big + wide + 0.2,
                    [big, wide, half_h, 0.0],
                    s.inv_rot,
                    s.color,
                    em,
                    s.spec,
                ),
                Sdf::Hex { r, h } => push(&mut v, 6.0 + kb, s.pos, (r * r + h * h).sqrt() + 0.1, [r, h, 0.0, 0.0], s.inv_rot, s.color, em, s.spec),
                Sdf::Capsule { r, half } => push(&mut v, 2.0, s.pos, half + r + 0.1, [r, half, 0.0, 0.0], s.inv_rot, s.color, em, s.spec),
                Sdf::RBox { half, round } => push(&mut v, 3.0, s.pos, half.length() + 0.1, [half.x, half.y, half.z, round], s.inv_rot, s.color, em, s.spec),
                Sdf::None => {}
            }
        }
        let static_n = v.len();
        // ---- MOVERS, as dynamics ----
        // A platform baked into the static grid reflects wherever it was
        // when the map loaded, which on CHAMBER is 190 chess parts sitting
        // in the wrong squares. They go in the dynamic suffix instead, which
        // is scanned live — capped and distance-sorted, because that scan is
        // per-fragment and CHAMBER would otherwise put 190 spheres in it.
        {
            let mut movers: Vec<(f32, usize)> = self
                .world
                .shapes
                .iter()
                .enumerate()
                .filter(|(wi, sh)| !sh.cut && self.world.is_mover(*wi) && !matches!(sh.sdf, Sdf::None))
                .map(|(wi, sh)| (sh.pos.distance_squared(rc.0), wi))
                .filter(|(d2, _)| *d2 < 190.0 * 190.0)
                .collect();
            movers.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (_, wi) in movers.into_iter().take(28) {
                let s = &self.world.shapes[wi];
                let em = (s.emissive + s.flash * 1.1).min(1.0);
                let kb = if s.blend { 16.0 } else { 0.0 };
                match s.sdf {
                    Sdf::Sphere { r } => push(&mut v, kb, s.pos, r + 0.1, [r, 0.0, 0.0, 0.0], s.inv_rot, s.color, em, s.spec),
                    Sdf::Torus { big, tube } => push(&mut v, 1.0 + kb, s.pos, big + tube + 0.1, [big, tube, 0.0, 0.0], s.inv_rot, s.color, em, s.spec),
                    Sdf::Capsule { r, half } => push(&mut v, 2.0 + kb, s.pos, r + half + 0.1, [r, half, 0.0, 0.0], s.inv_rot, s.color, em, s.spec),
                    Sdf::RBox { half, round } => push(&mut v, 3.0 + kb, s.pos, half.length() + 0.1, [half.x, half.y, half.z, round], s.inv_rot, s.color, em, s.spec),
                    Sdf::Hex { r, h } => push(&mut v, 6.0 + kb, s.pos, (r * r + h * h).sqrt() + 0.1, [r, h, 0.0, 0.0], s.inv_rot, s.color, em, s.spec),
                    Sdf::Shell { r, half, thick } => push(&mut v, 5.0 + kb, s.pos, ((r + thick) * (r + thick) + half * half).sqrt() + 0.1, [r, half, thick, 0.0], s.inv_rot, s.color, em, s.spec),
                    Sdf::TubeTorus { big, tube, thick } => push(&mut v, 9.0 + kb, s.pos, big + tube + thick + 0.1, [big, tube, thick, 0.0], s.inv_rot, s.color, em, s.spec),
                    Sdf::RingDeck { big, wide, half_h } => push(&mut v, 10.0 + kb, s.pos, big + wide + 0.2, [big, wide, half_h, 0.0], s.inv_rot, s.color, em, s.spec),
                    Sdf::None => {}
                }
            }
        }


        // live pad orbs, gold and glowing (bob matches build_scene)
        for p in self.world.pads.iter().filter(|p| p.timer <= 0.0) {
            let bob = (self.time * 2.1 + p.phase).sin() * 0.09;
            let r = if p.big { 0.42 } else { 0.2 };
            let pos = p.pos + Vec3::Y * (if p.big { 1.05 } else { 0.55 } + bob);
            push(&mut v, 0.0, pos, r + 0.05, [r, 0.0, 0.0, 0.0], Quat::IDENTITY, world::GOLD, 0.95, 0.6);
        }

        // the ball: exact sphere, kind 4 = panel-banded in reflections, with
        // its live rotation so mirrored floors show it actually spinning
        let ball_id = v.len();
        // SPHERE kind (4 was falling through to a degenerate rounded box —
        // the "long pole"); the panel-band identity rides in e.w instead
        push(&mut v, 0.0, rb.0, self.ball.r + 0.05,
            [self.ball.r, 0.0, 0.0, 0.0], rb.1.inverse(),
            [0.93, 0.93, 0.96, 1.0], 0.03, 0.55);
        v[ball_id].e[3] = 4.0;

        // cars as rounded-box proxies
        let car_box = |m: &model::Model| -> ([f32; 4], f32) {
            let half = Vec3::new(m.half_len * 0.55, m.half_h, m.half_len);
            ([half.x, half.y, half.z, 0.08], half.length() + 0.15)
        };
        // the local car: a five-piece silhouette in the car's REAL axis
        // convention (length along local Z, width along X — v1.11 had these
        // swapped, which read as the reflection turned 90 degrees) so wet
        // floors reflect the actual car shape: low body, cabin, tapered
        // nose, four wheels.
        let car_id = v.len();
        {
            let m = &self.cars.models[self.cars.active];
            let tint = self.car_tint(self.cars.active);
            if !m.cloud.is_empty() {
                // EXACT model: march the baked SDF atlas slab
                let sz = self.cfg.car.size.max(0.05);
                let h = m.sdf_half * sz;
                push(&mut v, 8.0, rc.0, h.length() + 0.25, [h.x, h.y, h.z, self.cars.active.min(11) as f32], rc.1.inverse(), tint, 0.0, 0.55);
                v[car_id].e[2] = sz;
            } else {
                self.push_car_proxy(&mut v, m, tint, rc.0, rc.1, true, &mut push);
            }
        }
        let decor0 = v.len();
        for m in &self.world.decor_cars {
            let (_, rot, tr) = m.to_scale_rotation_translation();
            let (b, br) = car_box(&self.decor_model);
            push(&mut v, 3.0, tr, br, b, rot.inverse(), [0.52, 0.09, 0.09, 1.0], 0.0, 0.5);
        }
        let remote0 = v.len();
        for p in self.net.peers() {
            let mi = p.car.min(self.cars.models.len() - 1);
            let m = &self.cars.models[mi];
            if !m.cloud.is_empty() {
                let h = m.sdf_half;
                let pi = v.len();
                push(&mut v, 8.0, p.shown, h.length() + 0.25, [h.x, h.y, h.z, mi.min(11) as f32], p.rot.inverse(), self.car_tint(mi), 0.0, 0.55);
                v[pi].e[2] = 1.0;
            } else {
                self.push_car_proxy(&mut v, m, self.car_tint(mi), p.shown, p.rot, false, &mut push);
            }
        }

        // the twenty nearest bots cast real shadows; the rest live in the fog
        if !self.bots.is_empty() {
            let cam = self.cam.pos;
            let mut order: Vec<(f32, usize)> = self
                .bots
                .iter()
                .enumerate()
                .map(|(i, b)| (b.car.pos.distance_squared(cam), i))
                .collect();
            order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            for &(_, i) in order.iter().take(20) {
                let b = &self.bots[i];
                let m = &self.cars.models[b.model];
                let t = b.tint;
                self.push_car_proxy(&mut v, m, [t[0] * 0.6, t[1] * 0.6, t[2] * 0.6, 1.0], b.car.pos, b.car.rot, false, &mut push);
            }
        }

        v.truncate(render::MAX_RT_SHAPES);
        (v, RtIds { ball: ball_id, car: car_id, decor0, remote0, static_n })
    }

    /// The console in the room the player is standing in, if any.
    ///
    /// The join `aboard.rs` was built for: the world knows where the ship is
    /// parked, `interior::room_at` turns a position into a room, and
    /// `Station::cell()` has named each console's room since the stations
    /// landed.
    /// Consoles the player could walk to from where they are, through doors
    /// that open.
    ///
    /// Falls back to every station when the player is not aboard — being
    /// outside the ship should not silently empty the console list, and the
    /// deck is reachable from the surface today.
    fn consoles_reachable_now(&self) -> Vec<stations::Station> {
        let Some((origin, ship)) = self.world.parked.as_ref() else {
            return stations::STATIONS.to_vec();
        };
        let Some(room) = ship.room_at(self.car.pos - *origin) else {
            return stations::STATIONS.to_vec();
        };
        let open = ship.reachable_from(room.id);
        let v: Vec<stations::Station> =
            stations::STATIONS.into_iter().filter(|s| open.contains(&s.cell())).collect();
        if v.is_empty() {
            stations::STATIONS.to_vec()
        } else {
            v
        }
    }

    /// Notice when the player walks into or out of a room.
    ///
    /// An EVENT, not a state: the room is a fact every frame, and a HUD line
    /// that reprints it sixty times a second is noise. Only the transition is
    /// worth saying, and the useful thing to say is what you can do here.
    /// Put the ship back in the world, wherever the world came from.
    ///
    /// The single place that parks. `World::generate` returns
    /// `parked: None`, so every path that builds a world has to call this or
    /// the hull keeps drawing and stops being solid.
    /// Fly the ship somewhere and load what is there.
    ///
    /// The starmap could plot a course and the world never changed, so
    /// "arriving" was a line of text. This is the rest of it: pick the style
    /// that matches the destination's physics, load it, and close the console
    /// — you are not at the chartroom any more, you are on Mars.
    fn depart_to(&mut self, dest: &'static str, line: &str) {
        let Some(d) = patrol::ROUTE.iter().find(|x| x.id == dest) else {
            self.toastf("no such destination");
            return;
        };
        // SYNC PATROL TO WHERE YOU ACTUALLY ARE. `Patrol::bridge_to`'s own
        // doc: "a jump is a change of world, not a fast-travel within
        // one" -- exactly this call site, which already treats arrival
        // the same way (fresh map, fresh medium).
        self.patrol.bridge_to(dest);
        let style = mapsim::style_for(d);
        self.load_map(style, dest_seed(d.id));
        self.set_medium_for(d);
        // the console belongs to the ship you just left
        self.close_deck();
        // `d.note` — "what the archive says about it" per Destination's own
        // doc comment, "surfaced on arrival" — had no consumer anywhere in
        // the codebase before this. `line` is the landing-quality readout
        // (gravity, miss distance, impact speed); this is the one place
        // both a mechanical result and the place itself are known at once.
        let line = format!("{line} — {}", d.note);
        let line = line.as_str();
        // YARD TIME: berthing at a dock squares the hull away — the one
        // release for the wear that LZ-pinned sorties accumulate
        // (`stations::ship_wear`), so the loop closes: fight from open
        // ground, fly dirtier passes, put in at a station to fix it.
        if stations::is_dock(d) {
            let was = stations::clear_ship_wear();
            if was > 0.05 {
                self.toastf(&format!("{line} · yard time: hull wear {was:.1} squared away"));
                return;
            }
        }
        self.toastf(line);
    }

    /// The medium the shots fly through at this destination, and its
    /// gravity. One derivation for every way of arriving somewhere —
    /// airless worlds get the vacuum their ballistics deserve.
    fn set_medium_for(&mut self, d: &'static patrol::Destination) {
        let g = mapsim::ground_for(d);
        self.ambient = if g.air <= 0.001 {
            ballistics::Ambient::vacuum()
        } else {
            ballistics::Ambient::earthlike()
        };
        self.world_g = g.g.max(0.01);
    }

    /// Accept a sortie order and open the flow: load the destination's
    /// world — the SAME style and seed a Depart would use, so the mission
    /// happens at the place the starmap flies you to and not a parallel
    /// copy of it — then hold at the brief until the player launches.
    fn begin_sortie(&mut self, mission_id: &'static str, week: Option<u64>) {
        match sortie::Sortie::begin(mission_id, week) {
            Ok(s) => {
                let d = s.dest;
                self.load_map(mapsim::style_for(d), dest_seed(d.id));
                self.set_medium_for(d);
                self.close_deck();
                // `content::Commander.desc` — real, hand-written doctrine
                // flavour ("presses relentlessly; concentrates on the
                // player and rarely yields ground") — had no consumer
                // anywhere in the codebase. `campaign::foe_doctrine(mission)`
                // already picks which of the five doctrines actually drives
                // this mission's opposition (`skirmish::Squad::new` reads
                // it too), so this is real information about the fight the
                // player is about to walk into, not decoration.
                let doctrine = campaign::foe_doctrine(s.mission);
                let foe = content::commander(doctrine).map(|c| c.desc).unwrap_or("");
                // `content::Film.title`/`.subtitle` — every film (story
                // chapter) has both, real authored names, and neither had a
                // UI consumer: campaign.rs tracks which film a mission
                // belongs to for progression logic, but nothing ever
                // announced a chapter to the player. Surfaced once, on that
                // film's OPENING mission only (checked via
                // `content::missions_of`, not a new stored flag) rather than
                // on every mission of it, which would just be repetitive
                // chrome.
                let is_films_first_mission = content::missions_of(s.mission.film)
                    .next()
                    .is_some_and(|first| first.id == s.mission.id);
                let chapter = if is_films_first_mission {
                    content::film(s.mission.film).map(|f| format!("{} — {} — ", f.title, f.subtitle)).unwrap_or_default()
                } else {
                    String::new()
                };
                self.toastf(&format!("{chapter}sortie accepted — {} over {} — {foe}", s.mission.id, d.name));
                self.raid = None; // one flight at a time
                let mut s = s;
                s.set_ride_h(self.car.ride_h.max(self.car.half_h));
                self.sortie = Some(s);
            }
            Err(e) => self.toastf(e),
        }
    }

    /// Accept the Wardeck's raid order: brief phase one and load its
    /// arena's world. The dropship pass launches from the brief, same as
    /// a sortie — a raid just does it once per phase.
    fn begin_raid(&mut self, boss_id: &'static str) {
        match raidflow::RaidFlow::begin(boss_id) {
            Ok(r) => {
                let a = r.arena();
                self.arena_map(a);
                self.close_deck();
                // arena.note is real, authored lore (10 arenas, one line
                // each — e.g. "the horizon curves the wrong way; every SYNC
                // ends here") that had zero UI consumer anywhere in the
                // codebase before this: constructed once in raidcon.rs and
                // never read again. This is the moment a player is dropped
                // into that arena, so it's the natural place for it.
                self.toastf(&format!("the signal is answered — {} — {}", r.boss.name, a.note));
                self.sortie = None; // one flight at a time
                self.raid = Some(r);
            }
            Err(e) => self.toastf(e),
        }
    }

    /// Fly a forged echo from the Wardeck's browser: same flow, same
    /// arenas, banks nothing.
    fn begin_echo(&mut self, id: &str) {
        let Some(e) = raidforge::find(id) else {
            self.toastf("that echo has faded from the file");
            return;
        };
        // WHO THE ECHO IS ALLOWED TO CARRY. The model may have asked for
        // anyone; `casting::admit` checks each name against this save's own
        // campaign, so the same forged raid can legally cast Cleave for a
        // player mid-Film-3 and cast nobody for one who has seen the plates
        // close. Parsing deliberately kept the request unvalidated because it
        // has no idea which save it is for.
        let asked: Vec<&'static str> = e
            .cast_request
            .iter()
            .filter_map(|w| casting::entry(w).map(|c| c.id))
            .collect();
        let cast = self
            .deck
            .as_ref()
            .map(|d| casting::admit(&asked, d.campaign()))
            .unwrap_or_default();
        match raidflow::RaidFlow::begin_echo(e.to_plan()) {
            Ok(r) => {
                let a = r.arena();
                self.arena_map(a);
                self.close_deck();
                match cast.first() {
                    // BILLED, not named. A player who has not met this
                    // character sees `???` — true, rather than a stand-in
                    // that a friend on voice chat contradicts in one
                    // sentence. The spoiler decision belongs to the table.
                    Some((id, p)) => {
                        let shown = self
                            .deck
                            .as_ref()
                            .and_then(|d| casting::billing(id, d.campaign()))
                            .map(|b| b.label)
                            .unwrap_or(casting::REDACTED);
                        self.toastf(&format!(
                            "the echo answers — {} — {shown} is {}",
                            r.boss.name,
                            p.label()
                        ))
                    }
                    None => self.toastf(&format!("the echo answers — {}", r.boss.name)),
                }
                self.sortie = None;
                self.raid = Some(r);
            }
            Err(e) => self.toastf(e),
        }
    }

    /// Load the world for an arena. Arenas that hang off a patrol
    /// destination (`via`) land on that destination's OWN style and seed —
    /// the raid happens at the place the starmap knows. Orphan arenas get
    /// a hashed style: the same known-wrong-but-cheap mapping `style_for`
    /// already owns, one shelf over.
    fn arena_map(&mut self, a: &'static raidcon::Arena) {
        if let Some(via) = a.via {
            if let Some(d) = patrol::ROUTE.iter().find(|x| x.id == via) {
                self.load_map(mapsim::style_for(d), dest_seed(d.id));
                self.set_medium_for(d);
                return;
            }
        }
        let h = raidgen::strhash(a.id);
        self.load_map(h as usize % world::STYLE_COUNT, h);
        // an orphan arena breathes: earthlike until arenas carry a sky
        self.ambient = ballistics::Ambient::earthlike();
        self.world_g = 9.81 * 0.8;
    }

    /// Step through the hull classes (`aboard::FLEET`). A ship is one
    /// number today — the hull length the eight rooms are laid out in — so
    /// cycling is regenerate, repark, re-upload, and put the player
    /// OUTSIDE: rebuilding the hull around somebody standing in it would
    /// leave them inside solid structure, which the seating guard would
    /// then shove somewhere arbitrary.
    fn cycle_ship(&mut self, dir: i32) {
        let n = aboard::FLEET.len() as i32;
        self.ship_class = (((self.ship_class as i32 + dir) % n + n) % n) as usize;
        let (name, heights, note) = aboard::FLEET[self.ship_class];
        let body_h = self.car.half_h * 2.0;
        self.aboard.ship = aboard::fleet_ship(self.ship_class, body_h.max(0.5));
        self.aboard.ship.unseal(); // a ship at rest has its doors open
        self.repark();
        self.aboard.hull_slot = None; // force a fresh upload at the new size
        self.aboard.cells_slot = None;
        self.upload_ship_hull();
        // RELOAD THE EXTERIOR MODEL AT THE NEW SIZE. Without this the swap
        // is invisible: `load_exterior` bakes `hull_half.z * 2.0` in at
        // load time and the model is drawn INSTEAD of the procedural skin,
        // so the hull, the rooms and the collision would all resize behind
        // an exterior still sitting at whatever size it was first loaded
        // at. That is a swap that changes everything except the thing you
        // are looking at — which is indistinguishable from the key not
        // working, and is what a playtest reported.
        let bots_dir = assets_dir().join("bots");
        self.aboard.load_exterior(&mut self.gpu, &bots_dir, aboard::EXTERIOR_MODEL);
        self.step_outside();
        // THE RAMP IS PART OF THE SWAP. `boarding::RampSpec`/`RampMount`
        // derive from the hull that just changed, so a ship swap that
        // did not re-report them would leave the boarding geometry
        // silently stale on screen and in the toast.
        let ramp = boarding::RampSpec::for_hull_len(
            self.aboard.ship.hull_half.z * 2.0,
            self.aboard.ship.occupant_h,
        );
        let mount = boarding::RampMount::place(
            &ramp,
            self.aboard.ship.profile(),
            self.aboard.ship.deck_y,
        );
        self.toastf(&format!(
            "{name} — {:.0} m hull ({:.0} heights): {note}  |  ramp {:.1} m x {:.1} m, {:.0}°{}",
            self.aboard.ship.hull_half.z * 2.0,
            heights,
            ramp.width_m,
            mount.slope_len(),
            ramp.grade_deg,
            if mount.fits { "" } else { "  (STERN TOO NARROW)" }
        ));
    }

    /// Put the player down clear of the parked hull, on the ground, facing
    /// it. Used when the ship changes shape underneath them.
    fn step_outside(&mut self) {
        let Some((origin, ship)) = self.world.parked.as_ref() else { return };
        let (origin, half) = (*origin, ship.hull_half);
        let out = origin + Vec3::new(half.x + 6.0 + self.car.half_len * 2.0, 0.0, 0.0);
        let gy = self.world.ground_h(out.x, out.z);
        self.car.pos = Vec3::new(out.x, gy + self.car.stand_h() + 0.2, out.z);
        self.car.vel = Vec3::ZERO;
        // look back at what you just changed
        let to = (origin - self.car.pos) * Vec3::new(1.0, 0.0, 1.0);
        self.car.rot = Quat::from_rotation_arc(Vec3::NEG_Z, to.normalize_or(Vec3::NEG_Z));
        self.last_room = None;
        self.cam.indoors = None;
    }

    /// Upload (or replace) the parked hull's mesh. One path, so a ship
    /// that changed shape cannot leave a stale silhouette on screen.
    fn upload_ship_hull(&mut self) {
        self.aboard.upload_hull(&mut self.gpu);
    }

    fn repark(&mut self) {
        let at = Vec3::new(0.0, aboard::ground_lift(&self.aboard.ship), 0.0);
        self.world.park(at, self.aboard.ship.clone());
        self.last_room = None;
        self.cam.indoors = None;
    }

    /// WIRING DEBT (docs/PLAN.md Phase 0: "patrol -- a game mode that
    /// calls update"). `patrol::Patrol::update` returns both an ambient/
    /// lore text drip AND a Surge/Hold/Onslaught combat-event state
    /// machine in one `Tick` -- this consumes ONLY the text half.
    ///
    /// The event half is deliberately NOT surfaced: `out.started` would
    /// mean announcing "SURGE incoming" with nothing actually spawning
    /// to fight (this codebase's bot-spawn system isn't hooked up to
    /// patrol's event position/quota), which reads as a broken promise
    /// rather than a missing feature. The event state machine still
    /// runs in the background (there's no way to call update() and get
    /// only half its side effects) and will quietly time out and fail
    /// on its own clock -- harmless, since nothing ever announced it in
    /// the first place. Wiring the event half for real needs a bot
    /// spawned at `Event::at` with a kill-credit hook into
    /// `Patrol::credit_kill`, which is its own scoped follow-up, not
    /// something to bolt on blind here.
    fn tick_patrol(&mut self, dt: f32) {
        if self.mode != Mode::Play {
            return; // a menu/console shouldn't advance the world's own clock
        }
        // no day/night cycle exists yet in this engine (checked: no
        // caller anywhere computes one) -- 0.0 (always day) is an
        // honest stand-in, not a guess at what night should look like
        let night = 0.0;
        let player = [self.car.pos.x, self.car.pos.z];
        let tick = self.patrol.update(dt, player, night);
        if let Some(line) = tick.ambient {
            self.toastf(line);
        } else if let Some(line) = tick.lore {
            self.toastf(line);
        }
    }

    fn tick_aboard(&mut self) {
        // THE DOORS HAVE NEVER MOVED. `step_doors` exists, `Door::update`
        // animates travel, `passable()` reads it — and nothing ticked the
        // clock, so every door in the game has been frozen at whatever state
        // it was generated in. The ship being scenery is why nobody noticed.
        let dt = 1.0 / 60.0;
        let at = self.car.pos;
        let mut doors_now = 0u32;
        if let Some((origin, ship)) = self.world.parked.as_mut() {
            // doors notice you before they move, so the sense comes first
            ship.sense_doors(at - *origin);
            ship.step_doors(dt);
            for (i, l) in ship.links.iter().enumerate().take(32) {
                // `passable()`, matching what `hull_mesh` now draws on. This
                // read `void()` — so the mask flipped at travel 0.01 while
                // the mesh changes at 0.75, and the hull was re-uploaded on
                // frames where nothing about it had changed. Harmless and
                // wasteful, and it is the same two-answers-to-one-question
                // mistake `--parity` exists to find. The scan caught this one
                // in my own code, written an hour after I wrote the scan.
                if l.door.passable() {
                    doors_now |= 1 << i;
                }
            }
        }
        // RE-UPLOAD WHEN THE DOORWAYS CHANGE, and only then. `hull_mesh`
        // omits shut corridors, so a door opening changes the geometry — and
        // a mesh uploaded once at launch would show walls where the collision
        // has a corridor. A bitmask of which doorways are carved is the
        // cheapest thing that notices.
        if doors_now != self.door_mask {
            self.door_mask = doors_now;
            if let (Some(cells_slot), Some((_, ship))) =
                (self.aboard.cells_slot, self.world.parked.as_ref())
            {
                let m = aboard::hull_cells_mesh(ship);
                self.gpu.replace_mesh(cells_slot, &m);
            }
            // the hull SKIN doesn't change shape when a door opens (only
            // the cell/link boundary boxes do — the outer silhouette is
            // fixed by `hull_half`), so it isn't re-uploaded here.
        }

        // and the camera: pull the boom in while the player is inside, or it
        // sits outside the hull looking at the back of a wall
        let inside = self
            .world
            .parked
            .as_ref()
            .map(|(o, ship)| (*o, ship.room_at(self.car.pos - *o).is_some()))
            .unwrap_or((Vec3::ZERO, false));
        if inside.1 {
            if self.cam.indoors.is_none() {
                if let Some((origin, ship)) = self.world.parked.as_ref() {
                    let o = *origin;
                    let probe = ship.clone();
                    self.cam.indoors =
                        Some(Box::new(move |p: Vec3| probe.walkable(p - o)));
                }
            }
        } else if self.cam.indoors.is_some() {
            self.cam.indoors = None;
        }

        let now = self
            .world
            .parked
            .as_ref()
            .and_then(|(o, ship)| ship.room_at(self.car.pos - *o))
            .map(|c| c.id);
        if now == self.last_room {
            return;
        }
        self.last_room = now;
        let Some(id) = now else { return };
        let name = self
            .world
            .parked
            .as_ref()
            .and_then(|(_, s)| s.cell(id))
            .map(|c| c.name)
            .unwrap_or(id);
        match stations::STATIONS.into_iter().find(|s| s.cell() == id) {
            Some(st) => self.toastf(&format!("{} — press V for {}", name, st.title())),
            None => self.toastf(name),
        }
    }

    fn console_underfoot(&self) -> Option<stations::Station> {
        let (origin, ship) = self.world.parked.as_ref()?;
        let local = self.car.pos - *origin;
        let room = ship.room_at(local)?;
        stations::STATIONS.into_iter().find(|s| s.cell() == room.id)
    }

    fn build_scene(&self, ids: &RtIds, rc: (Vec3, Quat), rb: (Vec3, Quat), planes: &[[f32; 4]; 6]) -> Vec<ObjDraw> {
        let mut objs: Vec<ObjDraw> = Vec::with_capacity(self.world.shapes.len() + 1024);

        let push = |objs: &mut Vec<ObjDraw>,
                    mesh: usize,
                    tex: Option<usize>,
                    model: Mat4,
                    color: [f32; 4],
                    params: [f32; 4]| {
            let nrm3 = Mat3::from_mat4(model).inverse().transpose();
            let mut raw = ObjectRaw::default();
            raw.model = model.to_cols_array();
            raw.nrm = Mat4::from_mat3(nrm3).to_cols_array();
            raw.color = color;
            raw.params = params;
            objs.push(ObjDraw { mesh, tex, raw });
        };

        // terrain, chunk by chunk — the ground is 97k triangles and used to
        // be a single mesh, so all of it drew every frame regardless of
        // where you were looking
        for (ci, (c, r)) in self.tchunks.iter().enumerate() {
            if !sphere_visible(planes, *c, *r) {
                continue;
            }
            push(
                &mut objs,
                world::M_TERRAIN_0 + ci,
                None,
                Mat4::IDENTITY,
                self.style().grass,
                [1.0, 0.0, 0.04, 0.0],
            );
        }

        // THE SHIP. Parked at the origin, lifted clear of the deck so it sits
        // on the ground rather than half-buried. One draw, ~1000 triangles.
        //
        // Same shape as the terrain chunks above: a mesh slot, an identity-ish
        // transform, a colour. Placed here rather than behind a mode because
        // the fastest way to find out whether it looks right is to be able to
        // drive up to it.
        // THE PARKED SHIP IS THE ONE THAT MOVES. `aboard.ship` is the
        // template it was cloned from and its doors never animate, so drawing
        // from it would show a hull that disagrees with the one you collide
        // with — the exact "see it here, hit it there" failure the lift
        // shares a function to avoid.
        //
        // TWO DRAW CALLS, TWO COLOURS, not one merged mesh in one flat
        // colour. Before this split the outer hull skin and the internal
        // room/corridor boundary boxes shared a single push() and a
        // single untextured colour — indistinguishable from "a cluster of
        // grey boxes," which is exactly what it read as. A darker,
        // slightly blued structural tone for the skin and a warmer,
        // lighter one for the room framing gives a viewer at least two
        // things to tell apart; real materials/texturing are a further
        // step this doesn't attempt.
        if let Some(m) = self.aboard.exterior.as_ref() {
            // A REAL MODEL, drawn prim-by-prim with its own real textures
            // and colours, in place of the procedural skin push below —
            // this is the whole point of `Aboard::exterior`: something
            // that reads as a ship rather than as a shape. Collision is
            // still the procedural hull box (see `exterior`'s own doc
            // comment), so this loop only ever runs if the model loaded;
            // on any failure `exterior` stays `None` and the procedural
            // skin (further down) draws instead, so there is always
            // SOMETHING there.
            if let Some((origin, _)) = self.world.parked.as_ref() {
                let base = Mat4::from_translation(*origin);
                for prim in &m.prims {
                    push(&mut objs, prim.mesh_id, prim.tex_id, base, prim.color, [0.0, 0.0, 0.0, 0.0]);
                }
            }
        } else if let (Some(skin_slot), Some((origin, _))) =
            (self.aboard.hull_slot, self.world.parked.as_ref())
        {
            // FALLBACK: the procedural loft, only when no real model is
            // loaded. Same two-colour split as before this pass — see
            // the comment above `cells_slot`'s own draw call below.
            let lift = Mat4::from_translation(*origin);
            push(
                &mut objs,
                skin_slot,
                None,
                lift,
                [0.30, 0.33, 0.38, 1.0],
                [0.35, 0.0, 0.25, 0.0],
            );
        }
        // THE WHITE BOXES. The room/corridor framing was drawn whether or
        // not a real exterior model had loaded, so the playtest saw a ship
        // model with light-grey boxes sticking out of it — the interior,
        // rendered from outside, through a hull that is not there in the
        // draw list. With a real exterior the framing is only drawn when
        // you are ABOARD (inside, where it is the walls you walk between);
        // the procedural fallback keeps drawing it because without the
        // framing that ship is one flat loft.
        let aboard_now = self
            .world
            .parked
            .as_ref()
            .map(|(o, ship)| ship.room_at(self.car.pos - *o).is_some())
            .unwrap_or(false);
        let show_cells = self.aboard.exterior.is_none() || aboard_now;
        if let (true, Some(cells_slot), Some((origin, _))) =
            (show_cells, self.aboard.cells_slot, self.world.parked.as_ref())
        {
            let lift = Mat4::from_translation(*origin);
            push(
                &mut objs,
                cells_slot,
                None,
                lift,
                [0.78, 0.80, 0.74, 1.0],
                [0.35, 0.0, 0.25, 0.0],
            );
        }

        // playful shapes
        for (si, s) in self.world.shapes.iter().enumerate() {
            if s.cut {
                continue; // CSG cutters carve the SDF; they are not drawn
            }
            let em = (s.emissive + s.flash * 1.1).min(1.0);
            // OFF-SCREEN SHAPES COST NOTHING TO SKIP. The renderer had a
            // distance cull but never a frustum one, so on a big map most of
            // what it submitted was behind the camera.
            let bound = shape_bound(&s.sdf).max(2.0);
            if !sphere_visible(planes, s.pos, bound + 6.0) {
                continue;
            }
            let dd2 = s.pos.distance_squared(rc.0);
            if dd2 > 640.0 * 640.0 {
                continue;
            }
            if dd2 > 250.0 * 250.0 {
                let small = match s.sdf {
                    world::Sdf::Sphere { r } => r < 4.0,
                    world::Sdf::Torus { big, tube } => big + tube < 4.5,
                    world::Sdf::Hex { r, .. } => r < 5.0,
                    world::Sdf::Capsule { r, half } => r + half < 5.0,
                    world::Sdf::RBox { half, .. } => half.length() < 5.0,
                    world::Sdf::Shell { r, half, .. } => r.max(half) < 6.0,
                    world::Sdf::TubeTorus { .. } => false, // loops are landmarks
                    world::Sdf::RingDeck { .. } => false,
                    world::Sdf::None => true,
                };
                if small {
                    continue;
                }
            }
            // LOD BY HOW BIG IT LOOKS, NOT HOW FAR IT IS.
            //
            // This was `d2 > 90²` / `d2 > 140²` — one set of distances for
            // every shape, whatever its size. That is correct only when
            // everything is the same size, and it is not: the shapes here
            // already span an order of magnitude, and `space.rs` was written
            // because scaling KUIPER's worlds up widens that further. A
            // distance rule gives a giant at four hundred metres and a pebble
            // at eighty the SAME treatment, which is backwards for both — the
            // giant fills the screen and gets a low-poly mesh, the pebble is
            // a speck and keeps 750 triangles.
            //
            // What decides how much detail something needs is the angle it
            // subtends, `2·atan(r/d)`. `space::tier_for` is that arithmetic,
            // tested, and until now called only by the `--space` CLI — the
            // measurement existed and the renderer never asked it.
            let d = s.pos.distance(rc.0);
            let tier = space::tier_for(world::shape_reach(&s.sdf).max(0.05), d);
            // SWAP AT IMPOSTOR, NOT LOW. `TIER_DEG` lists where each tier
            // BEGINS, so `tier <= Low` fires below 8 degrees — which for a
            // 1.5 m prop is 21 m, against the old rule's 90 m. That is a
            // visible downgrade on every rock and orb in the near field, and
            // I had it in this file before a test worked the arithmetic out.
            // The old thresholds correspond to about 1.9 degrees, so Impostor
            // (2.5) is the parity-preserving line.
            let coarse = tier <= space::Tier::Impostor;
            let mesh = match (s.mesh, coarse) {
                (world::M_SHELL, true) => world::M_SHELL_LO,
                (world::M_SHELL_OPEN, true) => world::M_SHELL_OPEN_LO,
                (world::M_SPHERE, true) => world::M_SPHERE_LO,
                (world::M_CAP_TALL, true) => world::M_CAP_TALL_LO,
                _ => s.mesh,
            };
            // params.x flags ROUGH GLASS: tubes and junction rooms get a
            // frosted, scattering reflection instead of a mirror finish
            let glassy = matches!(
                s.mesh,
                world::M_SHELL | world::M_SHELL_LO | world::M_SHELL_OPEN | world::M_SHELL_OPEN_LO | world::M_TORUS_TUBE
            );
            // a portal FACE carries its index, so the shader can march the
            // world from the far mouth and show what is actually over there
            let pflag = self
                .world
                .portals
                .iter()
                .take(8)
                .position(|p| p.shape == si)
                .map(|k| 2.0 + k as f32);
            let px = pflag.unwrap_or(if glassy { 1.0 } else { 0.0 });
            push(&mut objs, mesh, None, s.mat, s.color, [px, em, s.spec, 0.0]);
        }

        // ---- REPLAY: draw the recorded car and ball ----
        if let Some((mi, t)) = self.watching {
            if let Some(m) = self.moments.get(mi) {
                if let Some(f) = replay::pose_at(&m.frames, t) {
                    let cm = &self.cars.models[self.cars.active];
                    let base = Mat4::from_scale_rotation_translation(
                        Vec3::splat(self.cfg.car.size),
                        f.car_rot,
                        f.car,
                    );
                    for p in cm.prims.iter() {
                        push(&mut objs, p.mesh_id, p.tex_id, base, p.color, [0.0, 0.0, 0.5, 0.0]);
                    }
                    let bm = Mat4::from_scale_rotation_translation(
                        Vec3::splat(self.ball.r),
                        f.ball_rot,
                        f.ball,
                    );
                    push(&mut objs, world::M_SPHERE, None, bm, [0.93, 0.93, 0.96, 1.0], [0.0, 0.03, 0.55, 0.0]);
                }
            }
        }

        // ---- GHOST: your best run on this trial, racing alongside ----
        if let (Some(run), false) = (self.trial.as_ref(), self.ghost.is_empty()) {
            if let Some((gp, gr)) = trials::ghost_at(&self.ghost, run.t) {
                let m = &self.cars.models[self.cars.active];
                let base = Mat4::from_scale_rotation_translation(
                    Vec3::splat(self.cfg.car.size),
                    gr,
                    gp,
                );
                for p in m.prims.iter() {
                    let model = base;
                    push(
                        &mut objs,
                        p.mesh_id,
                        None,
                        model,
                        [0.45, 0.85, 1.0, 0.28],
                        [0.0, 0.35, 0.1, 0.0],
                    );
                }
            }
        }

        // ---- rain: instanced streaks around the car (MONKEY RUN) ----
        if self.wx_rain > 0.02 {
            let n = (self.wx_rain * 380.0) as u32;
            let t = self.time;
            for i in 0..n {
                let h1 = i.wrapping_mul(2654435761);
                let h2 = i.wrapping_mul(1597334677);
                let px = rc.0.x + ((h1 % 1100) as f32 * 0.1 - 55.0);
                let pz = rc.0.z + (((h1 >> 11) % 1100) as f32 * 0.1 - 55.0);
                let fall = (t * 34.0 + (h2 % 700) as f32 * 0.1) % 70.0;
                let py = rc.0.y + 26.0 - fall;
                let m = Mat4::from_translation(Vec3::new(px, py, pz))
                    * Mat4::from_rotation_z(0.10)
                    * Mat4::from_scale(Vec3::new(0.035, 0.95, 0.035));
                push(&mut objs, world::M_BOX, None, m, [0.72, 0.80, 0.95, 0.45], [0.0, 0.12, 0.1, 0.0]);
            }
        }

        // ---- CONTACT SHADOWS, the cheap version (C13-2a) ----
        // Nothing in the recording touched the ground: the bot floats. The
        // SDF shadow probe sees the RT car proxies, but a proxy is a low
        // car-shaped box under a body that is standing up as a robot, and
        // only the twenty nearest bots have one. A flattened dark disc at
        // the ground under every body is the decal C13 asked for first:
        // it sits on `ground_h`, so a body in the air leaves it behind and
        // shrinks it — the oldest contact cue there is. Alpha, so the
        // back-to-front sort draws it over the terrain and under the body.
        {
            let blob = |objs: &mut Vec<ObjDraw>, pos: Vec3, r: f32| {
                let gy = self.world.ground_h(pos.x, pos.z);
                let h = (pos.y - gy).max(0.0);
                let fade = (1.0 - h / 6.0).clamp(0.12, 1.0);
                let rr = r * (1.0 + h * 0.08);
                let m = Mat4::from_translation(Vec3::new(pos.x, gy + 0.035, pos.z))
                    * Mat4::from_scale(Vec3::new(rr, 0.028, rr * 0.92));
                push(objs, world::M_SPHERE_LO, None, m, [0.02, 0.02, 0.03, 0.62 * fade], [0.0, 0.0, 0.0, 0.0]);
            };
            if self.mode == Mode::Play {
                blob(&mut objs, rc.0, self.car.half_len * 1.15);
            }
            let cam = self.cam.pos;
            for b in self.bots.iter() {
                if b.car.pos.distance_squared(cam) > 160.0 * 160.0 {
                    continue;
                }
                blob(&mut objs, b.car.pos, b.car.half_len * 1.1);
            }
        }

        // ---- TRACERS: every live shot, both pools ----
        // The shot pools flew invisible rounds: `shots::step` landed them
        // and `fx` burst on impact, but nothing drew the flight. War for
        // the Foundry's whole feel is the stream in the air; a fight where
        // you cannot see what is coming reads as nothing happening. A
        // stretched emissive sphere along the velocity, element-tinted;
        // hostile rounds pulled toward a hot red so whose it is reads at a
        // glance, the way Destiny keeps enemy fire on its own palette.
        {
            let tracer = |objs: &mut Vec<ObjDraw>, s: &shots::Live, hostile: bool| {
                let v = s.vel;
                let speed = v.length();
                if speed < 1.0 {
                    return;
                }
                let dir = v / speed;
                let len = (speed * 0.035).clamp(0.35, 2.6);
                let (rad, glow) = match s.spec.delivery {
                    ballistics::Delivery::Blast => (0.16, 0.85),
                    ballistics::Delivery::Beam => (0.05, 1.0),
                    _ => (0.06, 1.0),
                };
                let mut c = element_glow(s.element);
                if hostile {
                    c = [c[0] * 0.5 + 0.5, c[1] * 0.35 + 0.08, c[2] * 0.35 + 0.12, 0.95];
                }
                let rot = Quat::from_rotation_arc(Vec3::Y, dir);
                let m = Mat4::from_translation(s.pos) * Mat4::from_quat(rot) * Mat4::from_scale(Vec3::new(rad, len, rad));
                push(objs, world::M_SPHERE_LO, None, m, c, [0.0, glow, 0.0, 0.0]);
            };
            for s in &self.shots.live {
                tracer(&mut objs, s, false);
            }
            for s in &self.hostile_shots.live {
                tracer(&mut objs, s, true);
            }
        }

        // ---- CABLES, drawn ----
        // The field's lines were a burst and a toast. A cable is a thin
        // box between two chests, the same primitive the rain streaks
        // use; the tow line is warmer than a hostile's so you know whose
        // it is when both are in the frame.
        if let Some(s) = self.sortie.as_ref() {
            let chest = rc.0 + Vec3::Y * (self.car.half_h * 0.8);
            for line in s.field.cables(chest) {
                let d = line.to - line.from;
                let len = d.length();
                if len < 0.2 {
                    continue;
                }
                let dir = d / len;
                let rot = Quat::from_rotation_arc(Vec3::Y, dir);
                let mid = line.from + d * 0.5;
                let m = Mat4::from_translation(mid) * Mat4::from_quat(rot) * Mat4::from_scale(Vec3::new(0.045, len * 0.5, 0.045));
                let c = if line.tow { [0.95, 0.82, 0.45, 1.0] } else { [0.55, 0.6, 0.7, 1.0] };
                push(&mut objs, world::M_BOX, None, m, c, [0.0, if line.tow { 0.6 } else { 0.25 }, 0.4, 0.0]);
            }
        }

        // boost pads
        for p in &self.world.pads {
            let ready = p.timer <= 0.0;
            let r = if p.big { 1.55 } else { 0.95 };
            let marker = Mat4::from_translation(p.pos + Vec3::Y * 0.03)
                * Mat4::from_scale(Vec3::new(r, r * 0.28, r));
            push(
                &mut objs,
                world::M_TORUS_THIN,
                None,
                marker,
                world::GOLD,
                [0.0, if ready { 0.55 } else { 0.06 }, 0.5, 0.0],
            );
            let bob = (self.time * 2.1 + p.phase).sin() * 0.09;
            let orb_s = if !ready { 0.02 } else if p.big { 0.42 } else { 0.2 };
            let orb = Mat4::from_translation(
                p.pos + Vec3::Y * (if p.big { 1.05 } else { 0.55 } + bob),
            ) * Mat4::from_scale(Vec3::splat(orb_s));
            push(&mut objs, world::M_SPHERE, None, orb, world::GOLD, [0.0, 0.95, 0.6, 0.0]);
        }

        // display plinth cars (red Dominus)
        for (i, m) in self.world.decor_cars.iter().enumerate() {
            let slot = (ids.decor0 + i + 1) as f32;
            for prim in &self.decor_model.prims {
                let spec = if matches!(prim.tag, model::Tag::Body) { 0.55 } else { 0.15 };
                push(&mut objs, prim.mesh_id, prim.tex_id, *m, prim.color, [0.0, 0.0, spec, slot]);
            }
        }

        // the ball
        let ball_m = Mat4::from_translation(rb.0)
            * Mat4::from_quat(rb.1)
            * Mat4::from_scale(Vec3::splat(self.ball.r));
        let ball_slot = (ids.ball + 1) as f32;
        for prim in &self.ball_model.prims {
            push(&mut objs, prim.mesh_id, prim.tex_id, ball_m, prim.color, [0.0, 0.03, 0.45, ball_slot]);
        }

        // THE LOCAL CAR, or the local BOT MID-STRIDE. walk_frame swaps the
        // static robot pose for the phase anim::Locomotion is already
        // computing — it returns None for a vehicle, for a chassis with no
        // walk strip, and for anything not standing at the robot end, so the
        // fold pose stays the default and the walk is the exception.
        let local_model = self.walk_frame(self.cars.active).unwrap_or(self.cars.active);
        self.push_car(
            &mut objs,
            local_model,
            rc.0,
            rc.1,
            self.flip_vis,
            self.car.wheel_spin,
            self.car.steer_vis,
            (ids.car + 1) as f32,
            self.cfg.car.size,
            None,
            true,
            self.skin_params(),);
        // ---- GARAGE LIVE VIEW ----
        // The customizer used to describe a car in words while the car
        // itself sat behind the panel. So put it on stage: the SAME model,
        // pushed into the scene near the camera, which means it gets the
        // real skin, the real rig motion, the real reflections and the real
        // lighting rather than a flat thumbnail that would have to be kept
        // in sync with all four.
        // the catalogue previews what you are HIGHLIGHTING, not what you are
        // driving — otherwise browsing shows you the same car forty times
        let stage_car = match self.mode {
            Mode::Catalog { sel } => Some(sel.min(self.cars.models.len() - 1)),
            Mode::Garage { .. } => Some(self.cars.active),
            _ => None,
        };
        if let Some(show) = stage_car {
            self.push_stage(&mut objs, show);
        }

        // remote cars + their boost flames
        for (i, p) in self.net.peers().enumerate() {
            self.push_car(&mut objs, p.car.min(self.cars.models.len() - 1), p.shown, p.rot, p.flip, p.wheel_spin, p.steer_vis,
                (ids.remote0 + i + 1) as f32, 1.0, None, true, [0.0; 4],);
            if p.boosting {
                let fwd = p.rot * Vec3::NEG_Z;
                let tail = p.shown - fwd * (self.cars.models[p.car.min(2)].half_len + 0.12);
                let fm = Mat4::from_translation(tail) * Mat4::from_scale(Vec3::splat(0.13));
                push(&mut objs, world::M_SPHERE, None, fm, [1.0, 0.55, 0.08, 0.9], [0.0, 1.0, 0.0, 0.0]);
            }
        }

        // the swarm: distance-LOD bot rendering (wheels only up close)
        let cam = self.cam.pos;
        for (bi, b) in self.bots.iter().enumerate() {
            let d2 = b.car.pos.distance_squared(cam);
            if d2 > 150.0 * 150.0 {
                continue;
            }
            // the same for the others: you spend far more of a raid watching
            // somebody else walk than watching yourself
            self.push_car(
                &mut objs,
                // golden-ratio offset per bot: any stride count spreads
                // evenly, where dividing by the bot count would re-synchronise
                // whenever two of them happened to share a phase
                self.walk_frame_at(b.model, (bi as f32) * 0.618_034)
                    .unwrap_or(b.model),
                b.car.pos,
                b.car.rot,
                false,
                b.car.wheel_spin,
                b.car.steer_vis,
                0.0,
                1.0,
                Some(b.tint),
                d2 < 60.0 * 60.0, [0.0; 4],);
            // a field hostile driving in vehicle form burns its own
            // nozzles, the same way you do — read off ITS model
            if b.brain.hostile_slot().is_some() && !b.car.robot && b.car.vel.length() > 2.0 {
                let thr = &self.cars.models[b.model.min(self.cars.models.len() - 1)].hardpoints.thrusters;
                let flick = 0.09 + ((self.time * 29.0 + bi as f32).sin() * 0.5 + 0.5) * 0.05;
                for t in thr.iter() {
                    let at = b.car.pos + b.car.rot * t.pos;
                    let aim = (b.car.rot * t.aim).normalize_or(Vec3::Z);
                    let rot = Quat::from_rotation_arc(Vec3::Y, aim);
                    let fm = Mat4::from_translation(at + aim * flick) * Mat4::from_quat(rot) * Mat4::from_scale(Vec3::new(flick * 0.8, flick * 1.8, flick * 0.8));
                    push(&mut objs, world::M_SPHERE, None, fm, [1.0, 0.5, 0.15, 0.85], [0.0, 1.0, 0.0, 0.0]);
                }
            }
        }

        // local boost flame — FROM THE THRUSTERS the hull actually has
        // (`hardpoints::find`), one flame per nozzle at its exit, along its
        // own aim; the tail-centre blob only when the hull has none.
        let boosting = self.car.boost > 0.0 && self.action_down(Action::Boost);
        if boosting && self.mode == Mode::Play {
            let flick = 0.11 + ((self.time * 31.0).sin() * 0.5 + 0.5) * 0.07;
            let size = self.cfg.car.size;
            // each pose has its own fittings: the vehicle's nozzles at the
            // tail, the robot's on its back or its boots, wherever the
            // model put them. Only the tail blob when a pose has none.
            let thr = &self.cars.models[local_model.min(self.cars.models.len() - 1)].hardpoints.thrusters;
            if thr.is_empty() {
                let tail = rc.0 - (rc.1 * Vec3::NEG_Z) * (self.car.half_len + 0.12);
                let fm = Mat4::from_translation(tail) * Mat4::from_scale(Vec3::splat(flick));
                push(&mut objs, world::M_SPHERE, None, fm, [1.0, 0.55, 0.08, 0.9], [0.0, 1.0, 0.0, 0.0]);
            } else {
                for (k, t) in thr.iter().enumerate() {
                    let at = rc.0 + rc.1 * (t.pos * size);
                    let aim = (rc.1 * t.aim).normalize_or(Vec3::Z);
                    // each nozzle on its own flicker so a pair does not pulse in step
                    let f = flick * (0.85 + 0.15 * ((self.time * 27.0 + k as f32 * 1.7).sin() * 0.5 + 0.5));
                    let rot = Quat::from_rotation_arc(Vec3::Y, aim);
                    let fm = Mat4::from_translation(at + aim * (f * 1.2)) * Mat4::from_quat(rot) * Mat4::from_scale(Vec3::new(f * 0.8, f * 1.9, f * 0.8));
                    push(&mut objs, world::M_SPHERE, None, fm, [1.0, 0.6, 0.12, 0.9], [0.0, 1.0, 0.0, 0.0]);
                }
            }
        }

        objs
    }

    #[allow(clippy::too_many_arguments)]
    /// The garage skin as shader inputs: id, a phase so two cars with the
    /// same finish are not in lockstep, intensity, and the car's speed
    /// (FORGE heats with it).
    /// The exotic skins' second input: where the ball is, in the CAR's own
    /// frame (so a finish can face the play as you turn), and whether boost
    /// is running. A texture has neither of these.
    fn skin_params2(&self) -> [f32; 4] {
        let to_ball = self.car.rot.inverse() * (self.ball.pos - self.car.pos);
        let d = to_ball.normalize_or(Vec3::NEG_Z);
        let boosting = self.action_down(Action::Boost) && self.car.boost > 0.0;
        [d.x, d.y, d.z, if boosting { 1.0 } else { 0.0 }]
    }

    /// The third input: the last contact point in the car's own frame, and
    /// how long ago, so IMPACT can roll a wave out from where it was hit.
    fn skin_params3(&self) -> [f32; 4] {
        let h = self.last_hit.0;
        [h.x, h.y, h.z, self.last_hit.1]
    }

    fn skin_params(&self) -> [f32; 4] {
        let sp = (self.car.vel.length() / 25.0).clamp(0.0, 1.0);
        // z doubles as the MOMENTUM input: how hot the current combo is
        let combo = (self.combo as f32 / 8.0).clamp(0.0, 1.0);
        [self.look.skin as f32, (self.save.car as f32) * 1.7, combo.max(0.35), sp]
    }

    /// THE CATALOGUE STAND. Extracted because it was the only place four
    /// near-identical `push_car` calls could be told apart, and editing it
    /// by string match into a five-thousand-line render function put the
    /// same block into the peer-rendering loop twice.
    ///
    /// One place to change, and the things it needs — camera basis, the
    /// stand's position, the model's scale — are locals here rather than
    /// buried in the middle of another function.
    fn push_stage(&self, objs: &mut Vec<ObjDraw>, show: usize) {
                let aspect = self.gpu.config.width.max(1) as f32 / self.gpu.config.height.max(1) as f32;
                // camera basis, so the stand follows wherever you are looking
                let fwd = (self.cam.look - self.cam.pos).normalize_or(Vec3::NEG_Z);
                let right = fwd.cross(self.cam.up).normalize_or(Vec3::X);
                let up = right.cross(fwd).normalize_or(Vec3::Y);
                // WIDE SCREENS PUT IT BESIDE THE PANEL, TALL ONES ABOVE IT.
                // On a wide window there is room to the right and none above; on
                // a tall or square one the reverse. The panel narrows to match
                // (see `garage_menu`), so the two never overlap.
                let wide = aspect > 1.25;
                let dist = 4.2;
                let off = if wide {
                    right * (dist * 0.40) + up * (dist * 0.02)
                } else {
                    up * (dist * 0.30)
                };
                let at = self.cam.pos + fwd * dist + off;
                // a slow turntable, so you see the whole car without touching it
                let spin = Quat::from_rotation_y(self.time * 0.5);
                // A BOSS PREVIEWS AT BOSS SIZE. Scaling it up alone would put it
                // through the camera, so the stand backs off by the same factor —
                // the model reads as huge because the frame is the same and the
                // thing in it is not.
                let cscale = self.chassis_scale(show);
                let at = if cscale > 1.0 { self.cam.pos + (at - self.cam.pos) * cscale } else { at };

                // IDLE, from the chassis's own size rather than a per-model
                // curve. A pendulum's period goes as the square root of its
                // length, so a boss breathes slowly for the same reason its
                // fold is slow — one relationship, two uses.
                let idl = idle::pose(
                    &idle::Chassis {
                        kind: idle::Kind::from_class(self.cars.class_of(show)),
                        scale: cscale,
                        reach_m: self.cfg.car.size.max(0.3) * 4.0,
                    },
                    self.time,
                );
                let at = at + up * idl.bob + right * idl.sway;
                let spin = spin
                    * Quat::from_rotation_y(idl.yaw)
                    * Quat::from_rotation_z(idl.roll)
                    * Quat::from_rotation_x(idl.pitch);

                // MINIONS FOR SCALE. A boss previewed alone is a model that
                // fills the frame — you cannot tell 2.6x from a closer
                // camera. Three ordinary chassis beside it give the eye
                // something to measure against, which is the only way a size
                // reads at all.
                if cscale > 1.0 {
                    let n = self.cars.models.len().max(1);
                    for m in 0..3usize {
                        let mut pick = (show + 1 + m * 7) % n;
                        for _ in 0..n.min(24) {
                            if !self.is_boss(pick) {
                                break;
                            }
                            pick = (pick + 1) % n;
                        }
                        if self.is_boss(pick) {
                            continue;
                        }
                        let ang = std::f32::consts::TAU * (m as f32 / 3.0) + 0.6;
                        let moff =
                            right * ang.cos() * cscale * 0.9 + fwd * ang.sin() * cscale * 0.45;
                        // each on its own phase, or three bots breathe in
                        // unison and read as one object
                        let mi = idle::pose(
                            &idle::Chassis {
                                kind: idle::Kind::from_class(self.cars.class_of(pick)),
                                scale: 1.0,
                                reach_m: self.cfg.car.size.max(0.3) * 4.0,
                            },
                            self.time + m as f32 * 1.7,
                        );
                        self.push_car(
                            objs,
                            pick,
                            at + moff + up * (mi.bob - 0.12 * cscale),
                            Quat::from_rotation_y(self.time * 0.5 + ang)
                                * Quat::from_rotation_z(mi.roll),
                            false,
                            0.0,
                            0.0,
                            0.0,
                            self.cfg.car.size,
                            None,
                            true,
                            self.skin_params(),
                        );
                    }
                }

                // NO 3D MARKER YET. It wants an ObjDraw with the renderer's raw
                // object format and a mesh to point at, and guessing at that
                // blind is how you get a marker in the wrong space that reads as
                // a joint in the wrong place — the exact failure this tool
                // exists to catch. The readout names the joint and its parts;
                // the marker is the next piece, and it belongs beside a display.
                self.push_car(
                    objs,
                    show,
                    at,
                    spin,
                    false,
                    self.time * 2.2,
                    0.0,
                    0.0,
                    self.cfg.car.size * 1.35,
                    None,
                    true,
                    self.skin_params(),
                );
    }

    fn push_car(
        &self,
        objs: &mut Vec<ObjDraw>,
        model_i: usize,
        pos: Vec3,
        rot: Quat,
        flip: bool,
        wheel_spin: f32,
        steer_vis: f32,
        rt_slot: f32,
        scale: f32,
        tint: Option<[f32; 3]>,
        wheels: bool,
        // [id, phase, intensity, speed] — the garage skin, or zeros
        skin: [f32; 4],
    ) {
        let push_skinned = |objs: &mut Vec<ObjDraw>,
                    mesh: usize,
                    tex: Option<usize>,
                    model: Mat4,
                    color: [f32; 4],
                    params: [f32; 4],
                    skin: [f32; 4],
                    skin2: [f32; 4],
                    skin3: [f32; 4]| {
            let nrm3 = Mat3::from_mat4(model).inverse().transpose();
            let mut raw = ObjectRaw::default();
            raw.model = model.to_cols_array();
            raw.nrm = Mat4::from_mat3(nrm3).to_cols_array();
            raw.color = color;
            raw.params = params;
            raw.skin = skin;
            raw.skin2 = skin2;
            raw.skin3 = skin3;
            objs.push(ObjDraw { mesh, tex, raw });
        };
        // one rim per wheel position: a split wheel can be several prims
        fn pivot_key(p: Vec3) -> [i32; 3] {
            [(p.x * 400.0) as i32, (p.y * 400.0) as i32, (p.z * 400.0) as i32]
        }
        let mut seen_pivots: Vec<[i32; 3]> = Vec::new();
        let rim_style = if rt_slot > 0.5 { self.look.wheels } else { 0 };
        let rig = if rt_slot > 0.5 { self.look.rig } else { 0 };
        let skin2 = if rt_slot > 0.5 { self.skin_params2() } else { [0.0; 4] };
        let skin3 = if rt_slot > 0.5 { self.skin_params3() } else { [0.0; 4] };
        // PAINT multiplies the body's own colour, so a finish is a behaviour
        // and the paint is whose car it is
        let paint = if rt_slot > 0.5 { self.look.paint_rgb() } else { [1.0; 3] };
        let wheel_r = self.cars.models[model_i].wheel_r * scale;
        let rim_col = [0.72, 0.75, 0.80, 1.0];
        #[allow(unused)]
        let push = |objs: &mut Vec<ObjDraw>,
                    mesh: usize,
                    tex: Option<usize>,
                    model: Mat4,
                    color: [f32; 4],
                    params: [f32; 4]| {
            let nrm3 = Mat3::from_mat4(model).inverse().transpose();
            let mut raw = ObjectRaw::default();
            raw.model = model.to_cols_array();
            raw.nrm = Mat4::from_mat3(nrm3).to_cols_array();
            raw.color = color;
            raw.params = params;
            objs.push(ObjDraw { mesh, tex, raw });
        };
        let flip_m = if flip {
            Mat4::from_quat(Quat::from_rotation_y(std::f32::consts::PI))
        } else {
            Mat4::IDENTITY
        };
        let base = Mat4::from_translation(pos) * Mat4::from_quat(rot) * flip_m
            * Mat4::from_scale(Vec3::splat(scale));
        let active = &self.cars.models[model_i];
        // THE TURRET TRACKS THE LOOK, not the hull. Two orientations on one
        // body is what makes armour read as armour: the hull goes where you
        // steer and the gun goes where you are aiming. Yaw only, about the
        // RING rather than the turret's mass (`turret::find`) — a gun that
        // pivots on its own centroid swings its breech through the hull.
        let turret_q = if active.fittings.has_turret() {
            // where the EYE is looking, taken from the camera's own
            // geometry rather than a euler it does not keep: the vector
            // from the eye to what it is framing, flattened.
            let view = (self.cam.look - self.cam.pos) * Vec3::new(1.0, 0.0, 1.0);
            let want = if view.length_squared() > 1e-4 {
                view.x.atan2(-view.z)
            } else {
                let f = rot * Vec3::NEG_Z;
                f.x.atan2(-f.z)
            };
            let hull = {
                let f = rot * Vec3::NEG_Z;
                f.x.atan2(-f.z)
            };
            let mut d = want - hull;
            while d > std::f32::consts::PI {
                d -= std::f32::consts::TAU;
            }
            while d < -std::f32::consts::PI {
                d += std::f32::consts::TAU;
            }
            Some(Quat::from_rotation_y(d))
        } else {
            None
        };
        // ELEVATION, on the one prim `turret::find` identified as the
        // barrel, about its own detected pivot/axis rather than the
        // ring — pitching about the ring would swing the whole turret
        // body, not just tip the gun. Composed INSIDE the yaw above
        // (applied to the raw prim before `turret_q`, see the order of
        // the two match arms below): elevate the barrel in the turret's
        // own unyawed frame first, then yaw the already-elevated barrel
        // along with the rest of the turret.
        //
        // FIRST-PASS ELEVATION LIMITS, not a measured or playtested
        // number: +20/-8 degrees is a generic real-world tank-gun range,
        // used here as a physically-motivated default rather than an
        // arbitrary one, since this needs a controller in someone's
        // hands to actually tune. Expect these two constants to move.
        const MAX_ELEVATION: f32 = 20.0 * std::f32::consts::PI / 180.0;
        const MAX_DEPRESSION: f32 = 8.0 * std::f32::consts::PI / 180.0;
        let elevation_q = active.fittings.barrel.map(|_| {
            let view = self.cam.look - self.cam.pos;
            let flat = (view.x * view.x + view.z * view.z).sqrt().max(1e-4);
            let want_pitch = view.y.atan2(flat).clamp(-MAX_DEPRESSION, MAX_ELEVATION);
            // `elevation_axis` is defined (turret.rs, verified by its own
            // test) so a POSITIVE angle raises the muzzle — same sign as
            // "looking up is a positive pitch" here, so no sign flip
            // needed at this call site.
            Quat::from_axis_angle(active.fittings.elevation_axis, want_pitch)
        });
        let robot = self.xf.is_robot();
        // STAND THE DESTINATION UP WITHOUT BENDING THE JOURNEY.
        //
        // The fold is coherent — the animation lines up from vehicle into
        // robot — so the shared chassis frame is right and must not be
        // touched. What is wrong is where it ENDS: an artist may author the
        // robot standing along a different axis from the one the vehicle
        // drives along, and a correct interpolation carries it there. So
        // the leftover uprighting (`Model::settle`) is applied RAMPED by
        // fold progress: identity while driving, full at the end, smooth
        // in between. The animation is untouched; only its destination
        // moves.
        let settle_q = {
            let t = self.xf.eased().clamp(0.0, 1.0);
            if active.settle == Quat::IDENTITY || t <= 0.0 {
                None
            } else {
                Some(Quat::IDENTITY.slerp(active.settle, t))
            }
        };
        for (pi, prim) in active.prims.iter().enumerate() {
            if !wheels && !matches!(prim.tag, model::Tag::Body) {
                continue;
            }
            let mut color = prim.color;
            if matches!(prim.tag, model::Tag::Body) && self.look.paint > 0 {
                color = [color[0] * paint[0], color[1] * paint[1], color[2] * paint[2], color[3]];
            }
            if let Some(t) = tint {
                color = [color[0] * t[0], color[1] * t[1], color[2] * t[2], color[3]];
            }
            // THE RIG: displace this part before it is drawn. Each prim gets
            // its own phase from its position, so panels move out of step
            // with one another the way real machinery does — a car where
            // everything moves together reads as one wobbling object.
            // the turret's own transform rides inside the hull's: rotate
            // about the ring in MODEL space, before the hull matrix
            // ROBOT MODE PULLS ITS APPENDAGES IN. A wing is correct at
            // vehicle scale and absurd on a standing bot, and the fold
            // cannot fix it because it interpolates vertices and the wing
            // is where the artist left it (`skeleton::compact`).
            // EACH MODE PACKS ITS OWN WAY. Robot mode shortens overhangs
            // so the bot is not swallowed by its wingspan (`compact`);
            // vehicle mode contains them inside the hull envelope so the
            // fold reads as a solid object rather than a skeleton in a box
            // (`contain`). Both are zero for a body that never needed it.
            //
            // CROSSFADED, not switched. These used to flip on `robot` — a
            // binary read of eased() > 0.5 — so mid-fold every prim with a
            // different pack and tuck JUMPED by their difference in one
            // frame, at exactly the moment the transformation has the
            // player's eyes on the body. Blend by the same eased fold
            // parameter the morph itself uses: pack at the vehicle end,
            // tuck at the robot end, identical to the old values at both
            // endpoints, continuous everywhere between.
            let base = match &settle_q {
                Some(q) => base * Mat4::from_quat(*q),
                None => base,
            };
            let fold_t = self.xf.eased().clamp(0.0, 1.0);
            let pack = active.pack.get(pi).copied().unwrap_or_default();
            let tuck = active.tuck.get(pi).copied().unwrap_or(Vec3::ZERO);
            let shift = pack.shift.lerp(tuck, fold_t);
            let base = if shift.length_squared() > 1e-12 {
                base * Mat4::from_translation(shift)
            } else {
                base
            };
            // THE HINGE rides the same crossfade: full at the vehicle end,
            // slerped away to identity as the body stands up — the wing
            // that stowed along the hull swings back out through exactly
            // the arc it stowed by. Rotation about the limb's own root,
            // because rotating about the origin detaches the limb
            // (`skeleton::fold_point`'s warning, honoured here).
            let base = if pack.rot != Quat::IDENTITY {
                let q = Quat::IDENTITY.slerp(pack.rot, 1.0 - fold_t);
                base * Mat4::from_translation(pack.pivot)
                    * Mat4::from_quat(q)
                    * Mat4::from_translation(-pack.pivot)
            } else {
                base
            };
            // DETECTED WHEELS SPIN TOO. `split_wheels` tags wheels it
            // recognises and the match below turns those; an imported bot
            // whose wheels are not at tidy corners got no tags, so nothing
            // turned. `fittings.rollers` is the fallback, spun on the same
            // `wheel_spin` the tagged ones use so they cannot drift apart.
            // ROLLERS ONLY ROLL WHEN THE BODY IS A VEHICLE. The detector
            // runs on every pose in the strip, and a robot pose has plenty
            // of round low outboard prims — knees, shoulders, heel blocks.
            // Spinning those is the reported "body parts rolling like
            // wheels in bot mode". The tagged wheels below have the same
            // problem in principle, but they are tagged from the vehicle
            // pose and the fold hides them; these are detected per pose
            // and are not hidden, so the guard goes here.
            let base = match active.fittings.rollers.iter().find(|(i, _)| *i == pi) {
                Some((_, pivot)) if !robot && matches!(prim.tag, model::Tag::Body) => {
                    base * Mat4::from_translation(*pivot)
                        * Mat4::from_rotation_x(-wheel_spin)
                        * Mat4::from_translation(-*pivot)
                }
                _ => base,
            };
            let base = match (&turret_q, active.fittings.turret.contains(&pi)) {
                (Some(q), true) => {
                    let r = active.fittings.ring;
                    base * Mat4::from_translation(r)
                        * Mat4::from_quat(*q)
                        * Mat4::from_translation(-r)
                }
                _ => base,
            };
            // ELEVATION, on the barrel only, composed AFTER the yaw above
            // so it ends up INNER to it (matrix chain applies right-to-
            // left to the raw vertex): pitch the barrel about its own
            // pivot in the turret's unyawed local frame first, then the
            // yaw carries the already-elevated barrel around with the
            // rest of the turret, exactly like a real mantlet riding on
            // a rotating turret ring.
            let base = match (&elevation_q, active.fittings.barrel) {
                (Some(eq), Some(bi)) if bi == pi => {
                    let p = active.fittings.elevation_pivot;
                    base * Mat4::from_translation(p)
                        * Mat4::from_quat(*eq)
                        * Mat4::from_translation(-p)
                }
                _ => base,
            };
            // WEAPON POP-OUT (cosmetic transform-lite): telescope the
            // barrel outward along its own root->tip axis while aiming/
            // firing, composed innermost of all (right after elevation,
            // before it) so it extends in the tube's own rest direction
            // and rides along with whatever elevation/yaw is already
            // applied to the rest of the assembly. Character expression,
            // not a mechanical system -- no new detection, no new
            // resource, no new keybinding; reuses the barrel this
            // session already built for elevation and ties to the
            // player already aiming or firing (`Car::weapon_pop`).
            let base = match active.fittings.barrel {
                Some(bi) if bi == pi && self.car.weapon_pop > 1e-4 => {
                    let root = active.fittings.elevation_pivot;
                    let reach = active.fittings.barrel_tip - root;
                    let len = reach.length();
                    let dir = if len > 1e-5 { reach / len } else { Vec3::NEG_Z };
                    // extend by up to a third of the barrel's own length
                    // -- enough to read as a real motion, not so much
                    // the muzzle detaches from its own mount visually.
                    // Not a number checked against how any specific
                    // chassis' barrel should look extending, same
                    // first-pass-guess caveat as this session's other
                    // new magnitudes.
                    base * Mat4::from_translation(dir * len * 0.33 * self.car.weapon_pop)
                }
                _ => base,
            };
            let base = if rig > 0 {
                let seed = prim.pivot.x * 3.1 + prim.pivot.z * 2.3 + prim.mesh_id as f32 * 0.37;
                let t = self.time;
                let d = match rig {
                    // BREATHING: a slow rise and settle
                    1 => Vec3::Y * ((t * 1.3 + seed).sin() * 0.012),
                    // FLUTTER: fast, and strongest away from the centre
                    2 => {
                        let reach = (prim.pivot.length() + 0.2).min(1.0);
                        Vec3::new((t * 17.0 + seed).sin(), (t * 15.0 + seed * 1.7).sin(), 0.0)
                            * 0.004
                            * reach
                    }
                    // UNFOLD: plates lift away under boost, close when you lift
                    3 => {
                        // boost tank falling = boost being used
                        // boost in the tank draining = boost being used
                        // a full tank sits at 1.0; using boost drains it, so
                        // the plates open as the reserve falls under power
                        let open = if self.car.boost < 0.995 { 1.0 } else { 0.0 };
                        Vec3::new(prim.pivot.x.signum() * 0.6, 0.5, 0.0).normalize_or(Vec3::Y)
                            * (0.030 * open)
                    }
                    // HOVER: the body floats and leans into the turn
                    4 => {
                        if matches!(prim.tag, model::Tag::Body) {
                            Vec3::Y * (0.020 + (t * 0.9).sin() * 0.006)
                                + Vec3::X * (self.car.ang.y * -0.02)
                        } else {
                            Vec3::ZERO
                        }
                    }
                    // SHIVER: a jolt on contact that damps out
                    _ => {
                        let k = self.cam.shake_amt();
                        Vec3::new((t * 41.0 + seed).sin(), (t * 37.0 + seed).sin(), (t * 43.0 + seed).sin())
                            * 0.012
                            * k
                    }
                };
                base * Mat4::from_translation(d)
            } else {
                base
            };
            let m = match prim.tag {
                model::Tag::Body => base,
                model::Tag::WheelFront => {
                    base * Mat4::from_translation(prim.pivot)
                        * Mat4::from_rotation_y(steer_vis)
                        * Mat4::from_rotation_x(-wheel_spin)
                        * Mat4::from_translation(-prim.pivot)
                }
                model::Tag::WheelBack => {
                    base * Mat4::from_translation(prim.pivot)
                        * Mat4::from_rotation_x(-wheel_spin)
                        * Mat4::from_translation(-prim.pivot)
                }
            };
            let spec = if matches!(prim.tag, model::Tag::Body) { 0.6 } else { 0.15 };
            // the skin rides on the BODY only — wheels keep their own look
            let sk = if matches!(prim.tag, model::Tag::Body) { skin } else { [0.0; 4] };
            push_skinned(objs, prim.mesh_id, prim.tex_id, m, color, [0.0, 0.0, spec, rt_slot], sk, skin2, skin3);
            // THE GARAGE RIM, drawn over whatever wheel the car already has.
            // Because it rides the wheel's own pivot and transform, a style
            // applies to every car in the game — the imported bodies too —
            // without touching a single one of their meshes.
            if rim_style > 0 && !matches!(prim.tag, model::Tag::Body) && !seen_pivots.contains(&pivot_key(prim.pivot)) {
                seen_pivots.push(pivot_key(prim.pivot));
                let side = if prim.pivot.x >= 0.0 { 1.0 } else { -1.0 };
                let rm = m
                    * Mat4::from_translation(prim.pivot)
                    * Mat4::from_scale(Vec3::new(side * wheel_r * 1.02, wheel_r * 1.02, wheel_r * 1.02));
                push_skinned(objs, world::M_RIM_0 + rim_style, None, rm, rim_col, [0.0, 0.08, 0.75, 0.0], [0.0; 4], [0.0; 4], [0.0; 4]);
            }
        }
    }
}

/// What the gait machine is told about leg damage: the MAGNITUDE of
/// `sections::Body::limp()`. `limp()` is signed — negative is the left leg —
/// and `anim::Locomotion::condition` clamps its input to 0..1, so handing it
/// the signed value silently dropped every hurt left leg (C20_TODO: "a hurt
/// left leg never limps"). The sign is the rig's business (`rigmotion`), not
/// the gait's: a body crouches and hugs a wall the same whichever leg it is.
fn gait_hurt(limp: f32) -> f32 {
    limp.abs()
}

#[cfg(test)]
mod limp_tests {
    use super::*;

    #[test]
    fn a_hurt_left_leg_reaches_the_gait_machine_as_damage_not_as_zero() {
        // left (negative) and right (positive) damage are the same hurt to the gait
        assert_eq!(gait_hurt(-0.8), 0.8);
        assert_eq!(gait_hurt(0.8), 0.8);
        assert_eq!(gait_hurt(0.0), 0.0);
        // and the value the machine clamps is already in range, so nothing is lost at the clamp
        let mut l = anim::Locomotion::new();
        l.condition(gait_hurt(-0.8), 0.0);
        let hobbled = l.update(0.016, 2.0, 0.0, true, false, false);
        let mut fit = anim::Locomotion::new();
        fit.condition(gait_hurt(0.0), 0.0);
        let sound = fit.update(0.016, 2.0, 0.0, true, false, false);
        assert_ne!(format!("{hobbled:?}"), format!("{sound:?}"), "a left-leg limp changes the gait; before the fix it clamped to no damage at all");
    }
}

#[cfg(test)]
mod dims_tests {
    use super::dims_model;

    const STRIP: [usize; 5] = [10, 11, 12, 13, 14]; // f00 vehicle .. f04 robot

    #[test]
    fn a_folded_body_is_measured_by_the_pose_it_is_in() {
        // A smooth fold pins the displayed model to the strip's BASE and
        // rewrites its meshes, so anything re-deriving dimensions read a
        // car's numbers for a robot's body — `ride_h` 0.239 against feet at
        // 0.400 on `analog`, sixteen centimetres of sink.
        assert_eq!(dims_model(10, Some(&STRIP), true), 14, "robot should measure as f04");
        assert_eq!(dims_model(10, Some(&STRIP), false), 10, "vehicle measures as itself");
    }

    #[test]
    fn nothing_else_is_remapped() {
        assert_eq!(dims_model(12, Some(&STRIP), true), 12, "a mid-strip pose is itself");
        assert_eq!(dims_model(14, Some(&STRIP), true), 14, "the end pose is itself");
        // a chassis with no fold is never touched, in either mode
        assert_eq!(dims_model(7, None, true), 7);
        assert_eq!(dims_model(7, None, false), 7);
        // and an empty strip cannot panic on `last()`
        assert_eq!(dims_model(0, Some(&[]), true), 0);
    }
}

#[cfg(test)]
mod tweak_reach_tests {
    use super::*;

    /// Every menu the settings tree can reach from the root, followed.
    fn reachable_menus() -> Vec<u8> {
        let mut seen = vec![0u8];
        let mut i = 0;
        while i < seen.len() {
            let m = seen[i];
            for (_, row) in tweak_rows(m) {
                if let Row::Link(t) = row {
                    if !seen.contains(&t) {
                        seen.push(t);
                    }
                }
            }
            i += 1;
        }
        seen
    }

    /// THE GAP THIS CLOSES. `--tune` listed `story_anchor` as a knob and its
    /// own documentation called it "a main-menu dial", and it was in no menu
    /// at all — reachable only by hand-editing a save. Describing something
    /// as a setting does not put it in the settings.
    ///
    /// So: every knob the tuning ledger presents as a `cfg` setting must be
    /// movable from the menu tree. The ledger and the UI are two tables about
    /// the same thing, and nothing joined them.
    #[test]
    fn every_tuning_knob_is_reachable_in_a_menu() {
        let base = Config::default();
        for knob in tuning::KNOBS {
            // does any row in any reachable menu move this knob's value?
            let moves_it = reachable_menus().into_iter().any(|m| {
                tweak_rows(m).into_iter().any(|(_, row)| match row {
                    Row::Slider { get, set, min, max, .. } => {
                        let mut c = Config::default();
                        // drive the slider to both ends and see if the knob follows
                        for v in [min, max] {
                            set(&mut c, v);
                            if ((knob.read)(&c) - (knob.read)(&base)).abs() > 1e-6 {
                                return true;
                            }
                        }
                        let _ = get;
                        false
                    }
                    Row::Toggle { set, .. } => {
                        let mut c = Config::default();
                        for v in [true, false] {
                            set(&mut c, v);
                            if ((knob.read)(&c) - (knob.read)(&base)).abs() > 1e-6 {
                                return true;
                            }
                        }
                        false
                    }
                    _ => false,
                })
            });
            assert!(
                moves_it,
                "'{}' is in the tuning ledger and no menu row can move it — \
                 it is a setting only in the sense that the save file has a line for it",
                knob.id
            );
        }
    }

    #[test]
    fn no_settings_menu_is_orphaned() {
        // a menu nothing links to is a page nobody can open; the STORY menu
        // was one keystroke from being exactly that
        let reach = reachable_menus();
        // DERIVED, not a hand-maintained literal: this test's own comment
        // used to warn "1..=9, not 1..=8" -- a literal bound left the newest
        // menu (DEBUG, 9) unchecked when it was added, which is exactly
        // backwards, since the newest menu is the one most likely to still
        // be unreachable. A literal bound can silently go stale the same
        // way again the next time a menu is added. Scanning menu_title's
        // own match arms for numeric patterns can't drift from what menus
        // actually exist, because it reads the same source that defines them.
        let src = include_str!("main.rs");
        let body = src
            .split("fn menu_title(menu: u8) -> &'static str {")
            .nth(1)
            .and_then(|s| s.split("\n}").next())
            .expect("menu_title's body");
        let mut menus: Vec<u8> = body
            .lines()
            .filter_map(|l| l.trim().split_once(" =>"))
            .filter_map(|(pat, _)| pat.trim().parse::<u8>().ok())
            .collect();
        menus.sort_unstable();
        assert!(menus.len() >= 9, "only found {} numbered menus; the scan broke", menus.len());
        for m in menus {
            assert!(
                reach.contains(&m),
                "settings menu {m} ({}) cannot be reached from the root",
                menu_title(m)
            );
        }
    }
}


/// Row-enable helper for the Online screen (menu::Menu has no setter; rows
/// are public data).
fn set_row_enabled(m: &mut menu::Menu, id: &str, on: bool) {
    if let Some(r) = m.rows.iter_mut().find(|r| r.id == id) {
        r.enabled = on;
    }
}
