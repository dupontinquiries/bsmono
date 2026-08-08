//! THE FOLD, as an animation rather than a flipbook.
//!
//! Two things live here.
//!
//! **The state machine**, ported from `exengine/transform.h`. It is not a
//! cooldown ability: transforming is an instant toggle EXCEPT for a forced
//! lockout after taking a heavy hit, so you cannot transform-cancel your way
//! out of every bad trade. That lockout is the damage rule the C engine
//! already had, and it is the thing that makes the move cost something.
//!
//! **The morph.** A chassis ships as a strip of five glTF poses, and the
//! first version of this stepped between them — five discrete bodies in
//! nine tenths of a second, which is a flipbook. Every pose in a strip comes
//! from the same source mesh through the same colour merge, so they share
//! vertex count AND vertex ORDER prim for prim. That means the poses are
//! morph targets: lerp vertex i of pose k with vertex i of pose k+1 and the
//! body moves continuously through the fold. Five poses stop being five
//! frames and become four segments of a spline.
//!
//! Personality comes out of the easing, not out of new geometry. A bot at
//! full health unfolds with a settled ease; a damaged one snaps, overshoots
//! and hitches. Same five poses, different creature.

use glam::Vec3;

use crate::meshgen::{MeshData, Vertex};

/// From `EX_XFORM_BLEND_SEC` — the C engine's visual transform duration.
/// Nearly half again the 0.9 the first pass used, because a fold that reads
/// as choreography needs time to have beats in it.
pub const BLEND_SEC: f32 = 1.3;
/// `EX_XFORM_LOCK_ON_HEAVY_HIT_SEC`
pub const LOCK_ON_HEAVY_HIT_SEC: f32 = 1.2;
/// `EX_XFORM_MIN_INTERVAL_SEC` — a hard floor against spam-toggling
pub const MIN_INTERVAL_SEC: f32 = 0.15;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// settled, free to transform
    Idle,
    /// mid-blend
    Transforming,
    /// denied — post-heavy-hit lockout
    Locked,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Vehicle,
    Robot,
}

/// How the fold carries itself. The names are foldview's, and they are the
/// same three moods its scheduler already understands, so a bot animated
/// here and a bot rendered there agree about what it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    /// unhurt: a settled, weighted ease with a held beat near the end
    Fluid,
    /// hurt: faster in, harder stop, no dwell
    Combat,
    /// badly hurt: snaps, overshoots, and hitches on the way
    Anger,
}

impl Profile {
    /// Personality FROM DAMAGE. This is the whole "different every time"
    /// idea: nothing about the geometry changes, only how it is timed, so a
    /// wounded bot reads as wounded without a second set of poses.
    pub fn from_health(health: f32) -> Profile {
        if health > 0.66 {
            Profile::Fluid
        } else if health > 0.30 {
            Profile::Combat
        } else {
            Profile::Anger
        }
    }

    /// Map linear time to fold position. Must return 0 at 0 and 1 at 1 —
    /// an easing that does not land exactly is a body that does not finish
    /// closing, and there is a test for it.
    pub fn ease(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            // smoothstep, then a held beat at 0.78 — the finale lands late,
            // which is the same rule foldview's scheduler uses
            Profile::Fluid => {
                let s = t * t * (3.0 - 2.0 * t);
                let hold = 1.0 - 0.06 * (1.0 - (t - 0.78).abs() * 6.0).max(0.0);
                (s * hold).clamp(0.0, 1.0) + (t >= 1.0) as i32 as f32 * (1.0 - (s * hold).clamp(0.0, 1.0))
            }
            // fast in, hard out: most of the motion is spent by 0.6
            Profile::Combat => {
                let s = 1.0 - (1.0 - t).powi(3);
                s
            }
            // overshoot and hitch. The wobble is windowed so it dies before
            // the end — a fold that is still ringing when it finishes has
            // not finished.
            Profile::Anger => {
                let s = 1.0 - (1.0 - t).powi(4);
                let window = (t * (1.0 - t) * 4.0).clamp(0.0, 1.0);
                let ring = (t * 22.0).sin() * 0.055 * window;
                (s + ring).clamp(0.0, 1.0)
            }
        }
    }
}

/// The state machine. Owns *when*; the morph owns *what it looks like*.
pub struct Xform {
    pub mode: Mode,
    pub state: State,
    /// 0..1 through the current blend, in the vehicle -> robot direction
    pub t: f32,
    dir: f32,
    lock_timer: f32,
    since_last: f32,
    /// per-chassis duration scale, the C engine's `dur_mul`
    pub dur_mul: f32,
    pub profile: Profile,
}

impl Default for Xform {
    fn default() -> Xform {
        Xform::new()
    }
}

impl Xform {
    pub fn new() -> Xform {
        Xform {
            mode: Mode::Vehicle,
            state: State::Idle,
            t: 0.0,
            dir: 1.0,
            lock_timer: 0.0,
            since_last: MIN_INTERVAL_SEC,
            dur_mul: 1.0,
            profile: Profile::Fluid,
        }
    }

    /// A heavy hit denies the transform for a beat. Not a cooldown on the
    /// ability — a consequence of being hit.
    pub fn heavy_hit(&mut self) {
        self.lock_timer = LOCK_ON_HEAVY_HIT_SEC;
        self.state = State::Locked;
    }

    /// Ask to fold. Returns false when denied, so the caller can play a
    /// refusal rather than silently doing nothing.
    pub fn request(&mut self, health: f32) -> bool {
        if self.lock_timer > 0.0 || self.since_last < MIN_INTERVAL_SEC {
            return false;
        }
        // reversing mid-fold is allowed and keeps its position: the body
        // turns around from where it is rather than snapping to an end
        self.dir = if self.state == State::Transforming {
            -self.dir
        } else if self.mode == Mode::Vehicle {
            1.0
        } else {
            -1.0
        };
        self.profile = Profile::from_health(health);
        self.state = State::Transforming;
        self.since_last = 0.0;
        true
    }

    pub fn update(&mut self, dt: f32) {
        self.since_last += dt;
        if self.lock_timer > 0.0 {
            self.lock_timer = (self.lock_timer - dt).max(0.0);
            if self.lock_timer <= 0.0 && self.state == State::Locked {
                self.state = State::Idle;
            }
            return;
        }
        if self.state != State::Transforming {
            return;
        }
        let dur = (BLEND_SEC * self.dur_mul).max(0.05);
        self.t = (self.t + self.dir * dt / dur).clamp(0.0, 1.0);
        if self.t >= 1.0 {
            self.mode = Mode::Robot;
            self.state = State::Idle;
        } else if self.t <= 0.0 {
            self.mode = Mode::Vehicle;
            self.state = State::Idle;
        }
    }

    /// Eased fold position, which is what the morph and the hitbox read.
    pub fn eased(&self) -> f32 {
        self.profile.ease(self.t)
    }

    pub fn locked(&self) -> bool {
        self.lock_timer > 0.0
    }

    /// True once the body is on its feet — the gate for robot controls.
    pub fn is_robot(&self) -> bool {
        self.eased() > 0.5
    }
}

/// The strip's vertices, kept CPU-side so they can be blended.
///
/// `poses[k][p]` is prim `p` of pose `k`. Every pose in a strip comes from
/// one source mesh through one colour merge, so the shapes match prim for
/// prim and vertex for vertex — `check()` refuses to build a morph that does
/// not, because a mismatched strip would blend garbage rather than fail.
pub struct Morph {
    poses: Vec<Vec<Vec<Vertex>>>,
    idx: Vec<Vec<u32>>,
    /// scratch, reused every frame so a 60 Hz morph does not allocate
    scratch: Vec<Vertex>,
}

impl Morph {
    pub fn build(poses: Vec<Vec<Vec<Vertex>>>, idx: Vec<Vec<u32>>) -> Option<Morph> {
        if poses.len() < 2 {
            return None;
        }
        let shape: Vec<usize> = poses[0].iter().map(|p| p.len()).collect();
        for p in &poses {
            if p.len() != shape.len() {
                return None;
            }
            for (a, b) in p.iter().zip(&shape) {
                if a.len() != *b {
                    return None;
                }
            }
        }
        Some(Morph { poses, idx, scratch: Vec::new() })
    }

    pub fn prims(&self) -> usize {
        self.poses[0].len()
    }

    /// Blend prim `p` at fold position `t` (0 vehicle, 1 robot).
    ///
    /// `t` lands between two poses, so this is a lerp of the pair either
    /// side. Normals are blended too and renormalised — skipping that is
    /// what makes a cheap morph look like shrink-wrap partway through.
    pub fn sample(&mut self, p: usize, t: f32) -> MeshData {
        let n = self.poses.len();
        let x = (t.clamp(0.0, 1.0) * (n - 1) as f32).min((n - 1) as f32 - 1e-5);
        let k = x.floor() as usize;
        let f = x - k as f32;
        let a = &self.poses[k][p];
        let b = &self.poses[(k + 1).min(n - 1)][p];
        self.scratch.clear();
        self.scratch.reserve(a.len());
        for (va, vb) in a.iter().zip(b.iter()) {
            let pa = Vec3::from_array(va.pos).lerp(Vec3::from_array(vb.pos), f);
            let na = Vec3::from_array(va.nrm)
                .lerp(Vec3::from_array(vb.nrm), f)
                .normalize_or(Vec3::Y);
            self.scratch.push(Vertex {
                pos: pa.to_array(),
                nrm: na.to_array(),
                uv: va.uv,
            });
        }
        MeshData { verts: std::mem::take(&mut self.scratch), idx: self.idx[p].clone() }
    }

    /// Half-extents at this fold position, so the hitbox tracks the body
    /// instead of jumping at pose boundaries.
    pub fn half_at(&mut self, t: f32) -> Vec3 {
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in 0..self.prims() {
            let m = self.sample(p, t);
            for v in &m.verts {
                let q = Vec3::from_array(v.pos);
                lo = lo.min(q);
                hi = hi.max(q);
            }
            self.scratch = m.verts;
        }
        (hi - lo) * 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vtx(x: f32) -> Vertex {
        Vertex { pos: [x, 0.0, 0.0], nrm: [0.0, 1.0, 0.0], uv: [0.0, 0.0] }
    }

    #[test]
    fn every_profile_starts_at_zero_and_lands_exactly() {
        for p in [Profile::Fluid, Profile::Combat, Profile::Anger] {
            assert!(p.ease(0.0).abs() < 1e-5, "{p:?} does not start closed");
            assert!((p.ease(1.0) - 1.0).abs() < 1e-5, "{p:?} does not finish open");
            // and never leaves the rails, however much it overshoots
            for i in 0..=100 {
                let v = p.ease(i as f32 / 100.0);
                assert!((0.0..=1.0).contains(&v), "{p:?} left the range at {i}: {v}");
            }
        }
    }

    #[test]
    fn damage_picks_the_personality() {
        assert_eq!(Profile::from_health(1.0), Profile::Fluid);
        assert_eq!(Profile::from_health(0.5), Profile::Combat);
        assert_eq!(Profile::from_health(0.1), Profile::Anger);
        // the wounded ones commit harder early — that is what "snappier" is
        let f = Profile::Fluid.ease(0.35);
        let a = Profile::Anger.ease(0.35);
        assert!(a > f, "anger should be further along at the same time ({a} vs {f})");
    }

    #[test]
    fn a_heavy_hit_denies_the_fold() {
        let mut x = Xform::new();
        assert!(x.request(1.0));
        x.update(BLEND_SEC * 2.0);
        assert_eq!(x.mode, Mode::Robot);
        x.heavy_hit();
        assert!(x.locked());
        assert!(!x.request(1.0), "a heavy hit did not deny the transform");
        x.update(LOCK_ON_HEAVY_HIT_SEC * 1.1);
        assert!(!x.locked());
        assert!(x.request(1.0), "the lockout never released");
    }

    #[test]
    fn spam_toggling_is_floored() {
        let mut x = Xform::new();
        assert!(x.request(1.0));
        x.update(MIN_INTERVAL_SEC * 0.5);
        assert!(!x.request(1.0), "toggled again inside the minimum interval");
        x.update(MIN_INTERVAL_SEC);
        assert!(x.request(1.0));
    }

    #[test]
    fn reversing_mid_fold_keeps_its_place() {
        let mut x = Xform::new();
        x.request(1.0);
        x.update(BLEND_SEC * 0.4);
        // the interval wait keeps folding, so read the position AFTER it —
        // that is the place the reversal has to start from
        x.update(MIN_INTERVAL_SEC);
        let mid = x.t;
        assert!(mid > 0.1 && mid < 0.9, "mid was {mid}");
        assert!(x.request(1.0));
        x.update(0.001);
        // it turned around from where it was, it did not snap to an end
        assert!(x.t < mid + 0.05 && x.t > 0.05, "reversal jumped to {}", x.t);
        x.update(BLEND_SEC * 2.0);
        assert_eq!(x.mode, Mode::Vehicle);
    }

    #[test]
    fn the_morph_is_continuous_between_poses() {
        // three poses, a point sliding 0 -> 1 -> 2
        let poses = vec![
            vec![vec![vtx(0.0)]],
            vec![vec![vtx(1.0)]],
            vec![vec![vtx(2.0)]],
        ];
        let mut m = Morph::build(poses, vec![vec![0]]).expect("strip rejected");
        assert!((m.sample(0, 0.0).verts[0].pos[0] - 0.0).abs() < 1e-5);
        assert!((m.sample(0, 0.5).verts[0].pos[0] - 1.0).abs() < 1e-4);
        assert!((m.sample(0, 1.0).verts[0].pos[0] - 2.0).abs() < 1e-4);
        // quarter way is halfway between pose 0 and pose 1 — the thing a
        // flipbook cannot do
        assert!((m.sample(0, 0.25).verts[0].pos[0] - 0.5).abs() < 1e-4);
        // and no step anywhere across the whole fold
        let mut prev = m.sample(0, 0.0).verts[0].pos[0];
        for i in 1..=200 {
            let v = m.sample(0, i as f32 / 200.0).verts[0].pos[0];
            assert!((v - prev).abs() < 0.05, "a step at {i}: {prev} -> {v}");
            prev = v;
        }
    }

    #[test]
    fn a_mismatched_strip_is_refused_not_blended() {
        let bad = vec![vec![vec![vtx(0.0), vtx(1.0)]], vec![vec![vtx(0.0)]]];
        assert!(Morph::build(bad, vec![vec![0]]).is_none());
        let single = vec![vec![vec![vtx(0.0)]]];
        assert!(Morph::build(single, vec![vec![0]]).is_none());
    }

    #[test]
    fn the_hitbox_follows_the_body() {
        // a body that grows tall as it folds
        let tall = |y: f32| Vertex { pos: [0.0, y, 0.0], nrm: [0.0, 1.0, 0.0], uv: [0.0, 0.0] };
        let poses = vec![
            vec![vec![tall(0.0), tall(1.0)]],
            vec![vec![tall(0.0), tall(4.0)]],
        ];
        let mut m = Morph::build(poses, vec![vec![0, 1]]).unwrap();
        let a = m.half_at(0.0).y;
        let b = m.half_at(0.5).y;
        let c = m.half_at(1.0).y;
        assert!(a < b && b < c, "the hitbox did not grow with the body: {a} {b} {c}");
    }
}
