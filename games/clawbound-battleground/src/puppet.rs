//! puppet — what a cat's body does about the world it is standing in.
//!
//! `anim` picks a gait and `creature-rig` poses boxes, and both are blind:
//! neither knows there is a root under the left forepaw, a fern overhead, a
//! river round the ankles, or a rock the cat is about to land on. This layer
//! is the one that looks.
//!
//! ## The seam is [`Probe`]
//!
//! Four questions about a point — how high is the ground, is there
//! something to climb, how hidden is it, how deep is the water. That is the
//! whole of what this module knows about a world, and it is the part that
//! will actually vary: today it is `consts` + `Blockers`, and an AI-authored
//! map (`mapgen`) answers the same four questions from different data. The
//! tests answer them from closures.
//!
//! ## Presentation, not simulation
//!
//! A `Puppet` reads sim state and never writes it. Climbing over a root
//! raises the DRAWN cat, not `GodUnit::pos`, so hit tests, replays and bot
//! pathing are untouched. The one thing that flows back is
//! [`PuppetEvent`] — rustles and droplets for `vfx` to draw.
//!
//! ## The invariant worth testing
//!
//! On the ground, every sole sits on whatever is under it. Crouching,
//! gathering, landing and climbing all move the body and must all fold the
//! legs by exactly the amount that keeps that true.
//! `tests::soles_stay_on_the_surface_in_every_grounded_state` checks it for
//! all of them at once, because each was individually easy to get wrong.

use creature_rig::{Combat, MotionInput};
use glam::Vec3;

use crate::anim::{Anim, Build, Gait};
use crate::rigged::{BodyAdjust, LegAdjust, QuadRig};

/// What the puppet may ask of the world.
pub trait Probe {
    /// Terrain height.
    fn ground(&self, x: f32, z: f32) -> f32;
    /// Top of something walkable standing ON the terrain here — a root, a
    /// low slab — in world Y. `None` on open ground.
    fn step_top(&self, x: f32, z: f32) -> Option<f32>;
    /// Concealment, `0..=1`.
    fn cover(&self, x: f32, z: f32) -> f32;
    /// Water depth, world units.
    fn water(&self, x: f32, z: f32) -> f32;
}

/// Where a leap is in its arc, as the sim reports it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LeapView {
    /// Coiling. `t` runs 0→1.
    Gather { t: f32 },
    /// Airborne. `climb` is vertical speed over horizontal speed, so the
    /// body can follow the arc without this module knowing the arc.
    Flight { t: f32, climb: f32 },
    /// Absorbing the landing. `perched`: came down on something narrow.
    Land { t: f32, perched: bool },
}

/// How this ancestor's basic attack reads on the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrikeStyle {
    /// Alternating forepaw slash.
    Swipe,
    /// Ranged: the head snaps, the tail cracks.
    Snap,
}

/// Body language for a cast, by what the ability DOES rather than by slot,
/// so two ancestors with a dash in different slots move alike and an
/// ancestor's four abilities do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastStyle {
    /// One forepaw raised and slammed.
    Stomp,
    /// Coil and spring (dashes, pounces).
    Spring,
    /// Back arched, tail up (novas, wards, roars).
    Arch,
    /// Rear onto the hind legs and come down (ultimates).
    Rear,
    /// Head thrown forward (beams, bolts, breath).
    Spit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PuppetInput {
    pub pos: Vec3,
    pub yaw: f32,
    pub hp01: f32,
    /// The sim's attack pulse: jumps to ~1 on a swing and decays.
    pub atk_pulse: f32,
    pub strike_style: StrikeStyle,
    pub leap: Option<LeapView>,
    /// Standing on something it jumped onto.
    pub perched: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PuppetEvent {
    /// Pushed through foliage.
    Rustle { pos: Vec3, strength: f32 },
    /// Shaking water off.
    Droplets { pos: Vec3 },
    /// A forepaw came down hard (stomp / rear landing).
    PawSlam { pos: Vec3 },
}

/// Everything the draw path needs for one cat, one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PuppetPose {
    pub motion: MotionInput,
    pub legs: [LegAdjust; 4],
    pub body: BodyAdjust,
    /// World Y added to the drawn cat (climbing over roots).
    pub lift: f32,
}

const STRIKE_SECS: f32 = 0.30;
const SHAKE_SECS: f32 = 0.85;
const RUSTLE_EVERY: f32 = 0.35;
/// Fold beyond this and a voxel leg reads as a stump.
const MAX_FOLD: f32 = 0.7;
const MAX_EXTEND: f32 = -0.3;

#[derive(Debug, Clone)]
pub struct Puppet {
    pub anim: Anim,
    pub combat: Combat,
    last_pos: Option<Vec3>,
    last_yaw: f32,
    last_hp: f32,
    last_pulse: f32,
    speed: f32,
    turn: f32,
    /// Smoothed per-foot surface height relative to the terrain under the body.
    foot_h: [f32; 4],
    rise: f32,
    crouch: f32,
    strike_t: f32,
    strike_left: bool,
    cast: Option<(CastStyle, f32, f32)>, // style, elapsed, duration
    cast_slammed: bool,
    in_cover: bool,
    rustle_t: f32,
    wet: f32,
    shake_t: f32,
    idle_t: f32,
}

impl Puppet {
    pub fn new(build: Build) -> Puppet {
        Puppet {
            anim: Anim::new(build),
            combat: Combat::new(),
            last_pos: None,
            last_yaw: 0.0,
            last_hp: 1.0,
            last_pulse: 0.0,
            speed: 0.0,
            turn: 0.0,
            foot_h: [0.0; 4],
            rise: 0.0,
            crouch: 0.0,
            strike_t: f32::INFINITY,
            strike_left: false,
            cast: None,
            cast_slammed: false,
            in_cover: false,
            rustle_t: 0.0,
            wet: 0.0,
            shake_t: f32::INFINITY,
            idle_t: 0.0,
        }
    }

    pub fn begin_cast(&mut self, style: CastStyle) {
        let secs = if style == CastStyle::Rear { 0.75 } else { 0.45 };
        self.cast = Some((style, 0.0, secs));
        self.cast_slammed = false;
        self.combat.begin_cast(secs);
    }

    pub fn speed(&self) -> f32 {
        self.speed
    }
    pub fn crouch(&self) -> f32 {
        self.crouch
    }
    pub fn is_shaking(&self) -> bool {
        self.shake_t < SHAKE_SECS
    }

    /// Advance one frame. `rig` supplies the hip positions to probe under.
    pub fn advance(&mut self, dt: f32, inp: &PuppetInput, rig: &QuadRig, world: &dyn Probe, events: &mut Vec<PuppetEvent>) {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.1) } else { 0.0 };
        if dt <= 0.0 {
            return;
        }
        // Speed is MEASURED, not read from input: a dash, a knockback and a
        // push-out all move the cat and all have to move its legs.
        let pos = if inp.pos.is_finite() { inp.pos } else { self.last_pos.unwrap_or(Vec3::ZERO) };
        let raw_speed = match self.last_pos {
            Some(p) => Vec3::new(pos.x - p.x, 0.0, pos.z - p.z).length() / dt,
            None => 0.0,
        };
        // A respawn teleports; that is not a 900 u/s sprint.
        let raw_speed = if raw_speed > 60.0 { 0.0 } else { raw_speed };
        self.speed += (raw_speed - self.speed) * (dt * 10.0).min(1.0);
        let dyaw = wrap_pi(inp.yaw - self.last_yaw);
        self.turn += ((dyaw / dt / 6.0).clamp(-1.0, 1.0) - self.turn) * (dt * 8.0).min(1.0);
        self.last_pos = Some(pos);
        self.last_yaw = inp.yaw;

        let airborne = matches!(inp.leap, Some(LeapView::Flight { .. }));
        self.anim.advance(dt, self.speed, airborne);
        self.idle_t = if self.speed < 0.3 { self.idle_t + dt } else { 0.0 };

        // Took damage: flinch away from where we are facing (the sim does
        // not say who hit us, and "recoil backwards" is the honest default).
        if inp.hp01 < self.last_hp - 0.004 {
            let back = [-inp.yaw.cos(), -inp.yaw.sin()];
            self.combat.hit(back, ((self.last_hp - inp.hp01) * 6.0).clamp(0.15, 1.0), inp.hp01);
        }
        self.last_hp = inp.hp01;
        self.combat.tick(dt);

        // Attack: rising edge only. The pulse stays high for a quarter of a
        // second and must not restart the swing every frame it does.
        if inp.atk_pulse > self.last_pulse + 0.3 {
            self.strike_t = 0.0;
            self.strike_left = !self.strike_left;
        }
        self.last_pulse = inp.atk_pulse;
        self.strike_t += dt;

        if let Some((style, t, dur)) = &mut self.cast {
            *t += dt;
            let p = *t / *dur;
            if !self.cast_slammed && matches!(style, CastStyle::Stomp | CastStyle::Rear) && p >= slam_at(*style) {
                self.cast_slammed = true;
                let f = Vec3::new(inp.yaw.cos(), 0.0, inp.yaw.sin());
                events.push(PuppetEvent::PawSlam { pos: pos + f * 0.7 });
            }
            if *t >= *dur {
                self.cast = None;
            }
        }

        // ── what is under each foot ───────────────────────────────────
        let fwd = Vec3::new(inp.yaw.cos(), 0.0, inp.yaw.sin());
        let right = Vec3::new(-inp.yaw.sin(), 0.0, inp.yaw.cos());
        let base = world.ground(pos.x, pos.z);
        let grounded = !airborne && !inp.perched;
        for i in 0..4 {
            let hip = rig.hip(i);
            // Look a little ahead of the hip: a paw reaches for where it is
            // going to land, which is what makes it rise BEFORE the root.
            let p = pos + fwd * (hip[0] + self.speed.min(8.0) * 0.06) + right * hip[2];
            let want = if grounded {
                let g = world.ground(p.x, p.z);
                let top = world.step_top(p.x, p.z).unwrap_or(g).max(g);
                finite_or(top - base, 0.0).clamp(-0.6, 2.5)
            } else {
                0.0
            };
            // Up fast, down slow: a paw snaps onto a root and eases off it.
            let rate = if want > self.foot_h[i] { 22.0 } else { 10.0 };
            self.foot_h[i] += (want - self.foot_h[i]) * (dt * rate).min(1.0);
        }
        let mean = self.foot_h.iter().sum::<f32>() * 0.25;
        self.rise += (mean - self.rise) * (dt * 9.0).min(1.0);

        // ── foliage ───────────────────────────────────────────────────
        let cover = if grounded { finite_or(world.cover(pos.x, pos.z), 0.0).clamp(0.0, 1.0) } else { 0.0 };
        // A cat at a dead run does not stalk; it bursts through.
        let want_crouch = cover * (1.0 - (self.speed / self.anim.build.trot_to.min(12.0)).clamp(0.0, 1.0) * 0.7);
        self.crouch += (want_crouch - self.crouch) * (dt * 6.0).min(1.0);
        let now_in = cover > 0.3;
        self.rustle_t -= dt;
        if now_in && (!self.in_cover || (self.speed > 1.0 && self.rustle_t <= 0.0)) {
            let strength = if self.in_cover { (self.speed / 8.0).clamp(0.2, 1.0) } else { 1.0 };
            events.push(PuppetEvent::Rustle { pos, strength });
            self.rustle_t = RUSTLE_EVERY;
        }
        self.in_cover = now_in;

        // ── water ─────────────────────────────────────────────────────
        let depth = finite_or(world.water(pos.x, pos.z), 0.0).max(0.0);
        if depth > 0.25 {
            self.wet = 1.0;
        } else if depth < 0.05 && self.wet > 0.5 && !airborne {
            // Out, and soaked: shake. Once.
            self.wet = 0.0;
            self.shake_t = 0.0;
            events.push(PuppetEvent::Droplets { pos });
        }
        self.shake_t += dt;
    }

    /// The pose for this frame.
    pub fn pose(&self, inp: &PuppetInput, rig: &QuadRig) -> PuppetPose {
        let leg_len = (rig.hip(0)[1] - rig.sole(0)).max(0.05);
        let moving = self.speed > 0.3;
        let (state, progress) = self.combat.state(moving);
        let mut m = self.anim.motion_input(self.turn, (0.0, 0.0), 0.0, state, progress);
        self.combat.apply(&mut m, moving);
        let mut legs = [LegAdjust::default(); 4];
        let mut body = BodyAdjust::default();
        let time = self.anim.time;
        // Extra body drop each state asks for, summed, then paid for by the
        // legs in ONE place at the end so the soles invariant holds.
        let mut drop = 0.0f32;
        // Which paws are on the ground. A planted leg stays VERTICAL under
        // a tilting body and is folded to reach its surface; a free one is
        // posed by whoever freed it.
        let mut planted = [true; 4];

        // Idle life: the tail never quite stops, and a bored cat looks about.
        body.tail_lash = (time * 0.9).sin() * 0.18 * (1.0 - (self.speed / 4.0).min(1.0));
        if self.idle_t > 2.0 {
            let k = ((self.idle_t - 2.0) * 0.8).min(1.0);
            m.gaze_yaw += (time * 0.37).sin() * 0.45 * k;
        }

        // Stalk. Not a gait — a posture (docs/WARRIORS_MECHANICS.md): chest
        // down, stride short, tail low and STILL.
        let c = self.crouch;
        if c > 0.01 {
            drop += c * 0.30 * leg_len;
            m.gaze_pitch -= 0.22 * c;
            m.swing_scale *= 1.0 - 0.35 * c;
            body.tail_lift -= 0.35 * c;
            body.tail_lash *= 1.0 - c;
            body.pitch -= 0.06 * c;
            if c > 0.5 && moving && self.anim.gait() != Gait::Bound {
                m.leg_phases = Some(Gait::Pace.leg_phases());
            }
            for l in &mut legs {
                l.splay += 0.16 * c;
            }
        }

        // Wading: high, deliberate steps.
        if self.wet > 0.5 && moving {
            m.swing_scale *= 1.25;
            m.leg_phases = Some(Gait::Walk.leg_phases());
        }
        // Shaking dry: a fast decaying roll, tail whipping against it.
        if self.shake_t < SHAKE_SECS {
            let env = 1.0 - self.shake_t / SHAKE_SECS;
            let w = (self.shake_t * 38.0).sin() * env;
            body.roll += 0.24 * w;
            body.tail_lash -= 0.7 * w;
            m.gaze_yaw += 0.3 * w;
        }

        // Basic attack.
        if self.strike_t < STRIKE_SECS {
            let p = self.strike_t / STRIKE_SECS;
            match inp.strike_style {
                StrikeStyle::Swipe => {
                    // FL is index 0 and is the LEFT paw.
                    let paw = if self.strike_left { 0 } else { 1 };
                    planted[paw] = false;
                    let (pitch, fold) = swipe_curve(p);
                    legs[paw].pitch += pitch;
                    legs[paw].fold += fold;
                    legs[paw].splay -= 0.35 * bump(p, 0.45, 0.25); // across the body
                    body.roll += if self.strike_left { -0.10 } else { 0.10 } * bump(p, 0.45, 0.3);
                    m.strike = p;
                }
                StrikeStyle::Snap => {
                    m.gaze_pitch += 0.35 * bump(p, 0.2, 0.2) - 0.30 * bump(p, 0.5, 0.2);
                    body.tail_lash += 0.9 * bump(p, 0.45, 0.2) * if self.strike_left { 1.0 } else { -1.0 };
                    body.tail_lift += 0.5 * bump(p, 0.4, 0.3);
                    m.strike = p * 0.6;
                }
            }
        }

        // Cast.
        if let Some((style, t, dur)) = self.cast {
            let p = (t / dur).clamp(0.0, 1.0);
            match style {
                CastStyle::Stomp => {
                    let up = bump(p, 0.3, 0.3);
                    planted[1] = up < 0.02;
                    legs[1].pitch += 0.9 * up;
                    legs[1].fold += 0.45 * up;
                    body.pitch += 0.10 * up;
                    m.impact = m.impact.max(bump(p, 0.62, 0.12));
                }
                CastStyle::Spring => {
                    drop += 0.32 * leg_len * bump(p, 0.35, 0.35);
                    body.pitch += -0.12 * bump(p, 0.3, 0.3) + 0.22 * bump(p, 0.75, 0.2);
                    body.tail_lash += (time * 30.0).sin() * 0.2 * bump(p, 0.3, 0.3);
                }
                CastStyle::Arch => {
                    let k = bump(p, 0.45, 0.45);
                    drop -= 0.10 * leg_len * k;
                    body.tail_lift += 1.0 * k;
                    m.gaze_pitch -= 0.25 * k;
                    for l in &mut legs {
                        l.splay += 0.12 * k;
                    }
                }
                CastStyle::Rear => {
                    let k = bump(p, 0.4, 0.4);
                    body.pitch += 0.80 * k;
                    for l in &mut legs[0..2] {
                        l.pitch += 0.7 * k;
                        l.fold += 0.30 * k;
                    }
                    body.tail_lift -= 0.4 * k;
                    m.impact = m.impact.max(bump(p, 0.85, 0.12));
                    // Up on the hind legs: they stay planted and the body
                    // turns about THEIR hips.
                    if k > 0.02 {
                        planted[0] = false;
                        planted[1] = false;
                        body.pivot = [rig.hip(2)[0], rig.hip(2)[1]];
                    }
                }
                CastStyle::Spit => {
                    m.gaze_pitch += 0.30 * bump(p, 0.25, 0.25) - 0.35 * bump(p, 0.55, 0.2);
                    body.pitch += -0.10 * bump(p, 0.55, 0.25);
                    drop += 0.12 * leg_len * bump(p, 0.55, 0.25);
                    body.tail_lift += 0.6 * bump(p, 0.5, 0.4);
                }
            }
        }

        // Leap.
        match inp.leap {
            Some(LeapView::Gather { t }) => {
                let k = ease(t);
                drop += 0.38 * leg_len * k;
                body.pitch -= 0.14 * k;
                // The wiggle. It is the whole tell of a cat about to jump.
                body.tail_lash += (time * 30.0).sin() * 0.30 * k;
                body.roll += (time * 30.0).sin() * 0.035 * k;
            }
            Some(LeapView::Flight { t, climb }) => {
                planted = [false; 4];
                let t = t.clamp(0.0, 1.0);
                body.pitch += finite_or(climb, 0.0).atan().clamp(-0.75, 0.75);
                // Stretched out going up, gathered coming down.
                let reach = 1.0 - smooth(t, 0.35, 0.8);
                for l in &mut legs[0..2] {
                    l.pitch += 1.05 * reach + 0.45 * (1.0 - reach);
                    l.fold += 0.05 * reach + 0.10 * (1.0 - reach);
                }
                for l in &mut legs[2..4] {
                    l.pitch += -0.95 * reach + 0.55 * (1.0 - reach);
                    l.fold += 0.0 * reach + 0.45 * (1.0 - reach);
                }
                body.arch += 0.10 * reach - 0.06 * (1.0 - reach);
                body.tail_lift += 0.35;
                m.leg_phases = Some(Gait::Bound.leg_phases());
                m.swing_scale = 0.0; // the gait must not fight the pose
            }
            Some(LeapView::Land { t, perched }) => {
                let k = (1.0 - t.clamp(0.0, 1.0)).powi(2);
                drop += 0.34 * leg_len * k;
                body.pitch -= 0.16 * k;
                m.impact = m.impact.max(k);
                if perched {
                    // Narrow footing: the body rocks and the tail goes the
                    // other way, harder, because that is what a tail is for.
                    let wob = (t * 14.0).sin() * (1.0 - t).max(0.0);
                    body.roll += 0.09 * wob;
                    body.tail_lash -= 0.30 * wob;
                }
            }
            None => {}
        }
        if inp.perched && inp.leap.is_none() {
            body.tail_lash += (time * 1.3).sin() * 0.22;
            body.tail_lift -= 0.25;
        }

        // ── pay for the body's height with the legs ───────────────────
        let lift = self.rise;
        if planted.iter().any(|p| *p) && !inp.perched {
            let front = (self.foot_h[0] + self.foot_h[1]) * 0.5;
            let hind = (self.foot_h[2] + self.foot_h[3]) * 0.5;
            let left = (self.foot_h[0] + self.foot_h[2]) * 0.5;
            let rgt = (self.foot_h[1] + self.foot_h[3]) * 0.5;
            let span = (rig.hip(0)[0] - rig.hip(2)[0]).abs().max(0.2);
            let width = (rig.hip(1)[2] - rig.hip(0)[2]).abs().max(0.1);
            body.pitch += ((front - hind) / span).atan() * 0.8;
            body.roll += ((left - rgt) / width).atan() * 0.5; // right-side-down positive
        }
        for (i, l) in legs.iter_mut().enumerate() {
            if !planted[i] {
                continue;
            }
            // The body sits at `rise - drop` and tilting moved this hip by
            // its lever arm about the pivot. The TOTAL tilt, not just the
            // terrain's share: a stalk or a gather pitches the chest down
            // too, and using the terrain tilt alone here put the forepaws
            // 8 cm through the floor.
            let hip = rig.hip(i);
            let tilt = (hip[0] - body.pivot[0]) * body.pitch.sin() - hip[2] * body.roll.sin();
            let need = (self.foot_h[i] - (self.rise - drop) - tilt) / leg_len;
            l.fold = (l.fold + need).clamp(MAX_EXTEND, MAX_FOLD);
            // Stay vertical in the world: undo the body's pitch at the hip.
            // Without this a rearing cat's hind legs swing forward with it
            // and a cat on a slope stands on the edges of its paws.
            l.pitch -= body.pitch;
        }
        body.drop += drop;

        // Never hand the renderer a NaN: one bad probe would erase the cat.
        for l in &mut legs {
            l.pitch = finite_or(l.pitch, 0.0);
            l.fold = finite_or(l.fold, 0.0);
            l.splay = finite_or(l.splay, 0.0);
        }
        body.drop = finite_or(body.drop, 0.0);
        body.pitch = finite_or(body.pitch, 0.0).clamp(-1.2, 1.2);
        body.roll = finite_or(body.roll, 0.0).clamp(-0.8, 0.8);
        body.tail_lift = finite_or(body.tail_lift, 0.0).clamp(-0.9, 1.4);
        body.tail_lash = finite_or(body.tail_lash, 0.0).clamp(-1.3, 1.3);
        body.arch = finite_or(body.arch, 0.0);
        PuppetPose { motion: m, legs, body, lift: finite_or(lift, 0.0) }
    }
}

/// The cast of puppets, one per ancestor slot, plus what they emitted.
///
/// Lives in `Vfx` because that is the presentation state both shells (the
/// windowed app and the headless harness) already thread through update and
/// draw; the sim never sees it.
#[derive(Debug, Clone, Default)]
pub struct Troupe {
    pub puppets: Vec<Puppet>,
    pub events: Vec<PuppetEvent>,
    /// `Vfx::time` at the last advance, so `draw` can derive its own dt.
    pub last_time: f32,
}

impl Troupe {
    pub fn ensure(&mut self, n: usize) {
        while self.puppets.len() < n {
            self.puppets.push(Puppet::new(Build::feline()));
        }
    }
    pub fn on_cast(&mut self, god: usize, style: CastStyle) {
        self.ensure(god + 1);
        self.puppets[god].begin_cast(style);
    }
}

/// Body language from what an ability DOES. An ultimate rears unless it is
/// itself a movement, because rearing up mid-dash reads as a stumble.
pub fn cast_style(arch: &crate::gods::Archetype, ult: bool) -> CastStyle {
    use crate::gods::Archetype as A;
    match arch {
        A::Dash { .. } | A::Blink { .. } => CastStyle::Spring,
        _ if ult => CastStyle::Rear,
        A::Bolt { .. } | A::Volley { .. } | A::Beam { .. } => CastStyle::Spit,
        A::Lob { .. } | A::Ring { .. } | A::Pull { .. } => CastStyle::Stomp,
        A::Nova { .. } | A::Storm { .. } | A::Shield { .. } | A::Heal { .. } | A::Buff { .. } => CastStyle::Arch,
    }
}

/// When a Stomp/Rear's paw hits the floor, as cast progress.
fn slam_at(style: CastStyle) -> f32 {
    if style == CastStyle::Rear { 0.82 } else { 0.6 }
}

/// Forepaw slash: drawn back and up, thrown forward and across, recovered.
fn swipe_curve(p: f32) -> (f32, f32) {
    let wind = bump(p, 0.15, 0.18);
    let slash = bump(p, 0.5, 0.22);
    (-0.55 * wind + 1.15 * slash, 0.40 * wind + 0.18 * slash)
}

/// A smooth bump of height 1 centred on `c` with half-width `w`, 0 outside.
fn bump(p: f32, c: f32, w: f32) -> f32 {
    let x = ((p - c) / w).clamp(-1.0, 1.0);
    let k = 1.0 - x * x;
    k * k
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn smooth(x: f32, a: f32, b: f32) -> f32 {
    ease((x - a) / (b - a))
}

fn finite_or(x: f32, d: f32) -> f32 {
    if x.is_finite() { x } else { d }
}

fn wrap_pi(mut a: f32) -> f32 {
    if !a.is_finite() {
        return 0.0;
    }
    while a > std::f32::consts::PI {
        a -= std::f32::consts::TAU;
    }
    while a < -std::f32::consts::PI {
        a += std::f32::consts::TAU;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    struct World<G: Fn(f32, f32) -> Option<f32>, C: Fn(f32, f32) -> f32, W: Fn(f32, f32) -> f32> {
        step: G,
        cover: C,
        water: W,
    }
    impl<G: Fn(f32, f32) -> Option<f32>, C: Fn(f32, f32) -> f32, W: Fn(f32, f32) -> f32> Probe for World<G, C, W> {
        fn ground(&self, _: f32, _: f32) -> f32 { 0.0 }
        fn step_top(&self, x: f32, z: f32) -> Option<f32> { (self.step)(x, z) }
        fn cover(&self, x: f32, z: f32) -> f32 { (self.cover)(x, z) }
        fn water(&self, x: f32, z: f32) -> f32 { (self.water)(x, z) }
    }
    fn open() -> impl Probe {
        World { step: |_, _| None, cover: |_, _| 0.0, water: |_, _| 0.0 }
    }

    fn rig() -> QuadRig {
        let leg = |x: f32, z: f32| ([x, -0.7, z], [0.1, 0.5, 0.08]);
        QuadRig::new(
            Vec3::new(0.0, 1.2, 0.0), [0.9, 0.37, 0.35],
            leg(0.55, -0.3), leg(0.55, 0.3), leg(-0.5, -0.3), leg(-0.5, 0.3),
            ([1.05, 0.4, 0.0], [0.27, 0.25, 0.25]),
            ([-1.1, 0.15, 0.0], [0.22, 0.07, 0.07]),
        )
    }
    fn input(pos: Vec3) -> PuppetInput {
        PuppetInput { pos, yaw: 0.0, hp01: 1.0, atk_pulse: 0.0, strike_style: StrikeStyle::Swipe, leap: None, perched: false }
    }
    fn settle(p: &mut Puppet, inp: &PuppetInput, r: &QuadRig, w: &dyn Probe, secs: f32) -> Vec<PuppetEvent> {
        let mut ev = Vec::new();
        for _ in 0..(secs * 60.0) as usize {
            p.advance(1.0 / 60.0, inp, r, w, &mut ev);
        }
        ev
    }
    /// Sole height above the terrain for leg `i`, from the pose alone —
    /// small-angle, which is what the pose itself assumes.
    fn sole(pose: &PuppetPose, r: &QuadRig, i: usize) -> f32 {
        let len = r.hip(i)[1] - r.sole(i);
        let hip = r.hip(i);
        let tilt = (hip[0] - pose.body.pivot[0]) * pose.body.pitch.sin() - hip[2] * pose.body.roll.sin();
        pose.lift - pose.body.drop + tilt + pose.legs[i].fold * len
    }

    /// yaw 0 faces +X and right is +Z, so the front-LEFT paw is at +x, -z.
    #[test]
    fn a_root_under_one_paw_lifts_that_paw_and_tilts_the_body() {
        let r = rig();
        let w = World { step: |x, z| (x > 0.3 && z < 0.0).then_some(0.4), cover: |_, _| 0.0, water: |_, _| 0.0 };
        let mut p = Puppet::new(Build::feline());
        let inp = input(Vec3::ZERO);
        settle(&mut p, &inp, &r, &w, 1.0);
        let pose = p.pose(&inp, &r);
        let f: Vec<f32> = pose.legs.iter().map(|l| l.fold).collect();
        // HR is the closest rival, and rightly: the body tilts toward it, so
        // its hip drops and it has to fold a little too.
        assert!(f[0] > f[1] + 0.1 && f[0] > f[2] + 0.1 && f[0] > f[3] + 0.1, "FL folds most, it is the one on the root: {f:?}");
        assert!(f[1] < 0.0, "FR is on the far side of the tilt and has to REACH: {f:?}");
        assert!(pose.body.pitch > 0.02, "front is higher, nose comes up: {}", pose.body.pitch);
        assert!(pose.body.roll > 0.02, "LEFT is higher, so the right side is down: {}", pose.body.roll);
        assert!((0.05..0.2).contains(&pose.lift), "body rides a quarter of one paw's rise: {}", pose.lift);
    }

    /// The invariant. Each of these states moves the body; none may leave a
    /// paw in the air or under the floor.
    #[test]
    fn soles_stay_on_the_surface_in_every_grounded_state() {
        let r = rig();
        let root = World { step: |x, _| (x > 0.2).then_some(0.35), cover: |_, _| 0.0, water: |_, _| 0.0 };
        let fern = World { step: |_, _| None, cover: |_, _| 0.9, water: |_, _| 0.0 };
        let flat = open();
        let gather = PuppetInput { leap: Some(LeapView::Gather { t: 1.0 }), ..input(Vec3::ZERO) };
        let land = PuppetInput { leap: Some(LeapView::Land { t: 0.1, perched: false }), ..input(Vec3::ZERO) };
        let cases: [(&str, &dyn Probe, PuppetInput, [f32; 4]); 4] = [
            ("root under the forepaws", &root, input(Vec3::ZERO), [0.35, 0.35, 0.0, 0.0]),
            ("stalking in a fern", &fern, input(Vec3::ZERO), [0.0; 4]),
            ("gathering to leap", &flat, gather, [0.0; 4]),
            ("absorbing a landing", &flat, land, [0.0; 4]),
        ];
        let mut checked = 0;
        for (name, world, inp, want) in cases {
            let mut p = Puppet::new(Build::feline());
            settle(&mut p, &inp, &r, world, 1.5);
            let pose = p.pose(&inp, &r);
            assert!(pose.body.drop.abs() > 0.02 || pose.lift > 0.02, "{name}: the body did not move, so this proves nothing");
            for i in 0..4 {
                let s = sole(&pose, &r, i);
                assert!((s - want[i]).abs() < 0.03, "{name}: leg {i} sole at {s}, surface at {}", want[i]);
                checked += 1;
            }
        }
        assert_eq!(checked, 16);
    }

    #[test]
    fn foliage_means_a_stalk_and_one_rustle_on_entry() {
        let r = rig();
        let w = World { step: |_, _| None, cover: |x, _| if x > 5.0 { 0.8 } else { 0.0 }, water: |_, _| 0.0 };
        let mut p = Puppet::new(Build::feline());
        assert!(settle(&mut p, &input(Vec3::ZERO), &r, &w, 0.5).is_empty(), "open ground is silent");
        assert!(p.crouch() < 0.01);
        let inside = input(Vec3::new(6.0, 0.0, 0.0));
        // Standing still inside: exactly one rustle, from the entry.
        p.last_pos = Some(inside.pos);
        let ev = settle(&mut p, &inside, &r, &w, 1.0);
        let rustles = ev.iter().filter(|e| matches!(e, PuppetEvent::Rustle { .. })).count();
        assert_eq!(rustles, 1, "entry rustles once; standing still must not keep rustling");
        assert!((0.6..=0.85).contains(&p.crouch()), "crouch tracks cover: {}", p.crouch());
        let pose = p.pose(&inside, &r);
        assert!(pose.body.drop > 0.15 && pose.body.tail_lift < -0.1, "chest down, tail low");
    }

    #[test]
    fn leaving_the_river_shakes_once() {
        let r = rig();
        let w = World { step: |_, _| None, cover: |_, _| 0.0, water: |x, _| if x < 0.0 { 0.8 } else { 0.0 } };
        let mut p = Puppet::new(Build::feline());
        let wet = input(Vec3::new(-2.0, 0.0, 0.0));
        assert!(settle(&mut p, &wet, &r, &w, 0.5).iter().all(|e| !matches!(e, PuppetEvent::Droplets { .. })));
        let dry = input(Vec3::new(2.0, 0.0, 0.0));
        p.last_pos = Some(dry.pos);
        let ev = settle(&mut p, &dry, &r, &w, 0.2);
        assert_eq!(ev.iter().filter(|e| matches!(e, PuppetEvent::Droplets { .. })).count(), 1);
        assert!(p.is_shaking());
        let rolls: Vec<f32> = (0..12).map(|_| { settle(&mut p, &dry, &r, &w, 0.02); p.pose(&dry, &r).body.roll }).collect();
        assert!(rolls.iter().any(|x| *x > 0.03) && rolls.iter().any(|x| *x < -0.03), "a shake goes both ways: {rolls:?}");
        settle(&mut p, &dry, &r, &w, 1.5);
        assert!(!p.is_shaking() && p.pose(&dry, &r).body.roll.abs() < 1e-3, "and it ends");
    }

    #[test]
    fn a_held_attack_pulse_swings_once_and_paws_alternate() {
        let r = rig();
        let w = open();
        let mut p = Puppet::new(Build::feline());
        let mut ev = Vec::new();
        let mut which = Vec::new();
        for swing in 0..3 {
            for f in 0..24 {
                // The sim's pulse: 1.0 then decaying at 4/s.
                let pulse = (1.0 - f as f32 / 60.0 * 4.0).max(0.0);
                let inp = PuppetInput { atk_pulse: pulse, ..input(Vec3::ZERO) };
                p.advance(1.0 / 60.0, &inp, &r, &w, &mut ev);
                if f == 9 {
                    let pose = p.pose(&inp, &r);
                    let (l, rr) = (pose.legs[0].pitch, pose.legs[1].pitch);
                    assert!(l.abs().max(rr.abs()) > 0.5, "swing {swing}: mid-slash a paw must be thrown");
                    assert!(l.abs().min(rr.abs()) < 0.05, "and only one");
                    which.push(l.abs() > rr.abs());
                }
            }
        }
        assert_eq!(which, vec![true, false, true], "left, right, left");
    }

    #[test]
    fn a_leap_wiggles_then_follows_its_arc_then_wobbles_on_a_perch() {
        let r = rig();
        let w = open();
        let mut p = Puppet::new(Build::feline());
        let g = PuppetInput { leap: Some(LeapView::Gather { t: 0.8 }), ..input(Vec3::ZERO) };
        let lashes: Vec<f32> = (0..20).map(|_| { settle(&mut p, &g, &r, &w, 0.02); p.pose(&g, &r).body.tail_lash }).collect();
        let (lo, hi) = lashes.iter().fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
        assert!(hi - lo > 0.3, "the gather wiggle: {lo}..{hi}");

        let up = PuppetInput { leap: Some(LeapView::Flight { t: 0.15, climb: 0.9 }), ..g };
        let down = PuppetInput { leap: Some(LeapView::Flight { t: 0.9, climb: -0.9 }), ..g };
        let (pu, pd) = (p.pose(&up, &r), p.pose(&down, &r));
        assert!(pu.body.pitch > 0.4 && pd.body.pitch < -0.4, "nose follows the arc");
        assert!(pu.legs[0].pitch > 0.8 && pu.legs[2].pitch < -0.7, "stretched: forepaws reach, hind legs trail");
        assert!(pd.legs[2].fold > 0.3, "gathered: hind legs tuck for the landing");

        let perch = |t: f32| p.pose(&PuppetInput { leap: Some(LeapView::Land { t, perched: true }), perched: true, ..g }, &r).body;
        let ground = |t: f32| p.pose(&PuppetInput { leap: Some(LeapView::Land { t, perched: false }), ..g }, &r).body;
        assert!((perch(0.11).roll - ground(0.11).roll).abs() > 0.05, "a perch landing rocks; a ground landing does not");
        assert!(perch(0.11).roll * (perch(0.11).tail_lash - ground(0.11).tail_lash) < 0.0, "the tail swings AGAINST the roll");
    }

    /// The screenshot that prompted this: a rear pivoted on the body centre,
    /// so the haunches sank through the floor and the hind legs swung
    /// forward with the body.
    #[test]
    fn a_rearing_cat_stands_on_vertical_hind_legs() {
        let r = rig();
        let w = open();
        let mut p = Puppet::new(Build::feline());
        let inp = input(Vec3::ZERO);
        p.begin_cast(CastStyle::Rear);
        settle(&mut p, &inp, &r, &w, 0.3); // 0.3 of 0.75 s: the top of the rear
        let pose = p.pose(&inp, &r);
        assert!(pose.body.pitch > 0.6, "chest is up: {}", pose.body.pitch);
        for i in 2..4 {
            let world_pitch = pose.legs[i].pitch + pose.body.pitch;
            assert!(world_pitch.abs() < 0.02, "hind leg {i} leans {world_pitch} rad instead of standing");
            assert!(sole(&pose, &r, i).abs() < 0.03, "hind sole {i} left the ground: {}", sole(&pose, &r, i));
        }
        assert!(pose.legs[0].pitch + pose.body.pitch > 1.0, "forepaws are thrown up and out");
    }

    /// The point of styling by archetype: one ancestor's four abilities
    /// should not all move alike. Assert the roster actually gets variety,
    /// or the mapping is decoration.
    #[test]
    fn most_ancestors_use_at_least_three_cast_styles() {
        let mut varied = 0;
        for def in crate::gods::ROSTER.iter() {
            let mut seen: Vec<CastStyle> = Vec::new();
            for a in &def.abilities {
                let st = cast_style(&a.arch, a.ult);
                if !seen.contains(&st) {
                    seen.push(st);
                }
            }
            assert!(seen.len() >= 2, "{} casts everything the same way", def.name);
            varied += (seen.len() >= 3) as usize;
        }
        assert!(varied >= 9, "only {varied}/13 ancestors have three or more cast styles");
    }

    #[test]
    fn garbage_in_never_reaches_the_renderer() {
        let r = rig();
        let w = World { step: |_, _| Some(f32::NAN), cover: |_, _| f32::NAN, water: |_, _| f32::INFINITY };
        let mut p = Puppet::new(Build::feline());
        let inp = PuppetInput { pos: Vec3::new(f32::NAN, 0.0, 1.0), yaw: f32::NAN, leap: Some(LeapView::Flight { t: f32::NAN, climb: f32::NAN }), ..input(Vec3::ZERO) };
        let mut ev = Vec::new();
        for dt in [0.016, f32::NAN, -1.0, 5.0] {
            p.advance(dt, &inp, &r, &w, &mut ev);
        }
        let pose = p.pose(&inp, &r);
        let all = [pose.lift, pose.body.drop, pose.body.pitch, pose.body.roll, pose.body.tail_lash, pose.body.tail_lift, pose.body.arch];
        assert!(all.iter().all(|x| x.is_finite()), "{all:?}");
        assert!(pose.legs.iter().all(|l| l.pitch.is_finite() && l.fold.is_finite() && l.splay.is_finite()));
    }
}
