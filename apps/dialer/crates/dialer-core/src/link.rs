//! The protocol between the UI shell and whichever backend is running.
//!
//! Deliberately a pair of `std::sync::mpsc` channels rather than a trait
//! object: the shell is immediate-mode and must never block, and a channel
//! pair gives it exactly two non-blocking operations (`send`, `try_recv`)
//! with the threading question settled at the seam instead of leaking into
//! egui code. A backend is anything that consumes [`Command`]s and produces
//! [`Event`]s — the loop stays visible in each backend's `run` function.

use std::sync::mpsc::{Receiver, Sender};

use crate::types::{CallId, CallInfo, CallState};

/// UI → backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Place a call. The number is passed as entered; backends may normalize.
    Dial(String),
    /// Answer a ringing incoming call.
    Accept(CallId),
    /// End (or reject, if ringing) a call.
    Hangup(CallId),
    /// Send DTMF tones on an active call.
    Dtmf(CallId, String),
    /// Mock backend only: fake an incoming call (demo affordance). The mm
    /// backend ignores it — a real modem's incoming calls come from the tower.
    SimulateIncoming(String),
    /// Stop the backend thread.
    Quit,
}

/// Backend → UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A modem (or the simulator) is present and can place calls.
    ModemUp { label: String },
    /// No usable modem. `detail` says why, verbatim, for the toast.
    ModemDown { detail: String },
    /// A call appeared (either direction).
    CallAdded(CallInfo),
    /// A call changed state.
    CallState { id: CallId, state: CallState },
    /// A call object was removed by the backend/modem.
    CallRemoved { id: CallId },
    /// Something failed; the shell toasts it and moves on.
    Error(String),
}

/// The shell's handle on a running backend.
pub struct Link {
    pub tx: Sender<Command>,
    pub rx: Receiver<Event>,
    /// For status UI: which transport this is ("mock", "modemmanager").
    pub kind: &'static str,
}
