//! Score every authored map for cinematic liveliness — the pacing of motion (calm baseline + twitch spikes)
//! that keeps a player on edge. Runs each map for 10 seconds from its centre and reports the profile.
use early_fps::liveliness::analyze_preset;
fn main() {
    println!("{:<20} {:>7} {:>7} {:>6} {:>6} {:>7} {:>7}", "MAP", "mean", "base", "peak", "events", "spiky", "SCORE");
    println!("{}", "-".repeat(64));
    let maps = ["Desert Town","Malecon Breakwater","Redwood Canyon","Concrete Slab","Pyre Garage","Flood Zone","Reference Town"];
    for name in maps {
        if let Some(r) = analyze_preset(name, 7, 600) {
            println!("{:<20} {:>7.3} {:>7.3} {:>6.2} {:>6} {:>7.2} {:>6.2}",
                name, r.mean_activity, r.baseline, r.peak, r.events, r.spikiness, r.score);
        }
    }
    println!("\nscore = cinematic liveliness: calm-but-alive baseline + distinct twitch spikes at a healthy");
    println!("cadence, penalising both dead flatlines and constant motion the eye tunes out.");
}
