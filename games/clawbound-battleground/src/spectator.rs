//! spectator -- watching a match: a director that finds the fight, a
//! chase-cam on any cat, and a free-flying camera, all moving smoothly.
//!
//! Headless: [`Spectator::update`] takes the game and this frame's input
//! and returns a camera shot; the app draws from it. Nothing here touches
//! the sim -- a spectator changes what is seen, never what happens.
//!
//! [`SmoothCam`] is the smoothing: a critically damped spring on the eye
//! and on the look target, so every move -- a cut between cats, the
//! director changing fights, a free-fly stop -- eases in and out and never
//! overshoots.

use glam::Vec3;

use crate::intro::Shot;
use crate::sim::Game;

/// A critically damped spring toward a desired shot: the fastest settle
/// with no overshoot. `omega` is its stiffness (rad/s): about 4 / omega
/// seconds to arrive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothCam {
    pub eye: Vec3,
    pub target: Vec3,
    eye_v: Vec3,
    target_v: Vec3,
    pub omega: f32,
    pub fov_y: f32,
}

impl SmoothCam {
    pub fn new(at: Shot, omega: f32) -> SmoothCam {
        SmoothCam { eye: at.eye, target: at.target, eye_v: Vec3::ZERO, target_v: Vec3::ZERO, omega, fov_y: at.fov_y }
    }

    /// Jump straight to a shot (the first frame, a teleport the viewer
    /// asked for).
    pub fn snap(&mut self, to: Shot) {
        *self = SmoothCam::new(to, self.omega);
    }

    /// Move toward `want`; returns the shot to draw.
    pub fn update(&mut self, dt: f32, want: Shot) -> Shot {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.1) } else { 0.0 };
        let spring = |x: &mut Vec3, v: &mut Vec3, goal: Vec3, w: f32| {
            // exact integration of x'' = -w^2 (x - goal) - 2 w x' over dt
            let d = *x - goal;
            let e = (-w * dt).exp();
            let a = *v + d * w;
            let nx = (d + a * dt) * e;
            let nv = (*v - a * w * dt) * e;
            *x = goal + nx;
            *v = nv;
        };
        if want.eye.is_finite() && want.target.is_finite() {
            spring(&mut self.eye, &mut self.eye_v, want.eye, self.omega);
            // the SAME stiffness as the eye: a stiffer target ran ahead of the
            // eye mid-move and swung the view 19 deg in a frame
            spring(&mut self.target, &mut self.target_v, want.target, self.omega);
            self.fov_y += (want.fov_y - self.fov_y) * (1.0 - (-self.omega * dt).exp());
        }
        Shot { eye: self.eye, target: self.target, fov_y: self.fov_y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// Frames the most interesting fight.
    Director,
    /// Chase-cam behind one cat.
    Follow(usize),
    /// Flies where the viewer steers.
    Free,
}

/// A frame of spectator input (edges for the switches).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Input {
    /// +1: the next cat; -1: the previous one (Tab / Shift+Tab).
    pub cycle: i32,
    pub director: bool,
    pub free: bool,
    /// Free flight: x right, y forward (the move stick / WASD).
    pub mv: glam::Vec2,
    /// Free flight: up/down.
    pub rise: f32,
    /// Look: yaw and pitch deltas, radians.
    pub look: glam::Vec2,
}

/// A new shot this far from the current one is cut to, not flown to.
pub const CUT_DIST: f32 = 40.0;

/// The director holds a fight at least this long before it considers
/// another, and switches only for one clearly better (`SWITCH_MARGIN`).
pub const DIRECTOR_HOLD: f32 = 4.0;
pub const SWITCH_MARGIN: f32 = 1.5;
/// Cats within this of one another are one fight.
pub const FIGHT_RADIUS: f32 = 18.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Spectator {
    pub mode: Mode,
    pub cam: SmoothCam,
    /// The director's current focus (a god index) and how long it has held.
    pub focus: usize,
    held: f32,
    free_eye: Vec3,
    free_yaw: f32,
    free_pitch: f32,
    started: bool,
    /// Deliberate cuts made (a switch too far to fly: see CUT_DIST).
    pub cuts: u32,
    /// How far the eye is lifted clear of the ground (rises fast but not
    /// instantly: see `update`).
    lift: f32,
}

impl Spectator {
    pub fn new() -> Spectator {
        let s = Shot { eye: Vec3::new(0.0, 60.0, -80.0), target: Vec3::ZERO, fov_y: 1.0 };
        Spectator { mode: Mode::Director, cam: SmoothCam::new(s, 3.0), focus: 0, held: 0.0, free_eye: s.eye, free_yaw: 0.0, free_pitch: -0.5, started: false, cuts: 0, lift: 0.0 }
    }

    /// How much is going on around god `i`: enemies close by, damage taken
    /// recently by anyone near, and someone near death.
    pub fn action_score(game: &Game, i: usize) -> f32 {
        let g = &game.gods[i];
        if g.respawn_t > 0.0 {
            return 0.0;
        }
        let mut score = 0.0;
        for (j, o) in game.gods.iter().enumerate() {
            if o.respawn_t > 0.0 || (o.pos - g.pos).length() > FIGHT_RADIUS {
                continue;
            }
            if o.team != g.team {
                score += 1.0;
            }
            if game.t - o.last_dmg_t < 3.0 {
                score += 1.5;
            }
            if o.hp < o.max_hp * 0.3 {
                score += 1.0;
            }
            let _ = j;
        }
        score
    }

    /// Where the director looks: the living cats round its focus, framed
    /// from above and to the side, pulled back to fit their spread.
    fn director_shot(game: &Game, focus: usize) -> Shot {
        let f = game.gods[focus].pos;
        let near: Vec<Vec3> = game.gods.iter().filter(|o| o.respawn_t <= 0.0 && (o.pos - f).length() <= FIGHT_RADIUS).map(|o| o.pos).collect();
        let n = near.len().max(1) as f32;
        let c = near.iter().fold(Vec3::ZERO, |a, p| a + *p) / n;
        let spread = near.iter().map(|p| (*p - c).length()).fold(0.0f32, f32::max);
        let back = 14.0 + spread * 1.2;
        // from the side the fight is fought across, and a little behind the
        // focus cat's team (blue attacks +z) -- unless rock is in the way:
        // on the pass that framing put the camera INSIDE a cliff. Then round
        // the fight, and closer, until the fight is in plain view.
        let side = Vec3::new(0.7, 0.0, -0.7 * game.gods[focus].team.push_z()).normalize();
        let target = c + Vec3::Y * 1.2;
        clear_shot(target, side, back, 0.7).unwrap_or(Shot { eye: c - side * back + Vec3::Y * (back * 0.7), target, fov_y: 1.0 })
    }

    /// Behind cat `i`, a little above, looking past it.
    fn follow_shot(game: &Game, i: usize) -> Shot {
        let g = &game.gods[i];
        let facing = crate::abilities::facing_of(g.yaw);
        // a dead cat: watch where it will come back
        let at = if g.respawn_t > 0.0 { g.team.fountain() } else { g.pos };
        let target = at + facing * 4.0 + Vec3::Y * 1.2;
        let eye = at - facing * 11.0 + Vec3::Y * 6.5;
        if crate::map::line_clear(eye, target) && eye.y > crate::map::surface_y(eye.x, eye.z) + 1.5 {
            return Shot { eye, target, fov_y: 0.95 };
        }
        clear_shot(target, facing, 11.0, 0.6).unwrap_or(Shot { eye, target, fov_y: 0.95 })
    }

    pub fn update(&mut self, game: &Game, inp: Input, dt: f32) -> Shot {
        let n = game.gods.len();
        if n == 0 {
            return Shot { eye: self.cam.eye, target: self.cam.target, fov_y: self.cam.fov_y };
        }
        // mode switches
        if inp.cycle != 0 {
            let cur = match self.mode {
                Mode::Follow(i) => i,
                _ => self.focus,
            };
            let next = (cur as i32 + inp.cycle).rem_euclid(n as i32) as usize;
            self.mode = Mode::Follow(next);
        }
        if inp.director {
            self.mode = Mode::Director;
            self.held = DIRECTOR_HOLD; // reconsider at once
        }
        if inp.free && self.mode != Mode::Free {
            // take off from where the camera is
            self.free_eye = self.cam.eye;
            let d = (self.cam.target - self.cam.eye).normalize_or_zero();
            self.free_yaw = d.z.atan2(d.x);
            self.free_pitch = d.y.clamp(-0.99, 0.99).asin();
            self.mode = Mode::Free;
        }
        let want = match self.mode {
            Mode::Director => {
                self.held += dt;
                if self.held >= DIRECTOR_HOLD || game.gods[self.focus.min(n - 1)].respawn_t > 0.0 {
                    let cur = Self::action_score(game, self.focus.min(n - 1));
                    let (best, score) = (0..n).map(|i| (i, Self::action_score(game, i))).fold((self.focus, -1.0), |b, c| if c.1 > b.1 { c } else { b });
                    if best != self.focus && (score > cur * SWITCH_MARGIN + 0.5 || game.gods[self.focus.min(n - 1)].respawn_t > 0.0) {
                        self.focus = best;
                        self.held = 0.0;
                    }
                }
                Self::director_shot(game, self.focus.min(n - 1))
            }
            Mode::Follow(i) => Self::follow_shot(game, i.min(n - 1)),
            Mode::Free => {
                self.free_yaw += inp.look.x;
                self.free_pitch = (self.free_pitch + inp.look.y).clamp(-1.4, 0.6);
                let fwd = Vec3::new(self.free_yaw.cos(), 0.0, self.free_yaw.sin());
                let right = Vec3::new(-fwd.z, 0.0, fwd.x);
                self.free_eye += (fwd * inp.mv.y + right * inp.mv.x) * 40.0 * dt + Vec3::Y * inp.rise * 25.0 * dt;
                // above the ground, inside the map
                let r = crate::consts::arena_radius() + 30.0;
                let flat = Vec3::new(self.free_eye.x, 0.0, self.free_eye.z);
                if flat.length() > r {
                    let k = r / flat.length();
                    self.free_eye.x *= k;
                    self.free_eye.z *= k;
                }
                let ground = crate::render::terrain_height(self.free_eye.x, self.free_eye.z);
                self.free_eye.y = self.free_eye.y.clamp(ground + 2.0, 260.0);
                let look = Vec3::new(self.free_pitch.cos() * fwd.x, self.free_pitch.sin(), self.free_pitch.cos() * fwd.z);
                Shot { eye: self.free_eye, target: self.free_eye + look * 10.0, fov_y: 1.0 }
            }
        };
        if !self.started {
            self.started = true;
            self.cam.snap(want);
        }
        // Free flight answers the stick directly; the spring would make it
        // swim. Everything else eases.
        if self.mode == Mode::Free {
            self.cam.snap(want);
            return want;
        }
        // A switch too far to fly CUTS, as a broadcast director would: on
        // the pass a flight between two fights crossed the peaks, and the
        // ground clamp shoved the eye up a cliff's height in one frame
        // (measured: 17.8 deg and 49 u in a frame).
        if (want.target - self.cam.target).length() > CUT_DIST {
            self.cam.snap(want);
            self.cuts += 1;
            self.lift = 0.0;
            return want;
        }
        let mut s = self.cam.update(dt, want);
        // clear of the ground: a lift that rises fast (60 u/s) and settles,
        // not an instant shove
        let ground = crate::map::surface_y(s.eye.x, s.eye.z) + 1.5;
        let need = (ground - s.eye.y).max(0.0);
        self.lift = if need > self.lift { (self.lift + 60.0 * dt).min(need) } else { (self.lift - 20.0 * dt).max(need) };
        s.eye.y += self.lift;
        s
    }

    /// A line for the overlay: the mode and who is watched.
    pub fn label(&self, game: &Game) -> String {
        match self.mode {
            Mode::Director => format!("DIRECTOR -- {}", game.god_name(self.focus.min(game.gods.len().saturating_sub(1)))),
            Mode::Follow(i) => format!("FOLLOWING {} ({})", game.god_name(i.min(game.gods.len().saturating_sub(1))), game.gods[i.min(game.gods.len() - 1)].team.name()),
            Mode::Free => "FREE CAMERA".to_string(),
        }
    }
}

/// A camera looking at `target`: from `side` (pulled back `back`, raised
/// `rise` of that), else turned round the target in eighths, else closer --
/// the first above the ground with a clear view of the target.
fn clear_shot(target: Vec3, side: Vec3, back: f32, rise: f32) -> Option<Shot> {
    for pull in [1.0f32, 0.75, 0.5, 0.35] {
        for turn in [0.0f32, 0.785, -0.785, 1.571, -1.571, 2.356, -2.356, 3.1416] {
            let (c, sn) = (turn.cos(), turn.sin());
            let dir = Vec3::new(side.x * c - side.z * sn, 0.0, side.x * sn + side.z * c).normalize_or_zero();
            let b = back * pull;
            let eye = target - dir * b + Vec3::Y * (b * rise);
            if eye.y > crate::map::surface_y(eye.x, eye.z) + 1.5 && crate::map::line_clear(eye, target) {
                return Some(Shot { eye, target, fov_y: 1.0 });
            }
        }
    }
    None
}

impl Default for Spectator {
    fn default() -> Self {
        Spectator::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step_stats(s: &mut Spectator, game: &Game, inp: Input, frames: usize) -> (f32, f32) {
        let mut prev: Option<Shot> = None;
        let (mut turn, mut step) = (0.0f32, 0.0f32);
        for k in 0..frames {
            let shot = s.update(game, if k == 0 { inp } else { Input { mv: inp.mv, rise: inp.rise, look: inp.look, ..Default::default() } }, 1.0 / 60.0);
            if let Some(p) = prev {
                turn = turn.max((p.target - p.eye).normalize().dot((shot.target - shot.eye).normalize()).clamp(-1.0, 1.0).acos().to_degrees());
                step = step.max((shot.eye - p.eye).length());
            }
            prev = Some(shot);
        }
        (turn, step)
    }

    /// The spring settles on its goal without overshooting.
    #[test]
    fn the_spring_arrives_without_overshoot() {
        let a = Shot { eye: Vec3::ZERO, target: Vec3::X, fov_y: 1.0 };
        let mut c = SmoothCam::new(a, 4.0);
        let goal = Shot { eye: Vec3::new(10.0, 0.0, 0.0), target: Vec3::new(11.0, 0.0, 0.0), fov_y: 0.8 };
        let mut max_x = 0.0f32;
        for _ in 0..240 {
            let s = c.update(1.0 / 60.0, goal);
            max_x = max_x.max(s.eye.x);
        }
        assert!(max_x <= 10.0 + 1e-3, "overshot to {max_x}");
        assert!((c.eye - goal.eye).length() < 0.01, "did not arrive: {}", c.eye);
        assert!((c.fov_y - 0.8).abs() < 0.01);
    }

    /// Cutting from one cat to another across the map is a move, not a
    /// jump; and the camera never goes into the ground doing it.
    #[test]
    fn switching_cats_moves_smoothly() {
        let mut game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        game.gods[9].pos = Vec3::new(40.0, 0.0, 90.0);
        let mut s = Spectator::new();
        s.update(&game, Input { cycle: 1, ..Default::default() }, 1.0 / 60.0);
        step_stats(&mut s, &game, Input::default(), 120);
        let (turn, step) = step_stats(&mut s, &game, Input { cycle: 8, ..Default::default() }, 180);
        assert_eq!(s.mode, Mode::Follow(9));
        assert!(turn < 8.0, "the view whipped {turn:.1} deg in a frame");
        assert!(step < 10.0, "the eye jumped {step:.1} u in a frame");
    }

    /// The director finds the fight: two enemies at each other's throats
    /// beat a cat standing alone, and it holds its pick against a marginally
    /// better one.
    #[test]
    fn the_director_frames_the_fight_and_does_not_flicker() {
        let mut game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        for (k, g) in game.gods.iter_mut().enumerate() {
            g.pos = Vec3::new(k as f32 * 40.0 - 180.0, 0.0, 0.0); // everyone alone
        }
        game.gods[7].pos = game.gods[2].pos + Vec3::new(3.0, 0.0, 0.0); // blue 2 v red 7
        game.gods[7].last_dmg_t = game.t;
        game.gods[2].last_dmg_t = game.t;
        let mut s = Spectator::new();
        s.update(&game, Input { director: true, ..Default::default() }, 1.0 / 60.0);
        assert!(s.focus == 2 || s.focus == 7, "focused {} -- not the fight", s.focus);
        let before = s.focus;
        // a marginally busier cat elsewhere does not steal the camera
        game.gods[4].pos = game.gods[8].pos + Vec3::new(2.0, 0.0, 0.0);
        for _ in 0..60 {
            s.update(&game, Input::default(), 1.0 / 60.0);
        }
        assert_eq!(s.focus, before, "flickered to another fight within the hold");
    }

    /// Free flight goes where it is steered, stays inside the map and
    /// above the ground.
    #[test]
    fn free_flight_steers_and_stays_in_bounds() {
        let game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        let mut s = Spectator::new();
        s.update(&game, Input { free: true, ..Default::default() }, 1.0 / 60.0);
        let start = s.cam.eye;
        for _ in 0..60 {
            s.update(&game, Input { mv: glam::Vec2::new(0.0, 1.0), ..Default::default() }, 1.0 / 60.0);
        }
        assert!((s.cam.eye - start).length() > 20.0, "did not fly");
        for _ in 0..1200 {
            s.update(&game, Input { mv: glam::Vec2::new(0.0, 1.0), rise: -1.0, ..Default::default() }, 1.0 / 60.0);
        }
        let e = s.cam.eye;
        assert!(Vec3::new(e.x, 0.0, e.z).length() <= crate::consts::arena_radius() + 30.0 + 1e-2, "left the map: {e}");
        assert!(e.y >= crate::render::terrain_height(e.x, e.z) + 2.0 - 1e-3, "into the ground: {e}");
    }
}
