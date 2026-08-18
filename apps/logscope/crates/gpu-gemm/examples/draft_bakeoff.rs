//! draft_bakeoff.rs — two prompt-lookup drafters ship in this crate and only one is wired.
//!
//! `spec::PromptLookup` takes a FIXED n-gram (default 2) and returns what followed its most recent
//! earlier occurrence. `lookupdraft::draft` searches suffix lengths 8 down to 3 and takes the first
//! hit, on the argument that a longer match is rarer and much likelier to continue correctly. The
//! backend calls the first one. Nothing has ever compared them.
//!
//! The table in `PromptLookup::default`'s doc compared ng=2 against ng=3 and ng=4 — three FIXED
//! lengths — and concluded 2 wins because it matches more often and a wrong proposal costs only a
//! verification slot. That is a sound argument about fixed lengths and says nothing about
//! longest-match-first, which is not a fixed length: it gets a long match's precision when one
//! exists and falls back when it does not.
//!
//! Acceptance depends only on the token sequence, never on the hardware, so this is answerable
//! without a GPU by replaying real text — the corpus is both the context and the ground truth for
//! what comes next. Same method as the existing table, so the numbers are comparable to it.
//!
//! ## The proxy, named
//!
//! Two things here are not the real thing. Replay models a generation that ECHOES its context,
//! which is what repository reading looks like and is not what prose looks like, so absolute
//! numbers are an upper bound. And there is no BPE tokenizer in this crate, so text is split on
//! word and punctuation boundaries — coarser than BPE on prose, close to it on code, where
//! identifiers and delimiters are the tokens either way. Both drafters see exactly the same
//! sequence, so the COMPARISON holds even where the absolute figures drift.
//!
//! Run: `cargo run --release --example draft_bakeoff -- <dir>...`

use gpu_kernels::spec::{Drafter, PromptLookup};
use std::collections::HashMap;

/// Split into word / punctuation / whitespace-run units and intern to ids.
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

/// Tokens emitted per verification pass. 1.00 means the drafter paid for nothing.
///
/// Mirrors `spec.rs`'s own `replay`: the drafter proposes from the committed context, the corpus
/// says what actually came next, the matching prefix plus one correction commits.
fn replay(toks: &[u32], k: usize, prompt_len: usize, draft: &mut dyn FnMut(&[u32], usize) -> Vec<u32>) -> f64 {
    if toks.len() <= prompt_len { return 1.0; }
    let mut ctx: Vec<u32> = toks[..prompt_len].to_vec();
    let (mut i, mut rounds, mut emitted) = (prompt_len, 0usize, 0usize);
    while i < toks.len() {
        let d = draft(&ctx, k);
        rounds += 1;
        let mut j = 0;
        while j < d.len() && i + j < toks.len() && d[j] == toks[i + j] { j += 1; }
        let take = (j + 1).min(toks.len() - i);
        ctx.extend_from_slice(&toks[i..i + take]);
        i += take;
        emitted += take;
    }
    emitted as f64 / rounds.max(1) as f64
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
    let cap: usize = std::env::var("CAP").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
    // Replay is O(context x corpus) per round — the drafter rescans the whole committed context
    // every proposal — so a 56k-line file is minutes on its own. Truncating keeps every file
    // contributing instead of one dominating the wall clock, and both drafters see the same prefix.
    let tcap: usize = std::env::var("TCAP").ok().and_then(|s| s.parse().ok()).unwrap_or(3000);

    let mut files = Vec::new();
    for d in &dirs { collect(std::path::Path::new(d), &mut files, cap); }
    if files.is_empty() { eprintln!("no .rs files under {dirs:?}"); return; }

    println!("{} files. tokens emitted per verification pass; 1.00 = the drafter paid for nothing.\n",
             files.len());
    println!("{:>4} {:>14} {:>14} {:>14} {:>10}", "k", "fixed ng=2", "fixed ng=3", "longest 8..3", "best");
    println!("{}", "-".repeat(60));

    for k in [2usize, 4, 7] {
        let (mut a, mut b, mut c, mut n) = (0f64, 0f64, 0f64, 0usize);
        for (_, src) in &files {
            let mut vocab = HashMap::new();
            let mut toks = tokenize(src, &mut vocab);
            toks.truncate(tcap);
            if toks.len() < 400 { continue; }
            let plen = toks.len() / 4;
            let mut d2 = PromptLookup { ngram: 2 };
            let mut d3 = PromptLookup { ngram: 3 };
            a += replay(&toks, k, plen, &mut |c, n| d2.draft(c, n));
            b += replay(&toks, k, plen, &mut |c, n| d3.draft(c, n));
            c += replay(&toks, k, plen, &mut |ctx, n| gpu_kernels::lookupdraft::draft(ctx, n));
            n += 1;
        }
        if n == 0 { continue; }
        let (a, b, c) = (a / n as f64, b / n as f64, c / n as f64);
        let best = if c >= a && c >= b { "longest" } else if a >= b { "ng=2" } else { "ng=3" };
        println!("{k:>4} {a:>14.3} {b:>14.3} {c:>14.3} {best:>10}");
    }

    println!("\nk=7 is the ceiling: a draft of k verifies k+1 rows and the batched matvec stops at 8.");
    println!("Projected tok/s from the operator's measured 7.17 (latency-bound, free drafter):");
    for k in [4usize, 7] {
        let mut acc = 0f64;
        let mut n = 0usize;
        for (_, src) in &files {
            let mut v = HashMap::new();
            let mut toks = tokenize(src, &mut v);
            toks.truncate(tcap);
            if toks.len() < 400 { continue; }
            acc += replay(&toks, k, toks.len() / 4, &mut |ctx, m| gpu_kernels::lookupdraft::draft(ctx, m));
            n += 1;
        }
        if n > 0 { println!("  longest-match k={k}: {:.2}x -> {:.1} tok/s", acc / n as f64, 7.17 * acc / n as f64); }
    }
}
