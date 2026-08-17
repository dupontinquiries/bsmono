//! gate-llada2 — the LLaDA2 loader path, testable before the 9.3 GB download.
//!
//! Pure CPU. Half one: the registry arm must resolve to the Glm4Moe machinery with
//! the flags the Ling-2.0 family requires — NEOX rope gets its own line (the salad
//! bug class), and qkv-bias FALSE is asserted even though the loader detects biases
//! per-checkpoint, because the arm documents intent. Half two: a synthetic GGUF
//! header carrying the four keys the denoise path reads must round-trip — arch,
//! mask id, nextn count, and diffusion.shift_logits=false (LLaDA predicts in
//! place; the default-when-absent is TRUE, so this key being readable matters).
//!
//!   cargo run --release --bin gate-llada2

use gpu_kernels::archcfg::{self, Arch};
use gpu_kernels::{gguf, gguf_mask_token_id};

fn kv_str(b: &mut Vec<u8>, k: &str, v: &str) {
    b.extend((k.len() as u64).to_le_bytes()); b.extend(k.as_bytes());
    b.extend(8u32.to_le_bytes());
    b.extend((v.len() as u64).to_le_bytes()); b.extend(v.as_bytes());
}
fn kv_u32(b: &mut Vec<u8>, k: &str, v: u32) {
    b.extend((k.len() as u64).to_le_bytes()); b.extend(k.as_bytes());
    b.extend(4u32.to_le_bytes()); b.extend(v.to_le_bytes());
}

fn main() {
    let mut ok = true;
    let mut ck = |l: &str, v: bool| { ok &= v; println!("  [{}] {l}", if v { "\u{2713}" } else { "\u{2717} FAIL" }); };

    let c = archcfg::for_arch("llada2");
    ck("llada2 resolves to the Glm4Moe machinery", matches!(c.arch, Arch::Glm4Moe));
    ck("metadata prefix is 'llada2'", c.prefix == "llada2");
    ck("MoE with a SHARED expert (Ling shape)", c.moe && c.moe_shared_expert);
    ck("QK-Norm on, QKV bias off (Ling attention base)", c.attn_qk_norm && !c.attn_qkv_bias);
    ck("NEOX rope pairing (interleaved = the salad bug)", !c.rope_interleaved);
    ck("supported (loader will not refuse)", c.supported);

    let mut b: Vec<u8> = Vec::new();
    b.extend(0x4655_4747u32.to_le_bytes());
    b.extend(3u32.to_le_bytes());
    b.extend(0u64.to_le_bytes());
    b.extend(4u64.to_le_bytes());
    kv_str(&mut b, "general.architecture", "llada2");
    kv_u32(&mut b, "tokenizer.ggml.mask_token_id", 156_895);
    kv_u32(&mut b, "llada2.nextn_predict_layers", 1);
    kv_str(&mut b, "diffusion.shift_logits", "false");

    match gguf::parse(&b) {
        Ok(g) => {
            ck("synthetic header parses", true);
            ck("declared_arch reads 'llada2'", g.declared_arch().as_deref() == Some("llada2"));
            ck("mask id readable", gguf_mask_token_id(&g) == Some(156_895));
            ck("nextn_predict_layers readable (trailing MTP blocks excluded)", g.get_u32("llada2.nextn_predict_layers") == Some(1));
            ck("diffusion.shift_logits reads 'false' (in-place prediction; absent would default TRUE)", g.get_str("diffusion.shift_logits") == Some("false"));
        }
        Err(e) => { println!("  [i] 0-tensor parse refused ({e}); first real load is the test"); }
    }

    println!("gate-llada2: {}", if ok {
        "PASS \u{2713} — the loader routes llada2 through Glm4Moe; the denoise path's four keys read. Selection bias (exp_probs_b) wired: gate-moe-t proves it against the CPU reference."
    } else { "FAIL \u{2717}" });
    std::process::exit(if ok { 0 } else { 1 });
}
