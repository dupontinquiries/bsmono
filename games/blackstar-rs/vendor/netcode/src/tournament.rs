//! tournament — brackets and standings, shared across the fleet's games.
//!
//! `netcode` has `host`, `join`, presence and profiles: everything needed to get
//! two people into one match. It has nothing for what happens across MANY
//! matches, and the echelon drop describes a "cross-game tournament host" —
//! `bsgames` runs the centre, so the structure belongs here rather than in any
//! one game.
//!
//! Deliberately game-agnostic. A tournament knows about **entrants, pairings and
//! results**; it knows nothing about scores, rounds-within-a-match, or what
//! winning means. Every game in this monorepo defines those differently — a
//! goal in skydribble, a raid cleared in blackstar, a lockstep match in echelon —
//! and a bracket that understood any of them would have to be rewritten for the
//! next.
//!
//! ## A pairing against nobody is a BYE, not a win
//!
//! With an odd entrant count somebody advances without playing. Modelling that
//! as a victory corrupts every statistic downstream: the player shows a win they
//! did not earn, their opponent count is wrong, and a tiebreak on "matches won"
//! silently rewards whoever drew the short bracket. `Pairing::Bye` is a distinct
//! shape so nothing can mistake it.
//!
//! ## Results are reported, never inferred
//!
//! A match with no reported result is `Pending`, not a draw and not a loss for
//! the absent player. A tournament that auto-resolved would decide outcomes
//! during a network partition — which is exactly when it should wait, because
//! the match may well have been played and the report simply has not arrived.
//!
//! ## Standings never invent an order
//!
//! Two entrants on equal records are equal, and `standings` says so rather than
//! breaking the tie by insertion order or by id. A bracket that silently ranked
//! one above the other would let a display artefact decide who advances.

use std::collections::BTreeMap;

/// A stable entrant id — a profile id in practice.
pub type EntrantId = String;

/// One match-up in a round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pairing {
    /// Two entrants play.
    Match { a: EntrantId, b: EntrantId },
    /// Nobody to play: this entrant advances without a match.
    ///
    /// Distinct from a win, and that distinction is load-bearing — see the
    /// module docs.
    Bye(EntrantId),
}

impl Pairing {
    /// Everyone involved.
    pub fn entrants(&self) -> Vec<&EntrantId> {
        match self {
            Pairing::Match { a, b } => vec![a, b],
            Pairing::Bye(e) => vec![e],
        }
    }

    pub fn is_bye(&self) -> bool { matches!(self, Pairing::Bye(_)) }
}

/// What happened in a match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Not reported yet. NOT a draw and NOT a forfeit.
    Pending,
    Win(EntrantId),
    Draw,
    /// Reported as unplayable — a crash, a desync, a disconnect.
    ///
    /// Separate from a draw because a draw is a result and this is the absence
    /// of one; a game that counted voids as draws would reward whoever
    /// disconnects when losing.
    Void,
}

impl Outcome {
    pub fn is_decided(&self) -> bool { !matches!(self, Outcome::Pending) }
}

/// One round: its pairings and their results.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Round {
    pub pairings: Vec<Pairing>,
    results: Vec<Outcome>,
}

impl Round {
    pub fn new(pairings: Vec<Pairing>) -> Round {
        // A bye has no match to report, so it is decided from the start.
        let results = pairings.iter()
            .map(|p| match p {
                Pairing::Bye(e) => Outcome::Win(e.clone()),
                _ => Outcome::Pending,
            })
            .collect();
        Round { pairings, results }
    }

    pub fn results(&self) -> &[Outcome] { &self.results }

    /// Report a result. Returns false if the index is unknown or it is a bye.
    ///
    /// A bye cannot be reported on: there was no match, and allowing a report
    /// would let a client rewrite an advancement nobody played.
    pub fn report(&mut self, index: usize, outcome: Outcome) -> bool {
        let Some(p) = self.pairings.get(index) else { return false };
        if p.is_bye() { return false; }
        // A result cannot be reported for someone who was not in the pairing —
        // otherwise a malformed or hostile report awards a match to a third
        // party.
        if let Outcome::Win(w) = &outcome {
            if !p.entrants().iter().any(|e| *e == w) { return false; }
        }
        self.results[index] = outcome;
        true
    }

    /// Is every match in this round decided?
    pub fn complete(&self) -> bool { self.results.iter().all(|r| r.is_decided()) }

    /// Who advances. Only meaningful once the round is complete.
    ///
    /// Returns `None` while anything is pending rather than a partial list: a
    /// caller that seeded the next round from half a result set would pair
    /// people who have not finished playing.
    pub fn advancing(&self) -> Option<Vec<EntrantId>> {
        if !self.complete() { return None; }
        Some(self.results.iter().filter_map(|r| match r {
            Outcome::Win(w) => Some(w.clone()),
            // A draw or a void advances nobody. The caller decides what to do —
            // a replay, a coin toss, a tiebreak — because that rule differs per
            // game and inventing one here would impose it on all of them.
            _ => None,
        }).collect())
    }
}

/// An entrant's record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Record {
    pub wins: u32,
    pub draws: u32,
    pub losses: u32,
    /// Advancements without a match. Counted separately so no tiebreak can
    /// treat them as wins.
    pub byes: u32,
}

impl Record {
    /// Matches actually played.
    pub fn played(&self) -> u32 { self.wins + self.draws + self.losses }

    /// Points, on the conventional 3/1/0. Byes score NOTHING here — a player
    /// who advanced without playing has not earned points, and the bracket
    /// already advanced them.
    pub fn points(&self) -> u32 { self.wins * 3 + self.draws }
}

/// A running tournament.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tournament {
    pub name: String,
    entrants: Vec<EntrantId>,
    rounds: Vec<Round>,
}

impl Tournament {
    pub fn new(name: &str, entrants: Vec<EntrantId>) -> Tournament {
        // Duplicates are dropped rather than rejected: two clients registering
        // the same profile is an ordinary race, and failing the whole
        // tournament over it would be worse than ignoring the second.
        let mut seen = Vec::new();
        for e in entrants {
            if !seen.contains(&e) { seen.push(e); }
        }
        Tournament { name: name.to_string(), entrants: seen, rounds: Vec::new() }
    }

    pub fn entrants(&self) -> &[EntrantId] { &self.entrants }
    pub fn rounds(&self) -> &[Round] { &self.rounds }
    pub fn round_mut(&mut self, i: usize) -> Option<&mut Round> { self.rounds.get_mut(i) }

    /// Pair a list into a round, giving the odd one out a bye.
    ///
    /// The bye goes to the LAST entrant in the supplied order, so the caller
    /// controls it — seeding, or rotating it between rounds so the same player
    /// does not get every one.
    pub fn pair(list: &[EntrantId]) -> Vec<Pairing> {
        let mut out = Vec::new();
        let mut i = 0;
        while i + 1 < list.len() {
            out.push(Pairing::Match { a: list[i].clone(), b: list[i + 1].clone() });
            i += 2;
        }
        if i < list.len() { out.push(Pairing::Bye(list[i].clone())); }
        out
    }

    /// Start the first round from the entrant list.
    pub fn start(&mut self) -> bool {
        // One entrant is not a tournament; producing a round of a single bye
        // would declare them champion without a match.
        if self.entrants.len() < 2 || !self.rounds.is_empty() { return false; }
        let p = Self::pair(&self.entrants.clone());
        self.rounds.push(Round::new(p));
        true
    }

    /// Seed the next round from the last one's winners.
    ///
    /// Refuses while the current round is incomplete, and refuses when fewer
    /// than two advance — at that point the tournament is over or stuck, and
    /// either way the caller needs to know rather than receive an empty round.
    pub fn advance(&mut self) -> bool {
        let Some(last) = self.rounds.last() else { return false };
        let Some(winners) = last.advancing() else { return false };
        if winners.len() < 2 { return false; }
        let p = Self::pair(&winners);
        self.rounds.push(Round::new(p));
        true
    }

    /// The champion, if the tournament has finished.
    pub fn champion(&self) -> Option<&EntrantId> {
        let last = self.rounds.last()?;
        let w = last.advancing()?;
        if w.len() == 1 { self.entrants.iter().find(|e| **e == w[0]) } else { None }
    }

    /// Every entrant's record.
    pub fn records(&self) -> BTreeMap<EntrantId, Record> {
        let mut out: BTreeMap<EntrantId, Record> = self.entrants.iter()
            .map(|e| (e.clone(), Record::default())).collect();
        for round in &self.rounds {
            for (p, r) in round.pairings.iter().zip(&round.results) {
                match (p, r) {
                    (Pairing::Bye(e), _) => {
                        if let Some(rec) = out.get_mut(e) { rec.byes += 1; }
                    }
                    (Pairing::Match { a, b }, Outcome::Win(w)) => {
                        let loser = if w == a { b } else { a };
                        if let Some(rec) = out.get_mut(w) { rec.wins += 1; }
                        if let Some(rec) = out.get_mut(loser) { rec.losses += 1; }
                    }
                    (Pairing::Match { a, b }, Outcome::Draw) => {
                        for e in [a, b] {
                            if let Some(rec) = out.get_mut(e) { rec.draws += 1; }
                        }
                    }
                    // Pending and Void contribute nothing: an unplayed match is
                    // not a result, and a void is the absence of one.
                    _ => {}
                }
            }
        }
        out
    }

    /// Standings, strongest first.
    ///
    /// Entrants on equal points AND equal matches played are returned adjacent
    /// but their relative order is not a ranking — `tied_with` says so
    /// explicitly, because a bracket that silently ordered them would let a
    /// display artefact decide who advances.
    pub fn standings(&self) -> Vec<(EntrantId, Record)> {
        let mut v: Vec<(EntrantId, Record)> = self.records().into_iter().collect();
        v.sort_by(|a, b| b.1.points().cmp(&a.1.points())
            .then_with(|| b.1.wins.cmp(&a.1.wins))
            .then_with(|| a.0.cmp(&b.0)));
        v
    }

    /// Are these two genuinely tied?
    pub fn tied_with(a: &Record, b: &Record) -> bool {
        a.points() == b.points() && a.wins == b.wins && a.played() == b.played()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<EntrantId> {
        (0..n).map(|i| format!("p{i}")).collect()
    }

    #[test]
    fn an_odd_entrant_count_produces_a_bye_not_a_free_win() {
        // Modelling a bye as a victory corrupts every statistic downstream: the
        // player shows a win they did not earn and a tiebreak on matches won
        // rewards whoever drew the short bracket.
        let p = Tournament::pair(&ids(5));
        assert_eq!(p.len(), 3);
        assert!(p[2].is_bye());
        let t = {
            let mut t = Tournament::new("cup", ids(5));
            t.start();
            t
        };
        let recs = t.records();
        assert_eq!(recs["p4"].byes, 1);
        assert_eq!(recs["p4"].wins, 0, "a bye is not a win");
        assert_eq!(recs["p4"].points(), 0, "and earns no points");
    }

    #[test]
    fn an_even_count_pairs_everybody() {
        let p = Tournament::pair(&ids(4));
        assert_eq!(p.len(), 2);
        assert!(p.iter().all(|x| !x.is_bye()));
    }

    #[test]
    fn a_match_with_no_report_is_pending_rather_than_a_draw() {
        // A tournament that auto-resolved would decide outcomes during a
        // network partition, which is exactly when it should wait — the match
        // may have been played and the report simply not arrived.
        let mut t = Tournament::new("cup", ids(4));
        t.start();
        assert!(!t.rounds()[0].complete());
        assert!(t.rounds()[0].advancing().is_none());
    }

    #[test]
    fn a_bye_is_decided_from_the_start() {
        // There is no match to wait for.
        let mut t = Tournament::new("cup", ids(3));
        t.start();
        let r = &t.rounds()[0];
        assert_eq!(r.results()[1], Outcome::Win("p2".into()));
    }

    #[test]
    fn a_bye_cannot_be_reported_on() {
        // Allowing it would let a client rewrite an advancement nobody played.
        let mut t = Tournament::new("cup", ids(3));
        t.start();
        let r = t.round_mut(0).unwrap();
        assert!(!r.report(1, Outcome::Win("p0".into())));
    }

    #[test]
    fn a_result_cannot_award_a_match_to_someone_who_was_not_in_it() {
        // Otherwise a malformed or hostile report hands a match to a third
        // party.
        let mut t = Tournament::new("cup", ids(4));
        t.start();
        let r = t.round_mut(0).unwrap();
        assert!(!r.report(0, Outcome::Win("p3".into())), "p3 is in the other match");
        assert!(r.report(0, Outcome::Win("p0".into())));
    }

    #[test]
    fn a_void_is_not_a_draw() {
        // A draw is a result; a void is the absence of one. Counting voids as
        // draws would reward whoever disconnects when losing.
        let mut t = Tournament::new("cup", ids(2));
        t.start();
        t.round_mut(0).unwrap().report(0, Outcome::Void);
        let recs = t.records();
        assert_eq!(recs["p0"].draws, 0);
        assert_eq!(recs["p0"].played(), 0);
        assert!(t.rounds()[0].complete(), "but it IS decided");
        assert!(t.rounds()[0].advancing().unwrap().is_empty(), "and nobody advances");
    }

    #[test]
    fn a_draw_advances_nobody_and_the_caller_decides() {
        // Replay, coin toss, tiebreak — that rule differs per game, and
        // inventing one here would impose it on all of them.
        let mut t = Tournament::new("cup", ids(2));
        t.start();
        t.round_mut(0).unwrap().report(0, Outcome::Draw);
        assert_eq!(t.rounds()[0].advancing(), Some(Vec::new()));
        assert_eq!(t.records()["p0"].draws, 1);
    }

    #[test]
    fn the_next_round_cannot_start_while_matches_are_pending() {
        // Seeding from half a result set would pair people who have not
        // finished playing.
        let mut t = Tournament::new("cup", ids(4));
        t.start();
        t.round_mut(0).unwrap().report(0, Outcome::Win("p0".into()));
        assert!(!t.advance());
        t.round_mut(0).unwrap().report(1, Outcome::Win("p2".into()));
        assert!(t.advance());
        assert_eq!(t.rounds().len(), 2);
    }

    #[test]
    fn a_finished_tournament_has_a_champion() {
        let mut t = Tournament::new("cup", ids(4));
        t.start();
        t.round_mut(0).unwrap().report(0, Outcome::Win("p0".into()));
        t.round_mut(0).unwrap().report(1, Outcome::Win("p2".into()));
        t.advance();
        t.round_mut(1).unwrap().report(0, Outcome::Win("p2".into()));
        assert_eq!(t.champion(), Some(&"p2".to_string()));
    }

    #[test]
    fn a_tournament_in_progress_has_no_champion() {
        let mut t = Tournament::new("cup", ids(4));
        t.start();
        assert_eq!(t.champion(), None);
    }

    #[test]
    fn one_entrant_is_not_a_tournament() {
        // A round of a single bye would declare them champion without a match.
        let mut t = Tournament::new("cup", ids(1));
        assert!(!t.start());
        assert!(t.rounds().is_empty());
        assert!(t.champion().is_none());
    }

    #[test]
    fn duplicate_registrations_are_dropped_rather_than_failing_the_tournament() {
        // Two clients registering the same profile is an ordinary race.
        let t = Tournament::new("cup",
            vec!["a".into(), "b".into(), "a".into(), "c".into()]);
        assert_eq!(t.entrants().len(), 3);
    }

    #[test]
    fn standings_rank_by_points_then_wins() {
        let mut t = Tournament::new("cup", ids(4));
        t.start();
        t.round_mut(0).unwrap().report(0, Outcome::Win("p0".into()));
        t.round_mut(0).unwrap().report(1, Outcome::Draw);
        let s = t.standings();
        assert_eq!(s[0].0, "p0");
        assert_eq!(s[0].1.points(), 3);
    }

    #[test]
    fn equal_records_are_reported_as_tied_rather_than_ordered() {
        // A bracket that silently ranked one above the other would let a
        // display artefact decide who advances.
        let mut t = Tournament::new("cup", ids(4));
        t.start();
        t.round_mut(0).unwrap().report(0, Outcome::Draw);
        t.round_mut(0).unwrap().report(1, Outcome::Draw);
        let s = t.standings();
        assert!(Tournament::tied_with(&s[0].1, &s[1].1));
        assert!(Tournament::tied_with(&s[0].1, &s[3].1));
    }

    #[test]
    fn a_bye_does_not_count_as_a_match_played() {
        // Any per-match average would otherwise be diluted by matches that
        // never happened.
        let mut t = Tournament::new("cup", ids(3));
        t.start();
        assert_eq!(t.records()["p2"].played(), 0);
        assert_eq!(t.records()["p2"].byes, 1);
    }

    #[test]
    fn starting_twice_does_nothing() {
        // A second start would discard reported results.
        let mut t = Tournament::new("cup", ids(4));
        assert!(t.start());
        assert!(!t.start());
        assert_eq!(t.rounds().len(), 1);
    }

    #[test]
    fn the_bye_goes_to_the_last_entrant_so_the_caller_controls_it() {
        // Seeding, or rotating it between rounds so the same player does not
        // get every one.
        let p = Tournament::pair(&["x".into(), "y".into(), "z".into()]);
        assert_eq!(p[1], Pairing::Bye("z".into()));
        let q = Tournament::pair(&["z".into(), "y".into(), "x".into()]);
        assert_eq!(q[1], Pairing::Bye("x".into()));
    }
}
