use cockpit::matrix::{run_repo_matrix, RunOpts};
fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    // restrict to a few fast crates so the demo is quick; real runs drop `only`.
    let opts = RunOpts { test: true, include_cmake: false, only: Some(vec!["repo-map".into(), "netcode".into()]) };
    let report = run_repo_matrix(&root, &opts);
    println!("{}", report.summary());
    for r in &report.results {
        println!("  {:<12} {:?}", r.name, r.outcome);
    }
    for f in report.failures() {
        println!("\n--- {} failed ---\n{}", f.name, f.detail);
    }
    println!("\nall_green: {}", report.all_green());
}
