//! picks -- who plays whom: the match's god limit, and dealing the seats.
//!
//! A match's [`GodLimit`] says how often one ancestor may be fielded:
//!
//! * [`GodLimit::Lobby`] (the default) -- every cat in the match different.
//!   On the 15-a-side maps there are more seats (30) than lines (20), so
//!   there it means as few repeats as the roster allows: no line twice
//!   until every line has been fielded once, and never twice on one side.
//! * [`GodLimit::Team`] -- no line twice on a side; the two sides may field
//!   the same ones (a mirror is possible).
//! * [`GodLimit::Open`] -- no limit: any line, any number of times. With
//!   [`Fill::Mirror`] every seat the player does not choose is filled with
//!   the player's own line (fifteen Claypaws a side).
//!
//! [`allowed`] is the rule itself, seat by seat, for anything that picks
//! one cat at a time (a draft screen, a lobby greying out taken cats).
//! [`deal`] fills the seats the bots take by asking [`allowed`] for every
//! seat in turn, so whatever it deals is legal under the rule by
//! construction.
//!
//! Headless: the select screen's picker is `ui::god_limit_picker`, the
//! match start `app::start_game`.

use crate::sim::Rng;

/// How often one ancestor may be fielded in a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GodLimit {
    #[default]
    Lobby,
    Team,
    Open,
}

impl GodLimit {
    pub const ALL: [GodLimit; 3] = [GodLimit::Lobby, GodLimit::Team, GodLimit::Open];

    /// The name on the select screen.
    pub fn name(self) -> &'static str {
        match self {
            GodLimit::Lobby => "One per match",
            GodLimit::Team => "One per team",
            GodLimit::Open => "No limit",
        }
    }

    /// The key on the command line (`--gods`) and in `CLAWBOUND_GODS`.
    pub fn key(self) -> &'static str {
        match self {
            GodLimit::Lobby => "lobby",
            GodLimit::Team => "team",
            GodLimit::Open => "open",
        }
    }

    pub fn from_key(k: &str) -> Option<GodLimit> {
        match k.trim().to_ascii_lowercase().as_str() {
            "lobby" | "match" | "unique" => Some(GodLimit::Lobby),
            "team" | "side" => Some(GodLimit::Team),
            "open" | "none" | "any" | "nolimit" | "no-limit" => Some(GodLimit::Open),
            _ => None,
        }
    }

    /// One line on what it does on a map of `size` a side, `n` lines.
    pub fn blurb(self, size: usize, n: usize) -> String {
        match self {
            GodLimit::Lobby if 2 * size <= n => "Every cat in the match is a different ancestor.".into(),
            GodLimit::Lobby => format!("{} seats, {n} ancestors: every one fields before any fields twice, and never twice on a side.", 2 * size),
            GodLimit::Team => "No ancestor twice on a side; the other side may field the same ones.".into(),
            GodLimit::Open => "Any ancestor, any number of times.".into(),
        }
    }
}

/// How the bots' seats are filled under [`GodLimit::Open`] (the other
/// limits always fill at random).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fill {
    #[default]
    Random,
    /// Every seat plays the player's own ancestor.
    Mirror,
}

/// May `god` take a seat on `blue` (true) or red, with `blue` and `red`
/// already seated, from a roster of `n` lines?
pub fn allowed(limit: GodLimit, n: usize, blue: &[u32], red: &[u32], on_blue: bool, god: u32) -> bool {
    if god as usize >= n {
        return false;
    }
    let own = if on_blue { blue } else { red };
    let on_own = own.contains(&god);
    match limit {
        GodLimit::Open => true,
        GodLimit::Team => !on_own,
        GodLimit::Lobby => {
            if on_own {
                return false;
            }
            let fielded = |g: u32| blue.contains(&g) || red.contains(&g);
            // a repeat only once every line is in the match
            !fielded(god) || (0..n as u32).all(fielded)
        }
    }
}

/// The seats for a match of `size` a side from a roster of `n`: blue's
/// first is the `player`'s. Blue's seats are dealt, then red's, each drawn
/// uniformly from the lines [`allowed`] lets sit there (in id order, so the
/// draw is reproducible from the seed).
pub fn deal(limit: GodLimit, fill: Fill, player: u32, size: usize, n: usize, rng: &mut Rng) -> (Vec<u32>, Vec<u32>) {
    let mut blue = vec![player];
    let mut red: Vec<u32> = Vec::with_capacity(size);
    if limit == GodLimit::Open && fill == Fill::Mirror {
        blue.resize(size, player);
        red.resize(size, player);
        return (blue, red);
    }
    for on_blue in std::iter::repeat_n(true, size.saturating_sub(1)).chain(std::iter::repeat_n(false, size)) {
        let ok: Vec<u32> = (0..n as u32).filter(|&g| allowed(limit, n, &blue, &red, on_blue, g)).collect();
        // (a side larger than the roster under Team or Lobby: nothing left
        // is legal, so the seat is filled as Open would fill it rather than
        // left empty)
        let pick = if ok.is_empty() { (rng.next_u() % n as u64) as u32 } else { ok[(rng.next_u() % ok.len() as u64) as usize] };
        if on_blue {
            blue.push(pick);
        } else {
            red.push(pick);
        }
    }
    (blue, red)
}

/// Deal around seated humans (a LAN match): `seated` is `(on_blue, seat on that side, pick)`
/// in player order. Each human's pick is placed first -- a pick the limit refuses against the
/// humans already seated (a second Claypaw under "one per match") is dealt instead, as the
/// bots' seats are -- and every other seat is filled as [`deal`] fills it. Under
/// [`Fill::Mirror`] the empty seats are the first human's line. Both sides come back
/// `size` long.
pub fn deal_seated(limit: GodLimit, fill: Fill, seated: &[(bool, usize, u32)], size: usize, n: usize, rng: &mut Rng) -> (Vec<u32>, Vec<u32>) {
    let mut blue: Vec<Option<u32>> = vec![None; size];
    let mut red: Vec<Option<u32>> = vec![None; size];
    let placed = |v: &[Option<u32>]| -> Vec<u32> { v.iter().flatten().copied().collect() };
    for &(on_blue, seat, pick) in seated {
        let taken = if on_blue { blue.get(seat).map_or(true, |s| s.is_some()) } else { red.get(seat).map_or(true, |s| s.is_some()) };
        if taken || pick as usize >= n {
            continue;
        }
        let ok = allowed(limit, n, &placed(&blue), &placed(&red), on_blue, pick);
        if ok {
            let side = if on_blue { &mut blue } else { &mut red };
            side[seat] = Some(pick);
        }
    }
    let first = seated.first().map(|s| s.2).unwrap_or(0);
    for on_blue in [true, false] {
        for seat in 0..size {
            let side = if on_blue { &blue } else { &red };
            if side[seat].is_some() {
                continue;
            }
            let pick = if limit == GodLimit::Open && fill == Fill::Mirror {
                first
            } else {
                let ok: Vec<u32> = (0..n as u32).filter(|&g| allowed(limit, n, &placed(&blue), &placed(&red), on_blue, g)).collect();
                if ok.is_empty() { (rng.next_u() % n as u64) as u32 } else { ok[(rng.next_u() % ok.len() as u64) as usize] }
            };
            let side = if on_blue { &mut blue } else { &mut red };
            side[seat] = Some(pick);
        }
    }
    (blue.into_iter().flatten().collect(), red.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Humans keep their picks where the limit allows, lose them to the deal where it does
    /// not, and the rest of the seats are filled legally; Open+Mirror fills with the host's.
    #[test]
    fn humans_are_seated_first_and_the_limit_still_holds() {
        let mut rng = Rng::new(5);
        let seated = [(true, 0, 3u32), (false, 0, 3), (true, 1, 7), (false, 1, 7)];
        let (b, r) = deal_seated(GodLimit::Lobby, Fill::Random, &seated, 5, 20, &mut rng);
        assert_eq!((b.len(), r.len()), (5, 5));
        assert_eq!(b[0], 3, "the host's pick");
        assert_ne!(r[0], 3, "one per match: the second human's duplicate was dealt instead");
        assert_eq!(b[1], 7);
        assert_ne!(r[1], 7);
        let mut all = b.clone();
        all.extend(&r);
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 10, "ten different lines under Lobby");
        let (b, r) = deal_seated(GodLimit::Team, Fill::Random, &seated, 5, 20, &mut rng);
        assert_eq!((b[0], r[0], b[1], r[1]), (3, 3, 7, 7), "one per team: the mirror picks stand");
        let (b, r) = deal_seated(GodLimit::Open, Fill::Mirror, &[(true, 0, 9)], 3, 20, &mut rng);
        assert_eq!((b, r), (vec![9, 9, 9], vec![9, 9, 9]));
        // a seat off the side and a pick off the roster are skipped, not panicked on
        let (b, _) = deal_seated(GodLimit::Open, Fill::Random, &[(true, 9, 1), (true, 0, 99)], 2, 20, &mut rng);
        assert_eq!(b.len(), 2);
    }

    fn count(v: &[u32], g: u32) -> usize {
        v.iter().filter(|&&x| x == g).count()
    }

    /// Replaying a deal seat by seat, every seat was legal when it was
    /// taken: `deal` never fields what `allowed` forbids.
    fn legal(limit: GodLimit, n: usize, b: &[u32], r: &[u32]) -> bool {
        let (mut bb, mut rr) = (vec![b[0]], Vec::new());
        for &g in &b[1..] {
            if !allowed(limit, n, &bb, &rr, true, g) {
                return false;
            }
            bb.push(g);
        }
        for &g in r {
            if !allowed(limit, n, &bb, &rr, false, g) {
                return false;
            }
            rr.push(g);
        }
        true
    }

    /// One per match: on a 5-a-side map, ten different ancestors; on a
    /// 15-a-side map with 20 lines, all 20 fielded, none twice on a side,
    /// so exactly 10 fielded twice (once a side).
    #[test]
    fn one_per_match_is_every_cat_different_or_as_near_as_the_roster_allows() {
        for seed in 0..60 {
            let mut rng = Rng::new(seed);
            let p = (seed % 20) as u32;
            let (b, r) = deal(GodLimit::Lobby, Fill::Random, p, 5, 20, &mut rng);
            assert_eq!((b.len(), r.len(), b[0]), (5, 5, p));
            let mut all: Vec<u32> = b.iter().chain(&r).copied().collect();
            all.sort();
            all.dedup();
            assert_eq!(all.len(), 10, "seed {seed}: a repeat on a 5v5: {b:?} {r:?}");
            let (b, r) = deal(GodLimit::Lobby, Fill::Random, p, 15, 20, &mut rng);
            assert_eq!((b.len(), r.len(), b[0]), (15, 15, p));
            let twice = (0..20).filter(|&g| count(&b, g) + count(&r, g) == 2).count();
            for g in 0..20 {
                assert!(count(&b, g) <= 1 && count(&r, g) <= 1, "seed {seed}: line {g} twice on a side");
                assert!(count(&b, g) + count(&r, g) >= 1, "seed {seed}: line {g} sat out while another fielded twice");
            }
            assert_eq!(twice, 10, "seed {seed}");
            assert!(legal(GodLimit::Lobby, 20, &b, &r));
        }
    }

    /// One per team: no line twice on a side, and the sides do share
    /// lines (it is not one-per-match under another name).
    #[test]
    fn one_per_team_lets_the_sides_share() {
        let mut shared = 0;
        for seed in 0..60 {
            let (b, r) = deal(GodLimit::Team, Fill::Random, 3, 5, 20, &mut Rng::new(seed));
            assert_eq!(b[0], 3);
            for g in 0..20 {
                assert!(count(&b, g) <= 1 && count(&r, g) <= 1, "seed {seed}: {b:?} {r:?}");
            }
            shared += b.iter().any(|g| r.contains(g)) as usize;
            assert!(legal(GodLimit::Team, 20, &b, &r));
        }
        // P(no shared line) for 5 against 5 of 20 is about 19%, so most
        // of 60 deals share one; one-per-match would share in none
        assert!((30..=58).contains(&shared), "{shared} of 60 deals shared a line");
    }

    /// No limit: repeats on a side happen; the mirror fills every seat
    /// with the player's line.
    #[test]
    fn no_limit_repeats_and_the_mirror_fills_with_yours() {
        let mut repeat = 0;
        for seed in 0..40 {
            let (b, r) = deal(GodLimit::Open, Fill::Random, 19, 15, 20, &mut Rng::new(seed));
            assert_eq!((b.len(), r.len(), b[0]), (15, 15, 19));
            repeat += (0..20).any(|g| count(&b, g) > 1 || count(&r, g) > 1) as usize;
        }
        // 15 draws from 20 with replacement repeat almost surely
        assert!(repeat >= 38, "only {repeat} of 40 fielded a line twice on a side");
        let (b, r) = deal(GodLimit::Open, Fill::Mirror, 19, 15, 20, &mut Rng::new(1));
        assert!(b.iter().chain(&r).all(|&g| g == 19) && b.len() == 15 && r.len() == 15);
        // the mirror is a no-limit fill only: under a limit it deals as usual
        let (b, _) = deal(GodLimit::Team, Fill::Mirror, 19, 5, 20, &mut Rng::new(1));
        assert_eq!(count(&b, 19), 1);
    }

    /// The rule seat by seat, as a draft screen would ask it.
    #[test]
    fn allowed_is_the_rule_for_one_seat() {
        let (b, r) = (vec![1, 2], vec![3]);
        // one per match: taken anywhere is taken
        assert!(!allowed(GodLimit::Lobby, 20, &b, &r, true, 3));
        assert!(!allowed(GodLimit::Lobby, 20, &b, &r, false, 1));
        assert!(allowed(GodLimit::Lobby, 20, &b, &r, false, 4));
        // per team: only your own side's
        assert!(allowed(GodLimit::Team, 20, &b, &r, false, 1));
        assert!(!allowed(GodLimit::Team, 20, &b, &r, false, 3));
        // no limit
        assert!(allowed(GodLimit::Open, 20, &b, &r, false, 3));
        // not a line at all
        assert!(!allowed(GodLimit::Open, 20, &b, &r, true, 20));
        // one per match with every line fielded: a repeat, but not on your side
        let (b, r) = (vec![0, 1], vec![2]);
        assert!(allowed(GodLimit::Lobby, 3, &b, &r, false, 0));
        assert!(!allowed(GodLimit::Lobby, 3, &b, &r, true, 0));
    }

    #[test]
    fn keys_round_trip() {
        for l in GodLimit::ALL {
            assert_eq!(GodLimit::from_key(l.key()), Some(l));
        }
        assert_eq!(GodLimit::from_key("NONE"), Some(GodLimit::Open));
        assert_eq!(GodLimit::from_key("sometimes"), None);
        assert_eq!(GodLimit::default(), GodLimit::Lobby);
    }
}
