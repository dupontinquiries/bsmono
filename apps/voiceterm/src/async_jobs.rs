// src/async_jobs.rs
// Generic cancellable async work registry.
//
// Use case: QML spawns a "fragment check" or any other async backend
// task and receives a job_id. While the task runs, QML can poll for
// the result OR cancel it. Cancellation kills any subprocess the task
// registered, AND sets a flag the task can poll to short-circuit
// cooperatively (e.g. between iterations of an inference loop).
//
// Why this pattern:
//   - The current llm_query_async pattern has no cancellation hook —
//     subprocess child is owned by ollama_query_model and isn't reachable
//     once spawned. If QML wants to abort because the user said "yes"
//     before the LLM finished, it can't.
//   - Future use cases (cancel whisper inference when fingerprint match
//     is high enough, cancel intent-classifier LLM when phonetic confidence
//     overrides it) want the same plumbing.
//   - Building one shared registry now means future-us doesn't have to
//     re-invent it per call site.
//
// Lifecycle: a job is in one of these states
//   - Running     — thread is alive, no result yet
//   - Cancelled   — cancel was called; thread may still be winding down
//                   but its result will not be returned
//   - Done(value) — thread completed, result ready for poll
//   - Consumed    — poll returned the value; subsequent polls return None
//
// The registry is process-global. Job IDs are monotonic u64 so they
// never collide. We don't garbage-collect old Consumed entries because
// the registry is keyed by id and holding a few hundred dead entries
// across an app session costs nothing. If memory ever becomes a concern,
// add a periodic compaction.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Monotonic job ID source. First job is 1; 0 means "no job".
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// State held per job. Both flags are atomic so the worker thread can
/// cheaply check `is_cancelled()` between iterations without locking
/// the registry.
struct JobState {
    /// Set true by cancel_job(). Worker should poll this and exit early.
    cancelled: Arc<AtomicBool>,
    /// Subprocess PID if the worker registered one, for kill-on-cancel.
    /// Wrapped in Mutex<Option> so the worker can register/unregister
    /// the PID at the moment it spawns/reaps the child.
    child_pid: Arc<Mutex<Option<u32>>>,
    /// Result slot. None until the worker writes; replaced with None
    /// once poll_job consumes it.
    result: Arc<Mutex<Option<Result<String, String>>>>,
}

impl JobState {
    fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            child_pid: Arc::new(Mutex::new(None)),
            result: Arc::new(Mutex::new(None)),
        }
    }
}

/// Handle passed to the worker closure so it can check cancellation
/// and register a subprocess PID for kill-on-cancel.
pub struct JobHandle {
    cancelled: Arc<AtomicBool>,
    child_pid: Arc<Mutex<Option<u32>>>,
}

impl JobHandle {
    /// True if cancel_job() has been called for this job. Workers should
    /// check this between expensive operations and bail out early.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Register a subprocess PID so cancel_job() can kill it externally.
    /// Call this immediately after spawning. Pass None to unregister
    /// once the child has been waited on.
    pub fn set_child_pid(&self, pid: Option<u32>) {
        if let Ok(mut g) = self.child_pid.lock() { *g = pid; }
    }
}

static REGISTRY: OnceLock<Mutex<HashMap<u64, JobState>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<u64, JobState>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Spawn a new async job. The closure runs on a fresh thread and
/// receives a JobHandle for cooperative cancellation. Returns the
/// job ID, which QML/caller uses to poll or cancel.
///
/// The closure's return value (`Result<String, String>`) becomes the
/// poll result. Convention: Ok(s) for success, Err(s) for failure with
/// a human-readable reason.
pub fn start_job<F>(work: F) -> u64
where
    F: FnOnce(JobHandle) -> Result<String, String> + Send + 'static,
{
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let state = JobState::new();
    let handle = JobHandle {
        cancelled: state.cancelled.clone(),
        child_pid: state.child_pid.clone(),
    };
    let result_slot = state.result.clone();
    let cancelled_flag = state.cancelled.clone();

    if let Ok(mut g) = registry().lock() {
        g.insert(id, state);
    }

    std::thread::spawn(move || {
        let result = work(handle);
        // If cancelled, drop the result silently. The poll path will
        // never see it (we still write None below to signal "consumed").
        // This means a cancelled job that races to completion has its
        // result thrown away — exactly what cancel callers want.
        if cancelled_flag.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(mut g) = result_slot.lock() {
            *g = Some(result);
        }
    });

    id
}

/// Cancel a job by id. Sets the cancelled flag and kills any registered
/// subprocess PID. Returns true if the job exists; false if the id was
/// invalid (already consumed/never existed).
///
/// Killing the subprocess uses libc::kill(pid, SIGKILL). The worker
/// thread will then see ECHILD or read failure on its child handle and
/// exit. Either way the cancelled flag prevents its result from being
/// reported.
pub fn cancel_job(id: u64) -> bool {
    let g = match registry().lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    let Some(state) = g.get(&id) else { return false; };
    state.cancelled.store(true, Ordering::Relaxed);
    // Kill any subprocess the worker registered.
    if let Ok(pid_g) = state.child_pid.lock() {
        if let Some(pid) = *pid_g {
            // SAFETY: kill is signal-safe and accepts any pid; if the
            // process has already exited the call is a no-op (returns
            // ESRCH which we ignore).
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
    }
    true
}

/// Poll a job. Returns:
///   - Some(Ok(s))  → job succeeded, here's the value (registry slot cleared)
///   - Some(Err(s)) → job failed, here's why (registry slot cleared)
///   - None         → job still running, OR job id invalid, OR cancelled
///
/// Caller cannot distinguish "still running" from "doesn't exist" on
/// purpose: the polling pattern only cares whether a result is ready.
/// An invalid id will simply never return Some — the caller eventually
/// gives up.
pub fn poll_job(id: u64) -> Option<Result<String, String>> {
    let g = match registry().lock() {
        Ok(g) => g,
        Err(_) => return None,
    };
    let state = g.get(&id)?;
    let mut slot = state.result.lock().ok()?;
    slot.take()
}

/// Drop a finished job from the registry. Optional — the registry
/// holds entries forever otherwise. Call this from poll_job's caller
/// once a result has been consumed if you want to keep memory tight.
pub fn forget_job(id: u64) {
    if let Ok(mut g) = registry().lock() {
        g.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    /// Helper: spin-wait until predicate returns true or `max` elapses.
    /// Returns whether the predicate succeeded. Used because async work
    /// is racy by nature — we can't synchronously assert on a thread's
    /// completion.
    fn wait_for<F: Fn() -> bool>(max: Duration, f: F) -> bool {
        let start = Instant::now();
        while start.elapsed() < max {
            if f() { return true; }
            std::thread::sleep(Duration::from_millis(5));
        }
        f()
    }

    #[test]
    fn job_id_is_monotonic_and_unique() {
        // Spawn two no-op jobs; their IDs must differ and be > 0.
        let a = start_job(|_h| Ok(String::new()));
        let b = start_job(|_h| Ok(String::new()));
        assert!(a > 0, "id must be positive: {}", a);
        assert!(b > 0, "id must be positive: {}", b);
        assert_ne!(a, b, "ids must be unique");
        // Wait for the threads to finish so they don't leak past the
        // test. (No assertions on the polled value; this is purely a
        // hygiene step.)
        let _ = wait_for(Duration::from_secs(1), || poll_job(a).is_some());
        let _ = wait_for(Duration::from_secs(1), || poll_job(b).is_some());
    }

    #[test]
    fn poll_returns_ok_for_successful_job() {
        let id = start_job(|_h| Ok("hello".into()));
        let got = wait_for(Duration::from_secs(2), || {
            match poll_job(id) {
                Some(Ok(s)) => s == "hello",
                _ => false,
            }
        });
        assert!(got, "did not see Ok(\"hello\") within 2s");
    }

    #[test]
    fn poll_returns_err_for_failed_job() {
        let id = start_job(|_h| Err("nope".into()));
        let got = wait_for(Duration::from_secs(2), || {
            match poll_job(id) {
                Some(Err(s)) => s == "nope",
                _ => false,
            }
        });
        assert!(got, "did not see Err(\"nope\") within 2s");
    }

    #[test]
    fn poll_returns_none_before_completion() {
        // Block the worker for a moment; poll immediately should see None.
        let id = start_job(|_h| {
            std::thread::sleep(Duration::from_millis(150));
            Ok(String::new())
        });
        // Poll on the same thread immediately — way before 150ms.
        let r = poll_job(id);
        assert!(r.is_none(), "poll should return None while running, got {:?}", r);
        // Drain.
        let _ = wait_for(Duration::from_secs(1), || poll_job(id).is_some());
    }

    #[test]
    fn poll_returns_none_for_invalid_id() {
        // ID 0 is reserved as "no job"; arbitrary far-future ID is
        // also invalid (we never wrap).
        assert!(poll_job(0).is_none());
        assert!(poll_job(u64::MAX).is_none());
    }

    #[test]
    fn cancel_returns_false_for_invalid_id() {
        assert!(!cancel_job(u64::MAX),
            "cancel should report false for unknown id");
    }

    #[test]
    fn cancel_returns_true_for_known_id() {
        let id = start_job(|h| {
            // Cooperative loop: check cancelled flag, exit early.
            for _ in 0..50 {
                if h.is_cancelled() { return Err("cancelled".into()); }
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok("done".into())
        });
        assert!(cancel_job(id), "cancel of known id must return true");
        // Worker may have written its result before noticing the flag,
        // but the cancelled-flag check in start_job's thread closure
        // drops the result. So poll should NEVER return Some for a
        // cancelled job that hadn't already completed.
        std::thread::sleep(Duration::from_millis(80));
        assert!(poll_job(id).is_none(),
            "cancelled job must not deliver a result");
    }

    #[test]
    fn worker_can_observe_cancellation() {
        // Counter the worker increments while it's running; we cancel
        // before the loop finishes and assert the counter is well under
        // the max.
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();
        let id = start_job(move |h| {
            for _ in 0..200 {
                if h.is_cancelled() { return Err("aborted".into()); }
                counter_clone.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok("full run".into())
        });
        // Let the worker start ticking.
        std::thread::sleep(Duration::from_millis(30));
        cancel_job(id);
        // Give the worker a beat to wake up and notice.
        std::thread::sleep(Duration::from_millis(80));
        let observed = counter.load(Ordering::Relaxed);
        assert!(observed > 0, "worker didn't run at all");
        assert!(observed < 200,
            "worker ran to completion despite cancel ({} iters)", observed);
    }

    #[test]
    fn poll_consumes_result() {
        // After a successful poll, a second poll should return None
        // because the result slot is taken.
        let id = start_job(|_h| Ok("once".into()));
        // Wait for the result to be ready, but don't consume it via
        // wait_for. Race-free way: just wait deterministically and
        // then poll once.
        std::thread::sleep(Duration::from_millis(50));
        let first = poll_job(id);
        assert_eq!(first, Some(Ok("once".into())),
            "first poll should return Ok(\"once\"), got {:?}", first);
        let second = poll_job(id);
        assert_eq!(second, None,
            "second poll should return None (consumed), got {:?}", second);
    }

    #[test]
    fn forget_job_removes_from_registry() {
        let id = start_job(|_h| Ok("bye".into()));
        // Wait so the worker drops out cleanly.
        std::thread::sleep(Duration::from_millis(40));
        forget_job(id);
        // After forget, poll_job for that id must return None even if
        // the result was sitting in the slot.
        assert!(poll_job(id).is_none(),
            "after forget_job, poll must not return a value");
        // cancel_job for a forgotten id returns false (it's invalid).
        assert!(!cancel_job(id), "cancel on forgotten id must return false");
    }

    #[test]
    fn handle_set_child_pid_does_not_panic_on_none() {
        // Defensive: register and unregister a pid. The test is mostly
        // a smoke check that the locking path doesn't deadlock or
        // panic; we don't actually have a child to kill.
        let id = start_job(|h| {
            h.set_child_pid(Some(99999));
            h.set_child_pid(None);
            Ok(String::new())
        });
        // Wait for the job to complete.
        let drained = wait_for(Duration::from_secs(1),
            || poll_job(id).is_some());
        assert!(drained, "set_child_pid path did not complete");
    }
}
