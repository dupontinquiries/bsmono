//! PROVENANCE — what the evidence supports, and how sure that is.
//!
//! The archaeological map is not a checklist of collected lore. It is an
//! inference: the player finds objects, objects bear on historical CLAIMS,
//! and a claim's standing rises as independent finds corroborate it and
//! falls when finds contradict it. This module is that inference and
//! nothing else. It holds no history, names no civilisation, and would
//! compile unchanged in a game about something else — the content lives in
//! `archive.rs`, and the separation is the point: the evidence tables will
//! grow with every film and every raid, and the rule for reading them
//! should not move when they do.
//!
//! ## The ladder
//!
//! One find is a curiosity. Two independent finds are a pattern. Three are
//! an argument. That is the whole of `Standing`, and the thresholds are
//! deliberately small — a player who has to collect eleven of something is
//! doing inventory, not archaeology.
//!
//! ## Independence, and why it is counted rather than assumed
//!
//! Corroboration only means anything if the finds are independent. Two
//! fragments of the SAME satellite saying the same thing is one
//! observation, not two, and a system that scored it as two would let a
//! player farm certainty out of a single wreck. So each finding declares a
//! `source` — the body of evidence it came from — and `standing` counts
//! DISTINCT sources, not finds.
//!
//! ## Contradiction is a first-class outcome
//!
//! A claim with strong support and strong opposition is not a claim with a
//! medium score. It is a live dispute, and the map should say so, because
//! the moment the official timeline collides with a satellite record is
//! the moment the player learns the official timeline is a story somebody
//! chose to tell. `Standing::Contested` exists so that collision has
//! somewhere to land instead of quietly cancelling out.
//!
//! ## What this does NOT do
//!
//! No transitive inference. If A supports B and B supports C, this module
//! does not conclude anything about C. Chained inference over a graph the
//! player is still filling in produces confident nonsense, and a wrong
//! conclusion the game states firmly is worse than a gap it leaves open.
//! Chains here are for the player to make.

use std::collections::{BTreeMap, BTreeSet};

/// How much weight one body of evidence carries on one claim.
///
/// Deliberately coarse. A fragment either bears on a question or it does
/// not; fine-grained weights would imply a precision the fiction does not
/// have and would make the tables miserable to author.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bearing {
    /// The find is consistent with the claim and hard to explain without it.
    Supports,
    /// The find is inconsistent with the claim.
    Contradicts,
    /// The find is ABOUT the claim without settling it — the shape of a
    /// question, not an answer. Puts a claim on the map without moving it.
    Mentions,
}

/// One piece of evidence's bearing on one claim.
#[derive(Clone, Copy, Debug)]
pub struct Link {
    pub claim: &'static str,
    pub bearing: Bearing,
}

/// How a piece of evidence is physically encountered, which decides WHERE
/// it can be read.
///
/// Presentation metadata: the inference ignores it entirely. It lives on
/// `Finding` rather than in a parallel table because it is a property of the
/// object — a hard drive is read at the terminal it is plugged into, a
/// scorched plate is carried back to the ship — and parallel tables drift.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Medium {
    /// A thing you pick up. Goes back to the ship to be looked at properly.
    Artifact,
    /// Records: a drive, a log, a file. Read in place, on the machine that
    /// holds them, because carrying a dead terminal home proves nothing.
    Terminal,
    /// The evidence IS the place — a cut wall, a launch scar, a void. It
    /// cannot be moved and has to be read standing in it.
    Site,
}

/// Something the player can find and read.
///
/// `source` is the independence key: two findings sharing a source are two
/// pages of one document, and corroborate each other not at all.
#[derive(Clone, Copy, Debug)]
pub struct Finding {
    pub id: &'static str,
    /// Where it is. A destination id from `patrol::ROUTE`, checked by test.
    pub site: &'static str,
    /// What the player sees on scanning it, in the game's own terse voice.
    pub blurb: &'static str,
    /// The body of evidence this belongs to. Fragments of one wreck, pages
    /// of one archive, and readings from one instrument share a source.
    pub source: &'static str,
    /// Where this can be read. Presentation only; see `Medium`.
    pub medium: Medium,
    /// An OPAQUE precondition key. This module never interprets it — it is
    /// a string the content layer hands to a resolver of its own choosing
    /// (in this game, a campaign mission id). Kept here rather than in a
    /// side table so a finding's reachability sits next to the finding,
    /// but kept meaningless here so the engine stays content-free.
    pub gate: Option<&'static str>,
    pub links: &'static [Link],
}

/// A historical proposition the evidence is about.
#[derive(Clone, Copy, Debug)]
pub struct Claim {
    pub id: &'static str,
    /// Is this proposition somebody's ASSERTION rather than a fact about the
    /// world? An official date, a published figure, a party line.
    ///
    /// It matters because such a claim does not exist until somebody makes
    /// it. Finding a 1996 excavation layer is a refutation of the official
    /// record only once you have met the official record; before that it is
    /// a 1996 excavation layer. Without this the map cheerfully draws
    /// "SOURCES DISAGREE" over a record the player has never seen, which is
    /// how the low end of the mystery anchor first read, and it was
    /// nonsense.
    pub asserted: bool,
    /// Which stratum of history it belongs to. `archive::Stratum as u8`,
    /// kept as a plain integer so this module stays content-free.
    pub layer: u8,
    /// The claim, stated flatly, as a proposition that could be false.
    pub text: &'static str,
}

/// Where a claim stands given what has been found.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Standing {
    /// Nothing found bears on it. Not drawn.
    Dark,
    /// Named by something, corroborated by nothing.
    Unknown,
    /// Two independent sources agree.
    Possible,
    /// Three or more independent sources agree, with nothing against.
    High,
    /// Sources disagree. The interesting one.
    Contested,
}

impl Standing {
    /// The label the map prints. Phrased as the map's own hedging, not as
    /// an enum name leaking into the fiction.
    pub fn label(self) -> &'static str {
        match self {
            Standing::Dark => "--",
            Standing::Unknown => "UNKNOWN TECHNOLOGICAL RELATION",
            Standing::Possible => "POSSIBLE SHARED ORIGIN",
            Standing::High => "HIGH-CONFIDENCE CONNECTION",
            Standing::Contested => "SOURCES DISAGREE",
        }
    }

    /// A compact tag, for rows in a console where the full sentence does not
    /// fit. The long `label` is for the detail view; this is for the list.
    pub fn short(self) -> &'static str {
        match self {
            Standing::Dark => "--",
            Standing::Unknown => "unknown",
            Standing::Possible => "possible",
            Standing::High => "HIGH",
            Standing::Contested => "CONTESTED",
        }
    }

    pub fn drawn(self) -> bool {
        self != Standing::Dark
    }
}

/// The tally behind one claim's standing, kept so the map can show its work.
///
/// A player who is told "HIGH-CONFIDENCE" and cannot see why has been given
/// a verdict, not evidence. `for_` and `against` are the citation list.
#[derive(Clone, Default, Debug)]
pub struct Assay {
    pub for_: Vec<&'static str>,
    pub against: Vec<&'static str>,
    pub mentions: Vec<&'static str>,
    /// Mirrored from `Claim::asserted` while tallying, so `standing` can be
    /// read without carrying the claim alongside it.
    pub needs_claimant: bool,
}

impl Assay {
    /// Distinct supporting sources.
    pub fn support(&self) -> usize {
        self.for_.iter().collect::<BTreeSet<_>>().len()
    }
    /// Distinct contradicting sources.
    pub fn opposition(&self) -> usize {
        self.against.iter().collect::<BTreeSet<_>>().len()
    }
}

/// How many independent sources it takes to firm a claim up.
///
/// Split out because it is a FEEL number, not a fact: two-for-a-pattern and
/// three-for-an-argument is a reasonable guess about pacing and nothing more,
/// and the only way to know is to play far enough that the map either firms up
/// too fast to feel earned or too slow to feel responsive. Tunable at runtime
/// (`cfg arch_possible_at` / `arch_high_at`) so that is a save-file edit rather
/// than a rebuild.
#[derive(Clone, Copy, Debug)]
pub struct Ladder {
    pub possible_at: usize,
    pub high_at: usize,
}

impl Default for Ladder {
    fn default() -> Ladder {
        Ladder { possible_at: 2, high_at: 3 }
    }
}

impl Ladder {
    pub fn from_cfg(c: &crate::config::Config) -> Ladder {
        Ladder { possible_at: c.story.possible_at as usize, high_at: c.story.high_at as usize }
            .sane()
    }

    /// Keep the rungs in order whatever the save file says: a ladder where
    /// HIGH is easier than POSSIBLE would make standings non-monotonic in the
    /// evidence, which is worse than any pacing it could buy.
    pub fn sane(self) -> Ladder {
        let possible_at = self.possible_at.max(1);
        Ladder { possible_at, high_at: self.high_at.max(possible_at + 1) }
    }
}

impl Assay {
    /// Standing at the stock ladder. Most callers want this.
    pub fn standing(&self) -> Standing {
        self.standing_with(Ladder::default())
    }

    /// Standing at a tuned ladder.
    pub fn standing_with(&self, ladder: Ladder) -> Standing {
        let l = ladder.sane();
        let (s, o) = (self.support(), self.opposition());
        // nobody has made this claim yet, so there is nothing to dispute
        if self.needs_claimant && s == 0 {
            return Standing::Dark;
        }
        match (s, o) {
            (0, 0) if self.mentions.is_empty() => Standing::Dark,
            (0, 0) => Standing::Unknown,
            // any real disagreement is a dispute, not an average
            (_, o) if o > 0 && s > 0 => Standing::Contested,
            // contradicted with nothing for it is still a live question:
            // the player has learned the claim is probably false, which is
            // knowledge, and the map should keep showing it
            (0, _) => Standing::Contested,
            (s, _) if s < l.possible_at => Standing::Unknown,
            (s, _) if s < l.high_at => Standing::Possible,
            _ => Standing::High,
        }
    }
}

/// Something taken as known before any digging — what the culture already
/// believes, or what the official record already asserts.
///
/// A prior is evidence like any other, and deliberately so: it carries a
/// `source`, so it counts once and no more, and it can be CONTRADICTED.
/// That is the whole reason received knowledge is modelled here rather than
/// as a flag that pre-reveals map nodes. "Everyone knows the site was built
/// in 2003" and "here is the file saying 2003" are the same claim from the
/// same source, and three independent dig dates should beat both together
/// exactly as hard as they beat either alone.
#[derive(Clone, Copy, Debug)]
pub struct Prior {
    pub claim: &'static str,
    pub bearing: Bearing,
    pub source: &'static str,
}

/// The map: every claim's standing given a set of discovered finding ids.
///
/// Unknown finding ids are ignored rather than erroring — the save file is
/// lenient in, canonical out, same contract as `raidlog.rs`, so a save from
/// a build that knew about a finding this one does not is a dropped line,
/// not a crash.
pub fn assay(
    claims: &'static [Claim],
    findings: &'static [Finding],
    found: &BTreeSet<String>,
) -> BTreeMap<&'static str, Assay> {
    assay_with(claims, findings, found, &[])
}

/// The same, starting from what is already believed.
pub fn assay_with(
    claims: &'static [Claim],
    findings: &'static [Finding],
    found: &BTreeSet<String>,
    priors: &[Prior],
) -> BTreeMap<&'static str, Assay> {
    let mut out: BTreeMap<&'static str, Assay> = BTreeMap::new();
    for c in claims {
        out.insert(c.id, Assay { needs_claimant: c.asserted, ..Assay::default() });
    }
    let push = |a: &mut Assay, b: Bearing, src: &'static str| match b {
        Bearing::Supports => a.for_.push(src),
        Bearing::Contradicts => a.against.push(src),
        Bearing::Mentions => a.mentions.push(src),
    };
    for p in priors {
        if let Some(a) = out.get_mut(p.claim) {
            push(a, p.bearing, p.source);
        }
    }
    for f in findings {
        if !found.contains(f.id) {
            continue;
        }
        for l in f.links {
            if let Some(a) = out.get_mut(l.claim) {
                push(a, l.bearing, f.source);
            }
        }
    }
    out
}

/// Everything findable at a site right now: not already found, and not
/// behind a closed gate.
///
/// `open` decides what a gate key means. The engine does not know, which is
/// what lets the same engine serve a mission-gated find and a gate-free one
/// without learning the word "mission".
pub fn unfound_at(
    findings: &'static [Finding],
    site: &str,
    found: &BTreeSet<String>,
    open: impl Fn(&str) -> bool,
) -> Vec<&'static Finding> {
    findings
        .iter()
        .filter(|f| f.site == site && !found.contains(f.id))
        .filter(|f| f.gate.map(|g| open(g)).unwrap_or(true))
        .collect()
}

/// The record of what has been dug up. Rides the save file as one
/// comma-separated `finds` line of ids — ids, not indices, so the tables
/// can grow and reorder without corrupting old saves.
#[derive(Clone, Default, Debug)]
pub struct Dig {
    found: BTreeSet<String>,
}

impl Dig {
    pub fn from_line(line: &str, findings: &'static [Finding]) -> Dig {
        let mut d = Dig::default();
        for id in line.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if findings.iter().any(|f| f.id == id) {
                d.found.insert(id.to_string());
            }
        }
        d
    }

    /// Canonical out: sorted, deduplicated.
    pub fn line(&self) -> String {
        self.found.iter().cloned().collect::<Vec<_>>().join(",")
    }

    /// Bank a find. Returns whether it was NEW — the caller's cue to
    /// announce something rather than shrug at a repeat.
    pub fn record(&mut self, id: &str) -> bool {
        self.found.insert(id.to_string())
    }

    pub fn has(&self, id: &str) -> bool {
        self.found.contains(id)
    }
    pub fn count(&self) -> usize {
        self.found.len()
    }
    pub fn ids(&self) -> &BTreeSet<String> {
        &self.found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static CLAIMS: [Claim; 2] = [
        Claim { id: "old", asserted: false, layer: 4, text: "the thing is old" },
        Claim { id: "built", asserted: true, layer: 4, text: "the thing was built" },
    ];

    static FINDS: [Finding; 4] = [
        Finding {
            id: "a1",
            site: "earth",
            blurb: "",
            source: "wreck_a",
            medium: Medium::Artifact,
            gate: None,
            links: &[Link { claim: "old", bearing: Bearing::Supports }],
        },
        Finding {
            id: "a2",
            site: "earth",
            blurb: "",
            source: "wreck_a", // SAME source as a1
            medium: Medium::Artifact,
            gate: None,
            links: &[Link { claim: "old", bearing: Bearing::Supports }],
        },
        Finding {
            id: "b1",
            site: "luna",
            blurb: "",
            source: "wreck_b",
            medium: Medium::Site,
            gate: None,
            links: &[Link { claim: "old", bearing: Bearing::Supports }],
        },
        Finding {
            id: "c1",
            site: "mars",
            blurb: "",
            source: "ledger_c",
            medium: Medium::Terminal,
            gate: Some("late_mission"),
            links: &[Link { claim: "old", bearing: Bearing::Contradicts }],
        },
    ];

    fn dig(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn one_source_twice_is_still_one_source() {
        // the whole reason `source` exists: farming a single wreck must not
        // manufacture corroboration
        let a = assay(&CLAIMS, &FINDS, &dig(&["a1", "a2"]));
        assert_eq!(a["old"].support(), 1);
        assert_eq!(a["old"].standing(), Standing::Unknown);

        let b = assay(&CLAIMS, &FINDS, &dig(&["a1", "b1"]));
        assert_eq!(b["old"].support(), 2);
        assert_eq!(b["old"].standing(), Standing::Possible);
    }

    #[test]
    fn disagreement_is_a_dispute_not_an_average() {
        let a = assay(&CLAIMS, &FINDS, &dig(&["a1", "b1", "c1"]));
        assert_eq!(a["old"].standing(), Standing::Contested);
        // and it keeps the citation list, both ways
        assert_eq!(a["old"].support(), 2);
        assert_eq!(a["old"].opposition(), 1);
    }

    #[test]
    fn contradiction_alone_still_draws() {
        // learning a claim is false is learning something
        let a = assay(&CLAIMS, &FINDS, &dig(&["c1"]));
        assert_eq!(a["old"].standing(), Standing::Contested);
        assert!(a["old"].standing().drawn());
    }

    #[test]
    fn untouched_claims_stay_dark() {
        let a = assay(&CLAIMS, &FINDS, &dig(&["a1", "b1", "c1"]));
        assert_eq!(a["built"].standing(), Standing::Dark);
        assert!(!a["built"].standing().drawn());
        let none = assay(&CLAIMS, &FINDS, &BTreeSet::new());
        assert!(none.values().all(|a| a.standing() == Standing::Dark));
    }

    #[test]
    fn the_dig_line_is_lenient_in_canonical_out() {
        let d = Dig::from_line(" b1 ,a1,, nonexistent ,a1", &FINDS);
        assert_eq!(d.count(), 2, "unknown ids and duplicates should not survive");
        assert_eq!(d.line(), "a1,b1", "the line should be sorted and deduplicated");
        assert_eq!(Dig::from_line("", &FINDS).line(), "");
        // round trip
        let back = Dig::from_line(&d.line(), &FINDS);
        assert_eq!(back.line(), d.line());
    }

    #[test]
    fn you_cannot_refute_a_claim_nobody_has_made() {
        // "built" is an asserted claim. contradict it with nothing asserting
        // it and the map must stay quiet rather than inventing a dispute.
        static AGAINST_BUILT: [Finding; 2] = [
            Finding { id: "d1", site: "earth", blurb: "", source: "layer_d", medium: Medium::Site, gate: None,
                links: &[Link { claim: "built", bearing: Bearing::Contradicts }] },
            Finding { id: "d2", site: "earth", blurb: "", source: "ledger_e", medium: Medium::Terminal, gate: None,
                links: &[Link { claim: "built", bearing: Bearing::Supports }] },
        ];
        let only_against: BTreeSet<String> = ["d1"].iter().map(|s| s.to_string()).collect();
        let a = assay(&CLAIMS, &AGAINST_BUILT, &only_against);
        assert_eq!(a["built"].standing(), Standing::Dark, "refuted an unmade claim");

        // once somebody asserts it, the refutation lands
        let both: BTreeSet<String> = ["d1", "d2"].iter().map(|s| s.to_string()).collect();
        let b = assay(&CLAIMS, &AGAINST_BUILT, &both);
        assert_eq!(b["built"].standing(), Standing::Contested);

        // an unasserted claim keeps the old behaviour: learning it is false
        // IS learning something, when the world implies the claim by itself
        let c = assay(&CLAIMS, &FINDS, &dig(&["c1"]));
        assert_eq!(c["old"].standing(), Standing::Contested);
    }

    #[test]
    fn a_prior_counts_once_and_can_be_beaten() {
        // received knowledge is evidence, not a reveal flag
        let believed =
            [Prior { claim: "old", bearing: Bearing::Supports, source: "the_record" }];
        let a = assay_with(&CLAIMS, &FINDS, &BTreeSet::new(), &believed);
        assert_eq!(a["old"].standing(), Standing::Unknown, "belief alone is one source");

        // a find from the SAME source adds nothing -- "everyone knows it" and
        // "here is the file everyone is quoting" are one observation
        let same = assay_with(&CLAIMS, &FINDS, &dig(&["a1"]), &[Prior {
            claim: "old",
            bearing: Bearing::Supports,
            source: "wreck_a",
        }]);
        assert_eq!(same["old"].support(), 1);

        // and a prior is contradictable like anything else
        let beaten = assay_with(&CLAIMS, &FINDS, &dig(&["c1"]), &believed);
        assert_eq!(beaten["old"].standing(), Standing::Contested);
        assert_eq!(beaten["old"].support(), 1);
        assert_eq!(beaten["old"].opposition(), 1);
    }

    #[test]
    fn no_priors_is_the_same_as_the_plain_assay() {
        let d = dig(&["a1", "b1"]);
        let plain = assay(&CLAIMS, &FINDS, &d);
        let empty = assay_with(&CLAIMS, &FINDS, &d, &[]);
        for c in CLAIMS.iter() {
            assert_eq!(plain[c.id].standing(), empty[c.id].standing());
        }
    }

    #[test]
    fn record_reports_novelty() {
        let mut d = Dig::default();
        assert!(d.record("a1"));
        assert!(!d.record("a1"), "a repeat find should not announce itself");
    }

    #[test]
    fn unfound_at_narrows_by_site_and_by_progress() {
        let none = BTreeSet::new();
        let all_open = |_: &str| true;
        assert_eq!(unfound_at(&FINDS, "earth", &none, all_open).len(), 2);
        assert_eq!(unfound_at(&FINDS, "luna", &none, all_open).len(), 1);
        assert_eq!(unfound_at(&FINDS, "titan", &none, all_open).len(), 0);
        assert_eq!(unfound_at(&FINDS, "earth", &dig(&["a1"]), all_open).len(), 1);
    }

    #[test]
    fn a_closed_gate_hides_the_find_without_the_engine_knowing_why() {
        let none = BTreeSet::new();
        // c1 is gated on "late_mission"
        assert_eq!(unfound_at(&FINDS, "mars", &none, |_| false).len(), 0);
        assert_eq!(unfound_at(&FINDS, "mars", &none, |g| g == "late_mission").len(), 1);
        // an ungated find is unaffected by a resolver that refuses everything
        assert_eq!(unfound_at(&FINDS, "luna", &none, |_| false).len(), 1);
    }
}
