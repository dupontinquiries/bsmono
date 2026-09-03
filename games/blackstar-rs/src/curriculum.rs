//! CURRICULUM — what the campaign teaches, and the promise that the open
//! world never takes it away.
//!
//! The campaign's shape is Call of Duty's: authored set-piece missions that
//! introduce an extended mechanic set one piece at a time, each in a place
//! built to make that one thing legible. That structure is well understood
//! and it works.
//!
//! **Then it inverts.** A Call of Duty campaign graduates you into
//! multiplayer, which is a SUBSET — smaller maps, fewer verbs, most of the
//! campaign's toys gone. Here the campaign graduates you into a procedurally
//! generated open world that is a STRICT SUPERSET: everything you were taught
//! still works, in more places, forever. Destiny's patrol loop with Ark or
//! Rust's world generation underneath, rather than Destiny's fixed
//! destinations.
//!
//! That inversion is the whole design claim of the campaign, and it is the
//! kind of claim that quietly stops being true. A mechanic gets built for one
//! scripted mission and never wired into patrol; a systemic verb gets added to
//! the open world and nothing ever teaches it. Both are invisible until
//! somebody plays for six hours and feels the game get smaller.
//!
//! So it is a ledger with a lint, not a paragraph in a design doc:
//!
//! * every mechanic is INTRODUCED by exactly one mission — re-tutorialising is
//!   the specific sin this table exists to prevent;
//! * teaching order is monotone with mission order, so nothing is taught before
//!   the mission that assumes it;
//! * every mechanic is available in the open world. **There is no
//!   campaign-only verb.** A mechanic that fails this is either a design
//!   mistake or an admission the campaign is a demo;
//! * raids may DEEPEN a mechanic; they may never own one exclusively, because a
//!   verb you can only use in a six-player raid is not a verb the game has.
//!
//! ## What this does not decide
//!
//! Which mechanics exist. This is a ledger of the ones that do, and several
//! rows below name systems that are modules today rather than played features
//! (see the `built` flag). The lint checks CONSISTENCY, not completeness — it
//! cannot tell you the curriculum is good, only that it is coherent.

use crate::content::{Mission, MISSIONS};

/// Where a mechanic can be used. The campaign is the narrow end of the funnel
/// and everything else is wider; that ordering is the point.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Where {
    /// authored missions
    Campaign,
    /// procedural patrol — the destination the funnel opens onto
    OpenWorld,
    /// the raid ladder, which may deepen but never own
    Raid,
}

/// One thing the player learns to do.
pub struct Mechanic {
    pub id: &'static str,
    /// The mission that introduces it. Exactly one, by lint.
    pub taught_by: &'static str,
    /// Everywhere it can be used afterwards. Must include `OpenWorld`.
    pub used_in: &'static [Where],
    /// Is this a played feature today, or a module waiting to be wired?
    /// Honest bookkeeping: `docs/PLAN.md` exists because this project's
    /// failure mode is building systems nothing calls.
    pub built: bool,
    /// Which module owns it, so the ledger points at real code.
    pub module: &'static str,
    pub note: &'static str,
}

const ALL: &[Where] = &[Where::Campaign, Where::OpenWorld, Where::Raid];
const FIELD: &[Where] = &[Where::Campaign, Where::OpenWorld];

/// The ledger, in teaching order.
pub static CURRICULUM: &[Mechanic] = &[
    Mechanic {
        id: "transform",
        taught_by: "the_pluto_line",
        used_in: ALL,
        built: true,
        module: "xform.rs",
        note: "the fold itself: vehicle to robot and back. the first verb, taught \
               in a corridor where there is nothing else to do with it.",
    },
    Mechanic {
        id: "elements",
        taught_by: "the_scavenger_king",
        used_in: ALL,
        built: true,
        module: "weapons.rs",
        note: "seven elements and their matchups. introduced against an enemy \
               that punishes exactly one of them.",
    },
    Mechanic {
        id: "gear_rails",
        taught_by: "the_red_extraction",
        used_in: ALL,
        built: true,
        module: "gear.rs",
        note: "three weapon rails and worn armour. the extraction gives you more \
               than you can carry, which is the lesson.",
    },
    Mechanic {
        id: "flight",
        taught_by: "the_beach_opens",
        used_in: ALL,
        built: true,
        module: "physics.rs",
        note: "lift, and what a chassis with no lift cannot do. the shore tears \
               open and the ground stops being an option.",
    },
    Mechanic {
        id: "corruption",
        taught_by: "what_assay_grew",
        used_in: ALL,
        built: true,
        module: "corruption.rs",
        note: "the corrupted do not fight like troops. first time the player has \
               to read an enemy rather than out-shoot it.",
    },
    Mechanic {
        id: "survival_exposure",
        taught_by: "the_mantle_dive",
        used_in: FIELD,
        built: true,
        module: "survival.rs",
        note: "heat, pressure, and worlds a body cannot stand on. the dive is the \
               first place the environment is the opponent.",
    },
    Mechanic {
        id: "hydro",
        taught_by: "the_drowned_approach",
        used_in: FIELD,
        built: true,
        module: "hydro.rs, subsea.rs",
        note: "buoyancy and drag. movement everybody already knows, made \
               unfamiliar.",
    },
    Mechanic {
        id: "clinch",
        taught_by: "the_ashen_truce",
        used_in: ALL,
        built: false,
        module: "clinch.rs",
        note: "air-to-air grapples. the truce is a standoff, so the mission is the \
               one place a grapple beats a gun. NOT WIRED -- see docs/PLAN.md.",
    },
    Mechanic {
        id: "chi_attunement",
        taught_by: "beneath_the_pacific",
        used_in: ALL,
        built: true,
        module: "chi.rs",
        note: "elemental attunement, and the worthiness gate on the heavy kit.",
    },
    Mechanic {
        id: "space_bridge",
        taught_by: "the_far_gate",
        used_in: ALL,
        built: true,
        module: "exoworlds.rs, procgen.rs",
        note: "the funnel's mouth. this is the mission that ends the corridor and \
               opens the procedural sky -- every jump after it is a world nobody \
               authored.",
    },
    Mechanic {
        id: "clones",
        taught_by: "the_vault_of_ice",
        used_in: ALL,
        built: true,
        module: "clones.rs",
        note: "the vats, the lineages, and what a clone is for.",
    },
    Mechanic {
        id: "combine",
        taught_by: "the_tide_forges",
        used_in: ALL,
        built: true,
        module: "combine.rs",
        note: "side-by-side into one larger body. the forges are the only place \
               big enough to try it first. reachable via --combine; not in the \
               real-time loop.",
    },
    Mechanic {
        id: "voyage_logistics",
        taught_by: "the_tithe_road",
        used_in: FIELD,
        built: true,
        module: "voyage.rs, dispatch.rs",
        note: "delta-v, supplies, and a destination you cannot come back from. \
               the tithe road is a supply problem wearing a mission.",
    },
    Mechanic {
        id: "archaeology",
        taught_by: "the_ashes_court",
        used_in: ALL,
        built: true,
        module: "archive.rs, provenance.rs",
        note: "scanning, provenance, and the map that is built out of what you \
               carry home. taught here because boarding Blackstar's flagship is \
               the first time the player stands inside somebody else's records.",
    },
    Mechanic {
        id: "watch_forms",
        taught_by: "the_heart_of_the_god_machine",
        used_in: ALL,
        built: true,
        module: "watch.rs",
        note: "the bearer's forms and the ceiling nobody has mapped. last, because \
               it re-reads every mechanic above it.",
    },
];

// No `mechanic(id)` lookup. It existed for symmetry with every other table
// in this project and nothing ever wanted one mechanic by name — the ledger
// is read whole, by `taught_by`, `known_after` and `--tune kit`. A finder
// nobody calls is one more thing to keep correct.

/// What a given mission introduces. Most introduce exactly one thing; a
/// mission that introduces none is a mission that only advances the story,
/// which is allowed and sometimes correct.
pub fn taught_by(mission_id: &str) -> Vec<&'static Mechanic> {
    CURRICULUM.iter().filter(|m| m.taught_by == mission_id).collect()
}

/// Everything the player can do by the time a mission is cleared.
pub fn known_after(mission: &Mission) -> Vec<&'static Mechanic> {
    CURRICULUM
        .iter()
        .filter(|m| {
            MISSIONS
                .iter()
                .find(|x| x.id == m.taught_by)
                .map(|x| x.order <= mission.order)
                .unwrap_or(false)
        })
        .collect()
}

/// The whole point, as a function: what survives graduation.
///
/// If this ever returns fewer entries than `CURRICULUM`, the open world has
/// become a subset and the campaign has become a demo.
pub fn open_world_kit() -> Vec<&'static Mechanic> {
    CURRICULUM.iter().filter(|m| m.used_in.contains(&Where::OpenWorld)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_mechanic_is_taught_by_a_real_mission_exactly_once() {
        for m in CURRICULUM {
            assert!(
                MISSIONS.iter().any(|x| x.id == m.taught_by),
                "{} is taught by '{}', which is not a mission",
                m.id,
                m.taught_by
            );
        }
        // re-tutorialising is the sin this table exists to prevent
        let mut seen = BTreeSet::new();
        for m in CURRICULUM {
            assert!(seen.insert(m.taught_by), "{} re-teaches in the same mission", m.taught_by);
        }
        let ids: BTreeSet<_> = CURRICULUM.iter().map(|m| m.id).collect();
        assert_eq!(ids.len(), CURRICULUM.len(), "duplicate mechanic id");
    }

    #[test]
    fn nothing_is_campaign_only() {
        // THE inversion. A Call of Duty campaign graduates into a subset; this
        // one graduates into a superset, and that is only true if it is true
        // of every row.
        for m in CURRICULUM {
            assert!(
                m.used_in.contains(&Where::OpenWorld),
                "{} is campaign-only -- either wire it into patrol or cut it",
                m.id
            );
        }
        assert_eq!(
            open_world_kit().len(),
            CURRICULUM.len(),
            "the open world has become a subset of the campaign"
        );
    }

    #[test]
    fn raids_deepen_but_never_own() {
        for m in CURRICULUM {
            if m.used_in.contains(&Where::Raid) {
                assert!(
                    m.used_in.contains(&Where::OpenWorld),
                    "{} is raid-gated; a verb you need five other people for is not a \
                     verb the game has",
                    m.id
                );
            }
        }
    }

    #[test]
    fn the_curriculum_is_in_teaching_order() {
        // the table reads as a syllabus, so it should BE one -- otherwise the
        // order in the file quietly disagrees with the order in play
        let mut last = 0u8;
        for m in CURRICULUM {
            let mission = MISSIONS.iter().find(|x| x.id == m.taught_by).unwrap();
            assert!(
                mission.order >= last,
                "{} is listed after a later mission; the table is not a syllabus",
                m.id
            );
            last = mission.order;
        }
    }

    #[test]
    fn the_bridge_opens_the_funnel_and_most_things_precede_it() {
        // the campaign's structural hinge: the_far_gate is where authored
        // corridors end and the procedural sky starts. Most of the kit should
        // be taught BEFORE it, or the funnel opens onto a player who cannot
        // use it.
        let gate = MISSIONS.iter().find(|m| m.id == "the_far_gate").expect("the bridge mission");
        let before = CURRICULUM
            .iter()
            .filter(|m| {
                MISSIONS.iter().find(|x| x.id == m.taught_by).unwrap().order <= gate.order
            })
            .count();
        assert!(
            before * 2 >= CURRICULUM.len(),
            "only {before} of {} mechanics are taught by the time the bridge opens",
            CURRICULUM.len()
        );
    }

    #[test]
    fn known_after_accumulates_and_never_shrinks() {
        let mut last = 0;
        let mut ordered: Vec<_> = MISSIONS.iter().collect();
        ordered.sort_by_key(|m| m.order);
        for m in ordered {
            let n = known_after(m).len();
            assert!(n >= last, "the kit shrank at {}", m.id);
            last = n;
        }
        assert_eq!(last, CURRICULUM.len(), "the last mission does not leave the full kit");
    }

    #[test]
    fn the_unwired_rows_are_the_ones_plan_md_already_names() {
        // honest bookkeeping: this asserts the ledger's `built` flags match
        // what docs/PLAN.md's wiring-debt table says, so the two cannot drift
        let unbuilt: BTreeSet<_> = CURRICULUM.iter().filter(|m| !m.built).map(|m| m.id).collect();
        let expected: BTreeSet<_> = ["clinch"].into_iter().collect();
        assert_eq!(
            unbuilt, expected,
            "the built flags moved -- update docs/PLAN.md and STORY_BIBLE.md together"
        );
    }
}
