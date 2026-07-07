//! `sim::dragon_wing_flex` — soft wing-membrane motion, built on
//! `sim::verlet_strand`. The forge rig (`dragon_forge::skin`) poses the wings
//! rigidly; this adds the whip/lag a real membrane has: a Verlet strand runs along
//! each wing span, its root pinned to the rigid wing root each frame, the rest of
//! the chain trailing under inertia + a little droop. The per-span deviation from
//! the rigid line is returned as an offset the renderer adds to the wing boxes, so
//! the trailing edge lags a fast down-beat and settles when the wing holds still.
//!
//! Stateful (Verlet keeps previous positions across frames), so it lives on the
//! creature, not inside the stateless `skin`. Pure: no render/IO/clock/RNG. The
//! wing root motion is supplied by the caller (from the rigid skin), so this never
//! reaches into the renderer.

use crate::sim::verlet_strand::{step as strand_step, Strand};

/// Tunables for the membrane feel.
#[derive(Clone, Copy, Debug)]
pub struct FlexParams {
    /// Constraint relaxation passes (stiffer membrane with more).
    pub iters: u8,
    /// Velocity damping per step, 0..1 (higher = settles faster).
    pub damping01: f32,
    /// Downward droop applied to the free points (membrane sag), world units/s².
    pub droop: f32,
    /// Pull of the free points back toward the rigid wing line each step, 0..1
    /// (higher = stiffer membrane, less lag). Keeps the wing near-rigid (it has
    /// bones) with only a soft trailing flex.
    pub follow01: f32,
    /// Clamp on the returned offset magnitude (keeps the membrane from blowing up).
    pub max_offset: f32,
}

impl Default for FlexParams {
    fn default() -> Self {
        FlexParams { iters: 6, damping01: 0.15, droop: 3.0, follow01: 0.35, max_offset: 0.6 }
    }
}

/// Per-wing membrane state. One strand per wing, span = root → tip.
#[derive(Clone, Debug)]
pub struct WingFlex {
    left: Strand,
    right: Strand,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn mag(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

impl WingFlex {
    /// Build two strands of `n` points (n≥2), each running `span` units laterally
    /// from a wing root out to the tip (left toward −X, right toward +X), starting
    /// at the origin (the caller re-pins the root every `step`).
    pub fn new(span: f32, n: usize) -> WingFlex {
        let n = n.max(2);
        let span = if span.is_finite() { span.abs() } else { 1.0 };
        let left = Strand::line([0.0, 0.0, 0.0], [-span, 0.0, 0.0], n);
        let right = Strand::line([0.0, 0.0, 0.0], [span, 0.0, 0.0], n);
        WingFlex { left, right }
    }

    /// Number of span samples per wing (points along root→tip).
    pub fn samples(&self) -> usize {
        self.left.points.len()
    }

    /// Advance both wings one tick. Each wing supplies its current *rigid* root and
    /// tip positions (from the skinned model, same space) as `(root, tip)`. Both
    /// ends are pinned to the rigid wing (leading bone + tip bone); the mid-span
    /// membrane lags under inertia + droop and is pulled back toward the rigid line
    /// by `follow01`. Returns, per wing, the offset for each span sample =
    /// (strand point) − (rigid line point). The two endpoints are ≈0; the lag peaks
    /// mid-span — the soft flutter the renderer adds to the wing boxes.
    pub fn step(
        &mut self,
        l: ([f32; 3], [f32; 3]),
        r: ([f32; 3], [f32; 3]),
        dt: f32,
        p: &FlexParams,
    ) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let g = [0.0, -p.droop, 0.0];
        let off_l = Self::drive(&mut self.left, l.0, l.1, g, dt, p);
        let off_r = Self::drive(&mut self.right, r.0, r.1, g, dt, p);
        (off_l, off_r)
    }

    fn drive(
        s: &mut Strand,
        root: [f32; 3],
        tip: [f32; 3],
        g: [f32; 3],
        dt: f32,
        p: &FlexParams,
    ) -> Vec<[f32; 3]> {
        let n = s.points.len();
        if n == 0 {
            return Vec::new();
        }
        let denom = (n - 1).max(1) as f32;
        let target = |i: usize| {
            let t = i as f32 / denom;
            [root[0] + (tip[0] - root[0]) * t, root[1] + (tip[1] - root[1]) * t, root[2] + (tip[2] - root[2]) * t]
        };
        // pin both ends to the rigid wing bone (leading root + wing tip)
        s.pinned[0] = true;
        s.points[0] = root;
        s.prev[0] = root;
        if n >= 2 {
            s.pinned[n - 1] = true;
            s.points[n - 1] = tip;
            s.prev[n - 1] = tip;
        }
        if dt > 0.0 {
            strand_step(s, g, p.damping01, dt, p.iters);
        }
        // follow: pull free points back toward the rigid line (membrane has bones)
        let f = if p.follow01.is_finite() { p.follow01.max(0.0).min(1.0) } else { 0.0 };
        let mut i = 1;
        while i + 1 < n {
            let tg = target(i);
            let mut k = 0;
            while k < 3 {
                s.points[i][k] += (tg[k] - s.points[i][k]) * f;
                k += 1;
            }
            i += 1;
        }
        // offset = strand point − rigid line point, clamped + sanitised
        let mut out = Vec::with_capacity(n);
        let mut i = 0;
        while i < n {
            let mut d = sub(s.points[i], target(i));
            let m = mag(d);
            if m > p.max_offset && m > 1e-6 {
                d = scale(d, p.max_offset / m);
            }
            if !(d[0].is_finite() && d[1].is_finite() && d[2].is_finite()) {
                d = [0.0, 0.0, 0.0];
            }
            out.push(d);
            i += 1;
        }
        out
    }
}

/// Map a wing box's span fraction (0 at root … 1 at tip) to a flex offset by
/// sampling the per-span offsets. `offsets` is one wing's output from [`WingFlex::step`].
pub fn offset_at(offsets: &[[f32; 3]], span_frac01: f32) -> [f32; 3] {
    if offsets.is_empty() {
        return [0.0, 0.0, 0.0];
    }
    let f = if span_frac01.is_finite() { span_frac01.max(0.0).min(1.0) } else { 0.0 };
    let last = offsets.len() - 1;
    let x = f * last as f32;
    let i = x.floor() as usize;
    if i >= last {
        return offsets[last];
    }
    let t = x - i as f32;
    let a = offsets[i];
    let b = offsets[i + 1];
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn max_off(v: &[[f32; 3]]) -> f32 {
        v.iter().fold(0.0f32, |m, d| m.max(mag(*d)))
    }

    #[test]
    fn pinned_ends_have_no_offset() {
        let mut wf = WingFlex::new(2.0, 6);
        let p = FlexParams::default();
        let (l, _r) = wf.step(([1.0, 3.0, 0.0], [-1.0, 3.0, 0.0]), ([1.0, 3.0, 0.0], [3.0, 3.0, 0.0]), 0.016, &p);
        assert!(mag(l[0]) < 1e-3 && mag(l[l.len() - 1]) < 1e-3, "root and tip ride the rigid wing bone");
    }

    #[test]
    fn tip_lags_a_sudden_beat_then_settles() {
        let mut wf = WingFlex::new(2.0, 7);
        let p = FlexParams::default();
        // whip the whole wing upward fast for a few frames (root + tip rise together)
        let mut y = 0.0f32;
        let mut peak = 0.0f32;
        for _ in 0..6 {
            y += 0.25;
            let (l, _r) = wf.step(([0.0, y, 0.0], [-2.0, y, 0.0]), ([0.0, y, 0.0], [2.0, y, 0.0]), 0.016, &p);
            peak = peak.max(max_off(&l));
        }
        assert!(peak > 0.02, "the membrane lags the driven beat");
        // hold still — the active-beat lag should clearly exceed the resting sag
        for _ in 0..240 {
            wf.step(([0.0, y, 0.0], [-2.0, y, 0.0]), ([0.0, y, 0.0], [2.0, y, 0.0]), 0.016, &p);
        }
        let (l, _r) = wf.step(([0.0, y, 0.0], [-2.0, y, 0.0]), ([0.0, y, 0.0], [2.0, y, 0.0]), 0.016, &p);
        let resting = max_off(&l);
        assert!(peak > resting * 1.3, "a beat lags more than the resting sag (peak {peak} vs rest {resting})");
    }

    #[test]
    fn offsets_are_bounded_and_finite() {
        let mut wf = WingFlex::new(3.0, 8);
        let p = FlexParams { max_offset: 0.5, ..FlexParams::default() };
        for k in 0..200 {
            let y = (k as f32 * 0.7).sin() * 5.0;
            let x = (k as f32).cos() * 5.0;
            let (l, r) = wf.step(([x, y, 0.0], [x - 3.0, y, 0.0]), ([x, y, 0.0], [x + 3.0, y, 0.0]), 0.016, &p);
            for d in l.iter().chain(r.iter()) {
                assert!(d[0].is_finite() && d[1].is_finite() && d[2].is_finite());
                assert!(mag(*d) <= p.max_offset + 1e-3, "offset clamped to max_offset");
            }
        }
    }

    #[test]
    fn dt_zero_is_a_noop_on_the_chain() {
        let mut wf = WingFlex::new(2.0, 5);
        let p = FlexParams::default();
        let (l, _r) = wf.step(([0.0, 1.0, 0.0], [-2.0, 1.0, 0.0]), ([0.0, 1.0, 0.0], [2.0, 1.0, 0.0]), 0.0, &p);
        assert!(l.iter().all(|d| mag(*d) < 2.0), "dt=0 doesn't explode");
    }

    #[test]
    fn offset_at_interpolates_and_clamps() {
        let offs = vec![[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 2.0, 0.0]];
        assert_eq!(offset_at(&offs, 0.0)[1], 0.0);
        assert_eq!(offset_at(&offs, 1.0)[1], 2.0);
        assert!((offset_at(&offs, 0.5)[1] - 1.0).abs() < 1e-5);
        assert_eq!(offset_at(&offs, 2.0)[1], 2.0, "out-of-range clamps to the tip");
        assert_eq!(offset_at(&[], 0.5), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn deterministic() {
        let run = || {
            let mut wf = WingFlex::new(2.0, 6);
            let p = FlexParams::default();
            let mut acc = 0.0f32;
            let mut root = [0.0, 0.0, 0.0];
            for k in 0..50 {
                root[1] = (k as f32 * 0.3).sin();
                let (l, r) = wf.step((root, [root[0] - 2.0, root[1], 0.0]), (root, [root[0] + 2.0, root[1], 0.0]), 0.016, &p);
                for d in l.iter().chain(r.iter()) {
                    acc += mag(*d);
                }
            }
            acc
        };
        assert_eq!(run(), run());
    }
}

// ── S1 (r94): register as a sim::service_registry service (co-located) ───────
// Adapter ticks the membrane with a NEUTRAL rigid wing line (root at origin,
// tips ±span on X) and default FlexParams; real per-frame wing pose is a later
// wiring pass.
struct WingFlexService(WingFlex);
impl crate::sim::service_registry::Service for WingFlexService {
    fn name(&self) -> &'static str { "dragon_wing_flex" }
    fn tick(&mut self, ctx: &crate::sim::service_registry::ServiceCtx) {
        let p = FlexParams::default();
        let l = ([0.0, 0.0, 0.0], [2.0, 0.0, 0.0]);
        let r = ([0.0, 0.0, 0.0], [-2.0, 0.0, 0.0]);
        let _ = self.0.step(l, r, ctx.dt, &p);
    }
}
#[linkme::distributed_slice(crate::sim::service_registry::SERVICES)]
static WING_FLEX_SERVICE: crate::sim::service_registry::ServiceEntry =
    crate::sim::service_registry::ServiceEntry {
        name: "dragon_wing_flex",
        make: || Box::new(WingFlexService(WingFlex::new(2.0, 8))),
    };

#[cfg(test)]
mod s1_wiring_tests {
    use crate::sim::service_registry::{Service, ServiceCtx};
    #[test]
    fn wing_flex_service_ticks_panic_free() {
        let mut s = super::WingFlexService(super::WingFlex::new(2.0, 8));
        let ctx = ServiceCtx { dt: 1.0 / 60.0 };
        for _ in 0..240 { s.tick(&ctx); }
        assert_eq!(s.name(), "dragon_wing_flex");
    }
}
