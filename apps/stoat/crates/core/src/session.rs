//! Session state, sans-IO. Events go in, `Change`s come out, follow-up requests
//! are submitted to whatever `Backend` is passed along. No threads, no clocks, no
//! channels: this is the part that gets replayed against recorded transcripts.

use crate::backend::*;
use crate::diag::{self, Diagnostic};
use crate::model::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Started { pid: Option<u32> },
    State(RunState),
    Threads,
    Frames,
    /// Selected frame index changed (by the user or by blame selection).
    Selection(usize),
    Locals { frame: u32 },
    Value { expr: String, value: String },
    Diagnostic(Diagnostic),
    /// A frame-marker breakpoint was crossed. The session already resumed the
    /// target; shells forward the timestamp to `vitals::FrameLog`.
    FrameMark(Instant),
    Output { channel: Channel, text: String },
    Wire { dir: Dir, line: String },
    Gone(String),
}

enum Awaiting {
    Marker,
    PanicBreak,
    FaultAddr,
}

#[derive(Default)]
pub struct Session {
    pub state: RunState,
    pub pid: Option<u32>,
    pub threads: Vec<Thread>,
    pub thread: Option<u32>,
    pub frames: Vec<Frame>,
    pub selected: usize,
    pub locals: HashMap<u32, Vec<Variable>>,
    pub last_stop: Option<Stop>,
    pub fault_addr: Option<u64>,
    /// Frames under this directory are preferred when choosing which frame to
    /// show first after a stop.
    pub project_root: Option<PathBuf>,
    markers: HashSet<u32>,
    panic_breaks: HashSet<u32>,
    awaiting: HashMap<ReqId, Awaiting>,
    resumed_from_marker: bool,
}

impl Session {
    pub fn new(project_root: Option<PathBuf>) -> Self {
        Session { project_root, ..Default::default() }
    }

    /// A breakpoint that records a timestamp and resumes instead of stopping.
    /// Put it on whatever the program calls once per frame. This costs a full
    /// ptrace stop and an MI round trip per frame, so it perturbs what it
    /// measures; see `vitals` for what that means for the numbers.
    pub fn insert_frame_marker(&mut self, location: &str, b: &mut dyn Backend) {
        let id = b.submit(Request::InsertBreakpoint { location: location.into() });
        self.awaiting.insert(id, Awaiting::Marker);
    }

    /// Stop where a Rust panic happens instead of watching the process exit 101.
    ///
    /// Two locations because the symbol moved: rustc used to export an unmangled
    /// `rust_panic`; current toolchains (checked: 1.96) mangle it under a
    /// synthetic `__rustc` crate, and gdb (checked: 15.1) no longer resolves the
    /// bare name, leaving that breakpoint pending forever. Whichever does not
    /// apply stays pending and costs nothing. `rust_panic` rather than
    /// `rust_begin_unwind` so the panic message is already printed at the stop.
    pub fn break_on_rust_panic(&mut self, b: &mut dyn Backend) {
        for location in ["__rustc::rust_panic", "rust_panic"] {
            let id = b.submit(Request::InsertBreakpoint { location: location.into() });
            self.awaiting.insert(id, Awaiting::PanicBreak);
        }
    }

    pub fn select_frame(&mut self, idx: usize, b: &mut dyn Backend) -> Vec<Change> {
        if self.frames.is_empty() || self.state != RunState::Stopped {
            return Vec::new();
        }
        let idx = idx.min(self.frames.len() - 1);
        self.selected = idx;
        let level = self.frames[idx].level;
        if let (false, Some(thread)) = (self.locals.contains_key(&level), self.thread) {
            b.submit(Request::Locals { thread, frame: level });
        }
        vec![Change::Selection(idx)]
    }

    /// Toward the caller, as in gdb.
    pub fn up(&mut self, b: &mut dyn Backend) -> Vec<Change> {
        self.select_frame(self.selected + 1, b)
    }

    pub fn down(&mut self, b: &mut dyn Backend) -> Vec<Change> {
        self.select_frame(self.selected.saturating_sub(1), b)
    }

    pub fn select_thread(&mut self, id: u32, b: &mut dyn Backend) {
        if self.state == RunState::Stopped {
            self.thread = Some(id);
            self.frames.clear();
            self.locals.clear();
            b.submit(Request::Frames { thread: id });
        }
    }

    pub fn current_frame(&self) -> Option<&Frame> {
        self.frames.get(self.selected)
    }

    pub fn evaluate(&mut self, expr: &str, b: &mut dyn Backend) {
        if let (Some(thread), Some(f)) = (self.thread, self.current_frame()) {
            b.submit(Request::Evaluate { thread, frame: f.level, expr: expr.into() });
        }
    }

    fn blame_frame(&self) -> usize {
        let in_project = |f: &Frame| match (&self.project_root, &f.file) {
            (Some(root), Some(file)) => file.starts_with(root),
            _ => false,
        };
        self.frames
            .iter()
            .position(|f| f.has_source() && in_project(f))
            .or_else(|| self.frames.iter().position(Frame::has_source))
            .unwrap_or(0)
    }

    pub fn handle(&mut self, ev: Event, b: &mut dyn Backend) -> Vec<Change> {
        match ev {
            Event::Wire { dir, line } => vec![Change::Wire { dir, line }],
            Event::Output { channel, text } => vec![Change::Output { channel, text }],
            Event::BackendGone { detail } => {
                self.state = RunState::Exited;
                self.pid = None;
                vec![Change::Gone(detail)]
            }
            Event::Started { pid } => {
                self.pid = pid;
                vec![Change::Started { pid }]
            }
            Event::Running => {
                self.state = RunState::Running;
                self.frames.clear();
                self.locals.clear();
                self.fault_addr = None;
                if std::mem::take(&mut self.resumed_from_marker) {
                    return Vec::new();
                }
                vec![Change::State(RunState::Running)]
            }
            Event::Stopped(stop) => self.on_stop(stop, b),
            Event::Reply { id, result } => self.on_reply(id, result, b),
        }
    }

    fn on_stop(&mut self, stop: Stop, b: &mut dyn Backend) -> Vec<Change> {
        if let StopReason::Breakpoint { id } = stop.reason {
            if self.markers.contains(&id) {
                self.resumed_from_marker = true;
                b.submit(Request::Continue);
                return vec![Change::FrameMark(stop.at)];
            }
        }
        if stop.is_exit() {
            self.state = RunState::Exited;
            self.pid = None;
            let d = diag::for_stop(&stop, &[], 0, None, false);
            self.last_stop = Some(stop);
            return vec![Change::State(RunState::Exited), Change::Diagnostic(d)];
        }
        self.state = RunState::Stopped;
        self.thread = stop.thread.or(self.thread);
        let thread = self.thread.unwrap_or(1);
        // Order matters: gdb answers in submission order, so the fault address is
        // known by the time the frames reply triggers the diagnostic.
        if matches!(&stop.reason, StopReason::Signal { name, .. } if name == "SIGSEGV" || name == "SIGBUS") {
            let id = b.submit(Request::Evaluate { thread, frame: 0, expr: "$_siginfo._sifields._sigfault.si_addr".into() });
            self.awaiting.insert(id, Awaiting::FaultAddr);
        }
        b.submit(Request::Frames { thread });
        b.submit(Request::Threads);
        self.last_stop = Some(stop);
        vec![Change::State(RunState::Stopped)]
    }

    fn on_reply(&mut self, id: ReqId, result: Result<Reply, String>, b: &mut dyn Backend) -> Vec<Change> {
        let awaited = self.awaiting.remove(&id);
        match (result, awaited) {
            (Ok(Reply::Breakpoint { id: bp, .. }), Some(Awaiting::Marker)) => {
                self.markers.insert(bp);
                Vec::new()
            }
            (Ok(Reply::Breakpoint { id: bp, .. }), Some(Awaiting::PanicBreak)) => {
                self.panic_breaks.insert(bp);
                Vec::new()
            }
            (Ok(Reply::Value { value, .. }), Some(Awaiting::FaultAddr)) => {
                // Printed as `(void *) 0x8` or `0x8`.
                self.fault_addr = value.split_whitespace().last().and_then(stoat_mi::parse_u64);
                Vec::new()
            }
            // Core files from some kernels, and non-Linux targets, have no $_siginfo.
            (Err(_), Some(Awaiting::FaultAddr)) => Vec::new(),
            (Err(msg), _) => {
                let names: Vec<String> =
                    self.current_frame().and_then(|f| self.locals.get(&f.level)).into_iter().flatten().map(|v| v.name.clone()).collect();
                vec![Change::Diagnostic(diag::for_error(&msg, &names))]
            }
            (Ok(Reply::Threads { threads, current }), _) => {
                self.threads = threads;
                self.thread = self.thread.or(current);
                vec![Change::Threads]
            }
            (Ok(Reply::Frames { thread, frames }), _) if Some(thread) == self.thread => {
                self.frames = frames;
                let mut out = vec![Change::Frames];
                out.extend(self.select_frame(self.blame_frame(), b));
                if let Some(stop) = &self.last_stop {
                    let panic = matches!(stop.reason, StopReason::Breakpoint { id } if self.panic_breaks.contains(&id));
                    out.push(Change::Diagnostic(diag::for_stop(stop, &self.frames, self.selected, self.fault_addr, panic)));
                }
                out
            }
            (Ok(Reply::Locals { thread, frame, vars }), _) if Some(thread) == self.thread => {
                self.locals.insert(frame, vars);
                vec![Change::Locals { frame }]
            }
            (Ok(Reply::Value { expr, value }), _) => vec![Change::Value { expr, value }],
            _ => Vec::new(),
        }
    }
}
