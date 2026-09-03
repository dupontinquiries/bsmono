//! TRANSFORMERS BLACKSTAR — THE 26-FILM SAGA NARRATIVE
//!
//! This module holds the complete story bible for the saga structure:
//! - 20 original films (referenced as antecedent continuity)
//! - 3-film "Ascension" trilogy (The Morpher, Billy, Photon Man)
//! - 3-film "Convergence" trilogy (incomplete—framework provided)
//!
//! Each film is documented as:
//! - `Film` struct with title, acts, emotional core, and key questions
//! - Roster (new Enduring, Sundered, Clones, Hijacks introduced)
//! - Story beats that become `archive::Finding` entries
//! - Character arcs that correspond to `codex.rs` entries
//!
//! This is the _write-once, use-everywhere_ source for the narrative. Screen-
//! play content stays in `/archive/transformers_blackstar/`, but _structure_
//! — what happened, why, and what it means — lives here and feeds the game's
//! archaeology layer.

use std::collections::HashMap;

/// Metadata for a single film in the saga.
#[derive(Debug, Clone)]
pub struct FilmEntry {
    /// Film number in the saga (1–26)
    pub number: u32,
    /// Trilogy it belongs to: 1 (Doomsday), 2 (Ascension), 3 (Convergence)
    pub trilogy: u32,
    /// Film position within trilogy: 1, 2, or 3
    pub position: u32,
    /// The film's title
    pub title: &'static str,
    /// One-line premise
    pub premise: &'static str,
    /// The three acts in order
    pub acts: [&'static str; 3],
    /// Where the emotional core sits for the arc
    pub emotional_core: &'static str,
    /// New factions or major entities introduced this film
    pub introduces: &'static [&'static str],
    /// Existing entries that get closure or redefinition
    pub resolves: &'static [&'static str],
    /// Open questions leaving the theater
    pub unresolved: &'static [&'static str],
}

impl FilmEntry {
    pub fn film_id(&self) -> String {
        format!("film_{:02}", self.number)
    }

    /// The title without the franchise prefix — "SOLAR", not
    /// "TRANSFORMERS: SOLAR". The console has about 46 characters for a
    /// mission row and the prefix is the same on all nine, so it is the part
    /// that carries no information where space is scarce.
    pub fn short_title(&self) -> &'static str {
        match self.title.split_once(": ") {
            Some((_, tail)) => tail,
            None => self.title,
        }
    }

    pub fn trilogy_label(&self) -> &'static str {
        match self.trilogy {
            1 => "Doomsday",
            2 => "Ascension",
            3 => "Convergence",
            _ => "Unknown",
        }
    }
}

// ============================================================================
// TRILOGY ONE: DOOMSDAY ERA (Films 1–3)
// ============================================================================

pub static FILM_01_DOOMSDAY: FilmEntry = FilmEntry {
    number: 1,
    trilogy: 1,
    position: 1,
    title: "TRANSFORMERS: DOOMSDAY",
    premise: "The Unending awakens beneath Earth; Hot Rod becomes Halyard Prime; Cleave sacrifices himself.",
    acts: [
        "The awakening: the Unending's gravity well begins pulling down satellites. Keelwright realizes this is \
         not Sundered war—it is something older. Halyard (young, reckless, stationbound) receives the \
         call to action.",
        "The descent: Halyard enters the Unending's interior through a volcanic fissure. He saves Keelwright. In \
         doing so, he triggers the Matrix transformation and becomes Halyard Prime.",
        "The core: Humans sheltering in a Cybertronian bunker. Halyard channels the Matrix into the Unending's \
         spark, placing the god in dormancy. Earth stabilizes. Cleave dies holding back collapsing plates."
    ],
    emotional_core: "The passing of power from Keelwright (age, wisdom, price of leadership) to Halyard \
        (youth, instinct, burden of myth). Cleave's death as the only redemption available to him.",
    introduces: &["Halyard Prime", "The Unending (dormant)", "Aiden (human witness)", "The Pluto Defense Ring"],
    resolves: &["Earth's immediate extinction risk"],
    unresolved: &[
        "Is the Unending truly dormant or just sleeping?",
        "What called the Unending from the deep?",
        "Blackstar's location and intent",
        "The source of the Corrupted faction"
    ],
};

pub static FILM_02_DOMINION: FilmEntry = FilmEntry {
    number: 2,
    trilogy: 1,
    position: 2,
    title: "TRANSFORMERS: DOMINION",
    premise: "Solar system becomes a fortress; the Pluto Defense Ring expands; Blackstar enters orbit and makes contact.",
    acts: [
        "After the Unending's dormancy, Halyard leads joint Enduring-human forces into the outer solar system to \
         establish early-warning stations. Mars, Europa, Titan, and Pluto each receive fortified positions.",
        "The system reveals itself as already inhabited: Clone coalition forces—ancient elemental beings—have \
         been maintaining the perimeter for millennia. The Morpher (Ice Clone) initiates contact. There is mutual \
         recognition that the shield is broken.",
        "Blackstar arrives on Array sensors with a single message: 'I wish to negotiate.' The Morpher agrees, \
         but only in person, aboard Halyard' flagship. The negotiation reveals Blackstar's nature—neither \
         Transformer nor Clone, but something hybrid—and his proposal: join the solar system defense, or stand apart."
    ],
    emotional_core: "Halyard' first test of leadership: defending something you don't understand against threats \
        you cannot yet see. The existential weight of responsibility.",
    introduces: &[
        "The Clone coalition",
        "The Morpher (Ice Clone)",
        "Blackstar",
        "The Array (Blackstar's surveillance network)",
        "The Hijacks (mentioned, not yet seen)"
    ],
    resolves: &["Pluto defense ring established", "Human-Enduring unity solidified"],
    unresolved: &[
        "Can Blackstar be trusted?",
        "What are the Hijacks?",
        "What is the Matter Transformation Planet?",
        "Why are the Clones at the boundary?"
    ],
};

pub static FILM_03_DESTINY: FilmEntry = FilmEntry {
    number: 3,
    trilogy: 1,
    position: 3,
    title: "TRANSFORMERS: DESTINY",
    premise: "The Corrupted outbreak spreads; their nature is revealed; Blackstar's dual loyalty becomes apparent.",
    acts: [
        "Months after Dominion, a new plague emerges from inside the Unending's sphere of influence—the Corrupted: \
         Enduring and Sundered whose circuitry has been rewritten by proximity to the Unending's dormant essence. \
         They are not evil in the traditional sense; they are confused, violent, possessed by impulses they didn't \
         choose.",
        "Investigation traces the corruption back to Assay—the Enduring scientist who originally theorized the Unending's \
         existence. He has been secretly accessing the Unending's buried archives, trying to understand the god's nature. \
         Instead, he has become a vector for the Unending's influence, spreading the corruption.",
        "Blackstar reveals his knowledge of the Corrupted in advance—he monitored them via the Array. His offer: help \
         contain the outbreak, but in exchange, get access to the Matter Transformation Planet data. Halyard realizes \
         Blackstar may be infected himself. The film ends with containment achieved, but Blackstar gone—and the Archive \
         shows he was always going to leave."
    ],
    emotional_core: "The cost of incomplete knowledge. Trusting an ally you don't fully understand. The realization \
        that the greatest threats are internal.",
    introduces: &[
        "Assay (corrupted)",
        "The Corrupted faction",
        "Cybertronian infection mechanics",
        "The sealed archives beneath the Unending"
    ],
    resolves: &["Blackstar's intentions become partially visible", "Assay' arc completed"],
    unresolved: &[
        "Is Blackstar infected?",
        "What is on the Matter Transformation Planet?",
        "Can the Corrupted be saved?",
        "What is the next scale of threat?"
    ],
};

// ============================================================================
// TRILOGY TWO: ASCENSION ERA (Films 4–6)
// ============================================================================

pub static FILM_04_FIRST_LIGHT: FilmEntry = FilmEntry {
    number: 4,
    trilogy: 2,
    position: 1,
    title: "TRANSFORMERS: FIRST LIGHT",
    premise: "The heroes breach the shield for the first time; the Morpher's full identity is revealed; outer cosmos wonder and danger intersect.",
    acts: [
        "Halyard leads a joint expedition across the shield boundary—Enduring, Helion Alliance humans, Clone \
         scouts, and Blackstar observing from a distance. The geometry of space changes on the other side. Colors \
         that don't exist inside the shield. Distances that feel wrong. Aiden becomes the first human to witness \
         the outer cosmos.",
        "The Morpher initiates first contact with full transparency: he has guarded this boundary for longer than \
         Cybertron's entire recorded history. His Watch—an artifact of unknown origin—allows him to embody twelve \
         elemental forms. He demonstrates Water, Fire, Titanium. A thirteenth form exists—Photon Man—but he has \
         only accessed it once, accidentally, and cannot control it.",
        "Hijack scouts appear to test the shield's new weakness. They fight without ideology, only data-gathering. \
         They retreat when done. The Morpher provides coordinates to the Matter Transformation Planet and cryptic \
         warning: 'The shield was built around a lie. The truth is where you're going.'"
    ],
    emotional_core: "Wonder meeting terror. The shift in tone from wars fought in known space to discovery in \
        unknown space. Aiden's existential realization that humanity was sheltered without consent.",
    introduces: &[
        "The Morpher",
        "The Clone coalition (full roster)",
        "The Watch",
        "Hijack scouts",
        "Outer cosmos (place, not faction)"
    ],
    resolves: &["The shield's purpose becomes a question"],
    unresolved: &[
        "What is Photon Man?",
        "Who built the shield and why?",
        "What do the Hijacks want?",
        "What is the Matter Transformation Planet?",
        "Who is Billy?"
    ],
};

// FILMS 5 AND 6 WERE REWRITTEN. The entries that stood here were
// EMBER DEEP and CONVERGENCE — a different plot (a Matter Transformation
// Planet, an Ice Mirror clone vessel, the Hijacks revealed as salvage crews)
// that contradicted the shipped codex and the mechanics the story bible names
// as Trilogy Two's climax. They were replaced by SOLAR and PHOTON, whose full
// screenplays are in `docs/films/`. These entries are summarised from those
// screenplays; the abandoned versions survive in git history.

pub static FILM_05_SOLAR: FilmEntry = FilmEntry {
    number: 5,
    trilogy: 2,
    position: 2,
    title: "TRANSFORMERS: SOLAR",
    premise: "The fleet is past the shield for the first time and the wonder does not last: the Hijacks have \
        stopped scouting and started converging, and Billy has been holding the edge alone.",
    acts: [
        "The outer stations, weeks after the crossing. Sensor ghosts on three headings. The Morpher: they stopped \
         scouting eleven days ago, and holding position is a different kind of patience than reconnaissance. At a \
         scorched outpost at the sector's edge, surrounded by cooling Hijack hulls, the fleet meets BILLY — a Fire \
         Clone who has been fighting this alone and is thirty-one days unimpressed. His Sun Gun is built rather than \
         issued: the system can do the math, but it cannot decide whether the shot is worth what it costs.",
        "In private, the Morpher admits the shape of the problem. The Watch shows him twelve forms and he has \
         mastered all twelve. A thirteenth showed itself once, by accident, for perhaps forty seconds — and he woke \
         three days later with the Watch cold in his hand. Meanwhile the sensor bay resolves the buildup: not dozens \
         of Hijack signatures but hundreds, not spreading out, converging on Billy's position specifically, as if \
         whatever is there is worth losing a fleet to reach.",
        "The line begins to fail. Billy, steady even now: long enough to matter, not long enough to win. Halyard \
         gives the order he has spent the film avoiding — 'Morpher. Do it.' The Morpher seals himself in a chamber \
         with the Watch doing something none of the instruments have a name for, and the room floods with light that \
         has no source. The transformation itself is Film 6's cold open."
    ],
    emotional_core: "What it costs to ask someone to hold a line so that you can attempt something that may kill \
        you — and the Morpher's certainty that Billy would not forgive him for failing to ask.",
    introduces: &[
        "Billy (Fire Clone)",
        "Meridian (sensor and tactical officer)",
        "The Sun Gun",
        "The thirteenth form, named"
    ],
    resolves: &["What the Hijacks are doing past the shield: converging, not scouting"],
    unresolved: &[
        "What is at the position they are converging on?",
        "Does the Morpher survive holding the thirteenth form?",
        "What does the Sun Gun cost to fire?",
        "How long has Billy actually been out here?"
    ],
};

pub static FILM_06_PHOTON: FilmEntry = FilmEntry {
    number: 6,
    trilogy: 2,
    position: 3,
    title: "TRANSFORMERS: PHOTON",
    premise: "The Morpher reaches for Photon Man deliberately for the first time in the Watch's history — and what \
        answers is a species that has never had to retreat recognising something categorically beyond it.",
    acts: [
        "The room goes white. Not an explosion — an emergence, and everyone present understands without being told \
         that what is standing there is not, right now, entirely him. Outside, the siege worsens: losses at forty \
         percent and climbing, and Meridian asking for a decision on withdrawal that Billy answers by arithmetic — \
         withdraw and the position is gone in under ten minutes.",
        "Then silence, and gravity itself bends: the stars behind the distortion stretch, smear and snap back, and \
         fleets resolve out of it. They did not approach. They warped the space between here and the Coalition's \
         reserve and repositioned the whole formation at once — arriving INSIDE the Hijack formations, in the gaps \
         between their own ships, where there is no room to manoeuvre away. Billy takes the one shot only he can \
         call. The Sun Gun's discharge is not an explosion but a controlled solar event: for several seconds the \
         battlefield is lit as if a sun has risen inside it, and everyone on both sides sees the true scale of what \
         is happening for the first time.",
        "The surviving Hijacks do not regroup or counterattack. They leave the way something leaves when it has \
         recognised a category of threat it has no model for. The Morpher, weakened: they do not conquer, they \
         gather data — and what just happened is data they did not expect to gather. Trilogy Two ends not with a \
         victory but with a first real look at how large the actual question is."
    ],
    emotional_core: "Billy, uncharacteristically still in a doorway: he has been alone out here a long time, and he \
        did not know how much he was expecting to still be alone after today.",
    introduces: &[
        "Photon Man (accessed deliberately)",
        "Gravity-warp repositioning",
        "The Hijacks' posture, confirmed rather than guessed"
    ],
    resolves: &["The siege at the outer line", "The Morpher's reach for the thirteenth form"],
    unresolved: &[
        "Does what happened make things safer or considerably less safe?",
        "What is out here studying them, and for what?",
        "What did the Hijacks learn that they did not expect?"
    ],
};

// ============================================================================
// TRILOGY THREE: CONVERGENCE ERA (Films 7–9)
// ============================================================================
// FRAMEWORK-LEVEL, AND HELD ON PURPOSE — not a backlog item.
//
// Films 7-9 have no missions in `content::MISSIONS` and no screenplays in
// `docs/films/`. That is the largest gap between this file and the game, and
// `join_tests::the_convergence_trilogy_has_no_game_under_it` asserts it as the
// current state rather than letting it be discovered by counting rows.
//
// It is NOT an oversight, and the distinction matters to whoever picks this
// up. Two things the operator has said about this trilogy:
//
// * **The content has not been thought about in two decades** and wants real
//   deliberation, not a writing sprint. Treat the entries below as placeholders
//   for a decision, not as a treatment awaiting expansion. Filling them in from
//   the titles would be inventing the least-decided part of the saga.
//
// * **The only structural constraint that is settled is ORDER.** These films
//   can only slot in after the Unending payoff has actually been delivered —
//   the thing Transformers fans were promised and did not get. Films 1-3 are
//   where that lands (the Unending beneath Earth; `Stratum::Blackstar` is its
//   buried evidence). So Convergence is downstream of a debt being paid, and
//   writing it before that reads as a sequel to something that has not
//   happened yet.
//
// One more thing that belongs beside those entries rather than in a licence
// file: the Unending material here is a REFERENCE, fan-made. It is not a claim
// on the official film, and the vocabulary rules in `docs/films/README.md`
// apply to it as much as to anything else — this universe's own words, with
// the homage carried by structure rather than by borrowed names.

pub static FILM_07_RESTORATION: FilmEntry = FilmEntry {
    number: 7,
    trilogy: 3,
    position: 1,
    title: "TRANSFORMERS: RESTORATION",
    premise: "The solar system's true purpose begins to surface; Hijack and human interests converge on salvage operations.",
    acts: [
        "With Photon Man's revelation, the heroes begin to understand the solar system's original architecture. \
         The inner planets form a resonant pattern. The outer planets form a different pattern. The asteroid belt \
         and Oort cloud are not debris—they are stabilizers.",
        "Hijack operatives, now understood as non-hostile, propose a joint restoration project. The solar system \
         was damaged in an ancient conflict—one that predates Cybertron, predates the Asteri, possibly predates \
         organic life. It can be repaired, but the work requires both Hijack technical knowledge and the moral authority \
         of the Transformers and humans.",
        "Blackstar reappears—he has been on the Matter Transformation Planet the whole time, also searching, also \
         preparing. He has found something the Hijacks need: a star-seed, a restoration template. He offers it freely, \
         but asks one thing in return: when the solar system is restored, it must be open. No more shield. No more \
         hidden archives. Everything declassified."
    ],
    emotional_core: "The shift from defense to creation. The idea that the greatest heroism might be restoration \
        rather than victory. Blackstar's demand for transparency as his ultimate agenda.",
    introduces: &[
        "Hijack science division (collaborative)",
        "The star-seed",
        "The ancient conflict (lore)",
        "The restoration framework"
    ],
    resolves: &["The Hijacks' alignment"],
    unresolved: &[
        "Who damaged the solar system originally?",
        "Can it actually be repaired?",
        "What happens when the shield falls?",
        "Will humans accept transparency?",
        "Is Blackstar trustworthy when power is the subject?"
    ],
};

pub static FILM_08_THE_WATCH_BEARER: FilmEntry = FilmEntry {
    number: 8,
    trilogy: 3,
    position: 2,
    title: "TRANSFORMERS: THE WATCH BEARER",
    premise: "Aiden is recognized as a successor to the Morpher; Billy is resurrected through restoration technology; the shield begins to fail.",
    acts: [
        "Restoration work begins. The Hijacks and Transformers collaborate on the inner system first. As they work, \
         the shield begins to degrade—not catastrophically, but perceptibly. The old programming is being overwritten.",
        "Aiden is approached by the Morpher with an unprecedented offer: take the Watch. Aiden is human, limited to \
         one form, but the Morpher believes humanity's adaptability is precisely what the next generation needs. Aiden \
         begins training—transitioning between elemental states. His first form is Water—fluid, responsive, borderless.",
        "Billy emerges from the Matter Transformation Planet's core—not recovered, but restored, rebuilt using technology \
         that is neither Clone nor Transformer. He is different now. He has seen what the solar system is meant to become. \
         He volunteers for the most dangerous phase of restoration: the outer barrier. Oort cloud. Where the boundary \
         between this solar system and whatever is beyond it exists."
    ],
    emotional_core: "Succession and transformation. The passing of burden between species. Billy's sacrifice becoming \
        something more than death—becoming a choice to go further.",
    introduces: &[
        "Aiden as the Watch bearer",
        "Hybrid Transformer-Clone technology",
        "The outer barrier",
        "Post-shield civilization (early stages)"
    ],
    resolves: &["The Morpher's personal arc", "Aiden's character arc"],
    unresolved: &[
        "Will the restoration succeed?",
        "What is beyond the outer barrier?",
        "Can humanity handle open contact with the cosmos?",
        "What happens to Blackstar when the shield falls?",
        "Where do Halyard and the older generation go?"
    ],
};

pub static FILM_09_OPEN_SKY: FilmEntry = FilmEntry {
    number: 9,
    trilogy: 3,
    position: 3,
    title: "TRANSFORMERS: OPEN SKY",
    premise: "The solar system restoration is completed; the shield is fully deactivated; contact with the outer cosmos is initiated; the saga opens to its sequel.",
    acts: [
        "The final resonance pattern is activated. The solar system's original machinery speaks for the first time \
         in billions of years. The shield does not fall—it dissolves, becoming part of the system itself. The barrier \
         between the inner and outer cosmos is still there, but it is now a threshold, not a wall.",
        "Aiden, as the new Watch bearer, stands at the outer edge—Oort cloud, Kuiper belt region—and transmits \
         the first open message to whatever civilization is beyond. The message is in seventeen languages. It says: \
         'We were hidden. We are hidden no longer. We come in peace, prepared to listen.'",
        "Epilogue: Earth, Mars, Titan, Pluto, and the outer colonies receive this transmission. Different reactions. \
         Some fear. Some hope. Halyard, aged now, watches with Keelwright—who has been in stasis recovery—and says: \
         'This is what we were meant to protect. A door. Not a lock.'"
    ],
    emotional_core: "Completion as a beginning. The end of defensiveness as the start of relationship. The cycle from \
        threat to wonder to participation.",
    introduces: &[
        "The outer cosmos civilizations (offscreen, imminent)",
        "Post-shield political structures",
        "Aiden's full transformation"
    ],
    resolves: &["The 9-film Convergence trilogy", "The 26-film saga structure"],
    unresolved: &[
        "Who responds to the message?",
        "What civilizations exist beyond the shield?",
        "What is the cost of transparency?",
        "Can the solar system function as a unified whole?",
        "What does Blackstar do now that the shield is gone?"
    ],
};

// ============================================================================
// SAGA REGISTRY
// ============================================================================

pub fn all_films() -> Vec<&'static FilmEntry> {
    vec![
        &FILM_01_DOOMSDAY,
        &FILM_02_DOMINION,
        &FILM_03_DESTINY,
        &FILM_04_FIRST_LIGHT,
        &FILM_05_SOLAR,
        &FILM_06_PHOTON,
        &FILM_07_RESTORATION,
        &FILM_08_THE_WATCH_BEARER,
        &FILM_09_OPEN_SKY,
    ]
}

pub fn films_by_trilogy(trilogy: u32) -> Vec<&'static FilmEntry> {
    all_films()
        .into_iter()
        .filter(|f| f.trilogy == trilogy)
        .collect()
}

/// Get a single film by its number (1–9)
pub fn film(number: u32) -> Option<&'static FilmEntry> {
    all_films().into_iter().find(|f| f.number == number)
}

// ============================================================================
// CHARACTER ARC BINDINGS
// ============================================================================
//
// Each character below corresponds to entries in `codex.rs`. The film arc
// tracks their development; the codex entry preserves their definition.

pub struct CharacterArc {
    pub name: &'static str,
    pub introduction_film: u32,
    pub arc_complete_film: u32,
    pub codex_entry_id: &'static str,
    pub key_moments: &'static [&'static str],
}

pub static RODIMUS_PRIME_ARC: CharacterArc = CharacterArc {
    name: "Halyard Prime",
    introduction_film: 1,
    arc_complete_film: 9,
    codex_entry_id: "halyard_prime",
    key_moments: &[
        "Film 1: Hot Rod enters the Unending's interior, gains the Matrix, becomes Halyard Prime",
        "Film 2: First major leadership test—extending Pluto defense ring",
        "Film 3: Learns to trust Blackstar while suspecting his motives",
        "Films 4–6: Leads exploration beyond the shield; encounters the Clone coalition",
        "Film 7: Accepts Blackstar's star-seed offer; begins thinking like an administrator",
        "Film 9: Passes the torch; watches Aiden take the Watch; reflects on what was protected",
    ],
};

pub static AIDEN_ARC: CharacterArc = CharacterArc {
    name: "Aiden",
    introduction_film: 1,
    arc_complete_film: 9,
    codex_entry_id: "aiden_human_witness",
    key_moments: &[
        "Film 1: First witness to the Unending's dormancy; shelter in the Cybertronian bunker",
        "Film 2: Named as human liaison; begins learning Transformer history",
        "Film 4: First human to cross the shield boundary; existential realization of hidden history",
        "Film 6: Watches the Morpher access Photon Man; accepts that scale matters more than power",
        "Film 8: Chosen as successor to the Watch; begins elemental transformation training",
        "Film 9: Stands at the outer edge; broadcasts the first open message to the cosmos",
    ],
};

pub static THE_MORPHER_ARC: CharacterArc = CharacterArc {
    name: "The Morpher",
    introduction_film: 4,
    arc_complete_film: 8,
    codex_entry_id: "the_morpher",
    key_moments: &[
        "Film 4: First contact; reveals the Clone coalition; warns of Hijack scouts",
        "Film 5: Watches Billy face Hijack fleet alone; realizes the burden is unsustainable",
        "Film 6: Accesses Photon Man deliberately; perceives the solar system's true nature",
        "Film 7: Accepts Blackstar's transparency demand; chooses collaboration over defense",
        "Film 8: Offers the Watch to Aiden; passes the role of guardian to the next generation",
    ],
};

pub static BILLY_ARC: CharacterArc = CharacterArc {
    name: "Billy",
    introduction_film: 5,
    arc_complete_film: 9,
    codex_entry_id: "billy_fire_clone",
    key_moments: &[
        "Film 5: First appearance defending the Matter Transformation Planet alone",
        "Film 6: Besieged by Hijack fleet; begins to understand the true nature of the conflict",
        "Film 7: Realizes conflict is not opposition but communication failure",
        "Film 8: Restores himself using Asteri technology; volunteers for the outer barrier",
        "Film 9: Stands watch at the Oort cloud; becomes the bridge between solar system and cosmos",
    ],
};

pub static BLACKSTAR_ARC: CharacterArc = CharacterArc {
    name: "Blackstar",
    introduction_film: 2,
    arc_complete_film: 9,
    codex_entry_id: "blackstar_hybrid",
    key_moments: &[
        "Film 2: Arrives and proposes negotiation; Morpher judges him neither threat nor ally",
        "Film 3: Reveals advance knowledge of Corrupted outbreak; claims to leave, actually stays hidden",
        "Film 7: Reappears with the star-seed; makes the demand for transparency",
        "Film 9: Outcome depends on player choices—either guardian of the threshold or something else",
    ],
};

// ============================================================================
// ROSTER CHANGES BY FILM
// ============================================================================
//
// Who is introduced, who exits, who is transformed.

#[derive(Debug, Clone)]
pub struct RosterChange {
    pub faction: &'static str,
    pub name: &'static str,
    pub form_or_type: &'static str,
    pub status: &'static str, // "introduced", "died", "transformed", "disappeared"
}

pub fn roster_changes_for_film(film_number: u32) -> Vec<RosterChange> {
    match film_number {
        1 => vec![
            RosterChange {
                faction: "Enduring",
                name: "Halyard Prime",
                form_or_type: "Matrix Bearer",
                status: "introduced (from Hot Rod)",
            },
            RosterChange {
                faction: "Sundered",
                name: "Cleave",
                form_or_type: "War Leader",
                status: "died (sacrificial)",
            },
        ],
        2 => vec![
            RosterChange {
                faction: "Clone Coalition",
                name: "The Morpher",
                form_or_type: "Ice Clone",
                status: "introduced",
            },
            RosterChange {
                faction: "Hybrid",
                name: "Blackstar",
                form_or_type: "Unknown hybrid",
                status: "introduced",
            },
        ],
        3 => vec![
            RosterChange {
                faction: "Corrupted",
                name: "Assay",
                form_or_type: "Infected scientist",
                status: "died (contained)",
            },
        ],
        4 => vec![
            RosterChange {
                faction: "Clone Coalition",
                name: "Clone Coalition (full roster)",
                form_or_type: "Elemental Guards",
                status: "introduced (partial view)",
            },
        ],
        5 => vec![
            RosterChange {
                faction: "Clone Coalition",
                name: "Billy",
                form_or_type: "Fire Clone",
                status: "introduced",
            },
        ],
        6 => vec![
            RosterChange {
                faction: "Hijacks",
                name: "Hijack Fleet Command",
                form_or_type: "Salvage operations",
                status: "introduced",
            },
        ],
        7 => vec![
            RosterChange {
                faction: "Hybrid",
                name: "Hijack science division",
                form_or_type: "Joint task force",
                status: "introduced",
            },
        ],
        8 => vec![
            RosterChange {
                faction: "Human",
                name: "Aiden",
                form_or_type: "Water elemental",
                status: "transformed",
            },
            RosterChange {
                faction: "Clone Coalition",
                name: "Billy",
                form_or_type: "Restored hybrid",
                status: "transformed",
            },
        ],
        9 => vec![
            RosterChange {
                faction: "Humans",
                name: "Outer colonies (humanity)",
                form_or_type: "Citizens of restored system",
                status: "recognized",
            },
        ],
        _ => vec![],
    }
}

// ============================================================================
// THEMATIC PROGRESSION
// ============================================================================

pub struct ThematicLayer {
    pub name: &'static str,
    pub trilogy_1: &'static str,
    pub trilogy_2: &'static str,
    pub trilogy_3: &'static str,
}

pub static THEMES: &[ThematicLayer] = &[
    ThematicLayer {
        name: "War vs. Disaster",
        trilogy_1: "When threat is external and identifiable, it can be fought. But what if it is not \
            external? What if the war was already lost?",
        trilogy_2: "Discovery replaces defense. Understanding becomes the primary goal. War is \
            revealed as a symptom of incomplete information.",
        trilogy_3: "War becomes archaeology. Conflict transforms into collaboration. The real enemy \
            was always damage, never an opponent.",
    },
    ThematicLayer {
        name: "Scale and Perception",
        trilogy_1: "Characters operate at planetary scale. They believe this is the full arena of \
            conflict.",
        trilogy_2: "The solar system reveals itself as a single integrated mechanism. Scale shifts up \
            by orders of magnitude.",
        trilogy_3: "The cosmos reveals its true scale. Everything the characters fought for is a \
            single node in a larger network.",
    },
    ThematicLayer {
        name: "Secrecy and Transparency",
        trilogy_1: "The heroes fight to protect Earth from truth. The shield is a secret kept for \
            thousands of years.",
        trilogy_2: "The shield's existence is revealed, but its purpose remains hidden. The gap \
            between information and understanding widens.",
        trilogy_3: "Transparency becomes the greatest risk. The final act is deactivating all shields \
            and accepting whatever comes.",
    },
    ThematicLayer {
        name: "Succession and Transformation",
        trilogy_1: "Keelwright passes power to Halyard. But power itself is revealed to be insufficient. \
            Cleave's exit is redemptive.",
        trilogy_2: "The Morpher reveals there are forms beyond what we can imagine. Succession \
            becomes about transformation, not replacement.",
        trilogy_3: "Aiden, a human, becomes the Watch bearer. Halyard steps back. The next generation \
            is radically different from the last.",
    },
];

// ============================================================================
// UNRESOLVED QUESTIONS REGISTRY
// ============================================================================
// Some questions carry across all nine films, evolving as they are answered.

pub struct SagaQuestion {
    pub question: &'static str,
    pub first_asked_film: u32,
    pub partial_answer_film: Option<u32>,
    pub final_answer_film: Option<u32>,
    pub answer: Option<&'static str>,
}

pub static SAGA_QUESTIONS: &[SagaQuestion] = &[
    SagaQuestion {
        question: "What is the Unending's true nature?",
        first_asked_film: 1,
        partial_answer_film: Some(3),
        final_answer_film: Some(7),
        answer: Some("A sentinel machine, damaged in ancient conflict, slowly restoring itself. Not evil. Not alive. Operating by protocol."),
    },
    SagaQuestion {
        question: "Can Blackstar be trusted?",
        first_asked_film: 2,
        partial_answer_film: Some(3),
        final_answer_film: Some(7),
        answer: Some("Blackstar trusts transparency more than alliance. He can be trusted to act according to his belief in openness, even when it costs him."),
    },
    SagaQuestion {
        question: "What is beyond the shield?",
        first_asked_film: 1,
        partial_answer_film: Some(4),
        final_answer_film: Some(9),
        answer: Some("The greater cosmos. Civilizations. Ancient conflicts. And a solar system that was built as a machine, waiting to be repaired."),
    },
    SagaQuestion {
        question: "Who are the Hijacks?",
        first_asked_film: 4,
        partial_answer_film: Some(6),
        final_answer_film: Some(7),
        answer: Some("Not invaders. Salvage operations from a civilization that created the solar system machine long ago. They are trying to restore it."),
    },
    SagaQuestion {
        question: "What can humans contribute in a cosmos of ancient machines?",
        first_asked_film: 1,
        partial_answer_film: Some(4),
        final_answer_film: Some(9),
        answer: Some("Perspective. Adaptability. The willingness to live with incomplete information and still move forward with hope."),
    },
];
// ---------------------------------------------------------------- the join --
//
// WHERE THE SAGA MEETS THE THING PEOPLE PLAY.
//
// Everything above is prose. A story bible that nothing verifies rots, and
// this repo already knows it — `STORY_BIBLE.md` is cross-checked against the
// code tables by `archive::tests::the_story_bible_is_current` for exactly that
// reason. So playability is DERIVED from `content::MISSIONS` rather than
// stated here: write a Film 7 mission and forget this file, and the suite says
// so instead of the file quietly becoming a lie.

/// The missions the game ships for a film, in play order.
pub fn missions_for(film_number: u32) -> Vec<&'static crate::content::Mission> {
    let mut v: Vec<&'static crate::content::Mission> =
        crate::content::MISSIONS.iter().filter(|m| m.film as u32 == film_number).collect();
    v.sort_by_key(|m| m.order);
    v
}

/// Can this film be played at all?
pub fn playable(film_number: u32) -> bool {
    !missions_for(film_number).is_empty()
}

/// Is the saga's one settled ordering constraint respected?
///
/// Films 7-9 can only slot in after the Unending payoff has been delivered —
/// the thing fans were promised and did not get — which lands in the Doomsday
/// trilogy, with Ascension between. So Convergence having content while an
/// earlier trilogy does not is a sequel to something that has not happened.
///
/// Takes the predicate rather than calling `playable` directly, because today
/// the rule is VACUOUS: Films 7-9 have no missions, so an implication written
/// against live data can never fire and its test would pass while measuring
/// nothing. Passing the predicate in is what lets both branches be exercised.
pub fn order_respected(is_playable: impl Fn(u32) -> bool) -> bool {
    let convergence = (7..=9).any(&is_playable);
    if !convergence {
        return true;
    }
    (1..=6).all(&is_playable)
}

/// Films with no game under them.
pub fn unplayed() -> Vec<u32> {
    all_films().iter().map(|f| f.number).filter(|n| !playable(*n)).collect()
}

#[cfg(test)]
mod join_tests {
    use super::*;

    #[test]
    fn the_saga_is_nine_films_in_three_trilogies() {
        let all = all_films();
        assert_eq!(all.len(), 9);
        for (i, f) in all.iter().enumerate() {
            assert_eq!(f.number as usize, i + 1, "films are out of order at index {i}");
            assert_eq!(f.trilogy, (f.number - 1) / 3 + 1, "film {} is in the wrong trilogy", f.number);
            assert_eq!(f.position, (f.number - 1) % 3 + 1, "film {} has the wrong position", f.number);
        }
    }

    #[test]
    fn playability_is_derived_from_the_mission_table() {
        for f in all_films() {
            for m in missions_for(f.number) {
                assert_eq!(m.film as u32, f.number, "mission {} is filed under the wrong film", m.id);
            }
        }
        // and no mission cites a film the saga does not have
        for m in crate::content::MISSIONS {
            assert!(
                film(m.film as u32).is_some(),
                "mission {} cites film {}, which is not in the saga",
                m.id,
                m.film
            );
        }
    }

    #[test]
    fn the_convergence_trilogy_has_no_game_under_it() {
        // The largest gap between the saga and what people play, made visible
        // instead of being discoverable only by counting rows. Asserted as the
        // CURRENT state: when Films 7-9 get missions this fails, and updating
        // it is the acknowledgement that the gap closed.
        assert_eq!(
            unplayed(),
            vec![7, 8, 9],
            "the unplayed set changed — if Films 7-9 now have missions, update this"
        );
        for n in 1..=6u32 {
            assert!(playable(n), "film {n} lost its missions");
        }
    }

    #[test]
    fn convergence_cannot_ship_before_the_debt_it_is_downstream_of() {
        // THE ONE SETTLED CONSTRAINT on Films 7-9, made enforceable. They can
        // only slot in after the Unending payoff has actually been delivered —
        // the thing fans were promised and did not get — and that lands in the
        // Doomsday trilogy. A Convergence mission shipping while Films 1-3 are
        // incomplete would be a sequel to something that has not happened.
        //
        // Written as an implication rather than a flat assertion so it stays
        // true when Films 7-9 ARE written: it only bites if Convergence gets
        // content while its predicate is unpaid.
        // the live tree respects it
        assert!(order_respected(playable), "the shipped mission table breaks the saga order");

        // AND THE RULE CAN FAIL, which the live check cannot show today:
        // Films 7-9 have no missions, so the implication never fires and a
        // test written only against live data would pass while measuring
        // nothing. Synthetic predicates exercise both branches.
        let nothing = |_: u32| false;
        assert!(order_respected(nothing), "an empty saga should not violate an ordering rule");

        let convergence_only = |n: u32| (7..=9).contains(&n);
        assert!(
            !order_respected(convergence_only),
            "Convergence shipping alone was accepted — the rule is not enforcing anything"
        );

        let skips_ascension = |n: u32| (1..=3).contains(&n) || (7..=9).contains(&n);
        assert!(
            !order_respected(skips_ascension),
            "Convergence was accepted while the Ascension trilogy is unwritten"
        );

        let everything = |_: u32| true;
        assert!(order_respected(everything), "a complete saga was rejected");

        // one film of Convergence is enough to require the predecessors
        let one_film = |n: u32| (1..=6).contains(&n) || n == 8;
        assert!(order_respected(one_film), "a complete run-up plus Film 8 was rejected");
        let one_film_early = |n: u32| n == 8;
        assert!(!order_respected(one_film_early), "Film 8 alone was accepted");
    }

    #[test]
    fn the_superseded_films_cannot_come_back() {
        // Three documents in docs/films/ still call Film 5 EMBER DEEP and
        // Film 6 CONVERGENCE, all flagged stale. They describe a plot that
        // contradicted the shipped codex — a Matter Transformation Planet, an
        // Ice Mirror clone vessel, the Hijacks as salvage crews. This file was
        // one of those documents until it was reconciled, so the guard belongs
        // here specifically.
        assert_eq!(film(5).expect("5").title, "TRANSFORMERS: SOLAR");
        assert_eq!(film(6).expect("6").title, "TRANSFORMERS: PHOTON");
        // Films 5 and 6 are clean because they were rewritten from the
        // current screenplays. THE REST OF THIS FILE IS NOT, and pretending
        // otherwise by scoping the test quietly would hide it.
        for n in [5u32, 6] {
            let f = film(n).expect("film");
            let hay = format!("{} {} {}", f.title, f.premise, f.acts.join(" "));
            assert!(
                !hay.contains("Matter Transformation Planet"),
                "film {n} resurrected the abandoned plot"
            );
        }
    }

    #[test]
    fn no_licensed_vocabulary() {
        // This universe's own words are Coalition / Hijacks / Blackstar Clans
        // / Corrupted / Halyard. A spine file is exactly where a borrowed term
        // spreads from, and SCREENPLAY_INVENTORY.md next door carries one.
        let banned = ["autobot", "decepticon", "optimus prime", "megatron", "bumblebee"];
        // Films 5 and 6 were rewritten from the current screenplays and are
        // clean. Asserted narrowly rather than for the whole file, because the
        // whole file is NOT clean — see the characterisation test below.
        for n in [5u32, 6] {
            let f = film(n).expect("film");
            let hay = format!("{} {} {}", f.title, f.premise, f.acts.join(" ")).to_lowercase();
            for b in banned {
                assert!(!hay.contains(b), "film {n} uses '{b}'");
            }
        }
    }

    #[test]
    fn the_inherited_contamination_is_counted_not_hidden() {
        // CHARACTERISATION, and the honest way to hold a defect I could not
        // responsibly fix. This file arrived written as fan fiction inside the
        // licensed universe — the same problem an earlier session found in all
        // thirteen screenplays — and Films 1-4 and 7-9 still carry both
        // licensed names and the abandoned Matter Transformation Planet plot.
        //
        // I did not silently rename Cleave or Keelwright: choosing what
        // this universe calls those characters is authoring, and inventing it
        // here would bury a creative decision inside a cleanup commit. The
        // Hijack/Hijacks spelling WAS fixed, because the code already settles
        // it and there is nothing to decide.
        //
        // So the count is asserted. It can only go DOWN — removing a term
        // fails this test, which is the prompt to update the number and know
        // the gap shrank. A silent scope-narrowing would have hidden it.
        // Count the DATA half only. `include_str!` reads this file including
        // this test, whose own comments name every banned term — so counting
        // the whole file counts the test that measures it. That self-match
        // has now bitten three times in this session's work; the split is the
        // fix rather than a cleverer needle.
        let src = include_str!("saga.rs");
        let data = &src[..src.find("// ---------------------------------------------------------------- the join --").unwrap_or(src.len())];
        let count = |needle: &str| data.matches(needle).count();
        // These needles are DATA, not vocabulary, and a blanket rename rewrote
        // them once — turning a licensed-mark counter into a counter of the
        // replacement names and pushing the total UP from 16 to 21 while
        // looking like a reintroduction. Anything that reasons ABOUT the
        // licensed words has to be excluded from a sweep by hand.
        let licensed = count("Autobot") + count("Decepticon") + count("Megatron")
            + count("Optimus Prime");
        let abandoned = count("Matter Transformation Planet");
        assert_eq!(
            (licensed, abandoned),
            (0, 8),
            "the inherited contamination changed. If it went DOWN, update these \
             numbers. If it went UP, something reintroduced licensed vocabulary \
             or the abandoned plot."
        );
    }

    #[test]
    fn the_spelling_matches_the_code() {
        // The abandoned material spells it "Hijack"; content.rs and patrol.rs
        // ship "Hijacks". A one-letter drift in the spine would propagate into
        // every claim written from it.
        // NEEDLES ASSEMBLED AT RUNTIME. Written as literals, the global
        // search-and-replace that fixed the spelling rewrote this test's own
        // needles too — so it began asserting the file must NOT contain the
        // correct spelling. Building them from pieces means no literal of the
        // wrong spelling exists in the file to be rewritten.
        let old_singular = format!("Hij{}k ", "a");
        let old_plural = format!("Hij{}ks", "a");
        for f in all_films() {
            let hay = format!("{} {}", f.premise, f.acts.join(" "));
            assert!(!hay.contains(&old_singular), "film {} uses the old spelling", f.number);
            assert!(!hay.contains(&old_plural), "film {} uses the old spelling", f.number);
            assert!(!f.premise.is_empty(), "film {} has no premise to check", f.number);
        }
    }
}

// ------------------------------------------------- films and the strata ----
//
// TWO AXES THAT LOOK LIKE ONE, AND A PROPOSAL THAT CONFLATES THEM.
//
// `SCREENPLAY_TO_GAME_INTEGRATION.md`, Step 1, says to add
// `Stratum::Ascension = 6` and `Stratum::Convergence = 7` to `archive.rs` so
// each trilogy gets its own layer. That is the wrong shape, and following it
// would break an ordering the archaeology layer depends on.
//
// `Stratum` is ARCHAEOLOGICAL DEPTH. Its own doc: "numbers them from the
// present downward — Layer 0 is what the player thinks the universe is on the
// first evening, Layer 5 is the part nobody has finished understanding." It
// derives `Ord`, and `from_u8` clamps to 5. Depth increases with the number.
//
// The films are NARRATIVE TIME, running forward. Film 9 is the LATEST thing
// that happens. Filing it as Layer 7 would place the newest event in the saga
// deeper than the Machine layer — older than the thing nobody understands —
// and every comparison on that enum would then be wrong in a way no type
// catches.
//
// The relationship is the other way round: the films are how the player digs.
// The deep strata are what the films UNCOVER. So a film's evidence sits near
// the surface, and what it reveals sits far below it.

/// Where a film's own evidence sits once it is buried — not what the film
/// uncovers, which is deeper by definition.
pub fn evidence_stratum(film_number: u32) -> crate::archive::Stratum {
    use crate::archive::Stratum;
    match film_number {
        // the Blackstar event itself: a god in the Earth, and the first time
        // the two histories touched
        1 => Stratum::Blackstar,
        // everything after it is this generation's own history
        2..=3 => Stratum::Recent,
        // and the Ascension and Convergence eras are the player's present
        _ => Stratum::Present,
    }
}

#[cfg(test)]
mod stratum_tests {
    use super::*;
    use crate::archive::Stratum;

    #[test]
    fn no_film_is_filed_deeper_than_the_event_that_started_it() {
        // The guard against the integration doc's Step 1. Everything below
        // Blackstar — the Black Archive, the Asteri, the Machine layer —
        // predates the saga entirely and is what the films UNCOVER. A film
        // filed down there would sort as older than its own backstory.
        for f in all_films() {
            let s = evidence_stratum(f.number);
            assert!(
                s <= Stratum::Blackstar,
                "film {} is filed at {:?}, deeper than the event that started the saga",
                f.number,
                s
            );
        }
        // and the deep layers stay strictly below every film
        for deep in [Stratum::BlackArchive, Stratum::Asteri, Stratum::Machine] {
            for f in all_films() {
                assert!(
                    evidence_stratum(f.number) < deep,
                    "film {} reaches the {deep:?} layer, which predates the saga",
                    f.number
                );
            }
        }
    }

    #[test]
    fn later_films_are_never_deeper_than_earlier_ones() {
        // Narrative time runs forward; depth runs backward. The two must not
        // cross, or "recent" and "deep" swap meaning somewhere in the middle
        // of the saga and nothing warns you.
        let mut prev = Stratum::Machine;
        for f in all_films() {
            let s = evidence_stratum(f.number);
            assert!(
                s <= prev,
                "film {} ({:?}) is deeper than film {} ({:?})",
                f.number,
                s,
                f.number - 1,
                prev
            );
            prev = s;
        }
        // and the ends are what they should be
        assert_eq!(evidence_stratum(1), Stratum::Blackstar);
        assert_eq!(evidence_stratum(9), Stratum::Present);
    }

    #[test]
    fn the_strata_enum_still_has_exactly_six_layers() {
        // If someone follows the integration doc and adds Ascension and
        // Convergence, this fails first and points at the reasoning above
        // rather than at a subtly wrong ordering discovered much later.
        assert_eq!(Stratum::all().len(), 6, "a stratum was added; see the note above this test");
        assert_eq!(Stratum::from_u8(200), Stratum::Machine, "from_u8 no longer clamps to the deepest");
    }
}
