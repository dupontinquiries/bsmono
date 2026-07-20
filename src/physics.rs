//! Arcade physics with Rocket League's actual tuning where it matters
//! (100 uu = 1 m): gravity 6.5, max drive 14.1, supersonic cap 23.0, boost
//! accel 9.92 with 33.3/s drain, jump 2.92 + hold, 1.45 s dodge window, ball
//! restitution 0.6 with tangential friction on bounce, and the signature
//! extra "touch" impulse on car→ball contact that makes dribbles carry.
//!
//! v1.3: everything above is now the 100% baseline of a live [`Config`] —
//! gravity, arena bounce/friction, ball size/bounce/friction/weight, car
//! size/grip/speed/accel/boost/jump, flip strength — plus real flip rules:
//! directional flips (the dodge spin), **flip cancels** (hold opposite pitch),
//! and **flip resets**: touch anything with your wheels while airborne and
//! the flip comes back, RL-style, which is the whole air-dribble endgame.

use glam::{Quat, Vec3};

use crate::config::Config;
use crate::world::World;

pub const GRAV: f32 = 6.5;
pub const CAR_MAX: f32 = 23.0;
pub const DRIVE_MAX: f32 = 14.1;
pub const BOOST_ACCEL: f32 = 9.92;
pub const BOOST_DRAIN: f32 = 33.3;
pub const BALL_R: f32 = 0.9325;
pub const BALL_MAX: f32 = 60.0;
pub const M_CAR: f32 = 180.0;
pub const M_BALL: f32 = 30.0;

#[derive(Clone, Copy, Default)]
pub struct Controls {
    pub throttle: f32,   // +fwd / -back
    pub steer: f32,      // +1 = right
    pub pitch: f32,      // +1 = nose up (S), -1 = nose down (W)
    pub yaw: f32,        // +1 = yaw left (A)
    pub roll: f32,       // +1 = roll left (Q)
    pub boost: bool,
    pub jump_held: bool,
    pub jump_pressed: bool,
    pub jump_depth: f32, // wooting analog on the jump key: 1.0 = digital
    pub powerslide: bool,
}

pub struct Car {
    pub pos: Vec3,
    pub vel: Vec3,
    pub rot: Quat,
    pub ang: Vec3, // body-frame angular velocity
    pub boost: f32,
    pub on_ground: bool,
    pub ground_n: Vec3,
    pub half_len: f32,
    pub half_h: f32,
    pub half_w: f32,
    pub susp: f32, // suspension compression 0..1 (renderer bobs the body)
    pub wheel_r: f32,
    pub wheel_spin: f32,
    pub steer_vis: f32,
    // jump / dodge state
    jump_hold: f32,
    jbuf: f32, // jump input buffer: a press just before touchdown lands
    dodge_window: f32,
    has_dodge: bool,
    dodge_timer: f32,
    dodge_ang: Vec3,
    reset_cd: f32,
    pub supersonic: bool,
}

impl Car {
    pub fn new() -> Car {
        Car {
            pos: Vec3::new(0.0, 1.0, 26.0),
            vel: Vec3::ZERO,
            rot: Quat::IDENTITY,
            ang: Vec3::ZERO,
            boost: 45.0,
            on_ground: false,
            ground_n: Vec3::Y,
            half_len: 0.62,
            half_h: 0.19,
            wheel_r: 0.11,
            half_w: 0.32,
            susp: 0.0,
            wheel_spin: 0.0,
            steer_vis: 0.0,
            jump_hold: 0.0,
            jbuf: 0.0,
            dodge_window: 0.0,
            has_dodge: false,
            dodge_timer: 0.0,
            dodge_ang: Vec3::ZERO,
            reset_cd: 0.0,
            supersonic: false,
        }
    }
    pub fn forward(&self) -> Vec3 {
        self.rot * Vec3::NEG_Z
    }
    pub fn up(&self) -> Vec3 {
        self.rot * Vec3::Y
    }
    /// A jump/flip is banked (restoration probe for the landing sim).
    pub fn has_flip(&self) -> bool {
        self.has_dodge
    }
    /// The flip rotation is actively playing.
    pub fn flipping(&self) -> bool {
        self.dodge_timer > 0.0
    }
    /// A flip (or double jump) is in the tank right now.
    pub fn can_flip(&self) -> bool {
        self.has_dodge && self.dodge_window > 0.0
    }
    pub fn reset(&mut self, world: &World) {
        let h = world.ground_h(0.0, 26.0);
        *self = Car {
            half_len: self.half_len,
            half_h: self.half_h,
            wheel_r: self.wheel_r,
            boost: 45.0,
            pos: Vec3::new(0.0, h + self.half_h + 0.05, 26.0),
            ..Car::new()
        };
    }
}

pub struct Ball {
    pub pos: Vec3,
    pub vel: Vec3,
    pub rot: Quat,
    pub ang: Vec3,
    pub r: f32,
    pub last_touch: u8, // WHO_* code of the last car to touch the ball
    pinch_cd: f32,
}

impl Ball {
    pub fn new() -> Ball {
        Ball { pos: Vec3::new(0.0, BALL_R + 4.0, 14.0), vel: Vec3::ZERO, rot: Quat::IDENTITY, ang: Vec3::ZERO, r: BALL_R, last_touch: WHO_NONE, pinch_cd: 0.0 }
    }
    pub fn reset_in_front(&mut self, car: &Car) {
        self.pos = car.pos + car.forward() * 4.0 + Vec3::Y * (0.6 + self.r);
        self.vel = Vec3::ZERO;
        self.ang = Vec3::ZERO;
    }
    /// Tee up a friendly lob for starting an air dribble.
    pub fn toss(&mut self, car: &Car) {
        let f = car.forward();
        let fh = Vec3::new(f.x, 0.0, f.z).normalize_or(Vec3::NEG_Z);
        self.pos = car.pos + fh * 2.4 + Vec3::Y * (0.5 + self.r);
        self.vel = fh * (car.vel.length() * 0.7 + 2.0) + Vec3::Y * 7.2;
        self.ang = Vec3::ZERO;
    }
}

/// Ball-touch attribution codes (teams are bots::TEAM_* values 0..2).
pub const WHO_NONE: u8 = 255;
pub const WHO_PLAYER: u8 = 200;
pub const WHO_REMOTE: u8 = 210;

#[derive(Default)]
pub struct StepEvents {
    pub ball_rings: u32,
    pub car_rings: u32,
    pub pads_taken: u32,
    pub ring_hits: Vec<(Vec3, [f32; 4], bool)>, // pos, ring color, was it the ball?
    pub pad_hits: Vec<(Vec3, bool)>,            // pos, big pad?
    pub touches: Vec<(Vec3, f32)>,              // car→ball contact point, impact speed
    pub bounces: Vec<(Vec3, Vec3, f32)>,        // ball→world: pos, normal, impact speed
    pub goals: Vec<(Vec3, u8)>,                 // net opening pos + last toucher
    pub jumped: bool,
    pub dodged: bool,
    pub pinched: bool,
    pub ball_bounce: Option<(Vec3, f32)>, // world-contact point + impact speed
    pub flip_reset: Option<Vec3>,               // underside contact restored the flip
    pub landed: Option<f32>,                    // car landing impact speed
}

pub fn step(
    car: &mut Car,
    ball: &mut Ball,
    world: &mut World,
    c: &Controls,
    cfg: &Config,
    dt: f32,
    ev: &mut StepEvents,
) {
    let ball_prev = ball.pos;
    let car_prev = car.pos;

    step_car(car, ball, world, c, cfg, dt, ev);
    step_ball(ball, world, cfg, dt, ev);
    // ball strikes open tiles (DROPSHOT panels, GATEWORKS doors)
    if let Some((bp, pow)) = ev.ball_bounce.take() {
        if pow > 3.0 && !world.tiles.is_empty() {
            world.hit_tile(bp, pow);
        }
    }
    // DROPSHOT pit: a ball through the floor scores against that side
    if let Some(py) = world.drop_pit {
        if ball.pos.y < py - 3.0 && world.pit_score_ok() {
            ev.goals.push((ball.pos, ball.last_touch));
            ball.pos = Vec3::new(ball.pos.x * 0.5, py + 7.0, ball.pos.z * 0.5);
            ball.vel = Vec3::new(0.0, 5.0, 0.0);
        }
    }
    step_bodies(world, car, ball, dt);
    collide_car_ball(car, ball, cfg, WHO_PLAYER, ev);

    // blast-off launchers: anything on the ring gets the arrow
    for i in 0..world.launchers.len() {
        let (pos, dir, power, timer) = {
            let l = &world.launchers[i];
            (l.pos, l.dir, l.power, l.timer)
        };
        if timer <= 0.0 && car.pos.distance_squared(pos) < 2.6 * 2.6 {
            car.vel += dir * power;
            world.launchers[i].timer = 1.0;
            let sh = world.launchers[i].shape;
            world.shapes[sh].flash = 1.2;
            ev.jumped = true; // reuse the jump whoosh
        }
        if world.launchers[i].timer <= 0.0 && ball.pos.distance_squared(pos) < 2.9 * 2.9 {
            ball.vel += dir * power * 0.9;
            world.launchers[i].timer = 1.0;
        }
    }

    // boost pads
    for p in world.pads.iter_mut() {
        if p.timer > 0.0 {
            continue;
        }
        let r = if p.big { 1.7 } else { 1.05 };
        let d = car.pos - p.pos;
        if d.y.abs() < 2.2 && Vec3::new(d.x, 0.0, d.z).length() < r {
            car.boost = (car.boost + if p.big { 100.0 } else { 12.0 }).min(100.0 * cfg.car.boost_max);
            p.timer = if p.big { 10.0 } else { 4.0 };
            ev.pads_taken += 1;
            ev.pad_hits.push((p.pos, p.big));
        }
    }

    // ring passes (ball scores, car just flashes)
    for r in 0..world.rings.len() {
        let ring = &world.rings[r];
        if ring.pos.distance_squared(ball.pos) < (ring.hole_r + 6.0) * (ring.hole_r + 6.0) {
            if crossed(ring, ball_prev, ball.pos) {
                let color = world.shapes[ring.shape].color;
                world.shapes[ring.shape].flash = 1.2;
                ev.ball_rings += 1;
                ev.ring_hits.push((ring.pos, color, true));
            }
        }
        let ring = &world.rings[r];
        if ring.pos.distance_squared(car.pos) < (ring.hole_r + 6.0) * (ring.hole_r + 6.0) {
            if crossed(ring, car_prev, car.pos) {
                let color = world.shapes[ring.shape].color;
                world.shapes[ring.shape].flash = world.shapes[ring.shape].flash.max(0.6);
                ev.car_rings += 1;
                ev.ring_hits.push((ring.pos, color, false));
            }
        }
    }

    ball_goal_check(ball, ball_prev, world, ev);

    // ---- PINCH: squeezed between a car touch and a world surface in the
    // same frame. Instead of chattering between the two, the bounce is
    // GAMIFIED: total speed converts to kinetic exit along the wedge seam
    // (lifted slightly off both faces), like the ball squirting out of a
    // Rocket League pinch.
    if ball.pinch_cd <= 0.0 {
        if let Some(&(cp, closing)) = ev.touches.last() {
            let dw = world.sdf(ball.pos);
            if dw < ball.r + 0.14 {
                let nw = world.normal(ball.pos);
                let nc = (ball.pos - cp).normalize_or(nw);
                if nw.dot(nc) < -0.15 {
                    let seam = nw.cross(nc);
                    let out = if seam.length_squared() > 0.05 {
                        let s0 = seam.normalize();
                        let s1 = if ball.vel.dot(s0) >= 0.0 { s0 } else { -s0 };
                        (s1 + (nw + nc) * 0.35).normalize_or(nw)
                    } else {
                        nw
                    };
                    let speed = (ball.vel.length().max(closing) * 1.06 + 4.0).min(95.0);
                    ball.vel = out * speed;
                    ball.pinch_cd = 0.30;
                    ev.pinched = true;
                }
            }
        }
    }
}

/// The net-goal crossing test (shared by the full step and the sim).
/// KUIPER free bodies: drift, collide with the world and each other, and get
/// SHOVED — light asteroids move for the player, heavy ones move the player.
fn step_bodies(world: &mut World, car: &mut Car, ball: &mut Ball, dt: f32) {
    if world.bodies.is_empty() {
        return;
    }
    let n = world.bodies.len();
    for i in 0..n {
        let (mut pos, mut vel, r, mass) = {
            let b = &world.bodies[i];
            (b.pos, b.vel, b.r, b.mass)
        };
        vel *= (-0.02 * dt).exp(); // the faintest drag: space, but playable
        pos += vel * dt;
        // world surfaces
        let d = world.sdf(pos);
        if d < r {
            let nw = world.normal(pos);
            pos += nw * (r - d);
            let vn = vel.dot(nw);
            if vn < 0.0 {
                vel -= nw * vn * 1.55; // e = 0.55
            }
        }
        // car shove (both ways, mass-weighted)
        let dc = pos - car.pos;
        let rr = r + 0.7;
        if dc.length_squared() < rr * rr {
            let dist = dc.length().max(0.01);
            let nn = dc / dist;
            let rel = vel - car.vel;
            let vn = rel.dot(nn);
            if vn < 0.0 {
                let j = -(1.2) * vn / (1.0 / 12.0 + 1.0 / mass);
                vel += nn * (j / mass);
                car.vel -= nn * (j / 12.0);
            }
            pos += nn * (rr - dist) * 0.6;
            car.pos -= nn * (rr - dist) * (0.4 * (mass / (mass + 12.0)));
        }
        // ball shove
        let db = pos - ball.pos;
        let rb = r + ball.r;
        if db.length_squared() < rb * rb {
            let dist = db.length().max(0.01);
            let nn = db / dist;
            let rel = vel - ball.vel;
            let vn = rel.dot(nn);
            if vn < 0.0 {
                let j = -(1.4) * vn / (1.0 / 3.0 + 1.0 / mass);
                vel += nn * (j / mass);
                ball.vel -= nn * (j / 3.0);
            }
            ball.pos -= nn * (rb - dist) * 0.8;
        }
        world.bodies[i].pos = pos;
        world.bodies[i].vel = vel;
    }
    // asteroid-asteroid collisions (n <= 10: the O(n^2) is nothing)
    for i in 0..n {
        for j in (i + 1)..n {
            let (pi, vi, ri, mi) = {
                let b = &world.bodies[i];
                (b.pos, b.vel, b.r, b.mass)
            };
            let (pj, vj, rj, mj) = {
                let b = &world.bodies[j];
                (b.pos, b.vel, b.r, b.mass)
            };
            let d = pj - pi;
            let rr = ri + rj;
            if d.length_squared() >= rr * rr {
                continue;
            }
            let dist = d.length().max(0.01);
            let nn = d / dist;
            let vn = (vj - vi).dot(nn);
            if vn < 0.0 {
                let j2 = -(1.5) * vn / (1.0 / mi + 1.0 / mj);
                world.bodies[i].vel -= nn * (j2 / mi);
                world.bodies[j].vel += nn * (j2 / mj);
            }
            let push = (rr - dist) * 0.5;
            world.bodies[i].pos -= nn * push;
            world.bodies[j].pos += nn * push;
        }
    }
    // sync the drawn/marched shapes
    for i in 0..n {
        let (shape, pos) = (world.bodies[i].shape, world.bodies[i].pos);
        let sh = &mut world.shapes[shape];
        let sc = glam::Vec3::splat(world.bodies[i].r);
        sh.pos = pos;
        sh.mat = glam::Mat4::from_scale_rotation_translation(sc, glam::Quat::IDENTITY, pos);
    }
}

pub fn ball_goal_check(ball: &Ball, ball_prev: Vec3, world: &mut World, ev: &mut StepEvents) {
    for i in 0..world.nets.len() {
        let (pos, inv, hw, hh, timer, s0, ns) = {
            let n = &world.nets[i];
            (n.pos, n.inv_rot, n.hw, n.hh, n.timer, n.shape0, n.nshapes)
        };
        let reach = hw.max(hh) + 8.0;
        if timer > 0.0 || pos.distance_squared(ball.pos) > reach * reach {
            continue;
        }
        let la = inv * (ball_prev - pos);
        let lb = inv * (ball.pos - pos);
        if la.z > 0.0 && lb.z <= 0.0 {
            let t = la.z / (la.z - lb.z);
            let hit = la.lerp(lb, t);
            if hit.x.abs() < hw - ball.r * 0.4 && hit.y.abs() < hh - ball.r * 0.25 {
                world.nets[i].timer = 1.5;
                for k in s0..s0 + ns {
                    world.shapes[k].flash = 1.3;
                }
                ev.goals.push((pos, ball.last_touch));
            }
        }
    }
}

fn crossed(ring: &crate::world::Ring, a: Vec3, b: Vec3) -> bool {
    let la = ring.inv_rot * (a - ring.pos);
    let lb = ring.inv_rot * (b - ring.pos);
    if la.y == lb.y || la.y.signum() == lb.y.signum() {
        return false;
    }
    let t = la.y / (la.y - lb.y);
    let hit = la.lerp(lb, t);
    Vec3::new(hit.x, 0.0, hit.z).length() < ring.hole_r
}

// ---------------------------------------------------------------- car ----
fn step_car(car: &mut Car, ball: &Ball, world: &World, c: &Controls, cfg: &Config, dt: f32, ev: &mut StepEvents) {
    let d = world.sdf(car.pos);
    let n = world.normal(car.pos);
    let clearance = car.half_h;
    // Wheels grip ANY surface (RL-style wall/ceiling driving): contact counts
    // as "grounded" when the car's underside faces the surface. Floors accept
    // any orientation (you land, the align sorts you out); walls and ceilings
    // require wheels-toward-surface and enough speed to hold on — drop below
    // the peel speed and gravity takes you back.
    // jump buffering: remember a press for a beat so hitting jump a frame
    // before touchdown still hops the instant wheels touch (the "getting my
    // jump back is slow" fix — the press used to be eaten mid-air)
    if c.jump_pressed {
        car.jbuf = 0.09;
    } else {
        car.jbuf = (car.jbuf - dt).max(0.0);
    }

    let gdir = world.gravity_dir(car.pos);
    let up_w = -gdir;
    let ori = car.up().dot(n);
    let tang_sp = (car.vel - n * car.vel.dot(n)).length();
    // steepness is judged against LOCAL gravity, so the inside of a
    // perplexus tube counts as floor all the way around
    let nyy = n.dot(up_w);
    let steep = nyy < 0.40;
    let ceilingish = nyy < -0.30;
    let grounded = d <= clearance + 0.11
        && (if ceilingish {
            ori > 0.60 && tang_sp > 6.0
        } else if steep {
            ori > 0.45 && tang_sp > 3.2
        } else {
            // floors: wheels must be at least vaguely toward the ground —
            // a turtled (roof-down) car is body contact, not driving. A car
            // mid-DODGE is exempt from the tilt gate so slamming a flip into
            // the ground catches cleanly: that's the wave dash.
            nyy > 0.35 && (ori > 0.12 || car.dodge_timer > 0.0)
        });
    if grounded && !car.on_ground {
        ev.landed = Some((-car.vel.dot(n)).max(0.0));
        // WAVE DASH: landing while the dodge is still playing converts the
        // flip into pure forward speed (RL's wavedash) — spin dies on the
        // ground, momentum lives on with a kick along the surface.
        if car.dodge_timer > 0.0 {
            let f = car.forward();
            let flat = (f - n * f.dot(n)).normalize_or(f);
            car.vel += flat * 4.2 * cfg.flip.strength;
            ev.dodged = true; // the whoosh
        }
        // touching down ends any flip immediately and kills the tumble, so
        // the ground blend wins over leftover flip spin
        car.dodge_timer = 0.0;
        car.dodge_ang = Vec3::ZERO;
        car.ang = Vec3::ZERO;
        // hard partial upright on the landing frame: the ground wins NOW,
        // the smooth align finishes the rest
        let corr = Quat::from_rotation_arc(car.up(), n);
        car.rot = (Quat::IDENTITY.slerp(corr, 0.55) * car.rot).normalize();
    }
    car.on_ground = grounded;
    car.ground_n = n;
    car.reset_cd = (car.reset_cd - dt).max(0.0);

    // gravity
    car.vel += gdir * GRAV * cfg.arena.gravity * dt;
    // FLOODWATER: a flooded tube's current owns you — strong downstream
    // servo (buoyancy is implicit: the target velocity has no fall in it)
    if let Some(cv) = world.current(car.pos) {
        car.vel += (cv - car.vel) * (1.0 - (-2.6 * dt).exp());
    }

    // ---- pinball field zones ----
    match world.zone(car.pos) {
        Some(crate::world::ZoneKind::Vibe) => {
            let h = (car.pos.x * 12.9 + car.pos.z * 7.1 + world.time * 47.0).sin();
            let h2 = (car.pos.z * 11.3 - world.time * 39.0).cos();
            car.vel += Vec3::new(h, 0.3 * h2.abs(), h2) * 9.0 * dt;
            car.ang.y += h * 3.0 * dt;
        }
        Some(crate::world::ZoneKind::Boost) => {
            car.vel += car.forward() * 22.0 * dt;
        }
        _ => {}
    }

    let top = CAR_MAX * cfg.car.top_speed;
    let drive_max = DRIVE_MAX * cfg.car.top_speed;

    if grounded {
        car.dodge_window = 1.45;
        car.has_dodge = true;
        car.dodge_timer = 0.0;

        // align car up with the surface, keep heading
        let cur_up = car.up();
        let corr = Quat::from_rotation_arc(cur_up, n);
        let k = 1.0 - (-18.0 * dt).exp();
        car.rot = (Quat::IDENTITY.slerp(corr, k) * car.rot).normalize();
        car.ang.x *= (-16.0 * dt).exp();
        car.ang.z *= (-16.0 * dt).exp();
        car.ang.y = 0.0;

        // basis in the contact plane
        let f_raw = car.forward();
        let f_t = (f_raw - n * f_raw.dot(n)).normalize_or(f_raw);
        let side = n.cross(f_t).normalize_or(Vec3::X);
        let vf = car.vel.dot(f_t);
        let vs = car.vel.dot(side);

        // steering: tighter at low speed, RL-ish yaw rates. Powerslide is
        // the hairpin: it multiplies the yaw rate hard (drift-donut tight)
        // while the slip section below lets the rear come around.
        let sp = vf.abs();
        let mut rate = (3.0 - (sp * 0.050).min(1.65)).max(1.15) * cfg.car.steer_rate;
        if c.powerslide {
            rate *= 1.7;
        }
        let dir_sign = if vf < 0.0 { -1.0 } else { 1.0 };
        let yaw_rate = -c.steer * rate * dir_sign;
        if sp > 0.25 {
            car.rot = (Quat::from_axis_angle(n, yaw_rate * dt) * car.rot).normalize();
        }
        car.steer_vis += (c.steer * 0.42 - car.steer_vis) * (1.0 - (-16.0 * dt).exp());

        // throttle / brake / coast
        let acc = 16.5 * cfg.car.accel;
        let mut a = 0.0;
        if c.throttle > 0.05 {
            a = if vf < -0.3 { 35.0 * cfg.car.accel } else if vf < drive_max { acc * c.throttle } else { 0.0 };
        } else if c.throttle < -0.05 {
            a = if vf > 0.3 { -35.0 * cfg.car.accel } else if vf > -drive_max * 0.8 { acc * c.throttle } else { 0.0 };
        } else if sp > 0.1 {
            a = -5.25 * vf.signum();
        }
        car.vel += f_t * a * dt;

        // lateral grip (loosened by powerslide)
        let grip_k = cfg.car.grip * cfg.arena.friction;
        // slide grip near-zero so the rear swings: hairpins on a dime
        let grip = if c.powerslide { 1.3 * grip_k.sqrt() } else { 11.0 * grip_k };
        car.vel -= side * vs * (1.0 - (-grip * dt).exp());

        // keep the car pressed onto the surface
        let vn = car.vel.dot(n);
        // WHEEL SUSPENSION: a spring-damper ride instead of a rigid snap.
        // The bottom 45% of clearance is the compression zone — landings
        // sink in and rebound, bumps stay glued, contact reads SOLID.
        let zone = clearance * 0.45;
        let bottom = clearance - zone;
        if d < bottom {
            // axle bind: the hard stop under a truly heavy hit
            car.pos += n * (bottom - d);
            let vh = car.vel.dot(n);
            if vh < 0.0 {
                car.vel -= n * vh;
            }
        } else if d < clearance + 0.02 {
            let e = (clearance - d) / zone; // 1 at the stop, <0 just above ride
            // asymmetric damping: swallow the hit, let the rebound breathe
            let cdamp = if vn < 0.0 { 24.0 } else { 9.0 };
            car.vel += n * (e * 58.0 - vn * cdamp) * dt;
        } else {
            // stick to the surface: gentle over crests on the floor, strong
            // magnetic grip on walls and ceilings (the RL wall-drive feel)
            let stick = if steep { 22.0 } else { 2.4 };
            car.vel -= n * stick * dt;
        }
        let starget = ((clearance - d) / zone).clamp(0.0, 1.0);
        car.susp += (starget - car.susp) * (1.0 - (-14.0 * dt).exp());

        car.wheel_spin += vf / car.wheel_r.max(0.05) * dt;

        // first jump (a buffered press from just before touchdown counts);
        // wooting depth trims the impulse a touch for takeoff-pitch control
        if c.jump_pressed || car.jbuf > 0.0 {
            car.jbuf = 0.0;
            ev.jumped = true;
            let depth = 0.95 + 0.05 * c.jump_depth.clamp(0.0, 1.0);
            car.vel += n * 2.92 * cfg.car.jump * depth;
            car.jump_hold = 0.19;
            car.pos += n * 0.04;
            car.on_ground = false;
        }
    } else {
        // ---------------- airborne / body contact ----------------
        // TRUE landing physics (separate from the learnable land-assist):
        //
        // (a) end-pivot: a car propped on its nose or tail — center hovering
        //     just above the grounded threshold — pivots flat under gravity,
        //     ~0.75 s from 45 deg, exactly like a see-saw losing its prop.
        // (b) turtle scrape: roof/side against a surface is body contact —
        //     no wheel traction, real friction, boost scrapes you along,
        //     attitude inputs at reduced authority so you can wriggle over.
        if car.dodge_timer <= 0.0 {
            let f = car.forward();
            let side = car.rot * Vec3::X;
            let rim = car.wheel_r.max(0.10) + 0.06;
            // four rim probes: nose, tail, left, right. Any planted point
            // becomes a pivot and gravity torques the car FLAT: for a probe
            // offset o from center, tau = (-o) x (m g) -> axis = g x o_dir.
            // (v1.11 had this axis mirrored, which is why propped cars held
            // their pose against the pushout instead of laying down.)
            let probes = [
                car.pos + f * (car.half_len * 0.85),
                car.pos - f * (car.half_len * 0.85),
                car.pos + side * (car.half_w.max(0.2) * 0.95),
                car.pos - side * (car.half_w.max(0.2) * 0.95),
            ];
            let mut near = false; // any probe in its torque band
            for pt in probes {
                let dp = world.sdf(pt);
                // torque works through a NEAR-contact band, not just true
                // penetration — otherwise the pushout lifts the probe clear
                // each frame and the settle stalls in a hover (the sim's
                // landing matrix caught exactly that: nose45 "NEVER" flat)
                let band = rim * 1.9;
                if dp < band {
                    near = true;
                    let w = (1.0 - dp / band).clamp(0.0, 1.0);
                    let o_dir = (pt - car.pos).normalize_or(f);
                    let mut axis_w = gdir.cross(o_dir);
                    if axis_w.length_squared() < 0.02 {
                        // balanced EXACTLY on the point (nose-90 / side-90):
                        // zero moment is mathematically true and physically
                        // absurd — break the symmetry with a HORIZONTAL body
                        // axis (projected off gravity, else a vertical pick
                        // just yaws the car in place on its balance point)
                        for cand in [car.rot * Vec3::X, car.rot * Vec3::NEG_Z] {
                            let horiz = cand - gdir * cand.dot(gdir);
                            if horiz.length_squared() > 0.04 {
                                axis_w = horiz.normalize() * 0.9;
                                break;
                            }
                        }
                    }
                    let tau_b = car.rot.inverse() * axis_w;
                    // deeply inverted, "lay flat" would STABILIZE the
                    // turtle (roof-flat is flat): hand righting entirely
                    // to the inverted-hull roll below instead
                    if ori > -0.2 {
                        car.ang += tau_b * 15.0 * w * dt;
                    }
                }
                if dp < rim {
                    let np = world.normal(pt);
                    car.pos += np * (rim - dp) * 0.55;
                    let vn = car.vel.dot(np);
                    if vn < 0.0 {
                        car.vel -= np * vn * 0.5;
                    }
                    if ori > -0.15 {
                        // wheels-ish contact still counts as touching down:
                        // give the jump back (the air dribble needs it)
                        car.has_dodge = true;
                        car.dodge_window = 1.45;
                    }
                }
            }
            // roof/side scrape (center in range but not "grounded")
            if d <= clearance + 0.11 {
                if d < clearance {
                    car.pos += n * (clearance - d);
                }
                let vn = car.vel.dot(n);
                if vn < 0.0 {
                    car.vel -= n * vn * 0.9;
                }
                // body friction: the paint pays for the slide
                let vt = car.vel - n * car.vel.dot(n);
                car.vel -= vt * (1.0 - (-2.4 * dt).exp());
                car.ang *= (-1.5 * dt).exp();
                if ori > -0.15 {
                    // brushed the ground wheels-down, even if a beat too
                    // tilted to count as driving: restore the jump so a
                    // clean-LOOKING landing always refuels the air dribble
                    car.has_dodge = true;
                    car.dodge_window = 1.45;
                }
            }
            // An inverted hull isn't a stable pose. Frame-independent
            // righting: torque about up_car x up_world monotonically turns
            // our up toward world up from ANY pose (a signum-in-body scalar
            // flipped sign mid-roll and stabilized the turtle — the landing
            // matrix caught it). Own gate with EXTRA reach: rolling onto
            // the side lifts the center past the scrape gate, and the drive
            // used to die right there, parked at 56 degrees inverted.
            if std::env::var("SETTLE_DBG").is_ok() {
                eprintln!("settle: ori={ori:.2} d={d:.2} clr={clearance:.2} dodge={:.2}", car.dodge_timer);
            }
            // GROUND-GIVES-FLIP at the actual WHEEL corners: the four slim
            // body probes missed honest wheel brushes (too narrow a net).
            // Any wheel within a tire's reach of a surface, wheels-down-ish,
            // hands the flip back. Cost only while flipless.
            if !car.has_dodge && ori > -0.15 {
                let win = car.wheel_r + 0.12;
                'wheels: for sz in [-1.0f32, 1.0] {
                    for sx in [-1.0f32, 1.0] {
                        let wp = car.pos
                            + car.rot
                                * Vec3::new(
                                    sx * car.half_w * 0.98,
                                    -car.half_h * 0.45,
                                    sz * car.half_len * 0.62,
                                );
                        if world.sdf(wp) < win {
                            car.has_dodge = true;
                            car.dodge_window = 1.45;
                            break 'wheels;
                        }
                    }
                }
            }
            // reach: near-probe covers END-props of any model length (a
            // tumbled car parked on a nose probe at d=0.66 sat 2 cm past
            // the old fixed reach — the landing matrix found it)
            if ori < 0.2 && (near || d <= clearance + 0.45) {
                let upw = -gdir;
                let mut axw = car.up().cross(upw);
                if axw.length_squared() < 0.02 {
                    // exactly inverted: any horizontal axis breaks it
                    for cand in [car.rot * Vec3::X, car.rot * Vec3::NEG_Z] {
                        let h = cand - gdir * cand.dot(gdir);
                        if h.length_squared() > 0.04 {
                            axw = h;
                            break;
                        }
                    }
                }
                let drive = 5.0 + 11.0 * (-ori).clamp(0.0, 1.0);
                car.ang += (car.rot.inverse() * axw.normalize_or(Vec3::X)) * drive * dt;
            }
        }

        car.susp += (0.0 - car.susp) * (1.0 - (-9.0 * dt).exp());
        car.dodge_window = (car.dodge_window - dt).max(0.0);
        if cfg.flip.unlimited {
            car.has_dodge = true;
            car.dodge_window = 1.45;
        }

        if car.jump_hold > 0.0 {
            if c.jump_held {
                let depth = 0.88 + 0.12 * c.jump_depth.clamp(0.0, 1.0);
                car.vel += car.up() * 14.6 * cfg.car.jump * depth * dt;
            } else {
                car.jump_hold = 0.0;
            }
            car.jump_hold -= dt;
        }

        if c.jump_pressed && car.has_dodge && car.dodge_window > 0.0 {
            car.jbuf = 0.0; // an intentional dodge shouldn't also hop on land
            // dodge direction in body frame: steer right = +x, throttle fwd = -z
            let d2 = Vec3::new(c.steer, 0.0, -c.throttle);
            if d2.length() > 0.3 {
                let dn = d2.normalize();
                // flip: burst of body-frame spin + a velocity kick along the dodge
                // toned from 8.4: full-rate spin read too violent, and the
                // FLIP SPIN dial rides on top for +-50% in 5% steps
                car.dodge_ang = Vec3::Y.cross(dn).normalize_or(Vec3::X) * 7.4 * cfg.flip.strength.sqrt() * cfg.flip.spin;
                car.dodge_timer = 0.62;
                let yaw_only = yaw_quat(car.rot);
                let world_dir = yaw_only * dn;
                car.vel += world_dir * 5.2 * cfg.flip.strength;
                ev.dodged = true;
                if car.vel.y > 0.0 && dn.z < -0.5 {
                    car.vel.y *= 0.65; // forward flips flatten the arc a touch
                }
            } else {
                car.vel += car.up() * 2.92 * cfg.car.jump; // straight double jump
                ev.jumped = true;
            }
            car.has_dodge = false;
        }

        if car.dodge_timer > 0.0 {
            car.dodge_timer -= dt;
            // flip cancel: hold the opposite pitch to kill the rotation
            // (front flip + hold back = the speedflip/cancel family)
            if cfg.flip.cancel && c.pitch * car.dodge_ang.x < -3.0 {
                car.dodge_ang.x = 0.0;
                // half-flips: a cancel WITH roll intent hands control back
                // fast. Gated on roll so a mere held pitch (the bots' air
                // habit) doesn't truncate flicks — the sim caught the
                // whole offense collapsing when it did.
                if c.roll.abs() > 0.2 {
                    car.dodge_timer = car.dodge_timer.min(0.16);
                }
            }
            // TAPER: the flip spins full for its first 55%, then eases to
            // 42% by the end — the rotation fades out instead of cutting
            let tf = 1.0 - (car.dodge_timer / 0.62).clamp(0.0, 1.0);
            let taper = if tf < 0.55 { 1.0 } else { 1.0 - (tf - 0.55) / 0.45 * 0.58 };
            car.ang = car.dodge_ang * taper;
            // air roll stays LIVE through a dodge (the half-flip's 180):
            // the hard assign above used to eat the roll key entirely
            // roll must not STACK onto the flip's own spin (it was adding
            // 8.5 rad/s on a side flip = 1.5-2 rotations): reduced
            // authority plus a hard cap on the total
            car.ang.z = (car.dodge_ang.z * taper + c.roll * 5.0).clamp(-8.0, 8.0);
            if car.dodge_timer <= 0.0 {
                // flip complete: shed the flip spin entirely (RL's
                // end-of-dodge damping) so held keys can't keep the tumble
                car.ang = Vec3::ZERO;
                car.dodge_ang = Vec3::ZERO;
            }
        } else {
            // free air control (reduced authority while scraping a surface), RL torque magnitudes
            let auth = if d <= clearance + 0.11 { 0.45 } else { 1.0 };
            let sr = cfg.car.steer_rate;
            car.ang.x += c.pitch * 12.5 * auth * dt;
            car.ang.y += c.yaw * 9.1 * sr * auth * dt;
            car.ang.z += c.roll * 12.0 * sr * auth * dt;
            if c.pitch.abs() < 0.1 {
                car.ang.x *= (-4.0 * dt).exp();
            }
            if c.yaw.abs() < 0.1 {
                car.ang.y *= (-4.0 * dt).exp();
            }
            if c.roll.abs() < 0.1 {
                car.ang.z *= (-4.0 * dt).exp();
            }
            let l = car.ang.length();
            if l > 5.5 {
                car.ang *= 5.5 / l;
            }
        }
        car.steer_vis *= (-8.0 * dt).exp();
        car.wheel_spin *= 1.0 - (0.4 * dt).min(1.0);

        // ---- landing assist: when falling toward a nearby surface with no
        // flip in progress, softly magnetize wheels-down and bleed off any
        // leftover tumble, so grounding after a flip stops fighting you.
        // Scaled by settings and eased off while the player is actively
        // steering the air (aerials stay yours).
        let assist = cfg.flip.land_assist;
        if assist > 0.01 && car.dodge_timer <= 0.0 && car.vel.dot(gdir) > 0.5 && d < 6.0 {
            let input_mag = c.pitch.abs().max(c.yaw.abs()).max(c.roll.abs());
            let hold = 1.0 - 0.65 * input_mag.min(1.0);
            let t = ((6.0 - d) / 6.0).clamp(0.0, 1.0) * assist * hold;
            let target_n = if n.dot(up_w) > 0.2 { n } else { up_w };
            let corr = Quat::from_rotation_arc(car.up(), target_n);
            let k = 1.0 - (-7.0 * t * dt).exp();
            car.rot = (Quat::IDENTITY.slerp(corr, k) * car.rot).normalize();
            let damp = (-9.0 * t * dt).exp();
            car.ang.x *= damp;
            car.ang.z *= damp;
        }

        // ---- flip reset: wheels-side contact gives the flip back ---------
        if cfg.flip.resets && car.reset_cd <= 0.0 && !car.has_dodge {
            let align_need = if cfg.flip.strict { 0.72 } else { 0.30 };
            let up = car.up();
            let underside = car.pos - up * (car.half_h * 0.8);
            let mut got: Option<Vec3> = None;
            // off the ball (the classic): underside pressed onto the ball
            let delta = car.pos - ball.pos;
            let dist = delta.length();
            if dist < ball.r + car.half_h + 0.22 && dist > 1e-4 && (delta / dist).dot(up) > align_need {
                got = Some(ball.pos + (delta / dist) * ball.r);
            }
            // off any world surface that isn't drivable ground
            if got.is_none() {
                let dw = world.sdf(underside);
                if dw < car.half_h * 0.65 {
                    let nw = world.normal(underside);
                    if nw.dot(up) > align_need && !(grounded) {
                        got = Some(underside - nw * dw);
                    }
                }
            }
            if let Some(p) = got {
                car.has_dodge = true;
                car.dodge_window = 1.45;
                car.reset_cd = 0.30;
                ev.flip_reset = Some(p);
            }
        }
    }

    // boost works everywhere; the tank obeys the boost-economy settings
    let tank = 100.0 * cfg.car.boost_max;
    if cfg.car.unlimited_boost {
        car.boost = tank;
    }
    if c.boost && car.boost > 0.0 {
        car.vel += car.forward() * BOOST_ACCEL * cfg.car.boost * dt;
        if !cfg.car.unlimited_boost {
            car.boost = (car.boost - BOOST_DRAIN * dt).max(0.0);
        }
    } else if cfg.car.boost_regen > 0.0 {
        car.boost = (car.boost + cfg.car.boost_regen * dt).min(tank);
    }
    car.boost = car.boost.min(tank);

    // integrate rotation (body-frame angular velocity)
    if car.ang.length_squared() > 1e-8 {
        car.rot = (car.rot * Quat::from_scaled_axis(car.ang * dt)).normalize();
    }

    // clamp + integrate position
    let sp = car.vel.length();
    car.supersonic = sp >= top - 0.4;
    if sp > top {
        car.vel *= top / sp;
    }
    car.pos += car.vel * dt;

    // collide nose / center / tail spheres against the world
    let f = car.forward();
    for off in [0.0f32, 0.8, -0.8] {
        let p = car.pos + f * (car.half_len * off);
        let d = world.sdf(p);
        let r = car.half_h;
        if d < r {
            let n = world.normal(p);
            car.pos += n * (r - d);
            let vn = car.vel.dot(n);
            if vn < 0.0 {
                car.vel -= n * vn * (1.0 + 0.05 * cfg.arena.bounce);
                // scrub a little tangential speed on impacts
                if !car.on_ground {
                    car.vel *= 0.985;
                }
            }
        }
    }

    // hard floor safety
    if car.pos.y < -60.0 {
        car.pos = Vec3::new(0.0, world.ground_h(0.0, 26.0) + car.half_h + 0.1, 26.0);
        car.vel = Vec3::ZERO;
    }
}

/// What a bot's car felt this step (the slice of StepEvents a brain needs).
#[derive(Default, Clone, Copy)]
pub struct BotStepInfo {
    pub landed: bool,
    pub flip_reset: bool,
    #[allow(dead_code)] // brains may key celebrations off it later
    pub jumped: bool,
}

/// Step one extra car (a bot) through the same car physics: gravity, ground
/// blend, jumps/flips, landing assist — everything except world scoring.
pub fn step_car_only(car: &mut Car, ball: &Ball, world: &World, c: &Controls, cfg: &Config, dt: f32) -> BotStepInfo {
    let mut scratch = StepEvents::default();
    step_car(car, ball, world, c, cfg, dt, &mut scratch);
    BotStepInfo {
        landed: scratch.landed.is_some(),
        flip_reset: scratch.flip_reset.is_some(),
        jumped: scratch.jumped,
    }
}

fn yaw_quat(q: Quat) -> Quat {
    let f = q * Vec3::NEG_Z;
    let yaw = f.x.atan2(-f.z);
    Quat::from_rotation_y(-yaw)
}

// --------------------------------------------------------------- ball ----
/// Public for the sim harness: the ball vs world, no car required.
pub fn step_ball(ball: &mut Ball, world: &World, cfg: &Config, dt: f32, ev: &mut StepEvents) {
    ball.pinch_cd = (ball.pinch_cd - dt).max(0.0);
    ball.vel += world.gravity_dir(ball.pos) * GRAV * cfg.arena.gravity * dt;
    if let Some(cv) = world.current(ball.pos) {
        ball.vel += (cv - ball.vel) * (1.0 - (-2.2 * dt).exp());
    }
    ball.vel *= 1.0 - (0.0303 * dt).min(0.5); // RL air drag
    ball.pos += ball.vel * dt;

    let d = world.sdf(ball.pos);
    if d < ball.r {
        let n = world.normal(ball.pos);
        ball.pos += n * (ball.r - d);
        let vn = ball.vel.dot(n);
        if vn < 0.0 {
            let vt = ball.vel - n * vn;
            let e = (0.6 * cfg.ball.bounce * cfg.arena.bounce).clamp(0.05, 0.97);
            if vn < -0.8 {
                if -vn > 2.0 {
                    ev.ball_bounce = Some((ball.pos, -vn));
                }
                ball.vel = vt * 0.714 - n * vn * e; // RL bounce, tuned restitution
                ev.bounces.push((ball.pos, n, -vn));
            } else {
                let roll = 1.8 * cfg.ball.friction * cfg.arena.friction;
                ball.vel = vt * (1.0 - (roll * dt).min(1.0)); // rolling friction
            }
            ball.ang = n.cross(ball.vel) / ball.r;
        }
    }

    let sp = ball.vel.length();
    if sp > BALL_MAX {
        ball.vel *= BALL_MAX / sp;
    }
    if ball.ang.length_squared() > 1e-8 {
        ball.rot = (Quat::from_scaled_axis(ball.ang * dt) * ball.rot).normalize();
    }
    if ball.pos.y < -60.0 {
        ball.pos = Vec3::new(0.0, ball.r + 6.0, 14.0);
        ball.vel = Vec3::ZERO;
    }
}

// ---------------------------------------------------- car <-> ball -------
pub fn collide_car_ball(car: &mut Car, ball: &mut Ball, cfg: &Config, who: u8, ev: &mut StepEvents) {
    // car as a capsule along its length, slightly above center (hood-biased)
    let f = car.forward();
    let up = car.up();
    let a = car.pos + f * (car.half_len * 0.78) + up * 0.03;
    let b = car.pos - f * (car.half_len * 0.78) + up * 0.03;
    let ab = b - a;
    let t = ((ball.pos - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
    let cp = a + ab * t;
    let delta = ball.pos - cp;
    let dist = delta.length();
    // the car is NOT a thin spine: width counts, and a car mid-dodge gets
    // a lunge margin — this is most of "I keep whiffing the ball"
    // height near stock (carry balance is tuned to it), width counts for
    // half its excess (side hits connect), and the flat margin is small —
    // the earlier max()+0.08 raised the ball's ride 11 cm and the striker
    // carry pipeline fell apart (air1 touches went to zero in the sim)
    let body = car.half_h * 1.12 + (car.half_w - car.half_h).max(0.0) * 0.5 + 0.04;
    let lunge = if car.dodge_timer > 0.0 { 0.14 } else { 0.0 };
    let radius = ball.r + body + lunge;
    if dist >= radius || dist < 1e-5 {
        return;
    }
    let n = delta / dist;
    let pen = radius - dist;
    ball.pos += n * pen * 0.85;
    car.pos -= n * pen * 0.15;

    let m_ball = M_BALL * cfg.ball.weight;
    let rel = ball.vel - car.vel;
    let vn = rel.dot(n);
    if vn < 0.0 {
        ev.touches.push((cp, -vn));
        ball.last_touch = who;
        // soft-touch canon: gentle contacts deaden (the ball settles instead
        // of pinging off the hood), hard hits keep full restitution
        let soft = ((-vn) / 8.0).clamp(0.0, 1.0);
        let e = (0.10 + 0.25 * soft) * cfg.ball.bounce.sqrt();
        let j = -(1.0 + e) * vn / (1.0 / M_CAR + 1.0 / m_ball);
        ball.vel += n * (j / m_ball);
        car.vel -= n * (j / M_CAR);

        // carry assist: a ball riding on top of the car gets a whisper of
        // horizontal speed-matching, which is what makes hood dribbles and
        // nose-tip air carries balanceable ("match the ball's speed")
        if n.y > 0.55 {
            let dvx = car.vel.x - ball.vel.x;
            let dvz = car.vel.z - ball.vel.z;
            ball.vel.x += dvx * 0.16;
            ball.vel.z += dvz * 0.16;
        }

        // the "psyonix impulse": extra push that scales with the hit, biased
        // horizontal so touches carry the ball forward instead of ballooning
        let mut dir = ball.pos - car.pos;
        dir.y *= 0.35;
        // hits go where you're DRIVING: blend a slice of car velocity into
        // the push direction so committed touches carry intent
        let dir = (dir.normalize_or(n) + car.vel * 0.025).normalize_or(n);
        let strength = (-vn).min(23.0) * 0.38 / cfg.ball.weight.sqrt();
        ball.vel += dir * strength;
        ball.ang += n.cross(rel) * 0.35 / ball.r;
    }
}
