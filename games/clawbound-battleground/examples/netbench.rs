// Lockstep netplay, naive vs tuned, over simulated networks: reaction time
// (press -> the tick it takes effect), stalls, bandwidth, and sync.
//   cargo run --example netbench [seconds]
use clawbound::netplay::{trial, NetSim, Tuning};
fn main() {
    let secs: f32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(60.0);
    let nets: [(&str, f32, f32, f32); 4] = [
        ("wired LAN   (1 ms, 0.5 jitter, 0% loss)", 1.0, 0.5, 0.0),
        ("Wi-Fi       (15 ms, 8 jitter, 2% loss)", 15.0, 8.0, 0.02),
        ("busy Wi-Fi  (30 ms, 15 jitter, 5% loss)", 30.0, 15.0, 0.05),
        ("internet    (60 ms, 20 jitter, 3% loss)", 60.0, 20.0, 0.03),
    ];
    println!("{secs:.0} s per trial, 2 machines, 60 Hz; reaction = press -> takes effect\n");
    println!("{:<42} {:<6} {:>8} {:>8} {:>8} {:>9} {:>7} {:>5}", "network", "stack", "p50 ms", "p95 ms", "stall %", "bytes/s", "delay", "sync");
    for (name, lat, jit, loss) in nets {
        for (label, tuning) in [("naive", Tuning::NAIVE), ("tuned", Tuning::TUNED)] {
            let t = trial(tuning, NetSim::new(lat, jit, loss, 42), secs);
            println!(
                "{:<42} {:<6} {:>8.1} {:>8.1} {:>8.2} {:>9.0} {:>7} {:>5}",
                name, label, t.pct(0.5), t.pct(0.95), 100.0 * t.stalled_frames as f32 / t.frames as f32, t.bytes_per_sec,
                format!("{}/{}", t.final_delay[0], t.final_delay[1]), if t.desync { "NO" } else { "ok" }
            );
        }
    }
}
