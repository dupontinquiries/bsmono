//! task — run a blocking cockpit call (ssh, build) off the UI thread and poll the result, so the
//! egui frame never freezes. The egui analogue of the Qt GUI's shared Job.
use std::sync::{Arc, Mutex};

#[derive(Default, Clone)]
pub struct Task {
    inner: Arc<Mutex<TaskState>>,
}

#[derive(Default)]
struct TaskState {
    running: bool,
    done: bool,
    result: String,
}

impl Task {
    pub fn is_running(&self) -> bool {
        self.inner.lock().unwrap().running
    }
    /// Take the finished result once (clears it), or None if not done.
    pub fn take_result(&self) -> Option<String> {
        let mut s = self.inner.lock().unwrap();
        if s.done {
            s.done = false;
            Some(std::mem::take(&mut s.result))
        } else {
            None
        }
    }
    /// Spawn `f` on a worker thread; its return becomes the result. Refuses (returns false) if a
    /// job is already running, so the UI can't launch two at once.
    pub fn run<F: FnOnce() -> String + Send + 'static>(&self, f: F) -> bool {
        {
            let mut s = self.inner.lock().unwrap();
            if s.running {
                return false;
            }
            *s = TaskState { running: true, done: false, result: String::new() };
        }
        let inner = self.inner.clone();
        std::thread::spawn(move || {
            let r = f();
            let mut s = inner.lock().unwrap();
            s.result = r;
            s.running = false;
            s.done = true;
        });
        true
    }
}
