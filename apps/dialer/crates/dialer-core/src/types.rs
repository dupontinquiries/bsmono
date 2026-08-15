//! Call state types. The discriminants are ModemManager's own
//! (`MMCallState` / `MMCallDirection` in `ModemManager-enums.h`), so the mm
//! backend converts with a match on the raw integer and nothing can drift
//! silently — an unknown value degrades to `Unknown` rather than panicking,
//! because MM does grow states across releases.

use serde::{Deserialize, Serialize};

/// A call's lifecycle state (== `MMCallState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallState {
    Unknown = 0,
    /// Outgoing, not yet ringing at the far end.
    Dialing = 1,
    /// Outgoing, ringing at the far end.
    RingingOut = 2,
    /// Incoming, ringing here.
    RingingIn = 3,
    Active = 4,
    Held = 5,
    /// Incoming while another call is active (call waiting).
    Waiting = 6,
    Terminated = 7,
}

impl CallState {
    pub fn from_mm(v: i32) -> CallState {
        match v {
            1 => CallState::Dialing,
            2 => CallState::RingingOut,
            3 => CallState::RingingIn,
            4 => CallState::Active,
            5 => CallState::Held,
            6 => CallState::Waiting,
            7 => CallState::Terminated,
            _ => CallState::Unknown,
        }
    }

    /// The call still exists from the user's point of view.
    pub fn is_live(self) -> bool {
        !matches!(self, CallState::Terminated | CallState::Unknown)
    }

    /// Ringing on our side — the accept/reject affordance applies.
    pub fn is_ringing_in(self) -> bool {
        matches!(self, CallState::RingingIn | CallState::Waiting)
    }

    /// Human label for the in-call screen.
    pub fn label(self) -> &'static str {
        match self {
            CallState::Unknown => "…",
            CallState::Dialing => "Dialing…",
            CallState::RingingOut => "Ringing…",
            CallState::RingingIn => "Incoming call",
            CallState::Active => "In call",
            CallState::Held => "On hold",
            CallState::Waiting => "Call waiting",
            CallState::Terminated => "Call ended",
        }
    }
}

/// Who initiated (== `MMCallDirection`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallDirection {
    Unknown = 0,
    Incoming = 1,
    Outgoing = 2,
}

impl CallDirection {
    pub fn from_mm(v: i32) -> CallDirection {
        match v {
            1 => CallDirection::Incoming,
            2 => CallDirection::Outgoing,
            _ => CallDirection::Unknown,
        }
    }
}

/// A stable handle for one call. For the mm backend it is the call's D-Bus
/// object path; for the mock it is a counter. Strings, because object paths
/// are the identity MM itself uses and inventing a parallel integer id would
/// just mean keeping a map in sync.
pub type CallId = String;

/// Everything the session needs to know about one call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallInfo {
    pub id: CallId,
    pub number: String,
    pub direction: CallDirection,
    pub state: CallState,
}
