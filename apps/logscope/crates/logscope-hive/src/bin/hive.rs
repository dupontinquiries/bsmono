//! hive — the CLI over logscope-hive. Every subcommand is one visible operation on the matrix, the
//! ledger, or a plan; the egui shell will call the same library functions.
use hive::{counter, dispatch::{self, Observer, Outcome}, endpoint::{self, Endpoint, Event, Request}, ledger::{self, Entry}, matrix::{self, BoxSpec, Locality, Matrix, Role, Tier}, plan::Plan};

fn arg(args: &[String], flag: &str) -> Option<String> { args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned()) }

fn endpoint_from(kind: &str, url: &str, model: Option<&str>) -> Result<Endpoint, String> {
    let model = model.unwrap_or("default").to_string();
    Ok(match kind {
        "openai" => Endpoint::OpenAi { base: url.into(), model, key: std::env::var("OPENAI_API_KEY").ok() },
        "anthropic" => Endpoint::Anthropic { base: url.into(), model, key: std::env::var("ANTHROPIC_API_KEY").ok() },
        "ollama" => Endpoint::Ollama { base: url.into(), model },
        "claude" => Endpoint::ClaudeCode { extra: Vec::new(), cwd: None },
        k => return Err(format!("unknown kind {k}: openai | anthropic | ollama | claude")),
    })
}

struct Printer;
impl Observer for Printer {
    fn wave(&mut self, n: usize, ids: &[String]) { println!("\n── wave {n}: {} ──", ids.join(", ")); }
    fn started(&mut self, task: &str, on: &str) { println!("  {task} → {on}"); }
    fn finished(&mut self, o: &Outcome) {
        println!("  {} on {}: {} · {} tok · {:.1} s · forwarded {} of {} chars{}", o.task, o.r#box, if o.keep { "KEEP" } else { "DISCARD" }, o.out_tokens, o.ms / 1000.0,
            o.summary.chars().count(), o.full.chars().count(), if o.keep { String::new() } else { format!(" · {}", o.reason) });
    }
}

const EXAMPLE_PLAN: &str = r#"{
  "tasks": [
    { "id": "design", "role": "integrator", "function": "design", "route": { "min_tier": "frontier", "max_tier": "frontier", "allow_external": true },
      "prompt": "Write a one-page design note for a KV-reusing session in a Rust llama.cpp backend: goals, the seam between engine and shell, and three acceptance checks. Markdown.", "max_tokens": 900 },
    { "id": "guide", "role": "integrator", "function": "guide", "deps": ["design"], "route": { "min_tier": "large", "max_tier": "frontier", "allow_external": true },
      "prompt": "From this design, write a worker guide: numbered steps, each with the file to touch and the check to run.\n{{design:design}}", "max_tokens": 700 },
    { "id": "facts-a", "role": "worker", "function": "build", "prompt": "List the public functions in src/core/syscheck.rs as `name: one line`. Wrap the list in <forward></forward>.", "forward": { "max_chars": 600 } },
    { "id": "facts-b", "role": "worker", "function": "build", "prompt": "List the public functions in src/core/models.rs as `name: one line`. Wrap the list in <forward></forward>.", "forward": { "max_chars": 600 } },
    { "id": "premise", "role": "verifier", "function": "verify", "premise": [ { "command": "rustc --version", "expect": "rustc 1." } ], "prompt": "Reply with the single word ok.", "max_tokens": 8 },
    { "id": "review", "role": "integrator", "function": "review", "deps": ["guide", "facts-a", "facts-b", "premise"],
      "route": { "min_tier": "small", "max_tier": "frontier", "allow_external": true, "escalate_on_fail": true },
      "prompt": "Guide:\n{{guide:guide}}\nWorker A:\n{{dep:facts-a}}\nWorker B:\n{{dep:facts-b}}\nReview both against the guide. Reply KEEP or DISCARD per worker with one reason each.",
      "verify": "echo \"$OUT\" | grep -q KEEP" }
  ]
}"#;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = cli(&args) { eprintln!("hive: {e}"); std::process::exit(1); }
}

fn cli(args: &[String]) -> Result<(), String> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    let rest = &args[args.len().min(1)..];
    match sub {
        "matrix" => matrix_cmd(rest),
        "gen" => {
            let m = Matrix::load();
            let b = rest.first().and_then(|n| m.get(n)).cloned().ok_or("usage: hive gen <box> \"prompt\" [--max N] [--system S]")?;
            let req = Request { prompt: rest.get(1).cloned().unwrap_or_default(), system: arg(rest, "--system"), max_tokens: arg(rest, "--max").and_then(|s| s.parse().ok()).unwrap_or(256), temperature: 0.0, resume: None, grammar: None };
            stream_to_stdout(&b.endpoint, &req)
        }
        "claude" => {
            let req = Request { prompt: rest.first().cloned().unwrap_or_default(), system: arg(rest, "--system"), max_tokens: 0, temperature: 0.0, resume: arg(rest, "--resume"), grammar: None };
            let extra: Vec<String> = rest.iter().skip(1).filter(|a| !["--resume", "--system"].contains(&a.as_str())).cloned().collect();
            stream_to_stdout(&Endpoint::ClaudeCode { extra, cwd: None }, &req)
        }
        "claude-parse" => {
            let text = std::fs::read_to_string(rest.first().ok_or("usage: hive claude-parse <stream.jsonl>")?).map_err(|e| e.to_string())?;
            let (mut m, mut partial) = (endpoint::Metrics::default(), false);
            let mut out = String::new();
            for l in text.lines() { if let Some(t) = endpoint::parse_claude_line(l, &mut m, &mut partial) { out.push_str(&t); } }
            println!("{out}\n--\nsession={:?} out_tokens={} cost={:?}", m.session_id, m.out_tokens, m.cost_usd); Ok(())
        }
        "run" => {
            let plan = Plan::load(rest.first().ok_or("usage: hive run plan.json")?)?;
            let m = Matrix::load();
            let out = dispatch::run(&plan, &m, &mut Printer)?;
            println!("\n{} tasks, {} kept", out.len(), out.iter().filter(|o| o.keep).count()); Ok(())
        }
        "example-plan" => { println!("{EXAMPLE_PLAN}"); Ok(()) }
        "absence" => ledger::append(Entry::Absence {
            looked_for: arg(rest, "--looked-for").ok_or("--looked-for")?, did_not_find: arg(rest, "--did-not-find").ok_or("--did-not-find")?,
            command: arg(rest, "--command").ok_or("--command")?, scope: arg(rest, "--scope").unwrap_or_default(), task: arg(rest, "--task") }),
        "recheck" => { for (l, absent, head) in ledger::recheck() { if let Entry::Absence { looked_for, .. } = &l.entry { println!("{} {:40} {}", if absent { "still absent" } else { "NOW PRESENT " }, looked_for, head); } } Ok(()) }
        "ledger" => { let n: usize = rest.first().and_then(|s| s.parse().ok()).unwrap_or(20); for l in ledger::read_all().iter().rev().take(n).rev() { println!("{}", serde_json::to_string(l).unwrap_or_default()); } Ok(()) }
        "count" => {
            let text = std::fs::read_to_string(rest.first().ok_or("usage: hive count <transcript.jsonl>")?).map_err(|e| e.to_string())?;
            println!("{}", serde_json::to_string_pretty(&counter::count(&text)).unwrap_or_default()); Ok(())
        }
        _ => Err("usage: hive matrix (add|probe|ls) | gen | claude | claude-parse | run plan.json | example-plan | absence | recheck | ledger [n] | count file".into()),
    }
}

fn matrix_cmd(rest: &[String]) -> Result<(), String> {
    let mut m = Matrix::load();
    match rest.first().map(|s| s.as_str()).unwrap_or("ls") {
        "add" => {
            let (name, kind, url) = (rest.get(1).ok_or("name")?, rest.get(2).ok_or("kind")?, rest.get(3).map(|s| s.as_str()).unwrap_or("-"));
            let roles: Vec<Role> = arg(rest, "--roles").unwrap_or("worker".into()).split(',').filter_map(|r| match r { "worker" => Some(Role::Worker), "verifier" => Some(Role::Verifier), "integrator" => Some(Role::Integrator), _ => None }).collect();
            let mut b = BoxSpec { name: name.clone(), endpoint: endpoint_from(kind, url, arg(rest, "--model").as_deref())?, roles, concurrency: arg(rest, "--np").and_then(|s| s.parse().ok()).unwrap_or(1),
                tier: Tier::Large, locality: Locality::Local, cost_per_mtok: arg(rest, "--cost").and_then(|s| s.parse().ok()), capability: Default::default(), healthy: false };
            b.infer_tier_and_locality();
            if let Some(t) = arg(rest, "--tier") { b.tier = match t.as_str() { "small" => Tier::Small, "large" => Tier::Large, "frontier" => Tier::Frontier, _ => return Err("--tier small|large|frontier".into()) }; }
            if let Some(l) = arg(rest, "--locality") { b.locality = match l.as_str() { "local" => Locality::Local, "external" => Locality::External, _ => return Err("--locality local|external".into()) }; }
            m.upsert(b); m.save()?; println!("added {name}; run `hive matrix probe {name}`"); Ok(())
        }
        "probe" => {
            let which: Vec<String> = match rest.get(1) { Some(n) => vec![n.clone()], None => m.boxes.iter().map(|b| b.name.clone()).collect() };
            for name in which {
                let Some(b) = m.boxes.iter_mut().find(|b| &b.name == &name) else { continue };
                match matrix::probe(b) { Ok(d) => println!("{name}: ok · {d} · ttft {:.0} ms · {:.1} tok/s", b.capability.ttft_ms, b.capability.tok_s), Err(e) => println!("{name}: DOWN · {e}") }
            }
            m.save()
        }
        _ => { for b in &m.boxes { println!("{:12} {:7} {:?}/{:?} roles={:?} np={} {:.1} tok/s", b.name, if b.healthy { "up" } else { "unknown" }, b.tier, b.locality, b.roles, b.concurrency, b.capability.tok_s); } println!("width: worker {} verifier {} integrator {}", m.width(Role::Worker), m.width(Role::Verifier), m.width(Role::Integrator)); Ok(()) }
    }
}

fn stream_to_stdout(ep: &Endpoint, req: &Request) -> Result<(), String> {
    use std::io::Write;
    let mut on = |e: Event| { match e { Event::Delta(s) => { print!("{s}"); let _ = std::io::stdout().flush(); } Event::Error(e) => eprintln!("\nerror: {e}"), _ => {} } true };
    let m = endpoint::generate(ep, req, &mut on)?;
    println!("\n--\nttft {:.0} ms · {} tok · {:.1} tok/s (client){}{}{}", m.ttft_ms, m.out_tokens, m.gen_tok_s,
        m.server_gen_tok_s.map(|v| format!(" · {v:.1} (server)")).unwrap_or_default(),
        m.draft_accept.map(|v| format!(" · accept {:.0}%", v * 100.0)).unwrap_or_default(),
        m.cost_usd.map(|v| format!(" · ${v:.4}")).unwrap_or_default());
    Ok(())
}
