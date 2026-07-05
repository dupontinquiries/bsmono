//! logscope-cli — run a headless command-script through the conversation-graph runner. Drives the SAME
//! command language as the GUI against either a deterministic MOCK backend (works anywhere, for testing
//! the orchestration + building the suite off-GPU) or the real wgpu engine (`--model GGUF`, needs a build
//! with `--features wgpu-engine`, for capturing real outputs on hardware).
//!
//! Output discipline: requested outputs (`:export`, `:best`, `:fill` assemblies) go to STDOUT; progress
//! notes + the final tree summary go to STDERR. So `logscope-cli script.logscope > out.md` captures just
//! the artifacts and you can diff them across runs to build a regression suite.
//!
//!   logscope-cli --mock examples/sort.logscope > runs/sort.mock.md
//!   logscope-cli --model ~/models/qwen2.5-coder-3b-q8.gguf examples/sort.logscope > runs/sort.real.md

use logscope_egui::backend::{Backend, TemplateKind};
use logscope_egui::runner::Runner;
use logscope_egui::script::parse_script;
use logscope::llm_backend::GenParams;

/// Deterministic stand-in for a model: enough structure (a code block per reply, a body for fill
/// sub-tasks) to exercise every command path without a GPU. Same prompt → same reply. Carries an `id` so a
/// multi-model pool of mocks tags which one replied — that's how `:use` switching is visible in a test.
struct MockBackend { id: String, tmpl: TemplateKind }
impl Backend for MockBackend {
    fn name(&self) -> &'static str { "mock" }
    fn loaded(&self) -> bool { true }
    fn load(&mut self, _p: &str, _params: &GenParams) -> Result<(), String> { Ok(()) }
    fn generate(&mut self, prompt: &str, _params: &GenParams, _cancel: &dyn Fn() -> bool, on: &mut dyn FnMut(&str) -> bool) -> Result<usize, String> {
        let reply = mock_reply(prompt, &self.id);
        on(&reply);
        Ok(reply.split_whitespace().count())
    }
    fn template(&self) -> TemplateKind { self.tmpl }
}
fn mock_reply(prompt: &str, id: &str) -> String {
    if prompt.contains("clarifying questions") {
        return "Here are the key questions:\n\
                1. Which data structure should back it? (Vec / HashMap / BTreeMap)\n\
                2. Should it be thread-safe?\n\
                3. What is the eviction policy when full?".to_string();
    }
    if prompt.contains("Implement ONLY the body") {
        "{ /* mock-filled */ Default::default() }".to_string()      // a plausible body for :fill (must parse cleanly)
    } else {
        let h = prompt.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
        let n = h % 3;
        format!("[model:{id}] Mock approach #{n}.\n```rust\nfn step_{n}() {{ /* sketch */ }}\n```")
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // arg parsing via cli-kit (shared across the fleet's CLIs)
    let cli = cli_kit::Cli::from(args.iter().skip(1).cloned().collect::<Vec<String>>());
    let mut model_args: Vec<String> = cli.opt_all("model").iter().map(|s| s.to_string()).collect();
    let mock = cli.flag("mock");
    let system = cli
        .opt("system")
        .map(|s| s.to_string())
        .unwrap_or_else(|| "You are a terse Rust coding assistant. Prefer code, keep replies short.".to_string());
    let max_tokens = cli.opt("max").and_then(|s| s.parse().ok()).unwrap_or(512usize); // reasoning models (R1-Distill) need 1024-2048
    let final_fmt = cli.opt("final").unwrap_or("md").to_string();
    let check_cmd = cli.opt("check").map(|s| s.to_string());        // shell cmd that compiles a node's code
    let check_file = cli.opt("check-file").map(|s| s.to_string());  // where to drop the code (for a real-crate check)
    let no_kv = cli.flag("no-kv");                                  // disable the KV-prefix-tree speedup
    let kv_tree = cli.opt("kv-tree").map(|s| s.to_string());        // keep N branch tips warm (the prefix tree)
    let script_path = cli
        .positionals_with_values(&["model", "system", "max", "final", "check", "check-file", "kv-tree"])
        .first()
        .map(|s| s.to_string());
    if no_kv { std::env::set_var("LOGSCOPE_KV_OFF", "1"); std::env::remove_var("LOGSCOPE_KV_TREE"); }
    else if let Some(n) = kv_tree.as_ref() { std::env::set_var("LOGSCOPE_KV_TREE", n); }
    let script_path = script_path.unwrap_or_else(|| {
        eprintln!("usage: logscope-cli [--model NAME=GGUF ...] [--mock] [--system TEXT] [--max N] [--kv-tree N|--no-kv] [--check CMD] [--final md|ascii|git] <script.logscope>");
        eprintln!("  --model can repeat: --model big=~/big.gguf --model small=~/small.gguf  then `:use big` / `:use small` in the script (a bare path is named 'default'; =mock makes a stub)");
        std::process::exit(2);
    });
    let text = std::fs::read_to_string(&script_path).unwrap_or_else(|e| { eprintln!("can't read {script_path}: {e}"); std::process::exit(2); });
    let steps = parse_script(&text);

    let mut params = GenParams::default();
    params.max_tokens = max_tokens;
    params.reply_ceiling = max_tokens;

    // Build the model pool. Each --model is `name=path`; a bare path is named "default"; path "mock" (or a
    // --mock with no models) becomes a named MockBackend. `:use <name>` in the script switches among them.
    if model_args.is_empty() { eprintln!("# backend: mock (no --model given)"); model_args.push("default=mock".into()); }
    else if mock { model_args.push("mock=mock".into()); } // --mock alongside real models adds a mock to compare against
    let mut pool: Vec<(String, Box<dyn Backend>)> = Vec::new();
    for ma in &model_args {
        let (name, path) = match ma.split_once('=') { Some((n, p)) => (n.trim().to_string(), p.trim().to_string()), None => ("default".to_string(), ma.clone()) };
        if pool.iter().any(|(n, _)| n == &name) { eprintln!("# duplicate model name '{name}' — skipping"); continue; }
        if path == "mock" {
            eprintln!("# model '{name}': mock");
            pool.push((name.clone(), Box::new(MockBackend { id: name, tmpl: TemplateKind::ChatML })));
        } else {
            #[cfg(feature = "wgpu-engine")]
            {
                let mut b = logscope_egui::backend::make(logscope_egui::backend::BackendKind::Wgpu);
                if let Err(e) = b.load(&path, &params) { eprintln!("# model '{name}' load failed: {e}"); std::process::exit(1); }
                eprintln!("# model '{name}': {} · {}", b.name(), path);
                pool.push((name, b));
            }
            #[cfg(not(feature = "wgpu-engine"))]
            { let _ = (&name, &path); eprintln!("# --model <name>=<path> needs a build with --features wgpu-engine (or use <name>=mock)"); std::process::exit(1); }
        }
    }
    eprintln!("# pool: {} model(s) — {}", pool.len(), pool.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "));

    eprintln!("# running {} steps from {}", steps.len(), script_path);
    let mut runner = Runner::new(pool, system, params);
    if let Some(c) = check_cmd { runner.check_cmd = c; }
    runner.check_file = check_file;
    for step in &steps {
        if let Some(out) = runner.run_step(step) {
            println!("{out}\n");
        }
    }
    for line in &runner.log { eprintln!("{line}"); }
    if runner.gen_count > 0 {
        let total = runner.gen_time.as_secs_f64();
        eprintln!("# gen: {} calls, {:.2}s total, {:.0} ms/call avg{}",
            runner.gen_count, total, total * 1000.0 / runner.gen_count as f64,
            if no_kv { "   [KV reuse OFF — full prefill each turn]" } else { "" });
    }
    eprintln!("# final tree: {} nodes", runner.nodes.len());
    println!("===== FINAL TREE ({final_fmt}) =====");
    println!("{}", runner.export(&final_fmt));
}
