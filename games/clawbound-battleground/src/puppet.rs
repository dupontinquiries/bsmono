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
    /// The nearest stem of something LIGHT -- a fern, a bush, a hanging
    /// vine -- within `within` of the point. Not rocks or trunks: those are
    /// collision, and the sim has already kept the cat out of them. These
    /// are the things a cat could push through and, stalking, will not.
    fn stem(&self, _x: f32, _z: f32, _within: f32) -> Option<(f32, f32)> {
        None
    }
    /// Is there something solid at this point that no paw can stand in: a
    /// trunk, a boulder's face. `footfall` steers landings off it.
    fn solid(&self, _x: f32, _z: f32) -> bool {
        false
    }
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
    /// `impact`: how hard, `0..=1`.
    Land { t: f32, perched: bool, impact: f32 },
    /// Falling, not leaping. `roll` is what is left of the tumble it began
    /// with, `twist` how far the front half leads the hind, `speed` the
    /// downward speed. The sim owns the righting curve; this draws it.
    Fall { roll: f32, twist: f32, speed: f32 },
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
    /// ...and that something is a ridge: a wall top, a pillar cap.
    pub narrow: bool,
    /// How quietly the sim says this cat is carrying itself, `0..=1`:
    /// stalking, an ability's hush, or wounded. See `Game::quiet`.
    pub quiet: f32,
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
    /// World-space sole positions from `footfall`, for the legs that are
    /// walking (a leg an action has freed -- a swipe, a rear -- is `None`
    /// and keeps its `legs` delta). `None` for all four in the air, on a
    /// perch, or before the first grounded frame.
    pub feet: [Option<Vec3>; 4],
}

/// How hard a moving cat shoulders what it passes, `0..=1`.
///
/// The whole difference between lurking and barrelling, as one number, and
/// everything that touches foliage reads it: the rustle, the leaves bending
/// in the shader, whether the body bothers to go round a stem.
///
/// A stalking cat threads between stems lighter than itself and leaves them
/// standing. It is not that it moves slowly and so disturbs a little less:
/// not disturbing things is what it is DOING. Hence sneak is a multiplier
/// of a tenth, not a smaller speed fed to the same curve -- which is also
/// what lets an ability's hush be quiet at a full run.
pub fn disturbance(speed: f32, quiet: f32) -> f32 {
    pace(speed) * (1.0 - 0.9 * finite_or(quiet, 0.0).clamp(0.0, 1.0))
}

/// How fast, `0..=1`, with nothing said about how quietly.
pub fn pace(speed: f32) -> f32 {
    ((finite_or(speed, 0.0) - 1.0) / 9.0).clamp(0.0, 1.0).powf(1.2)
}

/// Rustle below this and there is nothing to see or hear.
pub const QUIET: f32 = 0.12;
/// How close a stalking body lets a stem come to its centreline.
pub const STEM_CLEARANCE: f32 = 0.55;
/// The most a body will lean out of its line to go round one.
pub const MAX_SWAY: f32 = 0.42;

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
    sneak: f32,
    /// Sideways give at the shoulders and at the haunches, smoothed. The
    /// haunches pass a stem after the shoulders do, so sampling the two
    /// separately is all it takes for the body to CURVE round it.
    give: [f32; 2],
    last_cover: f32,
    /// Ground velocity, measured and smoothed like `speed`.
    vel: Vec3,
    /// Paw placement. See `footfall`.
    pub feet: crate::footfall::Footfall,
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
            sneak: 0.0,
            give: [0.0; 2],
            last_cover: 0.0,
            vel: Vec3::ZERO,
            feet: crate::footfall::Footfall::new(),
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
    /// See [`disturbance`].
    pub fn disturb(&self) -> f32 {
        disturbance(self.speed, self.sneak)
    }
    /// What this cat is doing to the foliage round it right now: where, and
    /// how hard. `None` when it is not among any.
    pub fn brush(&self) -> Option<(Vec3, f32)> {
        let p = self.last_pos?;
        (self.last_cover > 0.05 && self.disturb() > 0.01).then(|| (p, self.disturb()))
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
        let raw_vel = match self.last_pos {
            Some(p) if raw_speed > 0.0 => Vec3::new(pos.x - p.x, 0.0, pos.z - p.z) / dt,
            _ => Vec3::ZERO,
        };
        self.vel += (raw_vel - self.vel) * (dt * 10.0).min(1.0);
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

        // ── where the paws go down ────────────────────────────────────
        // Only on the ground. A leap, a fall, a dash or a perch lets go of
        // every plant; the first grounded frame after stands where it is.
        if grounded && inp.leap.is_none() {
            use crate::anim::Gait;
            let leg = (rig.hip(0)[1] - rig.sole(0)).max(0.05);
            // The same table `pose` will hand the rig, so the paws lift in
            // the pattern the body is moving to.
            let moving = self.speed > 0.3;
            let table = if self.crouch > 0.5 && moving && self.anim.gait() != Gait::Bound {
                Gait::Pace.leg_phases()
            } else if self.wet > 0.5 && moving {
                Gait::Walk.leg_phases()
            } else {
                self.anim.gait().leg_phases()
            };
            let body = crate::footfall::Body {
                hips: std::array::from_fn(|i| {
                    let h = rig.hip(i);
                    pos + fwd * h[0] + right * h[2]
                }),
                leg,
                vel: self.vel,
                gait_phase: self.anim.gait_phase,
                offsets: table,
                hz: self.anim.build.stride_hz(self.anim.smoothed_speed()),
                lift: creature_rig::anim_rig::SWING_LIFT,
            };
            self.feet.advance(dt, &body, world);
        } else {
            self.feet.release();
        }

        // ── foliage ───────────────────────────────────────────────────
        let cover = if grounded { finite_or(world.cover(pos.x, pos.z), 0.0).clamp(0.0, 1.0) } else { 0.0 };
        // A cat at a dead run does not stalk; it bursts through.
        self.sneak += (finite_or(inp.quiet, 0.0).clamp(0.0, 1.0) - self.sneak) * (dt * 10.0).min(1.0);
        self.last_cover = cover;
        // Cover invites a crouch, and a run cancels it: a cat at full tilt
        // through a fern bed is not hiding. A deliberate stalk is a stalk
        // anywhere, open ground included.
        let bold = disturbance(self.speed, self.sneak);
        let want_crouch = (cover * (1.0 - bold)).max(0.9 * self.sneak) * if grounded { 1.0 } else { 0.0 };
        self.crouch += (want_crouch - self.crouch) * (dt * 6.0).min(1.0);
        let now_in = cover > 0.3;
        self.rustle_t -= dt;
        // A rustle is what the cat DID to the leaves, not the fact that it
        // arrived among them. This fired at full strength on entry whatever
        // the carriage, so a stalking cat announced itself -- exactly
        // backwards. Below QUIET there is nothing: no event at all, rather
        // than a small one for something downstream to round up.
        let disturb = self.disturb();
        if now_in && disturb >= QUIET && (!self.in_cover || self.rustle_t <= 0.0) {
            events.push(PuppetEvent::Rustle { pos, strength: disturb });
            self.rustle_t = RUSTLE_EVERY;
        }
        self.in_cover = now_in;

        // ── the slither ───────────────────────────────────────────────
        // Shoulders and haunches each give way to the nearest light stem.
        // Weighted by how much this cat is lurking: at a run the weight is
        // zero and it goes straight through, which is the other half of the
        // same behaviour.
        // By SPEED, not by quietness. A hushed cat is crouched and silent at a
        // full run, and a body cannot thread stems at 12 u/s: the give lags
        // and it reads as a wobble. (A break test found this: the factor here
        // used to be `1 - bold`, which is ~1 under a hush.)
        let threading = 1.0 - ((self.speed - 4.5) / 4.0).clamp(0.0, 1.0); // full care at a stalk, none by a run
        let care = self.crouch * threading * if grounded { 1.0 } else { 0.0 };
        for (k, leg) in [(0usize, 0usize), (1, 2)] {
            let ahead = rig.hip(leg)[0];
            let at = pos + fwd * ahead;
            let mut want = 0.0;
            if care > 0.02 {
                if let Some((sx, sz)) = world.stem(at.x, at.z, 1.2) {
                    let rel = Vec3::new(sx - at.x, 0.0, sz - at.z);
                    let (along, across) = (rel.dot(fwd), rel.dot(right));
                    if along.is_finite() && across.is_finite() && across.abs() < STEM_CLEARANCE {
                        // Dead ahead has no obvious side. Pick by the stem's
                        // own position so the choice is stable frame to
                        // frame; a sign that flickers is a cat that shivers.
                        let side = if across.abs() > 0.04 { across.signum() } else if (sx + sz).fract().abs() > 0.5 { 1.0 } else { -1.0 };
                        let near = 1.0 - (along.abs() / 0.9).clamp(0.0, 1.0);
                        want = -side * (STEM_CLEARANCE - across.abs()) * near * near * (3.0 - 2.0 * near) * care;
                    }
                }
            }
            let want = want.clamp(-MAX_SWAY, MAX_SWAY);
            self.give[k] += (want - self.give[k]) * (dt * 7.0).min(1.0);
        }

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

        // Going round a stem: the body shifts by the mean give and turns by
        // the difference, the head holds its line, the tail finishes the curve.
        if self.give[0].abs() + self.give[1].abs() > 1e-4 {
            let span = (rig.hip(0)[0] - rig.hip(2)[0]).abs().max(0.2);
            body.sway += (self.give[0] + self.give[1]) * 0.5;
            let bend = ((self.give[0] - self.give[1]) / span).atan();
            body.yaw += bend;
            m.gaze_yaw -= bend * 0.9;
            body.tail_lash += bend * 1.4;
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
                // A leap to a pillar cap is nearly vertical; let it look it.
                body.pitch += finite_or(climb, 0.0).atan().clamp(-1.0, 1.2);
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
            Some(LeapView::Land { t, perched, impact }) => {
                let k = (1.0 - t.clamp(0.0, 1.0)).powi(2);
                let impact = finite_or(impact, 0.6).clamp(0.0, 1.0);
                drop += (0.16 + 0.30 * impact) * leg_len * k;
                body.pitch -= 0.16 * k;
                m.impact = m.impact.max(k * impact);
                if perched {
                    // Narrow footing: the body rocks and the tail goes the
                    // other way, harder, because that is what a tail is for.
                    let wob = (t * 14.0).sin() * (1.0 - t).max(0.0);
                    body.roll += 0.09 * wob;
                    body.tail_lash -= 0.30 * wob;
                }
            }
            Some(LeapView::Fall { roll, twist, speed }) => {
                planted = [false; 4];
                let roll = finite_or(roll, 0.0);
                let turned = (roll.abs() / std::f32::consts::PI).min(1.0);
                body.roll += roll;
                body.twist = finite_or(twist, 0.0);
                // Forelegs TUCK while the front half turns (a smaller moment
                // turns faster) and the hind legs stay out; once upright all
                // four spread to the air and then reach for the ground.
                let reach = (finite_or(speed, 0.0) / 9.0).clamp(0.0, 1.0) * (1.0 - turned);
                for l in &mut legs[0..2] {
                    l.fold += 0.40 * turned;
                    l.pitch += 0.35 * (1.0 - turned) + 0.25 * reach;
                    l.splay += 0.40 * (1.0 - turned) * (1.0 - reach);
                }
                for l in &mut legs[2..4] {
                    l.pitch += -0.35 * (1.0 - reach) + 0.15 * reach;
                    l.splay += 0.40 * (1.0 - reach);
                }
                // Forepaws first.
                body.pitch -= 0.22 * (1.0 - turned);
                // The tail goes the OTHER way: it is the only thing a falling
                // cat has to push against.
                body.tail_lash -= (roll * 0.6).clamp(-1.2, 1.2);
                body.tail_lift += 0.4;
                body.arch += 0.06 * turned;
                m.swing_scale = 0.0;
            }
            None => {}
        }
        if inp.perched && inp.leap.is_none() {
            if inp.narrow {
                // A ridge: paws in one line, a walk however fast, and the
                // tail swinging against every step. This is the fence-top
                // trot, and it is the tail that makes it read.
                let step = self.anim.gait_phase.sin();
                let go = (self.speed / 5.0).clamp(0.0, 1.0);
                for l in &mut legs {
                    l.splay -= 0.13;
                }
                if moving {
                    m.leg_phases = Some(Gait::Walk.leg_phases());
                }
                body.roll += step * 0.035 * go;
                body.tail_lash += -step * 0.50 * go + (time * 1.1).sin() * 0.20 * (1.0 - go);
                body.tail_lift += 0.20;
            } else {
                body.tail_lash += (time * 1.3).sin() * 0.22;
                body.tail_lift -= 0.25;
            }
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
        body.twist = finite_or(body.twist, 0.0).clamp(-1.6, 1.6);
        body.yaw = finite_or(body.yaw, 0.0).clamp(-0.7, 0.7);
        body.sway = finite_or(body.sway, 0.0).clamp(-MAX_SWAY, MAX_SWAY);
        let soles = self.feet.soles();
        let feet = std::array::from_fn(|i| if planted[i] && !inp.perched && inp.leap.is_none() { soles[i].filter(|p| p.is_finite()) } else { None });
        PuppetPose { motion: m, legs, body, lift: finite_or(lift, 0.0), feet }
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
    /// Where cats have just been, for the leaves to swing back through.
    pub wake: Vec<Wake>,
    wake_t: f32,
}

/// A place a cat shouldered through a moment ago.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wake {
    pub pos: Vec3,
    pub strength: f32,
    pub age: f32,
}

/// How many brushes the voxel shader takes (`FrameData::brush`).
pub const MAX_BRUSHES: usize = 8;
const WAKE_SECS: f32 = 0.9;
const WAKE_EVERY: f32 = 0.12;

impl Wake {
    /// A pushed leaf does not return to rest, it swings back THROUGH it:
    /// a damped spring, so this goes negative. The shader multiplies its
    /// push by this, and a negative push is a leaf leaning toward where the
    /// cat was.
    pub fn now(&self) -> f32 {
        self.strength * (-4.5 * self.age).exp() * (15.0 * self.age).cos()
    }
}

impl Troupe {
    pub fn ensure(&mut self, n: usize) {
        while self.puppets.len() < n {
            self.puppets.push(Puppet::new(Build::feline()));
        }
    }
    /// Age the wake and lay new points behind every cat that is pushing
    /// through something hard enough to matter.
    pub fn trail(&mut self, dt: f32) {
        for w in &mut self.wake {
            w.age += dt;
        }
        self.wake.retain(|w| w.age < WAKE_SECS);
        self.wake_t -= dt;
        if self.wake_t <= 0.0 {
            self.wake_t = WAKE_EVERY;
            for p in &self.puppets {
                if let Some((pos, strength)) = p.brush() {
                    if strength >= QUIET {
                        self.wake.push(Wake { pos, strength, age: 0.0 });
                    }
                }
            }
        }
    }

    /// What the shader should bend leaves away from this frame, strongest
    /// first: `[x, y, z, strength]`. Live cats, then what they left behind.
    pub fn brushes(&self) -> Vec<[f32; 4]> {
        let mut all: Vec<[f32; 4]> = self
            .puppets
            .iter()
            .filter_map(|p| p.brush())
            .map(|(p, s)| [p.x, p.y, p.z, s])
            .chain(self.wake.iter().filter(|w| w.age > WAKE_EVERY).map(|w| [w.pos.x, w.pos.y, w.pos.z, w.now()]))
            .filter(|b| b[3].abs() > 0.004 && b.iter().all(|v| v.is_finite()))
            .collect();
        all.sort_by(|a, b| b[3].abs().partial_cmp(&a[3].abs()).unwrap_or(std::cmp::Ordering::Equal));
        all.truncate(MAX_BRUSHES);
        all
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
        PuppetInput { pos, yaw: 0.0, hp01: 1.0, atk_pulse: 0.0, strike_style: StrikeStyle::Swipe, leap: None, perched: false, narrow: false, quiet: 0.0 }
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
        let land = PuppetInput { leap: Some(LeapView::Land { t: 0.1, perched: false, impact: 0.6 }), ..input(Vec3::ZERO) };
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

    /// A fern bed with one stem in it, for the slither.
    struct Thicket {
        stem: (f32, f32),
    }
    impl Probe for Thicket {
        fn ground(&self, _: f32, _: f32) -> f32 { 0.0 }
        fn step_top(&self, _: f32, _: f32) -> Option<f32> { None }
        fn cover(&self, _: f32, _: f32) -> f32 { 0.8 }
        fn water(&self, _: f32, _: f32) -> f32 { 0.0 }
        fn stem(&self, x: f32, z: f32, within: f32) -> Option<(f32, f32)> {
            ((x - self.stem.0).hypot(z - self.stem.1) < within).then_some(self.stem)
        }
    }

    /// Drive a cat along +x through `w` at `speed`, return what it did.
    fn cross(w: &dyn Probe, speed: f32, quiet: f32, from: f32, to: f32) -> (Puppet, Vec<PuppetEvent>, Vec<PuppetPose>) {
        let r = rig();
        let mut p = Puppet::new(Build::feline());
        let (mut ev, mut poses) = (Vec::new(), Vec::new());
        let mut x = from;
        while x < to {
            let inp = PuppetInput { quiet, ..input(Vec3::new(x, 0.0, 0.0)) };
            p.advance(1.0 / 60.0, &inp, &r, w, &mut ev);
            poses.push(p.pose(&inp, &r));
            x += speed / 60.0;
        }
        (p, ev, poses)
    }

    /// This replaces a test that asserted the OPPOSITE: one rustle on entry,
    /// whatever the cat was doing. A cat standing in a fern, or stalking
    /// through one, does nothing to it. A cat running through one does.
    #[test]
    fn a_stalking_cat_leaves_the_fern_alone_and_a_running_one_does_not() {
        let fern = World { step: |_, _| None, cover: |x, _| if x > 5.0 { 0.8 } else { 0.0 }, water: |_, _| 0.0 };
        let rustles = |ev: &[PuppetEvent]| ev.iter().filter_map(|e| if let PuppetEvent::Rustle { strength, .. } = e { Some(*strength) } else { None }).collect::<Vec<f32>>();

        let (p, ev, poses) = cross(&fern, 12.4 * 0.26, 1.0, 0.0, 12.0);
        assert!(rustles(&ev).is_empty(), "a stalk rustled: {:?}", rustles(&ev));
        assert!(p.disturb() < 0.05, "stalking disturbance {}", p.disturb());
        assert!(p.crouch() > 0.7, "and it IS stalking: crouch {}", p.crouch());
        assert!(poses.last().unwrap().body.drop > 0.15, "chest down");

        let (p, ev, _) = cross(&fern, 12.4, 0.0, 0.0, 30.0);
        let r = rustles(&ev);
        assert!((3..=40).contains(&r.len()), "a run through 25 u of fern rustled {} times", r.len());
        assert!(r.iter().all(|s| *s > 0.8), "and hard: {r:?}");
        assert!(p.disturb() > 0.9 && p.crouch() < 0.15, "barrelling, upright: {} / {}", p.disturb(), p.crouch());

        // Standing in it: nothing, however long.
        let (_, ev, _) = cross(&fern, 0.0001, 0.0, 8.0, 8.01);
        assert!(rustles(&ev).is_empty());
        // Sneak is not "slow": the same speed un-sneaked brushes harder.
        assert!(disturbance(3.2, 0.0) > disturbance(3.2, 1.0) * 5.0);
    }

    /// The slither. A stem just right of the line: the shoulders give LEFT
    /// first, the haunches after, so the body turns one way coming up to the
    /// stem and the other way leaving it. A running cat does none of this.
    #[test]
    fn a_stalking_body_curves_round_a_stem_and_a_running_one_goes_straight_through() {
        let w = Thicket { stem: (6.0, 0.2) }; // yaw 0: forward +x, right +z
        let (_, _, poses) = cross(&w, 3.2, 1.0, 0.0, 12.0);
        let sway: Vec<f32> = poses.iter().map(|p| p.body.sway).collect();
        let yaw: Vec<f32> = poses.iter().map(|p| p.body.yaw).collect();
        let peak = sway.iter().cloned().fold(0.0f32, f32::min);
        assert!((-MAX_SWAY..-0.12).contains(&peak), "leans LEFT, away from a stem on its right, by a paw's width or so: {peak}");
        assert!(sway.iter().all(|s| *s < 0.02), "and never toward it");
        let (lo, hi) = yaw.iter().fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(*y), b.max(*y)));
        assert!(lo < -0.06 && hi > 0.06, "the body turns away coming in and back going out: {lo}..{hi}");
        let first_neg = yaw.iter().position(|y| *y < -0.06).unwrap();
        let first_pos = yaw.iter().position(|y| *y > 0.06).unwrap();
        assert!(first_neg < first_pos, "shoulders give before haunches");
        assert!(sway.last().unwrap().abs() < 0.02 && yaw.last().unwrap().abs() < 0.02, "and it straightens out afterwards");
        let head = poses[first_neg].motion.gaze_yaw;
        assert!(head > 0.03, "the head holds its line while the body turns: {head}");

        let (_, _, poses) = cross(&w, 12.4, 0.0, 0.0, 12.0);
        let worst = poses.iter().map(|p| p.body.sway.abs().max(p.body.yaw.abs())).fold(0.0f32, f32::max);
        assert!(worst < 0.03, "a running cat swerved {worst} for a fern");
        // Hushed: low and silent, but at a full run -- and so it must NOT try
        // to go round. Quiet is not slow.
        let (_, _, hushed) = cross(&w, 12.4, 0.9, 0.0, 12.0);
        let worst = hushed.iter().map(|p| p.body.sway.abs().max(p.body.yaw.abs())).fold(0.0f32, f32::max);
        assert!(worst < 0.04, "a hushed RUN does not slither: {worst}");
        assert!(hushed.last().unwrap().body.drop > 0.15, "though it does stay low");
    }

    /// Leaves swing back THROUGH rest and settle; and the brush list is
    /// bounded, strongest first, with nothing in it from a stalking cat.
    /// The balance idea from review, pinned: stalking is not a key you
    /// hold, it is what state you are in. Hush is quiet AT A FULL RUN (the
    /// sneak boost an ability grants costs no speed) and wounded is
    /// half-quiet. Ranges, because for the middle two both "as loud as a
    /// run" and "as silent as a stalk" would be wrong.
    #[test]
    fn quiet_scales_the_disturbance_at_any_speed() {
        let (run, hush, hurt, stalk) = (disturbance(12.4, 0.0), disturbance(12.4, 0.9), disturbance(12.4, 0.5), disturbance(3.2, 1.0));
        assert!(run > 0.95, "{run}");
        assert!((0.12..0.25).contains(&hush), "hushed at a run: {hush}");
        assert!((0.45..0.65).contains(&hurt), "wounded at a run: {hurt}");
        assert!(stalk < 0.03, "{stalk}");
        assert!(hush < QUIET * 2.0 && run > QUIET * 5.0);
        assert_eq!(disturbance(0.5, 0.0), 0.0, "a cat picking its way disturbs nothing, unasked");
        assert_eq!(disturbance(f32::NAN, f32::NAN), 0.0);
    }

    #[test]
    fn the_wake_swings_back_and_a_stalk_leaves_none() {
        let w = Wake { pos: Vec3::ZERO, strength: 1.0, age: 0.0 };
        let samples: Vec<f32> = (0..90).map(|i| Wake { age: i as f32 / 100.0, ..w }.now()).collect();
        assert!(samples.iter().any(|s| *s < -0.15), "it overshoots: min {:?}", samples.iter().cloned().fold(f32::MAX, f32::min));
        assert!(samples.last().unwrap().abs() < 0.03, "and settles");

        let fern = World { step: |_, _| None, cover: |_, _| 0.8, water: |_, _| 0.0 };
        for (quiet, speed, want_wake) in [(1.0, 3.2, false), (0.0, 12.4, true)] {
            let r = rig();
            let mut t = Troupe::default();
            t.ensure(12);
            let mut ev = Vec::new();
            for f in 0..60 {
                for (k, p) in t.puppets.iter_mut().enumerate() {
                    let inp = PuppetInput { quiet, ..input(Vec3::new(f as f32 * speed / 60.0, 0.0, k as f32 * 3.0)) };
                    p.advance(1.0 / 60.0, &inp, &r, &fern, &mut ev);
                }
                t.trail(1.0 / 60.0);
            }
            let b = t.brushes();
            assert_eq!(!t.wake.is_empty(), want_wake, "quiet {quiet}: wake {}", t.wake.len());
            if want_wake {
                assert_eq!(b.len(), MAX_BRUSHES, "twelve cats and their wakes, capped");
                assert!(b.windows(2).all(|p| p[0][3].abs() >= p[1][3].abs()), "strongest first");
                assert!(b[0][3] > 0.9);
            } else {
                assert!(b.iter().all(|x| x[3].abs() < 0.05), "a stalk barely registers: {b:?}");
            }
        }
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

        let perch = |t: f32| p.pose(&PuppetInput { leap: Some(LeapView::Land { t, perched: true, impact: 0.6 }), perched: true, ..g }, &r).body;
        let ground = |t: f32| p.pose(&PuppetInput { leap: Some(LeapView::Land { t, perched: false, impact: 0.6 }), ..g }, &r).body;
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

    /// The reflex, as drawn: the body carries the roll the sim reports, the
    /// front half is ahead of the hind, the forelegs are tucked while it
    /// turns, and the tail is thrown the other way.
    #[test]
    fn a_tumbling_cat_tucks_twists_and_throws_its_tail_the_other_way() {
        let r = rig();
        let p = Puppet::new(Build::feline());
        let on_its_back = PuppetInput { leap: Some(LeapView::Fall { roll: 2.4, twist: 0.9, speed: 2.0 }), ..input(Vec3::ZERO) };
        let upright = PuppetInput { leap: Some(LeapView::Fall { roll: 0.0, twist: 0.0, speed: 9.0 }), ..input(Vec3::ZERO) };
        let (a, b) = (p.pose(&on_its_back, &r), p.pose(&upright, &r));
        assert!(a.body.roll > 0.75 && a.body.twist > 0.8, "rolled, and twisting: {:?}", a.body);
        assert!(a.body.tail_lash < -0.5, "tail against the roll: {}", a.body.tail_lash);
        assert!(a.legs[0].fold > b.legs[0].fold + 0.2, "forelegs tucked while turning, out again once upright");
        assert!(b.body.roll.abs() < 1e-3 && b.body.twist == 0.0);
        assert!(b.body.pitch < -0.15 && b.legs[0].pitch > 0.4, "upright and close to the ground: nose down, forepaws reaching");
        // And in the matrices: the front half really is turned further than the hind.
        let mut posed = Vec::new();
        creature_rig::pose(r.parts(), r.torso_half, &a.motion, None, &mut posed);
        let m = crate::rigged::slot_transforms(&r, &posed, &a.legs, &a.body);
        let up = |s: crate::rigged::Slot| m[s as usize].transform_vector3(Vec3::Y).normalize().y;
        assert!(up(crate::rigged::Slot::Head) > up(crate::rigged::Slot::Tail0) + 0.3, "head is nearer upright than the tail: {} vs {}", up(crate::rigged::Slot::Head), up(crate::rigged::Slot::Tail0));
    }

    /// The fence-top walk: paws pulled into one line, and the tail swinging
    /// against the steps rather than idling.
    #[test]
    fn on_a_ridge_the_paws_line_up_and_the_tail_counters_each_step() {
        let r = rig();
        let w = open();
        let mut p = Puppet::new(Build::feline());
        let mut lashes = Vec::new();
        let mut splay = 0.0;
        for f in 0..90 {
            let inp = PuppetInput { perched: true, narrow: true, ..input(Vec3::new(f as f32 * 0.06, 3.0, 0.0)) };
            p.advance(1.0 / 60.0, &inp, &r, &w, &mut Vec::new());
            let pose = p.pose(&inp, &r);
            lashes.push(pose.body.tail_lash);
            splay = pose.legs[0].splay;
        }
        assert!(splay < -0.1, "paws drawn in under the body: {splay}");
        let (lo, hi) = lashes[30..].iter().fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
        assert!(hi > 0.2 && lo < -0.2, "the tail works both sides while walking: {lo}..{hi}");
        let flat = PuppetInput { perched: true, narrow: false, ..input(Vec3::new(5.4, 3.0, 0.0)) };
        assert!(p.pose(&flat, &r).legs[0].splay > -0.01, "a boulder is not a ridge");
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
