//! Balance across MAPS, not one map. `battleground_report` plays each ancestor
//! once on the default country, which is thirteen deterministic samples of a
//! single terrain roll — and a 9-4 split in that harness is inside coin-flip
//! variance (P(>=9 of 13) is about 13%), so it can neither confirm nor dismiss
//! a real bias. This one re-rolls the land per run and plays a mirrored pair on
//! every roll, which separates the two questions that matter:
//!
//! * **Is a side favoured?** Clan A's win rate across all games. The map is
//!   symmetric by construction (half-turn for the jungle, anti-diagonal for the
//!   river and the slope), so anything far from 50% is a bug in that claim.
//! * **Is a kit favoured?** Every ancestor's record across the games it appears
//!   in, over RANDOM drafts. The first version of this harness scored each kit
//!   by leading its own default draft, and that reading was worthless: the
//!   default draft fills packmates and opponents by pool order, so a kit's
//!   record was mostly a statement about which trio it happened to face. It put
//!   Twofang at 88% and Stillsmoke at 12% while Twofang's fixed opponents were
//!   the three kits at the bottom of that same table. Random drafts scatter the
//!   teammate quality instead of baking it in.
//!
//! Every draft is played twice on the same land with the sides swapped, so a
//! kit that wins only because it started in the west corner cancels out.
//!
//! Run: cargo run --release --example battleground_balance [rolls]

use clawbound_rs::ancestor::{self, Ancestor};
use clawbound_rs::battleground::{Battleground, CLAN_A};

fn play(a: [Ancestor; 3], b: [Ancestor; 3], seed: u32, wombo: bool) -> Option<u8> {
    let mut bg = Battleground::new_on(a, b, seed);
    bg.wombo = wombo;
    let mut steps = 0u32;
    while bg.outcome.is_none() && steps < 30 * 60 * 13 {
        bg.step(1.0 / 30.0);
        steps += 1;
    }
    bg.outcome
}

fn main() {
    let rolls: u32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    let drafts: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(13);
    // arg 3: "nowombo" turns off AI crowd-control coordination, so the same
    // drafts on the same land can be played both ways and compared
    let wombo = std::env::args().nth(3).as_deref() != Some("nowombo");
    let pool = ancestor::ALL;
    let mut wins = vec![0u32; pool.len()];
    let mut games = vec![0u32; pool.len()];
    let mut clan_a_wins = 0u32;
    let mut total = 0u32;
    let mut fells = 0u32;
    let mut rng: u64 = 0x243f_6a88_85a3_08d3;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };

    for roll in 0..rolls {
        let seed = 0x9e37_79b9u32.wrapping_mul(roll + 1);
        for _ in 0..drafts {
            // six distinct ancestors, drawn without replacement
            let mut bag: Vec<usize> = (0..pool.len()).collect();
            let mut picked = [0usize; 6];
            for slot in picked.iter_mut() {
                let k = (next() as usize) % bag.len();
                *slot = bag.swap_remove(k);
            }
            let side_a = [pool[picked[0]], pool[picked[1]], pool[picked[2]]];
            let side_b = [pool[picked[3]], pool[picked[4]], pool[picked[5]]];
            for swap in [false, true] {
                let (x, y) = if swap { (side_b, side_a) } else { (side_a, side_b) };
                let Some(o) = play(x, y, seed, wombo) else { continue };
                total += 1;
                if o != 2 {
                    fells += 1;
                }
                if o == CLAN_A {
                    clan_a_wins += 1;
                }
                for (n, &pi) in picked.iter().enumerate() {
                    games[pi] += 1;
                    // whose side was this ancestor on, after the swap?
                    let on_a = (n < 3) != swap;
                    if o != 2 && (o == CLAN_A) == on_a {
                        wins[pi] += 1;
                    }
                }
            }
        }
        eprintln!("roll {} of {rolls} done ({total} games)", roll + 1);
    }

    eprintln!("\n{total} games over {rolls} terrain rolls (wombo {})", if wombo { "on" } else { "off" });
    eprintln!(
        "  clan A took {clan_a_wins} ({:.0}%) — the map claims symmetry, so this wants ~50%",
        clan_a_wins as f64 / total as f64 * 100.0
    );
    eprintln!(
        "  {fells} decided by felling a Great Tree ({:.0}%)",
        fells as f64 / total as f64 * 100.0
    );
    let mut rows: Vec<(f64, &str, u32, u32)> = pool
        .iter()
        .enumerate()
        .map(|(i, &a)| {
            let rate = if games[i] > 0 { wins[i] as f64 / games[i] as f64 } else { 0.0 };
            (rate, ancestor::kit(a).name, wins[i], games[i])
        })
        .collect();
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    eprintln!("\n  by ancestor, over random drafts (both sides of every draft played):");
    for (rate, name, w, g) in rows {
        eprintln!("    {name:<12} {w:2}/{g:2}  {:.0}%", rate * 100.0);
    }
}
