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
    /// the Watch: which element is attuned, and whether it is mid-swap
    pub watch: Watch,
    /// fitted addons; only the first MOD_SLOTS count
    pub mods: Vec<Mod>,
    /// Billy's held star — charge it, release it
    pub star: Star,
}

/// STAR-FIRE, from exengine/weapon.h. Billy absorbs stars and throws them:
/// hold to charge, release for a multiplier from 1 to 6. A release at or
/// above STAR_FULL is a real star — a nova — and it locks the mechanic out
/// for STAR_COOLDOWN afterwards. That lockout IS the cost; without it the
/// right play would always be to hold.
pub const STAR_CHARGE_SEC: f32 = 1.4;
pub const STAR_FULL: f32 = 0.85;
pub const STAR_COOLDOWN: f32 = 6.0;

#[derive(Clone, Copy, Default)]
pub struct Star {
    pub charge: f32,
    pub charging: bool,
    pub cooldown: f32,
}

impl Star {
    pub fn ready(&self) -> bool {
        self.cooldown <= 0.0
    }
    pub fn begin(&mut self) {
        if self.ready() {
            self.charging = true;
            self.charge = 0.0;
        }
    }
    pub fn update(&mut self, dt: f32, held: bool) {
        self.cooldown = (self.cooldown - dt).max(0.0);
        if self.charging && held {
            self.charge = (self.charge + dt / STAR_CHARGE_SEC).min(1.0);
        } else if !held {
            self.charging = false;
        }
    }
    /// Returns the damage multiplier (1..6) and whether it was a full star.
    pub fn release(&mut self) -> (f32, bool) {
        let c = self.charge;
        self.charging = false;
        self.charge = 0.0;
        let full = c >= STAR_FULL;
        if full {
            self.cooldown = STAR_COOLDOWN;
        }
        (1.0 + c * 5.0, full)
    }
    /// 0..1 remaining lockout, for the HUD.
    pub fn cooldown_frac(&self) -> f32 {
        (self.cooldown / STAR_COOLDOWN).clamp(0.0, 1.0)
    }
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
            watch: Watch::default(),
            mods: Vec::new(),
            star: Star::default(),
        }
    }

    pub fn profile(&self) -> &'static Form {
        &TABLE[self.form.min(FORMS - 1)]
    }

    /// The current form as the Watch and the fitted mods leave it.
    pub fn effective(&self) -> Effective {
        self.profile().effective(self.watch.active(), &self.mods)
    }

    /// Movement multiplier the attuned element grants, for the caller to
    /// apply to the robot's speed. Air runs; Rock does not.
    pub fn move_mul(&self) -> f32 {
        self.watch.active().profile().move_mul
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
        // the element and the fitted mods have their say BEFORE anything
        // else: a rail core means this really is a railshot, not a push with
        // a longer range
        let p = self.effective();
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

// ==========================================================================
// ELEMENTS — ported from exengine/element.h
// ==========================================================================
//
// The coalition races from the film material: Ice, Fire, Air, Electricity,
// Vine, Rock, Water. Two things ride on the element and neither is a
// reskin — a combat profile that changes how you fight, and a
// rock-paper-scissors graph where each element beats the next two and loses
// to the previous two, so everything has exactly two good and two bad
// matchups and nothing is strictly best.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Element {
    None,
    Ice,
    Fire,
    Air,
    Electric,
    Vine,
    Rock,
    Water,
}

pub const ELEMENTS: [Element; 7] = [
    Element::Ice,
    Element::Fire,
    Element::Air,
    Element::Electric,
    Element::Vine,
    Element::Rock,
    Element::Water,
];

/// Damage, range, cooldown and movement, straight from `ex_element_profile`.
/// Ice controls, Fire burns close, Air is mobility, Electric reaches, Vine
/// sustains, Rock hits hard and slow, Water is the flexible baseline.
pub struct ElemProfile {
    pub damage: f32,
    pub range: f32,
    pub cooldown: f32,
    pub move_mul: f32,
}

impl Element {
    pub fn profile(self) -> ElemProfile {
        let (d, r, c, m) = match self {
            Element::Ice => (0.90, 1.40, 1.10, 0.95),
            Element::Fire => (1.50, 0.70, 1.00, 1.00),
            Element::Air => (0.70, 1.00, 0.60, 1.35),
            Element::Electric => (1.10, 1.50, 0.90, 1.00),
            Element::Vine => (1.00, 1.00, 1.00, 0.90),
            Element::Rock => (1.40, 0.60, 1.30, 0.75),
            Element::Water => (1.00, 1.15, 0.95, 1.05),
            Element::None => (1.00, 1.00, 1.00, 1.00),
        };
        ElemProfile { damage: d, range: r, cooldown: c, move_mul: m }
    }

    pub fn name(self) -> &'static str {
        match self {
            Element::None => "none",
            Element::Ice => "ice",
            Element::Fire => "fire",
            Element::Air => "air",
            Element::Electric => "electric",
            Element::Vine => "vine",
            Element::Rock => "rock",
            Element::Water => "water",
        }
    }

    fn ring(self) -> Option<usize> {
        ELEMENTS.iter().position(|e| *e == self)
    }

    /// 1.5 for a strong matchup, 0.66 for a weak one, 1.0 otherwise.
    pub fn advantage(self, defender: Element) -> f32 {
        let (Some(a), Some(d)) = (self.ring(), defender.ring()) else {
            return 1.0;
        };
        let n = ELEMENTS.len();
        let step = (d + n - a) % n;
        if step == 1 || step == 2 {
            1.5
        } else if step == n - 1 || step == n - 2 {
            0.66
        } else {
            1.0
        }
    }
}

/// THE WATCH. Cycling element is not free: there is an attune delay, and
/// while attuning the active element is None — you fight with no bonus at
/// all. That is what stops a hot-swap every shot.
pub const ATTUNE_SEC: f32 = 0.5;

#[derive(Clone, Copy)]
pub struct Watch {
    current: Element,
    pending: Element,
    timer: f32,
    attuning: bool,
}

impl Default for Watch {
    fn default() -> Watch {
        Watch::new(Element::Ice)
    }
}

impl Watch {
    pub fn new(start: Element) -> Watch {
        Watch { current: start, pending: start, timer: 0.0, attuning: false }
    }

    /// Denied mid-attune: you commit to a switch once you start it.
    pub fn switch(&mut self, to: Element) -> bool {
        if self.attuning || to == self.current {
            return false;
        }
        self.pending = to;
        self.timer = ATTUNE_SEC;
        self.attuning = true;
        true
    }

    pub fn cycle(&mut self) -> bool {
        let i = self.current.ring().map_or(0, |i| (i + 1) % ELEMENTS.len());
        self.switch(ELEMENTS[i])
    }

    pub fn update(&mut self, dt: f32) {
        if !self.attuning {
            return;
        }
        self.timer -= dt;
        if self.timer <= 0.0 {
            self.current = self.pending;
            self.attuning = false;
            self.timer = 0.0;
        }
    }

    /// None while attuning — the window where the Watch is between forms.
    pub fn active(&self) -> Element {
        if self.attuning {
            Element::None
        } else {
            self.current
        }
    }

    pub fn attuning(&self) -> bool {
        self.attuning
    }
}

// ==========================================================================
// MODS — ported from exengine/weaponmod.h
// ==========================================================================
//
// Sixteen addons, three slots. Six of them are CONVERSIONS: they do not
// nudge a number, they change the verb. A rail core turns anything into a
// railshot, a scatter barrel turns anything into a cone. That matters more
// here than it did in the source, because the verb IS the weapon — so a
// conversion mod genuinely rebuilds the gun rather than retuning it.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mod {
    LongBarrel,
    Scope,
    ExtMag,
    Underbarrel,
    Stabilizer,
    Amplifier,
    ChainCoil,
    EnergonCell,
    RapidCoil,
    PierceCore,
    ScatterBarrel,
    RippleCoil,
    CryoRounds,
    FlakCore,
    PlasmaCore,
    SuppressCore,
}

pub const MOD_SLOTS: usize = 3;

impl Mod {
    pub fn name(self) -> &'static str {
        match self {
            Mod::LongBarrel => "LONG BARREL",
            Mod::Scope => "SCOPE",
            Mod::ExtMag => "EXTENDED MAG",
            Mod::Underbarrel => "UNDERBARREL",
            Mod::Stabilizer => "STABILIZER",
            Mod::Amplifier => "AMPLIFIER",
            Mod::ChainCoil => "CHAIN COIL",
            Mod::EnergonCell => "ENERGON CELL",
            Mod::RapidCoil => "RAPID COIL",
            Mod::PierceCore => "PIERCE CORE",
            Mod::ScatterBarrel => "SCATTER BARREL",
            Mod::RippleCoil => "RIPPLE COIL",
            Mod::CryoRounds => "CRYO ROUNDS",
            Mod::FlakCore => "FLAK CORE",
            Mod::PlasmaCore => "PLASMA CORE",
            Mod::SuppressCore => "SUPPRESS CORE",
        }
    }

    /// The six that rebuild the gun rather than retuning it.
    pub fn conversion(self) -> Option<Verb> {
        match self {
            Mod::PierceCore => Some(Verb::Pierce),
            Mod::ScatterBarrel => Some(Verb::Cone),
            Mod::RippleCoil => Some(Verb::Bank),
            Mod::FlakCore => Some(Verb::Flak),
            Mod::PlasmaCore => Some(Verb::Chain),
            Mod::SuppressCore => Some(Verb::Beam),
            _ => None,
        }
    }
}

/// A form after the element and the fitted mods have had their say. Same
/// field names as `Form`, so the firing code reads identically either way.
#[derive(Clone, Copy)]
pub struct Effective {
    pub verb: Verb,
    pub cooldown: f32,
    pub cost: f32,
    pub impulse: f32,
    pub reach: f32,
    pub speed: f32,
    pub spread: f32,
    pub pellets: u32,
    pub slow: f32,
    pub chains: u32,
    pub color: [f32; 4],
    pub muzzle: FxKind,
    pub impact: FxKind,
    pub move_mul: f32,
}

impl Form {
    pub fn effective(&self, elem: Element, mods: &[Mod]) -> Effective {
        let ep = elem.profile();
        let mut e = Effective {
            verb: self.verb,
            cooldown: self.cooldown * ep.cooldown,
            cost: self.cost,
            impulse: self.impulse * ep.damage,
            reach: self.reach * ep.range,
            speed: self.speed,
            spread: self.spread,
            pellets: self.pellets,
            slow: self.slow,
            chains: if self.verb == Verb::Chain { 1 } else { 0 },
            color: self.color,
            muzzle: self.muzzle,
            impact: self.impact,
            move_mul: ep.move_mul,
        };
        for m in mods.iter().take(MOD_SLOTS) {
            match m {
                Mod::LongBarrel => { e.reach *= 1.35; e.impulse *= 1.15; e.cooldown *= 1.15; }
                Mod::Scope => { e.spread *= 0.55; e.reach *= 1.20; }
                Mod::ExtMag => { e.cooldown *= 0.75; }
                Mod::Underbarrel => { e.pellets += 1; e.spread += 4.0; }
                Mod::Stabilizer => { e.spread *= 0.60; }
                Mod::Amplifier => { e.impulse *= 1.30; e.cooldown *= 1.20; }
                Mod::ChainCoil => { e.chains += 1; }
                Mod::EnergonCell => { e.impulse *= 1.45; e.cooldown *= 1.30; }
                Mod::RapidCoil => { e.cooldown *= 0.60; e.impulse *= 0.75; }
                Mod::CryoRounds => { e.slow = e.slow.max(0.45); }
                Mod::PierceCore => { e.speed = 0.0; e.reach *= 1.25; e.cooldown *= 1.20; }
                Mod::ScatterBarrel => { e.pellets = e.pellets.max(1) + 4; e.spread += 10.0; }
                Mod::RippleCoil => {}
                Mod::FlakCore => {}
                Mod::PlasmaCore => { e.chains += 2; }
                Mod::SuppressCore => { e.reach *= 1.4; e.slow = e.slow.max(0.7); e.impulse *= 0.6; }
            }
            if let Some(v) = m.conversion() {
                e.verb = v;
            }
        }
        // a conversion to a travelling verb needs a speed to travel at
        if e.speed <= 0.0 && matches!(e.verb, Verb::Cone | Verb::Bank | Verb::Flak | Verb::Chain) {
            e.speed = 34.0;
        }
        e
    }
}

#[cfg(test)]
mod ported_tests {
    use super::*;

    #[test]
    fn every_element_has_two_good_and_two_bad_matchups() {
        for a in ELEMENTS {
            let mut good = 0;
            let mut bad = 0;
            for d in ELEMENTS {
                let x = a.advantage(d);
                if x > 1.2 { good += 1; }
                if x < 0.9 { bad += 1; }
            }
            assert_eq!(good, 2, "{} has {good} strong matchups", a.name());
            assert_eq!(bad, 2, "{} has {bad} weak matchups", a.name());
        }
        // and nothing beats itself
        for a in ELEMENTS {
            assert!((a.advantage(a) - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn attuning_costs_you_your_element() {
        let mut w = Watch::new(Element::Ice);
        assert_eq!(w.active(), Element::Ice);
        assert!(w.switch(Element::Fire));
        // the whole point: mid-swap you have no element at all
        assert_eq!(w.active(), Element::None);
        assert!(!w.switch(Element::Rock), "a switch was allowed mid-attune");
        w.update(ATTUNE_SEC * 0.5);
        assert_eq!(w.active(), Element::None);
        w.update(ATTUNE_SEC);
        assert_eq!(w.active(), Element::Fire);
    }

    #[test]
    fn the_watch_cycles_all_seven() {
        let mut w = Watch::new(Element::Ice);
        let mut seen = vec![w.active()];
        for _ in 0..6 {
            assert!(w.cycle());
            w.update(ATTUNE_SEC * 1.1);
            seen.push(w.active());
        }
        for e in ELEMENTS {
            assert!(seen.contains(&e), "{} never came round", e.name());
        }
    }

    #[test]
    fn element_changes_how_a_form_fights() {
        let base = TABLE[0].effective(Element::None, &[]);
        let fire = TABLE[0].effective(Element::Fire, &[]);
        let ice = TABLE[0].effective(Element::Ice, &[]);
        assert!(fire.impulse > base.impulse && fire.reach < base.reach, "fire should hit harder, closer");
        assert!(ice.reach > base.reach && ice.impulse < base.impulse, "ice should reach further, softer");
        let air = TABLE[0].effective(Element::Air, &[]);
        assert!(air.cooldown < base.cooldown && air.move_mul > 1.0, "air should be the mobile one");
    }

    #[test]
    fn conversion_mods_change_the_verb_not_the_numbers() {
        // WATER is a Push; a rail core should make it a railshot outright
        let e = TABLE[0].effective(Element::None, &[Mod::PierceCore]);
        assert_eq!(e.verb, Verb::Pierce);
        let s = TABLE[0].effective(Element::None, &[Mod::ScatterBarrel]);
        assert_eq!(s.verb, Verb::Cone);
        assert!(s.pellets > TABLE[0].pellets);
        // and a travelling conversion must have something to travel at
        let f = TABLE[7].effective(Element::None, &[Mod::FlakCore]); // TITANIUM is hitscan
        assert_eq!(f.verb, Verb::Flak);
        assert!(f.speed > 0.0, "a converted flak round had no speed");
    }

    #[test]
    fn a_full_star_costs_you_the_star() {
        let mut st = Star::default();
        assert!(st.ready());
        st.begin();
        // a light tap is a weak throw and costs nothing
        st.update(0.2, true);
        let (mul, full) = st.release();
        assert!(mul > 1.0 && mul < 2.0, "a tap gave {mul}x");
        assert!(!full);
        assert!(st.ready(), "a tap locked the star out");

        // a held charge is a nova, and then you have nothing for a while
        st.begin();
        for _ in 0..120 {
            st.update(1.0 / 60.0, true);
        }
        let (mul, full) = st.release();
        assert!(mul > 5.5, "a full hold only gave {mul}x");
        assert!(full);
        assert!(!st.ready(), "the nova did not lock out");
        assert!(st.cooldown_frac() > 0.9);
        st.begin();
        assert!(!st.charging, "it started charging while locked out");
    }

    #[test]
    fn the_element_reaches_the_gun() {
        // the wiring test: what fire() actually uses must be the effective
        // profile, not the base table row
        let mut w = Weapons::new();
        w.form = 0;
        w.watch = Watch::new(Element::Rock);
        let rock = w.effective();
        w.watch = Watch::new(Element::Ice);
        let ice = w.effective();
        assert!(rock.impulse > ice.impulse, "rock should hit harder than ice");
        assert!(ice.reach > rock.reach, "ice should out-range rock");
        w.mods.push(Mod::PierceCore);
        assert_eq!(w.effective().verb, Verb::Pierce, "the mod never reached the gun");
    }

    #[test]
    fn only_three_mods_are_ever_fitted() {
        let many = [Mod::Amplifier, Mod::Amplifier, Mod::Amplifier, Mod::Amplifier, Mod::Amplifier];
        let three = TABLE[0].effective(Element::None, &many[..3]);
        let five = TABLE[0].effective(Element::None, &many);
        assert!((three.impulse - five.impulse).abs() < 1e-4, "a fourth mod took effect");
    }
}
