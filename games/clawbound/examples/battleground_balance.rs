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
use clawbound_rs::battleground::{Battleground, LanePolicy, CLAN_A};

/// Returns the winner, and whether the clan that broke the enemy's first border
/// went on to win. That second number is a measure of the AI itself rather than
/// of the roster: a clan that takes the first objective and then loses anyway
/// was not converting an advantage, and no amount of kit balance shows that.
fn play(
    a: [Ancestor; 3],
    b: [Ancestor; 3],
    seed: u32,
    wombo: bool,
    policy: LanePolicy,
) -> (Option<u8>, Option<bool>) {
    let mut bg = Battleground::new_with(a, b, seed, policy);
    bg.wombo = wombo;
    bg.collapse_on_breach = std::env::args().any(|s| s == "push");
    bg.spoils = std::env::args().any(|s| s == "spoils");
    let mut steps = 0u32;
    let mut first_breaker: Option<u8> = None;
    while bg.outcome.is_none() && steps < 30 * 60 * 13 {
        bg.step(1.0 / 30.0);
        steps += 1;
        if first_breaker.is_none() {
            if let Some(t) = bg.heal_trees.iter().find(|t| !t.alive()) {
                first_breaker = Some(1 - t.team);   // whoever felled it
            }
        }
    }
    let converted = match (first_breaker, bg.outcome) {
        (Some(b), Some(w)) if w != 2 => Some(b == w),
        _ => None,
    };
    (bg.outcome, converted)
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
    // arg 3: "wombo" turns ON AI crowd-control coordination (off by default,
    // because two ablations at 168 games each measured it at nothing), so the
    // same drafts on the same land can be played both ways and compared
    let wombo = std::env::args().nth(3).as_deref() == Some("wombo");
    // arg 4: "seat" plays the old draft-order laning, for A/B against "kit"
    let policy = if std::env::args().nth(4).as_deref() == Some("seat") {
        LanePolicy::Seat
    } else {
        LanePolicy::Kit
    };
    // arg 5: a draft-set seed. Arms are compared on IDENTICAL drafts by default
    // (same seed, same lands), which is what makes a delta meaningful; change it
    // to pool independent draft sets and shrink the per-kit error bar.
    let draft_seed: u64 = std::env::args()
        .nth(5)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x243f_6a88_85a3_08d3);
    let pool = ancestor::ALL;
    let mut wins = vec![0u32; pool.len()];
    let mut games = vec![0u32; pool.len()];
    let mut clan_a_wins = 0u32;
    let mut total = 0u32;
    let mut fells = 0u32;
    let (mut pairs, mut pair_a_both, mut pair_b_both, mut pair_split) = (0u32, 0u32, 0u32, 0u32);
    let (mut conversions, mut converted_wins) = (0u32, 0u32);
    let mut rng: u64 = draft_seed;
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
            let mut pair_a_wins = 0u32;   // how many of the two orientations clan A took
            let mut pair_played = 0u32;
            for swap in [false, true] {
                let (x, y) = if swap { (side_b, side_a) } else { (side_a, side_b) };
                let (outcome, converted) = play(x, y, seed, wombo, policy);
                let Some(o) = outcome else { continue };
                if let Some(c) = converted {
                    conversions += 1;
                    if c {
                        converted_wins += 1;
                    }
                }
                pair_played += 1;
                if o == CLAN_A { pair_a_wins += 1; }
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
            // Orientation bookkeeping. If the map and sim are fair, a draft pair
            // played both ways should split 1-1 (the stronger COMPOSITION wins
            // both times, once as A and once as B). Pairs where one SIDE takes
            // both are where a side bias would hide, and counting them separates
            // "clan A is disadvantaged" from "this draft set favoured clan B".
            if pair_played == 2 {
                pairs += 1;
                match pair_a_wins {
                    2 => pair_a_both += 1,
                    0 => pair_b_both += 1,
                    _ => pair_split += 1,
                }
            }
        }
        eprintln!("roll {} of {rolls} done ({total} games)", roll + 1);
    }

    eprintln!(
        "\n{total} games over {rolls} terrain rolls (wombo {}, laning {:?})",
        if wombo { "on" } else { "off" },
        policy
    );
    // decided games only: a draw is not half a win for clan A, and counting it
    // in the denominator quietly depresses the number the whole harness is for
    let decided = fells;
    eprintln!(
        "  clan A took {clan_a_wins} of {decided} decided ({:.0}%) — the map claims symmetry, so this wants ~50%",
        clan_a_wins as f64 / decided.max(1) as f64 * 100.0
    );
    eprintln!(
        "  {fells} decided by felling a Great Tree ({:.0}%)",
        fells as f64 / total as f64 * 100.0
    );
    eprintln!(
        "  first border broken -> match won: {converted_wins}/{conversions} ({:.0}%) — an AI measure, not a roster one",
        converted_wins as f64 / conversions.max(1) as f64 * 100.0
    );
    eprintln!(
        "  of {pairs} draft pairs played both ways: {pair_split} split 1-1, {pair_a_both} taken by clan A twice, {pair_b_both} by clan B twice"
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
    // With the error bar printed beside it, a ten-point difference stops looking
    // like a finding. At n≈70 games a kit's standard error is about six points,
    // so nothing inside ±12 of even is evidence of anything on one arm.
    for (rate, name, w, g) in rows {
        let se = if g > 0 { (0.25f64 / g as f64).sqrt() * 100.0 } else { 0.0 };
        let flag = if (rate * 100.0 - 50.0).abs() > 2.0 * se { " *" } else { "" };
        eprintln!("    {name:<12} {w:2}/{g:2}  {:.0}% ± {se:.0}{flag}", rate * 100.0);
    }
    eprintln!("  (* = outside two standard errors; anything else is this arm's noise)");
}
