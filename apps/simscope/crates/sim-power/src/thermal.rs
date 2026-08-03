//! thermal.rs — conductor resistance is not a constant.
//!
//! A transmission line's resistance rises as it heats, which raises its losses, which heats it
//! further. Power flow normally ignores this and solves at a nominal 20 °C, which is fine for a
//! lightly loaded network and progressively wrong for a heavily loaded one — a line at 80 °C has
//! about 24% more resistance than the datasheet says.
//!
//! This is the coupling `sim-mat` makes expressible: the temperature coefficient now comes from a
//! material rather than being a constant buried in a power flow crate. Full temperature-dependent
//! power flow — solving conductor temperature and network state together — is the natural
//! extension, and is not implemented; this is the scaling factor it would use.

use sim_mat::Material;

/// Ratio of resistance at `t_celsius` to resistance at 20 °C.
///
/// Returns `None` for a material with no temperature coefficient, rather than assuming zero:
/// silently treating an unknown coefficient as "resistance does not change" is exactly the sort of
/// default this workspace refuses to make.
pub fn resistance_scale(mat: &Material, t_celsius: f64) -> Option<f64> {
    let cold = mat.resistivity_at(20.0)?;
    let hot = mat.resistivity_at(t_celsius)?;
    Some(hot / cold)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hot_conductor_carries_more_resistance() {
        let cu = sim_mat::library::copper();
        assert_eq!(resistance_scale(&cu, 20.0), Some(1.0));
        // 80 C is a routine conductor temperature under load.
        let s = resistance_scale(&cu, 80.0).unwrap();
        assert!((s - 1.236).abs() < 0.01, "copper at 80 C should be ~24% higher, got {s:.3}");
        // Aluminium's coefficient is slightly larger than copper's.
        let al = resistance_scale(&sim_mat::library::aluminium_6061(), 80.0).unwrap();
        assert!(al > s, "aluminium should rise faster than copper: {al:.3} vs {s:.3}");
    }

    #[test]
    fn a_material_without_a_coefficient_reports_nothing() {
        // Not 1.0 — that would be a claim that resistance is temperature-independent.
        assert!(resistance_scale(&sim_mat::library::fr4(), 100.0).is_none());
        assert!(resistance_scale(&sim_mat::library::alumina_99(), 100.0).is_none());
    }

    #[test]
    fn cooling_lowers_resistance_symmetrically() {
        let cu = sim_mat::library::copper();
        let hot = resistance_scale(&cu, 20.0 + 50.0).unwrap();
        let cold = resistance_scale(&cu, 20.0 - 50.0).unwrap();
        assert!((hot - 1.0) + (cold - 1.0) < 1e-12, "the linear model must be symmetric");
        assert!(cold < 1.0);
    }
}
