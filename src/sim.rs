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
    for (assist, h, label) in [(1.0f32, 5.0f32, "assist=default drop=5m"), (0.0, 5.0, "assist=OFF drop=5m"), (0.0, 2.2, "assist=OFF drop=2.2m (post-flip case)")] {
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
        println!("{}", if ok1 && ok2 { "MECHANICS: PASS" } else { "MECHANICS: FAIL" });
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
