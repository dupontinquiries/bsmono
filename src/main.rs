//! SKYDRIBBLE — a Rocket-League-inspired open-world air-dribble playground.
//! Rust + wgpu 22 + winit 0.29 (the same stack as the reference repo's games).

mod anim;
mod actuator;
mod airlock;
mod analog;
mod bigbody;
mod bots;
mod camera;
mod chord;
mod clinch;
mod config;
mod drop;
mod font;
mod fx;
mod gamepad;
mod gyro;
mod content;
mod cyber;
mod custom;
mod partcars;
mod partdata;
mod patrol;
mod trials;
mod replay;
mod academy;
mod stream;
mod hud;
mod input;
mod lift;
mod interior;
mod menu;
mod meshgen;
mod model;
mod net;
mod physics;
mod platform;
mod procars;
mod render;
mod style;
mod save;
mod ship;
mod sim;
mod sound;
mod terrain;
mod touch;
mod ui;
mod weapons;
mod xform;
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
    Garage { sel: usize },
    Academy { sel: usize },
    Trials { sel: usize },
    Splash,
    Play,
    Pause,
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
            ("RAY QUALITY", pct(|c| c.video.quality, |c, v| c.video.quality = v, 0.4, 1.8)),
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
        Row::Link(_) | Row::Controls | Row::Trials | Row::Academy | Row::Garage | Row::GameMode(_) => (String::new(), None),
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
};
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--gfx" => o.gfx = Some(args.next().as_deref() != Some("classic")),
            "--seed" => o.seed = args.next().and_then(|v| v.parse().ok()),
            "--map" => o.map = args.next().and_then(|v| v.parse().ok()),
            "--fullscreen" => o.fullscreen = Some(true),
            "--windowed" => o.fullscreen = Some(false),
            "--mute" => o.mute = true,
            "--mobile" => o.mobile = true,
            "--desktop-ui" => o.desktop_ui = true,
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
    /// jets fly instead of driving: never grounded, lift and nose thrust
    flight: bool,
}

struct CarModels {
    models: Vec<model::Model>,
    names: Vec<&'static str>,
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

struct Game {
    gpu: render::Gpu,
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
    /// 0..1 health, which is what picks the fold's personality
    health: f32,
    /// the weapons module owns the lock; the camera borrows it
    wep: weapons::Weapons,
    /// autotarget on: ball cam follows an opponent instead of the ball
    autotarget: bool,

    /// how the current chassis stays up, when it flies at all
    lift: Option<lift::Lift>,
    /// space, as a tap-or-hold whose meaning the form decides
    space: chord::Chord,
    /// right mouse: glance when held, latch when tapped
    sight: chord::Sight,
    /// walk / run / boost / fall, for robot mode
    loco: anim::Locomotion,
    /// the pose it produced this frame
    loco_pose: anim::Pose,
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
    toast: String,
    toast_t: f32,
    mode: Mode,
    menu_from_pause: bool,
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
    use render::{CSDF_H, CSDF_MAXM, CSDF_SLAB, CSDF_W};
    let mut out = vec![255u8; (CSDF_W * CSDF_H * CSDF_SLAB * CSDF_MAXM) as usize];
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
                    let idx = (((mi as u32 * CSDF_SLAB + z) * CSDF_H + y) * CSDF_W + x) as usize;
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

fn bake_heightmap(gpu: &render::Gpu, seed: u64, style: &world::MapStyle) {
    let n = render::HMAP_SIZE as usize;
    let half = render::HMAP_HALF;
    let mut data = vec![0f32; n * n];
    for j in 0..n {
        let z = (j as f32 / (n - 1) as f32 * 2.0 - 1.0) * half;
        for i in 0..n {
            let x = (i as f32 / (n - 1) as f32 * 2.0 - 1.0) * half;
            data[j * n + i] = world::terrain_h_s(x, z, seed, style.amp, style.freq);
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
    env_logger::init();
    let opts = parse_args();
    let (mut sv, mut binds, mut cfg) = save::load();
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
    bake_heightmap(&gpu, seed, style0);

    // ---- models -----------------------------------------------------------
    let dir = assets_dir();
    println!("loading models from {:?} ...", dir);
    let t0 = Instant::now();
    let octane = model::load(&dir.join("octane"), &mut gpu, 1.28);
    let fennec = model::load(&dir.join("fennec"), &mut gpu, 1.28);
    let dominus = model::load(&dir.join("dominus"), &mut gpu, 1.32);
    let decor_model = model::load(&dir.join("dominus_red"), &mut gpu, 1.32);
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
        ("elegance", 1.34, physics::Hover { ride: 0.56, stiff: 38.0, hold: 0.12, field_r: 3.4, field_k: 25.0, base_tank: 40.0, base_regen: 50.0, ..physics::Hover::default() }),
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
        hovercars.push(m);
    }
    // imported bodies (single-file .glb, geometry and materials inline)
    let cars_dir = dir.join("cars");
    let backfire = model::load(&cars_dir.join("backfire"), &mut gpu, 1.28);
    let breakout = model::load(&cars_dir.join("breakout"), &mut gpu, 1.30);
    let assael = model::load(&cars_dir.join("assael"), &mut gpu, 1.34);
    let guardian = model::load(&cars_dir.join("guardian"), &mut gpu, 1.30); // orientation is detected now
    let orion = model::load(&cars_dir.join("orion"), &mut gpu, 1.32);
    let vulcan = model::load(&cars_dir.join("vulcan"), &mut gpu, 1.30);
    let xdevil = model::load(&cars_dir.join("xdevil"), &mut gpu, 1.32);
    let mut cars = CarModels {
        models: vec![
            octane, fennec, dominus, breadbox, wedge, roller, longboy, cube, shark,
            volt, axion, hauler, pulse, tram, vipera, aegis,
            backfire, breakout, assael, guardian, orion, vulcan, xdevil,
            rollout, cryofin, tl4, spearhead, outback,
        ],
        names: vec![
            "Octane", "Fennec", "Dominus", "Breadbox", "Wedge", "Roller", "Longboy", "Cube", "Shark",
            "Volt EV", "Axion EV", "Hauler EV", "Pulse EV", "Tram EV", "Vipera", "Aegis",
            "Backfire", "Breakout", "Assael", "Guardian", "Orion", "Vulcan", "X-Devil",
            "Rollout", "Cryofin", "TL-4", "Spearhead", "Outback",
        ],
        active: 0,
        xform: HashMap::new(),
        chassis: Vec::new(),
        cycle: Vec::new(),
    };
    for (i, m) in hovercars.into_iter().enumerate() {
        cars.models.push(m);
        cars.names.push(["Brutis", "Akimbo", "Matris", "Vectorzoid", "Tempest", "Contingency", "Blackstar", "Elegance", "Trifang", "Echotron", "Hardwall", "Folly", "Lineback", "Whistlewind"][i]);
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
        }
        cars.active = sv.car.min(cars.models.len() - 1);
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
    if bots_dir.is_dir() {
        let mut bots: Vec<String> = std::fs::read_dir(&bots_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        bots.sort();
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
            let flight = bot.starts_with("jet_");
            let label = bot.trim_start_matches("jet_").to_string();
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
                let (_, _, _, s) = model::load_raw_at(p, 1.0, false);
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
                cars.xform.insert(idx, (cid, k));
            }
            println!("bots: {label} — {} poses{}", strip.len(), if flight { ", flight" } else { "" });
            cars.chassis.push(Chassis { strip, dirs, flight });
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
    let world = World::generate(seed, style0, cars.models[cars.active].half_h, cfg.arena.scale);
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
        gpu,
        world,
        car,
        ball,
        cam,
        cars,
        xf: xform::Xform::new(),
        morph: None,
        health: 1.0,
        wep: weapons::Weapons::new(),
        autotarget: false,
        lift: None,
        space: chord::Chord::new(),
        sight: chord::Sight::new(),
        loco: anim::Locomotion::new(),
        loco_pose: anim::Pose::default(),
        ball_model,
        decor_model,
        fx: fx::Fx::new(seed),
        snd,
        pad: gamepad::Pad::new(),
        last_pad: gamepad::PadState::default(),
        save: sv,
        binds,
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
        toast: String::new(),
        toast_t: 0.0,
        mode: Mode::Splash,
        menu_from_pause: false,
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
    };

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
                            // [ and ] cycle cars in singleplayer, matched on
                            // BOTH the logical character and the physical key
                            // position. Dvorak moves the bracket CHARACTERS to
                            // where QWERTY has - and =, so a player who thinks
                            // in characters and a player who thinks in key
                            // positions both get what they reached for.
                            if !event.repeat {
                                use winit::keyboard::{KeyCode, PhysicalKey};
                                let ch = match &key {
                                    Key::Character(c) => c.as_str(),
                                    _ => "",
                                };
                                let phys = event.physical_key;
                                let dir = if ch == "[" || phys == PhysicalKey::Code(KeyCode::BracketLeft) {
                                    -1
                                } else if ch == "]" || phys == PhysicalKey::Code(KeyCode::BracketRight) {
                                    1
                                } else {
                                    0
                                };
                                if dir != 0 {
                                    game.cycle_car(dir);
                                }
                            }
                            if !event.repeat {
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
                    }
                },
                WindowEvent::RedrawRequested => {
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

/// How long a vehicle<->robot fold takes. Blackstar's own `xform_dur` is
/// per-chassis in `content/chassis.json`; nothing carries that across the
/// glTF strip, so the engine uses one duration for every bot until it does.
const XFORM_DUR: f32 = 0.9;

fn apply_car_dims(car: &mut Car, m: &model::Model, size: f32) {
    car.half_len = m.half_len * size;
    car.half_h = (m.half_h * size).max(0.10);
    car.half_w = (m.half_w * size).max(0.12);
    car.wheel_r = (m.wheel_r * size).max(0.05);
    car.drive = m.drive;
    car.hover = m.hover;
}

impl Game {
    /// Sync live settings into the save struct and write it to disk.
    fn persist(&mut self) {
        self.save.volume = self.snd.volume;
        self.save.music = self.snd.music_on;
        self.save.ball_cam = self.cam.ball_cam;
        self.save.car = self.cars.active;
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
        self.save.map = map % world::STYLE_COUNT;
        self.save.seed = seed;
        let style = world::style(self.save.map);
        self.world = World::generate(seed, style, self.cars.models[self.cars.active].half_h, self.cfg.arena.scale);
        self.applied_scale = self.cfg.arena.scale;
        self.tchunks = terrain_chunks(&mut self.gpu, &self.world);
        self.gpu.set_rcells(&bake_rcells(&self.world));
        bake_heightmap(&self.gpu, seed, style);
        let dims_i = self.cars.active;
        self.car.reset(&self.world);
        apply_car_dims(&mut self.car, &self.cars.models[dims_i], self.cfg.car.size);
        self.ball.reset_in_front(&self.car);
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
                let mut spawn = |team: u8, k: usize, brain: bots::Brain, sel: &mut Self| {
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
            Action::BallCam => {
                // three states now, not two. ball_cam is kept in step so the
                // HUD and the save file, which both predate the enum, still
                // mean what they meant.
                // the chassis decides whether a gunner seat exists at all
                let guns = self.gun_mounts();
                self.cam.gun_seat = guns.seat();
                self.cam.mode = self.cam.mode.next(guns);
                self.cam.ball_cam = self.cam.mode == camera::CamMode::Ball;
                if self.cam.mode != camera::CamMode::First {
                    self.cam.look_pitch = self.cam.look_pitch.clamp(-0.55, 1.05);
                }
                let m = self.cam.mode.label();
                self.toastf(m);
            }
            Action::Toss => self.ball.toss(&self.car),
            Action::BallFront => self.ball.reset_in_front(&self.car),
            Action::ResetCar => {
                self.car.reset(&self.world);
                let m = self.cars.active;
                apply_car_dims(&mut self.car, &self.cars.models[m], self.cfg.car.size);
            }
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
            Action::FlipVis => self.flip_vis = !self.flip_vis,
            Action::Transform => self.transform(),
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
            Action::Car1 => self.switch_car(0),
            Action::Car2 => self.switch_car(1),
            Action::Car3 => self.switch_car(2),
            _ => {}
        }
    }

    fn switch_car(&mut self, i: usize) {
        if i < self.cars.models.len() {
            self.cars.active = i;
            apply_car_dims(&mut self.car, &self.cars.models[i], self.cfg.car.size);
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
        self.switch_car(m);
        let d = self.world.sdf(self.car.pos);
        if d < self.car.half_h {
            self.car.pos += self.world.normal(self.car.pos) * (self.car.half_h - d + 0.02);
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
        if self.cars.xform.get(&self.cars.active).is_none() {
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
        let mut poses: Vec<Vec<Vec<meshgen::Vertex>>> = Vec::new();
        let mut idx: Vec<Vec<u32>> = Vec::new();
        for (k, (d, target)) in dirs.iter().enumerate() {
            let (raws, _) = model::load_raw(d, *target);
            if k == 0 {
                idx = raws.iter().map(|r| r.idx.clone()).collect();
            }
            poses.push(raws.into_iter().map(|r| r.verts).collect());
        }
        self.morph = xform::Morph::build(poses, idx);
        if self.morph.is_none() {
            // a strip whose poses disagree cannot be blended; stepping it is
            // still better than blending garbage
            self.toastf("strip mismatch — stepping");
        }
    }

    /// Step the running fold    /// Step the running fold, swapping the body when the strip index changes.
    fn advance_xform(&mut self, dt: f32) {
        self.xf.update(dt);
        if self.xf.state != xform::State::Transforming {
            return;
        }
        let t = self.xf.eased();
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
                let h = m.half_at(t) * self.cfg.car.size;
                self.car.half_w = h.x.max(0.12);
                self.car.half_h = h.y.max(0.10);
                self.car.half_len = h.z.max(0.10);
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
        if d < self.car.half_h {
            self.car.pos += self.world.normal(self.car.pos) * (self.car.half_h - d + 0.02);
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
        self.toastf(&format!("{} ({}/{})", self.cars.names[m], next + 1, n));
    }

    /// Pull the trigger.
    ///
    /// Robot mode only: a car has no arms. Boost is the ammunition, which is
    /// the same tank the thrusters and the second jump draw from — shooting
    /// costs you mobility, which is a decision rather than a resource.
    fn fire_weapon(&mut self) {
        if !self.car.robot || self.mode != Mode::Play {
            return;
        }
        let origin = self.car.pos + self.car.up() * (self.car.half_h * 0.8);
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
        let cost = self.wep.effective().cost;
        let out = self.wep.fire(origin, aim, &targets, self.car.boost);
        if !out.fired {
            return;
        }
        self.car.boost = (self.car.boost - cost).max(0.0);
        self.car.vel += out.self_impulse;
        if let Some(p) = out.teleport {
            self.car.pos = p;
        }
        for e in &out.fx {
            self.fx.ring_burst(e.pos, e.color, e.scale > 1.5);
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
        self.loco_pose = self
            .loco
            .update(dt, horiz, vert, self.car.on_ground, boosting);
        // element movement also belongs here: Air runs, Rock does not
        let _ = self.wep.move_mul();
        self.car.susp = (self.loco_pose.squash * self.loco_pose.weight).clamp(0.0, 1.0);
    }

    /// What this chassis is, for the binding resolver.
    fn chassis_class(&self) -> chord::Class {
        let n = self.cars.names[self.cars.active].to_ascii_lowercase();
        if self.car.flight || n.starts_with("jet") {
            chord::Class::Flyer
        } else if self.car.hover.is_some() || n.contains("hover") {
            chord::Class::Hoverer
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

    fn open_menu(&mut self, m: Mode) {
        self.menu_from_pause = self.mode == Mode::Pause;
        self.mode = m;
        self.mscroll = 0.0;
        self.mfling = 0.0;
    }
    fn close_menu(&mut self) {
        self.mode = if self.menu_from_pause { Mode::Pause } else { Mode::Splash };
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
            Mode::Pause => {
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
                        self.mode = Mode::Online { sel: i };
                        if i >= 2 {
                            self.online_key(&enter, "", i);
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
    /// Keep the selected row inside the viewport after a keyboard move.
    fn follow_selection(&mut self) {
        let (sel, n) = match self.mode {
            Mode::Tweaks { menu, sel } => (sel, tweak_rows(menu).len()),
            Mode::Binds { sel, .. } => (sel, hud::BIND_ROWS),
            Mode::Maps { sel } => (sel, world::STYLE_COUNT),
            _ => return,
        };
        let rh = self.menu_row_h();
        let top = match self.mode {
            Mode::Maps { .. } => hud::MAPS_TOP,
            Mode::Binds { .. } => hud::BIND_TOP,
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
            _ => hud::TW_ROW_H,
        }
    }

    /// How far this menu can scroll, in rows.
    fn menu_max_scroll(&self) -> f32 {
        let rh = self.menu_row_h();
        let top = match self.mode {
            Mode::Maps { .. } => hud::MAPS_TOP,
            Mode::Binds { .. } => hud::BIND_TOP,
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
            if !matches!(self.mode, Mode::Splash | Mode::Play | Mode::Pause) {
                self.nav_to(b, elwt);
                return;
            }
        }
        match self.mode {
            Mode::Splash => self.mode = Mode::Play,
            Mode::Pause => {
                if let Some(b) = hud::hit_pause_btn(self.cursor.0, self.cursor.1) {
                    match b {
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

    /// Escape-equivalent for gestures: leave the current menu.
    fn menu_back(&mut self, _elwt: &winit::event_loop::EventLoopWindowTarget<()>) {
        self.mode = match self.mode {
            Mode::Tweaks { .. } | Mode::Binds { .. } | Mode::Maps { .. } | Mode::Online { .. } => {
                if self.menu_from_pause {
                    Mode::Pause
                } else {
                    Mode::Splash
                }
            }
            Mode::Pause => Mode::Play,
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
            Mode::Online { .. } => hud::hit_row(hud::ONL_TOP, hud::ONL_ROW_H, hud::ONLINE_ROWS, y),
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
            Row::Link(_) | Row::Controls | Row::Trials | Row::Academy | Row::Garage => {
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
        match self.mode {
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
            Mode::Pause => match (&key, ch) {
                (Key::Named(NamedKey::Escape), _) | (Key::Named(NamedKey::Tab), _) => {
                    self.mode = Mode::Play
                }
                (Key::Named(NamedKey::Enter), _) => self.open_menu(Mode::Tweaks { menu: 0, sel: 0 }),
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
                        self.mode = Mode::Pause;
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
                    self.binds = Bindings::classic();
                    self.toastf("PRESET: CLASSIC");
                } else if sel == ACTIONS.len() + 1 {
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

    fn online_key(&mut self, key: &Key, ch: &str, sel: usize) {
        match key {
            Key::Named(NamedKey::Escape) => {
                self.persist();
                self.close_menu();
                return;
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.mode = Mode::Online { sel: (sel + hud::ONLINE_ROWS - 1) % hud::ONLINE_ROWS };
                return;
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.mode = Mode::Online { sel: (sel + 1) % hud::ONLINE_ROWS };
                return;
            }
            Key::Named(NamedKey::Backspace) => {
                match sel {
                    0 => {
                        self.save.name.pop();
                    }
                    1 => {
                        self.friend.pop();
                    }
                    _ => {}
                }
                return;
            }
            Key::Named(NamedKey::Enter) => {
                match sel {
                    2 => {
                        let n = self.save.name.clone();
                        let s = self.net.host(&n);
                        self.toastf(&s);
                    }
                    3 => {
                        let n = self.save.name.clone();
                        let f = self.friend.clone();
                        let s = self.net.join(&n, &f);
                        self.toastf(&s);
                    }
                    4 => {
                        self.net.leave();
                        self.toastf("OFFLINE");
                    }
                    5 => {
                        self.persist();
                        self.close_menu();
                    }
                    _ => {}
                }
                return;
            }
            _ => {}
        }
        // text entry into the selected field
        if (sel == 0 || sel == 1) && !ch.is_empty() {
            let c = ch.chars().next().unwrap_or(' ');
            if c.is_ascii_graphic() {
                let field = if sel == 0 { &mut self.save.name } else { &mut self.friend };
                if field.len() < 16 {
                    field.push(c);
                }
            }
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
        Controls {
            throttle: (fwd - rev + ps.throttle + t_thr).clamp(-1.0, 1.0),
            steer,
            pitch: (rev - fwd - ps.stick_y + t_air.map(|v| -v.y).unwrap_or(0.0)).clamp(-1.0, 1.0),
            yaw: ((-yaw_src).clamp(-1.0, 1.0) + t_air.map(|v| v.x).unwrap_or(0.0) * roll_mix).clamp(-1.0, 1.0),
            roll: ((rl - rr + roll_extra).clamp(-1.0, 1.0) + t_air.map(|v| v.x).unwrap_or(0.0) * (1.0 - roll_mix)).clamp(-1.0, 1.0),
            boost: self.action_down(Action::Boost) || t_boost,
            jump_held: self.action_down(Action::Jump) || t_jump,
            jump_pressed: false,
            jump_depth: self.axis_raw(Action::Jump).max(0.01),
            powerslide: slide_a > 0.35,
        }
    }

    fn frame(&mut self, dt: f32, win: &winit::window::Window) {
        self.time += dt;
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
        self.update_autotarget(dt);
        self.update_locomotion(dt);

        // ---- controller ----
        let ps = self.pad.poll();
        self.last_pad = ps;
        if ps.start_pressed {
            match self.mode {
                Mode::Splash | Mode::Pause => self.mode = Mode::Play,
                Mode::Play => {
                    self.mode = Mode::Pause;
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
                self.mode = Mode::Pause;
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
            self.acc += dt;
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
                physics::step(&mut self.car, &mut self.ball, &mut self.world, &c, &self.cfg, FIXED_DT, &mut ev);
                c.jump_pressed = false;
                self.acc -= FIXED_DT;
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
            self.fx.update(dt);
            let rear = self.held_mouse.contains(&MouseButton::Middle);
            self.cam.update(&self.car, &self.ball, &self.world, &self.cfg.cam, self.car.flipping(), rear, dt);

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
                self.cam.update(&self.car, &self.ball, &self.world, &self.cfg.cam, false, false, dt);
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
            Mode::Pause => {
                self.draw_hud(&mut ui, aspect);
                hud::pause(&mut ui, aspect, &self.save, self.air_t, self.cursor);
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
                        let value = if matches!(r, Row::CarPick) {
                            self.cars.names[self.cars.active].to_uppercase()
                        } else {
                            value
                        };
                        hud::TweakRow { label: (*l).to_string(), value, bar, header: is_header(r), action }
                    })
                    .collect();
                hud::tweaks_menu(&mut ui, aspect, menu_title(menu), &trs, sel, self.mscroll.max(0.0) as usize, self.cursor, nav_here(menu));
            }
            Mode::Online { sel } => {
                let players: Vec<String> = std::iter::once(format!(
                    "{} (YOU)",
                    if self.save.name.is_empty() { "PILOT" } else { &self.save.name }
                ))
                .chain(self.net.peers().map(|p| p.name.clone()))
                .collect();
                let o = hud::OnlineUi {
                    sel,
                    name: &self.save.name,
                    friend: &self.friend,
                    status: if self.toast.is_empty() { &self.net.status } else { &self.toast },
                    players,
                    online: self.net.online(),
                };
                hud::online_menu(&mut ui, aspect, &o, self.time);
            }
        }

        // cinema bars
        if self.cfg.video.letterbox && matches!(self.mode, Mode::Play | Mode::Pause) {
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

        let globals = Globals {
            view_proj: vp.to_cols_array(),
            inv_view_proj: ivp.to_cols_array(),
            cam: [self.cam.pos.x, self.cam.pos.y, self.cam.pos.z, self.time],
            sun: [st.sun[0], st.sun[1], st.sun[2], 0.0],
            fog: [
                st.fog[0].max(wx_flash * 0.92),
                st.fog[1].max(wx_flash * 0.92),
                st.fog[2].max(wx_flash * 0.96),
                st.fog_d * wx_fogd,
            ],
            misc: [st.zenith[0], st.zenith[1], st.zenith[2], 0.0],
            rt: [if self.cfg.video.rays { 1.0 } else { 0.0 }, self.cfg.video.quality, rt_ids.static_n as f32, st.wet.max(wx_wet)],
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
            self.snd.fire(if big { Sfx::PadBig } else { Sfx::PadSmall });
        }
        for &(pos, n, sp) in ev.bounces.iter().take(3) {
            self.fx.dust(pos - n * physics::BALL_R * 0.9, n, sp);
            self.snd.fire(Sfx::Bounce(sp));
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

        for &(_, sp) in ev.touches.iter().take(1) {
            if sp > 0.7 {
                self.snd.fire(Sfx::Touch(sp));
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
            self.snd.fire(Sfx::CarRing);
            self.fx.pad_sparkle(p, false);
            self.toastf("FLIP RESET");
        }
        if let Some(sp) = ev.landed {
            self.snd.fire(Sfx::Land(sp));
            self.cam.add_shake((sp * 0.045).min(0.5));
            if sp > 2.0 {
                let n = self.car.ground_n;
                self.fx.dust(self.car.pos - n * self.car.half_h, n, sp);
            }
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
        self.tick_trial(dt);
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
            scores: if matches!(self.game_mode, 1 | 3..=7) { Some((self.scores[0], self.scores[1])) } else { None },
            minimap: self.save.minimap,
            air_roll: !self.car.on_ground && self.roll_mix_s > 0.5,
            map_name: self.style().name,
            online,
        };
        let bot_dots: Vec<(Vec3, u8)> = self.bots.iter().map(|b| (b.car.pos, b.team)).collect();
        hud::hud(ui, &info, &self.world, &self.car, &self.ball, &bot_dots);
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
                let t = bots::team_tint(b.team);
                self.push_car_proxy(&mut v, m, [t[0] * 0.6, t[1] * 0.6, t[2] * 0.6, 1.0], b.car.pos, b.car.rot, false, &mut push);
            }
        }

        v.truncate(render::MAX_RT_SHAPES);
        (v, RtIds { ball: ball_id, car: car_id, decor0, remote0, static_n })
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
            // tube LOD: far shells swap to the 10-segment mesh (same SDF)
            // LOD: distant primitives swap to low-poly meshes. Spheres and
            // capsules are the most-used shapes in the game (foliage, rocks,
            // pillars, orbs) and the full-fat sphere is ~750 triangles.
            let d2 = s.pos.distance_squared(rc.0);
            let far = d2 > 140.0 * 140.0;
            let vfar = d2 > 90.0 * 90.0;
            let mesh = match (s.mesh, far, vfar) {
                (world::M_SHELL, true, _) => world::M_SHELL_LO,
                (world::M_SHELL_OPEN, true, _) => world::M_SHELL_OPEN_LO,
                (world::M_SPHERE, _, true) => world::M_SPHERE_LO,
                (world::M_CAP_TALL, _, true) => world::M_CAP_TALL_LO,
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

        // the local car
        self.push_car(
            &mut objs,
            self.cars.active,
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
        if let Mode::Garage { .. } = self.mode {
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
            // stand it upright in the world, not in the camera's frame: a
            // car leaning with the camera reads as broken
            self.push_car(
                &mut objs,
                self.cars.active,
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
        for b in &self.bots {
            let d2 = b.car.pos.distance_squared(cam);
            if d2 > 150.0 * 150.0 {
                continue;
            }
            self.push_car(
                &mut objs,
                b.model,
                b.car.pos,
                b.car.rot,
                false,
                b.car.wheel_spin,
                b.car.steer_vis,
                0.0,
                1.0,
                Some(bots::team_tint(b.team)),
                d2 < 60.0 * 60.0, [0.0; 4],);
        }

        // local boost flame
        let boosting = self.car.boost > 0.0 && self.action_down(Action::Boost);
        if boosting && self.mode == Mode::Play {
            let tail = rc.0 - (rc.1 * Vec3::NEG_Z) * (self.car.half_len + 0.12);
            let flick = 0.11 + ((self.time * 31.0).sin() * 0.5 + 0.5) * 0.07;
            let fm = Mat4::from_translation(tail) * Mat4::from_scale(Vec3::splat(flick));
            push(&mut objs, world::M_SPHERE, None, fm, [1.0, 0.55, 0.08, 0.9], [0.0, 1.0, 0.0, 0.0]);
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
        for prim in active.prims.iter() {
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
