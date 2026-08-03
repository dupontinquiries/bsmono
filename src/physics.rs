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
    /// where we were last step, for portal plane crossing
    pub prev: Vec3,
    /// brief lockout so an exit cannot immediately re-enter its own partner
    pub tp_cd: f32,
    /// world-space angular velocity INCLUDING the grounded steering yaw,
    /// which is applied straight to the rotation and never enters `ang`
    pub omega: Vec3,
    prev_rot: Quat,
    pub wheel_r: f32,
    /// drivetrain: an electric car pulls harder from a standstill
    pub drive: crate::model::Drive,
    /// HOVER CHASSIS. A hovercar has no tyres, so most of what the wheeled
    /// car does is wrong for it: it does not sit ON the ground, it does not
    /// grip sideways, and it does not strike the ball with its bodywork.
    pub hover: Option<Hover>,
    /// FLIGHT CHASSIS. A jet has no wheels to put down, so it is never
    /// grounded — which is precisely the state the aerial control block
    /// already handles. Setting this is most of what "flying" means here.
    pub flight: bool,
    /// ROBOT MODE. A bot on its feet does not dodge — it jumps again. The
    /// flip is spent on a second jump instead, driven by the boost tank,
    /// which is the thruster now rather than a nozzle on somebody's back.
    pub robot: bool,
    /// pad-collected charge, spent before the base reserve (hovercars)
    pub charge: f32,
    /// seconds until the pulse is available again
    pub pulse_t: f32,
    /// counts up while a pulse is firing, for the effect
    pub pulse_fire: f32,
    pub wheel_spin: f32,
    pub steer_vis: f32,
    // jump / dodge state
    jump_hold: f32,
    jbuf: f32, // jump input buffer: a press just before touchdown lands
    dodge_window: f32,
    has_dodge: bool,
    dodge_timer: f32,
    dodge_ang: Vec3,
    // RL-style scripted flip: a full 2-pi rotation about ONE cardinal body
    // axis, so a flip always ends at the orientation it started from
    dodge_axis: Vec3,
    dodge_start: Quat,
    dodge_len: f32,
    dodge_roll: f32,
    /// how much STEER went into choosing this dodge's direction
    dodge_steer: f32,
    dodge_turn: f32,
    dodge_cancel: bool,
    reset_cd: f32,
    pub supersonic: bool,
}

/// A hovercar's character, in five numbers.
#[derive(Clone, Copy)]
pub struct Hover {
    pub ride: f32,
    pub stiff: f32,
    /// lateral hold, 0..1 — a tyre is effectively 1, a cushion is not
    pub hold: f32,
    pub field_r: f32,
    pub field_k: f32,
    // ---- TWO-TIER THRUST ----
    // A hovercar with no lateral grip cannot change direction without
    // thrust, so an empty tank leaves it a passenger — far worse than for a
    // wheeled car, which can always steer. It therefore carries a small
    // BASE reserve that refills quickly and pushes gently, and spends
    // PAD-collected charge first at a higher output than a wheeled car gets.
    /// base tank size, in the same units as the wheeled car's 0..100
    pub base_tank: f32,
    /// base refill per second — high on purpose: it has to come back often
    pub base_regen: f32,
    /// thrust multiplier while running on the base reserve
    pub base_power: f32,
    /// thrust multiplier while running on collected charge
    pub charge_power: f32,
    // ---- PULSE ----
    /// seconds between pulses
    pub pulse_cd: f32,
    /// upward impulse it gives the car
    pub pulse_lift: f32,
    /// radius and strength of the omnidirectional blast
    pub pulse_r: f32,
    pub pulse_k: f32,
}

impl Default for Hover {
    fn default() -> Hover {
        Hover {
            ride: 0.55,
            stiff: 42.0,
            hold: 0.18,
            field_r: 3.2,
            field_k: 26.0,
            base_tank: 30.0,
            // Faster than it drains, deliberately. A hovercar that has to
            // WAIT for thrust is a hovercar that cannot steer, so the base
            // tier is designed to be back before you miss it.
            base_regen: 44.0,
            base_power: 0.55,
            charge_power: 1.25,
            pulse_cd: 1.6,
            pulse_lift: 7.5,
            pulse_r: 5.0,
            pulse_k: 15.0,
        }
    }
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
            drive: crate::model::Drive::Combustion,
            hover: None,
            flight: false,
            robot: false,
            charge: 0.0,
            pulse_t: 0.0,
            pulse_fire: 0.0,
            half_w: 0.32,
            susp: 0.0,
            prev: Vec3::ZERO,
            tp_cd: 0.0,
            omega: Vec3::ZERO,
            prev_rot: Quat::IDENTITY,
            wheel_spin: 0.0,
            steer_vis: 0.0,
            jump_hold: 0.0,
            jbuf: 0.0,
            dodge_window: 0.0,
            has_dodge: false,
            dodge_timer: 0.0,
            dodge_ang: Vec3::ZERO,
            dodge_axis: Vec3::X,
            dodge_start: Quat::IDENTITY,
            dodge_len: 0.62,
            dodge_roll: 0.0,
            dodge_steer: 0.0,
            dodge_turn: 0.0,
            dodge_cancel: false,
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
    /// Which cardinal axis the active flip turns about (flip sim).
    pub fn flip_axis_name(&self) -> &'static str {
        if self.dodge_axis.x.abs() > self.dodge_axis.z.abs() {
            if self.dodge_axis.x > 0.0 { "+X" } else { "-X" }
        } else if self.dodge_axis.z > 0.0 {
            "+Z"
        } else {
            "-Z"
        }
    }
    /// Is a flip currently playing? (flip sim)
    pub fn is_flipping(&self) -> bool {
        self.dodge_timer > 0.0
    }
    /// Is the dodge window still open? (flip sim)
    pub fn dodge_window_open(&self) -> bool {
        self.dodge_window > 0.0
    }
    /// The orientation the active flip began from (flip sim).
    pub fn dodge_start_pose(&self) -> Quat {
        self.dodge_start
    }
    /// The active flip's total duration (flip sim).
    pub fn dodge_len_for_sim(&self) -> f32 {
        self.dodge_len
    }
    /// How far the active flip has turned, radians (flip sim).
    pub fn flip_turn(&self) -> f32 {
        self.dodge_turn
    }
    /// Call after setting `rot` directly (a reset, a respawn, a portal).
    /// Angular velocity is derived from the rotation delta between frames,
    /// so a teleport that changes orientation instantly reads as an enormous
    /// spin for one frame — and now that flicks throw the ball with the
    /// car's spin, that one frame would fling the ball across the pitch.
    pub fn sync_rot(&mut self) {
        self.prev_rot = self.rot;
        self.omega = Vec3::ZERO;
    }

    /// Half width, for the flick harness.
    pub fn half_w(&self) -> f32 {
        self.half_w
    }
    /// Time left in the current dodge (flick harness).
    pub fn dodge_left(&self) -> f32 {
        self.dodge_timer
    }
    /// Angular speed, for the flick harness.
    pub fn omega_len(&self) -> f32 {
        self.omega.length()
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
        // land on the real surface, not the terrain height: raised decks
        // (DROPSHOT) used to spawn the car INSIDE the platform
        let clr = self.half_h + 0.05;
        let p = world.spawn_at(0.0, 26.0, clr);
        *self = Car {
            half_len: self.half_len,
            half_h: self.half_h,
            half_w: self.half_w,
            wheel_r: self.wheel_r,
            boost: 45.0,
            pos: p,
            ..Car::new()
        };
        self.prev_rot = self.rot;
        self.omega = Vec3::ZERO;
    }
}

pub struct Ball {
    pub pos: Vec3,
    pub vel: Vec3,
    pub rot: Quat,
    pub ang: Vec3,
    pub r: f32,
    pub last_touch: u8, // WHO_* code of the last car to touch the ball
    pub prev: Vec3,
    pub tp_cd: f32,
    pinch_cd: f32,
}

impl Ball {
    pub fn new() -> Ball {
        Ball { pos: Vec3::new(0.0, BALL_R + 4.0, 14.0), vel: Vec3::ZERO, rot: Quat::IDENTITY, ang: Vec3::ZERO, r: BALL_R, last_touch: WHO_NONE, pinch_cd: 0.0, prev: Vec3::ZERO, tp_cd: 0.0 }
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
    /// a hovercar's pulse fired here, with this radius and strength — the
    /// caller applies it to bots and peers, which physics cannot reach
    pub pulses: Vec<(Vec3, f32, f32)>,
    pub pinched: bool,
    pub blasted: bool,
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
    // arcade bumpers fire the ball on contact
    if let Some(imp) = world.bump_ball(ball.pos, dt) {
        ball.vel = imp;
        ball.ang *= 0.4;
    }
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
    // SHOCKWAVE: the detonation frame throws car, ball and every loose
    // world outward, falling off with distance (never inward, never NaN)
    if let Some((c, power)) = world.blast {
        let kick = |p: Vec3, mass: f32| -> Vec3 {
            let to = p - c;
            let d = to.length().max(2.0);
            let f = (power / d).min(70.0) * (12.0 / mass.max(1.0)).clamp(0.25, 1.6);
            to / d * f
        };
        car.vel += kick(car.pos, 12.0);
        ball.vel += kick(ball.pos, 3.0);
        for b in world.bodies.iter_mut() {
            let k = kick(b.pos, b.mass);
            b.vel += k;
        }
        ev.blasted = true;
    }
    step_bodies(world, car, ball, dt, cfg.arena.gravity);
    collide_car_ball(car, ball, cfg, WHO_PLAYER, ev);
    hover_field(car, ball, dt, c.boost, ev);
    // A hovercar's jump is the pulse: it has no wheels to push off with.
    if car.hover.is_some() && c.jump_pressed && hover_pulse(car, ball, ev) {
        ev.jumped = true;
    }

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
            // a hovercar's pads fill the CHARGE pool, not the base reserve:
            // that is what makes picking one up matter to it
            if car.hover.is_some() {
                car.charge = (car.charge + if p.big { 100.0 } else { 12.0 }).min(100.0 * cfg.car.boost_max);
            } else {
                car.boost = (car.boost + if p.big { 100.0 } else { 12.0 }).min(100.0 * cfg.car.boost_max);
            }
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
fn step_bodies(world: &mut World, car: &mut Car, ball: &mut Ball, dt: f32, cfg_gravity: f32) {
    if world.bodies.is_empty() {
        return;
    }
    let n = world.bodies.len();
    for i in 0..n {
        let (mut pos, mut vel, r, mass) = {
            let b = &world.bodies[i];
            (b.pos, b.vel, b.r, b.mass)
        };
        // free worlds are in the same field as everything else: they fall
        // toward whatever is nearest and heaviest, which is what makes the
        // vertical billiards break
        let (gd, gm) = world.gravity_field(pos);
        vel += gd * 6.5 * gm * 0.55 * dt;
        let bouncy = world.bodies[i].bouncy;
        if bouncy {
            // drifters fall, so they can land, squash and spring back
            vel += world.gravity_dir(pos) * GRAV * cfg_gravity * dt;
        }
        if !bouncy {
            vel *= (-world.bodies[i].drag * dt).exp(); // leaves flutter, rocks coast
        }
        pos += vel * dt;
        // world surfaces
        let d = world.sdf(pos);
        if d < r {
            let nw = world.normal(pos);
            pos += nw * (r - d);
            let vn = vel.dot(nw);
            if vn < 0.0 {
                // planets keep (almost) everything: the DVD logo never
                // stops. Everything else loses the usual 45%.
                let e = if bouncy { 0.985 } else { 0.55 };
                vel -= nw * vn * (1.0 + e);
                let b = &mut world.bodies[i];
                if bouncy {
                    // SQUASH: flatten along the impact normal, scaled by how
                    // hard it hit, then let the wobble spring it back out
                    b.squash = b.squash.max(((-vn) / 9.0).clamp(0.06, 0.42));
                    b.sq_dir = nw;
                    b.sq_t = 0.0;
                }
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
        // jelly wobble: a damped oscillation, so the squash overshoots
        // back past round before settling — that is the "bounce back"
        if world.bodies[i].squash > 0.0 {
            let b = &mut world.bodies[i];
            b.sq_t += dt;
            if b.sq_t > 1.4 {
                b.squash = 0.0;
                b.sq_t = 0.0;
            }
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
        let b = &world.bodies[i];
        let (shape, pos, r) = (b.shape, b.pos, b.r);
        // squash-and-stretch, volume-ish preserving: flatten along the hit
        // normal, bulge sideways, wobbling out through zero as it settles
        let (rot, sc) = if b.squash > 0.0 {
            let a = b.squash * (-3.4 * b.sq_t).exp() * (b.sq_t * 19.0).cos();
            (
                glam::Quat::from_rotation_arc(glam::Vec3::Y, b.sq_dir),
                glam::Vec3::new(r * (1.0 + a * 0.5), r * (1.0 - a), r * (1.0 + a * 0.5)),
            )
        } else {
            (glam::Quat::IDENTITY, glam::Vec3::splat(r))
        };
        let sh = &mut world.shapes[shape];
        sh.pos = pos;
        sh.inv_rot = rot.inverse();
        sh.mat = glam::Mat4::from_scale_rotation_translation(sc, rot, pos);
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
    // every goal winds the chess board tighter
    if !ev.goals.is_empty() {
        if let Some(ch) = world.chess.as_mut() {
            ch.fury = (ch.fury + 1.0).min(9.0);
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
    // ---- THE CUSHION ----
    // A wheeled car is held ON the surface; a hovercar is held ABOVE it by a
    // spring along the surface normal, damped so it settles rather than
    // bouncing. It runs before and independently of the grounded test,
    // because a hovercar is never "grounded" in the wheeled sense — the
    // cushion IS its ground.
    if let Some(h) = car.hover {
        let gap = world.sdf(car.pos);
        let want = car.half_h + h.ride;
        if gap < want * 3.0 {
            let n = world.normal(car.pos);
            let err = want - gap;
            let closing = -car.vel.dot(n);
            // A proportional spring alone balances gravity at an OFFSET —
            // every chassis floated a fixed distance below its ride height,
            // by exactly g/stiffness. Carrying the car's weight explicitly
            // leaves the spring to do only what it is for: correcting error.
            let push = err * h.stiff + closing * (h.stiff * 0.22).sqrt() * 2.0 + GRAV;
            car.vel += n * push.clamp(-30.0, 70.0) * dt;
        }
    }
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
    if (c.jump_pressed && car.hover.is_none()) {
        car.jbuf = 0.09;
    } else {
        car.jbuf = (car.jbuf - dt).max(0.0);
    }

    let (gdir, gmul) = world.gravity_field(car.pos);
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
    // A FLYER IS NEVER GROUNDED. Everything below that reads `grounded`
    // — the landing event, the wave dash, the upright snap, drive and grip
    // — is about wheels on a surface, and a jet has none. Gating it here
    // rather than at each site means flight reuses the aerial control path
    // (pitch/yaw/roll torque) exactly as an airborne car already does,
    // which is the whole trick: no second control scheme to write.
    let grounded = grounded && !car.flight;
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
    car.vel += gdir * GRAV * gmul * cfg.arena.gravity * dt;

    // ---- FLIGHT: hold the sky up, push along the nose ------------------
    //
    // Marking a jet airborne buys the aerial controls but not the altitude:
    // left alone it flies beautifully into the floor. So carry most of its
    // own weight (not all — LIFT < 1 means it still sinks with the throttle
    // shut, which is what makes the throttle mean something), and put the
    // thrust along the nose rather than along the ground plane, so pitching
    // up climbs. Drag gives it a terminal speed of its own instead of
    // inheriting the car's.
    if car.flight {
        const LIFT: f32 = 0.92;
        const THRUST: f32 = 26.0;
        const DRAG: f32 = 0.55;
        car.vel -= gdir * GRAV * gmul * cfg.arena.gravity * LIFT * dt;
        let t = c.throttle.max(0.0) + if c.boost { 0.8 } else { 0.0 };
        car.vel += car.forward() * t * THRUST * dt;
        car.vel *= 1.0 - (DRAG * dt).min(0.9);
    }
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

    let top = CAR_MAX * cfg.car.top_speed * car.drive.top();
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
        // A GROUNDED CAR HAS NO FREE ROTATION. Orientation comes from two
        // things only: the surface (the align above) and your steering (the
        // yaw below) — RL's direct input-to-rotation mapping. Pitch/roll
        // were merely DAMPED here, so leftover spin from a landing or a
        // bump kept turning the car for a quarter second, fighting the
        // align and stealing commanded yaw (a hard turn delivered 54% of
        // what the stick asked for; after a landing, 76%).
        car.ang = Vec3::ZERO;

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
            // (omega is measured after integration; see step_car's tail)
        } else {
            // (omega measured below)
        }
        car.steer_vis += (c.steer * 0.42 - car.steer_vis) * (1.0 - (-16.0 * dt).exp());

        // throttle / brake / coast
        // ELECTRIC TORQUE: full pull from zero, fading to parity with pace.
        // This is the whole character difference — an EV wins the first two
        // car-lengths off a kickoff and gives it back on a long straight.
        let speed_frac = (vf.abs() / drive_max.max(1.0)).clamp(0.0, 1.0);
        let acc = 16.5 * cfg.car.accel * car.drive.torque(speed_frac);
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
        // A HOVERCAR BARELY HOLDS: it keeps its momentum while the nose
        // points elsewhere, so you steer by pointing and thrusting.
        let grip = match car.hover {
            Some(h) => 11.0 * grip_k * h.hold,
            None if c.powerslide => 1.3 * grip_k.sqrt(),
            None => 11.0 * grip_k,
        };
        car.vel -= side * vs * (1.0 - (-grip * dt).exp());

        // keep the car pressed onto the surface
        let vn = car.vel.dot(n);
        // MOVING FRAME. Everything below — grip, braking, steering, the
        // suspension — is written for ground that stands still. On a moving
        // deck that same code was braking the car to a stop in WORLD space
        // while the deck slid out from under it. So we step into the
        // platform's frame, run the ordinary ground solve there, and step
        // back out at the end: riding a deck then feels exactly like driving
        // on the floor, and leaving one hands you its momentum for free.
        let plat_v = world
            .platform_vel(car.pos - n * (clearance * 0.9), clearance * 3.0)
            .unwrap_or(Vec3::ZERO);
        car.vel -= plat_v;

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
            // rebound damping was light enough that a landing bounced the
            // car clean off the ground again — you cannot dribble on a
            // trampoline. Compression stays soft; the way back is firm.
            let cdamp = if vn < 0.0 { 24.0 } else { 19.0 };
            if car.hover.is_none() {
                car.vel += n * (e * 58.0 - vn * cdamp) * dt;
            }
        } else {
            // stick to the surface: gentle over crests on the floor, strong
            // magnetic grip on walls and ceilings (the RL wall-drive feel)
            // stickier on a heavy world: the surface holds you through the
            // curve instead of letting you drift off the horizon
            let stick = if steep { 22.0 } else { 2.4 } * gmul.clamp(1.0, 3.2);
            if car.hover.is_none() {
                car.vel -= n * stick * dt;
            }
        }
        // PLATFORM RIDING: a car standing on a moving deck is carried by
        // it — position follows exactly (so a fast platform cannot slide out
        // from under you) and velocity is dragged toward the surface, which
        // is what lets you inherit its momentum when you jump off.
        let starget = ((clearance - d) / zone).clamp(0.0, 1.0);
        car.susp += (starget - car.susp) * (1.0 - (-14.0 * dt).exp());

        car.wheel_spin += vf / car.wheel_r.max(0.05) * dt;

        // first jump (a buffered press from just before touchdown counts);
        // wooting depth trims the impulse a touch for takeoff-pitch control
        if (c.jump_pressed && car.hover.is_none()) || car.jbuf > 0.0 {
            car.jbuf = 0.0;
            ev.jumped = true;
            let depth = 0.95 + 0.05 * c.jump_depth.clamp(0.0, 1.0);
            car.vel += n * 2.92 * cfg.car.jump * depth;
            car.jump_hold = 0.19;
            car.pos += n * 0.04;
            car.on_ground = false;
        }
        // ...and back out of the platform frame
        car.vel += plat_v;
        car.pos += plat_v * dt;
    } else {
        // A fast deck can outrun the frame in which contact is lost: keep
        // carrying while the car is still within a whisker of one, so a
        // 18 m/s carriage does not leave you standing in the air.
        let probe = car.pos - Vec3::Y * (car.half_h + 0.12);
        if let Some(pv) = world.platform_vel(probe, 0.9) {
            car.pos += pv * dt;
            let rel = car.vel - pv;
            car.vel -= (rel - Vec3::Y * rel.y) * (1.0 - (-8.0 * dt).exp()) * 0.6;
        }
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
                if dp < rim && car.hover.is_none() {
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

        // ---- ROBOT: the flip becomes a thruster-fed second jump ----------
        //
        // Reusing the dodge's own gate (has_dodge + its window) means the
        // second jump inherits everything already tuned about when a flip is
        // available: it is granted on landing, spent once, and restored by a
        // flip reset. The cost is boost, so the thrusters that push a vehicle
        // forward lift a robot instead — one tank, two meanings, and no extra
        // resource to explain.
        if car.robot && c.jump_pressed && car.has_dodge && car.dodge_window > 0.0 {
            const JUMP2_COST: f32 = 18.0;
            const JUMP2_IMPULSE: f32 = 7.4;
            if car.boost >= JUMP2_COST {
                car.boost -= JUMP2_COST;
                car.has_dodge = false;
                car.dodge_window = 0.0;
                car.jbuf = 0.0;
                let up = -world.gravity_dir(car.pos);
                // replace downward motion rather than adding to it, so a
                // late second jump still saves you
                let vd = car.vel.dot(up);
                car.vel += up * (JUMP2_IMPULSE - vd.min(JUMP2_IMPULSE * 0.5));
                car.ang = Vec3::ZERO;
                ev.jumped = true;
            }
        } else if (c.jump_pressed && car.hover.is_none()) && car.has_dodge && car.dodge_window > 0.0 {
            car.jbuf = 0.0; // an intentional dodge shouldn't also hop on land
            // dodge direction in body frame: steer right = +x, throttle fwd = -z
            let d2 = Vec3::new(c.steer, 0.0, -c.throttle);
            if d2.length() > 0.3 {
                // CARDINAL SNAP (Rocket League): any key combination sharing
                // a dominant direction flips in THAT axis — no half-diagonal
                // tumbles. Ties break toward pitch, as in RL.
                // DIAGONAL FLIPS (your tuning). RL dodges along the input
                // VECTOR, not a snapped cardinal — W+A is a diagonal flip,
                // not a left one. Normalising keeps the flip's power constant
                // regardless of how far the stick is pushed, while the
                // direction stays exactly what you asked for.
                let mut dn = Vec3::new(c.steer, 0.0, -c.throttle);
                dn = if dn.length_squared() > 1e-6 { dn.normalize() } else { Vec3::Z };
                // flip: burst of body-frame spin + a velocity kick along the dodge
                // toned from 8.4: full-rate spin read too violent, and the
                // FLIP SPIN dial rides on top for +-50% in 5% steps
                car.dodge_axis = Vec3::Y.cross(dn).normalize_or(Vec3::X);
                car.dodge_ang = car.dodge_axis * 7.4 * cfg.flip.strength.sqrt() * cfg.flip.spin;
                // one full rotation, always: duration scales with FLIP SPIN
                car.dodge_len = (0.76 / cfg.flip.spin.max(0.3)).clamp(0.38, 1.60);
                car.dodge_timer = car.dodge_len;
                car.dodge_start = car.rot;
                car.dodge_roll = 0.0;
                car.dodge_steer = c.steer.abs().min(1.0);
                car.dodge_turn = 0.0;
                car.dodge_cancel = false;
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
            if car.dodge_timer <= 0.0 && !car.dodge_cancel {
                // exact close-out: land on the start orientation with no
                // drift from the eased integration
                car.rot = (car.dodge_start * Quat::from_rotation_z(car.dodge_roll)).normalize();
                car.dodge_ang = Vec3::ZERO;
                car.ang = Vec3::ZERO;
            }
            // flip cancel: hold the opposite pitch to kill the rotation
            // (front flip + hold back = the speedflip/cancel family)
            if cfg.flip.cancel && c.pitch * car.dodge_ang.x < -3.0 {
                car.dodge_ang.x = 0.0;
                car.dodge_cancel = true; // freeze the rotation where it is
                if c.roll.abs() > 0.2 {
                    car.dodge_timer = car.dodge_timer.min(0.16);
                }
            }
            // SCRIPTED FLIP: drive the orientation directly from the start
            // pose through exactly one turn about the cardinal axis, eased
            // out. Because the total is 2-pi, the flip lands on the SAME
            // orientation it began with — wheels-down starts finish
            // wheels-down, air starts finish level.
            if !car.dodge_cancel {
                let p = 1.0 - (car.dodge_timer / car.dodge_len.max(0.01)).clamp(0.0, 1.0);
                // Rotation profile, measured decile by decile: the old curve
                // put 30% of the turn in the first 10% of the time (~1100
                // deg/s) and 90% of it in the first half, so the flip
                // lunged and then appeared to stop dead. This blend of a
                // smoothstep and a mild ease-out peaks at 13% per decile,
                // starts decisively without snapping, and is STILL turning
                // at the end (3%) instead of asymptoting into a freeze.
                let inv = 1.0 - p;
                let smooth = p * p * (3.0 - 2.0 * p);
                let mild = 1.0 - inv * inv.sqrt();
                let e = 0.5 * smooth + 0.5 * mild;
                car.dodge_turn = std::f32::consts::TAU * e;
            }
            // air roll layers ON TOP (the half-flip's 180) without stealing
            // from the flip's own rotation
            // ONE INPUT, ONE ROTATION. With powerslide held, steer is also
            // the air-roll axis — so a forward-right flip spent the same
            // stick twice: once choosing the dodge's diagonal direction and
            // again as barrel roll layered on top, which over a 0.78 s dodge
            // is another 220 degrees. It read as flipping twice.
            //
            // The roll passthrough is now scaled down by however much steer
            // went into the dodge itself. A diagonal dodge consumed the
            // steer axis and gets no roll from it; a straight forward or
            // back dodge did not, so it keeps the full passthrough — which
            // is what the half-flip needs.
            let roll_share = (1.0 - car.dodge_steer).clamp(0.0, 1.0);
            car.dodge_roll += c.roll * 5.0 * dt * roll_share;
            car.rot = (car.dodge_start
                * Quat::from_axis_angle(car.dodge_axis, car.dodge_turn)
                * Quat::from_rotation_z(car.dodge_roll))
            .normalize();
            car.ang = Vec3::ZERO; // orientation is scripted; no integration

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
    if let Some(h) = car.hover {
        // TWO TIERS. Collected charge is spent first and pushes harder than
        // a wheeled car's boost; underneath it a small base reserve refills
        // fast and pushes gently. The point is that a hovercar is never
        // completely without thrust — with no lateral grip, an empty tank
        // would leave it unable to change direction at all, which is a much
        // worse place to be than a wheeled car with an empty tank.
        car.pulse_t = (car.pulse_t - dt).max(0.0);
        car.pulse_fire = (car.pulse_fire - dt * 4.0).max(0.0);
        if c.boost {
            if car.charge > 0.0 {
                car.vel += car.forward() * BOOST_ACCEL * cfg.car.boost * h.charge_power * dt;
                if !cfg.car.unlimited_boost {
                    car.charge = (car.charge - BOOST_DRAIN * dt).max(0.0);
                }
            } else if car.boost > 0.0 {
                car.vel += car.forward() * BOOST_ACCEL * cfg.car.boost * h.base_power * dt;
                if !cfg.car.unlimited_boost {
                    car.boost = (car.boost - BOOST_DRAIN * dt).max(0.0);
                }
            }
        } else {
            // the base reserve comes back quickly whether or not the config
            // gives wheeled cars any regen at all
            car.boost = (car.boost + h.base_regen * dt).min(h.base_tank);
        }
        car.boost = car.boost.min(h.base_tank);
        car.charge = car.charge.min(tank);
    } else if c.boost && car.boost > 0.0 {
        car.vel += car.forward() * BOOST_ACCEL * cfg.car.boost * dt;
        if !cfg.car.unlimited_boost {
            car.boost = (car.boost - BOOST_DRAIN * dt).max(0.0);
        }
    } else if cfg.car.boost_regen > 0.0 {
        car.boost = (car.boost + cfg.car.boost_regen * dt).min(tank);
    }
    if car.hover.is_none() {
        car.boost = car.boost.min(tank);
    }

    // integrate rotation (body-frame angular velocity)
    if !car.on_ground {
        // (omega measured after the rotation integration)
    }
    portal_car(car, world, dt);
    if car.ang.length_squared() > 1e-8 {
        car.rot = (car.rot * Quat::from_scaled_axis(car.ang * dt)).normalize();
    }
    // MEASURE the turn actually taken this frame — UNCONDITIONALLY. A
    // scripted flip drives the quaternion directly and leaves `ang` at
    // zero, so this used to sit inside the `ang != 0` guard and never ran
    // during the exact manoeuvre that throws the ball: omega read stale
    // and no flick could work.
    {
        let dq = (car.rot * car.prev_rot.inverse()).normalize();
        let (ax, mut angle) = dq.to_axis_angle();
        if angle > std::f32::consts::PI {
            angle -= std::f32::consts::TAU;
        }
        // a wrapped quaternion delta can read as a huge spin (the flick sim
        // caught 144 rad/s on a side dodge); a car cannot turn faster than
        // its own flip, so clamp to something physical
        car.omega = (ax * (angle / dt.max(1e-5))).clamp_length_max(14.0);
        car.prev_rot = car.rot;
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
/// Teleport a car through any portal its motion crossed this step. Position,
/// velocity, orientation and spin all take the same rotation, so momentum is
/// preserved exactly and a fall becomes a launch.
pub fn portal_car(car: &mut Car, world: &World, dt: f32) -> bool {
    car.tp_cd = (car.tp_cd - dt).max(0.0);
    if world.portals.is_empty() || car.tp_cd > 0.0 {
        car.prev = car.pos;
        return false;
    }
    if let Some(i) = world.portal_crossed(car.prev, car.pos) {
        let r = world.portal_rot(i);
        car.pos = world.portal_point(i, car.pos);
        car.vel = r * car.vel;
        car.rot = (r * car.rot).normalize();
        car.sync_rot();
        car.ang = r * car.ang;
        // Clear of the exit face — and if the far side is tight (a moving
        // mouth can swing into its own mount), push out along the surface
        // normal until we are actually in open air. Getting parked inside
        // the drum was the worst bug in TURNSTILE.
        car.pos += (r * Vec3::Z) * 1.2;
        for _ in 0..8 {
            let d = world.sdf(car.pos);
            if d > car.half_h + 0.15 {
                break;
            }
            car.pos += world.normal(car.pos) * (car.half_h + 0.25 - d).max(0.15);
        }
        car.tp_cd = 0.12;
        car.prev = car.pos;
        return true;
    }
    car.prev = car.pos;
    false
}

pub fn step_ball(ball: &mut Ball, world: &World, cfg: &Config, dt: f32, ev: &mut StepEvents) {
    ball.pinch_cd = (ball.pinch_cd - dt).max(0.0);
    // the ball goes through portals too — half the play depends on it
    ball.tp_cd = (ball.tp_cd - dt).max(0.0);
    if !world.portals.is_empty() && ball.tp_cd <= 0.0 {
        if let Some(i) = world.portal_crossed(ball.prev, ball.pos) {
            let r = world.portal_rot(i);
            ball.pos = world.portal_point(i, ball.pos);
            ball.vel = r * ball.vel;
            ball.ang = r * ball.ang;
            ball.pos += (r * Vec3::Z) * 0.2;
            ball.tp_cd = 0.12;
        }
    }
    ball.prev = ball.pos;
    let (bdir, bmul) = world.gravity_field(ball.pos);
    ball.vel += bdir * GRAV * bmul * cfg.arena.gravity * dt;
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
                // ---- RIGID-SPHERE CONTACT (the Rocket League bounce) ----
                // Spin used to be a SLAVE to velocity: recomputed from it
                // after every bounce, so it could never change where the ball
                // went. In RL the angular velocity is its own state and the
                // two trade through friction at the contact point, which is
                // where curve, backspin and topspin all come from.
                //
                //   contact point sits at -n*r from the centre
                //   its material velocity is v + w x r_c
                //   friction acts on the SLIP of that point, Coulomb-limited
                //   by the normal impulse, and it torques the ball in return.
                let rc = -n * ball.r;
                let u = ball.vel + ball.ang.cross(rc);
                let slip = u - n * u.dot(n);
                let jn = -(1.0 + e) * vn; // per unit mass
                // solid sphere: I = 0.4 m r^2, so the tangential effective
                // mass at the contact is m / 3.5
                let inertia = 0.4 * ball.r * ball.r;
                let m_t = 1.0 / (1.0 + ball.r * ball.r / inertia);
                let want = -slip * m_t;
                let mu_b = 0.36 * cfg.ball.friction;
                let jt = if want.length() > mu_b * jn {
                    want.normalize_or(Vec3::ZERO) * (mu_b * jn)
                } else {
                    want
                };
                ball.vel += n * jn + jt;
                ball.ang += rc.cross(jt) / inertia;
                ev.bounces.push((ball.pos, n, -vn));
            } else {
                // Rolling friction turned out NOT to be the shot killer (a
                // driven ball spends its first seconds airborne, and the sim
                // measured identical travel from 0.42 to 1.8). It IS what
                // lets a ball settle for a dribble, so it stays high — 1.5
                // scored best across the bot air-dribble runs.
                // ---- RESTING / ROLLING CONTACT ----
                // The same slip model, run continuously: a ball dropped with
                // topspin drives itself forward, a sliding ball spins up
                // until it rolls, and once it IS rolling the slip is zero so
                // nothing fights it — that is why a rolling ball keeps going.
                let rc = -n * ball.r;
                let u = ball.vel + ball.ang.cross(rc);
                let slip = u - n * u.dot(n);
                let inertia = 0.4 * ball.r * ball.r;
                let m_t = 1.0 / (1.0 + ball.r * ball.r / inertia);
                // available friction from the weight it is resting under
                // Coulomb friction from the ball's own weight. RL's ball
                // friction is low (~0.35) and its spin caps at 6 rad/s, so a
                // ball faster than about 5.5 m/s SLIDES rather than rolls —
                // true in RL as well. At 0.7 that slide was a brake that
                // stopped a 12 m/s ball dead in two seconds.
                let load = GRAV * cfg.arena.gravity * dt;
                let mu_b = 0.33 * cfg.ball.friction * cfg.arena.friction;
                let want = -slip * m_t;
                let jt = if want.length() > mu_b * load {
                    want.normalize_or(Vec3::ZERO) * (mu_b * load)
                } else {
                    want
                };
                ball.vel += jt;
                ball.ang += rc.cross(jt) / inertia;
                // rolling resistance: a real ball on turf does eventually
                // stop, but far more gently than a velocity haircut
                let rr = 0.02 * cfg.ball.friction * cfg.arena.friction;
                let vt2 = ball.vel - n * ball.vel.dot(n);
                ball.vel -= vt2 * (rr * dt).min(0.4);
                ball.ang *= 1.0 - (rr * 0.5 * dt).min(0.3);
                // rest ON the surface: cancel what little downward velocity
                // gravity added this step. (This line used to subtract HALF
                // of a negative — an upward ratchet that launched a rolling
                // ball a metre into the air every second.)
                let vn2 = ball.vel.dot(n);
                if vn2 < 0.0 {
                    ball.vel -= n * vn2;
                }
            }
        }
    }

    let sp = ball.vel.length();
    if sp > BALL_MAX {
        ball.vel *= BALL_MAX / sp;
    }
    // RL caps ball spin at 6 rad/s — without it a hard pinch can leave the
    // ball spinning fast enough to drive itself across the pitch
    let asp = ball.ang.length();
    if asp > 6.0 {
        ball.ang *= 6.0 / asp;
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
/// THE PULSE — a hovercar's jump, and its only way to fight.
///
/// A hovercar has no wheels to push off with, so it cannot jump or flip. In
/// exchange it gets a shove: an omnidirectional blast that lifts the car and
/// pushes the ball AND anything else nearby away from it. That single move
/// covers the two things the chassis was missing — it can leave the ground
/// to meet a ball, and it can refuse to be knocked off one. Used on a ball
/// already moving, the blast adds to the shot rather than replacing it,
/// which is why a redirect is the thing these are for.
///
/// It is on a cooldown rather than a resource, so it is always eventually
/// available — the same reasoning as the base thrust reserve.
pub fn hover_pulse(car: &mut Car, ball: &mut Ball, ev: &mut StepEvents) -> bool {
    let Some(h) = car.hover else { return false };
    if car.pulse_t > 0.0 {
        return false;
    }
    car.pulse_t = h.pulse_cd;
    car.pulse_fire = 1.0;
    // lift: straight up in the WORLD, so it works whatever the car is doing
    car.vel += Vec3::Y * h.pulse_lift;
    // and the blast, falling off to nothing at the rim
    let to = ball.pos - car.pos;
    let d = to.length();
    if d < h.pulse_r + ball.r && d > 1e-4 {
        let f = 1.0 - (d / (h.pulse_r + ball.r)).clamp(0.0, 1.0);
        let push = h.pulse_k * f * f;
        ball.vel += (to / d) * push;
        ev.touches.push((ball.pos - (to / d) * ball.r, push));
    }
    ev.pulses.push((car.pos, h.pulse_r, h.pulse_k));
    true
}

/// THE REPULSOR FIELD. A hovercar has no bodywork worth hitting a ball
/// with — it is held up by pressure and it moves the ball the same way. A
/// cone in front of the nose pushes the ball away, hardest straight ahead
/// and falling off with distance, so the ball is never STRUCK, it is HERDED:
/// held ahead of you at range, steered round a corner untouched. Power has
/// to come from a PULSE rather than from driving hard.
pub fn hover_field(car: &mut Car, ball: &mut Ball, dt: f32, boost: bool, ev: &mut StepEvents) {
    let Some(h) = car.hover else { return };
    let to = ball.pos - car.pos;
    let dist = to.length();
    if dist > h.field_r + ball.r || dist < 1e-4 {
        return;
    }
    let dir = to / dist;
    let aim = dir.dot(car.forward()).max(0.0);
    let cone = aim * aim;
    if cone < 0.02 {
        return;
    }
    let reach = (1.0 - (dist / (h.field_r + ball.r)).clamp(0.0, 1.0)).powi(2);
    let mut push = h.field_k * cone * reach;
    if boost {
        push *= 3.4; // the pulse: the hovercar's shot
    }
    ball.vel += dir * push * dt;
    // the ball presses back, so the field is not a free tractor beam
    car.vel -= dir * push * dt * 0.16;
    ball.ang += dir.cross(Vec3::Y) * push * dt * 0.05;
    if push > 4.0 {
        ev.touches.push((ball.pos - dir * ball.r, push.min(40.0)));
    }
}

pub fn collide_car_ball(car: &mut Car, ball: &mut Ball, cfg: &Config, who: u8, ev: &mut StepEvents) {
    // THE HITBOX IS A BOX. A capsule is round on top, so a ball resting on
    // it had barely a hand's width of contact before separating — which is
    // why flips never caught the ball and no flick in this game worked.
    // Rocket League's flicks come from a boxy hull whose nose and roof
    // corner sweep UP into the ball. Closest point on an oriented box:
    let half = Vec3::new(
        car.half_w.max(0.18),
        car.half_h.max(0.12),
        car.half_len.max(0.30),
    );
    let local = car.rot.inverse() * (ball.pos - car.pos);
    let clamped = local.clamp(-half, half);
    let cp = car.pos + car.rot * clamped;
    let delta = ball.pos - cp;
    let dist = delta.length();
    // the car is NOT a thin spine: width counts, and a car mid-dodge gets
    // a lunge margin — this is most of "I keep whiffing the ball"
    // height near stock (carry balance is tuned to it), width counts for
    // half its excess (side hits connect), and the flat margin is small —
    // the earlier max()+0.08 raised the ball's ride 11 cm and the striker
    // carry pipeline fell apart (air1 touches went to zero in the sim)
    // skin over the box: a little give, plus a lunge margin mid-dodge
    // contact skin: 6 cm held the ball visibly clear of the shell, so a
    // carried ball looked like it was floating. 2 cm reads as resting on it.
    let body = 0.02;
    let lunge = if car.dodge_timer > 0.0 { 0.45 } else { 0.0 };
    let radius = ball.r + body + lunge;
    if dist >= radius || dist < 1e-5 {
        return;
    }
    let n = delta / dist;
    let pen = radius - dist;
    ball.pos += n * pen * 0.85;
    car.pos -= n * pen * 0.15;

    let m_ball = M_BALL * cfg.ball.weight;
    // the patch under the ball moves with v + omega x r — a swinging hood
    // is the whole mechanism behind every flick
    let v_surf = car.vel + car.omega.cross((cp + n * body) - car.pos);
    let rel = ball.vel - v_surf;
    let vn = rel.dot(n);
    if vn < 0.0 {
        ev.touches.push((cp, -vn));
        ball.last_touch = who;
        // soft-touch canon: gentle contacts deaden (the ball settles instead
        // of pinging off the hood), hard hits keep full restitution
        let soft = ((-vn) / 8.0).clamp(0.0, 1.0);
        let e = (0.10 + 0.25 * soft) * cfg.ball.bounce.sqrt();
        // A ball RESTING on the hood must not bounce off it. At full
        // restitution the jump alone threw the ball clear — the flick sim
        // measured zero contact frames during the flip, which means every
        // "flick" was really just the jump. Gentle contacts are now nearly
        // inelastic, so the ball rides the car up and is still there when
        // the car rotates out from under it; hard hits keep their bounce.
        let e = e * (0.18 + 0.82 * soft);
        // A car meets a grounded ball from below-behind, so the contact
        // normal points steeply UP and every shot ballooned. Hard hits now
        // drive THROUGH: the impulse normal flattens toward horizontal as
        // the hit gets stronger, while soft touches keep the true normal so
        // dribbles and carries behave exactly as before.
        let ni = if soft > 0.05 {
            let flat = Vec3::new(n.x, n.y * (1.0 - 0.55 * soft), n.z).normalize_or(n);
            n.lerp(flat, soft).normalize_or(n)
        } else {
            n
        };
        let j = -(1.0 + e) * vn / (1.0 / M_CAR + 1.0 / m_ball);
        ball.vel += ni * (j / m_ball);
        car.vel -= ni * (j / M_CAR);

        // carry assist: a ball riding on top of the car gets a whisper of
        // horizontal speed-matching, which is what makes hood dribbles and
        // nose-tip air carries balanceable ("match the ball's speed")
        // ...but ONLY for dribble-speed contacts. On a power shot this was
        // dragging the ball back toward the car's own speed every contact
        // frame — the assist that makes carries balanceable was quietly
        // capping every shot.
        if n.y > 0.30 && -vn < 6.0 {
            let w = 0.012 * (1.0 - (-vn / 6.0).clamp(0.0, 1.0));
            let dvx = car.vel.x - ball.vel.x;
            let dvz = car.vel.z - ball.vel.z;
            ball.vel.x += dvx * w;
            ball.vel.z += dvz * w;
            // CONTACT FRICTION — the missing piece. A ball resting on a
            // moving surface is dragged by it: the contact point's velocity
            // includes the car's YAW, so turning the car carries the ball
            // around with you instead of leaving it behind. (Grounded yaw is
            // applied straight to the rotation, so it lives in `omega`, not
            // in `ang`.)
            let rel_c = ball.vel - v_surf;
            let tang = rel_c - n * rel_c.dot(n);
            // FRICTION, not glue: killing 75% of the slip every frame welded
            // the ball to the roof — no roll-off, no feathering, and any
            // flick was absorbed before it could leave. A capped fraction
            // lets the ball live on the hood while still being flickable.
            // Real dribbling is PRECARIOUS: the ball must be able to roll
            // off if you turn too hard or too long. Friction that held it
            // through anything made carries a formality rather than a skill.
            // GROUND dribbling is precarious — hold a turn too long and the ball
            // rolls off, which is the skill. An AIR carry is a different move:
            // the ball rides the nose through small touches, and stripping the
            // grip there kills air dribbles outright (the bots lost two thirds
            // of their air touches when this was one number for both).
            // A dribble is a BALANCING ACT, not a magnet. Ground grip is
            // now light enough that the ball keeps its own momentum: boost
            // ahead and it rolls back down the hood toward you, brake and it
            // runs away in front, and you recover it by scooping under from
            // the far side. Air carries keep more grip — that touch is the
            // whole move and there is no ground to catch a mistake.
            // Barely any grip on the ground: the ball keeps its own
            // momentum and WILL leave the roof when you turn or brake. Air
            // carries keep theirs — that touch is the whole move.
            let mu = if car.on_ground { 0.018 } else { 0.30 };
            ball.vel -= (tang * mu).clamp_length_max(if car.on_ground { 0.35 } else { 1.4 });
            // and the hood is dished, so the ball rolls back to the middle
            let side = car.rot * Vec3::X;
            let off = (ball.pos - car.pos).dot(side);
            let reach = car.half_w.max(0.25) + ball.r;
            let hold = (1.0 - (off.abs() / reach).clamp(0.0, 1.0)).powi(2);
            // the dish is a HINT toward the centre line, not a magnet: it
            // only bites near the sweet spot, so a ball already sliding off
            // keeps going
            // the dish only whispers now: enough that a centred ball stays
            // centred, not enough to drag a sliding one back
            // THE DISH is what makes a dribble a skill rather than a coin
            // flip. Near the sweet spot the hood cradles the ball firmly, so
            // small errors self-correct and you can hold a line; past about
            // two thirds of the way out the cradle falls away to nothing and
            // the ball is gone. That gradient IS the balance point — uniform
            // grip (what this was) gives you either a magnet or a marble,
            // and neither of those is dribbling.
            let f = (off.abs() / reach).clamp(0.0, 1.0);
            let well = if f < 0.66 { (1.0 - f / 0.66).powi(2) } else { 0.0 };
            // NO DISH. A hood that pulls the ball back to its centre line
            // is what made carries feel magnetic. Keeping the ball is your
            // steering now, not the car's shape.
            // fore/aft is deliberately looser: that axis is where you PLAY —
            // push the ball out front, boost past it, catch it up again
            let fwd = car.forward();
            let foff = (ball.pos - car.pos).dot(fwd);
            let freach = car.half_len + ball.r;
            let ff = (foff.abs() / freach).clamp(0.0, 1.0);
            let fwell = if ff < 0.7 { (1.0 - ff / 0.7).powi(2) } else { 0.0 };
            ball.vel -= fwd * (foff.signum() * foff.abs().min(freach) * fwell * 0.9 * w);
        }

        // the "psyonix impulse": extra push that scales with the hit, biased
        // horizontal so touches carry the ball forward instead of ballooning
        // A SHOT SHOULD GO WHERE THE CAR POINTS. The contact normal off a
        // box corner points diagonally, and blending raw car velocity in
        // skewed it further — that combination is the "weird angles". The
        // harder the hit, the more the push direction leaves the contact
        // normal behind and follows the car's own heading; soft touches
        // still use the true normal, which is what dribbles need.
        let mut dir_n = ball.pos - car.pos;
        dir_n.y *= 0.35;
        let dir_n = dir_n.normalize_or(n);
        let aim = {
            let f = car.forward();
            let flat = Vec3::new(f.x, f.y * 0.35, f.z).normalize_or(dir_n);
            flat
        };
        // THE FLICK. Flattening the push direction stops shots ballooning,
        // but it also flattened the one touch whose whole point is vertical:
        // a flick throws the ball because the car ROTATES out from under it,
        // and the direction that throw wants is the contact point's own
        // surface velocity. The faster the car is spinning, the more the
        // push follows that instead of the flattened line — so a backflip
        // lofts the ball and a forward flick stays low, which is exactly the
        // difference between the two in Rocket League.
        // THE THROW DIRECTION MUST EARN ITS WEIGHT.
        //
        // `v_surf - car.vel` is how fast the hood is moving under the ball
        // from the car's own rotation. Near the rotation axis that vector is
        // almost zero — and the DIRECTION of an almost-zero vector is
        // arbitrary, so normalising it hands back noise. Blending toward it
        // on spin alone meant a fast flip could throw the ball somewhere
        // unrelated to the contact, which is exactly "sometimes it goes in a
        // weird direction".
        //
        // Weight it by the surface speed as well: a hood that is genuinely
        // sweeping throws the ball, a contact sitting on the axis does not.
        let surf = v_surf - car.vel;
        let surf_speed = surf.length();
        let spin = (car.omega.length() / 7.0).clamp(0.0, 1.0);
        let throw_w = spin * (surf_speed / 3.0).clamp(0.0, 1.0);
        let throw_dir = surf.normalize_or(Vec3::ZERO);
        let dir = dir_n
            .lerp(aim, 0.65 * soft)
            .normalize_or(dir_n)
            .lerp(throw_dir, 0.62 * throw_w)
            .normalize_or(dir_n);
        let strength = (-vn).min(23.0) * 0.38 / cfg.ball.weight.sqrt();
        // The extra "psyonix" push on top of the elastic impulse was doing
        // most of the work, which is why touches felt like blasts. At a tenth
        // it shapes the hit instead of dominating it. (Your tuning.)
        // A flat tenth killed power shots along with the blasting: a soft
        // touch and a committed 22 m/s strike got the same treatment. Scale
        // it with the hit — dribble touches stay gentle, real shots punch.
        ball.vel += dir * strength * (0.06 + 0.55 * soft);
        // ...and the hood's own motion carries the ball with it. Without
        // this a flick is only ever as strong as the collision, when in
        // truth the car is throwing the ball, not hitting it.
        if throw_w > 0.05 {
            let carry = surf_speed.min(9.0);
            ball.vel += throw_dir * carry * 0.85 * throw_w;
        }
        // SPIN: a gentle touch rolls the ball (dribbles need that), a hard
        // hit drives THROUGH it. Spin used to scale up with the impact,
        // which is what made shots knuckle off at odd angles.
        let spin_w = 0.30 * (1.0 - soft);
        ball.ang += n.cross(rel) * spin_w / ball.r;
        ball.ang += car.omega * 0.12 * (1.0 - soft);
    }
}
