//! Run headless Battleground matches and report how each one unfolded — the same
//! visible-loop idea as sim_balance.rs. One match per player ancestor:
//! who won, how long it took, what each clan ate, gathered, and lost.
//!
//! Also prints a one-time map-fairness readout (`mapeval::lane_water_report`)
//! before the matches: per lane, what fraction of each clan's home-side half
//! is water, and how far apart the two sides are — the TODO.md item about
//! grading whether the river basin favors one camp's approach over the
//! other's, same map every match so once is enough.

use clawbound_rs::ancestor;
use clawbound_rs::battleground::{Battleground, CLAN_A, TRAIL_BEND};
use clawbound_rs::mapeval;

fn main() {
    // Map fairness once, up front: which lane (if any) hands one camp a
    // wetter approach than the other. `bg.camps` is the actual land-adjusted
    // camp position (see `land_near` in battleground.rs), not the raw
    // CAMP_ANCHOR — the report should grade the map that's actually played,
    // not the anchor the search started from.
    {
        let bg = Battleground::new(ancestor::ALL[0]);
        let report = mapeval::lane_water_report(bg.camps[0], bg.camps[1], &TRAIL_BEND, 32);
        eprintln!("MAP FAIRNESS (per lane, home-side half water fraction):");
        for r in &report {
            eprintln!(
                "  lane {}   water A {:>4.0}%  water B {:>4.0}%   asymmetry {:>4.0}pp{}",
                r.lane, r.water_a * 100.0, r.water_b * 100.0, r.asymmetry * 100.0,
                if r.asymmetry > 0.15 { "  <- lopsided" } else { "" },
            );
        }
    }

    for a in ancestor::ALL {
        let mut bg = Battleground::new(a);
        let mut steps = 0u32;
        let mut first_tree: Option<(f32, u8)> = None;
        while bg.outcome.is_none() && steps < 30 * 60 * 13 {
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
        let down = |t: u8| bg.heal_trees.iter().filter(|h| h.team == t && !h.alive()).count();
        eprintln!(
            "{:<12} {:>6}  in {:>3.0}s   piles A {:>5.1} / B {:>5.1}   herbs A {} / B {}   trees down A {} / B {}   great A {:>4.0} / B {:>4.0}   {}",
            ancestor::kit(a).name, win, bg.time,
            bg.clans[0].pile, bg.clans[1].pile,
            bg.clans[0].herbs, bg.clans[1].herbs,
            down(0), down(1),
            bg.clans[0].great_tree_hp.max(0.0), bg.clans[1].great_tree_hp.max(0.0), ft,
        );
    }
}
