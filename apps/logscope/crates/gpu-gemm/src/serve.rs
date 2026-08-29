//! logscope-serve — expose the wgpu inference engine on the LAN, authenticated by API key.
//!
//! Usage:
//!   logscope-serve --model <model.gguf> [--addr 0.0.0.0:8080] [--key <KEY> ...] [--keyfile <path>]
//!                   [--unix [path]] [--idle SECS]
//!   --unix   also serve local apps over a unix socket (they share these weights)
//!   --idle   seconds of no local request before releasing the weights (0 = never, default 600)
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

    // THE LOCAL DOOR, opened alongside the TCP one rather than instead of
    // it. `--unix` with no value takes the conventional path, so an app
    // and the daemon agree on where to meet without either being
    // configured; a value overrides it for a second instance or a test.
    //
    // Served on a thread because both listeners block: a daemon that can
    // only be reached one way is the thing this flag exists to stop.
    let mut life_for_tcp: Option<netserve::LifeHandle> = None;
    let unix_path = args.iter().position(|a| a == "--unix").map(|i| {
        args.get(i + 1)
            .filter(|v| !v.starts_with("--"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(default_unix_path)
    });
    if let Some(path) = unix_path.clone() {
        let (keys2, gen2) = (keys.clone(), gen.clone());
        // IDLE-UNLOAD, and only on the unix door. A daemon holding 7.2
        // GiB after every local app has closed is worse than no daemon —
        // it took the memory the loader already clamps and gave nothing
        // back. `--idle 0` disables it, for a long-lived server where
        // paying the 86-second load again is the worse trade.
        let idle_secs: u64 = val("--idle").and_then(|v| v.parse().ok()).unwrap_or(600);
        let life = std::sync::Arc::new(std::sync::Mutex::new(
            gpu_kernels::daemonlife::Life::new(std::time::Duration::from_secs(idle_secs)),
        ));
        let life2 = life.clone();
        life_for_tcp = Some(life.clone());
        if idle_secs > 0 {
            let started = std::time::Instant::now();
            let sock = path.clone();
            std::thread::spawn(move || {
                gpu_kernels::daemonlife::janitor(
                    life,
                    started,
                    std::time::Duration::from_secs(5),
                    || {
                        eprintln!("idle for {idle_secs}s — releasing the weights and exiting");
                        // Remove the socket before exiting so the next
                        // client's probe fails fast instead of connecting
                        // to a path nothing is listening on.
                        let _ = std::fs::remove_file(&sock);
                        std::process::exit(0);
                    },
                );
            });
        }
        std::thread::spawn(move || {
            eprintln!("serving on unix://{}  (local apps share these weights)", path.display());
            // Empty keyring: the filesystem authorises local callers.
            // `Keyring::accepts` returns true on an empty ring, so this
            // is not a bypass bolted on — it is the ring being empty.
            let _ = keys2;
            if let Err(e) = netserve::serve_unix_tracked(
                &path, netserve::Keyring::new(vec![]), gen2, Some(life2))
            {
                eprintln!("unix socket {}: {e}", path.display());
            }
        });
    }

    let listener = std::net::TcpListener::bind(&addr).unwrap_or_else(|e| { eprintln!("bind {addr}: {e}"); std::process::exit(1); });
    eprintln!("serving on http://{addr}  (POST /generate, GET /health)");
    // The TCP door reports to the same `Life`. A daemon that exited
    // because its unix socket was quiet, while a LAN client was
    // mid-conversation on the TCP port, would be a bug wearing a
    // feature's clothes.
    netserve::serve_tracked(listener, netserve::Keyring::new(keys), gen, life_for_tcp);
}

/// Where the unix socket lives when `--unix` is given no value.
///
/// Delegates to the library rather than repeating the rule: two copies
/// is how the daemon and its clients end up looking in different places,
/// each concluding the other is not running.
fn default_unix_path() -> std::path::PathBuf {
    netserve::default_socket_path()
}
