//! envlock — set an environment variable in a test without breaking the
//! tests running beside it.
//!
//! ## The bug this exists for, four times over
//!
//! `std::env::set_var` mutates the whole process. `cargo test` runs a
//! binary's tests on many threads. Put those together and a test that sets
//! `HOME` to a scratch directory is silently altering every other test that
//! reads `HOME`, for as long as it runs.
//!
//! It has bitten this fleet four times, and the instructive part is that the
//! symptom never looked like a race:
//!
//! * **switcheroo** — a file written to a temp `XDG_DATA_HOME` "did not
//!   exist". It did; the variable had changed underneath the read.
//! * **voiceterm** — a mail pane claimed a store existed when the test had
//!   just created an empty one. The test controlled `HOME` and `MAIL` but not
//!   `USER`, and `/var/mail/$USER` on the build host is real.
//! * **bo-mux** — a "dead socket" test connected to the **actual running
//!   daemon** and got a status object where it wanted an error. A sibling
//!   test had set `XDG_RUNTIME_DIR` and never put it back.
//! * **gitmind** — a duplicated `#[test]` attribute ran one test twice
//!   concurrently, and the two copies deleted each other's fixtures.
//!
//! In every case the code under test was correct and the test was blamed for
//! several sessions. That is the real cost: not the failure, but the hours
//! spent reading the wrong file.
//!
//! ## What the guard does that a bare mutex does not
//!
//! Three crates already grew a hand-rolled `ENV_TEST_LOCK`. A lock alone is
//! not enough, and `bo-mux` showed why — its `client.rs` test **never
//! restored the variable at all**, so it was not merely racing its neighbour,
//! it left `/run/user/1000` set for whatever ran next. A lock that you have
//! to remember to pair with a manual restore has the same shape as the bug.
//!
//! So restoration is on `Drop`:
//!
//! ```no_run
//! # fn main() {
//! let _env = envlock_kit::guard()
//!     .set("HOME", "/tmp/scratch")
//!     .remove("MAIL");
//! // ... the test ...
//! # }
//! // lock released and every variable put back, including on panic.
//! ```
//!
//! ## Scope
//!
//! The lock is a `static` in this crate, so each test binary that links it
//! gets its own. That is exactly right: the race is between threads of one
//! process, and a cross-process lock would serialise unrelated crates for no
//! reason.
//!
//! This does not make `set_var` safe in production code, and it is not meant
//! to. It makes a *test* honest about the fact that it is reaching into
//! global state.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};

/// The one lock. Private: a caller who could take it directly could also
/// forget to restore, which is the failure mode this crate removes.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Take the environment lock. Released when the returned guard drops.
pub fn guard() -> EnvGuard {
    // `into_inner` on a poisoned lock rather than unwrap: a test that panics
    // while holding this must not cascade into every other test failing with
    // "PoisonError", which turns one real failure into a wall of noise and
    // buries it.
    let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    EnvGuard { _lock: lock, prior: BTreeMap::new() }
}

/// Holds the lock and remembers what to put back.
pub struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    /// Original value per key. `None` means the variable was absent, which is
    /// DIFFERENT from empty and has to be restored as absence — a stray empty
    /// `HOME` is not the same as no `HOME`, and code that falls back on
    /// absence would take the wrong branch.
    prior: BTreeMap<String, Option<OsString>>,
}

impl EnvGuard {
    /// Remember the current value of `key`, once.
    ///
    /// Only the FIRST observation is kept: a test that sets the same variable
    /// twice must be restored to what it was before the test, not to what it
    /// was between its own two writes.
    fn remember(&mut self, key: &str) {
        self.prior
            .entry(key.to_string())
            .or_insert_with(|| std::env::var_os(key));
    }

    /// Set a variable for the lifetime of the guard.
    pub fn set(mut self, key: &str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        self.remember(key);
        std::env::set_var(key, value);
        self
    }

    /// Remove a variable for the lifetime of the guard.
    pub fn remove(mut self, key: &str) -> Self {
        self.remember(key);
        std::env::remove_var(key);
        self
    }

    /// Set several at once.
    pub fn set_all<'a, I, K, V>(mut self, pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<std::ffi::OsStr>,
    {
        for (k, v) in pairs {
            self = self.set(k.as_ref(), v);
        }
        self
    }

    /// Variables this guard will restore. For diagnostics and tests.
    pub fn tracked(&self) -> Vec<&str> {
        self.prior.keys().map(String::as_str).collect()
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // Restore before the lock releases — the MutexGuard field is declared
        // first, so it drops last, and no other test can observe the
        // half-restored environment in between.
        for (k, v) in &self.prior {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The core promise: set it, and it goes back by itself.
    #[test]
    fn a_variable_is_restored_when_the_guard_drops() {
        let key = "ENVLOCK_TEST_BASIC";
        std::env::set_var(key, "before");
        {
            let _g = guard().set(key, "during");
            assert_eq!(std::env::var(key).unwrap(), "during");
        }
        assert_eq!(std::env::var(key).unwrap(), "before");
    }

    /// ABSENCE IS NOT EMPTINESS. Restoring a previously-unset variable as ""
    /// would send code that branches on absence down the wrong path — the
    /// exact shape of voiceterm's `$USER` bug, where the fallback path was
    /// the one that mattered.
    #[test]
    fn a_variable_that_was_absent_is_restored_as_absent() {
        let key = "ENVLOCK_TEST_ABSENT";
        std::env::remove_var(key);
        {
            let _g = guard().set(key, "x");
            assert!(std::env::var_os(key).is_some());
        }
        assert!(std::env::var_os(key).is_none(), "must be absent, not empty");
    }

    #[test]
    fn remove_is_restored_too() {
        let key = "ENVLOCK_TEST_REMOVE";
        std::env::set_var(key, "kept");
        {
            let _g = guard().remove(key);
            assert!(std::env::var_os(key).is_none());
        }
        assert_eq!(std::env::var(key).unwrap(), "kept");
    }

    /// Restore to the value from BEFORE the test, not to an intermediate one.
    #[test]
    fn setting_twice_restores_the_original_not_the_middle_value() {
        let key = "ENVLOCK_TEST_TWICE";
        std::env::set_var(key, "original");
        {
            let _g = guard().set(key, "first").set(key, "second");
            assert_eq!(std::env::var(key).unwrap(), "second");
        }
        assert_eq!(std::env::var(key).unwrap(), "original");
    }

    /// THE bo-mux FAILURE. A test that panics must still put the environment
    /// back, or the next test inherits it — that one never restored at all
    /// and left /run/user/1000 set for whatever ran next.
    #[test]
    fn a_panicking_test_still_restores() {
        let key = "ENVLOCK_TEST_PANIC";
        std::env::set_var(key, "before");
        let r = std::panic::catch_unwind(|| {
            let _g = guard().set(key, "during");
            panic!("as a test would");
        });
        assert!(r.is_err());
        assert_eq!(std::env::var(key).unwrap(), "before", "Drop ran despite the panic");
    }

    /// ...and the lock must not stay poisoned afterwards, or one real failure
    /// becomes a wall of PoisonError that buries it.
    #[test]
    fn a_panic_does_not_poison_the_lock_for_everyone_else() {
        let _ = std::panic::catch_unwind(|| {
            let _g = guard().set("ENVLOCK_TEST_POISON", "x");
            panic!("boom");
        });
        // Still usable.
        let g = guard().set("ENVLOCK_TEST_POISON2", "y");
        assert_eq!(std::env::var("ENVLOCK_TEST_POISON2").unwrap(), "y");
        drop(g);
    }

    #[test]
    fn several_variables_restore_together() {
        let (a, b) = ("ENVLOCK_TEST_MULTI_A", "ENVLOCK_TEST_MULTI_B");
        std::env::set_var(a, "1");
        std::env::remove_var(b);
        {
            let g = guard().set_all([(a, "2"), (b, "3")]);
            assert_eq!(g.tracked(), vec![a, b]);
            assert_eq!(std::env::var(a).unwrap(), "2");
            assert_eq!(std::env::var(b).unwrap(), "3");
        }
        assert_eq!(std::env::var(a).unwrap(), "1");
        assert!(std::env::var_os(b).is_none());
    }

    /// The guard is what serialises; two of them cannot be held at once.
    /// Proven by the fact that a second `guard()` on another thread blocks
    /// until the first drops.
    #[test]
    fn the_guard_serialises_concurrent_tests() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let inside = Arc::new(AtomicBool::new(false));
        let g = guard().set("ENVLOCK_TEST_SERIAL", "held");
        let flag = inside.clone();
        let t = std::thread::spawn(move || {
            let _g2 = guard();
            flag.store(true, Ordering::SeqCst);
        });
        // The other thread cannot have taken the lock while we hold it.
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!inside.load(Ordering::SeqCst), "second guard entered while the first was held");
        drop(g);
        t.join().unwrap();
        assert!(inside.load(Ordering::SeqCst), "and proceeds once released");
    }
}
