//! The call session model — the part with decisions in it.
//!
//! [`Session`] consumes [`Event`]s and answers the shell's questions
//! ("is anything ringing?", "what do I show full-screen?"). It also owns the
//! one judgement backends must not make: **what a finished call was**. A
//! terminated call is
//!
//! * `Completed` if it was ever active (talk time is active-to-end);
//! * `Rejected` if it was incoming, never active, and *we* hung up;
//! * `Missed` if it was incoming, never active, and the far end / network
//!   ended it;
//! * `NoAnswer` if it was outgoing and never became active.
//!
//! Time comes in as unix seconds from the caller so every rule here is
//! testable without sleeping.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::history::HistoryEntry;
use crate::link::Event;
use crate::types::{CallDirection, CallId, CallInfo, CallState};

/// How a call ended, for the history list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Completed,
    Missed,
    Rejected,
    NoAnswer,
}

#[derive(Debug, Clone)]
struct Tracked {
    info: CallInfo,
    /// When the call appeared (unix secs).
    started: u64,
    /// When it went Active, if ever.
    active_at: Option<u64>,
    /// The user pressed hang-up while it was ringing in.
    we_rejected: bool,
}

/// Connection status, mirrored from ModemUp/ModemDown for the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModemStatus {
    Probing,
    Up(String),
    Down(String),
}

/// All live calls plus the pending history entries the shell should persist.
pub struct Session {
    calls: BTreeMap<CallId, Tracked>,
    /// Finished calls, oldest first. The shell drains these into the store.
    pending_history: Vec<HistoryEntry>,
    pub modem: ModemStatus,
}

impl Default for Session {
    fn default() -> Self {
        Session { calls: BTreeMap::new(), pending_history: Vec::new(), modem: ModemStatus::Probing }
    }
}

impl Session {
    /// Feed one backend event. `now` is unix seconds.
    pub fn apply(&mut self, ev: Event, now: u64) {
        match ev {
            Event::ModemUp { label } => self.modem = ModemStatus::Up(label),
            Event::ModemDown { detail } => self.modem = ModemStatus::Down(detail),
            Event::CallAdded(info) => {
                let mut t = Tracked { info, started: now, active_at: None, we_rejected: false };
                if t.info.state == CallState::Active {
                    // Backends that only poll can first see a call already active.
                    t.active_at = Some(now);
                }
                self.calls.insert(t.info.id.clone(), t);
            }
            Event::CallState { id, state } => {
                let finished = {
                    let Some(t) = self.calls.get_mut(&id) else { return };
                    t.info.state = state;
                    if state == CallState::Active && t.active_at.is_none() {
                        t.active_at = Some(now);
                    }
                    state == CallState::Terminated
                };
                if finished {
                    self.finish(&id, now);
                }
            }
            Event::CallRemoved { id } => {
                // Removal without a Terminated first still ends the call.
                if self.calls.contains_key(&id) {
                    self.finish(&id, now);
                }
            }
            Event::Error(_) => {} // surfaced by the shell, not state
        }
    }

    /// The shell tells us the user hung up a call; if it is still ringing in,
    /// that makes an eventual termination a rejection rather than a miss.
    pub fn note_user_hangup(&mut self, id: &str) {
        if let Some(t) = self.calls.get_mut(id) {
            if t.info.state.is_ringing_in() {
                t.we_rejected = true;
            }
        }
    }

    fn finish(&mut self, id: &str, now: u64) {
        let Some(t) = self.calls.remove(id) else { return };
        let outcome = match (t.active_at, t.info.direction, t.we_rejected) {
            (Some(_), _, _) => Outcome::Completed,
            (None, CallDirection::Incoming, true) => Outcome::Rejected,
            (None, CallDirection::Incoming, false) => Outcome::Missed,
            (None, _, _) => Outcome::NoAnswer,
        };
        let duration = t.active_at.map(|a| now.saturating_sub(a)).unwrap_or(0);
        self.pending_history.push(HistoryEntry {
            number: t.info.number,
            direction: t.info.direction,
            started: t.started,
            duration_secs: duration,
            outcome,
        });
    }

    /// Drain history entries produced since the last drain.
    pub fn take_history(&mut self) -> Vec<HistoryEntry> {
        std::mem::take(&mut self.pending_history)
    }

    /// Live calls, id-ordered (stable across frames).
    pub fn calls(&self) -> impl Iterator<Item = &CallInfo> {
        self.calls.values().map(|t| &t.info)
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// The call the in-call screen should foreground: a ringing-in call wins
    /// (it needs an answer), otherwise the first live call.
    pub fn foreground(&self) -> Option<&CallInfo> {
        self.calls
            .values()
            .map(|t| &t.info)
            .find(|c| c.state.is_ringing_in())
            .or_else(|| self.calls.values().map(|t| &t.info).next())
    }

    /// Seconds of talk time so far for a call, for the ticking mm:ss.
    pub fn talk_secs(&self, id: &str, now: u64) -> Option<u64> {
        self.calls.get(id).and_then(|t| t.active_at).map(|a| now.saturating_sub(a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn added(id: &str, number: &str, dir: CallDirection, state: CallState) -> Event {
        Event::CallAdded(CallInfo { id: id.into(), number: number.into(), direction: dir, state })
    }

    #[test]
    fn outgoing_answered_call_completes_with_talk_time_only() {
        let mut s = Session::default();
        s.apply(added("c1", "+15551234", CallDirection::Outgoing, CallState::Dialing), 100);
        s.apply(Event::CallState { id: "c1".into(), state: CallState::RingingOut }, 101);
        s.apply(Event::CallState { id: "c1".into(), state: CallState::Active }, 110);
        assert_eq!(s.talk_secs("c1", 115), Some(5));
        s.apply(Event::CallState { id: "c1".into(), state: CallState::Terminated }, 170);
        let h = s.take_history();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].outcome, Outcome::Completed);
        // Duration is active→end (60s), NOT dial→end (70s): ring time is not talk time.
        assert_eq!(h[0].duration_secs, 60);
        assert!(s.is_empty());
    }

    #[test]
    fn ignored_incoming_is_missed_but_our_hangup_is_rejected() {
        let mut s = Session::default();
        s.apply(added("a", "+1", CallDirection::Incoming, CallState::RingingIn), 10);
        s.apply(Event::CallState { id: "a".into(), state: CallState::Terminated }, 40);
        assert_eq!(s.take_history()[0].outcome, Outcome::Missed);

        s.apply(added("b", "+2", CallDirection::Incoming, CallState::RingingIn), 50);
        s.note_user_hangup("b");
        s.apply(Event::CallState { id: "b".into(), state: CallState::Terminated }, 55);
        assert_eq!(s.take_history()[0].outcome, Outcome::Rejected);
    }

    #[test]
    fn unanswered_outgoing_is_no_answer_and_removal_counts_as_end() {
        let mut s = Session::default();
        s.apply(added("c", "+3", CallDirection::Outgoing, CallState::RingingOut), 0);
        // Backend removed the call object without a Terminated transition.
        s.apply(Event::CallRemoved { id: "c".into() }, 30);
        let h = s.take_history();
        assert_eq!(h[0].outcome, Outcome::NoAnswer);
        assert_eq!(h[0].duration_secs, 0);
    }

    #[test]
    fn ringing_in_call_wins_the_foreground_over_an_active_one() {
        let mut s = Session::default();
        s.apply(added("active", "+4", CallDirection::Outgoing, CallState::Active), 0);
        s.apply(added("ringing", "+5", CallDirection::Incoming, CallState::Waiting), 5);
        assert_eq!(s.foreground().unwrap().id, "ringing");
    }

    #[test]
    fn call_first_seen_already_active_still_gets_a_duration() {
        // A polling backend can miss the ringing phase entirely.
        let mut s = Session::default();
        s.apply(added("c", "+6", CallDirection::Incoming, CallState::Active), 100);
        s.apply(Event::CallState { id: "c".into(), state: CallState::Terminated }, 160);
        let h = s.take_history();
        assert_eq!(h[0].outcome, Outcome::Completed);
        assert_eq!(h[0].duration_secs, 60);
    }
}
