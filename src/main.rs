//! SKYDRIBBLE — a Rocket-League-inspired open-world air-dribble playground.
//! Rust + wgpu 22 + winit 0.29 (the same stack as the reference repo's games).

mod analog;
mod bots;
mod camera;
mod config;
mod font;
mod fx;
mod gamepad;
mod gyro;
mod hud;
mod input;
mod meshgen;
mod model;
mod net;
mod physics;
mod platform;
mod procars;
mod render;
mod style;
mod save;
mod sim;
mod sound;
mod touch;
mod world;

use std::collections::HashSet;
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
            ("FIELD OF VIEW", Slider { get: |c| c.cam.fov_deg, set: |c, v| c.cam.fov_deg = v, min: 50.0, max: 115.0, step: 1.0, def: 76.0, fmt: 2 }),
            ("DISTANCE", Slider { get: |c| c.cam.dist, set: |c, v| c.cam.dist = v, min: 2.5, max: 9.0, step: 0.1, def: 4.9, fmt: 1 }),
            ("HEIGHT", Slider { get: |c| c.cam.height, set: |c, v| c.cam.height = v, min: 0.4, max: 3.2, step: 0.05, def: 1.35, fmt: 1 }),
            ("ANGLE", Slider { get: |c| c.cam.angle_deg, set: |c, v| c.cam.angle_deg = v, min: -15.0, max: 15.0, step: 1.0, def: -3.0, fmt: 2 }),
            ("STIFFNESS", pct(|c| c.cam.stiffness, |c, v| c.cam.stiffness = v, 0.25, 2.2)),
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
            ("WORLD SIZE (REBUILDS)", pct(|c| c.arena.scale, |c, v| c.arena.scale = v, 0.55, 1.6)),
            ("BALL", Header),
            ("BALL SIZE", pct(|c| c.ball.size, |c, v| c.ball.size = v, 0.35, 3.0)),
            ("BALL BOUNCINESS", pct(|c| c.ball.bounce, |c, v| c.ball.bounce = v, 0.2, 2.0)),
            ("BALL FRICTION", pct(|c| c.ball.friction, |c, v| c.ball.friction = v, 0.0, 2.5)),
            ("BALL WEIGHT", pct(|c| c.ball.weight, |c, v| c.ball.weight = v, 0.25, 3.0)),
            ("BACK", Back),
        ],
        3 => vec![
            ("CHASSIS", Header),
            ("CAR", CarPick),
            ("CAR SIZE", pct(|c| c.car.size, |c, v| c.car.size = v, 0.55, 2.0)),
            ("TIRE GRIP", pct(|c| c.car.grip, |c, v| c.car.grip = v, 0.3, 2.5)),
            ("POWER", Header),
            ("TOP SPEED", pct(|c| c.car.top_speed, |c, v| c.car.top_speed = v, 0.5, 1.9)),
            ("ACCELERATION", pct(|c| c.car.accel, |c, v| c.car.accel = v, 0.4, 2.6)),
            ("JUMP STRENGTH", pct(|c| c.car.jump, |c, v| c.car.jump = v, 0.5, 2.2)),
            ("BOOST", Header),
            ("BOOST POWER", pct(|c| c.car.boost, |c, v| c.car.boost = v, 0.4, 2.6)),
            ("BOOST REGEN /S", Slider { get: |c| c.car.boost_regen, set: |c, v| c.car.boost_regen = v, min: 0.0, max: 35.0, step: 1.0, def: 0.0, fmt: 1 }),
            ("BOOST TANK", pct(|c| c.car.boost_max, |c, v| c.car.boost_max = v, 0.3, 2.5)),
            ("UNLIMITED BOOST", Toggle { get: |c| c.car.unlimited_boost, set: |c, v| c.car.unlimited_boost = v }),
            ("INPUT", Header),
            ("ANALOG KEYS (WOOTING)", Toggle { get: |c| c.car.analog, set: |c, v| c.car.analog = v }),
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
        Row::Link(_) | Row::Controls | Row::GameMode(_) => (String::new(), None),
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

struct CarModels {
    models: Vec<model::Model>,
    names: Vec<&'static str>,
    active: usize,
}

struct Game {
    gpu: render::Gpu,
    world: World,
    car: Car,
    ball: Ball,
    cam: camera::Camera,
    cars: CarModels,
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
                if sh.cut {
                    continue;
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
        gpu.add_mesh(&meshgen::tube(28)),
        gpu.add_mesh(&meshgen::tube(10)), // M_SHELL_LO: far-field tube
        gpu.add_mesh(&meshgen::tube_open(28)), // M_SHELL_OPEN: no cap rings
        gpu.add_mesh(&meshgen::tube_open(10)), // M_SHELL_OPEN_LO
        gpu.add_mesh(&meshgen::torus_tube(64, 20)), // M_TORUS_TUBE: seamless loop
        gpu.add_mesh(&meshgen::ring_deck(72)), // M_RING_DECK: drivable ring
        gpu.add_mesh(&meshgen::hex_prism()),   // M_HEX: dropshot tiles
    ];
    assert_eq!(ids[world::M_TERRAIN], world::M_TERRAIN);
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
    let mut cars = CarModels {
        models: vec![octane, fennec, dominus, breadbox, wedge, roller, longboy, cube, shark],
        names: vec!["Octane", "Fennec", "Dominus", "Breadbox", "Wedge", "Roller", "Longboy", "Cube", "Shark"],
        active: sv.car.min(8),
    };
    gpu.set_car_sdfs(&bake_car_sdfs(&mut cars.models));
    let cars = cars;


    let mut car = Car::new();
    apply_car_dims(&mut car, &cars.models[cars.active], cfg.car.size);
    let world = World::generate(seed, style0, cars.models[cars.active].half_h, cfg.arena.scale);
    gpu.replace_mesh(world::M_TERRAIN, &terrain_mesh(&world));
    gpu.set_rcells(&bake_rcells(&world));
    car.reset(&world);
    let mut ball = Ball::new();
    ball.reset_in_front(&car);

    let mut cam = camera::Camera::new();
    cam.ball_cam = sv.ball_cam;
    let snd = sound::Sound::new(sv.volume, sv.music, opts.mute);

    let (pv_cp, pv_cr, pv_bp, pv_br) = (car.pos, car.rot, ball.pos, ball.rot);
    let vs0 = cfg.video.vsync;
    let rs0 = cfg.video.render_scale;
    let gyro_on0 = cfg.gyro.on;
    let mut game = Game {
        gpu,
        world,
        car,
        ball,
        cam,
        cars,
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
                            if !event.repeat {
                                game.key_down(key.clone(), elwt, &win);
                            } else if let Mode::Online { .. } = game.mode {
                                game.key_down(key.clone(), elwt, &win); // key repeat in text fields
                            }
                            game.held.insert(key);
                        }
                        ElementState::Released => {
                            game.held.remove(&key);
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

fn apply_car_dims(car: &mut Car, m: &model::Model, size: f32) {
    car.half_len = m.half_len * size;
    car.half_h = (m.half_h * size).max(0.10);
    car.half_w = (m.half_w * size).max(0.12);
    car.wheel_r = (m.wheel_r * size).max(0.05);
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
        self.gpu.replace_mesh(world::M_TERRAIN, &terrain_mesh(&self.world));
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
            Action::BallCam => self.cam.ball_cam = !self.cam.ball_cam,
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
            Row::Link(_) | Row::Controls => {
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
                        self.mode = Mode::Pause;
                        self.persist(); // checkpoint whenever the player pauses
                        return;
                    }
                    _ => {}
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
        if dt > 1e-5 {
            self.fps += (1.0 / dt - self.fps) * 0.06;
        }
        self.toast_t = (self.toast_t - dt).max(0.0);
        if self.toast_t <= 0.0 {
            self.toast.clear();
        }

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
                self.fx.boost_trail(tail, back, self.car.vel);
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
        let mut objs = self.build_scene(&rt_ids, rc, rb);
        self.fx.draw(&mut objs);

        let aspect = self.gpu.aspect();
        let (vp, ivp) = self.cam.view_proj(&self.cfg.cam, aspect);
        self.last_ivp = ivp;

        let mut ui: Vec<UiVert> = Vec::new();
        self.peer_tags(&mut ui, vp, aspect);
        match self.mode {
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
        if ev.dodged {
            self.snd.fire(Sfx::Dodge);
            self.fx.dust(self.car.pos, self.car.up(), 3.0);
        }
        for &(gp, who) in &ev.goals {
            self.cam.add_shake(0.45);
            self.fx.ring_burst(gp, [0.45, 1.0, 0.55, 1.0], true);
            self.fx.pad_sparkle(gp, true);
            if who == physics::WHO_PLAYER {
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
            car_name: self.cars.names[self.cars.active],
            ball_cam: self.cam.ball_cam,
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

        for s in &self.world.shapes {
            if s.cut {
                continue; // CSG cutters carve the SDF; they are not drawn
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

    fn build_scene(&self, ids: &RtIds, rc: (Vec3, Quat), rb: (Vec3, Quat)) -> Vec<ObjDraw> {
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

        // terrain
        push(
            &mut objs,
            world::M_TERRAIN,
            None,
            Mat4::IDENTITY,
            self.style().grass,
            [1.0, 0.0, 0.04, 0.0],
        );

        // playful shapes
        for s in &self.world.shapes {
            if s.cut {
                continue; // CSG cutters carve the SDF; they are not drawn
            }
            let em = (s.emissive + s.flash * 1.1).min(1.0);
            // small far shapes don't earn their draw call (monkey run is
            // 775 instances otherwise, hoops and vines included)
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
            let far = s.pos.distance_squared(rc.0) > 140.0 * 140.0;
            let mesh = match (s.mesh, far) {
                (world::M_SHELL, true) => world::M_SHELL_LO,
                (world::M_SHELL_OPEN, true) => world::M_SHELL_OPEN_LO,
                _ => s.mesh,
            };
            push(&mut objs, mesh, None, s.mat, s.color, [0.0, em, s.spec, 0.0]);
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
        );
        // remote cars + their boost flames
        for (i, p) in self.net.peers().enumerate() {
            self.push_car(&mut objs, p.car.min(self.cars.models.len() - 1), p.shown, p.rot, p.flip, p.wheel_spin, p.steer_vis,
                (ids.remote0 + i + 1) as f32, 1.0, None, true);
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
                d2 < 60.0 * 60.0,
            );
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
    ) {
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
            if let Some(t) = tint {
                color = [color[0] * t[0], color[1] * t[1], color[2] * t[2], color[3]];
            }
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
            push(objs, prim.mesh_id, prim.tex_id, m, color, [0.0, 0.0, spec, rt_slot]);
        }
    }
}
