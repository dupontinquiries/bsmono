//! Backend-neutral vocabulary. Nothing here knows about MI.

use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Frame {
    pub level: u32,
    pub addr: u64,
    pub func: Option<String>,
    /// Path as recorded in debug info; may not exist on this machine.
    pub file: Option<PathBuf>,
    pub line: Option<u32>,
    /// Object file the pc belongs to, when there is no source info.
    pub from: Option<String>,
}

impl Frame {
    pub fn has_source(&self) -> bool {
        self.file.as_deref().is_some_and(|f| f.is_file()) && self.line.is_some()
    }

    pub fn location(&self) -> String {
        match (&self.file, self.line, &self.from) {
            (Some(f), Some(l), _) => format!("{}:{l}", f.display()),
            (_, _, Some(from)) => from.clone(),
            _ => format!("{:#x}", self.addr),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub id: u32,
    pub name: Option<String>,
    pub target_id: String,
    pub stopped: bool,
    pub top: Option<Frame>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub ty: Option<String>,
    /// None for aggregates when the backend was asked for simple values only;
    /// evaluate on demand instead of paying for every struct at every stop.
    pub value: Option<String>,
    pub is_arg: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    Breakpoint { id: u32 },
    Watchpoint { id: u32 },
    Signal { name: String, meaning: String },
    StepDone,
    FunctionFinished,
    Exited { code: i32 },
    ExitedSignalled { name: String },
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub reason: StopReason,
    pub thread: Option<u32>,
    pub frame: Option<Frame>,
    /// Stamped by the transport when the record was read, not when the UI got to
    /// it. Frame pacing is computed from these.
    pub at: Instant,
}

impl Stop {
    pub fn is_exit(&self) -> bool {
        matches!(self.reason, StopReason::Exited { .. } | StopReason::ExitedSignalled { .. })
    }

    pub fn is_crash(&self) -> bool {
        matches!(&self.reason, StopReason::Signal { name, .. }
            if matches!(name.as_str(), "SIGSEGV" | "SIGBUS" | "SIGABRT" | "SIGILL" | "SIGFPE" | "SIGTRAP" | "SIGSYS"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunState {
    #[default]
    NotStarted,
    Running,
    Stopped,
    Exited,
}
