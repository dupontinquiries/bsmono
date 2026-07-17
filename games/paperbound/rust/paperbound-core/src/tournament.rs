//! tournament.rs — slice 7: the Foldworks tournament gamemode, ported from
//! rts_tournament.py — with one upgrade the Python herald can't afford: here
//! non-player matches are resolved by ACTUALLY FIGHTING them through the
//! skirmish engine, not a power heuristic. Same bracket rules:
//!
//!   • every faction seeds into a single-elimination bracket
//!   • the entrant list pads to the next power of two with byes
//!   • the player (when present) is seeded LAST so round one is always real
//!   • state persists to JSON so a run survives quitting
//!
//! Deterministic: skirmish itself is deterministic, so a bracket with the
//! same entrants replays to the same champion.

use crate::data::Faction;
use crate::skirmish::run_skirmish;
use std::collections::BTreeMap;
use std::path::Path;

pub const PLAYER: &str = "__player__";

#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub a: Option<String>, // None = bye
    pub b: Option<String>,
    pub winner: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Bracket {
    pub rounds: Vec<Vec<Match>>,
    pub round: usize,
}

fn next_pow2(n: usize) -> usize {
    let mut p = 1;
    while p < n {
        p *= 2;
    }
    p.max(2)
}

impl Bracket {
    /// rts_tournament.new: entrants padded with byes, player seeded last.
    pub fn new(mut entrants: Vec<String>) -> Self {
        if let Some(pi) = entrants.iter().position(|e| e == PLAYER) {
            let p = entrants.remove(pi);
            entrants.push(p);
        }
        let size = next_pow2(entrants.len());
        let mut slots: Vec<Option<String>> = entrants.into_iter().map(Some).collect();
        while slots.len() < size {
            // byes interleave from the FRONT so the late-seeded player never
            // draws one (the Python module's guarantee)
            slots.insert(0, None);
        }
        let first: Vec<Match> = slots
            .chunks(2)
            .map(|p| Match { a: p[0].clone(), b: p[1].clone(), winner: None })
            .collect();
        Bracket { rounds: vec![first], round: 0 }
    }

    pub fn champion(&self) -> Option<&str> {
        let last = self.rounds.last()?;
        if last.len() == 1 {
            last[0].winner.as_deref()
        } else {
            None
        }
    }

    /// Resolve every undecided match of the current round by playing it in
    /// the skirmish engine (byes advance silently), then build the next round.
    pub fn play_round(&mut self, book: &BTreeMap<String, Faction>) -> Vec<(String, String, String)> {
        let mut tales = Vec::new();
        let n = self.rounds[self.round].len();
        for mi in 0..n {
            let (a, b, decided) = {
                let m = &self.rounds[self.round][mi];
                (m.a.clone(), m.b.clone(), m.winner.is_some())
            };
            if decided {
                continue;
            }
            let w = match (a, b) {
                (Some(x), None) | (None, Some(x)) => x,
                (Some(x), Some(y)) => {
                    let rep = run_skirmish(book, &x, &y, 900.0, None, 10.0);
                    let w = match rep.winner {
                        Some(0) => x.clone(),
                        Some(_) => y.clone(),
                        // a stalemate falls to whoever holds the bigger army,
                        // then the deeper ink reserves — never a coin flip
                        None => {
                            if rep.units_left[0] != rep.units_left[1] {
                                if rep.units_left[0] > rep.units_left[1] { x.clone() } else { y.clone() }
                            } else if rep.ink[0] >= rep.ink[1] {
                                x.clone()
                            } else {
                                y.clone()
                            }
                        }
                    };
                    tales.push((x, y, w.clone()));
                    w
                }
                (None, None) => continue,
            };
            self.rounds[self.round][mi].winner = Some(w);
        }
        // advance
        let winners: Vec<Option<String>> = self.rounds[self.round]
            .iter()
            .map(|m| m.winner.clone())
            .collect();
        if winners.len() > 1 {
            let next: Vec<Match> = winners
                .chunks(2)
                .map(|p| Match { a: p[0].clone(), b: p.get(1).cloned().flatten(), winner: None })
                .collect();
            self.rounds.push(next);
            self.round += 1;
        }
        tales
    }

    /// Run the whole bracket to a champion. Returns (champion, match tales).
    pub fn run(
        entrants: Vec<String>,
        book: &BTreeMap<String, Faction>,
    ) -> (String, Vec<(String, String, String)>) {
        let mut br = Bracket::new(entrants);
        let mut tales = Vec::new();
        for _ in 0..12 {
            tales.extend(br.play_round(book));
            if let Some(c) = br.champion() {
                return (c.to_string(), tales);
            }
        }
        ("".into(), tales)
    }

    // ── persistence, mirroring tournament.json's survive-a-quit contract ──
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut out = String::from("{\n \"round\": ");
        out.push_str(&self.round.to_string());
        out.push_str(",\n \"rounds\": [\n");
        for (ri, r) in self.rounds.iter().enumerate() {
            out.push_str("  [");
            for (mi, m) in r.iter().enumerate() {
                let q = |o: &Option<String>| match o {
                    Some(s) => format!("\"{s}\""),
                    None => "null".into(),
                };
                out.push_str(&format!(
                    "{{\"a\":{},\"b\":{},\"winner\":{}}}",
                    q(&m.a),
                    q(&m.b),
                    q(&m.winner)
                ));
                if mi + 1 < r.len() {
                    out.push(',');
                }
            }
            out.push(']');
            if ri + 1 < self.rounds.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str(" ]\n}\n");
        std::fs::write(path, out)
    }

    pub fn load(path: &Path) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        let doc: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let round = doc["round"].as_u64()? as usize;
        let mut rounds = Vec::new();
        for r in doc["rounds"].as_array()? {
            let ms: Vec<Match> = r
                .as_array()?
                .iter()
                .map(|m| Match {
                    a: m["a"].as_str().map(String::from),
                    b: m["b"].as_str().map(String::from),
                    winner: m["winner"].as_str().map(String::from),
                })
                .collect();
            rounds.push(ms);
        }
        Some(Bracket { rounds, round })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::load_factbook;

    #[test]
    fn brackets_pad_and_seed_the_player_clear_of_byes() {
        let br = Bracket::new(vec![
            "a".into(),
            PLAYER.into(),
            "b".into(),
            "c".into(),
            "d".into(),
        ]);
        assert_eq!(br.rounds[0].len(), 4, "5 entrants pad to 8 slots");
        let byes: usize = br.rounds[0]
            .iter()
            .map(|m| (m.a.is_none() as usize) + (m.b.is_none() as usize))
            .sum();
        assert_eq!(byes, 3);
        // the player's match has NO bye in it
        let pm = br.rounds[0]
            .iter()
            .find(|m| m.a.as_deref() == Some(PLAYER) || m.b.as_deref() == Some(PLAYER))
            .unwrap();
        assert!(pm.a.is_some() && pm.b.is_some(), "player round one is real");
    }

    #[test]
    fn a_full_bracket_crowns_a_deterministic_champion() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let book = load_factbook(&dir).unwrap();
        let entrants: Vec<String> =
            ["court", "ember", "tide", "press"].iter().map(|s| s.to_string()).collect();
        let (champ1, tales1) = Bracket::run(entrants.clone(), &book);
        let (champ2, tales2) = Bracket::run(entrants, &book);
        assert!(!champ1.is_empty(), "someone must win");
        assert_eq!(champ1, champ2, "deterministic engine → deterministic crown");
        assert_eq!(tales1, tales2);
        assert_eq!(tales1.len(), 3, "4 entrants → 2 semis + 1 final");
    }

    #[test]
    fn persistence_round_trips() {
        let br = Bracket::new(vec!["a".into(), "b".into(), "c".into()]);
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/tournament_test.json");
        br.save(&p).unwrap();
        let lo = Bracket::load(&p).unwrap();
        assert_eq!(lo.round, br.round);
        assert_eq!(lo.rounds[0], br.rounds[0]);
        std::fs::remove_file(&p).ok();
    }
}
