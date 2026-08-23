//! THE BLACK ARCHIVE — the history under the game, as evidence.
//!
//! `provenance.rs` is the engine and knows no history. This is the history:
//! the propositions the universe's past consists of, and the objects lying
//! around the solar system that bear on them. It is the only file in the
//! archaeology layer that a writer needs to touch.
//!
//! ## The strata
//!
//! History here is layered, and the player does not meet the layers in
//! order. `Stratum` numbers them from the present downward — Layer 0 is
//! what the player thinks the universe is on the first evening, Layer 5 is
//! the part nobody has finished understanding. A find from Layer 4 can
//! surface before anything from Layer 1, and the map is designed to survive
//! that: a claim's standing depends on corroboration, never on depth.
//!
//! ## The three mechanics this table has to carry
//!
//! **Corroboration.** Independent sources agreeing is the only thing that
//! moves a claim. Note `array_reading_one/two/three` below: they are three
//! separate finds that all carry `source: "array_logs"`, so together they
//! reach UNKNOWN and stop. Blackstar's instrument cannot confirm itself.
//! That is not a limitation being worked around, it is the lesson.
//!
//! **Contradiction.** `facility_built_2003` is the official record, and it
//! is supported — by the Directorate's own file. Three unrelated finds put
//! the excavation in 1998, 1996 and 1994. The map does not average them; it
//! reports SOURCES DISAGREE, and the player learns the official history is
//! something somebody chose to write.
//!
//! **False leads.** `satellite_fragment_17`, `asteri_signal_beacon` and
//! `unicron_detection_scan` look exactly as important as anything else on
//! this list. They resolve to Cold War debris, a later civilisation's
//! imitation, and an instrument artefact. Chasing them is not punished and
//! not rewarded — it is answered. A universe where every glowing object
//! means something is a theme park.
//!
//! ## The rule this file is written under
//!
//! Never invent a system to hold a clue. Every number here that claims to
//! be physics comes from `orbits.rs`, which is a real long-period orbit
//! solver tested against Halley's published elements and would be worth
//! having if the Asteri had never been written. The prose is this game's
//! own terse voice, paraphrased from the treatment's beats — same contract
//! as `content.rs` and `intel.rs`.

use crate::campaign::CampaignLog;
use crate::provenance::{Bearing, Claim, Finding, Link, Medium, Prior};
use crate::raidlog::RaidLog;

/// How deep a claim sits. Lower is more recent.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Stratum {
    /// What the player thinks the universe is: ships, bridges, raids, factions.
    Present = 0,
    /// Wars, fallen colonies, older factions, the first space bridges.
    Recent = 1,
    /// Unicron in the Earth, and the first convergence anybody remembers.
    Blackstar = 2,
    /// The human clandestine societies, the observation network, the Watchers.
    BlackArchive = 3,
    /// The terrestrial civilisation that had all this first, and was wrong.
    Asteri = 4,
    /// Whatever the Asteri were themselves borrowing from.
    Machine = 5,
}

impl Stratum {
    pub fn all() -> [Stratum; 6] {
        use Stratum::*;
        [Present, Recent, Blackstar, BlackArchive, Asteri, Machine]
    }

    pub fn name(self) -> &'static str {
        match self {
            Stratum::Present => "THE PRESENT",
            Stratum::Recent => "RECENT HISTORY",
            Stratum::Blackstar => "THE BLACKSTAR ERA",
            Stratum::BlackArchive => "THE BLACK ARCHIVE",
            Stratum::Asteri => "ASTERI",
            Stratum::Machine => "BEFORE ASTERI",
        }
    }

    /// What the layer is, once the player can see it at all.
    pub fn gloss(self) -> &'static str {
        match self {
            Stratum::Present => "ships, bridges, raids, and who is shooting at whom",
            Stratum::Recent => "the wars and colonies this generation inherited",
            Stratum::Blackstar => "a god in the Earth, and the first time the two histories touched",
            Stratum::BlackArchive => "humans held this technology first, and hid it from each other",
            Stratum::Asteri => "a civilisation that had it all before, and made one arithmetic error",
            Stratum::Machine => "the Asteri did not invent it either",
        }
    }

    pub fn from_u8(n: u8) -> Stratum {
        Stratum::all()[(n as usize).min(5)]
    }
}


// ---------------------------------------------------------------- anchor ---

/// MYSTERY ANCHOR — how much of this history the game hands you already
/// believing.
///
/// The question this exists to answer: the player watched Unicron turn over
/// in the Earth's mantle during the campaign, so a map that files it under
/// UNKNOWN TECHNOLOGICAL RELATION is lying about what they know. But a
/// player who wants the universe strange should be able to have it strange,
/// and neither of us can tell from here which one reads better. So it is a
/// dial, and the dial is on the menu where it can be argued with.
///
/// The subtlety is that turning it UP is not simply "more tutorial". Received
/// knowledge enters `provenance` as a `Prior` — one source, contradictable —
/// so a high anchor preloads the OFFICIAL RECORD along with the folk memory,
/// and a good deal of the official record is false. Anchored high, the game
/// is about catching history in a lie; anchored low, it is about not knowing
/// there was a history at all. Both are the same tables read at a different
/// depth.
#[derive(Clone, Copy, Debug)]
pub struct Received {
    pub claim: &'static str,
    pub bearing: Bearing,
    /// Who says so. Shares a key with any FINDING that is the same
    /// observation, so believing the record and then finding the record is
    /// one source, not two.
    pub source: &'static str,
    /// The anchor level at or above which this is taken as known, 0..1.
    pub at: f32,
    /// How the setting screen phrases it.
    pub gloss: &'static str,
}

/// What is common knowledge, cheapest to hold at the top.
///
/// Ordered by `at` so the list reads as a descent: the things everybody
/// knows, then what the services admit among themselves, then the parts only
/// a specialist would assert.
pub static RECEIVED: &[Received] = &[
    Received {
        claim: "unicron_in_earth",
        bearing: Bearing::Supports,
        source: "lived_through_it",
        at: 0.20,
        gloss: "the thing in the mantle -- you were there",
    },
    Received {
        claim: "bridge_predates_coalition",
        bearing: Bearing::Supports,
        source: "lived_through_it",
        at: 0.35,
        gloss: "bridge geometry is inherited, whatever the Coalition teaches",
    },
    Received {
        claim: "convergence_was_not_first",
        bearing: Bearing::Supports,
        source: "lived_through_it",
        at: 0.45,
        gloss: "humans had handled this before Blackstar",
    },
    Received {
        claim: "facility_built_2003",
        bearing: Bearing::Supports,
        source: "the_record",
        at: 0.55,
        gloss: "the official excavation date, on file and uncontested",
    },
    Received {
        claim: "societies_existed",
        bearing: Bearing::Supports,
        source: "the_record",
        at: 0.65,
        gloss: "the services concede each other's existence",
    },
    Received {
        claim: "instrument_artefact",
        bearing: Bearing::Supports,
        source: "the_record",
        at: 0.75,
        gloss: "rim detections are known to be instrument noise",
    },
    Received {
        claim: "asteri_existed",
        bearing: Bearing::Supports,
        source: "scholarship",
        at: 0.85,
        gloss: "somebody was here first; the specialists argue about who",
    },
    Received {
        claim: "human_interface_unstable",
        bearing: Bearing::Supports,
        source: "scholarship",
        at: 0.95,
        gloss: "the interface eats its host, and this is not a secret",
    },
];

/// The four named stops on the dial, for a menu that wants words.
pub fn anchor_name(anchor: f32) -> &'static str {
    match anchor {
        a if a < 0.15 => "FRESH",
        a if a < 0.50 => "SPARSE",
        a if a < 0.80 => "FAMILIAR",
        _ => "HOMEY",
    }
}

/// What the player already believes at this setting.
pub fn priors(anchor: f32) -> Vec<Prior> {
    RECEIVED
        .iter()
        .filter(|r| anchor >= r.at)
        .map(|r| Prior { claim: r.claim, bearing: r.bearing, source: r.source })
        .collect()
}

/// The same list, with the prose, for the settings screen.
pub fn received_at(anchor: f32) -> Vec<&'static Received> {
    RECEIVED.iter().filter(|r| anchor >= r.at).collect()
}


// ----------------------------------------------------------------- lens ---

/// WHO IS READING. The same object, four different people.
///
/// The campaign is human and character-driven — three protagonists, plus the
/// villain campaign that lets you run the other side (the WfC/FoC move). A
/// Division-style world puts its history on terminals and in rooms, and the
/// reason that works is that the reader's expertise decides what they
/// NOTICE. Hand the same alloy plate to four people and you get four
/// reports, all honest, none complete.
///
/// So evidence splits in two:
///
/// * the FINDING is what is physically there. Invariant. It is what the
///   provenance engine consumes, and it does not change when the cast does.
/// * the READING is what somebody said about it. Varies by lens, and adding
///   a fifth character adds readings, not findings.
///
/// That seam is the one that survives the requirements moving. A villain
/// campaign is not a new evidence table; it is a new column.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lens {
    /// The Transformer on patrol. The ship's own archive, and the default
    /// voice everything is authored in.
    Patrol,
    /// Discipline, pulse architecture, a body that pays for what it does.
    DragonPulse,
    /// Charm and electromagnetics; reads rooms and power draw first.
    Voltage,
    /// The kinetic operative. Reads movement, distance, and what left.
    Kinetic,
    /// The Archivist. Already knows; is checking whether YOU do.
    Archivist,
}

impl Lens {
    pub fn all() -> [Lens; 5] {
        [Lens::Patrol, Lens::DragonPulse, Lens::Voltage, Lens::Kinetic, Lens::Archivist]
    }
    pub fn id(self) -> &'static str {
        match self {
            Lens::Patrol => "patrol",
            Lens::DragonPulse => "dragon",
            Lens::Voltage => "voltage",
            Lens::Kinetic => "kinetic",
            Lens::Archivist => "archivist",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Lens::Patrol => "PATROL",
            Lens::DragonPulse => "DRAGON PULSE",
            Lens::Voltage => "VOLTAGE",
            Lens::Kinetic => "KINETIC",
            Lens::Archivist => "THE ARCHIVIST",
        }
    }
    pub fn parse(s: &str) -> Option<Lens> {
        Lens::all().into_iter().find(|l| l.id() == s)
    }
    /// Which campaign this lens belongs to — the ship surface is the
    /// Transformer's, the terminals are the humans'.
    pub fn human(self) -> bool {
        self != Lens::Patrol
    }
}

/// One character's account of one finding.
pub struct Reading {
    pub finding: &'static str,
    pub lens: Lens,
    pub text: &'static str,
}

/// Character readings. DELIBERATELY PARTIAL.
///
/// Four voices across thirty-five findings is a hundred and forty pieces of
/// writing, and inventing them all to fill a matrix would produce a hundred
/// and forty pieces of bad writing. A lens with nothing to add falls back to
/// the finding's own text, which is the honest default: not every character
/// has a take on every object, and pretending otherwise is what makes
/// companion barks feel generated.
///
/// The ones below are where the four of them genuinely disagree.
pub static READINGS: &[Reading] = &[
    Reading {
        finding: "bunker_alloy",
        lens: Lens::DragonPulse,
        text: "i put my hand on it and my arm went quiet. not numb -- quiet. the \
               plate is drinking what i carry. whoever cast this knew what a \
               body like mine does, and built somewhere to put it.",
    },
    Reading {
        finding: "bunker_alloy",
        lens: Lens::Voltage,
        text: "okay so this room has no power. no conduit, no panel, nothing. and \
               my hair is standing up. that is not a draft. somebody left \
               something switched on down here and forgot which century.",
    },
    Reading {
        finding: "bunker_alloy",
        lens: Lens::Archivist,
        text: "plate seventeen. i catalogued this in 1971 and left it, because a \
               thing nobody can move is a thing nobody can lose. if you are \
               reading it now, you are further along than i wanted.",
    },
    Reading {
        finding: "farmhouse_photograph",
        lens: Lens::Kinetic,
        text: "look at the fence line, not the people. the grass is flattened in a \
               line from the ridge to the gate and back. whatever stood there \
               walked, and it walked the same route more than once.",
    },
    Reading {
        finding: "farmhouse_photograph",
        lens: Lens::Voltage,
        text: "a birthday. somebody's kid. and five years of paperwork says the \
               building behind them does not exist yet. man, i hate this job.",
    },
    Reading {
        finding: "farmhouse_photograph",
        lens: Lens::Archivist,
        text: "i have this photograph. mine is the same frame, one second later, \
               and the fence line is empty. one of us was given the wrong \
               second on purpose.",
    },
    Reading {
        finding: "interface_autopsy",
        lens: Lens::DragonPulse,
        text: "i know this chart. i have this chart. the damage is not from a \
               mistake -- it is the shape of doing it right, for long enough. \
               they were not injured. they were used correctly.",
    },
    Reading {
        finding: "rotation_ledger",
        lens: Lens::DragonPulse,
        text: "revision one gives them nine years. revision four gives them four. \
               i have been doing this for eleven. somebody drew that line and \
               kept filing it under scheduling.",
    },
    Reading {
        finding: "rotation_ledger",
        lens: Lens::Archivist,
        text: "i wrote revisions two and three. shortening the interval was the \
               only kindness available to me at the time, and it is the reason i \
               stopped believing this could be managed.",
    },
    Reading {
        finding: "interface_autopsy",
        lens: Lens::Voltage,
        text: "the tissue is carrying more than tissue carries. i can do the math \
               on a substation. i do not want to do it on a person.",
    },
    Reading {
        finding: "station_drive",
        lens: Lens::Voltage,
        text: "CONTINUOUS EARTH, terminated 2009. whole network, switched off in \
               one afternoon. you do not shut down a thing that size because it \
               stopped working. you shut it down because it started.",
    },
    Reading {
        finding: "station_drive",
        lens: Lens::Archivist,
        text: "i signed the termination order. it was the correct decision and i \
               would sign it again. what it saw is the only argument i have \
               ever lost.",
    },
    Reading {
        finding: "regolith_launch_scar",
        lens: Lens::Kinetic,
        text: "i can read a launch off a burn the way you read a skid. this one \
               went hard, straight up, no correction. nobody aims like that \
               unless they are throwing something away.",
    },
    Reading {
        finding: "modern_solution",
        lens: Lens::DragonPulse,
        text: "they were not careless. they measured for a hundred years and \
               published what the measurements allowed. discipline does not \
               protect you from asking a question your instrument cannot \
               answer. that is the part that frightens me.",
    },
    Reading {
        finding: "modern_solution",
        lens: Lens::Archivist,
        text: "four hundred and seventeen years. every civilisation that finds \
               this makes the same error in the same direction, because hope is \
               a systematic bias. i stopped hoping. that is the whole \
               difference between us.",
    },
    Reading {
        finding: "museum_specimen",
        lens: Lens::Kinetic,
        text: "sixty years behind glass, next to the fire exit, and nobody stole \
               it. i have cased this building twice. that case is the only thing \
               in here with no alarm on it. somebody wanted it findable.",
    },
    Reading {
        finding: "covenant_cache",
        lens: Lens::Archivist,
        text: "you were meant to find this one. i keep a copy in every vault i \
               empty, because the arithmetic is not the secret. the secret is \
               how long i have been holding it.",
    },
];

/// What this character says about this finding — their own account if they
/// have one, the object's plain description if they do not.
pub fn reading(finding: &Finding, lens: Lens) -> &'static str {
    READINGS
        .iter()
        .find(|r| r.finding == finding.id && r.lens == lens)
        .map(|r| r.text)
        .unwrap_or(finding.blurb)
}

/// Does this character have something of their own to say?
pub fn has_reading(finding_id: &str, lens: Lens) -> bool {
    READINGS.iter().any(|r| r.finding == finding_id && r.lens == lens)
}

/// Where a finding can be read, given how it is physically encountered.
///
/// This is why `Medium` is on the finding rather than on the UI: a hard
/// drive is read at a terminal because it is a hard drive, not because a
/// screen was assigned to it.
pub fn surface(m: Medium) -> &'static str {
    match m {
        Medium::Artifact => "the ship's chartroom -- carried back and put under a light",
        Medium::Terminal => "in place, on the machine holding it",
        Medium::Site => "standing in it; there is nothing to carry",
    }
}


// --------------------------------------------------------------- sources ---

/// The bodies of evidence, named.
///
/// `Finding::source` and `Received::source` are the INDEPENDENCE KEYS: two
/// findings sharing one are two pages of one document and corroborate each
/// other not at all. That makes them the highest-consequence free strings in
/// the project, and until now nothing constrained them.
///
/// A typo does not fail. It SPLITS one body of evidence into two, and the map
/// then reports corroboration the player never earned — a claim reaching
/// POSSIBLE or HIGH-CONFIDENCE off a single wreck read twice. The whole
/// ladder rests on this string matching.
///
/// So the namespace is closed, and each entry says what the body of evidence
/// actually is — which the ids alone do not ("scholarship" and "the_record"
/// are both institutions, and they are not the same institution).
pub struct Source {
    pub id: &'static str,
    pub what: &'static str,
}

pub static SOURCES: &[Source] = &[
    // -- instruments and their logs --
    Source { id: "array_logs", what: "Blackstar's own array; three readings, one instrument" },
    Source { id: "geo_scan", what: "a deep geological scan, and the array that ran it" },
    Source { id: "observatory_solution", what: "a modern recomputation of the Asteri arc" },
    Source { id: "debris_stream", what: "the object's own dust trail, counted back" },
    Source { id: "bridge_telemetry", what: "survey telemetry off a Coalition bridge anchor" },
    Source { id: "weather_archive", what: "an archived weather-satellite thermal plate" },
    // -- places, and what was found in them --
    Source { id: "la_bunker", what: "the Cold War bunker under Los Angeles, and its dead floor" },
    Source { id: "ground_station", what: "a satellite ground station, raided for something else" },
    Source { id: "museum_hall", what: "a regional museum case near the fire exit" },
    Source { id: "farmhouse", what: "a drawer in a farmhouse nobody has lived in since" },
    Source { id: "port_cctv", what: "a container port's tape, kept for an insurance claim" },
    Source { id: "medical_archive", what: "a medical file with the name cut out" },
    Source { id: "trench_survey", what: "spoil from the ridge cut, layered and datable" },
    Source { id: "mantle_survey", what: "a deep survey run for energon, not for this" },
    Source { id: "grail_void", what: "a lunar void GRAIL mapped and nobody entered" },
    Source { id: "luna_scar", what: "a glassed launch strip under four thousand years of regolith" },
    Source { id: "luna_relay", what: "a relay in a tube mouth, still answering a dead network" },
    Source { id: "europa_post", what: "the listening post's final hour" },
    Source { id: "io_forge", what: "the tide-forges, pouring into moulds nobody cut" },
    Source { id: "emptied_vaults", what: "a lattice pool vault, opened cleanly and emptied" },
    Source { id: "pluto_marker", what: "a boundary marker set into the ice" },
    Source { id: "carrion_wreck", what: "a hull that came apart on a straight edge" },
    Source { id: "cybertron_core", what: "core substrate, dated properly for once" },
    Source { id: "judgement_docket", what: "the Judgementor's open case file" },
    Source { id: "covenant_cache", what: "a tithe-vault, and the one thing in it worth taking" },
    Source { id: "asteri_chart", what: "the Asteri's last astronomical record and its working" },
    // -- the false leads' own bodies of evidence --
    Source { id: "cold_war_sat", what: "twentieth-century orbital hardware, stencilled and numbered" },
    Source { id: "launch_manifest", what: "a launch manifest from a programme cancelled for being boring" },
    Source { id: "imitation_beacon", what: "a later civilisation's forgery, badly soldered" },
    // -- institutions, and the things they assert --
    Source { id: "the_record", what: "the Directorate's official file, and what everyone repeats from it" },
    Source { id: "personnel_rotation", what: "a rotation schedule, four revisions of it" },
    Source { id: "scholarship", what: "the specialists, who argue but broadly agree" },
    Source { id: "lived_through_it", what: "what the player watched happen in the campaign" },
];

pub fn source(id: &str) -> Option<&'static Source> {
    SOURCES.iter().find(|s| s.id == id)
}

// ---------------------------------------------------------------- claims ---

pub static CLAIMS: &[Claim] = &[
    // ---- Layer 1 ----
    Claim {
        id: "bridge_predates_coalition",
        asserted: false,
        layer: Stratum::Recent as u8,
        text: "space-bridge geometry is older than the Coalition that teaches it",
    },
    // ---- Layer 2 ----
    Claim {
        id: "unicron_in_earth",
        asserted: false,
        layer: Stratum::Blackstar as u8,
        text: "the mass in Earth's mantle is a body, and it is not dead",
    },
    Claim {
        id: "convergence_was_not_first",
        asserted: false,
        layer: Stratum::Blackstar as u8,
        text: "the Blackstar-era contact was not the first time humans handled this technology",
    },
    // ---- Layer 3 ----
    Claim {
        id: "societies_existed",
        asserted: false,
        layer: Stratum::BlackArchive as u8,
        text: "human organisations held anomalous technology for decades, in secret, against each other",
    },
    Claim {
        id: "facility_built_2003",
        asserted: true,
        layer: Stratum::BlackArchive as u8,
        text: "the Directorate's excavation record: the site was constructed in 2003",
    },
    Claim {
        id: "continuous_earth",
        asserted: false,
        layer: Stratum::BlackArchive as u8,
        text: "an Earth-built global observation network existed, and was terminated deliberately",
    },
    Claim {
        id: "watchers_observed",
        asserted: false,
        layer: Stratum::BlackArchive as u8,
        text: "something was standing in the photographs, and it was not human",
    },
    Claim {
        id: "archivist_collecting",
        asserted: false,
        layer: Stratum::BlackArchive as u8,
        text: "one collector has been gathering every fragment for longer than any of these agencies existed",
    },
    Claim {
        id: "human_interface_unstable",
        asserted: false,
        layer: Stratum::BlackArchive as u8,
        text: "the human interface degrades its host; the technology was never meant for that body",
    },
    // ---- Layer 4 ----
    Claim {
        id: "asteri_existed",
        asserted: false,
        layer: Stratum::Asteri as u8,
        text: "a terrestrial civilisation held this technology thousands of years before any record admits",
    },
    Claim {
        id: "asteri_orbital_error",
        asserted: false,
        layer: Stratum::Asteri as u8,
        text: "their published return window is wrong, and the error is small enough to be honest",
    },
    Claim {
        id: "two_artifacts",
        asserted: false,
        layer: Stratum::Asteri as u8,
        text: "two pieces were separated at the end: one buried, one put into a long orbit",
    },
    // ---- Layer 5 ----
    Claim {
        id: "asteri_were_not_first",
        asserted: false,
        layer: Stratum::Machine as u8,
        text: "the Asteri encountered this technology; they did not originate it",
    },
    Claim {
        id: "architecture_is_machine",
        asserted: false,
        layer: Stratum::Machine as u8,
        text: "the energy architecture is machine-native, and every biological version is a misreading of it",
    },
    Claim {
        id: "edge_lattice",
        asserted: false,
        layer: Stratum::Machine as u8,
        text: "there is structure at the edge of the heliosphere that predates everyone arguing about it",
    },
    // ---- the mundane answers ----
    Claim {
        id: "cold_war_debris",
        asserted: false,
        layer: Stratum::Recent as u8,
        text: "some of the orbital wreckage is exactly what it looks like: twentieth-century hardware",
    },
    Claim {
        id: "later_forgery",
        asserted: false,
        layer: Stratum::Recent as u8,
        text: "a later civilisation copied Asteri marks onto its own work, badly",
    },
    Claim {
        id: "instrument_artefact",
        asserted: false,
        layer: Stratum::Present as u8,
        text: "the detection was the instrument, not the thing",
    },
];

// -------------------------------------------------------------- findings ---

const fn s(claim: &'static str) -> Link {
    Link { claim, bearing: Bearing::Supports }
}
const fn c(claim: &'static str) -> Link {
    Link { claim, bearing: Bearing::Contradicts }
}
const fn m(claim: &'static str) -> Link {
    Link { claim, bearing: Bearing::Mentions }
}

/// The mission that puts the player inside Blackstar's own systems, and so
/// releases his Array logs. Was `intel::REVEAL_AFTER`.
pub const ARRAY_LOG_GATE: &str = "the_ashes_court";

pub static FINDINGS: &[Finding] = &[
    // ================= EARTH — the archaeological layer =================
    Finding {
        id: "bunker_alloy",
        site: "earth",
        source: "la_bunker",
        medium: Medium::Artifact,
        gate: None,
        blurb: "a Cold War bunker under Los Angeles, four floors of filing and damp. the fifth \
                floor is not on the plans. a plate down there takes a scan the way cybertronian \
                hull does, and it was cast before anyone here had seen any.",
        links: &[s("societies_existed"), s("convergence_was_not_first"), m("architecture_is_machine")],
    },
    Finding {
        id: "bunker_roster",
        site: "earth",
        source: "la_bunker",
        medium: Medium::Terminal,
        gate: None,
        blurb: "a duty roster on the same dead floor. eleven names, no agency, thirty years of \
                signatures. the last shift signed in and never signed out.",
        links: &[s("societies_existed"), m("archivist_collecting")],
    },
    Finding {
        id: "directorate_record",
        site: "earth",
        source: "the_record",
        medium: Medium::Terminal,
        gate: None,
        blurb: "the official file, complete and unredacted, which is itself odd. FACILITY \
                CONSTRUCTED 2003. every subsequent document agrees with it, and each one cites \
                this one.",
        links: &[s("facility_built_2003"), s("societies_existed")],
    },
    Finding {
        id: "farmhouse_photograph",
        site: "earth",
        source: "farmhouse",
        medium: Medium::Artifact,
        gate: None,
        blurb: "a drawer in a farmhouse nobody has lived in since. a photograph dated 1998: two \
                people, a truck, a birthday. behind them on the ridge, the facility that was not \
                built for another five years -- and something standing at the fence line that is \
                the wrong height to be a person.",
        links: &[c("facility_built_2003"), s("watchers_observed")],
    },
    Finding {
        id: "trench_excavation_scar",
        site: "earth",
        source: "trench_survey",
        medium: Medium::Site,
        gate: None,
        blurb: "spoil from the ridge cut, settled into the trench in layers. the bottom layer \
                carries 1996 fallout isotopes. somebody was digging two years before the \
                photograph and seven before the paperwork.",
        links: &[c("facility_built_2003")],
    },
    Finding {
        id: "thermal_plate",
        site: "earth",
        source: "weather_archive",
        medium: Medium::Terminal,
        gate: None,
        blurb: "a weather satellite's thermal plate, archived and never examined. 1994. a heat \
                signature under the ridge with no road to it, no power line, and no season.",
        links: &[c("facility_built_2003"), m("continuous_earth")],
    },
    Finding {
        id: "museum_specimen",
        site: "earth",
        source: "museum_hall",
        medium: Medium::Artifact,
        gate: None,
        blurb: "a regional museum, a case near the fire exit, a label reading PROVENANCE \
                UNCERTAIN. the scan returns MATERIAL COMPOSITION: UNKNOWN and stops trying. it \
                has been behind that glass for sixty years.",
        links: &[s("asteri_existed"), s("convergence_was_not_first"), m("architecture_is_machine")],
    },
    Finding {
        id: "station_drive",
        site: "earth",
        source: "ground_station",
        medium: Medium::Terminal,
        gate: None,
        blurb: "a satellite ground station, raided for something else entirely. one drive still \
                spins. one file survives the corruption: PROJECT: CONTINUOUS EARTH / STATUS: \
                TERMINATED / DATE: 2009.",
        links: &[s("continuous_earth")],
    },
    Finding {
        id: "station_pointing_log",
        site: "earth",
        source: "ground_station",
        medium: Medium::Terminal,
        gate: None,
        blurb: "the same station's pointing log, the last four months of it. the dish spent more \
                time aimed at empty sky than at any catalogued bird. whatever it was talking to \
                is not in the ephemeris.",
        links: &[s("continuous_earth"), m("watchers_observed")],
    },
    Finding {
        id: "port_reel",
        site: "earth",
        source: "port_cctv",
        medium: Medium::Terminal,
        gate: None,
        blurb: "a container port's tape, kept because of an insurance claim. thirty frames where \
                a stack of empties is the wrong shape, and one where it is walking.",
        links: &[s("watchers_observed"), s("convergence_was_not_first")],
    },
    Finding {
        id: "interface_autopsy",
        site: "earth",
        source: "medical_archive",
        medium: Medium::Terminal,
        gate: None,
        blurb: "a medical file with the name cut out. the nervous tissue is carrying more energy \
                than tissue can, and the damage is not from an accident -- it is from working \
                correctly, for years.",
        links: &[s("human_interface_unstable"), s("societies_existed"), m("architecture_is_machine")],
    },
    Finding {
        id: "rotation_ledger",
        site: "earth",
        source: "personnel_rotation",
        medium: Medium::Terminal,
        gate: None,
        blurb: "a personnel rotation schedule, four revisions of it, filed with the \
                duty rosters where nobody would look twice. each revision cycles \
                interface staff out sooner than the last. nobody wrote down why the \
                interval kept shrinking, because everybody already knew.",
        links: &[s("human_interface_unstable"), s("societies_existed")],
    },
    Finding {
        id: "core_resonance",
        site: "earth",
        source: "mantle_survey",
        medium: Medium::Terminal,
        gate: None,
        blurb: "a deep survey run for energon, not for this. the return from the mantle is not \
                the return of a mass. it has a period.",
        links: &[s("unicron_in_earth")],
    },
    Finding {
        id: "buried_twin",
        site: "earth",
        source: "mantle_survey",
        medium: Medium::Terminal,
        gate: None,
        blurb: "the same survey, shallower, ignored at the time. a void with worked walls, a \
                long way above the thing with the period, and something small and dense sitting \
                in the middle of it.",
        links: &[s("two_artifacts"), m("asteri_existed")],
    },
    Finding {
        id: "cold_war_manifest",
        site: "earth",
        source: "launch_manifest",
        medium: Medium::Terminal,
        gate: None,
        blurb: "a launch manifest from a programme that was cancelled for being boring. it \
                accounts for every fragment anybody has been getting excited about, by serial \
                number.",
        links: &[s("cold_war_debris")],
    },
    // ================= LUNA =================
    Finding {
        id: "grail_void_seam",
        site: "luna",
        source: "grail_void",
        medium: Medium::Site,
        gate: None,
        blurb: "one of the voids GRAIL mapped and nobody entered. the walls did not collapse into \
                this shape. they were cut, and the cut is clean enough to read a tool from.",
        links: &[s("asteri_existed"), m("two_artifacts")],
    },
    Finding {
        id: "regolith_launch_scar",
        site: "luna",
        source: "luna_scar",
        medium: Medium::Site,
        gate: None,
        blurb: "a glassed strip under four thousand years of regolith. something left from here, \
                hard, in a direction nothing in this system needs to go.",
        links: &[s("two_artifacts"), m("asteri_existed")],
    },
    Finding {
        id: "relay_stub",
        site: "luna",
        source: "luna_relay",
        medium: Medium::Terminal,
        gate: None,
        blurb: "a relay in a tube mouth, still powered, still answering. it is handshaking with a \
                network that was switched off in 2009 and has not noticed.",
        links: &[s("continuous_earth")],
    },
    Finding {
        id: "satellite_fragment_17",
        site: "luna",
        source: "cold_war_sat",
        medium: Medium::Artifact,
        gate: None,
        blurb: "SATELLITE FRAGMENT 17. stencilled, numbered, deliberate. it looks like the first \
                real piece of something enormous.",
        links: &[s("cold_war_debris")],
    },
    // ================= EUROPA =================
    Finding {
        id: "listening_post_final",
        site: "europa",
        source: "europa_post",
        medium: Medium::Terminal,
        gate: None,
        blurb: "the listening post's last hour. it was not listening for life. it was running a \
                lattice-matching routine against something under the ice, and on the final line \
                the routine returns a match.",
        links: &[s("architecture_is_machine"), m("asteri_were_not_first")],
    },
    // ================= IO =================
    Finding {
        id: "forge_lattice",
        site: "io",
        source: "io_forge",
        medium: Medium::Site,
        gate: None,
        blurb: "the tide-forges pour into moulds nobody cut. the lattice that comes out is the \
                same lattice in the museum case, at a hundred times the scale, and the forge has \
                been doing it unattended since before the clans arrived.",
        links: &[s("architecture_is_machine"), s("asteri_were_not_first")],
    },
    // ================= GLACIEM =================
    Finding {
        id: "emptied_vault",
        site: "glaciem",
        source: "emptied_vaults",
        medium: Medium::Site,
        gate: None,
        blurb: "a lattice pool vault, opened cleanly and emptied. the inventory plate is still \
                bolted on. every line is struck through in the same hand, and the hand is older \
                than the vault.",
        links: &[s("archivist_collecting")],
    },
    // ================= PLUTO =================
    Finding {
        id: "pluto_line_marker",
        site: "pluto",
        source: "pluto_marker",
        medium: Medium::Site,
        gate: None,
        blurb: "a boundary marker on the line, set into the ice. the script is not cybertronian \
                and not human. it is warning somebody about something, politely.",
        links: &[s("asteri_were_not_first"), m("bridge_predates_coalition")],
    },
    Finding {
        id: "asteri_signal_beacon",
        site: "pluto",
        source: "imitation_beacon",
        medium: Medium::Terminal,
        gate: None,
        blurb: "a beacon transmitting an Asteri identifier on a loop. the identifier is correct. \
                the checksum convention it is wrapped in postdates the Asteri by two thousand \
                years, and the solder is worse.",
        links: &[s("later_forgery")],
    },
    // ================= OUTER RIM =================
    Finding {
        id: "bridge_survey",
        site: "outer_rim",
        source: "bridge_telemetry",
        medium: Medium::Terminal,
        gate: None,
        blurb: "survey telemetry from a bridge anchor the Coalition built. the geometry it \
                settles into is not the geometry it was cut to. it is relaxing into an older \
                solution that somebody else worked out first.",
        links: &[s("bridge_predates_coalition"), m("asteri_were_not_first")],
    },
    Finding {
        id: "unicron_detection_scan",
        site: "outer_rim",
        source: "geo_scan",
        medium: Medium::Terminal,
        gate: None,
        blurb: "a deep scan returning a Unicron-class signature at the rim, which should be \
                impossible and would change everything. re-running it with the array cooled \
                returns nothing. the signature was the array.",
        links: &[s("instrument_artefact")],
    },
    // ============ INSIDE A WARDEN — history the raid teaches ============
    Finding {
        id: "undelivered_verdict",
        site: "outer_rim",
        source: "judgement_docket",
        medium: Medium::Terminal,
        gate: Some("warden:judgementor"),
        blurb: "the docket, still open, in a courtroom the size of a city. the \
                defendant is named in a script the machine itself has no entry \
                for -- it was trying a civilisation older than its own \
                jurisdiction, and it never returned a sentence. the belt outside \
                is the ones it did.",
        links: &[s("asteri_were_not_first"), m("two_artifacts"), m("archivist_collecting")],
    },
    // ================= CARRION =================
    Finding {
        id: "carrion_hull_scar",
        site: "carrion",
        source: "carrion_wreck",
        medium: Medium::Site,
        gate: None,
        blurb: "a hull that came apart on something at the rim. the tear is not from a weapon. \
                the ship hit a straight edge, and there is no straight edge out there.",
        links: &[s("edge_lattice")],
    },
    // ================= SHIELD EDGE — Blackstar's own instrument =============
    Finding {
        id: "array_reading_one",
        site: "shield_edge",
        source: "array_logs",
        medium: Medium::Terminal,
        gate: Some(ARRAY_LOG_GATE),
        blurb: "act one, outer rim, probe response: geometry, not a signal. a faint lattice at \
                the edge of the heliosphere, filed and forgotten between tactical calls.",
        links: &[s("edge_lattice")],
    },
    Finding {
        id: "array_reading_two",
        site: "shield_edge",
        source: "array_logs",
        medium: Medium::Terminal,
        gate: Some(ARRAY_LOG_GATE),
        blurb: "act two, earth, urban containment: the corrupted stopped hunting for four seconds \
                and oriented outward -- toward the same edge. something even a broken echo of \
                unicron recognises as real.",
        links: &[s("edge_lattice"), m("unicron_in_earth")],
    },
    Finding {
        id: "array_reading_three",
        site: "shield_edge",
        source: "array_logs",
        medium: Medium::Terminal,
        gate: Some(ARRAY_LOG_GATE),
        blurb: "act three, the lhc, mid-fight: strength up a third time, clarity high enough the \
                array stops calling it noise and starts calling it structure. a sentence started \
                and never finished: 'every civilization that reached this far --'",
        links: &[s("edge_lattice"), m("asteri_were_not_first")],
    },
    // ================= CYBERTRON =================
    Finding {
        id: "substrate_sample",
        site: "cybertron",
        source: "cybertron_core",
        medium: Medium::Artifact,
        gate: None,
        blurb: "core substrate, dated properly for once. the energy architecture in it is older \
                than the planet's own record of inventing it. cybertron did not write this \
                either. it inherited it and stopped asking.",
        links: &[s("architecture_is_machine"), s("asteri_were_not_first")],
    },
    // ================= THE OBSERVATORY — the Asteri arithmetic ==============
    Finding {
        id: "star_chart_return",
        site: "earth",
        source: "asteri_chart",
        medium: Medium::Artifact,
        gate: None,
        blurb: "the last astronomical record they cut, and the most confident thing on it: \
                RETURN EXPECTED: 417 YEARS. read for a long time as proof they knew what was \
                coming back.",
        links: &[s("asteri_existed"), m("asteri_orbital_error"), m("two_artifacts")],
    },
    Finding {
        id: "chart_observation_arc",
        site: "earth",
        source: "asteri_chart",
        medium: Medium::Terminal,
        gate: None,
        blurb: "the working behind it. a hundred and four years of observations, every one of \
                them good, all of them taken while the object was close. nothing after it left.",
        links: &[m("asteri_orbital_error"), s("asteri_existed")],
    },
    Finding {
        id: "modern_solution",
        site: "earth",
        source: "observatory_solution",
        medium: Medium::Terminal,
        gate: None,
        blurb: "a modern recomputation from the same arc. the software errors out, twice. done by \
                hand it gives an answer, and the answer is that the arc never constrained the \
                period at all -- 417 years sits at the optimistic floor of a band with no \
                ceiling.",
        links: &[s("asteri_orbital_error"), s("asteri_existed")],
    },
    Finding {
        id: "debris_stream",
        site: "uncharted",
        source: "debris_stream",
        medium: Medium::Terminal,
        gate: None,
        blurb: "a dust stream on the object's own track, thinning the way a long-period trail \
                thins. counted back, it has been shedding for something on the order of four \
                thousand years, not four hundred.",
        links: &[s("asteri_orbital_error")],
    },
    Finding {
        id: "covenant_cache",
        site: "titan",
        source: "covenant_cache",
        medium: Medium::Artifact,
        gate: None,
        blurb: "a tithe-vault with one thing in it worth taking: a hand copy of the same orbital \
                solution, decades old, annotated. somebody worked this out a long time ago and \
                has been waiting quietly ever since.",
        links: &[s("asteri_orbital_error"), s("archivist_collecting")],
    },
];

/// The finding table's own entry point for the rest of the game.
pub fn finding(id: &str) -> Option<&'static Finding> {
    FINDINGS.iter().find(|f| f.id == id)
}

/// Resolve a finding's gate key.
///
/// The engine treats `Finding::gate` as opaque; this is the content layer's
/// vocabulary for it. A bare id is a campaign mission, which is what every
/// gate meant before raids could gate anything — so old rows keep working
/// untouched. `warden:<id>` is a raid clear.
///
/// The prefix rather than a second field because a gate is one precondition
/// with several possible kinds, and two nullable fields where one belongs
/// invites the state where both are set.
pub fn gate_open(gate: &str, camp: &CampaignLog, raids: &RaidLog) -> bool {
    match gate.split_once(':') {
        Some(("warden", id)) => raids.is_cleared(id),
        Some((kind, _)) => {
            debug_assert!(false, "unknown gate kind '{kind}'");
            false
        }
        None => camp.is_cleared(gate),
    }
}

pub fn claim(id: &str) -> Option<&'static Claim> {
    CLAIMS.iter().find(|c| c.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::campaign::CampaignLog;
    use crate::content::MISSIONS;
    use crate::patrol::ROUTE;
    use crate::provenance::{assay, assay_with, unfound_at, Dig, Standing};
    use std::collections::BTreeSet;

    fn all_found() -> BTreeSet<String> {
        FINDINGS.iter().map(|f| f.id.to_string()).collect()
    }

    #[test]
    fn every_finding_sits_on_a_real_destination() {
        for f in FINDINGS {
            assert!(
                ROUTE.iter().any(|d| d.id == f.site),
                "{} is sited on '{}', which is not in patrol::ROUTE",
                f.id,
                f.site
            );
        }
    }

    #[test]
    fn every_gate_names_something_real() {
        use crate::raidcon::BOSSES;
        for f in FINDINGS {
            let Some(g) = f.gate else { continue };
            match g.split_once(':') {
                Some(("warden", id)) => assert!(
                    BOSSES.iter().any(|b| b.id == id),
                    "{} is gated on warden '{id}', which is not in the roster",
                    f.id
                ),
                Some((kind, _)) => panic!("{} uses unknown gate kind '{kind}'", f.id),
                None => assert!(
                    MISSIONS.iter().any(|m| m.id == g),
                    "{} is gated on '{}', which is not a mission",
                    f.id,
                    g
                ),
            }
        }
    }

    #[test]
    fn gate_open_reads_both_vocabularies() {
        let mut camp = CampaignLog::default();
        let mut raids = RaidLog::default();
        assert!(!gate_open(ARRAY_LOG_GATE, &camp, &raids));
        assert!(!gate_open("warden:judgementor", &camp, &raids));

        camp.record(ARRAY_LOG_GATE);
        assert!(gate_open(ARRAY_LOG_GATE, &camp, &raids));
        // a mission clear must not open a raid gate
        assert!(!gate_open("warden:judgementor", &camp, &raids));

        raids.record("judgementor");
        assert!(gate_open("warden:judgementor", &camp, &raids));
        // ... and vice versa: clearing a warden opens only its own gate
        assert!(!gate_open("warden:mantis", &camp, &raids));
    }

    #[test]
    fn a_raid_carries_history_the_campaign_cannot_reach() {
        // the addendum's rule: a raid should teach something about the
        // universe rather than only paying out. the Judgementor's open docket
        // is Layer 5 evidence that exists nowhere else in the campaign.
        let f = finding("undelivered_verdict").expect("the docket");
        assert_eq!(f.gate, Some("warden:judgementor"));
        assert!(f.links.iter().any(|l| l.claim == "asteri_were_not_first"));

        // and it is genuinely additive, not a duplicate of a campaign source
        let others: Vec<_> = FINDINGS
            .iter()
            .filter(|o| o.id != f.id && o.source == f.source)
            .collect();
        assert!(others.is_empty(), "the docket shares a source and so corroborates nothing new");
    }

    #[test]
    fn every_link_names_a_real_claim() {
        for f in FINDINGS {
            for l in f.links {
                assert!(claim(l.claim).is_some(), "{} links to unknown claim '{}'", f.id, l.claim);
            }
        }
    }

    #[test]
    fn every_independence_key_is_a_declared_source() {
        // The consequence of a bad key is not a crash, it is MANUFACTURED
        // CORROBORATION: a split source makes one wreck read as two and a
        // claim reaches POSSIBLE off evidence the player never found.
        for f in FINDINGS {
            assert!(
                source(f.source).is_some(),
                "{} cites '{}', which is not a declared source — if that is a \
                 typo it silently doubles its own corroboration",
                f.id,
                f.source
            );
        }
        for r in RECEIVED {
            assert!(source(r.source).is_some(), "received '{}' cites '{}'", r.claim, r.source);
        }
    }

    #[test]
    fn no_declared_source_is_unused() {
        // a source nothing cites is either a typo's other half or a body of
        // evidence that got deleted; both are worth seeing
        for s in SOURCES {
            let used = FINDINGS.iter().any(|f| f.source == s.id)
                || RECEIVED.iter().any(|r| r.source == s.id);
            assert!(used, "source '{}' is declared and nothing cites it", s.id);
        }
        for s in SOURCES {
            assert!(s.what.len() > 20, "source '{}' does not say what it is", s.id);
        }
    }

    #[test]
    fn no_two_sources_are_one_typo_apart() {
        // The failure this whole table exists to prevent has a shape: two keys
        // differing by a character, where one was meant to be the other. A
        // closed namespace catches a key that matches NOTHING; it cannot catch
        // a key that accidentally matches a second real entry. This can.
        fn near(a: &str, b: &str) -> bool {
            if a.len().abs_diff(b.len()) > 1 {
                return false;
            }
            let (x, y): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
            if x.len() == y.len() {
                return x.iter().zip(&y).filter(|(p, q)| p != q).count() == 1;
            }
            // one insertion: the shorter must be a subsequence with one skip
            let (s, l) = if x.len() < y.len() { (&x, &y) } else { (&y, &x) };
            let (mut i, mut skipped) = (0usize, false);
            for c in l.iter() {
                if i < s.len() && s[i] == *c {
                    i += 1;
                } else if skipped {
                    return false;
                } else {
                    skipped = true;
                }
            }
            i == s.len()
        }
        for (i, a) in SOURCES.iter().enumerate() {
            for b in SOURCES.iter().skip(i + 1) {
                assert!(
                    !near(a.id, b.id),
                    "sources '{}' and '{}' differ by one character; if that is a \
                     typo the evidence is split and the map over-corroborates",
                    a.id,
                    b.id
                );
            }
        }
    }

    #[test]
    fn ids_are_unique() {
        let f: BTreeSet<_> = FINDINGS.iter().map(|f| f.id).collect();
        assert_eq!(f.len(), FINDINGS.len(), "duplicate finding id");
        let c: BTreeSet<_> = CLAIMS.iter().map(|c| c.id).collect();
        assert_eq!(c.len(), CLAIMS.len(), "duplicate claim id");
    }

    #[test]
    fn no_claim_is_unreachable() {
        // a claim nothing can ever bear on is a dead node on the map
        let a = assay(CLAIMS, FINDINGS, &all_found());
        for c in CLAIMS {
            assert!(
                a[c.id].standing().drawn(),
                "'{}' cannot be reached by any finding in the table",
                c.id
            );
        }
    }

    #[test]
    fn the_official_history_ends_up_contested() {
        // the mechanic, end to end: the Directorate's own file supports it,
        // three unrelated sources put the dig earlier, and the map refuses
        // to average them
        let a = assay(CLAIMS, FINDINGS, &all_found());
        let f = &a["facility_built_2003"];
        assert_eq!(f.standing(), Standing::Contested);
        assert_eq!(f.support(), 1, "only the Directorate's own file should support it");
        assert_eq!(f.opposition(), 3, "1998, 1996 and 1994 should each be independent");
    }

    #[test]
    fn blackstars_array_cannot_corroborate_itself() {
        // three readings, one instrument. without the Carrion wreck the edge
        // lattice stays a curiosity no matter how many logs are read.
        let logs: BTreeSet<String> =
            ["array_reading_one", "array_reading_two", "array_reading_three"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        let a = assay(CLAIMS, FINDINGS, &logs);
        assert_eq!(a["edge_lattice"].support(), 1, "one instrument is one source");
        assert_eq!(a["edge_lattice"].standing(), Standing::Unknown);

        let mut with_wreck = logs.clone();
        with_wreck.insert("carrion_hull_scar".to_string());
        let b = assay(CLAIMS, FINDINGS, &with_wreck);
        assert_eq!(b["edge_lattice"].standing(), Standing::Possible);
    }

    #[test]
    fn the_false_leads_resolve_to_something_boring() {
        let a = assay(CLAIMS, FINDINGS, &all_found());
        // each false lead's mundane answer is reachable ...
        for id in ["cold_war_debris", "later_forgery", "instrument_artefact"] {
            assert!(a[id].standing().drawn(), "{id} should be answerable");
            assert_ne!(
                a[id].standing(),
                Standing::High,
                "{id} is a shrug, not a revelation -- it should never read HIGH-CONFIDENCE"
            );
        }
        // ... and none of them touch the real history
        for f in FINDINGS.iter().filter(|f| {
            matches!(f.id, "satellite_fragment_17" | "asteri_signal_beacon" | "unicron_detection_scan")
        }) {
            for l in f.links {
                let c = claim(l.claim).unwrap();
                assert!(
                    c.layer <= Stratum::Recent as u8,
                    "false lead {} bears on {}, which is real history",
                    f.id,
                    l.claim
                );
            }
        }
    }

    #[test]
    fn the_deep_strata_are_reachable_without_the_gated_logs() {
        // a player who never boards Blackstar's flagship must still be able
        // to get to Layer 5 -- the gate is a shortcut, not a wall
        let ungated: BTreeSet<String> =
            FINDINGS.iter().filter(|f| f.gate.is_none()).map(|f| f.id.to_string()).collect();
        let a = assay(CLAIMS, FINDINGS, &ungated);
        assert_eq!(a["architecture_is_machine"].standing(), Standing::High);
        assert!(a["asteri_were_not_first"].standing() >= Standing::Possible);
    }

    #[test]
    fn the_asteri_error_is_corroborated_three_ways() {
        let a = assay(CLAIMS, FINDINGS, &all_found());
        assert_eq!(a["asteri_orbital_error"].standing(), Standing::High);
        assert!(a["asteri_orbital_error"].support() >= 3);
    }

    #[test]
    fn the_map_can_be_built_without_ever_raiding() {
        // a claim reachable ONLY through a six-player raid is a claim most
        // players never see. raids may DEEPEN the history (that is the whole
        // point of the Judgementor docket) but they must not be the sole
        // route to any of it.
        let no_raids: BTreeSet<String> = FINDINGS
            .iter()
            .filter(|f| !matches!(f.gate, Some(g) if g.starts_with("warden:")))
            .map(|f| f.id.to_string())
            .collect();
        let a = assay(CLAIMS, FINDINGS, &no_raids);
        for c in CLAIMS {
            assert!(
                a[c.id].standing().drawn(),
                "'{}' is only reachable by raiding",
                c.id
            );
        }
    }

    #[test]
    fn the_campaign_alone_carries_the_spine_of_the_history() {
        // stronger: without raids AND without the flagship logs, the deep
        // strata should still be arguable. The gated content is a shortcut
        // and a payoff, never the only door.
        let plain: BTreeSet<String> =
            FINDINGS.iter().filter(|f| f.gate.is_none()).map(|f| f.id.to_string()).collect();
        let a = assay(CLAIMS, FINDINGS, &plain);
        for id in ["asteri_existed", "asteri_orbital_error", "architecture_is_machine"] {
            assert!(
                a[id].standing() >= Standing::Possible,
                "'{id}' needs gated content to get anywhere: {:?}",
                a[id].standing()
            );
        }
    }

    #[test]
    fn every_finding_is_collectable_in_at_least_one_campaign() {
        // the audit that matters before somebody plays: a finding sited
        // somewhere with no way to recover it is content nobody will ever
        // read. Earth findings need an address (the archive era walks to
        // them); off-world findings need a destination the ship can reach.
        use crate::groundside;
        use crate::patrol::ROUTE;
        for f in FINDINGS {
            if f.site == "earth" {
                assert!(
                    groundside::place_of(f.id).is_some(),
                    "{} is on earth with no address, so nobody can stand near it",
                    f.id
                );
            } else {
                assert!(
                    ROUTE.iter().any(|d| d.id == f.site),
                    "{} is sited off the route entirely",
                    f.id
                );
            }
        }
    }

    #[test]
    fn the_generated_c_header_is_up_to_date() {
        // The C engine reads `archive_gen.h`, which is generated from THIS
        // file by `--arch emit-c`. Nothing checked that the checked-in header
        // still matches, so editing a table and forgetting to regenerate left
        // the C side reading a stale roster in silence — which is the exact
        // drift the generated header exists to prevent, reintroduced one
        // level up. `test_provenance.c` cannot catch it either: it validates
        // the header against itself.
        let path = crate::archsim::C_HEADER_PATH;
        let on_disk = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
        let fresh = crate::archsim::render_c();
        if on_disk != fresh {
            // point at the first difference rather than dumping two files
            let (a, b) = (on_disk.lines().collect::<Vec<_>>(), fresh.lines().collect::<Vec<_>>());
            let at = (0..a.len().max(b.len()))
                .find(|&i| a.get(i) != b.get(i))
                .unwrap_or(0);
            panic!(
                "{path} is stale — run `--arch emit-c`.\n  line {}: on disk {:?}\n                   line {}: expected {:?}",
                at + 1,
                a.get(at),
                at + 1,
                b.get(at)
            );
        }
    }

    #[test]
    fn a_dig_round_trips_through_the_save_line() {
        let mut d = Dig::default();
        for id in ["museum_specimen", "grail_void_seam", "not_a_real_find"] {
            d.record(id);
        }
        let back = Dig::from_line(&d.line(), FINDINGS);
        assert_eq!(back.count(), 2, "the bogus id should not survive a load");
        assert!(back.has("museum_specimen") && back.has("grail_void_seam"));
    }

    #[test]
    fn the_array_logs_stay_dark_until_the_flagship_falls() {
        // carried over from intel.rs, which this module replaced: the three
        // Array readings are Blackstar's own, and are unreachable until the
        // player is standing in his systems.
        let camp = CampaignLog::default();
        let none = BTreeSet::new();
        let open = unfound_at(FINDINGS, "shield_edge", &none, |g| camp.is_cleared(g));
        assert!(open.is_empty(), "the logs leaked before the flagship fell");

        let mut cleared = CampaignLog::default();
        cleared.record(ARRAY_LOG_GATE);
        let open = unfound_at(FINDINGS, "shield_edge", &none, |g| cleared.is_cleared(g));
        assert_eq!(open.len(), 3, "all three readings should arrive together");
    }

    #[test]
    fn clearing_something_else_does_not_leak_the_logs() {
        let mut camp = CampaignLog::default();
        for id in ["the_pluto_line", "the_scavenger_king", "the_red_extraction", "the_beach_opens"] {
            camp.record(id);
        }
        let open = unfound_at(FINDINGS, "shield_edge", &BTreeSet::new(), |g| camp.is_cleared(g));
        assert!(open.is_empty(), "an unrelated mission unlocked the flagship's own data");
    }

    #[test]
    fn the_reveal_mission_is_late_but_not_the_last_one() {
        // also from intel.rs: the logs must land BEFORE the campaign's finale,
        // so Blackstar reads them and still has a choice to make.
        let m = MISSIONS.iter().find(|m| m.id == ARRAY_LOG_GATE).expect("gate must be a mission");
        assert!(
            MISSIONS.iter().any(|o| o.order > m.order),
            "the flagship boarding is already the last mission"
        );
    }

    #[test]
    fn the_anchor_is_monotone_and_bounded() {
        let mut prev = 0;
        for step in 0..=20 {
            let n = priors(step as f32 / 20.0).len();
            assert!(n >= prev, "priors shrank as the anchor rose");
            prev = n;
        }
        assert!(priors(0.0).is_empty(), "FRESH should assume nothing");
        assert_eq!(priors(1.0).len(), RECEIVED.len(), "HOMEY should assume all of it");
    }

    #[test]
    fn every_received_claim_is_real_and_reachable_by_evidence() {
        for r in RECEIVED {
            assert!(claim(r.claim).is_some(), "received names unknown claim '{}'", r.claim);
            assert!(
                (0.0..=1.0).contains(&r.at),
                "'{}' sits at {}, off the dial",
                r.claim,
                r.at
            );
            // belief must never be the ONLY way to a node -- otherwise FRESH
            // makes that history permanently unreachable
            assert!(
                FINDINGS.iter().any(|f| f.links.iter().any(|l| l.claim == r.claim)),
                "'{}' is believable but not findable; FRESH would strand it",
                r.claim
            );
        }
    }

    #[test]
    fn a_high_anchor_hands_you_a_lie_to_catch() {
        // the point of the dial: HOMEY preloads the official record, and the
        // official record is wrong about the excavation date.
        let believed = priors(1.0);
        let a = assay_with(CLAIMS, FINDINGS, &BTreeSet::new(), &believed);
        assert_eq!(a["facility_built_2003"].standing(), Standing::Unknown);

        // finding the file itself changes nothing -- same source
        let with_file: BTreeSet<String> = ["directorate_record"].iter().map(|s| s.to_string()).collect();
        let b = assay_with(CLAIMS, FINDINGS, &with_file, &believed);
        assert_eq!(b["facility_built_2003"].support(), 1, "belief and its own file are one source");

        // and one dig date is enough to start the fight
        let with_dig: BTreeSet<String> = ["trench_excavation_scar"].iter().map(|s| s.to_string()).collect();
        let c = assay_with(CLAIMS, FINDINGS, &with_dig, &believed);
        assert_eq!(c["facility_built_2003"].standing(), Standing::Contested);
    }

    #[test]
    fn fresh_leaves_the_whole_map_dark() {
        let a = assay_with(CLAIMS, FINDINGS, &BTreeSet::new(), &priors(0.0));
        assert!(a.values().all(|x| !x.standing().drawn()), "FRESH should start blank");
    }

    #[test]
    fn a_lens_without_an_opinion_falls_back_to_the_object() {
        // partial coverage is the design, not a gap: a character with
        // nothing to add reads the plain description rather than a
        // manufactured one
        let f = finding("bunker_alloy").unwrap();
        assert_eq!(reading(f, Lens::Patrol), f.blurb, "patrol is the authored voice");
        assert_ne!(reading(f, Lens::DragonPulse), f.blurb);
        assert_ne!(reading(f, Lens::Voltage), f.blurb);
        // kinetic has no take on this one
        assert!(!has_reading("bunker_alloy", Lens::Kinetic));
        assert_eq!(reading(f, Lens::Kinetic), f.blurb);
        // and every finding is readable through every lens, always
        for f in FINDINGS {
            for l in Lens::all() {
                assert!(!reading(f, l).is_empty(), "{} went blank under {:?}", f.id, l);
            }
        }
    }

    #[test]
    fn every_reading_names_a_real_finding_and_does_not_duplicate() {
        let mut seen = BTreeSet::new();
        for r in READINGS {
            assert!(finding(r.finding).is_some(), "reading for unknown finding '{}'", r.finding);
            assert!(
                seen.insert((r.finding, r.lens.id())),
                "two readings for {} as {}",
                r.finding,
                r.lens.id()
            );
            assert!(!r.text.is_empty());
        }
    }

    #[test]
    fn lens_ids_round_trip_and_split_the_two_campaigns() {
        for l in Lens::all() {
            assert_eq!(Lens::parse(l.id()), Some(l));
            assert!(!l.name().is_empty());
        }
        assert!(Lens::parse("nobody").is_none());
        // the ship surface is the transformer's; the terminals are the humans'
        assert!(!Lens::Patrol.human());
        assert!(Lens::all().iter().filter(|l| l.human()).count() == 4);
    }

    #[test]
    fn the_medium_decides_the_surface_and_the_ship_only_sees_artifacts() {
        use crate::provenance::Medium;
        for f in FINDINGS {
            assert!(!surface(f.medium).is_empty());
        }
        // all three media are actually used -- an unused variant means the
        // split is decorative
        for m in [Medium::Artifact, Medium::Terminal, Medium::Site] {
            assert!(FINDINGS.iter().any(|f| f.medium == m), "{m:?} is never used");
        }
        // the Array logs are records on Blackstar's own machine: read in
        // place, never carried home
        for id in ["array_reading_one", "array_reading_two", "array_reading_three"] {
            assert_eq!(finding(id).unwrap().medium, Medium::Terminal);
        }
        // a cut wall cannot be carried
        assert_eq!(finding("grail_void_seam").unwrap().medium, Medium::Site);
    }

    /// Pull the ids out of a ```canon:<kind> fenced block in the bible.
    fn canon_block(doc: &str, kind: &str) -> Vec<String> {
        let open = format!("```canon:{kind}");
        let start = doc.find(&open).unwrap_or_else(|| panic!("no ```canon:{kind} block in STORY_BIBLE.md"));
        let body = &doc[start + open.len()..];
        let end = body.find("```").expect("unterminated canon block");
        body[..end]
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    }

    fn bible() -> String {
        // the test runs from the crate root; the doc is part of the crate's
        // own source of truth, so a missing file is a failure, not a skip
        std::fs::read_to_string("docs/STORY_BIBLE.md")
            .expect("docs/STORY_BIBLE.md is missing -- it is checked content, not decoration")
    }

    /// Documentation rots because nothing breaks when it does. This breaks.
    ///
    /// Set equality both ways, per kind: the bible cannot silently omit
    /// content that was added, and cannot keep naming content that was
    /// deleted. Prose is not checked -- only the ids -- so keeping the
    /// explanations honest is still a human job, but the INVENTORY cannot
    /// drift without a red test.
    #[test]
    fn the_story_bible_is_current() {
        let doc = bible();

        let listed = canon_block(&doc, "claims");
        let actual: Vec<String> = CLAIMS.iter().map(|c| c.id.to_string()).collect();
        assert_eq!(
            listed.iter().collect::<BTreeSet<_>>(),
            actual.iter().collect::<BTreeSet<_>>(),
            "STORY_BIBLE.md's canon:claims block is out of date with archive::CLAIMS"
        );

        let listed = canon_block(&doc, "lenses");
        let actual: Vec<String> = Lens::all().iter().map(|l| l.id().to_string()).collect();
        assert_eq!(
            listed.iter().collect::<BTreeSet<_>>(),
            actual.iter().collect::<BTreeSet<_>>(),
            "STORY_BIBLE.md's canon:lenses block is out of date with archive::Lens"
        );

        let listed = canon_block(&doc, "films");
        let actual: Vec<String> =
            crate::content::FILMS.iter().map(|f| f.title.to_string()).collect();
        assert_eq!(
            listed, actual,
            "STORY_BIBLE.md's canon:films block is out of date with content::FILMS (order matters)"
        );

        let listed = canon_block(&doc, "strata");
        assert_eq!(listed.len(), Stratum::all().len(), "canon:strata has the wrong depth");
        for (i, name) in listed.iter().enumerate() {
            assert_eq!(
                format!("{:?}", Stratum::all()[i]),
                *name,
                "canon:strata entry {i} disagrees with archive::Stratum"
            );
        }
    }

    /// The bible claims specific things about the code. Check the ones that
    /// are cheap to check, because a doc that is confidently wrong about a
    /// mechanic is worse than one that is silent.
    #[test]
    fn the_bible_does_not_lie_about_the_mechanics() {
        let doc = bible();

        // it says the Array logs are gated on the_ashes_court
        assert!(doc.contains(ARRAY_LOG_GATE));
        assert!(FINDINGS.iter().any(|f| f.gate == Some(ARRAY_LOG_GATE)));

        // it names three false leads and three mundane answers
        for id in ["satellite_fragment_17", "asteri_signal_beacon", "unicron_detection_scan"] {
            assert!(doc.contains(id), "the bible stopped naming false lead {id}");
            assert!(finding(id).is_some(), "the bible names a finding that no longer exists: {id}");
        }

        // it says human_interface_unstable has exactly one supporting finding.
        // when that is fixed, this fails and the OPEN question must be struck.
        let n = FINDINGS
            .iter()
            .filter(|f| {
                f.links.iter().any(|l| {
                    l.claim == "human_interface_unstable" && l.bearing == Bearing::Supports
                })
            })
            .count();
        // was 1, and STORY_BIBLE.md carried it as an OPEN question. the
        // rotation ledger is the independent second source, so the claim can
        // now reach POSSIBLE at FRESH. keep the tripwire pointed at the
        // current number so the doc and the table stay married.
        assert_eq!(n, 2, "human_interface_unstable support changed -- update STORY_BIBLE.md");
        assert!(
            !doc.contains("human_interface_unstable` has exactly one"),
            "the bible still lists a solved OPEN question"
        );

        // the campaign-shape section makes a hard claim: nothing is
        // campaign-only. if that stops being true the doc is lying.
        assert!(doc.contains("nothing is campaign-only"));
        assert_eq!(
            crate::curriculum::open_world_kit().len(),
            crate::curriculum::CURRICULUM.len(),
            "STORY_BIBLE.md claims nothing is campaign-only, and something now is"
        );
        // and it names the funnel's hinge
        assert!(doc.contains("the_far_gate"));
        assert!(crate::curriculum::taught_by("the_far_gate").len() == 1);

        // the LA section now claims the ASYMMETRY: no base in the archive
        // era, one in the present. this replaced a shared-base claim that the
        // source contradicted, and the guard caught the change.
        assert!(doc.contains("no secret base"));
        assert!(
            crate::groundside::home(crate::groundside::Era::Archive).is_none(),
            "the bible says the archive era has no base and it now has one"
        );
        assert!(crate::groundside::home(crate::groundside::Era::Present).is_some());
        // and that every earth finding has an address
        for f in FINDINGS.iter().filter(|f| f.site == "earth") {
            assert!(
                crate::groundside::place_of(f.id).is_some(),
                "the bible claims every earth finding has an address; {} does not",
                f.id
            );
        }

        // it says the anchor default is 0.45
        assert!(doc.contains("0.45"));
        assert!((crate::config::Config::default().story.anchor - 0.45).abs() < 1e-6);
    }
}
