//! from_spec — turn a fan `WorkerSpec` (an operational contract: what a worker may edit, which tools
//! it may call, how success is checked) into a hive `Task`. This is the seam between logscope-fan's
//! roster and the dispatcher, and it goes contract → task, not contract → box: a WorkerSpec is a
//! job's constraints, and a box is an endpoint. The task carries the spec's tool whitelist (so the
//! caller can build the tool-fence grammar from it), its `success` as the shell verifier, and its
//! purpose + owns + constraints in the prompt.
//!
//! Kept as free functions taking plain fields rather than depending on logscope-fan, so this crate
//! stays leaf: the caller (logscope-egui, which has both) passes the pieces in.
use crate::plan::{Check, Forward, Function, Route, Task};
use crate::matrix::{Role, Tier};

/// The pieces of a `WorkerSpec` this needs. The caller maps its `WorkerSpec` onto this.
pub struct SpecFields<'a> {
    pub name: &'a str,
    pub purpose: &'a str,
    pub owns: &'a [String],
    pub context: &'a [String],
    pub tools: &'a [String],
    pub constraints: &'a [String],
    pub success: &'a [String],
}

/// The task's tool whitelist — the spec's tools, for building the grammar and sizing the harness.
pub fn tool_whitelist(spec: &SpecFields) -> Vec<String> { spec.tools.to_vec() }

/// One shell verifier from the spec's success criteria: the first line that looks like a command
/// (contains a space and no prose punctuation) is run as-is; otherwise the criteria are advisory and
/// the task has no automatic verifier. Conservative on purpose — a wrong verifier passes junk.
pub fn verifier(spec: &SpecFields) -> Option<String> {
    spec.success.iter().find(|s| looks_like_command(s)).map(|s| s.trim().to_string())
}

fn looks_like_command(s: &str) -> bool {
    let s = s.trim();
    // `cargo test passes` / `cargo build succeeds` → the command is the leading `cargo …`; a plain
    // English sentence ("the output is readable") is not runnable.
    const RUNNERS: &[&str] = &["cargo ", "npm ", "pytest", "go test", "make ", "./", "sh ", "bash ", "python ", "node ", "rustc "];
    RUNNERS.iter().any(|r| s.starts_with(r)) && !s.ends_with('.')
}

/// Extract the runnable prefix of a success line like "cargo test passes" → "cargo test".
pub fn command_prefix(success_line: &str) -> String {
    let s = success_line.trim();
    const TAILS: &[&str] = &[" passes", " succeeds", " is green", " green", " exits 0", " must pass", " should pass", " passes cleanly"];
    for t in TAILS { if let Some(stripped) = s.strip_suffix(t) { return stripped.trim().to_string(); } }
    s.to_string()
}

/// Build the task. `deps` and `route` are the caller's (a fan branch is usually a Worker on the local
/// tier); `id` defaults to the spec name.
pub fn to_task(spec: &SpecFields, task: &str, deps: Vec<String>, route: Route) -> Task {
    let mut prompt = format!("You are `{}`: {}\n\nTask: {task}\n", spec.name, spec.purpose);
    if !spec.owns.is_empty() { prompt.push_str(&format!("\nYou may edit only: {}. A change anywhere else is out of your lane — say so and stop rather than working around it.\n", spec.owns.join(", "))); }
    else { prompt.push_str("\nYou are read-only: propose changes as text, do not write files.\n"); }
    if !spec.context.is_empty() { prompt.push_str(&format!("Read for context, in order: {}.\n", spec.context.join(", "))); }
    if !spec.constraints.is_empty() { prompt.push_str(&format!("\nConstraints:\n{}\n", spec.constraints.iter().map(|c| format!("- {c}")).collect::<Vec<_>>().join("\n"))); }
    if !spec.success.is_empty() { prompt.push_str(&format!("\nDone when: {}.\n", spec.success.join("; "))); }
    let verify = verifier(spec).map(|line| command_prefix(&line));
    // owns as a premise is not expressible as a shell check here; it is enforced by the harness's
    // lane gate. The verifier is the success command; the forward is a short handoff.
    Task {
        id: spec.name.to_string(), deps, role: Role::Worker, function: Function::Build, route,
        prompt, system: None, max_tokens: 700, temperature: 0.0, forward: Forward { max_chars: 800 },
        premise: Vec::<Check>::new(), verify,
    }
}

/// A sensible default route for a fan worker: local worker tier, escalate to large if it fails.
pub fn worker_route() -> Route { Route { min_tier: Tier::Small, max_tier: Tier::Large, allow_external: false, escalate_on_fail: true } }

#[cfg(test)]
mod tests {
    use super::*;

    fn spec<'a>(tools: &'a [String], success: &'a [String], owns: &'a [String]) -> SpecFields<'a> {
        SpecFields { name: "patcher", purpose: "fix the parser", owns, context: &[], tools, constraints: &[], success }
    }

    #[test]
    fn success_becomes_a_runnable_verifier_or_none() {
        let t = ["cargo test passes".to_string()];
        assert_eq!(verifier(&spec(&[], &t, &[])).as_deref(), Some("cargo test passes"));
        assert_eq!(command_prefix("cargo test passes"), "cargo test");
        assert_eq!(command_prefix("cargo build succeeds"), "cargo build");
        // prose success is advisory, not a verifier
        let p = ["the diff is minimal".to_string()];
        assert_eq!(verifier(&spec(&[], &p, &[])), None);
    }

    #[test]
    fn task_carries_lane_tools_and_verifier() {
        let tools = ["edit".to_string(), "read_file".to_string()];
        let success = ["cargo test passes".to_string()];
        let owns = ["src/parser.rs".to_string()];
        let sp = spec(&tools, &success, &owns);
        let task = to_task(&sp, "make it handle empty input", vec![], worker_route());
        assert_eq!(task.id, "patcher");
        assert_eq!(task.verify.as_deref(), Some("cargo test"));
        assert!(task.prompt.contains("src/parser.rs") && task.prompt.contains("out of your lane"));
        assert!(task.prompt.contains("fix the parser"));
        assert_eq!(tool_whitelist(&sp), tools);
        assert!(task.route.escalate_on_fail);
        // read-only worker
        let ro = SpecFields { owns: &[], ..spec(&tools, &success, &owns) };
        assert!(to_task(&ro, "x", vec![], worker_route()).prompt.contains("read-only"));
    }
}
