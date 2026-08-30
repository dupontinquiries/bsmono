//! library.rs — the materials themselves.
//!
//! Every number carries a source. Where the literature disagrees the spread is recorded rather
//! than averaged away, and where a value is an estimate it says so, because a report that quotes
//! an estimate as a handbook constant is worse than one that omits it.

use crate::material::{Class, Elastic, Electrical, Fracture, Material, Thermal};
use crate::property::Property;

const CRC: &str = "CRC Handbook of Chemistry and Physics";
const MMPDS: &str = "MMPDS / MIL-HDBK-5 typical values";
const IPC: &str = "IPC-4101 / typical FR-4 laminate datasheets";

pub fn copper() -> Material {
    Material {
        name: "copper (annealed)",
        class: Class::Metal,
        density: Property::handbook(8960.0, CRC),
        elastic: Elastic::Isotropic {
            e: Property::handbook(117e9, CRC).with_range(110e9, 128e9),
            nu: Property::handbook(0.34, CRC),
        },
        thermal: Some(Thermal {
            k: Property::handbook(401.0, CRC).valid_between(200.0, 800.0),
            cp: Property::handbook(385.0, CRC),
            alpha: Property::handbook(16.5e-6, CRC),
            alpha_z: None,
        }),
        electrical: Some(Electrical {
            rho_20c: Some(Property::handbook(1.678e-8, CRC)),
            alpha_r: Some(Property::handbook(3.93e-3, CRC)),
            eps_r: None,
            eps_r_z: None,
            loss_tangent: None,
        }),
        fracture: Some(Fracture {
            k_ic: Property::handbook(60e6, MMPDS).with_range(30e6, 100e6),
            yield_strength: Some(Property::handbook(70e6, MMPDS).with_range(33e6, 300e6)),
            ultimate_strength: Some(Property::handbook(220e6, MMPDS)),
        }),
        caveat: Some("Strength depends strongly on temper; annealed and hard-drawn copper differ by 4x in yield."),
    }
}

pub fn aluminium_6061() -> Material {
    Material {
        name: "aluminium 6061-T6",
        class: Class::Metal,
        density: Property::handbook(2700.0, MMPDS),
        elastic: Elastic::Isotropic {
            e: Property::handbook(68.9e9, MMPDS),
            nu: Property::handbook(0.33, MMPDS),
        },
        thermal: Some(Thermal {
            k: Property::handbook(167.0, CRC),
            cp: Property::handbook(896.0, CRC),
            alpha: Property::handbook(23.6e-6, CRC),
            alpha_z: None,
        }),
        electrical: Some(Electrical {
            rho_20c: Some(Property::handbook(3.99e-8, CRC)),
            alpha_r: Some(Property::handbook(4.03e-3, CRC)),
            eps_r: None,
            eps_r_z: None,
            loss_tangent: None,
        }),
        fracture: Some(Fracture {
            k_ic: Property::handbook(29e6, MMPDS).with_range(24e6, 34e6),
            yield_strength: Some(Property::handbook(276e6, MMPDS)),
            ultimate_strength: Some(Property::handbook(310e6, MMPDS)),
        }),
        caveat: None,
    }
}

pub fn stainless_304() -> Material {
    Material {
        name: "stainless steel 304",
        class: Class::Metal,
        density: Property::handbook(8000.0, MMPDS),
        elastic: Elastic::Isotropic {
            e: Property::handbook(193e9, MMPDS),
            nu: Property::handbook(0.29, MMPDS),
        },
        thermal: Some(Thermal {
            k: Property::handbook(16.2, CRC),
            cp: Property::handbook(500.0, CRC),
            alpha: Property::handbook(17.3e-6, CRC),
            alpha_z: None,
        }),
        electrical: Some(Electrical {
            rho_20c: Some(Property::handbook(7.2e-7, CRC)),
            alpha_r: Some(Property::handbook(1.0e-3, CRC)),
            eps_r: None,
            eps_r_z: None,
            loss_tangent: None,
        }),
        fracture: Some(Fracture {
            k_ic: Property::handbook(150e6, MMPDS).with_range(100e6, 220e6),
            yield_strength: Some(Property::handbook(215e6, MMPDS)),
            ultimate_strength: Some(Property::handbook(505e6, MMPDS)),
        }),
        caveat: Some("A heavily disordered alloy: its thermal conductivity is ~25x below copper's while its resistivity is only ~43x higher, so the Wiedemann-Franz Lorenz number comes out ~60% high. That excess is lattice conduction, not an error in these numbers."),
    }
}

pub fn alumina_99() -> Material {
    Material {
        name: "alumina 99.5%",
        class: Class::Ceramic,
        density: Property::handbook(3890.0, "CoorsTek AD-995 datasheet"),
        elastic: Elastic::Isotropic {
            e: Property::handbook(370e9, "CoorsTek AD-995 datasheet"),
            nu: Property::handbook(0.22, "CoorsTek AD-995 datasheet"),
        },
        thermal: Some(Thermal {
            k: Property::handbook(35.0, "CoorsTek AD-995 datasheet").valid_between(273.0, 1273.0),
            cp: Property::handbook(880.0, CRC),
            alpha: Property::handbook(8.2e-6, CRC),
            alpha_z: None,
        }),
        electrical: Some(Electrical {
            rho_20c: None, // an insulator: there is no meaningful conductor resistivity
            alpha_r: None,
            eps_r: Some(Property::handbook(9.8, "CoorsTek AD-995 datasheet")),
            eps_r_z: None,
            loss_tangent: Some(Property::handbook(1e-4, "CoorsTek AD-995 datasheet")),
        }),
        fracture: Some(Fracture {
            k_ic: Property::handbook(4.0e6, "CoorsTek AD-995 datasheet").with_range(3.0e6, 5.0e6),
            yield_strength: None, // brittle: it fractures rather than yielding, and saying so matters
            ultimate_strength: Some(Property::handbook(379e6, "flexural, CoorsTek AD-995")),
        }),
        caveat: Some("Strength is flexural and highly flaw-dependent; Weibull scatter is large and a single number is a poor design basis."),
    }
}

pub fn zirconia_ysz() -> Material {
    Material {
        name: "zirconia (3Y-TZP)",
        class: Class::Ceramic,
        density: Property::handbook(6050.0, "CoorsTek YTZP datasheet"),
        elastic: Elastic::Isotropic {
            e: Property::handbook(210e9, "CoorsTek YTZP datasheet"),
            nu: Property::handbook(0.31, "CoorsTek YTZP datasheet"),
        },
        thermal: Some(Thermal {
            k: Property::handbook(2.0, CRC),
            cp: Property::handbook(400.0, CRC),
            alpha: Property::handbook(10.3e-6, CRC),
            alpha_z: None,
        }),
        electrical: None,
        fracture: Some(Fracture {
            k_ic: Property::handbook(9.0e6, "CoorsTek YTZP datasheet").with_range(5.0e6, 13.0e6),
            yield_strength: None,
            ultimate_strength: Some(Property::handbook(1000e6, "flexural, CoorsTek YTZP")),
        }),
        caveat: Some("Transformation toughening degrades with hydrothermal ageing; K_Ic is not stable over service life."),
    }
}

pub fn silicon_carbide() -> Material {
    Material {
        name: "silicon carbide (sintered)",
        class: Class::Ceramic,
        density: Property::handbook(3100.0, "CoorsTek SC-30 datasheet"),
        elastic: Elastic::Isotropic {
            e: Property::handbook(410e9, "CoorsTek SC-30 datasheet"),
            nu: Property::handbook(0.14, "CoorsTek SC-30 datasheet"),
        },
        thermal: Some(Thermal {
            k: Property::handbook(125.0, CRC),
            cp: Property::handbook(750.0, CRC),
            alpha: Property::handbook(4.0e-6, CRC),
            alpha_z: None,
        }),
        electrical: None,
        fracture: Some(Fracture {
            k_ic: Property::handbook(4.6e6, "CoorsTek SC-30 datasheet").with_range(3.0e6, 6.0e6),
            yield_strength: None,
            ultimate_strength: Some(Property::handbook(410e6, "flexural, CoorsTek SC-30")),
        }),
        caveat: None,
    }
}

/// FR-4 laminate — the reason `Elastic` has a transversely isotropic case at all.
pub fn fr4() -> Material {
    Material {
        name: "FR-4 laminate",
        class: Class::Composite,
        density: Property::handbook(1850.0, IPC),
        elastic: Elastic::TransverselyIsotropic {
            e_p: Property::handbook(22e9, IPC).with_range(17e9, 26e9),
            e_z: Property::handbook(10e9, IPC).with_range(7e9, 12e9),
            nu_p: Property::handbook(0.15, IPC),
            nu_zp: Property::handbook(0.29, IPC),
            g_zp: Property::handbook(3.5e9, IPC),
        },
        thermal: Some(Thermal {
            k: Property::handbook(0.35, IPC).with_range(0.25, 0.6),
            cp: Property::handbook(1100.0, IPC),
            alpha: Property::handbook(14e-6, IPC).with_range(12e-6, 18e-6),
            // The number that cracks vias: through-thickness expansion is an order of magnitude
            // larger than in-plane, and it jumps again above the glass transition.
            alpha_z: Some(Property::handbook(70e-6, IPC).with_range(50e-6, 90e-6).valid_between(233.0, 403.0)),
        }),
        electrical: Some(Electrical {
            rho_20c: None,
            alpha_r: None,
            eps_r: Some(Property::handbook(4.4, IPC).with_range(4.2, 4.8)),
            eps_r_z: Some(Property::handbook(4.0, IPC).with_range(3.8, 4.4)),
            loss_tangent: Some(Property::handbook(0.02, IPC).with_range(0.015, 0.025)),
        }),
        fracture: Some(Fracture {
            k_ic: Property::estimated(1.5e6, "epoxy-glass laminate, in-plane, order of magnitude"),
            yield_strength: None,
            ultimate_strength: Some(Property::handbook(310e6, "in-plane flexural, IPC-4101")),
        }),
        caveat: Some("Anisotropic and weave-dependent: eps_r varies with glass-to-resin ratio along the trace, which causes skew. Above Tg (~135 C) alpha_z roughly triples."),
    }
}

/// Ultra-low-expansion glass — the material EUV optics and mask blanks are made of, and an
/// illustration of why L3's debts list thermomechanical distortion: at 1e-8/K expansion, a
/// few kelvin of absorbed EUV still moves a mask by nanometres.
pub fn ule_glass() -> Material {
    Material {
        name: "ULE glass (titania-silicate)",
        class: Class::Ceramic,
        density: Property::handbook(2210.0, "Corning ULE 7973 datasheet"),
        elastic: Elastic::Isotropic {
            e: Property::handbook(67.6e9, "Corning ULE 7973 datasheet"),
            nu: Property::handbook(0.17, "Corning ULE 7973 datasheet"),
        },
        thermal: Some(Thermal {
            k: Property::handbook(1.31, "Corning ULE 7973 datasheet"),
            cp: Property::handbook(767.0, "Corning ULE 7973 datasheet"),
            alpha: Property::handbook(0.0e-6, "Corning ULE 7973 datasheet")
                .with_range(-3.0e-8, 3.0e-8)
                .valid_between(278.0, 308.0),
            alpha_z: None,
        }),
        electrical: None,
        fracture: Some(Fracture {
            k_ic: Property::handbook(0.7e6, "fused silica family, typical"),
            yield_strength: None,
            ultimate_strength: Some(Property::estimated(50e6, "glass, flaw-limited")),
        }),
        caveat: Some("CTE is nominally zero only in a narrow band around room temperature; the stated value is a zero crossing, not a constant."),
    }
}

/// Aragonite — the mineral nacre is made of, included precisely so the contrast is visible.
pub fn aragonite() -> Material {
    Material {
        name: "aragonite (CaCO3)",
        class: Class::Ceramic,
        density: Property::handbook(2930.0, CRC),
        elastic: Elastic::Isotropic {
            e: Property::handbook(100e9, "single-crystal average, mineralogy literature")
                .with_range(75e9, 145e9),
            nu: Property::handbook(0.30, "mineralogy literature"),
        },
        thermal: Some(Thermal {
            k: Property::handbook(2.3, CRC),
            cp: Property::handbook(850.0, CRC),
            alpha: Property::handbook(16e-6, CRC).with_range(10e-6, 35e-6),
            alpha_z: None,
        }),
        electrical: None,
        fracture: Some(Fracture {
            // The number that makes nacre remarkable: the mineral alone is as tough as chalk.
            k_ic: Property::handbook(0.25e6, "single-crystal CaCO3").with_range(0.2e6, 0.4e6),
            yield_strength: None,
            ultimate_strength: Some(Property::estimated(30e6, "brittle mineral, flaw-limited")),
        }),
        caveat: Some("Strongly anisotropic as a single crystal; the isotropic value here is a directional average and is only appropriate as a constituent input."),
    }
}

/// Nacre — deliberately the least confident entry in this library, and labelled as such.
///
/// Nacre is ~95% aragonite by volume in a brick-and-mortar arrangement of ~0.5 µm platelets, yet
/// reaches a fracture toughness one to two orders of magnitude above the mineral it is made of.
/// **None of that toughness is in the constituents.** It comes from crack deflection at platelet
/// boundaries, platelet pull-out, mineral bridges and viscoelastic dissipation in the organic
/// matrix — all interfacial. A bulk continuum model cannot produce it, and treating these numbers
/// as ordinary material constants would predict a stiff brittle ceramic very convincingly.
///
/// These values are placeholders for a homogenization that has not been done. They are here so the
/// gap is visible in the library rather than invisible in someone's head.
pub fn nacre() -> Material {
    Material {
        name: "nacre (hydrated)",
        class: Class::Biogenic,
        density: Property::measured(2650.0, "nacre literature, red abalone").with_range(2500.0, 2800.0),
        elastic: Elastic::TransverselyIsotropic {
            e_p: Property::measured(70e9, "nacre literature, in-plane").with_range(60e9, 80e9),
            e_z: Property::measured(50e9, "nacre literature, through-thickness").with_range(30e9, 70e9),
            nu_p: Property::estimated(0.25, "assumed from constituent average"),
            nu_zp: Property::estimated(0.25, "assumed from constituent average"),
            g_zp: Property::estimated(20e9, "assumed from E and nu; not measured"),
        },
        thermal: Some(Thermal {
            k: Property::estimated(2.0, "aragonite-dominated, rule of mixtures"),
            cp: Property::estimated(850.0, "aragonite-dominated, rule of mixtures"),
            alpha: Property::estimated(16e-6, "aragonite-dominated, rule of mixtures"),
            alpha_z: None,
        }),
        electrical: None,
        fracture: Some(Fracture {
            // A factor of forty above aragonite, and the spread is a factor of five.
            k_ic: Property::measured(6.0e6, "nacre literature; species and hydration dependent")
                .with_range(2.0e6, 10.0e6),
            yield_strength: None,
            ultimate_strength: Some(Property::measured(140e6, "nacre literature, flexural").with_range(80e6, 220e6)),
        }),
        caveat: Some(
            "DRY NACRE IS BRITTLE. The organic matrix's viscoelastic dissipation requires water, and \
             dehydrated specimens lose most of the toughening. Toughness is also interfacial in \
             origin — crack deflection, platelet pull-out, mineral bridges — so these bulk constants \
             cannot reproduce it in a continuum model. Use them for stiffness only, and treat K_Ic \
             as a target for a cohesive-interface model rather than as an input.",
        ),
    }
}

/// Everything in the library, for sweeps and for the consistency tests.
pub fn all() -> Vec<Material> {
    vec![
        copper(),
        aluminium_6061(),
        stainless_304(),
        alumina_99(),
        zirconia_ysz(),
        silicon_carbide(),
        fr4(),
        ule_glass(),
        aragonite(),
        nacre(),
    ]
}

/// Look up by name; returns `None` rather than a default, for the same reason `cad-ref` refuses to
/// invent a volume.
pub fn by_name(name: &str) -> Option<Material> {
    all().into_iter().find(|m| m.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nacre_is_far_tougher_than_the_mineral_it_is_made_of() {
        // The fact the whole materials arm exists to explain, stated as a test so it cannot be
        // quietly lost: nacre is ~95% aragonite and yet an order of magnitude tougher.
        let a = aragonite().fracture.unwrap().k_ic.value;
        let n = nacre().fracture.unwrap().k_ic.value;
        assert!(n / a > 10.0, "toughness amplification is only {:.1}x", n / a);
        // And the stiffnesses are comparable, which is what makes the toughness surprising rather
        // than just "it is a different material".
        let ratio = nacre().elastic.e_inplane() / aragonite().elastic.e_inplane();
        assert!(ratio > 0.5 && ratio < 1.2, "stiffness ratio {ratio:.2}");
    }

    #[test]
    fn nacres_uncertainty_is_visible_in_the_api() {
        let n = nacre();
        assert!(n.caveat.unwrap().contains("DRY NACRE IS BRITTLE"));
        let soft = n.soft_properties();
        assert!(!soft.is_empty(), "nacre's estimated properties must be reportable");
        // Its toughness spread is a factor of five and must not read as a precise constant.
        assert!(n.fracture.unwrap().k_ic.scatter().unwrap() > 1.0);
    }

    #[test]
    fn fr4_anisotropy_is_recorded_where_it_causes_failures() {
        let f = fr4();
        let t = f.thermal.unwrap();
        let ratio = t.alpha_z.unwrap().value / t.alpha.value;
        assert!(ratio > 4.0, "through-plane CTE should dominate: ratio {ratio:.1}");
        // And in-plane vs through-plane permittivity differ, which is why L2's scalar eps_r is an
        // approximation rather than the whole truth.
        let e = f.electrical.unwrap();
        assert!(e.eps_r.unwrap().value != e.eps_r_z.unwrap().value);
    }

    #[test]
    fn brittle_materials_report_no_yield_strength() {
        // Absence is informative: a ceramic does not yield, it fractures. Storing a fabricated
        // yield strength would let a plasticity model run on a material that has none.
        for m in [alumina_99(), zirconia_ysz(), silicon_carbide(), aragonite()] {
            assert!(
                m.fracture.unwrap().yield_strength.is_none(),
                "{} should not claim a yield strength",
                m.name
            );
        }
        assert!(copper().fracture.unwrap().yield_strength.is_some());
    }

    #[test]
    fn ceramics_are_stiffer_and_less_tough_than_metals() {
        // A structural ordering that must hold across the library, and which catches a units error
        // in any single entry.
        let cu = copper();
        let al2o3 = alumina_99();
        assert!(al2o3.elastic.e_inplane() > cu.elastic.e_inplane());
        assert!(
            al2o3.fracture.unwrap().k_ic.value < cu.fracture.unwrap().k_ic.value,
            "ceramics should be markedly less tough than metals"
        );
    }

    #[test]
    fn lookup_returns_nothing_rather_than_a_default() {
        assert!(by_name("copper (annealed)").is_some());
        assert!(by_name("unobtainium").is_none());
    }

    #[test]
    fn ule_glass_earns_its_name() {
        // Its whole reason for existing: expansion near zero at room temperature, which is what
        // makes an EUV mask blank dimensionally stable under absorbed power.
        let u = ule_glass();
        let a = u.thermal.unwrap().alpha;
        assert!(a.value.abs() < 1e-7, "ULE CTE should be ~0, got {}", a.value);
        assert!(a.valid_k.is_some(), "and that is only true over a narrow band, which must be stated");
        assert!(!a.valid_at(600.0));
    }
}
