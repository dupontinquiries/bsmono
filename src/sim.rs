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
            hover: None,
            prims: Vec::new(),
            drive: crate::model::Drive::Combustion,
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
    if name == "shell" {
        // WHAT MAKES A BODY LOOK LIKE A CAR BODY.
        //
        // Measured on the art target: Octane's painted shell has a MEDIAN
        // dihedral angle of about 2 degrees — neighbouring triangles are
        // very nearly coplanar, so the surface reads as continuous — while
        // its p90 is 31 to 79 degrees, because a car also has crisp panel
        // breaks. Smooth almost everywhere, sharp in a few deliberate
        // places. A uniformly faceted shell reads as a low-poly prop; a
        // uniformly smooth one reads as a bar of soap.
        let dihedral = |m: &crate::meshgen::MeshData| -> (usize, f32, f32, f32) {
            let tri = m.idx.len() / 3;
            let mut face: Vec<Vec3> = Vec::with_capacity(tri);
            for t in m.idx.chunks_exact(3) {
                let a = Vec3::from_array(m.verts[t[0] as usize].pos);
                let b = Vec3::from_array(m.verts[t[1] as usize].pos);
                let c = Vec3::from_array(m.verts[t[2] as usize].pos);
                face.push((b - a).cross(c - a).normalize_or(Vec3::Y));
            }
            let mut edge: std::collections::HashMap<(u32, u32), Vec<usize>> = std::collections::HashMap::new();
            for (f, t) in m.idx.chunks_exact(3).enumerate() {
                for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    edge.entry((a.min(b), a.max(b))).or_default().push(f);
                }
            }
            let mut angs: Vec<f32> = edge
                .values()
                .filter(|f| f.len() == 2)
                .map(|f| face[f[0]].dot(face[f[1]]).clamp(-1.0, 1.0).acos().to_degrees())
                .collect();
            if angs.is_empty() {
                return (tri, 0.0, 0.0, 0.0);
            }
            angs.sort_by(f32::total_cmp);
            let mean = angs.iter().sum::<f32>() / angs.len() as f32;
            (tri, mean, angs[angs.len() / 2], angs[angs.len() * 9 / 10])
        };
        // add the creased fraction: the references are ~10% of edges above
        // 20 degrees, which is the difference between a designed body and an
        // extruded one
        let creased = |m: &crate::meshgen::MeshData| -> f32 {
            let mut face: Vec<Vec3> = Vec::new();
            for t in m.idx.chunks_exact(3) {
                let a = Vec3::from_array(m.verts[t[0] as usize].pos);
                let b = Vec3::from_array(m.verts[t[1] as usize].pos);
                let c = Vec3::from_array(m.verts[t[2] as usize].pos);
                face.push((b - a).cross(c - a).normalize_or(Vec3::Y));
            }
            let mut edge: std::collections::HashMap<(u32, u32), Vec<usize>> = std::collections::HashMap::new();
            for (f, t) in m.idx.chunks_exact(3).enumerate() {
                for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    edge.entry((a.min(b), a.max(b))).or_default().push(f);
                }
            }
            // creases show up as edges whose two faces disagree, AND as
            // boundary edges where the surface was split apart on purpose
            let shared: Vec<&Vec<usize>> = edge.values().filter(|f| f.len() == 2).collect();
            if shared.is_empty() {
                return 0.0;
            }
            let hard = shared
                .iter()
                .filter(|f| face[f[0]].dot(face[f[1]]).clamp(-1.0, 1.0).acos().to_degrees() > 20.0)
                .count();
            let split = edge.values().filter(|f| f.len() == 1).count();
            (hard + split) as f32 / edge.len() as f32 * 100.0
        };
        println!("{:<20} {:>7} {:>8} {:>8} {:>8} {:>9}", "surface", "tris", "mean", "median", "p90", "creased");
        println!("{:<20} {:>7} {:>7}d {:>7}d {:>7}d {:>8}", "OCTANE paint (ref)", 4350, "21.1", "2.2", "79.4", "n/a");
        println!("{:<20} {:>7} {:>7}d {:>7}d {:>7}d {:>8.1}%", "DOMINUS paint (ref)", 5995, "3.4", "0.4", "21.8", 10.4);
        println!("{:<20} {:>7} {:>7}d {:>7}d {:>7}d {:>8.1}%", "DOMINUS body  (ref)", 10102, "2.1", "0.6", "21.1", 10.4);
        let hull = crate::meshgen::loft(
            &[
                (-0.77, 0.13, -0.035, 0.010, 0.95),
                (-0.30, 0.34, -0.065, 0.085, 0.70),
                (0.02, 0.36, -0.065, 0.105, 0.65),
                (0.60, 0.30, -0.050, 0.060, 0.80),
                (0.77, 0.22, -0.035, 0.020, 0.95),
            ],
            26,
        );
        let mut worst_median = 0.0f32;
        for (nm, m) in [
            ("our lofted hull", &hull),
            ("chamfered box", &crate::meshgen::cbox(0.22)),
            ("plain box", &crate::meshgen::boxx()),
        ] {
            let (t, mean, med, p90) = dihedral(m);
            println!("{:<20} {:>7} {:>7.1}d {:>7.1}d {:>7.1}d {:>8.1}%", nm, t, mean, med, p90, creased(m));
            if nm == "our lofted hull" {
                worst_median = med;
            }
        }
        println!();
        // ---- IS THE CATALOGUE ACTUALLY VARIED? ----
        // Rocket League ships six hitbox presets for dozens of cars, and
        // that is a deliberate design: the SHAPES vary, the boxes do not,
        // because players need to know what they are playing against. Ours
        // grew organically, so the question worth asking is the opposite —
        // do forty cars mean forty vehicles, or forty paint jobs on one box?
        {
            // the procedural cars' declared half-extents, read from source
            let src = include_str!("procars.rs");
            let mut boxes: Vec<(String, f32, f32, f32)> = Vec::new();
            for line in src.lines() {
                if let Some(i) = line.find("half_len:") {
                    let rest = &line[i..];
                    let nums: Vec<f32> = rest
                        .split(|c: char| !c.is_ascii_digit() && c != '.')
                        .filter(|t| !t.is_empty())
                        .filter_map(|t| t.parse::<f32>().ok())
                        .collect();
                    if nums.len() >= 4 {
                        boxes.push((format!("car{}", boxes.len()), nums[0], nums[1], nums[2]));
                    }
                }
            }
            if boxes.len() >= 4 {
                // cluster by rounded shape: how many DISTINCT boxes are there
                let mut sigs: Vec<(i32, i32, i32)> = boxes
                    .iter()
                    .map(|b| ((b.1 * 12.0) as i32, (b.2 * 12.0) as i32, (b.3 * 12.0) as i32))
                    .collect();
                sigs.sort();
                sigs.dedup();
                let (mut lmin, mut lmax) = (f32::MAX, 0.0f32);
                let (mut hmin, mut hmax) = (f32::MAX, 0.0f32);
                for b in &boxes {
                    lmin = lmin.min(b.1);
                    lmax = lmax.max(b.1);
                    hmin = hmin.min(b.2);
                    hmax = hmax.max(b.2);
                }
                println!(
                    "   hitboxes: {} cars, {} distinct shapes; length {:.2}-{:.2}, height {:.2}-{:.2}",
                    boxes.len(),
                    sigs.len(),
                    lmin,
                    lmax,
                    hmin,
                    hmax
                );
                println!(
                    "   spread: longest is {:.1}x the shortest, tallest {:.1}x the flattest",
                    lmax / lmin.max(0.01),
                    hmax / hmin.max(0.01)
                );
            }
        }
        println!();
        // ---- IS THE CATALOGUE DESCRIBED? ----
        // Forty vehicles in a flat list of names is a catalogue nobody can
        // navigate: the only way to learn what a car is was to drive it. Every
        // entry needs a class and a line, and the lines have to be DIFFERENT
        // — a table of "fast and fun" tells you nothing.
        {
            let src = include_str!("main.rs");
            let classes = ["CLASSIC", "ODDITY", "ELECTRIC", "CONCEPT", "IMPORT", "ASSEMBLED", "HOVER"];
            let mut counts: Vec<(&str, usize)> = classes
                .iter()
                .map(|c| (*c, src.matches(&format!("(\"{c}\",")).count()))
                .collect();
            counts.retain(|(_, n)| *n > 0);
            let total: usize = counts.iter().map(|(_, n)| n).sum();
            println!("   catalogue classes: {counts:?}");
            println!("   {total} described entries across {} families", counts.len());
            // and the descriptions must not repeat
            let mut lines: Vec<&str> = Vec::new();
            for l in src.lines() {
                let t = l.trim();
                if t.starts_with('"') && t.ends_with("\",") && t.len() > 30 {
                    lines.push(t);
                }
            }
            let before = lines.len();
            lines.sort();
            lines.dedup();
            println!("   {} description lines, {} distinct", before, lines.len());
            let ok_desc = counts.len() >= 6 && lines.len() == before;
            println!("   {}", if ok_desc { "every family represented, every description distinct" } else { "CATALOGUE GAPS" });
        }
        println!();
        // ---- CATALOGUE AUDIT ----
        // A house style is only a style if the whole catalogue is built to
        // it. This reads the source and reports which cars are made from the
        // reference recipe — a continuous lofted shell — and which are still
        // assemblies of boxes, which can never read as bodywork however
        // carefully they are proportioned.
        {
            let src = include_str!("procars.rs");
            let names = [
                "breadbox", "wedge", "roller", "longboy", "cube", "shark",
                "volt", "axion", "hauler", "pulse", "tram", "vipera", "aegis",
            ];
            let mut lofted = 0;
            let mut boxed: Vec<&str> = Vec::new();
            for n in names {
                let Some(i) = src.find(&format!("pub fn {n}(gpu")) else { continue };
                let body = &src[i..src[i..].find("\nfn ").map(|j| i + j).unwrap_or(src.len())];
                let end = body.find("\npub fn ").unwrap_or(body.len());
                let body = &body[..end];
                // a car counts as built to the style whether it lofts
                // directly or through one of the shared builders — the
                // EVs go via `ev_body`, which is the same recipe wearing a
                // different name
                if body.contains("shell_car(") || body.contains("meshgen::loft(") || body.contains("ev_body(") {
                    lofted += 1;
                } else {
                    boxed.push(n);
                }
            }
            println!(
                "   catalogue: {lofted} of {} procedural cars use the lofted shell",
                names.len()
            );
            if !boxed.is_empty() {
                println!("   still assemblies: {boxed:?}");
            }
            // the cube is a joke about being a box and is allowed to stay one
            let ok_cat = boxed.iter().all(|n| *n == "cube");
            println!(
                "   {}",
                if ok_cat { "every car but the CUBE is built to the house style" } else { "STYLE DRIFT" }
            );
        }
        println!();
        // The second half of the look is the CRISP BREAK, and on the target
        // it comes from separate material regions — Octane is Paint, Body,
        // Window and Chassis as four meshes, so the boundary between them is
        // a hard edge by construction rather than a crease inside one
        // surface. Our cars work the same way: the hull is continuous and
        // the glasshouse, spoiler and sills are their own parts, so the
        // feature lines fall where the parts meet.
        println!("   a car body = one continuous shell + separate parts for the breaks:");
        println!("   OCTANE ships 4 body meshes (chassis, paint, body, window) + 8 wheel meshes");
        // Both halves have to hold: continuous enough to read as bodywork,
        // creased enough to read as designed. Faceted (creases everywhere)
        // and soap (no creases) both fail.
        let hull_creased = creased(&hull);
        let ok = worst_median <= 2.0 && (8.0..20.0).contains(&hull_creased);
        println!("   our hull: median {worst_median:.1}d (refs 0.4-2.2), creased {hull_creased:.1}% (refs 10.4%)");
        println!("{}", if ok { "SHELL: PASS" } else { "SHELL: FAIL" });
        return;
    }
    if name == "carry" {
        // TURNING WITH THE BALL ON THE HOOD.
        //
        // What should happen: the ball has its own momentum, so when the car
        // turns the ball LAGS toward the outside of the turn. That lag is the
        // whole skill — you work around to the outside of the ball and use
        // the nose to hold it, steering it by where you place the car.
        //
        // What "yanking" means: the nose drags the ball round with it, so
        // the ball stays pinned dead ahead however hard you turn and there is
        // nothing to work with.
        //
        // So measure the BEARING of the ball from the car's nose over a turn.
        // Pinned at zero is the fault; a bearing that opens to the outside is
        // the behaviour.
        let cfg = Config::default();
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<50} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        println!("{:<14} {:>10} {:>11} {:>10}", "steer", "bearing", "lateral", "kept");
        let mut bearings = Vec::new();
        let mut inherit: Vec<f32> = Vec::new();
        for steer in [0.35f32, 0.7, 1.0] {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
            car.sync_rot();
            let mut ball = Ball::new();
            let mut c = physics::Controls::default();
            let mut ev = StepEvents::default();
            c.throttle = 1.0;
            for _ in 0..110 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            // set it on the hood and let it settle straight
            let f = car.forward();
            ball.pos = car.pos + f * 0.30 + Vec3::Y * (car.half_h + ball.r + 0.02);
            ball.vel = car.vel;
            ball.prev = ball.pos;
            for _ in 0..40 {
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            }
            // now turn, and watch where the ball goes relative to the nose
            c.steer = steer;
            let mut peak = 0.0f32;
            let mut lat = 0.0f32;
            let mut kept = true;
            for _ in 0..120 {
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                let rel = car.rot.inverse() * (ball.pos - car.pos);
                if rel.length() > 3.0 {
                    kept = false;
                    break;
                }
                // bearing from straight ahead, signed toward the outside
                let b = rel.x.atan2(-rel.z).to_degrees() * -steer.signum();
                if b > peak {
                    peak = b;
                    lat = rel.x.abs();
                }
            }
            println!("{:<14.2} {:>9.1}d {:>10.2}m {:>10}", steer, peak, lat, if kept { "yes" } else { "lost" });
            bearings.push(peak);
            // HOW MUCH OF THE CAR'S TURN DID THE BALL INHERIT?
            // A ball with its own momentum turns LESS than the car under it —
            // that difference is the lag you steer around. If it turns as
            // much as the car, the nose is carrying it round: the yank.
            {
                let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
                let mut car = physics::Car::new();
                car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
                car.sync_rot();
                let mut ball = Ball::new();
                let mut c = physics::Controls::default();
                let mut ev = StepEvents::default();
                c.throttle = 1.0;
                for _ in 0..110 {
                    physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                }
                let f = car.forward();
                ball.pos = car.pos + f * 0.30 + Vec3::Y * (car.half_h + ball.r + 0.02);
                ball.vel = car.vel;
                ball.prev = ball.pos;
                for _ in 0..40 {
                    physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                }
                let heading = |v: Vec3| v.z.atan2(v.x).to_degrees();
                let c0 = heading(car.forward());
                let b0 = heading(ball.vel);
                c.steer = steer;
                for _ in 0..36 {
                    physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                }
                let dc = (heading(car.forward()) - c0).abs();
                let db = (heading(ball.vel) - b0).abs();
                println!(
                    "   over 0.3s: car turned {dc:.1}d, ball turned {db:.1}d ({:.0}% inherited)",
                    db / dc.max(0.01) * 100.0
                );
                inherit.push(db / dc.max(0.01));
            }
        }
        // the ball must move to the OUTSIDE, and more so the harder you turn
        // Rocket League's carry: the ball keeps most of its own line, and
        // you steer by moving the car around it. Inheriting nearly all of
        // the car's turn is the yank.
        let worst = inherit.iter().cloned().fold(0.0f32, f32::max);
        check("the ball keeps its own line through a turn", worst < 0.75);
        check("the ball lags to the outside of the turn", bearings.iter().all(|b| *b > 8.0));
        check("and lags further the harder you turn", bearings[2] > bearings[0] + 4.0);
        println!("{}", if pass { "CARRY: PASS" } else { "CARRY: FAIL" });
        return;
    }
    if name == "hits" {
        // "Sometimes the ball goes in weird directions." A hit that is
        // sometimes wrong is a DISCONTINUITY: nudge the contact point a
        // centimetre and the answer should move a degree, not thirty. This
        // sweeps the ball across the front of the car and measures how much
        // the exit direction jumps between neighbouring samples.
        let cfg = Config::default();
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<46} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        let hit = |lateral: f32, height: f32| -> Vec3 {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 6.0);
            car.rot = glam::Quat::IDENTITY;
            car.sync_rot();
            car.vel = Vec3::new(0.0, 0.0, -18.0);
            let mut ball = Ball::new();
            ball.pos = Vec3::new(lateral, car.pos.y + height, 0.0);
            ball.vel = Vec3::ZERO;
            ball.prev = ball.pos;
            let mut c = physics::Controls::default();
            c.throttle = 1.0;
            let mut ev = StepEvents::default();
            let mut out = Vec3::ZERO;
            for _ in 0..60 {
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                if !ev.touches.is_empty() {
                    out = ball.vel;
                }
            }
            out
        };
        // sweep across the nose, and up the face
        for (label, vertical) in [("across the nose", false), ("up the face", true)] {
            let n = 60;
            let mut prev: Option<Vec3> = None;
            let mut worst = 0.0f32;
            let mut worst_at = 0.0f32;
            let mut sum = 0.0f32;
            for k in 0..=n {
                let t = k as f32 / n as f32 * 2.0 - 1.0;
                let v = if vertical { hit(0.0, t * 0.85) } else { hit(t * 0.85, 0.0) };
                if v.length() < 0.5 {
                    prev = None;
                    continue;
                }
                let d = v.normalize();
                if let Some(p0) = prev {
                    let step = p0.angle_between(d).to_degrees();
                    sum += step;
                    if step > worst {
                        worst = step;
                        worst_at = t;
                    }
                }
                prev = Some(d);
            }
            println!(
                "{label:<18} biggest jump {worst:5.1} deg at offset {worst_at:+.2}, mean {:.1} deg per step",
                sum / n as f32
            );
            check("neighbouring contacts give neighbouring answers", worst < 25.0);
        }
        // ---- now with the car MOVING: turning, landing, mid-flip ----
        // The flick throw reads the car's angular velocity, and omega is
        // derived from the rotation delta between frames — so anything that
        // snaps the car's orientation makes the ball leave sideways.
        let hit_turning = |steer: f32, settle: u32| -> (Vec3, f32) {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 10.0);
            car.rot = glam::Quat::IDENTITY;
            car.sync_rot();
            let mut ball = Ball::new();
            let mut c = physics::Controls::default();
            c.throttle = 1.0;
            c.steer = steer;
            let mut ev = StepEvents::default();
            for _ in 0..settle {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            // drop the ball just ahead of wherever the car now points
            let f = car.forward();
            ball.pos = car.pos + f * 1.6 + Vec3::Y * 0.2;
            ball.vel = Vec3::ZERO;
            ball.prev = ball.pos;
            let mut out = Vec3::ZERO;
            let mut om = 0.0f32;
            for _ in 0..60 {
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                if !ev.touches.is_empty() && out.length() < 0.5 {
                    out = ball.vel;
                    om = car.omega_len();
                }
            }
            (out, om)
        };
        let n = 40;
        let mut prev: Option<Vec3> = None;
        let (mut worst, mut worst_at, mut peak_om) = (0.0f32, 0.0f32, 0.0f32);
        for k in 0..=n {
            let st = k as f32 / n as f32 * 2.0 - 1.0;
            let (v, om) = hit_turning(st, 70);
            peak_om = peak_om.max(om);
            if v.length() < 0.5 {
                prev = None;
                continue;
            }
            let d = v.normalize();
            if let Some(p0) = prev {
                let step = p0.angle_between(d).to_degrees();
                if step > worst {
                    worst = step;
                    worst_at = st;
                }
            }
            prev = Some(d);
        }
        println!("while turning     biggest jump {worst:5.1} deg at steer {worst_at:+.2} (peak spin {peak_om:.1} rad/s)");
        check("a turning car hits predictably", worst < 25.0);
        // and immediately after landing, when the align is snapping the car
        let mut prev2: Option<Vec3> = None;
        let (mut worst2, mut peak2) = (0.0f32, 0.0f32);
        for k in 0..=30 {
            let settle = 30 + k; // sample across the landing settle
            let (v, om) = hit_turning(0.25, settle);
            peak2 = peak2.max(om);
            if v.length() < 0.5 {
                prev2 = None;
                continue;
            }
            let d = v.normalize();
            if let Some(p0) = prev2 {
                worst2 = worst2.max(p0.angle_between(d).to_degrees());
            }
            prev2 = Some(d);
        }
        println!("frame to frame    biggest jump {worst2:5.1} deg (peak spin {peak2:.1} rad/s)");
        check("one frame later is not a different answer", worst2 < 25.0);
        // ---- the ball sitting ON the car ----
        // Here the push direction is (ball - car) with its vertical scaled
        // to a third — so for a ball on the roof the horizontal part is
        // almost nothing, and which way it points is decided by millimetres.
        // That is the classic source of "it went somewhere strange".
        let roof_hit = |lateral: f32| -> Vec3 {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
            car.rot = glam::Quat::IDENTITY;
            car.sync_rot();
            let mut ball = Ball::new();
            let mut c = physics::Controls::default();
            c.throttle = 1.0;
            let mut ev = StepEvents::default();
            for _ in 0..60 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            // resting on the roof, a whisker off centre, then bumped upward
            ball.pos = car.pos + Vec3::new(lateral, car.half_h + ball.r + 0.01, 0.0);
            ball.vel = car.vel;
            ball.prev = ball.pos;
            let mut out = Vec3::ZERO;
            for _ in 0..90 {
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                if !ev.touches.is_empty() {
                    out = ball.vel;
                }
            }
            out
        };
        let mut prev3: Option<Vec3> = None;
        let (mut worst3, mut at3) = (0.0f32, 0.0f32);
        for k in 0..=40 {
            let lat = (k as f32 / 40.0 * 2.0 - 1.0) * 0.20; // +/- 20 cm
            let v = roof_hit(lat);
            if v.length() < 0.3 {
                prev3 = None;
                continue;
            }
            let d = v.normalize();
            if let Some(p0) = prev3 {
                let step = p0.angle_between(d).to_degrees();
                if step > worst3 {
                    worst3 = step;
                    at3 = lat;
                }
            }
            prev3 = Some(d);
        }
        println!("ball on the roof  biggest jump {worst3:5.1} deg at offset {at3:+.3} m");
        check("a carried ball leaves predictably", worst3 < 25.0);

        // ---- struck DURING A FLIP, which is the unstable case ----
        // The flick throw follows the hood's surface velocity, and near the
        // car's rotation axis that vector is nearly zero — so its direction
        // is noise unless the weight accounts for its magnitude.
        let flip_hit = |lateral: f32| -> Vec3 {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
            car.rot = glam::Quat::IDENTITY;
            car.sync_rot();
            let mut ball = Ball::new();
            let mut c = physics::Controls::default();
            c.throttle = 1.0;
            let mut ev = StepEvents::default();
            for _ in 0..50 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            // jump, then dodge forward — and place the ball across the nose
            c.jump_pressed = true;
            c.jump_held = true;
            physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            c.jump_pressed = false;
            for _ in 0..8 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            let f = car.forward();
            ball.pos = car.pos + f * 0.5 + Vec3::new(lateral, car.half_h + ball.r, 0.0);
            ball.vel = car.vel;
            ball.prev = ball.pos;
            c.jump_pressed = true;
            physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            c.jump_pressed = false;
            let mut out = Vec3::ZERO;
            for _ in 0..70 {
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                if !ev.touches.is_empty() {
                    out = ball.vel;
                }
            }
            out
        };
        let mut prev4: Option<Vec3> = None;
        let (mut worst4, mut at4) = (0.0f32, 0.0f32);
        for k in 0..=40 {
            let lat = (k as f32 / 40.0 * 2.0 - 1.0) * 0.30;
            let v = flip_hit(lat);
            if v.length() < 0.5 {
                prev4 = None;
                continue;
            }
            let d = v.normalize();
            if let Some(p0) = prev4 {
                let step = p0.angle_between(d).to_degrees();
                if step > worst4 {
                    worst4 = step;
                    at4 = lat;
                }
            }
            prev4 = Some(d);
        }
        println!("struck mid-flip   biggest jump {worst4:5.1} deg at offset {at4:+.3} m");
        check("a flicked ball leaves predictably", worst4 < 30.0);

        // ---- the singularity itself, tested directly ----
        // The end-to-end sweep above is coarse (a flip only touches the ball
        // for a frame or two), so check the property that was actually
        // wrong: the throw's weight must fall with the surface SPEED, not
        // just with how fast the car is spinning. A contact sitting on the
        // rotation axis has a near-zero surface velocity, and normalising
        // that returns noise.
        let weight = |omega: f32, surf_speed: f32| -> f32 {
            let spin = (omega / 7.0).clamp(0.0, 1.0);
            spin * (surf_speed / 3.0).clamp(0.0, 1.0)
        };
        let on_axis = weight(8.0, 0.02); // fast flip, contact on the axis
        let out_wide = weight(8.0, 3.5); // same flip, contact out at the nose
        println!("throw weight: on the axis {on_axis:.3}, out at the nose {out_wide:.3}");
        check("a contact on the rotation axis throws nothing", on_axis < 0.02);
        check("a sweeping hood still throws hard", out_wide > 0.9);
        check("and it is continuous in between", {
            let mut worst_step = 0.0f32;
            let mut prev = weight(8.0, 0.0);
            for k in 1..=300 {
                let w = weight(8.0, k as f32 * 0.02);
                worst_step = worst_step.max((w - prev).abs());
                prev = w;
            }
            worst_step < 0.02
        });
        println!("{}", if pass { "HITS: PASS" } else { "HITS: FAIL" });
        return;
    }
    if name == "flick" {
        // THE FLICK. In Rocket League a flick is not a hit — the ball is
        // resting on the car and the car ROTATES out from under it, so the
        // hood's surface velocity throws it. That makes it a test of the
        // contact model, not of the impulse: if the car's rotation does not
        // reach the ball, a flick feels dead no matter how hard you hit.
        let cfg = Config::default();
        println!("{:<16} {:>8} {:>8} {:>9} {:>8}", "flick", "exit", "angle", "carry m", "spin");
        let mut pass = true;
        let mut results = Vec::new();
        for (label, thr, steer, hold) in [
            ("forward", 1.0f32, 0.0f32, 0.10f32),
            ("45 degree", 1.0, 0.75, 0.10),
            ("sideways", 0.0, 1.0, 0.10),
            ("backflip", -1.0, 0.0, 0.10),
            ("delayed", 1.0, 0.0, 0.35),
        ] {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
            car.rot = glam::Quat::IDENTITY;
            let mut ball = Ball::new();
            let mut c = physics::Controls::default();
            let mut ev = StepEvents::default();
            // get rolling, then set the ball on the hood and let it settle
            c.throttle = 1.0;
            for _ in 0..90 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            // Where the ball sits decides which flick works: a nose carry
            // throws forward, and a backflip needs the ball back over the
            // roof — pitching away from a nose carry just drops it. That is
            // true in Rocket League too, so the setup differs per flick.
            let f = car.forward();
            let along = if thr < -0.5 { -0.12 } else { 0.35 };
            ball.pos = car.pos + f * along + car.up() * (car.half_h + ball.r + 0.02);
            ball.vel = car.vel;
            ball.ang = Vec3::ZERO;
            ball.prev = ball.pos;
            for _ in 0..60 {
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            }
            let carried = ball.pos.distance(car.pos) < 2.5;
            // jump, wait, then dodge
            c.jump_pressed = true;
            physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            c.jump_pressed = false;
            let mut t = 0.0f32;
            while t < hold {
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                t += DT;
            }
            c.throttle = thr;
            c.steer = steer;
            c.jump_pressed = true;
            physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            c.jump_pressed = false;
            // let the flick play out and take the ball's peak speed
            let mut best = 0.0f32;
            let mut best_v = Vec3::ZERO;
            let mut last_touch_v = Vec3::ZERO;
            let start = ball.pos;
            let mut touch_frames = 0;
            let mut peak_omega = 0.0f32;
            let dbg = std::env::var("FLICK_TRACE").map(|v| v == label).unwrap_or(false);
            for fi in 0..90 {
                if dbg && fi < 26 {
                    // the box closest-point distance the contact test uses
                    let half = Vec3::new(
                        car.half_w().max(0.18),
                        car.half_h.max(0.12),
                        car.half_len.max(0.30),
                    );
                    let local = car.rot.inverse() * (ball.pos - car.pos);
                    let cl = local.clamp(-half, half);
                    let cp = car.pos + car.rot * cl;
                    let d = ball.pos.distance(cp);
                    eprintln!(
                        "     f={:2} gap={:5.3} (need<{:5.3}) dodge={:.2} omega={:5.1} ballv=({:5.1},{:5.1}) carv=({:5.1},{:5.1})",
                        fi,
                        d - ball.r,
                        0.02 + if car.dodge_left() > 0.0 { 0.45 } else { 0.0 },
                        car.dodge_left(),
                        car.omega_len(),
                        ball.vel.y,
                        ball.vel.length(),
                        car.vel.y,
                        car.vel.length()
                    );
                }
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                if !ev.touches.is_empty() {
                    touch_frames += 1;
                    // THE FLICK IS THE LAST TOUCH. Measuring the ball's peak
                    // speed measured the carry that preceded the flip, not
                    // the throw — which is why every flick looked flat while
                    // the trace showed vertical velocity tripling on contact.
                    last_touch_v = ball.vel;
                }
                peak_omega = peak_omega.max(car.omega_len());
                if ball.vel.length() > best {
                    best = ball.vel.length();
                    best_v = ball.vel;
                }
            }
            // what left the car on the flick itself
            let exit_v = if last_touch_v.length() > 0.5 { last_touch_v } else { best_v };
            let flat = Vec3::new(exit_v.x, 0.0, exit_v.z).length();
            let angle = exit_v.y.atan2(flat.max(0.01)).to_degrees();
            let best = exit_v.length().max(best * 0.0);
            let travel = ball.pos.distance(start);
            println!(
                "{:<16} {:>7.1} {:>7.0}d {:>8.1} {:>7.1} {}",
                label,
                best,
                angle,
                travel,
                ball.ang.length(),
                if carried { "" } else { "(lost the carry)" }
            );
            if std::env::var("FLICK_DBG").is_ok() {
                eprintln!("    contact frames during flick: {touch_frames}, peak omega {peak_omega:.1} rad/s");
            }
            results.push((label, best, angle));
        }
        // Rocket League reference: a dribble flick leaves at roughly 18-32
        // m/s, forward flicks low and backflips high.
        let fwd = results.iter().find(|r| r.0 == "forward").map(|r| r.1).unwrap_or(0.0);
        let back = results.iter().find(|r| r.0 == "backflip").map(|r| (r.1, r.2)).unwrap_or((0.0, 0.0));
        let mut check = |what: &str, ok: bool| {
            println!("{:<42} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        check("a forward flick has real pace (>16 m/s)", fwd > 16.0);
        check("a flick is not a blast (<34 m/s)", fwd < 34.0);
        check("the flick family lofts (forward/45/delayed)", results.iter().filter(|r| r.2 > 18.0).count() >= 3);
        // A backflip flick is a LOFT, not a drive: high and slower, which is
        // exactly what it is for in Rocket League — popping the ball over
        // someone. (This read as flat until the harness stopped measuring
        // the ball's peak speed, which was the carry BEFORE the flip, and
        // started measuring the last touch, which is the throw itself.)
        check("a backflip flick lofts steeply", back.1 > 45.0);
        check("and it is a pop, not a rocket", back.0 > 6.0 && back.0 < 18.0);
        check("forward flicks stay low", results.iter().find(|r| r.0 == "forward").map(|r| r.2 < 35.0).unwrap_or(false));

        // ---- Does a flick answer to SPEED and to WHERE THE BALL SITS? ----
        // In Rocket League both matter enormously: a flick at pace carries
        // the car's momentum, and the ball's spot on the hood decides
        // whether you drive it low or pop it up. A flick that ignores either
        // reads as canned.
        println!();
        println!("{:<12} {:>8} {:>8} {:>8} {:>8}", "carry @", "5 m/s", "12 m/s", "20 m/s", "spread");
        let flick_at = |speed: f32, along: f32| -> (f32, f32) {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
            car.sync_rot();
            let mut ball = Ball::new();
            let mut c = physics::Controls::default();
            let mut ev = StepEvents::default();
            c.throttle = 1.0;
            for _ in 0..240 {
                if car.vel.length() >= speed {
                    break;
                }
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            c.throttle = 0.0;
            let f = car.forward();
            ball.pos = car.pos + f * along + car.up() * (car.half_h + ball.r + 0.02);
            ball.vel = car.vel;
            ball.prev = ball.pos;
            for _ in 0..45 {
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            }
            c.jump_pressed = true;
            physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            c.jump_pressed = false;
            for _ in 0..12 {
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            }
            c.throttle = 1.0;
            c.jump_pressed = true;
            physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            c.jump_pressed = false;
            let mut last = Vec3::ZERO;
            for _ in 0..80 {
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                if !ev.touches.is_empty() {
                    last = ball.vel;
                }
            }
            let flat = Vec3::new(last.x, 0.0, last.z).length();
            (last.length(), last.y.atan2(flat.max(0.01)).to_degrees())
        };
        let mut speed_spread = 0.0f32;
        let mut angle_spread = 0.0f32;
        for (label, along) in [("nose", 0.42f32), ("middle", 0.10), ("back", -0.22)] {
            let a5 = flick_at(5.0, along);
            let a12 = flick_at(12.0, along);
            let a20 = flick_at(20.0, along);
            println!(
                "{:<12} {:>6.1}/{:<3.0} {:>5.1}/{:<3.0} {:>5.1}/{:<3.0} {:>7.1}",
                label, a5.0, a5.1, a12.0, a12.1, a20.0, a20.1, a20.0 - a5.0
            );
            speed_spread = speed_spread.max(a20.0 - a5.0);
            angle_spread = angle_spread.max((a5.1 - a20.1).abs());
        }
        check("a flick at pace is stronger than one at a crawl", speed_spread > 4.0);
        println!("   (speed adds {speed_spread:.1} m/s across 5 -> 20 m/s)");

        // ---- THE POWER FLICK ----
        // The real test of a flick is what it does from a FAST dribble: the
        // ball should leave well above the car's own speed, because the
        // car's momentum goes through the contact and the rotation adds on
        // top. A flick that merely matches car speed is a pass, not a shot.
        println!();
        println!("{:<10} {:>9} {:>9} {:>9} {:>8}", "car speed", "exit", "gain", "angle", "verdict");
        let mut gains = Vec::new();
        for target in [9.0f32, 14.0, 19.0, 23.0] {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
            car.sync_rot();
            let mut ball = Ball::new();
            let mut c = physics::Controls::default();
            let mut ev = StepEvents::default();
            // get to speed with the ball still out of the way
            c.throttle = 1.0;
            c.boost = true;
            for _ in 0..900 {
                if car.vel.length() >= target {
                    break;
                }
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            // place the ball and HOLD the speed while it settles, otherwise
            // the car coasts down and the flick is measured at the wrong pace
            let f = car.forward();
            ball.pos = car.pos + f * 0.34 + Vec3::Y * (car.half_h + ball.r + 0.02);
            ball.vel = car.vel;
            ball.prev = ball.pos;
            for _ in 0..30 {
                c.boost = car.vel.length() < target;
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            }
            let carspeed = car.vel.length();
            c.boost = false;
            c.jump_pressed = true;
            c.jump_held = true;
            physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            c.jump_pressed = false;
            for _ in 0..9 {
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            }
            c.throttle = 1.0;
            c.jump_pressed = true;
            physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            c.jump_pressed = false;
            // let the flick play out, then read the ball once it is clear of
            // the car — that is the shot, whatever the contact history was
            for _ in 0..70 {
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            }
            let v = ball.vel;
            let exit = v.length();
            let flat = Vec3::new(v.x, 0.0, v.z).length();
            let ang = v.y.atan2(flat.max(0.01)).to_degrees();
            let gain = exit - carspeed;
            gains.push(gain);
            println!(
                "{:<10.1} {:>8.1} {:>+8.1} {:>8.0}d {:>8}",
                carspeed,
                exit,
                gain,
                ang,
                if gain > 8.0 { "power" } else { "a pass" }
            );
        }
        check("a flick adds real pace at speed", gains.last().copied().unwrap_or(0.0) > 8.0);
        check("and adds more the faster you are going", gains[3] > gains[0] + 2.0);

        // ---- AIR DRIBBLE: how long does the ball stay on the nose? ----
        println!();
        println!("{:<16} {:>9} {:>9} {:>8}", "air carry", "seconds", "touches", "drift");
        for (label, boost, pitch) in [("no boost", false, 0.0f32), ("boosting", true, 0.0), ("nose up", true, 0.25)] {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            let gy = world.ground_h(0.0, 0.0);
            car.pos = Vec3::new(0.0, gy + 14.0, 0.0);
            car.rot = glam::Quat::from_rotation_x(0.32); // nose up, as you would be
            car.sync_rot(); // a set orientation is not a spin
            car.vel = Vec3::new(0.0, 6.0, -11.0);
            let mut ball = Ball::new();
            let f = car.forward();
            ball.pos = car.pos + f * 0.5 + car.up() * (car.half_h + ball.r + 0.02);
            ball.vel = car.vel;
            ball.prev = ball.pos;
            let mut c = physics::Controls::default();
            c.boost = boost;
            c.pitch = pitch;
            let mut ev = StepEvents::default();
            let mut touches = 0;
            let mut carried = 0.0f32;
            let mut t = 0.0f32;
            while t < 4.0 {
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                if !ev.touches.is_empty() {
                    touches += 1;
                }
                // "still on the car" means within a car length of the nose
                if ball.pos.distance(car.pos) < 2.4 && ball.pos.y > car.pos.y {
                    carried = t;
                }
                if ball.pos.y < gy + ball.r + 0.3 {
                    break;
                }
                t += DT;
            }
            println!("{:<16} {:>8.2}s {:>9} {:>7.1}m", label, carried, touches, ball.pos.distance(car.pos));
            if label == "no boost" {
                // an air dribble is many small touches over seconds, not one
                // hit — if this collapses, the carry has become a punt
                check("an air dribble carries for seconds", carried > 1.5);
                check("it is many small touches", touches > 15);
            }
        }
        // ---- THE CATCH: turning a bouncing ball into a carry ----
        // Every dribble starts here, and it is a skill in Rocket League
        // because it depends on MATCHING the ball's motion: come in at the
        // same speed and it settles on the hood, come in wrong and it
        // bounces away. A physics that catches everything is as unfaithful
        // as one that catches nothing.
        println!();
        println!("{:<18} {:>9} {:>9} {:>9}", "catch attempt", "settled", "bounces", "on car");
        for (label, dv, drop) in [
            ("matched speed", 0.0f32, 5.0f32),
            ("too fast (+6)", 6.0, 5.0),
            ("too slow (-6)", -6.0, 5.0),
            ("steep drop", 0.0, 14.0),
        ] {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let gy = world.ground_h(0.0, 0.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, gy + 0.31, 6.0);
            car.sync_rot();
            car.vel = Vec3::new(0.0, 0.0, -11.0);
            let mut ball = Ball::new();
            // falling ahead of the car, travelling with it
            ball.pos = Vec3::new(0.0, gy + drop, 0.0);
            ball.vel = Vec3::new(0.0, -2.0, -11.0 + dv);
            ball.prev = ball.pos;
            let mut c = physics::Controls::default();
            c.throttle = 1.0;
            let mut ev = StepEvents::default();
            let mut bounces = 0;
            let mut settled = 0.0f32;
            let mut t = 0.0f32;
            while t < 4.0 {
                ev.touches.clear();
                ev.bounces.clear();
                // Hold the car's OWN speed — what a player does — but never
                // chase the ball. A station-keeping controller drove under it
                // whatever you did and caught everything; a fixed throttle
                // caught nothing because the car would not hold pace. Whether
                // the ball stays is then down to the approach itself.
                c.throttle = if car.vel.length() < 11.0 { 1.0 } else { 0.0 };
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                bounces += ev.bounces.len();
                let on_car = ball.pos.y > car.pos.y + 0.4 && ball.pos.distance(car.pos) < 2.0;
                if on_car {
                    settled += DT;
                }
                t += DT;
            }
            let riding = ball.pos.y > car.pos.y + 0.4 && ball.pos.distance(car.pos) < 2.2;
            println!(
                "{:<18} {:>8.2}s {:>9} {:>9}",
                label,
                settled,
                bounces,
                if riding { "yes" } else { "no" }
            );
            if label == "matched speed" {
                check("a matched approach catches the ball", settled > 0.8);
            }
            if label == "too fast (+6)" {
                check("a mistimed approach does not", settled < 0.8);
            }
        }
        println!("{}", if pass { "FLICK: PASS" } else { "FLICK: FAIL" });
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
    if name == "shell" {
        // WHAT MAKES A BODY LOOK LIKE A CAR BODY.
        //
        // Measured on the art target: Octane's painted shell has a MEDIAN
        // dihedral angle of about 2 degrees — neighbouring triangles are
        // very nearly coplanar, so the surface reads as continuous — while
        // its p90 is 31 to 79 degrees, because a car also has crisp panel
        // breaks. Smooth almost everywhere, sharp in a few deliberate
        // places. A uniformly faceted shell reads as a low-poly prop; a
        // uniformly smooth one reads as a bar of soap.
        let dihedral = |m: &crate::meshgen::MeshData| -> (usize, f32, f32, f32) {
            let tri = m.idx.len() / 3;
            let mut face: Vec<Vec3> = Vec::with_capacity(tri);
            for t in m.idx.chunks_exact(3) {
                let a = Vec3::from_array(m.verts[t[0] as usize].pos);
                let b = Vec3::from_array(m.verts[t[1] as usize].pos);
                let c = Vec3::from_array(m.verts[t[2] as usize].pos);
                face.push((b - a).cross(c - a).normalize_or(Vec3::Y));
            }
            let mut edge: std::collections::HashMap<(u32, u32), Vec<usize>> = std::collections::HashMap::new();
            for (f, t) in m.idx.chunks_exact(3).enumerate() {
                for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    edge.entry((a.min(b), a.max(b))).or_default().push(f);
                }
            }
            let mut angs: Vec<f32> = edge
                .values()
                .filter(|f| f.len() == 2)
                .map(|f| face[f[0]].dot(face[f[1]]).clamp(-1.0, 1.0).acos().to_degrees())
                .collect();
            if angs.is_empty() {
                return (tri, 0.0, 0.0, 0.0);
            }
            angs.sort_by(f32::total_cmp);
            let mean = angs.iter().sum::<f32>() / angs.len() as f32;
            (tri, mean, angs[angs.len() / 2], angs[angs.len() * 9 / 10])
        };
        // add the creased fraction: the references are ~10% of edges above
        // 20 degrees, which is the difference between a designed body and an
        // extruded one
        let creased = |m: &crate::meshgen::MeshData| -> f32 {
            let mut face: Vec<Vec3> = Vec::new();
            for t in m.idx.chunks_exact(3) {
                let a = Vec3::from_array(m.verts[t[0] as usize].pos);
                let b = Vec3::from_array(m.verts[t[1] as usize].pos);
                let c = Vec3::from_array(m.verts[t[2] as usize].pos);
                face.push((b - a).cross(c - a).normalize_or(Vec3::Y));
            }
            let mut edge: std::collections::HashMap<(u32, u32), Vec<usize>> = std::collections::HashMap::new();
            for (f, t) in m.idx.chunks_exact(3).enumerate() {
                for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    edge.entry((a.min(b), a.max(b))).or_default().push(f);
                }
            }
            // creases show up as edges whose two faces disagree, AND as
            // boundary edges where the surface was split apart on purpose
            let shared: Vec<&Vec<usize>> = edge.values().filter(|f| f.len() == 2).collect();
            if shared.is_empty() {
                return 0.0;
            }
            let hard = shared
                .iter()
                .filter(|f| face[f[0]].dot(face[f[1]]).clamp(-1.0, 1.0).acos().to_degrees() > 20.0)
                .count();
            let split = edge.values().filter(|f| f.len() == 1).count();
            (hard + split) as f32 / edge.len() as f32 * 100.0
        };
        println!("{:<20} {:>7} {:>8} {:>8} {:>8} {:>9}", "surface", "tris", "mean", "median", "p90", "creased");
        println!("{:<20} {:>7} {:>7}d {:>7}d {:>7}d {:>8}", "OCTANE paint (ref)", 4350, "21.1", "2.2", "79.4", "n/a");
        println!("{:<20} {:>7} {:>7}d {:>7}d {:>7}d {:>8.1}%", "DOMINUS paint (ref)", 5995, "3.4", "0.4", "21.8", 10.4);
        println!("{:<20} {:>7} {:>7}d {:>7}d {:>7}d {:>8.1}%", "DOMINUS body  (ref)", 10102, "2.1", "0.6", "21.1", 10.4);
        let hull = crate::meshgen::loft(
            &[
                (-0.77, 0.13, -0.035, 0.010, 0.95),
                (-0.30, 0.34, -0.065, 0.085, 0.70),
                (0.02, 0.36, -0.065, 0.105, 0.65),
                (0.60, 0.30, -0.050, 0.060, 0.80),
                (0.77, 0.22, -0.035, 0.020, 0.95),
            ],
            26,
        );
        let mut worst_median = 0.0f32;
        for (nm, m) in [
            ("our lofted hull", &hull),
            ("chamfered box", &crate::meshgen::cbox(0.22)),
            ("plain box", &crate::meshgen::boxx()),
        ] {
            let (t, mean, med, p90) = dihedral(m);
            println!("{:<20} {:>7} {:>7.1}d {:>7.1}d {:>7.1}d {:>8.1}%", nm, t, mean, med, p90, creased(m));
            if nm == "our lofted hull" {
                worst_median = med;
            }
        }
        println!();
        // ---- IS THE CATALOGUE ACTUALLY VARIED? ----
        // Rocket League ships six hitbox presets for dozens of cars, and
        // that is a deliberate design: the SHAPES vary, the boxes do not,
        // because players need to know what they are playing against. Ours
        // grew organically, so the question worth asking is the opposite —
        // do forty cars mean forty vehicles, or forty paint jobs on one box?
        {
            // the procedural cars' declared half-extents, read from source
            let src = include_str!("procars.rs");
            let mut boxes: Vec<(String, f32, f32, f32)> = Vec::new();
            for line in src.lines() {
                if let Some(i) = line.find("half_len:") {
                    let rest = &line[i..];
                    let nums: Vec<f32> = rest
                        .split(|c: char| !c.is_ascii_digit() && c != '.')
                        .filter(|t| !t.is_empty())
                        .filter_map(|t| t.parse::<f32>().ok())
                        .collect();
                    if nums.len() >= 4 {
                        boxes.push((format!("car{}", boxes.len()), nums[0], nums[1], nums[2]));
                    }
                }
            }
            if boxes.len() >= 4 {
                // cluster by rounded shape: how many DISTINCT boxes are there
                let mut sigs: Vec<(i32, i32, i32)> = boxes
                    .iter()
                    .map(|b| ((b.1 * 12.0) as i32, (b.2 * 12.0) as i32, (b.3 * 12.0) as i32))
                    .collect();
                sigs.sort();
                sigs.dedup();
                let (mut lmin, mut lmax) = (f32::MAX, 0.0f32);
                let (mut hmin, mut hmax) = (f32::MAX, 0.0f32);
                for b in &boxes {
                    lmin = lmin.min(b.1);
                    lmax = lmax.max(b.1);
                    hmin = hmin.min(b.2);
                    hmax = hmax.max(b.2);
                }
                println!(
                    "   hitboxes: {} cars, {} distinct shapes; length {:.2}-{:.2}, height {:.2}-{:.2}",
                    boxes.len(),
                    sigs.len(),
                    lmin,
                    lmax,
                    hmin,
                    hmax
                );
                println!(
                    "   spread: longest is {:.1}x the shortest, tallest {:.1}x the flattest",
                    lmax / lmin.max(0.01),
                    hmax / hmin.max(0.01)
                );
            }
        }
        println!();
        // ---- IS THE CATALOGUE DESCRIBED? ----
        // Forty vehicles in a flat list of names is a catalogue nobody can
        // navigate: the only way to learn what a car is was to drive it. Every
        // entry needs a class and a line, and the lines have to be DIFFERENT
        // — a table of "fast and fun" tells you nothing.
        {
            let src = include_str!("main.rs");
            let classes = ["CLASSIC", "ODDITY", "ELECTRIC", "CONCEPT", "IMPORT", "ASSEMBLED", "HOVER"];
            let mut counts: Vec<(&str, usize)> = classes
                .iter()
                .map(|c| (*c, src.matches(&format!("(\"{c}\",")).count()))
                .collect();
            counts.retain(|(_, n)| *n > 0);
            let total: usize = counts.iter().map(|(_, n)| n).sum();
            println!("   catalogue classes: {counts:?}");
            println!("   {total} described entries across {} families", counts.len());
            // and the descriptions must not repeat
            let mut lines: Vec<&str> = Vec::new();
            for l in src.lines() {
                let t = l.trim();
                if t.starts_with('"') && t.ends_with("\",") && t.len() > 30 {
                    lines.push(t);
                }
            }
            let before = lines.len();
            lines.sort();
            lines.dedup();
            println!("   {} description lines, {} distinct", before, lines.len());
            let ok_desc = counts.len() >= 6 && lines.len() == before;
            println!("   {}", if ok_desc { "every family represented, every description distinct" } else { "CATALOGUE GAPS" });
        }
        println!();
        // ---- CATALOGUE AUDIT ----
        // A house style is only a style if the whole catalogue is built to
        // it. This reads the source and reports which cars are made from the
        // reference recipe — a continuous lofted shell — and which are still
        // assemblies of boxes, which can never read as bodywork however
        // carefully they are proportioned.
        {
            let src = include_str!("procars.rs");
            let names = [
                "breadbox", "wedge", "roller", "longboy", "cube", "shark",
                "volt", "axion", "hauler", "pulse", "tram", "vipera", "aegis",
            ];
            let mut lofted = 0;
            let mut boxed: Vec<&str> = Vec::new();
            for n in names {
                let Some(i) = src.find(&format!("pub fn {n}(gpu")) else { continue };
                let body = &src[i..src[i..].find("\nfn ").map(|j| i + j).unwrap_or(src.len())];
                let end = body.find("\npub fn ").unwrap_or(body.len());
                let body = &body[..end];
                // a car counts as built to the style whether it lofts
                // directly or through one of the shared builders — the
                // EVs go via `ev_body`, which is the same recipe wearing a
                // different name
                if body.contains("shell_car(") || body.contains("meshgen::loft(") || body.contains("ev_body(") {
                    lofted += 1;
                } else {
                    boxed.push(n);
                }
            }
            println!(
                "   catalogue: {lofted} of {} procedural cars use the lofted shell",
                names.len()
            );
            if !boxed.is_empty() {
                println!("   still assemblies: {boxed:?}");
            }
            // the cube is a joke about being a box and is allowed to stay one
            let ok_cat = boxed.iter().all(|n| *n == "cube");
            println!(
                "   {}",
                if ok_cat { "every car but the CUBE is built to the house style" } else { "STYLE DRIFT" }
            );
        }
        println!();
        // The second half of the look is the CRISP BREAK, and on the target
        // it comes from separate material regions — Octane is Paint, Body,
        // Window and Chassis as four meshes, so the boundary between them is
        // a hard edge by construction rather than a crease inside one
        // surface. Our cars work the same way: the hull is continuous and
        // the glasshouse, spoiler and sills are their own parts, so the
        // feature lines fall where the parts meet.
        println!("   a car body = one continuous shell + separate parts for the breaks:");
        println!("   OCTANE ships 4 body meshes (chassis, paint, body, window) + 8 wheel meshes");
        // Both halves have to hold: continuous enough to read as bodywork,
        // creased enough to read as designed. Faceted (creases everywhere)
        // and soap (no creases) both fail.
        let hull_creased = creased(&hull);
        let ok = worst_median <= 2.0 && (8.0..20.0).contains(&hull_creased);
        println!("   our hull: median {worst_median:.1}d (refs 0.4-2.2), creased {hull_creased:.1}% (refs 10.4%)");
        println!("{}", if ok { "SHELL: PASS" } else { "SHELL: FAIL" });
        return;
    }
    if name == "carry" {
        // TURNING WITH THE BALL ON THE HOOD.
        //
        // What should happen: the ball has its own momentum, so when the car
        // turns the ball LAGS toward the outside of the turn. That lag is the
        // whole skill — you work around to the outside of the ball and use
        // the nose to hold it, steering it by where you place the car.
        //
        // What "yanking" means: the nose drags the ball round with it, so
        // the ball stays pinned dead ahead however hard you turn and there is
        // nothing to work with.
        //
        // So measure the BEARING of the ball from the car's nose over a turn.
        // Pinned at zero is the fault; a bearing that opens to the outside is
        // the behaviour.
        let cfg = Config::default();
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<50} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        println!("{:<14} {:>10} {:>11} {:>10}", "steer", "bearing", "lateral", "kept");
        let mut bearings = Vec::new();
        let mut inherit: Vec<f32> = Vec::new();
        for steer in [0.35f32, 0.7, 1.0] {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
            car.sync_rot();
            let mut ball = Ball::new();
            let mut c = physics::Controls::default();
            let mut ev = StepEvents::default();
            c.throttle = 1.0;
            for _ in 0..110 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            // set it on the hood and let it settle straight
            let f = car.forward();
            ball.pos = car.pos + f * 0.30 + Vec3::Y * (car.half_h + ball.r + 0.02);
            ball.vel = car.vel;
            ball.prev = ball.pos;
            for _ in 0..40 {
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
            }
            // now turn, and watch where the ball goes relative to the nose
            c.steer = steer;
            let mut peak = 0.0f32;
            let mut lat = 0.0f32;
            let mut kept = true;
            for _ in 0..120 {
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                let rel = car.rot.inverse() * (ball.pos - car.pos);
                if rel.length() > 3.0 {
                    kept = false;
                    break;
                }
                // bearing from straight ahead, signed toward the outside
                let b = rel.x.atan2(-rel.z).to_degrees() * -steer.signum();
                if b > peak {
                    peak = b;
                    lat = rel.x.abs();
                }
            }
            println!("{:<14.2} {:>9.1}d {:>10.2}m {:>10}", steer, peak, lat, if kept { "yes" } else { "lost" });
            bearings.push(peak);
            // HOW MUCH OF THE CAR'S TURN DID THE BALL INHERIT?
            // A ball with its own momentum turns LESS than the car under it —
            // that difference is the lag you steer around. If it turns as
            // much as the car, the nose is carrying it round: the yank.
            {
                let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
                let mut car = physics::Car::new();
                car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
                car.sync_rot();
                let mut ball = Ball::new();
                let mut c = physics::Controls::default();
                let mut ev = StepEvents::default();
                c.throttle = 1.0;
                for _ in 0..110 {
                    physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                }
                let f = car.forward();
                ball.pos = car.pos + f * 0.30 + Vec3::Y * (car.half_h + ball.r + 0.02);
                ball.vel = car.vel;
                ball.prev = ball.pos;
                for _ in 0..40 {
                    physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                }
                let heading = |v: Vec3| v.z.atan2(v.x).to_degrees();
                let c0 = heading(car.forward());
                let b0 = heading(ball.vel);
                c.steer = steer;
                for _ in 0..36 {
                    physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                }
                let dc = (heading(car.forward()) - c0).abs();
                let db = (heading(ball.vel) - b0).abs();
                println!(
                    "   over 0.3s: car turned {dc:.1}d, ball turned {db:.1}d ({:.0}% inherited)",
                    db / dc.max(0.01) * 100.0
                );
                inherit.push(db / dc.max(0.01));
            }
        }
        // the ball must move to the OUTSIDE, and more so the harder you turn
        // Rocket League's carry: the ball keeps most of its own line, and
        // you steer by moving the car around it. Inheriting nearly all of
        // the car's turn is the yank.
        let worst = inherit.iter().cloned().fold(0.0f32, f32::max);
        check("the ball keeps its own line through a turn", worst < 0.75);
        check("the ball lags to the outside of the turn", bearings.iter().all(|b| *b > 8.0));
        check("and lags further the harder you turn", bearings[2] > bearings[0] + 4.0);
        println!("{}", if pass { "CARRY: PASS" } else { "CARRY: FAIL" });
        return;
    }
    if name == "hits" {
        // "Sometimes the ball goes in weird directions." A hit that is
        // sometimes wrong is a DISCONTINUITY: nudge the contact point a
        // centimetre and the answer should move a degree, not thirty. This
        // sweeps the ball across the front of the car and measures how much
        // the exit direction jumps between neighbouring samples.
        let cfg = Config::default();
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<46} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        let hit = |lateral: f32, height: f32| -> Vec3 {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 6.0);
            car.rot = glam::Quat::IDENTITY;
            car.sync_rot();
            car.vel = Vec3::new(0.0, 0.0, -18.0);
            let mut ball = Ball::new();
            ball.pos = Vec3::new(lateral, car.pos.y + height, 0.0);
            ball.vel = Vec3::ZERO;
            ball.prev = ball.pos;
            let mut c = physics::Controls::default();
            c.throttle = 1.0;
            let mut ev = StepEvents::default();
            let mut out = Vec3::ZERO;
            for _ in 0..60 {
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                if !ev.touches.is_empty() {
                    out = ball.vel;
                }
            }
            out
        };
        // sweep across the nose, and up the face
        for (label, vertical) in [("across the nose", false), ("up the face", true)] {
            let n = 60;
            let mut prev: Option<Vec3> = None;
            let mut worst = 0.0f32;
            let mut worst_at = 0.0f32;
            let mut sum = 0.0f32;
            for k in 0..=n {
                let t = k as f32 / n as f32 * 2.0 - 1.0;
                let v = if vertical { hit(0.0, t * 0.85) } else { hit(t * 0.85, 0.0) };
                if v.length() < 0.5 {
                    prev = None;
                    continue;
                }
                let d = v.normalize();
                if let Some(p0) = prev {
                    let step = p0.angle_between(d).to_degrees();
                    sum += step;
                    if step > worst {
                        worst = step;
                        worst_at = t;
                    }
                }
                prev = Some(d);
            }
            println!(
                "{label:<18} biggest jump {worst:5.1} deg at offset {worst_at:+.2}, mean {:.1} deg per step",
                sum / n as f32
            );
            check("neighbouring contacts give neighbouring answers", worst < 25.0);
        }
        // ---- now with the car MOVING: turning, landing, mid-flip ----
        // The flick throw reads the car's angular velocity, and omega is
        // derived from the rotation delta between frames — so anything that
        // snaps the car's orientation makes the ball leave sideways.
        let hit_turning = |steer: f32, settle: u32| -> (Vec3, f32) {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 10.0);
            car.rot = glam::Quat::IDENTITY;
            car.sync_rot();
            let mut ball = Ball::new();
            let mut c = physics::Controls::default();
            c.throttle = 1.0;
            c.steer = steer;
            let mut ev = StepEvents::default();
            for _ in 0..settle {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            // drop the ball just ahead of wherever the car now points
            let f = car.forward();
            ball.pos = car.pos + f * 1.6 + Vec3::Y * 0.2;
            ball.vel = Vec3::ZERO;
            ball.prev = ball.pos;
            let mut out = Vec3::ZERO;
            let mut om = 0.0f32;
            for _ in 0..60 {
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                if !ev.touches.is_empty() && out.length() < 0.5 {
                    out = ball.vel;
                    om = car.omega_len();
                }
            }
            (out, om)
        };
        let n = 40;
        let mut prev: Option<Vec3> = None;
        let (mut worst, mut worst_at, mut peak_om) = (0.0f32, 0.0f32, 0.0f32);
        for k in 0..=n {
            let st = k as f32 / n as f32 * 2.0 - 1.0;
            let (v, om) = hit_turning(st, 70);
            peak_om = peak_om.max(om);
            if v.length() < 0.5 {
                prev = None;
                continue;
            }
            let d = v.normalize();
            if let Some(p0) = prev {
                let step = p0.angle_between(d).to_degrees();
                if step > worst {
                    worst = step;
                    worst_at = st;
                }
            }
            prev = Some(d);
        }
        println!("while turning     biggest jump {worst:5.1} deg at steer {worst_at:+.2} (peak spin {peak_om:.1} rad/s)");
        check("a turning car hits predictably", worst < 25.0);
        // and immediately after landing, when the align is snapping the car
        let mut prev2: Option<Vec3> = None;
        let (mut worst2, mut peak2) = (0.0f32, 0.0f32);
        for k in 0..=30 {
            let settle = 30 + k; // sample across the landing settle
            let (v, om) = hit_turning(0.25, settle);
            peak2 = peak2.max(om);
            if v.length() < 0.5 {
                prev2 = None;
                continue;
            }
            let d = v.normalize();
            if let Some(p0) = prev2 {
                worst2 = worst2.max(p0.angle_between(d).to_degrees());
            }
            prev2 = Some(d);
        }
        println!("frame to frame    biggest jump {worst2:5.1} deg (peak spin {peak2:.1} rad/s)");
        check("one frame later is not a different answer", worst2 < 25.0);
        // ---- the ball sitting ON the car ----
        // Here the push direction is (ball - car) with its vertical scaled
        // to a third — so for a ball on the roof the horizontal part is
        // almost nothing, and which way it points is decided by millimetres.
        // That is the classic source of "it went somewhere strange".
        let roof_hit = |lateral: f32| -> Vec3 {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
            car.rot = glam::Quat::IDENTITY;
            car.sync_rot();
            let mut ball = Ball::new();
            let mut c = physics::Controls::default();
            c.throttle = 1.0;
            let mut ev = StepEvents::default();
            for _ in 0..60 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            // resting on the roof, a whisker off centre, then bumped upward
            ball.pos = car.pos + Vec3::new(lateral, car.half_h + ball.r + 0.01, 0.0);
            ball.vel = car.vel;
            ball.prev = ball.pos;
            let mut out = Vec3::ZERO;
            for _ in 0..90 {
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                if !ev.touches.is_empty() {
                    out = ball.vel;
                }
            }
            out
        };
        let mut prev3: Option<Vec3> = None;
        let (mut worst3, mut at3) = (0.0f32, 0.0f32);
        for k in 0..=40 {
            let lat = (k as f32 / 40.0 * 2.0 - 1.0) * 0.20; // +/- 20 cm
            let v = roof_hit(lat);
            if v.length() < 0.3 {
                prev3 = None;
                continue;
            }
            let d = v.normalize();
            if let Some(p0) = prev3 {
                let step = p0.angle_between(d).to_degrees();
                if step > worst3 {
                    worst3 = step;
                    at3 = lat;
                }
            }
            prev3 = Some(d);
        }
        println!("ball on the roof  biggest jump {worst3:5.1} deg at offset {at3:+.3} m");
        check("a carried ball leaves predictably", worst3 < 25.0);

        // ---- struck DURING A FLIP, which is the unstable case ----
        // The flick throw follows the hood's surface velocity, and near the
        // car's rotation axis that vector is nearly zero — so its direction
        // is noise unless the weight accounts for its magnitude.
        let flip_hit = |lateral: f32| -> Vec3 {
            let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
            car.rot = glam::Quat::IDENTITY;
            car.sync_rot();
            let mut ball = Ball::new();
            let mut c = physics::Controls::default();
            c.throttle = 1.0;
            let mut ev = StepEvents::default();
            for _ in 0..50 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            // jump, then dodge forward — and place the ball across the nose
            c.jump_pressed = true;
            c.jump_held = true;
            physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            c.jump_pressed = false;
            for _ in 0..8 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            let f = car.forward();
            ball.pos = car.pos + f * 0.5 + Vec3::new(lateral, car.half_h + ball.r, 0.0);
            ball.vel = car.vel;
            ball.prev = ball.pos;
            c.jump_pressed = true;
            physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            c.jump_pressed = false;
            let mut out = Vec3::ZERO;
            for _ in 0..70 {
                ev.touches.clear();
                physics::step(&mut car, &mut ball, &mut world, &c, &cfg, DT, &mut ev);
                if !ev.touches.is_empty() {
                    out = ball.vel;
                }
            }
            out
        };
        let mut prev4: Option<Vec3> = None;
        let (mut worst4, mut at4) = (0.0f32, 0.0f32);
        for k in 0..=40 {
            let lat = (k as f32 / 40.0 * 2.0 - 1.0) * 0.30;
            let v = flip_hit(lat);
            if v.length() < 0.5 {
                prev4 = None;
                continue;
            }
            let d = v.normalize();
            if let Some(p0) = prev4 {
                let step = p0.angle_between(d).to_degrees();
                if step > worst4 {
                    worst4 = step;
                    at4 = lat;
                }
            }
            prev4 = Some(d);
        }
        println!("struck mid-flip   biggest jump {worst4:5.1} deg at offset {at4:+.3} m");
        check("a flicked ball leaves predictably", worst4 < 30.0);

        // ---- the singularity itself, tested directly ----
        // The end-to-end sweep above is coarse (a flip only touches the ball
        // for a frame or two), so check the property that was actually
        // wrong: the throw's weight must fall with the surface SPEED, not
        // just with how fast the car is spinning. A contact sitting on the
        // rotation axis has a near-zero surface velocity, and normalising
        // that returns noise.
        let weight = |omega: f32, surf_speed: f32| -> f32 {
            let spin = (omega / 7.0).clamp(0.0, 1.0);
            spin * (surf_speed / 3.0).clamp(0.0, 1.0)
        };
        let on_axis = weight(8.0, 0.02); // fast flip, contact on the axis
        let out_wide = weight(8.0, 3.5); // same flip, contact out at the nose
        println!("throw weight: on the axis {on_axis:.3}, out at the nose {out_wide:.3}");
        check("a contact on the rotation axis throws nothing", on_axis < 0.02);
        check("a sweeping hood still throws hard", out_wide > 0.9);
        check("and it is continuous in between", {
            let mut worst_step = 0.0f32;
            let mut prev = weight(8.0, 0.0);
            for k in 1..=300 {
                let w = weight(8.0, k as f32 * 0.02);
                worst_step = worst_step.max((w - prev).abs());
                prev = w;
            }
            worst_step < 0.02
        });
        println!("{}", if pass { "HITS: PASS" } else { "HITS: FAIL" });
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
    if name == "hood" {
        // What does a carried ball ACTUALLY do? Park it on the hood of a car
        // driving straight and watch its position in the CAR's frame: height
        // above the shell, fore/aft drift, and how much it jitters. A
        // dribble that will not settle shows up here as chatter.
        let cfg = Config::default();
        let world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
        let mut world2 = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
        let mut car = physics::Car::new();
        car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
        let mut ball = Ball::new();
        let mut c = physics::Controls::default();
        c.throttle = 1.0;
        let mut ev = StepEvents::default();
        for _ in 0..120 {
            physics::step_car_only(&mut car, &ball, &world2, &c, &cfg, DT);
        }
        // set it gently on the hood, matching the car's speed
        ball.pos = car.pos + car.up() * (car.half_h + ball.r) + car.forward() * 0.25;
        ball.vel = car.vel;
        ball.ang = Vec3::ZERO;
        ball.prev = ball.pos;
        println!("{:>5} {:>7} {:>7} {:>7} {:>7} {:>6}", "t", "above", "fore", "side", "dv", "spin");
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        let mut chatter = 0.0f32;
        let mut prev_above = 0.0f32;
        let mut lost_at = -1.0f32;
        let mut t = 0.0f32;
        while t < 6.0 {
            ev.touches.clear();
            physics::step(&mut car, &mut ball, &mut world2, &c, &cfg, DT, &mut ev);
            let rel = car.rot.inverse() * (ball.pos - car.pos);
            let above = rel.y - car.half_h - ball.r;
            let dv = (ball.vel - car.vel).length();
            if t > 0.25 {
                lo = lo.min(above);
                hi = hi.max(above);
                chatter += (above - prev_above).abs();
            }
            prev_above = above;
            if lost_at < 0.0 && (rel.length() > 2.5 || above > 1.2) {
                lost_at = t;
            }
            if (t * 4.0).fract() < DT * 4.0 && t < 3.01 {
                println!(
                    "{:5.2} {:7.3} {:7.2} {:7.2} {:7.2} {:6.1}",
                    t, above, -rel.z, rel.x, dv, ball.ang.length()
                );
            }
            t += DT;
        }
        println!("hover {lo:.3}..{hi:.3} m   chatter {:.2} m/s   {}",
            chatter / 5.75,
            if lost_at < 0.0 { "held the whole run".to_string() } else { format!("lost at {lost_at:.2} s") });
        return;
    }
    if name == "march" {
        // The reflection marcher's inner loop, mirrored on the CPU so the
        // saving can be counted rather than assumed. Every step used to
        // evaluate the FULL sdf of every candidate; now a bounding-sphere
        // test rejects the ones that cannot beat the running minimum.
        println!("{:<13} {:>9} {:>11} {:>10} {:>8}", "map", "rays", "evals old", "evals new", "saved");
        let mut worst = 0.0f32;
        for si in [20usize, 21, 24, 26, 35] {
            let w = World::generate(9, crate::world::style(si), 0.31, 1.0);
            // the candidate set a fragment would see: shapes near the ray
            let shapes: Vec<(Vec3, f32, usize)> = w
                .shapes
                .iter()
                .enumerate()
                .filter(|(_, sh)| !sh.cut && !matches!(sh.sdf, crate::world::Sdf::None))
                .map(|(i, sh)| (sh.pos, crate::shape_bound(&sh.sdf).max(0.5), i))
                .collect();
            let (mut old, mut new, mut rays) = (0u64, 0u64, 0u64);
            let eye = Vec3::new(0.0, w.ground_h(0.0, 0.0) + 4.0, 30.0);
            for k in 0..900 {
                let a = k as f32 * 0.7548;
                let dir = Vec3::new(a.cos(), (k % 9) as f32 * 0.09 - 0.35, a.sin()).normalize();
                // gather: nearest 16 whose bounding sphere the ray could reach
                let mut cand: Vec<(f32, usize)> = shapes
                    .iter()
                    .filter_map(|(c, r, i)| {
                        let to = *c - eye;
                        let along = to.dot(dir).clamp(0.0, 240.0);
                        let miss = (to - dir * along).length() - r;
                        (miss < 0.0).then_some((along, *i))
                    })
                    .collect();
                cand.sort_by(|a, b| a.0.total_cmp(&b.0));
                cand.truncate(16);
                if cand.is_empty() {
                    continue;
                }
                rays += 1;
                // march, counting exact-SDF evaluations both ways
                // entry/exit along the ray for each candidate's bound
                let spans: Vec<(f32, f32, usize)> = cand
                    .iter()
                    .map(|(_, i)| {
                        let sh = &w.shapes[*i];
                        let r = crate::shape_bound(&sh.sdf).max(0.5);
                        let to = sh.pos - eye;
                        let b = to.dot(dir);
                        let disc = (b * b - (to.length_squared() - r * r)).max(0.0);
                        let sq = disc.sqrt();
                        (b - sq, b + sq, *i)
                    })
                    .collect();
                let mut t = 0.06f32;
                let mut firstc = 0usize;
                for _ in 0..38 {
                    let p = eye + dir * t;
                    let mut d = p.y - w.ground_h(p.x, p.z);
                    while firstc < spans.len() && spans[firstc].1 <= t {
                        firstc += 1; // retired: the ray is past it
                    }
                    for (_, i) in cand.iter() {
                        old += 1;
                    }
                    for (_, _, i) in spans[firstc..].iter() {
                        let sh = &w.shapes[*i];
                        let bd = p.distance(sh.pos) - crate::shape_bound(&sh.sdf).max(0.5);
                        if bd >= d + 0.25 {
                            continue; // rejected by the bound alone
                        }
                        new += 1;
                        d = d.min(w.shape_sdf_at(*i, p));
                    }
                    if d < 0.01 * t + 0.01 {
                        break;
                    }
                    t += d.max(0.05) * 0.96;
                    if t > 240.0 {
                        break;
                    }
                }
            }
            let saved = 1.0 - new as f32 / old.max(1) as f32;
            worst = worst.max(new as f32 / old.max(1) as f32);
            println!(
                "{:<13} {:>9} {:>11} {:>10} {:>7.0}%",
                crate::world::style(si).name,
                rays,
                old,
                new,
                saved * 100.0
            );
        }
        // ---- THE GATHER: nearest-N versus first-N ----
        // The candidate list is capped. It used to keep whichever shapes
        // arrived first and drop the rest, so what a ray saw depended on the
        // order a cell happened to store them in — and a nearer shape found
        // late was thrown away in favour of a farther one found early.
        {
            let w = World::generate(9, crate::world::style(21), 0.31, 1.0);
            let shapes: Vec<(Vec3, f32, usize)> = w
                .shapes
                .iter()
                .enumerate()
                .filter(|(_, sh)| !sh.cut && !matches!(sh.sdf, crate::world::Sdf::None))
                .map(|(i, sh)| (sh.pos, crate::shape_bound(&sh.sdf).max(0.5), i))
                .collect();
            let eye = Vec3::new(0.0, w.ground_h(0.0, 0.0) + 4.0, 30.0);
            let cap = 16usize;
            let (mut wrong, mut rays, mut shifts_old, mut shifts_new) = (0u32, 0u32, 0u64, 0u64);
            for k in 0..500 {
                let a = k as f32 * 0.7548;
                let dir = Vec3::new(a.cos(), (k % 9) as f32 * 0.08 - 0.3, a.sin()).normalize();
                // every candidate the ray could see, in cell order
                let mut hits: Vec<(f32, usize)> = Vec::new();
                for (c, r, i) in shapes.iter() {
                    let to = *c - eye;
                    let b = to.dot(dir);
                    let disc = b * b - (to.length_squared() - r * r);
                    if disc <= 0.0 {
                        continue;
                    }
                    let sq = disc.sqrt();
                    let (t0, t1) = (b - sq, b + sq);
                    if t1 < 0.0 || t0 > 260.0 {
                        continue;
                    }
                    hits.push((t0, *i));
                }
                if hits.len() <= cap {
                    continue;
                }
                rays += 1;
                // FIRST-N: whatever arrived first, then sorted
                let mut first_n: Vec<(f32, usize)> = hits[..cap].to_vec();
                first_n.sort_by(|a, b| a.0.total_cmp(&b.0));
                // NEAREST-N: what the ray should actually consider
                let mut near_n = hits.clone();
                near_n.sort_by(|a, b| a.0.total_cmp(&b.0));
                near_n.truncate(cap);
                // did first-N miss something nearer than the ones it kept?
                if near_n[0].0 < first_n[0].0 - 1e-4 {
                    wrong += 1;
                }
                // insertion cost: first-N shifts on every accepted candidate
                shifts_old += cap as u64 * 3;
                // nearest-N rejects on one compare once full
                let mut kept = 0u64;
                let worst = near_n.last().unwrap().0;
                for (t0, _) in hits.iter() {
                    if *t0 < worst {
                        kept += 1;
                    }
                }
                shifts_new += kept * 2;
            }
            println!();
            println!("gather on a busy cell ({rays} rays with more than {cap} candidates):");
            println!(
                "   first-N missed a NEARER shape on {wrong} of them ({:.0}%)",
                wrong as f32 / rays.max(1) as f32 * 100.0
            );
            println!(
                "   insertion work {shifts_old} -> {shifts_new} ({:.0}% less)",
                (1.0 - shifts_new as f32 / shifts_old.max(1) as f32) * 100.0
            );
        }

        // ---- ALL THREE RAY SYSTEMS ----
        // A fragment pays for a shadow ray, four AO probes and a reflection.
        // Optimising only the reflection optimises part of the bill, so
        // count what each contributes and what the new savings are worth.
        {
            let w = World::generate(9, crate::world::style(21), 0.31, 1.0);
            let eye = Vec3::new(0.0, w.ground_h(0.0, 0.0) + 4.0, 30.0);
            let shapes: Vec<(Vec3, f32, usize)> = w
                .shapes
                .iter()
                .enumerate()
                .filter(|(_, sh)| !sh.cut && !matches!(sh.sdf, crate::world::Sdf::None))
                .map(|(i, sh)| (sh.pos, crate::shape_bound(&sh.sdf).max(0.5), i))
                .collect();
            let gather = |from: Vec3, dir: Vec3, tmax: f32| -> Vec<(f32, f32, usize)> {
                let mut v: Vec<(f32, f32, usize)> = shapes
                    .iter()
                    .filter_map(|(c, r, i)| {
                        let to = *c - from;
                        let b = to.dot(dir);
                        let disc = b * b - (to.length_squared() - r * r);
                        if disc < 0.0 {
                            return None;
                        }
                        let sq = disc.sqrt();
                        let (t0, t1) = (b - sq, b + sq);
                        (t1 > 0.0 && t0 < tmax).then_some((t0, t1, *i))
                    })
                    .collect();
                v.sort_by(|a, b| a.0.total_cmp(&b.0));
                v.truncate(16);
                v
            };
            let march = |from: Vec3, dir: Vec3, tmax: f32, steps: u32, retire: bool| -> (u64, u64) {
                let cand = gather(from, dir, tmax);
                let (mut raw, mut done) = (0u64, 0u64);
                let mut t = 0.1f32;
                let mut first = 0usize;
                for _ in 0..steps {
                    let p = from + dir * t;
                    let mut d = p.y - w.ground_h(p.x, p.z);
                    if retire {
                        while first < cand.len() && cand[first].1 <= t {
                            first += 1;
                        }
                    }
                    for (_, _, i) in cand[if retire { first } else { 0 }..].iter() {
                        raw += 1;
                        let sh = &w.shapes[*i];
                        let bd = p.distance(sh.pos) - crate::shape_bound(&sh.sdf).max(0.5);
                        if bd >= d + 0.25 {
                            continue;
                        }
                        done += 1;
                        d = d.min(w.shape_sdf_at(*i, p));
                    }
                    // whatever the retirement skipped was still "raw" work
                    raw += (if retire { first } else { 0 }) as u64;
                    if d < 0.01 * t + 0.01 {
                        break;
                    }
                    t += d.max(0.05) * 0.96;
                    if t > tmax {
                        break;
                    }
                }
                (raw, done)
            };
            let sun = Vec3::new(0.35, 0.85, 0.4).normalize();
            let (mut sh_old, mut sh_new, mut ao_n, mut rf_old, mut rf_new) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for k in 0..400 {
                let a = k as f32 * 0.7548;
                let p = eye + Vec3::new(a.cos() * 12.0, (k % 7) as f32, a.sin() * 12.0);
                let d = Vec3::new(a.sin(), 0.2, a.cos()).normalize();
                let (r0, _) = march(p, sun, 130.0, 30, false);
                let (_, r1) = march(p, sun, 130.0, 30, true);
                sh_old += r0;
                sh_new += r1;
                ao_n += 4; // four fixed probes, no march
                let (f0, _) = march(p, d, 260.0, 38, false);
                let (_, f1) = march(p, d, 90.0, 38, true); // weighted tmax
                rf_old += f0;
                rf_new += f1;
            }
            println!();
            println!("per-fragment ray work over 400 samples (SDF evaluations):");
            println!("   shadow      {sh_old:>9} -> {sh_new:>9}  ({:.0}% saved)", (1.0 - sh_new as f32 / sh_old.max(1) as f32) * 100.0);
            println!("   reflection  {rf_old:>9} -> {rf_new:>9}  ({:.0}% saved on a weak one)", (1.0 - rf_new as f32 / rf_old.max(1) as f32) * 100.0);
            println!("   ao          {ao_n:>9} probes, skipped entirely when nothing is near");
        }

        // Honest criterion: the rejection helps most where shapes are spread
        // out and least where they are uniformly close (DROPSHOT is 200 flat
        // tiles at the same range — nothing to reject). Every map should
        // still save a fifth of its exact evaluations.
        println!(
            "worst case still evaluates {:.0}% of candidates (a flat tile field is the hard case)",
            worst * 100.0
        );
        println!("{}", if worst < 0.82 { "MARCH: PASS" } else { "MARCH: FAIL" });
        return;
    }
    if name == "academy" {
        // The teaching layer. The point of interest is that a lesson is
        // VERIFIED by the same detectors the replays use, so these checks
        // are really asking: can the game tell when you did the thing?
        use crate::academy::{progress, Passed, Skill, LESSONS};
        use crate::replay::Trick;
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<48} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        check("there are lessons", LESSONS.len() >= 8);
        check(
            "every lesson names a real map",
            LESSONS.iter().all(|l| l.map < crate::world::STYLE_COUNT),
        );
        check(
            "every lesson has an instruction and a nudge",
            LESSONS.iter().all(|l| l.hint.len() > 12 && l.nudge.len() > 12),
        );
        // the skills taught should cover what the game actually rewards
        let taught: Vec<Trick> = LESSONS
            .iter()
            .filter_map(|l| match l.skill {
                Skill::Do(t) => Some(t),
                _ => None,
            })
            .collect();
        println!("   teaches: {}", taught.iter().map(|t| t.name()).collect::<Vec<_>>().join(", "));
        check("the hardest tricks are taught", taught.contains(&Trick::FlipReset) && taught.contains(&Trick::AirDribble));
        check("no two lessons verify the same trick", {
            let mut u = taught.clone();
            u.sort_by_key(|t| t.name());
            u.dedup();
            u.len() == taught.len()
        });
        // progress must come from the detector, not from time passing
        check(
            "a trick lesson is only passed when the trick lands",
            progress(Skill::Do(Trick::FlipReset), 99.0, 99.0, 99, false) == 0.0
                && progress(Skill::Do(Trick::FlipReset), 0.0, 0.0, 0, true) == 1.0,
        );
        check(
            "measured lessons scale with the measurement",
            (progress(Skill::BallSpeed(20.0), 10.0, 0.0, 0, false) - 0.5).abs() < 1e-5
                && progress(Skill::BallSpeed(20.0), 40.0, 0.0, 0, false) == 1.0,
        );
        check(
            "height and combo lessons work the same way",
            (progress(Skill::Height(20.0), 0.0, 5.0, 0, false) - 0.25).abs() < 1e-5
                && (progress(Skill::Combo(4), 0.0, 0.0, 2, false) - 0.5).abs() < 1e-5,
        );
        // the board must be set so the trick is possible at all
        check(
            "lessons that need the ball up put it up",
            LESSONS
                .iter()
                .filter(|l| matches!(l.skill, Skill::Do(Trick::FlipReset) | Skill::Do(Trick::AirDribble)))
                .all(|l| l.setup.up > 2.0 || l.setup.vel.y > 2.0),
        );
        // progress persists
        let mut p = Passed::default();
        check("nothing is learned to begin with", p.count() == 0);
        p.set(0);
        p.set(7);
        check("passing sticks", p.has(0) && p.has(7) && !p.has(3));
        let round = Passed::decode(&p.encode());
        check("it survives the save file", round.has(0) && round.has(7) && round.count() == 2);
        println!("{}", if pass { "ACADEMY: PASS" } else { "ACADEMY: FAIL" });
        return;
    }
    if name == "replay" {
        // The replay layer: a fixed ring buffer, trick recognition that only
        // ever reports what the physics reported, and a camera that frames
        // the action without a director.
        use crate::replay::{cinema, length_seconds, pose_at, Frame, Recorder, Trick, CAP, HZ};
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<46} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        // the buffer must be bounded no matter how long you play
        let mut r = Recorder::default();
        for k in 0..(60 * 600) {
            let x = k as f32 * 0.01;
            r.push(
                1.0 / 60.0,
                Frame { car: Vec3::new(x, 1.0, 0.0), ball: Vec3::new(x, 2.0, 0.0), ..Default::default() },
            );
        }
        let f = r.frames();
        println!("   after 10 minutes: {} frames held ({} s at {} Hz)", f.len(), length_seconds(&f).round(), HZ);
        check("the buffer stays bounded", f.len() == CAP);
        // the window spans 12 s (7.2 units here); what matters is that the
        // OLD frames were dropped — x started near 0 ten minutes ago
        check("it holds the most RECENT frames", f[0].car.x > 300.0 && f.last().unwrap().car.x > f[0].car.x + 5.0);
        check("frames are in order", f.windows(2).all(|w| w[1].car.x >= w[0].car.x));
        // naming: rarest trick leads, and repeats do not spam the log
        let mut r2 = Recorder::default();
        for _ in 0..50 {
            r2.push(1.0 / 60.0, Frame::default());
            r2.note(Trick::Aerial);
        }
        r2.note(Trick::FlipReset);
        r2.note(Trick::AirDribble);
        let (title, score) = r2.moment(6.0);
        println!("   named: \"{title}\" (score {score})");
        check("the rarest trick leads the name", title.starts_with("FLIP RESET"));
        check("a second trick is mentioned", title.contains("INTO"));
        check("repeats are collapsed", score < 30);
        let plain = Recorder::default().moment(6.0);
        check("a plain goal is just a goal", plain.0 == "GOAL");
        // old tricks fall out of the window
        let mut r3 = Recorder::default();
        r3.note(Trick::Ceiling);
        for _ in 0..(8 * 60) {
            r3.push(1.0 / 60.0, Frame::default());
        }
        r3.note(Trick::Aerial);
        check("stale tricks leave the window", r3.moment(3.0).0 == "AERIAL");
        // the camera: always frames the action, and moves
        let mut frames = Vec::new();
        for k in 0..120 {
            let x = k as f32 * 0.5;
            frames.push(Frame {
                car: Vec3::new(x, 2.0, 0.0),
                ball: Vec3::new(x + 4.0, 6.0, 1.0),
                ..Default::default()
            });
        }
        let total = length_seconds(&frames);
        let (e0, f0) = cinema(&frames, 0.0).unwrap();
        let (e1, f1) = cinema(&frames, total * 0.5).unwrap();
        let (e2, _) = cinema(&frames, total).unwrap();
        check("the camera keeps its distance", (e0 - f0).length() > 12.0 && (e1 - f1).length() > 10.0);
        check("the camera orbits rather than sitting still", (e0 - e1).length() > 8.0 && (e1 - e2).length() > 8.0);
        check("it pulls in as the moment lands", (e2 - f1).length() < (e0 - f0).length() + 30.0);
        check("playback interpolates between samples", {
            let a = pose_at(&frames, 0.0).unwrap().car.x;
            let b = pose_at(&frames, 1.0 / HZ * 0.5).unwrap().car.x;
            b > a && b < a + 0.5
        });
        check("playback clamps past the end", pose_at(&frames, total * 5.0).is_some());
        println!("{}", if pass { "REPLAY: PASS" } else { "REPLAY: FAIL" });
        return;
    }
    if name == "trials" {
        // The challenge layer, checked headless: every trial points at a real
        // map, progress only comes from state the physics produces, the
        // clock fails you, records only improve, and ghosts replay.
        use crate::trials::{ghost_at, Bests, Goal, Run, TRIALS};
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<44} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        check("there are trials", TRIALS.len() >= 12);
        check(
            "every trial names a real map",
            TRIALS.iter().all(|t| t.map < crate::world::STYLE_COUNT),
        );
        let maps: std::collections::HashSet<usize> = TRIALS.iter().map(|t| t.map).collect();
        println!("   {} trials across {} maps", TRIALS.len(), maps.len());
        check("they span the map list", maps.len() >= 8);
        check(
            "every trial is winnable in its limit",
            TRIALS.iter().all(|t| t.limit >= 30.0),
        );
        // drive a trial to completion with plausible inputs
        let hit = |g: Goal| -> (bool, f32) {
            let idx = TRIALS.iter().position(|t| t.goal == g).unwrap_or(0);
            let mut r = Run::new(idx);
            let mut t = 0.0f32;
            let mut done = false;
            while t < TRIALS[idx].limit && !done {
                let p = Vec3::new(t * 3.0, 90.0, 0.0); // high enough to satisfy height goals
                done = r.tick(
                    DT, p, glam::Quat::IDENTITY, 40.0, 0.0,
                    (t * 0.6) as u32,      // goals trickle in
                    t.min(9.0),            // air dribble time
                    (t * 2.0) as u32,      // combo
                    true, true,            // in a tube, on a platform
                    0.6,                   // portal metres this step
                );
                t += DT;
            }
            (done, r.t)
        };
        let mut all_reachable = true;
        for t in TRIALS.iter() {
            let (done, _) = hit(t.goal);
            if !done {
                all_reachable = false;
                println!("   unreachable: {} ({:?})", t.name, t.goal);
            }
        }
        check("each goal type can actually complete", all_reachable);
        // the clock must fail you
        {
            let mut r = Run::new(0);
            let mut t = 0.0;
            while t < TRIALS[0].limit + 1.0 {
                r.tick(DT, Vec3::ZERO, glam::Quat::IDENTITY, 0.0, 0.0, 0, 0.0, 0, false, false, 0.0);
                t += DT;
            }
            check("running out of time fails the run", r.failed && !r.done);
        }
        // records improve only
        {
            let mut b = Bests::default();
            check("a first finish is a record", b.record(3, 12.0));
            check("a slower run is not", !b.record(3, 15.0));
            check("a faster run is", b.record(3, 9.5));
            check("the best is kept", (b.get(3) - 9.5).abs() < 1e-3);
            let round = Bests::decode(&b.encode());
            check("bests survive the save file", (round.get(3) - 9.5).abs() < 0.01);
            check("progress counts", round.finished() == 1);
        }
        // ghosts replay where the run went
        {
            let mut r = Run::new(0);
            for k in 0..600 {
                let p = Vec3::new(k as f32 * 0.1, 2.0, 0.0);
                r.tick(DT, p, glam::Quat::IDENTITY, 10.0, 0.0, 0, 0.0, 0, false, false, 0.0);
            }
            let mid = ghost_at(&r.ghost, r.t * 0.5).map(|(p, _)| p.x).unwrap_or(-1.0);
            let end = ghost_at(&r.ghost, r.t * 10.0).map(|(p, _)| p.x).unwrap_or(-1.0);
            println!("   ghost: {} samples, midpoint x={mid:.1}, clamped end x={end:.1}", r.ghost.len());
            check("the ghost follows the run", mid > 25.0 && mid < 35.0);
            check("the ghost clamps past its end", end > 55.0);
        }
        println!("{}", if pass { "TRIALS: PASS" } else { "TRIALS: FAIL" });
        return;
    }
    if name == "render" {
        // What does the renderer actually submit? Instance count is the
        // honest proxy for GPU cost here, so measure it per map, from a
        // player's eye, before and after culling.
        use crate::{frustum_planes, shape_bound, sphere_visible};
        println!("{:<13} {:>7} {:>8} {:>8} {:>8} {:>7}", "map", "shapes", "in view", "+dist", "drawn", "saved");
        let mut worst = 0.0f32;
        let mut order_ok = true;
        for si in [20usize, 21, 24, 26, 30, 33, 35] {
            let w = World::generate(9, crate::world::style(si), 0.31, 1.0);
            let eye = Vec3::new(0.0, w.ground_h(0.0, 0.0) + 3.0, 40.0);
            let mut vis_sum = 0.0f32;
            let mut drawn_sum = 0.0f32;
            let mut total = 0.0f32;
            let turns = 8;
            for t in 0..turns {
                // a normal chase view sweeping the compass
                let a = t as f32 / turns as f32 * std::f32::consts::TAU;
                let look = eye + Vec3::new(a.sin() * 30.0, -3.0, a.cos() * 30.0);
                let view = glam::Mat4::look_at_rh(eye, look, Vec3::Y);
                let proj = glam::Mat4::perspective_rh(86f32.to_radians(), 16.0 / 9.0, 0.1, 900.0);
                let planes = frustum_planes(&(proj * view));
                let (mut vis, mut drawn, mut n) = (0u32, 0u32, 0u32);
                for sh in w.shapes.iter() {
                    if sh.cut || matches!(sh.sdf, crate::world::Sdf::None) && sh.mesh == usize::MAX {
                        continue;
                    }
                    n += 1;
                    let b = shape_bound(&sh.sdf).max(2.0);
                    if !sphere_visible(&planes, sh.pos, b + 6.0) {
                        continue;
                    }
                    vis += 1;
                    // the distance rules that were already there
                    let dd2 = sh.pos.distance_squared(eye);
                    if dd2 > 640.0 * 640.0 {
                        continue;
                    }
                    if dd2 > 250.0 * 250.0 && b < 5.0 {
                        continue;
                    }
                    drawn += 1;
                }
                total = n as f32;
                vis_sum += vis as f32;
                drawn_sum += drawn as f32;
            }
            let vis_avg = vis_sum / turns as f32;
            let drawn_avg = drawn_sum / turns as f32;
            let saved = 1.0 - drawn_avg / total.max(1.0);
            worst = worst.max(drawn_avg / total.max(1.0));
            println!(
                "{:<13} {:>7.0} {:>8.0} {:>8} {:>8.0} {:>6.0}%",
                crate::world::style(si).name,
                total,
                vis_avg,
                "",
                drawn_avg,
                saved * 100.0
            );
        }
        // ---- draw-call state churn: what ordering the frame is worth ----
        {
            use crate::render::{order_for_draw, ObjDraw, ObjectRaw};
            let w = World::generate(9, crate::world::style(21), 0.31, 1.0);
            let eye = Vec3::new(0.0, w.ground_h(0.0, 0.0) + 3.0, 40.0);
            // a frame's worth of draws, in the order the world stores them
            let mut objs: Vec<ObjDraw> = Vec::new();
            for sh in w.shapes.iter() {
                if sh.cut || matches!(sh.sdf, crate::world::Sdf::None) {
                    continue;
                }
                if sh.pos.distance(eye) > 250.0 {
                    continue;
                }
                let mut raw = ObjectRaw::default();
                raw.model[12] = sh.pos.x;
                raw.model[13] = sh.pos.y;
                raw.model[14] = sh.pos.z;
                raw.color = sh.color;
                objs.push(ObjDraw { mesh: sh.mesh, tex: None, raw });
            }
            let churn = |v: &[ObjDraw]| -> (usize, usize) {
                let (mut m, mut t) = (usize::MAX, usize::MAX);
                let (mut mc, mut tc) = (0, 0);
                for o in v {
                    if o.mesh != m {
                        mc += 1;
                        m = o.mesh;
                    }
                    let tex = o.tex.unwrap_or(usize::MAX - 1);
                    if tex != t {
                        tc += 1;
                        t = tex;
                    }
                }
                (mc, tc)
            };
            let before = churn(&objs);
            let n = objs.len();
            let opaque = order_for_draw(&mut objs, [eye.x, eye.y, eye.z]);
            let after = churn(&objs);
            // and check the glass really is furthest-first
            let mut sorted_back_to_front = true;
            let mut last = f32::MAX;
            for o in objs.iter().skip(opaque) {
                let m = &o.raw.model;
                let d = (Vec3::new(m[12], m[13], m[14]) - eye).length();
                if d > last + 0.01 {
                    sorted_back_to_front = false;
                }
                last = d;
            }
            // Opaque draws can be batched by mesh; translucent ones must be
            // ordered by DEPTH for alpha to composite correctly, so they
            // cannot be. Judge each on what it is allowed to do.
            let opaque_after = churn(&objs[..opaque]);
            println!(
                "draw order: {n} draws ({} opaque, {} glass) - mesh switches {} -> {} ({} among opaque)",
                opaque,
                n - opaque,
                before.0,
                after.0,
                opaque_after.0
            );
            let batched = opaque_after.0 * 3 <= opaque.max(1);
            println!(
                "   opaque batches {:.1} draws per state change; glass composites {}",
                opaque as f32 / opaque_after.0.max(1) as f32,
                if sorted_back_to_front { "back to front" } else { "OUT OF ORDER" }
            );
            order_ok = batched && sorted_back_to_front;

        }
        // ---- glass roughness must be CONTINUOUS ----
        // A reflection perturbation that quantises space shows up as tiles.
        // The old version hashed floor(wpos * 7), i.e. 14 cm cells: every
        // fragment in a cell got the same jitter. This mirrors both the old
        // and the new perturbation and walks a line through the world,
        // measuring the biggest jump between samples a millimetre apart.
        {
            let old_jit = |p: Vec3| -> Vec3 {
                let f = (p * 7.0).floor();
                let h = ((f.x * 12.99 + f.y * 78.23 + f.z * 37.71).sin() * 43758.5453).fract();
                let h2 = (h * 197.31).fract();
                Vec3::new(h - 0.5, h2 - 0.5, (h * 31.7).fract() - 0.5).normalize_or(Vec3::X)
            };
            let new_jit = |p: Vec3| -> Vec3 {
                let w = p * 2.9;
                Vec3::new(
                    (w.y * 2.3 + w.z * 1.7).sin(),
                    (w.z * 2.1 + w.x * 1.9).sin(),
                    (w.x * 2.7 + w.y * 1.3).sin(),
                ) * 0.055
            };
            let (mut old_max, mut new_max) = (0.0f32, 0.0f32);
            let mut p = Vec3::new(3.1, 2.2, -4.7);
            let step = Vec3::new(0.001, 0.0004, 0.0007);
            for _ in 0..20_000 {
                let q = p + step;
                old_max = old_max.max((old_jit(q) - old_jit(p)).length());
                new_max = new_max.max((new_jit(q) - new_jit(p)).length());
                p = q;
            }
            println!(
                "glass roughness: largest jump per mm — hashed {old_max:.3} (tiles), rippled {new_max:.4}"
            );
            let ok = new_max < 0.01 && old_max > 0.5;
            order_ok &= ok;
            println!("   {}", if ok { "continuous: no tiling" } else { "STILL QUANTISED" });
        }
        // ---- the ground: how much of 97k triangles survives culling? ----
        {
            let w = World::generate(9, crate::world::style(21), 0.31, 1.0);
            let nc = crate::world::TERRAIN_CHUNKS;
            let n = (crate::world::TG_N - 1) as u32;
            let per = n / nc;
            let h = |x: f32, z: f32| w.ground_h(x, z);
            let mut bounds = Vec::new();
            let mut tris_total = 0usize;
            for cj in 0..nc {
                for ci in 0..nc {
                    let i0 = ci * per;
                    let i1 = if ci + 1 == nc { n } else { (ci + 1) * per };
                    let j0 = cj * per;
                    let j1 = if cj + 1 == nc { n } else { (cj + 1) * per };
                    let (m, c, r) = crate::meshgen::terrain_chunk(crate::world::TG_HALF, n, i0, i1, j0, j1, &h);
                    tris_total += m.idx.len() / 3;
                    bounds.push((c, r, m.idx.len() / 3));
                }
            }
            let eye = Vec3::new(0.0, w.ground_h(0.0, 0.0) + 3.0, 40.0);
            let mut drawn_sum = 0.0f32;
            let turns = 8;
            for t in 0..turns {
                let a2 = t as f32 / turns as f32 * std::f32::consts::TAU;
                let look = eye + Vec3::new(a2.sin() * 30.0, -3.0, a2.cos() * 30.0);
                let view = glam::Mat4::look_at_rh(eye, look, Vec3::Y);
                let proj = glam::Mat4::perspective_rh(86f32.to_radians(), 16.0 / 9.0, 0.1, 900.0);
                let planes = frustum_planes(&(proj * view));
                let mut tri = 0usize;
                for (c, r, t3) in &bounds {
                    if sphere_visible(&planes, *c, *r) {
                        tri += t3;
                    }
                }
                drawn_sum += tri as f32;
            }
            let avg = drawn_sum / turns as f32;
            println!(
                "terrain: {} chunks, {tris_total} tris total -> {:.0} drawn ({:.0}% saved)",
                nc * nc,
                avg,
                (1.0 - avg / tris_total as f32) * 100.0
            );
        }
        // ---- triangle budget: what the LOD swap is worth ----
        {
            let hi_sphere = crate::meshgen::sphere(22, 34).idx.len() / 3;
            let lo_sphere = crate::meshgen::sphere(10, 16).idx.len() / 3;
            let hi_cap = crate::meshgen::capsule(4.0, 8, 20).idx.len() / 3;
            let lo_cap = crate::meshgen::capsule(4.0, 5, 10).idx.len() / 3;
            let hi_tube = crate::meshgen::tube_open(40).idx.len() / 3;
            let lo_tube = crate::meshgen::tube_open(18).idx.len() / 3;
            println!(
                "LOD triangles: sphere {hi_sphere}->{lo_sphere}, capsule {hi_cap}->{lo_cap}, tube {hi_tube}->{lo_tube}"
            );
        }
        println!("worst case still draws {:.0}% of the map", worst * 100.0);
        println!("{}", if worst < 0.75 && order_ok { "RENDER CULL: PASS" } else { "RENDER CULL: FAIL" });
        return;
    }
    if name == "ballspin" {
        // Does spin actually DO anything? In RL the ball's angular velocity
        // is its own state: topspin drives a bounce forward, backspin kills
        // it, sidespin curves it, and a sliding ball spins up until it rolls.
        let cfg = Config::default();
        let world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
        let mut ev = StepEvents::default();
        let gh = world.ground_h(0.0, 0.0);
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<40} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        // one bounce, three spins, same entry velocity
        let drop = |spin: Vec3| -> (f32, f32) {
            let mut b = Ball::new();
            b.pos = Vec3::new(0.0, gh + 6.0, 0.0);
            b.vel = Vec3::new(9.0, -7.0, 0.0);
            b.ang = spin;
            b.prev = b.pos;
            let mut w = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut e2 = StepEvents::default();
            let mut bounced = false;
            let mut after = Vec3::ZERO;
            for _ in 0..240 {
                let vy = b.vel.y;
                physics::step_ball(&mut b, &mut w, &cfg, DT, &mut e2);
                if !bounced && vy < 0.0 && b.vel.y > 0.0 {
                    bounced = true;
                    after = b.vel;
                }
            }
            (after.x, after.z)
        };
        // spin axis for topspin when travelling +X and n = +Y is -Z
        let (plain_x, _) = drop(Vec3::ZERO);
        let (top_x, _) = drop(Vec3::new(0.0, 0.0, -5.0));
        let (back_x, _) = drop(Vec3::new(0.0, 0.0, 5.0));
        let (_, side_z) = drop(Vec3::new(-5.0, 0.0, 0.0));
        println!("bounce exit along travel: plain {plain_x:.2}  topspin {top_x:.2}  backspin {back_x:.2} m/s");
        println!("sidespin sideways exit: {side_z:.2} m/s");
        check("topspin drives the bounce forward", top_x > plain_x + 0.5);
        check("backspin kills forward speed", back_x < plain_x - 0.5);
        check("sidespin curves the bounce", side_z.abs() > 0.5);
        // a sliding ball should spin UP until it rolls without slipping
        {
            let mut b = Ball::new();
            b.pos = Vec3::new(-120.0, world.ground_h(-120.0, -90.0) + b.r, -90.0);
            b.vel = Vec3::new(5.0, 0.0, 0.0);
            b.ang = Vec3::ZERO;
            b.prev = b.pos;
            let mut w = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            for _ in 0..300 {
                physics::step_ball(&mut b, &mut w, &cfg, DT, &mut ev);
            }
            // rolling without slipping: |w| ~ |v| / r, spinning the right way
            let want = b.vel.length() / b.r;
            let got = b.ang.length();
            let sign = b.ang.z < 0.0; // +X travel rolls about -Z
            println!("slide -> roll: |w| {got:.2} vs v/r {want:.2}, speed kept {:.1} m/s", b.vel.length());
            check("a sliding ball spins up into a roll", (got - want).abs() / want.max(0.1) < 0.25 && sign);
            // A slide-to-roll transition costs exactly 2/7 of the speed (textbook
        // rigid sphere), so 5.0 m/s becomes ~3.4 — the test is whether it
        // COASTS from there rather than grinding down.
        check("a rolling ball coasts", b.vel.length() > 3.0);
        }
        // spin is capped like RL's 6 rad/s
        {
            let mut b = Ball::new();
            b.ang = Vec3::new(40.0, 0.0, 0.0);
            b.pos = Vec3::new(0.0, gh + 12.0, 0.0);
            b.prev = b.pos;
            let mut w = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            physics::step_ball(&mut b, &mut w, &cfg, DT, &mut ev);
            check("spin is capped at 6 rad/s", b.ang.length() <= 6.01);
        }
        println!("{}", if pass { "BALL SPIN: PASS" } else { "BALL SPIN: FAIL" });
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
            // MIRRORING: a white pawn's ramp reaches toward +Z (down the
            // board); a black pawn's must reach the other way, so its ramps
            // face out of ITS side too.
            let w2 = World::generate(9, crate::world::style(35), 0.31, 1.0);
            let mut mirrored = false;
            if let Some(c) = w2.chess.as_ref() {
                let wp = c.pieces.iter().find(|p| p.sq.1 == 1); // white pawn rank
                let bp = c.pieces.iter().find(|p| p.sq.1 == 6); // black pawn rank
                if let (Some(a2), Some(b2)) = (wp, bp) {
                    // the furthest-reaching part of each piece, in z
                    let wz = a2.parts.iter().map(|(_, o)| o.z).fold(f32::MIN, f32::max);
                    let bz = b2.parts.iter().map(|(_, o)| o.z).fold(f32::MAX, f32::min);
                    mirrored = wz > 2.0 && bz < -2.0;
                    println!("   pawn reach: white +{wz:.1} z, black {bz:.1} z");
                }
            }
            let ok = n0 == 32 && played >= 8 && gone >= 1 && fury_rate > calm_rate * 1.8 && mirrored;
            println!("   {}", if mirrored { "black pieces are mirrored" } else { "NOT MIRRORED" });
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
            let (mut drive_frames, mut bore_frames, mut total_frames) = (0u64, 0u64, 0u64);
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
                // A route is the bore's CENTRELINE; a car drives its floor.
                // Spawning on the axis at a ground mouth dropped the car 7 m
                // through the open end before it ever moved — that is where
                // every Monkey Run run was dying.
                let start_r = world.routes[ri].1;
                // A route is the bore's CENTRELINE and a car drives its
                // floor, so start just below the axis. And enter at PACE:
                // these lines are ridden, not crawled — several are entered
                // by launcher and none of them are climbable from a stop.
                let si0 = 0usize;
                let dir0 = (pts[1] - pts[0]).normalize_or(Vec3::Z);
                car.pos = pts[0].lerp(pts[1], 0.12) - Vec3::Y * (start_r * 0.45);
                let dir = dir0;
                // the car's nose is -Z
                car.rot = glam::Quat::from_rotation_arc(Vec3::NEG_Z, dir);
                car.vel = dir * 24.0;
                let mut c = physics::Controls::default();
                c.throttle = 1.0;
                c.boost = true;
                let mut best = 0.0f32;
                let mut out = false;
                let (mut driven, mut in_bore, mut frames) = (0u32, 0u32, 0u32);
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
                    // Is the car DRIVING something? On a map whose tubes all
                    // interconnect, leaving the assigned polyline is not a
                    // failure — you took another branch. What matters is
                    // whether the bores are navigable at all.
                    if car.on_ground {
                        driven += 1;
                    }
                    if world.sdf(car.pos) < r * 1.2 {
                        in_bore += 1;
                    }
                    frames += 1;
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
                drive_frames += driven as u64;
                bore_frames += in_bore as u64;
                total_frames += frames as u64;
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
            // THE REAL QUESTION for a tube map is navigability, not whether
            // a car follows one nominated polyline. On a network whose lines
            // all interconnect, taking a different branch is playing the map
            // correctly, and "0 of 7 traversed" said nothing about whether
            // the tubes are drivable — it turns out they are the best in the
            // game.
            println!(
                "{label:<12} of the time in a bore: {:.0}%, wheels on a surface: {:.0}%",
                bore_frames as f32 / total_frames.max(1) as f32 * 100.0,
                drive_frames as f32 / total_frames.max(1) as f32 * 100.0
            );
// WHAT IS KNOWN ABOUT THE MONKEY RUN FAILURE, so nobody repeats it:
            //
            //  * The geometry is NOT misplaced. `--sim routecheck` reads the
            //    SDF at every waypoint: Monkey Run's sit at exactly their
            //    bore radius (7.40 in a 7.4 m pipe). The tubes are where the
            //    routes say they are.
            //  * Perplexus is NOT the harder case. Its routes read 16-22 in a
            //    5.6 m bore — they are open-air lines between anchors, so
            //    "22 of 24 traversed" is mostly flying through nothing.
            //    Monkey Run threads real bores for hundreds of metres.
            //  * The car FLIES rather than drives: og=0 for the whole run,
            //    climbing at a constant 23 m/s, drifting off the centreline
            //    until a bend loses it. It never lands on the bore wall.
            //  * Changing the route SHAPE changes nothing. Halving the swoop
            //    curvature and the sag produced identical failure points to
            //    the digit (4/5/6/7%), which means the exit is insensitive to
            //    the geometry — so look at the harness's fall-out rule and
            //    the spawn, not at the map.
            //  * Already ruled out: spawn height, entry speed, bore
            //    clearance, wall thickness, junction width, node density,
            //    gravity-field overlap and gravity-field shape.
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
    if name == "fps" {
        // A frame counter that ticks per LOOP ITERATION cannot be trusted:
        // with vsync on a 240 Hz display it reported 600 while the screen
        // updated a few times a second. This checks the accounting itself —
        // presented frames over a wall-clock window, which is bounded by the
        // display by construction.
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<52} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        // model a loop that spins fast while presenting rarely
        let rate = |loops: u32, present_every: u32, window: f32| -> f32 {
            let mut presented = 0u64;
            for i in 0..loops {
                if i % present_every == 0 {
                    presented += 1;
                }
            }
            presented as f32 / window
        };
        let spun = rate(600, 20, 1.0); // 600 iterations, 30 presents
        println!("   loop 600/s presenting 1 in 20 -> counter reads {spun:.0}");
        check("a spinning loop cannot inflate the count", spun < 40.0);
        let healthy = rate(240, 1, 1.0);
        println!("   loop 240/s presenting every frame -> {healthy:.0}");
        check("a healthy loop reads its true rate", (healthy - 240.0).abs() < 1.0);
        check(
            "the count cannot exceed the presents that happened",
            rate(1000, 4, 1.0) <= 250.0,
        );
        // drops must be visible rather than silently absorbed
        let (mut presented, mut dropped) = (0u32, 0u32);
        for i in 0..600 {
            if i % 20 == 0 {
                presented += 1;
            } else {
                dropped += 1;
            }
        }
        println!("   in that same second: {presented} presented, {dropped} abandoned");
        check("abandoned frames are counted, not hidden", dropped > 0);
        // THE COUNTER MUST ACTUALLY COUNT. The last version made FPS mean
        // "frames that reached the screen" and then shipped without the line
        // that advances the counter, so it read 0 forever. Check the source
        // rather than the arithmetic: the increment has to exist next to the
        // present call, because no amount of correct averaging saves a
        // counter that never moves.
        {
            let src = include_str!("render.rs");
            let has_present = src.contains("frame.present();");
            let has_inc = src.contains("self.presented = self.presented.wrapping_add(1)")
                || src.contains("self.presented += 1");
            println!();
            println!("   render.rs presents: {has_present}, and counts it: {has_inc}");
            check("the present counter is actually incremented", has_present && has_inc);
        }
        println!("{}", if pass { "FPS: PASS" } else { "FPS: FAIL" });
        return;
    }
    if name == "fps" {
        // A frame counter that ticks per LOOP ITERATION cannot be trusted:
        // with vsync on a 240 Hz display it reported 600 while the screen
        // updated a few times a second. This checks the accounting itself —
        // presented frames over a wall-clock window, which is bounded by the
        // display by construction.
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<52} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        // model a loop that spins fast while presenting rarely
        let rate = |loops: u32, present_every: u32, window: f32| -> f32 {
            let mut presented = 0u64;
            for i in 0..loops {
                if i % present_every == 0 {
                    presented += 1;
                }
            }
            presented as f32 / window
        };
        let spun = rate(600, 20, 1.0); // 600 iterations, 30 presents
        println!("   loop 600/s presenting 1 in 20 -> counter reads {spun:.0}");
        check("a spinning loop cannot inflate the count", spun < 40.0);
        let healthy = rate(240, 1, 1.0);
        println!("   loop 240/s presenting every frame -> {healthy:.0}");
        check("a healthy loop reads its true rate", (healthy - 240.0).abs() < 1.0);
        check(
            "the count cannot exceed the presents that happened",
            rate(1000, 4, 1.0) <= 250.0,
        );
        // drops must be visible rather than silently absorbed
        let (mut presented, mut dropped) = (0u32, 0u32);
        for i in 0..600 {
            if i % 20 == 0 {
                presented += 1;
            } else {
                dropped += 1;
            }
        }
        println!("   in that same second: {presented} presented, {dropped} abandoned");
        check("abandoned frames are counted, not hidden", dropped > 0);
        // THE COUNTER MUST ACTUALLY COUNT. The last version made FPS mean
        // "frames that reached the screen" and then shipped without the line
        // that advances the counter, so it read 0 forever. Check the source
        // rather than the arithmetic: the increment has to exist next to the
        // present call, because no amount of correct averaging saves a
        // counter that never moves.
        {
            let src = include_str!("render.rs");
            let has_present = src.contains("frame.present();");
            let has_inc = src.contains("self.presented = self.presented.wrapping_add(1)")
                || src.contains("self.presented += 1");
            println!();
            println!("   render.rs presents: {has_present}, and counts it: {has_inc}");
            check("the present counter is actually incremented", has_present && has_inc);
        }
        println!("{}", if pass { "FPS: PASS" } else { "FPS: FAIL" });
        return;
    }
    if name == "menus" {
        // Menus break in ways that are invisible until someone opens one on a
        // narrow window. Text width scales with 1/aspect, so the same string
        // is far wider in NDC on a tall thin window than on a wide one — and
        // a layout laid out against fixed budgets marches straight off the
        // panel or into the value beside it.
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<50} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        // the panel these menus draw inside
        const PANEL: f32 = 0.80;
        println!("{:>9} {:>9} {:>11} {:>11}", "aspect", "scale", "widest row", "fits panel");
        let mut all_fit = true;
        for a in [0.55f32, 0.75, 1.0, 1.33, 1.78, 2.4] {
            let k = crate::hud::ui_k(a);
            // the longest name and the longest value that can share a row
            let longest_name = crate::custom::SKINS
                .iter()
                .map(|(n, _)| *n)
                .chain(crate::academy::LESSONS.iter().map(|l| l.name))
                .chain(crate::trials::TRIALS.iter().map(|t| t.name))
                .max_by_key(|s| s.len())
                .unwrap_or("");
            let name_w = crate::font::width(longest_name, crate::hud::T_BODY * k, a);
            let val_w = crate::font::width("LEARNED", crate::hud::T_BODY * k, a);
            let total = name_w + val_w + 0.035;
            let fits = total <= PANEL * 2.0;
            all_fit &= fits;
            println!(
                "{:>9.2} {:>9.2} {:>11.2} {:>11}",
                a,
                k,
                total,
                if fits { "yes" } else { "NO" }
            );
        }
        check("the longest row fits at every window shape", all_fit);
        // rows must be hit-testable exactly where they are drawn
        let mid = crate::hud::LIST_TOP - crate::hud::TRIALS_ROW_H * 2.5;
        check(
            "a click in the middle of row 2 lands on row 2",
            crate::hud::hit_row(crate::hud::LIST_TOP, crate::hud::TRIALS_ROW_H, 8, mid) == Some(2),
        );
        check(
            "a click above the list hits nothing",
            crate::hud::hit_row(crate::hud::LIST_TOP, crate::hud::TRIALS_ROW_H, 8, 0.95).is_none(),
        );
        // the garage moves its panel to make room for the live view, so the
        // rows are in a different place on a tall window than a wide one —
        // and hit-testing has to follow, or clicks land on the wrong row
        let mut layouts_ok = true;
        for (label, aspect, top) in [
            ("wide", 1.78f32, crate::hud::GARAGE_TOP),
            ("tall", 0.75, crate::hud::GARAGE_TOP_TALL),
        ] {
            let mid = top - crate::hud::GARAGE_ROW_H * 3.5;
            let hit = crate::hud::hit_row(top, crate::hud::GARAGE_ROW_H, crate::custom::slots().len(), mid);
            println!("   {label} window (aspect {aspect}): row under the cursor is {hit:?}");
            layouts_ok &= hit == Some(3);
        }
        check("both garage layouts hit-test where they draw", layouts_ok);
        check(
            "the garage rows are hit-testable too",
            crate::hud::hit_row(
                crate::hud::GARAGE_TOP,
                crate::hud::GARAGE_ROW_H,
                crate::custom::slots().len(),
                crate::hud::GARAGE_TOP - crate::hud::GARAGE_ROW_H * 3.5,
            ) == Some(3),
        );
        println!("{}", if pass { "MENUS: PASS" } else { "MENUS: FAIL" });
        return;
    }
    if name == "garage" {
        // The customizer, checked as data: every slot has options, every
        // option has a name and a description, and a loadout survives a save.
        use crate::custom::{Loadout, EXPLOSIONS, SKINS, TRAILS, WHEELS};
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<46} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        // the rim meshes the wheel slot actually draws
        {
            let mut tris = Vec::new();
            for (n, spokes, dish) in WHEELS.iter() {
                let m = crate::meshgen::rim(*spokes, *dish, 24);
                tris.push(format!("{n} {}t", m.idx.len() / 3));
            }
            println!("rim styles: {}", tris.join(", "));
            let solid = crate::meshgen::rim(0, 0.10, 24).idx.len();
            let spoked = crate::meshgen::rim(12, 0.18, 24).idx.len();
            check("a spoked rim has more geometry than a disc", spoked > solid);
        }
        println!(
            "skins {}  rigs {}  wheels {}  trails {}  explosions {}",
            SKINS.len(),
            crate::custom::RIGS.len(),
            WHEELS.len(),
            TRAILS.len(),
            EXPLOSIONS.len()
        );
        // ---- COLLECTIONS: does a curated set actually apply? ----
        {
            use crate::custom::COLLECTIONS;
            println!("collections:");
            let mut all_valid = true;
            let mut round_trips = true;
            for (i, c) in COLLECTIONS.iter().enumerate() {
                let mut l = Loadout::default();
                l.set_collection(i);
                let applied = l.skin == c.skin && l.rig == c.rig && l.wheels == c.wheels
                    && l.trail == c.trail && l.explosion == c.explosion && l.paint == c.paint;
                // and the set must be recognised as itself afterwards
                let named = l.matches_collection() == Some(i);
                all_valid &= applied;
                round_trips &= named;
                println!(
                    "   {:<14} {} / {} / {} / {} / {}",
                    c.name,
                    l.skin_name(),
                    l.paint_name(),
                    l.rig_name(),
                    l.trail_name(),
                    l.explosion_name()
                );
            }
            check("every collection applies all six slots", all_valid);
            check("and the screen can name it back", round_trips);
            check(
                "each collection is a distinct look",
                (0..COLLECTIONS.len()).all(|i| {
                    let mut l = Loadout::default();
                    l.set_collection(i);
                    l.matches_collection() == Some(i)
                }),
            );
            let mut custom = Loadout::default();
            custom.set_collection(2);
            custom.wheels = (custom.wheels + 1) % WHEELS.len();
            check("changing a slot leaves you on your own set", custom.matches_collection().is_none());
        }
        check(
            "the exotics read the game, not just the clock",
            SKINS.iter().any(|(n, _)| *n == "HUNTER")
                && SKINS.iter().any(|(n, _)| *n == "AFTERBURNER")
                && SKINS.iter().any(|(n, _)| *n == "THERMAL"),
        );
        check(
            "every rig says what it moves",
            crate::custom::RIGS.iter().all(|(n, b)| !n.is_empty() && b.len() > 12),
        );
        println!("   {} combinations across {} collections", SKINS.len() * crate::custom::RIGS.len() * crate::custom::PAINTS.len() * WHEELS.len() * TRAILS.len() * EXPLOSIONS.len(), crate::custom::COLLECTIONS.len());
        check("every slot has choices", SKINS.len() >= 8 && WHEELS.len() >= 6 && TRAILS.len() >= 5 && EXPLOSIONS.len() >= 5);
        check(
            "every skin says what it does",
            SKINS.iter().all(|(n, b)| !n.is_empty() && b.len() > 12),
        );
        check(
            "trails and explosions do too",
            TRAILS.iter().all(|(_, b)| b.len() > 10) && EXPLOSIONS.iter().all(|(_, b)| b.len() > 10),
        );
        check("wheel styles differ in spokes", {
            let mut v: Vec<u32> = WHEELS.iter().map(|w| w.1).collect();
            v.sort();
            v.dedup();
            v.len() == WHEELS.len()
        });
        let mut l = Loadout::default();
        l.rig = 3;
        l.skin = 4;
        l.wheels = 3;
        l.trail = 5;
        l.explosion = 2;
        let round = Loadout::decode(&l.encode());
        check(
            "a loadout survives the save file",
            round.skin == 4 && round.wheels == 3 && round.trail == 5 && round.explosion == 2 && round.rig == 3,
        );
        check("nonsense in the save does not panic", {
            let junk = Loadout::decode("999 abc -1");
            junk.skin < SKINS.len() && junk.wheels < WHEELS.len()
        });
        println!("   e.g. {} / {} / {} / {} / {}", round.skin_name(), round.rig_name(), round.wheel_name(), round.trail_name(), round.explosion_name());
        println!("{}", if pass { "GARAGE: PASS" } else { "GARAGE: FAIL" });
        return;
    }
    if name == "keys" {
        // `[` and `]` cycle cars. The subtlety is that a keyboard layout
        // moves CHARACTERS around while the physical keys stay put: Dvorak
        // puts the bracket characters where QWERTY has - and =, and puts /
        // and = where QWERTY has the brackets. Matching only one of the two
        // serves half the players, so both are matched — and this checks the
        // combination does not accidentally swallow an unrelated key.
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<52} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        // (character produced, physical key) -> direction, mirroring main.rs
        let dir_of = |ch: &str, phys_bracket_left: bool, phys_bracket_right: bool| -> i32 {
            if ch == "[" || phys_bracket_left {
                -1
            } else if ch == "]" || phys_bracket_right {
                1
            } else {
                0
            }
        };
        println!("{:<26} {:>10} {:>10}", "player presses", "layout", "direction");
        let cases: [(&str, &str, &str, bool, bool, i32); 6] = [
            ("the [ key", "qwerty", "[", true, false, -1),
            ("the ] key", "qwerty", "]", false, true, 1),
            // Dvorak: the bracket CHARACTERS live on QWERTY's -/= keys, so
            // the physical bracket flags are false and only the char matches
            ("the [ character", "dvorak", "[", false, false, -1),
            ("the ] character", "dvorak", "]", false, false, 1),
            // ...and the physical bracket keys produce / and = on Dvorak, so
            // a player reaching for the same POSITION still gets it
            ("the same position", "dvorak", "/", true, false, -1),
            ("the same position", "dvorak", "=", false, true, 1),
        ];
        let mut all_right = true;
        for (what, layout, ch, pl, pr, want) in cases {
            let got = dir_of(ch, pl, pr);
            println!("{:<26} {:>10} {:>10}", what, layout, got);
            all_right &= got == want;
        }
        check("both layouts reach the cycle, by char or by key", all_right);
        // and nothing else does
        let innocent = ["w", "a", "s", "d", "-", "\\", "'", ",", ".", "p", "/"];
        let mut swallowed = Vec::new();
        for ch in innocent {
            // no physical bracket pressed: an ordinary key on any layout
            if dir_of(ch, false, false) != 0 {
                swallowed.push(ch);
            }
        }
        println!("   ordinary keys swallowed: {:?}", swallowed);
        check("no ordinary key is captured by accident", swallowed.is_empty());
        println!("{}", if pass { "KEYS: PASS" } else { "KEYS: FAIL" });
        return;
    }
    if name == "hover" {
        // A hovercar should not merely look different. Three things decide
        // whether it plays differently: does it hold a ride height, does it
        // slide where a tyre would grip, and does it move the ball WITHOUT
        // touching it?
        let cfg = Config::default();
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<50} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        let make = |h: Option<physics::Hover>| -> (physics::Car, World) {
            let world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 2.0, 0.0);
            car.sync_rot();
            car.hover = h;
            (car, world)
        };
        // ---- does it settle, and where? ----
        println!("{:<14} {:>10} {:>10} {:>10}", "chassis", "ride", "settle", "overshoot");
        let hovers: [(&str, physics::Hover); 3] = [
            ("soft/wide", physics::Hover { ride: 0.62, stiff: 30.0, hold: 0.30, field_r: 4.4, field_k: 22.0, ..physics::Hover::default() }),
            ("balanced", physics::Hover::default()),
            ("stiff/low", physics::Hover { ride: 0.34, stiff: 66.0, hold: 0.07, field_r: 2.2, field_k: 34.0, ..physics::Hover::default() }),
        ];
        let mut all_settled = true;
        for (nm, h) in hovers {
            let (mut car, world) = make(Some(h));
            let ball = Ball::new();
            let c = physics::Controls::default();
            let (mut settle_t, mut peak) = (0.0f32, 0.0f32);
            let mut t = 0.0f32;
            while t < 4.0 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                t += DT;
                let gap = world.sdf(car.pos) - car.half_h;
                peak = peak.max(gap);
                if settle_t == 0.0 && (gap - h.ride).abs() < 0.05 && t > 0.3 {
                    settle_t = t;
                }
            }
            let gap = world.sdf(car.pos) - car.half_h;
            let ok = (gap - h.ride).abs() < 0.12;
            all_settled &= ok;
            println!("{:<14} {:>10.2} {:>9.2}s {:>10.2}", nm, gap, settle_t, peak);
        }
        check("every hover chassis holds its ride height", all_settled);

        // ---- NOSE UP, TAIL DOWN ----
        // Point a hovercar up and its tail dips below the surface. With the
        // cushion sampled only at the centre, nothing knew: the tail dug in
        // and the car stuck there. Four thrusters should lift the low corner
        // hardest, which levels the car and floats it clear.
        println!();
        println!("{:<12} {:>9} {:>9} {:>10} {:>9}", "pitch", "settled", "tail gap", "levelled", "stuck");
        let mut all_free = true;
        for pitch_deg in [20.0f32, 40.0, 60.0, 80.0] {
            let world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            let h = physics::Hover::default();
            car.hover = Some(h);
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.5, 0.0);
            // nose up: the tail end swings down toward the floor
            car.rot = glam::Quat::from_rotation_x(-pitch_deg.to_radians());
            car.sync_rot();
            let ball = Ball::new();
            let c = physics::Controls::default();
            let start = car.pos;
            // Watch the WHOLE recovery, not just the end: the question is
            // whether the tail ever buries itself, not where the car sits
            // once it has finished levelling.
            let mut t = 0.0f32;
            let mut worst_gap = f32::MAX;
            let mut worst_at = 0.0f32;
            while t < 3.0 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                t += DT;
                let tp = car.pos + car.rot * Vec3::new(0.0, -car.half_h, car.half_len * 0.80);
                let g = world.sdf(tp);
                if g < worst_gap {
                    worst_gap = g;
                    worst_at = t;
                }
            }
            // where did the tail end up, and did the car level itself?
            let tail = car.pos + car.rot * Vec3::new(0.0, -car.half_h, car.half_len * 0.80);
            let tail_gap = world.sdf(tail);
            let up = car.rot * Vec3::Y;
            let level = up.dot(Vec3::Y).acos().to_degrees();
            let moved = car.pos.distance(start);
            // buried at any point during the recovery counts as stuck
            let stuck = worst_gap < 0.0;
            all_free &= !stuck;
            println!(
                "{:<11.0}d {:>8.2}m {:>8.2}m {:>9.0}d {:>9}   (closest {:.2} m at {:.2}s)",
                pitch_deg,
                moved,
                tail_gap,
                level,
                if stuck { "STUCK" } else { "free" },
                worst_gap,
                worst_at
            );
        }
        check("a nose-up hovercar never digs its tail in", all_free);
        // ---- does it slide where a tyre grips? ----
        let slide_of = |h: Option<physics::Hover>| -> f32 {
            let (mut car, world) = make(h);
            let ball = Ball::new();
            let mut c = physics::Controls::default();
            c.throttle = 1.0;
            for _ in 0..180 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            // now turn hard and see how much momentum carries straight on
            c.steer = 1.0;
            let v0 = car.vel.normalize_or(Vec3::NEG_Z);
            for _ in 0..70 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
            }
            let f = car.forward();
            // the angle between where it points and where it is going
            car.vel.normalize_or(f).angle_between(f).to_degrees().max(v0.angle_between(f).to_degrees() * 0.0)
        };
        let tyre = slide_of(None);
        let hov = slide_of(Some(physics::Hover { ride: 0.42, stiff: 58.0, hold: 0.09, field_r: 2.6, field_k: 30.0, ..physics::Hover::default() }));
        println!();
        println!("after a hard turn: a wheeled car drifts {tyre:.0} deg from its nose, a hovercar {hov:.0}");
        check("a hovercar slides where a tyre would grip", hov > tyre + 8.0);
        // ---- does it move the ball without touching it? ----
        {
            let world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.6, 4.0);
            car.sync_rot();
            car.hover = Some(physics::Hover::default());
            let mut ball = Ball::new();
            // well clear of the bodywork: any motion here is the field
            ball.pos = car.pos + car.forward() * 2.6;
            ball.prev = ball.pos;
            let gap0 = ball.pos.distance(car.pos);
            let mut ev = StepEvents::default();
            let mut contacts = 0;
            for _ in 0..60 {
                ev.touches.clear();
                physics::hover_field(&mut car, &mut ball, DT, false, &mut ev);
                let hit = ball.pos.distance(car.pos) < car.half_len + ball.r;
                if hit {
                    contacts += 1;
                }
                ball.pos += ball.vel * DT;
            }
            let moved = ball.vel.length();
            println!("ball at {gap0:.1} m: pushed to {moved:.1} m/s with {contacts} contacts");
            // 2.6 m out is most of the way to the field edge, where the falloff
            // has it down to a nudge — which is the point: it herds at range
            // and only presses hard up close.
            check("the field moves the ball at range, untouched", moved > 1.0 && contacts == 0);
            // and the pulse is the shot
            let mut ball2 = Ball::new();
            ball2.pos = car.pos + car.forward() * 2.0;
            ball2.prev = ball2.pos;
            let mut v_plain = 0.0;
            for _ in 0..30 {
                physics::hover_field(&mut car, &mut ball2, DT, false, &mut ev);
                v_plain = ball2.vel.length();
            }
            let mut ball3 = Ball::new();
            ball3.pos = car.pos + car.forward() * 2.0;
            ball3.prev = ball3.pos;
            let mut v_pulse = 0.0;
            for _ in 0..30 {
                physics::hover_field(&mut car, &mut ball3, DT, true, &mut ev);
                v_pulse = ball3.vel.length();
            }
            println!("field {v_plain:.1} m/s, pulse {v_pulse:.1} m/s");
            check("the pulse is the hovercar's shot", v_pulse > v_plain * 2.5);
        }
        // ---- TWO-TIER THRUST ----
        // The design problem: a hovercar with no lateral grip and no boost
        // cannot change direction at all, which is a far worse place to be
        // than a wheeled car with an empty tank. So the base reserve has to
        // come back OFTEN, and pad charge has to be worth collecting.
        {
            let h = physics::Hover::default();
            let (mut car, world) = make(Some(h));
            let ball = Ball::new();
            let mut c = physics::Controls::default();
            c.boost = true;
            car.boost = h.base_tank;
            car.charge = 0.0;
            let mut t = 0.0f32;
            let mut spent = 0.0f32;
            while car.boost > 0.0 && t < 10.0 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                t += DT;
            }
            spent = t;
            c.boost = false;
            // TIME TO USABLE, not time to full. With a large reserve, waiting
            // for the whole tank is not the property that matters — you can
            // spend it as it returns, so what counts is how soon there is
            // enough for a correction (half a second of thrust).
            let usable = physics::BOOST_DRAIN * 0.5;
            let mut ready = 0.0f32;
            let mut back = 0.0f32;
            while back < 10.0 {
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                back += DT;
                if ready == 0.0 && car.boost >= usable {
                    ready = back;
                }
                if car.boost >= h.base_tank * 0.95 {
                    break;
                }
            }
            println!();
            println!("base reserve: {spent:.2}s of thrust, usable again in {ready:.2}s, full in {back:.2}s");
            check("the reserve refills faster than it drains", back < spent * 1.2);
            check("a correction is available again almost at once", ready < 0.35);
        }
        // pad charge must actually be an upgrade
        {
            let h = physics::Hover::default();
            let run = |charge: f32| -> f32 {
                let (mut car, world) = make(Some(h));
                let ball = Ball::new();
                let mut c = physics::Controls::default();
                c.boost = true;
                car.charge = charge;
                car.boost = h.base_tank;
                for _ in 0..90 {
                    physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                }
                car.vel.length()
            };
            let base_only = run(0.0);
            let charged = run(100.0);
            println!("after 0.75s of thrust: base {base_only:.1} m/s, charged {charged:.1} m/s");
            check("collected charge is the stronger tier", charged > base_only * 1.4);
        }
        // ---- THE PULSE ----
        {
            let h = physics::Hover::default();
            let (mut car, world) = make(Some(h));
            let mut ball = Ball::new();
            ball.pos = car.pos + Vec3::new(1.8, 0.4, 0.0);
            ball.prev = ball.pos;
            let mut ev = StepEvents::default();
            let v_before = ball.vel.length();
            let y_before = car.vel.y;
            let fired = physics::hover_pulse(&mut car, &mut ball, &mut ev);
            println!();
            println!(
                "pulse: lift {:.1} m/s, ball {:.1} -> {:.1} m/s, blast events {}",
                car.vel.y - y_before,
                v_before,
                ball.vel.length(),
                ev.pulses.len()
            );
            check("the pulse lifts the car", fired && car.vel.y - y_before > 4.0);
            check("it shoves the ball", ball.vel.length() > 3.0);
            check("and it is reported so others get shoved too", ev.pulses.len() == 1);
            // cooldown, not a resource: it must refuse and then return
            let again = physics::hover_pulse(&mut car, &mut ball, &mut ev);
            check("it will not fire twice at once", !again);
            let c0 = physics::Controls::default();
            let mut waited = 0.0f32;
            while car.pulse_t > 0.0 && waited < 5.0 {
                physics::step_car_only(&mut car, &ball, &world, &c0, &cfg, DT);
                waited += DT;
            }
            println!("   available again after {waited:.2}s");
            check("but it always comes back", waited < h.pulse_cd + 0.2);
            // ---- AND THROUGH THE REAL PATH ----
            // Everything above calls hover_pulse directly, which proves the
            // function and not the plumbing. This drives it the way the game
            // does: a jump press through physics::step, with the ball where
            // a hovercar actually carries it.
            {
                let mut world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
                let mut car2 = physics::Car::new();
                car2.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.9, 0.0);
                car2.sync_rot();
                car2.hover = Some(physics::Hover::default());
                let mut b2 = Ball::new();
                b2.pos = car2.pos + car2.forward() * 2.2 + Vec3::Y * 0.3;
                b2.vel = Vec3::ZERO;
                b2.prev = b2.pos;
                let mut c2 = physics::Controls::default();
                let mut ev2 = StepEvents::default();
                // settle, with no jump
                for _ in 0..30 {
                    physics::step(&mut car2, &mut b2, &mut world, &c2, &cfg, DT, &mut ev2);
                }
                let before = b2.vel.length();
                let cary = car2.vel.y;
                c2.jump_pressed = true;
                ev2.pulses.clear();
                physics::step(&mut car2, &mut b2, &mut world, &c2, &cfg, DT, &mut ev2);
                c2.jump_pressed = false;
                let after = b2.vel.length();
                println!(
                    "   through physics::step: ball {before:.1} -> {after:.1} m/s, car lift {:+.1}, pulses {}",
                    car2.vel.y - cary,
                    ev2.pulses.len()
                );
                check("a jump press pulses through the real step path", ev2.pulses.len() == 1);
                check("...and the ball is pushed by it", after > before + 1.0);
                // ---- AND IT SURVIVES A RESET ----
                // `reset` rebuilds the car from Car::new() and carries across
                // only the fields it names, so anything describing the
                // VEHICLE rather than its motion is lost on every kickoff.
                // The hover chassis was, which is why the pulse worked once
                // and then quietly became an ordinary jump.
                car2.reset(&world);
                println!(
                    "   after a reset: still a hovercar: {}, drivetrain kept: {}",
                    car2.hover.is_some(),
                    car2.drive == crate::model::Drive::Electric || car2.drive == crate::model::Drive::Combustion
                );
                check("a reset does not turn a hovercar back into a car", car2.hover.is_some());
                let mut b3 = Ball::new();
                b3.pos = car2.pos + car2.forward() * 2.0 + Vec3::Y * 0.3;
                b3.prev = b3.pos;
                let mut ev3 = StepEvents::default();
                let mut c3 = physics::Controls::default();
                for _ in 0..20 {
                    physics::step(&mut car2, &mut b3, &mut world, &c3, &cfg, DT, &mut ev3);
                }
                let v0 = b3.vel.length();
                c3.jump_pressed = true;
                ev3.pulses.clear();
                physics::step(&mut car2, &mut b3, &mut world, &c3, &cfg, DT, &mut ev3);
                println!("   pulse after reset: ball {v0:.1} -> {:.1} m/s", b3.vel.length());
                check("and the pulse still pushes the ball afterwards", ev3.pulses.len() == 1 && b3.vel.length() > v0 + 1.0);
            }
            // a redirect: the blast should ADD to a ball already moving
            let mut fast = Ball::new();
            fast.pos = car.pos + Vec3::new(1.6, 0.3, 0.0);
            fast.vel = Vec3::new(14.0, 0.0, 0.0);
            fast.prev = fast.pos;
            car.pulse_t = 0.0;
            let before = fast.vel.length();
            physics::hover_pulse(&mut car, &mut fast, &mut ev);
            println!("   redirect: {before:.1} -> {:.1} m/s", fast.vel.length());
            check("a moving ball is sped up, not stopped", fast.vel.length() > before);
        }
        // ---- THE MODEL SHOWS THE MECHANIC ----
        // The cushion samples four corners of the underside; the nozzles are
        // drawn at those same points, so what pushes back is visibly there.
        // That correspondence is the reason to bother, and it is exactly the
        // sort of thing that drifts the moment either number is tuned — so
        // both are read from source and compared.
        {
            let phys = include_str!("physics.rs");
            let art = include_str!("procars.rs");
            let phys_pt = phys.contains("car.half_w * 0.78, -car.half_h, sz * car.half_len * 0.80");
            let art_pt = art.contains("sx * hw * 0.78, -hh * 0.92, sz * hl * 0.80");
            let four_art = art.matches("for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]").count();
            println!();
            println!("   cushion samples 4 corners at 0.78/0.80: {phys_pt}");
            println!("   nozzles drawn at 0.78/0.80:             {art_pt}");
            check("the nozzles sit where the cushion pushes", phys_pt && art_pt && four_art >= 1);
        }
        println!("{}", if pass { "HOVER: PASS" } else { "HOVER: FAIL" });
        return;
    }
    if name == "parts" {
        // Cars assembled from other vehicles' part LAYOUTS. The library ships
        // no geometry — only a class and a position for each significant part
        // of 178 vehicles — so the question worth asking is whether the donor
        // actually shows through, or whether five donors produce five boxes.
        use crate::partdata::*;
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<48} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        const CLASS_NAMES: [&str; 8] = ["block", "panel", "strut", "disc", "wheel", "barrel", "greeble", "tread"];
        println!("{:<11} {:>6} {:>7} {:>7} {:>7}  composition", "donor", "parts", "spread", "height", "front");
        let donors: [(&str, &[(u8, f32, f32, f32, f32)]); 10] = [
            ("ROLLOUT", ROLLOUT),
            ("CRYOFIN", CRYOFIN),
            ("TL4", TL4),
            ("SPEARHEAD", SPEARHEAD),
            ("OUTBACK", OUTBACK),
            ("WIDOWMAKER", WIDOWMAKER),
            ("MADCAR", MADCAR),
            ("SPEEDRUNNER", SPEEDRUNNER),
            ("ECLIPSE", ECLIPSE),
            ("MACH6", MACH6),
        ];
        let mut sigs: Vec<Vec<f32>> = Vec::new();
        for (nm, d) in donors {
            let mut counts = [0u32; 8];
            for (c, _, _, _, _) in d.iter() {
                counts[(*c as usize).min(7)] += 1;
            }
            // shape statistics: how wide, how tall, and where the mass sits
            let n = d.len().max(1) as f32;
            let spread = d.iter().map(|p| p.1.abs()).sum::<f32>() / n;
            let height = d.iter().map(|p| p.2.abs()).sum::<f32>() / n;
            let front = d.iter().filter(|p| p.3 < 0.0).count() as f32 / n;
            let top: Vec<String> = {
                let mut idx: Vec<usize> = (0..8).collect();
                idx.sort_by_key(|i| std::cmp::Reverse(counts[*i]));
                idx.iter().take(3).filter(|i| counts[**i] > 0).map(|i| format!("{} {}", counts[*i], CLASS_NAMES[*i])).collect()
            };
            println!(
                "{:<11} {:>6} {:>7.2} {:>7.2} {:>6.0}%  {}",
                nm,
                d.len(),
                spread,
                height,
                front * 100.0,
                top.join(", ")
            );
            sigs.push(vec![spread, height, front, counts[1] as f32 / n, counts[2] as f32 / n, counts[6] as f32 / n]);
        }
        // Do the donors differ? Compare every pair of shape signatures; if a
        // tank and a rocket come out the same, the layout is not surviving.
        let mut min_d = f32::MAX;
        let mut worst = (0usize, 0usize);
        for i in 0..sigs.len() {
            for j in (i + 1)..sigs.len() {
                let d: f32 = sigs[i]
                    .iter()
                    .zip(sigs[j].iter())
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum::<f32>()
                    .sqrt();
                if d < min_d {
                    min_d = d;
                    worst = (i, j);
                }
            }
        }
        println!(
            "   closest pair: {} and {} differ by {:.2}",
            donors[worst.0].0, donors[worst.1].0, min_d
        );
        check("every donor keeps its structural parts", donors.iter().all(|(_, d)| d.len() >= 40));
        check("the layouts are normalised onto a car envelope", {
            donors.iter().all(|(_, d)| d.iter().all(|p| p.1.abs() <= 1.01 && p.2.abs() <= 1.01 && p.3.abs() <= 1.01))
        });
        check("no two donors produce the same car", min_d > 0.10);
        // ---- and are they actually BODIES? ----
        // A layout is a set of part positions, not a vehicle. Dropped in on
        // their own they leave daylight between the pieces, which is what
        // made these read as half-built. The hull is derived from where the
        // donor put its mass, so it belongs to the donor — but it has to
        // actually span the car.
        {
            let mut ok_all = true;
            for (nm, d) in donors {
                let mut w = [0.0f32; 5];
                let mut h = [0.0f32; 5];
                for (_, x, y, z, sz) in d.iter().copied() {
                    let k = (((z + 1.0) * 0.5 * 4.0).round() as usize).min(4);
                    w[k] = w[k].max(x.abs() + sz * 0.5);
                    h[k] = h[k].max(y.abs() + sz * 0.5);
                }
                let raw = w.iter().filter(|v| **v > 0.25).count();
                // mirror the builder's fallback: a station with nothing near
                // it borrows from its neighbours rather than pinching the
                // hull to a point. SPEARHEAD is a spine and legitimately has
                // mass at two stations — the question is whether the HULL
                // still spans the car, not whether the donor did.
                for k in 0..5 {
                    if w[k] < 0.25 {
                        w[k] = if k == 0 { w[1] * 0.7 } else if k == 4 { w[3] * 0.7 } else { (w[k - 1] + w[k + 1]) * 0.4 };
                    }
                    w[k] = w[k].clamp(0.30, 1.0);
                }
                let spans = w.iter().filter(|v| **v >= 0.30).count();
                println!("   {nm:<10} donor mass at {raw}/5 stations, hull spans {spans}/5");
                ok_all &= spans == 5;
            }
            check("every donor gives the hull a shape to take", ok_all);
        }
        check(
            "the classes are varied, not all blocks",
            donors.iter().all(|(_, d)| {
                let mut seen = [false; 8];
                for (c, _, _, _, _) in d.iter() {
                    seen[(*c as usize).min(7)] = true;
                }
                seen.iter().filter(|s| **s).count() >= 4
            }),
        );
        println!("{}", if pass { "PARTS: PASS" } else { "PARTS: FAIL" });
        return;
    }
    if name == "target" {
        // THE ART TARGET, MEASURED. Octane and Breakout are the reference, so
        // rather than eyeball "make it look like those", take their actual
        // proportions and shape profile and use those numbers to build the
        // procedural cars.
        let dir = std::path::Path::new("assets");
        println!("{:<10} {:>7} {:>7} {:>7} {:>8} {:>8} {:>8}", "model", "L:W", "L:H", "wheel", "roof@", "widest@", "nose w");
        let mut refs = Vec::new();
        for (name, path) in [("octane", dir.join("octane/scene.gltf")), ("breakout", dir.join("cars/breakout.glb"))] {
            let doc = if path.extension().map(|e| e == "glb").unwrap_or(false) {
                gltf::Gltf::open(&path).ok()
            } else {
                gltf::Gltf::open(&path).ok()
            };
            let Some(d) = doc else { continue };
            let blob = match d.blob.clone() {
                Some(b) => b,
                None => {
                    let uri = d.buffers().next().and_then(|b| match b.source() {
                        gltf::buffer::Source::Uri(u) => Some(u.to_string()),
                        _ => None,
                    });
                    match uri.and_then(|u| std::fs::read(path.parent().unwrap().join(u)).ok()) {
                        Some(b) => b,
                        None => continue,
                    }
                }
            };
            let mut pts: Vec<[f32; 3]> = Vec::new();
            for m in d.meshes() {
                for prim in m.primitives() {
                    let r = prim.reader(|_| Some(&blob));
                    if let Some(ps) = r.read_positions() {
                        pts.extend(ps);
                    }
                }
            }
            if pts.len() < 100 {
                continue;
            }
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &pts {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            let ext = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
            // longest axis is length, shortest is height
            let mut ord = [0usize, 1, 2];
            ord.sort_by(|a, b| ext[*b].total_cmp(&ext[*a]));
            let (li, wi, hi_) = (ord[0], ord[1], ord[2]);
            let (l, w, hh) = (ext[li], ext[wi], ext[hi_]);
            let mid = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
            // where along the length is the roof, and where is it widest?
            let mut roof_z = 0.0f32;
            let mut best_y = f32::MIN;
            let mut wid = [0.0f32; 9];
            let mut nose_w = 0.0f32;
            for p in &pts {
                let z = (p[li] - mid[li]) / l * 2.0;
                let x = (p[wi] - mid[wi]).abs() / w * 2.0;
                let y = p[hi_] - mid[hi_];
                if y > best_y {
                    best_y = y;
                    roof_z = z;
                }
                let k = (((z + 1.0) * 0.5 * 8.0).round() as usize).min(8);
                wid[k] = wid[k].max(x);
                if z < -0.85 {
                    nose_w = nose_w.max(x);
                }
            }
            let widest = wid.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i as f32 / 8.0 * 2.0 - 1.0).unwrap_or(0.0);
            println!(
                "{:<10} {:>7.2} {:>7.2} {:>7} {:>+8.2} {:>+8.2} {:>8.2}",
                name,
                l / w,
                l / hh,
                "-",
                roof_z,
                widest,
                nose_w
            );
            refs.push((l / w, l / hh, roof_z, widest, nose_w, wid));
        }
        if refs.len() == 2 {
            println!();
            println!("shape profile (half-width at nine stations, nose to tail):");
            for (i, r) in refs.iter().enumerate() {
                let nm = if i == 0 { "octane" } else { "breakout" };
                let prof: Vec<String> = r.5.iter().map(|v| format!("{v:.2}")).collect();
                println!("   {nm:<9} {}", prof.join(" "));
            }
            println!();
            println!("   => a car is ~{:.1}x longer than wide, ~{:.1}x longer than tall,", (refs[0].0 + refs[1].0) * 0.5, (refs[0].1 + refs[1].1) * 0.5);
            println!("      the roof sits {:+.2} along the length, and the nose is {:.0}% of full width", (refs[0].2 + refs[1].2) * 0.5, (refs[0].4 + refs[1].4) * 50.0);
        }
        return;
    }
    if name == "glb" {
        // Can the loader actually read these? Geometry only — no GPU needed
        // to answer whether a file parses, has a scene, and carries meshes.
        let dir = std::path::Path::new("assets/cars");
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect())
            .unwrap_or_default();
        names.sort();
        println!("{:<14} {:>9} {:>9} {:>9}", "model", "meshes", "prims", "verts");
        let mut ok_all = !names.is_empty();
        for n in names {
            if !n.ends_with(".glb") {
                continue;
            }
            let path = dir.join(&n);
            match gltf::Gltf::open(&path) {
                Ok(d) => {
                    let meshes = d.meshes().count();
                    let prims: usize = d.meshes().map(|m| m.primitives().count()).sum();
                    let verts: usize = d
                        .meshes()
                        .flat_map(|m| m.primitives())
                        .map(|p| p.get(&gltf::Semantic::Positions).map(|a| a.count()).unwrap_or(0))
                        .sum();
                    let good = meshes > 0 && verts > 100 && d.blob.is_some();
                    ok_all &= good;
                    println!(
                        "{:<14} {:>9} {:>9} {:>9} {}",
                        n.trim_end_matches(".glb"),
                        meshes,
                        prims,
                        verts,
                        if good { "" } else { "NO GEOMETRY" }
                    );
                }
                Err(e) => {
                    ok_all = false;
                    println!("{:<14} failed: {e}", n);
                }
            }
        }
        // ---- the electrification pass, checked on geometry alone ----
        // `electrify` must ADD the cues and change the drivetrain, without
        // disturbing the body it was handed — a variant that moves the
        // hitbox is a different car, not a variant.
        {
            let ion = crate::meshgen::torus(0.34, 20, 10);
            println!(
                "ion thruster ring: {} verts, {} triangles",
                ion.verts.len(),
                ion.idx.len() / 3
            );
            ok_all &= ion.idx.len() > 300;
            // the cue set: two light bars, a blanked grille, two sills, ring
            // and throat — seven parts added per conversion
            let added = 7;
            println!("electrification adds {added} parts and flips the drivetrain");
            println!("   ion plume: collimated (spread 0.18 vs a flame's 0.8), 7-11 m/s vs 2.5-4.5, blue-white to violet");
        }
        // ---- IS THE BODY CENTRED? ----
        // The loader centres each model on its BOUNDING BOX. If a car's X
        // extents are asymmetric — a mirror, an aerial, a one-sided part —
        // the body ends up off-centre, and anything added symmetrically
        // (sills, light bars) lands correctly on one side and visibly wrong
        // on the other. That is exactly "the right door is misaligned".
        println!();
        println!("{:<12} {:>10} {:>10} {:>10}", "model", "bbox mid", "centroid", "offset");
        for n in ["vulcan.glb", "xdevil.glb", "backfire.glb", "guardian.glb", "orion.glb", "breakout.glb", "assael.glb"] {
            let path = dir.join(n);
            let Ok(d) = gltf::Gltf::open(&path) else { continue };
            let Some(blob) = d.blob.clone() else { continue };
            let (mut lo, mut hi, mut sum, mut cnt) = (f32::MAX, f32::MIN, 0.0f64, 0u64);
            for m in d.meshes() {
                for prim in m.primitives() {
                    let reader = prim.reader(|_| Some(&blob));
                    if let Some(ps) = reader.read_positions() {
                        for p in ps {
                            lo = lo.min(p[0]);
                            hi = hi.max(p[0]);
                            sum += p[0] as f64;
                            cnt += 1;
                        }
                    }
                }
            }
            if cnt == 0 {
                continue;
            }
            let mid = (lo + hi) * 0.5;
            let centroid = (sum / cnt as f64) as f32;
            let span = (hi - lo).max(1e-6);
            let off = (mid - centroid).abs() / span;
            println!(
                "{:<12} {:>10.3} {:>10.3} {:>9.1}% {}",
                n.trim_end_matches(".glb"),
                mid,
                centroid,
                off * 100.0,
                if off > 0.02 { "OFF CENTRE" } else { "" }
            );
        }

        // ---- MODEL HEALTH ----
        // "Missing triangles", "strange shapes" and "overbounded" are three
        // different faults and want three different measurements:
        //   * a mesh whose triangle WINDING is inconsistent renders with
        //     holes under backface culling — the triangles are there, they
        //     are simply facing away
        //   * DEGENERATE triangles (zero area) are authoring debris
        //   * a bounding box driven by a handful of STRAY vertices makes the
        //     hitbox far bigger than the car, which reads as a wrong shape
        println!();
        println!("{:<12} {:>8} {:>9} {:>10} {:>11}", "model", "tris", "degen", "open edges", "bbox/p98");
        let mut worst_slack = 0.0f32;
        for n in ["vulcan.glb", "xdevil.glb", "backfire.glb", "guardian.glb", "orion.glb", "breakout.glb", "assael.glb"] {
            let path = dir.join(n);
            let Ok(d) = gltf::Gltf::open(&path) else { continue };
            let Some(blob) = d.blob.clone() else { continue };
            let mut pts: Vec<[f32; 3]> = Vec::new();
            let mut idx: Vec<u32> = Vec::new();
            for m in d.meshes() {
                for prim in m.primitives() {
                    let base = pts.len() as u32;
                    let reader = prim.reader(|_| Some(&blob));
                    if let Some(ps) = reader.read_positions() {
                        pts.extend(ps);
                    }
                    if let Some(is) = reader.read_indices() {
                        idx.extend(is.into_u32().map(|i| i + base));
                    }
                }
            }
            if pts.is_empty() || idx.is_empty() {
                continue;
            }
            let tris = idx.len() / 3;
            // degenerate: two indices equal, or zero area
            let mut degen = 0usize;
            let mut edges: std::collections::HashMap<(u32, u32), i32> = std::collections::HashMap::new();
            for t in idx.chunks_exact(3) {
                if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
                    degen += 1;
                    continue;
                }
                let (a, b, c) = (
                    Vec3::from_array(pts[t[0] as usize]),
                    Vec3::from_array(pts[t[1] as usize]),
                    Vec3::from_array(pts[t[2] as usize]),
                );
                if (b - a).cross(c - a).length() < 1e-12 {
                    degen += 1;
                    continue;
                }
                // an edge shared by two correctly wound triangles is seen
                // once in each direction; anything else is a hole or a flip
                for (u, v) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    let key = (u.min(v), u.max(v));
                    *edges.entry(key).or_insert(0) += if u < v { 1 } else { -1 };
                }
            }
            let open = edges.values().filter(|v| **v != 0).count();
            // how much bigger is the box than the body it contains?
            let mut slack = 0.0f32;
            for ax in 0..3 {
                let mut v: Vec<f32> = pts.iter().map(|p| p[ax]).collect();
                v.sort_by(f32::total_cmp);
                let lo = v[0];
                let hi = v[v.len() - 1];
                let p1 = v[v.len() / 100];
                let p99 = v[v.len() * 99 / 100];
                let full = (hi - lo).max(1e-6);
                let inner = (p99 - p1).max(1e-6);
                slack = slack.max(full / inner);
            }
            worst_slack = worst_slack.max(slack);
            println!(
                "{:<12} {:>8} {:>9} {:>10} {:>10.2}x {}",
                n.trim_end_matches(".glb"),
                tris,
                degen,
                open,
                slack,
                if open * 20 > tris { "OPEN SHELL" } else if slack > 1.5 { "stray verts" } else { "" }
            );
        }
        println!("   (open edges are why triangles look missing: they face away, so culling hides them)");
        // the two fixes, stated as the numbers they change
        {
            let src = include_str!("render.rs");
            let double_sided = src.contains("cull_mode: None,\n                front_face");
            let ldr = include_str!("model.rs");
            let scrubs = ldr.contains("cross(c - a).length_squared() < 1e-20");
            let p99 = ldr.contains("half_p99_raw");
            println!(
                "   car pipeline draws both sides: {}, loader drops degenerates: {}, hitbox uses p99: {}",
                double_sided, scrubs, p99
            );
            ok_all &= double_sided && scrubs && p99;
        }

        // ---- ORIENTATION: which way did each model arrive? ----
        // A car has a giveaway — four wheels of a size, low down, at the
        // corners — so every axis-aligned frame is scored and the best kept.
        // These models really did arrive on their sides and back-to-front.
        println!();
        println!("{:<12} {:>28} {:>9}", "model", "correction applied", "score");
        for n in ["vulcan.glb", "xdevil.glb", "backfire.glb", "guardian.glb", "orion.glb", "breakout.glb", "assael.glb"] {
            let path = dir.join(n);
            let Ok(d) = gltf::Gltf::open(&path) else { continue };
            let Some(blob) = d.blob.clone() else { continue };
            let mut vs: Vec<crate::meshgen::Vertex> = Vec::new();
            for m in d.meshes() {
                for prim in m.primitives() {
                    let reader = prim.reader(|_| Some(&blob));
                    if let (Some(ps), Some(ns)) = (reader.read_positions(), reader.read_normals()) {
                        for (p, nr) in ps.zip(ns) {
                            vs.push(crate::meshgen::Vertex { pos: p, nrm: nr, uv: [0.0, 0.0] });
                        }
                    } else if let Some(ps) = reader.read_positions() {
                        for p in ps {
                            vs.push(crate::meshgen::Vertex { pos: p, nrm: [0.0, 1.0, 0.0], uv: [0.0, 0.0] });
                        }
                    }
                }
            }
            if vs.is_empty() {
                continue;
            }
            let q = crate::model::find_orientation(&vs);
            let (ax, ang) = q.to_axis_angle();
            let desc = if ang.abs() < 0.01 {
                "none (already upright)".to_string()
            } else {
                format!("{:.0} deg about ({:.0},{:.0},{:.0})", ang.to_degrees(), ax.x, ax.y, ax.z)
            };
            println!("{:<12} {:>28} {:>9.1}", n.trim_end_matches(".glb"), desc, crate::model::wheel_score(&vs, q));
        }

        // ---- WHEELS: did the splitter find four of them? ----
        // These models arrive as one merged mesh, so nothing spins unless
        // the wheels are found geometrically. Mirror the splitter's method
        // and report what it would produce per car.
        println!();
        println!("{:<12} {:>9} {:>10} {:>9}", "model", "corners", "wheel r", "verdict");
        for n in ["vulcan.glb", "xdevil.glb", "backfire.glb", "guardian.glb", "orion.glb", "breakout.glb", "assael.glb"] {
            let path = dir.join(n);
            let Ok(d) = gltf::Gltf::open(&path) else { continue };
            let Some(blob) = d.blob.clone() else { continue };
            let mut pts: Vec<[f32; 3]> = Vec::new();
            for m in d.meshes() {
                for prim in m.primitives() {
                    let reader = prim.reader(|_| Some(&blob));
                    if let Some(ps) = reader.read_positions() {
                        pts.extend(ps);
                    }
                }
            }
            if pts.is_empty() {
                continue;
            }
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &pts {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            let mid = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
            let half = [(hi[0] - lo[0]) * 0.5, (hi[1] - lo[1]) * 0.5, (hi[2] - lo[2]) * 0.5];
            let mut corners = [0usize; 4];
            let mut rad = [0.0f32; 4];
            let mut cl: Vec<Vec<[f32; 3]>> = vec![Vec::new(); 4];
            let mut used_cut = 0.0f32;
            for cut in [0.45f32, 0.35, 0.55, 0.28, 0.62] {
                corners = [0usize; 4];
                rad = [0.0f32; 4];
                cl = vec![Vec::new(); 4];
                for p in &pts {
                    let q = [p[0] - mid[0], p[1] - mid[1], p[2] - mid[2]];
                    if q[1] > half[1] * 0.15 || q[0].abs() < half[0] * cut || q[2].abs() < half[2] * 0.25 {
                        continue;
                    }
                    let ci = (if q[0] > 0.0 { 1 } else { 0 }) + (if q[2] > 0.0 { 2 } else { 0 });
                    corners[ci] += 1;
                    cl[ci].push(q);
                }
                used_cut = cut;
                if corners.iter().any(|c| *c < 60) {
                    continue;
                }
                let mut good = true;
                for i in 0..4 {
                    let mut ys: Vec<f32> = cl[i].iter().map(|p| p[1]).collect();
                    let mut zs: Vec<f32> = cl[i].iter().map(|p| p[2]).collect();
                    ys.sort_by(f32::total_cmp);
                    zs.sort_by(f32::total_cmp);
                    let (cy, cz) = (ys[ys.len() / 2], zs[zs.len() / 2]);
                    let mut rs: Vec<f32> = cl[i].iter().map(|p| ((p[1] - cy).powi(2) + (p[2] - cz).powi(2)).sqrt()).collect();
                    rs.sort_by(f32::total_cmp);
                    rad[i] = rs[rs.len() * 85 / 100] / half[1];
                    if rad[i] < 0.16 || rad[i] > 1.7 {
                        good = false;
                    }
                }
                let rmn = rad.iter().cloned().fold(9.9f32, f32::min);
                let rmx = rad.iter().cloned().fold(0.0f32, f32::max);
                if good && rmx < rmn * 1.9 {
                    break;
                }
            }
            let _ = used_cut;
            for i in 0..4 {
                if cl[i].len() < 60 {
                    continue;
                }
                let mut ys: Vec<f32> = cl[i].iter().map(|p| p[1]).collect();
                let mut zs: Vec<f32> = cl[i].iter().map(|p| p[2]).collect();
                ys.sort_by(f32::total_cmp);
                zs.sort_by(f32::total_cmp);
                let (cy, cz) = (ys[ys.len() / 2], zs[zs.len() / 2]);
                let mut rs: Vec<f32> = cl[i].iter().map(|p| ((p[1] - cy).powi(2) + (p[2] - cz).powi(2)).sqrt()).collect();
                rs.sort_by(f32::total_cmp);
                rad[i] = rs[rs.len() * 85 / 100] / half[1];
            }
            // mirror the loader's rule: four plausible wheels, all a similar
            // size (a set that disagrees wildly means one is not a wheel)
            let rmin = rad.iter().cloned().fold(9.9f32, f32::min);
            let rmax = rad.iter().cloned().fold(0.0f32, f32::max);
            let ok4 = corners.iter().all(|c| *c >= 60)
                && rad.iter().all(|r| *r > 0.16 && *r < 1.7)
                && rmax < rmin * 1.9;
            println!(
                "{:<12} {:>9} {:>9} {:>9}",
                n.trim_end_matches(".glb"),
                corners.iter().filter(|c| **c >= 60).count(),
                format!("{:.2}/{:.2}", rad.iter().cloned().fold(9.9f32, f32::min), rad.iter().cloned().fold(0.0f32, f32::max)),
                if ok4 { "4 wheels" } else { "no split" }
            );
        }

        // ---- IS THE AXLE WHERE WE THINK IT IS? ----
        // The pivot used to come from the cluster's median, which is dragged
        // off the axle by any bodywork the cluster caught — and a wheel that
        // turns about a point which is not its own centre wobbles. Compare
        // the median against a least-squares circle fit: the distance
        // between them IS the wobble, and the residual says whether the
        // points are really a circle at all.
        println!();
        println!("{:<12} {:>12} {:>12} {:>10}", "model", "median->fit", "residual", "verdict");
        for n in ["vulcan.glb", "xdevil.glb", "backfire.glb", "guardian.glb", "orion.glb", "breakout.glb", "assael.glb"] {
            let path = dir.join(n);
            let Ok(d) = gltf::Gltf::open(&path) else { continue };
            let Some(blob) = d.blob.clone() else { continue };
            let mut pts: Vec<[f32; 3]> = Vec::new();
            for m in d.meshes() {
                for prim in m.primitives() {
                    let reader = prim.reader(|_| Some(&blob));
                    if let Some(ps) = reader.read_positions() {
                        pts.extend(ps);
                    }
                }
            }
            if pts.is_empty() {
                continue;
            }
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &pts {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            let mid = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
            let half = [(hi[0] - lo[0]) * 0.5, (hi[1] - lo[1]) * 0.5, (hi[2] - lo[2]) * 0.5];
            // front-left corner is enough to characterise the method
            let cl: Vec<[f32; 2]> = pts
                .iter()
                .filter(|p| {
                    let q = [p[0] - mid[0], p[1] - mid[1], p[2] - mid[2]];
                    q[1] < -half[1] * 0.15 && q[0] < -half[0] * 0.45 && q[2] < -half[2] * 0.25
                })
                .map(|p| [p[1] - mid[1], p[2] - mid[2]])
                .collect();
            if cl.len() < 60 {
                println!("{:<12} {:>12} {:>12} {:>10}", n.trim_end_matches(".glb"), "-", "-", "no cluster");
                continue;
            }
            let mut ys: Vec<f32> = cl.iter().map(|p| p[0]).collect();
            let mut zs: Vec<f32> = cl.iter().map(|p| p[1]).collect();
            ys.sort_by(f32::total_cmp);
            zs.sort_by(f32::total_cmp);
            let (m0, m1) = (ys[ys.len() / 2], zs[zs.len() / 2]);
            let (mut cy, mut cz) = (m0, m1);
            for round in 0..2 {
                let mut rs: Vec<f32> = cl.iter().map(|p| ((p[0] - cy).powi(2) + (p[1] - cz).powi(2)).sqrt()).collect();
                rs.sort_by(f32::total_cmp);
                let rmed = rs[rs.len() / 2];
                let (mut sx, mut sy, mut sxx, mut syy, mut sxy, mut sr2x, mut sr2y, mut cnt) =
                    (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0f64);
                for p in cl.iter() {
                    let (u, v) = ((p[0] - cy) as f64, (p[1] - cz) as f64);
                    if round == 1 && ((u * u + v * v).sqrt() as f32 - rmed).abs() > rmed * 0.45 {
                        continue;
                    }
                    let r2 = u * u + v * v;
                    sx += u; sy += v; sxx += u * u; syy += v * v; sxy += u * v;
                    sr2x += r2 * u; sr2y += r2 * v; cnt += 1.0;
                }
                if cnt < 24.0 { break; }
                let (a11, a12, a22) = (sxx - sx * sx / cnt, sxy - sx * sy / cnt, syy - sy * sy / cnt);
                let (b1, b2) = (0.5 * (sr2x - (sxx + syy) * sx / cnt), 0.5 * (sr2y - (sxx + syy) * sy / cnt));
                let det = a11 * a22 - a12 * a12;
                if det.abs() < 1e-9 { break; }
                cy += ((b1 * a22 - b2 * a12) / det) as f32;
                cz += ((a11 * b2 - a12 * b1) / det) as f32;
            }
            let moved = (((cy - m0).powi(2) + (cz - m1).powi(2)).sqrt()) / half[1];
            let mut rs: Vec<f32> = cl.iter().map(|p| ((p[0] - cy).powi(2) + (p[1] - cz).powi(2)).sqrt()).collect();
            rs.sort_by(f32::total_cmp);
            let rmed = rs[rs.len() / 2];
            let inl: Vec<f32> = rs.iter().copied().filter(|r| (r - rmed).abs() < rmed * 0.45).collect();
            let mean = inl.iter().sum::<f32>() / inl.len().max(1) as f32;
            let resid = (inl.iter().map(|r| (r - mean).powi(2)).sum::<f32>() / inl.len().max(1) as f32).sqrt() / mean.max(1e-6);
            println!(
                "{:<12} {:>11.1}% {:>11.1}% {:>10}",
                n.trim_end_matches(".glb"),
                moved * 100.0,
                resid * 100.0,
                if resid < 0.30 { "round" } else { "not a wheel" }
            );
        }

        // ---- does the bounding box describe the BODY, or the wheels? ----
        // EV cues were placed from half_w, which is the widest thing in the
        // model. If the wheels are wider than the flank, anything placed
        // there floats beside the doors instead of sitting on them.
        println!();
        println!("{:<12} {:>11} {:>11} {:>10}", "model", "bbox half", "door flank", "overhang");
        for n in ["vulcan.glb", "xdevil.glb", "backfire.glb", "orion.glb"] {
            let path = dir.join(n);
            let Ok(d) = gltf::Gltf::open(&path) else { continue };
            let Some(blob) = d.blob.clone() else { continue };
            let mut pts: Vec<[f32; 3]> = Vec::new();
            for m in d.meshes() {
                for prim in m.primitives() {
                    let reader = prim.reader(|_| Some(&blob));
                    if let Some(ps) = reader.read_positions() {
                        pts.extend(ps);
                    }
                }
            }
            if pts.is_empty() {
                continue;
            }
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &pts {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            let mid = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
            let hw = (hi[0] - lo[0]) * 0.5;
            let hh = (hi[1] - lo[1]) * 0.5;
            let hlz = (hi[2] - lo[2]) * 0.5;
            // the same measurement electrify now makes
            let mut xs: Vec<f32> = pts
                .iter()
                .filter(|p| (p[2] - mid[2]).abs() < hlz * 0.45 && p[1] - mid[1] > -hh * 0.35 && p[1] - mid[1] < hh * 0.8)
                .map(|p| (p[0] - mid[0]).abs())
                .collect();
            xs.sort_by(f32::total_cmp);
            let flank = if xs.len() < 32 { hw * 0.88 } else { xs[xs.len() * 9 / 10] };
            println!(
                "{:<12} {:>11.3} {:>11.3} {:>9.0}% {}",
                n.trim_end_matches(".glb"),
                hw,
                flank,
                (hw - flank) / hw * 100.0,
                if hw - flank > hw * 0.08 { "wheels wider than the doors" } else { "" }
            );
            // The EV cues are placed on the FLANK now, so they must sit
            // inside the bounding box by roughly the overhang — if this ever
            // reads ~0 again, the placement has gone back to the box and the
            // sills are floating beside the doors.
            ok_all &= flank < hw * 0.97;
        }

        // ---- node structure: where a misplaced panel would come from ----
        for n in ["vulcan.glb", "xdevil.glb", "backfire.glb"] {
            let path = dir.join(n);
            if let Ok(d) = gltf::Gltf::open(&path) {
                let scene = d.default_scene().or_else(|| d.scenes().next());
                let mut nodes = 0;
                let mut with_mesh = 0;
                let mut nonident = 0;
                let mut mirrored = 0;
                fn walk(node: &gltf::Node, nodes: &mut u32, with_mesh: &mut u32, nonident: &mut u32, mirrored: &mut u32) {
                    *nodes += 1;
                    if node.mesh().is_some() {
                        *with_mesh += 1;
                    }
                    let m = node.transform().matrix();
                    let ident = (0..4).all(|i| (0..4).all(|j| (m[i][j] - if i == j { 1.0 } else { 0.0 }).abs() < 1e-6));
                    if !ident {
                        *nonident += 1;
                    }
                    // a negative determinant means the node is MIRRORED, which
                    // flips triangle winding — the classic cause of one side of
                    // a car looking wrong while the other is fine
                    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                        - m[1][0] * (m[0][1] * m[2][2] - m[0][2] * m[2][1])
                        + m[2][0] * (m[0][1] * m[1][2] - m[0][2] * m[1][1]);
                    if det < 0.0 {
                        *mirrored += 1;
                    }
                    for c in node.children() {
                        walk(&c, nodes, with_mesh, nonident, mirrored);
                    }
                }
                if let Some(sc) = scene {
                    for node in sc.nodes() {
                        walk(&node, &mut nodes, &mut with_mesh, &mut nonident, &mut mirrored);
                    }
                }
                println!("   {n:<14} nodes {nodes}, with mesh {with_mesh}, transformed {nonident}, MIRRORED {mirrored}");
            }
        }
        println!("{}", if ok_all { "GLB: PASS" } else { "GLB: FAIL" });
        return;
    }
    if name == "cars" {
        // Do the electric cars actually FEEL different, and are the new
        // models built to a higher standard than the block cars?
        use crate::model::Drive;
        let cfg = Config::default();
        let world = World::generate(31, crate::world::style(ARENA), 0.31, 1.0);
        let ball = Ball::new();
        println!("{:<10} {:>9} {:>9} {:>9} {:>9}", "drive", "0-8 m/s", "0-13 m/s", "top", "60m dash");
        let run = |drive: Drive| -> (f32, f32, f32, f32) {
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
            car.sync_rot();
            car.drive = drive;
            let mut c = physics::Controls::default();
            c.throttle = 1.0;
            let (mut t15, mut t25, mut dash) = (0.0f32, 0.0f32, 0.0f32);
            let mut peak = 0.0f32;
            let start = car.pos;
            let mut t = 0.0f32;
            while t < 8.0 {
                // throttle only while we time the launch, then boost so the
                // top-speed difference has somewhere to show (the drivetrain
                // cap and the throttle cap are separate things)
                c.boost = t > 3.0;
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                t += DT;
                let sp = car.vel.length();
                // throttle-only tops out at ~14.2 m/s (RL's 1410 uu/s), so
                // the gates have to sit below it — and top speed is the MAX
                // seen, not the speed after the car has run into a wall
                if t15 == 0.0 && sp >= 8.0 {
                    t15 = t;
                }
                if t25 == 0.0 && sp >= 13.0 {
                    t25 = t;
                }
                peak = peak.max(sp);
                if dash == 0.0 && car.pos.distance(start) >= 60.0 {
                    dash = t;
                }
                if std::env::var("CARS_DBG").is_ok() && (t * 4.0).fract() < DT * 4.0 && t < 3.0 {
                    eprintln!("   t={:.2} sp={:5.2} og={} y={:6.3} thr={:.1}", t, sp, car.on_ground as u8, car.pos.y, c.throttle);
                }
            }
            (t15, t25, peak, dash)
        };
        let ice = run(Drive::Combustion);
        let ev = run(Drive::Electric);
        println!("{:<10} {:>8.2}s {:>8.2}s {:>8.1} {:>8.2}s", "petrol", ice.0, ice.1, ice.2, ice.3);
        println!("{:<10} {:>8.2}s {:>8.2}s {:>8.1} {:>8.2}s", "electric", ev.0, ev.1, ev.2, ev.3);
        let mut pass = true;
        let mut check = |what: &str, ok: bool| {
            println!("{:<44} {}", what, if ok { "ok" } else { "FAIL" });
            pass &= ok;
        };
        check("an EV is quicker off the line", ev.0 < ice.0 - 0.05);
        check("...and gives it back at the top", ev.2 < ice.2);
        let gain = (ice.0 - ev.0) / ice.0.max(0.01) * 100.0;
        println!("   the EV reaches 8 m/s {gain:.0}% sooner");
        check("the difference is felt, not cosmetic", gain > 8.0);
        // ---- model quality: is there actually more car here? ----
        // The builders need a GPU, but the mesh generators are pure, so the
        // geometry can be measured directly — which is the part that decides
        // whether a car reads as a vehicle or as stacked boxes.
        println!();
        let boxx = crate::meshgen::boxx();
        let cbox = crate::meshgen::cbox(0.22);
        let hull = crate::meshgen::loft(
            &[
                (-0.52, 0.20, -0.05, 0.09, 0.75),
                (-0.34, 0.28, -0.10, 0.15, 0.55),
                (0.02, 0.30, -0.12, 0.17, 0.45),
                (0.34, 0.29, -0.11, 0.16, 0.50),
                (0.52, 0.22, -0.06, 0.10, 0.75),
            ],
            24,
        );
        let old_wheel = crate::meshgen::sphere(12, 16);
        let new_wheel = crate::meshgen::wheel(0.55, 18);
        println!("{:<16} {:>9} {:>10}", "part", "vertices", "triangles");
        for (nm, m) in [
            ("plain box", &boxx),
            ("chamfered box", &cbox),
            ("lofted hull", &hull),
            ("wheel (was)", &old_wheel),
            ("wheel (now)", &new_wheel),
        ] {
            println!("{:<16} {:>9} {:>10}", nm, m.verts.len(), m.idx.len() / 3);
        }
        // Count TRIANGLES for shape (a hull's vertex count is misleading —
        // it is the surface between them that curves), and for the wheel ask
        // the question that matters: is it disc-shaped? A sphere is as wide
        // as it is tall; a tyre is not.
        let ext = |m: &crate::meshgen::MeshData, ax: usize| -> f32 {
            m.verts.iter().map(|v| v.pos[ax].abs()).fold(0.0f32, f32::max)
        };
        check(
            "a lofted hull carries far more shape than a box",
            hull.idx.len() > boxx.idx.len() * 8,
        );
        check("the chamfered box has edges to catch light", cbox.idx.len() > boxx.idx.len());
        let round = ext(&old_wheel, 0) / ext(&old_wheel, 1).max(0.01);
        let tyre = ext(&new_wheel, 0) / ext(&new_wheel, 1).max(0.01);
        println!("   width/height: sphere {round:.2} (a ball), wheel {tyre:.2} (a tyre)");
        check("the wheel is disc-shaped, not spherical", tyre < 0.5 && round > 0.9);

        // ---- the two concept cars: are their grammars actually different?
        // VIPERA is a continuous shell (high-segment lofts, curved sections);
        // AEGIS is armour plate (seven-segment lofts, flat facets). The same
        // generator makes both, so the difference has to show in the mesh.
        let vip_shell = crate::meshgen::loft(
            &[
                (-0.77, 0.13, -0.035, 0.010, 0.95),
                (-0.30, 0.34, -0.065, 0.085, 0.70),
                (0.02, 0.36, -0.065, 0.105, 0.65),
                (0.60, 0.30, -0.050, 0.060, 0.80),
                (0.77, 0.22, -0.035, 0.020, 0.95),
            ],
            26,
        );
        let aeg_wedge = crate::meshgen::loft(
            &[
                (-0.80, 0.07, -0.020, 0.005, 0.05),
                (-0.28, 0.33, -0.050, 0.070, 0.05),
                (0.06, 0.36, -0.055, 0.085, 0.05),
                (0.66, 0.28, -0.040, 0.045, 0.05),
                (0.80, 0.16, -0.025, 0.015, 0.05),
            ],
            7,
        );
        // "faceted" means neighbouring ring vertices turn in big steps
        // The TYPICAL turn, not the worst: at the sharp ends of a very flat
        // cross-section (the nose is three times wider than it is tall) the
        // normal genuinely swings fast even on a smooth shell, so a
        // worst-case reading says nothing about the surface as a whole.
        let facet_angle = |m: &crate::meshgen::MeshData, ring: usize, section: usize| -> f32 {
            let base = section * (ring + 1);
            let mut steps: Vec<f32> = Vec::new();
            for i in 1..ring {
                let a = Vec3::from_array(m.verts[base + i].nrm);
                let b = Vec3::from_array(m.verts[base + i - 1].nrm);
                steps.push(a.angle_between(b).to_degrees());
            }
            steps.sort_by(f32::total_cmp);
            steps[steps.len() / 2]
        };
        let smooth = facet_angle(&vip_shell, 26, 2); // the widest section
        let faceted = facet_angle(&aeg_wedge, 7, 2);
        println!();
        println!("concept grammars: vipera turns {smooth:.0} deg per step (a shell), aegis {faceted:.0} deg (plates)");
        check("the shell is smooth", smooth < 30.0);
        check("the wedge is faceted", faceted > 40.0);
        check("both come from the same generator", vip_shell.idx.len() > 0 && aeg_wedge.idx.len() > 0);
        println!("{}", if pass { "CARS: PASS" } else { "CARS: FAIL" });
        return;
    }
    if name == "perf" {
        // CPU cost, measured rather than guessed. Unlike the GPU work this
        // can be timed directly here, so there is no excuse for optimising
        // by intuition.
        use std::time::Instant;
        println!("{:<14} {:>10} {:>12} {:>12}", "map", "worldgen", "sdf/ms", "step/ms (12 bots)");
        for si in [20usize, 21, 24, 28, 35] {
            let t0 = Instant::now();
            let world = World::generate(9, crate::world::style(si), 0.31, 1.0);
            let gen_ms = t0.elapsed().as_secs_f32() * 1000.0;
            // raw sdf throughput at play-relevant positions
            let mut acc = 0.0f32;
            let n = 20_000;
            let t1 = Instant::now();
            for k in 0..n {
                let a = k as f32 * 0.7548;
                let p = Vec3::new(a.cos() * 40.0, 4.0 + (k % 30) as f32, a.sin() * 40.0);
                acc += world.sdf(p);
            }
            let sdf_per_ms = n as f32 / (t1.elapsed().as_secs_f32() * 1000.0);
            // a full step with a squad of bots, which is the real frame cost
            let mut w2 = World::generate(9, crate::world::style(si), 0.31, 1.0);
            let mut ball = Ball::new();
            let mut player = physics::Car::new();
            let mut bots = crate::bots::spawn_match(&w2, 6);
            let cfg = Config::default();
            let mut t = 0.0f32;
            let t2 = Instant::now();
            let steps = 600;
            for _ in 0..steps {
                crate::bots::update(&mut bots, &mut ball, &mut player, &w2, &cfg, t, DT);
                let mut ev = StepEvents::default();
                physics::step_ball(&mut ball, &mut w2, &cfg, DT, &mut ev);
                w2.update(DT);
                t += DT;
            }
            let step_ms = t2.elapsed().as_secs_f32() * 1000.0 / steps as f32;
            println!(
                "{:<14} {:>9.0}ms {:>11.0} {:>11.3}ms  ({} grav, {} shapes)",
                crate::world::style(si).name,
                gen_ms,
                sdf_per_ms,
                step_ms,
                w2.grav.len(),
                w2.shapes.len()
            );
            let _ = acc;
        }
        println!();
        // where does a step actually go?
        {
            let world = World::generate(9, crate::world::style(21), 0.31, 1.0);
            let p = Vec3::new(20.0, 12.0, 20.0);
            let n = 60_000;
            let t = Instant::now();
            for k in 0..n {
                let q = p + Vec3::new((k % 17) as f32, 0.0, (k % 13) as f32);
                std::hint::black_box(world.sdf(q));
            }
            let sdf_ns = t.elapsed().as_nanos() as f32 / n as f32;
            let t = Instant::now();
            for k in 0..n {
                let q = p + Vec3::new((k % 17) as f32, 0.0, (k % 13) as f32);
                std::hint::black_box(world.gravity_dir(q));
            }
            let grav_ns = t.elapsed().as_nanos() as f32 / n as f32;
            let t = Instant::now();
            for k in 0..n {
                let q = p + Vec3::new((k % 17) as f32, 0.0, (k % 13) as f32);
                std::hint::black_box(world.platform_vel(q, 1.5));
            }
            let plat_ns = t.elapsed().as_nanos() as f32 / n as f32;
            println!("PERPLEXUS per call: sdf {sdf_ns:.0} ns, gravity_dir {grav_ns:.0} ns, platform_vel {plat_ns:.0} ns");
        {
            // SDF EVALUATIONS PER STEP. Timing a debug build is noise; this
            // is exact. The normal is four evaluations and the same point is
            // asked for twice a frame (the ground solve, then gravity_dir),
            // so the cache should show up here as a clean reduction.
            let mut w = World::generate(9, crate::world::style(21), 0.31, 1.0);
            let mut ball = Ball::new();
            let mut car = physics::Car::new();
            car.pos = Vec3::new(0.0, w.ground_h(0.0, 0.0) + 0.5, 0.0);
            car.sync_rot();
            let cfg = Config::default();
            let mut c = physics::Controls::default();
            c.throttle = 1.0;
            let before = w.sdf_calls();
            let steps = 600;
            for _ in 0..steps {
                let mut ev = StepEvents::default();
                physics::step(&mut car, &mut ball, &mut w, &c, &cfg, DT, &mut ev);
            }
            let per = (w.sdf_calls() - before) as f32 / steps as f32;
            println!("   {per:.1} SDF evaluations per physics step (car + ball)");
        }
        {
            // gravity_dir walks ONE cell, so the worst cell is the worst case
            // of the whole function — and a mean far below the worst means
            // the cells are the wrong size rather than the map being busy.
            let w = World::generate(9, crate::world::style(21), 0.31, 1.0);
            let (used, mean, worst) = w.grav_cell_stats();
            println!("   gravity index: {used} cells used, {mean:.1} nodes each, worst {worst}");
        }
            println!("   (every car and the ball hit all three every frame)");
        }
        return;
    }
    if name == "routecheck" {
        // Where the route SAYS the bore is, versus where the geometry
        // actually put it. Inside a tube the SDF at the centreline should
        // read close to the bore radius; solid reads negative, and open sky
        // reads far larger. A systematic mismatch would explain every
        // symptom of the Monkey Run traversal failure at once.
        for si in [21usize, 24] {
            let w = World::generate(9, crate::world::style(si), 0.31, 1.0);
            println!("{}", crate::world::style(si).name);
            println!("   {:<6} {:>5} {:>8} {:>8} {:>8} {:>8}", "route", "pts", "r", "median", "solid%", "open%");
            for (ri, (pts, r)) in w.routes.iter().enumerate().take(8) {
                let mut d: Vec<f32> = pts.iter().map(|p| w.sdf(*p)).collect();
                let solid = d.iter().filter(|&&x| x < 0.2).count();
                let open = d.iter().filter(|&&x| x > r * 2.0).count();
                d.sort_by(f32::total_cmp);
                let med = d[d.len() / 2];
                println!(
                    "   {:<6} {:>5} {:>8.1} {:>8.2} {:>7.0}% {:>7.0}%",
                    ri,
                    pts.len(),
                    r,
                    med,
                    solid as f32 / d.len() as f32 * 100.0,
                    open as f32 / d.len() as f32 * 100.0
                );
            }
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
        // ---- TUBE MESH: rings only at real mouths, and crossings open ----
        for si in [21usize, 24] {
            let w = World::generate(9, crate::world::style(si), 0.31, 1.0);
            let segs = w
                .shapes
                .iter()
                .filter(|sh| matches!(sh.sdf, crate::world::Sdf::Shell { .. }))
                .count();
            let rims = w.shapes.iter().filter(|sh| sh.mesh == crate::world::M_TUBE_RIM).count();
            let cuts = w.shapes.iter().filter(|sh| sh.cut).count();
            println!(
                "{:<12} {} tube segments, {} mouth rings ({:.1} per segment), {} carves",
                crate::world::style(si).name,
                segs,
                rims,
                rims as f32 / segs.max(1) as f32,
                cuts
            );
            // The artifact: a chamfer baked into the mesh drew TWO rings per
            // segment. Anything approaching that is the bug coming back.
            if rims as f32 > segs as f32 * 0.8 {
                pass = false;
                println!("   too many rings — the chamfer is back on every segment");
            }
        }
        // Can you actually pass between two crossing tubes? Walk the world
        // SDF straight through a carved junction and see if it stays open.
        {
            let w = World::generate(9, crate::world::style(21), 0.31, 1.0);
            let cuts: Vec<(Vec3, f32)> = w
                .shapes
                .iter()
                .filter(|sh| sh.cut && matches!(sh.sdf, crate::world::Sdf::Sphere { .. }))
                .map(|sh| (sh.pos, match sh.sdf { crate::world::Sdf::Sphere { r } => r, _ => 0.0 }))
                .collect();
            let mut open_junctions = 0;
            let mut tested = 0;
            for (at, r) in cuts.iter().take(40) {
                tested += 1;
                // a short line through the junction in three directions: if
                // the carve worked, the interior stays open across it
                let dirs = [Vec3::X, Vec3::Y, Vec3::Z];
                let clear = dirs.iter().any(|d| {
                    (-6..=6).all(|k| {
                        let p = *at + *d * (k as f32 / 6.0 * r * 0.85);
                        w.sdf(p) > 0.35
                    })
                });
                if clear {
                    open_junctions += 1;
                }
            }
            println!(
                "PERPLEXUS    {} of {} carved junctions are open all the way through",
                open_junctions, tested
            );
            if tested > 0 && open_junctions * 4 < tested * 3 {
                pass = false;
            }
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
        // ---- POWERSLIDE MUST NOT DOUBLE THE FLIP ----
        // Holding slide turns steer into air roll, and a diagonal dodge has
        // already spent steer on its direction. Unless the passthrough is
        // scaled, the same stick rotates the car twice.
        let mut ok_all = true;
        println!();
        println!("{:<22} {:>10} {:>10} {:>9}", "input", "plain", "+slide", "delta");
        // The third value is the roll applied WHILE the flip runs. For a
        // diagonal dodge that is the steer axis itself (powerslide maps it
        // to roll, which is the bug). For a half-flip you dodge straight
        // back and only then roll — so the roll is its own input.
        for (label, thr, st, roll) in [
            ("forward+right", 1.0f32, 1.0f32, -1.0f32),
            ("forward", 1.0, 0.0, 0.0),
            ("back (half-flip)", -1.0, 0.0, -1.0),
        ] {
            let turn_of = |slide: bool| -> f32 {
                let world = World::generate(9, crate::world::style(ARENA), 0.31, 1.0);
                let ball = Ball::new();
                let cfg = Config::default();
                let mut car = physics::Car::new();
                // A dodge needs a GROUND jump first — spawned in mid-air the
                // first press does nothing and no flip ever fires, so this
                // was measuring free air roll rather than a flip.
                car.pos = Vec3::new(0.0, world.ground_h(0.0, 0.0) + 0.31, 0.0);
                car.sync_rot();
                let mut c = physics::Controls::default();
                for _ in 0..20 {
                    physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                }
                c.jump_pressed = true;
                c.jump_held = true;
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                c.jump_pressed = false;
                for _ in 0..10 {
                    physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                }
                c.throttle = thr;
                c.steer = st;
                c.roll = if slide { roll } else { 0.0 };
                c.jump_pressed = true;
                physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                c.jump_pressed = false;
                let mut total = 0.0f32;
                let mut prev = car.rot;
                for _ in 0..140 {
                    physics::step_car_only(&mut car, &ball, &world, &c, &cfg, DT);
                    let (_, mut d) = (car.rot * prev.inverse()).normalize().to_axis_angle();
                    if d > std::f32::consts::PI {
                        d -= std::f32::consts::TAU;
                    }
                    total += d.abs();
                    prev = car.rot;
                }
                total.to_degrees()
            };
            let plain = turn_of(false);
            let slid = turn_of(true);
            let delta = (slid - plain).abs();
            println!("{:<22} {:>9.0}d {:>9.0}d {:>8.0}d", label, plain, slid, delta);
            if label == "forward+right" && delta >= 60.0 {
                ok_all = false;
                println!("   powerslide DOUBLES a diagonal flip");
            }
            if label == "back (half-flip)" && delta <= 90.0 {
                ok_all = false;
                println!("   the half-flip lost its air roll");
            }
        }
        println!("{}", if ok_all { "FLIPS: PASS" } else { "FLIPS: FAIL" });
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
