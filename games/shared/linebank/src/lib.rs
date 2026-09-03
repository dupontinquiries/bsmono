//! LINEBANK — the recording side of voice.
//!
//! Nothing in this repo addresses a line of dialogue. `blackstar-rs` keeps its
//! prose as inline string literals across `content.rs`, `saga.rs`, `codex.rs`
//! and `archive.rs`, and `content.rs`'s `subtitle` field is a FILM subtitle,
//! not a spoken one. Without stable ids you cannot hand an actor a script,
//! match a returned WAV back to the line it answers, or diff a re-record. That
//! is the gap this fills, and it is why the first piece of a voice pipeline is
//! not audio.
//!
//! Deliberately no samples here. `audio-kit` owns level and pitch,
//! `voiceprint-kit` owns the MFCC fingerprint that catches a mismatched take.
//! This crate owns ids, coverage, the script, and selection — all of which are
//! decisions, and all of which are testable in milliseconds.
//!
//! ## Variety is a selection problem, not a content problem
//!
//! The instinct is that varied blips need many takes. Mostly they need a
//! better selector. Uniform random repeats far sooner than people expect —
//! with five takes, one in five plays back-to-back with itself, and an
//! immediate repeat is the single most audible failure in game audio.
//!
//! [`Bag`] draws without replacement and reshuffles when empty, and on
//! reshuffle it refuses to put the take you just heard at the front. Five
//! takes through a bag beat a dozen through `rand()`, and
//! `a_bag_never_repeats_before_it_is_exhausted` plus
//! `a_reshuffle_never_butts_a_take_against_itself` are why that is a property
//! rather than a hope.
//!
//! ## Units are phrases, not words
//!
//! Splicing word-level takes fails on PROSODY, not on the joins: pitch
//! contour, timing and coarticulation are sentence-level, so a word recorded
//! in isolation carries the wrong contour for every position it is pasted
//! into. The technique that works is carrier-and-slot with the slot filler
//! read IN POSITION — the actor speaks the whole sentence for each variant.
//!
//! [`Carrier`] models that honestly, which mainly means making the cost
//! visible: [`Carrier::reads`] is the product of the slot cardinalities, and
//! `the_cost_of_a_carrier_is_multiplicative_and_says_so` exists so nobody
//! designs a three-slot line and discovers it is 60 reads at the session.

pub mod assets;
pub mod session;
pub mod wav_error;
pub mod slice;
pub mod verify;

use std::collections::{BTreeMap, BTreeSet};

/// What a unit is for, which decides how it may be spliced.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Kind {
    /// A short non-lexical vocalisation — an acknowledgement, a grunt, a
    /// breath. Safe to play alone, so safe to vary freely. This is where a
    /// bank pays off first.
    Blip,
    /// A complete spoken phrase. Plays alone; never spliced mid-sentence.
    Phrase,
    /// A slot filler, recorded inside its carrier so the prosody is right.
    /// NOT usable outside the carrier it was read for.
    Slot,
}

/// One recordable thing.
#[derive(Clone, Debug)]
pub struct Unit {
    /// Stable, filename-safe, unique. This becomes a WAV name and a save-file
    /// key, so it may never be renamed once recorded.
    pub id: String,
    /// Who says it.
    pub speaker: String,
    /// What they say.
    pub text: String,
    pub kind: Kind,
    /// Direction for the actor. The most valuable column and the one always
    /// left empty — a line read with the wrong intent is a re-record.
    pub direction: String,
    /// How many distinct takes this wants. Blips want several; a
    /// once-per-campaign phrase wants one.
    pub wanted: u8,
}

impl Unit {
    pub fn new(id: &str, speaker: &str, text: &str, kind: Kind) -> Unit {
        Unit {
            id: id.to_string(),
            speaker: speaker.to_string(),
            text: text.to_string(),
            kind,
            direction: String::new(),
            wanted: match kind {
                Kind::Blip => 5,
                Kind::Phrase | Kind::Slot => 1,
            },
        }
    }
    pub fn directed(mut self, d: &str) -> Unit {
        self.direction = d.to_string();
        self
    }
    pub fn wanting(mut self, n: u8) -> Unit {
        self.wanted = n.max(1);
        self
    }
}

/// A recorded take of a unit.
#[derive(Clone, Debug, PartialEq)]
pub struct Take {
    pub unit: String,
    /// Take number within the unit, from 1.
    pub n: u8,
    pub seconds: f32,
}

/// A carrier sentence with slots, and the reads it costs.
///
/// `"{who} is down on {where}"` with 4 speakers and 6 places is 24 reads, not
/// 10. Stating it as a method rather than a comment is the point: the number
/// is what gets negotiated in a session, and nobody eyeballs a product.
#[derive(Clone, Debug)]
pub struct Carrier {
    pub id: String,
    pub speaker: String,
    /// The frame, with `{slot}` markers.
    pub frame: String,
    /// Slot name → the fillers it can take.
    pub slots: BTreeMap<String, Vec<String>>,
}

impl Carrier {
    pub fn new(id: &str, speaker: &str, frame: &str) -> Carrier {
        Carrier {
            id: id.to_string(),
            speaker: speaker.to_string(),
            frame: frame.to_string(),
            slots: BTreeMap::new(),
        }
    }
    pub fn slot(mut self, name: &str, fillers: &[&str]) -> Carrier {
        self.slots.insert(name.to_string(), fillers.iter().map(|s| s.to_string()).collect());
        self
    }
    /// Every sentence this carrier can say. The actor reads all of them.
    pub fn reads(&self) -> Vec<String> {
        let mut out = vec![self.frame.clone()];
        for (name, fillers) in &self.slots {
            let marker = format!("{{{name}}}");
            let mut next = Vec::new();
            for base in &out {
                for f in fillers {
                    next.push(base.replace(&marker, f));
                }
            }
            out = next;
        }
        out
    }
    /// How many reads, without building them.
    pub fn read_count(&self) -> usize {
        self.slots.values().map(|f| f.len().max(1)).product()
    }
    /// Slot markers in the frame that no slot supplies — a line that would go
    /// to the studio with `{who}` still in it.
    pub fn unfilled(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = self.frame.as_str();
        while let Some(a) = rest.find('{') {
            let Some(b) = rest[a..].find('}') else { break };
            let name = &rest[a + 1..a + b];
            if !self.slots.contains_key(name) {
                out.push(name.to_string());
            }
            rest = &rest[a + b + 1..];
        }
        out
    }
}

/// Everything to be recorded, and what has come back.
#[derive(Clone, Debug, Default)]
pub struct Bank {
    units: Vec<Unit>,
    takes: Vec<Take>,
}

/// Why a bank is not ready to record or to ship.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Fault {
    /// Two units share an id, or share one case-insensitively — these become
    /// filenames, and Windows and macOS will silently collide them.
    DuplicateId(String),
    /// An id that cannot be a filename.
    UnsafeId(String),
    /// A take references a unit that does not exist. Usually a rename after
    /// recording, which is why ids are permanent.
    OrphanTake(String),
    /// Fewer takes than the unit asked for.
    Short { unit: String, have: u8, wanted: u8 },
}

impl Bank {
    pub fn add(&mut self, u: Unit) -> &mut Bank {
        self.units.push(u);
        self
    }
    /// Add every read of a carrier as a `Slot` unit, ids suffixed by index.
    pub fn add_carrier(&mut self, c: &Carrier) -> &mut Bank {
        for (i, text) in c.reads().iter().enumerate() {
            let u = Unit::new(&format!("{}_{:02}", c.id, i + 1), &c.speaker, text, Kind::Slot)
                .directed("read in position — this filler is not reusable elsewhere");
            self.units.push(u);
        }
        self
    }
    pub fn record(&mut self, unit: &str, n: u8, seconds: f32) -> &mut Bank {
        self.takes.push(Take { unit: unit.to_string(), n, seconds });
        self
    }
    pub fn units(&self) -> &[Unit] {
        &self.units
    }
    pub fn takes_of(&self, unit: &str) -> Vec<&Take> {
        self.takes.iter().filter(|t| t.unit == unit).collect()
    }

    /// Everything wrong, in one pass. Ordered so ids come before takes: an
    /// orphan take is usually a consequence of an id problem, not a cause.
    pub fn faults(&self) -> Vec<Fault> {
        let mut out = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for u in &self.units {
            let key = u.id.to_lowercase();
            if !seen.insert(key) {
                out.push(Fault::DuplicateId(u.id.clone()));
            }
            if u.id.is_empty()
                || !u.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                out.push(Fault::UnsafeId(u.id.clone()));
            }
        }
        let known: BTreeSet<&str> = self.units.iter().map(|u| u.id.as_str()).collect();
        for t in &self.takes {
            if !known.contains(t.unit.as_str()) {
                out.push(Fault::OrphanTake(t.unit.clone()));
            }
        }
        for u in &self.units {
            let have = self.takes_of(&u.id).len() as u8;
            if have < u.wanted {
                out.push(Fault::Short { unit: u.id.clone(), have, wanted: u.wanted });
            }
        }
        out
    }

    /// A script to hand one actor: their units only, grouped, with ids and
    /// direction. Plain text on purpose — it gets printed, marked up in pencil
    /// and read from a stand.
    pub fn script_for(&self, speaker: &str) -> String {
        let mut s = format!("VOICE SCRIPT — {speaker}\n\n");
        s.push_str(
            "Read the ID aloud before each line (slate). Leave a beat of silence\n\
             between takes; the slicer cuts on silence.\n\n",
        );
        for kind in [Kind::Blip, Kind::Phrase, Kind::Slot] {
            let mine: Vec<&Unit> =
                self.units.iter().filter(|u| u.speaker == speaker && u.kind == kind).collect();
            if mine.is_empty() {
                continue;
            }
            let label = match kind {
                Kind::Blip => "BLIPS — short, non-verbal. Several takes each; vary them.",
                Kind::Phrase => "PHRASES — complete lines. Play alone, never spliced.",
                Kind::Slot => "IN-POSITION READS — whole sentence each time. Do not shortcut.",
            };
            s.push_str(&format!("== {label}\n\n"));
            for u in mine {
                s.push_str(&format!("[{}]  x{}\n", u.id, u.wanted));
                s.push_str(&format!("  \"{}\"\n", u.text));
                if !u.direction.is_empty() {
                    s.push_str(&format!("  ({})\n", u.direction));
                }
                s.push('\n');
            }
        }
        s
    }

    /// Speakers with anything to record, in a stable order.
    pub fn speakers(&self) -> Vec<String> {
        let set: BTreeSet<String> = self.units.iter().map(|u| u.speaker.clone()).collect();
        set.into_iter().collect()
    }
}

/// Draw-without-replacement selection over a unit's takes.
///
/// The whole point is what it REFUSES: the same take twice in a row, across
/// the reshuffle boundary as well as inside a cycle. Uniform random cannot
/// promise that at any bank size.
#[derive(Clone, Debug)]
pub struct Bag {
    order: Vec<u8>,
    next: usize,
    last: Option<u8>,
    seed: u32,
}

impl Bag {
    /// `takes` is how many exist for the unit. A bag of one is legal and
    /// always returns that take — a once-per-campaign line does not need
    /// variety.
    pub fn new(takes: u8, seed: u32) -> Bag {
        // NOT `seed | 1`. That was the first version and it collapses adjacent
        // seeds: 4242 | 1 and 4243 | 1 are both 4243, so two callers one apart
        // heard an identical sequence. Mixing first spreads them, and the OR
        // then only guards xorshift's one forbidden input.
        let mixed = seed.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
        let mut b = Bag {
            order: (1..=takes.max(1)).collect(),
            next: 0,
            last: None,
            seed: if mixed == 0 { 0xA341_316C } else { mixed },
        };
        b.shuffle();
        b
    }

    fn roll(&mut self) -> u32 {
        // xorshift: deterministic per seed, which matters because a replay or
        // a netcode peer has to hear the same sequence.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed
    }

    fn shuffle(&mut self) {
        for i in (1..self.order.len()).rev() {
            let j = (self.roll() as usize) % (i + 1);
            self.order.swap(i, j);
        }
        // Never butt a take against itself across the boundary. With one take
        // there is nothing to do and nothing to promise.
        if self.order.len() > 1 && self.last == self.order.first().copied() {
            let n = self.order.len();
            self.order.swap(0, n - 1);
        }
        self.next = 0;
    }

    /// The next take number to play.
    pub fn draw(&mut self) -> u8 {
        if self.next >= self.order.len() {
            self.shuffle();
        }
        let t = self.order[self.next];
        self.next += 1;
        self.last = Some(t);
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bank() -> Bank {
        let mut b = Bank::default();
        b.add(Unit::new("billy_ack", "Billy", "(short acknowledgement)", Kind::Blip).wanting(6))
            .add(
                Unit::new("billy_hit", "Billy", "(taking a hit)", Kind::Blip)
                    .directed("winded, not screaming"),
            )
            .add(Unit::new(
                "billy_one_shot",
                "Billy",
                "You have one shot.",
                Kind::Phrase,
            ))
            .add(Unit::new("keelwright_consequence", "Keelwright", "It believes in consequence.", Kind::Phrase)
                .directed("cold and measured until the last word"));
        b
    }

    #[test]
    fn a_bag_never_repeats_before_it_is_exhausted() {
        // The property that makes a small bank sound big.
        for seed in [1u32, 7, 12345, 0xDEAD_BEEF] {
            let mut bag = Bag::new(5, seed);
            let mut seen = BTreeSet::new();
            for _ in 0..5 {
                assert!(seen.insert(bag.draw()), "a take repeated inside one cycle (seed {seed})");
            }
            assert_eq!(seen.len(), 5, "not every take was used");
        }
    }

    #[test]
    fn a_reshuffle_never_butts_a_take_against_itself() {
        // The audible failure. Uniform random cannot promise this; a bag can,
        // and only if the reshuffle looks at what just played.
        for seed in 1..40u32 {
            let mut bag = Bag::new(4, seed);
            let mut prev = bag.draw();
            for _ in 0..200 {
                let t = bag.draw();
                assert_ne!(t, prev, "take {t} played twice running (seed {seed})");
                prev = t;
            }
        }
    }

    #[test]
    fn a_bag_of_one_is_legal_and_promises_nothing() {
        // A once-per-campaign line has one take. It must not be an error, and
        // the no-repeat promise obviously cannot hold.
        let mut bag = Bag::new(1, 99);
        assert_eq!(bag.draw(), 1);
        assert_eq!(bag.draw(), 1);
    }

    #[test]
    fn the_same_seed_hears_the_same_sequence() {
        // Replays and netcode peers have to agree.
        let seq = |s: u32| {
            let mut b = Bag::new(5, s);
            (0..20).map(|_| b.draw()).collect::<Vec<_>>()
        };
        assert_eq!(seq(4242), seq(4242));
        // ADJACENT seeds, which is the case the first version failed: callers
        // seeded from an incrementing entity id are exactly one apart.
        for s in 0..64u32 {
            assert_ne!(seq(s), seq(s + 1), "seeds {s} and {} heard the same sequence", s + 1);
        }
    }

    #[test]
    fn ids_that_cannot_be_filenames_are_faults() {
        let mut b = Bank::default();
        b.add(Unit::new("billy ack", "Billy", "x", Kind::Blip))
            .add(Unit::new("billy/ack", "Billy", "x", Kind::Blip))
            .add(Unit::new("", "Billy", "x", Kind::Blip));
        let f = b.faults();
        assert_eq!(f.iter().filter(|x| matches!(x, Fault::UnsafeId(_))).count(), 3);
    }

    #[test]
    fn ids_colliding_only_by_case_are_faults_too() {
        // Windows and macOS filesystems are case-insensitive, so BILLY_ACK.wav
        // and billy_ack.wav are one file and the second take silently wins.
        let mut b = Bank::default();
        b.add(Unit::new("billy_ack", "Billy", "x", Kind::Blip))
            .add(Unit::new("Billy_Ack", "Billy", "x", Kind::Blip));
        assert!(b.faults().iter().any(|f| matches!(f, Fault::DuplicateId(_))));
    }

    #[test]
    fn a_take_for_a_unit_that_no_longer_exists_is_reported() {
        // What a rename after recording looks like. Ids are permanent for this
        // reason and the fault says so.
        let mut b = bank();
        b.record("billy_ack_OLD", 1, 0.4);
        assert!(b.faults().iter().any(|f| *f == Fault::OrphanTake("billy_ack_OLD".into())));
    }

    #[test]
    fn short_units_are_reported_with_the_numbers() {
        let mut b = bank();
        for n in 1..=3 {
            b.record("billy_ack", n, 0.4);
        }
        let faults = b.faults();
        assert!(faults.contains(&Fault::Short { unit: "billy_ack".into(), have: 3, wanted: 6 }));
    }

    #[test]
    fn the_cost_of_a_carrier_is_multiplicative_and_says_so() {
        // The number people get wrong. Two slots of 4 and 6 is 24 reads, not
        // 10, and it is worth knowing before the session rather than during.
        let c = Carrier::new("down", "Meridian", "{who} is down on {place}")
            .slot("who", &["Jib", "Halyard", "Cleave", "the Morpher"])
            .slot("place", &["Pluto", "Io", "Glaciem", "Titan", "Luna", "Mars"]);
        assert_eq!(c.read_count(), 24);
        let reads = c.reads();
        assert_eq!(reads.len(), 24);
        assert!(reads.iter().all(|r| !r.contains('{')), "a read went out with a marker in it");
        assert!(reads.contains(&"Halyard is down on Glaciem".to_string()));
    }

    #[test]
    fn a_frame_with_a_slot_nobody_fills_is_caught_before_the_studio() {
        let c = Carrier::new("x", "Meridian", "{who} is down on {place}").slot("who", &["Jib"]);
        assert_eq!(c.unfilled(), vec!["place".to_string()]);
        // and a fully-specified carrier reports nothing
        let ok = c.clone().slot("place", &["Io"]);
        assert!(ok.unfilled().is_empty());
    }

    #[test]
    fn a_carrier_expands_into_addressable_units() {
        let c = Carrier::new("down", "Meridian", "{who} is down")
            .slot("who", &["Jib", "Halyard"]);
        let mut b = Bank::default();
        b.add_carrier(&c);
        assert_eq!(b.units().len(), 2);
        assert_eq!(b.units()[0].id, "down_01");
        assert_eq!(b.units()[1].kind, Kind::Slot);
        assert!(b.faults().iter().all(|f| matches!(f, Fault::Short { .. })), "{:?}", b.faults());
    }

    #[test]
    fn a_script_holds_every_line_that_actor_has_and_none_that_they_do_not() {
        let b = bank();
        let s = b.script_for("Billy");
        for id in ["billy_ack", "billy_hit", "billy_one_shot"] {
            assert!(s.contains(id), "script is missing {id}");
        }
        assert!(!s.contains("keelwright_consequence"), "Billy's script has Keelwright's line");
        // the take count has to be on the page or the actor does one and leaves
        assert!(s.contains("x6"), "the script does not say how many takes");
        // direction is the column always left out, so assert it survives
        assert!(s.contains("winded, not screaming"));
    }

    #[test]
    fn the_script_separates_what_may_be_spliced_from_what_may_not() {
        // Slot reads are only valid inside their carrier. If the script does
        // not say so, they end up in the general pool and the prosody is wrong.
        let mut b = bank();
        b.add_carrier(&Carrier::new("c", "Billy", "hold at {n}").slot("n", &["one", "two"]));
        let s = b.script_for("Billy");
        assert!(s.contains("BLIPS"));
        assert!(s.contains("IN-POSITION READS"));
        assert!(s.contains("not reusable elsewhere"));
        assert!(
            s.find("BLIPS") < s.find("IN-POSITION READS"),
            "the tiring reads should not be first"
        );
    }

    #[test]
    fn speakers_are_listed_once_and_in_a_stable_order() {
        let b = bank();
        assert_eq!(b.speakers(), vec!["Billy".to_string(), "Keelwright".to_string()]);
    }

    #[test]
    fn a_complete_bank_has_no_faults() {
        let mut b = bank();
        for n in 1..=6 {
            b.record("billy_ack", n, 0.4);
        }
        // billy_hit is a Blip, and Blips default to wanting 5 — the default
        // caught this fixture recording one and calling it done, which is the
        // mistake a real session makes.
        for n in 1..=5 {
            b.record("billy_hit", n, 0.4);
        }
        for id in ["billy_one_shot", "keelwright_consequence"] {
            b.record(id, 1, 1.2);
        }
        assert_eq!(b.faults(), vec![], "a fully recorded bank still reported faults");
    }
}
