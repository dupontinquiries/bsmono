//! bridge.rs — connect the conversation-graph work to the distributed dispatcher.
//!
//! Two shapes the app already produces, turned into `dispatch::Task`s and folded back:
//!   • decompose: a skeleton with `todo!()` holes → one task per hole (payload = its subtask prompt);
//!     the members' replies → `extract_body` each → `assemble` back into the finished artifact.
//!   • fan-out: one prompt → K identical tasks (the `:fa` "K candidates" pattern, spread across machines);
//!     the K replies come back for the existing best-of selection.
//!
//! These are pure data transforms (`find_holes`/`subtask_prompt`/`extract_body`/`assemble`) — no IO,
//! fully unit-tested. Between the two halves the caller drives a `dispatch::Dispatcher` over any
//! `Transport`/`Executor`; folding the finished artifact (or the candidates) back into the conversation
//! graph is the thin gui step. No kernels.
#![allow(dead_code)]

use crate::decompose::{assemble, extract_body, find_holes, subtask_prompt, Hole};
use crate::dispatch::Task;
use std::collections::HashMap;

/// Decompose a skeleton into one task per hole. Returns the tasks (task id = hole index) and the holes,
/// which `assemble_results` needs to splice the replies back. Empty when the skeleton has no holes.
pub fn tasks_from_skeleton(skeleton: &str) -> (Vec<Task>, Vec<Hole>) {
    let holes = find_holes(skeleton);
    let tasks = holes.iter().enumerate()
        .map(|(i, h)| Task { id: i as u64, payload: subtask_prompt(skeleton, h) })
        .collect();
    (tasks, holes)
}

/// Fold the members' replies into the finished artifact: each hole's reply (`results[hole index]`) goes
/// through `extract_body` to become its fill, then all are `assemble`d into the skeleton. `None` if any
/// hole has no reply (the run is incomplete — e.g. a task was abandoned).
pub fn assemble_results(skeleton: &str, holes: &[Hole], results: &HashMap<u64, String>) -> Option<String> {
    let mut fills = Vec::with_capacity(holes.len());
    for (i, h) in holes.iter().enumerate() {
        let reply = results.get(&(i as u64))?;
        fills.push((h.clone(), extract_body(reply)));
    }
    Some(assemble(skeleton, &fills))
}

/// Fan one prompt into `k` identical tasks — the `:fa` K-candidates pattern, distributed across members.
pub fn tasks_fanout(prompt: &str, k: usize) -> Vec<Task> {
    (0..k as u64).map(|id| Task { id, payload: prompt.to_string() }).collect()
}

/// Collect fan-out replies in candidate order (0..k), each through `extract_body`. Missing candidates are
/// skipped (a member may have failed past its retries), so the result may be shorter than `k`.
pub fn collect_fanout(k: usize, results: &HashMap<u64, String>) -> Vec<String> {
    (0..k as u64).filter_map(|id| results.get(&id).map(|r| extract_body(r))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKEL: &str = "impl C {\n  fn a() -> u32 { todo!() }\n  fn b() -> u32 { todo!() }\n}";

    #[test]
    fn tasks_from_skeleton_one_per_hole() {
        let (tasks, holes) = tasks_from_skeleton(SKEL);
        assert_eq!(tasks.len(), 2);
        assert_eq!(holes.len(), 2);
        assert_eq!(tasks[0].id, 0);
        assert_eq!(tasks[1].id, 1);
        assert!(!tasks[0].payload.is_empty(), "payload is the subtask prompt");
    }

    #[test]
    fn assemble_results_folds_replies_into_artifact() {
        let (_tasks, holes) = tasks_from_skeleton(SKEL);
        let mut results = HashMap::new();
        results.insert(0u64, "```rust\n7\n```".to_string()); // fenced reply
        results.insert(1u64, "9".to_string());               // bare reply
        let art = assemble_results(SKEL, &holes, &results).expect("complete");
        assert!(!art.contains("todo!()"), "all holes filled: {art}");
        assert!(art.contains('7') && art.contains('9'));
    }

    #[test]
    fn assemble_results_incomplete_returns_none() {
        let (_t, holes) = tasks_from_skeleton(SKEL);
        let mut results = HashMap::new();
        results.insert(0u64, "7".to_string()); // hole 1 missing
        assert!(assemble_results(SKEL, &holes, &results).is_none());
    }

    #[test]
    fn fanout_makes_k_identical_tasks_and_collects_in_order() {
        let tasks = tasks_fanout("write a haiku", 3);
        assert_eq!(tasks.len(), 3);
        assert!(tasks.iter().all(|t| t.payload == "write a haiku"));
        assert_eq!(tasks.iter().map(|t| t.id).collect::<Vec<_>>(), vec![0, 1, 2]);
        let mut results = HashMap::new();
        results.insert(0u64, "first".to_string());
        results.insert(2u64, "third".to_string()); // candidate 1 missing (member failed)
        assert_eq!(collect_fanout(3, &results), vec!["first".to_string(), "third".to_string()]);
    }

    // minimal in-memory transport (mirrors dispatch's loopback) for the end-to-end test
    use crate::dispatch::{Dispatcher, Executor, Worker};
    use crate::enlist::{Msg, PeerId, Transport};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Bus { inbox: HashMap<PeerId, Vec<(PeerId, Msg)>>, peers: Vec<PeerId> }
    struct Loop { me: PeerId, bus: Rc<RefCell<Bus>> }
    impl Transport for Loop {
        fn broadcast(&mut self, m: &Msg) {
            let peers = self.bus.borrow().peers.clone();
            let mut b = self.bus.borrow_mut();
            for p in peers { if p != self.me { b.inbox.entry(p).or_default().push((self.me.clone(), m.clone())); } }
        }
        fn send(&mut self, to: &PeerId, m: &Msg) {
            self.bus.borrow_mut().inbox.entry(to.clone()).or_default().push((self.me.clone(), m.clone()));
        }
        fn recv(&mut self) -> Vec<(PeerId, Msg)> {
            self.bus.borrow_mut().inbox.get_mut(&self.me).map(std::mem::take).unwrap_or_default()
        }
    }
    // every assigned hole comes back filled with a fenced "42"
    struct Fill42;
    impl Executor for Fill42 {
        fn run(&mut self, _task: u64, _payload: &str) -> Result<String, String> { Ok("```rust\n42\n```".to_string()) }
    }

    #[test]
    fn distributed_decompose_roundtrip_assembles_artifact() {
        // skeleton → tasks → dispatch across two members (mock ollama) → results → assemble
        let (tasks, holes) = tasks_from_skeleton(SKEL);
        let cid = "c:1".to_string();
        let mids = ["m0:1", "m1:1"];
        let bus = Rc::new(RefCell::new(Bus {
            inbox: HashMap::new(),
            peers: std::iter::once(cid.clone()).chain(mids.iter().map(|s| s.to_string())).collect(),
        }));
        let mut ct = Loop { me: cid.clone(), bus: bus.clone() };
        let members: Vec<(PeerId, u32)> = mids.iter().map(|m| (m.to_string(), 1)).collect();
        let mut disp = Dispatcher::new(&cid, &members, tasks, 1);
        let mut workers: Vec<(Worker, Loop, Fill42)> = mids.iter()
            .map(|id| (Worker::new(id), Loop { me: id.to_string(), bus: bus.clone() }, Fill42)).collect();

        disp.dispatch(&mut ct);
        for _ in 0..10 {
            for (w, wt, ex) in workers.iter_mut() { w.work(wt, ex); }
            disp.pump(&mut ct);
            disp.dispatch(&mut ct);
            if disp.is_complete() { break; }
        }
        assert!(disp.is_complete(), "all holes dispatched and returned");
        let art = assemble_results(SKEL, &holes, disp.results()).expect("artifact assembled");
        assert!(!art.contains("todo!()"));
        assert_eq!(art.matches("42").count(), 2, "both holes filled with the remote output");
    }
}
