//! batching.rs — a continuous (in-flight) batching scheduler: the pure slot-admission logic that keeps the
//! GPU full so total tokens/sec scales with concurrent requests.
//!
//! A batch-1 decode barely uses the GPU; decoding many sequences in one batched forward is far more
//! efficient. Static batching wastes that by waiting for the whole batch to finish before starting the
//! next. *Continuous* batching instead frees each sequence's KV-cache slot the moment it completes and
//! admits a waiting request into it immediately, so the active batch stays as full as the work allows.
//!
//! This module owns the admission/eviction bookkeeping over a fixed pool of `capacity` slots (FCFS). The
//! batched forward pass itself is the GPU seam — the engine decodes [`active`](Scheduler::active) each
//! step, then calls [`complete`](Scheduler::complete) for any sequence that hit EOS / its token cap.
#![allow(dead_code)]

use std::collections::VecDeque;

/// Fixed-capacity continuous-batching scheduler over request ids.
pub struct Scheduler {
    capacity: usize,
    active: Vec<u64>,
    waiting: VecDeque<u64>,
    admitted_total: u64,
    completed_total: u64,
}

impl Scheduler {
    pub fn new(capacity: usize) -> Self {
        Self { capacity: capacity.max(1), active: Vec::new(), waiting: VecDeque::new(),
               admitted_total: 0, completed_total: 0 }
    }

    /// Enqueue a request (FCFS). It is admitted on the next [`admit`](Self::admit) call if a slot is free.
    pub fn submit(&mut self, id: u64) { self.waiting.push_back(id); }

    /// Fill every free slot from the waiting queue. Returns the ids newly admitted this call (the engine
    /// must prefill their prompts before the next decode step). Call once per step, before decoding.
    pub fn admit(&mut self) -> Vec<u64> {
        let mut just_admitted = Vec::new();
        while self.active.len() < self.capacity {
            match self.waiting.pop_front() {
                Some(id) => { self.active.push(id); self.admitted_total += 1; just_admitted.push(id); }
                None => break,
            }
        }
        just_admitted
    }

    /// Free a sequence's slot (it hit EOS or its token cap). A waiting request can take the slot at the
    /// next `admit` — the continuous-batching win. Returns false if the id wasn't active.
    pub fn complete(&mut self, id: u64) -> bool {
        if let Some(pos) = self.active.iter().position(|&a| a == id) {
            self.active.swap_remove(pos);
            self.completed_total += 1;
            true
        } else { false }
    }

    /// The sequences to decode this step (the current batch).
    pub fn active(&self) -> &[u64] { &self.active }
    pub fn free_slots(&self) -> usize { self.capacity - self.active.len() }
    pub fn waiting(&self) -> usize { self.waiting.len() }
    pub fn capacity(&self) -> usize { self.capacity }
    /// No work left anywhere.
    pub fn is_idle(&self) -> bool { self.active.is_empty() && self.waiting.is_empty() }
    /// Slot utilization this step, 0..=1 — what continuous batching is trying to keep near 1.
    pub fn utilization(&self) -> f32 { self.active.len() as f32 / self.capacity as f32 }
    pub fn admitted_total(&self) -> u64 { self.admitted_total }
    pub fn completed_total(&self) -> u64 { self.completed_total }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admits_up_to_capacity_and_queues_the_rest() {
        let mut s = Scheduler::new(2);
        for id in [1, 2, 3, 4] { s.submit(id); }
        let admitted = s.admit();
        assert_eq!(admitted, vec![1, 2]);
        assert_eq!(s.active(), &[1, 2]);
        assert_eq!(s.waiting(), 2);
        assert_eq!(s.free_slots(), 0);
        assert!((s.utilization() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn completing_a_sequence_admits_a_waiting_one_mid_flight() {
        let mut s = Scheduler::new(2);
        for id in [1, 2, 3] { s.submit(id); }
        s.admit(); // [1, 2], 3 waits
        assert!(s.complete(1)); // slot frees immediately
        assert_eq!(s.free_slots(), 1);
        let admitted = s.admit(); // 3 takes the freed slot — the continuous-batching win
        assert_eq!(admitted, vec![3]);
        assert!(s.active().contains(&3) && s.active().contains(&2));
        assert_eq!(s.waiting(), 0);
    }

    #[test]
    fn completing_unknown_id_is_a_noop() {
        let mut s = Scheduler::new(2);
        s.submit(1); s.admit();
        assert!(!s.complete(99));
        assert_eq!(s.active(), &[1]);
    }

    #[test]
    fn drains_to_idle_and_tracks_totals() {
        let mut s = Scheduler::new(2);
        for id in [1, 2, 3] { s.submit(id); }
        s.admit();
        assert!(s.complete(1)); assert!(s.complete(2));
        s.admit(); // 3
        assert!(s.complete(3));
        assert!(s.is_idle());
        assert_eq!(s.admitted_total(), 3);
        assert_eq!(s.completed_total(), 3);
        assert!((s.utilization() - 0.0).abs() < 1e-6);
    }
}
