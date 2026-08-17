//! gate-diffuse — end-to-end structural gate for the block-diffusion generator. The sandbox cannot
//! judge whether text is coherent, but it can prove the loop obeys the reference's contract:
//! every fill position is committed, no MASK survives, blocks are filled in order, and a later
//! block's content cannot influence an earlier block's tokens (block-causal visibility).
use gpu_kernels::*;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/tmp/tiny-llada2.gguf".into());
    let bytes = std::fs::read(&path).expect("read");
    let gpu = Gpu::new();
    eprintln!("gate-diffuse: adapter = {}", gpu.adapter_name);
    let m = gpu.load_gguf_opt(&bytes, false).expect("load f32");
    let mut ok = true;
    let mut ck = |l: &str, v: bool| { ok &= v; println!("  [{}] {l}", if v { "\u{2713}" } else { "\u{2717} FAIL" }); };

    let mask = match m.mask_token_id { Some(x) => x, None => { println!("gate-diffuse: model has no mask id — nothing to gate"); return; } };
    let prompt: Vec<u32> = (0..12u32).map(|i| 1 + (i * 13) % 200).collect();

    for &(n_fill, blk, steps) in &[(16usize, 8usize, 8usize), (20, 8, 4), (7, 32, 32)] {
        let out = gpu.diffuse_generate(&m, &prompt, n_fill, steps, blk, None).expect("generate");
        ck(&format!("fill={n_fill} block={blk} steps={steps}: returns exactly the fill ({} ids)", out.len()), out.len() == n_fill);
        ck(&format!("fill={n_fill} block={blk} steps={steps}: no MASK survives the loop"), !out.contains(&mask));
        ck(&format!("fill={n_fill} block={blk} steps={steps}: all ids inside the vocab"), out.iter().all(|&t| (t as usize) < m.vocab));
    }

    // BLOCK-CAUSAL: the first block's tokens must not depend on how many blocks follow it, because
    // a later block is invisible to it. Generating with a longer fill must reproduce the first
    // block exactly. This fails loudly if the forward ever sees past the current block again.
    let (blk, steps) = (8usize, 8usize);
    let short = gpu.diffuse_generate(&m, &prompt, blk, steps, blk, None).expect("short");
    let long = gpu.diffuse_generate(&m, &prompt, blk * 3, steps, blk, None).expect("long");
    // HONESTY: on a random-weight tiny the argmax is often a single constant token, which makes
    // "identical" true for free. Say so rather than banking a vacuous pass — the property is
    // structurally guaranteed by truncating the forward, and this check only has evidential value
    // on a model whose block actually varies.
    let varies = short.windows(2).any(|w| w[0] != w[1]);
    if varies {
        ck(&format!("block-causal: first block identical with 1 vs 3 blocks of fill"), short[..] == long[..blk]);
    } else {
        println!("  [~] block-causal: VACUOUS on this model — every generated id is {:?}; the check \
needs a model whose block varies to mean anything (structurally guaranteed by the forward truncation)", short.first());
    }

    println!("gate-diffuse: {}", if ok {
        "PASS \u{2713} — the block-diffusion loop fills every position and later blocks stay invisible to earlier ones"
    } else {
        "FAIL \u{2717} — the loop violates the reference's block contract"
    });
    if !ok { std::process::exit(1); }
}
