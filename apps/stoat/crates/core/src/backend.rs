//! The seam between the session and whatever actually controls the process.
//!
//! This exists because the backend really does vary for the intended targets:
//! local gdb on the device, gdb talking to gdbserver on a phone, core files, and
//! possibly a DAP adapter or a native RSP engine later. The shape is the
//! session's needs, not MI's or DAP's.
//!
//! Requests are fire-and-forget with a correlation id; everything comes back as
//! an `Event` on one channel. No async fn and no generics on the trait, so it
//! stays dyn-compatible and a shell can hold a `Box<dyn Backend>`.

use crate::model::*;
use std::path::PathBuf;

pub type ReqId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Over,
    Into,
    Out,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Load { program: PathBuf, args: Vec<String>, env: Vec<(String, String)>, cwd: Option<PathBuf> },
    LoadCore { program: PathBuf, core: PathBuf },
    Attach { pid: u32 },
    Run,
    Continue,
    Interrupt,
    Step { kind: StepKind, thread: Option<u32> },
    Threads,
    Frames { thread: u32 },
    Locals { thread: u32, frame: u32 },
    Evaluate { thread: u32, frame: u32, expr: String },
    InsertBreakpoint { location: String },
    DeleteBreakpoint { id: u32 },
    /// A line for the backend's own command language. The escape hatch that keeps
    /// the tool steerable: anything the UI does not model is still reachable.
    Raw(String),
    TargetInput(String),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Ok,
    Threads { threads: Vec<Thread>, current: Option<u32> },
    Frames { thread: u32, frames: Vec<Frame> },
    Locals { thread: u32, frame: u32, vars: Vec<Variable> },
    Value { expr: String, value: String },
    Breakpoint { id: u32, pending: bool, locations: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// The debugger talking.
    Console,
    /// The debugger's diagnostics.
    Log,
    /// The debuggee talking.
    Target,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    ToBackend,
    FromBackend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Reply { id: ReqId, result: Result<Reply, String> },
    Started { pid: Option<u32> },
    Running,
    Stopped(Stop),
    Output { channel: Channel, text: String },
    /// Every line exchanged with the backend, verbatim. Shells can ignore it or
    /// show it; it is how the loop stays inspectable.
    Wire { dir: Dir, line: String },
    BackendGone { detail: String },
}

pub trait Backend {
    fn submit(&mut self, req: Request) -> ReqId;
}
