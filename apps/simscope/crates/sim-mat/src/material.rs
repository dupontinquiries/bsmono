//! material.rs — one description of a material, used by every layer.
//!
//! Before this crate, permittivity lived in `sim-field` as a bare closure and in `sim-pcb` as a
//! `Layer { eps_r }`, and neither knew anything about conductivity, stiffness or expansion. That
//! works while a layer needs one property. It stops working the moment the same FR-4 needs ε for
//! L2, k and α for a thermal solve, and E and ν for warp — because then the same material is
//! described four times in four places and they drift.
//!
//! **Anisotropy is first-class rather than an option**, because the materials that matter here are
//! anisotropic in exactly the way that breaks things. FR-4 is a woven glass composite: its
//! through-plane CTE is roughly ten times its in-plane CTE, and that mismatch is what cracks vias
//! during reflow. A scalar α cannot express the failure mode. Nacre is worse — its whole point is
//! that the platelet direction and the stacking direction behave completely differently.

use crate::property::{Property, Provenance};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Metal,
    Ceramic,
    Polymer,
    /// Engineered mixture — properties may be homogenized rather than measured.
    Composite,
    /// Biological or biologically derived. Expect large scatter and strong environmental
    /// dependence; nacre's toughness collapses when dry.
    Biogenic,
}

/// Elastic behaviour. Transverse isotropy covers the layered cases — a laminate, a nacre tablet
/// stack — with one in-plane and one out-of-plane direction, which is the anisotropy that actually
/// appears here. Full 21-constant anisotropy is not modelled.
#[derive(Clone, Copy, Debug)]
pub enum Elastic {
    Isotropic {
        e: Property,
        nu: Property,
    },
    /// `_p` is in-plane, `_z` is through-thickness.
    TransverselyIsotropic {
        e_p: Property,
        e_z: Property,
        nu_p: Property,
        nu_zp: Property,
        g_zp: Property,
    },
}

impl Elastic {
    /// In-plane Young's modulus, the value a plane-stress solve wants.
    pub fn e_inplane(&self) -> f64 {
        match self {
            Elastic::Isotropic { e, .. } => e.value,
            Elastic::TransverselyIsotropic { e_p, .. } => e_p.value,
        }
    }
    pub fn nu_inplane(&self) -> f64 {
        match self {
            Elastic::Isotropic { nu, .. } => nu.value,
            Elastic::TransverselyIsotropic { nu_p, .. } => nu_p.value,
        }
    }
    /// Shear modulus for the isotropic case: G = E / 2(1+ν).
    pub fn shear(&self) -> f64 {
        match self {
            Elastic::Isotropic { e, nu } => e.value / (2.0 * (1.0 + nu.value)),
            Elastic::TransverselyIsotropic { g_zp, .. } => g_zp.value,
        }
    }
    /// Bulk modulus for the isotropic case: K = E / 3(1−2ν).
    pub fn bulk(&self) -> Option<f64> {
        match self {
            Elastic::Isotropic { e, nu } => Some(e.value / (3.0 * (1.0 - 2.0 * nu.value))),
            _ => None,
        }
    }
    /// Thermodynamic admissibility for an isotropic solid: −1 < ν < 0.5. Outside that the stiffness
    /// tensor is not positive definite, meaning the material would release energy under load, and
    /// any solve using it is meaningless rather than merely inaccurate.
    pub fn is_stable(&self) -> bool {
        match self {
            Elastic::Isotropic { e, nu } => e.value > 0.0 && nu.value > -1.0 && nu.value < 0.5,
            Elastic::TransverselyIsotropic { e_p, e_z, g_zp, .. } => {
                e_p.value > 0.0 && e_z.value > 0.0 && g_zp.value > 0.0
            }
        }
    }
}

/// Thermal transport and expansion. `alpha_z` differs from `alpha` for layered materials.
#[derive(Clone, Copy, Debug)]
pub struct Thermal {
    /// Conductivity, W/(m·K).
    pub k: Property,
    /// Specific heat, J/(kg·K).
    pub cp: Property,
    /// In-plane linear expansion, 1/K.
    pub alpha: Property,
    /// Through-thickness expansion, when it differs materially.
    pub alpha_z: Option<Property>,
}

/// Electrical and dielectric behaviour.
#[derive(Clone, Copy, Debug)]
pub struct Electrical {
    /// Resistivity at 20 °C, Ω·m. Absent for insulators.
    pub rho_20c: Option<Property>,
    /// Temperature coefficient of resistivity, 1/K. This is what L0 needs for a
    /// temperature-dependent power flow, and it is why conductor resistance is not a constant.
    pub alpha_r: Option<Property>,
    /// Relative permittivity, in-plane.
    pub eps_r: Option<Property>,
    /// Through-plane permittivity, when it differs — FR-4's does by about 10%.
    pub eps_r_z: Option<Property>,
    pub loss_tangent: Option<Property>,
}

/// Strength and fracture.
#[derive(Clone, Copy, Debug)]
pub struct Fracture {
    /// Mode I fracture toughness, Pa·√m.
    pub k_ic: Property,
    /// Yield strength, Pa. Absent for brittle materials, which is itself informative.
    pub yield_strength: Option<Property>,
    pub ultimate_strength: Option<Property>,
}

#[derive(Clone, Copy, Debug)]
pub struct Material {
    pub name: &'static str,
    pub class: Class,
    /// kg/m³.
    pub density: Property,
    pub elastic: Elastic,
    pub thermal: Option<Thermal>,
    pub electrical: Option<Electrical>,
    pub fracture: Option<Fracture>,
    /// Anything a user must know before trusting the numbers. Nacre's hydration dependence lives
    /// here, and it is not optional reading.
    pub caveat: Option<&'static str>,
}

impl Material {
    /// Resistivity at temperature, Ω·m: ρ(T) = ρ₂₀ (1 + α (T − 20 °C)).
    ///
    /// This is the linearisation power systems uses, and it is the entire content of a
    /// temperature-dependent power flow: a line's resistance rises as it heats, which raises its
    /// losses, which heats it further.
    pub fn resistivity_at(&self, t_celsius: f64) -> Option<f64> {
        let e = self.electrical.as_ref()?;
        let rho = e.rho_20c?.value;
        let a = e.alpha_r.map_or(0.0, |p| p.value);
        Some(rho * (1.0 + a * (t_celsius - 20.0)))
    }

    /// Longitudinal wave speed, √(E/ρ) — sets the explicit time step for an impact simulation and
    /// is a quick sanity check on E and ρ together.
    pub fn wave_speed(&self) -> f64 {
        (self.elastic.e_inplane() / self.density.value).sqrt()
    }

    /// Every property that is not firm, so a report can flag them rather than quoting estimates as
    /// though they were measurements.
    pub fn soft_properties(&self) -> Vec<(&'static str, Provenance)> {
        let mut out = Vec::new();
        let mut check = |name: &'static str, p: &Property| {
            if !p.provenance.is_firm() {
                out.push((name, p.provenance));
            }
        };
        check("density", &self.density);
        match &self.elastic {
            Elastic::Isotropic { e, nu } => {
                check("E", e);
                check("nu", nu);
            }
            Elastic::TransverselyIsotropic { e_p, e_z, .. } => {
                check("E_p", e_p);
                check("E_z", e_z);
            }
        }
        if let Some(t) = &self.thermal {
            check("k", &t.k);
            check("alpha", &t.alpha);
        }
        if let Some(f) = &self.fracture {
            check("K_Ic", &f.k_ic);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library;

    #[test]
    fn isotropic_moduli_are_mutually_consistent() {
        // G = E/2(1+nu) and K = E/3(1-2nu) are definitions, not measurements, so any material
        // whose stored values contradict them has a typo.
        for m in library::all() {
            if let Elastic::Isotropic { e, nu } = m.elastic {
                let g = m.elastic.shear();
                assert!(
                    (g - e.value / (2.0 * (1.0 + nu.value))).abs() < 1.0,
                    "{}: shear modulus inconsistent",
                    m.name
                );
                let k = m.elastic.bulk().unwrap();
                assert!(k > 0.0 && k.is_finite(), "{}: bulk modulus {k}", m.name);
            }
        }
    }

    #[test]
    fn every_material_in_the_library_is_thermodynamically_stable() {
        // nu outside (-1, 0.5) makes the stiffness tensor indefinite: the material would release
        // energy under load. A solve using it is meaningless rather than inaccurate, so this is a
        // precondition and not a warning.
        for m in library::all() {
            assert!(m.elastic.is_stable(), "{} is not elastically stable", m.name);
            assert!(m.density.value > 0.0, "{} has non-positive density", m.name);
        }
    }

    #[test]
    fn conductor_resistance_rises_with_temperature_by_the_expected_amount() {
        // Copper's temperature coefficient is about 0.00393/K, so 100 K of heating raises its
        // resistivity by roughly 39%. This is the effect a temperature-dependent power flow exists
        // to capture.
        let cu = library::copper();
        let cold = cu.resistivity_at(20.0).unwrap();
        let hot = cu.resistivity_at(120.0).unwrap();
        let rise = hot / cold - 1.0;
        assert!((rise - 0.393).abs() < 0.02, "copper resistivity rose {rise:.3} over 100 K");
        // And an insulator has no resistivity to report, rather than a fabricated one.
        assert!(library::fr4().resistivity_at(20.0).is_none());
    }

    #[test]
    fn pure_metals_satisfy_the_wiedemann_franz_law() {
        // A genuine cross-check between two independently sourced properties: in a metal, heat and
        // charge are carried by the same electrons, so k/(sigma*T) equals the Sommerfeld value
        // 2.44e-8 W*ohm/K^2. If either the thermal conductivity or the resistivity in this library
        // were wrong, this catches it — which no amount of internal consistency checking can,
        // because those two numbers come from different measurements.
        const LORENZ: f64 = 2.44e-8;
        for m in [library::copper(), library::aluminium_6061()] {
            let th = m.thermal.unwrap();
            let sigma = 1.0 / m.resistivity_at(26.85).unwrap();
            let l = th.k.value / (sigma * 300.0);
            assert!((l / LORENZ - 1.0).abs() < 0.2, "{}: Lorenz {l:.3e} vs {LORENZ:.3e}", m.name);
        }
    }

    #[test]
    fn an_alloy_exceeds_the_wiedemann_franz_value_because_phonons_carry_heat() {
        // The first version of this test applied Wiedemann-Franz to every metal and stainless 304
        // failed at 3.9e-8 against 2.44e-8. That is not a bad number in the library — it is the
        // law's actual domain. WF describes ELECTRONIC conduction, and in a heavily disordered
        // alloy electron scattering is so strong that the lattice carries a large share of the
        // heat, so total k exceeds the electronic prediction and the apparent Lorenz number comes
        // out high. Asserting that excess is a stronger statement than exempting the material.
        const LORENZ: f64 = 2.44e-8;
        let m = library::stainless_304();
        let sigma = 1.0 / m.resistivity_at(26.85).unwrap();
        let l = m.thermal.unwrap().k.value / (sigma * 300.0);
        assert!(
            l > LORENZ * 1.2,
            "a disordered alloy should show a phonon excess, Lorenz came out {l:.3e}"
        );
        assert!(l < LORENZ * 3.0, "but not an implausible one: {l:.3e}");
    }

    #[test]
    fn wave_speeds_land_in_the_physically_expected_band() {
        // Longitudinal wave speed in engineering solids runs a few km/s. A value far outside that
        // means E and density disagree by orders of magnitude — the classic units error.
        for m in library::all() {
            let c = m.wave_speed();
            assert!(
                c > 500.0 && c < 15_000.0,
                "{}: wave speed {c:.0} m/s is not physical for a solid",
                m.name
            );
        }
    }

    #[test]
    fn soft_properties_are_reported_rather_than_hidden() {
        // Nacre's numbers are largely measured-with-huge-scatter or estimated, and the API must
        // surface that rather than let them read like handbook constants.
        let nacre = library::nacre();
        assert!(nacre.caveat.is_some(), "nacre must carry its hydration caveat");
        let steel = library::stainless_304();
        assert!(steel.soft_properties().is_empty(), "steel should be all handbook values");
    }
}
