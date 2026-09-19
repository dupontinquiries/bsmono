//! Driver — a Session, its Backend, and the vitals glue, as one unit that can
//! live on whatever thread receives the backend's events.
//!
//! Why this is not left to each shell. The CLI and the GTK shell handle events
//! on a loop that wakes the moment one arrives, so they could afford to do this
//! glue inline. An immediate-mode shell cannot: its "loop" is the next painted
//! frame, which is a vsync away on hardware and tens of milliseconds under
//! software GL. Two things must not wait that long:
//!
//!   * a frame-marker stop. The target is halted until the session resumes it,
//!     so resume latency is added to every frame of the program being measured.
//!     Measured in the egui shell with the session on the UI thread: p50 12.5 ms
//!     for a 3 ms frame, against 4.2 ms from the CLI. The instrument was
//!     measuring the debugger's repaint rate.
//!   * the last vitals sample. It is meant to be the state AT the stop, and the
//!     rings must freeze before the sampler overwrites the run-up to it.
//!
//! So the shell hands events to `Driver::handle` from the receiving thread and
//! gets back the `Change`s it should render, already acted on where acting is
//! time-critical. `version` lets a shell that mirrors session state notice that
//! it changed without diffing.

use crate::backend::{Backend, Event, Request};
use crate::model::RunState;
use crate::session::{Change, Session};
use crate::vitals::{self, Sampler};
use std::time::Duration;

pub struct Driver {
    pub session: Session,
    backend: Box<dyn Backend + Send>,
    pub vitals: vitals::Shared,
    sampler: Option<Sampler>,
    sample_every: Duration,
    /// Bumped on every handled event and every command.
    pub version: u64,
}

impl Driver {
    pub fn new(session: Session, backend: Box<dyn Backend + Send>, vitals: vitals::Shared) -> Self {
        Driver { session, backend, vitals, sampler: None, sample_every: Duration::from_millis(100), version: 0 }
    }

    pub fn handle(&mut self, ev: Event) -> Vec<Change> {
        let changes = self.session.handle(ev, self.backend.as_mut());
        self.after(changes)
    }

    /// Run a session command (`up`, `select_frame`, `evaluate`, ...).
    pub fn with(&mut self, f: impl FnOnce(&mut Session, &mut dyn Backend) -> Vec<Change>) -> Vec<Change> {
        let changes = f(&mut self.session, self.backend.as_mut());
        self.after(changes)
    }

    pub fn submit(&mut self, req: Request) {
        self.backend.submit(req);
        self.version += 1;
    }

    fn after(&mut self, changes: Vec<Change>) -> Vec<Change> {
        self.version += 1;
        for c in &changes {
            match c {
                Change::Started { pid: Some(pid) } => {
                    // A rerun is a new process; the old run-up is not its history.
                    self.vitals.lock().unwrap().reset();
                    self.sampler = Some(Sampler::start(*pid, self.sample_every, self.vitals.clone()));
                }
                Change::State(RunState::Stopped) => {
                    if let Some(pid) = self.session.pid {
                        vitals::sample_now(pid, &self.vitals);
                    }
                    self.vitals.lock().unwrap().set_frozen(true);
                }
                Change::State(RunState::Running) => self.vitals.lock().unwrap().set_frozen(false),
                Change::State(RunState::Exited) | Change::Gone(_) => self.sampler = None,
                Change::FrameMark(at) => self.vitals.lock().unwrap().mark_frame(*at),
                _ => {}
            }
        }
        changes
    }
}

/// Whether a batch of changes is worth waking a UI for. Wire lines, target
/// output and frame marks arrive constantly while the target runs; a shell that
/// repaints on each of them spends the phone's battery redrawing a page nobody
/// is reading. They are still delivered, just not announced.
pub fn wants_repaint(changes: &[Change]) -> bool {
    changes.iter().any(|c| !matches!(c, Change::Wire { .. } | Change::FrameMark(_) | Change::Output { .. }))
}
