//! Is a side favoured, and where does the difference show up? Tallies per clan.
use clawbound_rs::ancestor::Ancestor;
use clawbound_rs::battleground::{Battleground, Mind, Role, CLAN_A};

fn main() {
    let picks = [Ancestor::Lion, Ancestor::Tiger, Ancestor::Leopard];
    let mut agg = [[0.0f64; 4]; 2]; // retreat-time, shelter-ish, deaths, tree-hp-left
    let mut a_wins = 0;
    let n = 8;
    for seed in 0..n {
        let mut bg = Battleground::new_on(picks, picks, seed as u32 * 7 + 1);
        let mut retreat = [0.0f64; 2];
        let mut deaths = [0u32; 2];
        let mut was_alive = vec![true; 64];
        let mut steps = 0;
        while bg.outcome.is_none() && steps < 30 * 60 * 13 {
            bg.step(1.0 / 30.0);
            steps += 1;
            for (i, w) in bg.warriors.iter().enumerate() {
                if w.role != Role::Ancestor { continue }
                let t = w.team as usize;
                if w.mind == Mind::Retreat { retreat[t] += 1.0 / 30.0; }
                if i < was_alive.len() {
                    let alive = w.mind != Mind::Dead;
                    if was_alive[i] && !alive { deaths[t] += 1; }
                    was_alive[i] = alive;
                }
            }
        }
        if bg.outcome == Some(CLAN_A) { a_wins += 1; }
        for t in 0..2 {
            agg[t][0] += retreat[t];
            agg[t][2] += deaths[t] as f64;
            agg[t][3] += bg.clans[t].great_tree_hp.max(0.0) as f64;
        }
    }
    eprintln!("identical drafts both sides, {n} lands: clan A won {a_wins}");
    for t in 0..2 {
        eprintln!("  clan {}: retreat {:.0}s  deaths {:.1}  great tree left {:.0}",
            if t == 0 { "A" } else { "B" }, agg[t][0] / n as f64, agg[t][2] / n as f64, agg[t][3] / n as f64);
    }
}
