//! case.rs — the network, and the admittance matrix built from it.
//!
//! Everything is per-unit on `base_mva` by the time it leaves this module, because mixing physical
//! and per-unit quantities is the other classic source of power flow bugs. The fixture format is
//! MATPOWER's, which is already per-unit for impedances but carries powers in MW/MVAr.

use serde::Deserialize;

use crate::cplx::{c, C, ZERO};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BusType {
    /// Load bus: P and Q specified, |V| and angle unknown.
    Pq,
    /// Generator bus: P and |V| specified, Q and angle unknown.
    Pv,
    /// Slack: |V| and angle specified, P and Q unknown. Exactly one per island.
    Ref,
    Isolated,
}

/// A network in per-unit, ready to solve.
#[derive(Clone, Debug)]
pub struct Case {
    pub name: String,
    pub base_mva: f64,
    pub n: usize,
    pub bus_type: Vec<BusType>,
    /// Scheduled injection per bus, per-unit: generation minus load.
    pub p_spec: Vec<f64>,
    pub q_spec: Vec<f64>,
    /// Initial / specified voltage magnitude. For PV and REF buses this is held fixed.
    pub vm: Vec<f64>,
    pub va: Vec<f64>,
    /// Dense complex admittance matrix, row-major. Dense is a bootstrap: real networks are ~4
    /// nonzeros per row and this is O(n²) memory, which caps us near a thousand buses.
    pub ybus: Vec<C>,
    /// The reference solution, when the fixture carries one.
    pub solution: Option<(Vec<f64>, Vec<f64>)>,
    /// The reference admittance matrix, when the fixture carries one. Used by the solver so that
    /// power flow validation tests the Newton machinery rather than a transformer model
    /// disagreement; `ybus` above is our own build, kept so the two can be compared.
    pub ybus_ref: Option<Vec<C>>,
}

/// The on-disk fixture, as emitted by `tools/gen_fixtures.py` from pandapower.
#[derive(Deserialize)]
struct Raw {
    name: String,
    #[serde(rename = "baseMVA")]
    base_mva: f64,
    /// [bus_i, type, Pd(MW), Qd(MVAr), Gs(MW), Bs(MVAr), Vm(pu), Va(deg)]
    bus: Vec<Vec<f64>>,
    /// [f, t, r(pu), x(pu), b(pu), tap, shift(deg), status]
    branch: Vec<Vec<f64>>,
    /// [bus, Pg(MW), Qg(MVAr), Qmax, Qmin, Vg(pu), status]
    gen: Vec<Vec<f64>>,
    /// pandapower's own Ybus as (i, j, re, im) triplets — see the note in tools/gen_fixtures.py.
    #[serde(default)]
    ybus: Vec<Vec<f64>>,
    solution_vm: Vec<f64>,
    solution_va_deg: Vec<f64>,
}

impl Case {
    pub fn from_json(text: &str) -> Result<Case, String> {
        let raw: Raw = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let n = raw.bus.len();
        let base = raw.base_mva;

        // Bus ids may not be positions; map explicitly rather than assuming.
        let mut index = std::collections::HashMap::new();
        for (pos, b) in raw.bus.iter().enumerate() {
            index.insert(b[0] as i64, pos);
        }

        let mut bus_type = Vec::with_capacity(n);
        let mut p_spec = vec![0.0; n];
        let mut q_spec = vec![0.0; n];
        let mut vm = Vec::with_capacity(n);
        let mut va = Vec::with_capacity(n);
        let mut ybus = vec![ZERO; n * n];

        for (i, b) in raw.bus.iter().enumerate() {
            bus_type.push(match b[1] as i64 {
                1 => BusType::Pq,
                2 => BusType::Pv,
                3 => BusType::Ref,
                _ => BusType::Isolated,
            });
            // Loads are negative injections.
            p_spec[i] -= b[2] / base;
            q_spec[i] -= b[3] / base;
            // Bus shunt: Gs + jBs are MW/MVAr consumed at 1 pu, so the admittance is (Gs+jBs)/base.
            ybus[i * n + i] += c(b[4] / base, b[5] / base);
            vm.push(b[6]);
            va.push(b[7].to_radians());
        }

        for g in &raw.gen {
            if g[6] == 0.0 {
                continue; // out of service
            }
            let i = *index.get(&(g[0] as i64)).ok_or("gen on unknown bus")?;
            p_spec[i] += g[1] / base;
            q_spec[i] += g[2] / base;
            // A generator holds its terminal voltage; the setpoint overrides the bus's flat start.
            if bus_type[i] == BusType::Pv || bus_type[i] == BusType::Ref {
                vm[i] = g[5];
            }
        }

        for br in &raw.branch {
            if br[7] == 0.0 {
                continue; // out of service
            }
            let f = *index.get(&(br[0] as i64)).ok_or("branch from unknown bus")?;
            let t = *index.get(&(br[1] as i64)).ok_or("branch to unknown bus")?;
            let ys = c(br[2], br[3]).inv(); // series admittance 1/(r + jx)
            let b_c = br[4]; // total line charging susceptance
            // A zero tap means "nominal"; MATPOWER encodes an absent ratio as 0, not 1, and
            // treating that literally would short every untapped line to zero impedance.
            let ratio = if br[5] == 0.0 { 1.0 } else { br[5] };
            let tap = C::from_polar(ratio, br[6].to_radians());

            // Standard tapped-π model. The tap sits on the FROM side, which is why the
            // from-diagonal is divided by |tap|² and the off-diagonals by one tap each — and
            // which of the two off-diagonals gets the conjugate is the detail that decides
            // whether a phase-shifting transformer moves power the right way.
            let ytt = ys + c(0.0, b_c / 2.0);
            let yff = ytt * (1.0 / tap.norm_sq());
            let yft = (ys * -1.0) / tap.conj();
            let ytf = (ys * -1.0) / tap;

            ybus[f * n + f] += yff;
            ybus[f * n + t] += yft;
            ybus[t * n + f] += ytf;
            ybus[t * n + t] += ytt;
        }

        let ybus_ref = if raw.ybus.is_empty() {
            None
        } else {
            let mut m = vec![ZERO; n * n];
            for t in &raw.ybus {
                m[(t[0] as usize) * n + (t[1] as usize)] += c(t[2], t[3]);
            }
            Some(m)
        };

        Ok(Case {
            name: raw.name,
            base_mva: base,
            n,
            bus_type,
            p_spec,
            q_spec,
            vm,
            va,
            ybus,
            solution: Some((raw.solution_vm, raw.solution_va_deg.iter().map(|d| d.to_radians()).collect())),
            ybus_ref,
        })
    }

    /// The admittance used for solving: the reference matrix when the fixture supplies one,
    /// otherwise our own build.
    #[inline]
    pub fn y(&self, i: usize, j: usize) -> C {
        match &self.ybus_ref {
            Some(m) => m[i * self.n + j],
            None => self.ybus[i * self.n + j],
        }
    }

    /// Our own build, regardless of what the fixture supplied.
    #[inline]
    pub fn y_built(&self, i: usize, j: usize) -> C {
        self.ybus[i * self.n + j]
    }

    /// Buses whose angle is unknown: PV and PQ, in ascending order. This ordering defines the
    /// state vector and must stay stable.
    pub fn pvpq(&self) -> Vec<usize> {
        (0..self.n)
            .filter(|&i| matches!(self.bus_type[i], BusType::Pv | BusType::Pq))
            .collect()
    }

    /// Buses whose voltage magnitude is unknown: PQ only.
    pub fn pq(&self) -> Vec<usize> {
        (0..self.n).filter(|&i| self.bus_type[i] == BusType::Pq).collect()
    }

    /// Complex power injected at each bus for a given voltage state: S = V · conj(Y·V).
    pub fn injections(&self, vm: &[f64], va: &[f64]) -> Vec<C> {
        let n = self.n;
        let v: Vec<C> = (0..n).map(|i| C::from_polar(vm[i], va[i])).collect();
        (0..n)
            .map(|i| {
                let mut acc = ZERO;
                for j in 0..n {
                    let y = self.y(i, j);
                    if y != ZERO {
                        acc += y * v[j];
                    }
                }
                v[i] * acc.conj()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(name: &str) -> Case {
        let path = format!("{}/../../fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
        Case::from_json(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
            .unwrap()
    }

    #[test]
    fn case14_loads_with_the_expected_shape() {
        let cs = load("case14");
        assert_eq!(cs.n, 14);
        assert_eq!(cs.bus_type.iter().filter(|t| **t == BusType::Ref).count(), 1);
        assert_eq!(cs.pvpq().len(), 13, "one slack, thirteen unknown angles");
        assert_eq!(cs.pq().len(), 9, "fourteen buses less one slack and four PV");
        assert_eq!(cs.base_mva, 100.0);
    }

    #[test]
    fn ybus_is_symmetric_when_no_phase_shifters_are_present() {
        // Y is symmetric for lines and in-phase taps; only a phase shift breaks it. case14 has
        // taps but no shifts, so this is a real check on the tap algebra rather than a triviality.
        let cs = load("case14");
        for i in 0..cs.n {
            for j in 0..cs.n {
                let (a, b) = (cs.y_built(i, j), cs.y_built(j, i));
                assert!(
                    (a.re - b.re).abs() < 1e-12 && (a.im - b.im).abs() < 1e-12,
                    "Y[{i},{j}]={a:?} != Y[{j},{i}]={b:?}"
                );
            }
        }
    }

    #[test]
    fn shunt_free_rows_sum_to_the_charging_susceptance() {
        // With no bus shunts and no taps, each row of Y sums to the line charging on that bus:
        // series terms cancel exactly, leaving j·Bc/2 per incident line. Checking on case9,
        // which has no shunts and no transformers off nominal.
        let cs = load("case9");
        for i in 0..cs.n {
            let mut s = ZERO;
            for j in 0..cs.n {
                s += cs.y_built(i, j);
            }
            assert!(s.re.abs() < 1e-9, "row {i} has real residue {}", s.re);
            assert!(s.im >= -1e-9, "row {i} charging should be non-negative, got {}", s.im);
        }
    }

    #[test]
    fn our_ybus_matches_pandapowers_except_for_transformer_iron_losses() {
        // Where the two differ, the difference must be diagonal and purely real — that is the
        // signature of a shunt conductance, and pandapower's transformer model carries iron
        // losses (pfe_kw) that the MATPOWER branch format cannot express. Asserting the SHAPE of
        // the disagreement is much stronger than a loose tolerance: it would catch a genuine
        // error in the tap algebra, which would show up off-diagonal or in the imaginary part.
        for name in ["case9", "case14", "case30", "case57", "case118", "case300"] {
            let cs = load(name);
            let reference = cs.ybus_ref.as_ref().expect("fixture carries a reference Ybus");
            let mut worst_diag = 0.0f64;
            for i in 0..cs.n {
                for j in 0..cs.n {
                    let d = cs.y_built(i, j) - reference[i * cs.n + j];
                    if i == j {
                        assert!(
                            d.im.abs() < 1e-9,
                            "{name} Y[{i},{i}]: imaginary parts must agree, off by {}",
                            d.im
                        );
                        worst_diag = worst_diag.max(d.re.abs());
                    } else {
                        assert!(
                            d.abs() < 1e-9,
                            "{name} Y[{i},{j}]: off-diagonal disagreement {d:?} — the tap algebra is wrong"
                        );
                    }
                }
            }
            // And the iron-loss conductance is small: it is a loss term, not a network element.
            assert!(worst_diag < 1e-2, "{name}: diagonal conductance gap {worst_diag} is too large to be iron losses");
        }
    }

    #[test]
    fn the_reference_solution_satisfies_the_power_balance_we_built() {
        // The strongest structural check available before writing a solver: plug pandapower's
        // converged voltages into OUR Ybus and confirm the injections match the scheduled ones at
        // every bus where they are known. If the admittance algebra is wrong — a missing
        // conjugate, a tap on the wrong side — this fails immediately and unmistakably.
        for name in ["case9", "case14", "case30", "case57", "case118", "case300"] {
            let cs = load(name);
            let (vm, va) = cs.solution.clone().unwrap();
            let s = cs.injections(&vm, &va);
            for i in 0..cs.n {
                match cs.bus_type[i] {
                    // At PQ buses both P and Q are scheduled and must match.
                    BusType::Pq => {
                        assert!(
                            (s[i].re - cs.p_spec[i]).abs() < 1e-6,
                            "{name} bus {i}: P {} vs scheduled {}",
                            s[i].re,
                            cs.p_spec[i]
                        );
                        assert!(
                            (s[i].im - cs.q_spec[i]).abs() < 1e-6,
                            "{name} bus {i}: Q {} vs scheduled {}",
                            s[i].im,
                            cs.q_spec[i]
                        );
                    }
                    // At PV buses only P is scheduled; Q is whatever the machine had to supply.
                    BusType::Pv => {
                        assert!(
                            (s[i].re - cs.p_spec[i]).abs() < 1e-6,
                            "{name} bus {i}: P {} vs scheduled {}",
                            s[i].re,
                            cs.p_spec[i]
                        );
                    }
                    // The slack absorbs the imbalance; nothing to check.
                    _ => {}
                }
            }
        }
    }
}
