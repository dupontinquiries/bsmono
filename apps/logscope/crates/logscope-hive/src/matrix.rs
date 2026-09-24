//! matrix — the boxes in the session, what each can do, and what it measured. Persisted as JSON so
//! every consumer (CLI, egui, the dispatcher) sees the same set. A box is an endpoint plus roles.
use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::endpoint::{self, Endpoint, Event, Request};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Cheap, staleable; output is judged by the environment (compiler, test, measurement).
    Worker,
    /// Pre-flight: checks premises against reality before anyone builds on them.
    Verifier,
    /// Holds the boundary graph; expensive; not staled. Usually Claude Code or the biggest local model.
    Integrator,
}

/// How strong a box is. The router picks the CHEAPEST tier that satisfies a task's route, so a
/// frontier model is reached by policy (escalation or an explicit floor), never by default.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Tier { Small, Large, Frontier }

/// Where the bytes go. `Local` stays on the mesh; `External` leaves it (Claude Code, api.anthropic.com).
/// A task carrying device data or private code is routed only to boxes whose locality it allows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Locality { Local, External }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Capability {
    pub ttft_ms: f64,
    pub tok_s: f64,
    pub probed_at: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoxSpec {
    pub name: String,
    pub endpoint: Endpoint,
    pub roles: Vec<Role>,
    /// How many tasks may run on it at once (llama-server `-np`, Ollama `OLLAMA_NUM_PARALLEL`, Claude 1).
    pub concurrency: usize,
    #[serde(default = "default_tier")]
    pub tier: Tier,
    #[serde(default = "default_locality")]
    pub locality: Locality,
    /// Optional: USD per million output tokens, for the ledger's cost column. None = free/local.
    #[serde(default)]
    pub cost_per_mtok: Option<f64>,
    #[serde(default)]
    pub capability: Capability,
    #[serde(default)]
    pub healthy: bool,
}

fn default_tier() -> Tier { Tier::Large }
fn default_locality() -> Locality { Locality::Local }

impl BoxSpec {
    /// Sensible defaults from the endpoint kind: Claude Code and the Anthropic API are frontier and
    /// external unless the base URL is on the mesh (a llama-server speaking the Anthropic protocol).
    pub fn infer_tier_and_locality(&mut self) {
        use crate::endpoint::Endpoint::*;
        let local_url = |u: &str| u.contains("127.0.0.1") || u.contains("localhost") || u.contains("://ultra") || u.contains("://mini") || u.contains(".local") || u.contains("100.");
        match &self.endpoint {
            ClaudeCode { .. } => { self.tier = Tier::Frontier; self.locality = Locality::External; }
            Anthropic { base, .. } | OpenAi { base, .. } => {
                if local_url(base) { self.locality = Locality::Local; } else { self.locality = Locality::External; self.tier = Tier::Frontier; }
            }
            Ollama { .. } => { self.locality = Locality::Local; }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Matrix { pub boxes: Vec<BoxSpec> }

pub fn path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("logscope").join("matrix.json")
}

impl Matrix {
    pub fn load() -> Matrix {
        std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) -> Result<(), String> {
        let p = path();
        if let Some(d) = p.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
        std::fs::write(&p, serde_json::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }
    pub fn get(&self, name: &str) -> Option<&BoxSpec> { self.boxes.iter().find(|b| b.name == name) }
    pub fn upsert(&mut self, b: BoxSpec) {
        if let Some(x) = self.boxes.iter_mut().find(|x| x.name == b.name) { *x = b; } else { self.boxes.push(b); }
    }
    pub fn with_role(&self, role: Role) -> Vec<&BoxSpec> { self.boxes.iter().filter(|b| b.healthy && b.roles.contains(&role)).collect() }
    /// Boxes that satisfy a route: right role, tier inside [min, max], and locality the task allows.
    /// Sorted cheapest tier first, so the caller's first pick is the router's choice.
    pub fn candidates(&self, role: Role, min: Tier, max: Tier, allow_external: bool) -> Vec<&BoxSpec> {
        let mut v: Vec<&BoxSpec> = self.with_role(role).into_iter()
            .filter(|b| b.tier >= min && b.tier <= max && (allow_external || b.locality == Locality::Local)).collect();
        v.sort_by_key(|b| b.tier);
        v
    }
    /// Total concurrency available to a role: the fan-out width the dispatcher may use.
    pub fn width(&self, role: Role) -> usize { self.with_role(role).iter().map(|b| b.concurrency.max(1)).sum() }
}

/// Health + a 16-token smoke generation, so `capability` is measured rather than declared.
pub fn probe(b: &mut BoxSpec) -> Result<String, String> {
    let desc = endpoint::probe(&b.endpoint)?;
    let req = Request { prompt: "Reply with the single word: ready".into(), max_tokens: 16, temperature: 0.0, ..Default::default() };
    let t0 = Instant::now();
    let mut sink = |_e: Event| true;
    match endpoint::generate(&b.endpoint, &req, &mut sink) {
        Ok(m) => {
            b.capability = Capability { ttft_ms: m.ttft_ms, tok_s: if m.gen_tok_s > 0.0 { m.gen_tok_s } else { m.server_gen_tok_s.unwrap_or(0.0) },
                probed_at: Some(now()), note: Some(format!("{desc}; smoke {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0)) };
            b.healthy = true;
            Ok(desc)
        }
        Err(e) => { b.healthy = false; b.capability.note = Some(e.clone()); Err(e) }
    }
}

pub fn now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // ISO-ish without pulling a date crate: days since epoch is enough for a ledger; the CLI shows it raw
    format!("unix:{secs}")
}
