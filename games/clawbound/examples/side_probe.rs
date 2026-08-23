//! Is a *side* favoured? Mirror matches only: both clans field the identical
//! draft on terrain folded onto its own reflection, so every asymmetry left is
//! in the simulation rather than the map or the picks. Anything away from 50%
//! here is a bug with a win rate attached.
//!
//! Run: cargo run --release --example side_probe [matches]

use clawbound_rs::ancestor::{self, Ancestor};
use clawbound_rs::battleground::{Battleground, CLAN_A};

fn main() {
    let n: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(40);
    let pool = ancestor::ALL;
    let mut rng: u64 = 0x51ed_2701_9c3f_a1b7;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let (mut a_wins, mut b_wins, mut draws) = (0u32, 0u32, 0u32);
    let mut tree_lead = 0.0f64;
    let mut first_kill_a = 0u32;
    let mut simultaneous = 0u32;
    let mut decided = 0u32;

    for m in 0..n {
        // a fresh draft each match, mirrored onto both clans
        let mut bag: Vec<usize> = (0..pool.len()).collect();
        let mut picks = [Ancestor::Lion; 3];
        for slot in picks.iter_mut() {
            *slot = pool[bag.swap_remove((next() as usize) % bag.len())];
        }
        let mut bg = Battleground::new_on(picks, picks, 0x9e37_79b9u32.wrapping_mul(m + 1));
        // who draws first blood, since first contact is the one break in the
        // mirror the sim cannot avoid — if THAT is skewed, the tick order is
        let mut first: Option<u8> = None;
        let mut steps = 0;
        while bg.outcome.is_none() && steps < 30 * 60 * 13 {
            bg.step(1.0 / 30.0);
            steps += 1;
            if first.is_none() {
                // Count both clans' wounds in the FIRST tick any appear. Scanning
                // for "the first wounded warrior" instead would have reported a
                // 78/22 skew that is pure artefact: clan A holds the low indices,
                // so it wins every simultaneous tie in the iteration order.
                let hurt_a = bg.warriors.iter().filter(|w| w.team == CLAN_A && w.hp < w.max_hp).count();
                let hurt_b = bg.warriors.iter().filter(|w| w.team != CLAN_A && w.hp < w.max_hp).count();
                if hurt_a + hurt_b > 0 {
                    first = match hurt_a.cmp(&hurt_b) {
                        std::cmp::Ordering::Less => Some(CLAN_A),   // B bled more: A struck
                        std::cmp::Ordering::Greater => Some(1),
                        std::cmp::Ordering::Equal => None.or(Some(2)),
                    };
                }
            }
        }
        match first {
            Some(t) if t == CLAN_A => first_kill_a += 1,
            Some(2) => simultaneous += 1,
            _ => {}
        }
        tree_lead += (bg.clans[0].great_tree_hp - bg.clans[1].great_tree_hp) as f64;
        match bg.outcome {
            Some(t) if t == CLAN_A => {
                a_wins += 1;
                decided += 1;
            }
            Some(2) | None => draws += 1,
            Some(_) => {
                b_wins += 1;
                decided += 1;
            }
        }
    }
    let se = (0.25f64 / decided.max(1) as f64).sqrt() * 100.0;
    eprintln!("{n} mirror matches (identical drafts both sides, folded terrain)");
    eprintln!(
        "  clan A {a_wins} / clan B {b_wins} / undecided {draws}   ->  A {:.0}% of decided (se {:.1}%)",
        a_wins as f64 / decided.max(1) as f64 * 100.0,
        se
    );
    eprintln!(
        "  mean Great Tree lead for A: {:+.0} hp   first blood: A {first_kill_a}, simultaneous {simultaneous}, B {}",
        tree_lead / n as f64,
        n - first_kill_a - simultaneous
    );
}
