//! plan — tasks as a DAG, with the two things the dispatcher needs declared rather than inferred:
//! independence (`deps`) and the forwarding contract (`forward`). A JSON file is the interchange form
//! so the egui shell, the CLI and a model emitting a plan all speak the same thing.
use serde::{Deserialize, Serialize};

use crate::matrix::{Role, Tier};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    /// Shell command whose output must contain `expect`, run BEFORE the task's prompt goes anywhere.
    /// "Extract data rather than deriving it": the constant comes from the tool, not the model.
    pub command: String,
    pub expect: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Forward {
    /// Only this many characters of the output cross to dependents (after `<forward>` extraction).
    pub max_chars: usize,
}

impl Default for Forward { fn default() -> Self { Forward { max_chars: 800 } } }

/// What a task is FOR. Orthogonal to the box's role and tier: a design doc can come from a frontier
/// queen or from the 35B, and a review can be local or escalated. The function decides the artifact
/// kind and the default route; the route decides the box.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Function { Plan, Design, Guide, Build, Verify, Review, Integrate }

/// The optional frontier route, expressed as policy rather than as a row: a tier floor and ceiling,
/// whether bytes may leave the mesh, and whether a local failure escalates one tier before it is
/// declared DISCARD. Default: local-only, small-to-large, no escalation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    #[serde(default = "tier_small")]
    pub min_tier: Tier,
    #[serde(default = "tier_large")]
    pub max_tier: Tier,
    #[serde(default)]
    pub allow_external: bool,
    #[serde(default)]
    pub escalate_on_fail: bool,
}
fn tier_small() -> Tier { Tier::Small }
fn tier_large() -> Tier { Tier::Large }
impl Default for Route { fn default() -> Self { Route { min_tier: Tier::Small, max_tier: Tier::Large, allow_external: false, escalate_on_fail: false } } }

impl Route {
    /// The frontier route: floor and ceiling at Frontier, bytes may leave.
    pub fn frontier() -> Self { Route { min_tier: Tier::Frontier, max_tier: Tier::Frontier, allow_external: true, escalate_on_fail: false } }
    /// Local first, frontier only if the local attempt is discarded.
    pub fn escalating() -> Self { Route { min_tier: Tier::Small, max_tier: Tier::Frontier, allow_external: true, escalate_on_fail: true } }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default = "default_role")]
    pub role: Role,
    #[serde(default = "default_function")]
    pub function: Function,
    #[serde(default)]
    pub route: Route,
    /// `{{dep:ID}}` is replaced with that dependency's forwarded summary.
    pub prompt: String,
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default = "default_max")]
    pub max_tokens: u32,
    #[serde(default)]
    pub temperature: f32,
    #[serde(default)]
    pub forward: Forward,
    #[serde(default)]
    pub premise: Vec<Check>,
    /// Optional shell verifier run on the output (with the output in $OUT); non-zero exit = DISCARD.
    #[serde(default)]
    pub verify: Option<String>,
}

fn default_role() -> Role { Role::Worker }
fn default_function() -> Function { Function::Build }

impl Task {
    /// Design docs, guides and plans are artifacts: kept whole in the artifact store and injectable
    /// into later prompts with `{{guide:ID}}` / `{{design:ID}}` / `{{plan:ID}}`; everything else forwards
    /// only its summary.
    pub fn produces_artifact(&self) -> bool { matches!(self.function, Function::Plan | Function::Design | Function::Guide) }
}
fn default_max() -> u32 { 512 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan { pub tasks: Vec<Task> }

impl Plan {
    pub fn load(path: &str) -> Result<Plan, String> {
        let s = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let p: Plan = serde_json::from_str(&s).map_err(|e| format!("{path}: {e}"))?;
        p.validate()?;
        Ok(p)
    }
    pub fn validate(&self) -> Result<(), String> {
        let ids: Vec<&str> = self.tasks.iter().map(|t| t.id.as_str()).collect();
        for t in &self.tasks {
            for d in &t.deps { if !ids.contains(&d.as_str()) { return Err(format!("task {} depends on unknown {d}", t.id)); } }
        }
        self.waves().map(|_| ())
    }
    /// Topological waves: every task in wave k depends only on tasks in waves < k.
    /// Tasks in the same wave are independent by declaration, so they fan out.
    pub fn waves(&self) -> Result<Vec<Vec<String>>, String> {
        let mut done: Vec<String> = Vec::new();
        let mut waves = Vec::new();
        let mut remaining: Vec<&Task> = self.tasks.iter().collect();
        while !remaining.is_empty() {
            let (ready, rest): (Vec<&Task>, Vec<&Task>) = remaining.iter().partition(|t| t.deps.iter().all(|d| done.contains(d)));
            if ready.is_empty() { return Err(format!("cycle among: {}", rest.iter().map(|t| t.id.as_str()).collect::<Vec<_>>().join(", "))); }
            let ids: Vec<String> = ready.iter().map(|t| t.id.clone()).collect();
            done.extend(ids.iter().cloned());
            waves.push(ids);
            remaining = rest;
        }
        Ok(waves)
    }
}

/// The summary contract: if the worker wrapped its handoff in `<forward>…</forward>`, that is what
/// crosses; otherwise the head of the output, cut at `max_chars`. Everything else stays in the ledger.
pub fn forwarded(output: &str, f: &Forward) -> String {
    let body = match (output.find("<forward>"), output.rfind("</forward>")) {
        (Some(a), Some(b)) if b > a => output[a + 9..b].trim().to_string(),
        _ => output.trim().to_string(),
    };
    if body.chars().count() <= f.max_chars { body } else { body.chars().take(f.max_chars).collect::<String>() + "…" }
}
