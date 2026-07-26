fn main() {
    use logscope::gpumetrics::*;
    let mut t = Timeline::new(4096);
    // 8 layers x 6 tokens; layers 2 and 5 stream experts, every layer pays a router sync
    for tok in 0..6u32 { for l in 0..8u32 {
        t.record(Span { token: tok, layer: l, phase: Phase::Compute, ns: 900_000, bytes: 0 });
        t.record(Span { token: tok, layer: l, phase: Phase::Sync, ns: 2_800_000, bytes: 0 });
        t.record(Span { token: tok, layer: l, phase: Phase::Submit, ns: 120_000, bytes: 0 });
        if l == 2 || l == 5 { t.record(Span { token: tok, layer: l, phase: Phase::Upload, ns: 18_000_000, bytes: 68<<20 }); }
        if l == 5 && tok % 3 == 0 { t.record(Span { token: tok, layer: l, phase: Phase::Convert, ns: 4_000_000, bytes: 68<<20 }); }
    }}
    for line in t.summary() { println!("{line}"); }
    println!();
    for line in t.render_heatmap(Phase::Upload, 8, 6) { println!("{line}"); }
    println!();
    for line in t.render_heatmap(Phase::Sync, 8, 6) { println!("{line}"); }
    println!("\nhot upload layers: {:?}", t.hot_layers(Phase::Upload, 8, 2.0));
    println!("chrome trace bytes: {}", t.to_chrome_trace().len());
}
