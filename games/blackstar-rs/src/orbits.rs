//! LONG-PERIOD ORBITS — return windows, and why a short arc cannot fix one.
//!
//! `voyage.rs` plans transfers between two CIRCULAR orbits: Hohmann, coplanar,
//! both radii known. That covers every trip the ship actually takes. It does
//! not cover the other orbital question this game asks, which is the inverse
//! problem: something was seen for a while, and then it left. When does it
//! come back?
//!
//! For a body on a near-parabolic ellipse that is only observable near
//! perihelion, that question is badly conditioned, and it is badly
//! conditioned for a reason worth stating plainly, because it is the whole
//! content of this module:
//!
//! The orbital energy is carried by `1/a` (the reciprocal semi-major axis).
//! At perihelion, the vis-viva relation gives
//!
//! ```text
//!     v² = mu (2/q - 1/a)      =>      1/a = 2/q - v²/mu
//! ```
//!
//! When the orbit is large, `1/a` is a small difference of two large numbers.
//! `2/q` for a body grazing the inner system is of order 2; `1/a` for a
//! 4,000-year orbit is of order 0.004. So `1/a` is the last three digits of
//! a subtraction — and a velocity measurement good to one part in a thousand
//! leaves the PERIOD uncertain by a factor of several, or unbounded.
//!
//! This is not a contrivance. It is the standard difficulty in determining
//! original orbits for long-period comets from a single apparition, and it
//! is why such orbits are quoted with `1/a` rather than with `P`.
//!
//! ## What is measured, what is assumed
//!
//! Two-body, point-mass, heliocentric, in AU and Julian years with
//! `mu = 4 pi²` (which is exact in these units for a one-solar-mass primary,
//! by the definition of the astronomical unit). Kepler's third law and
//! vis-viva are exact within that model; `period_years` is checked below
//! against Halley's published elements, the same "check the formula against
//! a number somebody else measured" discipline `exoworlds.rs` uses for
//! surface gravity.
//!
//! What is NOT modelled: planetary perturbations, non-gravitational forces,
//! and the galactic tide — the three things that actually change a
//! long-period orbit between apparitions. So `period_band` gives the
//! uncertainty from MEASUREMENT alone. It is a floor on the real
//! uncertainty, never a ceiling, and a civilisation that took the low end of
//! this band as a prediction would be making an error of exactly the kind
//! this module exists to quantify.

/// Solar gravitational parameter in AU³/yr², for a one-solar-mass primary.
/// Exact in these units: `mu = 4 pi²` follows from Kepler's third law with
/// `a = 1 AU`, `P = 1 yr`.
pub const MU: f64 = 4.0 * std::f64::consts::PI * std::f64::consts::PI;

/// Kepler's third law: orbital period in years for a semi-major axis in AU.
pub fn period_years(a_au: f64) -> f64 {
    a_au.powf(1.5)
}

/// The inverse: the semi-major axis implied by a period.
pub fn semi_major_au(period_years: f64) -> f64 {
    period_years.powf(2.0 / 3.0)
}

/// Speed at perihelion, AU/yr, from vis-viva.
pub fn perihelion_speed(q_au: f64, a_au: f64) -> f64 {
    (MU * (2.0 / q_au - 1.0 / a_au)).sqrt()
}

/// The reciprocal semi-major axis implied by a perihelion state.
///
/// This is the quantity an orbit determination actually constrains. It is
/// signed: positive is a closed ellipse, zero is parabolic, negative is
/// hyperbolic — the body is leaving and will not return.
pub fn inv_a(q_au: f64, v_peri: f64) -> f64 {
    2.0 / q_au - v_peri * v_peri / MU
}

/// What a return window looks like once you admit the measurement error.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Window {
    /// Both ends of the error bar are closed orbits.
    Bounded { min_years: f64, max_years: f64 },
    /// The fast end returns; the slow end does not. The observation cannot
    /// distinguish "comes back in `min_years`" from "never comes back".
    Open { min_years: f64 },
}

impl Window {
    /// The earliest return the data allows — the number an optimist quotes.
    pub fn earliest(&self) -> f64 {
        match *self {
            Window::Bounded { min_years, .. } => min_years,
            Window::Open { min_years } => min_years,
        }
    }

    /// Does a candidate period sit inside the band? An open window admits
    /// everything above its floor.
    pub fn admits(&self, period_years: f64) -> bool {
        match *self {
            Window::Bounded { min_years, max_years } => {
                period_years >= min_years && period_years <= max_years
            }
            Window::Open { min_years } => period_years >= min_years,
        }
    }
}

/// The family of periods consistent with a perihelion speed known to a
/// given fractional precision.
///
/// `sigma_v_frac` is the one-sigma RELATIVE uncertainty in the perihelion
/// speed — 0.003 means three parts in a thousand. The band is asymmetric,
/// because `1/a` is linear in `v²` while `P` goes as `a^1.5`: a slow error
/// shortens the period a little, a fast error lengthens it enormously, and
/// past a threshold unbinds the orbit entirely.
pub fn period_band(q_au: f64, v_peri: f64, sigma_v_frac: f64) -> Window {
    let slow = inv_a(q_au, v_peri * (1.0 - sigma_v_frac));
    let fast = inv_a(q_au, v_peri * (1.0 + sigma_v_frac));
    // larger 1/a = smaller orbit = shorter period, so the SLOW end is the floor
    let min_years = period_years(1.0 / slow);
    if fast <= 0.0 {
        Window::Open { min_years }
    } else {
        Window::Bounded { min_years, max_years: period_years(1.0 / fast) }
    }
}

/// Inverting the question: how precisely must the perihelion speed be known
/// before the band's floor rises above `floor_years`?
///
/// This is the honest form of "were they capable of getting it right". It
/// answers in units of instrument precision rather than in units of blame.
/// Bisection on a monotone function; 60 iterations is well past f64
/// resolution on the bracket below.
pub fn precision_for_floor(q_au: f64, v_peri: f64, floor_years: f64) -> f64 {
    let (mut lo, mut hi) = (0.0f64, 0.5f64);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        // tighter sigma raises the floor toward the true period
        if period_band(q_au, v_peri, mid).earliest() < floor_years {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    0.5 * (lo + hi)
}

// ---------------------------------------------------------------- ASTERI ---
//
// The one long-period object in this system's history that anybody wrote
// down. The elements below are the game's own; the arithmetic above is not.

/// Perihelion of the object the Asteri tracked, in AU. Inside Earth's orbit,
/// which is why it was observable at all — and why the return matters.
pub const ASTERI_PERIHELION_AU: f64 = 0.9;

/// The return the Asteri's final astronomical records give.
pub const ASTERI_RETURN_PREDICTED_YEARS: f64 = 417.0;

/// The period the object is actually on.
pub const ASTERI_RETURN_WINDOW_YEARS: f64 = 3812.0;

/// The perihelion speed the true orbit implies, AU/yr.
pub fn asteri_true_speed() -> f64 {
    perihelion_speed(ASTERI_PERIHELION_AU, semi_major_au(ASTERI_RETURN_WINDOW_YEARS))
}

/// The fractional velocity error between the published prediction and the
/// truth — the entire size of the mistake, as a number.
pub fn asteri_speed_error() -> f64 {
    let truth = asteri_true_speed();
    let claimed =
        perihelion_speed(ASTERI_PERIHELION_AU, semi_major_au(ASTERI_RETURN_PREDICTED_YEARS));
    (truth - claimed) / truth
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn kepler_agrees_with_halley() {
        // Halley's published osculating elements: a = 17.834 AU, P = 75.3 yr.
        // Two-body Kepler should land on the published period without a fudge
        // factor; if it does not, everything downstream is decoration.
        assert!(close(period_years(17.834), 75.3, 0.1), "{}", period_years(17.834));
        // and the round trip
        assert!(close(semi_major_au(75.3), 17.834, 0.02));
        // Earth is the definition of the unit system
        assert!(close(period_years(1.0), 1.0, 1e-12));
    }

    #[test]
    fn vis_viva_round_trips_through_inv_a() {
        for &(q, a) in &[(0.586, 17.834), (0.9, 244.0), (1.0, 1.0), (0.05, 3.2)] {
            let v = perihelion_speed(q, a);
            assert!(close(inv_a(q, v), 1.0 / a, 1e-9), "q={q} a={a}");
        }
    }

    #[test]
    fn escape_speed_reads_as_unbound() {
        let q = 0.9;
        let v_esc = (2.0 * MU / q).sqrt();
        assert!(inv_a(q, v_esc).abs() < 1e-9, "parabolic should sit at zero");
        assert!(inv_a(q, v_esc * 1.01) < 0.0, "faster than escape must be hyperbolic");
    }

    #[test]
    fn the_asteri_mistake_is_three_parts_in_a_thousand() {
        // The claim this module exists to support: the difference between
        // "returns in 417 years" and "returns in 3,812 years" is a velocity
        // error far too small to be incompetence.
        let err = asteri_speed_error();
        assert!(err > 0.0, "the true orbit must be the FASTER one at perihelion");
        assert!(err < 0.005, "an error this small should not be a 9x period error: {err}");
        assert!(close(err, 0.00312, 0.0002), "{err}");
    }

    #[test]
    fn their_own_data_could_not_exclude_never() {
        // The harder half of the reveal. At the precision that admits 417
        // years as the floor, the ceiling is already unbounded — so the
        // records did not merely pick a wrong number, they picked the
        // optimistic end of a band that had no other end.
        let q = ASTERI_PERIHELION_AU;
        let v = asteri_true_speed();
        let sigma = precision_for_floor(q, v, ASTERI_RETURN_PREDICTED_YEARS);
        assert!(close(sigma, asteri_speed_error(), 1e-4), "sigma={sigma}");

        let band = period_band(q, v, sigma);
        assert!(
            matches!(band, Window::Open { .. }),
            "the band should be open at this precision, got {band:?}"
        );
        assert!(band.admits(ASTERI_RETURN_PREDICTED_YEARS), "417 must be inside the band");
        assert!(band.admits(ASTERI_RETURN_WINDOW_YEARS), "3812 must be inside the band too");
    }

    #[test]
    fn tightening_the_measurement_closes_the_window() {
        let q = ASTERI_PERIHELION_AU;
        let v = asteri_true_speed();
        // an order of magnitude better than they had, and it closes
        let tight = period_band(q, v, 0.0003);
        let Window::Bounded { min_years, max_years } = tight else {
            panic!("0.03% should be enough to bound it, got {tight:?}");
        };
        assert!(min_years < ASTERI_RETURN_WINDOW_YEARS && ASTERI_RETURN_WINDOW_YEARS < max_years);
        // monotonicity: a worse measurement never yields a tighter floor
        let mut prev = f64::INFINITY;
        for step in 1..=8 {
            let f = period_band(q, v, step as f64 * 0.0005).earliest();
            assert!(f <= prev + 1e-9, "floor rose with worse precision at step {step}");
            prev = f;
        }
    }
}
