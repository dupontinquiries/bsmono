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

pub const FORMS: usize = 12;

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
) -> Form {
    Form { name, verb, cooldown, cost, impulse, reach, speed, spread, pellets, slow, color }
}

/// Colours are carried over from `ex_weapon_profile` so the forms still read
/// as themselves; everything else is rebuilt around the verb.
pub const TABLE: [Form; FORMS] = [
    f("WATER", Verb::Push, 0.06, 1.2, 1.6, 34.0, 46.0, 0.0, 1, 0.0, [0.35, 0.67, 1.0, 1.0]),
    f("TSUNAMI", Verb::Shove, 0.85, 14.0, 15.0, 9.0, 0.0, 0.0, 1, 0.0, [0.27, 0.59, 0.90, 1.0]),
    f("ICE", Verb::Blink, 3.20, 26.0, 9.0, 26.0, 0.0, 0.0, 1, 0.55, [0.59, 0.86, 1.0, 1.0]),
    f("FIRE", Verb::Flak, 0.55, 8.0, 9.5, 40.0, 30.0, 0.0, 1, 0.0, [1.0, 0.47, 0.16, 1.0]),
    f("MAGMA", Verb::Pool, 1.30, 16.0, 4.0, 26.0, 20.0, 0.0, 1, 0.45, [0.90, 0.35, 0.12, 1.0]),
    f("BLIZZARD", Verb::Cone, 0.70, 10.0, 2.4, 18.0, 32.0, 14.0, 7, 0.35, [0.75, 0.90, 1.0, 1.0]),
    f("METAL", Verb::Bank, 0.30, 5.0, 7.0, 60.0, 40.0, 0.0, 1, 0.0, [0.71, 0.73, 0.76, 1.0]),
    f("TITANIUM", Verb::Pierce, 1.10, 20.0, 13.0, 70.0, 0.0, 0.0, 1, 0.0, [0.59, 0.63, 0.69, 1.0]),
    f("ELECTRICITY", Verb::Chain, 0.90, 15.0, 8.0, 45.0, 34.0, 0.0, 1, 0.20, [0.55, 0.71, 1.0, 1.0]),
    f("WIND", Verb::Thrust, 0.22, 3.0, 3.0, 22.0, 55.0, 0.0, 1, 0.0, [0.82, 0.92, 0.84, 1.0]),
    f("TORNADO", Verb::Vortex, 2.60, 22.0, 0.0, 11.0, 26.0, 0.0, 1, 0.0, [0.78, 0.86, 0.82, 1.0]),
    f("PHOTON", Verb::Beam, 2.40, 34.0, 26.0, 120.0, 0.0, 0.0, 1, 0.0, [1.0, 0.98, 0.90, 1.0]),
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
    pub fired: bool,
}

impl Outcome {
    fn hit(&mut self, id: u32, dv: Vec3) {
        self.hits_on.push((id, dv));
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
            Verb::Blink => {
                let to = origin + d * p.reach;
                out.teleport = Some(to);
                self.wave = Some(Wave { pos: to, timer: 2.2, r: 14.0 });
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
            let scheduled = w.wave_eta().is_some();
            let mut moved = o.total_impulse();
            for _ in 0..360 {
                moved += w.step(1.0 / 60.0, &tg, &sdf, &nrm).total_impulse();
            }
            if p.verb == Verb::Blink {
                assert!(o.teleport.is_some(), "ICE did not leap");
                assert!(scheduled, "ICE scheduled no wave");
                assert!(moved > 0.5, "the ICE wave never arrived ({moved})");
            } else {
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
