//! hive — the beehive matrix: which cell runs a job.
//!
//! `HiveMind_Strategy_Logscope.md` describes a hive of workers over a matrix
//! of models and machines. Three of its four axes already exist in this crate
//! as separate, correct modules, and nothing joined them:
//!
//! * [`crate::workerkit`] — the ROLE axis. `Reach` says whether a role may
//!   cross a network at all: a speculative draft alternates with its target
//!   every few tokens, so a 30 ms round trip is 37.5% overhead and a 150 ms
//!   one is 75% — the draft must be co-resident. A thinker amortises the same
//!   trip over a whole reply (1.8%).
//! * [`crate::capability`] — what a worker IS, so it can be told its job.
//! * [`crate::coordinator`] — the MODE, i.e. how many cells to light up.
//! * [`crate::continuation`] — the verb: what should happen next.
//!
//! The missing axis is the one that costs the most: **KV warmth**. A cell that
//! already holds the prefix for this prompt starts generating immediately; an
//! identical cell that does not must prefill first, and prefill is the
//! expensive pass (the field log's 1625-token first turn: 66 s of prefill
//! before a single token). The engine's prefix tree already measures this per
//! cache (`kvtree::pick_anchor`, pool cap 3). This module makes it a
//! placement input rather than a per-machine accident.
//!
//! ## The cell
//!
//! A [`Cell`] is one (model, node) pair with a KV state. The matrix is the set
//! of cells the hive can currently reach. Placement answers: given a [`Job`],
//! which cell should run it — or that none should, which is a real answer.
//!
//! ## The rule, in order
//!
//! 1. **Hard reach.** A co-resident role must land on the node holding its
//!    partner. No warmth or speed makes a remote draft correct.
//! 2. **Capability.** A cell whose model cannot do the job is not a candidate.
//! 3. **Warmth, then cost.** Among the survivors, prefer the cell that avoids
//!    a prefill; break ties by the node's own throughput, then by lowest
//!    latency. Warmth wins over raw speed by design: a faster machine that
//!    must prefill 1600 tokens loses to a slower one that must not, which is
//!    exactly the case a naive "fastest node" scheduler gets wrong.
//!
//! Pure: facts in, placement out. No I/O, no model calls, no clock — the same
//! discipline `continuation` keeps, and for the same reason (a scheduler that
//! costs a round trip to decide is one that gets switched off).

use crate::workerkit::Reach;

/// A machine in the hive.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    /// Round-trip latency to the coordinator, milliseconds. 0 for local.
    pub rtt_ms: u32,
    /// Rough decode throughput, tokens/sec, for tie-breaking only.
    pub tok_s: f32,
}

/// One cell of the matrix: a model resident on a node, with whatever KV it
/// currently holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub node: Node,
    /// The model's label — `read_harness`-style, so `8x7B` and `30B-A3B` parse.
    pub model: String,
    /// Longest prefix (in tokens) this cell's KV pool shares with the job's
    /// prompt. From the engine's own `pick_anchor`; 0 means a cold prefill.
    pub warm_prefix: usize,
    /// Whether this cell is already busy with another job.
    pub busy: bool,
}

/// What the hive is being asked to run.
#[derive(Clone, Debug)]
pub struct Job {
    /// The role, which decides whether the network is allowed at all.
    pub reach: Reach,
    /// The node holding this job's partner, for co-resident roles (a
    /// speculative draft's target, a verifier's subject).
    pub partner_node: Option<String>,
    /// Prompt length in tokens — what would have to be prefilled from cold.
    pub prompt_tokens: usize,
    /// Model labels acceptable for this job; empty means any.
    pub models: Vec<String>,
}

/// Why a cell was chosen, so a log line can explain a placement instead of
/// asserting one.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub cell: Cell,
    pub reason: String,
    /// Tokens this placement avoids prefilling versus a cold cell.
    pub saved_prefill: usize,
}

/// Choose a cell for `job`, or `None` when the matrix cannot serve it.
pub fn place(job: &Job, matrix: &[Cell]) -> Option<Placement> {
    let mut best: Option<(f64, Placement)> = None;
    for c in matrix {
        if c.busy {
            continue;
        }
        // 1. hard reach
        match job.reach {
            Reach::Colocated => {
                let Some(p) = job.partner_node.as_deref() else { continue };
                if c.node.id != p {
                    continue;
                }
            }
            _ => {}
        }
        // 2. capability
        if !job.models.is_empty() && !job.models.iter().any(|m| m == &c.model) {
            continue;
        }
        // 3. warmth, then cost. Score in seconds-ish: prefill dominates.
        let cold = job.prompt_tokens.saturating_sub(c.warm_prefix);
        let prefill_s = cold as f64 / (c.node.tok_s.max(0.1) as f64 * 8.0); // prefill ~8x decode
        let rtt_s = c.node.rtt_ms as f64 / 1000.0;
        let score = prefill_s + rtt_s;
        let saved = c.warm_prefix.min(job.prompt_tokens);
        let reason = if saved > 0 {
            format!("{} on {} — {saved} of {} prompt tokens already warm ({}ms rtt)",
                    c.model, c.node.id, job.prompt_tokens, c.node.rtt_ms)
        } else {
            format!("{} on {} — cold, {} tokens to prefill at {:.0} tok/s ({}ms rtt)",
                    c.model, c.node.id, cold, c.node.tok_s, c.node.rtt_ms)
        };
        let p = Placement { cell: c.clone(), reason, saved_prefill: saved };
        if best.as_ref().map(|(s, _)| score < *s).unwrap_or(true) {
            best = Some((score, p));
        }
    }
    best.map(|(_, p)| p)
}

/// Why no cell could serve a job — the operator-facing half of `place`
/// returning `None`. Distinguishing these matters: "everything is busy" is a
/// wait, "no model matches" is a configuration error, and "the partner's node
/// has no free cell" is a co-residency squeeze that adding remote machines
/// will not fix.
pub fn explain_empty(job: &Job, matrix: &[Cell]) -> String {
    if matrix.is_empty() {
        return "the hive is empty — no workers are enlisted".into();
    }
    let free = matrix.iter().filter(|c| !c.busy).count();
    if free == 0 {
        return format!("all {} cells are busy — this is a wait, not a failure", matrix.len());
    }
    if job.reach == Reach::Colocated {
        if let Some(p) = job.partner_node.as_deref() {
            let on_node = matrix.iter().filter(|c| c.node.id == p && !c.busy).count();
            if on_node == 0 {
                return format!(
                    "this role must be co-resident with its partner on {p}, which has no free cell. \
                     Adding remote machines cannot help: a draft alternating with its target pays the \
                     round trip PER TOKEN (see workerkit)");
            }
        } else {
            return "a co-resident role was given no partner node — the caller must say what it \
                    must sit beside".into();
        }
    }
    if !job.models.is_empty() {
        return format!("no free cell runs any of {:?} — this is configuration, not load", job.models);
    }
    "no cell matched, and the rules above do not explain why — this is a bug in place()".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, rtt: u32, tok_s: f32) -> Node {
        Node { id: id.into(), rtt_ms: rtt, tok_s }
    }
    fn cell(n: Node, model: &str, warm: usize) -> Cell {
        Cell { node: n, model: model.into(), warm_prefix: warm, busy: false }
    }

    /// The placement a naive scheduler gets wrong: a FASTER machine that must
    /// prefill loses to a slower one that is already warm. This is the whole
    /// reason KV warmth is an axis of the matrix rather than a local detail.
    #[test]
    fn warmth_beats_raw_speed() {
        let matrix = vec![
            cell(node("m2ultra", 30, 60.0), "Qwen3-14B", 0),
            cell(node("mini", 30, 20.0), "Qwen3-14B", 1600),
        ];
        let job = Job { reach: Reach::Lan, partner_node: None, prompt_tokens: 1700, models: vec![] };
        let p = place(&job, &matrix).expect("a cell must serve this");
        assert_eq!(p.cell.node.id, "mini", "the warm slower node must win: {}", p.reason);
        assert_eq!(p.saved_prefill, 1600);
        assert!(p.reason.contains("already warm"));
    }

    /// …but only when the warmth is real. With nothing warm anywhere, the
    /// fastest node wins, so the rule degrades to the obvious one.
    #[test]
    fn with_no_warmth_the_fast_node_wins() {
        let matrix = vec![
            cell(node("m2ultra", 30, 60.0), "Qwen3-14B", 0),
            cell(node("mini", 30, 20.0), "Qwen3-14B", 0),
        ];
        let job = Job { reach: Reach::Lan, partner_node: None, prompt_tokens: 1700, models: vec![] };
        let p = place(&job, &matrix).unwrap();
        assert_eq!(p.cell.node.id, "m2ultra", "{}", p.reason);
        assert_eq!(p.saved_prefill, 0);
        assert!(p.reason.contains("cold"));
    }

    /// Reach is HARD: a co-resident role may not be placed off its partner's
    /// node however warm or fast the alternative is. The tempting cell here is
    /// both warm AND fast, and must still lose.
    #[test]
    fn a_colocated_role_never_leaves_its_partner() {
        let matrix = vec![
            cell(node("elsewhere", 30, 90.0), "draft-1B", 1700),
            cell(node("host", 0, 10.0), "draft-1B", 0),
        ];
        let job = Job {
            reach: Reach::Colocated,
            partner_node: Some("host".into()),
            prompt_tokens: 1700,
            models: vec![],
        };
        let p = place(&job, &matrix).unwrap();
        assert_eq!(p.cell.node.id, "host",
                   "a speculative draft was placed across a network: {}", p.reason);
    }

    #[test]
    fn model_requirements_and_busy_cells_are_respected() {
        let matrix = vec![
            cell(node("a", 0, 50.0), "Qwen3-14B", 900),
            Cell { busy: true, ..cell(node("b", 0, 90.0), "deepseek-coder-6.7b", 1700) },
            cell(node("c", 40, 30.0), "deepseek-coder-6.7b", 0),
        ];
        let job = Job {
            reach: Reach::Lan,
            partner_node: None,
            prompt_tokens: 1700,
            models: vec!["deepseek-coder-6.7b".into()],
        };
        let p = place(&job, &matrix).unwrap();
        assert_eq!(p.cell.node.id, "c", "the busy warm cell must not be taken: {}", p.reason);
    }

    /// An empty answer must be explainable, and the three causes must not be
    /// confused: a wait, a misconfiguration, and a co-residency squeeze call
    /// for different operator actions.
    #[test]
    fn the_three_empty_causes_are_named_apart() {
        let job = |reach, partner: Option<&str>, models: Vec<String>| Job {
            reach, partner_node: partner.map(str::to_string), prompt_tokens: 100, models,
        };
        assert!(explain_empty(&job(Reach::Lan, None, vec![]), &[]).contains("hive is empty"));

        let busy = vec![Cell { busy: true, ..cell(node("a", 0, 50.0), "m", 0) }];
        assert!(explain_empty(&job(Reach::Lan, None, vec![]), &busy).contains("wait, not a failure"));

        let free = vec![cell(node("a", 0, 50.0), "m", 0)];
        let e = explain_empty(&job(Reach::Lan, None, vec!["other".into()]), &free);
        assert!(e.contains("configuration, not load"), "{e}");

        let e = explain_empty(&job(Reach::Colocated, Some("host"), vec![]), &free);
        assert!(e.contains("co-resident") && e.contains("PER TOKEN"), "{e}");
    }
}
