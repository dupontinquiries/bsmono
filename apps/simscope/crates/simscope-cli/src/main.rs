//! simscope — runs each layer headlessly against its reference data.
//!
//! There is no GUI and there will not be one here; every layer's claim is a number compared to a
//! number, and this binary is where those comparisons are reported.

use sim_num::NewtonOpts;
use sim_power::Case;

const CASES: [&str; 6] = ["case9", "case14", "case30", "case57", "case118", "case300"];

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "fixtures".into());
    println!("simscope — L0 power flow against pandapower\n");
    println!(
        "  {:<9} {:>5} {:>5} {:>5} {:>6} {:>11} {:>11} {:>6}",
        "case", "bus", "unk", "iter", "order", "max d|V|", "max dθ", ""
    );

    let mut failures = 0usize;
    for name in CASES {
        let path = format!("{root}/{name}.json");
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                println!("  {name:<9} could not read {path}: {e}");
                failures += 1;
                continue;
            }
        };
        let case = match Case::from_json(&text) {
            Ok(c) => c,
            Err(e) => {
                println!("  {name:<9} parse failed: {e}");
                failures += 1;
                continue;
            }
        };

        let t0 = std::time::Instant::now();
        let (report, vm, va) = sim_power::solve(&case, NewtonOpts { tol: 1e-10, ..Default::default() });
        let elapsed = t0.elapsed().as_secs_f64() * 1e3;

        let (ref_vm, ref_va) = case.solution.clone().unwrap();
        let dvm = (0..case.n).fold(0.0f64, |m, i| m.max((vm[i] - ref_vm[i]).abs()));
        let dva = (0..case.n).fold(0.0f64, |m, i| m.max((va[i] - ref_va[i]).abs()));
        let unknowns = case.pvpq().len() + case.pq().len();
        let ok = report.converged() && dvm < 1e-6 && dva < 1e-6;
        if !ok {
            failures += 1;
        }
        println!(
            "  {:<9} {:>5} {:>5} {:>5} {:>6.2} {:>11.2e} {:>11.2e} {:>6}  {:.1} ms",
            name,
            case.n,
            unknowns,
            report.iterations,
            report.observed_order().unwrap_or(0.0),
            dvm,
            dva,
            if ok { "ok" } else { "FAIL" },
            elapsed
        );
    }

    println!(
        "\n  residual history is the diagnostic: quadratic convergence means the analytic\n  \
         Jacobian is right, and the finite-difference test in sim-power proves it directly."
    );
    println!("\nL0: {}", if failures == 0 { "PASS" } else { "FAIL" });
    std::process::exit(if failures == 0 { 0 } else { 1 });
}
