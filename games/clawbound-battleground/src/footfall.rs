//! footfall — where each paw goes down, and keeping it there.
//!
//! `creature-rig` swings legs as pendulums from the gait phase. That reads as
//! walking from a distance and as skating up close: nothing ties a paw to
//! the ground, so every paw slides back and forth under a moving body, and
//! whatever the ground holds (a root's rim, a rock's edge, a hole) the paw
//! lands wherever the sine puts it. `puppet` already raised each foot to the
//! surface under it; it could not choose WHERE to put it.
//!
//! This is a small stepping controller, one per cat, four paws each:
//!
//! 1. **Stance locks.** A paw on the ground has a world position and keeps
//!    it until it lifts. The leg is solved to reach it (`rigged`'s plant
//!    IK), so the body moves over a still foot — no skate.
//! 2. **Lift-off is the gait's.** A paw lifts when the gait table says its
//!    swing begins, so the footfall pattern (walk, trot, pace, bound) is the
//!    same one the body bob and the rest of the rig are timed to. It also
//!    lifts early if the body has left it behind (`REACH`), and — standing
//!    still, where the gait does not cycle — shuffles when a turn in place
//!    has twisted it too far from under its hip.
//! 3. **Landing is chosen.** At lift-off the paw predicts where its hip will
//!    be at mid-stance and scores a small fan of spots round that: distance
//!    from the ideal, then penalties for solid ground (a trunk, a boulder's
//!    face), a rim (the edge of a root or a rock, where a paw would stand on
//!    a corner), a step taller than the leg can take, and deep water. The
//!    cheapest wins, re-scored every frame of the swing with a bias toward
//!    the previous pick so it does not flicker. Every candidate and its
//!    score is kept in [`Paw::choice`] for anyone who wants to see why.
//! 4. **The swing arcs over what is in the way**, not a fixed lift: the arc
//!    height is the gait's clearance plus whatever the surface rises to
//!    between lift-off and landing.
//!
//! ## Layer
//!
//! Headless: a [`Probe`] (the same seam `puppet` uses) in, world-space sole
//! positions out. It does not know about rigs, matrices or the sim; the
//! caller supplies hip positions. It is presentation only — nothing here
//! feeds back into `GodUnit::pos`.

use glam::Vec3;

use crate::puppet::Probe;

/// Horizontal distance from under the hip a planted paw may be left before
/// it lifts regardless of the gait, as a fraction of leg length.
pub const REACH: f32 = 0.95;
/// Standing still: a paw this far (fraction of leg length) from under its
/// hip shuffles back.
pub const SHUFFLE: f32 = 0.45;
/// How long a shuffle step takes, seconds.
pub const SHUFFLE_SECS: f32 = 0.22;
/// A rim steeper than this (height change per unit across the paw) costs.
const RIM_SLOPE: f32 = 0.9;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// On the ground at `at`.
    Stance { at: Vec3 },
    /// In the air from `from` toward `to`, `t` of `dur` seconds in.
    Swing { from: Vec3, to: Vec3, t: f32, dur: f32 },
}

/// One landing candidate, for inspection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub at: Vec3,
    pub score: f32,
    /// Refused outright (solid ground, or a step the leg cannot take).
    pub refused: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Paw {
    pub phase: Phase,
    /// Height of the current swing's arc over its straight line.
    pub arc: f32,
    /// Seen on the ground during the gait's STANCE window since it last
    /// landed. A paw lifts only when armed, so one that lands late (it was
    /// shoved, or held by a long swing) takes the next cycle's cue instead of
    /// re-lifting at once -- and a paw that missed its cue does not wait a
    /// whole cycle, because arming happens as soon as the window comes round.
    pub armed: bool,
    /// The last scoring round, kept so the choice is visible.
    pub choice: Vec<Candidate>,
}

/// What the stepper needs to know about the body this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    /// World hip positions, projected to the ground plane (y ignored).
    pub hips: [Vec3; 4],
    /// Leg length, world units.
    pub leg: f32,
    /// Ground velocity of the body.
    pub vel: Vec3,
    /// Gait phase, radians, and the per-leg offsets the gait uses.
    pub gait_phase: f32,
    pub offsets: [f32; 4],
    /// Stride frequency, cycles per second. ~0 standing still.
    pub hz: f32,
    /// Swing clearance at a run, fraction of leg length (the rig's
    /// `SWING_LIFT`), scaled by how fast the cat is going.
    pub lift: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Footfall {
    pub paws: [Paw; 4],
    last_phase: Option<f32>,
}

impl Default for Footfall {
    fn default() -> Self {
        Footfall::new()
    }
}

/// Surface height a paw would stand on: terrain, or a hump on it.
pub fn surface(world: &dyn Probe, x: f32, z: f32) -> f32 {
    let g = world.ground(x, z);
    let g = if g.is_finite() { g } else { 0.0 };
    world.step_top(x, z).filter(|t| t.is_finite()).map_or(g, |t| t.max(g))
}

/// Is the swing half of the cycle: the rig's own rule (`anim_rig`: stance
/// while the phase's cosine is negative).
fn swinging(theta: f32) -> bool {
    theta.cos() >= 0.0
}

impl Footfall {
    pub fn new() -> Footfall {
        let paw = Paw { phase: Phase::Stance { at: Vec3::NAN }, arc: 0.0, armed: true, choice: Vec::new() };
        Footfall { paws: [paw.clone(), paw.clone(), paw.clone(), paw], last_phase: None }
    }

    /// Forget every plant: the next grounded frame starts from under the
    /// hips. For leaving the ground (leap, fall, perch, respawn).
    pub fn release(&mut self) {
        *self = Footfall::new();
    }

    /// Where each sole is this frame, in world space. `None` for a paw that
    /// has not been placed (the first frame after a release).
    pub fn soles(&self) -> [Option<Vec3>; 4] {
        std::array::from_fn(|i| match self.paws[i].phase {
            Phase::Stance { at } => at.is_finite().then_some(at),
            Phase::Swing { from, to, t, dur } => Some(swing_point(from, to, t, dur, self.paws[i].arc)),
        })
    }

    pub fn advance(&mut self, dt: f32, body: &Body, world: &dyn Probe) {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.1) } else { 0.0 };
        let leg = if body.leg.is_finite() && body.leg > 0.01 { body.leg } else { 0.5 };
        let vel = if body.vel.is_finite() { Vec3::new(body.vel.x, 0.0, body.vel.z) } else { Vec3::ZERO };
        let moving = vel.length() > 0.4 && body.hz > 0.05;
        let omega = std::f32::consts::TAU * body.hz.max(1e-3);
        let half = std::f32::consts::PI / omega; // one half-cycle, seconds
        let swing_dur = if moving { half.clamp(0.08, 0.40) } else { SHUFFLE_SECS };
        self.last_phase = Some(body.gait_phase);

        // Standing still, one paw at a time: the one furthest out of place.
        let shuffle = if moving {
            None
        } else {
            let busy = self.paws.iter().any(|p| matches!(p.phase, Phase::Swing { .. }));
            (0..4)
                .filter(|_| !busy)
                .filter_map(|i| match self.paws[i].phase {
                    Phase::Stance { at } if at.is_finite() => Some((i, flat(at - body.hips[i]).length() / leg)),
                    _ => None,
                })
                .filter(|&(_, d)| d > SHUFFLE)
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i)
        };

        for i in 0..4 {
            let hip = flat(body.hips[i]);
            if !hip.is_finite() {
                continue;
            }
            // Where this paw would ideally come down: under the hip at the
            // MIDDLE of its next stance, so the stance stroke is centred on
            // the hip. That is the hip at TOUCHDOWN (what is left of the
            // swing, at the body's speed) plus half a stance of travel, the
            // second part held within reach of that touchdown hip.
            let remaining = match self.paws[i].phase {
                Phase::Swing { t, dur, .. } => (dur - t).max(0.0),
                Phase::Stance { .. } => swing_dur,
            };
            let ideal = if moving {
                hip + vel * remaining + (vel * half * 0.5).clamp_length_max(REACH * leg * 0.9)
            } else {
                hip
            };

            let phase = self.paws[i].phase;
            match phase {
                Phase::Stance { at } if !at.is_finite() => {
                    // First grounded frame: stand where we are.
                    let y = surface(world, hip.x, hip.z);
                    self.paws[i].phase = Phase::Stance { at: Vec3::new(hip.x, y, hip.z) };
                }
                Phase::Stance { at } => {
                    let theta = body.gait_phase + body.offsets[i];
                    if !swinging(theta) {
                        self.paws[i].armed = true;
                    }
                    let gait_lift = moving && self.paws[i].armed && swinging(theta);
                    // Moving only: standing, a paw out of place waits its
                    // turn in the shuffle queue, or a turn in place would
                    // lift every paw at once.
                    let left_behind = moving && flat(at - hip).length() > REACH * leg;
                    if gait_lift || left_behind || shuffle == Some(i) {
                        // Land when the gait's swing window closes, so the
                        // paw is back in step every cycle whenever it lifted.
                        let dur = if moving && swinging(theta) {
                            let th = (theta + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                            ((std::f32::consts::FRAC_PI_2 - th) / omega).clamp(0.08, 0.40)
                        } else {
                            swing_dur
                        };
                        self.paws[i].armed = false;
                        let ideal = if moving { ideal + vel * (dur - swing_dur) } else { ideal };
                        let to = self.pick(i, ideal, None, leg, world);
                        let arc = self.clearance(at, to, leg, body.lift * vel.length().min(8.0) / 8.0, world);
                        self.paws[i].arc = arc;
                        self.paws[i].phase = Phase::Swing { from: at, to, t: 0.0, dur };
                    }
                }
                Phase::Swing { from, to, t, dur } => {
                    let t = t + dt;
                    if t >= dur {
                        self.paws[i].phase = Phase::Stance { at: to };
                    } else {
                        // Keep choosing until late in the swing: a turn or a
                        // push changes where the hip will be.
                        let to = if t < dur * 0.7 { self.pick(i, ideal, Some(to), leg, world) } else { to };
                        let arc = self.paws[i].arc.max(self.clearance(from, to, leg, 0.0, world));
                        self.paws[i].arc = arc;
                        self.paws[i].phase = Phase::Swing { from, to, t, dur };
                    }
                }
            }
        }
    }

    /// Score the fan round `ideal` and return the winner's sole position.
    fn pick(&mut self, i: usize, ideal: Vec3, prev: Option<Vec3>, leg: f32, world: &dyn Probe) -> Vec3 {
        let base = surface(world, ideal.x, ideal.z);
        let fore = [0.0, 0.3, -0.3];
        let side = [0.0, 0.18, -0.18];
        let mut choice = Vec::with_capacity(9);
        // The fan is axis-aligned rather than body-aligned: it is a search
        // pattern, not a gait, and a paw has no preference between fore and
        // left once it is off the ideal.
        for f in fore {
            for s in side {
                let p = ideal + Vec3::new(f * leg, 0.0, s * leg);
                choice.push(score(p, ideal, base, prev, leg, world));
            }
        }
        if let Some(p) = prev {
            choice.push(score(Vec3::new(p.x, 0.0, p.z), ideal, base, prev, leg, world));
        }
        let best = choice
            .iter()
            .filter(|c| !c.refused)
            .min_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal))
            .map(|c| c.at)
            // Nowhere acceptable: go to the ideal anyway. The sim has already
            // decided the body is here; a paw hanging in the air is worse
            // than one on an edge.
            .unwrap_or_else(|| Vec3::new(ideal.x, base, ideal.z));
        self.paws[i].choice = choice;
        best
    }

    /// Arc height over the straight line from `a` to `b`: enough to clear
    /// what the surface rises to between them, plus the gait's own lift.
    fn clearance(&self, a: Vec3, b: Vec3, leg: f32, gait: f32, world: &dyn Probe) -> f32 {
        let mut over = 0.0f32;
        for k in 1..4 {
            let p = a.lerp(b, k as f32 / 4.0);
            let line = a.y + (b.y - a.y) * (k as f32 / 4.0);
            over = over.max(surface(world, p.x, p.z) - line);
        }
        (over + 0.08 * leg).max(0.0) + gait * leg
    }
}

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

fn score(p: Vec3, ideal: Vec3, base: f32, prev: Option<Vec3>, leg: f32, world: &dyn Probe) -> Candidate {
    let y = surface(world, p.x, p.z);
    let at = Vec3::new(p.x, y, p.z);
    let refused = world.solid(p.x, p.z) || (y - base).abs() > 0.9 * leg;
    let e = 0.12 * leg;
    let rim = [(e, 0.0), (0.0, e)]
        .iter()
        .map(|&(dx, dz)| (surface(world, p.x + dx, p.z + dz) - surface(world, p.x - dx, p.z - dz)).abs() / (2.0 * e))
        .fold(0.0f32, f32::max);
    let d = flat(p - ideal).length() / leg;
    let mut s = d * d * 4.0;
    s += (rim - RIM_SLOPE).max(0.0) * 1.5;
    s += ((y - base).abs() / leg - 0.4).max(0.0) * 2.0;
    s += (world.water(p.x, p.z) - 0.3).max(0.0) * 0.8;
    if let Some(q) = prev {
        // Stickiness: a paw in the air does not change its mind for a hair.
        if flat(p - q).length() > 1e-3 {
            s += 0.06;
        }
    }
    Candidate { at, score: if s.is_finite() { s } else { f32::MAX }, refused }
}

fn swing_point(from: Vec3, to: Vec3, t: f32, dur: f32, arc: f32) -> Vec3 {
    let s = (t / dur.max(1e-3)).clamp(0.0, 1.0);
    // Ease the travel so the paw leaves and lands gently; lift on a sine so
    // it is highest in the middle and zero at both ends.
    let e = s * s * (3.0 - 2.0 * s);
    from.lerp(to, e) + Vec3::Y * arc * (std::f32::consts::PI * s).sin()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Flat world with optional solid discs and a hump.
    struct W {
        solid: Vec<(f32, f32, f32)>,
        hump: Option<(f32, f32, f32, f32)>, // x, z, r, height
    }
    impl Probe for W {
        fn ground(&self, _: f32, _: f32) -> f32 { 0.0 }
        fn step_top(&self, x: f32, z: f32) -> Option<f32> {
            let (hx, hz, r, h) = self.hump?;
            let d2 = (x - hx).powi(2) + (z - hz).powi(2);
            (d2 < r * r).then(|| h * (1.0 - d2 / (r * r)).sqrt())
        }
        fn cover(&self, _: f32, _: f32) -> f32 { 0.0 }
        fn water(&self, _: f32, _: f32) -> f32 { 0.0 }
        fn solid(&self, x: f32, z: f32) -> bool {
            self.solid.iter().any(|&(sx, sz, r)| (x - sx).hypot(z - sz) < r)
        }
    }
    fn flat_world() -> W {
        W { solid: vec![], hump: None }
    }

    const LEG: f32 = 0.5;
    const TROT: [f32; 4] = [0.0, PI, PI, 0.0];
    const HIPS: [(f32, f32); 4] = [(0.45, -0.18), (0.45, 0.18), (-0.45, -0.18), (-0.45, 0.18)];

    fn body_at(pos: Vec3, vel: Vec3, phase: f32, hz: f32) -> Body {
        Body {
            hips: std::array::from_fn(|i| pos + Vec3::new(HIPS[i].0, 0.0, HIPS[i].1)),
            leg: LEG,
            vel,
            gait_phase: phase,
            offsets: TROT,
            hz,
            lift: 0.35,
        }
    }

    /// Walk in +x at `speed` for `secs`, recording every planted paw's
    /// position each frame it is planted.
    fn walk(world: &W, speed: f32, secs: f32) -> (Footfall, Vec<[Option<Vec3>; 4]>, Vec<Vec3>) {
        let mut f = Footfall::new();
        let dt = 1.0 / 60.0;
        let hz = speed / 1.6;
        let (mut pos, mut phase) = (Vec3::ZERO, 0.0f32);
        let mut planted = Vec::new();
        let mut positions = Vec::new();
        let vel = Vec3::X * speed;
        for _ in 0..(secs / dt) as usize {
            pos += vel * dt;
            phase = (phase + hz * dt * std::f32::consts::TAU) % std::f32::consts::TAU;
            f.advance(dt, &body_at(pos, vel, phase, hz), world);
            planted.push(std::array::from_fn(|i| match f.paws[i].phase {
                Phase::Stance { at } if at.is_finite() => Some(at),
                _ => None,
            }));
            positions.push(pos);
        }
        (f, planted, positions)
    }

    /// The point of the module: a planted paw does not move. Checked frame to
    /// frame for every paw over a run of trotting.
    #[test]
    fn a_planted_paw_does_not_skate() {
        let (_, planted, _) = walk(&flat_world(), 3.0, 3.0);
        let mut stances = 0;
        for w in planted.windows(2) {
            for i in 0..4 {
                if let (Some(a), Some(b)) = (w[0][i], w[1][i]) {
                    assert!((a - b).length() < 1e-6, "paw {i} slid {a} -> {b}");
                    stances += 1;
                }
            }
        }
        assert!(stances > 250, "paws were planted: {stances}");
    }

    /// ...and they do not trail: the body carries on, each paw keeps up,
    /// never further from its hip than the leg reaches.
    #[test]
    fn paws_keep_up_with_the_body_and_stay_within_reach() {
        let (f, planted, positions) = walk(&flat_world(), 4.0, 4.0);
        for (k, frame) in planted.iter().enumerate().skip(30) {
            for i in 0..4 {
                if let Some(at) = frame[i] {
                    let hip = positions[k] + Vec3::new(HIPS[i].0, 0.0, HIPS[i].1);
                    assert!(flat(at - hip).length() <= REACH * LEG + 1e-3, "frame {k} paw {i} {:.3} from its hip", flat(at - hip).length());
                }
            }
        }
        let end = *positions.last().unwrap();
        for s in f.soles().iter().flatten() {
            assert!((s.x - end.x).abs() < 1.0, "a paw was left behind at {s}, body at {end}");
        }
    }

    /// The trot's pairs alternate: FL+HR together, FR+HL together, and the
    /// two pairs never both in the air on a steady trot.
    #[test]
    fn the_footfall_pattern_is_the_gaits() {
        let (_, planted, _) = walk(&flat_world(), 3.0, 3.0);
        let mut both_pairs_up = 0;
        let mut pair_split = 0;
        for fr in planted.iter().skip(60) {
            let up = |i: usize| fr[i].is_none();
            if up(0) && up(1) && up(2) && up(3) {
                both_pairs_up += 1;
            }
            if up(0) != up(3) {
                pair_split += 1;
            }
        }
        assert_eq!(both_pairs_up, 0, "a trot never has all four up");
        assert!(pair_split < 12, "diagonal pairs lift together ({pair_split} split frames)");
    }

    /// A trunk where the paw would naturally land: it lands beside it.
    #[test]
    fn a_paw_does_not_land_inside_something_solid() {
        // Put solid ground on the path of the front-left paw.
        let world = W { solid: vec![(2.2, -0.18, 0.2)], hump: None };
        let (_, planted, _) = walk(&world, 3.0, 2.0);
        let mut n = 0;
        for fr in &planted {
            for at in fr.iter().flatten() {
                assert!(!world.solid(at.x, at.z), "planted inside the solid at {at}");
                n += 1;
            }
        }
        assert!(n > 100);
    }

    /// A root's rim is a bad place for a paw: it lands on the crest or off
    /// it, not on the slope between.
    #[test]
    fn landing_avoids_a_rim() {
        let world = W { solid: vec![], hump: Some((0.0, 0.0, 0.3, 0.25)) };
        let mut f = Footfall::new();
        // Ideal spot right on the hump's rim.
        let spot = f.pick(0, Vec3::new(0.26, 0.0, 0.0), None, LEG, &world);
        let rim_at = |p: Vec3| {
            let e = 0.12 * LEG;
            ((surface(&world, p.x + e, p.z) - surface(&world, p.x - e, p.z)) / (2.0 * e)).abs()
        };
        assert!(rim_at(spot) < rim_at(Vec3::new(0.26, 0.0, 0.0)), "moved off the rim: {spot}");
        assert!(f.paws[0].choice.len() >= 9, "the choice is kept for inspection");
        assert!((spot.y - surface(&world, spot.x, spot.z)).abs() < 1e-6, "on the surface");
    }

    /// The swing clears a hump between lift-off and landing.
    #[test]
    fn the_swing_clears_what_is_in_the_way() {
        let world = W { solid: vec![], hump: Some((0.5, 0.0, 0.2, 0.3)) };
        let f = Footfall::new();
        let arc = f.clearance(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), LEG, 0.0, &world);
        for k in 0..=20 {
            let t = k as f32 / 20.0;
            let p = swing_point(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), t, 1.0, arc);
            // The arc only needs to clear the hump where it is (s near 0.5).
            if (p.x - 0.5).abs() < 0.12 {
                assert!(p.y >= surface(&world, p.x, p.z) - 1e-3, "through the hump at {p}");
            }
        }
    }

    /// Turning on the spot: the gait does not cycle, so the paws shuffle,
    /// one at a time, until each is back under its hip.
    #[test]
    fn standing_turns_shuffle_one_paw_at_a_time() {
        let world = flat_world();
        let mut f = Footfall::new();
        let dt = 1.0 / 60.0;
        let hips = |yaw: f32| -> [Vec3; 4] {
            std::array::from_fn(|i| {
                let (x, z) = HIPS[i];
                Vec3::new(x * yaw.cos() - z * yaw.sin(), 0.0, x * yaw.sin() + z * yaw.cos())
            })
        };
        let mut body = Body { hips: hips(0.0), leg: LEG, vel: Vec3::ZERO, gait_phase: 0.0, offsets: TROT, hz: 0.0, lift: 0.35 };
        f.advance(dt, &body, &world);
        body.hips = hips(1.2); // a sharp turn in place
        let mut max_up = 0;
        for _ in 0..120 {
            f.advance(dt, &body, &world);
            let up = f.paws.iter().filter(|p| matches!(p.phase, Phase::Swing { .. })).count();
            max_up = max_up.max(up);
        }
        assert_eq!(max_up, 1, "one paw at a time");
        for (i, s) in f.soles().iter().enumerate() {
            let s = s.expect("placed");
            assert!(flat(s - body.hips[i]).length() <= SHUFFLE * LEG + 1e-3, "paw {i} still out of place");
        }
    }

    /// A shove (knockback, push-out) moves the body a long way at once: the
    /// planted paws let go rather than stretching the leg across the gap.
    #[test]
    fn a_shove_releases_paws_it_leaves_behind() {
        let world = flat_world();
        let mut f = Footfall::new();
        let b0 = body_at(Vec3::ZERO, Vec3::ZERO, 0.0, 0.0);
        f.advance(1.0 / 60.0, &b0, &world);
        let b1 = body_at(Vec3::new(1.5, 0.0, 0.0), Vec3::ZERO, 0.0, 0.0);
        f.advance(1.0 / 60.0, &b1, &world);
        let lifted = f.paws.iter().filter(|p| matches!(p.phase, Phase::Swing { .. })).count();
        assert!(lifted >= 1, "something let go");
        for _ in 0..200 {
            f.advance(1.0 / 60.0, &b1, &world);
        }
        for (i, s) in f.soles().iter().enumerate() {
            assert!(flat(s.unwrap() - b1.hips[i]).length() <= REACH * LEG, "paw {i} never caught up");
        }
    }

    #[test]
    fn garbage_in_stays_out() {
        let world = flat_world();
        let mut f = Footfall::new();
        let mut b = body_at(Vec3::ZERO, Vec3::X, 0.3, 1.0);
        b.vel = Vec3::NAN;
        b.leg = f32::NAN;
        f.advance(f32::NAN, &b, &world);
        f.advance(1.0 / 60.0, &b, &world);
        for s in f.soles().iter().flatten() {
            assert!(s.is_finite(), "{s}");
        }
    }
}
