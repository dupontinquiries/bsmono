//! denoise-dump — print denoise_stats for every position: "id p margin entropy", one line per
//! token, in the same format as tools/oracle/llada2_ref.py so the two diff directly. This is the
//! engine side of the BIDIRECTIONAL oracle; the causal oracle (llama.cpp) cannot cover this path
//! because llada2 has never landed upstream.
use gpu_kernels::*;
fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/tmp/tiny-llada2.gguf".into());
    let t: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(16);
    let bytes = std::fs::read(&path).expect("read");
    let gpu = Gpu::new();
    // Third arg "quant" loads exactly as the app does (int8/q4 resident) — the oracle's f32 mode
    // isolates kernel correctness, this mode prices what quantisation actually costs on the
    // bidirectional path, which no gate has ever measured.
    let quant = std::env::args().nth(3).as_deref() == Some("quant");
    let m = gpu.load_gguf_opt(&bytes, quant).expect("load");
    // Explicit ids via LOGSCOPE_IDS (comma-separated) so the oracle can be pointed at sequences
    // that occur MID-LOOP — in particular ones containing MASK tokens, which the fixed synthetic
    // pattern never produces and which are exactly what a denoise step actually sees.
    let toks: Vec<u32> = match std::env::var("LOGSCOPE_IDS") {
        Ok(v) => v.split(',').filter_map(|x| x.trim().parse().ok()).collect(),
        Err(_) => (0..t as u32).map(|i| 1 + (i * 7) % 240).collect(),
    };
    // LOGSCOPE_BC="bcz,bco" runs the block-causal mask, so the oracle can compare the engine's
    // masked forward against the reference's — the comparison that was impossible while both sides
    // truncated and therefore shared the same assumption.
    let s = match std::env::var("LOGSCOPE_BC") {
        Ok(v) => {
            let p: Vec<usize> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            let (bcz, bco) = (p.first().copied().unwrap_or(0), p.get(1).copied().unwrap_or(0));
            gpu.denoise_stats_rows_bc(&m, &toks, 0, toks.len(), bcz, bco)
        }
        Err(_) => gpu.denoise_stats(&m, &toks),
    };
    for r in s.chunks_exact(4).take(t) {
        println!("{} {:.9} {:.9} {:.9}", r[0] as u32, r[1], r[2], r[3]);
    }
}
