//! persona.rs — principled divergence for fan-out.
//!
//! Today the swarm's branches diverge stochastically (temperature, fuzz directives), so they tend to jitter
//! around the same answer. Personas make divergence MEAN something: each branch is assigned a role — a
//! skeptic, a minimalist, a performance hound — whose directive is injected into its prompt, so the branches
//! explore genuinely different regions of the solution space. This is the same idea as the deployment's
//! model Roles, applied to BRANCHES; pair it with the draft/target split (cheap model proposes directions,
//! big model develops the promising ones) and you get broad, non-redundant exploration.

/// A branch role: a name plus the steering directive prepended to that branch's prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Persona {
    pub name: &'static str,
    pub directive: &'static str,
}

static ROSTER: &[Persona] = &[
    Persona { name: "builder",     directive: "Produce the most direct working solution. Prioritize correctness and getting it running over elegance." },
    Persona { name: "skeptic",     directive: "Assume the obvious approach is subtly wrong. Find the edge cases and failure modes first, then solve around them." },
    Persona { name: "minimalist",  directive: "Solve it with the least code and the fewest dependencies. Delete anything not strictly required." },
    Persona { name: "perf-hound",  directive: "Optimize for speed and low allocation. Prefer tight data layouts, avoid copies, measure the hot path." },
    Persona { name: "architect",   directive: "Design for change. Draw clean seams and types so the solution generalizes; explain the boundaries." },
    Persona { name: "pragmatist",  directive: "Reuse what already exists. Prefer promoted tools and standard-library over new machinery; compose, don't invent." },
];

/// The full roster (for UI / choosing an ensemble).
pub fn roster() -> &'static [Persona] { ROSTER }

/// Look up a persona by name (case-insensitive).
pub fn by_name(name: &str) -> Option<&'static Persona> {
    ROSTER.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

/// Assign a persona to the i-th branch of a fan-out, round-robin over the roster.
pub fn assign(branch_index: usize) -> &'static Persona {
    &ROSTER[branch_index % ROSTER.len()]
}

/// A diverse ensemble of `n` personas for an n-way fan — distinct while `n ≤ roster`, then wrapping. Order
/// front-loads the most complementary pair (builder + skeptic) so even a 2-way fan gets real tension.
pub fn ensemble(n: usize) -> Vec<&'static Persona> {
    (0..n).map(assign).collect()
}

/// Prepend a persona's directive to a base prompt (the injection point the fan uses).
pub fn steer(persona: &Persona, base_prompt: &str) -> String {
    format!("[role: {}] {}\n\n{}", persona.name, persona.directive, base_prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_is_distinct_and_looked_up() {
        let names: Vec<&str> = roster().iter().map(|p| p.name).collect();
        let mut uniq = names.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(names.len(), uniq.len(), "no duplicate persona names");
        assert_eq!(by_name("SKEPTIC").unwrap().name, "skeptic", "case-insensitive lookup");
        assert!(by_name("nonesuch").is_none());
    }

    #[test]
    fn assignment_is_round_robin() {
        let r = roster().len();
        assert_eq!(assign(0).name, roster()[0].name);
        assert_eq!(assign(r).name, roster()[0].name, "wraps at the roster length");
        assert_eq!(assign(1).name, roster()[1].name);
    }

    #[test]
    fn small_fan_gets_complementary_roles() {
        let two = ensemble(2);
        assert_eq!(two.len(), 2);
        assert_ne!(two[0].name, two[1].name, "a 2-way fan gets two different roles");
        // builder + skeptic is the front-loaded tension
        assert_eq!(two[0].name, "builder");
        assert_eq!(two[1].name, "skeptic");
        // an n ≤ roster fan is fully distinct
        let full = ensemble(roster().len());
        let mut ns: Vec<&str> = full.iter().map(|p| p.name).collect();
        ns.sort_unstable(); ns.dedup();
        assert_eq!(ns.len(), roster().len(), "distinct across a full fan");
    }

    #[test]
    fn steer_injects_the_directive() {
        let p = by_name("minimalist").unwrap();
        let steered = steer(p, "write a GGUF header parser");
        assert!(steered.starts_with("[role: minimalist]"));
        assert!(steered.contains(p.directive));
        assert!(steered.contains("write a GGUF header parser"));
    }
}
