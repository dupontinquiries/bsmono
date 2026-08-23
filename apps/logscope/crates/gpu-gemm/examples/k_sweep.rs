//! k_sweep — how many tokens a verification pass actually commits, per k.
//!
//! ## The prediction this answers
//!
//! `ASAHI_GPU.md` records a specific, unmeasured claim:
//!
//! > Defaults today: `LOGSCOPE_SPEC_K=4`. Their run used k=7. **Worth
//! > re-sweeping k on this hardware for exactly the reason above** — a larger
//! > k costs almost nothing per forward when submits dominate, so the optimum
//! > here should sit higher than on a compute-bound card. That is a
//! > prediction, not a measurement.
//!
//! Half of it is measurable without hardware and half is not, and separating
//! them is the point of this example.
//!
//! **Acceptance length — tokens committed per verification pass — depends only
//! on the token sequence and the drafter.** Not on the GPU, not on submit
//! costs, not on the backend. `draft_bakeoff.rs` makes exactly this argument
//! for comparing drafters, and it applies unchanged to comparing k. So the
//! shape of the curve is answerable here, today, on any machine.
//!
//! **What that curve is WORTH is hardware-dependent and is not measured here.**
//! On a compute-bound card, verifying k+1 tokens costs meaningfully more than
//! verifying 1, so the optimum sits where the rising acceptance stops paying
//! for the rising verify cost. On the Asahi path — 92% GPU sync across ~35
//! submits per forward — the batch dimension is nearly free, so the optimum
//! sits wherever acceptance stops rising. Same curve, two different readings,
//! and only the curve is portable.
//!
//! Run: `cargo run --release --example k_sweep -- <dir>...`
//!      `CAP=n` files, `TCAP=n` tokens per file, `KMAX=n` largest k.

use gpu_kernels::spec::{Drafter, PromptLookup};
use std::collections::HashMap;

/// Word / punctuation / whitespace units, interned. Identical to
/// `draft_bakeoff.rs` so the two examples' numbers are comparable — a
/// different tokenizer would make them silently incomparable, which for two
/// tables sitting in the same repo is worse than having only one.
fn tokenize(src: &str, vocab: &mut HashMap<String, u32>) -> Vec<u32> {
    let mut out = Vec::new();
    let b: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let mut push = |s: String, out: &mut Vec<u32>, v: &mut HashMap<String, u32>| {
        let n = v.len() as u32;
        out.push(*v.entry(s).or_insert(n));
    };
    while i < b.len() {
        let c = b[i];
        if c.is_alphanumeric() || c == '_' {
            let s = i;
            while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_') { i += 1; }
            push(b[s..i].iter().collect(), &mut out, vocab);
        } else if c.is_whitespace() {
            let s = i;
            while i < b.len() && b[i].is_whitespace() { i += 1; }
            push(b[s..i].iter().collect(), &mut out, vocab);
        } else {
            push(c.to_string(), &mut out, vocab);
            i += 1;
        }
    }
    out
}

/// Tokens emitted per verification pass, and the mean accepted draft length.
///
/// Mirrors `spec.rs`'s own replay: the drafter proposes from the committed
/// context, the corpus says what actually came next, the matching prefix plus
/// one correction commits.
///
/// Returns (emitted/rounds, accepted/rounds). The second is what "acceptance
/// length" means literally; the first is what the engine converts into speed,
/// and it is the second plus one. Reporting both because `c4-829` records that
/// a high acceptance RATE on short drafts can be worth less than a mediocre
/// rate on long ones — the rate is the misleading one.
fn replay(toks: &[u32], k: usize, prompt_len: usize,
          draft: &mut dyn FnMut(&[u32], usize) -> Vec<u32>) -> (f64, f64) {
    if toks.len() <= prompt_len { return (1.0, 0.0); }
    let mut ctx: Vec<u32> = toks[..prompt_len].to_vec();
    let (mut i, mut rounds, mut emitted, mut accepted) = (prompt_len, 0usize, 0usize, 0usize);
    while i < toks.len() {
        let d = draft(&ctx, k);
        rounds += 1;
        let mut j = 0;
        while j < d.len() && i + j < toks.len() && d[j] == toks[i + j] { j += 1; }
        accepted += j;
        let take = (j + 1).min(toks.len() - i);
        ctx.extend_from_slice(&toks[i..i + take]);
        i += take;
        emitted += take;
    }
    let r = rounds.max(1) as f64;
    (emitted as f64 / r, accepted as f64 / r)
}

fn collect(dir: &std::path::Path, out: &mut Vec<(String, String)>, cap: usize) {
    if out.len() >= cap { return; }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut e: Vec<_> = rd.filter_map(|x| x.ok()).collect();
    e.sort_by_key(|x| x.path());
    for en in e {
        if out.len() >= cap { return; }
        let p = en.path();
        if p.is_dir() {
            let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if n == "target" || n == ".git" { continue; }
            collect(&p, out, cap);
        } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            if let Ok(s) = std::fs::read_to_string(&p) {
                if s.len() > 2000 { out.push((p.display().to_string(), s)); }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dirs = if args.is_empty() { vec![".".to_string()] } else { args };
    let cap: usize = std::env::var("CAP").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
    let tcap: usize = std::env::var("TCAP").ok().and_then(|s| s.parse().ok()).unwrap_or(2000);
    let kmax: usize = std::env::var("KMAX").ok().and_then(|s| s.parse().ok()).unwrap_or(12);

    let mut files = Vec::new();
    for d in &dirs { collect(std::path::Path::new(d), &mut files, cap); }
    if files.is_empty() {
        eprintln!("no .rs files over 2000 bytes under {dirs:?}");
        return;
    }

    let mut vocab = HashMap::new();
    let corpora: Vec<(String, Vec<u32>)> = files
        .iter()
        .map(|(n, s)| {
            let mut t = tokenize(s, &mut vocab);
            t.truncate(tcap);
            (n.clone(), t)
        })
        .filter(|(_, t)| t.len() > 200)
        .collect();

    println!("corpus: {} file(s), {} tokens, vocab {}",
             corpora.len(),
             corpora.iter().map(|(_, t)| t.len()).sum::<usize>(),
             vocab.len());
    // BOTH drafters, because the first sweep showed k is not the interesting
    // axis. `spec::PromptLookup` is the wired default (fixed 2-gram);
    // `lookupdraft::draft` searches suffix lengths 8 down to 3 and takes the
    // first hit. If the second is better the lever is the drafter, not k, and a
    // table of one column cannot say that.
    println!("   k   PromptLookup(ng=2)      lookupdraft(8..3)");
    println!("        emit/pass  accept       emit/pass  accept");

    let mut base = 0.0f64;
    let mut best = (0usize, 0.0f64, "");
    for k in 1..=kmax {
        let (mut em1, mut ac1, mut em2, mut ac2, mut n) = (0.0, 0.0, 0.0, 0.0, 0usize);
        for (_, toks) in &corpora {
            let plen = (toks.len() / 4).max(32);
            let mut d = PromptLookup { ngram: 2 };
            let (e1, a1) = replay(toks, k, plen, &mut |ctx, k| d.draft(ctx, k));
            let (e2, a2) = replay(toks, k, plen,
                                  &mut |ctx, k| gpu_kernels::lookupdraft::draft(ctx, k));
            em1 += e1; ac1 += a1; em2 += e2; ac2 += a2; n += 1;
        }
        let nf = n as f64;
        let (em1, ac1, em2, ac2) = (em1 / nf, ac1 / nf, em2 / nf, ac2 / nf);
        if k == 1 { base = em1; }
        if em1 > best.1 { best = (k, em1, "PromptLookup"); }
        if em2 > best.1 { best = (k, em2, "lookupdraft"); }
        println!("  {k:2}      {em1:6.3}   {ac1:6.3}          {em2:6.3}   {ac2:6.3}");
    }

    println!("\nbest emitted/pass: {:.3} at k={} ({})", best.1, best.0, best.2);
    println!("k=1 baseline was {base:.3}; the whole point of a drafter is the gap above it.");
    println!("\nWhat this does and does not say:");
    println!("  measured  — the acceptance curve, which depends only on the token");
    println!("              sequence and the drafter, so it holds on any machine.");
    println!("  NOT here  — what the curve is worth. A larger k costs more verify");
    println!("              compute; on a submit-bound backend that cost is close to");
    println!("              zero and the optimum is where acceptance flattens, on a");
    println!("              compute-bound one it is lower. Same curve, two readings.");
}
