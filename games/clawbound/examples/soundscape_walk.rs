//! Listen to the territory: walk a line across the map and print the mix at
//! each step, then play a match and print the one-shots as they fire.
//! Run: cargo run --release --example soundscape_walk
use clawbound_rs::ancestor::Ancestor;
use clawbound_rs::battleground::{Battleground, CLAN_A};
use clawbound_rs::soundscape::{Ears, Soundscape};

fn main() {
    let mut bg = Battleground::new(Ancestor::Lion);
    let mut s = Soundscape::new();
    // settle the beds so the walk is not measuring the fade-in
    let start = bg.camps[0];
    let e0 = Ears { x: start.0, z: start.1, heading: 0.0 };
    for _ in 0..120 {
        s.update(&bg, CLAN_A, e0, 1.0 / 30.0);
    }

    println!("a walk from clan A's camp to the contention ground:\n");
    let steps = 12;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let e = Ears { x: start.0 * (1.0 - t), z: start.1 * (1.0 - t), heading: 0.8 };
        // a few frames at each stop so the beds catch up
        let mut mix = Vec::new();
        for _ in 0..45 {
            mix = s.update(&bg, CLAN_A, e, 1.0 / 30.0);
        }
        mix.sort_by(|a, b| b.gain.total_cmp(&a.gain));
        let line: Vec<String> = mix
            .iter()
            .take(5)
            .map(|v| format!("{} {:.2}{}", v.cue.id(), v.gain,
                if v.pan.abs() < 0.2 { "" } else if v.pan > 0.0 { " R" } else { " L" }))
            .collect();
        println!("  ({:>4.0},{:>4.0})  {}", e.x, e.z, line.join("   "));
    }

    println!("\nand the cues a match fires:\n");
    let mut s = Soundscape::new();
    let mut steps = 0;
    while bg.outcome.is_none() && steps < 30 * 60 * 13 {
        bg.step(1.0 / 30.0);
        steps += 1;
        let me = &bg.warriors[0];
        let e = Ears { x: me.x, z: me.z, heading: me.heading };
        for v in s.update(&bg, CLAN_A, e, 1.0 / 30.0) {
            if v.cue.is_event() {
                println!("  {:>5.0}s  {}", bg.time, v.cue.id());
            }
        }
    }
}
