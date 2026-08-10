//! gate-filter — the Learn2PD loop end-to-end on pure CPU: synthesize a labeled
//! trace from a KNOWN rule, train with filter-train's exact code path (invoked as
//! a process so the export contract is tested, not an internal shortcut), then
//! load the weights with LearnedPolicy and check it recovers the rule.
//!
//! The synthetic rule: label = 1 iff 0.7*p + 0.3*(margin/10) + noise(±0.05) > 0.55.
//! A logistic model over (p, margin, entropy, step) can represent this almost
//! exactly, so held-out accuracy must clear 0.9 — failure means the trainer or
//! the standardization/export/parse round-trip is broken, not the model class.
//!
//!   cargo run --release --bin gate-filter

use gpu_kernels::*;
use std::io::Write;

fn main() {
    let mut ok = true;
    let mut ck = |l: &str, v: bool| {
        ok &= v;
        println!("  [{}] {l}", if v { "\u{2713}" } else { "\u{2717} FAIL" });
    };

    let mut rng = 0xf117e5_2024u64;
    let mut rnd = || {
        rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17;
        (rng >> 40) as f32 / (1u64 << 24) as f32
    };

    // Synthesize the trace in denoise-bench's exact JSONL shape.
    let trace = "/tmp/gate-filter-trace.jsonl";
    {
        let f = std::fs::File::create(trace).expect("create trace");
        let mut w = std::io::BufWriter::new(f);
        for i in 0..4000 {
            let p = rnd();
            let margin = rnd() * 10.0;
            let entropy = rnd() * 8.0;
            let step = (i % 16) as f32;
            let noise = (rnd() - 0.5) * 0.1;
            let label = (0.7 * p + 0.3 * (margin / 10.0) + noise > 0.55) as u8;
            writeln!(
                w,
                "{{\"step\":{step},\"pos\":{i},\"argmax\":7,\"p\":{p:.6},\"margin\":{margin:.4},\"entropy\":{entropy:.4},\"revealed\":0,\"label\":{label}}}"
            )
            .unwrap();
        }
    }

    // Train via the real binary so the file contract is what's under test.
    let weights = "/tmp/gate-filter.l2pd";
    let exe = std::env::current_exe().expect("current_exe");
    let train = exe.parent().unwrap().join("filter-train");
    let out = std::process::Command::new(&train).arg(trace).arg(weights).output();
    let ran = match &out {
        Ok(o) if o.status.success() => true,
        Ok(o) => { println!("  [i] filter-train stderr: {}", String::from_utf8_lossy(&o.stderr)); false }
        Err(e) => { println!("  [i] could not spawn filter-train ({e}) — build it first: cargo build --release --bin filter-train"); false }
    };
    ck("filter-train ran on the synthetic trace", ran);
    if !ran { println!("gate-filter: FAIL \u{2717}"); std::process::exit(1); }

    let txt = std::fs::read_to_string(weights).expect("read weights");
    let lp = LearnedPolicy::from_text(&txt);
    ck("exported weights parse via LearnedPolicy::from_text", lp.is_ok());
    let lp = lp.unwrap();

    // Held-out check against the same rule, fresh samples.
    let (mut right, mut total) = (0usize, 0usize);
    for i in 0..1000 {
        let p = rnd();
        let margin = rnd() * 10.0;
        let entropy = rnd() * 8.0;
        let step = (i % 16) as usize;
        let label = 0.7 * p + 0.3 * (margin / 10.0) > 0.55; // noiseless rule for eval
        let pred = lp.score(&[7.0, p, margin, entropy], step) >= 0.5;
        right += (pred == label) as usize;
        total += 1;
    }
    let acc = right as f32 / total as f32;
    ck(&format!("recovers the generating rule (fresh-sample acc {acc:.3} >= 0.90)"), acc >= 0.90);

    // Policy invariants: masked-only + progress floor, same contract as the heuristics.
    let stats: Vec<[f32; 4]> = vec![[7.0, 0.99, 9.0, 1.0], [7.0, 0.01, 0.1, 7.0], [7.0, 0.98, 8.0, 1.0]];
    let masked = vec![true, true, false];
    let mut pol = LearnedPolicy { tau: lp.tau, ..lp };
    let picks = pol.select(0, &stats, &masked);
    ck("learned policy never selects revealed positions", !picks.contains(&2));
    let masked = vec![true, true, true];
    let mut hopeless = LearnedPolicy { tau: 0.999_999, w: pol.w, b: pol.b, mu: pol.mu, sd: pol.sd };
    let picks = hopeless.select(0, &stats, &masked);
    ck("progress floor: impossible tau still reveals exactly one", picks.len() == 1);

    println!(
        "gate-filter: {}",
        if ok { "PASS \u{2713} — trace -> train -> export -> policy closes on this box" } else { "FAIL \u{2717}" }
    );
    std::process::exit(if ok { 0 } else { 1 });
}
