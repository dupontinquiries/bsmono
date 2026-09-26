//! playbook -- the booklet of team styles a match draws from.
//!
//! Each side opens the booklet at the start of a match and plays one page:
//! a named temperament ([`Genes`]) with a line of lore. The pages written
//! by hand are the clans' old ways; the arena (`arena`) adds pages of its
//! own -- strains bred from those ways that won their league -- read from
//! `dev/arena/booklet.txt` at build time. A missing or empty file is just
//! a booklet with no league pages.
//!
//! A page also decides how the team spreads over the lanes (`group`):
//! a pride that moves as one sends most of its cats down one lane, a
//! split-push style keeps them apart.

use crate::consts::Lane;
use crate::sim::Rng;
use crate::tactics::{Genes, GENE_COUNT};

#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub name: String,
    pub lore: String,
    pub genes: Genes,
}

fn page(name: &str, lore: &str, genes: Genes) -> Page {
    Page { name: name.to_string(), lore: lore.to_string(), genes }
}

/// The page a match plays before anyone opens the booklet (tests, tools):
/// the default genes, the old brain's temper without its circling.
pub fn even_keel() -> Page {
    page("Even Keel", "Trade blows, take the openings, give ground when outnumbered.", Genes::default())
}

/// The clans' old ways, written by hand.
pub fn old_ways() -> Vec<Page> {
    let d = Genes::default();
    vec![
        page(
            "Pride Hunt",
            "Move as one, pick out the weakest, and all bite at once.",
            Genes { commit: 1.15, give: 0.7, leash: 18.0, focus: 0.9, group: 0.9, rally: 1.0, reach: 0.9, hold: 2.5, ..d },
        ),
        page(
            "Shadow Stalk",
            "Roam alone, strike the stray, be gone before help comes.",
            Genes { commit: 1.5, give: 0.85, leash: 12.0, strafe: 1.1, focus: 1.0, group: 0.1, rally: 0.3, hold: 1.2, ..d },
        ),
        page(
            "Thornwall",
            "Hold the standing stones. Let them come onto the thorns.",
            Genes { commit: 1.35, give: 0.75, leash: 8.0, tower: 1.4, reach: 1.2, objective: 0.3, retreat: 0.3, ..d },
        ),
        page(
            "Storm Front",
            "Bleed them from afar; never let them close the distance.",
            Genes { commit: 1.3, give: 0.7, reach: 1.5, strafe: 1.2, focus: 0.5, leash: 14.0, ..d },
        ),
        page(
            "Rabble Rush",
            "All claws in. Fear is for later.",
            Genes { commit: 0.9, give: 0.45, retreat: 0.14, leash: 26.0, tower: 0.5, reach: 0.6, hold: 3.5, strafe: 0.5, ..d },
        ),
        page(
            "Den Mothers",
            "Nobody is left behind: the fight is fought around the weakest of us.",
            Genes { commit: 1.2, give: 0.8, retreat: 0.35, leash: 10.0, focus: 0.3, group: 0.8, rally: 0.7, ..d },
        ),
        page(
            "Split Tails",
            "Every trail at once; when they gather in one place, the stones fall in another.",
            Genes { commit: 1.45, give: 0.85, leash: 10.0, group: 0.0, rally: 0.2, objective: 0.9, ..d },
        ),
        page(
            "Moonwatch",
            "Take the beasts of the wild first, and grow, and then fight.",
            Genes { commit: 1.2, objective: 1.0, group: 0.6, rally: 0.6, ..d },
        ),
    ]
}

/// The league pages bred by the arena, as committed.
pub const LEAGUE_BOOKLET: &str = include_str!("../dev/arena/booklet.txt");

/// One booklet line: `name|lore|g0 g1 ... g11`. Lines starting with `#`
/// and blank lines are skipped by [`parse_booklet`].
pub fn page_line(p: &Page) -> String {
    let genes: Vec<String> = p.genes.to_array().iter().map(|v| format!("{v:.3}")).collect();
    format!("{}|{}|{}", p.name.replace('|', "/"), p.lore.replace('|', "/"), genes.join(" "))
}

pub fn parse_line(line: &str) -> Option<Page> {
    let mut parts = line.splitn(3, '|');
    let name = parts.next()?.trim();
    let lore = parts.next()?.trim();
    let nums: Vec<f32> = parts.next()?.split_whitespace().filter_map(|s| s.parse().ok()).collect();
    if name.is_empty() || nums.len() != GENE_COUNT {
        return None;
    }
    let mut a = [0.0; GENE_COUNT];
    a.copy_from_slice(&nums);
    Some(Page { name: name.to_string(), lore: lore.to_string(), genes: Genes::from_array(a) })
}

pub fn parse_booklet(text: &str) -> Vec<Page> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).filter_map(parse_line).collect()
}

/// Every page: the old ways, then the league's.
pub fn booklet() -> Vec<Page> {
    let mut v = old_ways();
    v.extend(parse_booklet(LEAGUE_BOOKLET));
    v
}

/// Two different pages for the two sides.
pub fn draw_pair(rng: &mut Rng) -> [Page; 2] {
    let b = booklet();
    let i = (rng.next_u() % b.len() as u64) as usize;
    let mut j = (rng.next_u() % (b.len() as u64 - 1)) as usize;
    if j >= i {
        j += 1;
    }
    [b[i].clone(), b[j].clone()]
}

/// The lanes a side's bots play under a page. `lanes` is the default
/// assignment; with probability `group` x 0.8 a bot joins the middle
/// lane, and a page with almost no `group` spreads the bots evenly.
pub fn lanes_for(genes: &Genes, lanes: &[Lane], rng: &mut Rng) -> Vec<Lane> {
    // the map's own lanes: two on the sky, where the "main" lane is the west
    let spread = crate::map::lanes();
    let main = spread[(spread.len() - 1) / 2];
    lanes
        .iter()
        .enumerate()
        .map(|(k, &l)| {
            if rng.f32() < genes.group * 0.8 {
                main
            } else if genes.group < 0.2 {
                spread[k % spread.len()]
            } else {
                l
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_booklet_has_the_old_ways_distinct_and_named() {
        let b = old_ways();
        assert!(b.len() >= 8);
        for (i, p) in b.iter().enumerate() {
            assert!(!p.name.is_empty() && !p.lore.is_empty());
            assert_eq!(p.genes, Genes::from_array(p.genes.to_array()), "{} is inside the gene ranges", p.name);
            for q in &b[i + 1..] {
                assert!(p.genes.distance(&q.genes) > 0.3, "{} and {} play alike", p.name, q.name);
            }
        }
    }

    #[test]
    fn a_page_round_trips_through_a_booklet_line() {
        for p in old_ways() {
            let q = parse_line(&page_line(&p)).expect("parses");
            assert_eq!(q.name, p.name);
            assert!(q.genes.distance(&p.genes) < 1e-3);
        }
        assert!(parse_line("half|a line").is_none());
        assert!(parse_line("x|y|1 2 3").is_none(), "wrong gene count");
        assert_eq!(parse_booklet("# a comment\n\n").len(), 0);
    }

    #[test]
    fn the_committed_league_pages_parse() {
        let n = LEAGUE_BOOKLET.lines().filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#')).count();
        assert_eq!(parse_booklet(LEAGUE_BOOKLET).len(), n, "every non-comment line of dev/arena/booklet.txt is a page");
    }

    #[test]
    fn draws_differ_between_sides_and_across_matches() {
        let mut rng = Rng::new(77);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let [a, b] = draw_pair(&mut rng);
            assert_ne!(a.name, b.name);
            seen.insert(a.name.clone());
        }
        assert!(seen.len() >= old_ways().len() * 3 / 4, "the whole booklet gets played: {}", seen.len());
    }

    #[test]
    fn grouping_pulls_lanes_together_and_splitting_spreads_them() {
        let lanes = [Lane::Top, Lane::Mid, Lane::Bot, Lane::Mid, Lane::Bot];
        let mut rng = Rng::new(5);
        let mut main = 0;
        for _ in 0..100 {
            let v = lanes_for(&Genes { group: 1.0, ..Genes::default() }, &lanes, &mut rng);
            main += v.iter().filter(|&&l| l == Lane::Mid).count();
        }
        assert!(main > 350, "a pride mostly shares one lane: {main}/500");
        let v = lanes_for(&Genes { group: 0.0, ..Genes::default() }, &lanes, &mut rng);
        assert_eq!(v, vec![Lane::Top, Lane::Mid, Lane::Bot, Lane::Top, Lane::Mid]);
    }
}
