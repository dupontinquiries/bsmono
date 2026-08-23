//! Why does a freshkill lead not compound? Samples every clan's pile through a
//! match and asks what a lead is worth two minutes later, plus who is doing the
//! hunting while it happens.
//!
//! Run: cargo run --release --example economy_probe [matches]

use clawbound_rs::ancestor::{self, Ancestor};
use clawbound_rs::battleground::{Battleground, Carry, Mind, Role, CLAN_A};

fn main() {
    let n: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20);
    let pool = ancestor::ALL;
    let mut rng: u64 = 0x2b7e_1516_28ae_d2a6;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    // (lead now, lead 120s later) pairs, and hunting effort by who is ahead
    let mut pairs: Vec<(f32, f32)> = Vec::new();
    let mut hunt_ahead = 0u64;
    let mut hunt_behind = 0u64;
    let mut growth_ahead = 0.0f64;
    let mut growth_behind = 0.0f64;
    let mut samples = 0u64;

    for m in 0..n {
        let mut bag: Vec<usize> = (0..pool.len()).collect();
        let mut picks = [Ancestor::Lion; 6];
        for slot in picks.iter_mut() {
            *slot = pool[bag.swap_remove((next() as usize) % bag.len())];
        }
        let mut bg = Battleground::new_on(
            [picks[0], picks[1], picks[2]],
            [picks[3], picks[4], picks[5]],
            m + 1,
        );
        let mut trace: Vec<f32> = Vec::new(); // lead for clan A, sampled each second
        let mut steps = 0;
        while bg.outcome.is_none() && steps < 30 * 60 * 13 {
            bg.step(1.0 / 30.0);
            steps += 1;
            if steps % 30 == 0 {
                let lead = bg.clans[0].pile - bg.clans[1].pile;
                trace.push(lead);
                // who is out hunting right now, and what is the growth worth?
                for w in bg.warriors.iter() {
                    if w.role != Role::Ancestor || !w.alive() {
                        continue;
                    }
                    let ahead = if w.team == CLAN_A { lead > 0.0 } else { lead < 0.0 };
                    if matches!(w.mind, Mind::Hunt | Mind::CarryHome)
                        || w.carry != Carry::Nothing
                    {
                        if ahead { hunt_ahead += 1 } else { hunt_behind += 1 }
                    }
                }
                let (ga, gb) = (bg.growth(0) as f64, bg.growth(1) as f64);
                if lead > 0.0 {
                    growth_ahead += ga;
                    growth_behind += gb;
                } else if lead < 0.0 {
                    growth_ahead += gb;
                    growth_behind += ga;
                }
                samples += 1;
            }
        }
        for (i, &now) in trace.iter().enumerate() {
            if let Some(&later) = trace.get(i + 120) {
                pairs.push((now, later));
            }
        }
    }

    // how much of a lead survives two minutes? slope of later on now
    let (mut sxy, mut sxx) = (0.0f64, 0.0f64);
    for &(a, b) in &pairs {
        sxy += a as f64 * b as f64;
        sxx += a as f64 * a as f64;
    }
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let big: Vec<&(f32, f32)> = pairs.iter().filter(|(a, _)| a.abs() >= 8.0).collect();
    let kept: f64 = big.iter().map(|(a, b)| (b / a) as f64).sum::<f64>() / big.len().max(1) as f64;

    eprintln!("{n} matches, {} lead samples paired 120s apart", pairs.len());
    eprintln!("  a freshkill lead keeps {:.0}% of itself after two minutes (slope {slope:.2})", slope * 100.0);
    eprintln!("  leads of 8+: {:.0}% of the lead survives, over {} samples", kept * 100.0, big.len());
    eprintln!(
        "  hunting effort while ahead {hunt_ahead} vs while behind {hunt_behind} ({:.2}x more when behind)",
        hunt_behind as f64 / hunt_ahead.max(1) as f64
    );
    eprintln!(
        "  mean growth multiplier: leader {:.3} against trailer {:.3} — the whole prize for a lead",
        growth_ahead / samples.max(1) as f64,
        growth_behind / samples.max(1) as f64
    );
}
