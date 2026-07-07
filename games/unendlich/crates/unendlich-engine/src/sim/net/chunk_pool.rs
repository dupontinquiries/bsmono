//! `sim::net::chunk_pool` — distributed worldgen dispatch across the host and
//! trusted admins ("admin-pooled chunk generation").
//!
//! Generating chunks is expensive, and in a session the host shouldn't have to
//! do it all alone. Privileged peers (admins — see `player_roster::is_privileged`)
//! can be enrolled as **workers** and handed chunk-generation jobs; the host
//! coordinates a shared queue, assigns pending chunks to workers with free
//! capacity, collects the results, and never double-assigns a chunk.
//!
//! Because admins are ordinary network peers, the pool is built for them
//! **dropping mid-job**: [`ChunkPool::remove_worker`] returns a disconnected
//! worker's in-flight jobs to the queue, and [`ChunkPool::reap_timeouts`]
//! reclaims jobs whose worker went silent past a deadline. Both hand the keys
//! back so the host can reassign them on the next [`ChunkPool::assign`].
//!
//! The pool only *dispatches* — it doesn't generate or trust the returned data.
//! Worldgen is deterministic from (seed, coord), so the host is expected to
//! verify an admin's result (e.g. hash-check) before committing; that check is
//! the caller's, kept out of the pure dispatch logic on purpose. Likewise the
//! *trust* decision (who may be a worker) is the caller's: it enrolls only peers
//! `player_roster` reports as privileged.
//!
//! Pure/std-only: `BTreeMap` (deterministic iteration) + integer keys. Chunk
//! coordinates are packed into an `i64` key by the caller; worker ids are the
//! `u64` player ids. Times are seconds (`f64`) on the host clock.

use std::collections::BTreeMap;

/// A packed chunk coordinate (the caller packs `(cx, cz)` or `(cx, cy, cz)`).
pub type ChunkKey = i64;
/// A worker is identified by its player id (the host enrolls itself too).
pub type WorkerId = u64;

/// How long an assigned job may run before the host reclaims it (worker presumed
/// stalled/dropped). Tuned above a normal chunk-gen round trip.
pub const JOB_TIMEOUT: f64 = 5.0;

/// A worker in the pool: the host or an enrolled admin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Worker {
    pub id: WorkerId,
    /// How many jobs it may hold at once.
    pub capacity: u32,
    /// How many it currently holds.
    pub in_flight: u32,
}

impl Worker {
    fn free(&self) -> u32 {
        self.capacity.saturating_sub(self.in_flight)
    }
}

/// State of a single chunk job.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum JobState {
    /// Waiting to be assigned.
    Pending,
    /// Handed to `worker`; will be reclaimed after `deadline` if not completed.
    Assigned { worker: WorkerId, deadline: f64 },
    /// Generation finished and accepted.
    Done,
}

#[derive(Clone, Copy, Debug)]
struct Job {
    /// Lower = more urgent (typically chunk distance to the nearest player).
    priority: i32,
    /// Tie-break / stable request order.
    seq: u64,
    state: JobState,
}

/// The host's distributed chunk-generation pool.
#[derive(Clone, Debug, Default)]
pub struct ChunkPool {
    workers: BTreeMap<WorkerId, Worker>,
    jobs: BTreeMap<ChunkKey, Job>,
    next_seq: u64,
}

impl ChunkPool {
    pub fn new() -> Self {
        ChunkPool { workers: BTreeMap::new(), jobs: BTreeMap::new(), next_seq: 0 }
    }

    // ---- workers ----

    /// Enrol or update a worker (the host enrols itself; admins are enrolled when
    /// `player_roster` reports them privileged). Updating `capacity` keeps the
    /// current `in_flight` load.
    pub fn set_worker(&mut self, id: WorkerId, capacity: u32) {
        let load = self.workers.get(&id).map(|w| w.in_flight).unwrap_or(0);
        self.workers.insert(id, Worker { id, capacity, in_flight: load.min(capacity) });
    }

    /// Remove a worker (disconnect). Any jobs it held return to `Pending`; the
    /// freed keys are returned so the caller can log/re-dispatch. Deterministic
    /// (sorted by key).
    pub fn remove_worker(&mut self, id: WorkerId) -> Vec<ChunkKey> {
        let mut freed = Vec::new();
        for (key, job) in self.jobs.iter_mut() {
            if let JobState::Assigned { worker, .. } = job.state {
                if worker == id {
                    job.state = JobState::Pending;
                    freed.push(*key);
                }
            }
        }
        self.workers.remove(&id);
        freed
    }

    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    pub fn worker(&self, id: WorkerId) -> Option<Worker> {
        self.workers.get(&id).copied()
    }

    // ---- jobs ----

    /// Request generation of a chunk. Ignored if the chunk is already known
    /// (pending, assigned, or done) — dedup by key. `priority` lower = sooner.
    /// Returns `true` if a new job was enqueued.
    pub fn request(&mut self, key: ChunkKey, priority: i32) -> bool {
        if self.jobs.contains_key(&key) {
            return false;
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.jobs.insert(key, Job { priority, seq, state: JobState::Pending });
        true
    }

    /// Update the priority of a still-`Pending` job (e.g. a player moved closer).
    /// No effect on assigned/done jobs.
    pub fn reprioritize(&mut self, key: ChunkKey, priority: i32) {
        if let Some(j) = self.jobs.get_mut(&key) {
            if matches!(j.state, JobState::Pending) {
                j.priority = priority;
            }
        }
    }

    /// Forget a chunk entirely (allows a later re-request, e.g. after an edit).
    /// If it was assigned, the worker's load is freed.
    pub fn forget(&mut self, key: ChunkKey) {
        if let Some(job) = self.jobs.remove(&key) {
            if let JobState::Assigned { worker, .. } = job.state {
                if let Some(w) = self.workers.get_mut(&worker) {
                    w.in_flight = w.in_flight.saturating_sub(1);
                }
            }
        }
    }

    /// Assign as many pending jobs as there is worker capacity, most-urgent
    /// first. Greedy + deterministic: pending jobs are ordered by `(priority,
    /// seq)`; each is given to the currently-least-loaded worker with free
    /// capacity (tie-broken by lowest id). Marks jobs `Assigned` with a
    /// `now + JOB_TIMEOUT` deadline and bumps worker load. Returns the
    /// `(worker, key)` orders for the caller to send over the wire.
    pub fn assign(&mut self, now: f64) -> Vec<(WorkerId, ChunkKey)> {
        // total free capacity up front — nothing to do if zero
        let total_free: u32 = self.workers.values().map(|w| w.free()).sum();
        if total_free == 0 {
            return Vec::new();
        }
        // pending jobs, most urgent first
        let mut pending: Vec<(ChunkKey, i32, u64)> = self
            .jobs
            .iter()
            .filter(|(_, j)| matches!(j.state, JobState::Pending))
            .map(|(k, j)| (*k, j.priority, j.seq))
            .collect();
        pending.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)));

        // working copy of free capacity per worker
        let mut free: BTreeMap<WorkerId, u32> =
            self.workers.iter().map(|(id, w)| (*id, w.free())).collect();

        let mut orders = Vec::new();
        for (key, _, _) in pending {
            // pick the worker with the most free slots (tie → lowest id)
            let pick = free
                .iter()
                .filter(|(_, f)| **f > 0)
                .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
                .map(|(id, _)| *id);
            let Some(wid) = pick else { break }; // no capacity left anywhere
            *free.get_mut(&wid).unwrap() -= 1;
            if let Some(w) = self.workers.get_mut(&wid) {
                w.in_flight += 1;
            }
            if let Some(j) = self.jobs.get_mut(&key) {
                j.state = JobState::Assigned { worker: wid, deadline: now + JOB_TIMEOUT };
            }
            orders.push((wid, key));
        }
        orders
    }

    /// Mark a job complete (the worker returned an accepted chunk). Frees the
    /// worker's capacity and records the chunk `Done`. Returns `true` if it was
    /// an outstanding assignment. A result from a worker other than the assigned
    /// one is still accepted (a late reassignment race) but only frees the
    /// worker that actually holds the slot.
    pub fn complete(&mut self, worker: WorkerId, key: ChunkKey) -> bool {
        let Some(job) = self.jobs.get_mut(&key) else { return false };
        let held_by = match job.state {
            JobState::Assigned { worker: w, .. } => Some(w),
            _ => None,
        };
        job.state = JobState::Done;
        // free whoever actually held the slot (usually == worker)
        let free_target = held_by.unwrap_or(worker);
        if let Some(w) = self.workers.get_mut(&free_target) {
            w.in_flight = w.in_flight.saturating_sub(1);
        }
        held_by.is_some()
    }

    /// Reclaim jobs whose deadline has passed (worker stalled/dropped): back to
    /// `Pending`, worker load freed. Returns the reclaimed keys (sorted) for
    /// re-dispatch on the next [`assign`](Self::assign).
    pub fn reap_timeouts(&mut self, now: f64) -> Vec<ChunkKey> {
        let mut reclaimed = Vec::new();
        for (key, job) in self.jobs.iter_mut() {
            if let JobState::Assigned { worker, deadline } = job.state {
                if now >= deadline {
                    job.state = JobState::Pending;
                    if let Some(w) = self.workers.get_mut(&worker) {
                        w.in_flight = w.in_flight.saturating_sub(1);
                    }
                    reclaimed.push(*key);
                }
            }
        }
        reclaimed
    }

    // ---- counts ----

    pub fn pending_count(&self) -> usize {
        self.jobs.values().filter(|j| matches!(j.state, JobState::Pending)).count()
    }
    pub fn in_flight_count(&self) -> usize {
        self.jobs.values().filter(|j| matches!(j.state, JobState::Assigned { .. })).count()
    }
    pub fn done_count(&self) -> usize {
        self.jobs.values().filter(|j| matches!(j.state, JobState::Done)).count()
    }
    pub fn state_of(&self, key: ChunkKey) -> Option<JobState> {
        self.jobs.get(&key).map(|j| j.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool_with_host(cap: u32) -> ChunkPool {
        let mut p = ChunkPool::new();
        p.set_worker(1, cap); // host is worker id 1
        p
    }

    #[test]
    fn request_dedups_by_key() {
        let mut p = pool_with_host(4);
        assert!(p.request(100, 0));
        assert!(!p.request(100, 0), "same chunk not enqueued twice");
        assert_eq!(p.pending_count(), 1);
    }

    #[test]
    fn assign_fills_capacity_most_urgent_first() {
        let mut p = pool_with_host(2);
        p.request(10, 5); // less urgent
        p.request(11, 1); // most urgent
        p.request(12, 3);
        let orders = p.assign(0.0);
        // only 2 slots → the two most-urgent (priority 1 then 3): keys 11, 12
        let keys: Vec<ChunkKey> = orders.iter().map(|(_, k)| *k).collect();
        assert_eq!(keys, vec![11, 12]);
        assert_eq!(p.in_flight_count(), 2);
        assert_eq!(p.pending_count(), 1);
    }

    #[test]
    fn assign_respects_full_worker() {
        let mut p = pool_with_host(1);
        p.request(1, 0);
        p.request(2, 0);
        assert_eq!(p.assign(0.0).len(), 1, "capacity 1 → one order");
        assert_eq!(p.assign(0.0).len(), 0, "already full → nothing more");
    }

    #[test]
    fn spreads_across_host_and_admins_by_capacity() {
        let mut p = ChunkPool::new();
        p.set_worker(1, 2); // host
        p.set_worker(7, 2); // admin
        p.set_worker(9, 1); // admin
        for k in 0..5 {
            p.request(k, 0);
        }
        let orders = p.assign(0.0);
        assert_eq!(orders.len(), 5, "5 jobs, 5 total slots");
        // each worker got up to its capacity
        assert_eq!(p.worker(1).unwrap().in_flight, 2);
        assert_eq!(p.worker(7).unwrap().in_flight, 2);
        assert_eq!(p.worker(9).unwrap().in_flight, 1);
    }

    #[test]
    fn complete_frees_capacity_and_marks_done() {
        let mut p = pool_with_host(1);
        p.request(42, 0);
        let orders = p.assign(0.0);
        let (w, k) = orders[0];
        assert!(p.complete(w, k));
        assert_eq!(p.state_of(42), Some(JobState::Done));
        assert_eq!(p.worker(1).unwrap().in_flight, 0, "slot freed");
        // a freed worker can take the next job
        p.request(43, 0);
        assert_eq!(p.assign(0.0).len(), 1);
    }

    #[test]
    fn remove_worker_returns_inflight_jobs() {
        let mut p = ChunkPool::new();
        p.set_worker(1, 1);
        p.set_worker(7, 1); // admin
        p.request(10, 0);
        p.request(11, 0);
        p.assign(0.0); // one to each
        assert_eq!(p.in_flight_count(), 2);
        let freed = p.remove_worker(7); // admin drops
        assert_eq!(freed.len(), 1, "its job returned to the queue");
        assert_eq!(p.pending_count(), 1);
        assert_eq!(p.worker_count(), 1);
    }

    #[test]
    fn reap_timeouts_reclaims_stale_jobs() {
        let mut p = pool_with_host(1);
        p.request(5, 0);
        p.assign(0.0); // deadline = JOB_TIMEOUT
        assert!(p.reap_timeouts(JOB_TIMEOUT - 0.1).is_empty(), "not yet stale");
        let reclaimed = p.reap_timeouts(JOB_TIMEOUT + 0.1);
        assert_eq!(reclaimed, vec![5]);
        assert_eq!(p.pending_count(), 1);
        assert_eq!(p.worker(1).unwrap().in_flight, 0, "stalled worker's slot freed");
    }

    #[test]
    fn forget_allows_regeneration() {
        let mut p = pool_with_host(1);
        p.request(8, 0);
        let (w, k) = p.assign(0.0)[0];
        p.complete(w, k);
        assert!(!p.request(8, 0), "done chunk not re-queued");
        p.forget(8);
        assert!(p.request(8, 0), "after forget it can be regenerated");
    }

    #[test]
    fn assignment_is_deterministic() {
        let build = || {
            let mut p = ChunkPool::new();
            p.set_worker(1, 2);
            p.set_worker(3, 2);
            for k in 0..4 {
                p.request(k, 0);
            }
            p.assign(0.0)
        };
        assert_eq!(build(), build(), "same inputs → identical dispatch");
    }

    #[test]
    fn end_to_end_drop_and_reassign() {
        // host + 2 admins, 6 chunks; an admin drops after assignment, its work
        // is reclaimed and finished by the remaining workers.
        let mut p = ChunkPool::new();
        p.set_worker(1, 2); // host
        p.set_worker(7, 2); // admin A
        p.set_worker(8, 2); // admin B
        for k in 0..6 {
            p.request(k, 0);
        }
        assert_eq!(p.assign(0.0).len(), 6);
        assert_eq!(p.in_flight_count(), 6);
        // admin A vanishes with its 2 jobs
        let freed = p.remove_worker(7);
        assert_eq!(freed.len(), 2);
        assert_eq!(p.pending_count(), 2);
        // remaining workers (host + B) finish their originals, freeing slots
        // complete everything still assigned, then re-dispatch the freed pair
        let still: Vec<(WorkerId, ChunkKey)> = p
            .jobs
            .iter()
            .filter_map(|(k, j)| match j.state {
                JobState::Assigned { worker, .. } => Some((worker, *k)),
                _ => None,
            })
            .collect();
        for (w, k) in still {
            p.complete(w, k);
        }
        let redispatch = p.assign(1.0);
        assert_eq!(redispatch.len(), 2, "the two reclaimed chunks go back out");
        for (w, k) in redispatch {
            p.complete(w, k);
        }
        assert_eq!(p.done_count(), 6, "all six chunks generated despite the drop");
        assert_eq!(p.pending_count(), 0);
    }
}
