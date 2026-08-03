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

    failures += l1();
    std::process::exit(if failures == 0 { 0 } else { 1 });
}

/// L1: a 2D field solve on a coax cross-section, checked against the closed form. The point of the
/// table is that Z0 and v_phase come out of an energy integral over triangles, with no
/// transmission-line formula anywhere in the code path — these are the numbers a network model
/// takes as given.
fn l1() -> usize {
    use sim_field::{coax_capacitance, extract, Mesh, C0, MU0, EPS0};
    println!("\nsimscope — L1 field solve against analytic coax\n");
    println!(
        "  {:<22} {:>5} {:>10} {:>10} {:>9} {:>9} {:>6}",
        "geometry", "nodes", "C pF/m", "exact", "err", "Z0 ohm", ""
    );
    let mut failures = 0usize;
    // (a, b, eps_r, label)
    let geoms = [
        (1.0e-3, 3.0e-3, 1.0, "air, b/a = 3"),
        (0.45e-3, 1.503e-3, 2.1, "PTFE 50 ohm"),
        (1.0e-3, 4.0e-3, 2.2, "FR4-ish, b/a = 4"),
    ];
    for (a, b, er, label) in geoms {
        let (nr, nt) = (24, 256);
        let m = Mesh::annulus(a, b, nr, nt);
        let inner: Vec<usize> = Mesh::annulus_ring(nt, 0).collect();
        let outer: Vec<usize> = Mesh::annulus_ring(nt, nr).collect();
        let r = match extract(&m, &inner, &outer, |_| er) {
            Ok(r) => r,
            Err(e) => {
                println!("  {label:<22} failed: {e}");
                failures += 1;
                continue;
            }
        };
        let exact = coax_capacitance(a, b, er);
        let z_exact = (MU0 / (EPS0 * er)).sqrt() * (b / a).ln() / std::f64::consts::TAU;
        let err = (r.c - exact).abs() / exact;
        let ok = err < 3e-3 && (r.z0 - z_exact).abs() / z_exact < 5e-3;
        if !ok {
            failures += 1;
        }
        println!(
            "  {:<22} {:>5} {:>10.3} {:>10.3} {:>8.3}% {:>9.2} {:>6}",
            label,
            m.nodes(),
            r.c * 1e12,
            exact * 1e12,
            err * 100.0,
            r.z0,
            if ok { "ok" } else { "FAIL" }
        );
        let v_err = (r.v_phase - C0 / er.sqrt()).abs() / (C0 / er.sqrt());
        if v_err > 3e-3 {
            println!("      phase velocity off by {:.3}%", v_err * 100.0);
            failures += 1;
        }
    }
    println!(
        "\n  C from an energy integral; L from the vacuum solve via L = 1/(c0^2 C_vac);\n  \
         Z0 and v_phase follow. No transmission-line formula is used to get them."
    );
    println!("\nL1: {}", if failures == 0 { "PASS" } else { "FAIL" });
    failures + l2()
}

/// L2: real board cross-sections. Stripline is the sharp case — it is homogeneous, so eps_eff must
/// come out exactly equal to the substrate, which is physics rather than a curve fit. Microstrip is
/// the useful case and is compared against Hammerstad.
fn l2() -> usize {
    use sim_pcb::{formulas, solve, MeshOpts, Stackup, Trace};
    println!("\nsimscope — L2 board cross-sections\n");
    println!(
        "  {:<28} {:>9} {:>9} {:>9} {:>9} {:>6}",
        "cross-section", "Z0 ohm", "ref", "eps_eff", "ps/mm", ""
    );
    let mut failures = 0usize;
    let opts = MeshOpts::default();

    // Stripline: homogeneous, so eps_eff is exactly the substrate.
    for (b, w, er) in [(1.0e-3, 0.5e-3, 4.2), (0.8e-3, 0.3e-3, 3.0)] {
        let r = match solve(&Stackup::stripline(b, er), &Trace { width: w, on_layer: 0 }, &opts) {
            Ok(r) => r,
            Err(e) => {
                println!("  stripline failed: {e}");
                failures += 1;
                continue;
            }
        };
        let refz = formulas::stripline_z0(w, b, er);
        let ok = (r.eps_eff - er).abs() / er < 5e-3 && (r.z0 - refz).abs() / refz < 0.10;
        if !ok {
            failures += 1;
        }
        println!(
            "  {:<28} {:>9.2} {:>9.2} {:>9.3} {:>9.2} {:>6}",
            format!("stripline w={:.2}mm b={:.1}mm", w * 1e3, b * 1e3),
            r.z0,
            refz,
            r.eps_eff,
            1e12 / r.v_phase / 1e3,
            if ok { "ok" } else { "FAIL" }
        );
    }

    // Microstrip: inhomogeneous, shielded box approximating the open case.
    for (h, w, er) in [(0.2e-3, 0.38e-3, 4.4), (0.5e-3, 1.0e-3, 4.4), (0.254e-3, 0.7e-3, 3.0)] {
        let stack = Stackup::microstrip(h, er, h * 12.0);
        let r = match solve(&stack, &Trace { width: w, on_layer: 0 }, &opts) {
            Ok(r) => r,
            Err(e) => {
                println!("  microstrip failed: {e}");
                failures += 1;
                continue;
            }
        };
        let refz = formulas::microstrip_z0(w, h, er);
        let ok = (r.z0 - refz).abs() / refz < 0.15 && r.eps_eff > 1.0 && r.eps_eff < er;
        if !ok {
            failures += 1;
        }
        println!(
            "  {:<28} {:>9.2} {:>9.2} {:>9.3} {:>9.2} {:>6}",
            format!("microstrip w={:.2}mm h={:.2}mm", w * 1e3, h * 1e3),
            r.z0,
            refz,
            r.eps_eff,
            1e12 / r.v_phase / 1e3,
            if ok { "ok" } else { "FAIL" }
        );
    }

    println!(
        "\n  stripline eps_eff must equal the substrate exactly — it is homogeneous, so that is\n  \
         physics, not a fit. The microstrip reference is Hammerstad, itself a ~1% curve fit to an\n  \
         OPEN geometry, while the solver models a shielded box; disagreement of a few percent is\n  \
         the lid, not an error."
    );
    println!("\nL2: {}", if failures == 0 { "PASS" } else { "FAIL" });
    failures + l3()
}

/// L3: EUV aerial imaging. The table is a resolution study — the same mask through pupils of
/// different size and illumination of different shape, with the coherent cutoff marked, because
/// that edge is exact physics and everything else in lithography is negotiating with it.
fn l3() -> usize {
    use sim_litho::{aerial_image, contrast, Illumination, Mask, Optics};
    println!("\nsimscope — L3 EUV aerial imaging\n");
    let euv = Optics::euv();
    let hna = Optics::high_na_euv();
    println!(
        "  lambda {:.1} nm; NA {:.2} -> coherent pitch limit {:.1} nm (half-pitch {:.1} nm)",
        euv.lambda * 1e9,
        euv.na,
        euv.coherent_pitch_limit() * 1e9,
        euv.coherent_pitch_limit() * 0.5e9
    );
    println!(
        "  High-NA {:.2} -> {:.1} nm pitch, but depth of focus falls {:.1}x\n",
        hna.na,
        hna.coherent_pitch_limit() * 1e9,
        euv.depth_of_focus(1.0) / hna.depth_of_focus(1.0)
    );
    println!(
        "  {:>8} {:>7} {:>11} {:>11} {:>11} {:>7}",
        "pitch", "hp", "coherent", "sigma 0.5", "dipole", ""
    );

    let n = 128;
    let mut failures = 0usize;
    let limit = euv.coherent_pitch_limit();
    // Any pitch is fine as long as it lands on a whole number of pixels; fixing 16 pixels per
    // period and deriving dx from the pitch guarantees that for every pitch we ask for.
    for k in [1.6f64, 1.2, 0.95, 0.8] {
        let pitch = limit * k;
        let dx = pitch / 16.0;
        let m = Mask::grating(n, dx, pitch, 0.5);
        let coh = contrast(&aerial_image(&m, &euv, &Illumination::Conventional { sigma: 0.0 }, 8), n);
        let par = contrast(&aerial_image(&m, &euv, &Illumination::Conventional { sigma: 0.5 }, 16), n);
        let dip = contrast(
            &aerial_image(&m, &euv, &Illumination::Dipole { sigma: 0.7, radius: 0.2 }, 24),
            n,
        );
        // Below the cutoff coherent illumination must give exactly nothing, and off-axis must not.
        let below_cutoff = pitch < limit;
        let ok = if below_cutoff { coh < 1e-9 && dip > 0.1 } else { coh > 0.1 };
        if !ok {
            failures += 1;
        }
        println!(
            "  {:>7.1}n {:>6.1}n {:>11.4} {:>11.4} {:>11.4} {:>7}",
            pitch * 1e9,
            pitch * 0.5e9,
            coh,
            par,
            dip,
            if ok { "ok" } else { "FAIL" }
        );
    }
    println!(
        "\n  Below the cutoff coherent contrast is exactly zero — one diffraction order is one\n  \
         plane wave, and a plane wave has uniform intensity. Off-axis illumination shifts the\n  \
         pupil onto an order that missed, which is why dipole still prints there."
    );
    println!("\nL3: {}", if failures == 0 { "PASS" } else { "FAIL" });
    failures
}
