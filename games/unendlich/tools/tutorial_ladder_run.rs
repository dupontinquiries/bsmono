//! Standalone debug runner for the practice-world training ladder.
//! Build: rustc --edition 2021 tools/tutorial_ladder_run.rs -o /tmp/ladder && /tmp/ladder
//! It scripts a "player" through every element and prints whether the bending
//! harmonisation mechanic accepted their pitch and how coupling accrued — the
//! bending debug suite in action.
#[path = "../crates/unendlich-engine/src/sim/tutorial_ladder.rs"]
mod tutorial_ladder;
use tutorial_ladder::*;

fn main() {
    let mut s = LadderState::new();
    // A scripted player: sings each element's target, but with varying skill.
    // (hz_offset_cents, sustain_s) — how sharp/flat and how long they hold it.
    let script = [
        (Element::Earth,       0.0,  3.2), // nails it
        (Element::Water,      10.0,  3.0), // slightly sharp but inside ±15c
        (Element::Fire,       25.0,  3.0), // too sharp — misses
        (Element::Air,         0.0,  1.5), // on pitch but can't hold it — misses
        (Element::Storm,      -8.0,  4.0), // flat but inside window
        (Element::Brine,       0.0,  3.0), // clean
        (Element::Information, 5.0,  3.0), // clean
    ];
    println!("\n=== PRACTICE-WORLD TRAINING LADDER — scripted debug run ===\n");
    for (el, cents, sustain) in script {
        // convert a cents offset into an actual sung frequency
        let hz = el.fundamental_hz() * 2f32.powf(cents / 1200.0);
        let run = run_element(&mut s, el, hz, sustain);
        let hits = run.harmonisations.iter().filter(|&&h| h).count();
        println!(
            "{:<12} mentor {:<26} target {:>5.0}Hz  sung {:>6.1}Hz ({:+.0}c, {:.1}s)  \
             harmonised {}/{}  coupling {:>4.0}%  {}",
            format!("{:?}", el),
            el.mentor(),
            el.fundamental_hz(),
            hz,
            cents,
            sustain,
            hits,
            run.harmonisations.len(),
            run.final_coupling * 100.0,
            if run.completed { "✓ complete" } else { "…" },
        );
    }
    println!(
        "\nElements complete: {}/7   Choir guide unlocked: {}",
        s.completed_count(),
        s.choir_unlocked()
    );
    let total: f32 = Element::ALL.iter().map(|&e| s.carry_forward_coupling(e)).sum();
    println!("Total coupling carried into the main world: {:.0}% across 7 bands\n", total * 100.0);
}
