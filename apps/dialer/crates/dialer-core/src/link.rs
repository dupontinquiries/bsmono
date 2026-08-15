//! The protocol between the UI shell and whichever backend is running.
//!
//! Deliberately a pair of `std::sync::mpsc` channels rather than a trait
//! object: the shell is immediate-mode and must never block, and a channel
//! pair gives it exactly two non-blocking operations (`send`, `try_recv`)
//! with the threading question settled at the seam instead of leaking into
//! egui code. A backend is anything that consumes [`Command`]s and produces
//! [`Event`]s — the loop stays visible in each backend's `run` function.

use std::sync::mpsc::{Receiver, Sender};

use crate::message::Payload;
use crate::types::{CallId, CallInfo, CallState};

/// What the setup wizard applies: enough to write a NetworkManager GSM
/// connection. Not the whole [`crate::providers::Apn`] — the network-id list
/// is a matching concern, not a configuration one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApnSetting {
    pub carrier: String,
    pub apn: String,
    pub username: String,
    pub password: String,
}

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
    /// Configure cellular data for a carrier. The mm backend writes a
    /// NetworkManager GSM connection; the mock pretends. Answered by
    /// `Event::Info` or `Event::Error`.
    ApplyApn(ApnSetting),
    /// Send a message to a peer. `peer` is transport-qualified
    /// ("tel:+1555…" for SMS via the modem, "matrix:@a:x"/"matrix/!room:x"
    /// for Matrix). The backend picks the wire encoding (SMS text vs Matrix
    /// custom event).
    SendMessage { peer: String, payload: Payload },
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
    /// The registered operator's MCC+MNC ("310260"), once known — drives the
    /// wizard's carrier suggestion. May never arrive (no SIM, no registration).
    Operator { mccmnc: String },
    /// A success worth telling the user about (APN applied, account signed
    /// in). The Info/Error pair keeps backends out of the toast business.
    Info(String),
    /// A message arrived from `peer` (already parsed into a payload; SMS
    /// bodies pass through `message::parse_sms`, Matrix custom events through
    /// `Payload::from_json`). `ts` is unix seconds.
    MessageIn { peer: String, payload: Payload, ts: u64 },
    /// A previously-sent message was accepted by the network (SMS delivery
    /// state / Matrix event id). Best-effort; not all transports report it.
    MessageSent { peer: String },
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
