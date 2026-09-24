//! server — the llama-server this box runs. One at a time; loading another kills the first. The
//! shim registers the running server as a box in this machine's matrix so `Run` directives and the
//! Ultra's Remote backend both find it by name.
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::{Config, Profile};

pub struct Running { pub child: Child, pub model: String, pub profile: String, pub since: Instant }

impl Drop for Running { fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); } }

pub fn start(cfg: &Config, model_path: &Path, model_name: &str, profile_name: &str, profile: &Profile) -> Result<Running, String> {
    let child = Command::new(&cfg.llama_server)
        .arg("-m").arg(model_path)
        .args(["--host", &cfg.server_host, "--port", &cfg.server_port.to_string(), "-ngl", "99", "--jinja", "-c", &profile.ctx.to_string(), "-np", &profile.np.to_string()])
        .args(&profile.args)
        .stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().map_err(|e| format!("spawn {}: {e}", cfg.llama_server))?;
    Ok(Running { child, model: model_name.into(), profile: profile_name.into(), since: Instant::now() })
}

/// 503 while loading, 200 when ready; an early exit is an error with the exit status.
pub fn wait_ready(r: &mut Running, port: u16, timeout: Duration) -> Result<Duration, String> {
    let t0 = Instant::now();
    loop {
        if let Ok(Some(st)) = r.child.try_wait() { return Err(format!("llama-server exited early: {st}")); }
        if ureq::get(&format!("http://127.0.0.1:{port}/health")).timeout(Duration::from_millis(500)).call().is_ok() { return Ok(t0.elapsed()); }
        if t0.elapsed() > timeout { return Err(format!("not ready after {timeout:?}")); }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Register (or refresh) this server in the local matrix as box `<name>-llama`, so hive plans run
/// here can route to it and the Ultra's `matrix:` specs can name it.
pub fn register_in_matrix(cfg: &Config, model_name: &str) -> Result<(), String> {
    use hive::matrix::{BoxSpec, Locality, Matrix, Role, Tier};
    let mut m = Matrix::load();
    let name = format!("{}-llama", cfg.name);
    let mut b = BoxSpec { name: name.clone(), endpoint: hive::endpoint::Endpoint::OpenAi { base: format!("http://127.0.0.1:{}", cfg.server_port), model: model_name.into(), key: None },
        roles: vec![Role::Worker, Role::Verifier, Role::Integrator], concurrency: 2, tier: Tier::Large, locality: Locality::Local, cost_per_mtok: None, capability: Default::default(), healthy: true };
    if let Some(old) = m.get(&name) { b.roles = old.roles.clone(); b.tier = old.tier; b.concurrency = old.concurrency; }
    m.upsert(b); m.save()
}
