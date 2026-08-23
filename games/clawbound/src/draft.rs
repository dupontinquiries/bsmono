//! draft.rs — choosing who fights.
//!
//! The battleground has thirteen ancestors and, until now, no way for a player
//! to choose among them: `Battleground::new` filled both clans by pool order and
//! the terminal client took a single name on the command line. This is the logic
//! behind a pick screen — headless, so the same rules serve the TTY client, a
//! future menu, and its own tests, and so the interesting parts (what a legal
//! draft is, what the enemy clan does about yours) can be argued about without a
//! renderer in the way.
//!
//! The rules, and why:
//!
//! * **No duplicates within a clan.** Two Goldenmanes would each be running the
//!   other's Pride's Warmth, which is a mechanic the roster does not have.
//!   Across clans is fine — a mirror match is the fairest match there is, and
//!   `side_probe` depends on being able to build one.
//! * **The enemy answers your draft.** A clan facing two thin-pelted yowlers
//!   should bring something that can reach them. The counter-picker is
//!   deliberately simple and deterministic — it scores each remaining ancestor
//!   against what you brought — because an opponent that is unpredictable is
//!   only unfair, and because a rule you can read is a rule a player can learn
//!   to beat.

use crate::ancestor::{self, Ancestor, Verb};

/// A draft in progress: your picks so far, and who is left.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub picks: Vec<Ancestor>,
}

/// What went wrong with a pick, in words a screen can print.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DraftError {
    Unknown,
    AlreadyPicked,
    Full,
}

impl DraftError {
    pub fn say(self) -> &'static str {
        match self {
            DraftError::Unknown => "no ancestor by that name",
            DraftError::AlreadyPicked => "that one is already in your clan",
            DraftError::Full => "your clan is full",
        }
    }
}

pub const CLAN_SIZE: usize = 3;

impl Default for Draft {
    fn default() -> Self {
        Draft::new()
    }
}

impl Draft {
    pub fn new() -> Draft {
        Draft { picks: Vec::new() }
    }

    pub fn full(&self) -> bool {
        self.picks.len() >= CLAN_SIZE
    }

    /// Resolve a typed name: the ancestor's own name, or the animal behind it,
    /// case-insensitively. Both because a player who read the lore types
    /// "stillsmoke" and a player who read the roster types "suanni".
    pub fn find(name: &str) -> Option<Ancestor> {
        let want = name.trim().to_lowercase();
        if want.is_empty() {
            return None;
        }
        ancestor::ALL.iter().copied().find(|&a| {
            ancestor::kit(a).name.to_lowercase() == want || species(a) == want
        })
    }

    pub fn pick(&mut self, name: &str) -> Result<Ancestor, DraftError> {
        if self.full() {
            return Err(DraftError::Full);
        }
        let a = Draft::find(name).ok_or(DraftError::Unknown)?;
        if self.picks.contains(&a) {
            return Err(DraftError::AlreadyPicked);
        }
        self.picks.push(a);
        Ok(a)
    }

    pub fn undo(&mut self) -> Option<Ancestor> {
        self.picks.pop()
    }

    /// Fill the rest of a clan without repeating: pool order after the last
    /// pick, which is deterministic and spreads the archetypes rather than
    /// handing out three great cats.
    pub fn autofill(&self) -> [Ancestor; CLAN_SIZE] {
        let mut out = self.picks.clone();
        let start = out
            .last()
            .and_then(|&a| ancestor::ALL.iter().position(|&p| p == a))
            .map(|i| i + 1)
            .unwrap_or(0);
        let mut k = 0;
        while out.len() < CLAN_SIZE {
            let cand = ancestor::ALL[(start + k) % ancestor::ALL.len()];
            if !out.contains(&cand) {
                out.push(cand);
            }
            k += 1;
        }
        [out[0], out[1], out[2]]
    }

    /// The enemy clan, chosen *against* yours. Deterministic: each remaining
    /// ancestor is scored on how well it answers what you brought, and the three
    /// best are fielded.
    ///
    /// Your picks are off the table. The *rules* permit a mirror — two clans may
    /// field the same ancestor, and `side_probe` needs that to build a fair
    /// mirror match — but a counter-picker that answers your sun-priest by
    /// bringing your sun-priest reads as a shrug rather than a reply, and a
    /// player learns nothing from it.
    pub fn counter(&self) -> [Ancestor; CLAN_SIZE] {
        let mine = self.autofill();
        let mut scored: Vec<(i32, usize, Ancestor)> = ancestor::ALL
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, a)| !mine.contains(a))
            .map(|(i, a)| (counter_score(a, &mine), i, a))
            .collect();
        // best score first; pool order breaks ties, so the choice is repeatable
        scored.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.cmp(&y.1)));
        let mut out = Vec::new();
        for (_, _, a) in scored {
            if !out.contains(&a) {
                out.push(a);
            }
            if out.len() == CLAN_SIZE {
                break;
            }
        }
        [out[0], out[1], out[2]]
    }
}

/// The animal behind the warrior name, for players who think in species.
pub fn species(a: Ancestor) -> &'static str {
    match a {
        Ancestor::Lion => "lion",
        Ancestor::Tiger => "tiger",
        Ancestor::Leopard => "leopard",
        Ancestor::Panther => "panther",
        Ancestor::Caracal => "caracal",
        Ancestor::Puma => "puma",
        Ancestor::Serval => "serval",
        Ancestor::CaveLion => "cavelion",
        Ancestor::Smilodon => "smilodon",
        Ancestor::SunCat => "suncat",
        Ancestor::Lynx => "lynx",
        Ancestor::Jaguar => "jaguar",
        Ancestor::Suanni => "suanni",
    }
}

/// How well `a` answers the clan `foes`. Every term is a counter that exists in
/// the sim rather than a taste: light pelts want reaching into, veils want
/// burning out, and a clan of slow siege wants somebody faster than it.
fn counter_score(a: Ancestor, foes: &[Ancestor; CLAN_SIZE]) -> i32 {
    let k = ancestor::kit(a);
    let verbs: Vec<Verb> = k
        .abilities
        .iter()
        .map(|ab| ab.verb)
        .chain(std::iter::once(k.ult.verb))
        .collect();
    let mut s = 0;
    for &f in foes {
        let fk = ancestor::kit(f);
        // a thin pelt at a distance is answered by reach or by a gap-closer
        if fk.hp < 150.0 {
            if k.reach > 4.0 {
                s += 2;
            }
            if verbs.contains(&Verb::Dash) || verbs.contains(&Verb::Pull) {
                s += 2;
            }
        }
        // a veil is answered by light: only the sun-priest sears one away
        if verbs.contains(&Verb::Zone)
            && ancestor::kit(f)
                .abilities
                .iter()
                .any(|ab| ab.verb == Verb::Stealth)
        {
            s += 3;
        }
        // something that only wants to sit on your borders is answered by speed
        if fk.siege > 1.4 && k.speed > fk.speed + 0.5 {
            s += 1;
        }
        // and a clan that heals is answered by burst rather than by grinding
        if fk.abilities.iter().any(|ab| matches!(ab.verb, Verb::Heal | Verb::Rain)) {
            if verbs.contains(&Verb::Execute) || k.attack > 12.0 {
                s += 2;
            }
        }
    }
    s
}

/// One line per ancestor, for a pick screen or a `--roster` flag. The passive's
/// prose is clipped rather than wrapped: a pick screen is a table, and the two
/// longest passives in the roster run past a hundred and sixty columns.
pub fn roster_line(a: Ancestor) -> String {
    let k = ancestor::kit(a);
    let head = format!(
        "{:<13} {:<9} hp {:>3.0}  atk {:>4.1}  spd {:>3.1}  reach {:>4.1}   {} — ",
        k.name,
        species(a),
        k.hp,
        k.attack,
        k.speed,
        k.reach,
        k.passive_name,
    );
    let room = 158usize.saturating_sub(head.chars().count());
    let mut tail: String = k.passive.chars().take(room).collect();
    if tail.chars().count() < k.passive.chars().count() {
        // trim to a word boundary so the clip does not fall mid-syllable
        if let Some(cut) = tail.rfind(' ') {
            tail.truncate(cut);
        }
        tail.push('…');
    }
    head + &tail
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_can_be_the_warrior_or_the_animal() {
        assert_eq!(Draft::find("Stillsmoke"), Some(Ancestor::Suanni));
        assert_eq!(Draft::find("suanni"), Some(Ancestor::Suanni));
        assert_eq!(Draft::find("  GOLDENMANE "), Some(Ancestor::Lion));
        assert_eq!(Draft::find("lion"), Some(Ancestor::Lion));
        assert_eq!(Draft::find(""), None);
        assert_eq!(Draft::find("dogstar"), None);
        // every ancestor is reachable by both names, or a player cannot pick it
        for a in ancestor::ALL {
            assert_eq!(Draft::find(species(a)), Some(a));
            assert_eq!(Draft::find(ancestor::kit(a).name), Some(a));
        }
    }

    #[test]
    fn a_clan_cannot_field_the_same_ancestor_twice() {
        let mut d = Draft::new();
        assert_eq!(d.pick("lion"), Ok(Ancestor::Lion));
        assert_eq!(d.pick("Goldenmane"), Err(DraftError::AlreadyPicked));
        assert_eq!(d.pick("nobody"), Err(DraftError::Unknown));
        assert_eq!(d.pick("tiger"), Ok(Ancestor::Tiger));
        assert_eq!(d.pick("lynx"), Ok(Ancestor::Lynx));
        assert!(d.full());
        assert_eq!(d.pick("puma"), Err(DraftError::Full));
        assert_eq!(d.undo(), Some(Ancestor::Lynx));
        assert_eq!(d.pick("puma"), Ok(Ancestor::Puma));
    }

    #[test]
    fn autofill_completes_a_clan_without_repeating() {
        for a in ancestor::ALL {
            let mut d = Draft::new();
            d.pick(species(a)).unwrap();
            let full = d.autofill();
            assert_eq!(full[0], a, "the player's pick leads");
            assert_ne!(full[0], full[1]);
            assert_ne!(full[1], full[2]);
            assert_ne!(full[0], full[2]);
        }
        // and an empty draft still yields a legal clan
        let d = Draft::new();
        let f = d.autofill();
        assert_ne!(f[0], f[1]);
        assert_ne!(f[1], f[2]);
    }

    #[test]
    fn the_enemy_answers_what_you_brought() {
        // a clan of thin-pelted range should be met by something that can reach
        // or close on it
        let mut d = Draft::new();
        d.pick("serval").unwrap();
        d.pick("suncat").unwrap();
        d.pick("suanni").unwrap();
        let foes = d.counter();
        assert!(
            foes.iter().any(|&f| {
                let k = ancestor::kit(f);
                k.reach > 4.0
                    || k.abilities.iter().chain(std::iter::once(&k.ult)).any(|ab| {
                        matches!(ab.verb, Verb::Dash | Verb::Pull)
                    })
            }),
            "somebody has to be able to touch them: {foes:?}"
        );
        // a clan built on veils should draw the one kit whose light strips them
        let mut d = Draft::new();
        d.pick("tiger").unwrap();
        d.pick("panther").unwrap();
        assert!(
            d.counter().contains(&Ancestor::SunCat),
            "two veils should call the sun-priest out"
        );
        // and the answer is legal, repeatable, and not a copy of your homework
        let a = d.counter();
        assert_eq!(a, d.counter(), "a counter-pick must not be a dice roll");
        assert_ne!(a[0], a[1]);
        assert_ne!(a[1], a[2]);
        for a in ancestor::ALL {
            let mut d = Draft::new();
            d.pick(species(a)).unwrap();
            let mine = d.autofill();
            for f in d.counter() {
                assert!(!mine.contains(&f), "{:?} answered its own clan with {f:?}", mine);
            }
        }
    }

    #[test]
    fn the_roster_line_says_what_a_player_needs() {
        for a in ancestor::ALL {
            let line = roster_line(a);
            assert!(line.contains(ancestor::kit(a).name));
            assert!(line.contains(species(a)));
            assert!(line.contains(ancestor::kit(a).passive_name));
            // columns, not bytes: these lines are full of em-dashes, and a
            // terminal counts characters
            assert!(
                line.chars().count() < 160,
                "a roster line should fit a terminal: {} columns\n{line}",
                line.chars().count()
            );
        }
    }
}
