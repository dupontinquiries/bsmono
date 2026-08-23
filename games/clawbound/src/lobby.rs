//! lobby.rs — quick play, and the page where ten cats are chosen.
//!
//! The page a Smite player knows: you press one button, you are put in a clan of
//! five, a clock starts, everyone picks an ancestor, and when the clock runs out
//! the match begins whether you were ready or not. This is that page's logic,
//! headless — the same reason `draft.rs` and `battleview.rs` are headless. A
//! select screen is mostly *rules about time and legality*, and rules are worth
//! testing without a renderer in front of them.
//!
//! ## Made for three sets of hands at once
//!
//! Every input arrives as a [`LobbyInput`], and nothing in here knows which
//! device produced it. That is not politeness, it is the only way one screen
//! serves a phone, a controller and a keyboard without three code paths that
//! drift apart:
//!
//! * **Touch** — [`LobbyInput::Point`] carries a grid cell, because a thumb hits
//!   a portrait of a cat, not a cursor position. [`Lobby::grid`] hands the
//!   renderer the same cell layout it will be given back, so the hit-test is
//!   this module's business rather than the UI's.
//! * **Controller** — [`LobbyInput::Move`] walks the grid a cell at a time and
//!   wraps at the edges, so a stick or a d-pad never gets stuck against a wall.
//! * **Keyboard** — the same `Move` for arrows, plus [`LobbyInput::Search`] for
//!   the thing only a keyboard can do quickly: type "still" and land on
//!   Stillsmoke. Search is additive; nobody else has to know it exists.
//!
//! ## What the clock is for
//!
//! A select screen that waits forever is a select screen that never starts a
//! match. Time runs down; when it does, anyone unlocked is **auto-picked** from
//! what they were hovering (or the first legal ancestor), and the match starts
//! after a short [`READY_SECS`] beat so the clans can be read. Locking in early
//! does not skip the clock — the other nine cats are still choosing, and the
//! screen is theirs too — but everyone locking in *does*, because then the clock
//! is only making people wait.

use crate::ancestor::{self, Ancestor};
use crate::draft::{self, Draft};

/// Seats per clan on this page. The battleground supports one to five; the
/// lobby always fields the full conquest shape.
pub const CLAN: usize = 5;
/// How long the picking lasts.
pub const PICK_SECS: f32 = 30.0;
/// The beat between the last lock and the first tick of the match, so both
/// clans can be read before the ground appears.
pub const READY_SECS: f32 = 5.0;
/// Grid width on the ancestor page. Three columns fits a portrait phone; the
/// thirteen wrap into five rows with the last row short.
pub const COLS: usize = 3;

/// Where the lobby is in its short life.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Nothing chosen yet: the quick-play button and nothing else.
    Idle,
    /// Ten seats, a clock, and thirteen portraits.
    Picking,
    /// Everyone is locked; the match is a few seconds away.
    Ready,
    /// Hand these rosters to the battleground and go.
    Launch,
}

/// One device-independent thing a player can do on this page.
#[derive(Clone, Debug, PartialEq)]
pub enum LobbyInput {
    /// The quick-play button, or the same key from any device.
    QuickPlay,
    /// Walk the grid: (dx, dy). Wraps, so a stick never jams on an edge.
    Move(i32, i32),
    /// A tap or click on a grid cell.
    Point(usize),
    /// Take the hovered ancestor.
    Lock,
    /// Change your mind, while the clock allows it.
    Unlock,
    /// Keyboard-only: filter the grid by a typed fragment.
    Search(String),
    /// Leave.
    Back,
}

/// A seat on the page: who holds it, what they are hovering, whether they are
/// locked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seat {
    pub human: bool,
    pub hover: Option<Ancestor>,
    pub locked: bool,
}

pub struct Lobby {
    pub phase: Phase,
    /// Clan A then clan B, five seats each. Seat `(0, 0)` is you.
    pub seats: [[Seat; CLAN]; 2],
    pub clock: f32,
    /// Cursor into the *filtered* grid.
    pub cursor: usize,
    pub search: String,
    /// Bumped every time a bot picks or the clock rolls a second, so a renderer
    /// knows something changed without diffing the whole page.
    pub revision: u32,
    rng: u64,
}

impl Default for Lobby {
    fn default() -> Self {
        Lobby::new()
    }
}

impl Lobby {
    pub fn new() -> Lobby {
        let empty = Seat { human: false, hover: None, locked: false };
        let mut seats = [[empty; CLAN]; 2];
        seats[0][0].human = true;
        Lobby {
            phase: Phase::Idle,
            seats,
            clock: 0.0,
            cursor: 0,
            search: String::new(),
            revision: 0,
            rng: 0x9e37_79b9_7f4a_7c15,
        }
    }

    fn roll(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    /// Which ancestors this clan may still take: no duplicates inside a clan,
    /// mirrors across clans allowed — the same rule `draft.rs` argues for.
    pub fn legal(&self, side: usize) -> Vec<Ancestor> {
        ancestor::ALL
            .iter()
            .copied()
            .filter(|a| {
                !self.seats[side]
                    .iter()
                    .any(|s| s.locked && s.hover == Some(*a))
            })
            .collect()
    }

    /// The grid the page shows: legal ancestors for your clan, filtered by the
    /// search box, in rows of [`COLS`]. The renderer draws this and hands back
    /// the index of whatever was tapped, so the hit-test and the layout cannot
    /// disagree.
    pub fn grid(&self) -> Vec<Ancestor> {
        let want = self.search.trim().to_lowercase();
        self.legal(0)
            .into_iter()
            .filter(|&a| {
                want.is_empty()
                    || ancestor::kit(a).name.to_lowercase().contains(&want)
                    || draft::species(a).contains(&want)
            })
            .collect()
    }

    pub fn hovered(&self) -> Option<Ancestor> {
        let g = self.grid();
        g.get(self.cursor.min(g.len().saturating_sub(1))).copied()
    }

    pub fn you(&self) -> Seat {
        self.seats[0][0]
    }

    fn take_seat(&mut self, side: usize, seat: usize, a: Ancestor) {
        self.seats[side][seat].hover = Some(a);
        self.seats[side][seat].locked = true;
        self.revision += 1;
    }

    pub fn handle(&mut self, input: LobbyInput) {
        match input {
            LobbyInput::QuickPlay => {
                if self.phase == Phase::Idle {
                    self.phase = Phase::Picking;
                    self.clock = PICK_SECS;
                    self.revision += 1;
                }
            }
            LobbyInput::Back => {
                *self = Lobby::new();
            }
            _ if self.phase != Phase::Picking => {}
            LobbyInput::Move(dx, dy) => {
                let n = self.grid().len().max(1) as i32;
                let cols = COLS as i32;
                let mut c = self.cursor as i32 + dx + dy * cols;
                // wrap both ways: a controller must never find a wall
                c = c.rem_euclid(n);
                self.cursor = c as usize;
                self.revision += 1;
            }
            LobbyInput::Point(cell) => {
                if cell < self.grid().len() {
                    self.cursor = cell;
                    self.revision += 1;
                }
            }
            LobbyInput::Search(s) => {
                self.search = s;
                self.cursor = 0;
                self.revision += 1;
            }
            LobbyInput::Lock => {
                if let Some(a) = self.hovered() {
                    if !self.you().locked {
                        self.take_seat(0, 0, a);
                    }
                }
            }
            LobbyInput::Unlock => {
                self.seats[0][0].locked = false;
                self.revision += 1;
            }
        }
    }

    /// Advance the page. Bots pick as the clock runs down rather than all at
    /// once, because a screen where nine seats fill instantly is a screen that
    /// tells you nothing about what you are about to play against.
    pub fn tick(&mut self, dt: f32) {
        match self.phase {
            Phase::Picking => {
                self.clock -= dt;
                // roughly one bot decision a second, deterministic in order
                for side in 0..2 {
                    for seat in 0..CLAN {
                        if self.seats[side][seat].human || self.seats[side][seat].locked {
                            continue;
                        }
                        // A bot's nerve goes as the clock does: roughly one
                        // decision a second early on, near-certain in the last
                        // few. The flat one-in-forty left a seat unlocked about
                        // one page in twenty — `force_remaining` covered it, but
                        // a seat that fills on the buzzer with no deliberation
                        // reads as a bug to whoever is watching the column.
                        let urgency = 1.0 - (self.clock / PICK_SECS).clamp(0.0, 1.0);
                        let odds = (40.0 * (1.0 - urgency * 0.9)).max(2.0) as u64;
                        if self.roll() % odds != 0 {
                            continue;
                        }
                        let legal = self.legal(side);
                        if legal.is_empty() {
                            continue;
                        }
                        let pick = legal[(self.roll() as usize) % legal.len()];
                        self.take_seat(side, seat, pick);
                    }
                }
                let everyone = self.seats.iter().flatten().all(|s| s.locked);
                if self.clock <= 0.0 || everyone {
                    self.force_remaining();
                    self.phase = Phase::Ready;
                    self.clock = READY_SECS;
                    self.revision += 1;
                }
            }
            Phase::Ready => {
                self.clock -= dt;
                if self.clock <= 0.0 {
                    self.phase = Phase::Launch;
                    self.revision += 1;
                }
            }
            _ => {}
        }
    }

    /// Nobody is left unpicked when the clock runs out: an unlocked seat takes
    /// what it was hovering, and a seat that never hovered takes the first legal
    /// ancestor. A player who walked away still gets a cat.
    fn force_remaining(&mut self) {
        for side in 0..2 {
            for seat in 0..CLAN {
                if self.seats[side][seat].locked {
                    continue;
                }
                let legal = self.legal(side);
                let pick = self.seats[side][seat]
                    .hover
                    .filter(|a| legal.contains(a))
                    .or_else(|| legal.first().copied());
                if let Some(a) = pick {
                    self.take_seat(side, seat, a);
                }
            }
        }
    }

    /// The two rosters, once the page is done with them.
    pub fn rosters(&self) -> Option<([Ancestor; CLAN], [Ancestor; CLAN])> {
        if self.phase != Phase::Launch {
            return None;
        }
        let side = |i: usize| -> Option<[Ancestor; CLAN]> {
            let v: Vec<Ancestor> = self.seats[i].iter().filter_map(|s| s.hover).collect();
            if v.len() == CLAN {
                Some([v[0], v[1], v[2], v[3], v[4]])
            } else {
                None
            }
        };
        Some((side(0)?, side(1)?))
    }

    /// What the clock should say, in whole seconds — a page like this counts
    /// down out loud.
    pub fn countdown(&self) -> u32 {
        self.clock.max(0.0).ceil() as u32
    }

    /// A line per seat for the two clan columns.
    pub fn seat_label(&self, side: usize, seat: usize) -> String {
        let s = self.seats[side][seat];
        let who = if s.human { "you" } else { "clanmate" };
        match (s.hover, s.locked) {
            (Some(a), true) => format!("{who}: {}", ancestor::kit(a).name),
            (Some(a), false) => format!("{who}: {}…", ancestor::kit(a).name),
            (None, _) => format!("{who}: choosing"),
        }
    }
}

/// Everything the lobby knows, for one draft that skips the page entirely —
/// the "again" button after a match.
pub fn quick_rosters(you: Ancestor, seed: u64) -> ([Ancestor; CLAN], [Ancestor; CLAN]) {
    let mut l = Lobby::new();
    l.rng = seed | 1;
    l.handle(LobbyInput::QuickPlay);
    let g = l.grid();
    if let Some(i) = g.iter().position(|&a| a == you) {
        l.handle(LobbyInput::Point(i));
        l.handle(LobbyInput::Lock);
    }
    // run the clock out rather than reimplementing the filling rules
    while l.phase != Phase::Launch {
        l.tick(0.5);
    }
    l.rosters().expect("a finished lobby has rosters")
}

/// The counter-picked enemy clan, for a player who wants an *answer* rather than
/// a random five — the same rule `draft.rs` uses, extended to a clan of five.
pub fn counter_five(mine: &[Ancestor]) -> Vec<Ancestor> {
    let mut d = Draft::new();
    for a in mine.iter().take(3) {
        let _ = d.pick(draft::species(*a));
    }
    let mut out = d.counter().to_vec();
    for a in ancestor::ALL {
        if out.len() == CLAN {
            break;
        }
        if !out.contains(&a) && !mine.contains(&a) {
            out.push(a);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picking() -> Lobby {
        let mut l = Lobby::new();
        l.handle(LobbyInput::QuickPlay);
        l
    }

    #[test]
    fn quick_play_opens_the_page_and_starts_the_clock() {
        let mut l = Lobby::new();
        assert_eq!(l.phase, Phase::Idle);
        assert_eq!(l.countdown(), 0);
        l.handle(LobbyInput::QuickPlay);
        assert_eq!(l.phase, Phase::Picking);
        assert_eq!(l.countdown(), PICK_SECS as u32);
        assert_eq!(l.seats[0][0].human, true, "seat one of clan A is you");
        assert_eq!(l.seats.iter().flatten().filter(|s| s.human).count(), 1);
        assert_eq!(l.seats.iter().flatten().count(), CLAN * 2, "five a side");
    }

    #[test]
    fn three_sets_of_hands_reach_the_same_cat() {
        // controller: walk the grid. keyboard: type. touch: tap the cell.
        // All three have to land on the same ancestor, or the page is three
        // pages wearing one hat.
        let want = ancestor::ALL[4];
        let mut pad = picking();
        let idx = pad.grid().iter().position(|&a| a == want).unwrap();
        for _ in 0..idx {
            pad.handle(LobbyInput::Move(1, 0));
        }
        assert_eq!(pad.hovered(), Some(want));

        let mut kbd = picking();
        kbd.handle(LobbyInput::Search(ancestor::kit(want).name[..4].to_string()));
        assert_eq!(kbd.hovered(), Some(want), "typing narrows to it");

        let mut touch = picking();
        touch.handle(LobbyInput::Point(idx));
        assert_eq!(touch.hovered(), Some(want));

        for l in [&mut pad, &mut kbd, &mut touch] {
            l.handle(LobbyInput::Lock);
            assert_eq!(l.you().hover, Some(want));
            assert!(l.you().locked);
        }
    }

    #[test]
    fn a_stick_never_finds_a_wall() {
        let mut l = picking();
        let n = l.grid().len();
        // walking right off the end wraps; walking down off the bottom wraps
        for _ in 0..n + 3 {
            l.handle(LobbyInput::Move(1, 0));
            assert!(l.cursor < n);
        }
        for _ in 0..8 {
            l.handle(LobbyInput::Move(0, 1));
            assert!(l.cursor < n);
        }
        l.handle(LobbyInput::Move(-1, 0));
        assert!(l.cursor < n);
        // and a search that matches nothing leaves the cursor somewhere legal
        l.handle(LobbyInput::Search("qqq".into()));
        assert!(l.grid().is_empty());
        assert_eq!(l.hovered(), None, "nothing to hover, and nothing panics");
        l.handle(LobbyInput::Lock);
        assert!(!l.you().locked, "you cannot lock in a cat that is not there");
    }

    #[test]
    fn a_clan_cannot_field_the_same_ancestor_twice_but_the_enemy_can() {
        let mut l = picking();
        let mine = l.grid()[0];
        l.handle(LobbyInput::Lock);
        assert!(!l.legal(0).contains(&mine), "your clan may not take it again");
        assert!(l.legal(1).contains(&mine), "the enemy clan still may");
        assert!(!l.grid().contains(&mine), "and it leaves your grid");
    }

    #[test]
    fn the_clock_starts_the_match_whether_you_were_ready_or_not() {
        let mut l = picking();
        // never touch anything
        let mut t = 0.0;
        while l.phase == Phase::Picking && t < PICK_SECS + 1.0 {
            l.tick(0.1);
            t += 0.1;
        }
        assert_eq!(l.phase, Phase::Ready, "the clock ran out and the page moved on");
        assert!(l.seats.iter().flatten().all(|s| s.locked), "every seat was filled");
        assert!(l.you().hover.is_some(), "including the one that walked away");
        // and the ready beat is a beat, not a wait
        let mut t = 0.0;
        while l.phase == Phase::Ready && t < READY_SECS + 1.0 {
            l.tick(0.1);
            t += 0.1;
        }
        assert_eq!(l.phase, Phase::Launch);
        let (a, b) = l.rosters().expect("rosters");
        assert_eq!(a.len(), CLAN);
        assert_eq!(b.len(), CLAN);
        // legal clans: no repeats within a side
        for side in [a, b] {
            let mut v = side.to_vec();
            v.sort_by_key(|x| format!("{x:?}"));
            v.dedup();
            assert_eq!(v.len(), CLAN, "a clan fielded the same ancestor twice");
        }
    }

    #[test]
    fn everyone_locking_in_skips_the_rest_of_the_clock() {
        let mut l = picking();
        l.handle(LobbyInput::Lock);
        // let the bots do their thing; they should finish well before the clock
        let mut t = 0.0;
        while l.phase == Phase::Picking && t < PICK_SECS {
            l.tick(0.1);
            t += 0.1;
        }
        assert_eq!(l.phase, Phase::Ready);
        assert!(t < PICK_SECS - 1.0, "waiting out a clock nobody needs: {t:.1}s");
    }

    #[test]
    fn the_bots_have_all_chosen_before_the_buzzer() {
        // Not a cosmetic point: a column of "choosing" that resolves the instant
        // the clock hits zero looks broken, and hides whether the filling rules
        // work at all.
        for seed in 0..12u64 {
            let mut l = picking();
            l.rng = 0x51ed_2701_9c3f_a1b7 ^ (seed * 0x9e37_79b9);
            let mut t = 0.0;
            while l.phase == Phase::Picking && t < PICK_SECS - 1.0 {
                l.tick(0.1);
                t += 0.1;
            }
            let unlocked = l.seats.iter().flatten().filter(|s| !s.locked && !s.human).count();
            assert_eq!(unlocked, 0, "seed {seed}: {unlocked} clanmates still dithering at the buzzer");
        }
    }

    #[test]
    fn a_page_can_be_left_and_a_lock_can_be_reconsidered() {
        let mut l = picking();
        l.handle(LobbyInput::Lock);
        assert!(l.you().locked);
        l.handle(LobbyInput::Unlock);
        assert!(!l.you().locked, "the clock has not run out yet");
        l.handle(LobbyInput::Back);
        assert_eq!(l.phase, Phase::Idle);
        assert_eq!(l.rosters(), None);
    }

    #[test]
    fn the_rosters_a_lobby_produces_start_a_match() {
        // The whole point of the page: what comes off it is what the sim takes.
        let (a, b) = quick_rosters(ancestor::ALL[7], 12345);
        let mut bg = crate::battleground::Battleground::new_roster(
            &a,
            &b,
            0,
            crate::battleground::LanePolicy::Kit,
        );
        assert!(a.contains(&ancestor::ALL[7]), "you got the cat you asked for");
        for _ in 0..30 * 30 {
            bg.step(1.0 / 30.0);
        }
        assert_eq!(
            bg.warriors
                .iter()
                .filter(|w| w.role == crate::battleground::Role::Ancestor)
                .count(),
            CLAN * 2
        );
    }

    #[test]
    fn the_enemy_five_can_be_an_answer_rather_than_a_shuffle() {
        let mine = [
            ancestor::Ancestor::Tiger,
            ancestor::Ancestor::Panther,
            ancestor::Ancestor::Serval,
        ];
        let theirs = counter_five(&mine);
        assert_eq!(theirs.len(), CLAN);
        for a in &mine {
            assert!(!theirs.contains(a), "an answer, not a copy");
        }
        let mut v = theirs.clone();
        v.sort_by_key(|x| format!("{x:?}"));
        v.dedup();
        assert_eq!(v.len(), CLAN, "and a legal clan");
    }
}
