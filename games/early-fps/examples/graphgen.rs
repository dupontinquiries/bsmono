// The map factory: mass-generate thousands of abstract graphs, score gameplay on the topology alone, and
// surface only the ones worth rendering — no geometry built for the rejects.
fn main() {
    use std::time::Instant;
    let t = Instant::now();
    let n = 5000;
    let ranked = early_fps::mapgraph::mass_rank(n, 1);
    let elapsed = t.elapsed();
    let passing = ranked.iter().filter(|(_, s)| s.connected && s.total >= 48.0).count();
    println!("Generated + scored {} candidate graphs in {:.1?}", n, elapsed);
    println!("{} graded C+ ({:.1}%) — only these get skinned into 3D.\n", passing, passing as f32 / n as f32 * 100.0);
    print!("{}", early_fps::mapgraph::report(64, 1));
    // show the blueprint the top graph compiles to
    let (best_seed, _) = ranked[0];
    let bp = early_fps::mapgraph::MapGraph::generate(best_seed).to_blueprint();
    println!("\n--- winning graph compiles to this data blueprint ---");
    print!("{}", bp.to_text());
}
