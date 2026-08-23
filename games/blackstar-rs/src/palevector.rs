//! PALE VECTOR — the relic-keeper, and testimony that is not evidence.
//!
//! Ported from `game.c`. The C header sources him to the screenplay: *"The
//! Ancient Relic-Keeper... cloaked. thin. almost skeletal."* A non-combat
//! apparition on the scavenger worlds who stands far off, whispers if you come
//! near, and **warps out if you press him**. He maintains the relics. He
//! remembers.
//!
//! ## The shape is the character
//!
//! Three distances and nothing else. Outside thirty metres he is scenery.
//! Inside it he talks, on a cooldown, so he cannot be farmed by pacing. Inside
//! twelve he leaves, and does not come back for a long time.
//!
//! That is a complete design: **the only way to get everything he knows is to
//! keep your distance for a long time**, and the instinct that works on
//! everything else in the game — close, press, take — is precisely the one
//! that costs you. He is the one thing you cannot approach.
//!
//! ## What he is FOR, which the C did not have anywhere to put
//!
//! His four lines in `game.c` are archaeology claims spoken aloud —
//! *"the lattice is stirring"*, *"the array saw it first, no one wanted to
//! look"*, *"i maintain what was meant to stay buried"*. That engine had no
//! provenance layer, so they were flavour text.
//!
//! Here they are **testimony**, and the distinction is one `provenance.rs`
//! already makes. A whisper enters the map as `Bearing::Mentions` from a
//! single source: it puts a claim on the board at UNKNOWN and moves it no
//! further. He tells you where to dig and never counts as proof of anything.
//!
//! That is right for him twice over. He is one voice, so he could never be
//! corroboration on his own; and he is a rumour rather than an artifact, so
//! the map should record that somebody SAID this, not that it is so.

use crate::archive;
use crate::provenance::{Bearing, Prior};
use glam::Vec3;

/// Where he is in his cycle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// not on the map
    Gone,
    /// standing, facing you, barely breathing
    Standing,
}

/// He talks inside this, in metres.
pub const WHISPER_M: f32 = 30.0;
/// Closer than this and he leaves.
pub const PRESS_M: f32 = 12.0;
/// Between whispers, so pacing at the edge cannot farm him.
pub const WHISPER_CD_S: f32 = 11.0;
/// After being pressed. Deliberately long: the cost of grabbing at him.
pub const PRESSED_CD_S: f32 = 40.0;
/// Before he first appears, and between ordinary disappearances.
pub const APPEAR_CD_S: f32 = 8.0;

/// One thing he says, and the claim it concerns.
pub struct Whisper {
    pub line: &'static str,
    /// A claim in `archive::CLAIMS`, or None for a line that is only mood.
    pub claim: Option<&'static str>,
}

/// His four lines, from `game.c`, with what each is testimony about.
pub static WHISPERS: &[Whisper] = &[
    Whisper { line: "the lattice is stirring.", claim: Some("edge_lattice") },
    Whisper {
        line: "he freed me. i keep his memory in working order.",
        claim: Some("archivist_collecting"),
    },
    Whisper {
        line: "the array saw it first. no one wanted to look.",
        claim: Some("edge_lattice"),
    },
    Whisper {
        line: "i maintain what was meant to stay buried.",
        claim: Some("two_artifacts"),
    },
];

/// What the map should be told when he has spoken.
///
/// `Mentions`, from one source, always. See the module doc: he is a rumour,
/// not an artifact, and one voice can never be corroboration.
pub const SOURCE: &str = "pale_vector";

pub struct PaleVector {
    pub state: State,
    pub pos: Vec3,
    /// shared clock: time until he appears, or until he returns after being pressed
    cd: f32,
    whisper_cd: f32,
    next_line: usize,
    /// which lines have been heard, by index
    heard: Vec<usize>,
    pub withdrawn: bool,
}

impl Default for PaleVector {
    fn default() -> Self {
        PaleVector {
            state: State::Gone,
            pos: Vec3::ZERO,
            cd: APPEAR_CD_S,
            whisper_cd: 0.0,
            next_line: 0,
            heard: Vec::new(),
            withdrawn: false,
        }
    }
}

/// What happened this tick.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Event {
    Nothing,
    Appeared,
    /// he said the line at this index
    Whispered(usize),
    /// pressed too close; gone for a long while
    Withdrew,
}

impl PaleVector {
    /// Put him somewhere. The caller picks the spot — far off, on a scavenger
    /// world — because only the world knows where "far off" is.
    pub fn place(&mut self, at: Vec3) {
        self.pos = at;
    }

    pub fn update(&mut self, dt: f32, player: Vec3) -> Event {
        self.withdrawn = false;
        self.cd -= dt;
        match self.state {
            State::Gone => {
                if self.cd <= 0.0 {
                    self.state = State::Standing;
                    self.whisper_cd = 0.0;
                    return Event::Appeared;
                }
                Event::Nothing
            }
            State::Standing => {
                // ground distance: he stands on a hill as often as not, and
                // height is terrain rather than intent
                let d = Vec3::new(self.pos.x - player.x, 0.0, self.pos.z - player.z).length();
                if d < PRESS_M {
                    self.state = State::Gone;
                    self.cd = PRESSED_CD_S;
                    self.withdrawn = true;
                    return Event::Withdrew;
                }
                self.whisper_cd -= dt;
                if d < WHISPER_M && self.whisper_cd <= 0.0 {
                    let i = self.next_line % WHISPERS.len();
                    self.next_line += 1;
                    self.whisper_cd = WHISPER_CD_S;
                    if !self.heard.contains(&i) {
                        self.heard.push(i);
                    }
                    return Event::Whispered(i);
                }
                Event::Nothing
            }
        }
    }

    /// Has he said everything? Only reachable by holding the line for a long
    /// time without ever closing.
    pub fn exhausted(&self) -> bool {
        self.heard.len() == WHISPERS.len()
    }

    pub fn heard_lines(&self) -> impl Iterator<Item = &'static Whisper> + '_ {
        self.heard.iter().map(|i| &WHISPERS[*i])
    }

    /// What he has told you, as something the archaeological map can read.
    ///
    /// Every one is a `Mentions` from one source, so the strongest effect he
    /// can have is putting a claim on the board at UNKNOWN. He points; the
    /// digging is still yours.
    pub fn testimony(&self) -> Vec<Prior> {
        // Deduplicated: two of his lines concern the edge lattice, and one
        // witness saying a thing twice is one witness. The assay counts
        // distinct sources so a duplicate would be harmless there, but it
        // would print as two pieces of testimony and read as corroboration.
        let mut seen = Vec::new();
        self.heard_lines()
            .filter_map(|w| w.claim)
            .filter(|c| {
                let fresh = !seen.contains(c);
                if fresh {
                    seen.push(*c);
                }
                fresh
            })
            .map(|claim| Prior { claim, bearing: Bearing::Mentions, source: SOURCE })
            .collect()
    }
}

// ------------------------------------------------------------- driver ---

/// `--pale` — walk toward him and see what it costs.
pub fn run() {
    println!("PALE VECTOR — the one thing you cannot approach\n");
    println!("  \"The Ancient Relic-Keeper... cloaked. thin. almost skeletal.\"");
    println!("  whispers inside {WHISPER_M:.0} m, withdraws inside {PRESS_M:.0} m.\n");

    // A: keep your distance. He talks.
    let mut pv = PaleVector::default();
    pv.place(Vec3::new(0.0, 0.0, 0.0));
    let mut t = 0.0f32;
    println!("  HOLDING AT 20 m");
    for _ in 0..(60 * 60) {
        t += 1.0 / 60.0;
        match pv.update(1.0 / 60.0, Vec3::new(20.0, 0.0, 0.0)) {
            Event::Appeared => println!("    t+{t:>5.1}s  he is standing there"),
            Event::Whispered(i) => {
                println!("    t+{t:>5.1}s  \"{}\"", WHISPERS[i].line)
            }
            _ => {}
        }
        if pv.exhausted() {
            break;
        }
    }
    println!("    everything he knows, in {t:.0} s of not touching him.\n");

    // B: close on him. He leaves.
    let mut pv = PaleVector::default();
    pv.place(Vec3::ZERO);
    let mut t = 0.0f32;
    // far enough out that he is standing there BEFORE you are on top of him;
    // starting at 25 m meant he appeared already inside press range and the
    // approach was over before it began
    let mut dist = 70.0f32;
    println!("  WALKING IN");
    for _ in 0..(60 * 60) {
        t += 1.0 / 60.0;
        dist = (dist - 3.0 / 60.0).max(0.0);
        match pv.update(1.0 / 60.0, Vec3::new(dist, 0.0, 0.0)) {
            Event::Appeared => println!("    t+{t:>5.1}s  he is standing there"),
            Event::Whispered(i) => println!("    t+{t:>5.1}s  \"{}\"", WHISPERS[i].line),
            Event::Withdrew => {
                println!("    t+{t:>5.1}s  PALE VECTOR withdraws -- somewhere, a relic hums");
                println!("    heard {} of {} before pressing him.", pv.heard_lines().count(), WHISPERS.len());
                break;
            }
            _ => {}
        }
    }

    println!("\n  WHAT THE MAP MAKES OF IT");
    let mut full = PaleVector::default();
    full.place(Vec3::ZERO);
    for _ in 0..(60 * 200) {
        full.update(1.0 / 60.0, Vec3::new(20.0, 0.0, 0.0));
    }
    for p in full.testimony() {
        let c = archive::claim(p.claim).map(|c| c.text).unwrap_or("?");
        println!("    NAMES  {:<24} {}", p.claim, c);
    }
    println!("\n  every line is a MENTION from one source, so the most he can do is");
    println!("  put a claim on the board at UNKNOWN. he tells you where to dig and");
    println!("  never counts as proof. he is a rumour, not an artifact.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::{assay_with, Standing};
    use std::collections::BTreeSet;

    fn hold_at(d: f32, secs: f32) -> PaleVector {
        let mut pv = PaleVector::default();
        pv.place(Vec3::ZERO);
        for _ in 0..((secs * 60.0) as i32) {
            pv.update(1.0 / 60.0, Vec3::new(d, 0.0, 0.0));
        }
        pv
    }

    #[test]
    fn he_appears_on_his_own_clock_and_not_before() {
        let mut pv = PaleVector::default();
        pv.place(Vec3::ZERO);
        assert_eq!(pv.state, State::Gone);
        for _ in 0..((APPEAR_CD_S - 1.0) * 60.0) as i32 {
            pv.update(1.0 / 60.0, Vec3::new(20.0, 0.0, 0.0));
        }
        assert_eq!(pv.state, State::Gone, "he arrived early");
        for _ in 0..120 {
            pv.update(1.0 / 60.0, Vec3::new(20.0, 0.0, 0.0));
        }
        assert_eq!(pv.state, State::Standing);
    }

    #[test]
    fn distance_is_the_whole_interaction() {
        // far: nothing. near: he talks. close: he goes.
        assert!(hold_at(200.0, 60.0).heard_lines().count() == 0, "he shouted across 200 m");
        assert!(hold_at(20.0, 60.0).heard_lines().count() > 0, "he said nothing at 20 m");
        let pressed = hold_at(5.0, 60.0);
        assert_eq!(pressed.state, State::Gone, "he stayed while being crowded");
    }

    #[test]
    fn pressing_him_costs_you_the_rest_of_what_he_knows() {
        // the design: the instinct that works on everything else in this game
        // is the one that costs you here
        let patient = hold_at(20.0, 200.0);
        assert!(patient.exhausted(), "patience should get everything");

        let mut greedy = PaleVector::default();
        greedy.place(Vec3::ZERO);
        let mut d = 25.0f32;
        for _ in 0..(60 * 60) {
            d = (d - 3.0 / 60.0).max(0.0);
            greedy.update(1.0 / 60.0, Vec3::new(d, 0.0, 0.0));
        }
        assert!(!greedy.exhausted(), "walking in got everything anyway");
        assert!(
            greedy.heard_lines().count() < patient.heard_lines().count(),
            "{} vs {}",
            greedy.heard_lines().count(),
            patient.heard_lines().count()
        );
    }

    #[test]
    fn he_cannot_be_farmed_by_pacing_the_edge() {
        // the whisper cooldown: an hour at the edge should not produce an
        // hour of lines
        let pv = hold_at(20.0, 30.0);
        let said = pv.heard_lines().count();
        assert!(said <= 3, "said {said} lines in 30 s");
    }

    #[test]
    fn withdrawing_is_expensive_and_he_does_come_back() {
        // Timed from the WITHDRAWAL, not from the start of the run. The first
        // version counted from t=0 and so included the eight seconds before
        // he ever appeared plus the tail of the loop that pressed him — 42
        // seconds against a 40-second cooldown, and it accused the code of
        // being early when the test could not do arithmetic.
        let mut pv = PaleVector::default();
        pv.place(Vec3::ZERO);
        let mut pressed_at = None;
        for i in 0..(60 * 30) {
            if pv.update(1.0 / 60.0, Vec3::new(5.0, 0.0, 0.0)) == Event::Withdrew {
                pressed_at = Some(i);
                break;
            }
        }
        assert!(pressed_at.is_some(), "he never withdrew");
        assert_eq!(pv.state, State::Gone);

        // one second short of the cooldown
        for _ in 0..((PRESSED_CD_S - 1.0) * 60.0) as i32 {
            pv.update(1.0 / 60.0, Vec3::new(60.0, 0.0, 0.0));
        }
        assert_eq!(pv.state, State::Gone, "he came back before the cooldown ran out");

        // and past it
        for _ in 0..120 {
            pv.update(1.0 / 60.0, Vec3::new(60.0, 0.0, 0.0));
        }
        assert_eq!(pv.state, State::Standing, "he never came back at all");
        // the long cooldown is the cost, and it is much longer than the first
        // appearance you got for free
        assert!(PRESSED_CD_S > APPEAR_CD_S * 4.0);
    }

    #[test]
    fn every_line_names_a_real_claim() {
        for w in WHISPERS {
            assert!(!w.line.is_empty());
            if let Some(c) = w.claim {
                assert!(archive::claim(c).is_some(), "'{}' names unknown claim '{c}'", w.line);
            }
        }
        let claims: BTreeSet<_> = WHISPERS.iter().filter_map(|w| w.claim).collect();
        assert!(claims.len() >= 2, "he only ever talks about one thing");
    }

    #[test]
    fn testimony_can_never_be_proof() {
        // the reason `Mentions` is the right bearing: he is one voice and a
        // rumour. Everything he says lands at UNKNOWN and stops.
        let pv = hold_at(20.0, 200.0);
        let priors = pv.testimony();
        assert!(!priors.is_empty());
        assert!(priors.iter().all(|p| p.bearing == Bearing::Mentions), "he became evidence");
        assert!(priors.iter().all(|p| p.source == SOURCE), "he became two witnesses");

        let a = assay_with(archive::CLAIMS, archive::FINDINGS, &BTreeSet::new(), &priors);
        for p in &priors {
            let st = a[p.claim].standing();
            assert_eq!(st, Standing::Unknown, "'{}' reached {st:?} on hearsay", p.claim);
            assert_eq!(a[p.claim].support(), 0, "a whisper counted as support");
        }
    }

    #[test]
    fn he_puts_claims_on_the_board_that_were_dark_before() {
        // and the reason he is worth talking to at all: without him those
        // nodes are not merely unproven, they are invisible
        let pv = hold_at(20.0, 200.0);
        let none = assay_with(archive::CLAIMS, archive::FINDINGS, &BTreeSet::new(), &[]);
        let told = assay_with(archive::CLAIMS, archive::FINDINGS, &BTreeSet::new(), &pv.testimony());
        let before = archive::CLAIMS.iter().filter(|c| none[c.id].standing().drawn()).count();
        let after = archive::CLAIMS.iter().filter(|c| told[c.id].standing().drawn()).count();
        assert!(after > before, "he told you nothing the map did not already show");
    }
}
