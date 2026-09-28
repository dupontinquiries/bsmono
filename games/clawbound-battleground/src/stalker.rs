//! stalker -- Hollowstep, and the other of him that is always somewhere else.
//!
//! The horror-film stalker's rules, made into a kit (the rules, not anyone's
//! likeness: an ash-grey wildcat of the fog moors, not a man in a mask):
//!
//! * **He never runs** (the lowest base speed on the roster), and he never
//!   has to: there is always **another of him** ([`Other`]) standing
//!   somewhere else on the map. Left alone, it walks -- slowly, never
//!   running -- toward the nearest enemy cat within [`HUNT_R`], and stops
//!   at [`LURK`] from it, watching. When he (re)spawns it rises on the far
//!   side of the map's centre from him ([`elsewhere`]).
//! * **The terror radius.** Within [`TERROR_R`] of him *or of any other of
//!   him*, an enemy cat is afraid (`GodUnit::fear`, 0 at the edge, 1 at
//!   [`FEAR_NEAR`]): its heart pounds (`Event::Heartbeat`, faster the
//!   closer it is -- [`BEAT_SLOW`] to [`BEAT_FAST`] seconds a beat), its
//!   screen darkens at the edges, and its legs go heavy, up to
//!   [`FEAR_SLOW`]. Because his other frightens just as well as he does,
//!   the heartbeat says he is near, never which one of him.
//! * **The noise.** He and every other of him walk heavily: a footfall
//!   every [`STRIDE`] u (`Event::HeavyStep`), heard far further than a
//!   cat's step.
//! * **Step Between** (`Archetype::Swap`): he and his other trade places.
//!   Whoever was waiting by it is struck and stumbles.
//! * **Send the Other** (`Archetype::Send`): his other glides where he
//!   points and waits there [`HOLD_SECS`] before it hunts again.
//! * **Every Shadow** (`Archetype::Echoes`, the ultimate): two echoes rise
//!   beside him and two beside his other for a few seconds, each hunting
//!   the nearest enemy within [`ECHO_HUNT`] and striking what it reaches.
//!   While they stand, Step Between goes to whichever of them is nearest
//!   the aim and comes back in [`ULT_STEP_CD`] s.
//!
//! Headless; the scene draws the others as his silhouette, and the HUD the
//! fear.

use glam::Vec3;

use crate::consts::Team;
use crate::sim::{Event, Game, TargetRef};

/// How far the fear reaches from him or any other of him.
pub const TERROR_R: f32 = 20.0;
/// Within this, the fear is whole.
pub const FEAR_NEAR: f32 = 4.0;
/// A wholly afraid cat's legs: this much slower.
pub const FEAR_SLOW: f32 = 0.10;
/// Seconds between heartbeats at the edge of the fear, and at its heart.
pub const BEAT_SLOW: f32 = 1.1;
pub const BEAT_FAST: f32 = 0.42;
/// His other walks at this pace -- slower than any cat runs.
pub const WALK: f32 = 4.5;
/// It hunts cats within this of it...
pub const HUNT_R: f32 = 45.0;
/// ...and stops this far from the one it has found, watching.
pub const LURK: f32 = 7.0;
/// Sent somewhere, it waits there this long before it hunts again.
pub const HOLD_SECS: f32 = 4.0;
/// An echo hunts within this, walks at this, strikes at this reach, this
/// often.
pub const ECHO_HUNT: f32 = 10.0;
pub const ECHO_WALK: f32 = 7.0;
pub const ECHO_REACH: f32 = 2.4;
pub const ECHO_EVERY: f32 = 1.1;
/// Echoes rise this far to either side of him and of his other.
pub const ECHO_SPREAD: f32 = 2.6;
/// A heavy footfall every this many units walked.
pub const STRIDE: f32 = 2.4;
/// While his echoes stand, Step Between comes back this fast.
pub const ULT_STEP_CD: f32 = 2.5;
/// Where his other rises when he spawns: this far past the map's centre
/// from him (at most).
pub const ELSEWHERE: f32 = 32.0;

/// Another of him.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Other {
    pub team: Team,
    /// The Hollowstep it belongs to.
    pub owner: usize,
    pub pos: Vec3,
    pub yaw: f32,
    /// An echo of the ultimate (gone in `life` s), or his standing other.
    pub echo: bool,
    pub life: f32,
    /// Sent (`Send`): where it glides, and how fast.
    pub goal: Option<(Vec3, f32)>,
    /// Seconds it waits where it was sent.
    pub hold: f32,
    /// An echo's next strike, and what it deals.
    pub strike_t: f32,
    pub dmg: f32,
    /// Units walked since its last footfall.
    pub stride: f32,
    /// Seconds alive, for the scene.
    pub age: f32,
}

/// Is this ancestor a stalker (has the other and the fear)?
pub fn is_stalker(def: &crate::gods::GodDef) -> bool {
    matches!(def.pelt, crate::gods::PeltKind::Hollow)
}

/// How afraid a cat `d` units from the nearest of him is, 0..=1.
pub fn fear_at(d: f32) -> f32 {
    ((TERROR_R - d) / (TERROR_R - FEAR_NEAR)).clamp(0.0, 1.0)
}

/// Seconds to the next heartbeat at fear `f` (> 0).
pub fn beat_every(f: f32) -> f32 {
    BEAT_SLOW + (BEAT_FAST - BEAT_SLOW) * f.clamp(0.0, 1.0)
}

/// How dark the afraid cat's screen edges are, 0..=1: with the fear, and
/// pulsing with the heart -- darkest on the beat, easing off before the
/// next. `beat_t` is the seconds to the next beat (`GodUnit::beat_t`).
pub fn vignette(fear: f32, beat_t: f32) -> f32 {
    if fear <= 0.0 {
        return 0.0;
    }
    let every = beat_every(fear);
    let since = (every - beat_t).clamp(0.0, every);
    let pulse = (-since / 0.25).exp();
    (fear * (0.55 + 0.45 * pulse)).clamp(0.0, 1.0)
}

/// Where his other rises when he stands at `pos`: through the map's centre
/// and on past it, at most [`ELSEWHERE`] out -- somewhere else, always.
pub fn elsewhere(pos: Vec3) -> Vec3 {
    let flat = Vec3::new(-pos.x, 0.0, -pos.z);
    let len = flat.length();
    let at = if len > 1e-3 { flat / len * len.min(ELSEWHERE) } else { Vec3::new(0.0, 0.0, ELSEWHERE) };
    Vec3::new(at.x, crate::map::ground_y(at.x, at.z), at.z)
}

impl Game {
    /// The others of Hollowstep `gi`, as indices into `others`.
    pub fn others_of(&self, gi: usize) -> impl Iterator<Item = usize> + '_ {
        self.others.iter().enumerate().filter(move |(_, o)| o.owner == gi).map(|(i, _)| i)
    }

    /// His standing other (not an echo), if there is one.
    pub fn main_other(&self, gi: usize) -> Option<usize> {
        self.others.iter().position(|o| o.owner == gi && !o.echo)
    }

    /// Step Between: trade places with the other of him nearest `aim`
    /// (only his standing one, unless his echoes stand). Enemies within
    /// `radius` of where he arrives are struck and slowed. Returns whether
    /// he stepped.
    #[allow(clippy::too_many_arguments)]
    pub fn step_between(&mut self, gi: usize, aim: Vec3, radius: f32, dmg: f32, slow: f32, secs: f32) -> bool {
        let echoes = self.others.iter().any(|o| o.owner == gi && o.echo);
        let pick = self
            .others
            .iter()
            .enumerate()
            .filter(|(_, o)| o.owner == gi && (echoes || !o.echo))
            .min_by(|a, b| a.1.pos.distance_squared(aim).total_cmp(&b.1.pos.distance_squared(aim)))
            .map(|(i, _)| i);
        let Some(k) = pick else { return false };
        let (from, to) = (self.gods[gi].pos, self.others[k].pos);
        {
            let g = &mut self.gods[gi];
            g.pos = Vec3::new(to.x, crate::map::ground_y(to.x, to.z), to.z);
            g.dash = None;
            g.leap = None;
            g.perch = None;
            g.fall = None;
        }
        let o = &mut self.others[k];
        o.pos = Vec3::new(from.x, crate::map::ground_y(from.x, from.z), from.z);
        o.goal = None;
        o.hold = HOLD_SECS * 0.5;
        self.events.push(Event::SwapFx { from, to });
        let team = self.gods[gi].team;
        let at = self.gods[gi].pos;
        for t in self.aoe_targets(at, radius, team) {
            if dmg > 0.0 {
                self.deal_damage(t, dmg, true, Some(gi), 0.0, 0.0);
            }
            if let TargetRef::God(j) = t {
                self.gods[j].slow = self.gods[j].slow.max(slow);
                self.gods[j].slow_t = self.gods[j].slow_t.max(secs);
            }
        }
        if echoes {
            self.gods[gi].cds[0] = self.gods[gi].cds[0].min(ULT_STEP_CD);
        }
        true
    }

    /// Send the Other: his standing other glides toward `to`.
    pub fn send_other(&mut self, gi: usize, to: Vec3, speed: f32) {
        if let Some(k) = self.main_other(gi) {
            let to = crate::abilities::clamp_arena(Vec3::new(to.x, 0.0, to.z));
            self.others[k].goal = Some((Vec3::new(to.x, crate::map::ground_y(to.x, to.z), to.z), speed));
            self.others[k].hold = 0.0;
        }
    }

    /// Every Shadow: `each` echoes beside him and `each` beside his other.
    pub fn every_shadow(&mut self, gi: usize, each: u32, life: f32, dmg: f32) {
        let team = self.gods[gi].team;
        let yaw = self.gods[gi].yaw;
        let mut spots = vec![(self.gods[gi].pos, yaw)];
        if let Some(k) = self.main_other(gi) {
            spots.push((self.others[k].pos, self.others[k].yaw));
        }
        let side = Vec3::new(-yaw.sin(), 0.0, yaw.cos());
        for (at, y) in spots {
            for e in 0..each {
                let s = (e as f32 - (each as f32 - 1.0) * 0.5) * 2.0;
                let s = if s.abs() < 0.5 { 1.0 } else { s };
                let p = at + side * (s * ECHO_SPREAD * 0.5);
                let p = self.blockers.resolve(Vec3::new(p.x, crate::map::ground_y(p.x, p.z), p.z), crate::consts::GOD_RADIUS);
                self.others.push(Other { team, owner: gi, pos: p, yaw: y, echo: true, life, goal: None, hold: 0.0, strike_t: 0.4, dmg, stride: 0.0, age: 0.0 });
                self.events.push(Event::EchoRise { pos: p, dmg });
            }
        }
    }

    /// Once a tick: keep each Hollowstep's other, walk them all, let the
    /// echoes strike and fade, and set every cat's fear.
    pub fn stalker_tick(&mut self, dt: f32) {
        // his other: rises when he does, goes when he falls
        for gi in 0..self.gods.len() {
            if !is_stalker(self.def(gi)) {
                continue;
            }
            let alive = self.gods[gi].respawn_t <= 0.0;
            if !alive {
                self.others.retain(|o| o.owner != gi);
                continue;
            }
            if self.main_other(gi).is_none() {
                let at = elsewhere(self.gods[gi].pos);
                let at = self.blockers.resolve(at, crate::consts::GOD_RADIUS);
                let team = self.gods[gi].team;
                self.others.push(Other { team, owner: gi, pos: at, yaw: 0.0, echo: false, life: f32::INFINITY, goal: None, hold: 0.0, strike_t: 0.0, dmg: 0.0, stride: 0.0, age: 0.0 });
            }
            // his own heavy tread
            let moved = self.gods[gi].move_dir.length_squared() > 0.01 && self.gods[gi].dash.is_none() && self.gods[gi].leap.is_none();
            if moved {
                self.gods[gi].stride += self.move_speed(gi) * dt;
                if self.gods[gi].stride >= STRIDE {
                    self.gods[gi].stride = 0.0;
                    self.events.push(Event::HeavyStep { pos: self.gods[gi].pos });
                }
            }
        }
        // the others walk
        let blockers = self.blockers.clone();
        let mut strikes: Vec<(usize, usize, f32)> = Vec::new();
        for k in 0..self.others.len() {
            let o = self.others[k];
            let (hunt, walk, stop) = if o.echo { (ECHO_HUNT, ECHO_WALK, ECHO_REACH * 0.7) } else { (HUNT_R, WALK, LURK) };
            let prey = (0..self.gods.len())
                .filter(|&j| self.gods[j].team != o.team && self.gods[j].respawn_t <= 0.0)
                .map(|j| (j, self.gods[j].pos.distance(o.pos)))
                .filter(|&(_, d)| d < hunt)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let mut o = o;
            o.age += dt;
            let mut step = Vec3::ZERO;
            if let Some((goal, speed)) = o.goal {
                let d = Vec3::new(goal.x - o.pos.x, 0.0, goal.z - o.pos.z);
                let l = d.length();
                if l <= speed * dt {
                    o.goal = None;
                    o.hold = HOLD_SECS;
                    step = d;
                } else {
                    step = d / l * speed * dt;
                }
            } else if o.hold > 0.0 {
                o.hold -= dt;
            } else if let Some((j, d)) = prey {
                if d > stop {
                    let to = self.gods[j].pos - o.pos;
                    let to = Vec3::new(to.x, 0.0, to.z).normalize_or_zero();
                    step = to * (walk * dt).min(d - stop);
                }
            }
            if step.length_squared() > 0.0 {
                o.yaw = step.z.atan2(step.x);
                let p = blockers.resolve(o.pos + step, crate::consts::GOD_RADIUS);
                let gone = Vec3::new(p.x - o.pos.x, 0.0, p.z - o.pos.z).length();
                o.pos = crate::abilities::clamp_arena(Vec3::new(p.x, crate::map::ground_y(p.x, p.z), p.z));
                // a glide (sent) is silent; a walk is not
                if o.goal.is_none() {
                    o.stride += gone;
                    if o.stride >= STRIDE {
                        o.stride = 0.0;
                        self.events.push(Event::HeavyStep { pos: o.pos });
                    }
                }
            }
            if o.echo {
                o.life -= dt;
                o.strike_t -= dt;
                if let Some((j, d)) = prey {
                    if d <= ECHO_REACH && o.strike_t <= 0.0 {
                        o.strike_t = ECHO_EVERY;
                        strikes.push((o.owner, j, o.dmg));
                        o.yaw = (self.gods[j].pos.z - o.pos.z).atan2(self.gods[j].pos.x - o.pos.x);
                    }
                }
            }
            self.others[k] = o;
        }
        for (owner, j, dmg) in strikes {
            self.deal_damage(TargetRef::God(j), dmg, true, Some(owner), 0.0, 0.0);
        }
        self.others.retain(|o| !o.echo || o.life > 0.0);
        // the fear
        for i in 0..self.gods.len() {
            let team = self.gods[i].team;
            let p = self.gods[i].pos;
            let mut near = f32::MAX;
            if self.gods[i].respawn_t <= 0.0 {
                for gi in 0..self.gods.len() {
                    if self.gods[gi].team != team && self.gods[gi].respawn_t <= 0.0 && is_stalker(self.def(gi)) {
                        near = near.min(self.gods[gi].pos.distance(p));
                    }
                }
                for o in &self.others {
                    if o.team != team {
                        near = near.min(o.pos.distance(p));
                    }
                }
            }
            let f = if near < f32::MAX { fear_at(near) } else { 0.0 };
            let g = &mut self.gods[i];
            g.fear = f;
            if f > 0.0 {
                g.beat_t -= dt;
                if g.beat_t <= 0.0 {
                    g.beat_t = beat_every(f);
                    self.events.push(Event::Heartbeat { god: i, fear: f });
                }
            } else {
                g.beat_t = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The screen darkens with the fear and pulses with the heart: darkest
    /// on the beat, easing before the next; nothing when calm.
    #[test]
    fn the_vignette_follows_the_fear_and_the_heart() {
        assert_eq!(vignette(0.0, 0.3), 0.0);
        let f = 0.8;
        let every = beat_every(f);
        let on_beat = vignette(f, every);
        let between = vignette(f, every * 0.2);
        assert!(on_beat > between + 0.2, "a pulse on the beat: {on_beat:.2} against {between:.2}");
        assert!(vignette(0.3, every) < on_beat, "less afraid, less dark");
        assert!((0.0..=1.0).contains(&vignette(1.0, 0.0)) && (0.0..=1.0).contains(&vignette(1.0, 9.0)));
    }

    /// The fear is whole at FEAR_NEAR, gone at TERROR_R, and in between it
    /// grows as he comes; the heart beats faster with it.
    #[test]
    fn the_fear_grows_as_he_comes() {
        assert_eq!(fear_at(FEAR_NEAR * 0.5), 1.0);
        assert_eq!(fear_at(TERROR_R + 0.1), 0.0);
        let mut last = 0.0;
        for k in 0..20 {
            let d = TERROR_R - k as f32;
            let f = fear_at(d);
            assert!(f >= last);
            last = f;
        }
        assert!(beat_every(1.0) < beat_every(0.1));
        assert!((beat_every(1.0) - BEAT_FAST).abs() < 1e-6 && (beat_every(0.0) - BEAT_SLOW).abs() < 1e-6);
    }

    /// Somewhere else: past the centre from him, never on top of him.
    #[test]
    fn his_other_rises_through_the_centre_from_him() {
        for p in [Vec3::new(-40.0, 0.0, -8.0), Vec3::new(10.0, 0.0, 100.0), Vec3::new(0.0, 0.0, 0.0)] {
            let o = elsewhere(p);
            let flat = Vec3::new(o.x, 0.0, o.z);
            assert!(flat.length() <= ELSEWHERE + 1e-3);
            if p.length() > 1.0 {
                assert!(flat.dot(Vec3::new(p.x, 0.0, p.z)) < 0.0, "on the far side of the centre from {p}: {o}");
            }
        }
    }
}
