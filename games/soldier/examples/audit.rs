fn main() {
    use early_fps::level::{Level, PRESETS};
    use early_fps::player::audit_landings;
    println!("Jetpack-landing audit (clean% / embedded / fell) — 4000 drops per map:");
    let mut worst: Vec<(f32, &str)> = Vec::new();
    for p in PRESETS.iter() {
        let lvl = Level::generate(p.config(7));
        let a = audit_landings(&lvl, 4000, 99);
        worst.push((a.clean_rate(), p.name));
    }
    worst.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    for (rate, name) in worst.iter().take(10) {
        println!("  {:5.1}%  {}", rate * 100.0, name);
    }
    let avg: f32 = worst.iter().map(|(r, _)| r).sum::<f32>() / worst.len() as f32;
    println!("  ---\n  fleet average clean landing rate: {:.2}%", avg * 100.0);
}
