// Extend: test a NORMALIZED dedup. Hypothesis: some oscillation is cosmetic
// (trailing whitespace, volatile attribute order, a redundant status field
// that doesn't reflect a real content change the USER cares about). Normalize
// before compare; measure re-parse reduction AND verify real changes survive.
fn normalize(html: &str) -> String {
    // collapse runs of whitespace, trim per-line trailing ws — cheap, removes
    // the most common cosmetic diffs without touching real content.
    html.lines().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n")
        .split_whitespace().collect::<Vec<_>>().join(" ")
}
fn reparse_normalized(frames: &[String]) -> usize {
    let mut last = String::new(); let mut n = 0;
    for f in frames { let nf = normalize(f); if nf != last { n+=1; last = nf; } }
    n
}
fn reparse_raw(frames: &[String]) -> usize {
    let mut last = String::new(); let mut n = 0;
    for f in frames { if *f != last { n+=1; last = f.clone(); } }
    n
}
fn main(){
    let body = "<span>fn main() {</span>".repeat(40);
    // Case A: oscillation is ONLY trailing whitespace (cosmetic) — normalize
    // should collapse it to ~1 re-parse.
    let mut a = Vec::new();
    for i in 0..600 { let pad = if i%2==0 {"  "} else {"   "};
        a.push(format!("{}{}", body, pad)); }
    println!("Case A — cosmetic trailing-ws oscillation:");
    println!("  raw: {} re-parses  normalized: {} re-parses", reparse_raw(&a), reparse_normalized(&a));

    // Case B: oscillation is a REAL statusline field (e.g. a clock / position).
    // normalize must NOT collapse it (it's a genuine content change). We
    // EXPECT it to stay high — proving normalization doesn't hide real diffs.
    let mut b = Vec::new();
    for i in 0..600 { b.push(format!("{}<span>{}</span>", body, 2536 + (i%2)*4)); }
    println!("Case B — real statusline field oscillation:");
    println!("  raw: {} re-parses  normalized: {} re-parses (SHOULD stay high — real change)",
        reparse_raw(&b), reparse_normalized(&b));

    // Conclusion gate: normalization only helps if the diff is cosmetic.
    println!("\nDiagnosis path: capture two consecutive 'idle' frames from the");
    println!("real WEB2QML_PROFILE run and diff them. If the only delta is");
    println!("whitespace/attr-order -> normalize (cheap win). If a real field");
    println!("animates -> the fix is to throttle/coalesce idle republishes or");
    println!("stop re-rendering rows whose logical content is unchanged.");
}
