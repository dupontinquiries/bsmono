//! formulas.rs — the closed forms, kept strictly as *references to check against*.
//!
//! Nothing in the solver path calls these. They exist so the field solve has something
//! independent to be measured against, and they are labelled as approximations because that is
//! what they are: curve fits to earlier numerical work, typically good to 1–2% over stated ranges
//! and quietly wrong outside them.
//!
//! That asymmetry is the point of building L2 at all. A closed form gives a number for the
//! geometry it was fitted to; a field solve gives a number for the geometry you actually have.

use std::f64::consts::PI;

/// Free-space impedance, ohms.
pub const ETA0: f64 = 376.730_313_412;

/// Hammerstad's effective permittivity for an open microstrip: trace of width `w` on a substrate
/// of height `h` and relative permittivity `er`, with air above.
///
/// The field is partly in the substrate and partly in the air, so the mode sees something between
/// 1 and `er`. This inhomogeneity is the entire difference between microstrip and stripline, and
/// it is why microstrip has dispersion and stripline does not.
pub fn microstrip_eps_eff(w: f64, h: f64, er: f64) -> f64 {
    let u = w / h;
    let a = (er + 1.0) / 2.0;
    let b = (er - 1.0) / 2.0;
    if u >= 1.0 {
        a + b * (1.0 + 12.0 / u).powf(-0.5)
    } else {
        a + b * ((1.0 + 12.0 / u).powf(-0.5) + 0.04 * (1.0 - u).powi(2))
    }
}

/// Hammerstad's characteristic impedance for an open microstrip, ohms. Stated accuracy ~1% for
/// 0.05 ≤ w/h ≤ 20 and er ≤ 16.
pub fn microstrip_z0(w: f64, h: f64, er: f64) -> f64 {
    let u = w / h;
    let ee = microstrip_eps_eff(w, h, er);
    if u <= 1.0 {
        (60.0 / ee.sqrt()) * (8.0 / u + u / 4.0).ln()
    } else {
        (120.0 * PI / ee.sqrt()) / (u + 1.393 + 0.667 * (u + 1.444).ln())
    }
}

/// Wheeler's centred stripline impedance, ohms: a trace of width `w` midway between ground planes
/// separated by `b`, fully embedded in dielectric `er`. Valid for w/b ≳ 0.35.
///
/// Unlike microstrip this geometry is *homogeneous*, so its effective permittivity is exactly `er`
/// — which makes it the sharper validation case, because that is exact physics rather than a fit.
pub fn stripline_z0(w: f64, b: f64, er: f64) -> f64 {
    (30.0 * PI / er.sqrt()) * b / (w + 0.441 * b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microstrip_formulas_bracket_the_dielectric() {
        // The effective permittivity of an open microstrip must lie strictly between air and the
        // substrate — a mode that saw only one of them would not be a microstrip.
        for (w, h, er) in [(0.3e-3, 0.2e-3, 4.4), (1.0e-3, 0.2e-3, 4.4), (0.1e-3, 0.5e-3, 2.2)] {
            let ee = microstrip_eps_eff(w, h, er);
            assert!(ee > 1.0 && ee < er, "eps_eff {ee} outside (1, {er})");
        }
    }

    #[test]
    fn a_standard_fifty_ohm_microstrip_comes_out_near_fifty() {
        // FR-4, er 4.4, 0.2 mm substrate: about 0.38 mm of trace is the well-known 50 ohm width.
        let z = microstrip_z0(0.38e-3, 0.2e-3, 4.4);
        assert!((z - 50.0).abs() < 3.0, "expected ~50 ohm, formula gives {z:.1}");
    }

    #[test]
    fn wider_traces_have_lower_impedance() {
        // Monotonicity, which any correct impedance model must have and which catches a sign or
        // reciprocal error immediately.
        let mut last = f64::INFINITY;
        for w in [0.1e-3, 0.2e-3, 0.4e-3, 0.8e-3, 1.6e-3] {
            let z = microstrip_z0(w, 0.2e-3, 4.4);
            assert!(z < last, "Z0 should fall as width rises: {z} after {last}");
            last = z;
        }
    }
}
