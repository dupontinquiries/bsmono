//! task — run a blocking cockpit call (ssh, build, log-follow) off the UI thread, streaming live
//! log lines and polling the result, so the egui frame never freezes. The egui analogue of the Qt
//! GUI's shared Job (start / log_line / tail / finish + a cancel flag for `-f` streams).
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Default, Clone)]
pub struct Task {
    inner: Arc<Mutex<TaskState>>,
    cancel: Arc<AtomicBool>,
}

struct TaskState {
    running: bool,
    done: bool,
    result: String,
    /// (ms elapsed since the run started, line) — the timestamp is what lets
    /// `cockpit::build_report` turn a raw log into "time by crate" without
    /// re-running anything. Kept alongside the plain-`String` log rather than
    /// replacing it: `log()` stays the cheap, existing shape every current
    /// caller (`uikit_egui::scroll::log`) already uses.
    log: Vec<(u64, String)>,
    started: Option<Instant>,
}

impl Default for TaskState {
    fn default() -> Self {
        TaskState { running: false, done: false, result: String::new(), log: Vec::new(), started: None }
    }
}

/// A handle a worker closure uses to push live log lines back to the UI.
#[derive(Clone)]
pub struct LogSink {
    inner: Arc<Mutex<TaskState>>,
}

impl LogSink {
    pub fn push(&self, line: &str) {
        let mut s = self.inner.lock().unwrap();
        let ms = s.started.map(|t| t.elapsed().as_millis() as u64).unwrap_or(0);
        s.log.push((ms, line.to_string()));
        let n = s.log.len();
        if n > 2000 {
            s.log.drain(0..n - 2000);
        }
    }
}

impl Task {
    pub fn is_running(&self) -> bool {
        self.inner.lock().unwrap().running
    }
    pub fn take_result(&self) -> Option<String> {
        let mut s = self.inner.lock().unwrap();
        if s.done {
            s.done = false;
            Some(std::mem::take(&mut s.result))
        } else {
            None
        }
    }
    pub fn log(&self) -> Vec<String> {
        self.inner.lock().unwrap().log.iter().map(|(_, l)| l.clone()).collect()
    }
    /// Same log, with each line's elapsed-ms since the run started — the input
    /// `cockpit::build_report`'s error/timing/export functions read.
    pub fn timed_log(&self) -> Vec<cockpit::build_report::TimedLine> {
        self.inner.lock().unwrap().log.iter()
            .map(|(ms, text)| cockpit::build_report::TimedLine { ms: *ms, text: text.clone() })
            .collect()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    fn claim(&self) -> bool {
        let mut s = self.inner.lock().unwrap();
        if s.running {
            return false;
        }
        *s = TaskState { running: true, done: false, result: String::new(), log: Vec::new(), started: Some(Instant::now()) };
        self.cancel.store(false, Ordering::Relaxed);
        true
    }
    fn finish(&self, result: String) {
        let mut s = self.inner.lock().unwrap();
        s.result = result;
        s.running = false;
        s.done = true;
    }

    pub fn run<F: FnOnce() -> String + Send + 'static>(&self, f: F) -> bool {
        if !self.claim() {
            return false;
        }
        let this = self.clone();
        std::thread::spawn(move || {
            let r = f();
            this.finish(r);
        });
        true
    }

    pub fn run_logged<F: FnOnce(LogSink) -> String + Send + 'static>(&self, f: F) -> bool {
        if !self.claim() {
            return false;
        }
        let sink = LogSink { inner: self.inner.clone() };
        let this = self.clone();
        std::thread::spawn(move || {
            let r = f(sink);
            this.finish(r);
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn run_logged_streams_lines_and_finishes() {
        let t = Task::default();
        assert!(t.run_logged(|sink| {
            for i in 0..3 { sink.push(&format!("line {i}")); }
            "done".to_string()
        }));
        // wait for the worker
        for _ in 0..200 {
            if !t.is_running() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(t.log().len(), 3);
        assert_eq!(t.take_result().as_deref(), Some("done"));
        assert!(t.take_result().is_none(), "result taken once");
    }
    #[test]
    fn refuses_second_concurrent_run() {
        let t = Task::default();
        assert!(t.run(|| { std::thread::sleep(std::time::Duration::from_millis(50)); "a".into() }));
        assert!(!t.run(|| "b".into()), "second run refused while first is in flight");
    }
    #[test]
    fn timed_log_carries_nondecreasing_elapsed_ms() {
        let t = Task::default();
        assert!(t.run_logged(|sink| {
            sink.push("first");
            std::thread::sleep(std::time::Duration::from_millis(10));
            sink.push("second");
            "done".to_string()
        }));
        for _ in 0..200 {
            if !t.is_running() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let tl = t.timed_log();
        assert_eq!(tl.len(), 2);
        assert_eq!(tl[0].text, "first");
        assert_eq!(tl[1].text, "second");
        assert!(tl[1].ms >= tl[0].ms, "elapsed time doesn't go backward");
    }
}
