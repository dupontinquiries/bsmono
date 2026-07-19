//! dispatch.rs — fan a project's work across the joined fleet.
//!
//! `enlist.rs` gets up to [`crate::enlist::MAX_FLEET`] machines to `Ready`; this is the next step: hand
//! each member slices of the project, collect their results, and reassign a slice if a member fails or
//! drops. It reuses the `enlist` wire (`Msg::{Assign,Result,Failed}`) and `Transport`, so the same LAN
//! bus carries both join and work traffic.
//!
//! A "slice" is an opaque [`Task`] — an id + a `payload` string the caller fills however it likes (a
//! serialized `gpu_kernels::FleetRequest`, a devloop subtask prompt, an ollama request). The scheduler
//! never interprets the payload; the member side runs it through the [`Executor`] seam (real impl: run
//! the slice locally via `run_fleet_trie` or an ollama call; tests use a mock). Pure scheduler + mocked
//! seam, unit-tested like the rest of the crate. No GUI, no kernels.
#![allow(dead_code)]

use crate::enlist::{Msg, PeerId, Transport};
use std::collections::{HashMap, VecDeque};

/// One unit of work. `payload` is opaque to the scheduler.
#[derive(Clone, Debug)]
pub struct Task { pub id: u64, pub payload: String }

/// The member side: run an assigned slice, producing output or an error (which triggers a reassign).
pub trait Executor {
    fn run(&mut self, task: u64, payload: &str) -> Result<String, String>;
}

// ── coordinator-side scheduler ──────────────────────────────────────────────────────────────────--

/// Greedy capacity-aware scheduler over the joined members. Tracks free slots per member, a queue of
/// pending tasks, in-flight assignments, completed outputs, and per-task retry counts; on failure (or a
/// re-queued drop) a task goes back to the queue until `max_retries` is exhausted.
pub struct Dispatcher {
    coordinator: PeerId,
    free: HashMap<PeerId, u32>,        // member -> currently-free slots
    payload: HashMap<u64, String>,     // task id -> payload (kept so a task can be reassigned)
    pending: VecDeque<u64>,            // queued task ids
    inflight: HashMap<u64, (PeerId, u64)>, // task id -> (assignee, lease start time)
    done: HashMap<u64, String>,        // task id -> output
    retries: HashMap<u64, u32>,        // task id -> attempts already failed
    abandoned: Vec<u64>,               // tasks that exhausted retries
    max_retries: u32,
}

impl Dispatcher {
    pub fn new(coordinator: &str, members: &[(PeerId, u32)], tasks: Vec<Task>, max_retries: u32) -> Self {
        let mut payload = HashMap::new();
        let mut pending = VecDeque::new();
        for t in tasks { pending.push_back(t.id); payload.insert(t.id, t.payload); }
        Self {
            coordinator: coordinator.into(),
            free: members.iter().cloned().collect(),
            payload, pending, inflight: HashMap::new(), done: HashMap::new(),
            retries: HashMap::new(), abandoned: Vec::new(), max_retries,
        }
    }

    /// Assign as many queued tasks as there is free capacity, most-free member first (spreads load).
    /// Returns the `Assign` messages to send. Ties broken by peer id for determinism. `now` stamps each
    /// assignment's lease (see `sweep`).
    pub fn assign_at(&mut self, now: u64) -> Vec<Msg> {
        let mut out = Vec::new();
        while !self.pending.is_empty() {
            let pick = self.free.iter().filter(|(_, &s)| s > 0)
                .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                .map(|(p, _)| p.clone());
            let Some(member) = pick else { break };
            let task = self.pending.pop_front().unwrap();
            *self.free.get_mut(&member).unwrap() -= 1;
            self.inflight.insert(task, (member.clone(), now));
            let payload = self.payload.get(&task).cloned().unwrap_or_default();
            out.push(Msg::Assign { coordinator: self.coordinator.clone(), peer: member, task, payload });
        }
        out
    }

    /// Assign without lease tracking (timeouts disabled).
    pub fn assign(&mut self) -> Vec<Msg> { self.assign_at(0) }

    /// A member finished a task: record the output and free its slot. Backfill with `assign` afterward.
    pub fn on_result(&mut self, peer: &PeerId, task: u64, output: String) {
        match self.inflight.get(&task) { Some((p, _)) if p == peer => {}, _ => return } // stale / not the assignee
        self.inflight.remove(&task);
        self.done.insert(task, output);
        if let Some(s) = self.free.get_mut(peer) { *s += 1; }
    }

    /// A member failed a task: free its slot and requeue the task, unless it has exhausted `max_retries`.
    pub fn on_failed(&mut self, peer: &PeerId, task: u64, _reason: &str) {
        match self.inflight.get(&task) { Some((p, _)) if p == peer => {}, _ => return }
        self.inflight.remove(&task);
        if let Some(s) = self.free.get_mut(peer) { *s += 1; }
        let n = self.retries.entry(task).or_insert(0);
        *n += 1;
        if *n > self.max_retries { self.abandoned.push(task); } else { self.pending.push_back(task); }
    }

    /// A member dropped out of the fleet: free it and requeue whatever it had in flight.
    pub fn drop_member(&mut self, peer: &PeerId) {
        self.free.remove(peer);
        let lost: Vec<u64> = self.inflight.iter().filter(|(_, (m, _))| m == peer).map(|(t, _)| *t).collect();
        for t in lost { self.inflight.remove(&t); self.pending.push_back(t); }
    }

    /// Reclaim in-flight tasks unanswered for longer than `lease` (a dropped `Assign` or a silently-dead
    /// member): free the slot and requeue, counting it against `max_retries` like a failure. Returns how
    /// many were reclaimed; call `assign_at` afterward to re-send them.
    pub fn sweep(&mut self, now: u64, lease: u64) -> usize {
        let stale: Vec<(u64, PeerId)> = self.inflight.iter()
            .filter(|(_, (_, sent))| now.saturating_sub(*sent) > lease)
            .map(|(t, (m, _))| (*t, m.clone())).collect();
        for (task, member) in &stale {
            self.inflight.remove(task);
            if let Some(s) = self.free.get_mut(member) { *s += 1; }
            let n = self.retries.entry(*task).or_insert(0);
            *n += 1;
            if *n > self.max_retries { self.abandoned.push(*task); } else { self.pending.push_back(*task); }
        }
        stale.len()
    }

    pub fn is_complete(&self) -> bool { self.pending.is_empty() && self.inflight.is_empty() }
    pub fn results(&self) -> &HashMap<u64, String> { &self.done }
    pub fn abandoned(&self) -> &[u64] { &self.abandoned }

    // transport glue -------------------------------------------------------------------------------

    /// Assign what we can and send it (`now` stamps the leases).
    pub fn dispatch_at<T: Transport>(&mut self, t: &mut T, now: u64) {
        for a in self.assign_at(now) {
            if let Msg::Assign { peer: ref p, .. } = a { t.send(p, &a); }
        }
    }

    /// Assign and send without lease tracking.
    pub fn dispatch<T: Transport>(&mut self, t: &mut T) { self.dispatch_at(t, 0); }

    /// Drain inbound results/failures and apply them. Call `dispatch` afterward to backfill.
    pub fn pump<T: Transport>(&mut self, t: &mut T) {
        for (_from, msg) in t.recv() {
            match msg {
                Msg::Result { peer, task, output } => self.on_result(&peer, task, output),
                Msg::Failed { peer, task, reason } => self.on_failed(&peer, task, &reason),
                _ => {}
            }
        }
    }
}

// ── member-side worker ─────────────────────────────────────────────────────────────────────────--

/// A joined machine doing work: drains `Assign`s addressed to it, runs each through the `Executor`, and
/// replies `Result`/`Failed` to the coordinator named in the assignment.
pub struct Worker { pub id: PeerId }

impl Worker {
    pub fn new(id: &str) -> Self { Self { id: id.into() } }

    pub fn work<T: Transport, E: Executor>(&mut self, t: &mut T, ex: &mut E) {
        for (_from, msg) in t.recv() {
            if let Msg::Assign { coordinator, peer, task, payload } = msg {
                if peer != self.id { continue; }
                let reply = match ex.run(task, &payload) {
                    Ok(output) => Msg::Result { peer: self.id.clone(), task, output },
                    Err(reason) => Msg::Failed { peer: self.id.clone(), task, reason },
                };
                t.send(&coordinator, &reply);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    use std::cell::RefCell;

    fn tasks(n: u64) -> Vec<Task> { (0..n).map(|i| Task { id: i, payload: format!("p{i}") }).collect() }

    #[test]
    fn assign_spreads_across_members_by_free_slots() {
        // members a:1, b:3 free → first three assignments should favor b (more slots), then a.
        let mut d = Dispatcher::new("c", &[("a".into(), 1), ("b".into(), 3)], tasks(4), 2);
        let msgs = d.assign();
        assert_eq!(msgs.len(), 4, "all capacity used (1+3)");
        let to_b = msgs.iter().filter(|m| matches!(m, Msg::Assign { peer, .. } if peer == "b")).count();
        let to_a = msgs.iter().filter(|m| matches!(m, Msg::Assign { peer, .. } if peer == "a")).count();
        assert_eq!((to_a, to_b), (1, 3));
        // capacity now exhausted: nothing more goes out until a slot frees
        assert!(d.assign().is_empty());
    }

    #[test]
    fn result_frees_slot_and_backfills() {
        let mut d = Dispatcher::new("c", &[("a".into(), 1)], tasks(2), 2);
        let first = d.assign();
        assert_eq!(first.len(), 1);                 // one slot → one task
        assert!(d.assign().is_empty());             // blocked
        d.on_result(&"a".into(), 0, "out0".into()); // a frees up
        let second = d.assign();
        assert_eq!(second.len(), 1);                // backfilled with the remaining task
        d.on_result(&"a".into(), 1, "out1".into());
        assert!(d.is_complete());
        assert_eq!(d.results().len(), 2);
    }

    #[test]
    fn failure_requeues_until_retries_exhausted() {
        let mut d = Dispatcher::new("c", &[("a".into(), 1)], tasks(1), 1); // max_retries=1
        d.assign();
        d.on_failed(&"a".into(), 0, "boom");        // attempt 1 failed → requeued
        assert!(!d.is_complete());
        let again = d.assign();
        assert_eq!(again.len(), 1);
        d.on_failed(&"a".into(), 0, "boom again");  // attempt 2 failed → exhausted
        assert!(d.is_complete(), "no longer pending/inflight");
        assert_eq!(d.abandoned(), &[0]);
        assert!(d.results().is_empty());
    }

    #[test]
    fn sweep_reclaims_unanswered_assignments() {
        let mut d = Dispatcher::new("c", &[("a".into(), 1)], tasks(1), 2);
        d.assign_at(100);                          // task 0 → a, leased at t=100
        assert!(d.assign_at(105).is_empty());      // a is busy
        assert_eq!(d.sweep(120, 30), 0);           // 120-100 = 20 ≤ 30 → still within lease
        assert_eq!(d.sweep(150, 30), 1);           // 150-100 = 50 > 30 → reclaimed
        assert!(!d.is_complete());                 // task requeued
        let re = d.assign_at(160);
        assert_eq!(re.len(), 1, "slot freed, task reassigned");
        d.on_result(&"a".into(), 0, "ok".into());  // a healthy reply settles it
        assert!(d.is_complete());
        assert!(d.abandoned().is_empty());
    }

    #[test]
    fn dropped_member_requeues_its_inflight_work() {
        let mut d = Dispatcher::new("c", &[("a".into(), 1), ("b".into(), 1)], tasks(2), 2);
        d.assign();                                 // a and b each take one
        d.drop_member(&"a".into());                 // a vanishes mid-task
        d.on_result(&"b".into(), 1, "out1".into()); // b finishes its task (id 1, since b sorts after a → got task 1? order below)
        // a's task is requeued; only b remains to run it
        let re = d.assign();
        assert_eq!(re.len(), 1);
        if let Msg::Assign { peer, .. } = &re[0] { assert_eq!(peer, "b"); }
    }

    // minimal in-memory transport (mirrors enlist's loopback) for the end-to-end test
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

    // executor that fails the FIRST (global) attempt at task 2, then succeeds — shared across workers so
    // the retry succeeds no matter which member it lands on (one failure total, deterministic).
    #[derive(Clone)]
    struct FlakyOnce { seen: Rc<RefCell<std::collections::HashSet<u64>>> }
    impl Executor for FlakyOnce {
        fn run(&mut self, task: u64, payload: &str) -> Result<String, String> {
            if task == 2 && self.seen.borrow_mut().insert(2) { Err("transient".into()) }
            else { Ok(format!("ran:{payload}")) }
        }
    }

    #[test]
    fn loopback_dispatch_completes_all_tasks_over_transport() {
        let cid = "coord:1".to_string();
        let mids: Vec<PeerId> = (0..3).map(|i| format!("m{i}:1")).collect();
        let bus = Rc::new(RefCell::new(Bus {
            inbox: HashMap::new(),
            peers: std::iter::once(cid.clone()).chain(mids.clone()).collect(),
        }));
        let mut ct = Loop { me: cid.clone(), bus: bus.clone() };
        let members: Vec<(PeerId, u32)> = mids.iter().map(|m| (m.clone(), 2)).collect();
        let mut disp = Dispatcher::new(&cid, &members, tasks(6), 2);
        let flaky = FlakyOnce { seen: Rc::new(RefCell::new(std::collections::HashSet::new())) };
        let mut workers: Vec<(Worker, Loop, FlakyOnce)> = mids.iter()
            .map(|id| (Worker::new(id), Loop { me: id.clone(), bus: bus.clone() }, flaky.clone()))
            .collect();

        // pump until complete (bounded so a bug can't hang the test)
        disp.dispatch(&mut ct);
        for _ in 0..20 {
            for (w, wt, ex) in workers.iter_mut() { w.work(wt, ex); }
            disp.pump(&mut ct);
            disp.dispatch(&mut ct);
            if disp.is_complete() { break; }
        }
        assert!(disp.is_complete(), "all tasks settled");
        assert_eq!(disp.results().len(), 6, "every task produced output (the one failure was retried)");
        assert!(disp.abandoned().is_empty());
    }
}
