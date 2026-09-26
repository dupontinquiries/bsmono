//! arena -- a league where the playbook's pages play each other headless,
//! are rated, and breed.
//!
//! The shape follows AlphaStar's league, cut down to what a laptop runs:
//!
//! * The **main pages** (the playbook's old ways) are frozen: never
//!   changed, never removed. They are the fixed opposition every newcomer
//!   is measured against, so the league cannot drift into a private
//!   rock-paper-scissors nobody else plays.
//! * **Strains** are mutations of whoever leads the table. A strain that
//!   stays at the bottom after its trial games is dropped.
//! * **Matchmaking is prioritised fictitious self-play**: an entry is
//!   paired mostly against the opponents it is expected to lose to (weight
//!   (1 - p_win)^2), so the league spends its matches on its weaknesses.
//! * **Ratings are Elo** on a graded result: a titan kill is a full win,
//!   a match that reaches the time cap is scored from titan health,
//!   towers and kills.
//!
//! Every pairing is played twice with the sides AND the rosters swapped,
//! so the map's and the draw's asymmetries cancel.
//!
//! [`League`] takes the match as a function, so its bookkeeping is tested
//! with a toy match in milliseconds; [`play`] is the real one, a full
//! simulation with every cat a bot. `cargo run --release --example arena`
//! runs a league and can write its best strains to `dev/arena/booklet.txt`,
//! where the playbook reads them as league pages.

use crate::consts::{Lane, Team};
use crate::playbook::Page;
use crate::sim::{AiBrain, Game, Rng};

/// One match: two pages, two rosters, a seed, a time cap (sim seconds).
#[derive(Clone, Debug)]
pub struct MatchSpec {
    pub blue: Page,
    pub red: Page,
    pub blue_picks: Vec<u32>,
    pub red_picks: Vec<u32>,
    pub seed: u64,
    pub max_t: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outcome {
    /// Blue's result in 0..=1: 1 a titan kill, 0 a loss, between for a
    /// match that hit the cap.
    pub blue: f32,
    pub winner: Option<Team>,
    pub t: f32,
    pub kills: [u32; 2],
    pub towers: [usize; 2],
}

/// Blue's graded result for a match that ended at the cap: titan health
/// counts half, towers standing 30%, kills 20%, squashed into 0..1.
pub fn grade(titan_frac: [f32; 2], towers: [usize; 2], kills: [u32; 2]) -> f32 {
    let titan = titan_frac[0] - titan_frac[1];
    let tw = (towers[0] as f32 - towers[1] as f32) / 6.0;
    let k = (kills[0] as f32 - kills[1] as f32) / (kills[0] + kills[1] + 5) as f32;
    let s = 0.5 * titan + 0.3 * tw + 0.2 * k;
    0.5 + 0.5 * (3.0 * s).tanh()
}

/// Play one match to the end or the cap, every cat a bot.
pub fn play(spec: &MatchSpec) -> Outcome {
    let mut game = Game::new_teams(&spec.blue_picks, &spec.red_picks);
    game.rng = Rng::new(0xA7E4A ^ spec.seed.wrapping_mul(0x9E37_79B9));
    // god 0 is "the player" to Game; here it is a bot like the rest
    game.gods[0].is_player = false;
    game.gods[0].ai = Some(AiBrain::new(Lane::Mid, 0.7));
    game.auto_buy(0);
    let mut rng = Rng::new(spec.seed ^ 0x51DE);
    game.set_styles([spec.blue.clone(), spec.red.clone()], &mut rng);
    let dt = 1.0 / 60.0;
    while game.over.is_none() && game.t < spec.max_t {
        game.tick(dt);
        game.events.clear();
    }
    let kills = [Team::Blue, Team::Red].map(|t| game.gods.iter().filter(|g| g.team == t).map(|g| g.kills).sum::<u32>());
    let towers = game.tower_alive;
    let blue = match game.over {
        Some(Team::Blue) => 1.0,
        Some(Team::Red) => 0.0,
        None => grade([game.titans[0].hp / crate::consts::TITAN_HP, game.titans[1].hp / crate::consts::TITAN_HP], towers, kills),
    };
    Outcome { blue, winner: game.over, t: game.t, kills, towers }
}

/// Elo expectation of a over b.
pub fn expected(a: f32, b: f32) -> f32 {
    1.0 / (1.0 + 10f32.powf((b - a) / 400.0))
}

pub const K: f32 = 24.0;
pub const START_ELO: f32 = 1200.0;

#[derive(Clone, Debug)]
pub struct Entry {
    pub page: Page,
    pub elo: f32,
    pub games: u32,
    /// Sum of graded results, for the table.
    pub points: f32,
    /// A main page: never mutated, never dropped.
    pub frozen: bool,
    pub generation: u32,
}

/// A league. `play` in [`League::round`] is the match: it gets (blue page,
/// red page, seed) and returns blue's graded result.
pub struct League {
    pub entries: Vec<Entry>,
    pub rng: Rng,
    pub generation: u32,
    /// Most strains alive at once.
    pub pool: usize,
    /// Games before a strain can be dropped.
    pub trial: u32,
    next_seed: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pairing {
    pub a: usize,
    pub b: usize,
    pub seed: u64,
}

/// A strain's root: its page name without the strain suffix.
pub fn root_name(name: &str) -> &str {
    name.split(" strain ").next().unwrap_or(name)
}

impl League {
    pub fn new(mains: Vec<Page>, seed: u64) -> League {
        League {
            entries: mains
                .into_iter()
                .map(|page| Entry { page, elo: START_ELO, games: 0, points: 0.0, frozen: true, generation: 0 })
                .collect(),
            rng: Rng::new(seed),
            generation: 0,
            pool: 6,
            trial: 8,
            next_seed: seed.wrapping_mul(31) + 7,
        }
    }

    /// Prioritised fictitious self-play: an opponent for `i`, weighted by
    /// (1 - p_win)^2 plus a floor so nobody is never played.
    pub fn pick_opponent(&mut self, i: usize) -> usize {
        let me = self.entries[i].elo;
        let w: Vec<f32> = (0..self.entries.len())
            .map(|j| if j == i { 0.0 } else { (1.0 - expected(me, self.entries[j].elo)).powi(2) + 0.05 })
            .collect();
        let total: f32 = w.iter().sum();
        let mut x = self.rng.f32() * total;
        for (j, wj) in w.iter().enumerate() {
            if x < *wj {
                return j;
            }
            x -= wj;
        }
        (i + 1) % self.entries.len()
    }

    /// Record a graded result (0..1, `a`'s view) between entries a and b.
    pub fn record(&mut self, a: usize, b: usize, score: f32) {
        let ea = expected(self.entries[a].elo, self.entries[b].elo);
        let delta = K * (score - ea);
        self.entries[a].elo += delta;
        self.entries[b].elo -= delta;
        self.entries[a].games += 1;
        self.entries[b].games += 1;
        self.entries[a].points += score;
        self.entries[b].points += 1.0 - score;
    }

    /// A round's pairings: every entry against a PFSP pick, each with a
    /// fresh seed. Planning and playing are split so the matches can run
    /// in parallel while the bookkeeping stays in order.
    pub fn plan(&mut self) -> Vec<Pairing> {
        (0..self.entries.len())
            .map(|a| {
                let b = self.pick_opponent(a);
                let seed = self.next_seed;
                self.next_seed = self.next_seed.wrapping_add(1);
                Pairing { a, b, seed }
            })
            .collect()
    }

    /// Record a pairing's two games: `first` with a as blue, `second` with
    /// b as blue (each blue's result, as `play` returns it).
    pub fn record_pairing(&mut self, p: Pairing, first: f32, second: f32) -> f32 {
        let (s1, s2) = (first, 1.0 - second);
        self.record(p.a, p.b, s1);
        self.record(p.a, p.b, s2);
        (s1 + s2) * 0.5
    }

    /// Plan, play (both sides of each pairing, same seed) and record one
    /// round. Returns (a, b, a's mean result) per pairing.
    pub fn round(&mut self, play: &mut dyn FnMut(&Page, &Page, u64) -> f32) -> Vec<(usize, usize, f32)> {
        let plan = self.plan();
        let mut log = Vec::new();
        for p in plan {
            let (pa, pb) = (self.entries[p.a].page.clone(), self.entries[p.b].page.clone());
            let first = play(&pa, &pb, p.seed);
            let second = play(&pb, &pa, p.seed);
            log.push((p.a, p.b, self.record_pairing(p, first, second)));
        }
        log
    }

    /// One generation: drop strains that finished their trial at the
    /// bottom, then breed from the leaders until the pool is full.
    pub fn evolve(&mut self, amount: f32) -> Vec<String> {
        self.generation += 1;
        let mut news = Vec::new();
        // drop: strains past their trial, below every main page's median
        let mut mains: Vec<f32> = self.entries.iter().filter(|e| e.frozen).map(|e| e.elo).collect();
        mains.sort_by(|a, b| a.total_cmp(b));
        let median = mains.get(mains.len() / 2).copied().unwrap_or(START_ELO);
        let before = self.entries.len();
        self.entries.retain(|e| {
            let keep = e.frozen || e.games < self.trial || e.elo >= median;
            if !keep {
                news.push(format!("dropped {} ({:.0})", e.page.name, e.elo));
            }
            keep
        });
        let _ = before;
        // breed from the top two
        let mut order: Vec<usize> = (0..self.entries.len()).collect();
        order.sort_by(|&a, &b| self.entries[b].elo.total_cmp(&self.entries[a].elo));
        let mut k = 0;
        while self.entries.iter().filter(|e| !e.frozen).count() < self.pool && !order.is_empty() {
            let parent = self.entries[order[k % order.len().min(2)]].clone();
            let root = root_name(&parent.page.name).to_string();
            let n = self.entries.iter().filter(|e| root_name(&e.page.name) == root).count();
            let genes = parent.page.genes.mutate(&mut self.rng, amount);
            let page = Page { name: format!("{root} strain {}.{}", self.generation, n), lore: format!("Bred in the arena from {root}."), genes };
            news.push(format!("bred {} from {} ({:.0})", page.name, parent.page.name, parent.elo));
            self.entries.push(Entry { page, elo: parent.elo, games: 0, points: 0.0, frozen: false, generation: self.generation });
            k += 1;
        }
        news
    }

    /// Strains worth a page in the booklet: past their trial and rated
    /// above every main page's median, best first, at most `n`.
    pub fn champions(&self, n: usize) -> Vec<&Entry> {
        let mut mains: Vec<f32> = self.entries.iter().filter(|e| e.frozen).map(|e| e.elo).collect();
        mains.sort_by(|a, b| a.total_cmp(b));
        let median = mains.get(mains.len() / 2).copied().unwrap_or(START_ELO);
        let mut v: Vec<&Entry> = self.entries.iter().filter(|e| !e.frozen && e.games >= self.trial && e.elo > median).collect();
        v.sort_by(|a, b| b.elo.total_cmp(&a.elo));
        v.truncate(n);
        v
    }

    /// The table, best first.
    pub fn table(&self) -> String {
        let mut order: Vec<&Entry> = self.entries.iter().collect();
        order.sort_by(|a, b| b.elo.total_cmp(&a.elo));
        let mut s = format!("generation {}\n  {:<28} {:>6} {:>5} {:>6}\n", self.generation, "page", "elo", "games", "score");
        for e in order {
            let pct = if e.games > 0 { e.points / e.games as f32 * 100.0 } else { 0.0 };
            s += &format!("  {:<28} {:>6.0} {:>5} {:>5.0}%{}\n", e.page.name, e.elo, e.games, pct, if e.frozen { "" } else { "  *" });
        }
        s
    }
}

/// The real match for a league: two rosters drawn from `seed`, the first
/// for blue. `League::round` plays each pairing twice with the pages
/// swapped and the same seed, so each page plays both sides with both
/// rosters.
pub fn real_match(size: usize, max_t: f32) -> impl FnMut(&Page, &Page, u64) -> f32 {
    move |blue: &Page, red: &Page, seed: u64| {
        let (a, b) = rosters(size, seed);
        play(&MatchSpec { blue: blue.clone(), red: red.clone(), blue_picks: a, red_picks: b, seed, max_t }).blue
    }
}

/// Two rosters of `size` from the seed, no repeats within a side.
pub fn rosters(size: usize, seed: u64) -> (Vec<u32>, Vec<u32>) {
    let mut rng = Rng::new(seed ^ 0x2057E2);
    let n = crate::gods::ROSTER_LEN as u32;
    let mut pool: Vec<u32> = (0..n).collect();
    let mut side = |rng: &mut Rng| -> Vec<u32> {
        (0..size)
            .map(|_| {
                if pool.is_empty() {
                    pool = (0..n).collect();
                }
                pool.remove((rng.next_u() % pool.len() as u64) as usize)
            })
            .collect()
    };
    let a = side(&mut rng);
    let b = side(&mut rng);
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tactics::Genes;

    fn toy_pages() -> Vec<Page> {
        // a toy world where a lower commit bar simply wins
        [1.8, 1.4, 1.0]
            .iter()
            .enumerate()
            .map(|(i, &c)| Page { name: format!("P{i}"), lore: String::new(), genes: Genes { commit: c, ..Genes::default() } })
            .collect()
    }

    fn toy(a: &Page, b: &Page, _seed: u64) -> f32 {
        if a.genes.commit < b.genes.commit - 0.05 {
            0.9
        } else if a.genes.commit > b.genes.commit + 0.05 {
            0.1
        } else {
            0.5
        }
    }

    #[test]
    fn the_grade_is_graded_and_symmetric() {
        assert!((grade([1.0, 1.0], [6, 6], [3, 3]) - 0.5).abs() < 1e-5);
        let up = grade([1.0, 0.4], [6, 3], [10, 2]);
        let down = grade([0.4, 1.0], [3, 6], [2, 10]);
        assert!(up > 0.85 && (up + down - 1.0).abs() < 1e-5, "{up} {down}");
        let small = grade([1.0, 1.0], [6, 5], [4, 3]);
        assert!(small > 0.52 && small < 0.75, "a small lead is a small result: {small}");
    }

    #[test]
    fn the_league_ranks_the_toy_world_correctly() {
        let mut lg = League::new(toy_pages(), 3);
        for _ in 0..30 {
            lg.round(&mut toy);
        }
        let elo = |n: &str| lg.entries.iter().find(|e| e.page.name == n).unwrap().elo;
        assert!(elo("P2") > elo("P1") && elo("P1") > elo("P0"), "{}", lg.table());
        let total: f32 = lg.entries.iter().map(|e| e.elo).sum();
        assert!((total - START_ELO * 3.0).abs() < 1e-2, "Elo is zero-sum");
    }

    #[test]
    fn pfsp_prefers_the_opponents_you_lose_to() {
        let mut lg = League::new(toy_pages(), 4);
        lg.entries[0].elo = 1000.0;
        lg.entries[1].elo = 1200.0;
        lg.entries[2].elo = 1600.0;
        let mut hits = [0; 3];
        for _ in 0..2000 {
            hits[lg.pick_opponent(0)] += 1;
        }
        assert_eq!(hits[0], 0, "never itself");
        // weights (1-p)^2 + 0.05: 0.99 against 1600, 0.63 against 1200
        let ratio = hits[2] as f32 / hits[1] as f32;
        assert!((1.35..1.85).contains(&ratio), "the stronger opponent about 1.6x as often: {hits:?}");
        assert!(hits[1] > 100, "but not only the strongest: {hits:?}");
    }

    #[test]
    fn evolution_breeds_strains_keeps_mains_and_drops_losers() {
        let mut lg = League::new(toy_pages(), 5);
        lg.pool = 3;
        lg.trial = 4;
        let news = lg.evolve(0.3);
        assert_eq!(lg.entries.iter().filter(|e| !e.frozen).count(), 3, "{news:?}");
        assert!(lg.entries.iter().all(|e| e.frozen || e.page.name.contains(" strain ")));
        for _ in 0..40 {
            lg.round(&mut toy);
            lg.evolve(0.3);
        }
        assert_eq!(lg.entries.iter().filter(|e| e.frozen).count(), 3, "main pages are never dropped");
        // in the toy world a lower commit wins, so the league should have
        // bred toward it
        let best = lg.entries.iter().filter(|e| !e.frozen).map(|e| e.page.genes.commit).fold(f32::MAX, f32::min);
        assert!(best < 1.0, "the league bred toward what wins: best commit {best}\n{}", lg.table());
        for c in lg.champions(3) {
            assert!(!c.frozen && c.games >= lg.trial);
        }
    }

    #[test]
    fn rosters_are_full_and_differ_by_seed() {
        let (a, b) = rosters(5, 1);
        assert_eq!((a.len(), b.len()), (5, 5));
        let mut all: Vec<u32> = a.iter().chain(&b).copied().collect();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 10);
        assert_ne!(rosters(5, 1), rosters(5, 2));
    }
}
