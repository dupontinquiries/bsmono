//! Offline AI. One brain, tuned for comedy: BALLCHASE. Every bot wants the
//! ball, right now, with boost. Bots run the exact same car physics as the
//! player (physics::step_car_only) at frame rate, get unlimited boost (the
//! classic bot cheat), flip into the ball when close, and shove each other
//! apart through a little uniform-grid so a 300-spartan pile stays a pile
//! instead of a singularity.

use std::collections::HashMap;

use glam::{Quat, Vec3};

use crate::config::Config;

/// Immutable teammate snapshot handed to every brain each frame.
pub struct Mate {
    pub pos: Vec3,
    pub vel: Vec3,
    pub team: u8,
    pub idx: u8,
}

/// Per-bot brain context: who else is on the pitch, whether THIS bot is its
/// team's designated attacker (nearest to the ball), and live gravity for
/// ballistic prediction.
pub struct Ctx<'a> {
    pub mates: &'a [Mate],
    pub attacker: bool,
    pub grav: f32,
}

/// Anticipatory jump: leave the ground ~a third of a second BEFORE a
/// descending ball arrives in reach, so the touch happens in the air.
fn prejump(c: &mut Controls, car: &Car, jump_cd: &mut f32, ball: &crate::physics::Ball, grav: f32) {
    if !car.on_ground || *jump_cd > 0.0 || ball.vel.y > 0.6 {
        return;
    }
    for t in [0.30f32, 0.48] {
        let bp = ball.pos + ball.vel * t - Vec3::Y * (0.5 * grav * t * t);
        let flat = Vec3::new(bp.x - car.pos.x, 0.0, bp.z - car.pos.z).length();
        let dy = bp.y - car.pos.y;
        if flat < 2.8 && (1.0..3.6).contains(&dy) {
            c.jump_pressed = true;
            c.jump_held = true;
            *jump_cd = 1.1;
            return;
        }
    }
}
use crate::physics::{self, Ball, Car, Controls};
use crate::world::World;

pub const TEAM_BLUE: u8 = 0;
pub const TEAM_ORANGE: u8 = 1;
pub const TEAM_BRONZE: u8 = 2;

pub fn team_tint(team: u8) -> [f32; 3] {
    match team {
        TEAM_BLUE => [0.30, 0.50, 1.05],
        TEAM_ORANGE => [1.05, 0.48, 0.16],
        _ => [0.82, 0.55, 0.30], // bronze, for the three hundred
    }
}

/// What a bot is trying to do with its life.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Brain {
    /// Chase the ball. Boost at the ball. Flip at the ball. (The mob.)
    Ballchase,
    /// The tutorial-grounded technician: ground dribble -> pop -> nose-tip
    /// air carry with feathered boost -> flip reset -> finishing flick.
    Striker,
    /// Sits between the ball and the net it defends; clears when threatened.
    Defender { home: Vec3 },
    /// A striker that hands the ball to a teammate on a timer (pass drills).
    Passer { next: u8 },
}

/// Striker state machine phases (tutorial stages, one enum variant each).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Approach,  // get behind the ball, lined up with the goal
    Carry,     // ground dribble: ball on the hood, match speed
    Pop,       // jump under the ball the moment it settles -> launch
    AirCarry,  // nose-tip carry: under-behind, feather boost, tiny touches
    Reset,     // cradle under the ball, wheels to its belly, no boost
    Flick,     // spend the (re-gained) flip into the ball
}

pub struct Bot {
    pub car: Car,
    pub team: u8,
    pub model: usize,
    pub brain: Brain,
    pub phase: Phase,
    pub got_reset: bool,   // style bookkeeping: at least one flip reset
    pub touches_air: u32,  // touches made while airborne (style metric)
    phase_t: f32,
    jump_cd: f32,
    jitter: f32, // per-bot phase so the mob swirls instead of stacking
    shot_t: f32, // power-shot micro-sequence timer (hop, then dodge through)
    tube_iq: f32, // 0..1: odds per window of pursuing prey INTO tube routes
    /// GUIDED LEARNING: a tiny (1+1) hill-climb over four aerial knobs,
    /// scored on what we actually want to see (air touches and engagement)
    /// and re-evaluated every episode, so a bot gets better DURING a match.
    lrn: Learner,
    /// coordinated aerial: this bot has been elected to finish a mate's pop
    finisher: bool,
    air_hits: u32, // snapshot for reward accounting
}

#[derive(Clone, Copy)]
pub struct Learner {
    /// [0] intercept lead, [1] jump distance, [2] boost commit, [3] aim lift
    pub p: [f32; 4],
    best: [f32; 4],
    best_r: f32,
    r: f32,
    t: f32,
    sigma: f32,
    pub episodes: u32,
    enabled: bool,
}

impl Learner {
    fn new(seed: u32) -> Learner {
        let h = |k: u32| ((seed.wrapping_mul(2654435761).wrapping_add(k * 40503) >> 8) & 1023) as f32 / 1023.0;
        let p = [
            0.55 + h(1) * 0.5,
            2.2 + h(2) * 1.6,
            0.45 + h(3) * 0.4,
            0.8 + h(4) * 0.9,
        ];
        Learner { p, best: p, best_r: -1.0, r: 0.0, t: 0.0, sigma: 0.22, episodes: 0, enabled: true }
    }
    /// One episode of experience: keep what scored, else fall back and try
    /// a new perturbation around the best-known setting.
    fn tick(&mut self, dt: f32, reward: f32) {
        if !self.enabled {
            return;
        }
        self.r += reward;
        self.t += dt;
        if self.t < 9.0 {
            return;
        }
        if self.r > self.best_r {
            self.best_r = self.r;
            self.best = self.p;
        } else {
            // decay the memory a little so a lucky episode cannot freeze the
            // search forever — the match keeps teaching
            self.best_r *= 0.94;
        }
        let seed = (self.episodes as u32).wrapping_mul(2246822519) ^ (self.t.to_bits());
        let rnd = |k: u32| {
            let h = (seed ^ k.wrapping_mul(2654435761)).wrapping_mul(747796405);
            (((h >> 16) & 2047) as f32 / 2047.0) * 2.0 - 1.0
        };
        for i in 0..4 {
            self.p[i] = self.best[i] + rnd(i as u32) * self.sigma * self.best[i].abs().max(0.2);
        }
        self.p[0] = self.p[0].clamp(0.25, 1.5);
        self.p[1] = self.p[1].clamp(1.2, 5.0);
        self.p[2] = self.p[2].clamp(0.15, 1.0);
        self.p[3] = self.p[3].clamp(0.2, 2.2);
        self.sigma = (self.sigma * 0.97).max(0.06);
        self.episodes += 1;
        self.r = 0.0;
        self.t = 0.0;
    }
}

pub fn mk_bot_brain(pos: Vec3, yaw: f32, team: u8, i: usize, brain: Brain) -> Bot {
    let mut car = Car::new();
    car.pos = pos;
    car.rot = Quat::from_rotation_y(yaw);
    car.boost = 100.0;
    Bot {
        car,
        team,
        model: i % 3,
        brain,
        phase: Phase::Approach,
        got_reset: false,
        touches_air: 0,
        phase_t: 0.0,
        jump_cd: 0.4 + (i % 7) as f32 * 0.13,
        jitter: i as f32 * 2.399, // golden-angle-ish spread
        shot_t: 0.0,
        tube_iq: 0.35 + ((i.wrapping_mul(2654435761) >> 8) & 255) as f32 / 255.0 * 0.6,
        lrn: Learner::new(i as u32 + 7),
        finisher: false,
        air_hits: 0,
    }
}

fn mk_bot(pos: Vec3, yaw: f32, team: u8, i: usize) -> Bot {
    mk_bot_brain(pos, yaw, team, i, Brain::Ballchase)
}

/// Two phalanxes of 50, blue vs orange, facing each other across the plaza.
pub fn spawn_sparta_50v50(world: &World) -> Vec<Bot> {
    let mut v = Vec::with_capacity(100);
    for (team, side) in [(TEAM_BLUE, -1.0f32), (TEAM_ORANGE, 1.0)] {
        for i in 0..50usize {
            let (row, col) = (i / 10, i % 10);
            let x = (col as f32 - 4.5) * 2.4;
            let z = side * (26.0 + row as f32 * 2.6);
            let y = world.ground_h(x, z) + 0.4;
            v.push(mk_bot(Vec3::new(x, y, z), if side < 0.0 { 0.0 } else { std::f32::consts::PI }, team, i));
        }
    }
    v
}

/// Three hundred, one bronze army, one ball, no goals. THIS. IS. SKYDRIBBLE.
pub fn spawn_sparta_300(world: &World) -> Vec<Bot> {
    let mut v = Vec::with_capacity(300);
    for i in 0..300usize {
        let (row, col) = (i / 20, i % 20);
        let x = (col as f32 - 9.5) * 2.4;
        let z = 24.0 + row as f32 * 2.6;
        let y = world.ground_h(x, z) + 0.4;
        v.push(mk_bot(Vec3::new(x, y, z), std::f32::consts::PI, TEAM_BRONZE, i));
    }
    v
}

/// One frame of every bot: brain -> physics -> ball touch -> crowd shoving.
pub fn update(
    bots: &mut [Bot],
    ball: &mut Ball,
    player: &mut Car,
    world: &World,
    cfg: &Config,
    time: f32,
    dt: f32,
) {
    let dt = dt.min(1.0 / 30.0);
    // snapshot + roles: one designated attacker per team (nearest to ball),
    // everyone else spaces out into support lanes instead of dogpiling
    let mates: Vec<Mate> = bots
        .iter()
        .enumerate()
        .map(|(i, b)| Mate { pos: b.car.pos, vel: b.car.vel, team: b.team, idx: i as u8 })
        .collect();
    let mut nearest: std::collections::HashMap<u8, (f32, usize)> = std::collections::HashMap::new();
    for (i, b) in bots.iter().enumerate() {
        let d = b.car.pos.distance_squared(ball.pos);
        let e = nearest.entry(b.team).or_insert((d, i));
        if d < e.0 {
            *e = (d, i);
        }
    }
    let grav = physics::GRAV * cfg.arena.gravity;

    // COORDINATED AERIAL. When the ball is genuinely up and travelling, the
    // attacker who popped it is busy underneath — so the best-placed MATE is
    // elected to meet it in the air. One finisher per team, re-elected each
    // frame from whoever can actually reach the intercept.
    let ball_up = ball.pos.y > 3.2 && ball.vel.y > -9.0;
    let mut finisher_of: std::collections::HashMap<u8, usize> = std::collections::HashMap::new();
    if ball_up {
        for (i, b) in bots.iter().enumerate() {
            if nearest.get(&b.team).map(|e| e.1) == Some(i) {
                continue; // the attacker is the setter, not the finisher
            }
            if !matches!(b.brain, Brain::Striker | Brain::Passer { .. }) {
                continue;
            }
            // where the ball will be at this bot's chosen lead time
            let lead = b.lrn.p[0];
            let meet = ball.pos + ball.vel * lead - Vec3::Y * (0.5 * grav * lead * lead);
            let cost = b.car.pos.distance(meet) - b.car.vel.dot((meet - b.car.pos).normalize_or(Vec3::Z)) * 0.35;
            let e = finisher_of.entry(b.team).or_insert(i);
            let cur = &bots[*e];
            let cur_lead = cur.lrn.p[0];
            let cur_meet = ball.pos + ball.vel * cur_lead - Vec3::Y * (0.5 * grav * cur_lead * cur_lead);
            let cur_cost = cur.car.pos.distance(cur_meet)
                - cur.car.vel.dot((cur_meet - cur.car.pos).normalize_or(Vec3::Z)) * 0.35;
            if cost < cur_cost {
                *e = i;
            }
        }
    }

    let mut scratch = physics::StepEvents::default();
    for (bi, b) in bots.iter_mut().enumerate() {
        b.jump_cd = (b.jump_cd - dt).max(0.0);
        b.phase_t += dt;
        b.car.boost = 100.0 * cfg.car.boost_max;
        let ctx = Ctx {
            mates: &mates,
            attacker: nearest.get(&b.team).map(|e| e.1) == Some(bi),
            grav,
        };
        let c = match b.brain {
            Brain::Ballchase => ballchase(b, ball, time, &ctx),
            Brain::Striker => striker(b, ball, world, &ctx, dt),
            Brain::Defender { home } => defender(b, ball, home, time),
            Brain::Passer { next: _ } => striker(b, ball, world, &ctx, dt),
        };
        let mut c = c;
        b.finisher = finisher_of.get(&b.team) == Some(&bi);
        if b.finisher {
            air_finish(b, ball, world, grav, &mut c);
        }
        route_override(world, b, ball, time, &mut c);
        power_shot(world, b, ball, dt, ctx.attacker, &mut c);
        let info = physics::step_car_only(&mut b.car, ball, world, &c, cfg, dt);
        // REWARD: exactly the behaviour we want more of — touches made in
        // the air, and time spent engaged with a ball that is up.
        let gained = b.touches_air.saturating_sub(b.air_hits);
        b.air_hits = b.touches_air;
        let near = b.car.pos.distance(ball.pos);
        let engaged = if ball.pos.y > 2.5 && near < 12.0 { dt * 0.6 } else { 0.0 };
        b.lrn.tick(dt, gained as f32 * 3.0 + engaged);
        if info.flip_reset {
            b.got_reset = true;
            if b.phase == Phase::Reset {
                b.phase = Phase::AirCarry;
                b.phase_t = 0.0;
            }
        }
        if info.landed && matches!(b.brain, Brain::Striker | Brain::Passer { .. }) {
            // touched down: if the ball is still up and close, re-jump into
            // the carry (bounce-dribble rhythm) instead of restarting setup
            let d = (ball.pos - b.car.pos).length();
            b.phase = if ball.pos.y > 1.6 && d < 7.0 { Phase::Pop } else { Phase::Approach };
            b.phase_t = 0.0;
        }
        let pre = scratch.touches.len();
        physics::collide_car_ball(&mut b.car, ball, cfg, b.team, &mut scratch);
        if scratch.touches.len() > pre && !b.car.on_ground {
            b.touches_air += 1;
        }
    }

    // ---- crowd separation: uniform grid, sphere pushes ----
    const CELL: f32 = 3.2;
    let key = |p: Vec3| ((p.x / CELL).floor() as i32, (p.z / CELL).floor() as i32);
    let mut grid: HashMap<(i32, i32), Vec<u16>> = HashMap::with_capacity(bots.len() * 2);
    for (i, b) in bots.iter().enumerate() {
        grid.entry(key(b.car.pos)).or_default().push(i as u16);
    }
    let r = 0.85;
    let mut pushes: Vec<Vec3> = vec![Vec3::ZERO; bots.len()];
    let mut player_push = Vec3::ZERO;
    let pk = key(player.pos);
    for (&(cx, cz), cell) in grid.iter() {
        for dz in -1..=1 {
            for dx in -1..=1 {
                let Some(other) = grid.get(&(cx + dx, cz + dz)) else { continue };
                for &ia in cell {
                    for &ib in other {
                        if ib <= ia && dx == 0 && dz == 0 {
                            continue;
                        }
                        if (dx != 0 || dz != 0) && ib < ia {
                            continue; // each unordered pair once
                        }
                        if ia == ib {
                            continue;
                        }
                        let d = bots[ib as usize].car.pos - bots[ia as usize].car.pos;
                        let l = d.length();
                        if l < 2.0 * r && l > 1e-4 {
                            let p = d / l * (2.0 * r - l) * 0.5;
                            pushes[ia as usize] -= p;
                            pushes[ib as usize] += p;
                        }
                    }
                }
                // player vs this cell
                if (cx + dx - pk.0).abs() <= 1 && (cz + dz - pk.1).abs() <= 1 {
                    for &ia in cell {
                        let d = player.pos - bots[ia as usize].car.pos;
                        let l = d.length();
                        if l < 2.0 * r && l > 1e-4 {
                            let p = d / l * (2.0 * r - l) * 0.5;
                            pushes[ia as usize] -= p;
                            player_push += p * 0.6;
                        }
                    }
                }
            }
        }
    }
    for (b, p) in bots.iter_mut().zip(pushes) {
        b.car.pos += p;
        b.car.vel += p * 6.0;
    }
    player.pos += player_push;
    player.vel += player_push * 4.0;
}

/// Aim helper: pitch/yaw commands that turn the nose toward a world dir.
fn aim(c: &mut Controls, car: &Car, dir: Vec3) {
    let local = car.rot.inverse() * dir.normalize_or(Vec3::NEG_Z);
    // body frame: -z is the nose; +y up; +x right
    c.pitch = (local.y * 2.6).clamp(-1.0, 1.0);
    c.yaw = (local.x * 2.6).clamp(-1.0, 1.0);
}

/// Ground-steer toward a flat target point. Returns (dist, |angle|).
fn steer_to(c: &mut Controls, car: &Car, target: Vec3) -> (f32, f32) {
    let to = target - car.pos;
    let flat = Vec3::new(to.x, 0.0, to.z);
    let dist = flat.length().max(0.01);
    let f = car.forward();
    let ff = Vec3::new(f.x, 0.0, f.z).normalize_or(Vec3::NEG_Z);
    let dir = flat / dist;
    let cross = ff.x * dir.z - ff.z * dir.x;
    let ang = cross.atan2(ff.dot(dir).clamp(-1.0, 1.0));
    c.steer = (ang * 2.4).clamp(-1.0, 1.0);
    c.throttle = 1.0;
    (dist, ang.abs())
}

/// The technician. Stages straight from the tutorials: get behind the ball,
/// carry it on the hood matching speed, pop the moment it settles, then the
/// nose-tip air carry (stay under-behind, feather the boost, tiny touches),
/// cradle wheels-first into its belly for the reset, and spend the flip on
/// the finish.
fn striker(b: &mut Bot, ball: &Ball, world: &World, ctx: &Ctx, dt: f32) -> Controls {
    let goal = attack_goal(world).unwrap_or(Vec3::new(0.0, 2.0, 400.0));

    // passing: a Passer whose carry has matured aims the whole machine at a
    // teammate instead of the goal — the pop/carry/flick pipeline then IS
    // the pass. Receivers (non-attackers) run a lane and prejump the arrival.
    let my_team = b.team;
    let my_idx_mate = ctx.mates.iter().find(|m| m.team == my_team && (m.pos - b.car.pos).length() < 0.01).map(|m| m.idx);
    let pass_target = if let Brain::Passer { next } = b.brain {
        ctx.mates
            .iter()
            .find(|m| m.team == my_team && m.idx == next && Some(m.idx) != my_idx_mate)
            .map(|m| m.pos + m.vel * 0.4)
    } else {
        None
    };
    if !ctx.attacker {
        // receiver: hold a lane 9 m goal-side of the ball, offset by index,
        // face the play and prejump anything incoming
        let lat = ((b.jitter * 3.7).sin()) * 7.0;
        let gd = (goal - ball.pos).normalize_or(Vec3::Z);
        let lane = ball.pos + gd * 9.0 + Vec3::new(-gd.z, 0.0, gd.x) * lat;
        let mut c = Controls::default();
        let (d, _) = steer_to(&mut c, &b.car, lane);
        c.throttle = if d < 2.0 { 0.0 } else { 1.0 };
        c.boost = d > 14.0;
        prejump(&mut c, &b.car, &mut b.jump_cd, ball, ctx.grav);
        b.phase = Phase::Approach;
        return c;
    }
    let passing = pass_target.is_some() && b.phase_t > 1.1 && matches!(b.phase, Phase::Carry | Phase::AirCarry);
    let steer_goal = if passing { pass_target.unwrap() } else { goal };
    let to_goal = {
        let d = steer_goal - ball.pos;
        Vec3::new(d.x, 0.0, d.z).normalize_or(Vec3::Z)
    };
    if passing && (ball.pos - b.car.pos).length() < 2.2 {
        b.phase = Phase::Flick; // flick it to the mate now
    }
    let car = &b.car;
    let mut c = Controls::default();
    let ball_rel = ball.pos - car.pos;
    let flat_d = Vec3::new(ball_rel.x, 0.0, ball_rel.z).length();

    // airborne recovery: ground phases must not fight the air (throttle
    // pitches when flying); level out toward the ball and drop, hopping
    // free if wedged on geometry
    if !car.on_ground && matches!(b.phase, Phase::Approach | Phase::Carry) {
        let dir = Vec3::new(ball_rel.x, 0.0, ball_rel.z).normalize_or(to_goal);
        aim(&mut c, car, (dir - Vec3::Y * 0.25).normalize());
        c.throttle = 0.0;
        if car.vel.length() < 1.5 && b.phase_t > 0.8 {
            c.jump_pressed = true; // wedged: hop free
            b.phase_t = 0.0;
        }
        return c;
    }
    match b.phase {
        Phase::Approach => {
            // line up BEHIND the ball on the goal axis ("setup is 80%")
            let behind = ball.pos - to_goal * (2.6 + ball.vel.length() * 0.12);
            let (d, ang) = steer_to(&mut c, car, behind);
            c.boost = ang < 0.3 && d > 8.0;
            c.powerslide = ang > 1.2 && car.on_ground;
            prejump(&mut c, &b.car, &mut b.jump_cd, ball, ctx.grav);
            let on_hood = ball_rel.y > car.half_h * 0.8 && flat_d < 1.4;
            if on_hood {
                b.phase = Phase::Carry;
                b.phase_t = 0.0;
            } else if d < 1.8 && ang < 0.4 {
                b.phase = Phase::Carry;
                b.phase_t = 0.0;
            }
        }
        Phase::Carry => {
            // hood dribble: hold the ball slightly nose-of-center, match speed
            let hold = ball.pos + ball.vel * 0.20 - to_goal * 0.55;
            let (_, _ang) = steer_to(&mut c, car, hold);
            let bs = Vec3::new(ball.vel.x, 0.0, ball.vel.z).length();
            let cs = Vec3::new(car.vel.x, 0.0, car.vel.z).length();
            c.throttle = ((bs - cs) * 0.4 + 0.65).clamp(-0.4, 1.0);
            c.boost = cs + 2.0 < bs;
            let settled = ball_rel.y > car.half_h * 0.7
                && flat_d < 1.3
                && (ball.vel - car.vel).length() < 4.5;
            let bounced_high = ball_rel.y > 1.2 && flat_d < 1.7 && ball.vel.y > -2.0;
            if (settled && b.phase_t > 0.30 || bounced_high) && car.on_ground {
                b.phase = Phase::Pop;
                b.phase_t = 0.0;
            }
            if flat_d > 3.2 && b.phase_t > 1.0 {
                b.phase = Phase::Approach; // lost it: reset the setup
            }
        }
        Phase::Pop => {
            // jump under the ball the moment it settles; nose up, boost
            // almost immediately, ride the hold (the bounce-dribble launch)
            c.throttle = 1.0;
            c.jump_pressed = b.phase_t < dt * 1.5 && car.on_ground;
            c.jump_held = b.phase_t < 0.20;
            if b.phase_t > 0.05 {
                aim(&mut c, car, (to_goal * 0.7 + Vec3::Y * 1.5).normalize());
                c.boost = true;
            }
            if b.phase_t > 0.40 {
                b.phase = Phase::AirCarry;
                b.phase_t = 0.0;
            }
        }
        Phase::AirCarry => {
            // hold under-behind the ball; the nose points through its
            // lower-back third so every touch pushes it up the goal line
            let hold = ball.pos + ball.vel * 0.12 - to_goal * 0.55 - Vec3::Y * (ball.r + 0.45);
            let err = hold - car.pos;
            let aim_pt = ball.pos - to_goal * 0.25 - Vec3::Y * ball.r * 0.40;
            aim(&mut c, car, (aim_pt - car.pos).normalize_or(Vec3::Y) + err * 0.5);
            // feather: boost while below station or off it; cut when parked
            c.throttle = 0.0; // pitch comes from aim, not the throttle
            let below = err.y > 0.15;
            let closing = car.vel.dot(err.normalize_or(Vec3::Y));
            c.boost = below || (err.length() > 0.5 && closing < 12.0);
            let dist = ball_rel.length();
            let goal_d = (goal - ball.pos).length();
            if !b.got_reset && b.phase_t > 0.7 && dist < 2.2 && ball_rel.y > 0.3 {
                b.phase = Phase::Reset;
                b.phase_t = 0.0;
            } else if goal_d < 13.0 && dist < 2.4 {
                b.phase = Phase::Flick;
                b.phase_t = 0.0;
            }
        }
        Phase::Reset => {
            // cradle: wheels toward the ball's belly, boost off, drift in
            let want_up = ball_rel.normalize_or(Vec3::Y);
            let local = car.rot.inverse() * want_up;
            c.roll = (-local.x * 2.8).clamp(-1.0, 1.0);
            c.pitch = (-local.z * 2.8).clamp(-1.0, 1.0);
            c.boost = false;
            if b.phase_t > 0.9 {
                b.phase = Phase::AirCarry; // missed the slap; regroup
                b.phase_t = 0.0;
            }
        }
        Phase::Flick => {
            aim(&mut c, car, (ball.pos - car.pos).normalize_or(Vec3::Z));
            c.throttle = 1.0;
            if b.phase_t > 0.06 && car.can_flip() {
                c.jump_pressed = true; // the dodge into the ball: the finish
            }
            if b.phase_t > 0.5 {
                b.phase = Phase::AirCarry;
                b.phase_t = 0.0;
            }
        }
    }
    c
}

/// Stand between the ball and home; clear it when it comes knocking.
fn defender(b: &mut Bot, ball: &Ball, home: Vec3, time: f32) -> Controls {
    let mut c = Controls::default();
    let car = &b.car;
    let threat = (ball.pos - home).length();
    let post = home + (ball.pos - home).normalize_or(Vec3::Z) * (6.0 + threat * 0.15).min(14.0);
    if threat < 12.0 || (ball.pos - car.pos).length() < 7.0 {
        // challenge: drive the ball down, jump into it when close
        let (d, ang) = steer_to(&mut c, car, ball.pos + ball.vel * 0.2);
        c.boost = ang < 0.35;
        if car.on_ground && d < 3.0 && (0.2..3.0).contains(&(ball.pos.y - car.pos.y)) && b.jump_cd <= 0.0 {
            c.jump_pressed = true;
            c.jump_held = true;
            b.jump_cd = 1.2;
        }
    } else {
        let (d, _ang) = steer_to(&mut c, car, post);
        c.throttle = if d < 1.5 { 0.0 } else { 1.0 };
        c.boost = d > 16.0;
        let _ = time;
    }
    c
}

/// The goal a striker attacks: the +z net (sim convention; sparta strikers
/// would pass their team's target in here instead).
pub fn attack_goal(world: &World) -> Option<Vec3> {
    world.nets.iter().map(|n| n.pos).max_by(|a, b| a.z.total_cmp(&b.z))
}

/// The one true strategy. Chase the ball. Boost at the ball. Flip at the ball.
fn ballchase(b: &mut Bot, ball: &Ball, time: f32, ctx: &Ctx) -> Controls {
    let car = &b.car;
    // spacing: only the team's attacker presses the ball; everyone else
    // orbits a support lane ~11 m out on their own bearing
    let target = if ctx.attacker {
        let wob = Vec3::new((time * 0.7 + b.jitter).sin(), 0.0, (time * 0.9 + b.jitter * 1.7).cos()) * 1.4;
        ball.pos + ball.vel * 0.25 + wob
    } else {
        let a = b.jitter + time * 0.05;
        ball.pos + Vec3::new(a.cos(), 0.0, a.sin()) * (10.0 + (b.jitter % 1.0) * 5.0)
    };
    let to = target - car.pos;
    let flat = Vec3::new(to.x, 0.0, to.z);
    let dist = flat.length().max(0.01);
    let f = car.forward();
    let ff = Vec3::new(f.x, 0.0, f.z).normalize_or(Vec3::NEG_Z);
    let dir = flat / dist;
    // signed angle to target around Y
    let cross = ff.x * dir.z - ff.z * dir.x;
    let dot = ff.dot(dir).clamp(-1.0, 1.0);
    let ang = cross.atan2(dot); // + = target to the right? (see steer sign below)

    let mut c = Controls {
        throttle: 1.0,
        steer: (ang * 2.4).clamp(-1.0, 1.0),
        ..Controls::default()
    };
    let aligned = ang.abs() < 0.35;
    c.boost = aligned && dist > 7.0;
    c.powerslide = ang.abs() > 1.25 && car.on_ground;

    if car.on_ground {
        // pounce: jump (and flip) into a close, reachable ball
        let dy = ball.pos.y - car.pos.y;
        if ctx.attacker && b.jump_cd <= 0.0 && dist < 3.4 && (0.2..3.2).contains(&dy) && aligned {
            c.jump_pressed = true;
            c.jump_held = true;
            b.jump_cd = 1.4 + (b.jitter % 1.0);
        }
        // mini-aerial: a high ball in range gets met in the air
        if ctx.attacker && b.jump_cd <= 0.0 && (2.5..8.0).contains(&dy) && dist < 12.0 && aligned {
            c.jump_pressed = true;
            c.jump_held = true;
            b.jump_cd = 1.6;
        }
        if ctx.attacker {
            prejump(&mut c, &b.car, &mut b.jump_cd, ball, ctx.grav);
        }
    } else {
        // in the air: second press = the flip into the ball
        if b.jump_cd > 0.0 && b.jump_cd < 1.25 && dist < 2.6 && b.car.can_flip() {
            c.jump_pressed = true;
        }
        // nose toward the ball and boost when committed: real air time
        let want_pitch = (to.y / dist.max(2.0)).clamp(-0.7, 0.7);
        c.pitch = (want_pitch - f.y).clamp(-0.8, 0.8);
        c.yaw = (ang * 1.2).clamp(-0.8, 0.8);
        let to_ball = (ball.pos - car.pos).normalize_or(Vec3::Y);
        c.boost = ctx.attacker && f.dot(to_ball) > 0.7 && dist < 14.0;
    }
    c
}

// -------------------------------------------------- power shot ----
/// Pro habit: close + aligned + goal-side means HOP then DODGE THROUGH the
/// ball, not a polite nudge — the two-tap flip is what turns bot touches
/// into shots. Runs after the brain, for every brain.
fn power_shot(world: &World, b: &mut Bot, ball: &Ball, dt: f32, atk: bool, c: &mut Controls) {
    if b.shot_t > 0.0 {
        b.shot_t -= dt;
        c.throttle = 1.0;
        c.boost = true;
        let was = b.shot_t + dt;
        if was > 0.24 && b.shot_t <= 0.24 {
            c.jump_pressed = true; // the hop
        }
        if was > 0.10 && b.shot_t <= 0.10 {
            c.jump_pressed = true; // the dodge, straight through the ball
        }
        return;
    }
    // initiation gates: ATTACKERS in open approach only. A defender that
    // shot-dodges abandons its net; a carrier that hops ruins its own
    // dribble — the sim caught both (goals to zero, def_trav to 397).
    // Ballchase brawlers ONLY: the Striker/Passer phase machine has its own
    // finishing (pop -> carry -> flick), and the shot-dodge was preempting
    // the pop at exactly arrival distance — the sim showed the whole carry
    // pipeline dying (air1 touches 8.7 -> 1.3, goals to zero).
    if !atk || !matches!(b.brain, Brain::Ballchase) {
        return;
    }
    if !b.car.on_ground || ball.pos.y > 2.8 {
        return;
    }
    let goal = attack_goal(world).unwrap_or(Vec3::new(0.0, 2.0, 400.0));
    let to_ball = ball.pos - b.car.pos;
    let d2b = to_ball.length();
    if d2b > 4.6 || d2b < 0.8 {
        return;
    }
    let f = b.car.forward();
    let fw = Vec3::new(f.x, 0.0, f.z).normalize_or(Vec3::Z);
    let tb = Vec3::new(to_ball.x, 0.0, to_ball.z).normalize_or(fw);
    let gb = Vec3::new(goal.x - ball.pos.x, 0.0, goal.z - ball.pos.z).normalize_or(fw);
    if fw.dot(tb) > 0.86 && tb.dot(gb) > 0.45 && b.car.vel.length() > 9.0 {
        b.shot_t = 0.30;
    }
}

// -------------------------------------------------- tube pursuit ----
/// If the ball is inside a tube route, bots with tube IQ (rolled per
/// 5 s window: DROPOUT — sometimes they see the line, sometimes they
/// don't) head for the best mouth and give chase through the pipes.
fn route_override(world: &World, b: &Bot, ball: &Ball, time: f32, c: &mut Controls) {
    let Some((ri, _)) = world.route_locate(ball.pos) else { return };
    let w = (time * 0.2) as u32 ^ (b.jitter * 977.0) as u32;
    let h = w.wrapping_mul(747796405).wrapping_add(2891336453);
    let roll = ((h >> 16) & 1023) as f32 / 1023.0;
    if roll > b.tube_iq {
        return; // this window the bot doesn't out-think the pipes
    }
    // already inside the same route: the brain + interior gravity drive
    if world.route_locate(b.car.pos).map(|(rj, _)| rj == ri).unwrap_or(false) {
        return;
    }
    let (pts, _r) = &world.routes[ri];
    let (a0, z0) = (pts[0], *pts.last().unwrap());
    let goal = if b.car.pos.distance(a0) < b.car.pos.distance(z0) { a0 } else { z0 };
    let to = goal - b.car.pos;
    let f = b.car.forward();
    let fw = Vec3::new(f.x, 0.0, f.z).normalize_or(Vec3::Z);
    let right = Vec3::new(-fw.z, 0.0, fw.x);
    let flat = Vec3::new(to.x, 0.0, to.z).normalize_or(fw);
    c.steer = (flat.dot(right) * 3.0).clamp(-1.0, 1.0);
    c.throttle = 1.0;
    c.boost = to.length() > 20.0 && flat.dot(fw) > 0.7;
    if to.y > 3.0 && to.length() < 14.0 {
        c.jump_pressed = true; // hop for a raised mouth
    }
}

/// A small mixed squad for headless experiments (sim harnesses).
pub fn spawn_match(world: &World, per_team: usize) -> Vec<Bot> {
    let goal = attack_goal(world).unwrap_or(Vec3::new(0.0, 2.0, 400.0));
    let mut v = Vec::new();
    for t in 0..2u8 {
        let side = if t == TEAM_BLUE { -1.0 } else { 1.0 };
        for k in 0..per_team {
            let pos = Vec3::new((k as f32 - 1.0) * 8.0, 1.0, side * 26.0);
            let brain = match k {
                0 => Brain::Striker,
                1 => Brain::Passer { next: 0 },
                _ => Brain::Defender { home: goal * -side },
            };
            v.push(mk_bot_brain(pos, if side < 0.0 { 0.0 } else { std::f32::consts::PI }, t, v.len() + k, brain));
        }
    }
    v
}

/// Freeze or enable mid-match learning (the sim's control condition).
pub fn set_learning(bots: &mut [Bot], on: bool) {
    for b in bots.iter_mut() {
        b.lrn.enabled = on;
    }
}

impl Bot {
    pub fn episodes(&self) -> u32 {
        self.lrn.episodes
    }
}

// ------------------------------------------------- coordinated aerial ----
/// Meet a mate's pop in the air: drive at the predicted intercept, jump at
/// the bot's own learned distance, then boost and steer the nose through the
/// ball toward goal. Every constant here is one of the four learned knobs,
/// so the squad's aerial timing is what improves during a match.
fn air_finish(b: &mut Bot, ball: &Ball, world: &World, grav: f32, c: &mut Controls) {
    let lead = b.lrn.p[0];
    let jump_d = b.lrn.p[1];
    let commit = b.lrn.p[2];
    let lift = b.lrn.p[3];
    let meet = ball.pos + ball.vel * lead - Vec3::Y * (0.5 * grav * lead * lead);
    let goal = attack_goal(world).unwrap_or(Vec3::new(0.0, 2.0, 400.0));
    // aim UNDER-BEHIND the ball, on the goal side: that is what turns a
    // collision into a carry instead of a clearance
    let toward_goal = (goal - ball.pos).normalize_or(Vec3::Z);
    let aim = meet - toward_goal * 0.9 - Vec3::Y * lift * 0.35;
    let to = aim - b.car.pos;
    let f = b.car.forward();
    let fw = Vec3::new(f.x, 0.0, f.z).normalize_or(Vec3::Z);
    let right = Vec3::new(-fw.z, 0.0, fw.x);
    let flat = Vec3::new(to.x, 0.0, to.z);
    let aligned = flat.normalize_or(fw).dot(fw);
    c.steer = (flat.normalize_or(fw).dot(right) * 3.0).clamp(-1.0, 1.0);
    c.throttle = 1.0;
    if b.car.on_ground {
        // still chasing: boost only when pointed at the intercept
        c.boost = aligned > 0.75 && flat.length() > 6.0;
        if flat.length() < jump_d * 2.2 && aligned > 0.6 && b.jump_cd <= 0.0 {
            c.jump_pressed = true;
            b.jump_cd = 0.5;
        }
    } else {
        // airborne: nose at the ball, feather the boost toward it
        let dir = (aim - b.car.pos).normalize_or(f);
        c.pitch = (dir.y - f.y).clamp(-1.0, 1.0);
        c.yaw = (dir.cross(f).y * -2.0).clamp(-1.0, 1.0);
        c.boost = dir.dot(f) > commit && to.length() > 1.2;
        c.throttle = 1.0;
    }
}
