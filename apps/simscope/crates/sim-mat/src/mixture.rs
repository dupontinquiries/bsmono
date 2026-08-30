//! mixture.rs — bounds on what a mixture can be.
//!
//! Effective properties of a composite are a *solver output*, not a lookup — you compute them by
//! loading a representative volume element and measuring its response. But before any solver runs,
//! there are bounds that follow from thermodynamics alone, and they are unusually valuable because
//! they need no reference data at all:
//!
//! * **Voigt** (uniform strain) is an upper bound: `E_V = Σ fᵢEᵢ`.
//! * **Reuss** (uniform stress) is a lower bound: `1/E_R = Σ fᵢ/Eᵢ`.
//!
//! Any effective modulus outside `[E_R, E_V]` is *provably wrong*. That makes the bracket the
//! primary acceptance test for a homogenization solver: not "does it match a paper", which invites
//! tuning, but "is it admissible", which cannot be argued with.
//!
//! **Hashin–Shtrikman** narrows the bracket considerably for a statistically isotropic two-phase
//! mixture, and the gap between HS and Voigt/Reuss is itself informative — when HS is tight, the
//! microstructure barely matters; when it is wide, geometry dominates and only a solve will do.
//!
//! And there is a limit worth stating plainly, because it is exactly where nacre lives: **these
//! bounds constrain stiffness, not toughness.** Nacre's modulus sits comfortably inside the
//! Voigt–Reuss bracket for aragonite and protein. Its *toughness* is forty times either
//! constituent, and no bound of this kind can see that, because the mechanism is interfacial and
//! bounds assume perfectly bonded phases. A homogenization that reproduces nacre's stiffness
//! proves nothing about its toughness.

/// One phase of a mixture: a volume fraction and its properties.
#[derive(Clone, Copy, Debug)]
pub struct Phase {
    pub fraction: f64,
    /// Young's modulus, Pa.
    pub e: f64,
    pub nu: f64,
}

impl Phase {
    pub fn bulk(&self) -> f64 {
        self.e / (3.0 * (1.0 - 2.0 * self.nu))
    }
    pub fn shear(&self) -> f64 {
        self.e / (2.0 * (1.0 + self.nu))
    }
}

/// Voigt average — uniform strain, an upper bound.
pub fn voigt(phases: &[Phase]) -> f64 {
    phases.iter().map(|p| p.fraction * p.e).sum()
}

/// Reuss average — uniform stress, a lower bound.
pub fn reuss(phases: &[Phase]) -> f64 {
    let inv: f64 = phases.iter().map(|p| p.fraction / p.e).sum();
    if inv <= 0.0 {
        return 0.0;
    }
    1.0 / inv
}

/// Voigt–Reuss–Hill: the arithmetic mean of the two bounds. Not a bound and not derived from
/// anything — an empirical midpoint that happens to work well for many real polycrystals, and
/// should be labelled as an estimate wherever it is used.
pub fn hill(phases: &[Phase]) -> f64 {
    0.5 * (voigt(phases) + reuss(phases))
}

/// Hashin–Shtrikman bounds on bulk and shear modulus for a two-phase isotropic mixture.
///
/// Returns `((k_lo, k_hi), (g_lo, g_hi))`. Tighter than Voigt–Reuss whenever the phases differ
/// modestly, and it assumes statistical isotropy — so it does *not* apply to an aligned laminate
/// like nacre's platelet stack, whose whole point is that it is not isotropic.
pub fn hashin_shtrikman(a: Phase, b: Phase) -> ((f64, f64), (f64, f64)) {
    let (k1, g1, f1) = (a.bulk(), a.shear(), a.fraction);
    let (k2, g2, f2) = (b.bulk(), b.shear(), b.fraction);
    // The bound formulae are symmetric; evaluating with the stiffer phase as reference gives the
    // upper bound and the softer phase the lower.
    let k_bound = |_kr: f64, gr: f64| -> f64 {
        let term = f1 / (k1 + 4.0 / 3.0 * gr) + f2 / (k2 + 4.0 / 3.0 * gr);
        1.0 / term - 4.0 / 3.0 * gr
    };
    let g_bound = |kr: f64, gr: f64| -> f64 {
        let zeta = gr / 6.0 * (9.0 * kr + 8.0 * gr) / (kr + 2.0 * gr);
        let term = f1 / (g1 + zeta) + f2 / (g2 + zeta);
        1.0 / term - zeta
    };
    let (soft, stiff) = if g1 <= g2 { (a, b) } else { (b, a) };
    let (ks, gs) = (soft.bulk(), soft.shear());
    let (kt, gt) = (stiff.bulk(), stiff.shear());
    ((k_bound(ks, gs), k_bound(kt, gt)), (g_bound(ks, gs), g_bound(kt, gt)))
}

/// Whether an effective modulus is admissible for this mixture. A homogenization solver that
/// returns something outside the bracket has a bug, and this says so without needing a reference.
pub fn is_admissible(phases: &[Phase], e_eff: f64) -> bool {
    let (lo, hi) = (reuss(phases), voigt(phases));
    e_eff >= lo * (1.0 - 1e-9) && e_eff <= hi * (1.0 + 1e-9)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nacre_phases() -> Vec<Phase> {
        // ~95% aragonite by volume, ~5% organic matrix. The matrix modulus is soft and poorly
        // characterised; this is an order-of-magnitude figure and the test treats it as such.
        vec![
            Phase { fraction: 0.95, e: 100e9, nu: 0.30 },
            Phase { fraction: 0.05, e: 2e9, nu: 0.40 },
        ]
    }

    #[test]
    fn reuss_never_exceeds_voigt() {
        for f in [0.0, 0.1, 0.5, 0.9, 1.0] {
            let ps = vec![
                Phase { fraction: f, e: 400e9, nu: 0.25 },
                Phase { fraction: 1.0 - f, e: 3e9, nu: 0.35 },
            ];
            assert!(reuss(&ps) <= voigt(&ps) * (1.0 + 1e-12), "bounds crossed at f = {f}");
            assert!(hill(&ps) >= reuss(&ps) && hill(&ps) <= voigt(&ps));
        }
    }

    #[test]
    fn a_single_phase_mixture_is_that_phase() {
        // Degenerate case: both bounds must collapse onto the constituent, or the bracket is
        // meaningless at the ends.
        let ps = vec![Phase { fraction: 1.0, e: 210e9, nu: 0.3 }];
        assert!((voigt(&ps) - 210e9).abs() < 1.0);
        assert!((reuss(&ps) - 210e9).abs() < 1.0);
    }

    #[test]
    fn a_soft_minority_phase_dominates_the_lower_bound() {
        // The physical asymmetry that makes composites interesting: 5% of something soft barely
        // moves the Voigt average but collapses the Reuss one, because uniform stress puts all the
        // strain into the compliant phase. The bracket is therefore very wide, which is precisely
        // the signal that microstructure decides the answer and a solve is required.
        let ps = nacre_phases();
        let v = voigt(&ps);
        let r = reuss(&ps);
        assert!(v > 90e9, "Voigt should be near the mineral: {v:e}");
        assert!(r < 40e9, "Reuss should be dragged far down by the soft phase: {r:e}");
        assert!(v / r > 2.0, "a wide bracket means the geometry decides, ratio {:.1}", v / r);
    }

    #[test]
    fn measured_nacre_stiffness_is_admissible_but_its_toughness_is_not_explained() {
        // Nacre's modulus sits inside the bracket, so no bound is violated and stiffness is
        // unremarkable. Its toughness is ~24x aragonite's, and NO bound of this kind can account
        // for that, because bounds assume perfectly bonded phases while nacre's toughening comes
        // from interfaces sliding, deflecting and bridging. Passing a stiffness bracket proves
        // nothing about toughness, and this test exists so that is not quietly forgotten.
        let ps = nacre_phases();
        let measured = crate::library::nacre().elastic.e_inplane();
        assert!(is_admissible(&ps, measured), "measured {measured:e} outside [{:e}, {:e}]", reuss(&ps), voigt(&ps));

        let n_tough = crate::library::nacre().fracture.unwrap().k_ic.value;
        let a_tough = crate::library::aragonite().fracture.unwrap().k_ic.value;
        // A rule-of-mixtures toughness — the only thing bounds could offer — is nowhere near.
        let mixture_tough = 0.95 * a_tough + 0.05 * 0.1e6;
        assert!(
            n_tough > mixture_tough * 10.0,
            "toughness {n_tough:e} vs any mixture estimate {mixture_tough:e}: bounds cannot reach it"
        );
    }

    #[test]
    fn hashin_shtrikman_narrows_the_bracket() {
        // For a mixture whose phases are not wildly different, HS should sit strictly inside
        // Voigt-Reuss — that tightening is the whole reason to compute it.
        let a = Phase { fraction: 0.6, e: 200e9, nu: 0.3 };
        let b = Phase { fraction: 0.4, e: 70e9, nu: 0.33 };
        let ((k_lo, k_hi), (g_lo, g_hi)) = hashin_shtrikman(a, b);
        assert!(k_lo <= k_hi, "HS bulk bounds inverted: {k_lo:e} {k_hi:e}");
        assert!(g_lo <= g_hi, "HS shear bounds inverted: {g_lo:e} {g_hi:e}");
        let ps = vec![a, b];
        // Convert the HS bulk/shear bounds to a Young's modulus band and check containment.
        let e_of = |k: f64, g: f64| 9.0 * k * g / (3.0 * k + g);
        let e_lo = e_of(k_lo, g_lo);
        let e_hi = e_of(k_hi, g_hi);
        assert!(e_lo >= reuss(&ps) * 0.999, "HS lower {e_lo:e} below Reuss {:e}", reuss(&ps));
        assert!(e_hi <= voigt(&ps) * 1.001, "HS upper {e_hi:e} above Voigt {:e}", voigt(&ps));
        assert!(e_hi - e_lo < voigt(&ps) - reuss(&ps), "HS must be tighter than Voigt-Reuss");
    }

    #[test]
    fn admissibility_rejects_an_impossible_effective_modulus() {
        // What the bracket is for: a homogenization solver returning either of these has a bug,
        // and no reference data is needed to know it.
        let ps = nacre_phases();
        assert!(!is_admissible(&ps, voigt(&ps) * 1.01), "stiffer than uniform strain is impossible");
        assert!(!is_admissible(&ps, reuss(&ps) * 0.99), "softer than uniform stress is impossible");
        assert!(is_admissible(&ps, hill(&ps)));
    }
}
