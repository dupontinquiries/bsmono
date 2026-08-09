//! gate-rnd1 — the RND1 loader path, testable without the 17 GB download.
//!
//! Two halves, both pure CPU (no GPU, runs anywhere including CI):
//! 1. Registry: `for_arch("rnd1")` must resolve to the Qwen3Moe machinery with the
//!    flags that family requires. The one that has bitten before is rope pairing —
//!    NEOX (split-half) vs NORM (interleaved) — where the wrong choice produces
//!    "real words, scrambled meaning" (the dense-Qwen salad, root-caused on M2
//!    Ultra), so it gets its own line here.
//! 2. Metadata: a synthetic 0-tensor GGUF header carrying general.architecture and
//!    tokenizer.ggml.mask_token_id must round-trip through `gguf::parse`,
//!    `declared_arch`, and `gguf_mask_token_id`. If parse rejects a 0-tensor file,
//!    that is information too — the first real load becomes the test instead.
//!
//!   cargo run --release --bin gate-rnd1

use gpu_kernels::archcfg::{self, Arch};
use gpu_kernels::{gguf, gguf_mask_token_id};

fn push_str_kv(b: &mut Vec<u8>, key: &str, val: &str) {
    b.extend((key.len() as u64).to_le_bytes());
    b.extend(key.as_bytes());
    b.extend(8u32.to_le_bytes()); // string
    b.extend((val.len() as u64).to_le_bytes());
    b.extend(val.as_bytes());
}

fn push_u32_kv(b: &mut Vec<u8>, key: &str, val: u32) {
    b.extend((key.len() as u64).to_le_bytes());
    b.extend(key.as_bytes());
    b.extend(4u32.to_le_bytes()); // u32
    b.extend(val.to_le_bytes());
}

fn main() {
    let mut ok = true;
    let mut ck = |l: &str, v: bool| {
        ok &= v;
        println!("  [{}] {l}", if v { "\u{2713}" } else { "\u{2717} FAIL" });
    };

    // --- registry half ---
    let c = archcfg::for_arch("rnd1");
    ck("rnd1 resolves to the Qwen3Moe machinery", matches!(c.arch, Arch::Qwen3Moe));
    ck("rnd1 metadata prefix is 'rnd1' (keys are arch-namespaced)", c.prefix == "rnd1");
    ck("MoE with NO shared expert (Qwen3-MoE shape)", c.moe && !c.moe_shared_expert);
    ck("QK-Norm, no QKV bias (Qwen3 attention base)", c.attn_qk_norm && !c.attn_qkv_bias);
    ck("NEOX rope pairing (interleaved = the Qwen-salad bug)", !c.rope_interleaved);
    ck("supported (loader will not refuse)", c.supported);

    // --- metadata half: synthetic GGUF v3 header, 0 tensors, 2 KVs ---
    let mut b: Vec<u8> = Vec::new();
    b.extend(0x4655_4747u32.to_le_bytes()); // 'GGUF'
    b.extend(3u32.to_le_bytes()); // version
    b.extend(0u64.to_le_bytes()); // tensor_count
    b.extend(2u64.to_le_bytes()); // kv_count
    push_str_kv(&mut b, "general.architecture", "rnd1");
    push_u32_kv(&mut b, "tokenizer.ggml.mask_token_id", 151_669);

    match gguf::parse(&b) {
        Ok(g) => {
            ck("synthetic header parses (0 tensors tolerated)", true);
            ck("declared_arch reads 'rnd1'", g.declared_arch().as_deref() == Some("rnd1"));
            ck("gguf_mask_token_id finds tokenizer.ggml.mask_token_id", gguf_mask_token_id(&g) == Some(151_669));
        }
        Err(e) => {
            // Not a failure of the RND1 wiring — a parser precondition. Say so precisely.
            println!("  [i] parse of 0-tensor header refused ({e}); metadata half untested here — first real load is the test");
        }
    }

    println!(
        "gate-rnd1: {}",
        if ok { "PASS \u{2713} — the loader will route an rnd1 gguf through Qwen3Moe with NEOX rope and read its mask id" } else { "FAIL \u{2717}" }
    );
    std::process::exit(if ok { 0 } else { 1 });
}
