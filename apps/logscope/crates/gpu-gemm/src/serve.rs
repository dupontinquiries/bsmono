//! logscope-serve — expose the wgpu inference engine on the LAN, authenticated by API key.
//!
//! Usage:
//!   logscope-serve --model <model.gguf> [--addr 0.0.0.0:8080] [--key <KEY> ...] [--keyfile <path>]
//!
//! The model loads once into a shared `Arc<Model>`; the pipelines are warmed up front; then
//! each connection is served on its own thread against that one resident model (the engine
//! is Send+Sync). If no key is supplied, one is generated and printed — clients must send it
//! as `Authorization: Bearer <key>` (or `X-API-Key: <key>`).
//!
//!   curl -s -H "Authorization: Bearer $KEY" --data 'why is the disk full?' \
//!        'http://<host>:8080/generate?max=128'
//!
//! Auth only — not transport encryption. Keep it on a trusted LAN, tunnel over SSH, or put
//! TLS in front (see SERVING.md).
use gpu_kernels::tokenizer::Tokenizer;
use gpu_kernels::{gguf, netserve, Gpu};
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let val = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();

    let model_path = match val("--model") {
        Some(p) => p,
        None => {
            eprintln!("usage: logscope-serve --model <gguf> [--addr 0.0.0.0:8080] [--key K]... [--keyfile F]");
            std::process::exit(2);
        }
    };
    let addr = val("--addr").unwrap_or_else(|| "0.0.0.0:8080".into());

    // collect every --key occurrence, plus optional newline-delimited --keyfile
    let mut keys: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--key" {
            if let Some(k) = args.get(i + 1) { keys.push(k.clone()); }
            i += 2;
        } else { i += 1; }
    }
    if let Some(kf) = val("--keyfile") {
        if let Ok(s) = std::fs::read_to_string(&kf) {
            for l in s.lines() { let l = l.trim(); if !l.is_empty() { keys.push(l.to_string()); } }
        }
    }
    if keys.is_empty() {
        let k = netserve::generate_key();
        eprintln!("no key supplied — generated one (clients MUST send it):\n  {k}\n");
        keys.push(k);
    }

    eprintln!("loading model: {model_path}");
    let bytes = std::fs::read(&model_path).unwrap_or_else(|e| { eprintln!("read {model_path}: {e}"); std::process::exit(1); });
    let g = gguf::parse(&bytes).unwrap_or_else(|e| { eprintln!("parse gguf: {e}"); std::process::exit(1); });
    let tok = Arc::new(Tokenizer::from_gguf(&g).unwrap_or_else(|e| { eprintln!("tokenizer: {e}"); std::process::exit(1); }));
    let gpu = Arc::new(Gpu::new());
    let model = Arc::new(gpu.load_gguf(&bytes).unwrap_or_else(|e| { eprintln!("load model: {e}"); std::process::exit(1); }));
    let warm = gpu.warm(&model);
    let eos = g.get_u32("tokenizer.ggml.eos_token_id").or_else(|| tok.special_id("<|eot_id|>"));
    eprintln!("adapter: {} | {warm} pipelines warm | eos={eos:?} | model resident", gpu.adapter_name);

    // one shared generation closure over the resident model (Gpu/Model are Send+Sync).
    // Greedy with a repetition penalty + eos stop — coherent, terminating completions.
    let gen: Arc<dyn Fn(&str, usize) -> String + Send + Sync> = {
        let (gpu, model, tok) = (gpu.clone(), model.clone(), tok.clone());
        Arc::new(move |prompt: &str, max: usize| {
            let ids = tok.encode(prompt);
            let sp = gpu_kernels::SamplerParams { temperature: 0.0, top_k: 1, top_p: 1.0, repeat_penalty: 1.15, repeat_last_n: 64, min_p: 0.0 };
            let mut rng = 0xC0FFEE_u64;
            let mut out: Vec<u32> = Vec::new();
            gpu.generate_kv_stream(&model, &ids, max, eos, &sp, &mut rng, &|| false, |t| { out.push(t); true });
            tok.decode(&out)
        })
    };

    let listener = std::net::TcpListener::bind(&addr).unwrap_or_else(|e| { eprintln!("bind {addr}: {e}"); std::process::exit(1); });
    eprintln!("serving on http://{addr}  (POST /generate, GET /health)");
    netserve::serve(listener, netserve::Keyring::new(keys), gen);
}
