//! queen — designing workers instead of doing work.
//!
//! The operator's architecture: a **Queen** (a large model) does not perform
//! tasks; it designs **Worker Bees** — small models given one job, one lane
//! and the smallest tool set that job needs. A **Delegator** staffs work to
//! existing or new bees; a **Liaison** narrates to the operator. See
//! `docs/12-queen-and-workers.md` for the full design and the reasoning
//! behind each departure from the original sketch.
//!
//! ## Why this makes a small model better, mechanically
//!
//! Not by making it smarter. By removing the three things that make small
//! models fail in this codebase's own field logs:
//!
//! 1. **Choice.** A 1B given twelve tools picks wrongly; given two it picks
//!    correctly. The spec's `tools` list is a whitelist, not a suggestion.
//! 2. **Scope.** A 1B asked to "fix the renderer" edits five files badly. A
//!    lane (`owns`) turns that into a refusal it can actually make.
//! 3. **Rediscovery.** A 1B re-derives the architecture every turn and runs
//!    out of context doing it. `context` names the few paths that matter, and
//!    worker memory carries forward what was already established.
//!
//! The Queen is expensive and runs rarely — once per worker created, not per
//! turn. That is the whole economic argument: pay a large model once to write
//! a specification, then run a small model many times inside it.
//!
//! ## The tweak that matters most: the spec is a DATA STRUCTURE
//!
//! The sketch has the Queen emit a prose block. Prose is what the consuming
//! model must then interpret — and the consumer here is the smallest model in
//! the system, which is precisely the one that cannot. So [`WorkerSpec`] has a
//! parser and a validator, and a spec that does not parse is rejected and
//! re-asked rather than half-understood. This is the same reasoning that gave
//! `continuation::Command` a fixed vocabulary and `line_calls` a one-line
//! grammar: anything a small model must consume has to be checkable.
//!
//! Pure: text in, spec out, prompt strings out. No model calls, no I/O.

/// How long a worker is meant to live. The Queen decides this at creation,
/// because "is this knowledge worth keeping" is a design question, not a
/// runtime one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Persistence {
    /// One task, then retire. Its notes survive; its identity does not.
    OneShot,
    /// Kept and revived — the worker that knows a subsystem.
    Persistent,
}

/// A worker specification: the Queen's entire output.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkerSpec {
    /// Stable identity, lowercase-hyphenated, used as the memory directory.
    pub name: String,
    /// One sentence. If it needs two, the worker is doing two jobs.
    pub purpose: String,
    /// Paths this worker MAY edit. Empty is legal and means read-only.
    pub owns: Vec<String>,
    /// Paths worth reading for context, in priority order.
    pub context: Vec<String>,
    /// Tool names this worker may call — a whitelist.
    pub tools: Vec<String>,
    /// What it must not do, in the worker's own terms.
    pub constraints: Vec<String>,
    /// How anyone knows it succeeded. Must be checkable (see `validate`).
    pub success: Vec<String>,
    pub persistence: Persistence,
}

/// Why a spec was refused. Named apart because the Queen is re-asked with the
/// reason, and a vague reason produces a vague second attempt.
#[derive(Clone, Debug, PartialEq)]
pub enum SpecError {
    MissingField(&'static str),
    /// A name that cannot be a directory, or collides with an existing worker.
    BadName(String),
    /// Success criteria that no one could check — the failure that makes a
    /// worker unfalsifiable and therefore unretireable.
    UncheckableSuccess(String),
    /// More scope than one worker should hold.
    TooBroad(&'static str),
    /// A tool the runtime does not have.
    UnknownTool(String),
}

impl SpecError {
    /// The sentence handed back to the Queen. Says what to change, not merely
    /// what was wrong — a re-ask that only names the fault gets the same spec
    /// with different words.
    pub fn correction(&self) -> String {
        match self {
            SpecError::MissingField(f) =>
                format!("The specification has no {f}. Add it; a worker without {f} cannot be run."),
            SpecError::BadName(n) =>
                format!("`{n}` is not a usable worker name. Use lowercase words joined by hyphens, \
                         2-40 characters, e.g. `rust-ratatui-coder`."),
            SpecError::UncheckableSuccess(s) =>
                format!("Success criterion {s:?} cannot be checked by anyone. Replace it with \
                         something observable — a command that exits zero, a test that passes, a \
                         file that contains a thing. If the criterion cannot be observed, the \
                         worker can never be retired."),
            SpecError::TooBroad(w) =>
                format!("This worker is too broad: {w}. Split it into separate workers with one \
                         responsibility each; a worker that owns everything focuses on nothing."),
            SpecError::UnknownTool(t) =>
                format!("`{t}` is not a tool this runtime offers. Use only the listed tools; a \
                         worker cannot be given a capability that does not exist."),
        }
    }
}

/// Upper bounds on one worker's scope. Not arbitrary: each is the point past
/// which the focusing effect reverses and the worker is back to being a
/// general agent with extra steps.
pub const MAX_OWNED: usize = 6;
pub const MAX_TOOLS: usize = 6;
pub const MAX_CONTEXT: usize = 12;

/// Phrases that mark a success criterion nobody can check. A worker whose
/// success is "code is clean" cannot be retired, cannot be revived with
/// confidence, and will argue about it.
const UNCHECKABLE: &[&str] = &[
    "clean", "good", "better", "nice", "elegant", "readable", "maintainable",
    "improved", "properly", "correctly", "as appropriate", "makes sense",
];

/// The tool names the workspace runtime actually honours.
///
/// Duplicated here rather than imported because the dependency runs the other way — `logscope-egui`
/// depends on this crate, not the reverse — so the list cannot be shared by reference. It is kept
/// honest by `logscope_egui::workspace_exec`'s parity test, which fails if the two ever disagree;
/// a copy with a test beating on it is better than an inverted dependency for nine strings.
///
/// Before this existed, the ONLY list `validate` was ever called with was a test constant reading
/// `["read_file","read_range","list","log","edit","run_tests"]`. Three of those are not tools —
/// the runtime has `list_dir`, `git_log` and `run` — so a worker spec naming the real tools was
/// rejected as UnknownTool while one naming imaginary tools validated clean.
pub const RUNTIME_TOOLS: &[&str] = &[
    "write_file", "edit", "read_file", "read_range", "read_near", "list_dir", "search", "run",
    "git_log", "doc", "propose", "scratch_code", "make_tool",
];

impl WorkerSpec {
    /// Check a spec against the runtime that would run it. Pass [`RUNTIME_TOOLS`] unless you are
    /// deliberately validating against a restricted surface.
    pub fn validate(&self, known_tools: &[&str]) -> Result<(), SpecError> {
        if self.name.trim().is_empty() { return Err(SpecError::MissingField("name")); }
        let ok_name = self.name.len() >= 2 && self.name.len() <= 40
            && self.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !self.name.starts_with('-') && !self.name.ends_with('-');
        if !ok_name { return Err(SpecError::BadName(self.name.clone())); }
        if self.purpose.trim().is_empty() { return Err(SpecError::MissingField("purpose")); }
        if self.success.is_empty() { return Err(SpecError::MissingField("success criteria")); }
        if self.owns.len() > MAX_OWNED { return Err(SpecError::TooBroad("it owns too many paths")); }
        if self.tools.len() > MAX_TOOLS { return Err(SpecError::TooBroad("it has too many tools")); }
        if self.context.len() > MAX_CONTEXT { return Err(SpecError::TooBroad("it names too much context")); }
        for t in &self.tools {
            if !known_tools.contains(&t.as_str()) {
                return Err(SpecError::UnknownTool(t.clone()));
            }
        }
        for s in &self.success {
            let low = s.to_ascii_lowercase();
            // A criterion is checkable if it names something observable, even
            // if it also contains a soft word ("tests pass cleanly" is fine).
            let observable = low.contains("test") || low.contains("compile") || low.contains("build")
                || low.contains("exit") || low.contains("pass") || low.contains("output")
                || low.contains("file") || low.contains("command") || low.contains("renders")
                || low.contains("returns") || low.contains("no warnings") || low.contains("benchmark");
            if !observable && UNCHECKABLE.iter().any(|w| low.contains(w)) {
                return Err(SpecError::UncheckableSuccess(s.clone()));
            }
        }
        Ok(())
    }

    /// Parse the Queen's emitted block. Deliberately forgiving about layout
    /// and strict about content: the Queen is a large model and will vary its
    /// whitespace, but a field it did not emit must not be invented here.
    ///
    /// Format: `Key: value` lines, list fields as `- item` lines beneath.
    pub fn parse(text: &str) -> Result<WorkerSpec, SpecError> {
        let mut name = String::new();
        let mut purpose = String::new();
        let mut persistence = Persistence::OneShot;
        let (mut owns, mut context, mut tools, mut constraints, mut success) =
            (vec![], vec![], vec![], vec![], vec![]);
        let mut current: Option<&str> = None;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() { continue; }
            if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
                let item = item.trim().to_string();
                match current {
                    Some("owns") => owns.push(item),
                    Some("context") => context.push(item),
                    Some("tools") => tools.push(item),
                    Some("constraints") => constraints.push(item),
                    Some("success") => success.push(item),
                    _ => {}
                }
                continue;
            }
            let Some((k, v)) = line.split_once(':') else { continue };
            let key = k.trim().to_ascii_lowercase().replace(' ', "_");
            let val = v.trim().to_string();
            current = None;
            match key.as_str() {
                "name" => name = val,
                "purpose" => purpose = val,
                "persistence" => {
                    persistence = if val.eq_ignore_ascii_case("persistent") {
                        Persistence::Persistent
                    } else {
                        Persistence::OneShot
                    };
                }
                "owns" | "context" | "tools" | "constraints" | "success" | "success_criteria" => {
                    let field = if key.starts_with("success") { "success" } else {
                        match key.as_str() {
                            "owns" => "owns", "context" => "context",
                            "tools" => "tools", _ => "constraints",
                        }
                    };
                    current = Some(field);
                    // `Tools: read_file, list` on one line is also accepted
                    if !val.is_empty() {
                        let items = val.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                        match field {
                            "owns" => owns.extend(items),
                            "context" => context.extend(items),
                            "tools" => tools.extend(items),
                            "constraints" => constraints.extend(items),
                            _ => success.extend(items),
                        }
                    }
                }
                _ => {}
            }
        }
        if name.is_empty() { return Err(SpecError::MissingField("name")); }
        if purpose.is_empty() { return Err(SpecError::MissingField("purpose")); }
        Ok(WorkerSpec { name, purpose, owns, context, tools, constraints, success, persistence })
    }

    /// The worker's identity segment — the small, stable half of its prompt.
    ///
    /// Kept deliberately short. The sketch's version is eleven lines of
    /// posture; at 1B, posture text competes with the task for attention, so
    /// this states the job, the lane and the refusal, and stops.
    pub fn identity_prompt(&self) -> String {
        let mut s = format!("You are {}. {}\n", self.name, self.purpose);
        if self.owns.is_empty() {
            s.push_str("You do not edit files. You read and report.\n");
        } else {
            s.push_str(&format!("You may edit ONLY: {}. Anything else, you report instead of changing.\n",
                                self.owns.join(", ")));
        }
        if !self.constraints.is_empty() {
            s.push_str("Rules:\n");
            for c in &self.constraints { s.push_str(&format!("- {c}\n")); }
        }
        if !self.success.is_empty() {
            s.push_str("You are done when:\n");
            for c in &self.success { s.push_str(&format!("- {c}\n")); }
        }
        s
    }
}

/// The Queen's own prompt. It designs; it does not solve.
///
/// The single most important line is the refusal to answer the task, because
/// a large model handed a concrete problem will solve it — and then no worker
/// exists, no memory accumulates, and the next identical task pays the large
/// model again.
pub const QUEEN_PROMPT: &str = "\
You are the Queen. You design workers; you never do their work.

Given a task, you decide what kind of worker should exist to do it, and you \
emit ONE specification in exactly this format and nothing else:

Name: lowercase-hyphenated-name
Purpose: one sentence
Owns:
- path it may edit (omit the list entirely if it should not edit anything)
Context:
- path worth reading first
Tools:
- tool name (only from the offered list)
Constraints:
- what it must not do
Success:
- an OBSERVABLE outcome: a command that exits zero, a test that passes, a file that contains something
Persistence: persistent | one-shot

Rules you must follow:
- Do NOT solve the task. If you find yourself writing code, stop and describe the worker who would.
- One responsibility per worker. If the purpose needs the word \"and\", emit the more important half.
- Prefer the smallest tool set that can succeed. Every extra tool is a wrong turn the worker can take.
- Success criteria must be checkable by a machine or by a person reading output. \"Clean code\" is not a criterion.
- Reuse before creating: if an existing worker's purpose covers this, name it instead of inventing one.";

/// What the Liaison says when a worker is created — the operator-facing half.
/// One line, no jargon, names the worker so later references land.
pub fn liaison_announcement(spec: &WorkerSpec) -> String {
    match spec.persistence {
        Persistence::Persistent => format!(
            "A new worker, {}, has been created to {} It will keep what it learns.",
            spec.name, lower_first(&spec.purpose)),
        Persistence::OneShot => format!(
            "A temporary worker, {}, has been created to {} It retires when the task is done.",
            spec.name, lower_first(&spec.purpose)),
    }
}

fn lower_first(s: &str) -> String {
    let s = s.trim();
    let mut c = s.chars();
    let out: String = match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    };
    if out.ends_with('.') { out } else { out + "." }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The real runtime list, not an invented one — see RUNTIME_TOOLS' note on what the invented
    // one was doing to every spec that named a tool the runtime actually has.
    const TOOLS: &[&str] = RUNTIME_TOOLS;

    fn spec_text() -> &'static str {
        "\
Name: rust-ratatui-coder
Purpose: Implement terminal ASCII rendering using Ratatui.
Owns:
- crates/ui/src/render.rs
Context:
- crates/ui/
- docs/ui-architecture.md
Tools:
- read_file
- edit
- run
Constraints:
- do not change the rendering backend
- preserve the existing public API
Success:
- cargo test -p ui exits zero
- the ASCII view renders the grid
Persistence: persistent
"
    }

    #[test]
    fn a_queen_spec_parses_into_fields() {
        let s = WorkerSpec::parse(spec_text()).unwrap();
        assert_eq!(s.name, "rust-ratatui-coder");
        assert_eq!(s.owns, vec!["crates/ui/src/render.rs"]);
        assert_eq!(s.context.len(), 2);
        assert_eq!(s.tools, vec!["read_file", "edit", "run"]);
        assert_eq!(s.constraints.len(), 2);
        assert_eq!(s.success.len(), 2);
        assert_eq!(s.persistence, Persistence::Persistent);
        s.validate(TOOLS).unwrap();
    }

    /// Inline comma lists are accepted too: a large model varies its layout,
    /// and rejecting a spec over formatting wastes the expensive call.
    #[test]
    fn inline_lists_are_accepted() {
        let s = WorkerSpec::parse(
            "Name: a-b\nPurpose: Do one thing.\nTools: read_file, list_dir\nSuccess: tests pass\n").unwrap();
        assert_eq!(s.tools, vec!["read_file", "list_dir"]);
        assert_eq!(s.success, vec!["tests pass"]);
    }

    /// The refusals that keep a worker focused, each named apart because the
    /// Queen is re-asked with the correction text.
    #[test]
    fn scope_and_checkability_are_enforced() {
        let base = WorkerSpec::parse(spec_text()).unwrap();

        let mut broad = base.clone();
        broad.tools = (0..MAX_TOOLS + 1).map(|i| format!("t{i}")).collect();
        assert!(matches!(broad.validate(TOOLS), Err(SpecError::TooBroad(_))));

        let mut owns = base.clone();
        owns.owns = (0..MAX_OWNED + 1).map(|i| format!("p{i}")).collect();
        assert!(matches!(owns.validate(TOOLS), Err(SpecError::TooBroad(_))));

        let mut vague = base.clone();
        vague.success = vec!["the code is clean and maintainable".into()];
        let e = vague.validate(TOOLS).unwrap_err();
        assert!(matches!(e, SpecError::UncheckableSuccess(_)));
        assert!(e.correction().contains("never be retired"));

        let mut unknown = base.clone();
        unknown.tools = vec!["telepathy".into()];
        assert!(matches!(unknown.validate(TOOLS), Err(SpecError::UnknownTool(_))));

        let mut bad = base.clone();
        bad.name = "Rust Ratatui Coder".into();
        assert!(matches!(bad.validate(TOOLS), Err(SpecError::BadName(_))));
    }

    /// A soft word inside an OBSERVABLE criterion is fine — the check must not
    /// reject "tests pass cleanly", or the Queen learns to avoid plain English
    /// rather than to be specific.
    #[test]
    fn soft_words_inside_observable_criteria_are_allowed() {
        let mut s = WorkerSpec::parse(spec_text()).unwrap();
        s.success = vec!["cargo test passes cleanly with no warnings".into()];
        s.validate(TOOLS).unwrap();
    }

    /// A missing field is a named error, not a default. A spec with no success
    /// criteria would create a worker that can never finish.
    #[test]
    fn missing_fields_are_refused_with_a_correction() {
        assert_eq!(WorkerSpec::parse("Purpose: x\n"), Err(SpecError::MissingField("name")));
        let mut s = WorkerSpec::parse(spec_text()).unwrap();
        s.success.clear();
        let e = s.validate(TOOLS).unwrap_err();
        assert!(e.correction().contains("cannot be run"));
    }

    /// The identity prompt states the lane as a REFUSAL a small model can act
    /// on, and a read-only worker is told it does not edit at all.
    #[test]
    fn the_identity_prompt_states_the_lane() {
        let s = WorkerSpec::parse(spec_text()).unwrap();
        let p = s.identity_prompt();
        assert!(p.starts_with("You are rust-ratatui-coder."));
        assert!(p.contains("may edit ONLY: crates/ui/src/render.rs"));
        assert!(p.contains("report instead of changing"));
        assert!(p.contains("You are done when:"));

        let mut ro = s.clone();
        ro.owns.clear();
        assert!(ro.identity_prompt().contains("You do not edit files."));
    }

    #[test]
    fn the_liaison_says_something_a_person_can_follow() {
        let s = WorkerSpec::parse(spec_text()).unwrap();
        let a = liaison_announcement(&s);
        assert!(a.contains("rust-ratatui-coder"));
        assert!(a.contains("keep what it learns"));
        let mut once = s.clone();
        once.persistence = Persistence::OneShot;
        assert!(liaison_announcement(&once).contains("retires when the task is done"));
    }

    /// The Queen's prompt must forbid the failure mode it exists to prevent:
    /// a large model handed a concrete task will simply solve it.
    #[test]
    fn the_queen_is_told_not_to_do_the_work() {
        assert!(QUEEN_PROMPT.contains("never do their work"));
        assert!(QUEEN_PROMPT.contains("Do NOT solve the task"));
        assert!(QUEEN_PROMPT.contains("Reuse before creating"));
    }
}
