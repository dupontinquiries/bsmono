//! fabric.rs — the worker abstraction behind the LAN inference fabric (mock-first, network later).
//!
//! Each box on the LAN is a `Worker`: hand it a prefix of TOKENS (not KV — KV is local and ephemeral,
//! so the box re-prefills) plus a model + sampler, get back tokens. `LocalWorker` (the resident engine)
//! and a future `RemoteWorker` (HTTP to a box) are the same trait; only the backend differs. The
//! `Registry` aggregates each box's models — honoring a per-model public/private flag — and dispatches
//! a unit of work to a box that publicly serves the requested model, attaching the result to the tree
//! as a `FabricNode` that records WHICH device + author + model produced it. That provenance is the
//! point: the tree shows not just what diverged, but who generated each branch.
#![allow(dead_code)]

/// Who produced a branch. `device` is the box ("studio-m2"); `author` the operator/owner ("dustin").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerId {
    pub device: String,
    pub author: String,
}
impl WorkerId {
    pub fn new(device: &str, author: &str) -> Self { Self { device: device.into(), author: author.into() } }
    pub fn tag(&self) -> String { format!("{}@{}", self.author, self.device) }
}

/// A model a box can serve, plus whether it's shared to the fabric (the dropdown public/private toggle).
#[derive(Clone, Debug)]
pub struct ModelInfo {
    pub name: String,
    pub public: bool,
    pub params_b: f32, // parameter count in billions → size class for entry-point ranking
}

/// A unit of work. `prefix` is the tree path's tokens — the portable handle; the box re-prefills it.
pub struct GenRequest {
    pub prefix: Vec<u32>,
    pub model: String,
    pub max_new: usize,
    pub temperature: f32,
    pub seed: u64,
}

pub struct GenResult {
    pub tokens: Vec<u32>,
    pub worker: WorkerId,
    pub model: String,
}

pub trait Worker {
    fn id(&self) -> &WorkerId;
    fn models(&self) -> &[ModelInfo];
    /// True only if the box serves `model` AND it's marked public.
    fn serves_public(&self, model: &str) -> bool {
        self.models().iter().any(|m| m.name == model && m.public)
    }
    fn generate(&self, req: &GenRequest) -> Option<GenResult>;
}

/// What the fabric attaches to the tree: the produced tokens, tagged with full provenance.
#[derive(Clone, Debug)]
pub struct FabricNode {
    pub tokens: Vec<u32>,
    pub worker: WorkerId,
    pub model: String,
    pub parent: Option<usize>,
}
impl FabricNode {
    /// Human-readable provenance for the tree-node label, e.g. "deepseek-8b · dustin@studio-m2".
    pub fn provenance(&self) -> String { format!("{} · {}", self.model, self.worker.tag()) }
}

#[derive(Default)]
pub struct Registry {
    workers: Vec<Box<dyn Worker>>,
}
impl Registry {
    pub fn new() -> Self { Self { workers: Vec::new() } }
    pub fn add(&mut self, w: Box<dyn Worker>) { self.workers.push(w); }
    pub fn len(&self) -> usize { self.workers.len() }

    /// Every PUBLIC model across the fabric, paired with the device offering it — the dropdown source.
    pub fn available_models(&self) -> Vec<(String, WorkerId)> {
        let mut out = Vec::new();
        for w in &self.workers {
            for m in w.models() {
                if m.public { out.push((m.name.clone(), w.id().clone())); }
            }
        }
        out
    }

    /// Public (worker, model) pairs — the scheduler builds entry points from these.
    pub fn public_models(&self) -> Vec<(WorkerId, ModelInfo)> {
        let mut out = Vec::new();
        for w in &self.workers {
            for m in w.models() { if m.public { out.push((w.id().clone(), m.clone())); } }
        }
        out
    }

    /// Route a request to a worker that publicly serves the model, attaching the result as a tree node.
    /// Returns None if no public worker has that model (e.g. it's marked private everywhere).
    pub fn dispatch(&self, req: &GenRequest, parent: Option<usize>) -> Option<FabricNode> {
        let w = self.workers.iter().find(|w| w.serves_public(&req.model))?;
        let r = w.generate(req)?;
        Some(FabricNode { tokens: r.tokens, worker: r.worker, model: r.model, parent })
    }

    /// Fan a single fork out across every box that can serve the model — the throughput play: N boxes
    /// explore N branches of the same prefix in one wall-clock, each a distinct provenance-tagged node.
    pub fn fan_out(&self, base: &GenRequest, parent: Option<usize>) -> Vec<FabricNode> {
        let mut nodes = Vec::new();
        for (i, w) in self.workers.iter().enumerate() {
            if !w.serves_public(&base.model) { continue; }
            let req = GenRequest { prefix: base.prefix.clone(), model: base.model.clone(),
                max_new: base.max_new, temperature: base.temperature, seed: base.seed ^ (i as u64 + 1) };
            if let Some(r) = w.generate(&req) {
                nodes.push(FabricNode { tokens: r.tokens, worker: r.worker, model: r.model, parent });
            }
        }
        nodes
    }
}

/// Stand-in box for validating dispatch/registry without a network: echoes the prefix plus a
/// seed-derived token so different branches/devices produce distinguishable, deterministic output.
pub struct MockWorker {
    id: WorkerId,
    models: Vec<ModelInfo>,
}
impl MockWorker {
    pub fn new(device: &str, author: &str, models: &[(&str, bool, f32)]) -> Self {
        Self { id: WorkerId::new(device, author),
            models: models.iter().map(|&(n, p, b)| ModelInfo { name: n.into(), public: p, params_b: b }).collect() }
    }
}
impl Worker for MockWorker {
    fn id(&self) -> &WorkerId { &self.id }
    fn models(&self) -> &[ModelInfo] { &self.models }
    fn generate(&self, req: &GenRequest) -> Option<GenResult> {
        let mut tokens = req.prefix.clone();
        tokens.push((req.seed % 1000) as u32);
        Some(GenResult { tokens, worker: self.id.clone(), model: req.model.clone() })
    }
}

/// Self-test (no GPU/network): public/private filtering, routing-to-right-device, provenance, fan-out.
pub fn gate_fabric() {
    let mut reg = Registry::new();
    reg.add(Box::new(MockWorker::new("studio-m2", "dustin", &[("llama-1b", true, 1.0), ("deepseek-8b", false, 8.0)])));
    reg.add(Box::new(MockWorker::new("laptop", "dustin", &[("smollm2", true, 0.135), ("llama-1b", true, 1.0)])));

    let avail = reg.available_models();
    let names: Vec<&str> = avail.iter().map(|(n, _)| n.as_str()).collect();
    let priv_hidden = !names.contains(&"deepseek-8b");           // private model not advertised
    let llama_two = names.iter().filter(|&&n| n == "llama-1b").count() == 2; // two boxes offer it

    let req = |m: &str| GenRequest { prefix: vec![1, 2, 3], model: m.into(), max_new: 8, temperature: 0.8, seed: 42 };
    let n1 = reg.dispatch(&req("llama-1b"), None);
    let routed = n1.as_ref().map(|n| n.worker.device == "studio-m2").unwrap_or(false); // first matching box
    let prov_ok = n1.as_ref().map(|n| n.provenance() == "llama-1b · dustin@studio-m2").unwrap_or(false);
    let private_blocked = reg.dispatch(&req("deepseek-8b"), None).is_none(); // no PUBLIC server

    let fan = reg.fan_out(&req("llama-1b"), Some(0));
    let fan_two = fan.len() == 2 && fan[0].worker.device != fan[1].worker.device;

    for (label, ok) in [("private hidden from dropdown", priv_hidden), ("llama-1b on two boxes", llama_two),
        ("routed to a serving box", routed), ("provenance device+author+model", prov_ok),
        ("private model un-dispatchable", private_blocked), ("fan-out = one node per box", fan_two)] {
        println!("  [{}] {}", if ok { "\u{2713}" } else { "\u{2717} FAIL" }, label);
    }
    let all = priv_hidden && llama_two && routed && prov_ok && private_blocked && fan_two;
    println!("gate-fabric: {}", if all { "worker spine checks \u{2713}" } else { "FAIL \u{2717}" });
}
