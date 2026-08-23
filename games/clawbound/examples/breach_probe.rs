//! Why is breaking the enemy's first border a *liability*? The balance harness
//! says the clan that fells the first Heal Tree wins 43% of the time. This asks
//! what happens in the minute afterwards: who dies, and how deep.
//!
//! Run: cargo run --release --example breach_probe [matches]

use clawbound_rs::ancestor::{self, Ancestor};
use clawbound_rs::battleground::{Battleground, Mind, Role};

fn main() {
    let n: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(30);
    let pool = ancestor::ALL;
    let mut rng: u64 = 0xa1b2_c3d4_e5f6_0718;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    // deaths in the 60s after the first border falls, and how far from the
    // dying cat's OWN camp it happened — depth is the overextension signal
    let (mut br_deaths, mut df_deaths) = (0u32, 0u32);
    let (mut br_depth, mut df_depth) = (0.0f64, 0.0f64);
    let (mut br_wins, mut matches) = (0u32, 0u32);
    let (mut pile_lead, mut breach_time) = (0.0f64, 0.0f64);
    let mut ahead_at_break = 0u32;
    let (mut end_pile_lead, mut end_trees_lead) = (0.0f64, 0.0f64);

    for m in 0..n {
        let mut bag: Vec<usize> = (0..pool.len()).collect();
        let mut picks = [Ancestor::Lion; 6];
        for slot in picks.iter_mut() {
            *slot = pool[bag.swap_remove((next() as usize) % bag.len())];
        }
        let spoils = std::env::args().any(|s| s == "spoils");
        let mut bg = Battleground::new_on(
            [picks[0], picks[1], picks[2]],
            [picks[3], picks[4], picks[5]],
            m + 1,
        );
        bg.spoils = spoils;
        let mut breacher: Option<u8> = None;
        let mut breach_at = 0.0f32;
        let mut alive: Vec<bool> = bg.warriors.iter().map(|w| w.alive()).collect();
        let mut steps = 0;
        while bg.outcome.is_none() && steps < 30 * 60 * 13 {
            bg.step(1.0 / 30.0);
            steps += 1;
            if breacher.is_none() {
                if let Some(t) = bg.heal_trees.iter().find(|t| !t.alive()) {
                    let b = 1 - t.team;
                    breacher = Some(b);
                    breach_at = bg.time;
                    // the state at the moment of the break: is the clan that
                    // goes gnawing the one that was already winning, or the one
                    // that could not farm?
                    let (bp, dp) = (
                        bg.clans[b as usize].pile,
                        bg.clans[t.team as usize].pile,
                    );
                    pile_lead += (bp - dp) as f64;
                    if bp > dp { ahead_at_break += 1; }
                    breach_time += bg.time as f64;
                }
            }
            // count ancestor deaths in the window after the break
            if let Some(b) = breacher {
                if bg.time - breach_at <= 60.0 {
                    alive.resize(bg.warriors.len(), true);
                    for (i, w) in bg.warriors.iter().enumerate() {
                        if w.role != Role::Ancestor {
                            continue;
                        }
                        let now = w.mind != Mind::Dead;
                        if alive[i] && !now {
                            let home = bg.camps[w.team as usize];
                            let depth =
                                ((w.x - home.0).powi(2) + (w.z - home.1).powi(2)).sqrt() as f64;
                            if w.team == b {
                                br_deaths += 1;
                                br_depth += depth;
                            } else {
                                df_deaths += 1;
                                df_depth += depth;
                            }
                        }
                        alive[i] = now;
                    }
                }
            }
        }
        if let (Some(b), Some(w)) = (breacher, bg.outcome) {
            matches += 1;
            if w == b {
                br_wins += 1;
            }
            let d = 1 - b;
            end_pile_lead += (bg.clans[b as usize].pile - bg.clans[d as usize].pile) as f64;
            let standing = |t: u8| {
                bg.heal_trees.iter().filter(|h| h.team == t && h.alive()).count() as f64
            };
            end_trees_lead += standing(b) - standing(d);
        }
    }
    eprintln!("{matches} matches with a first border broken; the breacher won {br_wins} ({:.0}%)",
        br_wins as f64 / matches.max(1) as f64 * 100.0);
    eprintln!(
        "  in the 60s after: breacher lost {br_deaths} ancestors (mean {:.0}m from their own camp)",
        br_depth / br_deaths.max(1) as f64
    );
    eprintln!(
        "                    defender lost {df_deaths} ancestors (mean {:.0}m from their own camp)",
        df_depth / df_deaths.max(1) as f64
    );
    eprintln!(
        "  at the moment of the break (mean {:.0}s in): the breacher's freshkill lead was {:+.1}, and it was ahead in {ahead_at_break}/{matches}",
        breach_time / matches.max(1) as f64,
        pile_lead / matches.max(1) as f64
    );
    eprintln!(
        "  at the end: breacher's freshkill lead {:+.1}, borders-standing lead {:+.2}",
        end_pile_lead / matches.max(1) as f64,
        end_trees_lead / matches.max(1) as f64
    );
}
