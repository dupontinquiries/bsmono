//! THE WATCH ROSTER, as twelve different verbs.
//!
//! The C engine's `exengine/weapon.h` defines twelve forms, and the honest
//! problem with them is that ten are the same weapon: a projectile with a
//! different damage number, a different cooldown and a different colour.
//! `damage=42 range=200` and `damage=18 range=160` are not two weapons, they
//! are one weapon and a balance patch.
//!
//! They read as similar because they were written for a shooter, where the
//! only verb is "reduce hit points". So the rebuild asks a different
//! question — not how much a shot HURTS, but what it DOES to a body with
//! mass, position and momentum. There are far more answers to that, and a
//! raid boss you are shoving, lifting, dragging and pulling off its feet is
//! a better fight than one whose number is going down.
//!
//! Targets here are deliberately anonymous: a `Target` is a position, a
//! radius and an id. A boss, a rival bot, a destructible, a physics prop —
//! the weapons neither know nor care. Every form below has its own verb:
//!
//!   WATER        push       a steady jet — no impulse spike, pure control
//!   TSUNAMI      shove      radial shove that moves you too (recoil is real)
//!   ICE          blink      the quantum gravity leap, with the late wave
//!   FIRE         flak       airburst that lifts what it catches
//!   MAGMA        pool       lingering patch that drags anything crossing it
//!   BLIZZARD     cone       spread of pellets, each one slowing
//!   METAL        bank       ricochets off walls: shoot a corner, hit the ball
//!   TITANIUM     pierce     instant line, passes through everything
//!   ELECTRICITY  chain      arcs from first hit to whatever is nearest
//!   WIND         thrust     weak forward, strong BACKWARD — a mobility gun
//!   TORNADO      vortex     a well that pulls the ball for a few seconds
//!   PHOTON       beam       apex: enormous, instant, and a long lockout
//!
//! Two of those come straight out of the film material rather than the
//! engine header. ICE is the quantum gravity leap — Ice form only, because
//! "the quantum cold state requires Ice Clone biology specifically" — which
//! is silent and instant with a ground wave arriving seconds later that the
//! jumper can warn about but cannot stop. That delayed consequence is the
//! most distinctive thing in the whole roster and it was sitting in the
//! script, not the code. PHOTON is the apex form nobody has fully mapped, so
//! it hits like it and then leaves you with nothing for a while.
//!
//! WIND is the one pure invention: a gun whose recoil is the point. It comes
//! from the same place as the thruster hack — in a game about momentum, a
//! weapon that moves YOU is worth more than one that hurts someone else.

use glam::Vec3;

pub const FORMS: usize = 13;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verb {
    Push,
    Shove,
    Blink,
    Flak,
    Pool,
    Cone,
    Bank,
    Pierce,
    Chain,
    Thrust,
    Vortex,
    Beam,
    Grapple,
}

pub struct Form {
    pub name: &'static str,
    pub verb: Verb,
    /// seconds between shots
    pub cooldown: f32,
    /// boost spent per shot — the same tank the thrusters and the second
    /// jump draw on, so shooting costs you mobility
    pub cost: f32,
    /// how hard a hit shoves the ball, in m/s of delta-v
    pub impulse: f32,
    /// metres the shot reaches (or the radius, for the area verbs)
    pub reach: f32,
    /// projectile speed; 0 means hitscan
    pub speed: f32,
    /// cone half-angle in degrees, for Cone
    pub spread: f32,
    /// pellets per shot
    pub pellets: u32,
    /// 0..1 slow applied to what it hits
    pub slow: f32,
    pub color: [f32; 4],
    /// what firing this looks like
    pub muzzle: FxKind,
    /// what a hit looks like
    pub impact: FxKind,
}

const fn f(
    name: &'static str,
    verb: Verb,
    cooldown: f32,
    cost: f32,
    impulse: f32,
    reach: f32,
    speed: f32,
    spread: f32,
    pellets: u32,
    slow: f32,
    color: [f32; 4],
    muzzle: FxKind,
    impact: FxKind,
) -> Form {
    Form { name, verb, cooldown, cost, impulse, reach, speed, spread, pellets, slow, color, muzzle, impact }
}

/// Colours are carried over from `ex_weapon_profile` so the forms still read
/// as themselves; everything else is rebuilt around the verb.
pub const TABLE: [Form; FORMS] = [
    // effects are chosen to read as the VERB: a jet looks like a push, a
    // swirl looks like a pull, a cable looks like a thing you can climb
    f("WATER", Verb::Push, 0.06, 1.2, 1.6, 34.0, 46.0, 0.0, 1, 0.0, [0.35, 0.67, 1.0, 1.0], FxKind::Jet, FxKind::Spark),
    f("TSUNAMI", Verb::Shove, 0.85, 14.0, 15.0, 9.0, 0.0, 0.0, 1, 0.0, [0.27, 0.59, 0.90, 1.0], FxKind::Ripple, FxKind::Ripple),
    f("ICE", Verb::Blink, 3.20, 26.0, 9.0, 1.0e9, 0.0, 0.0, 1, 0.55, [0.59, 0.86, 1.0, 1.0], FxKind::Distort, FxKind::GroundWave),
    f("FIRE", Verb::Flak, 0.55, 8.0, 9.5, 40.0, 30.0, 0.0, 1, 0.0, [1.0, 0.47, 0.16, 1.0], FxKind::Muzzle, FxKind::Ripple),
    f("MAGMA", Verb::Pool, 1.30, 16.0, 4.0, 26.0, 20.0, 0.0, 1, 0.45, [0.90, 0.35, 0.12, 1.0], FxKind::Muzzle, FxKind::Patch),
    f("BLIZZARD", Verb::Cone, 0.70, 10.0, 2.4, 18.0, 32.0, 14.0, 7, 0.35, [0.75, 0.90, 1.0, 1.0], FxKind::Jet, FxKind::Spark),
    f("METAL", Verb::Bank, 0.30, 5.0, 7.0, 60.0, 40.0, 0.0, 1, 0.0, [0.71, 0.73, 0.76, 1.0], FxKind::Muzzle, FxKind::Spark),
    f("TITANIUM", Verb::Pierce, 1.10, 20.0, 13.0, 70.0, 0.0, 0.0, 1, 0.0, [0.59, 0.63, 0.69, 1.0], FxKind::Beam, FxKind::Spark),
    f("ELECTRICITY", Verb::Chain, 0.90, 15.0, 8.0, 45.0, 34.0, 0.0, 1, 0.20, [0.55, 0.71, 1.0, 1.0], FxKind::Muzzle, FxKind::Arc),
    f("WIND", Verb::Thrust, 0.22, 3.0, 3.0, 22.0, 55.0, 0.0, 1, 0.0, [0.82, 0.92, 0.84, 1.0], FxKind::Jet, FxKind::Spark),
    f("TORNADO", Verb::Vortex, 2.60, 22.0, 0.0, 11.0, 26.0, 0.0, 1, 0.0, [0.78, 0.86, 0.82, 1.0], FxKind::Muzzle, FxKind::Swirl),
    f("PHOTON", Verb::Beam, 2.40, 34.0, 26.0, 120.0, 0.0, 0.0, 1, 0.0, [1.0, 0.98, 0.90, 1.0], FxKind::Beam, FxKind::Ripple),
    // GRAPPLE is not a Watch form — it is the traversal tool the raid docs
    // already assume exists ("AIR: reachable only by flight or grapple").
    // Light things come to you; heavy things pull you to them, which makes
    // one button both a weapon and a way up.
    f("GRAPPLE", Verb::Grapple, 0.45, 6.0, 22.0, 55.0, 0.0, 0.0, 1, 0.0, [0.85, 0.78, 0.45, 1.0], FxKind::Cable, FxKind::Cable),
];

/// A shot in flight. Hitscan verbs never make one.
#[derive(Clone, Copy)]
pub struct Shot {
    pub pos: Vec3,
    pub vel: Vec3,
    pub life: f32,
    pub form: u8,
    pub bounces: u8,
}

/// A patch of ground that drags — MAGMA leaves these.
#[derive(Clone, Copy)]
pub struct Pool {
    pub pos: Vec3,
    pub r: f32,
    pub life: f32,
}

/// A well that pulls the ball — TORNADO leaves one.
#[derive(Clone, Copy)]
pub struct Vortex {
    pub pos: Vec3,
    pub r: f32,
    pub life: f32,
}

/// The delayed consequence of an ICE leap. Silent on departure; the wave
/// lands seconds later and does not care who is standing there.
#[derive(Clone, Copy)]
pub struct Wave {
    pub pos: Vec3,
    pub timer: f32,
    pub r: f32,
}

/// What the renderer should spawn. The weapons decide WHAT happens and
/// where; how it looks is the renderer's business, so this is a vocabulary
/// rather than a pile of particle parameters.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FxKind {
    /// space itself bending — the leap's aim and its arrival
    Distort,
    /// an expanding ring on the ground
    Ripple,
    /// the leap's late wave: a ring that arrives without a source
    GroundWave,
    /// a jet or continuous stream
    Jet,
    /// hard directional flash at a muzzle
    Muzzle,
    /// impact spark
    Spark,
    /// lingering surface patch
    Patch,
    /// swirling well
    Swirl,
    /// a straight line drawn for one frame
    Beam,
    /// an arc between two points
    Arc,
    /// a taut line to an anchor
    Cable,
}

/// One thing to draw, this frame.
#[derive(Clone, Copy, Debug)]
pub struct FxEvent {
    pub kind: FxKind,
    pub pos: Vec3,
    /// direction for beams/jets, or the far end for arcs and cables
    pub dir: Vec3,
    pub scale: f32,
    pub color: [f32; 4],
}

/// Anything a shot can hit. Anonymous on purpose — a raid boss, a rival, a
/// prop. The weapon does not ask what it is.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub id: u32,
    pub pos: Vec3,
    pub r: f32,
}

/// What firing asked the world to do. The caller owns the car, the targets
/// and the effects, so `fire` reports rather than reaches.
#[derive(Default)]
pub struct Outcome {
    /// delta-v to apply to the shooter (recoil, thrust, the ICE blink)
    pub self_impulse: Vec3,
    /// teleport the shooter here (ICE)
    pub teleport: Option<Vec3>,
    /// (target id, delta-v) — a chain or a cone can fill several
    pub hits_on: Vec<(u32, Vec3)>,
    /// slow to apply, per target id
    pub slowed: Vec<(u32, f32)>,
    /// where a visible impact happened, for effects
    pub hits: Vec<Vec3>,
    /// what to draw
    pub fx: Vec<FxEvent>,
    pub fired: bool,
}

impl Outcome {
    fn hit(&mut self, id: u32, dv: Vec3) {
        self.hits_on.push((id, dv));
    }
    fn spawn(&mut self, kind: FxKind, pos: Vec3, dir: Vec3, scale: f32, color: [f32; 4]) {
        self.fx.push(FxEvent { kind, pos, dir, scale, color });
    }
    /// total delta-v this outcome applied, for tests and HUD feedback
    pub fn total_impulse(&self) -> f32 {
        self.hits_on.iter().map(|(_, v)| v.length()).sum()
    }
}

pub struct Weapons {
    pub form: usize,
    pub cool: f32,
    pub shots: Vec<Shot>,
    pub pools: Vec<Pool>,
    pub vortex: Option<Vortex>,
    pub wave: Option<Wave>,
    /// the target the camera and the aim assist are following, if any
    pub locked: Option<u32>,
    /// ICE's targeting freecam. The leap has no range limit — "one moment
    /// they aren't there, then they are" — so it cannot be aimed down a
    /// barrel. You detach the view, fly it to where you want to arrive, and
    /// commit. That IS the weapon's interface.
    pub aim: Option<Aim>,
    /// the live grapple line, if one is out
    pub hook: Option<Hook>,
}

/// The detached targeting view for the leap.
#[derive(Clone, Copy)]
pub struct Aim {
    pub pos: Vec3,
    pub from: Vec3,
    /// 0..1, how far the space-bend has wound up — the distortion grows the
    /// longer you hold, which is also the tell that gives you away
    pub charge: f32,
}

#[derive(Clone, Copy)]
pub struct Hook {
    pub anchor: Vec3,
    /// which target it bit into, if it caught something rather than terrain
    pub on: Option<u32>,
    /// true when the anchor is heavier than you, so YOU travel
    pub reel_self: bool,
    pub life: f32,
}

impl Default for Weapons {
    fn default() -> Weapons {
        Weapons::new()
    }
}

impl Weapons {
    pub fn new() -> Weapons {
        Weapons {
            form: 0,
            cool: 0.0,
            shots: Vec::new(),
            pools: Vec::new(),
            vortex: None,
            wave: None,
            locked: None,
            aim: None,
            hook: None,
        }
    }

    pub fn profile(&self) -> &'static Form {
        &TABLE[self.form.min(FORMS - 1)]
    }

    pub fn cycle(&mut self, d: i32) {
        self.form = ((self.form as i32 + d).rem_euclid(FORMS as i32)) as usize;
        self.cool = 0.0;
    }

    /// Pull the trigger. `dir` is where the robot is aiming, `ball` is where
    /// the ball is (weapons in this game are judged on what they do to it).
    ///
    /// Returns what should happen; applying it is the caller's job, because
    /// the shooter's velocity and the ball live in the physics module and
    /// borrowing both through here would be worse than reporting.
    pub fn fire(
        &mut self,
        origin: Vec3,
        dir: Vec3,
        targets: &[Target],
        boost: f32,
    ) -> Outcome {
        let mut out = Outcome::default();
        let p = self.profile();
        if self.cool > 0.0 || boost < p.cost {
            return out;
        }
        let d = dir.normalize_or(Vec3::NEG_Z);
        self.cool = p.cooldown;
        out.fired = true;
        out.spawn(p.muzzle, origin, d, 1.0, p.color);

        // every target the ray passes through, nearest first — PIERCE and
        // BEAM want all of them, not just the first
        let along_ray = |reach: f32| -> Vec<(f32, Target)> {
            let mut v: Vec<(f32, Target)> = Vec::new();
            for t in targets {
                let to = t.pos - origin;
                let along = to.dot(d);
                if along < 0.0 || along > reach {
                    continue;
                }
                if (to - d * along).length() <= t.r * 1.35 {
                    v.push((along, *t));
                }
            }
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
            v
        };

        match p.verb {
            // instant, no travel: the line either reaches the ball or does not
            Verb::Pierce | Verb::Beam => {
                let line = along_ray(p.reach);
                if line.is_empty() {
                    out.hits.push(origin + d * p.reach.min(40.0));
                }
                for (i, (along, t)) in line.iter().enumerate() {
                    out.spawn(p.impact, origin + d * *along, d, 1.0, p.color);
                    // PIERCE keeps its force through the whole line; BEAM is
                    // the apex form and does not care either way
                    let falloff = if p.verb == Verb::Pierce { 1.0 } else { 1.0 / (1.0 + i as f32 * 0.35) };
                    out.hit(t.id, d * p.impulse * falloff);
                    out.hits.push(origin + d * *along);
                }
            }
            // radial: everything within reach gets shoved away from you, and
            // you get shoved the other way. The recoil is not a penalty, it
            // is the move — a tsunami launches you as much as the ball.
            Verb::Shove => {
                // radial: it does not aim, it just happens around you
                let mut any = false;
                for t in targets {
                    let to = t.pos - origin;
                    let dist = to.length();
                    if dist >= p.reach + t.r {
                        continue;
                    }
                    let n = to.normalize_or(d);
                    let falloff = 1.0 - (dist / (p.reach + t.r)).clamp(0.0, 1.0);
                    out.hit(t.id, n * p.impulse * falloff);
                    out.self_impulse -= n * p.impulse * 0.45 * falloff;
                    out.hits.push(t.pos);
                    out.spawn(p.impact, t.pos, n, falloff, p.color);
                    any = true;
                }
                if !any {
                    out.self_impulse = -d * p.impulse * 0.30;
                }
                out.hits.push(origin);
            }
            // THE QUANTUM GRAVITY LEAP. Silent, instant, no flash at the
            // origin — and a ground wave scheduled where you land, which you
            // can warn about and cannot stop.
            // pulling the trigger on ICE OPENS the targeting view rather
            // than firing; committing is a separate act (see aim_commit)
            Verb::Blink => {
                self.aim = Some(Aim { pos: origin + d * 12.0, from: origin, charge: 0.0 });
                out.spawn(FxKind::Distort, origin, d, 1.0, p.color);
                self.cool = 0.0; // the cooldown starts when you actually leap
            }
            // the hook: first target on the line, else the surface it meets.
            // Terrain resolution is the caller's (it owns the field), so a
            // miss on targets reports the ray and lets the caller anchor it.
            Verb::Grapple => {
                let line = along_ray(p.reach);
                if let Some((along, t)) = line.first() {
                    self.hook = Some(Hook { anchor: t.pos, on: Some(t.id), reel_self: false, life: 2.5 });
                    out.spawn(FxKind::Cable, origin, t.pos, 1.0, p.color);
                    let _ = along;
                } else {
                    let anchor = origin + d * p.reach;
                    self.hook = Some(Hook { anchor, on: None, reel_self: true, life: 2.5 });
                    out.spawn(FxKind::Cable, origin, anchor, 1.0, p.color);
                }
            }
            // recoil weapon: firing backwards is the fastest way to move
            Verb::Thrust => {
                out.self_impulse = -d * p.impulse;
                self.shots.push(Shot { pos: origin, vel: d * p.speed, life: p.reach / p.speed, form: self.form as u8, bounces: 0 });
            }
            Verb::Vortex => {
                let at = origin + d * (p.reach * 0.9);
                self.vortex = Some(Vortex { pos: at, r: p.reach, life: 4.5 });
                out.hits.push(at);
                out.spawn(FxKind::Swirl, at, Vec3::Y, p.reach, p.color);
            }
            Verb::Cone => {
                // deterministic fan rather than random spread: a cone you can
                // learn is more useful than one you cannot
                let up = if d.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
                let right = d.cross(up).normalize_or(Vec3::X);
                let n = p.pellets.max(1);
                for i in 0..n {
                    let f = if n == 1 { 0.0 } else { (i as f32 / (n - 1) as f32) * 2.0 - 1.0 };
                    let a = f * p.spread.to_radians();
                    let dv = (d * a.cos() + right * a.sin()).normalize_or(d);
                    self.shots.push(Shot { pos: origin, vel: dv * p.speed, life: p.reach / p.speed, form: self.form as u8, bounces: 0 });
                }
            }
            // everything else travels
            _ => {
                let bounces = if p.verb == Verb::Bank { 3 } else { 0 };
                self.shots.push(Shot {
                    pos: origin,
                    vel: d * p.speed,
                    life: p.reach / p.speed.max(1.0),
                    form: self.form as u8,
                    bounces,
                });
            }
        }
        out
    }

    /// Advance shots, pools, the vortex and the pending wave.
    ///
    /// `sdf` is the world distance field; a shot that finds itself inside
    /// geometry either dies or, for METAL, bounces off the surface normal.
    pub fn step(
        &mut self,
        dt: f32,
        targets: &[Target],
        sdf: &dyn Fn(Vec3) -> f32,
        normal: &dyn Fn(Vec3) -> Vec3,
    ) -> Outcome {
        let mut out = Outcome::default();
        self.cool = (self.cool - dt).max(0.0);

        let mut keep = Vec::with_capacity(self.shots.len());
        for mut s in self.shots.drain(..) {
            let p = &TABLE[s.form as usize];
            s.life -= dt;
            let next = s.pos + s.vel * dt;

            // a target first: it is the thing worth hitting
            if let Some(t) = targets.iter().find(|t| (next - t.pos).length() <= t.r + 0.25) {
                let along = s.vel.normalize_or(Vec3::NEG_Z);
                out.hit(t.id, along * p.impulse);
                out.hits.push(next);
                if p.slow > 0.0 {
                    out.slowed.push((t.id, p.slow));
                }
                match p.verb {
                    Verb::Pool => self.pools.push(Pool { pos: next, r: 5.0, life: 6.0 }),
                    // FLAK lifts what it catches rather than only pushing it
                    Verb::Flak => out.hit(t.id, Vec3::Y * p.impulse * 0.55),
                    // CHAIN arcs on to whatever else is nearest
                    Verb::Chain => {
                        if let Some(n2) = targets
                            .iter()
                            .filter(|o| o.id != t.id)
                            .min_by(|a, b| {
                                (a.pos - next).length().total_cmp(&(b.pos - next).length())
                            })
                            .filter(|o| (o.pos - next).length() < 18.0)
                        {
                            out.hit(n2.id, (n2.pos - next).normalize_or(along) * p.impulse * 0.6);
                            out.hits.push(n2.pos);
                        } else {
                            out.hit(t.id, along * p.impulse * 0.4);
                        }
                    }
                    _ => {}
                }
                continue;
            }

            if sdf(next) < 0.15 {
                if s.bounces > 0 {
                    // BANK: reflect and keep going, so a wall is a tool
                    let n = normal(next);
                    s.vel = s.vel - n * (2.0 * s.vel.dot(n));
                    s.bounces -= 1;
                    s.pos = next + n * 0.2;
                    out.hits.push(next);
                    if s.life > 0.0 {
                        keep.push(s);
                    }
                    continue;
                }
                if p.verb == Verb::Pool {
                    self.pools.push(Pool { pos: next, r: 5.0, life: 6.0 });
                }
                out.hits.push(next);
                continue;
            }
            s.pos = next;
            if s.life > 0.0 {
                keep.push(s);
            }
        }
        self.shots = keep;

        // pools drag anything crossing them
        let mut slowed: Vec<(u32, f32)> = Vec::new();
        self.pools.retain_mut(|pl| {
            pl.life -= dt;
            for t in targets {
                if (t.pos - pl.pos).length() < pl.r + t.r {
                    slowed.push((t.id, 0.45));
                }
            }
            pl.life > 0.0
        });
        out.slowed.extend(slowed);

        // the vortex pulls, which is a weapon that plays for position
        if let Some(v) = &mut self.vortex {
            v.life -= dt;
            for t in targets {
                let to = v.pos - t.pos;
                let d = to.length();
                if d < v.r && d > 0.05 {
                    let pull = (1.0 - d / v.r) * 14.0 * dt;
                    out.hits_on.push((t.id, to / d * pull));
                }
            }
            if v.life <= 0.0 {
                self.vortex = None;
            }
        }

        // the leap's wave: it arrives late and hits everything indifferently
        if let Some(w) = &mut self.wave {
            w.timer -= dt;
            if w.timer <= 0.0 {
                // it hits everything indifferently, including whatever you
                // were standing next to
                for t in targets {
                    let to = t.pos - w.pos;
                    let d = to.length();
                    if d < w.r {
                        let f = 1.0 - d / w.r;
                        out.hit(t.id, (to.normalize_or(Vec3::Y) + Vec3::Y * 0.6) * 18.0 * f);
                    }
                }
                out.hits.push(w.pos);
                self.wave = None;
            }
        }
        out
    }

    // ------------------------------------------------------------ leap ---

    /// Fly the targeting view. Free in every direction and UNBOUNDED, because
    /// the leap is: the script has Ice Clones crossing lightyears. Holding it
    /// winds up `charge`, which is what drives the distortion — and the
    /// distortion is the tell, so a long, careful aim is a loud one.
    pub fn aim_move(&mut self, delta: Vec3, dt: f32) -> Vec<FxEvent> {
        let Some(a) = &mut self.aim else { return Vec::new() };
        a.pos += delta;
        a.charge = (a.charge + dt * 0.55).min(1.0);
        let p = &TABLE[2];
        // space bends around both ends: where you are and where you are going
        vec![
            FxEvent { kind: FxKind::Distort, pos: a.pos, dir: Vec3::Y, scale: 0.5 + a.charge * 2.5, color: p.color },
            FxEvent { kind: FxKind::Distort, pos: a.from, dir: Vec3::Y, scale: 0.3 + a.charge * 1.2, color: p.color },
        ]
    }

    /// Commit the leap. Silent and instant at the origin — no flash, nothing
    /// to react to — and it schedules the wave that arrives 2.2 s later at
    /// the ARRIVAL point, which the jumper can warn about and cannot stop.
    pub fn aim_commit(&mut self, boost: f32) -> Outcome {
        let mut out = Outcome::default();
        let p = &TABLE[2];
        let Some(a) = self.aim.take() else { return out };
        if boost < p.cost {
            return out; // not enough to make the jump; the view just closes
        }
        out.fired = true;
        out.teleport = Some(a.pos);
        self.cool = p.cooldown;
        // radius grows with the distance travelled: a longer jump displaces
        // more, which is the only way the wave scales with anything
        let dist = (a.pos - a.from).length();
        let r = (10.0 + dist * 0.10).min(40.0);
        self.wave = Some(Wave { pos: a.pos, timer: 2.2, r });
        // the arrival itself is a ripple, not a bang — the bang is late
        out.spawn(FxKind::Distort, a.pos, Vec3::Y, 1.0 + a.charge, p.color);
        out.spawn(FxKind::Ripple, a.pos, Vec3::Y, r * 0.25, p.color);
        out
    }

    pub fn aim_cancel(&mut self) {
        self.aim = None;
    }

    // --------------------------------------------------------- grapple ---

    /// Anchor the live hook to a point the caller resolved against terrain.
    /// The weapon cannot see the world; the caller can.
    pub fn hook_anchor_at(&mut self, p: Vec3) {
        if let Some(h) = &mut self.hook {
            if h.on.is_none() {
                h.anchor = p;
            }
        }
    }

    pub fn hook_release(&mut self) {
        self.hook = None;
    }

    /// Reel. A light target comes to you; a heavy one — or bare terrain —
    /// pulls you to it, which is the traversal half of the tool.
    pub fn hook_step(&mut self, dt: f32, self_pos: Vec3, targets: &[Target]) -> Outcome {
        let mut out = Outcome::default();
        let p = &TABLE[12];
        let Some(h) = &mut self.hook else { return out };
        h.life -= dt;
        let anchor = match h.on.and_then(|id| targets.iter().find(|t| t.id == id)) {
            Some(t) => t.pos,
            None => h.anchor,
        };
        let to = anchor - self_pos;
        let d = to.length();
        out.spawn(FxKind::Cable, self_pos, anchor, 1.0, p.color);
        if d > 1.5 {
            let dir = to / d;
            if h.reel_self || h.on.is_none() {
                out.self_impulse += dir * p.impulse * dt;
            } else if let Some(id) = h.on {
                out.hit(id, -dir * p.impulse * dt);
            }
        }
        if h.life <= 0.0 || d <= 1.5 {
            self.hook = None;
        }
        out
    }

    /// LOCK ON. The camera and the aim want a subject, and picking it by
    /// "nearest to where you are looking" beats "nearest to you" — you can
    /// choose, and choosing between two bosses is a decision worth having.
    ///
    /// Keeps the current lock while it stays roughly in view, so glancing
    /// away does not drop it mid-fight.
    pub fn update_lock(&mut self, origin: Vec3, aim: Vec3, targets: &[Target], max_range: f32) {
        let d = aim.normalize_or(Vec3::NEG_Z);
        let score = |t: &Target| -> Option<f32> {
            let to = t.pos - origin;
            let dist = to.length();
            if dist > max_range || dist < 1e-3 {
                return None;
            }
            let cos = to.normalize().dot(d);
            // roughly a 75-degree half-cone to acquire
            (cos > 0.26).then(|| cos - dist / (max_range * 6.0))
        };
        // a held lock survives a wider cone than it takes to acquire one
        if let Some(id) = self.locked {
            if let Some(t) = targets.iter().find(|t| t.id == id) {
                let to = t.pos - origin;
                if to.length() <= max_range * 1.25
                    && to.normalize_or(d).dot(d) > 0.05
                {
                    return;
                }
            }
        }
        self.locked = targets
            .iter()
            .filter_map(|t| score(t).map(|s| (s, t.id)))
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id);
    }

    /// Where the camera should point, if anything is locked.
    pub fn lock_pos(&self, targets: &[Target]) -> Option<Vec3> {
        let id = self.locked?;
        targets.iter().find(|t| t.id == id).map(|t| t.pos)
    }

    /// Seconds until the pending gravity wave lands, for the HUD warning.
    /// The jumper knows it is coming and can call it out; that is the whole
    /// social contract of the move.
    pub fn wave_eta(&self) -> Option<f32> {
        self.wave.map(|w| w.timer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat() -> (impl Fn(Vec3) -> f32, impl Fn(Vec3) -> Vec3) {
        (|p: Vec3| p.y, |_p: Vec3| Vec3::Y)
    }

    #[test]
    fn every_form_does_something_distinct() {
        // the point of the rewrite: no two forms should share a verb
        let mut seen = Vec::new();
        for f in TABLE.iter() {
            assert!(!seen.contains(&f.verb), "{} duplicates a verb", f.name);
            seen.push(f.verb);
        }
        assert_eq!(seen.len(), FORMS);
    }

    #[test]
    fn every_form_reaches_a_target_inside_its_own_range() {
        let (sdf, nrm) = flat();
        for i in 0..FORMS {
            let p = &TABLE[i];
            // inside THIS form's reach: TSUNAMI is a 9 m shove and PHOTON
            // carries 120, so one fixed distance tests neither of them
            let at = Vec3::new(0.0, 1.0, -(p.reach * 0.5).min(30.0));
            let tg = [Target { id: 7, pos: at, r: 1.6 }];
            let mut w = Weapons::new();
            w.form = i;
            let o = w.fire(Vec3::new(0.0, 1.0, 0.0), Vec3::NEG_Z, &tg, 100.0);
            assert!(o.fired, "{} refused to fire with a full tank", p.name);
            // check the wave BEFORE stepping: it lands at 2.2 s and the loop
            // below runs six seconds, so by the end it has already gone
            // ICE opens the targeting view rather than firing; committing
            // is the second act, so drive it that way
            if p.verb == Verb::Blink {
                assert!(w.aim.is_some(), "ICE did not open the targeting view");
                w.aim_move(Vec3::new(0.0, 0.0, -18.0), 0.5);
                let c = w.aim_commit(100.0);
                assert!(c.teleport.is_some(), "ICE did not leap on commit");
                assert!(w.wave_eta().is_some(), "ICE scheduled no wave");
                let mut late = 0.0f32;
                for _ in 0..360 {
                    late += w.step(1.0 / 60.0, &tg, &sdf, &nrm).total_impulse();
                }
                assert!(late > 0.5, "the ICE wave never arrived ({late})");
                continue;
            }
            // the hook does its work by reeling, not by the shot
            if p.verb == Verb::Grapple {
                assert!(w.hook.is_some(), "GRAPPLE put no line out");
                // a target on the line gets reeled IN; bare terrain reels
                // YOU. Either counts as the hook doing its job.
                let mut pull = 0.0f32;
                for _ in 0..120 {
                    let o = w.hook_step(1.0 / 60.0, Vec3::new(0.0, 1.0, 0.0), &tg);
                    pull += o.self_impulse.length() + o.total_impulse();
                }
                assert!(pull > 0.5, "the hook never pulled ({pull})");
                continue;
            }
            let mut moved = o.total_impulse();
            for _ in 0..360 {
                moved += w.step(1.0 / 60.0, &tg, &sdf, &nrm).total_impulse();
            }
            {
                assert!(moved > 0.5, "{} never touched the target ({moved})", p.name);
            }
        }
    }

    #[test]
    fn chain_arcs_to_a_second_target() {
        let (sdf, nrm) = flat();
        let tg = [
            Target { id: 1, pos: Vec3::new(0.0, 1.0, -20.0), r: 1.6 },
            Target { id: 2, pos: Vec3::new(6.0, 1.0, -22.0), r: 1.6 },
        ];
        let mut w = Weapons::new();
        w.form = 8; // ELECTRICITY
        w.fire(Vec3::new(0.0, 1.0, 0.0), Vec3::NEG_Z, &tg, 100.0);
        let mut ids = std::collections::HashSet::new();
        for _ in 0..360 {
            for (id, _) in w.step(1.0 / 60.0, &tg, &sdf, &nrm).hits_on {
                ids.insert(id);
            }
        }
        assert!(ids.contains(&2), "the arc never jumped ({ids:?})");
    }

    #[test]
    fn pierce_passes_through_a_line_of_targets() {
        let tg = [
            Target { id: 1, pos: Vec3::new(0.0, 1.0, -10.0), r: 1.6 },
            Target { id: 2, pos: Vec3::new(0.0, 1.0, -20.0), r: 1.6 },
            Target { id: 3, pos: Vec3::new(0.0, 1.0, -30.0), r: 1.6 },
        ];
        let mut w = Weapons::new();
        w.form = 7; // TITANIUM
        let o = w.fire(Vec3::new(0.0, 1.0, 0.0), Vec3::NEG_Z, &tg, 100.0);
        assert_eq!(o.hits_on.len(), 3, "railshot stopped early");
    }

    #[test]
    fn lock_follows_where_you_look_and_holds() {
        let tg = [
            Target { id: 1, pos: Vec3::new(-20.0, 0.0, -20.0), r: 2.0 },
            Target { id: 2, pos: Vec3::new(20.0, 0.0, -20.0), r: 2.0 },
        ];
        let mut w = Weapons::new();
        w.update_lock(Vec3::ZERO, Vec3::new(-1.0, 0.0, -1.0), &tg, 80.0);
        assert_eq!(w.locked, Some(1));
        w.update_lock(Vec3::ZERO, Vec3::NEG_Z, &tg, 80.0);
        assert_eq!(w.locked, Some(1), "a small look-away dropped the lock");
        assert!(w.lock_pos(&tg).is_some());
    }

    #[test]
    fn the_leap_has_no_range_limit() {
        let mut w = Weapons::new();
        w.form = 2;
        w.fire(Vec3::ZERO, Vec3::NEG_Z, &[], 100.0);
        // fly the targeting view an absurd distance: the script has Ice
        // Clones crossing lightyears, so nothing here may clamp it
        for _ in 0..500 {
            w.aim_move(Vec3::new(0.0, 0.0, -400.0), 1.0 / 60.0);
        }
        let o = w.aim_commit(100.0);
        let at = o.teleport.expect("no leap");
        assert!(at.z < -100_000.0, "the leap was clamped at {at:?}");
        assert!(w.wave_eta().is_some());
    }

    #[test]
    fn aiming_the_leap_distorts_at_both_ends() {
        let mut w = Weapons::new();
        w.form = 2;
        w.fire(Vec3::ZERO, Vec3::NEG_Z, &[], 100.0);
        let fx = w.aim_move(Vec3::new(0.0, 0.0, -5.0), 0.25);
        assert_eq!(fx.len(), 2, "space should bend at both ends of the jump");
        assert!(fx.iter().all(|e| e.kind == FxKind::Distort));
        // holding winds the distortion up — the tell that gives you away
        let a = fx[0].scale;
        for _ in 0..60 {
            w.aim_move(Vec3::ZERO, 1.0 / 60.0);
        }
        let b = w.aim_move(Vec3::ZERO, 0.0)[0].scale;
        assert!(b > a, "the distortion did not grow while held ({a} -> {b})");
    }

    #[test]
    fn a_heavy_anchor_pulls_you_and_a_light_one_comes_to_you() {
        let mut w = Weapons::new();
        w.form = 12; // GRAPPLE
        // no target on the line: it bites terrain, so YOU travel
        w.fire(Vec3::ZERO, Vec3::NEG_Z, &[], 100.0);
        let trav = w.hook_step(0.1, Vec3::ZERO, &[]);
        assert!(trav.self_impulse.length() > 0.0, "terrain hook did not pull you");

        // a target on the line: it comes to you instead
        let tg = [Target { id: 3, pos: Vec3::new(0.0, 0.0, -20.0), r: 1.6 }];
        let mut w2 = Weapons::new();
        w2.form = 12;
        w2.fire(Vec3::ZERO, Vec3::NEG_Z, &tg, 100.0);
        let o = w2.hook_step(0.1, Vec3::ZERO, &tg);
        assert!(!o.hits_on.is_empty(), "target hook pulled nothing");
        assert!(o.hits_on[0].1.z > 0.0, "the target was pushed away, not reeled in");
    }

    #[test]
    fn every_form_draws_something() {
        for i in 0..FORMS {
            let mut w = Weapons::new();
            w.form = i;
            let o = w.fire(Vec3::ZERO, Vec3::NEG_Z, &[], 100.0);
            assert!(!o.fx.is_empty(), "{} fires invisibly", TABLE[i].name);
        }
    }

    #[test]
    fn wind_pushes_the_shooter_backwards() {
        let mut w = Weapons::new();
        w.form = 9; // WIND
        let o = w.fire(Vec3::ZERO, Vec3::NEG_Z, &[], 100.0);
        assert!(o.self_impulse.z > 0.0, "firing forward should push you back");
    }

    #[test]
    fn an_empty_tank_cannot_fire() {
        let mut w = Weapons::new();
        w.form = 11; // PHOTON, the most expensive
        let o = w.fire(Vec3::ZERO, Vec3::NEG_Z, &[], 0.0);
        assert!(!o.fired);
    }

    #[test]
    fn the_leap_wave_lands_late_and_alone() {
        let (sdf, nrm) = flat();
        let mut w = Weapons::new();
        w.form = 2; // ICE
        let tg = [Target { id: 1, pos: Vec3::new(0.0, 1.0, -26.0), r: 1.6 }];
        w.fire(Vec3::new(0.0, 1.0, 0.0), Vec3::NEG_Z, &tg, 100.0);
        w.aim_move(Vec3::new(0.0, 0.0, -14.0), 0.5);
        w.aim_commit(100.0);
        let mut early = 0.0f32;
        for _ in 0..120 {
            early += w.step(1.0 / 60.0, &tg, &sdf, &nrm).total_impulse();
        }
        assert!(early < 1e-6, "the leap was not silent");
        let mut late = 0.0f32;
        for _ in 0..60 {
            late += w.step(1.0 / 60.0, &tg, &sdf, &nrm).total_impulse();
        }
        assert!(late > 1.0, "the wave never arrived");
    }
}
