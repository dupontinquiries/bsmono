//! How long does a match cost? The balance protocol asks for ~800 games per
//! claim, so the price of one match is the price of knowing anything.
use clawbound_rs::ancestor::Ancestor;
use clawbound_rs::battleground::Battleground;
use std::time::Instant;

fn main() {
    let n: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(4);
    let t0 = Instant::now();
    let mut steps = 0u64;
    for m in 0..n {
        let mut bg = Battleground::new_on(
            [Ancestor::Lion, Ancestor::Tiger, Ancestor::Leopard],
            [Ancestor::Puma, Ancestor::Caracal, Ancestor::Panther],
            m,
        );
        while bg.outcome.is_none() && steps % 100_000 != 99_999 {
            bg.step(1.0 / 30.0);
            steps += 1;
            if bg.time > 13.0 * 60.0 { break }
        }
    }
    let el = t0.elapsed().as_secs_f64();
    eprintln!(
        "{n} matches, {steps} steps in {el:.2}s — {:.0} steps/s, {:.2}s per match, {:.0}s for an 800-game run",
        steps as f64 / el,
        el / n as f64,
        el / n as f64 * 800.0
    );
}
