//! v0.1.717: canvas physics for the graph/card views — pure math the QML calls
//! ONCE per gesture (not per frame, to keep the UI-thread/IPC cheap). Covers:
//!   - detent_snap: magnetic snap of a tilt/yaw angle to the nearest detent
//!     (north and the cardinals) when released within a threshold,
//!   - fit_scale: fit-to-view scale with looser padding + a gentle zoom-in cap,
//!   - flick_target / flick_duration: project where a released flick coasts to
//!     under a friction model, so QML animates to it with easing,
//!   - overscroll_clamp: pan bounds with a rubber-band margin (less cramped).

/// Snap `angle_deg` to the nearest detent in `detents` if within `threshold`
/// degrees; otherwise return it unchanged. North (0) is always a detent.
/// Angles are normalized to [0,360). Returns (snapped_deg, did_snap).
pub fn detent_snap(angle_deg: f64, detents: &[f64], threshold: f64) -> (f64, bool) {
    let a = norm360(angle_deg);
    let mut best = a;
    let mut best_d = f64::INFINITY;
    // always consider north
    for &d in std::iter::once(&0.0).chain(detents.iter()) {
        let dn = norm360(d);
        let dist = ang_dist(a, dn);
        if dist < best_d { best_d = dist; best = dn; }
    }
    if best_d <= threshold { (best, true) } else { (a, false) }
}

/// The four cardinal detents (N/E/S/W) — the default set for a rotatable view.
pub const CARDINALS: [f64; 4] = [0.0, 90.0, 180.0, 270.0];

/// Fit-to-view scale. `pad` is the fraction of the viewport to fill (0.96 =
/// 4% breathing room — looser than the old 0.92). `max_zoom` lets a small graph
/// zoom IN a little (e.g. 1.5) instead of being capped at 1.0 and stranded tiny
/// in a big viewport. `min_zoom` floors huge graphs.
pub fn fit_scale(gw: f64, gh: f64, vw: f64, vh: f64,
    pad: f64, min_zoom: f64, max_zoom: f64) -> f64 {
    if gw <= 0.0 || gh <= 0.0 || vw <= 0.0 || vh <= 0.0 { return 1.0; }
    let fit = (vw / gw).min(vh / gh) * pad;
    fit.clamp(min_zoom, max_zoom)
}

/// Project where a flick released with `velocity` (px/s) coasts to, starting
/// at `pos`, under exponential friction. `decel` is px/s² (lower = more glide).
/// Returns the projected end position. v0.1.717: distance = v²/(2·decel),
/// signed by velocity direction — the classic kinematic stopping distance.
pub fn flick_target(pos: f64, velocity: f64, decel: f64) -> f64 {
    if decel <= 0.0 { return pos; }
    let dist = (velocity * velocity) / (2.0 * decel);
    pos + dist * velocity.signum()
}

/// How long (ms) that flick takes to coast to rest: t = |v| / decel.
pub fn flick_duration(velocity: f64, decel: f64) -> f64 {
    if decel <= 0.0 { return 0.0; }
    (velocity.abs() / decel) * 1000.0
}

/// Clamp a pan position to [min, max] but allow `overscroll` px beyond each
/// edge (the rubber-band zone) so the view never feels hard-walled. Returns the
/// clamped position. The QML rebounds from the overscroll zone via a Behavior.
pub fn overscroll_clamp(pos: f64, min: f64, max: f64, overscroll: f64) -> f64 {
    let (lo, hi) = (min - overscroll, max + overscroll);
    pos.clamp(lo.min(hi), lo.max(hi))
}

// ── helpers ──

fn norm360(a: f64) -> f64 {
    let mut x = a % 360.0;
    if x < 0.0 { x += 360.0; }
    x
}

/// Smallest absolute angular distance between two normalized angles (0..180).
fn ang_dist(a: f64, b: f64) -> f64 {
    let d = (a - b).abs() % 360.0;
    if d > 180.0 { 360.0 - d } else { d }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool { (a - b).abs() < 1e-6 }

    #[test]
    fn snaps_to_north_within_threshold() {
        // 7° from north, threshold 10 → snaps to 0
        let (s, snapped) = detent_snap(7.0, &CARDINALS, 10.0);
        assert!(snapped && approx(s, 0.0));
        // 353° (i.e. -7°) also snaps to north across the wrap
        let (s2, snapped2) = detent_snap(353.0, &CARDINALS, 10.0);
        assert!(snapped2 && approx(s2, 0.0));
    }

    #[test]
    fn no_snap_when_far() {
        let (s, snapped) = detent_snap(45.0, &CARDINALS, 10.0);
        assert!(!snapped && approx(s, 45.0));
    }

    #[test]
    fn snaps_to_nearest_cardinal() {
        let (s, ok) = detent_snap(86.0, &CARDINALS, 10.0);
        assert!(ok && approx(s, 90.0));
        let (s2, ok2) = detent_snap(274.0, &CARDINALS, 10.0);
        assert!(ok2 && approx(s2, 270.0));
    }

    #[test]
    fn north_always_a_detent_even_with_empty_set() {
        let (s, ok) = detent_snap(3.0, &[], 5.0);
        assert!(ok && approx(s, 0.0));
    }

    #[test]
    fn fit_scale_loosens_and_allows_zoom_in() {
        // small graph (200×200) in a big viewport (1000×800): old code capped
        // at 1.0; now max_zoom 1.5 lets it fill more.
        let s = fit_scale(200.0, 200.0, 1000.0, 800.0, 0.96, 0.15, 1.5);
        assert!(s > 1.0 && s <= 1.5);
        // big graph shrinks to fit with padding, floored
        let big = fit_scale(5000.0, 4000.0, 1000.0, 800.0, 0.96, 0.15, 1.5);
        assert!(big < 1.0 && big >= 0.15);
        // degenerate inputs → 1.0
        assert!(approx(fit_scale(0.0, 100.0, 100.0, 100.0, 0.96, 0.15, 1.5), 1.0));
    }

    #[test]
    fn flick_projection_kinematics() {
        // v=1000 px/s, decel=1800 px/s² → distance = 1e6/3600 ≈ 277.8 px
        let end = flick_target(100.0, 1000.0, 1800.0);
        assert!(approx(end, 100.0 + 277.777_777_8));
        // negative velocity coasts the other way
        let endneg = flick_target(100.0, -1000.0, 1800.0);
        assert!(endneg < 100.0);
        // duration t = v/decel = 1000/1800 s ≈ 555.6 ms
        assert!((flick_duration(1000.0, 1800.0) - 555.555_555).abs() < 0.01);
    }

    #[test]
    fn overscroll_allows_margin_then_clamps() {
        // within bounds → unchanged
        assert!(approx(overscroll_clamp(50.0, 0.0, 100.0, 40.0), 50.0));
        // past max but within overscroll → allowed
        assert!(approx(overscroll_clamp(130.0, 0.0, 100.0, 40.0), 130.0));
        // beyond overscroll → clamped to the rubber-band edge
        assert!(approx(overscroll_clamp(200.0, 0.0, 100.0, 40.0), 140.0));
        assert!(approx(overscroll_clamp(-90.0, 0.0, 100.0, 40.0), -40.0));
    }
}
