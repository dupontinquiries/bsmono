//! conversation — the conversation detail view and the landmark dialog.
//!
//! `ConversationDetailDialog` is where a conversation actually happens: the snap
//! timeline, voice, and adding people. Its signals name the surface —
//! `snapTapped`, `sendVoicePayload`, `addPeopleRequested`, `sendSnapRequested`
//! — and `CallParticipant` says a conversation can carry a live call.
//!
//! ## The viewed-snap rule
//!
//! A snap has a timer, and tapping it starts that timer. The rule that matters:
//! **a snap is consumed once**. Tapping an already-viewed snap must not restart
//! it, or the whole premise fails — and the check has to be on the snap's own
//! state rather than on the view's, because a conversation reopened from a
//! notification builds a fresh view over the same snaps.
//!
//! ## Voice
//!
//! `sendVoicePayload(recipient, payload)`. Held to record, released to send,
//! and a release below the floor sends nothing: an accidental brush of the
//! button that ships 80ms of silence to someone is worse than a missed message,
//! because they have to open it to find out it was nothing.

/// One entry in the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub from: String,
    pub at: i64,
    pub kind: EntryKind,
    /// Set once the recipient has opened it.
    pub viewed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntryKind {
    Snap { duration_s: u32 },
    Voice { seconds: f32 },
    Text { body: String },
    /// Someone joined or left, shown inline so the conversation explains itself.
    System { note: String },
}

impl Entry {
    /// Can this be opened? A system note is not tappable, and a snap already
    /// viewed is spent.
    pub fn is_openable(&self) -> bool {
        matches!(self.kind, EntryKind::Snap { .. } | EntryKind::Voice { .. }) && !self.viewed
    }

    /// Does this entry disappear after viewing? Text and system notes stay;
    /// a snap with a zero timer stays too — zero means unlimited, the same rule
    /// the composer encodes.
    pub fn ephemeral(&self) -> bool {
        matches!(self.kind, EntryKind::Snap { duration_s } if duration_s > 0)
    }
}

/// Someone on a live call in this conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct CallParticipant {
    pub id: String,
    pub name: String,
    pub speaking: bool,
}

/// Minimum voice message length. Below this a release sends nothing — an
/// accidental brush that ships 80ms of silence is worse than a missed message,
/// because the recipient has to open it to find out it was nothing.
pub const MIN_VOICE_SECONDS: f32 = 0.4;

#[derive(Debug, Clone, Default)]
pub struct Conversation {
    pub peer: String,
    pub entries: Vec<Entry>,
    pub call: Vec<CallParticipant>,
    voice_held: Option<f32>,
}

/// What the view asks the owner to do.
#[derive(Debug, Clone, PartialEq)]
pub enum ConversationAction {
    OpenSnap(String),
    SendVoice { recipient: String, seconds: f32 },
    AddPeople,
    SendSnap,
}

impl Conversation {
    /// Timeline, oldest first, with ties on id so it cannot shuffle.
    pub fn timeline(&self) -> Vec<&Entry> {
        let mut v: Vec<&Entry> = self.entries.iter().collect();
        v.sort_by(|a, b| a.at.cmp(&b.at).then(a.id.cmp(&b.id)));
        v
    }

    /// Unopened entries addressed to us. Drives the badge, and counts entries
    /// rather than conversations because "3 unread" meaning three conversations
    /// is a different and less useful number.
    pub fn unopened(&self) -> usize {
        self.entries.iter().filter(|e| e.is_openable()).count()
    }

    /// `snapTapped`. Returns the action, or `None` when there is nothing to
    /// open — a spent snap must not restart, or the whole premise fails.
    pub fn tap(&mut self, id: &str) -> Option<ConversationAction> {
        let e = self.entries.iter_mut().find(|e| e.id == id)?;
        if !e.is_openable() { return None; }
        e.viewed = true;
        Some(ConversationAction::OpenSnap(id.to_string()))
    }

    pub fn hold_voice(&mut self) { self.voice_held = Some(0.0); }

    pub fn tick_voice(&mut self, dt: f32) {
        if let Some(t) = self.voice_held.as_mut() { *t += dt.max(0.0); }
    }

    pub fn voice_seconds(&self) -> f32 { self.voice_held.unwrap_or(0.0) }
    pub fn recording(&self) -> bool { self.voice_held.is_some() }

    /// Release. Sends only if the message is long enough to be a message.
    pub fn release_voice(&mut self) -> Option<ConversationAction> {
        let held = self.voice_held.take()?;
        if held < MIN_VOICE_SECONDS { return None; }
        Some(ConversationAction::SendVoice { recipient: self.peer.clone(), seconds: held })
    }

    /// Abandon a recording without sending — for a call arriving mid-hold.
    pub fn cancel_voice(&mut self) { self.voice_held = None; }

    pub fn on_call(&self) -> bool { !self.call.is_empty() }

    /// Who to show as the active speaker. `None` in a silent call, because
    /// highlighting an arbitrary participant implies they are talking.
    pub fn speaker(&self) -> Option<&CallParticipant> {
        self.call.iter().find(|p| p.speaking)
    }
}

// ── landmark ────────────────────────────────────────────────────────────────

/// A place saved from a captured frame. `saved(Landmark)` in the header.
#[derive(Debug, Clone, PartialEq)]
pub struct Landmark {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub has_frame: bool,
}

#[derive(Debug, Clone, Default)]
pub struct AddLandmark {
    pub open: bool,
    pub name: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub has_frame: bool,
}

impl AddLandmark {
    pub fn open(&mut self, lat: Option<f64>, lon: Option<f64>) {
        self.open = true;
        self.name.clear();
        self.lat = lat;
        self.lon = lon;
        self.has_frame = false;
    }

    pub fn close(&mut self) { self.open = false; }

    /// `setCapturedFrame`. A landmark without a picture is still a landmark, so
    /// this is not required — only recorded.
    pub fn set_frame(&mut self, present: bool) { self.has_frame = present; }

    pub fn problems(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.name.trim().is_empty() { v.push("a landmark needs a name".into()); }
        match (self.lat, self.lon) {
            (Some(la), Some(lo)) => {
                // Out-of-range coordinates put the pin somewhere impossible and
                // the map silently shows nothing, which reads as a broken map.
                if !(-90.0..=90.0).contains(&la) { v.push("latitude is out of range".into()); }
                if !(-180.0..=180.0).contains(&lo) { v.push("longitude is out of range".into()); }
            }
            _ => v.push("no location fix yet".into()),
        }
        v
    }

    pub fn can_save(&self) -> bool { self.problems().is_empty() }

    pub fn save(&self) -> Option<Landmark> {
        if !self.can_save() { return None; }
        Some(Landmark { name: self.name.trim().to_string(),
                        lat: self.lat?, lon: self.lon?, has_frame: self.has_frame })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(id: &str, at: i64, viewed: bool) -> Entry {
        Entry { id: id.into(), from: "ada".into(), at, viewed,
                kind: EntryKind::Snap { duration_s: 5 } }
    }

    fn conv() -> Conversation {
        Conversation { peer: "ada".into(),
                       entries: vec![snap("s1", 100, false), snap("s2", 200, false)],
                       ..Default::default() }
    }

    #[test]
    fn a_snap_is_consumed_once() {
        // Tapping a spent snap must not restart it, or the whole premise fails.
        let mut c = conv();
        assert_eq!(c.tap("s1"), Some(ConversationAction::OpenSnap("s1".into())));
        assert_eq!(c.tap("s1"), None, "already viewed");
        assert_eq!(c.unopened(), 1);
    }

    #[test]
    fn a_zero_timer_snap_is_not_ephemeral() {
        // Zero means unlimited, the same rule the composer encodes — treating it
        // as "gone instantly" is the inversion that leaks a photo.
        let unlimited = Entry { kind: EntryKind::Snap { duration_s: 0 }, ..snap("s", 1, false) };
        assert!(!unlimited.ephemeral());
        assert!(snap("s", 1, false).ephemeral());
    }

    #[test]
    fn system_notes_are_not_tappable() {
        let e = Entry { kind: EntryKind::System { note: "bo joined".into() }, ..snap("s", 1, false) };
        assert!(!e.is_openable());
        let t = Entry { kind: EntryKind::Text { body: "hi".into() }, ..snap("s", 1, false) };
        assert!(!t.is_openable(), "text does not need opening");
    }

    #[test]
    fn the_timeline_is_oldest_first_and_does_not_shuffle() {
        let c = conv();
        let a: Vec<&str> = c.timeline().iter().map(|e| e.id.as_str()).collect();
        let b: Vec<&str> = c.timeline().iter().map(|e| e.id.as_str()).collect();
        assert_eq!(a, vec!["s1", "s2"]);
        assert_eq!(a, b);
    }

    #[test]
    fn a_brush_of_the_voice_button_sends_nothing() {
        // 80ms of silence is worse than a missed message: the recipient has to
        // open it to find out it was nothing.
        let mut c = conv();
        c.hold_voice();
        c.tick_voice(0.1);
        assert!(c.recording());
        assert_eq!(c.release_voice(), None);
        assert!(!c.recording());
    }

    #[test]
    fn a_real_voice_message_sends_to_the_peer() {
        let mut c = conv();
        c.hold_voice();
        c.tick_voice(1.5);
        match c.release_voice() {
            Some(ConversationAction::SendVoice { recipient, seconds }) => {
                assert_eq!(recipient, "ada");
                assert!((seconds - 1.5).abs() < 1e-6);
            }
            other => panic!("expected a voice send, got {other:?}"),
        }
    }

    #[test]
    fn a_cancelled_recording_sends_nothing() {
        // A call arriving mid-hold must not ship the half-message.
        let mut c = conv();
        c.hold_voice();
        c.tick_voice(3.0);
        c.cancel_voice();
        assert_eq!(c.release_voice(), None);
    }

    #[test]
    fn a_silent_call_has_no_active_speaker() {
        // Highlighting an arbitrary participant implies they are talking.
        let mut c = conv();
        c.call = vec![CallParticipant { id: "a".into(), name: "Ada".into(), speaking: false },
                      CallParticipant { id: "b".into(), name: "Bo".into(), speaking: false }];
        assert!(c.on_call());
        assert!(c.speaker().is_none());
        c.call[1].speaking = true;
        assert_eq!(c.speaker().unwrap().id, "b");
    }

    #[test]
    fn a_landmark_needs_a_name_and_a_fix() {
        let mut d = AddLandmark::default();
        d.open(None, None);
        assert!(!d.can_save());
        assert!(d.problems().iter().any(|p| p.contains("name")));
        assert!(d.problems().iter().any(|p| p.contains("location")));
        d.name = "Pier".into();
        d.lat = Some(55.6); d.lon = Some(12.6);
        assert!(d.can_save());
        assert_eq!(d.save().unwrap().name, "Pier");
    }

    #[test]
    fn impossible_coordinates_are_refused() {
        // The map silently shows nothing for an out-of-range pin, which reads as
        // a broken map rather than bad data.
        let mut d = AddLandmark::default();
        d.open(Some(999.0), Some(12.0));
        d.name = "Nowhere".into();
        assert!(!d.can_save());
        assert!(d.problems().iter().any(|p| p.contains("latitude")));
    }

    #[test]
    fn a_landmark_without_a_picture_is_still_a_landmark() {
        let mut d = AddLandmark::default();
        d.open(Some(55.6), Some(12.6));
        d.name = "Pier".into();
        assert!(d.can_save(), "no frame required");
        assert!(!d.save().unwrap().has_frame);
        d.set_frame(true);
        assert!(d.save().unwrap().has_frame);
    }

    #[test]
    fn reopening_the_landmark_dialog_starts_clean() {
        let mut d = AddLandmark::default();
        d.open(Some(1.0), Some(2.0));
        d.name = "One".into();
        d.set_frame(true);
        d.open(Some(3.0), Some(4.0));
        assert!(d.name.is_empty());
        assert!(!d.has_frame, "the previous frame is not carried over");
    }
}
