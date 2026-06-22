// v0.1.704 — standalone WER scorer for corpus_wer_harness.sh.
// Built with `rustc -O scripts/wer_score.rs -o target/wer_score`.
// Mirrors src/wer.rs (kept in sync; the logic is small + identical) so the
// harness doesn't need the whole crate to compile.
//
// Modes:
//   wer_score --one <ref> <hyp>      → prints the WER float for one pair
//   wer_score --pooled <pairs.tsv>   → reads ref<TAB>hyp lines, prints pooled WER

use std::io::Read;

fn normalize_words(s: &str) -> Vec<String> {
    let mut cur = String::new();
    let mut out = Vec::new();
    for ch in s.chars() {
        // v0.1.706: ASCII alphanumerics only — drops mojibake/symbol artifacts
        // (e.g. the corpus's "â" from a double-encoded "→") so a mangled
        // reference doesn't inflate WER. Mirrors src/wer.rs::normalize_words.
        if ch.is_ascii_alphanumeric() {
            cur.push(ch.to_ascii_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() { out.push(cur); }
    out
}

fn word_edit_distance(a: &[String], b: &[String]) -> usize {
    let (n, m) = (a.len(), b.len());
    if n == 0 { return m; }
    if m == 0 { return n; }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0usize; m + 1];
    for i in 1..=n {
        cur[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

fn score_edits(reference: &str, hypothesis: &str) -> (usize, usize) {
    let r = normalize_words(reference);
    let h = normalize_words(hypothesis);
    (word_edit_distance(&r, &h), r.len())
}

// v0.1.705: full-matrix alignment backtrace → substitution pairs (ref→hyp),
// for confusion mining.
fn substitutions(reference: &str, hypothesis: &str) -> Vec<(String, String)> {
    let r = normalize_words(reference);
    let h = normalize_words(hypothesis);
    let (n, m) = (r.len(), h.len());
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 0..=n { dp[i][0] = i; }
    for j in 0..=m { dp[0][j] = j; }
    for i in 1..=n {
        for j in 1..=m {
            let cost = if r[i - 1] == h[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1).min(dp[i][j - 1] + 1).min(dp[i - 1][j - 1] + cost);
        }
    }
    let mut subs = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 {
            let cost = if r[i - 1] == h[j - 1] { 0 } else { 1 };
            if dp[i][j] == dp[i - 1][j - 1] + cost {
                if cost == 1 { subs.push((r[i - 1].clone(), h[j - 1].clone())); }
                i -= 1; j -= 1; continue;
            }
        }
        if i > 0 && dp[i][j] == dp[i - 1][j] + 1 { i -= 1; continue; }
        j -= 1;
    }
    subs
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("--one") => {
            let r = args.get(2).map(|s| s.as_str()).unwrap_or("");
            let h = args.get(3).map(|s| s.as_str()).unwrap_or("");
            let (edits, refw) = score_edits(r, h);
            let wer = if refw == 0 { if h.trim().is_empty() { 0.0 } else { 1.0 } }
                      else { edits as f64 / refw as f64 };
            println!("{:.3}", wer);
        }
        Some("--pooled") => {
            let body = match args.get(2) {
                Some(path) => std::fs::read_to_string(path).unwrap_or_default(),
                None => { let mut s = String::new();
                    std::io::stdin().read_to_string(&mut s).ok(); s }
            };
            let mut total_edits = 0usize;
            let mut total_ref = 0usize;
            for line in body.lines() {
                if line.trim().is_empty() { continue; }
                let mut it = line.splitn(2, '\t');
                let r = it.next().unwrap_or("");
                let h = it.next().unwrap_or("");
                let (e, rw) = score_edits(r, h);
                total_edits += e;
                total_ref += rw;
            }
            let wer = if total_ref == 0 { 0.0 } else { total_edits as f64 / total_ref as f64 };
            println!("{:.3}", wer);
        }
        // v0.1.705 (#1): per-speaker pooled WER. Reads speaker<TAB>ref<TAB>hyp
        // lines; prints "speaker<TAB>wer<TAB>refwords" sorted by speaker.
        Some("--pooled-speaker") => {
            use std::collections::BTreeMap;
            let body = std::fs::read_to_string(args.get(2).map(|s| s.as_str()).unwrap_or(""))
                .unwrap_or_default();
            let mut agg: BTreeMap<String, (usize, usize)> = BTreeMap::new();
            for line in body.lines() {
                if line.trim().is_empty() { continue; }
                let mut it = line.splitn(3, '\t');
                let spk = it.next().unwrap_or("").to_string();
                let r = it.next().unwrap_or("");
                let h = it.next().unwrap_or("");
                let (e, rw) = score_edits(r, h);
                let ent = agg.entry(spk).or_insert((0, 0));
                ent.0 += e; ent.1 += rw;
            }
            for (spk, (e, rw)) in agg {
                let wer = if rw == 0 { 0.0 } else { e as f64 / rw as f64 };
                println!("{}\t{:.3}\t{}", spk, wer, rw);
            }
        }
        // v0.1.705 (#4): confusion mining. Reads ref<TAB>hyp lines; prints the
        // substitution pairs "refword<TAB>hypword<TAB>count" sorted by count.
        Some("--subs") => {
            use std::collections::HashMap;
            let body = std::fs::read_to_string(args.get(2).map(|s| s.as_str()).unwrap_or(""))
                .unwrap_or_default();
            let mut counts: HashMap<(String, String), usize> = HashMap::new();
            for line in body.lines() {
                if line.trim().is_empty() { continue; }
                let mut it = line.splitn(2, '\t');
                let r = it.next().unwrap_or("");
                let h = it.next().unwrap_or("");
                for pair in substitutions(r, h) {
                    *counts.entry(pair).or_insert(0) += 1;
                }
            }
            let mut v: Vec<_> = counts.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            for ((rw, hw), c) in v {
                println!("{}\t{}\t{}", rw, hw, c);
            }
        }
        _ => {
            eprintln!("usage: wer_score --one <ref> <hyp> | --pooled <pairs.tsv> | --pooled-speaker <triples.tsv> | --subs <pairs.tsv>");
            std::process::exit(2);
        }
    }
}
