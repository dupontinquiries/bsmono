//! CYBERTRONIAN — a design grammar derived from a world, and frames built to it.
//!
//! ## Why this module exists, and a correction
//!
//! The census reported that the Foundry has fifty-six frames and not one of
//! them is anybody, and I read that as characterisation: a place where the
//! difference between one frame and another stopped mattering. It is not.
//! The fifty-six are `grimorph_part4` through `grimorph_part88` — **pieces of
//! a single exploded model**, listed in `bots.json` as if each were a bot.
//!
//! So the Foundry does not have fifty-six anonymous residents. It has **none**,
//! and a parts bin standing where they should be. That is a better reason to
//! design some than the one I invented, and `the_parts_bin_is_not_a_population`
//! keeps the finding from drifting back into lore.
//!
//! ## The grammar comes from the planet, not from a mood board
//!
//! Every other Machine design starts from "angular and mean". This one
//! starts from what this codebase already knows the planet IS, and the
//! answers fall out:
//!
//! | the world says | so a frame built there |
//! |---|---|
//! | `Material::Metal`, grip 1.0 — "purpose-built handholds everywhere" (`cling.rs`) | does not need wheels. Anything can climb, so wheels are a SPECIALISATION for prepared roads, not a default |
//! | no weather (`patrol::ROUTE`) | needs no sealing, no fairings, no weatherproofing. Open frames, exposed mechanism |
//! | `Sky::Lattice` — a built ceiling | has bounded flight. Vertical travel is climbing between levels, not soaring |
//! | nothing erodes it | keeps sharp edges and fine articulation forever |
//! | 8.5 m tall (`scale::LADDER`) | is measured against that, not against a car |
//!
//! Research agrees on the register — Machine alt-modes are angular and
//! unconstrained by Earth physics, and at home they carry armour openly
//! because there is nobody to hide from. It also names the failure mode
//! plainly: a bad Machine alt-mode "looks like a mishmash of robot
//! parts". `alt_mode_is_coherent` is that warning as a test.
//!
//! ## What these are not
//!
//! Original frames. Not ports of anyone. The catalogue happens to contain a
//! lot of third-party rips of copyrighted characters — see
//! `catalog_has_third_party_models` — and none of them is a reference here.
//! The grammar above is a set of physical consequences, and physical
//! consequences are not anybody's property.

use crate::cling::{surface_grip, Frame as GripFrame, Hold};
use crate::terrain::Material;

/// the Foundry's surface, from `mapsim`'s own mapping.
pub const SURFACE: Material = Material::Metal;
/// No weather at all, per the route table.
pub const HAS_WEATHER: bool = false;

/// How a frame gets about, given a world where everything is climbable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gait {
    /// wheels: fast on the prepared spans, useless off them. A CHOICE here,
    /// not a default — which is what makes it say something
    Rolling,
    /// legs that grip: the baseline a metal world actually rewards
    Striding,
    /// many small contacts; goes up walls without slowing
    Clinging,
    /// under the lattice, never above it
    Lofting,
    /// rides the rails the lattice already provides
    Railed,
}

impl Gait {
    pub fn all() -> [Gait; 5] {
        [Gait::Rolling, Gait::Striding, Gait::Clinging, Gait::Lofting, Gait::Railed]
    }
    pub fn name(self) -> &'static str {
        match self {
            Gait::Rolling => "rolling",
            Gait::Striding => "striding",
            Gait::Clinging => "clinging",
            Gait::Lofting => "lofting",
            Gait::Railed => "railed",
        }
    }
    /// Can it leave the prepared ground? The grammar's sharpest consequence:
    /// on a world with perfect grip, the wheeled frame is the constrained one.
    pub fn off_road(self) -> bool {
        self != Gait::Rolling
    }
    /// Does it climb walls without stopping to think about it?
    pub fn climbs(self) -> bool {
        matches!(self, Gait::Clinging | Gait::Railed)
    }
}

/// An original Machine frame.
pub struct Frame {
    pub id: &'static str,
    pub name: &'static str,
    pub gait: Gait,
    /// what it folds into. Never "a car with spikes"
    pub alt: &'static str,
    /// height in metres; `scale::LADDER` says a standard frame is 8.5
    pub height_m: f32,
    /// `chassis_traits` bulk, 0..1
    pub bulk: f32,
    /// what it is FOR, which on this world came before what it looks like
    pub role: &'static str,
    /// the line that makes it a person rather than a class
    pub hook: &'static str,
}

/// Twelve frames, built to the grammar above.
///
/// Varied along the axes the world actually provides — gait, scale, and
/// whether they predate the lattice — rather than along colour and attitude.
pub static FRAMES: &[Frame] = &[
    Frame {
        id: "spanwright",
        name: "SPANWRIGHT",
        gait: Gait::Railed,
        alt: "a gantry crawler that lays its own rail as it goes, and takes it up behind",
        height_m: 11.0,
        bulk: 0.80,
        role: "built the lattice, and is still extending it",
        hook: "it has not been told the war happened. it is four hundred years into a span \
               and it will finish.",
    },
    Frame {
        id: "kilnjack",
        name: "KILNJACK",
        gait: Gait::Striding,
        alt: "a walking forge — a crucible slung between four legs, pouring while it moves",
        height_m: 9.5,
        bulk: 0.88,
        role: "field repair, when the foundries stopped",
        hook: "it will mend anything that holds still. it does not check whose it is first, \
               and it has fixed enemies.",
    },
    Frame {
        id: "quietmile",
        name: "QUIETMILE",
        gait: Gait::Rolling,
        alt: "a low six-wheeled courier, built for the prepared spans and nothing else",
        height_m: 6.2,
        bulk: 0.30,
        role: "carried messages between districts before the relays",
        hook: "wheels on a world where everything grips: a boast. it can only use the roads, \
               and it is faster on them than anything alive.",
    },
    Frame {
        id: "hollowcant",
        name: "HOLLOWCANT",
        gait: Gait::Clinging,
        alt: "a flat many-legged plate that folds against a wall and stops looking like anything",
        height_m: 5.4,
        bulk: 0.35,
        role: "surveyed the underside of the lattice",
        hook: "it has spent so long against ceilings that upright reads as falling to it. \
               it fights better inverted.",
    },
    Frame {
        id: "toll",
        name: "TOLL",
        gait: Gait::Striding,
        alt: "a bell-frame: a resonant hollow drum on three legs, and the drum is the weapon",
        height_m: 13.0,
        bulk: 0.92,
        role: "district alarm, back when there was somebody to warn",
        hook: "it still sounds the hour. nothing left on the surface knows what the hour \
               was for, and it rings anyway.",
    },
    Frame {
        id: "shearline",
        name: "SHEARLINE",
        gait: Gait::Lofting,
        alt: "a bladed ring that flies by cutting — it rides the gap under the lattice on \
              edge, never above it",
        height_m: 7.8,
        bulk: 0.45,
        role: "cleared the lattice of what grew on it",
        hook: "the only frame that has seen the sky's underside from end to end, and it \
               will not say what is written up there.",
    },
    Frame {
        id: "gauge",
        name: "GAUGE",
        gait: Gait::Railed,
        alt: "a measuring carriage that runs the rails taking readings, arms out like calipers",
        height_m: 8.5,
        bulk: 0.55,
        role: "kept the lattice true",
        hook: "it has the only continuous record of the planet's shape. it does not know \
               that makes it the most valuable thing on the surface.",
    },
    Frame {
        id: "underwrit",
        name: "UNDERWRIT",
        gait: Gait::Clinging,
        alt: "a deep-shaft crawler, a segmented column that grips a bore on all sides at once",
        height_m: 15.0,
        bulk: 0.85,
        role: "went down to the substrate and came back",
        hook: "it is the only one that has been to the bottom. it will not go again and it \
               will not explain.",
    },
    Frame {
        id: "smallhours",
        name: "SMALLHOURS",
        gait: Gait::Striding,
        alt: "a two-legged frame barely taller than a human, all articulation and no armour",
        height_m: 2.4,
        bulk: 0.18,
        role: "worked inside the machines that were too small for anyone else",
        hook: "the only frame on the planet that can look a person in the eye, which nobody \
               designed on purpose and everybody has noticed since.",
    },
    Frame {
        id: "carriage_nine",
        name: "CARRIAGE NINE",
        gait: Gait::Railed,
        alt: "a passenger car — an actual one, with seats, from when the lattice moved crowds",
        height_m: 10.0,
        bulk: 0.70,
        role: "public transit",
        hook: "it remembers every route and has run none of them in an age. it still \
               announces the stops.",
    },
    Frame {
        id: "brace",
        name: "BRACE",
        gait: Gait::Striding,
        alt: "a load-bearing frame that folds into a pillar and holds a ceiling up",
        height_m: 12.5,
        bulk: 0.95,
        role: "temporary support during construction",
        hook: "somewhere under the third district it is still holding, and has been since \
               before the war. nobody has told it to stop.",
    },
    Frame {
        id: "offcut",
        name: "OFFCUT",
        gait: Gait::Clinging,
        alt: "not a designed frame at all — a working assembly of salvage that folds into a \
              heap indistinguishable from scrap",
        height_m: 4.6,
        bulk: 0.40,
        role: "none. it assembled itself",
        hook: "the first thing on the Foundry that nobody built. the parts bin stood up.",
    },
];

pub fn frame(id: &str) -> Option<&'static Frame> {
    FRAMES.iter().find(|f| f.id == id)
}

/// Would this frame's alt-mode read as a shape, or as a pile?
///
/// The research names the failure mode exactly — a weak Machine
/// alt-mode "looks like a mishmash of robot parts" — so the test for a good
/// one is that the description names a THING, not a collection of features.
/// Crude and it catches the actual mistake: an alt line that is a list of
/// parts has no noun at its head.
pub fn alt_mode_is_coherent(f: &Frame) -> bool {
    let a = f.alt.trim_start();
    // it must open by naming something, not by listing
    !a.starts_with("a set of")
        && !a.starts_with("an assortment")
        && a.split_whitespace().count() >= 4
        && f.alt.contains(' ')
}

/// Does the gait make sense on a metal world? The grammar, checkable.
pub fn suits_the_world(g: Gait) -> bool {
    // total grip means climbing is free, so a clinging frame is never a
    // strange choice here even though it would be on a dusty one
    let grip = surface_grip(SURFACE);
    match g {
        Gait::Clinging | Gait::Railed => grip > 0.8,
        // a lofting frame needs the lattice to fly under
        Gait::Lofting => true,
        // wheels are a specialisation, valid but constrained
        Gait::Rolling => true,
        Gait::Striding => true,
    }
}

// ------------------------------------------------------------- driver ---

pub fn run() {
    use crate::cling::hang_time_s;
    println!("CYBERTRONIAN FRAMES — designed from the planet\n");
    println!("  the world, as this codebase already describes it:");
    println!("    surface     {:?}, grip {:.2} — handholds everywhere", SURFACE, surface_grip(SURFACE));
    println!("    weather     {}", if HAS_WEATHER { "yes" } else { "none — nothing to seal against" });
    println!("    sky         a built lattice — a ceiling, so flight is bounded");
    println!("    a frame     {:.1} m, per scale::LADDER\n", crate::scale::sized("bot").unwrap().height_m);

    println!("  so wheels are a CHOICE here, not a default:\n");
    for g in Gait::all() {
        println!(
            "    {:<10} off-road {:<5} climbs {:<5} belongs here {}",
            g.name(),
            g.off_road(),
            g.climbs(),
            suits_the_world(g)
        );
    }

    println!("\n  TWELVE FRAMES\n");
    for f in FRAMES {
        // the id is how anything else refers to one, and `alt_mode_is_coherent`
        // is the research's own warning applied to my own writing
        let ok = if alt_mode_is_coherent(f) { "" } else { "   <- READS AS A PILE" };
        println!(
            "  {}  [{}]  ({:.1} m, {}, bulk {:.2}){ok}",
            f.name,
            f.id,
            f.height_m,
            f.gait.name(),
            f.bulk
        );
        println!("      alt: {}", wrap(f.alt, 66, 11));
        println!("      was: {}", f.role);
        println!("      {}\n", wrap(f.hook, 66, 6));
    }

    println!("  HOW LONG EACH CAN HOLD A CYBERTRONIAN WALL (cling.rs)\n");
    for f in FRAMES.iter().filter(|f| f.gait.climbs()) {
        let gf = GripFrame { bulk: f.bulk, crawler: f.gait == Gait::Clinging };
        let t = hang_time_s(Hold::Underside, gf, SURFACE).unwrap_or(0.0);
        println!("    {:<14} {:>6.1}s beneath an overhang", f.name, t);
    }
    println!("\n  metal is the best surface in the game for it, which is the point:");
    println!("  a clinging frame is unremarkable at home and terrifying anywhere else.");

    // and the one the human campaign needs, looked up by id rather than
    // found by scanning — because these are meant to be referred to
    if let Some(f) = frame("smallhours") {
        println!("\n  FOR THE HUMAN CAMPAIGN\n");
        println!("    {} is {:.1} m against a person's {:.2} m.", f.name, f.height_m,
            crate::scale::sized("human").unwrap().height_m);
        println!("    {}", wrap(f.hook, 68, 4));
    }
}

fn wrap(t: &str, w: usize, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let (mut out, mut col) = (String::new(), 0);
    for word in t.split_whitespace() {
        if col > 0 && col + 1 + word.len() > w {
            out.push('\n');
            out.push_str(&pad);
            col = 0;
        } else if col > 0 {
            out.push(' ');
            col += 1;
        }
        out.push_str(word);
        col += word.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_parts_bin_is_not_a_population() {
        // THE CORRECTION. The census's "fifty-six frames and not one is
        // anybody" was meaning read into a data artifact: those ids are
        // `grimorph_part4` .. `grimorph_part88`, pieces of one exploded model
        // listed as if each were a bot. the Foundry has no anonymous residents;
        // it has NONE, and a parts bin standing where they should be.
        let parts: Vec<&str> = crate::census::CENSUS
            .iter()
            .filter(|b| b.home == "foundry")
            .map(|b| b.id)
            .collect();
        assert!(!parts.is_empty());
        assert!(
            parts.iter().all(|id| id.starts_with("grimorph_part")),
            "something on foundry is no longer a model fragment — good, revisit this"
        );
        // and that is why the frames below had to be designed rather than found
        assert!(FRAMES.len() >= 8);
    }

    #[test]
    fn the_grammar_follows_from_the_world_the_code_already_describes() {
        // if the surface stopped being metal, half these designs would stop
        // making sense — so the derivation is checked, not asserted
        assert_eq!(SURFACE, Material::Metal);
        assert!(surface_grip(SURFACE) > 0.9, "the grammar assumes handholds everywhere");
        assert!(
            surface_grip(SURFACE) > surface_grip(Material::Regolith),
            "if metal were not the best surface, clinging frames would be strange"
        );
        for g in Gait::all() {
            assert!(suits_the_world(g), "{} does not belong here", g.name());
        }
    }

    #[test]
    fn wheels_are_the_constrained_choice_on_this_world() {
        // the sharpest consequence, and the one that makes a rolling frame
        // say something instead of being the default
        assert!(!Gait::Rolling.off_road());
        for g in Gait::all().into_iter().filter(|g| *g != Gait::Rolling) {
            assert!(g.off_road(), "{} is stuck on the roads too", g.name());
        }
    }

    #[test]
    fn no_alt_mode_is_a_mishmash_of_robot_parts() {
        // the failure mode the research names outright
        for f in FRAMES {
            assert!(alt_mode_is_coherent(f), "{} folds into a pile: {}", f.name, f.alt);
            assert!(!f.hook.is_empty() && f.hook.len() > 40, "{} has no character", f.name);
            assert!(!f.role.is_empty());
        }
    }

    #[test]
    fn they_are_varied_along_the_axes_the_world_provides() {
        // colour and attitude are not axes. Gait, scale and purpose are.
        let gaits: BTreeSet<&str> = FRAMES.iter().map(|f| f.gait.name()).collect();
        assert_eq!(gaits.len(), Gait::all().len(), "some gait went unused: {gaits:?}");

        let hs: Vec<f32> = FRAMES.iter().map(|f| f.height_m).collect();
        let (lo, hi) = (
            hs.iter().cloned().fold(f32::MAX, f32::min),
            hs.iter().cloned().fold(0.0, f32::max),
        );
        assert!(hi / lo > 5.0, "they are all the same size: {lo} to {hi}");

        let roles: BTreeSet<&str> = FRAMES.iter().map(|f| f.role).collect();
        assert_eq!(roles.len(), FRAMES.len(), "two frames were built for the same job");
        let ids: BTreeSet<&str> = FRAMES.iter().map(|f| f.id).collect();
        assert_eq!(ids.len(), FRAMES.len());
    }

    #[test]
    fn one_of_them_can_look_a_person_in_the_eye() {
        // the human campaign needs a frame that can share a room, and
        // `scale.rs` says what that means
        let human = crate::scale::sized("human").unwrap().height_m;
        let smallest = FRAMES.iter().min_by(|a, b| a.height_m.partial_cmp(&b.height_m).unwrap()).unwrap();
        assert!(
            smallest.height_m < human * 2.0,
            "the smallest frame is {} m against a {human} m person",
            smallest.height_m
        );
        // and the biggest is still a Machine, not a building
        let biggest = FRAMES.iter().map(|f| f.height_m).fold(0.0, f32::max);
        assert!(biggest < 30.0);
    }

    #[test]
    fn the_climbers_actually_climb_here() {
        // the designs claim clinging frames are unremarkable at home; check
        // that against `cling.rs` rather than trusting the prose
        use crate::cling::hang_time_s;
        for f in FRAMES.iter().filter(|f| f.gait == Gait::Clinging) {
            let gf = GripFrame { bulk: f.bulk, crawler: true };
            let home = hang_time_s(Hold::Underside, gf, SURFACE).unwrap();
            let away = hang_time_s(Hold::Underside, gf, Material::Regolith).unwrap();
            // COMPARATIVE, not a number I picked. The first version demanded
            // twenty seconds and HOLLOWCANT managed 16.8, which says nothing
            // about the design and everything about my threshold. What the
            // prose actually claims is that clinging is unremarkable HERE and
            // remarkable elsewhere, and that is two comparisons.
            let same_frame_not_clinging =
                hang_time_s(Hold::Underside, GripFrame { bulk: f.bulk, crawler: false }, SURFACE)
                    .unwrap();
            assert!(
                home > same_frame_not_clinging * 2.0,
                "{} gains little from clinging: {home}s vs {same_frame_not_clinging}s",
                f.name
            );
            assert!(home > away, "metal should be the easy surface");
        }
    }

    #[test]
    fn catalog_has_third_party_models_and_these_are_not_from_them() {
        // Worth a standing note rather than a comment. The representative
        // catalogue contains a great many rips of copyrighted characters
        // (`imported_both_transformers_fall_of_cybertron_*` and friends).
        // None of them is a reference for anything above: the grammar is a
        // set of physical consequences of a metal world with no weather, and
        // physical consequences are not anybody's property.
        //
        // The test asserts only the thing that matters here — that no frame
        // borrows a name from that pile.
        let borrowed = [
            "cleave", "soundwave", "grimlock", "ironhide", "ratchet", "jetfire", "starscream",
            "keelwright", "bumblebee", "shockwave", "warpath", "swoop", "slug", "airraid",
        ];
        for f in FRAMES {
            let low = f.id.to_lowercase();
            for b in borrowed {
                assert!(!low.contains(b), "{} borrows a name", f.id);
            }
        }
    }
}
