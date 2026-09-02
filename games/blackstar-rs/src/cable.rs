//! THE CABLE — a tether between two bodies, as physics rather than a verb.
//!
//! The Watch's GRAPPLE (`weapons.rs`) is an instant hook with a 2.5 s
//! reel: light things come to you, heavy things pull you to them. That is
//! the right one-button traversal tool and the wrong model for a fight
//! that has vehicles in it. A cable between a car doing 30 m/s and a
//! robot standing still is not a reel — it is a TOW, then a WHIP, and if
//! the thing on the end is heavier than the car it is a yank the other
//! way. Dark of the Moon's highway sequence is mostly this: mass and
//! speed on a line, and what happens when the line goes taut.
//!
//! So this module is a tether machine and nothing else. Two ends,
//! snapshotted by the caller each tick (position, velocity, mass); a rest
//! length the caller may hold, reel or slacken; and out come the delta-v
//! each end takes and what the cable itself did — held, snapped, reeled
//! home. Who the ends are (the player and a foe, a foe and the player, a
//! raid unit and an AIR node) is the caller's business, which is what
//! lets `field.rs` use one machine for the player's tow and the
//! hostiles' pull and `raidrun` reuse it later without a third copy.
//!
//! Physics, kept honest and cheap: the cable is a one-sided spring with
//! damping along its own line. Slack costs nothing. Past rest length the
//! stretch pulls both ends together by inverse mass, so a light body
//! comes and a heavy one barely notices — the exact trade the grapple's
//! doc line promises, now as arithmetic. Tension is reported in delta-v
//! per second; the caller decides what tension snaps a cable and what
//! it costs the body on the end.
//!
//! GUESSES, labelled: `STIFFNESS`, `DAMPING`, `SNAP_STRETCH` and the
//! default reel rate are chosen to feel like a steel cable on a car
//! rather than a bungee, in the sandbox's own units. Nobody has pulled
//! on one yet.

use glam::Vec3;

/// Spring constant: delta-v per second per metre of stretch, before the
/// mass split. GUESS.
pub const STIFFNESS: f32 = 60.0;
/// Damping along the line: fraction of closing/opening speed bled per
/// second. GUESS — high, because a cable does not bounce.
pub const DAMPING: f32 = 6.0;
/// Stretch beyond rest length, as a fraction, past which the cable parts.
/// GUESS: steel, not rope.
pub const SNAP_STRETCH: f32 = 0.35;
/// Tension (delta-v/s) past which the cable parts whatever the stretch —
/// a heavy body braking hard on the end. GUESS. Sized ABOVE what a car
/// taking up a slack line at road speed produces (the damping term alone
/// is `DAMPING` x closing speed, ~120 at 20 m/s), so the tow's first
/// tick is a yank and not a snap — the first value, 90, parted every
/// tow line on frame one, which a test caught.
pub const SNAP_TENSION: f32 = 480.0;
/// Default reel rate, m/s. Slower than the Watch's hook, which is a
/// traversal tool; this is for hauling a body.
pub const REEL_MS: f32 = 9.0;
/// A cable never reels shorter than this — the bodies would overlap.
pub const MIN_LEN: f32 = 3.0;

/// One end of the cable, this tick. The caller snapshots both.
#[derive(Clone, Copy, Debug)]
pub struct End {
    pub pos: Vec3,
    pub vel: Vec3,
    /// relative mass; only the RATIO matters. A car is 1.0 by
    /// convention (`CAR_MASS`), a chassis its bulk times that.
    pub mass: f32,
}

/// The car's mass, the unit everything else is expressed in.
pub const CAR_MASS: f32 = 1.0;

/// What the caller wants the rest length to do this tick.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// hold the rest length — a tow line
    Hold,
    /// shorten it at the reel rate — hauling in
    Reel,
    /// let it out to whatever the bodies are doing — a whip being loaded
    Slack,
}

/// The cable's own verdict on the tick.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fate {
    Holding,
    /// parted; `tension` is what did it
    Snapped { tension: f32 },
    /// reeled all the way home: the ends are together
    Reeled,
}

/// Delta-v each end takes this tick. Apply them; the cable does not.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Pull {
    pub a: Vec3,
    pub b: Vec3,
}

#[derive(Clone, Debug)]
pub struct Cable {
    /// rest length, metres
    pub len: f32,
    /// reel rate, m/s, when `Mode::Reel`
    pub reel_ms: f32,
    pub age: f32,
    /// tension last tick, delta-v per second along the line
    pub tension: f32,
    /// the sideways speed of end B relative to end A last tick — what a
    /// whip is loading. Reported so the caller can decide when to let go.
    pub swing_ms: f32,
    /// true once the cable has parted or reeled home; `step` is inert after
    pub done: bool,
}

impl Cable {
    /// A cable at rest at `len` — attach at the bodies' current distance
    /// so the first tick does not yank.
    pub fn new(len: f32) -> Cable {
        Cable { len: len.max(MIN_LEN), reel_ms: REEL_MS, age: 0.0, tension: 0.0, swing_ms: 0.0, done: false }
    }

    /// Attach between two ends at their current separation.
    pub fn between(a: &End, b: &End) -> Cable {
        Cable::new((b.pos - a.pos).length())
    }

    /// Advance one tick. Returns the delta-v for each end and what the
    /// cable did. `g` is only used to keep a reeled-home verdict honest
    /// on a slope — it is not applied to anything.
    pub fn step(&mut self, dt: f32, a: &End, b: &End, mode: Mode) -> (Pull, Fate) {
        if self.done || dt <= 0.0 {
            return (Pull::default(), if self.done { Fate::Snapped { tension: self.tension } } else { Fate::Holding });
        }
        self.age += dt;
        let d = b.pos - a.pos;
        let dist = d.length();
        let dir = if dist > 1e-4 { d / dist } else { Vec3::Z };

        // the rest length follows the mode
        match mode {
            Mode::Hold => {}
            Mode::Reel => self.len = (self.len - self.reel_ms * dt).max(MIN_LEN),
            Mode::Slack => self.len = self.len.max(dist),
        }

        // relative velocity, split into along the line and across it
        let rel = b.vel - a.vel;
        let along = rel.dot(dir);
        let across = rel - dir * along;
        self.swing_ms = across.length();

        let stretch = dist - self.len;
        if stretch <= 0.0 {
            self.tension = 0.0;
            let home = mode == Mode::Reel && self.len <= MIN_LEN + 1e-3 && dist <= MIN_LEN + 1.0;
            if home {
                self.done = true;
                return (Pull::default(), Fate::Reeled);
            }
            return (Pull::default(), Fate::Holding);
        }

        // taut: spring plus damping on the opening speed, as an
        // acceleration along the line, then split by inverse mass
        let accel = STIFFNESS * stretch + DAMPING * along.max(0.0);
        self.tension = accel;
        if stretch > self.len * SNAP_STRETCH || accel > SNAP_TENSION {
            self.done = true;
            return (Pull::default(), Fate::Snapped { tension: accel });
        }
        let total = (a.mass + b.mass).max(1e-3);
        let share_a = b.mass / total;
        let share_b = a.mass / total;
        let dv = accel * dt;
        (Pull { a: dir * (dv * share_a), b: -dir * (dv * share_b) }, Fate::Holding)
    }

    /// Let go. Returns the velocity end B carries away with — its own
    /// plus nothing, because the cable only ever constrained it; the
    /// swing was already in `b.vel`. Provided so the whip's release is
    /// one call the caller cannot get subtly wrong.
    pub fn release(&mut self, b: &End) -> Vec3 {
        self.done = true;
        b.vel
    }
}

// ------------------------------------------------------------------ tests ---

#[cfg(test)]
mod tests {
    use super::*;

    fn end(pos: Vec3, vel: Vec3, mass: f32) -> End {
        End { pos, vel, mass }
    }

    #[test]
    fn slack_costs_nothing() {
        let a = end(Vec3::ZERO, Vec3::ZERO, 1.0);
        let b = end(Vec3::new(5.0, 0.0, 0.0), Vec3::ZERO, 1.0);
        let mut c = Cable::new(10.0);
        let (p, f) = c.step(1.0 / 120.0, &a, &b, Mode::Hold);
        assert_eq!(p, Pull::default());
        assert_eq!(f, Fate::Holding);
        assert_eq!(c.tension, 0.0);
    }

    #[test]
    fn a_light_body_comes_and_a_heavy_one_barely_moves() {
        let a = end(Vec3::ZERO, Vec3::ZERO, 1.0);
        let b = end(Vec3::new(12.0, 0.0, 0.0), Vec3::ZERO, 0.25);
        let mut c = Cable::new(10.0);
        let (p, _) = c.step(1.0 / 120.0, &a, &b, Mode::Hold);
        assert!(p.b.x < 0.0, "the light end was not pulled toward the anchor");
        assert!(p.a.x > 0.0);
        assert!(p.b.length() > p.a.length() * 3.0, "mass split wrong: a={:?} b={:?}", p.a, p.b);
        // and the other way round: the anchor is the one that moves
        let heavy = end(Vec3::new(12.0, 0.0, 0.0), Vec3::ZERO, 4.0);
        let mut c = Cable::new(10.0);
        let (p, _) = c.step(1.0 / 120.0, &a, &heavy, Mode::Hold);
        assert!(p.a.length() > p.b.length() * 3.0, "the heavy end should pull the anchor: a={:?} b={:?}", p.a, p.b);
    }

    #[test]
    fn a_tow_holds_length_and_the_towed_body_follows() {
        // a car at 20 m/s with a body 8 m behind on an 8 m line
        let mut car = end(Vec3::ZERO, Vec3::new(20.0, 0.0, 0.0), 1.0);
        let mut body = end(Vec3::new(-8.0, 0.0, 0.0), Vec3::ZERO, 0.8);
        let mut c = Cable::between(&car, &body);
        let dt = 1.0 / 120.0;
        for _ in 0..(120 * 3) {
            let (p, f) = c.step(dt, &car, &body, Mode::Hold);
            assert_eq!(f, Fate::Holding, "the tow line parted at t={:.2} with tension {:.1}", c.age, c.tension);
            car.vel += p.a;
            body.vel += p.b;
            // the car is driven: it keeps its speed whatever the line does
            car.vel = Vec3::new(20.0, 0.0, 0.0);
            // the body is on the ground: without drag it overshoots, the
            // line goes slack, and it oscillates around the car's speed
            // — physically right for a body on ice, and not what a tow is
            body.vel *= 1.0 - 2.5 * dt;
            car.pos += car.vel * dt;
            body.pos += body.vel * dt;
        }
        let gap = (car.pos - body.pos).length();
        assert!(gap < c.len * (1.0 + SNAP_STRETCH), "the body fell off the back: gap {gap:.1}");
        assert!(body.vel.x > 15.0, "the body never came up to speed: {:?}", body.vel);
    }

    #[test]
    fn a_hard_stop_on_the_far_end_snaps_it() {
        // a fast car, a body bolted to the ground (infinite mass), line taut
        let car = end(Vec3::ZERO, Vec3::new(40.0, 0.0, 0.0), 1.0);
        let wall = end(Vec3::new(-6.0, 0.0, 0.0), Vec3::ZERO, 1.0e6);
        let mut c = Cable::new(6.0);
        let mut car = car;
        let dt = 1.0 / 120.0;
        let mut snapped = false;
        for _ in 0..120 {
            let (p, f) = c.step(dt, &car, &wall, Mode::Hold);
            if let Fate::Snapped { tension } = f {
                assert!(tension > 0.0);
                snapped = true;
                break;
            }
            car.vel += p.a;
            car.pos += car.vel * dt;
        }
        assert!(snapped, "a car driving off against a bolted body never parted the cable");
        // and it stays parted
        let (p, f) = c.step(dt, &car, &wall, Mode::Hold);
        assert_eq!(p, Pull::default());
        assert!(matches!(f, Fate::Snapped { .. }));
    }

    #[test]
    fn reeling_brings_the_ends_home() {
        let anchor = end(Vec3::ZERO, Vec3::ZERO, 1.0e6);
        let mut body = end(Vec3::new(0.0, 0.0, 20.0), Vec3::ZERO, 0.5);
        let mut c = Cable::between(&anchor, &body);
        let dt = 1.0 / 120.0;
        let mut fate = Fate::Holding;
        for _ in 0..(120 * 10) {
            let (p, f) = c.step(dt, &anchor, &body, Mode::Reel);
            body.vel += p.b;
            body.vel *= 0.98; // a little ground drag, or it overshoots forever
            body.pos += body.vel * dt;
            fate = f;
            if f != Fate::Holding {
                break;
            }
        }
        assert_eq!(fate, Fate::Reeled, "never reeled home; body at {:?}, len {:.1}", body.pos, c.len);
        assert!(body.pos.length() <= MIN_LEN + 1.0);
    }

    #[test]
    fn a_swing_loads_the_whip_and_release_carries_it() {
        // anchor turning: the body on the end circles it at ω·L
        let anchor = end(Vec3::ZERO, Vec3::ZERO, 1.0e6);
        let l = 8.0;
        let omega = 2.0; // rad/s
        let body = end(Vec3::new(l, 0.0, 0.0), Vec3::new(0.0, 0.0, omega * l), 0.5);
        let mut c = Cable::new(l);
        let (_, f) = c.step(1.0 / 120.0, &anchor, &body, Mode::Hold);
        assert_eq!(f, Fate::Holding);
        assert!((c.swing_ms - omega * l).abs() < 0.5, "swing {:.1} vs ω·L {:.1}", c.swing_ms, omega * l);
        let fling = c.release(&body);
        assert!((fling.length() - omega * l).abs() < 0.5);
        assert!(c.done);
    }
}
