//! scope_predict.rs — is module locality a real signal, or just frequency wearing a hat?
//!
//! The claim is that between two names a branch might write, the one this module already uses more
//! is the better bet. That is testable on the repository as it stands, with no model.
//!
//! ## The test
//!
//! For each file, take an identifier it actually uses and pair it with a DISTRACTOR: another
//! identifier from the repository that the file does not use, chosen to have as close to the same
//! GLOBAL frequency as possible. Matching on global frequency is the whole design — it makes global
//! frequency uninformative by construction, pinning it at 50%, so whatever accuracy remains is
//! attributable to locality and nothing else. An unmatched distractor would let a global scorer
//! win by noticing that `self` is commoner than `kv_dim`, which proves nothing.
//!
//! The index is built leave-one-file-out, so the file under test never contributes to the counts
//! that judge it.
//!
//! ## The temporal half
//!
//! Locality is a snapshot. The separate question is whether DIRECTION helps: build the index from
//! the tree as it was N commits ago, then predict today's vocabulary. A stale index is wrong about
//! anything that has moved since; if reading the intervening diffs recovers some of that, trend is
//! carrying information a snapshot cannot.
//!
//! Run: `cargo run --release --example scope_predict -- <repo-root> <dir>...`

use gpu_kernels::corpus::{identifiers, strip_noncode};
use gpu_kernels::scope::{ScopedIndex, TrendIndex};
use std::collections::{HashMap, HashSet};
use std::process::Command;

fn collect(dir: &std::path::Path, root: &str, out: &mut Vec<(String, String)>, cap: usize) {
    if out.len() >= cap { return; }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut e: Vec<_> = rd.filter_map(|x| x.ok()).collect();
    e.sort_by_key(|x| x.path());
    for en in e {
        if out.len() >= cap { return; }
        let p = en.path();
        if p.is_dir() {
            let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if n == "target" || n == ".git" || n == "node_modules" { continue; }
            collect(&p, root, out, cap);
        } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            if let Ok(src) = std::fs::read_to_string(&p) {
                if src.len() > 800 {
                    let rel = p.strip_prefix(root).unwrap_or(&p).display().to_string();
                    out.push((rel, src));
                }
            }
        }
    }
}

/// Distinct identifiers used at least twice, so a one-off typo is not treated as vocabulary.
fn vocab_of(src: &str) -> Vec<String> {
    let mut c: HashMap<String, u32> = HashMap::new();
    for id in identifiers(&strip_noncode(src)) { *c.entry(id).or_insert(0) += 1; }
    let mut v: Vec<String> = c.into_iter().filter(|(_, n)| *n >= 2).map(|(k, _)| k).collect();
    v.sort();
    v
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() { eprintln!("usage: scope_predict <repo-root> <dir>..."); return; }
    let root = args[0].clone();
    let dirs: Vec<String> = if args.len() > 1 { args[1..].to_vec() } else { vec![root.clone()] };
    let cap: usize = std::env::var("CAP").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    let back: usize = std::env::var("BACK").ok().and_then(|s| s.parse().ok()).unwrap_or(300);

    let mut files = Vec::new();
    for d in &dirs { collect(std::path::Path::new(d), &root, &mut files, cap); }
    if files.is_empty() { eprintln!("no files under {dirs:?}"); return; }

    // Global frequency table and the pool distractors are drawn from.
    let mut gcount: HashMap<String, u32> = HashMap::new();
    for (_, s) in &files {
        for id in identifiers(&strip_noncode(s)) { *gcount.entry(id).or_insert(0) += 1; }
    }
    let mut pool: Vec<(String, u32)> = gcount.iter()
        .filter(|(k, v)| **v >= 4 && k.len() > 3).map(|(k, v)| (k.clone(), *v)).collect();
    pool.sort_by_key(|(_, v)| *v);

    println!("{} files, {} identifiers in the distractor pool\n", files.len(), pool.len());

    for decay in [0.3f32, 0.5, 0.7, 1.0] {
        let (mut right, mut trials, mut ties) = (0usize, 0usize, 0usize);
        for (i, (path, src)) in files.iter().enumerate() {
            // Leave-one-out.
            let mut idx = ScopedIndex::new();
            for (j, (p2, s2)) in files.iter().enumerate() { if j != i { idx.add_file(p2, s2); } }
            let used: HashSet<String> = vocab_of(src).into_iter().collect();
            for (k, name) in vocab_of(src).iter().enumerate() {
                if k % 7 != 0 { continue; }               // subsample; leave-one-out is the cost
                if idx.global_count(name) == 0 { continue; }
                let want = gcount.get(name).copied().unwrap_or(0);
                // Nearest global frequency among names this file does not use.
                let Some((distractor, _)) = pool.iter()
                    .filter(|(n, _)| !used.contains(n) && idx.global_count(n) > 0)
                    .min_by_key(|(_, c)| (*c as i64 - want as i64).abs()) else { continue };
                let d = idx.prefer(path, name, distractor, decay);
                trials += 1;
                if d > 1e-6 { right += 1 } else if d.abs() <= 1e-6 { ties += 1 }
            }
        }
        if trials == 0 { println!("decay {decay:.1}: no trials"); continue; }
        // Ties are cases where lift has no opinion on one side or the other — usually a distractor
        // the leave-one-out index has never seen, which the no-opinion guard scores as 0. Counting
        // them as losses understates the signal; hiding them overstates it. Both, then.
        let decided = trials - ties;
        println!("decay {decay:.1}:  {}/{} = {:.1}% of all trials, {:.1}% of the {} decided ({} ties)",
                 right, trials, 100.0 * right as f32 / trials as f32,
                 100.0 * right as f32 / decided.max(1) as f32, decided, ties);
    }
    println!("\nglobal frequency is 50.0% by construction — distractors are frequency-matched.\n");

    // ---- temporal half ----
    let head_back = format!("HEAD~{back}");
    let ok = Command::new("git").args(["-C", &root, "rev-parse", "--verify", &head_back])
        .output().map(|o| o.status.success()).unwrap_or(false);
    if !ok { println!("no {head_back} in this repository; skipping the temporal half"); return; }

    println!("== stale index from {head_back}, predicting today's vocabulary ==");
    let mut stale_src: Vec<(String, Option<String>)> = Vec::with_capacity(files.len());
    for (path, _) in &files {
        let out = Command::new("git").args(["-C", &root, "show", &format!("{head_back}:{path}")])
            .output();
        let src = out.ok().filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok());
        stale_src.push((path.clone(), src));
    }
    let staled = stale_src.iter().filter(|(_, s)| s.is_some()).count();
    println!("{staled}/{} files existed at {head_back}", files.len());

    let diff = Command::new("git")
        .args(["-C", &root, "log", "-p", &format!("-{back}"), "--no-color", "--", "*.rs"])
        .output();
    let mut trend = TrendIndex::new();
    if let Ok(o) = diff {
        if let Ok(text) = String::from_utf8(o.stdout) { trend.add_diff(&text); }
    }

    let (mut s_right, mut st_right, mut trials, mut st_ties) = (0usize, 0usize, 0usize, 0usize);
    let mut tb_right = 0usize;
    for (i, (path, src)) in files.iter().enumerate() {
        // Leave-one-out over the STALE tree too. Without this a file's own past self contributes to
        // the scope counts that judge it, and since most files change little over 300 commits that
        // is close to being told the answer. The first run of this reported 78.3% that way.
        let mut stale = ScopedIndex::new();
        for (j, (p2, s2)) in stale_src.iter().enumerate() {
            if j == i { continue; }
            if let Some(s2) = s2 { stale.add_file(p2, s2); }
        }
        let used: HashSet<String> = vocab_of(src).into_iter().collect();
        for (k, name) in vocab_of(src).iter().enumerate() {
            if k % 7 != 0 { continue; }
            let want = gcount.get(name).copied().unwrap_or(0);
            let Some((distractor, _)) = pool.iter()
                .filter(|(n, _)| !used.contains(n)).min_by_key(|(_, c)| (*c as i64 - want as i64).abs())
                else { continue };
            let base = stale.prefer(path, name, distractor, 0.5);
            let m = trend.momentum(path, name, 4).unwrap_or(0.0)
                - trend.momentum(path, distractor, 4).unwrap_or(0.0);
            trials += 1;
            let tied = base.abs() <= 1e-6;
            if tied { st_ties += 1 }
            if base > 1e-6 { s_right += 1 }
            if base + 2.0 * m > 1e-6 { st_right += 1 }
            // Momentum as a TIEBREAK rather than an addend. Summing lets a fixed-weight trend term
            // override lift on the cases lift already gets right; on a directory where the stale
            // index is dense (26 ties in 1854) that cost more than the trend was worth.
            if (tied && m > 1e-6) || (!tied && base > 1e-6) { tb_right += 1 }
        }
    }
    if trials > 0 {
        let dec = (trials - st_ties).max(1);
        println!("  stale lift alone      : {:.1}% of all, {:.1}% of decided",
                 100.0 * s_right as f32 / trials as f32, 100.0 * s_right as f32 / dec as f32);
        println!("  + momentum (summed)   : {:.1}% of all", 100.0 * st_right as f32 / trials as f32);
        println!("  + momentum (tiebreak) : {:.1}% of all  ({trials} trials, {st_ties} lift-ties)",
                 100.0 * tb_right as f32 / trials as f32);
    }
}
