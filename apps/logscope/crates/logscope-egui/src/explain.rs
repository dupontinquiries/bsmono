//! explain.rs — the layer that makes logscope feel like a conversational explanation of a codebase.
//!
//! Everything else in this stack is *generic* orchestration over opaque prompts. This module grounds it
//! in code: a question plus the relevant [`CodeRef`]s (a file/symbol/snippet — supplied by the GUI side
//! from `ast::extract` + `retrieval`, the seam) becomes a set of code-anchored **explanation lanes**, each
//! a tight prompt a small model can answer *given the snippet*, covering a different facet (what it does,
//! how it works, what it touches, where it breaks, how it's tested, why it's built this way, and what
//! you'll probably ask next). The lanes run on the fleet (`dispatch`/`bridge`); [`assemble`] folds the
//! results into an [`Explanation`] — a conversational lead, ordered detail sections, and anticipated
//! follow-ups — ready to render as conversation-graph nodes.
//!
//! Pure + unit-tested. Seams: upstream retrieval/AST produce the `CodeRef`s; downstream the fleet runs the
//! tasks and the GUI renders the `Explanation` into `convograph`.
#![allow(dead_code)]

use crate::dispatch::Task;
use std::collections::HashMap;

/// Symbol kind — mirrors `ast::Kind` (which is gui-gated) so the GUI adapter maps 1:1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymKind { Fn, Struct, Enum, Trait, Impl, Mod, Const, Static, TypeAlias, Other }
impl SymKind {
    pub fn noun(self) -> &'static str {
        match self {
            SymKind::Fn => "function", SymKind::Struct => "struct", SymKind::Enum => "enum",
            SymKind::Trait => "trait", SymKind::Impl => "impl block", SymKind::Mod => "module",
            SymKind::Const => "constant", SymKind::Static => "static", SymKind::TypeAlias => "type alias",
            SymKind::Other => "item",
        }
    }
}

/// A grounded reference to a piece of code — the unit the explanation is anchored to.
#[derive(Clone, Debug)]
pub struct CodeRef {
    pub file: String,
    pub symbol: Option<String>,
    pub kind: SymKind,
    pub snippet: String, // the code body, used as model context (may be large; trimmed per lane)
    pub line: u32,       // 1-based start line, for citation
}
impl CodeRef {
    fn label(&self) -> String {
        match &self.symbol {
            Some(s) => format!("`{}` ({} in {}:{})", s, self.kind.noun(), self.file, self.line),
            None => format!("{} ({})", self.file, self.kind.noun()),
        }
    }
}

/// The explanation angles, in conversational priority order (lower = comes first in the answer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facet { Overview, Walkthrough, DataFlow, Dependencies, Callers, EdgeCases, Tests, Rationale, Followups }
impl Facet {
    fn priority(self) -> u8 {
        match self {
            Facet::Overview => 0, Facet::Walkthrough => 1, Facet::DataFlow => 2, Facet::Dependencies => 3,
            Facet::Callers => 4, Facet::EdgeCases => 5, Facet::Tests => 6, Facet::Rationale => 7,
            Facet::Followups => 8,
        }
    }
    pub fn heading(self) -> &'static str {
        match self {
            Facet::Overview => "Overview", Facet::Walkthrough => "How it works",
            Facet::DataFlow => "Data flow", Facet::Dependencies => "Depends on",
            Facet::Callers => "Used by", Facet::EdgeCases => "Edge cases & failure modes",
            Facet::Tests => "Testing", Facet::Rationale => "Why it's built this way",
            Facet::Followups => "You might next ask",
        }
    }
    /// A sensible conversational default: lead, mechanism, then the things people ask next.
    pub fn default_set() -> Vec<Facet> {
        vec![Facet::Overview, Facet::Walkthrough, Facet::EdgeCases, Facet::Dependencies, Facet::Followups]
    }
}

#[derive(Clone, Debug)]
pub struct ExplainConfig {
    pub facets: Vec<Facet>,
    pub max_lanes: usize,
    pub snippet_budget: usize, // max chars of snippet inlined per prompt
}
impl Default for ExplainConfig {
    fn default() -> Self { Self { facets: Facet::default_set(), max_lanes: 24, snippet_budget: 1200 } }
}

/// One grounded explanation prompt: a facet applied to a specific code reference.
#[derive(Clone, Debug)]
pub struct ExplainLane { pub id: u64, pub facet: Facet, pub target_idx: usize, pub prompt: String }

/// Build the grounded prompt for `facet` over `cref`, with the question for context and the snippet
/// trimmed to `budget`. Tight and small-model-friendly: one focused ask plus the code.
fn prompt_for(question: &str, cref: &CodeRef, facet: Facet, budget: usize) -> String {
    let label = cref.label();
    let ask = match facet {
        Facet::Overview => format!("In 2–3 plain, conversational sentences, what does {label} do and why does it exist?"),
        Facet::Walkthrough => format!("Walk through how {label} works, step by step, in plain language. Keep it tight."),
        Facet::DataFlow => format!("Trace the data through {label}: the inputs, the transformations, and what comes out."),
        Facet::Dependencies => format!("What does {label} call or depend on, and why? Name the key ones only."),
        Facet::Callers => format!("What kinds of code would call {label}, and for what? (Call sites may not be shown.)"),
        Facet::EdgeCases => format!("What are the edge cases, error paths, and failure modes in {label} that matter?"),
        Facet::Tests => format!("How would you test {label}? What behaviour needs coverage and what's tricky?"),
        Facet::Rationale => format!("Why might {label} be written this way? Note any non-obvious design choices."),
        Facet::Followups => format!("After understanding {label}, what are the 3 questions someone is most likely to ask next? One per line, no preamble."),
    };
    let snippet = trim_chars(cref.snippet.trim(), budget);
    // ground the facet in what the user actually asked (the doc contract), so the explanation is relevant
    // to their question rather than a generic description of the symbol.
    let q = question.trim();
    let ctx = if q.is_empty() { String::new() } else { format!("Context — the user asked: {q}\n\n") };
    if snippet.is_empty() { format!("{ctx}{ask}") } else { format!("{ctx}{ask}\n\n```\n{snippet}\n```") }
}

fn trim_chars(s: &str, budget: usize) -> String {
    if s.chars().count() <= budget { return s.to_string(); }
    let mut out: String = s.chars().take(budget.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Generate the explanation lanes for `question` grounded in `refs`. Facet-major (breadth first): every
/// ref's Overview before any deep dive, so a tight budget still explains the whole picture before
/// drilling in. Capped at `cfg.max_lanes`; ids assigned 0..n.
pub fn explanation_lanes(question: &str, refs: &[CodeRef], cfg: &ExplainConfig) -> Vec<ExplainLane> {
    let mut lanes = Vec::new();
    if refs.is_empty() { return lanes; }
    for &facet in &cfg.facets {
        for (idx, cref) in refs.iter().enumerate() {
            if lanes.len() >= cfg.max_lanes { return lanes; }
            lanes.push(ExplainLane {
                id: lanes.len() as u64, facet, target_idx: idx,
                prompt: prompt_for(question, cref, facet, cfg.snippet_budget),
            });
        }
    }
    lanes
}

/// Lanes → fleet tasks (payload = the grounded prompt).
pub fn tasks(lanes: &[ExplainLane]) -> Vec<Task> {
    lanes.iter().map(|l| Task { id: l.id, payload: l.prompt.clone() }).collect()
}

/// One answered facet, anchored to its code reference.
#[derive(Clone, Debug)]
pub struct Section { pub facet: Facet, pub target: CodeRef, pub body: String }

/// The assembled, conversational explanation.
#[derive(Debug, Default)]
pub struct Explanation {
    pub question: String,
    pub overview: String,        // the lead (joined Overview facets)
    pub sections: Vec<Section>,  // detail, ordered by conversational priority then file
    pub follow_ups: Vec<String>, // anticipated next questions (from the Followups facet)
}

/// Fold the fleet's results (task id → output) back into a coherent explanation: Overview becomes the
/// lead, Followups becomes the anticipated next questions, and the remaining facets become detail
/// sections ordered the way a person would explain it (mechanism before nitpicks).
pub fn assemble(question: &str, lanes: &[ExplainLane], refs: &[CodeRef], results: &HashMap<u64, String>) -> Explanation {
    let mut overview_parts = Vec::new();
    let mut follow_ups = Vec::new();
    let mut sections = Vec::new();
    for l in lanes {
        let Some(out) = results.get(&l.id) else { continue };
        let out = out.trim();
        if out.is_empty() { continue; }
        match l.facet {
            Facet::Overview => overview_parts.push(out.to_string()),
            Facet::Followups => follow_ups.extend(out.lines().map(|s| s.trim_start_matches(['-', '*', ' ']).to_string()).filter(|s| !s.is_empty())),
            _ => sections.push(Section { facet: l.facet, target: refs[l.target_idx].clone(), body: out.to_string() }),
        }
    }
    // conversational order: by facet priority, then by file/line for stability
    sections.sort_by(|a, b| a.facet.priority().cmp(&b.facet.priority())
        .then(a.target.file.cmp(&b.target.file)).then(a.target.line.cmp(&b.target.line)));
    Explanation { question: question.to_string(), overview: overview_parts.join("\n\n"), sections, follow_ups }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs() -> Vec<CodeRef> {
        vec![
            CodeRef { file: "src/parse.rs".into(), symbol: Some("parse_csv".into()), kind: SymKind::Fn,
                      snippet: "fn parse_csv(s: &str) -> Vec<Row> { /* … */ }".into(), line: 12 },
            CodeRef { file: "src/model.rs".into(), symbol: Some("Row".into()), kind: SymKind::Struct,
                      snippet: "struct Row { region: String, n: u64 }".into(), line: 3 },
        ]
    }

    #[test]
    fn lanes_are_grounded_and_breadth_first() {
        let cfg = ExplainConfig { facets: vec![Facet::Overview, Facet::EdgeCases], max_lanes: 24, snippet_budget: 1200 };
        let lanes = explanation_lanes("how does csv parsing work?", &refs(), &cfg);
        assert_eq!(lanes.len(), 4, "2 facets × 2 refs");
        // facet-major: both Overviews first
        assert_eq!(lanes[0].facet, Facet::Overview);
        assert_eq!(lanes[1].facet, Facet::Overview);
        assert_eq!(lanes[2].facet, Facet::EdgeCases);
        // grounded: prompt names the symbol/file and inlines the snippet
        assert!(lanes[0].prompt.contains("parse_csv") && lanes[0].prompt.contains("src/parse.rs"));
        assert!(lanes[0].prompt.contains("```") && lanes[0].prompt.contains("fn parse_csv"));
        // and grounded in the USER'S QUESTION (the prompt_for contract), not just a generic ask
        assert!(lanes[0].prompt.contains("how does csv parsing work?"), "prompt carries the question for context");
    }

    #[test]
    fn budget_caps_and_snippet_is_trimmed() {
        let big = CodeRef { file: "f.rs".into(), symbol: Some("g".into()), kind: SymKind::Fn,
                            snippet: "x".repeat(5000), line: 1 };
        let cfg = ExplainConfig { facets: Facet::default_set(), max_lanes: 3, snippet_budget: 100 };
        let lanes = explanation_lanes("q", std::slice::from_ref(&big), &cfg);
        assert_eq!(lanes.len(), 3, "capped at max_lanes");
        let body = &lanes[0].prompt;
        assert!(body.contains('…') && body.chars().count() < 300, "snippet trimmed to budget");
    }

    #[test]
    fn tasks_carry_the_grounded_prompts() {
        let lanes = explanation_lanes("q", &refs(), &ExplainConfig::default());
        let ts = tasks(&lanes);
        assert_eq!(ts.len(), lanes.len());
        assert!(ts.iter().zip(&lanes).all(|(t, l)| t.id == l.id && t.payload == l.prompt));
    }

    #[test]
    fn assemble_builds_lead_sections_and_followups() {
        let cfg = ExplainConfig { facets: vec![Facet::Overview, Facet::EdgeCases, Facet::Followups], max_lanes: 24, snippet_budget: 1200 };
        let r = refs();
        let lanes = explanation_lanes("explain parsing", &r, &cfg);
        // mock fleet outputs keyed by lane id
        let mut results = HashMap::new();
        for l in &lanes {
            let out = match l.facet {
                Facet::Overview => format!("{} reads text into rows.", r[l.target_idx].symbol.clone().unwrap()),
                Facet::EdgeCases => "Empty input and ragged rows are the risks.".to_string(),
                Facet::Followups => "How are errors reported?\nIs it streaming?".to_string(),
                _ => String::new(),
            };
            results.insert(l.id, out);
        }
        let ex = assemble("explain parsing", &lanes, &r, &results);
        assert!(ex.overview.contains("parse_csv") && ex.overview.contains("Row"), "lead joins both overviews");
        assert!(ex.sections.iter().all(|s| s.facet == Facet::EdgeCases), "non-lead/non-followup facets become sections");
        assert_eq!(ex.sections.len(), 2);
        assert!(ex.follow_ups.contains(&"How are errors reported?".to_string()));
        // overview sorts ahead of edge cases by priority (it's pulled out as the lead, sections start at EdgeCases)
        assert_eq!(ex.sections[0].facet, Facet::EdgeCases);
    }

    #[test]
    fn no_refs_no_lanes() {
        assert!(explanation_lanes("q", &[], &ExplainConfig::default()).is_empty());
    }
}
