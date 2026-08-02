//! rolequant.rs — precision by ROLE, not by model.
//!
//! A panel does not need every member at the same precision. The observation this encodes:
//!
//! > the liaison is just reporting on a thinker — quantizing it harder increases speed and reduces
//! > RAM, and could fit a larger thinker resident on GPU.
//!
//! That is right, and it generalises: **a role that reports does not need the precision a role that
//! reasons does.** The liaison paraphrases a backend's state and relays edits; a debater takes a
//! position and defends it; the thinker does the work everything else describes. Spending equal bits
//! on all three is spending the thinker's headroom on jobs that do not use it.
//!
//! ## Why this is a budget problem and not a preference
//!
//! In panel and debate modes memory multiplies. Three 8B debaters at q4 is ~13.5 GiB before the
//! thinker loads at all, and on a device whose measured ceiling is 31 GiB (M2 Max) that is most of it.
//! Dropping the reporters to Q2 recovers ~7 GiB — which is the difference between a 30B thinker
//! resident and a 30B thinker streaming.
//!
//! ## What this module does NOT decide
//!
//! Whether a given quantization is good enough for a job is a quality judgement no arithmetic can
//! make. The policy here is deliberately coarse and conservative: it never quantizes the thinker
//! below the level the operator chose, and it never claims a role *can* take a level — only that
//! doing so would free a stated number of bytes.

/// What a model in a panel is being asked to do.
///
/// Ordered by how much of the answer's quality depends on it, because that ordering is the whole
/// policy: when the budget binds, precision comes off the bottom first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Role {
    /// Paraphrases another model's state, relays edits, answers "what is it doing".
    ///
    /// The lowest bar in the panel: it is describing work rather than doing it, and a wrong word in a
    /// status line costs far less than a wrong step in a proof.
    Liaison,
    /// Drafts tokens for a larger model to verify.
    ///
    /// Its output is *checked* — a wrong draft is rejected and costs one round, never a wrong answer.
    /// That makes it the one role where lower precision has a bounded, visible cost.
    Draft,
    /// Holds and defends a position in a panel.
    Debater,
    /// The model whose reasoning is the product.
    Thinker,
}

/// Bits per weight of the formats the engine can hold resident.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level { Q2, Q3, Q4, Q8 }

impl Level {
    pub fn bpw(self) -> f32 {
        match self { Level::Q2 => 2.625, Level::Q3 => 3.4375, Level::Q4 => 4.5, Level::Q8 => 8.5 }
    }
    pub fn label(self) -> &'static str {
        match self { Level::Q2 => "Q2_K", Level::Q3 => "Q3_K", Level::Q4 => "Q4_K", Level::Q8 => "Q8_0" }
    }
    /// One step down, or None at the floor.
    ///
    /// Q2 is the floor rather than IQ1_S because below Q2 the loss stops being graceful — a 1.5-bit
    /// model is a deliberate choice about a whole checkpoint, not something to impose on a role.
    pub fn down(self) -> Option<Level> {
        match self { Level::Q8 => Some(Level::Q4), Level::Q4 => Some(Level::Q3),
                     Level::Q3 => Some(Level::Q2), Level::Q2 => None }
    }
}

/// The lowest level a role may be taken to.
///
/// A floor per role rather than a global one, because the roles differ in what a mistake costs. A
/// draft model's errors are caught by verification; a thinker's are the output.
pub fn floor(role: Role) -> Level {
    match role {
        // Reporting and drafting both have a checker downstream — the operator reads the status, the
        // target verifies the draft — so their floor is the lowest the engine holds.
        Role::Liaison | Role::Draft => Level::Q2,
        // A debater's position IS the product of its turn, and nothing downstream checks it.
        Role::Debater => Level::Q3,
        // Never reduced below what was chosen. The thinker's precision is the operator's decision and
        // this module does not get a vote.
        Role::Thinker => Level::Q8,
    }
}

/// One member of a panel.
#[derive(Clone, PartialEq, Debug)]
pub struct Member {
    pub name: String,
    pub role: Role,
    pub params: u64,
    /// What the operator asked for.
    pub chosen: Level,
}

impl Member {
    pub fn bytes_at(&self, l: Level) -> u64 { (self.params as f64 * l.bpw() as f64 / 8.0) as u64 }
}

/// A plan: what level each member ends up at, and what it cost or saved.
#[derive(Clone, PartialEq, Debug)]
pub struct Plan {
    pub levels: Vec<Level>,
    pub total_bytes: u64,
    /// Bytes freed relative to everyone at their chosen level.
    pub saved_bytes: u64,
    /// True when the plan still exceeds the budget after every permitted reduction.
    pub still_over: bool,
}

/// Fit a panel into `budget_bytes` by reducing the roles that can take it.
///
/// Reduces **one step at a time, lowest role first**, rather than dropping the liaison straight to its
/// floor. A single step is often enough, and taking more than is needed spends quality for nothing —
/// the same reason `next_draft_len` grows by one rather than doubling.
///
/// The thinker is never touched. If the panel does not fit with every other role at its floor, the
/// plan says so rather than reducing the one member whose precision is the point.
pub fn fit(members: &[Member], budget_bytes: u64) -> Plan {
    let mut levels: Vec<Level> = members.iter().map(|m| m.chosen).collect();
    let chosen_total: u64 = members.iter().map(|m| m.bytes_at(m.chosen)).sum();
    let total = |ls: &[Level]| -> u64 {
        members.iter().zip(ls).map(|(m, l)| m.bytes_at(*l)).sum()
    };

    // Reduce in role order, one step per pass, until it fits or nothing can move.
    loop {
        if total(&levels) <= budget_bytes { break; }
        let mut moved = false;
        // Lowest role first: Liaison, then Draft, then Debater. Thinker's floor is Q8 so it never
        // moves, and the ordering is the enum's own — declared by cost of a mistake.
        for target in [Role::Liaison, Role::Draft, Role::Debater] {
            for (i, m) in members.iter().enumerate() {
                if m.role != target { continue; }
                let f = floor(m.role);
                if let Some(next) = levels[i].down() {
                    if next >= f {
                        levels[i] = next;
                        moved = true;
                        if total(&levels) <= budget_bytes { return done(members, levels, chosen_total, budget_bytes); }
                    }
                }
            }
            if moved { break; }   // one step across a role class, then re-check
        }
        if !moved { break; }
    }
    done(members, levels, chosen_total, budget_bytes)
}

fn done(members: &[Member], levels: Vec<Level>, chosen_total: u64, budget: u64) -> Plan {
    let total_bytes: u64 = members.iter().zip(&levels).map(|(m, l)| m.bytes_at(*l)).sum();
    Plan {
        saved_bytes: chosen_total.saturating_sub(total_bytes),
        still_over: total_bytes > budget,
        total_bytes,
        levels,
    }
}

/// A human-readable account of what the plan changed and why.
///
/// Says what was reduced AND what it bought, because a plan that silently lowers precision is one an
/// operator cannot evaluate — and the whole premise here is that the trade is worth making for some
/// roles and not others.
pub fn explain(members: &[Member], plan: &Plan) -> String {
    let mut s = String::new();
    for (m, l) in members.iter().zip(&plan.levels) {
        if *l != m.chosen {
            s.push_str(&format!(
                "{} ({:?}) {} → {}, freeing {:.1} GiB; it reports rather than reasons, so the loss \
lands where it costs least. ",
                m.name, m.role, m.chosen.label(), l.label(),
                (m.bytes_at(m.chosen) - m.bytes_at(*l)) as f64 / (1u64 << 30) as f64));
        }
    }
    if s.is_empty() { s.push_str("every member fits at the level you chose. "); }
    if plan.still_over {
        s.push_str("Still over budget with every reporting role at its floor — the remainder would \
have to come from the thinker, which this will not do on its own.");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;
    fn m(name: &str, role: Role, params_b: f64, chosen: Level) -> Member {
        Member { name: name.into(), role, params: (params_b * 1e9) as u64, chosen }
    }

    #[test]
    fn the_LIAISON_gives_up_precision_before_anyone_else() {
        // The premise: a model that reports on a thinker does not need the thinker's precision, and
        // taking it from there frees room where the loss costs least.
        let panel = vec![
            m("thinker", Role::Thinker, 30.0, Level::Q4),
            m("liaison", Role::Liaison, 8.0, Level::Q4),
        ];
        let at_q4: u64 = panel.iter().map(|x| x.bytes_at(Level::Q4)).sum();
        let plan = fit(&panel, at_q4 - 2 * GIB);
        assert_eq!(plan.levels[0], Level::Q4, "the thinker is never reduced");
        assert!(plan.levels[1] < Level::Q4, "the liaison is");
        assert!(plan.saved_bytes > GIB);
    }

    #[test]
    fn it_reduces_ONE_STEP_at_a_time_rather_than_to_the_floor() {
        // Taking more than is needed spends quality for nothing — the same reason next_draft_len grows
        // by one rather than doubling.
        let panel = vec![
            m("thinker", Role::Thinker, 30.0, Level::Q4),
            m("liaison", Role::Liaison, 8.0, Level::Q4),
        ];
        let at_q4: u64 = panel.iter().map(|x| x.bytes_at(Level::Q4)).sum();
        // A budget one step clears. Q4->Q3 on 8B params frees 8e9*(4.5-3.4375)/8 = 0.99 GiB, so the
        // target has to be inside that — I first wrote `- GIB` and the planner correctly took a
        // SECOND step, which is the test being wrong about the arithmetic rather than the code.
        let plan = fit(&panel, at_q4 - 900_000_000);
        assert_eq!(plan.levels[1], Level::Q3, "one step, not straight to Q2");
        // And a budget that one step cannot clear does take the second.
        assert_eq!(fit(&panel, at_q4 - 2 * GIB).levels[1], Level::Q2);
    }

    #[test]
    fn a_DEBATER_has_a_higher_floor_than_a_liaison() {
        // A debater's position is the product of its turn and nothing downstream checks it; a
        // liaison's status line is read by a person who can see the backend themselves.
        assert_eq!(floor(Role::Liaison), Level::Q2);
        assert_eq!(floor(Role::Debater), Level::Q3);
        assert!(floor(Role::Debater) > floor(Role::Liaison));
    }

    #[test]
    fn a_DRAFT_model_may_go_as_low_as_a_liaison() {
        // Its output is verified — a wrong draft is rejected and costs one round, never a wrong
        // answer. That bounded cost is what earns it the lowest floor.
        assert_eq!(floor(Role::Draft), Level::Q2);
    }

    #[test]
    fn the_THINKER_is_never_reduced_even_when_nothing_else_fits() {
        // The precision of the model whose reasoning is the product is the operator's decision, and
        // this module does not get a vote.
        let panel = vec![m("thinker", Role::Thinker, 70.0, Level::Q4)];
        let plan = fit(&panel, GIB);
        assert_eq!(plan.levels[0], Level::Q4);
        assert!(plan.still_over, "and it says so rather than quietly lowering it");
        assert!(explain(&panel, &plan).contains("would have to come from the thinker"));
    }

    #[test]
    fn a_panel_that_already_fits_is_left_ALONE() {
        // Reducing precision nobody asked to reduce, to hit a budget already met, is a cost with no
        // benefit — and the explanation must say plainly that nothing changed.
        let panel = vec![
            m("thinker", Role::Thinker, 8.0, Level::Q4),
            m("liaison", Role::Liaison, 1.0, Level::Q4),
        ];
        let plan = fit(&panel, 100 * GIB);
        assert_eq!(plan.levels, vec![Level::Q4, Level::Q4]);
        assert_eq!(plan.saved_bytes, 0);
        assert!(explain(&panel, &plan).contains("fits at the level you chose"));
    }

    #[test]
    fn three_debaters_plus_a_thinker_is_the_case_this_exists_for() {
        // Three 8B debaters at q4 is ~13.5 GiB before the thinker loads. On a 31 GiB device that is
        // most of it, and the reduction is the difference between a resident thinker and a streaming
        // one.
        let panel = vec![
            m("thinker", Role::Thinker, 30.0, Level::Q4),
            m("a", Role::Debater, 8.0, Level::Q4),
            m("b", Role::Debater, 8.0, Level::Q4),
            m("liaison", Role::Liaison, 8.0, Level::Q4),
        ];
        let plan = fit(&panel, 24 * GIB);
        assert!(plan.saved_bytes > 2 * GIB, "freed {} bytes", plan.saved_bytes);
        assert_eq!(plan.levels[0], Level::Q4, "not from the thinker");
        // And the liaison went further than the debaters, because its floor is lower.
        assert!(plan.levels[3] <= plan.levels[1]);
    }

    #[test]
    fn the_explanation_says_what_was_TRADED_and_what_it_bought() {
        // A plan that silently lowers precision is one an operator cannot evaluate, and the premise is
        // that the trade is worth making for some roles and not others.
        let panel = vec![
            m("thinker", Role::Thinker, 30.0, Level::Q4),
            m("liaison", Role::Liaison, 8.0, Level::Q4),
        ];
        let at_q4: u64 = panel.iter().map(|x| x.bytes_at(Level::Q4)).sum();
        let e = explain(&panel, &fit(&panel, at_q4 - 2 * GIB));
        assert!(e.contains("liaison"));
        assert!(e.contains("freeing"));
        assert!(e.contains("reports rather than reasons"));
    }
}
