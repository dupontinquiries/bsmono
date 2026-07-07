//! Foam where wave crests fold: the surface Jacobian dropping below `cutoff`
//! (toward/below 0) indicates pinching → foam. Returns [0,1]. Pure.

pub fn foam(jacobian: f32, cutoff: f32) -> f32 {
    let c = cutoff.max(1e-6);
    if jacobian >= c {
        0.0
    } else {
        ((c - jacobian) / c).clamp(0.0, 1.0)
    }
}

// ---- r335-water-visual: shoreline foam by depth + bed slope ----------------
// Crest-fold foam (above) covers breaking waves; this adds the *shoreline* band
// of whitewater where shallow water meets the bed. Pure, bounded [0,1].

/// Shoreline-foam tuning. `band_depth`: foam fades out by this depth; `peak_depth`:
/// depth of strongest foam (the run-up line, just off the waterline); `slope_gain`:
/// how strongly a gentle bed slope widens the foam (flat beaches foam more than
/// cliffs). All clamped to sane ranges on use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShoreFoamParams {
    pub band_depth: f32,
    pub peak_depth: f32,
    pub slope_gain: f32,
}

impl ShoreFoamParams {
    pub fn default_params() -> ShoreFoamParams {
        ShoreFoamParams { band_depth: 1.6, peak_depth: 0.20, slope_gain: 1.0 }
    }
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let denom = e1 - e0;
    if denom.abs() < 1e-9 {
        return if x >= e1 { 1.0 } else { 0.0 };
    }
    let t = ((x - e0) / denom).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Shoreline foam at a surface point with water `depth` (surface − bed) and bed
/// `slope` (|∇bed|, ~0 flat .. large cliff). Returns `[0,1]`:
/// - `0` on dry land (`depth <= 0`) and in open water (`depth >= band_depth`);
/// - rises to a peak around `peak_depth` (the run-up line) inside the band;
/// - a gentler bed slope widens/strengthens the band (foam pools on flat shores).
pub fn shore_foam(depth: f32, slope: f32, params: ShoreFoamParams) -> f32 {
    let band = params.band_depth.max(1e-3);
    let peak = params.peak_depth.clamp(0.0, band);
    if depth <= 0.0 || depth >= band {
        return 0.0;
    }
    // triangular band: 0 at waterline, 1 at `peak`, back to 0 at `band`.
    let rise = if peak <= 0.0 { 1.0 } else { smoothstep(0.0, peak, depth) };
    let fall = 1.0 - smoothstep(peak, band, depth);
    let band_w = (rise * fall).clamp(0.0, 1.0);
    // gentle slopes foam more: weight = 1/(1 + gain·slope).
    let g = params.slope_gain.max(0.0);
    let slope_w = 1.0 / (1.0 + g * slope.max(0.0));
    (band_w * slope_w).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn flat_no_foam() { assert_eq!(foam(1.0, 0.3), 0.0); }
    #[test] fn folded_full_foam() { assert!((foam(0.0, 0.3) - 1.0).abs() < 1e-6); }
    #[test] fn negative_clamped() { assert_eq!(foam(-5.0, 0.3), 1.0); }
    #[test] fn partial() { let v = foam(0.15, 0.3); assert!(v > 0.0 && v < 1.0); }
    #[test] fn monotone() {
        let (mut prev, mut j) = (f32::MIN, 0.3f32);
        while j > -0.5 { let v = foam(j, 0.3); assert!(v >= prev - 1e-6); prev = v; j -= 0.02; }
    }
    #[test] fn deterministic() { assert_eq!(foam(0.15, 0.3).to_bits(), foam(0.15, 0.3).to_bits()); }

    // ---- r335-water-visual: shoreline foam tests ----
    #[test]
    fn shore_foam_zero_on_land_and_open_water() {
        let p = ShoreFoamParams::default_params();
        assert_eq!(shore_foam(-0.5, 0.1, p), 0.0, "dry land (depth<=0) ⇒ no foam");
        assert_eq!(shore_foam(0.0, 0.1, p), 0.0, "exactly at waterline ⇒ no foam");
        assert_eq!(shore_foam(p.band_depth, 0.1, p), 0.0, "at band edge ⇒ no foam");
        assert_eq!(shore_foam(p.band_depth + 5.0, 0.1, p), 0.0, "open water ⇒ no foam");
    }

    #[test]
    fn shore_foam_triggers_only_in_the_band() {
        let p = ShoreFoamParams::default_params();
        // sample across depth: nonzero strictly inside (0, band), zero outside.
        let mut d = -1.0f32;
        while d < p.band_depth + 1.0 {
            let v = shore_foam(d, 0.2, p);
            assert!((0.0..=1.0).contains(&v) && v.is_finite());
            if d <= 0.0 || d >= p.band_depth {
                assert_eq!(v, 0.0, "foam outside the shoreline band at depth {d}");
            }
            d += 0.05;
        }
        // and it actually fires somewhere in the band.
        assert!(shore_foam(p.peak_depth, 0.2, p) > 0.0, "foam should fire near the run-up line");
    }

    #[test]
    fn shore_foam_peaks_near_run_up_line() {
        let p = ShoreFoamParams::default_params();
        let at_peak = shore_foam(p.peak_depth, 0.2, p);
        let near_edge = shore_foam(p.band_depth * 0.95, 0.2, p);
        assert!(at_peak > near_edge, "foam strongest near the run-up line, fading to the band edge");
    }

    #[test]
    fn shore_foam_gentle_slope_foams_more() {
        let p = ShoreFoamParams::default_params();
        let flat = shore_foam(p.peak_depth, 0.05, p);
        let steep = shore_foam(p.peak_depth, 5.0, p);
        assert!(flat > steep, "a gentle bed slope should foam more than a cliff");
    }

    #[test]
    fn shore_foam_deterministic() {
        let p = ShoreFoamParams::default_params();
        assert_eq!(shore_foam(0.3, 0.4, p).to_bits(), shore_foam(0.3, 0.4, p).to_bits());
    }
}
