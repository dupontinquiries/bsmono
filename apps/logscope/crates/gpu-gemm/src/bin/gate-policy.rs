//! gate-policy — the decode-policy layer, tested where it lives: pure CPU.
//!
//! Policies are the seam Learn2PD's filter will occupy, so their contract gets
//! its own oracle before any learned model touches it. No GPU, runs anywhere.
//!
//! Covers: ThresholdPolicy reveals exactly the >= tau masked set; its floor
//! guarantees progress when nothing clears the bar; it never touches revealed
//! positions; ConfidenceTopK reveals exactly K by confidence order; and
//! TraceRecorder's oracle labels round-trip through JSONL — label must be
//! `argmax_at_step == final_token`, per position, per step.
//!
//!   cargo run --release --bin gate-policy

use gpu_kernels::*;

fn stats_of(ps: &[f32]) -> Vec<[f32; 4]> {
    ps.iter().enumerate().map(|(i, &p)| [(100 + i) as f32, p, 0.0, 0.0]).collect()
}

fn main() {
    let mut ok = true;
    let mut ck = |l: &str, v: bool| {
        ok &= v;
        println!("  [{}] {l}", if v { "\u{2713}" } else { "\u{2717} FAIL" });
    };

    // Threshold: exact >= tau set, masked-only.
    let stats = stats_of(&[0.95, 0.40, 0.91, 0.99, 0.10]);
    let masked = vec![true, true, true, false, true]; // pos 3 already revealed
    let mut th = ThresholdPolicy { tau: 0.9 };
    let mut picks = th.select(0, &stats, &masked);
    picks.sort_unstable();
    ck("threshold reveals exactly the masked >= tau set", picks == vec![0, 2]);
    ck("threshold never selects revealed positions", !picks.contains(&3));

    // Floor: all below tau -> exactly the single most confident masked position.
    let stats = stats_of(&[0.10, 0.30, 0.20]);
    let masked = vec![true, true, true];
    let picks = th.select(1, &stats, &masked);
    ck("progress floor reveals exactly one (the most confident)", picks == vec![1]);

    // Top-K: exact count, confidence-ordered membership.
    let stats = stats_of(&[0.5, 0.9, 0.1, 0.8, 0.7]);
    let masked = vec![true; 5];
    let mut tk = ConfidenceTopK { k: 3 };
    let mut picks = tk.select(0, &stats, &masked);
    picks.sort_unstable();
    ck("top-k reveals exactly k, by confidence", picks == vec![1, 3, 4]);

    // Recorder: labels are the Learn2PD oracle bit. Build a 2-step trajectory over
    // 3 positions where step-0 argmaxes are ids 100,101,102 and the FINAL tokens
    // are 100,999,102 -> labels at step 0 must be 1,0,1 for positions 0,1,2.
    let mut rec = TraceRecorder::new(ConfidenceTopK { k: 1 });
    let stats0 = stats_of(&[0.9, 0.5, 0.4]); // argmax ids 100,101,102
    let mut masked = vec![true, true, true];
    let p0 = rec.select(0, &stats0, &masked);
    for &i in &p0 { masked[i] = false; } // reveals pos 0
    let stats1 = stats_of(&[0.0, 0.6, 0.7]);
    let p1 = rec.select(1, &stats1, &masked);
    let _ = p1;
    let final_tokens = vec![100u32, 999, 102];
    let path = "/tmp/gate-policy-trace.jsonl";
    let n = rec.finalize(&final_tokens, path).expect("trace write");
    ck("recorder wrote one line per (step, masked position)", n == 5); // 3 at step 0 + 2 at step 1
    let body = std::fs::read_to_string(path).expect("trace read");
    let lines: Vec<&str> = body.lines().collect();
    ck("jsonl line count matches", lines.len() == 5);
    let has = |frag: &str| lines.iter().any(|l| l.contains(frag));
    ck(
        "step-0 labels are the oracle bit (100 right, 101 wrong vs final 999, 102 right)",
        has("\"pos\":0,\"argmax\":100") && has("\"pos\":1,\"argmax\":101") && {
            let l0 = lines.iter().find(|l| l.contains("\"step\":0,\"pos\":0,")).unwrap();
            let l1 = lines.iter().find(|l| l.contains("\"step\":0,\"pos\":1,")).unwrap();
            let l2 = lines.iter().find(|l| l.contains("\"step\":0,\"pos\":2,")).unwrap();
            l0.ends_with("\"label\":1}") && l1.ends_with("\"label\":0}") && l2.ends_with("\"label\":1}")
        },
    );
    ck("revealed flag recorded on the picked position", {
        let l0 = lines.iter().find(|l| l.contains("\"step\":0,\"pos\":0,")).unwrap();
        l0.contains("\"revealed\":1")
    });

    println!(
        "gate-policy: {}",
        if ok { "PASS \u{2713} — the policy seam and the Learn2PD trace pipe behave; a learned filter can drop in" } else { "FAIL \u{2717}" }
    );
    std::process::exit(if ok { 0 } else { 1 });
}
