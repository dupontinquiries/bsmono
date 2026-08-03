//! property.rs — a number, and where it came from.
//!
//! Material data is the single place in this workspace where a fabricated value does the most
//! damage, because it produces a converged, plausible, wrong answer with no symptom. A mesh that
//! is too coarse shows up in a convergence study; a Young's modulus that is 30% off shows up
//! nowhere at all — every solver downstream will faithfully propagate it.
//!
//! So a property is not an `f64`. It carries its provenance, its reported spread, and the
//! temperature window it is valid in, and there is deliberately **no `Default`** and no way to
//! construct one without saying where it came from. That is the same rule `cad-ref` follows when
//! it refuses to invent a volume: the API makes the absence of a source visible rather than
//! letting it pass as fact.

/// Where a number came from. The distinction between the first three is not bookkeeping — a
/// handbook value for annealed copper and a value homogenized from a microstructure are different
/// kinds of claim, and only one of them can be wrong because the microstructure was wrong.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provenance {
    /// Measured on a specific specimen, cited.
    Measured(&'static str),
    /// A standard reference value for a standard material condition.
    Handbook(&'static str),
    /// Derived by this workspace from a microstructure — see the named method.
    Computed(&'static str),
    /// An informed estimate. Present so that estimates can exist *and be visible*; anything
    /// carrying this should be treated as a placeholder in a report.
    Estimated(&'static str),
}

impl Provenance {
    /// Whether this number is safe to quote without a caveat.
    pub fn is_firm(self) -> bool {
        matches!(self, Provenance::Measured(_) | Provenance::Handbook(_))
    }
    pub fn source(self) -> &'static str {
        match self {
            Provenance::Measured(s)
            | Provenance::Handbook(s)
            | Provenance::Computed(s)
            | Provenance::Estimated(s) => s,
        }
    }
}

/// A material property in SI units.
#[derive(Clone, Copy, Debug)]
pub struct Property {
    pub value: f64,
    /// Reported spread, when the literature disagrees. For biogenic materials this is often
    /// larger than the value's order of magnitude, and hiding it would be dishonest.
    pub range: Option<(f64, f64)>,
    /// Temperature window of validity, kelvin.
    pub valid_k: Option<(f64, f64)>,
    pub provenance: Provenance,
}

impl Property {
    pub fn handbook(value: f64, source: &'static str) -> Property {
        Property { value, range: None, valid_k: None, provenance: Provenance::Handbook(source) }
    }
    pub fn measured(value: f64, source: &'static str) -> Property {
        Property { value, range: None, valid_k: None, provenance: Provenance::Measured(source) }
    }
    pub fn estimated(value: f64, basis: &'static str) -> Property {
        Property { value, range: None, valid_k: None, provenance: Provenance::Estimated(basis) }
    }
    pub fn computed(value: f64, method: &'static str) -> Property {
        Property { value, range: None, valid_k: None, provenance: Provenance::Computed(method) }
    }
    pub fn with_range(mut self, lo: f64, hi: f64) -> Property {
        self.range = Some((lo, hi));
        self
    }
    pub fn valid_between(mut self, lo_k: f64, hi_k: f64) -> Property {
        self.valid_k = Some((lo_k, hi_k));
        self
    }

    /// Relative spread, when known. A value above ~0.5 means the number is a category rather than
    /// a measurement, and a solver quoting four significant figures from it is lying.
    pub fn scatter(&self) -> Option<f64> {
        self.range.map(|(lo, hi)| (hi - lo) / self.value.abs())
    }

    /// Whether this value may be used at temperature `t_k`. Outside the stated window the answer
    /// is "unknown", not "probably fine".
    pub fn valid_at(&self, t_k: f64) -> bool {
        self.valid_k.map_or(true, |(lo, hi)| t_k >= lo && t_k <= hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_separates_firm_values_from_placeholders() {
        assert!(Property::handbook(1.0, "CRC").provenance.is_firm());
        assert!(Property::measured(1.0, "paper").provenance.is_firm());
        assert!(!Property::estimated(1.0, "guess").provenance.is_firm());
        assert!(!Property::computed(1.0, "Voigt").provenance.is_firm());
    }

    #[test]
    fn scatter_reports_how_much_the_literature_disagrees() {
        let firm = Property::handbook(200e9, "CRC").with_range(198e9, 202e9);
        assert!(firm.scatter().unwrap() < 0.05);
        // A biogenic toughness where sources vary by a factor of several.
        let soft = Property::measured(6.0e6, "nacre survey").with_range(2.0e6, 10.0e6);
        assert!(soft.scatter().unwrap() > 1.0, "a factor-of-five spread must be visible");
        assert!(Property::handbook(1.0, "x").scatter().is_none());
    }

    #[test]
    fn temperature_windows_are_enforced_rather_than_assumed() {
        let p = Property::handbook(400.0, "CRC").valid_between(200.0, 500.0);
        assert!(p.valid_at(300.0));
        assert!(!p.valid_at(900.0), "outside the window the answer is unknown, not extrapolated");
        // No stated window means no claim, so nothing is refused.
        assert!(Property::handbook(1.0, "x").valid_at(5000.0));
    }
}
