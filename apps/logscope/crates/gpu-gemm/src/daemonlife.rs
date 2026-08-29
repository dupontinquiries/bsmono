//! daemonlife — when the daemon should let go, and who gets to be it.
//!
//! The lifecycle is the last piece of the shared-weights daemon, and it
//! is the one that decides whether the thing is a help or a nuisance. A
//! daemon holding 7.2 GiB after everyone has closed their editor is
//! worse than no daemon: it took the memory the loader was already
//! clamping from 48 GiB to 31, and gave nothing back.
//!
//! Both decisions here are PURE — they take the clock rather than read
//! it — so the awkward cases (a request arriving during the unload
//! grace, a second daemon racing the first) are testable without a
//! process, a socket or a model.
//!
//! ## Why not just exit on last disconnect
//!
//! Because the expensive thing is the LOAD, not the process: 86 seconds
//! and a pipeline warm-up. An editor that completes, sits idle while
//! someone reads the result, then completes again would pay that twice
//! for a gap of seconds. The idle window exists to make the second
//! request cheap; it is not a politeness delay.

use std::time::Duration;

/// How long the daemon stays resident with nobody talking to it.
///
/// Ten minutes is long enough to cover reading a reply, switching
/// windows, and a compile — the gaps in real editing — and short enough
/// that a machine left alone gets its memory back before anyone would
/// think to ask for it.
pub const DEFAULT_IDLE: Duration = Duration::from_secs(600);

/// What the daemon should do right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Keep the weights resident.
    Stay,
    /// Release the model and exit.
    Unload,
}

/// The lifecycle state a caller keeps. Times are caller-supplied
/// durations since start, never `Instant::now()` — that is what makes
/// this testable.
#[derive(Debug, Clone, Copy)]
pub struct Life {
    /// When the last request finished.
    pub last_use: Duration,
    /// Requests currently being served.
    pub in_flight: u32,
    /// Idle window before unloading.
    pub idle: Duration,
}

impl Default for Life {
    fn default() -> Self {
        Life { last_use: Duration::ZERO, in_flight: 0, idle: DEFAULT_IDLE }
    }
}

impl Life {
    pub fn new(idle: Duration) -> Life {
        Life { idle, ..Default::default() }
    }

    /// A request arrived.
    pub fn begin(&mut self) {
        self.in_flight += 1;
    }

    /// A request finished at `now`.
    pub fn end(&mut self, now: Duration) {
        // Saturating because a stray `end` without a `begin` — a panicked
        // worker, a double-decrement during shutdown — must not wrap the
        // counter to four billion and make the daemon immortal.
        self.in_flight = self.in_flight.saturating_sub(1);
        self.last_use = now;
    }

    /// Should the daemon unload at `now`?
    ///
    /// Never while a request is in flight, however long the idle window
    /// says. A prefill can take a minute on a large prompt, and a daemon
    /// that unloads mid-generation turns a slow answer into a lost one.
    pub fn decide(&self, now: Duration) -> Decision {
        if self.in_flight > 0 {
            return Decision::Stay;
        }
        // Saturating: a `now` earlier than `last_use` (a clock the caller
        // computed oddly, a test) reads as "no time has passed" rather
        // than as a huge idle period that would unload immediately.
        if now.saturating_sub(self.last_use) >= self.idle {
            Decision::Unload
        } else {
            Decision::Stay
        }
    }
}

/// Why a second daemon should not start.
/// Not `Copy`: the third variant carries the other daemon's model path,
/// because a caller told only "something else is running" cannot write a
/// message worth reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartVerdict {
    /// No daemon is there; take the socket.
    Start,
    /// One is already serving this model — connect to it instead.
    AlreadyRunning,
    /// One is running a DIFFERENT model. Starting a second would defeat
    /// the point: two daemons hold two models' weights, which is the
    /// situation the daemon exists to avoid.
    RunningOtherModel { theirs: String },
}

/// Decide whether to start, given whatever a probe of the socket found.
///
/// `existing` is `None` when nothing answered, or the model path the
/// running daemon reported. Kept separate from the probe itself so the
/// awkward third case — a daemon running the wrong model — is decided in
/// one place with a name, rather than as an `if` inside a connect
/// routine.
pub fn should_start(existing: Option<&str>, want_model: &str) -> StartVerdict {
    match existing {
        None => StartVerdict::Start,
        Some(m) if m == want_model => StartVerdict::AlreadyRunning,
        Some(m) => StartVerdict::RunningOtherModel { theirs: m.to_string() },
    }
}

/// Watch a [`Life`] and call `on_unload` once it says so.
///
/// Blocks, so callers spawn it. Polls rather than waiting on a condvar
/// because the thing being waited for is a CLOCK, not an event — nothing
/// signals "ten minutes of nothing happened", and a poll every few
/// seconds costs nothing next to the weights it is guarding.
///
/// Returns after firing. The daemon's response to unload is to exit, and
/// a janitor that kept running would be watching a `Life` nobody updates
/// any more.
pub fn janitor(
    life: std::sync::Arc<std::sync::Mutex<Life>>,
    start: std::time::Instant,
    poll: Duration,
    on_unload: impl FnOnce(),
) {
    loop {
        std::thread::sleep(poll);
        let decision = match life.lock() {
            Ok(l) => l.decide(start.elapsed()),
            // Poisoned: a request thread panicked. Staying is the safe
            // read — unloading on the strength of state written by a
            // panicking thread could drop weights mid-generation.
            Err(_) => Decision::Stay,
        };
        if decision == Decision::Unload {
            on_unload();
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn an_idle_daemon_gives_the_memory_back() {
        // The whole reason the idle window exists. A daemon holding
        // 7.2 GiB after everyone closed their editor is worse than none.
        let mut l = Life::new(secs(600));
        l.begin();
        l.end(secs(100));
        assert_eq!(l.decide(secs(300)), Decision::Stay, "still within the window");
        assert_eq!(l.decide(secs(699)), Decision::Stay, "one second short");
        assert_eq!(l.decide(secs(700)), Decision::Unload, "exactly the window, at last");
    }

    #[test]
    fn it_never_unloads_mid_generation() {
        // A prefill can take a minute on a long prompt. Unloading during
        // one turns a slow answer into a lost one, and the person cannot
        // tell those apart from the outside.
        let mut l = Life::new(secs(1));
        l.begin();
        assert_eq!(l.decide(secs(99999)), Decision::Stay, "in flight outranks any idle window");
        l.end(secs(100));
        assert_eq!(l.decide(secs(200)), Decision::Unload, "and then it may go");
    }

    #[test]
    fn overlapping_requests_both_have_to_finish() {
        // Two editors completing at once. The first `end` must not clear
        // the way for an unload while the second is still running.
        let mut l = Life::new(secs(10));
        l.begin();
        l.begin();
        l.end(secs(1));
        assert_eq!(l.decide(secs(1000)), Decision::Stay, "one still in flight");
        l.end(secs(2));
        assert_eq!(l.decide(secs(1000)), Decision::Unload);
    }

    #[test]
    fn a_stray_end_cannot_make_the_daemon_immortal() {
        // A panicked worker, or a double-decrement during shutdown.
        // Wrapping the counter would leave in_flight at four billion and
        // the daemon would never unload again — a memory leak with no
        // symptom until the machine swaps.
        let mut l = Life::new(secs(10));
        l.end(secs(1));
        l.end(secs(2));
        assert_eq!(l.in_flight, 0);
        assert_eq!(l.decide(secs(100)), Decision::Unload);
    }

    #[test]
    fn a_backwards_clock_does_not_unload_immediately() {
        // `now` before `last_use` should read as "no time passed", not as
        // a huge idle period. Saturating rather than panicking, because
        // an arithmetic overflow in the janitor thread would take the
        // daemon down with a request in flight.
        let mut l = Life::new(secs(10));
        l.end(secs(500));
        assert_eq!(l.decide(secs(100)), Decision::Stay);
    }

    #[test]
    fn the_janitor_fires_once_the_window_passes_and_then_stops() {
        use std::sync::{Arc, Mutex};
        let life = Arc::new(Mutex::new(Life::new(Duration::from_millis(30))));
        let fired = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (l2, f2) = (life.clone(), fired.clone());
        let start = std::time::Instant::now();
        let h = std::thread::spawn(move || {
            janitor(l2, start, Duration::from_millis(5), || {
                f2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            });
        });
        h.join().unwrap();
        assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 1,
                   "fires exactly once, then returns rather than watching a dead Life");
    }

    #[test]
    fn the_janitor_waits_while_a_request_is_in_flight() {
        // The property that matters most: a long prefill must not be
        // killed by the clock.
        use std::sync::{Arc, Mutex};
        let life = Arc::new(Mutex::new(Life::new(Duration::from_millis(10))));
        life.lock().unwrap().begin();
        let fired = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (l2, f2) = (life.clone(), fired.clone());
        let start = std::time::Instant::now();
        std::thread::spawn(move || {
            janitor(l2, start, Duration::from_millis(5), || {
                f2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            });
        });
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 0,
                   "unloaded while a request was in flight");
    }

    #[test]
    fn a_second_daemon_for_the_same_model_defers_to_the_first() {
        assert_eq!(should_start(None, "a.gguf"), StartVerdict::Start);
        assert_eq!(should_start(Some("a.gguf"), "a.gguf"), StartVerdict::AlreadyRunning);
    }

    #[test]
    fn a_daemon_running_a_different_model_is_its_own_case() {
        // Starting a second one here would put TWO models' weights on a
        // machine that could not hold them — the exact situation the
        // daemon exists to prevent — so this is named rather than folded
        // into Start, and the caller has to decide what to tell the
        // person.
        let v = should_start(Some("big.gguf"), "small.gguf");
        assert_eq!(v, StartVerdict::RunningOtherModel { theirs: "big.gguf".into() });
        assert_ne!(v, StartVerdict::Start, "must not silently start a second daemon");
    }
}
