//! filter-train — the Learn2PD filter, trained on-box in seconds, no deps.
//!
//! Consumes the JSONL that `denoise-bench --trace=` writes (one labeled record
//! per step per masked position) and fits a class-weighted logistic model over
//! [p_top, margin, entropy, step]. Prints the precision/recall table across tau
//! on a held-out split — the visible loop — then exports weights + the chosen
//! tau for `LearnedPolicy` (`denoise-bench --filter=FILE`).
//!
//! Tau choice: smallest tau with held-out precision >= 0.98, else the max-
//! precision tau. Precision is the axis that matters: a false "reveal" commits
//! a wrong token early (quality loss); a false "hold" just costs one more step.
//!
//! Scale honesty: this is the paper's minute-level training made CPU-trivial
//! because the v1 filter is logistic. If a learned filter ever needs to be more
//! than logistic, that is the burn Autodiff<Wgpu> upgrade — same trace format,
//! same export contract.
//!
//!   cargo run --release --bin filter-train -- rnd1-run1.jsonl filter.l2pd

fn field(line: &str, key: &str) -> Option<f32> {
    let k = format!("\"{key}\":");
    let s = line.find(&k)? + k.len();
    let rest = &line[s..];
    let e = rest.find([',', '}']).unwrap_or(rest.len());
    rest[..e].trim().parse().ok()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (inp, outp) = match (args.get(1), args.get(2)) {
        (Some(i), Some(o)) => (i.clone(), o.clone()),
        _ => {
            eprintln!("usage: filter-train <trace.jsonl> <out.l2pd>");
            std::process::exit(2);
        }
    };
    let body = std::fs::read_to_string(&inp).expect("read trace");
    let mut xs: Vec<[f32; 4]> = Vec::new();
    let mut ys: Vec<f32> = Vec::new();
    for line in body.lines() {
        let (p, m, h, st, lb) = match (
            field(line, "p"), field(line, "margin"), field(line, "entropy"), field(line, "step"), field(line, "label"),
        ) {
            (Some(a), Some(b), Some(c), Some(d), Some(e)) => (a, b, c, d, e),
            _ => continue,
        };
        xs.push([p, m, h, st]);
        ys.push(lb);
    }
    let n = xs.len();
    assert!(n >= 50, "need at least 50 records to fit anything honest (got {n})");

    // Standardize on the TRAIN split only; export mu/sd so inference matches.
    let split = n * 4 / 5;
    let mut mu = [0.0f64; 4];
    let mut sd = [0.0f64; 4];
    for x in &xs[..split] { for j in 0..4 { mu[j] += x[j] as f64; } }
    for j in 0..4 { mu[j] /= split as f64; }
    for x in &xs[..split] { for j in 0..4 { let d = x[j] as f64 - mu[j]; sd[j] += d * d; } }
    for j in 0..4 { sd[j] = (sd[j] / split as f64).sqrt().max(1e-6); }
    let z = |x: &[f32; 4], j: usize| (x[j] as f64 - mu[j]) / sd[j];

    // Class weights: the trace is usually label-heavy on one side.
    let npos: f64 = ys[..split].iter().map(|&y| y as f64).sum();
    let nneg = split as f64 - npos;
    assert!(npos > 0.0 && nneg > 0.0, "single-class trace ({npos:.0} pos / {nneg:.0} neg) — nothing to learn; record a harder run");
    let (wp, wn) = (split as f64 / (2.0 * npos), split as f64 / (2.0 * nneg));

    // Full-batch gradient descent on class-weighted BCE. Seconds at 1e5 records.
    let mut w = [0.0f64; 4];
    let mut b = 0.0f64;
    let lr = 0.3;
    for _epoch in 0..400 {
        let mut gw = [0.0f64; 4];
        let mut gb = 0.0f64;
        for (x, &y) in xs[..split].iter().zip(&ys[..split]) {
            let mut zz = b;
            for j in 0..4 { zz += w[j] * z(x, j); }
            let p = 1.0 / (1.0 + (-zz).exp());
            let cw = if y > 0.5 { wp } else { wn };
            let g = cw * (p - y as f64);
            for j in 0..4 { gw[j] += g * z(x, j); }
            gb += g;
        }
        for j in 0..4 { w[j] -= lr * gw[j] / split as f64; }
        b -= lr * gb / split as f64;
    }

    // Held-out tau table, and the choice rule stated above.
    println!("  tau   precision  recall   reveal%   (held-out, {} records)", n - split);
    let score = |x: &[f32; 4]| {
        let mut zz = b;
        for j in 0..4 { zz += w[j] * z(x, j); }
        1.0 / (1.0 + (-zz).exp())
    };
    let mut chosen = (0.5f64, 0.0f64);
    for k in 0..10 {
        let tau = 0.50 + 0.05 * k as f64;
        let (mut tp, mut fp, mut fnn, mut rev) = (0.0f64, 0.0, 0.0, 0.0);
        for (x, &y) in xs[split..].iter().zip(&ys[split..]) {
            let r = score(x) >= tau;
            if r { rev += 1.0; if y > 0.5 { tp += 1.0 } else { fp += 1.0 } } else if y > 0.5 { fnn += 1.0 }
        }
        let prec = if tp + fp > 0.0 { tp / (tp + fp) } else { 1.0 };
        let rec = if tp + fnn > 0.0 { tp / (tp + fnn) } else { 0.0 };
        println!("  {tau:.2}   {prec:.4}     {rec:.4}   {:.1}%", 100.0 * rev / (n - split) as f64);
        if (chosen.1 < 0.98 && prec > chosen.1) || (prec >= 0.98 && chosen.1 < 0.98) {
            chosen = (tau, prec);
        }
    }
    println!("  chosen tau = {:.2} (precision {:.4})", chosen.0, chosen.1);

    let line = format!(
        "l2pd-v1 mu={:.6},{:.6},{:.6},{:.6} sd={:.6},{:.6},{:.6},{:.6} w={:.6},{:.6},{:.6},{:.6} b={:.6} tau={:.2}\n",
        mu[0], mu[1], mu[2], mu[3], sd[0], sd[1], sd[2], sd[3], w[0], w[1], w[2], w[3], b, chosen.0
    );
    std::fs::write(&outp, &line).expect("write weights");
    println!("filter-train: {n} records -> {outp}");
}
