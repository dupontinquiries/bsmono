//! llm_coalesce: in-flight request coalescing. When several of the four
//! concurrent LLM callers fire the SAME prompt at once (e.g. two
//! surfaces both classifying the same text), only the FIRST computes;
//! the rest ATTACH and share its result. Without this, identical
//! concurrent prompts each spawn a duplicate backend call, multiplying
//! the contention this whole arc is fighting (the "models lock up
//! against each other" report).
//!
//! Complements llm_cache: the cache serves prompts whose result is
//! already DONE; the coalescer serves prompts being computed RIGHT NOW.
//! Lookup order at the call site is: cache (done) → coalescer
//! (in-flight) → compute.
//!
//! DESIGN: the REGISTRY (who computes vs attaches, fan-out on
//! completion) is pure HashMap logic and fully unit-tested. The actual
//! cross-thread WAIT (a waiter blocking until the computer finishes) is
//! a thin condvar layer kept separate so the decision logic stays
//! verifiable without spawning threads.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex, OnceLock};

/// Whether a submitting caller should run the computation or wait on an
/// already-in-flight identical one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// You are the first — run the backend call, then call `complete`.
    Compute,
    /// An identical prompt is already in flight — wait for its result.
    Attach,
}

/// Pure in-flight registry: prompt-hash → number of parties waiting on
/// it (the computer plus any attached duplicates). No IO, no blocking —
/// this is the testable core.
#[derive(Default)]
pub struct Registry {
    in_flight: HashMap<u64, u32>,
}

impl Registry {
    pub fn new() -> Self {
        Self { in_flight: HashMap::new() }
    }

    /// Register a submission for `hash`. The first submission for a hash
    /// returns `Compute`; identical concurrent submissions return
    /// `Attach` and increment the waiter count.
    pub fn submit(&mut self, hash: u64) -> Admission {
        match self.in_flight.get_mut(&hash) {
            Some(n) => {
                *n += 1;
                Admission::Attach
            }
            None => {
                self.in_flight.insert(hash, 1);
                Admission::Compute
            }
        }
    }

    /// Mark `hash` complete and return how many parties were waiting on
    /// it (so the caller knows how many to fan the result out to).
    /// Completing a hash that was never submitted returns 0 (no panic).
    pub fn complete(&mut self, hash: u64) -> u32 {
        self.in_flight.remove(&hash).unwrap_or(0)
    }

    /// True if `hash` is currently being computed.
    pub fn is_in_flight(&self, hash: u64) -> bool {
        self.in_flight.contains_key(&hash)
    }

    /// Number of distinct prompts in flight.
    pub fn len(&self) -> usize {
        self.in_flight.len()
    }

    pub fn is_empty(&self) -> bool {
        self.in_flight.is_empty()
    }
}

/// Stable prompt hash (shared idiom with llm_cache).
pub fn hash_prompt(prompt: &str) -> u64 {
    crate::util::fnv1a(prompt)
}

// ─── Live layer: a shared registry + a place to publish results ───
//
// The registry above is pure. The process-wide instance plus the
// result-publishing map (so an attached waiter can pick up the
// computer's result) live here. The actual blocking wait is a condvar;
// this layer is intentionally thin and is exercised on-device.

struct Shared {
    registry: Registry,
    /// Results published by completers, keyed by hash, for waiters to
    /// collect. Cleared once all waiters have taken it (best-effort:
    /// we drop it on the next completion of the same hash).
    results: HashMap<u64, String>,
}

impl Shared {
    fn new() -> Self {
        Self { registry: Registry::new(), results: HashMap::new() }
    }
}

static SHARED: OnceLock<(Mutex<Shared>, Condvar)> = OnceLock::new();

fn shared() -> &'static (Mutex<Shared>, Condvar) {
    SHARED.get_or_init(|| (Mutex::new(Shared::new()), Condvar::new()))
}

/// Submit a prompt. `Compute` → caller runs the backend then calls
/// `publish`. `Attach` → caller calls `wait_for` to get the shared
/// result.
pub fn submit(prompt: &str) -> (Admission, u64) {
    let h = hash_prompt(prompt);
    let (lock, _cv) = shared();
    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
    (g.registry.submit(h), h)
}

/// The computer publishes its result; all attached waiters are woken
/// and can collect it. Returns the waiter count.
pub fn publish(hash: u64, result: &str) -> u32 {
    let (lock, cv) = shared();
    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let waiters = g.registry.complete(hash);
    if waiters > 1 {
        // there were attached duplicates — publish for them.
        g.results.insert(hash, result.to_string());
    }
    cv.notify_all();
    waiters
}

/// v0.1.556: global query — is this hash currently in flight? Useful
/// for a caller deciding whether to coalesce, and for tests.
pub fn is_in_flight_global(hash: u64) -> bool {
    let (lock, _cv) = shared();
    let g = lock.lock().unwrap_or_else(|e| e.into_inner());
    g.registry.is_in_flight(hash)
}

/// An attached waiter blocks until the computer publishes, then returns
/// the shared result (or None if the computer failed/timed out and the
/// hash was cleared without a result).
pub fn wait_for(hash: u64) -> Option<String> {
    let (lock, cv) = shared();
    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
    // Wait until either the result is published or the hash is no longer
    // in flight (computer finished without publishing for us).
    loop {
        if let Some(r) = g.results.get(&hash).cloned() {
            return Some(r);
        }
        if !g.registry.is_in_flight(hash) {
            // computer finished; no result staged for us.
            return None;
        }
        g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
    }
}

/// v0.1.556: the computer calls this if it FAILS before publishing
/// (backend error, timeout, panic-guard). Without it, the hash stays
/// in_flight forever and every attached `wait_for` blocks indefinitely
/// — a real deadlock once wired to live LLM calls, which do fail. This
/// clears the hash and wakes all waiters, who then observe
/// !is_in_flight and return None (so they can fall back to computing
/// themselves). Idempotent: cancelling an unknown hash is a no-op.
pub fn cancel(hash: u64) {
    let (lock, cv) = shared();
    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
    g.registry.complete(hash);   // remove from in_flight (returns count, ignored)
    g.results.remove(&hash);     // ensure no stale result lingers
    cv.notify_all();
}

/// v0.1.556: bounded variant of `wait_for`. Returns None if no result
/// arrives within `timeout` (the waiter should then compute itself or
/// surface an error) — a belt-and-suspenders guard so a lost `publish`/
/// `cancel` (e.g. a hard panic in the computer) can't hang a waiter
/// forever. Pure `wait_for` is kept for callers that have their own
/// upstream timeout.
pub fn wait_for_timeout(hash: u64, timeout: std::time::Duration) -> Option<String> {
    let (lock, cv) = shared();
    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(r) = g.results.get(&hash).cloned() {
            return Some(r);
        }
        if !g.registry.is_in_flight(hash) {
            return None;
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return None; // timed out — caller falls back
        }
        let (ng, res) = cv.wait_timeout(g, deadline - now)
            .unwrap_or_else(|e| e.into_inner());
        g = ng;
        if res.timed_out() {
            // re-check the predicates once more, then bail next loop.
            if let Some(r) = g.results.get(&hash).cloned() { return Some(r); }
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── live layer: cancel + timeout (v0.1.556 deadlock guard) ──
    // These exercise the real condvar layer with threads. They use
    // distinct, high hashes to avoid colliding with other tests sharing
    // the process-global SHARED registry.

    #[test]
    fn cancel_wakes_waiter_with_none() {
        // A waiter attaches; the computer cancels (simulating a backend
        // failure). The waiter must wake and get None — NOT hang.
        let h = 0xDEAD_0001u64;
        let (lock, _cv) = shared();
        {
            let mut g = lock.lock().unwrap();
            assert_eq!(g.registry.submit(h), Admission::Compute); // computer
            assert_eq!(g.registry.submit(h), Admission::Attach);  // waiter
        }
        let waiter = std::thread::spawn(move || wait_for(h));
        // Give the waiter a moment to block on the condvar.
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancel(h); // computer failed before publishing
        let got = waiter.join().unwrap();
        assert_eq!(got, None, "cancelled waiter must return None, not hang");
        // registry is clean afterward
        assert!(!is_in_flight_global(h));
    }

    #[test]
    fn publish_delivers_result_to_waiter() {
        let h = 0xDEAD_0002u64;
        let (lock, _cv) = shared();
        {
            let mut g = lock.lock().unwrap();
            g.registry.submit(h); // computer
            g.registry.submit(h); // waiter
        }
        let waiter = std::thread::spawn(move || wait_for(h));
        std::thread::sleep(std::time::Duration::from_millis(50));
        let n = publish(h, "shared-answer");
        assert_eq!(n, 2, "two parties were waiting");
        assert_eq!(waiter.join().unwrap(), Some("shared-answer".to_string()));
    }

    #[test]
    fn wait_for_timeout_bails_when_nothing_published() {
        // Computer never publishes AND never cancels (hard panic
        // scenario). The bounded waiter must time out, not hang.
        let h = 0xDEAD_0003u64;
        let (lock, _cv) = shared();
        {
            let mut g = lock.lock().unwrap();
            g.registry.submit(h); // computer (will "vanish")
            g.registry.submit(h); // waiter
        }
        let start = std::time::Instant::now();
        let got = wait_for_timeout(h, std::time::Duration::from_millis(100));
        assert_eq!(got, None);
        assert!(start.elapsed() >= std::time::Duration::from_millis(90),
            "should have actually waited ~the timeout");
        assert!(start.elapsed() < std::time::Duration::from_secs(2),
            "should not hang far past the timeout");
        // clean up so we don't leak the stuck hash to other tests
        cancel(h);
    }

    // ── pure registry (the verifiable core) ──

    #[test]
    fn first_computes_duplicates_attach() {
        let mut r = Registry::new();
        assert_eq!(r.submit(0xA), Admission::Compute);
        assert_eq!(r.submit(0xA), Admission::Attach);
        assert_eq!(r.submit(0xA), Admission::Attach);
        // distinct prompt computes independently
        assert_eq!(r.submit(0xB), Admission::Compute);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn completion_fans_out_to_all_waiters() {
        let mut r = Registry::new();
        r.submit(0xA);            // computer
        r.submit(0xA);            // +1
        r.submit(0xA);            // +1
        // 3 parties share one computation
        assert_eq!(r.complete(0xA), 3);
        assert!(!r.is_in_flight(0xA));
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn recomputes_after_completion_not_stale() {
        let mut r = Registry::new();
        r.submit(0xA);
        r.complete(0xA);
        // a fresh submission after completion computes again
        assert_eq!(r.submit(0xA), Admission::Compute);
    }

    #[test]
    fn completing_unknown_hash_is_zero_no_panic() {
        let mut r = Registry::new();
        assert_eq!(r.complete(0xDEAD), 0);
        assert!(r.is_empty());
    }

    #[test]
    fn single_request_no_duplicates() {
        let mut r = Registry::new();
        assert_eq!(r.submit(0x1), Admission::Compute);
        // one party only
        assert_eq!(r.complete(0x1), 1);
    }

    #[test]
    fn hash_is_stable_and_distinguishes() {
        assert_eq!(hash_prompt("classify: hello"), hash_prompt("classify: hello"));
        assert_ne!(hash_prompt("a"), hash_prompt("b"));
    }

    #[test]
    fn interleaved_distinct_prompts() {
        let mut r = Registry::new();
        assert_eq!(r.submit(0xA), Admission::Compute);
        assert_eq!(r.submit(0xB), Admission::Compute);
        assert_eq!(r.submit(0xA), Admission::Attach);
        assert_eq!(r.complete(0xB), 1);
        assert_eq!(r.complete(0xA), 2);
        assert!(r.is_empty());
    }
}
