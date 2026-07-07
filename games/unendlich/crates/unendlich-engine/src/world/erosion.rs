//! Erosion — water flow and exposure wear terrain down over time.
//!
//! This is the water roadmap's step 4 (`CLIMATE_PATH.md`) and the Earth↔Water
//! coupling from `ELEMENTS.md`: high-flow, exposed, soft cells weather faster,
//! and a cell that reaches full weathering is *spent* — washed away. The
//! caller removes spent cells, and that removal feeds [`crate::world::collapse`]
//! (an eroded undercut drops what it held).
//!
//! Pure and parameter-driven, like `elemental`: it advances a weathering
//! scalar from local `flow` / `slope` / `exposure` / `hardness`, with no
//! dependency on `weathering.rs` or the block registry. `flow` can come from
//! [`flow_proxy`] over `water_field` depths; `slope`, `sky_exposure`, and
//! `weathering` line up with the fields already on
//! [`crate::world::material_response::EnvironmentSample`].

/// Base weathering increment per unit flow per tick, before modifiers.
pub const EROSION_BASE: f32 = 0.02;

/// Floor on the hardness divisor, so very soft material can't produce a
/// runaway rate (and never divides by zero).
pub const MIN_HARDNESS: f32 = 0.1;

/// Weathering increment per tick at a cell. Rises with water `flow`, terrain
/// `slope`, and sky `exposure`; falls with material `hardness`. `flow`,
/// `slope`, `exposure` are roughly `[0, 1]`; `hardness` is `> 0` (clamped to
/// [`MIN_HARDNESS`]). Always `>= 0`.
pub fn erosion_rate(flow: f32, slope: f32, exposure: f32, hardness: f32) -> f32 {
    let h = hardness.max(MIN_HARDNESS);
    let r = EROSION_BASE
        * flow.max(0.0)
        * (1.0 + slope.max(0.0))
        * (0.25 + 0.75 * exposure.clamp(0.0, 1.0))
        / h;
    r.max(0.0)
}

/// Advance a weathering scalar by `rate` over `dt` ticks, capped at `1.0`
/// (fully weathered / spent).
pub fn advance(weathering: f32, rate: f32, dt: f32) -> f32 {
    (weathering + rate * dt).min(1.0)
}

/// Whether a cell has fully eroded (weathering reached `1.0`) and should be
/// removed by the caller — which then feeds collapse.
pub fn is_spent(weathering: f32) -> bool {
    weathering >= 1.0
}

/// Flow proxy at a cell from water depths: water runs downhill, so flow is the
/// sum of positive depth drops to lower neighbors. Zero on flat or filled
/// ground, large on a steep wet slope. `here` and `neighbors` are water
/// depths (e.g. from `water_field`).
pub fn flow_proxy(here: f32, neighbors: &[f32]) -> f32 {
    neighbors.iter().map(|&n| (here - n).max(0.0)).sum()
}

/// Advance a whole weathering grid one step and return the indices that became
/// **spent** (crossed to `1.0`) this step — the cells the caller should remove
/// and then feed to collapse. `flow`, `slope`, `exposure` are per-cell and
/// must be the same length as `weathering`.
pub fn erode_step(
    weathering: &mut [f32],
    flow: &[f32],
    slope: &[f32],
    exposure: &[f32],
    hardness: f32,
    dt: f32,
) -> Vec<usize> {
    let mut spent = Vec::new();
    for i in 0..weathering.len() {
        if weathering[i] >= 1.0 {
            continue;
        }
        let rate = erosion_rate(flow[i], slope[i], exposure[i], hardness);
        let w = advance(weathering[i], rate, dt);
        weathering[i] = w;
        if w >= 1.0 {
            spent.push(i);
        }
    }
    spent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_monotone_in_each_input() {
        assert_eq!(erosion_rate(0.0, 0.5, 1.0, 1.0), 0.0, "no flow, no erosion");
        assert!(erosion_rate(1.0, 0.5, 1.0, 1.0) > erosion_rate(0.5, 0.5, 1.0, 1.0));
        assert!(erosion_rate(1.0, 0.0, 1.0, 1.0) > erosion_rate(1.0, 0.0, 1.0, 4.0)); // harder erodes slower
        assert!(erosion_rate(1.0, 1.0, 1.0, 1.0) > erosion_rate(1.0, 0.0, 1.0, 1.0)); // steeper faster
        assert!(erosion_rate(1.0, 0.0, 1.0, 1.0) > erosion_rate(1.0, 0.0, 0.0, 1.0)); // exposed faster
        // Locked values from the sim.
        assert!((erosion_rate(1.0, 0.0, 1.0, 1.0) - 0.02).abs() < 1e-6);
        assert!((erosion_rate(1.0, 0.0, 1.0, 4.0) - 0.005).abs() < 1e-6);
        assert!((erosion_rate(1.0, 1.0, 1.0, 1.0) - 0.04).abs() < 1e-6);
        assert!((erosion_rate(1.0, 0.0, 0.0, 1.0) - 0.005).abs() < 1e-6);
    }

    #[test]
    fn advance_caps_and_accumulates() {
        assert_eq!(advance(0.95, 0.5, 1.0), 1.0);
        assert!((advance(0.2, 0.1, 1.0) - 0.3).abs() < 1e-6);
        assert!(is_spent(advance(0.95, 0.5, 1.0)));
        assert!(!is_spent(advance(0.2, 0.1, 1.0)));
    }

    #[test]
    fn flow_proxy_zero_flat_positive_on_slope() {
        assert_eq!(flow_proxy(1.0, &[1.0, 1.0, 1.0, 1.0]), 0.0);
        assert!((flow_proxy(1.0, &[0.5, 0.8, 1.0, 1.0]) - 0.7).abs() < 1e-6);
    }

    #[test]
    fn erode_step_reports_spent_cells() {
        let mut w = vec![0.99, 0.0, 0.999];
        let flow = vec![1.0, 0.0, 1.0];
        let slope = vec![1.0, 0.0, 0.0];
        let exposure = vec![1.0, 1.0, 1.0];
        let spent = erode_step(&mut w, &flow, &slope, &exposure, 0.1, 1.0);
        // cells 0 and 2 had high flow and were near 1.0 -> spent; cell 1 dry.
        assert!(spent.contains(&0));
        assert!(spent.contains(&2));
        assert!(!spent.contains(&1));
        assert_eq!(w[0], 1.0);
        assert_eq!(w[1], 0.0);
    }
}
