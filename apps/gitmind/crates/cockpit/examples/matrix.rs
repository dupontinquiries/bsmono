use repo_map::matrix::{run_repo_matrix, RunOpts};
fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let opts = RunOpts { test: true, include_cmake: false, only: Some(vec!["repo-map".into(), "netcode".into()]) };
    let report = run_repo_matrix(&root, &["apps", "shared", "games"], &opts);
    println!("{}", report.summary());
    for r in &report.results {
        println!("  {:<12} {:?}", r.name, r.outcome);
    }
    println!("all_green: {}", report.all_green());
}
