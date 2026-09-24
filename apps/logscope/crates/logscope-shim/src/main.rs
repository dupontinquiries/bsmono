//! logscope-shim — runs on the headless box. `GET /status`, `GET /models`, `POST /directive` (a
//! `hive::directive::Directive`), `POST /status` (a `hive::status::StatusLine` to relay). One thread
//! serves HTTP; long directives (copies, loads, plan runs) run inline and reply when done, which is
//! what a driver that wants an answer needs; the Ultra's UI puts the call on a thread.
mod config;
mod library;
mod server;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use hive::directive::{Directive, Reply};
use hive::status::{self, Level, Sink, StatusLine};
use hive::{dispatch, ledger, matrix::Matrix};

struct State { cfg: config::Config, running: Option<server::Running>, sinks: Vec<Box<dyn Sink>>, last_status: Vec<StatusLine> }

impl State {
    fn say(&mut self, level: Level, text: &str) {
        let line = StatusLine::new(&self.cfg.name, level, text);
        let errs = status::broadcast(&self.sinks, &line);
        for (s, e) in errs { eprintln!("shim: sink {s}: {e}"); }
        self.last_status.push(line); if self.last_status.len() > 50 { self.last_status.remove(0); }
    }

    fn status_json(&self) -> serde_json::Value {
        let cache_used = library::cache_bytes(&self.cfg.cache_dir);
        serde_json::json!({
            "name": self.cfg.name,
            "loaded": self.running.as_ref().map(|r| serde_json::json!({"model": r.model, "profile": r.profile, "up_s": r.since.elapsed().as_secs(), "port": self.cfg.server_port})),
            "cache_gib": library::gib(cache_used), "budget_gib": self.cfg.budget_gb, "free_gib": (self.cfg.budget_gb - library::gib(cache_used)).max(0.0),
            "library": self.cfg.library_dir, "cache": self.cfg.cache_dir,
            "server_health": self.running.as_ref().map(|_| ureq::get(&format!("http://127.0.0.1:{}/health", self.cfg.server_port)).timeout(Duration::from_millis(400)).call().is_ok()),
            "recent_status": self.last_status.iter().rev().take(5).map(|l| l.render()).collect::<Vec<_>>(),
        })
    }

    fn handle(&mut self, d: Directive) -> Reply {
        match d {
            Directive::Status => Reply::with(format!("{} · {}", self.cfg.name, self.running.as_ref().map(|r| format!("{} ({})", r.model, r.profile)).unwrap_or("nothing loaded".into())), self.status_json()),
            Directive::Models => {
                let lib = library::scan(&self.cfg.library_dir);
                let cached: Vec<String> = library::scan(&self.cfg.cache_dir).into_iter().map(|e| e.name).collect();
                let list: Vec<serde_json::Value> = lib.iter().map(|e| serde_json::json!({"name": e.name, "gib": library::gib(e.bytes), "cached": cached.contains(&e.name)})).collect();
                Reply::with(format!("{} in library, {} cached", lib.len(), cached.len()), serde_json::json!({"library": list, "cached": cached}))
            }
            Directive::Load { model, profile } => {
                let pname = profile.unwrap_or_else(|| "plain".into());
                let Some(prof) = self.cfg.profiles.get(&pname).cloned() else { return Reply::err(format!("no profile '{pname}'; have {:?}", self.cfg.profiles.keys().collect::<Vec<_>>())) };
                if let Some(r) = &self.running { if r.model == model && r.profile == pname { return Reply::ok(format!("{model} already loaded ({pname})")); } }
                let budget = (self.cfg.budget_gb * 1024f64.powi(3)) as u64;
                self.say(Level::Fact, &format!("caching {model}"));
                let mut last = 0u64;
                let (path, evicted) = match library::ensure(&self.cfg.library_dir, &self.cfg.cache_dir, budget, &model, &mut |done, total| { if done - last > (4u64 << 30) { last = done; eprintln!("shim: copy {model} {:.0}%", done as f64 / total as f64 * 100.0); } }) {
                    Ok(x) => x, Err(e) => { self.say(Level::Fact, &format!("load failed: {e}")); return Reply::err(e) }
                };
                self.running = None;   // kill the old server before the new one binds the port
                let mut r = match server::start(&self.cfg, &path, &model, &pname, &prof) { Ok(r) => r, Err(e) => return Reply::err(e) };
                match server::wait_ready(&mut r, self.cfg.server_port, Duration::from_secs(600)) {
                    Ok(d) => {
                        let _ = server::register_in_matrix(&self.cfg, &model);
                        self.running = Some(r);
                        let msg = format!("loaded {model} ({pname}) in {:.0}s{}", d.as_secs_f64(), if evicted.is_empty() { String::new() } else { format!(", evicted {}", evicted.join(", ")) });
                        self.say(Level::Fact, &msg);
                        Reply::with(msg, serde_json::json!({"port": self.cfg.server_port, "evicted": evicted}))
                    }
                    Err(e) => { self.say(Level::Fact, &format!("load failed: {e}")); Reply::err(e) }
                }
            }
            Directive::Unload => { let was = self.running.take().map(|r| r.model.clone()); self.say(Level::Fact, &format!("unloaded {}", was.clone().unwrap_or("nothing".into()))); Reply::ok(format!("unloaded {}", was.unwrap_or("nothing".into()))) }
            Directive::Run { plan } => {
                let m = Matrix::load();
                self.say(Level::Fact, &format!("running plan: {} tasks", plan.tasks.len()));
                match dispatch::run(&plan, &m, &mut dispatch::Quiet) {
                    Ok(outs) => {
                        let kept = outs.iter().filter(|o| o.keep).count();
                        let text = outs.iter().map(|o| format!("{} on {}: {} ({} tok, {:.1}s){}", o.task, o.r#box, if o.keep { "KEEP" } else { "DISCARD" }, o.out_tokens, o.ms / 1000.0, if o.keep { String::new() } else { format!(" · {}", o.reason) })).collect::<Vec<_>>().join("\n");
                        self.say(Level::Fact, &format!("plan done: {kept}/{} kept", outs.len()));
                        Reply::with(text, serde_json::json!({"kept": kept, "total": outs.len(), "summaries": outs.iter().filter(|o| o.keep).map(|o| (o.task.clone(), o.summary.clone())).collect::<std::collections::BTreeMap<_, _>>()}))
                    }
                    Err(e) => { self.say(Level::Fact, &format!("plan failed: {e}")); Reply::err(e) }
                }
            }
            Directive::Summary { n } => {
                let lines = ledger::read_all();
                let tail: Vec<String> = lines.iter().rev().take(n).rev().map(|l| serde_json::to_string(&l.entry).unwrap_or_default()).collect();
                let arts: Vec<String> = std::fs::read_dir(ledger::artifact_dir()).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
                Reply::with(format!("{}\n\nartifacts: {}", tail.join("\n"), arts.join(", ")), serde_json::json!({"artifacts": arts}))
            }
            Directive::Say { text, level } => { self.say(level.as_deref().map(Level::parse).unwrap_or(Level::Inference), &text); Reply::ok("said") }
            Directive::Integrate { name, bundle_b64, repo, branch, verify } => {
                let r = integrate(&name, &bundle_b64, &repo, &branch, verify.as_deref());
                match &r { Ok(rep) => self.say(Level::Fact, &format!("integrate {name}: {}", rep.text.lines().next().unwrap_or(""))), Err(e) => self.say(Level::Fact, &format!("integrate {name} failed: {}", e.chars().take(120).collect::<String>())) }
                r.unwrap_or_else(Reply::err)
            }
        }
    }
}

fn sh(cwd: &std::path::Path, argv: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new(argv[0]).args(&argv[1..]).current_dir(cwd).output().map_err(|e| format!("{}: {e}", argv[0]))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() { Ok(text) } else { Err(format!("`{}` exit {}: {}", argv.join(" "), out.status, text.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"))) }
}

/// The integration path out of the /tmp bubble: bundle → verified ref → throwaway worktree → the
/// verifier → the merge command, printed, not run.
fn integrate(name: &str, bundle_b64: &str, repo: &str, branch: &str, verify: Option<&[String]>) -> Result<Reply, String> {
    use hive::directive::b64;
    let safe: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    if safe.is_empty() { return Err("name must be [A-Za-z0-9_-]".into()); }
    let repo = std::path::Path::new(repo);
    if !repo.join(".git").exists() && sh(repo, &["git", "rev-parse", "--git-dir"]).is_err() { return Err(format!("{} is not a git repository", repo.display())); }
    let tmp = std::env::temp_dir().join(format!("logscope-integrate-{safe}"));
    let _ = std::fs::remove_dir_all(&tmp); std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let bundle = tmp.join("in.bundle");
    std::fs::write(&bundle, b64::decode(bundle_b64)?).map_err(|e| e.to_string())?;
    let b = bundle.to_string_lossy().to_string();
    sh(repo, &["git", "bundle", "verify", &b])?;
    let refname = format!("refs/logscope/{safe}");
    sh(repo, &["git", "fetch", "--no-tags", &b, &format!("{branch}:{refname}")])?;
    let wt = tmp.join("worktree"); let wts = wt.to_string_lossy().to_string();
    let _ = sh(repo, &["git", "worktree", "remove", "--force", &wts]);
    sh(repo, &["git", "worktree", "add", "--detach", &wts, &refname])?;
    let default_verify: Vec<String> = vec!["cargo".into(), "check".into()];
    let verify = verify.unwrap_or(&default_verify);
    let argv: Vec<&str> = verify.iter().map(|s| s.as_str()).collect();
    let (green, vout) = match sh(&wt, &argv) { Ok(o) => (true, o), Err(e) => (false, e) };
    let ahead = sh(repo, &["git", "rev-list", "--count", &format!("HEAD..{refname}")]).unwrap_or_default().trim().to_string();
    let _ = sh(repo, &["git", "worktree", "remove", "--force", &wts]);
    let text = format!("{}: `{}` {} in a worktree of {refname} ({ahead} commits ahead of HEAD)\nto land it:  git -C {} merge --ff-only {refname}\nto inspect:  git -C {} log --oneline HEAD..{refname}\n{}",
        safe, verify.join(" "), if green { "GREEN" } else { "RED" }, repo.display(), repo.display(), vout.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
    Ok(Reply::with(text, serde_json::json!({"green": green, "ref": refname, "ahead": ahead})))
}

fn authorized(req: &tiny_http::Request, token: &str) -> bool {
    token.is_empty() || req.headers().iter().any(|h| h.field.equiv("Authorization") && h.value.as_str() == format!("Bearer {token}"))
}

fn main() {
    let cfg = match config::load_or_create() { Ok(c) => c, Err(e) => { eprintln!("shim: {e}"); std::process::exit(1) } };
    let mut sinks: Vec<Box<dyn Sink>> = Vec::new();
    if let Some(u) = &cfg.status_http { sinks.push(Box::new(status::HttpSink { base: u.clone(), token: Some(cfg.token.clone()).filter(|t| !t.is_empty()) })); }
    #[cfg(feature = "dht")]
    if let Some(ch) = &cfg.status_channel {
        match status::dht::DhtSink::new(ch, "0.0.0.0:0".parse().unwrap(), None) { Ok(s) => sinks.push(Box::new(s)), Err(e) => eprintln!("shim: dht sink: {e}") }
    }
    let addr = format!("0.0.0.0:{}", cfg.port);
    let srv = match tiny_http::Server::http(&addr) { Ok(s) => s, Err(e) => { eprintln!("shim: bind {addr}: {e}"); std::process::exit(1) } };
    eprintln!("shim: {} listening on {addr}; library {}; cache {} ({:.0} GiB budget); config {}", cfg.name, cfg.library_dir.display(), cfg.cache_dir.display(), cfg.budget_gb, config::path().display());
    let state = Arc::new(Mutex::new(State { cfg: cfg.clone(), running: None, sinks, last_status: Vec::new() }));
    state.lock().unwrap().say(Level::Fact, "shim up");
    for mut req in srv.incoming_requests() {
        let mut st = state.lock().unwrap();
        if !authorized(&req, &st.cfg.token) { let _ = req.respond(tiny_http::Response::from_string("unauthorized").with_status_code(401)); continue; }
        let mut body = String::new(); let _ = req.as_reader().read_to_string(&mut body);
        let (code, out) = match (req.method(), req.url()) {
            (tiny_http::Method::Get, "/status") => (200, serde_json::to_string(&st.status_json()).unwrap_or_default()),
            (tiny_http::Method::Get, "/models") => (200, serde_json::to_string(&st.handle(Directive::Models)).unwrap_or_default()),
            (tiny_http::Method::Post, "/directive") => match serde_json::from_str::<Directive>(&body) {
                Ok(d) => (200, serde_json::to_string(&st.handle(d)).unwrap_or_default()),
                Err(e) => (400, serde_json::to_string(&Reply::err(format!("bad directive: {e}"))).unwrap_or_default()),
            },
            (tiny_http::Method::Post, "/status") => match serde_json::from_str::<StatusLine>(&body) {
                Ok(l) => { let errs = status::broadcast(&st.sinks, &l); st.last_status.push(l); (200, serde_json::json!({"ok": true, "sink_errors": errs}).to_string()) }
                Err(e) => (400, format!("bad status line: {e}")),
            },
            _ => (404, "status | models | directive | status(POST)".into()),
        };
        let _ = req.respond(tiny_http::Response::from_string(out).with_status_code(code).with_header(tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap()));
    }
}
