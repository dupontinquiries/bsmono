//! cpupool.rs — persistent workers, because spawning them per call costs more than the work.
//!
//! `std::thread::scope` creates fresh OS threads on every invocation. Measured on this machine:
//!
//! ```text
//!   scope with  2 threads:   63.3 us/call  ->  21.3 ms per token at 336 calls
//!   scope with  4 threads:  116.7 us/call  ->  39.2 ms per token
//!   scope with  8 threads:  259.6 us/call  ->  87.2 ms per token
//! ```
//!
//! A decode does roughly seven matvecs per layer across forty-eight layers, so 336 dispatches per token.
//!
//! ## What that measurement does NOT show, and I claimed it did
//!
//! This container has ONE core. Eight threads there contend for a single CPU, so the numbers above are
//! dominated by contention rather than by spawn cost — and the persistent pool measures the SAME,
//! slightly worse at 8 threads because its spin loop burns cycles a single core needs:
//!
//! ```text
//!   scope with 8 threads: 248.7 us/call
//!   pool   with 8 threads: 265.2 us/call
//! ```
//!
//! So the pool is here on llama.cpp's evidence and on the structural argument — creating and destroying
//! OS threads 336 times per token cannot be free, and every serious CPU inference implementation keeps
//! a persistent pool — but NOT on a measurement taken here, because this machine cannot take one.
//! `examples/spawn_cost.rs` run on the 24-core M2 Ultra is what would settle it, and if the pool does
//! not win there it should be deleted.
//!
//! ## Spin, then park
//!
//! Workers spin briefly before blocking. Between two matvecs of the same layer the next job arrives in
//! microseconds, and a futex sleep/wake round trip costs more than the gap — the same reasoning behind
//! Colibri's GPU keep-alive spin, applied to CPU threads. Spinning forever would burn a core per worker
//! while the user is typing, so the spin is bounded and then it parks properly.

use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Iterations a worker spins before parking. Long enough to cover the gap between consecutive matvecs,
/// short enough that an idle pool does not hold cores.
const SPIN: u32 = 2000;

struct Shared {
    /// Incremented once per job. Workers compare against their own last-seen value, so a worker that
    /// was slow to wake still sees the job rather than missing the edge — a plain flag would race.
    epoch: AtomicUsize,
    /// Workers still running the current job.
    active: AtomicUsize,
    /// The job, as a raw pointer valid only while `run` blocks on completion.
    job: Mutex<Option<JobPtr>>,
    /// Wakes workers when spinning did not catch the job in time.
    cv: Condvar,
    /// Wakes the caller when the last worker finishes.
    done: Condvar,
    done_lock: Mutex<()>,
    stop: AtomicUsize,
}

/// A pointer to the caller's closure plus the worker count.
///
/// SAFETY: `run` publishes this and does not return until `active` reaches zero, so the referent
/// outlives every use. The closure is `Sync`, so concurrent calls are sound. This is the same contract
/// rayon and llama.cpp rely on; it is written out because an unstated one is how it gets broken.
#[derive(Clone, Copy)]
struct JobPtr { f: *const (dyn Fn(usize, usize) + Sync), n: usize }
unsafe impl Send for JobPtr {}
unsafe impl Sync for JobPtr {}

pub struct Pool {
    shared: Arc<Shared>,
    handles: Vec<std::thread::JoinHandle<()>>,
    n: usize,
}

impl Pool {
    /// Create a pool with `n` workers. `n <= 1` runs inline and spawns nothing.
    pub fn new(n: usize) -> Self {
        let n = n.max(1);
        let shared = Arc::new(Shared {
            epoch: AtomicUsize::new(0), active: AtomicUsize::new(0),
            job: Mutex::new(None), cv: Condvar::new(),
            done: Condvar::new(), done_lock: Mutex::new(()), stop: AtomicUsize::new(0),
        });
        let mut handles = Vec::new();
        for id in 1..n {                       // worker 0 is the calling thread
            let sh = Arc::clone(&shared);
            handles.push(std::thread::spawn(move || worker(sh, id)));
        }
        Pool { shared, handles, n }
    }

    pub fn threads(&self) -> usize { self.n }

    /// Run `f(worker_id, n_workers)` on every worker, returning when all have finished.
    ///
    /// The CALLING thread is worker 0, so a pool of n uses n threads rather than n+1 — and a pool of 1
    /// never touches a lock at all.
    pub fn run<F: Fn(usize, usize) + Sync>(&self, f: F) {
        if self.n == 1 { f(0, 1); return; }
        // The trait object needs a lifetime the compiler can name; `run` guarantees the referent
        // outlives every use by blocking until active == 0, so erasing to 'static here is sound and is
        // the same contract rayon documents. Written as a transmute rather than hidden in a helper so
        // the unsoundness, if any, is visible at the point it would occur.
        let dynf: &(dyn Fn(usize, usize) + Sync) = &f;
        let dynf: &'static (dyn Fn(usize, usize) + Sync) = unsafe { std::mem::transmute(dynf) };
        self.shared.active.store(self.n - 1, Ordering::SeqCst);
        {
            let mut g = self.shared.job.lock().unwrap();
            *g = Some(JobPtr { f: dynf as *const _, n: self.n });
        }
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        self.shared.cv.notify_all();

        f(0, self.n);                          // the caller does its own share

        // Wait for the others. Spin first for the same reason the workers do.
        for _ in 0..SPIN {
            if self.shared.active.load(Ordering::SeqCst) == 0 { return; }
            std::hint::spin_loop();
        }
        let mut g = self.shared.done_lock.lock().unwrap();
        while self.shared.active.load(Ordering::SeqCst) != 0 {
            g = self.shared.done.wait(g).unwrap();
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.stop.store(1, Ordering::SeqCst);
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        self.shared.cv.notify_all();
        for h in self.handles.drain(..) { let _ = h.join(); }
    }
}

fn worker(sh: Arc<Shared>, id: usize) {
    let mut seen = 0usize;
    loop {
        // Spin for the common case: the next job is microseconds away and a park/unpark round trip
        // costs more than the gap.
        let mut cur = sh.epoch.load(Ordering::SeqCst);
        let mut spun = 0u32;
        while cur == seen && spun < SPIN {
            std::hint::spin_loop();
            cur = sh.epoch.load(Ordering::SeqCst);
            spun += 1;
        }
        if cur == seen {
            let mut g = sh.job.lock().unwrap();
            while sh.epoch.load(Ordering::SeqCst) == seen {
                g = sh.cv.wait(g).unwrap();
            }
            cur = sh.epoch.load(Ordering::SeqCst);
        }
        seen = cur;
        if sh.stop.load(Ordering::SeqCst) == 1 { return; }

        let job = *sh.job.lock().unwrap();
        if let Some(j) = job {
            if id < j.n {
                // SAFETY: `run` blocks until `active` hits zero, so the closure outlives this call.
                unsafe { (*j.f)(id, j.n); }
            }
        }
        if sh.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            let _g = sh.done_lock.lock().unwrap();
            sh.done.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn every_worker_runs_exactly_once_per_job() {
        for n in [1usize, 2, 4, 8] {
            let p = Pool::new(n);
            let hits = Arc::new(AtomicU64::new(0));
            let seen: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
            for _ in 0..20 {
                let (h, s) = (Arc::clone(&hits), Arc::clone(&seen));
                p.run(move |id, total| {
                    assert_eq!(total, n);
                    h.fetch_add(1, Ordering::SeqCst);
                    s.lock().unwrap().push(id);
                });
            }
            assert_eq!(hits.load(Ordering::SeqCst), (n * 20) as u64, "n={n}");
            let mut ids = seen.lock().unwrap().clone();
            ids.sort_unstable(); ids.dedup();
            assert_eq!(ids, (0..n).collect::<Vec<_>>(), "every id appears, n={n}");
        }
    }

    #[test]
    fn a_pool_of_one_runs_inline_and_spawns_nothing() {
        // The single-thread path must not touch a lock or a condvar: it is what a small matvec uses,
        // and paying synchronisation for one worker is how a "parallel" version loses to a serial one.
        let p = Pool::new(1);
        assert_eq!(p.threads(), 1);
        assert!(p.handles.is_empty(), "no OS threads for a pool of one");
        let hit = Arc::new(AtomicU64::new(0));
        let h = Arc::clone(&hit);
        p.run(move |id, n| { assert_eq!((id, n), (0, 1)); h.fetch_add(1, Ordering::SeqCst); });
        assert_eq!(hit.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn work_is_visible_to_the_caller_after_run_returns() {
        // The barrier's whole purpose. If run() returned early, results would be read while workers
        // were still writing — and it would pass under light load and fail under real work.
        let p = Pool::new(4);
        for _ in 0..50 {
            let out = Arc::new(Mutex::new(vec![0u64; 4]));
            let o = Arc::clone(&out);
            p.run(move |id, _| { o.lock().unwrap()[id] = (id as u64 + 1) * 7; });
            assert_eq!(*out.lock().unwrap(), vec![7, 14, 21, 28]);
        }
    }

    #[test]
    fn the_pool_survives_being_idle_between_jobs() {
        // Workers spin then park. A job arriving after the spin window must still be seen — the epoch
        // counter exists because a plain flag would let a slow worker miss the edge entirely.
        let p = Pool::new(4);
        let hits = Arc::new(AtomicU64::new(0));
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(20));   // well past the spin
            let h = Arc::clone(&hits);
            p.run(move |_, _| { h.fetch_add(1, Ordering::SeqCst); });
        }
        assert_eq!(hits.load(Ordering::SeqCst), 12);
    }
}
