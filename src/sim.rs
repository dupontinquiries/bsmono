//! Headless simulation + scoring for the bot AI. Runs the real physics at a
//! fixed 120 Hz with no window, no GPU, no audio — which means the striker's
//! air-dribble machine can be tuned by numbers instead of vibes.
//!
//!     skydribble --sim air1            one striker, empty net
//!     skydribble --sim pass2|pass3|pass4   pass drill with N strikers
//!     skydribble --sim defend1         striker vs a defender
//!     skydribble --sim all             everything, summary table
//!     (append a number for trials, default 6)
//!
//! Scoring is style-first, per the design brief: airborne touches and flip
//! resets outweigh raw time-to-goal, passes score in the team drills, and the
//! defender drill also rewards making the defender run (distance traveled).

use glam::Vec3;

use crate::bots::{self, Bot, Brain};
use crate::config::Config;
use crate::physics::{self, Ball, StepEvents};
use crate::world::World;

const DT: f32 = 1.0 / 120.0;
const ARENA: usize = 20; // ARENA CLASSIC: flat pitch, paired goals

pub struct Outcome {
    pub goal_t: Option<f32>,
    pub air_touches: u32,
    pub resets: u32,
    pub passes: u32,
    pub max_ball_h: f32,
    pub defender_travel: f32,
    pub style: f32,
}

fn style_score(o: &Outcome) -> f32 {
    let goal = if o.goal_t.is_some() { 30.0 } else { 0.0 };
    let speed = o.goal_t.map(|t| (28.0 - t).max(0.0) * 0.5).unwrap_or(0.0);
    2.0 * o.air_touches.min(25) as f32
        + 10.0 * o.resets.min(3) as f32
        + 4.0 * o.passes as f32
        + 0.8 * o.max_ball_h.min(14.0)
        + 0.03 * o.defender_travel
        + goal
        + speed
}

struct Rig {
    world: World,
    ball: Ball,
    bots: Vec<Bot>,
    cfg: Config,
    goal: Vec3,
}

fn rig(seed: u64, n_strikers: usize, defender: bool) -> Rig {
    let cfg = Config::default();
    let world = World::generate(seed, crate::world::style(ARENA), 0.31, cfg.arena.scale);
    // attack the +z goal; ball starts mid-pitch rolling goalward
    let goal = world
        .nets
        .iter()
        .map(|n| n.pos)
        .max_by(|a, b| a.z.total_cmp(&b.z))
        .unwrap_or(Vec3::new(0.0, 2.0, 56.0));
    let mut ball = Ball::new();
    ball.pos = Vec3::new(0.0, ball.r + 0.02, -6.0);
    ball.vel = Vec3::new(0.0, 0.0, 7.5);
    let mut bots = Vec::new();
    for i in 0..n_strikers {
        let x = (i as f32 - (n_strikers as f32 - 1.0) * 0.5) * 5.0;
        let brain = if n_strikers > 1 {
            Brain::Passer { next: ((i + 1) % n_strikers) as u8 }
        } else {
            Brain::Striker
        };
        bots.push(bots::mk_bot_brain(Vec3::new(x, 0.4, -14.0), 0.0, i as u8, i, brain));
    }
    if defender {
        let home = goal;
        bots.push(bots::mk_bot_brain(
            Vec3::new(0.0, 0.4, goal.z - 8.0),
            std::f32::consts::PI,
            9,
            9,
            Brain::Defender { home },
        ));
    }
    Rig { world, ball, bots, cfg, goal }
}

fn run_one(seed: u64, n_strikers: usize, defender: bool, secs: f32) -> Outcome {
    let mut r = rig(seed, n_strikers, defender);
    let mut o = Outcome {
        goal_t: None,
        air_touches: 0,
        resets: 0,
        passes: 0,
        max_ball_h: 0.0,
        defender_travel: 0.0,
        style: 0.0,
    };
    let mut t = 0.0f32;
    let mut last_toucher: u8 = physics::WHO_NONE;
    let mut def_prev = r.bots.last().map(|b| b.car.pos).unwrap_or(Vec3::ZERO);
    let mut resets_seen = 0u32;
    while t < secs {
        // brains + car physics + touches (bots::update also shoves the crowd,
        // but we bypass the player by handing it a parked dummy car)
        let mut dummy = physics::Car::new();
        dummy.pos = Vec3::new(0.0, -50.0, -300.0); // far out of play
        bots::update(&mut r.bots, &mut r.ball, &mut dummy, &r.world, &r.cfg, t, DT);

        // pass detection: consecutive touches by different striker bots
        let cur = r.ball.last_touch;
        if cur < 8 && cur != last_toucher {
            if last_toucher < 8 {
                o.passes += 1;
            }
            last_toucher = cur;
        }

        // ball vs world + goal check
        let mut ev = StepEvents::default();
        let prev = r.ball.pos;
        physics::step_ball(&mut r.ball, &r.world, &r.cfg, DT, &mut ev);
        physics::ball_goal_check(&r.ball, prev, &mut r.world, &mut ev);
        for i in 0..r.world.nets.len() {
            r.world.nets[i].timer = (r.world.nets[i].timer - DT).max(0.0);
        }
        if !ev.goals.is_empty() && o.goal_t.is_none() {
            o.goal_t = Some(t);
        }

        // metrics
        o.max_ball_h = o.max_ball_h.max(r.ball.pos.y);
        let reset_now: u32 = r.bots.iter().filter(|b| b.team < 8).map(|b| b.got_reset as u32).sum();
        if reset_now > resets_seen {
            o.resets += reset_now - resets_seen;
            resets_seen = reset_now;
        }
        if defender {
            if let Some(d) = r.bots.last() {
                o.defender_travel += (d.car.pos - def_prev).length().min(1.0);
                def_prev = d.car.pos;
            }
        }
        if std::env::var("SIM_DEBUG").is_ok() && (t * 2.0).fract() < DT * 2.0 {
            let b0 = &r.bots[0];
            eprintln!(
                "t={:5.1} phase={:?} car=({:6.1},{:5.1},{:6.1}) og={} ball=({:6.1},{:5.1},{:6.1}) bv.y={:5.1} d={:4.1}",
                t, b0.phase, b0.car.pos.x, b0.car.pos.y, b0.car.pos.z, b0.car.on_ground as u8,
                r.ball.pos.x, r.ball.pos.y, r.ball.pos.z, r.ball.vel.y,
                (r.ball.pos - b0.car.pos).length()
            );
        }
        t += DT;
        if o.goal_t.is_some() {
            break;
        }
    }
    o.air_touches = r.bots.iter().filter(|b| b.team < 8).map(|b| b.touches_air).sum();
    // sanity clamp: a striker that never leaves the ground shouldn't farm
    // "passes" by nudging the ball back and forth on the floor
    if n_strikers == 1 {
        o.passes = 0;
    }
    let _ = r.goal;
    o.style = style_score(&o);
    o
}

fn scenario(name: &str) -> Option<(usize, bool)> {
    Some(match name {
        "air1" => (1, false),
        "pass2" => (2, false),
        "pass3" => (3, false),
        "pass4" => (4, false),
        "defend1" => (1, true),
        _ => return None,
    })
}

/// Drop the car in a pose, time three moments: first driving contact,
/// jump/flip restored, upright+grounded. The inconsistency finder.
fn land_one(pose: &str, rot: glam::Quat, ang: Vec3, assist: f32, h: f32) -> (f32, f32, f32) {
    let mut cfg = Config::default();
    cfg.flip.land_assist = assist;
    let world = World::generate(77, crate::world::style(ARENA), 0.31, cfg.arena.scale);
    let mut ball = Ball::new();
    ball.pos = Vec3::new(200.0, 1.0, 200.0); // far away: pure landing test
    let mut car = physics::Car::new();
    car.pos = Vec3::new(6.0, world.ground_h(6.0, 6.0) + h, 6.0);
    car.rot = rot;
    car.ang = ang;
    car.vel = Vec3::new(0.0, -2.0, 0.0);
    let c = physics::Controls::default();
    let (mut t_contact, mut t_flip, mut t_up) = (f32::NAN, f32::NAN, f32::NAN);
    let mut t = 0.0f32;
    let mut ev = StepEvents::default();
    // LAND_DEBUG=<pose> traces that pose frame-by-frame (e.g. =tumble)
    let dbg = std::env::var("LAND_DEBUG").map(|v| v == pose || v == "1" && pose == "roof").unwrap_or(false);
    while t < 4.0 {
        physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
        if dbg && (t * 4.0).fract() < DT * 4.0 {
            eprintln!(
                "t={:4.2} y={:5.2} d={:5.2} up.y={:5.2} ang=({:5.2},{:5.2},{:5.2}) og={}",
                t, car.pos.y, world.sdf(car.pos), car.up().y, car.ang.x, car.ang.y, car.ang.z, car.on_ground as u8
            );
        }
        ev.goals.clear();
        if t_contact.is_nan() && car.on_ground {
            t_contact = t;
        }
        if t_flip.is_nan() && car.has_flip() {
            t_flip = t;
        }
        if t_up.is_nan() && car.on_ground && car.up().y > 0.95 {
            t_up = t;
        }
        t += DT;
    }
    let _ = pose;
    (t_contact, t_flip, t_up)
}

fn land_matrix() {
    use glam::Quat;
    let poses: [(&str, Quat, Vec3); 9] = [
        ("flat", Quat::IDENTITY, Vec3::ZERO),
        ("nose45", Quat::from_rotation_x(-0.785), Vec3::ZERO),
        ("nose90", Quat::from_rotation_x(-1.5707), Vec3::ZERO),
        ("tail45", Quat::from_rotation_x(0.785), Vec3::ZERO),
        ("side45", Quat::from_rotation_z(0.785), Vec3::ZERO),
        ("side90", Quat::from_rotation_z(1.5707), Vec3::ZERO),
        ("roof", Quat::from_rotation_z(3.1415), Vec3::ZERO),
        ("tumble", Quat::from_rotation_x(0.6), Vec3::new(5.0, 0.0, 2.0)),
        ("spin", Quat::from_rotation_z(0.5), Vec3::new(0.0, 4.0, 4.0)),
    ];
    for (assist, h, label) in [(1.0f32, 5.0f32, "assist=1.0 (opt-in) drop=5m"), (0.0, 5.0, "assist=OFF (DEFAULT) drop=5m"), (0.0, 2.2, "assist=OFF (DEFAULT) drop=2.2m")] {
        println!("-- {label}");
        println!("{:<8} {:>9} {:>9} {:>9}", "pose", "contact", "flipback", "upright");
        for (name, q, a) in poses {
            let (tc, tf, tu) = land_one(name, q, a, assist, h);
            let f = |x: f32| if x.is_nan() { "  NEVER".to_string() } else { format!("{x:7.2}s") };
            println!("{:<8} {:>9} {:>9} {:>9}", name, f(tc), f(tf), f(tu));
        }
    }
}

pub fn run(args: &[String]) {
    let i = args.iter().position(|a| a == "--sim").unwrap();
    let name = args.get(i + 1).map(String::as_str).unwrap_or("air1");
    let trials: u64 = args.get(i + 2).and_then(|s| s.parse().ok()).unwrap_or(6);
    if name == "genall" {
        // generate every style headless: worldgen panic + stats smoke test
        for si in 0..crate::world::STYLE_COUNT {
            let st = crate::world::style(si);
            let w = World::generate(9, st, 0.31, 1.0);
            let rc = crate::bake_rcells(&w);
            let cells = rc.len() / 32;
            let avg: f32 = (0..cells).map(|c| rc[c * 32] as f32).sum::<f32>() / cells as f32;
            println!(
                "{:>2} {:<12} shapes={:<4} grav={:<3} anims={:<3} zones={} launchers={} routes={} rcell_avg={:.1}",
                si, st.name, w.shapes.len(), w.grav.len(), w.anims.len(), w.zones.len(), w.launchers.len(), w.routes.len(), avg
            );
        }
        return;
    }
    if name == "plat" {
        // platform detection + the handheld preset, verifiable headlessly
        let d = crate::platform::detect();
        println!("detected: {:?}  ({})", d.profile, d.why);
        // the exact target: Fairphone 5 / postmarketOS / SC7280
        let fp5 = crate::platform::classify(
            "Linux version 7.1.2 #1-postmarketos-qcom-sc7280 SMP PREEMPT Mon Jul 13 21:02:06 UTC",
            "fp5-black",
            "Fairphone 5",
            true,
        );
        println!("fp5-black        : {:?}  ({})", fp5.profile, fp5.why);
        let pc = crate::platform::classify("Linux version 6.8.0-generic #1 SMP", "workstation", "", false);
        println!("desktop x86      : {:?}  ({})", pc.profile, pc.why);
        let target_ok = fp5.profile == crate::platform::Profile::Handheld
            && pc.profile == crate::platform::Profile::Desktop;
        let mut cfg = crate::config::Config::default();
        println!(
            "desktop defaults : quality={:.2} rays={} render_scale={:.2} touch={}",
            cfg.video.quality, cfg.video.rays, cfg.video.render_scale, cfg.touch.enabled
        );
        crate::platform::Profile::Handheld.apply(&mut cfg);
        println!(
            "handheld applied : quality={:.2} rays={} render_scale={:.2} touch={} vsync={}",
            cfg.video.quality, cfg.video.rays, cfg.video.render_scale, cfg.touch.enabled, cfg.video.vsync
        );
        // the on-screen pad: stick math, buttons, flick-to-dodge, tap-to-move
        let mut pad = crate::touch::TouchPad::new();
        pad.enabled = true;
        pad.down(1, glam::Vec2::new(0.18, 0.80));
        pad.moved(1, glam::Vec2::new(0.18 - 0.09, 0.80 - 0.09)); // up-left
        pad.tick(1.0 / 120.0);
        let sv = pad.stick_vec();
        println!("stick up-left    : steer={:+.2} throttle={:+.2}", sv.x, sv.y);
        let stick_ok = sv.x < -0.3 && sv.y > 0.3;
        pad.up(1);
        let b = crate::touch::buttons();
        pad.down(2, b.boost);
        pad.down(3, b.jump);
        pad.tick(1.0 / 120.0);
        println!("buttons          : boost={} jump={} jump_edge={}", pad.boost_down(), pad.jump_down(), pad.jump_edge);
        let btn_ok = pad.boost_down() && pad.jump_down() && pad.jump_edge;
        pad.up(2);
        pad.up(3);
        pad.jump_edge = false;
        // a flick in the open area: swipe right -> dodge right
        let mut t = 0.0f64;
        pad.down(4, glam::Vec2::new(0.60, 0.45));
        for k in 1..=6 {
            pad.moved(4, glam::Vec2::new(0.60 + k as f32 * 0.03, 0.45));
            pad.tick(1.0 / 240.0);
            t += 1.0;
        }
        pad.up(4);
        pad.tick(1.0 / 240.0);
        let _ = t;
        let flick = pad.flick;
        println!("swipe right      : flick={:?} jump_edge={}", flick.map(|f| (f.x, f.y)), pad.jump_edge);
        let flick_ok = flick.map(|f| f.x > 0.5).unwrap_or(false) && pad.jump_edge;
        // tap-to-move follow
        pad.target = Some(Vec3::new(30.0, 0.0, 0.0));
        let (thr, steer) = pad.follow(Vec3::ZERO, Vec3::Z);
        println!("drive-to (+X)    : throttle={thr:+.2} steer={steer:+.2}");
        let follow_ok = thr > 0.5 && steer.abs() > 0.5;
        println!(
            "{}",
            if target_ok && stick_ok && btn_ok && flick_ok && follow_ok { "PLATFORM+TOUCH: PASS" } else { "PLATFORM+TOUCH: FAIL" }
        );
        return;
    }
    if name == "plat" {
        // platform detection + the handheld preset, verifiable headlessly
        let d = crate::platform::detect();
        println!("detected: {:?}  ({})", d.profile, d.why);
        // the exact target: Fairphone 5 / postmarketOS / SC7280
        let fp5 = crate::platform::classify(
            "Linux version 7.1.2 #1-postmarketos-qcom-sc7280 SMP PREEMPT Mon Jul 13 21:02:06 UTC",
            "fp5-black",
            "Fairphone 5",
            true,
        );
        println!("fp5-black        : {:?}  ({})", fp5.profile, fp5.why);
        let pc = crate::platform::classify("Linux version 6.8.0-generic #1 SMP", "workstation", "", false);
        println!("desktop x86      : {:?}  ({})", pc.profile, pc.why);
        let target_ok = fp5.profile == crate::platform::Profile::Handheld
            && pc.profile == crate::platform::Profile::Desktop;
        let mut cfg = crate::config::Config::default();
        println!(
            "desktop defaults : quality={:.2} rays={} render_scale={:.2} touch={}",
            cfg.video.quality, cfg.video.rays, cfg.video.render_scale, cfg.touch.enabled
        );
        crate::platform::Profile::Handheld.apply(&mut cfg);
        println!(
            "handheld applied : quality={:.2} rays={} render_scale={:.2} touch={} vsync={}",
            cfg.video.quality, cfg.video.rays, cfg.video.render_scale, cfg.touch.enabled, cfg.video.vsync
        );
        // the on-screen pad: stick math, buttons, flick-to-dodge, tap-to-move
        let mut pad = crate::touch::TouchPad::new();
        pad.enabled = true;
        pad.down(1, glam::Vec2::new(0.18, 0.80));
        pad.moved(1, glam::Vec2::new(0.18 - 0.09, 0.80 - 0.09)); // up-left
        pad.tick(1.0 / 120.0);
        let sv = pad.stick_vec();
        println!("stick up-left    : steer={:+.2} throttle={:+.2}", sv.x, sv.y);
        let stick_ok = sv.x < -0.3 && sv.y > 0.3;
        pad.up(1);
        let b = crate::touch::buttons();
        pad.down(2, b.boost);
        pad.down(3, b.jump);
        pad.tick(1.0 / 120.0);
        println!("buttons          : boost={} jump={} jump_edge={}", pad.boost_down(), pad.jump_down(), pad.jump_edge);
        let btn_ok = pad.boost_down() && pad.jump_down() && pad.jump_edge;
        pad.up(2);
        pad.up(3);
        pad.jump_edge = false;
        // a flick in the open area: swipe right -> dodge right
        let mut t = 0.0f64;
        pad.down(4, glam::Vec2::new(0.60, 0.45));
        for k in 1..=6 {
            pad.moved(4, glam::Vec2::new(0.60 + k as f32 * 0.03, 0.45));
            pad.tick(1.0 / 240.0);
            t += 1.0;
        }
        pad.up(4);
        pad.tick(1.0 / 240.0);
        let _ = t;
        let flick = pad.flick;
        println!("swipe right      : flick={:?} jump_edge={}", flick.map(|f| (f.x, f.y)), pad.jump_edge);
        let flick_ok = flick.map(|f| f.x > 0.5).unwrap_or(false) && pad.jump_edge;
        // tap-to-move follow
        pad.target = Some(Vec3::new(30.0, 0.0, 0.0));
        let (thr, steer) = pad.follow(Vec3::ZERO, Vec3::Z);
        println!("drive-to (+X)    : throttle={thr:+.2} steer={steer:+.2}");
        let follow_ok = thr > 0.5 && steer.abs() > 0.5;
        println!(
            "{}",
            if target_ok && stick_ok && btn_ok && flick_ok && follow_ok { "PLATFORM+TOUCH: PASS" } else { "PLATFORM+TOUCH: FAIL" }
        );
        return;
    }
    if name == "carsdf" {
        // Does the baked reflection field actually resemble a CAR? Sample
        // the atlas the shader samples and compare its silhouette with the
        // model's real dimensions: a blob reads round, a car reads long.
        use crate::render::{CSDF_H, CSDF_SLAB, CSDF_W};
        // a stand-in model with known proportions (length >> width > height)
        let mut m = crate::model::Model {
            prims: Vec::new(),
            cloud: Vec::new(),
            cloud_n: Vec::new(),
            sdf_half: Vec3::ZERO,
            half_len: 0.64,
            half_h: 0.31,
            half_w: 0.32,
            wheel_r: 0.16,
        };
        // a box-ish shell of points, the shape a car actually is
        for i in 0..40 {
            for j in 0..40 {
                let (u, v) = (i as f32 / 39.0 * 2.0 - 1.0, j as f32 / 39.0 * 2.0 - 1.0);
                for (p, n) in [
                    (Vec3::new(u * 0.32, 0.31, v * 0.64), Vec3::Y),
                    (Vec3::new(u * 0.32, -0.31, v * 0.64), Vec3::NEG_Y),
                    (Vec3::new(0.32, u * 0.31, v * 0.64), Vec3::X),
                    (Vec3::new(-0.32, u * 0.31, v * 0.64), Vec3::NEG_X),
                    (Vec3::new(u * 0.32, v * 0.31, 0.64), Vec3::Z),
                    (Vec3::new(u * 0.32, v * 0.31, -0.64), Vec3::NEG_Z),
                ] {
                    m.cloud.push(p);
                    m.cloud_n.push(n);
                }
            }
        }
        let mut models = [m];
        let atlas = crate::bake_car_sdfs(&mut models);
        let h = models[0].sdf_half;
        let dec = |x: usize, y: usize, z: usize| -> f32 {
            let i = ((z as u32 * CSDF_H + y as u32) * CSDF_W + x as u32) as usize;
            (atlas[i] as f32 / 255.0) * 2.4 - 0.6
        };
        // walk out from the centre along each axis: where does solid end?
        let extent = |axis: usize| -> f32 {
            let (cx, cy, cz) = (CSDF_W as usize / 2, CSDF_H as usize / 2, CSDF_SLAB as usize / 2);
            let n = match axis {
                0 => CSDF_W as usize,
                1 => CSDF_H as usize,
                _ => CSDF_SLAB as usize,
            };
            let mut last = 0usize;
            for k in (n / 2)..n {
                let d = match axis {
                    0 => dec(k, cy, cz),
                    1 => dec(cx, k, cz),
                    _ => dec(cx, cy, k),
                };
                if d <= 0.0 {
                    last = k;
                }
            }
            let frac = (last as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            frac * match axis {
                0 => h.x,
                1 => h.y,
                _ => h.z,
            }
        };
        let (ex, ey, ez) = (extent(0), extent(1), extent(2));
        println!("baked half-extents: x {ex:.2}  y {ey:.2}  z {ez:.2}  (model 0.32 / 0.31 / 0.64)");
        let long_enough = ez > ex * 1.5;
        let flat_enough = ey < ez * 0.75;
        let tight = (ex - 0.32).abs() < 0.10 && (ez - 0.64).abs() < 0.12;
        println!("{:<34} {}", "longer than wide (not a sphere)", if long_enough { "ok" } else { "FAIL" });
        println!("{:<34} {}", "flatter than long", if flat_enough { "ok" } else { "FAIL" });
        println!("{:<34} {}", "matches the model within 10 cm", if tight { "ok" } else { "FAIL" });
        println!("{}", if long_enough && flat_enough && tight { "CAR SDF: PASS" } else { "CAR SDF: FAIL" });
        return;
    }
    if name == "dribble" {
        // RL dribbling is a DIRECT mapping: steer in, yaw out, nothing else
        // rotating. This measures the mapping fidelity and any rotation the
        // stick did not ask for, with the ball riding the hood.
        let cfg = Config::default();
        let world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
        println!("{:<16} {:>9} {:>9} {:>9} {:>8} {:>6}", "case", "yaw_cmd", "yaw_out", "stray", "carry", "gnd");
        for (label, steer, bump) in [
            ("straight", 0.0f32, false),
            ("steady turn", 0.6, false),
            ("hard turn", 1.0, false),
            ("turn after land", 0.6, true),
        ] {
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
            let mut ball = Ball::new();
            let mut c = physics::Controls::default();
            c.throttle = 1.0;
            let mut ev = StepEvents::default();
            let mut world2 = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            // get rolling, then set the ball on the hood
            for _ in 0..90 {
                physics::step_car_only(&mut car, &ball, &world2, &c, &cfg, DT);
            }
            if bump {
                // land from a small hop with leftover tumble, as you would
                // after any aerial touch
                car.pos += Vec3::Y * 1.2;
                car.ang = Vec3::new(1.6, 0.0, -1.2);
                // let the landing FINISH: the recovery is measured by the
                // landing matrix, this case is about dribbling once down
                for _ in 0..150 {
                    physics::step_car_only(&mut car, &ball, &world2, &c, &cfg, DT);
                }
            }
            ball.pos = car.pos + car.forward() * 0.55 + car.up() * (car.half_h + ball.r + 0.02);
            ball.vel = car.vel;
            ball.ang = Vec3::ZERO;
            c.steer = steer;
            let mut stray = 0.0f32;
            let mut yaw_out = 0.0f32;
            let mut carried = 0;
            let mut grounded_frames = 0;
            let mut frame = 0usize;
            let mut prev_og = true;
            let f0 = car.forward();
            let mut prev_f = Vec3::new(f0.x, 0.0, f0.z).normalize_or(Vec3::Z);
            let steps = 240;
            for _ in 0..steps {
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut world2, &c, &cfg, DT, &mut ev);
                // yaw actually delivered, per second
                let f = car.forward();
                let fh = Vec3::new(f.x, 0.0, f.z).normalize_or(prev_f);
                let dyaw = prev_f.cross(fh).y.asin() / DT;
                yaw_out += dyaw;
                prev_f = fh;
                // rotation the stick did NOT ask for: pitch and roll rates
                let body = car.rot.inverse() * car.ang;
                stray = stray.max(body.x.abs().max(body.z.abs()));
                if ball.pos.distance(car.pos) < 2.2 && ball.pos.y > car.pos.y {
                    carried += 1;
                }
                if car.on_ground {
                    grounded_frames += 1;
                }
                if std::env::var("DRIB_DBG").map(|v| v == label).unwrap_or(false) && car.on_ground != prev_og {
                    prev_og = car.on_ground;
                    eprintln!(
                        "  f={:3} y={:6.3} d={:6.3} og={} vy={:6.2} sp={:5.1}",
                        frame, car.pos.y, world2.sdf(car.pos), car.on_ground as u8, car.vel.y, car.vel.length()
                    );
                }
                frame += 1;
                let _ = &prev_og;
            }
            let sp = car.vel.length();
            let cmd = -(steer) * (3.0 - (sp * 0.050).min(1.65)).max(1.15) * cfg.car.steer_rate;
            println!(
                "{:<16} {:>9.2} {:>9.2} {:>9.2} {:>7.0}% {:>6.0}%",
                label,
                cmd,
                yaw_out / steps as f32,
                stray,
                carried as f32 / steps as f32 * 100.0,
                grounded_frames as f32 / steps as f32 * 100.0
            );
        }
        return;
    }
    if name == "flick" {
        // FAMOUS FLICKS, attempted for real: balance the ball on the hood,
        // drive, then run each input sequence and measure what leaves.
        let cfg = Config::default();
        let world = World::generate(31, crate::world::style(0), 0.31, 1.0);
        // settle a dribble: ball resting on the hood at driving speed
        let setup = |speed: f32, an: &mut crate::style::Analyzer| -> (physics::Car, Ball) {
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
            let mut ball = Ball::new();
            ball.pos = car.pos + car.forward() * 0.30 + Vec3::Y * (car.half_h + ball.r + 0.02);
            ball.vel = Vec3::ZERO;
            let mut c = physics::Controls::default();
            let mut w2 = World::generate(31, crate::world::style(0), 0.31, 1.0);
            let mut ev = StepEvents::default();
            let mut t = 0.0f32;
            while t < 2.6 {
                let sp = car.vel.length();
                c.throttle = if sp < speed { 1.0 } else { 0.0 };
                // steer gently toward the ball to keep it centred, as a
                // human dribbler would
                let off = (ball.pos - car.pos).dot(car.rot * Vec3::X);
                c.steer = (off * 1.6).clamp(-1.0, 1.0);
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut w2, &c, &cfg, DT, &mut ev);
                an.step(&crate::style::Frame {
                    dt: DT,
                    car_pos: car.pos,
                    car_vel: car.vel,
                    car_up: car.up(),
                    on_ground: car.on_ground,
                    flipping: car.is_flipping(),
                    omega: car.omega.length(),
                    ball_pos: ball.pos,
                    ball_vel: ball.vel,
                    touch: ev.touches.first().map(|&(_, sp)| sp),
                    flip_reset: ev.flip_reset.is_some(),
                    pinch: ev.pinched,
                    goal: false,
                    surf_up: w2.normal(car.pos).y,
                    supersonic: car.supersonic,
                });
                t += DT;
            }
            // A flick starts with the ball WORKED ONTO THE NOSE — that is
            // the setup every one of these plays begins from. Place it
            // there, matched in speed, and let contact settle a beat.
            ball.pos = car.pos + car.forward() * 0.52 + car.up() * (car.half_h + ball.r + 0.01);
            ball.vel = car.vel;
            for _ in 0..14 {
                let off = (ball.pos - car.pos).dot(car.rot * Vec3::X);
                c.steer = (off * 1.6).clamp(-1.0, 1.0);
                c.throttle = 0.0;
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut w2, &c, &cfg, DT, &mut ev);
                an.step(&crate::style::Frame {
                    dt: DT,
                    car_pos: car.pos,
                    car_vel: car.vel,
                    car_up: car.up(),
                    on_ground: car.on_ground,
                    flipping: car.is_flipping(),
                    omega: car.omega.length(),
                    ball_pos: ball.pos,
                    ball_vel: ball.vel,
                    touch: ev.touches.first().map(|&(_, sp)| sp),
                    flip_reset: ev.flip_reset.is_some(),
                    pinch: ev.pinched,
                    goal: false,
                    surf_up: w2.normal(car.pos).y,
                    supersonic: car.supersonic,
                });
            }
            if std::env::var("FLICK_DBG").is_ok() {
                let rel = ball.pos - car.pos;
                eprintln!(
                    "  setup: car spd {:.1}, ball above hood {:.2}, fwd offset {:.2}, side {:.2}, ball spd {:.1}",
                    car.vel.length(),
                    rel.dot(car.up()),
                    rel.dot(car.forward()),
                    rel.dot(car.rot * Vec3::X),
                    ball.vel.length()
                );
            }
            (car, ball)
        };
        struct Flick {
            name: &'static str,
            hint: &'static str,
            // (jump_pressed, throttle, steer, pitch, roll) per phase
            seq: &'static [(f32, f32, f32, f32, f32, bool)],
        }
        // phases are (seconds, throttle, steer, pitch, roll, jump_pressed)
        let flicks: [Flick; 7] = [
            Flick { name: "MUSTY", hint: "nose-down, then backflip under it", seq: &[(0.02, 0.0, 0.0, 0.0, 0.0, true), (0.05, 0.0, 0.0, -0.5, 0.0, false), (0.02, -1.0, 0.0, 0.0, 0.0, true), (0.60, 0.0, 0.0, 0.0, 0.0, false)] },
            Flick { name: "FORWARD", hint: "jump, front flip through it", seq: &[(0.02, 0.0, 0.0, 0.0, 0.0, true), (0.07, 0.0, 0.0, 0.0, 0.0, false), (0.02, 1.0, 0.0, 0.0, 0.0, true), (0.60, 1.0, 0.0, 0.0, 0.0, false)] },
            Flick { name: "45-DEGREE", hint: "diagonal flip, the classic", seq: &[(0.02, 0.0, 0.0, 0.0, 0.0, true), (0.07, 0.0, 0.0, 0.0, 0.0, false), (0.02, 1.0, 0.8, 0.0, 0.0, true), (0.60, 1.0, 0.8, 0.0, 0.0, false)] },
            Flick { name: "SIDEWAYS", hint: "side flip: pure lateral whip", seq: &[(0.02, 0.0, 0.0, 0.0, 0.0, true), (0.07, 0.0, 0.0, 0.0, 0.0, false), (0.02, 0.0, 1.0, 0.0, 0.0, true), (0.60, 0.0, 1.0, 0.0, 0.0, false)] },
            Flick { name: "TORNADO", hint: "air roll into the flip", seq: &[(0.02, 0.0, 0.0, 0.0, 0.0, true), (0.14, 0.0, 0.0, 0.0, 1.0, false), (0.02, 1.0, 0.0, 0.0, 1.0, true), (0.60, 1.0, 0.0, 0.0, 1.0, false)] },
            Flick { name: "DELAYED", hint: "pop, wait, then flip", seq: &[(0.02, 0.0, 0.0, 0.0, 0.0, true), (0.26, 0.0, 0.0, 0.0, 0.0, false), (0.02, 1.0, 0.0, 0.0, 0.0, true), (0.60, 1.0, 0.0, 0.0, 0.0, false)] },
            Flick { name: "BREEZI", hint: "backflip, cancel, roll out", seq: &[(0.02, 0.0, 0.0, 0.0, 0.0, true), (0.07, 0.0, 0.0, 0.0, 0.0, false), (0.02, -1.0, 0.0, 0.0, 0.0, true), (0.16, -1.0, 0.0, 0.0, 0.0, false), (0.50, 0.0, 0.0, -1.0, 1.0, false)] },
        ];
        println!("{:<11} {:>7} {:>7} {:>7} {:>7} {:>4} {:<6} {}", "flick", "exit", "up", "elev", "gain", "hits", "feel", "the analyzer says");
        for f in flicks {
            let mut an = crate::style::Analyzer::new();
            let (mut car, mut ball) = setup(11.0, &mut an);
            let base = car.vel.length();
            let mut w2 = World::generate(31, crate::world::style(0), 0.31, 1.0);
            let mut ev = StepEvents::default();
            let mut c = physics::Controls::default();
            // the launch is the ball's velocity AS IT LEAVES THE CAR, not
            // whatever it has after gravity has been at it
            let mut best = ball.vel;
            let mut touches = 0;
            let mut flip_touches = 0;
            let mut flipping = false;
            let mut record = |ev: &StepEvents, ball: &Ball, best: &mut Vec3, touches: &mut i32| {
                if !ev.touches.is_empty() {
                    *touches += 1;
                    *best = ball.vel; // last contact wins: that is separation
                }
            };
            for &(dur, thr, st, pit, roll, jump) in f.seq {
                let mut t = 0.0f32;
                let mut fired = false;
                while t < dur {
                    c.throttle = thr;
                    c.steer = st;
                    c.pitch = pit;
                    c.roll = roll;
                    c.jump_pressed = jump && !fired;
                    fired = true;
                    ev.touches.clear();
                    physics::step(&mut car, &mut ball, &mut w2, &c, &cfg, DT, &mut ev);
                    flipping = car.is_flipping();
                    if std::env::var("FLICK_TRACE").map(|v| v == f.name).unwrap_or(false) {
                        let rel = ball.pos - car.pos;
                        eprintln!(
                            "    up={:5.2} fwd={:5.2} gap={:5.2} omega={:5.1} flip={} touch={}",
                            rel.dot(car.up()), rel.dot(car.forward()), rel.length() - 0.9,
                            car.omega.length(), flipping as u8, !ev.touches.is_empty() as u8
                        );
                    }
                    if flipping && !ev.touches.is_empty() {
                        flip_touches += 1;
                    }
                    record(&ev, &ball, &mut best, &mut touches);
                    let sf = crate::style::Frame {
                        dt: DT,
                        car_pos: car.pos,
                        car_vel: car.vel,
                        car_up: car.up(),
                        on_ground: car.on_ground,
                        flipping: car.is_flipping(),
                        omega: car.omega.length(),
                        ball_pos: ball.pos,
                        ball_vel: ball.vel,
                        touch: ev.touches.first().map(|&(_, sp)| sp),
                        flip_reset: ev.flip_reset.is_some(),
                        pinch: ev.pinched,
                        goal: !ev.goals.is_empty(),
                        surf_up: w2.normal(car.pos).y,
                        supersonic: car.supersonic,
                    };
                    an.step(&sf);
                    t += DT;
                }
            }
            c = physics::Controls::default();
            for _ in 0..40 {
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut w2, &c, &cfg, DT, &mut ev);
                record(&ev, &ball, &mut best, &mut touches);
                let sf = crate::style::Frame {
                    dt: DT,
                    car_pos: car.pos,
                    car_vel: car.vel,
                    car_up: car.up(),
                    on_ground: car.on_ground,
                    flipping: car.is_flipping(),
                    omega: car.omega.length(),
                    ball_pos: ball.pos,
                    ball_vel: ball.vel,
                    touch: ev.touches.first().map(|&(_, sp)| sp),
                    flip_reset: ev.flip_reset.is_some(),
                    pinch: ev.pinched,
                    goal: !ev.goals.is_empty(),
                    surf_up: w2.normal(car.pos).y,
                    supersonic: car.supersonic,
                };
                an.step(&sf);
            }
            let _ = touches;
            let sp = best.length();
            let elev = best.y.atan2(Vec3::new(best.x, 0.0, best.z).length()).to_degrees();
            let named: Vec<String> = an.fresh.iter().map(|x| format!("{} +{}", x.name, x.points)).collect();
            let named = if named.is_empty() { format!("(nothing named)") } else { named.join(", ") };
            let feel = if sp > base * 1.8 && best.y > 4.0 {
                "BIG"
            } else if sp > base * 1.25 {
                "works"
            } else {
                "weak"
            };
            println!(
                "{:<11} {:>7.1} {:>7.1} {:>6.0}d {:>6.0}% {:>4} {:<6} {}",
                f.name, sp, best.y, elev, sp / base.max(0.1) * 100.0, flip_touches, feel, named
            );
        }
        return;
    }
    if name == "shot" {
        // POWER SHOT audit: drive into a resting ball at speed, then measure
        // exit speed, how far it actually travels, and how fast a roll dies
        let cfg = Config::default();
        // open plain, not the boxed arena: a wall rebound would fake the
        // travel numbers (the first run measured exactly that)
        let world = World::generate(31, crate::world::style(0), 0.31, 1.0);
        println!("{:<10} {:>7} {:>7} {:>7} {:>7} {:>7}", "approach", "exit", "loft", "1s", "3s", "carry%");
        for target in [8.0f32, 15.0, 24.0] {
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
            let mut ball = Ball::new();
            // the car's nose points -Z, so the ball goes in FRONT of it
            ball.pos = Vec3::new(0.0, world.ground_h(0.0, -30.0) + ball.r, -30.0);
            let _ = &world;
            ball.vel = Vec3::ZERO;
            let mut c = physics::Controls::default();
            c.throttle = 1.0;
            c.boost = true;
            let mut ev = StepEvents::default();
            let mut approach = 0.0f32;
            let mut exit = 0.0f32;
            let mut loft = 0.0f32;
            let mut d1 = 0.0f32;
            let mut d3 = 0.0f32;
            let start = ball.pos;
            let mut t = 0.0f32;
            let mut hit_t = -1.0f32;
            let mut world2 = World::generate(31, crate::world::style(0), 0.31, 1.0);
            while t < 11.0 {
                if hit_t < 0.0 {
                    // hold the approach speed rather than coasting: a slow
                    // target used to stall out before ever reaching the ball
                    let sp = car.vel.length();
                    c.throttle = if sp < target { 1.0 } else { 0.0 };
                    c.boost = sp < target - 1.0 && target > 16.0;
                }
                ev.touches.clear();
                let pre = car.vel.length();
                physics::step(&mut car, &mut ball, &mut world2, &c, &cfg, DT, &mut ev);
                if hit_t < 0.0 && ball.vel.length() > 0.5 {
                    hit_t = t;
                    approach = pre;
                }
                if hit_t >= 0.0 {
                    let since = t - hit_t;
                    if std::env::var("SHOT_DBG").is_ok() && (since * 4.0).fract() < DT * 4.0 && since < 3.1 {
                        eprintln!(
                            "  t={:4.2} spd={:5.1} y={:5.2} vy={:6.2} spin={:5.1} d={:6.1}",
                            since, ball.vel.length(), ball.pos.y, ball.vel.y, ball.ang.length(), ball.pos.distance(start)
                        );
                    }
                    if exit == 0.0 && since > 0.10 {
                        exit = ball.vel.length();
                        loft = ball.vel.y;
                    }
                    // distance ALONG the shot line, so a rebound cannot
                    // masquerade as travel
                    let along = (ball.pos - start).dot(Vec3::NEG_Z);
                    if d1 == 0.0 && since >= 1.0 {
                        d1 = along;
                    }
                    if d3 == 0.0 && since >= 3.0 {
                        d3 = along;
                        break;
                    }
                }
                t += DT;
            }
            println!(
                "{:<10.1} {:>7.1} {:>7.1} {:>7.1} {:>7.1} {:>6.0}%",
                approach, exit, loft, d1, d3, if approach > 0.0 { exit / approach * 100.0 } else { 0.0 }
            );
        }
        return;
    }
    if name == "spawn" {
        // EVERY map must put the car on a surface, not inside one. Raised
        // decks (DROPSHOT) used to bury the spawn and made maps unplayable.
        let cfg = Config::default();
        let mut bad = 0;
        println!("{:<14} {:>8} {:>8} {:>7} {:>6}", "map", "spawn_y", "clear", "settled", "up.y");
        for si in 0..crate::world::STYLE_COUNT {
            let st = crate::world::style(si);
            let world = World::generate(9, st, 0.31, 1.0);
            let mut car = physics::Car::new();
            car.reset(&world);
            let spawn_y = car.pos.y;
            let clear = world.sdf(car.pos);
            let ball = Ball::new();
            let c = physics::Controls::default();
            for _ in 0..300 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            let settled = car.on_ground && car.pos.y > -5.0;
            let ok = clear > 0.0 && settled && car.up().y > 0.85;
            if !ok {
                bad += 1;
            }
            println!(
                "{:<14} {:>8.2} {:>8.2} {:>7} {:>6.2}{}",
                st.name,
                spawn_y,
                clear,
                if settled { "yes" } else { "NO" },
                car.up().y,
                if ok { "" } else { "   <-- BAD" }
            );
        }
        println!("{}", if bad == 0 { "SPAWNS: PASS" } else { "SPAWNS: FAIL" });
        return;
    }
    if name == "kuiper" {
        // the deep-space systems: graded gravity, surface following, the
        // collapse/detonation event, arrivals, and planet billiards
        let cfg = Config::default();
        let mut world = World::generate(9, crate::world::style(28), 0.31, 1.0);
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<42} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        // 1. bigger worlds pull harder
        let mut heavy = (0.0f32, Vec3::ZERO, 0.0f32);
        let mut light = (f32::MAX, Vec3::ZERO, 0.0f32);
        for g in world.grav.iter() {
            if g.g > heavy.0 {
                heavy = (g.g, g.pos, g.r);
            }
            if g.g < light.0 {
                light = (g.g, g.pos, g.r);
            }
        }
        let gh = world.gravity_field(heavy.1 + Vec3::Y * heavy.2 * 0.3).1;
        let gl = world.gravity_field(light.1 + Vec3::Y * light.2 * 0.3).1;
        println!("  heaviest well g={:.2}, lightest g={:.2}", gh, gl);
        check("giants pull harder than dwarfs", gh > gl * 1.5);
        check("far field relaxes to normal gravity", (world.gravity_field(Vec3::new(900.0, 400.0, 900.0)).1 - 1.0).abs() < 0.01);
        // 2. gravity points INTO the nearest world, not just down
        let side = heavy.1 + Vec3::X * (heavy.2 * 0.35);
        let d = world.gravity_field(side).0;
        check("gravity points at the world from the side", d.x < -0.3);
        // 3. planets are free bodies that collide
        let n_bodies = world.bodies.len();
        check("free worlds exist", n_bodies >= 8);
        let (i, j) = (0usize, 1usize);
        world.bodies[i].pos = Vec3::new(0.0, 400.0, 0.0);
        world.bodies[j].pos = world.bodies[i].pos + Vec3::new(world.bodies[i].r + world.bodies[j].r - 0.3, 0.0, 0.0);
        world.bodies[i].vel = Vec3::new(24.0, 0.0, 0.0);
        world.bodies[j].vel = Vec3::ZERO;
        let mut car = physics::Car::new();
        car.pos = Vec3::new(600.0, 20.0, 600.0);
        let mut ball = Ball::new();
        ball.pos = Vec3::new(620.0, 20.0, 620.0);
        let c = physics::Controls::default();
        let mut ev = StepEvents::default();
        for _ in 0..30 {
            physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
        }
        let struck = world.bodies[j].vel.x;
        println!("  billiards: struck world now moving at {:.1} m/s", struck);
        check("planet-planet collision transfers momentum", struck > 1.0);
        // 4. the collapse event: pull inward, then a detonation that throws
        world.force_collapse_for_test();
        let bh_pos = world.bh_pos_for_test().unwrap();
        ball.pos = bh_pos + Vec3::new(40.0, 0.0, 0.0);
        ball.vel = Vec3::ZERO;
        car.pos = bh_pos + Vec3::new(52.0, 0.0, 0.0);
        car.vel = Vec3::ZERO;
        let d0 = ball.pos.distance(bh_pos);
        for _ in 0..120 {
            world.update(DT);
            physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
        }
        let d1 = ball.pos.distance(bh_pos);
        println!("  collapse: ball {d0:.1} m -> {d1:.1} m from the singularity");
        check("black hole hauls things inward", d1 < d0 - 1.0);
        // run out the fifteen seconds and catch the blast
        let mut blasted = false;
        let mut speed_after = 0.0;
        for _ in 0..(16 * 120) {
            ev.blasted = false;
            world.update(DT);
            physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            if ev.blasted {
                blasted = true;
                speed_after = ball.vel.length();
                break;
            }
        }
        println!("  detonation: ball thrown at {speed_after:.1} m/s");
        check("collapse detonates and throws things", blasted && speed_after > 4.0);
        // 5. the shockwave falls off with distance
        let mut w2 = World::generate(9, crate::world::style(28), 0.31, 1.0);
        w2.force_collapse_for_test();
        let c2 = w2.bh_pos_for_test().unwrap();
        let (mut near_b, mut far_b) = (Ball::new(), Ball::new());
        near_b.pos = c2 + Vec3::new(14.0, 0.0, 0.0);
        far_b.pos = c2 + Vec3::new(140.0, 0.0, 0.0);
        w2.set_blast_for_test(c2, 260.0);
        let mut car2 = physics::Car::new();
        car2.pos = c2 + Vec3::new(800.0, 0.0, 0.0);
        physics::step(&mut car2, &mut near_b, &mut w2, &c, &cfg, DT, &mut ev);
        let vn = near_b.vel.length();
        w2.set_blast_for_test(c2, 260.0);
        physics::step(&mut car2, &mut far_b, &mut w2, &c, &cfg, DT, &mut ev);
        let vf = far_b.vel.length();
        println!("  shockwave: {vn:.1} m/s at 14 m vs {vf:.1} m/s at 140 m");
        check("shockwave falls off with distance", vn > vf * 2.0);
        // 6. arrivals add a world
        let before = world.bodies.len();
        world.force_arrival_for_test();
        check("a new world arrives", world.bodies.len() == before + 1);
        let nb = world.bodies.last().unwrap();
        check("it arrives from high above, falling", nb.pos.y > 200.0 && nb.vel.y < -10.0);
        println!("{}", if pass { "KUIPER: PASS" } else { "KUIPER: FAIL" });
        return;
    }
    if name == "planets" {
        // The DVD-screensaver planets: do they squash on impact, spring
        // back to round, keep almost all their bounce, and never settle?
        let cfg = Config::default();
        let mut world = World::generate(9, crate::world::style(28), 0.31, 1.0); // KUIPER
        let n_bouncy = world.bodies.iter().filter(|b| b.bouncy).count();
        println!("drifting planets: {n_bouncy}");
        let bi = world.bodies.iter().position(|b| b.bouncy).unwrap();
        // park the car and ball far away so only the planet is in play
        let mut car = physics::Car::new();
        car.pos = Vec3::new(600.0, 40.0, 600.0);
        let mut ball = Ball::new();
        ball.pos = Vec3::new(-600.0, 40.0, -600.0);
        let c = physics::Controls::default();
        let mut ev = StepEvents::default();
        let (mut peak_squash, mut bounces, mut apex, mut first_apex) = (0.0f32, 0, 0.0f32, 0.0f32);
        let (mut prev_vy, mut settled_round) = (0.0f32, false);
        let mut t = 0.0f32;
        while t < 40.0 {
            ev.goals.clear();
            physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            let b = &world.bodies[bi];
            peak_squash = peak_squash.max(b.squash);
            if b.squash == 0.0 && t > 6.0 {
                settled_round = true; // it does return to a sphere
            }
            let vy = b.vel.y;
            if prev_vy < 0.0 && vy > 0.0 {
                bounces += 1; // direction flip at the ground = a bounce
            }
            if vy.abs() < 0.3 {
                apex = apex.max(b.pos.y);
                if first_apex == 0.0 && bounces >= 1 {
                    first_apex = b.pos.y;
                }
            }
            prev_vy = vy;
            t += DT;
        }
        let b = &world.bodies[bi];
        let still_moving = b.vel.length();
        println!("bounces in 40 s: {bounces}   peak squash: {:.0}%   returns to round: {}", peak_squash * 100.0, settled_round);
        println!("still moving after 40 s: {still_moving:.1} m/s   highest apex {apex:.0} m");
        let ok = n_bouncy >= 3 && bounces >= 3 && peak_squash > 0.05 && settled_round && still_moving > 1.0;
        println!("{}", if ok { "PLANETS: PASS" } else { "PLANETS: FAIL" });
        return;
    }
    if name == "stream" {
        // LAN RENDER ASSIST, end to end on loopback: the client marches only
        // the near field and hands the expensive tail to probes streamed by
        // a host. Measures the transport (RTT, bitrate), how much work the
        // split actually saves, and — the part that matters — how close the
        // assisted result is to a full local march.
        use crate::stream::{decode_probes, encode_probes, Cache, Endpoint, PoseMsg, Probe, DIRS, PROBES};
        let world = World::generate(9, crate::world::style(21), 0.31, 1.0);
        // a plain CPU march: the ground truth, and what the host runs
        let march = |mut p: Vec3, d: Vec3, max_t: f32, steps: u32| -> (Option<Vec3>, f32) {
            let mut t = 0.0f32;
            for _ in 0..steps {
                let dist = world.sdf(p);
                if dist < 0.05 {
                    return (Some(p), t);
                }
                t += dist.max(0.05);
                if t > max_t {
                    break;
                }
                p += d * dist.max(0.05);
            }
            (None, t)
        };
        let shade = |hit: Option<Vec3>, d: Vec3| -> [f32; 3] {
            match hit {
                Some(h) => {
                    let n = world.normal(h);
                    let l = Vec3::new(0.4, 0.8, 0.3).normalize();
                    let k = n.dot(l).max(0.0) * 0.7 + 0.25;
                    [k * 0.85, k * 0.9, k]
                }
                None => {
                    let up = d.y * 0.5 + 0.5;
                    [0.35 + up * 0.3, 0.45 + up * 0.35, 0.7 + up * 0.3]
                }
            }
        };
        let host = Endpoint::bind("127.0.0.1:0").expect("host bind");
        let client = Endpoint::bind("127.0.0.1:0").expect("client bind");
        let (haddr, caddr) = (host.addr().unwrap(), client.addr().unwrap());
        let mut cache = Cache::default();
        let mut buf = [0u8; 65536];
        let mut rtt_sum = 0.0f64;
        let mut bytes = 0usize;
        let mut frames = 0u32;
        let mut car = Vec3::new(0.0, 3.0, 0.0);
        let vel = Vec3::new(0.0, 0.0, -22.0);
        for seq in 0..40u32 {
            car += vel * (1.0 / 30.0);
            let t0 = std::time::Instant::now();
            client.send(haddr, &PoseMsg { seq, pos: car, vel }.encode());
            // ---- host side ----
            if let Some((n, from)) = host.recv(&mut buf) {
                let q = PoseMsg::decode(&buf[..n]).expect("pose decodes");
                // probes go AHEAD of the car, where its rays will land
                let mut probes = Vec::with_capacity(PROBES);
                for i in 0..PROBES {
                    let a = i as f32 * 0.61803 * std::f32::consts::TAU;
                    let r = 6.0 + (i as f32 / PROBES as f32) * 34.0;
                    let c = q.pos + q.vel.normalize_or(Vec3::NEG_Z) * (r * 0.6)
                        + Vec3::new(a.cos() * r * 0.5, (i % 4) as f32 * 2.5, a.sin() * r * 0.5);
                    let mut rgb = [[0u8; 3]; DIRS];
                    for (k, slot) in rgb.iter_mut().enumerate() {
                        let d = crate::stream::dir_of(k);
                        let (hit, _) = march(c, d, 260.0, 64);
                        let col = shade(hit, d);
                        *slot = [
                            (col[0] * 255.0) as u8,
                            (col[1] * 255.0) as u8,
                            (col[2] * 255.0) as u8,
                        ];
                    }
                    probes.push(Probe { pos: c, rgb });
                }
                let pkt = encode_probes(q.seq, &probes);
                bytes += pkt.len();
                host.send(from, &pkt);
                let _ = caddr;
            }
            // ---- client side ----
            std::thread::sleep(std::time::Duration::from_micros(200));
            if let Some((n, _)) = client.recv(&mut buf) {
                if let Some((seq_in, probes)) = decode_probes(&buf[..n]) {
                    cache.probes = probes;
                    cache.last_seq = seq_in;
                    cache.age = 0.0;
                    rtt_sum += t0.elapsed().as_secs_f64();
                    frames += 1;
                }
            }
        }
        println!("transport   frames {frames}/40   mean RTT {:.2} ms   {:.1} KB/frame   {:.0} kbit/s at 30 Hz",
            rtt_sum / frames.max(1) as f64 * 1000.0,
            bytes as f32 / frames.max(1) as f32 / 1024.0,
            bytes as f32 / frames.max(1) as f32 * 8.0 * 30.0 / 1000.0);
        // ---- the split: near field local, far field from the cache ----
        let near = 12.0f32;
        let (mut local, mut handed, mut err_sum, mut n_err) = (0u32, 0u32, 0.0f32, 0u32);
        let (mut steps_full, mut steps_split) = (0u64, 0u64);
        for i in 0..600u32 {
            let a = i as f32 * 0.7548;
            let d = Vec3::new(a.cos(), (i % 7) as f32 * 0.12 - 0.3, a.sin()).normalize();
            let p = car + Vec3::new((i % 11) as f32 - 5.0, 1.0 + (i % 5) as f32, (i % 13) as f32 - 6.0);
            let (hit_full, _) = march(p, d, 260.0, 64);
            let truth = shade(hit_full, d);
            steps_full += 64;
            // the client's cheap version: a short march, then the cache
            let (hit_near, _) = march(p, d, near, 12);
            steps_split += 12;
            let got = if hit_near.is_some() {
                local += 1;
                shade(hit_near, d)
            } else {
                handed += 1;
                match cache.sample(p, d) {
                    Some(c) => c,
                    None => shade(None, d),
                }
            };
            let e = ((got[0] - truth[0]).abs() + (got[1] - truth[1]).abs() + (got[2] - truth[2]).abs()) / 3.0;
            err_sum += e;
            n_err += 1;
        }
        let err = err_sum / n_err.max(1) as f32;
        println!("split       {:.0}% resolved locally, {:.0}% from probes   ray steps {:.0}% of full",
            local as f32 / 600.0 * 100.0, handed as f32 / 600.0 * 100.0,
            steps_split as f32 / steps_full as f32 * 100.0);
        println!("accuracy    mean channel error {:.3} vs a full march", err);
        let ok = frames >= 35 && err < 0.22;
        println!("{}", if ok { "STREAM: PASS" } else { "STREAM: FAIL" });
        return;
    }
    if name == "learn" {
        // Does the squad actually get better DURING a match? Run one long
        // match and compare aerial output in the first third against the
        // last third. Learning off (params frozen) is the control.
        let cfg = Config::default();
        println!("{:<10} {:>9} {:>9} {:>9} {:>8}", "mode", "early", "late", "gain", "episodes");
        for learn_on in [false, true] {
            let mut early = 0u32;
            let mut late = 0u32;
            let mut eps = 0u32;
            let trials: u64 = 3;
            for tr in 0..trials {
                let world = World::generate(31 + tr, crate::world::style(ARENA), 0.31, 1.0);
                let mut ball = Ball::new();
                ball.pos = Vec3::new(0.0, 6.0, 0.0);
                let mut player = physics::Car::new();
                player.pos = Vec3::new(0.0, 0.4, -40.0);
                let mut bots = crate::bots::spawn_match(&world, 3);
                crate::bots::set_learning(&mut bots, learn_on);
                let mut t = 0.0f32;
                let total = 150.0f32;
                let mut base: Vec<u32> = bots.iter().map(|b| b.touches_air).collect();
                while t < total {
                    crate::bots::update(&mut bots, &mut ball, &mut player, &world, &cfg, t, DT);
                    let mut ev = StepEvents::default();
                    physics::step_ball(&mut ball, &world, &cfg, DT, &mut ev);
                    if ball.pos.y < 0.2 && ball.vel.length() < 1.0 {
                        ball.pos = Vec3::new(0.0, 6.0, 0.0);
                        ball.vel = Vec3::ZERO;
                    }
                    t += DT;
                    if (t - total / 3.0).abs() < DT * 0.5 {
                        let now: Vec<u32> = bots.iter().map(|b| b.touches_air).collect();
                        early += now.iter().zip(&base).map(|(a, b)| a - b).sum::<u32>();
                        base = now;
                    }
                    if (t - total * 2.0 / 3.0).abs() < DT * 0.5 {
                        base = bots.iter().map(|b| b.touches_air).collect();
                    }
                }
                let end: Vec<u32> = bots.iter().map(|b| b.touches_air).collect();
                late += end.iter().zip(&base).map(|(a, b)| a - b).sum::<u32>();
                eps += bots.iter().map(|b| b.episodes()).sum::<u32>();
            }
            let gain = if early > 0 { late as f32 / early as f32 } else { 0.0 };
            println!(
                "{:<10} {:>9} {:>9} {:>8.2}x {:>8}",
                if learn_on { "learning" } else { "frozen" },
                early,
                late,
                gain,
                eps / (trials as u32).max(1)
            );
        }
        return;
    }
    if name == "portal" {
        // Portal's rules, checked: speed in equals speed out, direction is
        // rotated by the pair transform, a fall becomes a launch (the
        // FLING), and nothing ping-pongs between the two faces.
        let cfg = Config::default();
        let world = World::generate(9, crate::world::style(31), 0.31, 1.0);
        let ball0 = Ball::new();
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<38} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        check("map has linked portal pairs", world.portals.len() >= 6);
        // every link is mutual, and every transform is a pure rotation
        let mut mutual = true;
        let mut orthonormal = true;
        for (i, p) in world.portals.iter().enumerate() {
            if world.portals[p.link].link != i {
                mutual = false;
            }
            let r = world.portal_rot(i);
            let v = Vec3::new(0.3, -0.9, 0.31).normalize();
            if ((r * v).length() - 1.0).abs() > 1e-4 {
                orthonormal = false;
            }
        }
        check("links are mutual", mutual);
        check("transform preserves length (pure rotation)", orthonormal);
        // a ray INTO a portal leaves the partner pointing OUT of its face
        let mut exits_outward = true;
        for (i, p) in world.portals.iter().enumerate() {
            let n_in = p.rot * Vec3::Z;
            let out = world.portal_rot(i) * (-n_in); // travelling into the face
            let n_out = world.portals[p.link].rot * Vec3::Z;
            if out.dot(n_out) < 0.98 {
                exits_outward = false;
            }
        }
        check("entering a face exits the partner's face", exits_outward);

        // ---- drive a car through: speed and direction ----
        let mut car = physics::Car::new();
        let pf = &world.portals[0];
        let n0 = pf.rot * Vec3::Z;
        car.pos = pf.pos + n0 * 4.0;
        car.vel = -n0 * 26.0;
        car.prev = car.pos;
        car.rot = glam::Quat::from_rotation_arc(Vec3::NEG_Z, -n0);
        let ctl = physics::Controls::default();
        let mut speed_in = car.vel.length();
        let mut teleported = false;
        let mut speed_out = 0.0;
        let mut exit_dir = Vec3::ZERO;
        for _ in 0..60 {
            let before = car.pos;
            // speed measured at the instant of crossing, not at spawn: a few
            // frames of air drag on the approach is not a portal loss
            if !teleported {
                speed_in = car.vel.length();
            }
            physics::step_car_only(&mut car, &ball0, &world, &ctl, &cfg, DT);
            if !teleported && car.pos.distance(before) > 3.0 {
                teleported = true;
                speed_out = car.vel.length();
                exit_dir = car.vel.normalize_or(Vec3::Z);
            }
        }
        let n_exit = world.portals[world.portals[0].link].rot * Vec3::Z;
        check("car teleports on crossing", teleported);
        check(
            "speed is conserved through the portal",
            (speed_out - speed_in).abs() / speed_in.max(1.0) < 0.08,
        );
        check("car leaves along the exit face", exit_dir.dot(n_exit) > 0.9);
        println!("   in {speed_in:.1} m/s -> out {speed_out:.1} m/s, exit alignment {:.2}", exit_dir.dot(n_exit));

        // ---- THE FLING: fall into a floor portal, leave a wall horizontally
        let mut car = physics::Car::new();
        let fp = &world.portals[0];
        car.pos = fp.pos + Vec3::Y * 40.0;
        car.vel = Vec3::ZERO;
        car.prev = car.pos;
        let mut peak_h = 0.0f32;
        let mut fell = 0.0f32;
        for f in 0..900 {
            let before_y = car.pos.y;
            physics::step_car_only(&mut car, &ball0, &world, &ctl, &cfg, DT);
            if std::env::var("PORTAL_DBG").is_ok() && f % 20 == 0 {
                eprintln!("  f={:3} pos=({:6.1},{:6.1},{:6.1}) vel=({:5.1},{:6.1},{:5.1}) sdf={:6.2} og={}",
                    f, car.pos.x, car.pos.y, car.pos.z, car.vel.x, car.vel.y, car.vel.z, world.sdf(car.pos), car.on_ground as u8);
            }
            if car.pos.y < before_y {
                fell += before_y - car.pos.y;
            }
            let horiz = Vec3::new(car.vel.x, 0.0, car.vel.z).length();
            peak_h = peak_h.max(horiz);
        }
        check("a fall becomes horizontal speed (the fling)", peak_h > 18.0);
        println!("   fell {fell:.0} m -> {peak_h:.1} m/s horizontal");

        // ---- no ping-pong: park a ball right at a face and watch it settle
        let mut ball = Ball::new();
        let lp = &world.portals[2];
        ball.pos = lp.pos + (lp.rot * Vec3::Z) * 1.2;
        ball.vel = Vec3::ZERO;
        ball.prev = ball.pos;
        let mut world2 = World::generate(9, crate::world::style(31), 0.31, 1.0);
        let mut jumps = 0;
        let mut ev = StepEvents::default();
        for _ in 0..600 {
            let before = ball.pos;
            physics::step_ball(&mut ball, &mut world2, &cfg, DT, &mut ev);
            if ball.pos.distance(before) > 5.0 {
                jumps += 1;
            }
        }
        check("no runaway ping-pong at a face", jumps < 40);
        println!("   {jumps} transits in 5 s of hovering at a portal mouth");
        // ---- MOVING PORTALS (TURNSTILE): does a mouth on a turning drum
        // actually change where it sends you as it comes around? ----
        {
            let mut w2 = World::generate(9, crate::world::style(32), 0.31, 1.0);
            let movers = w2.portals.iter().filter(|p| p.follow.is_some()).count();
            check("turnstile has portals mounted on movers", movers >= 2);
            // sample the exit transform of a moving portal over time
            let mi = w2.portals.iter().position(|p| p.follow.is_some()).unwrap_or(0);
            let probe = Vec3::new(0.0, 0.0, -1.0); // travelling into the face
            let mut first = Vec3::ZERO;
            let mut spread = 0.0f32;
            for k in 0..90 {
                w2.update(DT * 8.0);
                let out = w2.portal_rot(mi) * probe;
                if k == 0 {
                    first = out;
                }
                spread = spread.max(first.angle_between(out).to_degrees());
            }
            check("a moving portal's exit direction changes", spread > 45.0);
            println!("   exit direction swings {spread:.0} degrees as the machine turns");
            // colours: every pair distinct, and the two ends differ
            let mut hue_ok = true;
            for i in (0..w2.portals.len()).step_by(2) {
                if i + 1 >= w2.portals.len() {
                    break;
                }
                let (a2, b2) = (w2.portals[i].tint, w2.portals[i + 1].tint);
                let d = (a2[0] - b2[0]).abs() + (a2[1] - b2[1]).abs() + (a2[2] - b2[2]).abs();
                if d < 0.25 {
                    hue_ok = false;
                }
            }
            check("the two ends of a pair are shaded apart", hue_ok);
        }
        println!("{}", if pass { "PORTALS: PASS" } else { "PORTALS: FAIL" });
        return;
    }
    if name == "platform" {
        // Can a car RIDE a moving platform? Park one on each kind of mover
        // in CLOCKWORK and check it is carried rather than left behind.
        let cfg = Config::default();
        let style_i: usize = std::env::var("PLAT_MAP").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
        let mut world = World::generate(9, crate::world::style(style_i), 0.31, 1.0);
        let ball = Ball::new();
        let ctl = physics::Controls::default();
        // find one shape per anim kind
        let mut cases: Vec<(&str, usize)> = Vec::new();
        for a in world.anims.iter() {
            let (kind, si) = match *a {
                crate::world::Anim::Slide { shape, dir, .. } => {
                    (if dir.y.abs() > 0.5 { "paternoster" } else { "shuttle" }, shape)
                }
                crate::world::Anim::Orbit { shape, .. } => ("carriage", shape),
                crate::world::Anim::Spin { shape, .. } => ("turntable", shape),
                crate::world::Anim::Door { shape, .. } => ("door", shape),
                crate::world::Anim::Swing { shape, .. } => ("log bridge", shape),
            };
            if !cases.iter().any(|(k, _)| *k == kind) {
                cases.push((kind, si));
            }
        }
        println!("{:<13} {:>9} {:>9} {:>8}", "platform", "drift", "carried", "grounded");
        let mut pass = true;
        for (kind, si) in cases {
            // settle the world so platform velocities are live
            for _ in 0..30 {
                world.update(DT);
            }
            let sh = &world.shapes[si];
            // stand on the actual DECK: a ring deck's centre is its hole,
            // and a box deck's centre is inside it
            let top = match sh.sdf {
                crate::world::Sdf::RingDeck { big, half_h, .. } => sh.pos + Vec3::new(big, half_h + 0.34, 0.0),
                crate::world::Sdf::RBox { half, .. } => sh.pos + Vec3::Y * (half.y + 0.34),
                _ => sh.pos + Vec3::Y * 1.2,
            };
            let mut car = physics::Car::new();
            car.pos = top;
            car.rot = glam::Quat::IDENTITY;
            car.vel = Vec3::ZERO;
            // measure in the platform's OWN frame: a rider on a spinner
            // moves in world space by design, so world drift is meaningless
            let start_rel = world.shapes[si].inv_rot * (car.pos - world.shapes[si].pos);
            let mut grounded = 0;
            let dbg = std::env::var("PLAT_DBG").map(|v| v == kind).unwrap_or(false);
            for f in 0..240 {
                world.update(DT);
                physics::step_car_only(&mut car, &ball, &world, &ctl, &cfg, DT);
                if car.on_ground {
                    grounded += 1;
                }
                if dbg && f % 24 == 0 {
                    let sp = world.shapes[si].pos;
                    eprintln!(
                        "  f={:3} car=({:6.1},{:6.1},{:6.1}) deck=({:6.1},{:6.1},{:6.1}) sdf={:6.2} shape_d={:6.2} og={}",
                        f, car.pos.x, car.pos.y, car.pos.z, sp.x, sp.y, sp.z,
                        world.sdf(car.pos), world.shape_sdf_at(si, car.pos), car.on_ground as u8
                    );
                }
            }
            let end_rel = world.shapes[si].inv_rot * (car.pos - world.shapes[si].pos);
            // how far did the car slip relative to the deck it stands on?
            let drift = (end_rel - start_rel).length();
            let carried = grounded > 120 && drift < 4.0;
            pass &= carried;
            println!(
                "{:<13} {:>8.2}m {:>9} {:>7}%",
                kind,
                drift,
                if carried { "yes" } else { "NO" },
                grounded * 100 / 240
            );
        }
        // ---- BREAKWATER flows: does the water actually carry you? ----
        {
            let w = World::generate(9, crate::world::style(34), 0.31, 1.0);
            let f = w.flows.first().map(|f| (f.pos, f.vel));
            match f {
                Some((fp, fv)) => {
                    let cur = w.current(fp);
                    let carried = cur.map(|c| c.dot(fv.normalize_or(Vec3::Z)) > 5.0).unwrap_or(false);
                    println!(
                        "breakwater: {} flows, sample current {:.1} m/s  {}",
                        w.flows.len(),
                        cur.map(|c| c.length()).unwrap_or(0.0),
                        if carried { "carries" } else { "NO PUSH" }
                    );
                    pass &= carried;
                }
                None => {
                    println!("breakwater: no flows");
                    pass = false;
                }
            }
        }
        // ---- CHAMBER: does the board actually play the game? ----
        {
            let mut w = World::generate(9, crate::world::style(35), 0.31, 1.0);
            let n0 = w.chess.as_ref().map(|c| c.pieces.len()).unwrap_or(0);
            let mvs = w.chess.as_ref().map(|c| c.moves.len()).unwrap_or(0);
            // run a while at rest tempo and count what happened
            let mut moved = 0usize;
            for _ in 0..(60 * 120) {
                w.update(DT);
            }
            let played = w.chess.as_ref().map(|c| c.next).unwrap_or(0);
            let gone = w.chess.as_ref().map(|c| c.pieces.iter().filter(|p| p.gone).count()).unwrap_or(0);
            if let Some(c) = w.chess.as_ref() {
                moved = c.pieces.iter().filter(|p| p.sq != (99, 99) && p.from != p.to || p.moving).count();
            }
            let _ = moved;
            println!("chamber: {n0} pieces, {mvs} scripted moves, {played} played in 60 s, {gone} captured");
            // now wind it up as if goals had been scored
            if let Some(c) = w.chess.as_mut() {
                c.fury = 6.0;
            }
            let before = w.chess.as_ref().map(|c| c.next).unwrap_or(0);
            for _ in 0..(30 * 120) {
                w.update(DT);
            }
            let after = w.chess.as_ref().map(|c| c.next).unwrap_or(0);
            let calm_rate = played as f32 / 60.0;
            let fury_rate = (after - before) as f32 / 30.0;
            println!("   tempo: {calm_rate:.2} moves/s calm -> {fury_rate:.2} moves/s furious");
            let ok = n0 == 32 && played >= 8 && gone >= 1 && fury_rate > calm_rate * 1.8;
            pass &= ok;
            println!("   {}", if ok { "the board plays and winds up" } else { "CHESS FAIL" });
        }
        println!("{}", if pass { "PLATFORMS: PASS" } else { "PLATFORMS: FAIL" });
        return;
    }
    if name == "drive" {
        // THE REAL TEST: can a car actually get through a tube? Spawn one
        // inside a route, point it down the pipe, hold throttle, and follow
        // it — how far along does it get, and does it ever fall OUT?
        let cfg = Config::default();
        for (si, label) in [(21usize, "PERPLEXUS"), (24, "MONKEY RUN")] {
            let world = World::generate(9, crate::world::style(si), 0.31, 1.0);
            let ball = Ball::new();
            let mut done = 0;
            let mut escaped = 0;
            let mut worst: Vec<(usize, f32)> = Vec::new();
            let n_routes = world.routes.len().min(24);
            for ri in 0..n_routes {
                let (pts, r) = world.routes[ri].clone();
                if pts.len() < 2 {
                    continue;
                }
                // total length, for progress as a fraction
                let total: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
                if std::env::var("ROUTE_DBG").map(|v| v == format!("{si}")).unwrap_or(false) {
                    let gaps: Vec<String> = pts.windows(2).take(6).map(|w| format!("{:.0}", w[0].distance(w[1]))).collect();
                    eprintln!("  route {ri}: {} pts, total {:.0} m, r={:.1}, first gaps: {}", pts.len(), total, r, gaps.join(","));
                }
                if total < 12.0 {
                    continue;
                }
                let mut car = physics::Car::new();
                car.pos = pts[0].lerp(pts[1], 0.06);
                let dir = (pts[1] - pts[0]).normalize_or(Vec3::Z);
                // the car's nose is -Z
                car.rot = glam::Quat::from_rotation_arc(Vec3::NEG_Z, dir);
                car.vel = dir * 12.0;
                let mut c = physics::Controls::default();
                c.throttle = 1.0;
                c.boost = true;
                let mut best = 0.0f32;
                let mut out = false;
                let dbg = std::env::var("DRIVE_DBG").map(|v| v == format!("{si}:{ri}")).unwrap_or(false);
                let mut t = 0.0f32;
                // these routes run 900-2200 m: give each one time to be
                // driven rather than judging it on a fixed clock
                let budget = (total / 13.0).clamp(12.0, 75.0);
                while t < budget {
                    physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                    // how far along the polyline are we, and how far off it?
                    let (mut along, mut off) = (0.0f32, f32::MAX);
                    let mut acc = 0.0f32;
                    for w in pts.windows(2) {
                        let ab = w[1] - w[0];
                        let len = ab.length().max(1e-4);
                        let u = ((car.pos - w[0]).dot(ab) / (len * len)).clamp(0.0, 1.0);
                        let d = (w[0] + ab * u).distance(car.pos);
                        if d < off {
                            off = d;
                            along = acc + u * len;
                        }
                        acc += len;
                    }
                    best = best.max(along / total);
                    if dbg && (t * 8.0).fract() < DT * 8.0 {
                        eprintln!(
                            "  t={:4.2} along={:4.0}% off={:5.2} (r={:.1}) y={:6.1} og={} spd={:5.1} sdf={:6.2}",
                            t, along / total * 100.0, off, r, car.pos.y, car.on_ground as u8, car.vel.length(), world.sdf(car.pos)
                        );
                    }
                    // leaving the pipe at the FAR END is the exit, not a
                    // failure — only count escapes that happen en route
                    // a chamber is wider than its pipes, so 2.2r counted
                    // flying across a junction room as an escape
                    if off > r * 3.2 {
                        if along / total < 0.9 {
                            out = true;
                        }
                        break;
                    }
                    t += DT;
                }
                // falling out is a FAILURE, not progress: the projection
                // keeps advancing while the car flies off, which flattered
                // the first version of this test
                if out {
                    escaped += 1;
                    worst.push((ri, best));
                } else if best > 0.85 {
                    done += 1;
                } else {
                    worst.push((ri, best));
                }
            }
            worst.sort_by(|a, b| a.1.total_cmp(&b.1));
            let checked = n_routes;
            println!(
                "{label:<12} traversed {done}/{checked} routes   fell out of {escaped}   worst: {}",
                worst
                    .iter()
                    .take(4)
                    .map(|(i, f)| format!("#{i} {:.0}%", f * 100.0))
                    .collect::<Vec<_>>()
                    .join("  ")
            );
        }
        return;
    }
    if name == "loop" {
        // Loops must be ONE seamless piece, not a chain. Three things are
        // checked per loop: it is a single shape (a chain would be ~27),
        // its own surface is continuous at every angle, and the bore has no
        // holes in the wall (the old elbow cutters punched clean through).
        let mut pass = true;
        for (si, label) in [(21usize, "PERPLEXUS"), (24, "MONKEY RUN")] {
            let w = World::generate(9, crate::world::style(si), 0.31, 1.0);
            let rings: Vec<(Vec3, glam::Quat, f32, f32, f32)> = w
                .shapes
                .iter()
                .filter_map(|sh| match sh.sdf {
                    crate::world::Sdf::TubeTorus { big, tube, thick } => {
                        Some((sh.pos, sh.inv_rot, big, tube, thick))
                    }
                    _ => None,
                })
                .collect();
            if rings.is_empty() {
                println!("{label:<12} no loops found");
                pass = false;
                continue;
            }
            for (k, &(c, inv_rot, big, tube, thick)) in rings.iter().enumerate() {
                let rot = inv_rot.inverse();
                let (u, v) = (rot * Vec3::X, rot * Vec3::Z);
                let axis = rot * Vec3::Y;
                // 1. the primitive's own surface, sampled all the way round
                let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                // 2. the wall: solid at every angle, top and bottom
                let mut wall_holes = 0;
                let mut open_flags: Vec<bool> = Vec::new();
                // 3. how much of the ring is merged into the entry run
                let mut buried = 0;
                for i in 0..288 {
                    let a = i as f32 / 288.0 * std::f32::consts::TAU;
                    let dir = u * a.cos() + v * a.sin();
                    let centre = c + dir * big;
                    let q = inv_rot * (centre - c);
                    let ring = glam::Vec2::new(glam::Vec2::new(q.x, q.z).length() - big, q.y);
                    let own = (ring.length() - tube).abs() - thick;
                    lo = lo.min(own);
                    hi = hi.max(own);
                    let mut open_here = false;
                    for s in [1.0f32, -1.0] {
                        if w.sdf(centre + axis * (s * tube)) > 0.0 {
                            wall_holes += 1;
                            open_here = true;
                        }
                    }
                    open_flags.push(open_here);
                    if w.sdf(centre) < 0.0 {
                        buried += 1;
                    }
                }
                // A loop MUST be open where you ride in and out. What it must
                // not be is perforated: count contiguous openings, not holes.
                let n = open_flags.len();
                let mut mouths = 0;
                for i in 0..n {
                    if open_flags[i] && !open_flags[(i + n - 1) % n] {
                        mouths += 1;
                    }
                }
                let open_pct = wall_holes as f32 / (n as f32 * 2.0) * 100.0;
                let ripple = hi - lo;
                let bury_pct = buried as f32 / n as f32 * 100.0;
                let ok = ripple < 1e-3 && mouths <= 4 && open_pct < 25.0 && bury_pct < 30.0;
                pass &= ok;
                println!(
                    "{label:<12} loop {k}: 1 shape  ripple {ripple:.5}  mouths {mouths} ({open_pct:.0}% open)  merged {bury_pct:.0}%  {}",
                    if ok { "SEAMLESS" } else { "PROBLEM" }
                );
            }
        }
        // ---- BORE CLEARANCE: how much room does a car actually have? ----
        // The car needs ~1.0 m of radius to fit and wants far more to move.
        let car_need = 1.05f32;
        for (si, label) in [(21usize, "PERPLEXUS"), (24, "MONKEY RUN")] {
            let w = World::generate(9, crate::world::style(si), 0.31, 1.0);
            let mut all: Vec<f32> = Vec::new();
            let dbg = std::env::var("BORE_DBG").is_ok();
            for (ri, (pts, _r)) in w.routes.iter().enumerate() {
                let mut route_min = f32::MAX;
                let mut route_tight = 0;
                let mut route_n = 0;
                for seg in pts.windows(2) {
                    for k in 0..6 {
                        let p = seg[0].lerp(seg[1], k as f32 / 6.0);
                        // waypoints AT chamber walls and mouths sit inside
                        // solid by construction, so sample the open bore:
                        // anything grazing a wall is a junction, not a pipe
                        let d = w.sdf(p);
                        if d > 0.35 {
                            all.push(d); // free radius at the axis
                            route_min = route_min.min(d);
                            route_n += 1;
                            if d < car_need * 1.6 {
                                route_tight += 1;
                            }
                        }
                    }
                }
                if dbg && route_n > 0 {
                    eprintln!(
                        "  {label} route {ri}: {} pts, min {:.2}, tight {:.0}%",
                        route_n,
                        route_min,
                        route_tight as f32 / route_n as f32 * 100.0
                    );
                }
            }
            all.sort_by(f32::total_cmp);
            if all.is_empty() {
                continue;
            }
            let p10 = all[all.len() / 10];
            let med = all[all.len() / 2];
            let tight = all.iter().filter(|&&d| d < car_need * 1.6).count();
            let tpct = tight as f32 / all.len() as f32 * 100.0;
            let ok = p10 > car_need && tpct < 15.0;
            pass &= ok;
            println!(
                "{label:<12} bore: p10 {p10:.2} m  median {med:.2} m  tight {tpct:.0}%  {}",
                if ok { "drivable" } else { "CRAMPED" }
            );
        }
        // ---- KUIPER: do worlds PULL? ----
        {
            let w = World::generate(9, crate::world::style(28), 0.31, 1.0);
            let planet = w
                .grav
                .iter()
                .filter(|n| n.inward)
                .max_by(|a, b| a.r.total_cmp(&b.r));
            match planet {
                Some(n) => {
                    let above = n.pos + Vec3::Y * (n.r * 0.5);
                    let g = w.gravity_dir(above);
                    let toward = (n.pos - above).normalize();
                    let dot = g.dot(toward);
                    let ok = dot > 0.5;
                    pass &= ok;
                    println!(
                        "KUIPER       planet field: dot(down, toward centre) = {dot:.2}  {}",
                        if ok { "attractive" } else { "REPULSIVE" }
                    );
                }
                None => {
                    println!("KUIPER       no planet gravity nodes");
                    pass = false;
                }
            }
        }
        // ---- RING DECKS: can a car actually stand on one? ----
        {
            let cfg = Config::default();
            let mut w = World::generate(9, crate::world::style(28), 0.31, 1.0); // KUIPER
            let deck = w.shapes.iter().find_map(|sh| match sh.sdf {
                crate::world::Sdf::RingDeck { big, wide, half_h } => {
                    Some((sh.pos, sh.inv_rot.inverse(), big, wide, half_h))
                }
                _ => None,
            });
            match deck {
                Some((c, rot, big, wide, half_h)) => {
                    let _ = wide;
                    let up = rot * Vec3::Y;
                    let out = rot * Vec3::X;
                    let mut car = physics::Car::new();
                    car.pos = c + out * big + up * (half_h + 0.45);
                    car.rot = glam::Quat::from_rotation_arc(Vec3::Y, up);
                    car.vel = Vec3::ZERO;
                    let ball = Ball::new();
                    let ctl = physics::Controls::default();
                    let mut grounded = 0;
                    for _ in 0..240 {
                        physics::step_car_only(&mut car, &ball, &w, &ctl, &cfg, DT);
                        if car.on_ground {
                            grounded += 1;
                        }
                    }
                    let held = car.pos.distance(c + out * big) < 6.0;
                    let ok = grounded > 120 && held;
                    pass &= ok;
                    println!(
                        "KUIPER       ring deck: grounded {grounded}/240 frames, stayed on {} {}",
                        if held { "yes" } else { "no" },
                        if ok { "drivable" } else { "NOT DRIVABLE" }
                    );
                }
                None => {
                    println!("KUIPER       no ring decks found");
                    pass = false;
                }
            }
            let _ = &mut w;
        }
        println!("{}", if pass { "LOOPS: PASS" } else { "LOOPS: FAIL" });
        return;
    }
    if name == "touch" {
        // the menu touch model, headless: row hit-testing, slider mapping,
        // fat-finger bar targets, pause buttons, tap-vs-drag classification
        use crate::hud;
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<40} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        // ---- readability + layout standards ----
        let vis = hud::vis_rows(hud::TW_ROW_H, hud::TW_TOP, hud::TW_BOTTOM);
        check("row height is finger-sized", hud::TW_ROW_H >= 0.055);
        check("viewport shows a workable page", (8..=26).contains(&vis));
        check(
            "type scale is ordered and modest",
            hud::T_HINT < hud::T_SECTION
                && hud::T_SECTION < hud::T_BODY
                && hud::T_BODY < hud::T_TITLE
                && hud::T_BODY <= 0.0090,
        );
        // long lists scroll instead of shrinking: selection always visible
        let big = 44usize;
        let mut always_visible = true;
        let mut monotone = true;
        let mut prev = 0usize;
        for sel in 0..big {
            let st = hud::scroll_for(sel, big, vis);
            if sel < st || sel >= st + vis {
                always_visible = false;
            }
            if st < prev {
                monotone = false;
            }
            prev = st;
            if st + vis > big {
                always_visible = false; // never scrolls past the end
            }
        }
        check("scroll keeps the selection on screen", always_visible);
        check("scroll never jumps backwards", monotone);
        check("short lists never scroll", hud::scroll_for(3, 9, vis) == 0);
        // a scrolled viewport hit-tests to ABSOLUTE indices
        let st = hud::scroll_for(30, big, vis);
        let first = hud::hit_row_scrolled(hud::TW_TOP, hud::TW_ROW_H, big, vis, st, hud::TW_TOP);
        let third = hud::hit_row_scrolled(hud::TW_TOP, hud::TW_ROW_H, big, vis, st, hud::TW_TOP - hud::TW_ROW_H * 2.0);
        check("scrolled hit: first visible row", first == Some(st));
        check("scrolled hit: third visible row", third == Some(st + 2));
        let past = hud::hit_row_scrolled(hud::TW_TOP, hud::TW_ROW_H, big, vis, st, hud::TW_TOP - hud::TW_ROW_H * (vis as f32 + 1.0));
        check("scrolled hit: past the page misses", past.is_none());

        // rows: first row under the top, later rows below, nothing above
        let n = 12usize;
        let rh = hud::rows_h(n);
        let first = hud::hit_row(hud::TW_TOP, rh, n, hud::TW_TOP);
        let third = hud::hit_row(hud::TW_TOP, rh, n, hud::TW_TOP - rh * 2.0);
        let above = hud::hit_row(hud::TW_TOP, rh, n, hud::TW_TOP + rh * 2.0);
        let below = hud::hit_row(hud::TW_TOP, rh, n, hud::TW_TOP - rh * (n as f32 + 3.0));
        check("row hit: top row", first == Some(0));
        check("row hit: third row", third == Some(2));
        check("row hit: above the list misses", above.is_none());
        check("row hit: below the list misses", below.is_none());
        // every row is reachable and unique (no gaps a finger can fall into)
        let mut seen = vec![false; n];
        let mut steps = 0;
        let mut y = hud::TW_TOP + rh * 0.5;
        while y > hud::TW_TOP - rh * (n as f32 + 1.0) {
            if let Some(i) = hud::hit_row(hud::TW_TOP, rh, n, y) {
                if i < n {
                    seen[i] = true;
                }
            }
            y -= rh * 0.05;
            steps += 1;
        }
        check("every row is touchable", seen.iter().all(|&b| b) && steps > 0);
        // slider bar mapping + fat targets
        check("bar left end = 0", hud::bar_t(hud::TW_BAR_X0) == 0.0);
        check("bar right end = 1", hud::bar_t(hud::TW_BAR_X1) == 1.0);
        let mid = hud::bar_t((hud::TW_BAR_X0 + hud::TW_BAR_X1) * 0.5);
        check("bar middle = 0.5", (mid - 0.5).abs() < 1e-5);
        check("bar clamps past the ends", hud::bar_t(-1.0) == 0.0 && hud::bar_t(1.0) == 1.0);
        check(
            "fat finger band is generous",
            hud::near_bar(hud::TW_BAR_X0 - 0.08) && hud::near_bar(hud::TW_BAR_X1 + 0.08) && !hud::near_bar(-0.5),
        );
        // pause buttons: all five hit, gaps between them miss, row is centred
        let hits: Vec<Option<usize>> = (0..hud::PAUSE_BTNS.len())
            .map(|i| hud::hit_pause_btn(hud::pbtn_x0(i) + hud::PBTN_W * 0.5, hud::PBTN_Y))
            .collect();
        check(
            "every pause button is tappable",
            hits.iter().enumerate().all(|(i, h)| *h == Some(i)),
        );
        check(
            "taps above/below the buttons miss",
            hud::hit_pause_btn(0.0, hud::PBTN_Y + hud::PBTN_H).is_none()
                && hud::hit_pause_btn(0.0, hud::PBTN_Y - hud::PBTN_H).is_none(),
        );
        let x0 = hud::pbtn_x0(0);
        let xn = hud::pbtn_x0(hud::PAUSE_BTNS.len() - 1) + hud::PBTN_W;
        check("button row is centred", (x0 + xn).abs() < 1e-5);
        check("buttons stay on screen", x0 > -1.0 && xn < 1.0);
        // ---- mobile: nav strip + drag-to-scroll maths ----
        let hits: Vec<Option<usize>> = (0..hud::NAV_BTNS.len())
            .map(|i| hud::hit_nav(hud::nav_x0(i) + hud::NAV_W * 0.5, hud::NAV_Y))
            .collect();
        check(
            "every nav button is tappable",
            hits.iter().enumerate().all(|(i, h)| *h == Some(i)),
        );
        let nx0 = hud::nav_x0(0);
        let nxn = hud::nav_x0(hud::NAV_BTNS.len() - 1) + hud::NAV_W;
        check("nav strip is centred and on screen", (nx0 + nxn).abs() < 1e-5 && nx0 > -1.0 && nxn < 1.0);
        check(
            "nav strip clears the list viewport",
            hud::NAV_Y + hud::NAV_H * 0.5 < hud::TW_BOTTOM,
        );
        // the real path hit-tests through the VIEWPORT, which stops above
        // the strip — so a nav tap can never also hit a row
        check(
            "nav taps do not land on rows",
            hud::hit_row_scrolled(hud::TW_TOP, hud::TW_ROW_H, 40, vis, 0, hud::NAV_Y).is_none(),
        );
        // graphics tiers exist and LOW really is cheaper than HIGH
        let mut lo = crate::config::Config::default();
        crate::config::apply_gfx_tier(&mut lo, 1);
        let mut hi = crate::config::Config::default();
        crate::config::apply_gfx_tier(&mut hi, 3);
        check(
            "LOW preset drops rays, quality and resolution",
            !lo.video.rays && lo.video.quality < hi.video.quality && lo.video.render_scale < hi.video.render_scale,
        );
        // the handheld profile must force low graphics from ANY starting cfg
        let mut phone = crate::config::Config::default();
        crate::platform::Profile::Handheld.apply(&mut phone);
        check(
            "handheld profile forces low graphics + touch",
            !phone.video.rays && phone.video.render_scale <= 0.5 && phone.touch.enabled,
        );

        // every real settings page: how much does it actually scroll?
        println!("{:<14} {:>5} {:>6} {:>7}", "menu", "rows", "pages", "scroll");
        let mut worst = 0.0f32;
        for m in 0..8u8 {
            let rows = crate::tweak_rows(m);
            let pages = rows.len() as f32 / vis as f32;
            worst = worst.max(pages);
            println!(
                "{:<14} {:>5} {:>6.1} {:>7}",
                crate::menu_title(m),
                rows.len(),
                pages,
                if rows.len() > vis { "yes" } else { "no" }
            );
        }
        check("no page is a marathon (<= 2 screens)", worst <= 2.0);
        println!("{}", if pass { "TOUCH MENUS: PASS" } else { "TOUCH MENUS: FAIL" });
        return;
    }
    if name == "gyro" {
        // the ported gyro stack, end to end: protocol parsing, drift
        // calibration, deadzone/scale shaping, and the drive mapping
        use crate::gyro::{parse_line, Gyro};
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<34} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        // 1. the daemon protocol (tilt_sensor_sysfs.c emits these)
        let a1 = parse_line("ACCEL 0.42 -0.15 9.71").unwrap();
        check("ACCEL line parses", (a1.x - 0.42).abs() < 1e-6 && (a1.y + 0.15).abs() < 1e-6 && a1.analog);
        check("header/garbage ignored", parse_line("TILT_SENSOR_V1").is_none() && parse_line("").is_none());
        // 2. monitor-sensor fallback events
        let t1 = parse_line("    Tilt changed: tilted-up").unwrap();
        let t2 = parse_line("Accelerometer orientation changed: right-up").unwrap();
        check("monitor-sensor tilt/orient parse", t1.y < 0.0 && !t1.analog && t2.x > 0.0);
        // 3. calibration kills a biased-at-rest device
        let mut cfg = crate::config::GyroCfg { on: true, sens: 1.0, deadzone: 0.05, smooth: 0.02, invert_x: false, invert_y: false, air_mode: 0, gate: 0 };
        let mut g = Gyro::new(false);
        g.feed_test(0.30, -0.20); // a device that reads non-zero while still
        g.calibrate();
        for _ in 0..140 {
            g.feed_test(0.30, -0.20);
            g.update(1.0 / 60.0, &cfg);
        }
        for _ in 0..60 {
            g.feed_test(0.30, -0.20);
            g.update(1.0 / 60.0, &cfg);
        }
        check("calibration removes rest bias", g.x.abs() < 0.02 && g.y.abs() < 0.02);
        // 4. real motion still gets through, deadzone still bites
        for _ in 0..90 {
            g.feed_test(0.30 + 0.5, -0.20);
            g.update(1.0 / 60.0, &cfg);
        }
        check("motion above deadzone drives", g.x > 0.35);
        for _ in 0..90 {
            g.feed_test(0.30 + 0.02, -0.20);
            g.update(1.0 / 60.0, &cfg);
        }
        check("small jitter stays in deadzone", g.x.abs() < 0.001);
        // 5. sensitivity scales, output clamps to the stick range
        cfg.sens = 4.0;
        for _ in 0..120 {
            g.feed_test(0.30 + 0.9, -0.20);
            g.update(1.0 / 60.0, &cfg);
        }
        check("output clamped to stick range", g.x <= 1.0 + 1e-5 && g.x > 0.95);
        // 6. the drive mapping: ground steers, air flick-aims, gate closes
        g.set_live_for_test(true);
        let (gs, gp, gr) = g.drive(&cfg, false, true);
        check("ground: tilt steers only", gs.abs() > 0.5 && gp == 0.0 && gr == 0.0);
        let (as_, ap, ar) = g.drive(&cfg, true, true);
        check("air mode 0: tilt rolls", ar.abs() > 0.5 && as_ == 0.0 && ap.abs() <= 1.0);
        cfg.air_mode = 1;
        let (ys, _, yr) = g.drive(&cfg, true, true);
        check("air mode 1: tilt yaws", ys.abs() > 0.5 && yr == 0.0);
        let closed = g.drive(&cfg, true, false);
        check("closed gate emits nothing", closed == (0.0, 0.0, 0.0));
        // 7. discrete impulses decay to rest (no runaway from event sources)
        let mut g2 = Gyro::new(false);
        g2.impulse_test(1.0, 0.0);
        let mut peak: f32 = 0.0;
        for _ in 0..30 {
            g2.update(1.0 / 60.0, &cfg);
            peak = peak.max(g2.x.abs());
        }
        for _ in 0..180 {
            g2.update(1.0 / 60.0, &cfg);
        }
        check("impulse kicks then decays", peak > 0.2 && g2.x.abs() < 0.02);
        println!("{}", if pass { "GYRO: PASS" } else { "GYRO: FAIL" });
        return;
    }
    if name == "flip" {
        // RL-style flip audit: every cardinal combo, from ground and air.
        // A flip must complete ONE full turn about ONE axis and finish at
        // the orientation it started from (wheels-down stays wheels-down).
        let cfg = Config::default();
        let world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
        let ball = Ball::new();
        let combos: [(&str, f32, f32); 8] = [
            ("forward", 1.0, 0.0),
            ("back", -1.0, 0.0),
            ("left", 0.0, -1.0),
            ("right", 0.0, 1.0),
            ("fwd+left", 1.0, -0.7),
            ("fwd+right", 1.0, 0.7),
            ("back+left", -1.0, -0.7),
            ("diag even", 0.8, 0.8),
        ];
        // rotation profile: how the turn is distributed across the flip
        {
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
            let mut c = physics::Controls::default();
            for _ in 0..60 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            c.jump_pressed = true;
            physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            c.jump_pressed = false;
            for _ in 0..12 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            c.throttle = 1.0;
            c.jump_pressed = true;
            physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            c.jump_pressed = false;
            c.throttle = 0.0;
            let len = car.dodge_len_for_sim();
            let mut samples: Vec<f32> = Vec::new();
            let mut t = 0.0f32;
            while t < len {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                samples.push(car.flip_turn().to_degrees());
                t += DT;
            }
            print!("flip profile ({:.2}s):", len);
            let n = samples.len().max(1);
            let mut prev = 0.0f32;
            let mut peak: f32 = 0.0;
            for d in 0..10 {
                let idx = ((d + 1) * n / 10).min(n) - 1;
                let v = samples[idx];
                let slice = (v - prev) / 3.6;
                peak = peak.max(slice);
                print!(" {slice:.0}%");
                prev = v;
            }
            println!("   peak decile {peak:.0}%");
        }
        for (air, label) in [(false, "GROUND START"), (true, "AIR START")] {
            println!("-- {label}");
            println!("{:<11} {:>8} {:>9} {:>9} {:>8}", "input", "axis", "turn", "err_deg", "up.y");
            for (name2, thr, steer) in combos {
                let mut car = physics::Car::new();
                // both cases start grounded: a dodge is only legal after a jump —
                // "air start" means the FLIP begins mid-air at an arbitrary
                // attitude, which is exactly the post-jump state
                car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
                let mut start = car.rot;
                let mut c = physics::Controls::default();
                if air {
                    // jump for a legal dodge window, then take an ARBITRARY
                    // attitude mid-air: the flip must return to THAT pose
                    for _ in 0..60 {
                        physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                    }
                    c.jump_pressed = true;
                    physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                    c.jump_pressed = false;
                    for _ in 0..14 {
                        physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                    }
                    car.rot = (glam::Quat::from_rotation_x(0.55) * glam::Quat::from_rotation_z(0.35)).normalize();
                    car.ang = Vec3::ZERO;
                } else {
                    for _ in 0..60 {
                        physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                    }
                    c.jump_pressed = true;
                    physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                    c.jump_pressed = false;
                    for _ in 0..12 {
                        physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                    }
                }
                // the dodge
                c.throttle = thr;
                c.steer = steer;
                c.jump_pressed = true;
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                c.jump_pressed = false;
                if air && std::env::var("FLIP_DBG").is_ok() {
                    eprintln!("  air gate: has_flip={} window_ok={} on_ground={}", car.has_flip(), car.dodge_window_open(), car.on_ground);
                }
                let axis = car.flip_axis_name();
                start = car.dodge_start_pose(); // the pose the flip began from
                // release the sticks: isolate flip fidelity from driving
                c.throttle = 0.0;
                c.steer = 0.0;
                let mut turn_max: f32 = 0.0;
                let mut end_rot = car.rot;
                let mut was = false;
                for _ in 0..150 {
                    physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                    turn_max = turn_max.max(car.flip_turn());
                    if was && !car.is_flipping() {
                        end_rot = car.rot; // the pose the instant the flip closes
                        was = false;
                    }
                    was = was || car.is_flipping();
                }
                // settle out
                for _ in 0..240 {
                    physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                }
                let dq = (start.inverse() * end_rot).normalize();
                let err = 2.0 * dq.w.abs().clamp(0.0, 1.0).acos();
                println!(
                    "{:<11} {:>8} {:>8.0}d {:>8.1} {:>8.2}",
                    name2,
                    axis,
                    turn_max.to_degrees(),
                    err.to_degrees(),
                    car.up().y
                );
            }
        }
        return;
    }
    if name == "mech" {
        // the new mechanics through the REAL step(): tile strikes open
        // holes (DROPSHOT), pit falls score, asteroid bodies get shoved
        let cfg = Config::default();
        let mut world = World::generate(9, crate::world::style(26), 0.31, 1.0); // DROPSHOT
        let mut car = physics::Car::new();
        car.pos = Vec3::new(40.0, world.ground_h(40.0, 40.0) + 0.4, 40.0); // out of play
        let mut ball = Ball::new();
        let tile0 = world.tiles[40].base; // a mid-deck tile
        let c = physics::Controls::default();
        let mut ev = StepEvents::default();
        let holes_before = world.tiles.iter().filter(|t| t.down_t > 0.0).count();
        let mut scored = 0;
        for round in 0..6 {
            ball.pos = tile0 + Vec3::new(0.0, 6.0, 0.0);
            ball.vel = Vec3::new(0.0, -18.0, 0.0);
            for _ in 0..240 {
                ev.goals.clear();
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                scored += ev.goals.len();
            }
            let _ = round;
        }
        let holes_after = world.tiles.iter().filter(|t| t.down_t > 0.0).count();
        println!("dropshot: holes {} -> {}   pit scores: {}", holes_before, holes_after, scored);
        let ok1 = holes_after > 0 && scored > 0;

        let mut world = World::generate(9, crate::world::style(28), 0.31, 1.0); // KUIPER
        let bi = world
            .bodies
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.mass.total_cmp(&b.1.mass))
            .map(|(i, _)| i)
            .unwrap();
        let (bp, v0) = (world.bodies[bi].pos, world.bodies[bi].vel.length());
        // deterministic: spawn in contact (the field is busy — a scene shot
        // deflects off OTHER asteroids and planet gravity long before bi)
        let rb = world.bodies[bi].r + ball.r;
        ball.pos = bp - Vec3::new(rb * 0.8, 0.0, 0.0);
        ball.vel = Vec3::new(22.0, 0.0, 0.0);
        for _ in 0..20 {
            ev.goals.clear();
            physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
        }
        let v1 = world.bodies[bi].vel.length();
        println!("kuiper: lightest asteroid speed {v0:.2} -> {v1:.2} after a ball slam");
        let ok2 = v1 > v0 + 1.0;
        // ---- DROPSHOT ramp walls: is the bowl actually drivable? ----
        let world3 = World::generate(9, crate::world::style(26), 0.31, 1.0);
        let deck_y = world3.drop_pit.unwrap_or(6.0);
        let mut climbed = 0.0f32;
        {
            let mut car = physics::Car::new();
            // start on the deck near the edge, aimed at the wall
            car.pos = Vec3::new(52.0, deck_y + 0.5, 0.0);
            car.rot = glam::Quat::from_rotation_arc(Vec3::NEG_Z, Vec3::X);
            car.vel = Vec3::X * 20.0;
            let mut c2 = physics::Controls::default();
            c2.throttle = 1.0;
            c2.boost = true;
            let b2 = Ball::new();
            for _ in 0..240 {
                physics::step_car_only(&mut car, &b2, &world3, &c2, &cfg, DT);
                climbed = climbed.max(car.pos.y - deck_y);
            }
        }
        let ok3 = climbed > 6.0;
        println!("dropshot: ramp wall climb {climbed:.1} m above the deck  {}",
            if ok3 { "drivable verticality" } else { "NOT CLIMBABLE" });
        println!("{}", if ok1 && ok2 && ok3 { "MECHANICS: PASS" } else { "MECHANICS: FAIL" });
        return;
    }
    if name == "mp" {
        // END-TO-END fleet multiplayer on loopback: two full Net stacks in
        // one process share a temp FileStore (the bsgames control plane),
        // then run the REAL UDP data plane — HELLO, WELCOME, state sync.
        let store = std::env::temp_dir().join(format!("skyd-mp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&store);
        let mut a = crate::net::Net::new();
        let mut b = crate::net::Net::new();
        println!("host: {}", a.host_with("ALICE", store.clone()));
        println!("join: {}", b.join_with("BOB", "ALICE", store.clone()));
        let mut ev: Vec<crate::net::NetEvent> = Vec::new();
        let mk = |x: f32| crate::net::MyState {
            car: 0,
            flip: false,
            boosting: false,
            pos: Vec3::new(x, 1.0, 0.0),
            vel: Vec3::new(1.0, 0.0, 0.0),
            rot: glam::Quat::IDENTITY,
            wheel_spin: 0.0,
            steer_vis: 0.0,
        };
        let mut t = 0.0f32;
        while t < 3.0 {
            a.tick(DT, &mk(-4.0 + t), Vec3::new(0.0, 1.0, 8.0), Vec3::ZERO, glam::Quat::IDENTITY, 20, 9, &mut ev);
            b.tick(DT, &mk(4.0 - t), Vec3::new(0.0, 1.0, 8.0), Vec3::ZERO, glam::Quat::IDENTITY, 20, 9, &mut ev);
            for e in ev.drain(..) {
                if let crate::net::NetEvent::Joined(n) = e {
                    println!("event: {n} JOINED");
                }
            }
            a.smooth(DT);
            b.smooth(DT);
            t += DT;
            std::thread::sleep(std::time::Duration::from_micros(300));
        }
        let ap: Vec<f32> = a.peers().map(|p| p.shown.x).collect();
        let bp: Vec<f32> = b.peers().map(|p| p.shown.x).collect();
        println!("host sees {} peer(s) at x={:?}", ap.len(), ap);
        println!("join sees {} peer(s) at x={:?}", bp.len(), bp);
        let ok = ap.len() == 1
            && bp.len() == 1
            && (ap[0] - 1.0).abs() < 1.6
            && (bp[0] - (-1.0)).abs() < 1.6;
        println!("{}", if ok { "MP LOOPBACK: PASS" } else { "MP LOOPBACK: FAIL" });
        let _ = std::fs::remove_dir_all(&store);
        return;
    }
    if name == "cam" {
        // does CamCfg actually reach the boom? Step the pure camera with
        // two configs and print the settled geometry.
        let world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
        let ball = Ball::new();
        let mut car = physics::Car::new();
        car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.5, 0.0);
        for (label, dist, height, ang) in [
            ("near/low/flat", 3.0f32, 0.6f32, 0.0f32),
            ("far/high/down", 9.0, 3.0, -14.0),
        ] {
            // the exact target: Fairphone 5 / postmarketOS / SC7280
        let fp5 = crate::platform::classify(
            "Linux version 7.1.2 #1-postmarketos-qcom-sc7280 SMP PREEMPT Mon Jul 13 21:02:06 UTC",
            "fp5-black",
            "Fairphone 5",
            true,
        );
        println!("fp5-black        : {:?}  ({})", fp5.profile, fp5.why);
        let pc = crate::platform::classify("Linux version 6.8.0-generic #1 SMP", "workstation", "", false);
        println!("desktop x86      : {:?}  ({})", pc.profile, pc.why);
        let target_ok = fp5.profile == crate::platform::Profile::Handheld
            && pc.profile == crate::platform::Profile::Desktop;
        let mut cfg = crate::config::Config::default();
            cfg.cam.dist = dist;
            cfg.cam.height = height;
            cfg.cam.angle_deg = ang;
            let mut cam = crate::camera::Camera::new();
            for _ in 0..600 {
                cam.update(&car, &ball, &world, &cfg.cam, false, false, DT);
            }
            let boom = cam.pos - car.pos;
            let look_up = (cam.look - car.pos).dot(Vec3::Y);
            println!(
                "{:<14} |boom|={:5.2}  boom_up={:5.2}  look_up={:5.2}",
                label,
                boom.length(),
                boom.dot(Vec3::Y),
                look_up
            );
        }
        return;
    }
    if name == "jump" {
        // press-to-liftoff latency across surfaces and timings: the ground
        // jump must be as instant as the flip
        let cfg = Config::default();
        let mut row = |label: &str, style_i: usize, presses: &[f32], drive: f32| {
            let world = World::generate(31, crate::world::style(style_i), 0.31, 1.0);
            let mut worst = 0.0f32;
            let mut fails = 0;
            for &pt in presses {
                let mut ball = Ball::new();
                ball.pos = Vec3::new(300.0, 1.0, 300.0);
                let mut car = physics::Car::new();
                car.pos = Vec3::new(4.0, world.ground_h(4.0, 4.0) + 0.31, 4.0);
                let mut c = physics::Controls::default();
                c.throttle = drive;
                let mut t = 0.0f32;
                let mut lift: Option<f32> = None;
                while t < pt + 1.0 {
                    c.jump_pressed = (t - pt).abs() < DT * 0.5;
                    physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                    if t >= pt && lift.is_none() && !car.on_ground && car.vel.y > 1.5 {
                        lift = Some(t - pt);
                    }
                    t += DT;
                }
                match lift {
                    Some(l) => worst = worst.max(l),
                    None => fails += 1,
                }
            }
            println!("{:<26} worst={:5.0} ms  fails={}/{}", label, worst * 1000.0, fails, presses.len());
        };
        let presses: Vec<f32> = (0..24).map(|k| 0.6 + k as f32 * 0.113).collect();
        row("flat idle", ARENA, &presses, 0.0);
        row("flat driving", ARENA, &presses, 1.0);
        row("bumpy idle (TEMPLE)", 9, &presses, 0.0);
        row("bumpy driving (TEMPLE)", 9, &presses, 1.0);
        row("bumpy driving (CARNIVAL)", 18, &presses, 1.0);
        return;
    }
    if name == "land" {
        println!("landing matrix - drop from 5 m in nine poses, ARENA flat");
        land_matrix();
        return;
    }
    let list: Vec<&str> = if name == "all" {
        vec!["air1", "pass2", "pass3", "pass4", "defend1"]
    } else {
        vec![name]
    };
    println!("skydribble sim - fixed 120 Hz, ARENA CLASSIC, {trials} trials each");
    println!("{:<9} {:>6} {:>7} {:>7} {:>7} {:>7} {:>9} {:>8}", "scenario", "goals", "avg_t", "airtch", "resets", "passes", "def_trav", "style");
    for sc in list {
        let Some((n, def)) = scenario(sc) else {
            println!("unknown scenario {sc}");
            continue;
        };
        let (mut goals, mut tt, mut at, mut rs, mut ps, mut dtr, mut st) = (0u64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for k in 0..trials {
            let o = run_one(1000 + k * 7, n, def, 28.0);
            if let Some(g) = o.goal_t {
                goals += 1;
                tt += g;
            }
            at += o.air_touches as f32;
            rs += o.resets as f32;
            ps += o.passes as f32;
            dtr += o.defender_travel;
            st += o.style;
        }
        let f = trials as f32;
        println!(
            "{:<9} {:>4}/{:<1} {:>7.1} {:>7.1} {:>7.2} {:>7.1} {:>9.1} {:>8.1}",
            sc,
            goals,
            trials,
            if goals > 0 { tt / goals as f32 } else { 0.0 },
            at / f,
            rs / f,
            ps / f,
            dtr / f,
            st / f
        );
    }
}
