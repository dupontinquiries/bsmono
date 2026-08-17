//! Run headless Battleground matches and report how each one unfolded — the same
//! visible-loop idea as sim_balance.rs. One match per player ancestor:
//! who won, how long it took, what each clan ate, gathered, and lost.

use clawbound_rs::ancestor;
use clawbound_rs::battleground::{Battleground, CLAN_A};

fn main() {
    for a in ancestor::ALL {
        let mut bg = Battleground::new(a);
        let mut steps = 0u32;
        let mut first_tree: Option<(f32, u8)> = None;
        while bg.outcome.is_none() && steps < 30 * 60 * 9 {
            bg.step(1.0 / 30.0);
            steps += 1;
            if first_tree.is_none() {
                if let Some(t) = bg.heal_trees.iter().find(|t| !t.alive()) {
                    first_tree = Some((bg.time, t.team));
                }
            }
        }
        let win = match bg.outcome {
            Some(t) if t == CLAN_A => "CLAN A",
            Some(2) => "DRAW",
            Some(_) => "CLAN B",
            None => "timeout",
        };
        let ft = first_tree
            .map(|(t, team)| format!("first Heal Tree fell at {:>3.0}s (clan {})", t, if team == 0 { "A" } else { "B" }))
            .unwrap_or_else(|| "no Heal Tree fell".into());
        eprintln!(
            "{:<12} {:>6}  in {:>3.0}s   piles A {:>5.1} / B {:>5.1}   herbs A {} / B {}   {}",
            ancestor::kit(a).name, win, bg.time,
            bg.clans[0].pile, bg.clans[1].pile,
            bg.clans[0].herbs, bg.clans[1].herbs, ft,
        );
    }
}
