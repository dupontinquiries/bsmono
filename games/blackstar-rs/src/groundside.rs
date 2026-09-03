//! GROUNDSIDE — named places, and the same city in two eras.
//!
//! Findings have been sited on a DESTINATION: seventeen of them say "earth"
//! and nothing more. That was fine while the archaeology layer was a table,
//! and it stops being fine the moment the player is expected to walk
//! somewhere. A destination is a planet. A place is an address.
//!
//! ## Why this is one module and not two
//!
//! Los Angeles is the strongest overlap the two campaigns have, and it is
//! physical rather than thematic. The human films are set there in the Black
//! Archive era; `the_beach_opens` (Film 1, mission 4) is set there in the
//! present, when the shore tears open under a kid with a skateboard. Same
//! coastline, two casts, fifteen years apart.
//!
//! So a place carries **two states**, not one. The bunker under the city is
//! an operational Directorate site in the archive era and four floors of damp
//! with a fifth floor missing from the plans in the present. The ridge above
//! the beach is an excavation in 1996 and a scar in the present. Standing in
//! the same spot in two campaigns and recognising it is the archaeology theme
//! made spatial, and it costs nothing extra: the evidence is already written.
//!
//! ## The thing the geography turned out to explain
//!
//! `farmhouse_photograph`, `trench_excavation_scar` and `thermal_plate` all
//! describe THE RIDGE, and together they put the Directorate's dig at 1998,
//! 1996 and 1994 against an official record of 2003. `the_beach_opens` puts
//! Unicron's fissure on the shore below that same ridge.
//!
//! Which answers a question the tables raised and never resolved: the record
//! is not falsified to hide a building. It is falsified because somebody was
//! digging toward the thing in the mantle a decade early, and the date is the
//! smallest lie that keeps anyone from asking what they were digging for.
//!
//! This module does not assert that. It places the evidence so a player can
//! reach it, and `RIDGE_OVERLOOKS_BEACH` keeps the two adjacent by test — the
//! inference is the player's, which is the whole design.
//!
//! ## Home base — and why only one era gets one
//!
//! This started as one building held by both casts, and the source overruled
//! it. The scene the human team is built out of is explicit: *old car, three
//! people inside, NO SECRET BASE, no holograms*. A safehouse with a wall of
//! monitors is exactly the thing that scene refuses, and it is the scene where
//! the audience is meant to fall in love with three people who do not yet
//! trust each other.
//!
//! So the archive era has no home base. It has a car, a case you carry, and a
//! scattered relay network — see `tradecraft.rs`, where the absence turned out
//! to be a better mechanic than the base would have been.
//!
//! The PRESENT keeps one. The bunker is Aiden's foothold: he is a kid who
//! stayed at a fissure, not an operative with a network, and the fixed
//! GTA-style home is right for him. Its Transformer-side twin is the ship
//! (`stations.rs`), which is why `Station::Chartroom` and this place do the
//! same job.
//!
//! That asymmetry is the point, not an omission, and the test below pins it.


/// Which of the two histories is standing here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Era {
    /// the human films: secret societies, roughly 1994-2009
    Archive,
    /// the Blackstar present: Unicron, the shield, the war
    Present,
}

/// A place you can stand.
pub struct Place {
    pub id: &'static str,
    /// the `patrol::ROUTE` destination it sits on
    pub dest: &'static str,
    pub name: &'static str,
    /// what it is in the archive era
    pub then: &'static str,
    /// what it is now
    pub now: &'static str,
    /// eras in which this is the campaign's home base. Deliberately not
    /// every era — see the module doc; the archive era has none.
    pub home_for: &'static [Era],
    /// places within walking reach — used for the ridge/beach adjacency that
    /// the story leans on
    pub near: &'static [&'static str],
    /// finding ids recoverable here. Cross-checked against `archive::FINDINGS`
    /// so this cannot drift from the evidence table.
    pub holds: &'static [&'static str],
    /// Campaign missions that put you at or beside this place.
    ///
    /// `Mission.site` names a locale in prose — "the beach", "the foundry" —
    /// and these are ids. Two namespaces for the same ground, with nothing
    /// joining them, which is why evidence was gated by DESTINATION: clearing
    /// any Earth mission opened all seventeen Earth findings, including an
    /// observatory the mission never went near. This is that join.
    pub reached_by: &'static [&'static str],
}

pub static PLACES: &[Place] = &[
    Place {
        id: "the_bunker",
        dest: "earth",
        reached_by: &["what_assay_grew"],
        name: "THE BUNKER",
        then: "an operational site nobody files paperwork about. four floors of \
               filing, and a fifth the plans do not admit to.",
        now: "damp, dark, and structurally sound, which is more than the rest of \
              the block can say. somebody left the lights wired.",
        home_for: &[Era::Present],
        near: &["the_port"],
        holds: &["bunker_alloy", "bunker_roster", "rotation_ledger"],
    },
    Place {
        id: "the_ridge",
        dest: "earth",
        reached_by: &["the_beach_opens"],
        name: "THE RIDGE",
        then: "an excavation with a fence around it and a date on the paperwork \
               that does not match the spoil heap.",
        now: "a scar above the water. whatever they were digging toward, the \
              ocean got to first.",
        home_for: &[],
        near: &["the_beach", "the_farmhouse"],
        holds: &["directorate_record", "trench_excavation_scar", "thermal_plate"],
    },
    Place {
        id: "the_beach",
        dest: "earth",
        reached_by: &["the_beach_opens"],
        name: "THE SHORE",
        then: "a beach. nothing has happened here. that is the point of visiting \
               it in this era.",
        now: "the fissure. the ground took a breath and did not let it out.",
        home_for: &[],
        near: &["the_ridge"],
        holds: &["core_resonance", "buried_twin"],
    },
    Place {
        id: "the_farmhouse",
        dest: "earth",
        reached_by: &["the_beach_opens"],
        name: "THE FARMHOUSE",
        then: "somebody lives here, and can see the fence from the kitchen \
               window.",
        now: "nobody has lived here since. the drawers were never emptied.",
        home_for: &[],
        near: &["the_ridge"],
        holds: &["farmhouse_photograph"],
    },
    Place {
        id: "the_ground_station",
        dest: "earth",
        reached_by: &["what_assay_grew"],
        name: "THE GROUND STATION",
        then: "a dish pointed at things that are not in the ephemeris, on a \
               programme that will be terminated in 2009.",
        now: "a dish full of birds, and one drive that still spins.",
        home_for: &[],
        near: &[],
        holds: &[
            "station_drive", "station_pointing_log", "cold_war_manifest",
            "undelivered_manifest", "kesswick_relay_log",
        ],
    },
    Place {
        id: "the_museum",
        dest: "earth",
        reached_by: &["what_assay_grew"],
        name: "THE MUSEUM",
        then: "a regional collection with a case near the fire exit and no alarm \
               on it.",
        now: "the same case. the same label. sixty years and nobody has taken it.",
        home_for: &[],
        near: &[],
        holds: &["museum_specimen"],
    },
    Place {
        id: "the_port",
        dest: "earth",
        reached_by: &["the_drowned_approach"],
        name: "THE CONTAINER PORT",
        then: "cranes, night shifts, and an insurance claim that will keep one \
               tape from being wiped.",
        now: "running dark. the ships still come in; nobody unloads them.",
        home_for: &[],
        near: &["the_bunker"],
        holds: &["port_reel"],
    },
    Place {
        id: "the_clinic",
        dest: "earth",
        reached_by: &["what_assay_grew"],
        name: "THE CLINIC",
        then: "a records room with one filing cabinet that locks separately.",
        now: "the cabinet is still locked. the name has been cut out of the file \
              anyway.",
        home_for: &[],
        near: &[],
        holds: &["interface_autopsy"],
    },
    Place {
        id: "the_observatory",
        dest: "earth",
        reached_by: &["the_mantle_dive"],
        name: "THE OBSERVATORY",
        then: "a working dome, an argument about a trajectory, and a room that \
               goes quiet.",
        now: "the dome is shut. the working is still on the board.",
        home_for: &[],
        near: &[],
        holds: &["star_chart_return", "chart_observation_arc", "modern_solution"],
    },
];

pub fn place(id: &str) -> Option<&'static Place> {
    PLACES.iter().find(|p| p.id == id)
}

/// Has the player been to this place — cleared a mission that reaches it?
pub fn visited(p: &Place, camp: &crate::campaign::CampaignLog) -> bool {
    p.reached_by.iter().any(|m| camp.is_cleared(m))
}

/// Where a finding is, more precisely than its destination.
pub fn place_of(finding_id: &str) -> Option<&'static Place> {
    PLACES.iter().find(|p| p.holds.contains(&finding_id))
}

/// The home base for an era. Exactly one, by test.
pub fn home(era: Era) -> Option<&'static Place> {
    PLACES.iter().find(|p| p.home_for.contains(&era))
}

impl Era {
    /// Which campaign the config says is running.
    pub fn from_cfg(c: &crate::config::Config) -> Era {
        if c.story.era >= 0.5 {
            Era::Archive
        } else {
            Era::Present
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Era::Archive => "THE BLACK ARCHIVE ERA",
            Era::Present => "THE BLACKSTAR PRESENT",
        }
    }
    /// Does discovery go through a case and a relay, or straight home to the
    /// ship? The one rule that separates the two campaigns' loops.
    pub fn relays(self) -> bool {
        self == Era::Archive
    }
}

impl Place {
    /// What this place is, to whoever is standing in it.
    pub fn as_of(&self, era: Era) -> &'static str {
        match era {
            Era::Archive => self.then,
            Era::Present => self.now,
        }
    }
    pub fn is_home(&self, era: Era) -> bool {
        self.home_for.contains(&era)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::MISSIONS;
    use crate::patrol::ROUTE;

    /// The adjacency the story leans on: the Directorate's dig looks down on
    /// the shore that opens in Film 1. A named constant so the test that
    /// keeps them together says WHY — `near` alone reads as geography, and
    /// the reason these two must not drift apart is that separating them
    /// removes the motive for the falsified excavation date.
    const RIDGE_OVERLOOKS_BEACH: (&str, &str) = ("the_ridge", "the_beach");
    use std::collections::BTreeSet;

    #[test]
    fn every_place_sits_on_a_real_destination_and_has_both_eras() {
        for p in PLACES {
            assert!(ROUTE.iter().any(|d| d.id == p.dest), "{} is on unknown dest {}", p.id, p.dest);
            // a place with only one era is a set, not a place: the whole
            // reason this module exists is that the city is held twice
            assert!(!p.then.is_empty() && !p.now.is_empty(), "{} is missing an era", p.id);
            assert_ne!(p.then, p.now, "{} reads identically in both eras", p.id);
            for n in p.near {
                assert!(place(n).is_some(), "{} is near unknown place {n}", p.id);
            }
        }
        let ids: BTreeSet<_> = PLACES.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), PLACES.len(), "duplicate place id");
    }

    #[test]
    fn nearness_is_mutual() {
        for p in PLACES {
            for n in p.near {
                let other = place(n).unwrap();
                assert!(
                    other.near.contains(&p.id),
                    "{} says it is near {}, but not the other way round",
                    p.id,
                    n
                );
            }
        }
    }

    #[test]
    fn every_earth_finding_has_an_address() {
        // the gap this module closes: seventeen findings that said only "earth"
        for f in crate::archive::FINDINGS.iter().filter(|f| f.site == "earth") {
            assert!(
                place_of(f.id).is_some(),
                "{} is on earth with no address -- add it to a place in groundside.rs",
                f.id
            );
        }
    }

    #[test]
    fn no_place_holds_a_finding_that_is_somewhere_else() {
        for p in PLACES {
            for id in p.holds {
                let f = crate::archive::FINDINGS
                    .iter()
                    .find(|f| f.id == *id)
                    .unwrap_or_else(|| panic!("{} holds unknown finding {id}", p.id));
                assert_eq!(f.site, p.dest, "{} holds {id}, which is sited on {}", p.id, f.site);
            }
        }
        // and nothing is in two places at once
        let mut seen = BTreeSet::new();
        for p in PLACES {
            for id in p.holds {
                assert!(seen.insert(*id), "{id} is held by two places");
            }
        }
    }

    #[test]
    fn the_dig_looks_down_on_the_fissure() {
        // the load-bearing adjacency. the excavation ladder (1998/1996/1994)
        // is at the ridge; Unicron's shore opens below it. separate these two
        // and the falsified date stops having a reason.
        let (ridge, beach) = RIDGE_OVERLOOKS_BEACH;
        let r = place(ridge).expect("the ridge");
        assert!(r.near.contains(&beach), "the ridge no longer overlooks the shore");

        // all three contradicting dates live at the ridge
        for id in ["trench_excavation_scar", "thermal_plate"] {
            assert_eq!(place_of(id).unwrap().id, ridge, "{id} moved off the ridge");
        }
        // and the farmhouse that sees the fence is next to it
        assert!(place(ridge).unwrap().near.contains(&"the_farmhouse"));

        // the beach is a real mission site in the present
        assert!(
            MISSIONS.iter().any(|m| m.dest == "earth" && m.id == "the_beach_opens"),
            "the shore mission is gone; the ridge overlooks nothing"
        );
    }

    #[test]
    fn the_present_has_a_home_and_the_archive_era_deliberately_does_not() {
        // "no secret base" is a line in the source, not an oversight here.
        // if the archive era ever grows a home base, that scene is gone and
        // somebody should have to argue for it.
        assert!(
            home(Era::Archive).is_none(),
            "the archive era grew a safehouse; the car scene says it should not have one"
        );
        let present: Vec<_> = PLACES.iter().filter(|p| p.is_home(Era::Present)).collect();
        assert_eq!(present.len(), 1, "the present has {} home bases", present.len());
        assert!(!present[0].holds.is_empty(), "a home worth returning to is worth searching");

        // and the era without a base must have somewhere to work instead
        assert!(
            crate::tradecraft::RELAYS
                .iter()
                .any(|r| crate::tradecraft::available(r, Era::Archive)),
            "no base AND no relays leaves the archive era with no way to report"
        );
    }

    #[test]
    fn every_place_is_reachable_by_a_real_mission() {
        // the join: `Mission.site` is prose ("the beach") and these are ids
        // ("the_beach"), so nothing connected them and nothing could.
        use crate::content::MISSIONS;
        for p in PLACES {
            assert!(
                !p.reached_by.is_empty(),
                "{} can never be visited, so its evidence can never be examined",
                p.id
            );
            for m in p.reached_by {
                assert!(
                    MISSIONS.iter().any(|x| x.id == *m),
                    "{} is reached by '{m}', which is not a mission",
                    p.id
                );
            }
        }
    }

    #[test]
    fn a_mission_does_not_reach_every_place_at_once() {
        // the property the destination-level gate violated
        use crate::content::MISSIONS;
        for m in MISSIONS.iter().filter(|m| m.dest == "earth") {
            let reached = PLACES.iter().filter(|p| p.reached_by.contains(&m.id)).count();
            assert!(reached > 0, "{} reaches nowhere", m.id);
            assert!(
                reached < PLACES.len(),
                "{} reaches all {} places, which is the coarse gate again",
                m.id,
                PLACES.len()
            );
        }
    }

    #[test]
    fn exactly_one_discovery_loop_runs_at_a_time() {
        // the incoherence this setting fixed: `--arch find` and `--arch carry`
        // are the two campaigns' mutually exclusive verbs and both worked at
        // once, because nothing recorded which campaign was running.
        let mut c = crate::config::Config::default();
        assert_eq!(Era::from_cfg(&c), Era::Present);
        assert!(!Era::from_cfg(&c).relays(), "the present should not use a case");

        c.load_line("story_era", "1");
        assert_eq!(Era::from_cfg(&c), Era::Archive);
        assert!(Era::from_cfg(&c).relays(), "the archive era must relay");

        // and the era that relays is the one with no home base to carry to
        assert!(home(Era::Archive).is_none());
        assert!(home(Era::Present).is_some());
        assert!(!Era::Present.relays(), "a ship makes relaying pointless");
    }

    #[test]
    fn as_of_reads_the_right_era() {
        let b = place("the_beach").unwrap();
        assert_eq!(b.as_of(Era::Archive), b.then);
        assert_eq!(b.as_of(Era::Present), b.now);
        assert!(b.as_of(Era::Present).contains("fissure"));
    }
}
