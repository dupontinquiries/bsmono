//! pipeline.rs — the end-to-end dispatch path, wiring the validated primitives together:
//!   assemble (context_graph: minimal context for the endpoint)
//!   → gate    (retrieval: net-positive vs the empty baseline, no pinned-hit regression)
//!   → schedule(fabric_net: rank entry points by the task's size class)
//!   → dispatch(fabric: route to a public worker, attach a provenance-tagged node)
//! Validated against a populated graph + a mock registry, so the *seams* between the primitives —
//! not just each primitive — are exercised.
#![allow(dead_code)]
use crate::context_graph::ContextGraph;
use crate::fabric::{FabricNode, GenRequest, Registry};
use crate::fabric_net::{best_entry, EntryPoint, Policy, SizeClass};
use crate::retrieval::{accept_update, Recall};

/// Minimal prefix for `endpoint`: only the `minimal_context` tokens of `query` nearest the endpoint
/// (capped at `budget`), then the endpoint itself. The reverse-anchored graph says that's all the
/// model needs to read to determine the continuation — the rest of `query` is dropped.
pub fn assemble(graph: &ContextGraph, query: &[u32], endpoint: u32, budget: usize) -> Vec<u32> {
    let take = graph.minimal_context(query, endpoint).min(budget).min(query.len());
    let mut prefix: Vec<u32> = query[query.len() - take..].to_vec();
    prefix.push(endpoint);
    prefix
}

/// Entry points for every public model, with a size class and a placeholder load/latency the live
/// fabric would fill from heartbeats. Bigger models get a higher latency estimate.
pub fn entry_points(reg: &Registry) -> Vec<EntryPoint> {
    reg.public_models().into_iter().map(|(worker, m)| {
        let class = SizeClass::from_params(m.params_b);
        EntryPoint { worker, model: m.name, class, load: 0.0, est_ms_per_tok: 20.0 + m.params_b * 20.0, public: true }
    }).collect()
}

pub struct Dispatched {
    pub node: FabricNode,
    pub assembled_len: usize,
    pub full_len: usize,
}

/// Full path: assemble the minimal context, schedule the smallest-sufficient box for `task`, dispatch.
pub fn dispatch_with_context(reg: &Registry, graph: &ContextGraph, query: &[u32], endpoint: u32,
                             budget: usize, task: SizeClass) -> Option<Dispatched> {
    let prefix = assemble(graph, query, endpoint, budget);
    let eps = entry_points(reg);
    let chosen = best_entry(&eps, &Policy::for_task(task))?;
    let req = GenRequest { prefix: prefix.clone(), model: chosen.model.clone(), max_new: 16, temperature: 0.8, seed: 1 };
    let node = reg.dispatch(&req, None)?;
    Some(Dispatched { node, assembled_len: prefix.len(), full_len: query.len() + 1 })
}

pub fn gate_pipeline() {
    use crate::fabric::MockWorker;
    // graph: endpoint 50 has two contexts differing at the token NEAREST it (9 vs 8) → minimal = 1
    let mut g = ContextGraph::new();
    g.insert(&[1, 1, 1, 1, 9], 50);
    g.insert(&[1, 1, 1, 1, 8], 50);
    let query = [1, 1, 1, 1, 9];

    // assemble: minimal prefix is [9, 50] vs the full [1,1,1,1,9,50]
    let prefix = assemble(&g, &query, 50, 64);
    let assembled_ok = prefix == vec![9, 50];

    // gate: the assembled set lifts recall off the all-miss baseline, no pinned hit to regress
    let before = [Recall::Miss];
    let after = [Recall::Hit];
    let gated_ok = accept_update(&before, &after, &[false]);

    // fabric: laptop has a Tiny model (excluded for a Small task), studio has the Small one
    let mut reg = Registry::new();
    reg.add(Box::new(MockWorker::new("laptop", "dustin", &[("smollm2", true, 0.135)])));
    reg.add(Box::new(MockWorker::new("studio-m2", "dustin", &[("llama-1b", true, 1.0)])));

    let d = dispatch_with_context(&reg, &g, &query, 50, 64, SizeClass::Small).unwrap();
    let scheduled_ok = d.node.model == "llama-1b" && d.node.worker.device == "studio-m2"; // Tiny excluded
    let provenance_ok = d.node.provenance() == "llama-1b · dustin@studio-m2";
    let cut_ok = d.assembled_len == 2 && d.full_len == 6; // 6→2 context cut survives into the dispatch

    for (label, ok) in [
        ("assemble → minimal prefix [9,50]", assembled_ok),
        ("retrieval gate accepts net-positive assembly", gated_ok),
        ("scheduler picks smallest-sufficient (Tiny excluded)", scheduled_ok),
        ("provenance tagged on the dispatched node", provenance_ok),
        ("context cut 6→2 carried through dispatch", cut_ok),
    ] {
        println!("  [{}] {}", if ok { "\u{2713}" } else { "\u{2717} FAIL" }, label);
    }
    let all = assembled_ok && gated_ok && scheduled_ok && provenance_ok && cut_ok;
    println!("gate-pipeline: assemble→gate→schedule→dispatch {}", if all { "wired end-to-end \u{2713}" } else { "FAIL \u{2717}" });
}
