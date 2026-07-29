//! coresidency.rs — which models can be resident together, and when swapping is worth it.
//!
//! ## The short answer on this machine: do not swap
//!
//! With the 63 GiB the probe verified:
//!
//! ```text
//!   Scout thinker + 3B draft   35.5 GiB   fits
//!   10x 3B panel               15.0 GiB   fits
//!   Scout + 10x 3B panel       49.0 GiB   fits
//! ```
//!
//! A thinker and a ten-model panel are co-resident with room to spare, so the machinery for rotating
//! models on and off the GPU would be built for a constraint that does not exist here. It is modelled
//! anyway, because it *does* exist on a 16 GiB machine and the answer should not be a constant.
//!
//! ## Swapping is disqualified by role, not only by size
//!
//! At the measured 21.1 GiB/s upload, a 3B costs 71 ms to bring in and Scout costs 1.6 s. Whether that
//! is affordable depends entirely on how long the model then stays:
//!
//! * A **panel member** answers for a whole round — hundreds of tokens. 71 ms amortises to nothing.
//! * A **speculative draft** alternates with its target every few tokens. Swapping there would cost
//!   71 ms per alternation against a step that takes single-digit milliseconds — thirty times the work
//!   it is meant to accelerate. Draft and target must be CO-RESIDENT or speculation is pointless.
//!
//! So the question "can these two swap?" is not answerable from sizes alone, and a planner that only
//! compared bytes would happily propose the one arrangement that destroys the feature.
//!
//! ## One hardware queue
//!
//! AGX has a single hardware queue (per the driver audit), so co-resident models do not run in
//! parallel — they time-slice. Residency buys the absence of a swap, not concurrency, and a panel's
//! wall-clock cost is the SUM of its members rather than the maximum.

/// How a model is used, which decides whether it may be swapped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Runs for a long stretch, then yields. Swappable.
    PanelMember,
    /// Alternates with its target every few tokens. Must be resident.
    SpeculativeDraft,
    /// The large model. Swappable in principle, ruinous in practice.
    Thinker,
    /// Small and constantly consulted.
    Liaison,
}

impl Role {
    /// May this model be rotated off the GPU between uses?
    ///
    /// The distinction is CADENCE, not size. A draft is small and must stay; a thinker is large and
    /// could go — the opposite of what a size-based rule would conclude.
    pub fn swappable(self) -> bool {
        matches!(self, Role::PanelMember | Role::Thinker)
    }

    pub fn why_not(self) -> Option<&'static str> {
        match self {
            Role::SpeculativeDraft => Some("a draft alternates with its target every few tokens, so a \
swap would cost more than the speculation saves — thirty times over at measured upload rates"),
            Role::Liaison => Some("the liaison is consulted constantly and is small enough that \
residency is nearly free"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Model { pub name: String, pub gib: f32, pub role: Role }

#[derive(Clone, PartialEq, Debug)]
pub enum Residency {
    /// Everything fits. The good case, and the one that holds on this machine.
    AllResident { used_gib: f32 },
    /// Some must rotate. Carries who, and what a rotation costs.
    Rotating { resident: Vec<String>, swapped: Vec<String>, swap_ms: f32 },
    /// Something that must be resident does not fit, so the configuration is impossible.
    Impossible { reason: String },
}

/// Plan residency for a set of models against a budget.
///
/// Models that cannot be swapped are placed first: if they do not fit, no arrangement works and saying
/// so is more useful than proposing one that will thrash.
pub fn plan(models: &[Model], budget_gib: f32, upload_gib_s: f32) -> Residency {
    let total: f32 = models.iter().map(|m| m.gib).sum();
    if total <= budget_gib {
        return Residency::AllResident { used_gib: total };
    }
    let pinned: f32 = models.iter().filter(|m| !m.role.swappable()).map(|m| m.gib).sum();
    if pinned > budget_gib {
        let who: Vec<&str> = models.iter().filter(|m| !m.role.swappable())
            .map(|m| m.name.as_str()).collect();
        return Residency::Impossible { reason: format!(
            "{:.1} GiB of models cannot be swapped ({}) against a {:.1} GiB budget. Rotating them would \
cost more than they save; use smaller models or fewer roles", pinned, who.join(", "), budget_gib) };
    }
    // Fill the remainder with swappables, largest first — a big model that fits is worth more resident
    // than several small ones whose swaps are individually cheap.
    let mut rest: Vec<&Model> = models.iter().filter(|m| m.role.swappable()).collect();
    rest.sort_by(|a, b| b.gib.partial_cmp(&a.gib).unwrap_or(std::cmp::Ordering::Equal));
    let (mut used, mut resident, mut swapped) = (pinned, Vec::new(), Vec::new());
    for m in models.iter().filter(|m| !m.role.swappable()) { resident.push(m.name.clone()); }
    for m in rest {
        if used + m.gib <= budget_gib { used += m.gib; resident.push(m.name.clone()); }
        else { swapped.push(m.name.clone()); }
    }
    let worst = models.iter().filter(|m| swapped.contains(&m.name))
        .map(|m| m.gib).fold(0f32, f32::max);
    Residency::Rotating { resident, swapped, swap_ms: worst / upload_gib_s.max(0.001) * 1000.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &str, gib: f32, role: Role) -> Model { Model { name: name.into(), gib, role } }

    #[test]
    fn a_thinker_and_a_ten_model_panel_fit_the_measured_ceiling() {
        // The answer to "should we rotate models for consensus mode": on this machine, no. Building the
        // rotation would be machinery for a constraint that does not exist here.
        let mut v = vec![m("scout", 34.0, Role::Thinker)];
        for i in 0..10 { v.push(m(&format!("p{i}"), 1.5, Role::PanelMember)); }
        match plan(&v, 63.0, 21.1) {
            Residency::AllResident { used_gib } => assert!((used_gib - 49.0).abs() < 0.1),
            other => panic!("expected all-resident, got {other:?}"),
        }
    }

    #[test]
    fn a_speculative_draft_is_never_swapped_out_however_small() {
        // The distinction is CADENCE, not size. A draft alternates with its target every few tokens, so
        // a 71 ms swap lands against a step measured in single-digit milliseconds. A size-based rule
        // would swap the draft first, since it is the smallest thing present.
        assert!(!Role::SpeculativeDraft.swappable());
        assert!(Role::Thinker.swappable(), "the LARGE model is the swappable one");
        assert!(Role::SpeculativeDraft.why_not().unwrap().contains("more than the speculation saves"));
    }

    #[test]
    fn an_over_budget_set_rotates_the_swappable_ones_only() {
        let v = vec![
            m("thinker", 10.0, Role::Thinker),
            m("draft", 1.5, Role::SpeculativeDraft),
            m("p0", 4.0, Role::PanelMember),
            m("p1", 4.0, Role::PanelMember),
        ];
        match plan(&v, 12.0, 21.1) {
            Residency::Rotating { resident, swapped, swap_ms } => {
                assert!(resident.contains(&"draft".to_string()), "the draft must stay");
                assert!(!swapped.contains(&"draft".to_string()));
                assert!(swap_ms > 0.0);
            }
            other => panic!("expected rotation, got {other:?}"),
        }
    }

    #[test]
    fn pinned_models_that_do_not_fit_are_IMPOSSIBLE_not_a_thrashing_plan() {
        // Proposing an arrangement that rotates a draft in and out every few tokens would be worse than
        // refusing: it would run, slowly, and look like the feature simply being bad.
        let v = vec![m("draft", 8.0, Role::SpeculativeDraft), m("liaison", 8.0, Role::Liaison)];
        match plan(&v, 10.0, 21.1) {
            Residency::Impossible { reason } => {
                assert!(reason.contains("cannot be swapped"));
                assert!(reason.contains("use smaller models or fewer roles"), "and says what to do");
            }
            other => panic!("expected Impossible, got {other:?}"),
        }
    }

    #[test]
    fn the_largest_swappable_is_kept_resident_first() {
        // A big model that fits is worth more resident than several small ones whose swaps are
        // individually cheap — the swap cost is per rotation, and the big one's is the expensive one.
        let v = vec![
            m("big", 20.0, Role::PanelMember),
            m("s0", 2.0, Role::PanelMember),
            m("s1", 2.0, Role::PanelMember),
        ];
        match plan(&v, 21.0, 21.1) {
            Residency::Rotating { resident, .. } => assert!(resident.contains(&"big".to_string())),
            other => panic!("got {other:?}"),
        }
    }
}
