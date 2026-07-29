fn main() {
    use logscope::coreclass::*;
    let cpus = read_capacities();
    println!("{} CPUs with a published capacity", cpus.len());
    match classify(&cpus) {
        None => println!("  uniform (or unknown) — no efficiency split, everything gets all cores"),
        Some((e, p)) => {
            println!("  performance: {} cores {:?}", p.len(), &p[..p.len().min(8)]);
            println!("  efficiency : {} cores {:?}", e.len(), e);
            println!("  E/P throughput ratio: {:.2}", efficiency_ratio(&cpus).unwrap_or(0.0));
            for (name, tps) in [("3B liaison", 40.0f32), ("30B thinker", 5.0)] {
                println!("  {name:12} -> {:?}", place(&cpus, tps, 2.0));
            }
        }
    }
}
