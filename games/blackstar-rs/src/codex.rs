//! THE CODEX — the settled reference.
//!
//! Fifty-four entries carried out of the C engine's `exengine/lore.c` before
//! it is retired. Original writing themed on the six-film arc — its own
//! header says so, and no source text is reproduced in it or here.
//!
//! It predates the Rust story bible, which makes it source rather than
//! derivative: the Archivist appears here, in `appendix: hybrid protocol`,
//! addressed directly, before anything in `archive.rs` existed.
//!
//! ## Reference, not reward
//!
//! `docs/STORY_BIBLE.md` carried this as an open question — whether the codex
//! should become something you excavate, dimming until earned, or stay a
//! reliable reference. Giving every entry a claim id and gating on it was the
//! obvious move and it is the wrong one: the codex is where you go to check
//! something, and a reference you have to unlock is a reference you stop
//! trusting.
//!
//! So entries are ANNOTATED, never gated. Five of the fifty-four name a claim
//! the archaeology layer tracks, and when the evidence has moved, the codex
//! says what the evidence currently says beside what the entry says. The
//! entry is readable either way.
//!
//! Five, deliberately. An annotation that fires on everything is decoration,
//! and the interesting case is the ONE entry the evidence disagrees with.

/// One codex entry. `lines` are pre-broken for a narrow column, as they were
/// in C — the breaks are the writing's own rhythm, not word wrap.
pub struct Entry {
    pub id: &'static str,
    pub title: &'static str,
    /// A claim in `archive::CLAIMS` this entry concerns, if any. Annotation
    /// only: nothing here is ever hidden.
    pub claim: Option<&'static str>,
    pub lines: &'static [&'static str],
}

pub static CODEX: &[Entry] = &[
    Entry {
        id: "apex_resonance",
        title: "apex: resonance",
        claim: None,
        lines: &[
            "the devourer keeps a rhythm. every nine seconds a pulse:",
            "a field that freezes the joints, a hurled ruin, a fan of",
            "fire, a well that drags you in. learn the count and live.",
        ],
    },
    Entry {
        id: "the_frontier_weave",
        title: "the frontier weave",
        claim: None,
        lines: &[
            "nanotech cloth with a memory of its shape. it drinks",
            "radiation, holds warmth, and goes quiet on sensors.",
            "out past the belt, the cloak is the difference between",
            "a patrol and a funeral.",
        ],
    },
    Entry {
        id: "the_under_garden",
        title: "the under-garden",
        claim: None,
        lines: &[
            "assay kept his failures. fed them. below the garden",
            "of helix the roof is alive, and it remembers how to",
            "hold on. do not trust a ceiling you cannot see.",
        ],
    },
    Entry {
        id: "the_eye_beneath",
        title: "the eye beneath",
        claim: None,
        lines: &[
            "it does not hunt. it does not need to. it watches the",
            "shore the way the deep watches a swimmer -- patient,",
            "certain, already home. and sometimes, it blinks.",
        ],
    },
    Entry {
        id: "i_doomsday",
        title: "I. doomsday",
        claim: None,
        lines: &[
            "a sleeping devourer stirs beneath the world.",
            "the first war is not for territory but for whether",
            "anyone survives to fight the next one.",
        ],
    },
    Entry {
        id: "ii_dominion",
        title: "II. dominion",
        claim: None,
        lines: &[
            "the tyrant does not want to rule you.",
            "he wants you to believe you chose him.",
            "order offered as a cage with the door left open.",
        ],
    },
    Entry {
        id: "iii_destiny",
        title: "III. destiny",
        claim: None,
        lines: &[
            "a leader becomes what the moment demands,",
            "and pays for it. the door to the wider cosmos opens.",
            "coordinates arrive from a hand that has waited an age.",
        ],
    },
    Entry {
        id: "iv_first_light",
        title: "IV. first light",
        claim: None,
        lines: &[
            "outside the shield the rules are different.",
            "wonder and dread share the same dark.",
            "the watch bearer trusts no one yet -- and must anyway.",
        ],
    },
    Entry {
        id: "v_solar",
        title: "V. solar",
        claim: None,
        lines: &[
            "a fire clone holds a star in his palm,",
            "and asks why you lead as if apologizing for it.",
            "some weapons are too large to aim.",
        ],
    },
    Entry {
        id: "vi_photon",
        title: "VI. photon",
        claim: None,
        lines: &[
            "the watch has a ceiling no one has found.",
            "for forty seconds once, it became light itself.",
            "the message home is simple: it is beautiful out here.",
        ],
    },
    Entry {
        id: "the_watch",
        title: "the watch",
        claim: None,
        lines: &[
            "eleven reliable forms, and one that is not.",
            "found, not made, at the last place the makers worked.",
            "it answers to one bearer, and to no other.",
        ],
    },
    Entry {
        id: "the_corrupted",
        title: "the corrupted",
        claim: None,
        lines: &[
            "unending's tissue hijacks the will to transform.",
            "no fixed shape, ever. they hunt by spark resonance,",
            "so stillness will not hide you. only the sparkless walk free.",
        ],
    },
    Entry {
        id: "the_hijacks",
        title: "the hijacks",
        claim: None,
        lines: &[
            "they do not conquer. they gather data.",
            "erratic, endless, indifferent to their own losses --",
            "sending fifty where three would do, and not caring.",
        ],
    },
    Entry {
        id: "the_gravity_leap",
        title: "the gravity leap",
        claim: None,
        lines: &[
            "an ice clone folds the cold and steps across light-years.",
            "no flash, no sound -- only a wave that arrives after,",
            "indifferent to who is standing where it lands.",
        ],
    },
    Entry {
        id: "the_array",
        title: "the array",
        claim: Some("edge_lattice"),
        lines: &[
            "a rotating crystal sphere aboard the enemy flagship.",
            "it watched a world's mantle for decades, then cracked",
            "the day the old legends turned out to be true.",
        ],
    },
    Entry {
        id: "the_boundary",
        title: "the boundary",
        claim: Some("edge_lattice"),
        lines: &[
            "at the edge of the system, a lattice woven from light.",
            "too faint to name on the first pass. flagged on the second.",
            "old enough that even a devourer's echo turns toward it.",
        ],
    },
    Entry {
        id: "the_deep_outpost",
        title: "the deep outpost",
        claim: None,
        lines: &[
            "they descended in the dark and split up -- wrong choice.",
            "a comrade stood still, facing away, and turned too far.",
            "the thing below was slow. the thing above was already up.",
        ],
    },
    Entry {
        id: "the_morpher",
        title: "the morpher",
        claim: None,
        lines: &[
            "the ice-clone watch bearer, glimpsed at the boundary.",
            "one watch, every elemental form -- and in cold form,",
            "the quantum leap no other clone can perform.",
        ],
    },
    Entry {
        id: "billy_the_fire_clone",
        title: "billy, the fire clone",
        claim: None,
        lines: &[
            "he holds the power of a star in his open hand.",
            "absorb one; fire it. a last resort even the clones",
            "are careful when to spend -- the cost is cosmic.",
        ],
    },
    Entry {
        id: "photon",
        title: "photon",
        claim: None,
        lines: &[
            "not an element the clone races ever had.",
            "the one watch form that transcends them all --",
            "solid light, and a ceiling no one has found.",
        ],
    },
    Entry {
        id: "the_reclaim",
        title: "the reclaim",
        claim: None,
        lines: &[
            "an overgrown world of organic-metal corridors.",
            "three long lanes run its length, and slow gun-plinths",
            "sweep them -- outrun their aim or die in the open.",
        ],
    },
    Entry {
        id: "the_corridors",
        title: "the corridors",
        claim: None,
        lines: &[
            "supply ships once ran these lanes at full burn.",
            "interceptor raiders learned to strike from the jungle,",
            "where the shear-corridor funnels everything into a kill-box.",
        ],
    },
    Entry {
        id: "assay",
        title: "assay",
        claim: None,
        lines: &[
            "the signal architect. never afraid, never cruel --",
            "he calls the horror a data point and revises the model.",
            "the thing he grew does not recognize its own maker.",
        ],
    },
    Entry {
        id: "blackstar",
        title: "blackstar",
        claim: None,
        lines: &[
            "right about the danger. wrong about the solution.",
            "he is not redeemed and he is not defeated --",
            "he is left holding both truths at once, and walks away.",
        ],
    },
    Entry {
        id: "keelwright",
        title: "keelwright",
        claim: None,
        lines: &[
            "he stops believing himself eternal, and begins",
            "believing in consequence. not a savior -- a steward,",
            "whose last duty is to leave his heirs stronger than he.",
        ],
    },
    Entry {
        id: "halyard",
        title: "halyard",
        claim: None,
        lines: &[
            "the heir who leads as if apologizing for it,",
            "until a fire clone asks him who taught him that.",
            "he follows the coordinates out past the boundary.",
        ],
    },
    Entry {
        id: "aiden",
        title: "aiden",
        claim: None,
        lines: &[
            "first human to speak for earth beyond the shield.",
            "five films of disaster behind him, one line home:",
            "it is beautiful out here. you would not believe it.",
        ],
    },
    Entry {
        id: "unending",
        title: "unending",
        claim: Some("unending_in_earth"),
        lines: &[
            "not a villain -- an event. a devourer the matrix",
            "drove dormant but never silenced. his neural remnants",
            "still lace the mantle and ocean floor, waiting to be found.",
        ],
    },
    Entry {
        id: "the_technocrats",
        title: "the technocrats",
        claim: None,
        lines: &[
            "archivists, navigators, economists of energy --",
            "a council that built a thing it did not understand,",
            "sealed the blackstar data, and called it coincidence.",
        ],
    },
    Entry {
        id: "foundry",
        title: "foundry",
        claim: Some("architecture_is_machine"),
        lines: &[
            "it stood for millions of cycles, and mistook",
            "endurance for destiny. rule by restricted data tier;",
            "only the keelwright-line held the oldest secret of all.",
        ],
    },
    Entry {
        id: "the_clone_coalition",
        title: "the clone coalition",
        claim: None,
        lines: &[
            "elemental nations older than foundry's golden age,",
            "each a people of a single power -- ice, fire, metal --",
            "holding the shield's perimeter against the hijacks.",
        ],
    },
    Entry {
        id: "the_helion_alliance",
        title: "the helion alliance",
        claim: None,
        lines: &[
            "humans who have lived inside the shield the longest,",
            "and so read its interior as no machine can.",
            "the bearer trusts them a shade more than the rest.",
        ],
    },
    Entry {
        id: "earth",
        title: "earth",
        claim: Some("convergence_was_not_first"),
        lines: &[
            "warm gold and noise and life -- pluto's opposite.",
            "beneath the crust, a metallic hand sinking, geological,",
            "and the sealed truth the keelwright-line always carried.",
        ],
    },
    Entry {
        id: "the_watch_forms",
        title: "the watch forms",
        claim: None,
        lines: &[
            "ice, water, tsunami. fire, magma, blizzard.",
            "metal, titanium, electricity. wind, tornado --",
            "eleven mapped. then the door with no handle: light.",
        ],
    },
    Entry {
        id: "the_matter_planet",
        title: "the matter planet",
        claim: None,
        lines: &[
            "the last place the makers worked -- where the watch",
            "was found, not made. it answers to one bearer alone,",
            "and holds a form even he cannot choose to become.",
        ],
    },
    Entry {
        id: "cryotek",
        title: "cryotek",
        claim: None,
        lines: &[
            "the cold open: an ice-base sundered holding the outer rim,",
            "ice-plasma and frozen shards, mining foundry's dead fragments.",
            "sam ends him early -- the first skirmish, and a statement of tone.",
        ],
    },
    Entry {
        id: "hot_rod",
        title: "jib",
        claim: None,
        lines: &[
            "a reckless young scout in earth orbit who idolizes keelwright.",
            "he dives the ruptured mantle, and at the worst moment",
            "takes up the matrix -- and rises out of it as halyard.",
        ],
    },
    Entry {
        id: "cleave",
        title: "cleave",
        claim: None,
        lines: &[
            "drawn to the valence pulse, he returns changed --",
            "hardened, reflective. he has watched the devourer eat",
            "even his own kind, and for the first time offers a truce.",
        ],
    },
    Entry {
        id: "the_matrix",
        title: "the matrix",
        claim: None,
        lines: &[
            "a relic of leadership that will not open on command.",
            "it opens for the one who reaches without wanting to,",
            "and answers the devourer with blinding gold.",
        ],
    },
    Entry {
        id: "the_sundered",
        title: "the sundered",
        claim: None,
        lines: &[
            "conquest was the old war. this one is different --",
            "the devourer consumes even them. some hold the line;",
            "one exile turns predator at the system's dark edge.",
        ],
    },
    Entry {
        id: "the_spearcons",
        title: "the spearcons",
        claim: None,
        lines: &[
            "cryotek's lieutenants -- shardclaw and icerender.",
            "sticky spears that cling where they strike and count down;",
            "delayed detonation turns a debris field into a ticking clock.",
        ],
    },
    Entry {
        id: "forgeheart",
        title: "forgeheart",
        claim: None,
        lines: &[
            "an enduring who fights the terrain itself: flips a spinning",
            "asteroid to crush an ambush, raises debris into cover.",
            "heavy, deliberate, and hard to move once he sets his feet.",
        ],
    },
    Entry {
        id: "the_solar_engine",
        title: "the solar engine",
        claim: None,
        lines: &[
            "mercury relays. venus tempers air. earth holds the core.",
            "mars strips metal; the outer moons refine; pluto contains.",
            "the whole system was one machine. we just lived on its parts.",
        ],
    },
    Entry {
        id: "the_reassembler",
        title: "the reassembler",
        claim: None,
        lines: &[
            "unending was never built to destroy. he was built to rebuild --",
            "break dead worlds down, reforge them into living ones.",
            "a god-machine. the makers simply lost the controls.",
        ],
    },
    Entry {
        id: "sam_witwicky",
        title: "sam witwicky",
        claim: None,
        lines: &[
            "the human who has stood with the enduring longest.",
            "he builds the interface, keeps the link to old lore,",
            "and mentors the one who will carry it forward.",
        ],
    },
    Entry {
        id: "the_asteroid_graveyard",
        title: "the asteroid graveyard",
        claim: None,
        lines: &[
            "between mars and jupiter, a belt of broken war --",
            "foundry's scattered fragments, dead outposts, drifting hulls.",
            "when the devourer woke, the whole belt woke with it.",
        ],
    },
    Entry {
        id: "the_containment",
        title: "the containment",
        claim: None,
        lines: &[
            "they could not kill him, so they caged him instead --",
            "earth for the head, the rim for the spine, pluto for the field.",
            "the field is old now. as he stirs, the cage starts to give.",
        ],
    },
    Entry {
        id: "dossier_cryotek",
        title: "dossier: cryotek",
        claim: None,
        lines: &[
            "archive enforcement, technocracy era. deep purple",
            "oxidation over hard geometric lines -- no flashy parts.",
            "speaks in clipped logic; never monologues. believes",
            "foundry fell when it abandoned purity. he wants the",
            "buried cores. everything else is an obstacle.",
        ],
    },
    Entry {
        id: "dossier_blackstar",
        title: "dossier: blackstar",
        claim: None,
        lines: &[
            "a naval commander scattered into deep orbit by the",
            "bridge -- he did not return. he adapted. the debris",
            "field became a kingdom. 'empires burn. i collect the",
            "ashes.' his array saw the devourer first; no one",
            "wanted to look. the name was earned, not given.",
        ],
    },
    Entry {
        id: "dossier_assay",
        title: "dossier: assay",
        claim: None,
        lines: &[
            "he helped design the control interface, under the",
            "technocrats, and regrets nothing. not evil -- clinical.",
            "foundry failed, he says, because it hesitated. his",
            "gardens grow bodies perfected past identity. the",
            "crawlers in the roots are his drafts.",
        ],
    },
    Entry {
        id: "dossier_pale_vector",
        title: "dossier: pale vector",
        claim: None,
        lines: &[
            "thin. pale. hooded. freed from technocrat imprisonment",
            "and loyal to blackstar for that alone. he maintains the",
            "relics and the amplifiers, keeps the forgotten codes,",
            "and whispers to the lattice. he is the crew's memory.",
            "do not press him. he withdraws, and remembers that too.",
        ],
    },
    Entry {
        id: "chicago_the_first_page",
        title: "chicago: the first page",
        claim: None,
        lines: &[
            "the record every archivist calls 'the blackstar material'",
            "opens here -- in the reconstruction, after the moon's dark",
            "day. cranes over the river. one bridge leaf still raised.",
            "a naval commander walked these ruins, read the salvage",
            "economy in them, and drew his first conclusion: empires",
            "burn. what he collected afterward, you already know.",
        ],
    },
    Entry {
        id: "the_space_bridge",
        title: "the space bridge",
        claim: None,
        lines: &[
            "the network was never destroyed -- only unlisted. gates",
            "sleep in the far places, keyed to relay fragments their",
            "builders scattered on purpose. each crossing charts a",
            "region the war forgot. the wardens were left to make",
            "sure nobody crossed carelessly. kill one and the gate",
            "reads you as worthy. that is not a malfunction.",
        ],
    },
    Entry {
        id: "appendix_hybrid_protocol",
        title: "appendix: hybrid protocol",
        claim: None,
        lines: &[
            "a fragment, unsigned, found threaded through the watch's code:",
            "'we grew them a body that remembers being both. metal that",
            "breathes. valence that beats. if you are reading this, archivist,",
            "the graft is yours to wake.'",
        ],
    },
];

pub fn entry(id: &str) -> Option<&'static Entry> {
    CODEX.iter().find(|e| e.id == id)
}

/// Entries that concern a claim, for the annotated view.
pub fn annotated() -> impl Iterator<Item = &'static Entry> {
    CODEX.iter().filter(|e| e.claim.is_some())
}

// ------------------------------------------------------------- driver ---

/// `--codex [entry]` — read it, and see where the evidence disagrees.
pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--codex").unwrap_or(0);
    match argv.get(i + 1).map(String::as_str) {
        None => {
            println!("THE CODEX — {} entries\n", CODEX.len());
            for e in CODEX {
                let mark = if e.claim.is_some() { "*" } else { " " };
                println!("  {mark} {:<28} {}", e.id, e.lines[0]);
            }
            println!("\n  * concerns a claim the archaeological map tracks");
            println!("  --codex <entry>");
        }
        Some(id) => match entry(id) {
            None => println!("no entry '{id}'."),
            Some(e) => show(e),
        },
    }
}

fn show(e: &Entry) {
    println!("{}\n", e.title.to_uppercase());
    for l in e.lines {
        println!("  {l}");
    }
    let Some(cid) = e.claim else { return };
    let Some(c) = crate::archive::claim(cid) else { return };
    // The annotation. The entry above does not change; this is what the
    // evidence currently supports, printed beside it.
    let (sv, _, cfg) = crate::save::load();
    let dig = crate::provenance::Dig::from_line(&sv.finds, crate::archive::FINDINGS);
    let a = crate::provenance::assay_with(
        crate::archive::CLAIMS,
        crate::archive::FINDINGS,
        dig.ids(),
        &crate::archive::priors(cfg.story.anchor),
    );
    let asy = &a[cid];
    let st = asy.standing_with(crate::provenance::Ladder::from_cfg(&cfg));
    println!("\n  ── the map says ──");
    println!("  {}", c.text);
    println!("  [{}]  {} for / {} against", st.label(), asy.support(), asy.opposition());
    if st == crate::provenance::Standing::Contested {
        println!("  the entry above is the settled account. the evidence disputes it.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn all_fifty_four_entries_came_across_intact() {
        assert_eq!(CODEX.len(), 54, "an entry was lost in the port");
        for e in CODEX {
            assert!(!e.title.is_empty(), "{} has no title", e.id);
            assert!(!e.lines.is_empty(), "{} has no body", e.id);
            assert!(e.lines.iter().all(|l| !l.trim().is_empty()), "{} has a blank line", e.id);
            // the breaks are the writing's rhythm, not word wrap: nothing
            // should be long enough to have been wrapped by accident
            assert!(
                e.lines.iter().all(|l| l.chars().count() <= 78),
                "{} has a line long enough to suggest a mangled parse",
                e.id
            );
        }
        let ids: BTreeSet<_> = CODEX.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), CODEX.len(), "duplicate entry id");
    }

    #[test]
    fn every_annotation_names_a_real_claim() {
        let n = annotated().count();
        assert!(n > 0, "nothing is annotated, so the feature does nothing");
        assert!(
            n * 4 < CODEX.len(),
            "{n} of {} entries annotated — an annotation that fires on \
             everything is decoration",
            CODEX.len()
        );
        for e in annotated() {
            let cid = e.claim.unwrap();
            assert!(
                crate::archive::claim(cid).is_some(),
                "codex entry '{}' names claim '{cid}', which does not exist",
                e.id
            );
        }
    }

    #[test]
    fn nothing_in_the_codex_is_ever_hidden() {
        // the decision `docs/STORY_BIBLE.md` recorded as open: annotate,
        // never gate. A reference you have to unlock is a reference you stop
        // trusting, so there is no visibility state on an Entry at all — and
        // this test exists so nobody adds one without arguing for it.
        for e in CODEX {
            let _ = e.lines; // readable, unconditionally
        }
        assert_eq!(
            CODEX.iter().filter(|e| e.claim.is_none()).count() + annotated().count(),
            CODEX.len(),
            "an entry is in neither category, which means a third state appeared"
        );
    }

    #[test]
    fn the_archivist_is_in_here_and_predates_the_bible() {
        // the C codex is SOURCE rather than derivative for this character:
        // the Archivist is addressed directly in it, before `archive.rs`
        // existed. Worth a test so a later edit does not quietly drop the
        // oldest thing in the file.
        let hit = CODEX
            .iter()
            .find(|e| e.lines.iter().any(|l| l.contains("archivist")));
        assert!(hit.is_some(), "the Archivist material is gone from the codex");
    }
}
