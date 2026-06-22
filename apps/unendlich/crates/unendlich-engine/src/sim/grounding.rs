//! grounding — stop planted feet from skating, and let weight settle onto the
//! parts below. Two cheap, pure pieces for the "moonwalk slide" and the "no
//! weight / float" feel, neither needing a physics solver.
//!
//! **foot_lock**: a foot in its stance phase is pinned to the ground point where
//! it landed; the body moves on but the foot stays put (no skating) until it
//! lifts into swing and re-plants ahead. Driven by `gait_phase`, which already
//! tracks distance travelled.
//!
//! **settle**: a spring-damped vertical compression. Each part sags onto the part
//! below by the weight it bears (a static `weight_sag`), and dips on impact then
//! recovers smoothly (`settle_step`). Chaining the borne mass top-down makes the
//! body's weight visibly press down through the legs when grounded.

/// Is a foot bearing weight (stance) at this `gait_phase` in `[0,1)`? A simple
/// two-beat: `phase_offset` staggers the feet (opposite feet ~0.5 apart). The
/// first half of a foot's own cycle is planted, the second half is the swing.
pub fn in_stance(gait_phase: f32, phase_offset: f32) -> bool {
    (gait_phase + phase_offset).rem_euclid(1.0) < 0.5
}

/// A single foot's ground lock. While planted it returns the fixed world point it
/// first touched (no sliding) no matter how the body moves; while swinging it
/// tracks the body's rest position and re-plants there on the next stance.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Foot {
    pub planted: Option<[f32; 2]>,
}

impl Foot {
    pub fn new() -> Foot {
        Foot { planted: None }
    }

    /// Resolve the foot's world XZ this step. `rest_world` is where the foot would
    /// sit if it just followed the body (body pos + the foot's body-space offset,
    /// rotated). On entering stance it plants there and then holds; on lift it
    /// releases and tracks the body again.
    pub fn resolve(&mut self, stance: bool, rest_world: [f32; 2]) -> [f32; 2] {
        if stance {
            let p = self.planted.unwrap_or(rest_world);
            self.planted = Some(p);
            p
        } else {
            self.planted = None;
            rest_world
        }
    }

    /// How far the planted foot is from where the body wants it — the skating that
    /// foot-lock removes (and that the swing must cover when it lifts). 0 while
    /// swinging.
    pub fn slip(&self, rest_world: [f32; 2]) -> f32 {
        match self.planted {
            Some(p) => {
                let (dx, dz) = (rest_world[0] - p[0], rest_world[1] - p[1]);
                (dx * dx + dz * dz).sqrt()
            }
            None => 0.0,
        }
    }
}

pub const SETTLE_STIFFNESS: f32 = 120.0;
pub const SETTLE_DAMPING: f32 = 18.0;
const GRAVITY_G: f32 = 9.8;

/// Static downward compression a borne load puts on the joint below it: heavier
/// load → more sag. Returned negative (down). `stiffness` is the joint spring.
pub fn weight_sag(borne_mass: f32, stiffness: f32) -> f32 {
    if stiffness <= 0.0 {
        return 0.0;
    }
    -(borne_mass.max(0.0) * GRAVITY_G / stiffness)
}

/// Advance a spring-damped vertical `offset` (and its velocity `vel`) toward
/// `rest` over `dt`. Returns `(offset, vel)`. On a landing, add a negative
/// velocity kick to `vel` before calling to get the impact dip. Tuned near
/// critical damping so it settles without ringing.
pub fn settle_step(offset: f32, vel: f32, rest: f32, dt: f32, stiffness: f32, damping: f32) -> (f32, f32) {
    let accel = -stiffness * (offset - rest) - damping * vel;
    let v = vel + accel * dt;
    let o = offset + v * dt;
    (o, v)
}

/// The resting compression at each joint of a top-down mass stack: joint `i`
/// bears its own mass plus everything above it, so lower joints sag more. Input
/// `masses` ordered top (head) → bottom (foot); output is the per-joint sag
/// (negative), same order.
pub fn stack_sag(masses: &[f32], stiffness: f32) -> Vec<f32> {
    let mut borne = 0.0;
    // accumulate from the top so each joint carries everything above it
    let mut cum = Vec::with_capacity(masses.len());
    for &m in masses {
        borne += m.max(0.0);
        cum.push(borne);
    }
    cum.into_iter().map(|b| weight_sag(b, stiffness)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opposite_feet_are_antiphase() {
        // A foot and its diagonal partner (offset 0.5) are never both planted.
        for k in 0..20 {
            let p = k as f32 / 20.0;
            assert_ne!(in_stance(p, 0.0), in_stance(p, 0.5), "phase {p}");
        }
    }

    #[test]
    fn a_planted_foot_does_not_slide_while_the_body_moves() {
        let mut foot = Foot::new();
        // Enters stance, plants at (0,0).
        let p0 = foot.resolve(true, [0.0, 0.0]);
        assert_eq!(p0, [0.0, 0.0]);
        // Body walks forward; the planted foot stays put (no skating).
        let p1 = foot.resolve(true, [0.4, 0.0]);
        assert_eq!(p1, [0.0, 0.0], "planted foot must not slide");
        assert!((foot.slip([0.4, 0.0]) - 0.4).abs() < 1e-5, "slip should report the lag");
    }

    #[test]
    fn a_lifted_foot_replants_ahead() {
        let mut foot = Foot::new();
        foot.resolve(true, [0.0, 0.0]); // planted
        let swing = foot.resolve(false, [0.6, 0.0]); // lifts, tracks body
        assert_eq!(swing, [0.6, 0.0]);
        assert_eq!(foot.planted, None);
        let replanted = foot.resolve(true, [0.8, 0.0]); // next stance plants ahead
        assert_eq!(replanted, [0.8, 0.0]);
    }

    #[test]
    fn settle_converges_to_rest_without_ringing() {
        let (mut o, mut v) = (0.0_f32, 0.0_f32);
        let rest = -0.1;
        let mut min_o = f32::INFINITY;
        for _ in 0..400 {
            let (no, nv) = settle_step(o, v, rest, 1.0 / 240.0, SETTLE_STIFFNESS, SETTLE_DAMPING);
            o = no;
            v = nv;
            min_o = min_o.min(o);
        }
        assert!((o - rest).abs() < 1e-3, "should settle to rest, got {o}");
        assert!(v.abs() < 1e-2, "should come to rest, vel {v}");
        // Near-critical damping: it shouldn't undershoot far past the target.
        assert!(min_o > rest - 0.05, "overshoot too large: {min_o}");
    }

    #[test]
    fn impact_dips_then_recovers() {
        // A landing kick: downward velocity, rest at 0.
        let (mut o, mut v) = (0.0_f32, -2.0_f32);
        let mut lowest = 0.0_f32;
        for _ in 0..400 {
            let (no, nv) = settle_step(o, v, 0.0, 1.0 / 240.0, SETTLE_STIFFNESS, SETTLE_DAMPING);
            o = no;
            v = nv;
            lowest = lowest.min(o);
        }
        assert!(lowest < -0.01, "should dip on impact, lowest {lowest}");
        assert!(o.abs() < 1e-2, "should recover to rest, got {o}");
    }

    #[test]
    fn heavier_load_sags_more() {
        let light = weight_sag(1.0, SETTLE_STIFFNESS);
        let heavy = weight_sag(10.0, SETTLE_STIFFNESS);
        assert!(heavy < light, "heavier should compress more (more negative)");
        assert!(light < 0.0, "any load sags down");
    }

    #[test]
    fn lower_joints_in_a_stack_bear_more_and_sag_more() {
        // head, torso, leg, foot — each carries everything above it.
        let sag = stack_sag(&[2.0, 8.0, 3.0, 1.0], SETTLE_STIFFNESS);
        assert_eq!(sag.len(), 4);
        for w in sag.windows(2) {
            assert!(w[1] <= w[0], "lower joint should sag at least as much: {:?}", w);
        }
        assert!(sag[3] < sag[0], "the foot bears the whole body, the head bears only itself");
    }
}
