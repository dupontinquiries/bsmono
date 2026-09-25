//! model_pool -- one loaded transcription model, handed from worker to worker.
//!
//! Reported from a Smite run (Fedora Asahi): the whisper model loaded TWICE.
//! A streamed URL starts transcription on the stream; when the download to
//! /tmp finishes, play_kept starts it again on the file, and each Worker's
//! factory loaded its own WhisperSource -- ~59 MB of weights and ~200 MB of
//! buffers resident twice, and seconds of loading, while the old worker
//! finished its chunk.
//!
//! Now a factory BORROWS the model: from the pool if one is there; else, if a
//! worker still holds it, it WAITS briefly for that worker to finish its chunk
//! and hand it back (the handoff is seconds); only then, or on a first use,
//! does it load. The borrowed source returns itself to the pool when its
//! worker drops it (`Pooled`'s Drop). A model nobody has borrowed for a while
//! is let go (`trim`, called from the UI loop) -- the phone's memory is worth
//! more than a warm model for a video that may never come.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use vidkit_transcribe::{Cue, CueSource};

struct Slot {
    idle: Option<(Box<dyn CueSource>, Instant)>,
    lent: usize,
}

pub struct Pool {
    slot: Mutex<Slot>,
    cv: Condvar,
}

impl Pool {
    pub const fn new() -> Pool {
        Pool { slot: Mutex::new(Slot { idle: None, lent: 0 }), cv: Condvar::new() }
    }

    /// The pooled model; or, while another worker holds it, up to `wait` for it
    /// to come back; or a fresh one from `make`.
    pub fn take_or(&'static self, wait: Duration, make: impl FnOnce() -> Result<Box<dyn CueSource>, String>)
        -> Result<Box<dyn CueSource>, String> {
        let deadline = Instant::now() + wait;
        let mut g = self.slot.lock().unwrap();
        loop {
            if let Some((s, _)) = g.idle.take() {
                g.lent += 1;
                return Ok(Box::new(Pooled { inner: Some(s), pool: self }));
            }
            let now = Instant::now();
            if g.lent == 0 || now >= deadline {
                break;
            }
            g = self.cv.wait_timeout(g, deadline - now).unwrap().0;
        }
        g.lent += 1;
        drop(g); // loading takes seconds: not under the lock
        match make() {
            Ok(s) => Ok(Box::new(Pooled { inner: Some(s), pool: self })),
            Err(e) => {
                self.slot.lock().unwrap().lent -= 1;
                Err(e)
            }
        }
    }

    fn give_back(&self, s: Box<dyn CueSource>) {
        let mut g = self.slot.lock().unwrap();
        g.lent = g.lent.saturating_sub(1);
        // One is kept; a second (a timed-out wait loaded its own) is dropped.
        if g.idle.is_none() {
            g.idle = Some((s, Instant::now()));
        }
        self.cv.notify_all();
    }

    /// Let go of a model nobody has borrowed for `max_idle`.
    pub fn trim(&self, max_idle: Duration) {
        let mut g = self.slot.lock().unwrap();
        if g.idle.as_ref().is_some_and(|(_, since)| since.elapsed() >= max_idle) {
            g.idle = None;
        }
    }

    /// (a model is idle in the pool, how many are lent out) -- for tests.
    pub fn state(&self) -> (bool, usize) {
        let g = self.slot.lock().unwrap();
        (g.idle.is_some(), g.lent)
    }
}

/// A borrowed model: a CueSource that goes back to its pool when dropped.
struct Pooled {
    inner: Option<Box<dyn CueSource>>,
    pool: &'static Pool,
}

impl CueSource for Pooled {
    fn transcribe(&mut self, pcm: &[f32], offset: Duration) -> Result<Vec<Cue>, String> {
        self.inner.as_mut().expect("present until drop").transcribe(pcm, offset)
    }
    fn name(&self) -> String {
        self.inner.as_ref().map(|s| s.name()).unwrap_or_default()
    }
    fn complete(&mut self) -> Option<Vec<Cue>> {
        self.inner.as_mut().and_then(|s| s.complete())
    }
}

impl Drop for Pooled {
    fn drop(&mut self) {
        if let Some(s) = self.inner.take() {
            self.pool.give_back(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake(usize);
    impl CueSource for Fake {
        fn transcribe(&mut self, _: &[f32], _: Duration) -> Result<Vec<Cue>, String> { Ok(Vec::new()) }
        fn name(&self) -> String { format!("fake#{}", self.0) }
    }

    fn leak() -> &'static Pool { Box::leak(Box::new(Pool::new())) }

    #[test]
    fn a_second_worker_gets_the_first_workers_model_instead_of_loading_one() {
        let pool = leak();
        let loads = std::sync::Arc::new(AtomicUsize::new(0));
        let make = |l: std::sync::Arc<AtomicUsize>| move || -> Result<Box<dyn CueSource>, String> {
            let n = l.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Fake(n)))
        };
        let first = pool.take_or(Duration::from_secs(5), make(loads.clone())).unwrap();
        assert_eq!(first.name(), "fake#0");
        // The download finished: a new worker starts while the old one is still
        // mid-chunk, and lets go 100 ms later.
        let h = std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(100)); drop(first); });
        let second = pool.take_or(Duration::from_secs(5), make(loads.clone())).unwrap();
        h.join().unwrap();
        assert_eq!(second.name(), "fake#0", "the same model, handed over");
        assert_eq!(loads.load(Ordering::SeqCst), 1, "loaded once");
        drop(second);
        assert_eq!(pool.state(), (true, 0), "back in the pool");
    }

    #[test]
    fn a_holder_that_never_lets_go_costs_only_the_wait() {
        let pool = leak();
        let held = pool.take_or(Duration::ZERO, || Ok(Box::new(Fake(0)))).unwrap();
        let t = Instant::now();
        let other = pool.take_or(Duration::from_millis(80), || Ok(Box::new(Fake(1)))).unwrap();
        assert!(t.elapsed() >= Duration::from_millis(80), "waited for the handoff");
        assert_eq!(other.name(), "fake#1", "then loaded its own");
        drop(held);
        drop(other);
        assert_eq!(pool.state(), (true, 0), "one kept, the second let go");
    }

    #[test]
    fn a_failed_load_is_not_counted_and_an_idle_model_is_trimmed() {
        let pool = leak();
        assert!(pool.take_or(Duration::ZERO, || Err("no model".into())).is_err());
        assert_eq!(pool.state(), (false, 0));
        drop(pool.take_or(Duration::ZERO, || Ok(Box::new(Fake(0)))).unwrap());
        pool.trim(Duration::from_secs(3600));
        assert_eq!(pool.state().0, true, "recently used: kept");
        pool.trim(Duration::ZERO);
        assert_eq!(pool.state(), (false, 0), "idle past the limit: let go");
    }
}
