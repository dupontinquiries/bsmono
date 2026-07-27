//! pathfan.rs — `:paths`: fan out N different POTENTIAL FEATURE-CREATION PATHS for one feature idea.
//!
//! The sibling of the fuzz fan, aimed at planning instead of solving: where `:fuzzd` asks "solve
//! this N different ways", `:paths` asks "there are N different ways to BUILD this — walk one each".
//! Each branch is anchored to a distinct build-path archetype and must end in the same comparison
//! block, so the branches line up side by side in the spatial view and `:pick` / `:merge-docs`
//! apply unchanged.

pub struct PathArchetype {
    pub name: &'static str,
    pub blurb: &'static str,
    pub directive: &'static str,
}

pub const ARCHETYPES: &[PathArchetype] = &[
    PathArchetype { name: "mvp-first",
        blurb: "smallest usable version, then iterate",
        directive: "Plan the SMALLEST end-to-end version a user could actually use, shipped first, then the iteration ladder from there." },
    PathArchetype { name: "api-first",
        blurb: "nail the contract, then implement to it",
        directive: "Design the public API/contract first — signatures, formats, invariants — and plan the implementation as filling in behind a frozen interface." },
    PathArchetype { name: "data-first",
        blurb: "get the data model right, everything else follows",
        directive: "Start from the data: what is stored, its schema/ownership/migrations. Plan the feature as operations over that model." },
    PathArchetype { name: "test-first",
        blurb: "executable spec before any implementation",
        directive: "Write the acceptance tests / executable spec first. Plan the build as the shortest route to making them pass." },
    PathArchetype { name: "ux-spike",
        blurb: "fake the inside, prove the outside",
        directive: "Prototype the user-facing surface with stubs behind it to validate the experience before investing in internals." },
    PathArchetype { name: "risk-spike",
        blurb: "kill the scariest unknown first",
        directive: "Identify the single riskiest technical unknown and plan a spike that proves or kills it before anything else is built." },
];

/// The comparison block every path must end with — identical shape across branches so they read
/// side by side and merge cleanly.
pub const SUMMARY_BLOCK: &str = "\n\nEnd with exactly this block, filled in:\n\
## Path summary\n- First shippable milestone:\n- What this path deliberately defers:\n- Biggest risk on this path:\n- Rough effort to milestone:";

/// N path prompts for a feature, deterministically rotated by seed, capped at the archetype count.
pub fn prompts(feature: &str, n: usize, seed: u64) -> Vec<(&'static str, String)> {
    let n = n.min(ARCHETYPES.len()).max(1);
    (0..n).map(|i| {
        let a = &ARCHETYPES[(i + seed as usize) % ARCHETYPES.len()];
        (a.name, format!(
            "Feature to build: {feature}\n\nYou are exploring ONE possible way to create it — the \
'{}' path ({}). {}\nLay out the concrete plan for THIS path: ordered steps, what exists after \
each, and where it could go wrong.{}",
            a.name, a.blurb, a.directive, SUMMARY_BLOCK))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn n_paths_are_distinct_archetypes_with_the_summary_block() {
        let ps = prompts("offline sync for notes", 6, 0);
        let names: std::collections::BTreeSet<_> = ps.iter().map(|(n, _)| *n).collect();
        assert_eq!(names.len(), 6);
        for (_, p) in &ps {
            assert!(p.contains("offline sync for notes"));
            assert!(p.contains("## Path summary"), "comparable block present");
        }
    }
    #[test]
    fn deterministic_and_capped() {
        assert_eq!(prompts("x", 4, 9), prompts("x", 4, 9));
        assert_eq!(prompts("x", 99, 0).len(), ARCHETYPES.len());
    }
}
