fn main() {
    use std::time::Instant;
    let t = Instant::now();
    let ranked = early_fps::mapagents::mass_rank(2000, 48, 48, 1);
    println!("Simulated {} territories in {:.1?}", 2000, t.elapsed());
    println!("top emergent territories (relief / lane / pockets):");
    for (seed, sc) in ranked.iter().take(6) {
        println!("  {:>5.1}  seed {:016x}  r{:.2} l{:.2} p{:.2}", sc.total, seed, sc.relief, sc.lane, sc.pockets);
    }
}
