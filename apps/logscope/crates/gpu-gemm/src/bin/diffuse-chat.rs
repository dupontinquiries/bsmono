//! diffuse-chat — run a prompt through a diffusion model and print DECODED TEXT.
//!
//! Every other binary here emits token ids or statistics. Nothing applied a chat template and
//! decoded the result, so scoring a diffusion model on the agent-protocol suite meant pasting
//! fifteen prompts into the GUI by hand. This closes that loop.
//!
//!   diffuse-chat --model m.gguf --prompt "..."            one prompt to stdout
//!   diffuse-chat --model m.gguf --suite <dir>             read <dir>/*.prompt, write <dir>/*.txt
//!
//! The suite mode writes exactly the filenames `agent-score --score` expects, so:
//!
//!   cargo run -p logscope-fan --bin agent-score -- --prompts > /tmp/q/cases.tsv
//!   diffuse-chat --model llada2-q4_0.gguf --suite /tmp/q
//!   cargo run -p logscope-fan --bin agent-score -- --score /tmp/q
//!
//! Schedule and sampling come from the same environment variables the app uses
//! (LOGSCOPE_DIFFUSION_STEPS / _BLOCK / _TAU / _RANK / _COLD_GREEDY), so a score is attributable to
//! a configuration rather than to whatever the GUI happened to be set to.
use gpu_kernels::{Gpu, SamplerParams};

fn env_usize(k: &str, d: usize) -> usize {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

/// Wrap a bare instruction in the model's own turn markers.
///
/// A diffusion model with no framing continues the prompt instead of answering it, which reads as
/// the model being incapable when it is merely being asked the wrong question — the failure the
/// TEMPLATE MISMATCH alarm exists to catch. Bailing2/LLaDA2 markers are the default because that is
/// what this family ships; --raw sends the text through untouched.
fn frame(prompt: &str, raw: bool) -> String {
    if raw { return prompt.to_string(); }
    format!("<role>HUMAN</role>{prompt}<|role_end|><role>ASSISTANT</role>")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| -> Option<String> {
        args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned()
    };
    let Some(model_path) = get("--model") else {
        eprintln!("usage: diffuse-chat --model <gguf> (--prompt <text> | --suite <dir>) [--raw] [--fill N]");
        std::process::exit(2);
    };
    let raw = args.iter().any(|a| a == "--raw");
    let fill = get("--fill").and_then(|v| v.parse().ok()).unwrap_or(256usize);

    let bytes = std::fs::read(&model_path).unwrap_or_else(|e| { eprintln!("{model_path}: {e}"); std::process::exit(2) });
    let gpu = Gpu::new();
    let m = gpu.load_gguf_opt(&bytes, true).unwrap_or_else(|e| { eprintln!("load: {e}"); std::process::exit(2) });
    if m.mask_token_id.is_none() {
        eprintln!("this model has no mask_token_id — it cannot run the diffusion path. If it was \
converted here, tools/llada2-to-gguf.py writes tokenizer.ggml.mask_token_id; older files need it \
grafted.");
        std::process::exit(2);
    }
    // The tokenizer is built from the GGUF, not carried on Model — the same two-step the app does.
    let g = gpu_kernels::gguf::parse(&bytes).unwrap_or_else(|e| { eprintln!("gguf: {e}"); std::process::exit(2) });
    let tok = gpu_kernels::tokenizer::Tokenizer::from_gguf(&g)
        .unwrap_or_else(|e| { eprintln!("tokenizer: {e}"); std::process::exit(2) });

    let block = env_usize("LOGSCOPE_DIFFUSION_BLOCK", 32);
    let steps = env_usize("LOGSCOPE_DIFFUSION_STEPS", block);
    let tau = std::env::var("LOGSCOPE_DIFFUSION_TAU").ok()
        .and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.95).clamp(0.0, 1.0);
    let tau = if tau <= 0.0 { None } else { Some(tau) };
    let sp = SamplerParams { temperature: 0.35, top_k: 40, top_p: 0.95,
                             repeat_penalty: 1.1, repeat_last_n: 64, min_p: 0.0 };
    eprintln!("diffuse-chat: block {block}, {steps} step(s) ({:.1} token(s)/commit), tau {tau:?}, fill {fill}",
              block as f32 / steps.min(block).max(1) as f32);

    let run = |text: &str| -> String {
        let ids = tok.encode(&frame(text, raw));
        // An empty tokenisation reaches the engine as a zero-length canvas and surfaces as
        // "Buffer with 'tensor' label binding size is zero" — a device error for what is really a
        // vocabulary mismatch. Say which it is.
        if ids.is_empty() {
            eprintln!("prompt tokenised to NOTHING with this model's vocabulary ({} tokens). Either \
the text is empty or this tokenizer cannot represent it — a fixture vocabulary will do that to \
English.", m.vocab);
            return String::new();
        }
        // Stop on any special the tokenizer knows, which is how the app builds its stop set.
        let stop: Vec<u32> = (0..m.vocab as u32).filter(|t| tok.is_special(*t)).collect();
        let mut rng = 0xC0FFEEu64;
        match gpu.diffuse_generate_sampled(&m, &ids, fill, steps, block, tau, &stop,
                                           Some(&sp), &mut rng, &|| false, &mut |_| true) {
            // Specials are dropped the way the streaming path drops them, so the scored text is
            // what a reader would have seen rather than what the loop produced.
            Ok(out) => tok.decode(&out.iter().copied().filter(|t| !tok.is_special(*t))
                                      .collect::<Vec<_>>()),
            Err(e) => { eprintln!("generate: {e}"); String::new() }
        }
    };

    if let Some(p) = get("--prompt") {
        println!("{}", run(&p));
        return;
    }
    let Some(dir) = get("--suite") else {
        eprintln!("need --prompt or --suite"); std::process::exit(2);
    };
    // agent-score --prompts emits "<case name>\t<prompt>" per line.
    let tsv = std::fs::read_to_string(format!("{dir}/cases.tsv"))
        .unwrap_or_else(|e| { eprintln!("{dir}/cases.tsv: {e} — generate it with \
`agent-score --prompts > {dir}/cases.tsv`"); std::process::exit(2) });
    for line in tsv.lines() {
        let Some((name, prompt)) = line.split_once('\t') else { continue };
        let out = run(prompt);
        let path = format!("{dir}/{}.txt", name.replace(' ', "_"));
        if let Err(e) = std::fs::write(&path, &out) { eprintln!("{path}: {e}"); }
        eprintln!("  {name}: {} chars", out.len());
    }
    eprintln!("done — score with: agent-score --score {dir}");
}
