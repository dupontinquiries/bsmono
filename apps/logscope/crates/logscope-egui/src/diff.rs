//! diff.rs — word-level LCS diff into redline ops: the track-changes engine behind the tree's
//! "highlight changes" view. Deletions and insertions are kept in place (strikethrough / colored
//! insert) so nuanced branch-to-branch differences stay visible — the professional redline standard,
//! not a line-oriented unified diff. Op computation is here (pure, testable); the egui render
//! (strikethrough for Del, color for Ins, fold runs of Equal) is the app's job.
#![allow(dead_code)]

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Equal(String),
    Del(String), // present in A (old), removed in B
    Ins(String), // added in B (new)
}

/// Word-level LCS diff of `a` (old) → `b` (new). Runs are coalesced so each Op is a phrase.
pub fn word_diff(a: &str, b: &str) -> Vec<Op> {
    let aw: Vec<&str> = a.split_whitespace().collect();
    let bw: Vec<&str> = b.split_whitespace().collect();
    let (n, m) = (aw.len(), bw.len());
    // LCS length table (suffix DP)
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if aw[i] == bw[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    // backtrack into per-word ops
    let mut raw: Vec<Op> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if aw[i] == bw[j] { raw.push(Op::Equal(aw[i].into())); i += 1; j += 1; }
        else if dp[i + 1][j] >= dp[i][j + 1] { raw.push(Op::Del(aw[i].into())); i += 1; }
        else { raw.push(Op::Ins(bw[j].into())); j += 1; }
    }
    while i < n { raw.push(Op::Del(aw[i].into())); i += 1; }
    while j < m { raw.push(Op::Ins(bw[j].into())); j += 1; }
    coalesce(raw)
}

fn coalesce(raw: Vec<Op>) -> Vec<Op> {
    let mut out: Vec<Op> = Vec::new();
    for op in raw {
        let merged = match (out.last_mut(), &op) {
            (Some(Op::Equal(s)), Op::Equal(w)) => { s.push(' '); s.push_str(w); true }
            (Some(Op::Del(s)), Op::Del(w)) => { s.push(' '); s.push_str(w); true }
            (Some(Op::Ins(s)), Op::Ins(w)) => { s.push(' '); s.push_str(w); true }
            _ => false,
        };
        if !merged { out.push(op); }
    }
    out
}

/// Fraction of words changed (Del+Ins over total) — a cheap branch-distance for the tree.
pub fn change_ratio(ops: &[Op]) -> f64 {
    let words = |s: &str| s.split_whitespace().count();
    let (mut changed, mut total) = (0usize, 0usize);
    for op in ops {
        match op {
            Op::Equal(s) => total += words(s),
            Op::Del(s) | Op::Ins(s) => { let w = words(s); changed += w; total += w; }
        }
    }
    if total == 0 { 0.0 } else { changed as f64 / total as f64 }
}

/// Plain-text redline for non-egui contexts and tests: ~~deleted~~ and {+inserted+}.
pub fn redline(ops: &[Op]) -> String {
    let mut s = String::new();
    for op in ops {
        match op {
            Op::Equal(w) => { s.push_str(w); s.push(' '); }
            Op::Del(w) => { s.push_str("~~"); s.push_str(w); s.push_str("~~ "); }
            Op::Ins(w) => { s.push_str("{+"); s.push_str(w); s.push_str("+} "); }
        }
    }
    s.trim_end().to_string()
}

/// Self-test (no deps): `--gate-diff`.
pub fn gate_diff() {
    let ops = word_diff("the quick brown fox", "the slow brown fox");
    let rl = redline(&ops);
    let c1 = rl == "the ~~quick~~ {+slow+} brown fox";
    let c2 = word_diff("same words", "same words") == vec![Op::Equal("same words".into())];
    let c3 = word_diff("", "added text") == vec![Op::Ins("added text".into())];
    let c4 = word_diff("gone now", "") == vec![Op::Del("gone now".into())];
    let c5 = (change_ratio(&ops) - 0.4).abs() < 1e-9; // del+ins=2 changed over 5 touched = 0.4
    for (label, ok) in [("substitution redline", c1), ("identical = all Equal", c2),
        ("pure insert", c3), ("pure delete", c4), ("change_ratio", c5)] {
        println!("  [{}] {}", if ok { "\u{2713}" } else { "\u{2717} FAIL" }, label);
    }
    println!("  redline: {rl}");
    println!("gate-diff: {}", if c1 && c2 && c3 && c4 && c5 { "diff engine checks \u{2713}" } else { "FAIL \u{2717}" });
}
