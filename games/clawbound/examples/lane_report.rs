//! What the three trails actually are, per clan. A one-second answer to the
//! question that used to need a mirror probe and a 168-game balance run.
//! Run: cargo run --release --example lane_report
use clawbound_rs::ancestor::Ancestor;
use clawbound_rs::battleground::Battleground;
use clawbound_rs::mapeval::lane_score;

fn main() {
    for seed in [0u32, 7, 42] {
        let bg = Battleground::new_on(
            [Ancestor::Lion, Ancestor::Tiger, Ancestor::Leopard],
            [Ancestor::Puma, Ancestor::Caracal, Ancestor::Panther],
            seed,
        );
        let s = lane_score(bg.camps[0], bg.camps[1], [(-85.0, 85.0), (0.0, 0.0), (85.0, -85.0)]);
        println!("\nland {seed}: {}", s.verdict());
        println!("  {:<6} {:>7} {:>6} {:>6} {:>6}", "lane", "height", "wet", "cover", "climb");
        for (name, p) in ["duo", "mid", "solo"].iter().zip(s.lanes.iter()) {
            println!(
                "  {:<6} {:>7.2} {:>6.2} {:>6.2} {:>6.2}",
                name, p.height, p.wet, p.cover, p.climb
            );
        }
        println!("  distinctness {:.3}   clan gap {:.4}", s.distinctness, s.clan_gap);
    }
}
