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

    /// Which way the current or most recent fold is/was running: `true`
    /// toward the robot, `false` toward the vehicle. Exists so callers that
    /// only need the direction (a sound cue, say) don't need `dir` itself
    /// made public.
    pub fn folding_to_robot(&self) -> bool {
        self.dir > 0.0
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
/// Remap a global fold parameter so each strip segment gets its own share
/// of the timeline.
///
/// `Morph::sample` maps `t` linearly onto the strip, so with five poses
/// every one of the four segments takes a quarter of the transform
/// whatever it contains. A staged transform does not want that: the
/// stage where the innards fuse should be slow and legible, and getting
/// the skeleton into position should be quick. See
/// `docs/STAGED_TRANSFORM.md`.
///
/// `weights` is one duration per SEGMENT (one fewer than the pose count).
/// Only the ratios matter. Returns a position along the strip in `[0, 1]`
/// which `sample` then scales by `n - 1` as before, so uniform weights
/// reproduce the old behaviour exactly.
///
/// Degenerate input falls back to uniform rather than distorting the
/// fold: no weights, a single weight, anything non-finite, anything
/// negative, or a total of zero. A transform that runs evenly is a tuning
/// mistake someone will notice; one that jumps or stalls looks like a
/// crash. (See docs/NON_FINITE_INPUTS.md for why this is spelled out.)
pub fn stage_time(t: f32, weights: Option<&[f32]>) -> f32 {
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
    let Some(w) = weights else { return t };
    if w.len() < 2 || w.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return t;
    }
    let total: f32 = w.iter().sum();
    if total <= 1e-6 {
        return t;
    }
    // Walk the segments, spending `t` against each one's share.
    let n = w.len() as f32;
    let mut acc = 0.0f32;
    for (i, seg) in w.iter().enumerate() {
        let share = seg / total;
        if t <= acc + share || i == w.len() - 1 {
            // A zero-width segment is stepped over rather than divided by.
            let f = if share > 1e-9 { ((t - acc) / share).clamp(0.0, 1.0) } else { 1.0 };
            return ((i as f32 + f) / n).clamp(0.0, 1.0);
        }
        acc += share;
    }
    t
}

pub struct Morph {
    /// Per-segment durations; `None` is uniform. See `stage_time`.
    pub stage_weights: Option<Vec<f32>>,
    poses: Vec<Vec<Vec<Vertex>>>,
    idx: Vec<Vec<u32>>,
    /// scratch, reused every frame so a 60 Hz morph does not allocate
    scratch: Vec<Vertex>,
    /// AllSpark-style geometric compaction layered over the lerp (see
    /// `compact.rs`). `None` keeps the original straight-lerp behaviour
    /// bit-for-bit, so this is opt-in per chassis rather than a change
    /// to how every already-shipped body folds.
    pub compaction: Option<crate::compact::Params>,
    /// per-prim centroid of the vehicle pose, cached at build time —
    /// `Style::Shrink` compacts toward it
    centres: Vec<Vec3>,
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
        // per-prim centroid of pose 0 (the vehicle), for Shrink
        let centres: Vec<Vec3> = poses[0]
            .iter()
            .map(|prim| {
                if prim.is_empty() {
                    return Vec3::ZERO;
                }
                let sum: Vec3 = prim.iter().fold(Vec3::ZERO, |acc, v| acc + Vec3::from_array(v.pos));
                sum / prim.len() as f32
            })
            .collect();
        Some(Morph { stage_weights: None, poses, idx, scratch: Vec::new(), compaction: None, centres })
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
        let t = stage_time(t, self.stage_weights.as_deref());
        let n = self.poses.len();
        let x = (t.clamp(0.0, 1.0) * (n - 1) as f32).min((n - 1) as f32 - 1e-5);
        let k = x.floor() as usize;
        let f = x - k as f32;
        let a = &self.poses[k][p];
        let b = &self.poses[(k + 1).min(n - 1)][p];
        self.scratch.clear();
        self.scratch.reserve(a.len());
        let compaction = self.compaction;
        let centre = self.centres.get(p).copied().unwrap_or(Vec3::ZERO);
        for (va, vb) in a.iter().zip(b.iter()) {
            let mut pa = Vec3::from_array(va.pos).lerp(Vec3::from_array(vb.pos), f);
            let na = Vec3::from_array(va.nrm)
                .lerp(Vec3::from_array(vb.nrm), f)
                .normalize_or(Vec3::Y);
            // compaction runs AFTER the lerp so the two compose: the
            // lerp decides where the vertex is going, compaction
            // decides how the matter gets there. `t`, not `f` -- the
            // curve spans the whole fold, not each pose segment.
            if let Some(params) = &compaction {
                pa = crate::compact::displace(pa, na, centre, t, params);
            }
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
        self.bounds_at(t).0
    }

    /// Half-extents AND the drop from the origin to the lowest point.
    ///
    /// These are the same number only when the pose is centred on its own
    /// bounding box, and a fold strip stops being centred the moment it stands
    /// up: measured on `analog`, the vehicle pose spans y -0.239..0.239 (half
    /// 0.239, |lo| 0.239) while the robot spans -0.400..0.355 — half 0.377,
    /// but 0.400 to the feet.
    ///
    /// The caller seats the body by pushing it up by half_h, so the difference
    /// is exactly how far the robot sinks. 50 of 221 chassis are affected, some
    /// by more than their own height — a legged bot's mass sits high and its
    /// feet hang far below centre, which is precisely why a tread's median
    /// point and a foot's contact point are not the same place.
    ///
    /// Returned together rather than as a second pass: sampling every prim at
    /// `t` is the expensive part and doing it twice to get two numbers out of
    /// one bounding box would double the cost of every frame that folds.
    pub fn bounds_at(&mut self, t: f32) -> (Vec3, f32) {
        // THE SAME MEASUREMENT THE STATIC LOAD MAKES. This used absolute
        // min/max over every blended vertex while `model::load_raw_at`
        // measures the MASS (`massfilter`), so a fold handed physics a
        // ride height computed one way and a spawn handed it one computed
        // the other — debris made them disagree by metres, and the body
        // sank or floated the moment a morph started. Two answers to one
        // question; now one.
        let mut pts: Vec<Vec<Vec3>> = Vec::with_capacity(self.prims());
        for p in 0..self.prims() {
            let m = self.sample(p, t);
            pts.push(m.verts.iter().map(|v| Vec3::from_array(v.pos)).collect());
            self.scratch = m.verts;
        }
        let refs: Vec<&[Vec3]> = pts.iter().map(|v| v.as_slice()).collect();
        let mass = crate::massfilter::classify(&refs);
        crate::massfilter::bounds(&refs, &mass.keep).unwrap_or_else(|| {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in refs.iter().flat_map(|p| p.iter()) {
                lo = lo.min(*v);
                hi = hi.max(*v);
            }
            ((hi - lo) * 0.5, -lo.y)
        })
    }
}

/// Redistributes bounding-box volume from LENGTH into HEIGHT when the
/// raw lerp's silhouette runs longer than a straight interpolation
/// between the vehicle's own length and the robot's own length.
///
/// This is the "expanding the long way, then shrinking back a ton"
/// artifact: vertex-lerp travels each vertex in a straight line toward
/// its counterpart in the next pose, and when a part's target sits far
/// away (a nose panel becoming a shoulder, say) the geometry in transit
/// pushes the silhouette's LENGTH out past what either end pose
/// actually measures. It snaps back the moment the fold finishes,
/// which reads as the body being briefly, arbitrarily larger rather
/// than folding with any sense of conserved mass.
///
/// The correction: cap the displayed length at the straight-line
/// envelope between the vehicle's and robot's own measured lengths,
/// and instead of just discarding the overflow, fold that volume into
/// HEIGHT — a transforming robot growing taller as it stands up reads
/// as purpose; a vehicle stretching longer than either of its own end
/// states reads as a glitch. Total displayed volume is conserved
/// (checked by the tests below), not just capped.
///
/// A layer on top of the measured bounds, same relationship
/// `compact.rs`'s displacement field has to the underlying lerp — this
/// does not touch a single vertex, only the half-extents a caller
/// (the hitbox, the seating check) uses afterward.
///
/// NOT visually verified: this environment has no GPU, so the actual
/// on-screen result is unchecked. The math is tested against the
/// invariant it claims (length capped, volume conserved), not against
/// a rendered frame — tune `min_overflow` and whether this should
/// blend with `t` toward the edges once someone can actually look at it.
pub fn redistribute_length_to_height(actual: Vec3, veh_len: f32, bot_len: f32, t: f32) -> Vec3 {
    // NON-FINITE IN, UNCHANGED OUT.
    //
    // Every argument here reaches a multiply that feeds the vehicle's
    // dimensions. Measured before this guard: a NaN `t` returned
    // (1.0, NaN, NaN), a NaN `veh_len` the same, and a NaN in `actual`
    // spread to the components beside it. A box with NaN extents is not a
    // mis-sized vehicle, it is one that fails every comparison it is later
    // used in — including the bounds checks meant to catch it.
    //
    // `actual` unchanged is this function's own answer for "nothing to
    // redistribute", so it is the honest result for "nothing usable to
    // redistribute with" too. Note `f32::clamp` does not launder a NaN:
    // `NAN.clamp(0.0, 1.0)` is NaN, which is how `t` got through.
    if !actual.is_finite() || !veh_len.is_finite() || !bot_len.is_finite() || !t.is_finite() {
        return actual;
    }
    let expected_len = veh_len + (bot_len - veh_len) * t.clamp(0.0, 1.0);
    if actual.z <= expected_len || actual.x <= 0.0 || actual.y <= 0.0 {
        return actual; // nothing to redistribute, or a degenerate box
    }
    let overflow_vol = 8.0 * actual.x * actual.y * (actual.z - expected_len);
    let capped_vol = 8.0 * actual.x * actual.y * expected_len;
    if capped_vol <= 0.0 {
        return actual;
    }
    let height_scale = (capped_vol + overflow_vol) / capped_vol;
    Vec3::new(actual.x, actual.y * height_scale, expected_len)
}

#[cfg(test)]
mod endpoint_tests {

    /// Per-segment durations. `Morph::sample` maps `t` linearly onto the
    /// strip, so every segment takes an equal share whatever it contains;
    /// a staged transform wants the fusion stage slow and the posing
    /// stages quick. See docs/STAGED_TRANSFORM.md.
    #[test]
    fn uniform_weights_reproduce_the_old_mapping() {
        // The whole point of the fallback: an existing chassis with no
        // weights must fold exactly as it did.
        for t in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            assert!((stage_time(t, None) - t).abs() < 1e-6);
            assert!((stage_time(t, Some(&[1.0, 1.0, 1.0, 1.0])) - t).abs() < 1e-5, "t={t}");
        }
    }

    #[test]
    fn a_heavier_segment_takes_more_of_the_timeline() {
        // Two segments, the second three times as long. At the halfway
        // point of the transform we should still be inside the FIRST
        // segment's share only if that share exceeds half — here it is a
        // quarter, so t=0.5 is well into segment two.
        let w = [1.0f32, 3.0];
        // segment one owns t in [0, 0.25] and maps onto strip [0, 0.5]
        assert!((stage_time(0.25, Some(&w)) - 0.5).abs() < 1e-5);
        // half the transform is a third of the way through segment two
        let mid = stage_time(0.5, Some(&w));
        assert!(mid > 0.5 && mid < 0.75, "got {mid}");
    }

    #[test]
    fn the_endpoints_are_still_the_endpoints() {
        for w in [vec![1.0f32, 3.0], vec![5.0, 1.0, 2.0], vec![0.1, 0.1, 9.0]] {
            assert!((stage_time(0.0, Some(&w)) - 0.0).abs() < 1e-6, "{w:?}");
            assert!((stage_time(1.0, Some(&w)) - 1.0).abs() < 1e-6, "{w:?}");
        }
    }

    #[test]
    fn the_mapping_is_monotonic() {
        // A fold that goes backwards mid-transform reads as a glitch.
        let w = [2.0f32, 1.0, 4.0, 1.0];
        let mut prev = -1.0;
        for i in 0..=200 {
            let v = stage_time(i as f32 / 200.0, Some(&w));
            assert!(v >= prev - 1e-6, "went backwards at {i}: {prev} -> {v}");
            prev = v;
        }
    }

    #[test]
    fn degenerate_weights_fall_back_to_uniform() {
        // A transform that runs evenly is a tuning mistake someone
        // notices; one that jumps or stalls looks like a crash.
        for bad in [
            vec![],
            vec![1.0f32],
            vec![0.0, 0.0],
            vec![1.0, f32::NAN],
            vec![1.0, -2.0],
            vec![f32::INFINITY, 1.0],
        ] {
            for t in [0.0f32, 0.3, 0.7, 1.0] {
                let v = stage_time(t, Some(&bad));
                assert!(v.is_finite(), "{bad:?} at {t} gave {v}");
                assert!((v - t).abs() < 1e-5, "{bad:?} at {t} gave {v}, expected uniform");
            }
        }
    }

    #[test]
    fn a_zero_width_segment_is_stepped_over_not_divided_by() {
        let w = [1.0f32, 0.0, 1.0];
        let mut prev = -1.0;
        for i in 0..=100 {
            let v = stage_time(i as f32 / 100.0, Some(&w));
            assert!(v.is_finite() && v >= prev - 1e-6);
            prev = v;
        }
    }

    #[test]
    fn a_non_finite_fold_parameter_does_not_escape() {
        for t in [f32::NAN, f32::INFINITY, -5.0, 9.0] {
            let v = stage_time(t, Some(&[1.0, 2.0]));
            assert!((0.0..=1.0).contains(&v), "t={t} gave {v}");
        }
    }

    #[test]
    fn non_finite_inputs_leave_the_box_alone() {
        // A box with NaN extents is not a mis-sized vehicle — it fails
        // every comparison it is later used in, including the bounds
        // checks meant to catch it.
        let good = Vec3::new(1.0, 1.0, 4.0);
        assert_eq!(redistribute_length_to_height(good, 2.0, 3.0, f32::NAN), good);
        assert_eq!(redistribute_length_to_height(good, f32::NAN, 3.0, 0.5), good);
        assert_eq!(redistribute_length_to_height(good, 2.0, f32::INFINITY, 0.5), good);
        // A NaN that ARRIVED in `actual` comes back out — this function
        // does not invent geometry — but it must not spread to the
        // components beside it, which is what happened before.
        // Compared per-component because NaN != NaN makes assert_eq on the
        // whole vector fail even when every element matches.
        let bad = Vec3::new(f32::NAN, 1.0, 4.0);
        let out = redistribute_length_to_height(bad, 2.0, 3.0, 0.5);
        assert!(out.x.is_nan());
        assert_eq!(out.y, 1.0, "the NaN spread into y");
        assert_eq!(out.z, 4.0, "the NaN spread into z");
    }

    #[test]
    fn a_normal_redistribution_still_conserves_volume() {
        // The guard must not change the working path: length above the
        // expected value is traded for height at constant volume.
        let a = Vec3::new(1.0, 1.0, 4.0);
        let r = redistribute_length_to_height(a, 2.0, 3.0, 1.0);
        assert_eq!(r.z, 3.0, "length capped to expected");
        let before = a.x * a.y * a.z;
        let after = r.x * r.y * r.z;
        assert!((before - after).abs() < 1e-4, "volume changed: {before} -> {after}");
    }
    use super::*;

    /// Step a fold the way the shell does, recording the `t` values a caller
    /// would actually act on if it gates on `state == Transforming` AFTER
    /// calling `update` — which is what `Game::advance_xform` did.
    fn observed(dir_to_robot: bool) -> Vec<f32> {
        let mut x = Xform::new();
        if !dir_to_robot {
            x.t = 1.0;
            x.mode = Mode::Robot;
        }
        x.since_last = MIN_INTERVAL_SEC;
        assert!(x.request(1.0), "the fold should start");
        let mut seen = Vec::new();
        for _ in 0..600 {
            x.update(1.0 / 60.0);
            if x.state == State::Transforming {
                seen.push(x.t);
            }
        }
        seen
    }

    #[test]
    fn the_endpoint_is_never_observed_by_a_state_gated_caller() {
        // THE TRAP, kept as a test so nobody re-introduces the gate.
        //
        // `update` flips state to Idle on the same call that `t` lands on the
        // endpoint, so a caller that checks the state afterwards never sees
        // t == 1.0 or t == 0.0. It sees the frame before, and if it is
        // writing geometry from `t` that means the fold's final pose is never
        // drawn: limbs stop short of their sockets, and the hitbox stops
        // short of its final size.
        let to_robot = observed(true);
        assert!(!to_robot.is_empty());
        let last = *to_robot.last().unwrap();
        assert!(last < 1.0, "the gate would have observed the endpoint after all");
        assert!(last > 0.9, "and it stops close to it, which is why it is easy to miss: {last}");

        let to_vehicle = observed(false);
        let last = *to_vehicle.last().unwrap();
        assert!(last > 0.0, "same in the other direction");
        assert!(last < 0.1, "{last}");
    }

    #[test]
    fn t_does_land_exactly_on_the_endpoint() {
        // the value is correct; only the observation is missed. So the fix is
        // for the caller to run once more after the flip, not to change the
        // clamp.
        let mut x = Xform::new();
        x.since_last = MIN_INTERVAL_SEC;
        assert!(x.request(1.0));
        for _ in 0..600 {
            x.update(1.0 / 60.0);
        }
        assert_eq!(x.t, 1.0);
        assert_eq!(x.state, State::Idle);
        assert_eq!(x.mode, Mode::Robot);
    }

    #[test]
    fn a_caller_that_runs_one_more_frame_sees_it() {
        // what `advance_xform` does now: remember whether it WAS folding, and
        // run the body on the frame the fold ends.
        let mut x = Xform::new();
        x.since_last = MIN_INTERVAL_SEC;
        assert!(x.request(1.0));
        let mut last_acted_on = None;
        for _ in 0..600 {
            let was = x.state == State::Transforming;
            x.update(1.0 / 60.0);
            if x.state == State::Transforming || was {
                last_acted_on = Some(x.t);
            }
        }
        assert_eq!(last_acted_on, Some(1.0), "the endpoint pose is written now");
    }
}

#[cfg(test)]
mod tests {

    /// ONE FOLD DURATION, AND IT IS PER-CHASSIS.
    ///
    /// `main.rs` carried a second constant, `XFORM_DUR = 0.9`, unused, beside
    /// this module's `BLEND_SEC = 1.3`. Its doc said per-chassis timing was
    /// impossible — "nothing carries that across the glTF strip, so the engine
    /// uses one duration for every bot until it does" — which had stopped
    /// being true: `chassis_fold_mul` feeds `content::CHASSIS::xform_dur` into
    /// `dur_mul`, so a juggernaut already folds slower than a raptor.
    ///
    /// A stale constant that contradicts the live path is worse than dead
    /// code, because the number is plausible and someone will eventually use
    /// it. It was deleted; this asserts the thing that made it wrong, so the
    /// deletion cannot be quietly undone.
    #[test]
    fn fold_duration_varies_by_chassis() {
        use crate::content::CHASSIS;
        assert!(CHASSIS.len() >= 3, "the chassis table shrank to {}", CHASSIS.len());
        let durs: Vec<f32> = CHASSIS.iter().map(|c| c.xform_dur).collect();
        let lo = durs.iter().cloned().fold(f32::MAX, f32::min);
        let hi = durs.iter().cloned().fold(f32::MIN, f32::max);
        assert!(
            hi > lo * 1.2,
            "every chassis folds at nearly the same rate ({lo:.2}..{hi:.2}) — \
             dur_mul is doing nothing and a flat constant would be equivalent"
        );
        // foldaudit.rs documents the range these multipliers may take
        assert!(lo >= 0.7 && hi <= 1.4, "xform_dur left its documented 0.8-1.35 band: {lo}..{hi}");
        // the baseline chassis is the unit, or "multiplier" means nothing
        let std = crate::content::chassis("standard").expect("a standard chassis");
        assert!((std.xform_dur - 1.0).abs() < 1e-6, "standard is not 1.0x");
        // and the real duration is BLEND_SEC scaled by it, with no other source
        assert!((BLEND_SEC * std.xform_dur - BLEND_SEC).abs() < 1e-6);
    }

    use super::*;

    #[test]
    fn no_redistribution_when_length_is_already_within_the_envelope() {
        // veh_len 2.0, bot_len 1.0, t 0.5 -> expected 1.5. Actual length
        // 1.4 is under that -- nothing should move.
        let actual = Vec3::new(0.5, 0.3, 1.4);
        let out = redistribute_length_to_height(actual, 2.0, 1.0, 0.5);
        assert_eq!(out, actual, "an already-short body was still redistributed");
    }

    #[test]
    fn overflowing_length_is_capped_and_the_volume_moves_to_height() {
        let actual = Vec3::new(0.5, 0.3, 2.5); // ballooned length
        let veh_len = 2.0;
        let bot_len = 1.0;
        let t = 0.5; // expected length 1.5
        let out = redistribute_length_to_height(actual, veh_len, bot_len, t);
        assert!((out.z - 1.5).abs() < 1e-5, "length should be capped to the envelope, got {}", out.z);
        assert!(out.y > actual.y, "height should have grown to absorb the overflow");
        assert_eq!(out.x, actual.x, "width is untouched -- only length and height move");
        // the real invariant: total volume before and after should match,
        // not just "length went down"
        let vol_before = 8.0 * actual.x * actual.y * actual.z;
        let vol_after = 8.0 * out.x * out.y * out.z;
        assert!((vol_before - vol_after).abs() < 1e-4,
            "volume should be conserved, not discarded: {vol_before} vs {vol_after}");
    }

    #[test]
    fn a_degenerate_box_is_returned_unchanged_rather_than_producing_nan_or_inf() {
        let zero = Vec3::new(0.0, 0.3, 2.5);
        let out = redistribute_length_to_height(zero, 1.0, 2.0, 0.5);
        assert!(out.x.is_finite() && out.y.is_finite() && out.z.is_finite());
    }

    #[test]
    fn the_endpoints_of_the_fold_are_never_touched() {
        // at t=0 and t=1 the expected length equals one of the real end
        // lengths exactly, so a body that is not currently ballooning
        // (the normal case at rest) never gets redistributed at the
        // poses that ship as-is -- same "endpoints untouched" property
        // compact.rs's own curve(t) already guarantees for its layer.
        let veh_len = 2.0;
        let bot_len = 1.0;
        let at_rest_vehicle = Vec3::new(0.5, 0.3, veh_len);
        let at_rest_robot = Vec3::new(0.5, 0.3, bot_len);
        assert_eq!(redistribute_length_to_height(at_rest_vehicle, veh_len, bot_len, 0.0), at_rest_vehicle);
        assert_eq!(redistribute_length_to_height(at_rest_robot, veh_len, bot_len, 1.0), at_rest_robot);
    }

    fn vtx(x: f32) -> Vertex {
        Vertex { pos: [x, 0.0, 0.0], nrm: [0.0, 1.0, 0.0], uv: [0.0, 0.0] }
    }

    #[test]
    fn compaction_composes_with_the_lerp_without_disturbing_either_end() {
        // three vertices spread far enough apart to land in different
        // voxel cells, so quantisation has something to actually do
        let pose_a = vec![vec![vtx(0.0), vtx(1.0), vtx(2.0)]];
        let pose_b = vec![vec![vtx(4.0), vtx(5.0), vtx(6.0)]];
        let idx = vec![vec![0u32, 1, 2]];

        let mut plain = Morph::build(vec![pose_a.clone(), pose_b.clone()], idx.clone()).unwrap();
        let mut compacted = Morph::build(vec![pose_a, pose_b], idx).unwrap();
        compacted.compaction = Some(crate::compact::Params {
            style: crate::compact::Style::Shrink,
            cell: 0.5,
            strength: 1.0,
            min_thickness: 0.0,
        });

        // the endpoints must be bit-identical: this is the property that
        // makes compaction safe to switch on for a shipped chassis
        for t in [0.0f32, 1.0] {
            let p = plain.sample(0, t);
            let c = compacted.sample(0, t);
            for (vp, vc) in p.verts.iter().zip(c.verts.iter()) {
                let d = Vec3::from_array(vp.pos) - Vec3::from_array(vc.pos);
                assert!(d.length() < 1e-5, "endpoint t={t} diverged by {}", d.length());
            }
        }

        // mid-fold they must genuinely differ, or the effect is inert
        let p = plain.sample(0, 0.5);
        let c = compacted.sample(0, 0.5);
        let moved: f32 = p
            .verts
            .iter()
            .zip(c.verts.iter())
            .map(|(vp, vc)| (Vec3::from_array(vp.pos) - Vec3::from_array(vc.pos)).length())
            .sum();
        assert!(moved > 0.05, "compaction changed nothing mid-fold (total {moved})");

        // and nothing became NaN on the way -- the f32 sin(PI) trap
        for t in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            for v in compacted.sample(0, t).verts {
                assert!(v.pos.iter().all(|x| x.is_finite()), "NaN/inf at t={t}: {:?}", v.pos);
            }
        }
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

    #[test]
    fn folding_to_robot_reports_the_direction_a_sound_cue_needs() {
        // fresh Xform: a first request always folds vehicle -> robot
        let mut x = Xform::new();
        assert!(x.request(1.0));
        assert!(x.folding_to_robot());
        x.update(BLEND_SEC * 2.0);
        assert_eq!(x.mode, Mode::Robot);

        // second request from Robot folds back down
        assert!(x.request(1.0));
        assert!(!x.folding_to_robot());
        x.update(BLEND_SEC * 2.0);
        assert_eq!(x.mode, Mode::Vehicle);

        // reversing mid-fold flips what the direction should report too —
        // this is the exact scenario `camera::Camera::start_cinematic`
        // gets re-entered under, so the sound and the shot must agree
        assert!(x.request(1.0));
        assert!(x.folding_to_robot());
        x.update(BLEND_SEC * 0.3);
        assert!(x.request(1.0)); // reverse
        assert!(!x.folding_to_robot(), "direction did not flip on reversal");
    }
}

