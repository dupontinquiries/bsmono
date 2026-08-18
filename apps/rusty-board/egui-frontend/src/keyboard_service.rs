//! keyboard_service — `io.rustyboard.Keyboard1`, ported as a state model.
//!
//! How other applications ask for the keyboard: `Show`, `Hide`, a `Visible`
//! property, `StartVoice`, and a `TextCommitted` signal going the other way.
//!
//! ## The transport stays with the host, the decisions do not
//!
//! `src/ipc/dbus.rs` is 61 lines of zbus glue around four one-line handlers
//! that flip a bool and log. The interface shape is the valuable part and the
//! zbus attributes are not portable to a frontend that may be launched without
//! a session bus at all, so this is the model: the host runs the connection and
//! calls in, and the answers to "may this show", "what happens if two apps ask"
//! and "who is allowed to hide it" live here where they can be tested.
//!
//! The Qt-side handlers do not answer those questions. `show()` sets
//! `visible = true` unconditionally. That is fine for a stub and wrong for a
//! keyboard: it means any app on the bus can cover a third of the screen at any
//! moment, including while a different app is mid-gesture.
//!
//! ## Requests are refused, not queued
//!
//! A show request that cannot be honoured now returns false rather than waiting
//! for a moment when it can. A keyboard that appeared two seconds after the
//! field was tapped — over whatever the user moved on to — is worse than one
//! that never appeared, because the second is a bug the app can retry and the
//! first is the keyboard acting on stale intent.
//!
//! ## Voice input does not raise the keyboard
//!
//! `StartVoice` is a separate method from `Show`, and this port keeps them
//! separate rather than having one imply the other. It is the same rule glove
//! mode needed from the other direction: a hand that cannot press keys must not
//! be shown keys, and a voice request that raised a keyboard would cover the
//! screen with the thing the caller was explicitly avoiding.

/// Why a request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Another client has it open and has not released it.
    HeldByOther,
    /// The keyboard is suppressed — glove mode, a voice session, a setting.
    Suppressed,
    /// Hide from a client that never showed it.
    NotOwner,
}

/// The outcome of a request.
pub type Reply = Result<(), Refused>;

/// Who asked. The bus name in production; any stable string in a test.
pub type ClientId = String;

/// What the service should tell the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceEvent {
    Shown,
    Hidden,
    /// Start voice capture WITHOUT raising the keyboard.
    VoiceRequested,
    /// The keyboard committed text; the host emits `TextCommitted`.
    TextCommitted(String),
}

/// The keyboard's request model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyboardService {
    visible: bool,
    /// Which client asked for the current showing.
    owner: Option<ClientId>,
    /// Set while something else owns the screen — glove mode, a voice session.
    suppressed: bool,
    /// Text the host has yet to emit as a signal.
    pending: Vec<String>,
}

impl KeyboardService {
    pub fn visible(&self) -> bool { self.visible }
    pub fn owner(&self) -> Option<&str> { self.owner.as_deref() }
    pub fn suppressed(&self) -> bool { self.suppressed }

    /// Suppress or release the keyboard.
    ///
    /// Suppressing HIDES it if it is up. A keyboard left on screen after the
    /// reason for suppressing it arrived — glove mode starting, a voice session
    /// opening — is the failure the suppression exists to prevent, arriving one
    /// state later.
    pub fn set_suppressed(&mut self, s: bool) -> Option<ServiceEvent> {
        self.suppressed = s;
        if s && self.visible {
            self.visible = false;
            self.owner = None;
            return Some(ServiceEvent::Hidden);
        }
        None
    }

    /// `Show()`.
    pub fn show(&mut self, client: &str) -> Reply {
        if self.suppressed { return Err(Refused::Suppressed); }
        match &self.owner {
            // Already ours: idempotent rather than an error. An app that shows
            // the keyboard on every focus event is normal, not misbehaving.
            Some(o) if o == client => Ok(()),
            Some(_) => Err(Refused::HeldByOther),
            None => {
                self.visible = true;
                self.owner = Some(client.to_string());
                Ok(())
            }
        }
    }

    /// `Hide()`.
    ///
    /// Only the owner may hide it. Otherwise a background app can dismiss the
    /// keyboard out from under whoever is typing, and the user loses a
    /// half-written message to something they cannot see.
    pub fn hide(&mut self, client: &str) -> Reply {
        match &self.owner {
            Some(o) if o == client => {
                self.visible = false;
                self.owner = None;
                Ok(())
            }
            Some(_) => Err(Refused::NotOwner),
            // Hiding an already-hidden keyboard succeeds: the caller wanted it
            // gone and it is gone.
            None => Ok(()),
        }
    }

    /// A client disconnected from the bus.
    ///
    /// Releases the keyboard if it held it. Without this, an app that crashes
    /// while the keyboard is up locks it open for every other client until the
    /// service restarts.
    pub fn client_gone(&mut self, client: &str) -> Option<ServiceEvent> {
        if self.owner.as_deref() == Some(client) {
            self.owner = None;
            if self.visible {
                self.visible = false;
                return Some(ServiceEvent::Hidden);
            }
        }
        None
    }

    /// `StartVoice()`.
    ///
    /// Does NOT raise the keyboard. Same rule glove mode needed from the other
    /// side: a caller asking for voice is explicitly avoiding the keys.
    pub fn start_voice(&mut self) -> Reply {
        if self.suppressed { return Err(Refused::Suppressed); }
        Ok(())
    }

    /// The keyboard committed text. Queued for the host to signal.
    ///
    /// Empty commits are dropped: a keyboard that fires `TextCommitted("")` on
    /// every backspace makes every listener filter it, and one of them will
    /// forget.
    pub fn commit_text(&mut self, text: &str) -> Option<ServiceEvent> {
        if text.is_empty() { return None; }
        self.pending.push(text.to_string());
        Some(ServiceEvent::TextCommitted(text.to_string()))
    }

    /// Drain the queue for emission.
    pub fn take_pending(&mut self) -> Vec<String> { std::mem::take(&mut self.pending) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn showing_makes_it_visible_and_records_the_owner() {
        let mut k = KeyboardService::default();
        assert_eq!(k.show("app.a"), Ok(()));
        assert!(k.visible());
        assert_eq!(k.owner(), Some("app.a"));
    }

    #[test]
    fn showing_twice_from_the_same_client_is_idempotent() {
        // An app that shows the keyboard on every focus event is normal, not
        // misbehaving.
        let mut k = KeyboardService::default();
        k.show("app.a").unwrap();
        assert_eq!(k.show("app.a"), Ok(()));
        assert!(k.visible());
    }

    #[test]
    fn a_second_client_cannot_take_the_keyboard() {
        // The Qt handler sets visible = true unconditionally, which means any
        // app on the bus can cover a third of the screen at any moment —
        // including while another app is mid-gesture.
        let mut k = KeyboardService::default();
        k.show("app.a").unwrap();
        assert_eq!(k.show("app.b"), Err(Refused::HeldByOther));
        assert_eq!(k.owner(), Some("app.a"));
    }

    #[test]
    fn only_the_owner_may_hide_it() {
        // Otherwise a background app dismisses the keyboard out from under
        // whoever is typing, and a half-written message goes with it.
        let mut k = KeyboardService::default();
        k.show("app.a").unwrap();
        assert_eq!(k.hide("app.b"), Err(Refused::NotOwner));
        assert!(k.visible());
        assert_eq!(k.hide("app.a"), Ok(()));
        assert!(!k.visible());
    }

    #[test]
    fn hiding_an_already_hidden_keyboard_succeeds() {
        // The caller wanted it gone and it is gone. Returning an error here
        // would make every dismiss path handle a failure that is not one.
        let mut k = KeyboardService::default();
        assert_eq!(k.hide("app.a"), Ok(()));
    }

    #[test]
    fn a_crashed_client_releases_the_keyboard() {
        // Without this, an app that dies while the keyboard is up locks it open
        // for every other client until the service restarts.
        let mut k = KeyboardService::default();
        k.show("app.a").unwrap();
        assert_eq!(k.client_gone("app.a"), Some(ServiceEvent::Hidden));
        assert!(!k.visible());
        assert_eq!(k.show("app.b"), Ok(()), "and someone else can have it");
    }

    #[test]
    fn another_clients_disconnect_changes_nothing() {
        let mut k = KeyboardService::default();
        k.show("app.a").unwrap();
        assert_eq!(k.client_gone("app.b"), None);
        assert!(k.visible());
        assert_eq!(k.owner(), Some("app.a"));
    }

    #[test]
    fn suppression_refuses_new_requests() {
        // Glove mode, a voice session, a setting.
        let mut k = KeyboardService::default();
        k.set_suppressed(true);
        assert_eq!(k.show("app.a"), Err(Refused::Suppressed));
        assert!(!k.visible());
    }

    #[test]
    fn suppression_also_hides_a_keyboard_that_is_already_up() {
        // A keyboard left on screen after the reason for suppressing it arrived
        // is the exact failure suppression exists to prevent, one state later.
        let mut k = KeyboardService::default();
        k.show("app.a").unwrap();
        assert_eq!(k.set_suppressed(true), Some(ServiceEvent::Hidden));
        assert!(!k.visible());
        assert_eq!(k.owner(), None);
    }

    #[test]
    fn releasing_suppression_does_not_bring_it_back() {
        // The moment has passed. Re-raising a keyboard the user has stopped
        // looking at is the stale-intent failure in a different costume; the
        // app must ask again.
        let mut k = KeyboardService::default();
        k.show("app.a").unwrap();
        k.set_suppressed(true);
        assert_eq!(k.set_suppressed(false), None);
        assert!(!k.visible());
    }

    #[test]
    fn voice_input_does_not_raise_the_keyboard() {
        // The same rule glove mode needed from the other direction: a caller
        // asking for voice is explicitly avoiding the keys, and covering the
        // screen with them is the thing they were trying not to do.
        let mut k = KeyboardService::default();
        assert_eq!(k.start_voice(), Ok(()));
        assert!(!k.visible());
        assert_eq!(k.owner(), None);
    }

    #[test]
    fn voice_is_refused_while_suppressed() {
        let mut k = KeyboardService::default();
        k.set_suppressed(true);
        assert_eq!(k.start_voice(), Err(Refused::Suppressed));
    }

    #[test]
    fn committed_text_is_queued_for_the_signal() {
        let mut k = KeyboardService::default();
        assert_eq!(k.commit_text("hello"),
                   Some(ServiceEvent::TextCommitted("hello".into())));
        assert_eq!(k.take_pending(), vec!["hello".to_string()]);
        assert!(k.take_pending().is_empty(), "draining empties it");
    }

    #[test]
    fn an_empty_commit_is_dropped_rather_than_signalled() {
        // A keyboard that fires TextCommitted("") on every backspace makes
        // every listener filter it, and one of them will forget.
        let mut k = KeyboardService::default();
        assert_eq!(k.commit_text(""), None);
        assert!(k.take_pending().is_empty());
    }

    #[test]
    fn text_can_be_committed_without_the_keyboard_being_visible() {
        // Voice input commits text with no keys on screen, which is the whole
        // point of StartVoice being a separate method.
        let mut k = KeyboardService::default();
        assert!(k.commit_text("spoken").is_some());
        assert!(!k.visible());
    }
}

/// Reasons the keyboard should stand down, from elsewhere in the fleet.
///
/// A single enum rather than a bool so the service can say WHY it refused, and
/// so a caller cannot clear someone else's suppression by setting a shared
/// flag false. Two systems both wanting the keyboard down is normal — a voice
/// session inside glove mode is the obvious case — and the last one to leave
/// should be the one that releases it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suppressor {
    /// Cold-weather glove mode: the screen cannot be touched, so keys are the
    /// one thing that cannot help.
    Glove,
    /// A voice session is capturing; keys would cover the field being dictated
    /// into.
    Voice,
    /// The user turned the on-screen keyboard off.
    Setting,
}

/// Tracks who is asking for suppression.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Suppressors {
    active: Vec<Suppressor>,
}

impl Suppressors {
    pub fn set(&mut self, s: Suppressor, on: bool) {
        let had = self.active.contains(&s);
        if on && !had { self.active.push(s); }
        if !on { self.active.retain(|x| *x != s); }
    }

    pub fn any(&self) -> bool { !self.active.is_empty() }
    pub fn contains(&self, s: Suppressor) -> bool { self.active.contains(&s) }

    /// Apply to a service. Returns the event, if the change hid the keyboard.
    pub fn apply(&self, k: &mut KeyboardService) -> Option<ServiceEvent> {
        k.set_suppressed(self.any())
    }
}

#[cfg(test)]
mod suppressor_tests {
    use super::*;

    #[test]
    fn glove_mode_suppresses_the_keyboard() {
        // The rule glove mode needed and could only state from its own side:
        // a hand that cannot press keys must not be shown keys.
        let (mut s, mut k) = (Suppressors::default(), KeyboardService::default());
        s.set(Suppressor::Glove, true);
        s.apply(&mut k);
        assert_eq!(k.show("app.a"), Err(Refused::Suppressed));
    }

    #[test]
    fn the_last_reason_to_leave_releases_it() {
        // A voice session inside glove mode is the obvious case, and a shared
        // bool would let whichever ended first re-enable the keyboard under the
        // other one.
        let (mut s, mut k) = (Suppressors::default(), KeyboardService::default());
        s.set(Suppressor::Glove, true);
        s.set(Suppressor::Voice, true);
        s.set(Suppressor::Voice, false);
        s.apply(&mut k);
        assert!(s.any(), "glove mode still wants it down");
        assert_eq!(k.show("app.a"), Err(Refused::Suppressed));

        s.set(Suppressor::Glove, false);
        s.apply(&mut k);
        assert_eq!(k.show("app.a"), Ok(()));
    }

    #[test]
    fn setting_the_same_suppressor_twice_does_not_double_count() {
        // Otherwise it takes two releases to clear one reason, and the second
        // never comes.
        let mut s = Suppressors::default();
        s.set(Suppressor::Voice, true);
        s.set(Suppressor::Voice, true);
        s.set(Suppressor::Voice, false);
        assert!(!s.any());
    }

    #[test]
    fn a_suppressor_arriving_mid_typing_takes_the_keyboard_away() {
        let (mut s, mut k) = (Suppressors::default(), KeyboardService::default());
        k.show("app.a").unwrap();
        s.set(Suppressor::Glove, true);
        assert_eq!(s.apply(&mut k), Some(ServiceEvent::Hidden));
    }

    #[test]
    fn suppressing_takes_a_visible_keyboard_down_immediately() {
        // The failure this exists to prevent, in the module's own words: a
        // keyboard left on screen after the reason for suppressing it arrived,
        // arriving one state later. A voice session starting while the
        // keyboard is up must hide it now, not at the next show().
        let mut s = KeyboardService::default();
        assert!(s.show("demo").is_ok());
        assert!(s.visible());
        let ev = s.set_suppressed(true);
        assert!(!s.visible(), "still up after suppression");
        assert_eq!(ev, Some(ServiceEvent::Hidden), "the hide is reported");
    }

    #[test]
    fn suppressing_an_already_hidden_keyboard_reports_nothing() {
        // No event, because nothing changed. An event here would make a
        // listener redraw or announce a hide that never happened.
        let mut s = KeyboardService::default();
        assert!(!s.visible());
        assert_eq!(s.set_suppressed(true), None);
    }

    #[test]
    fn a_suppressed_keyboard_refuses_to_come_back_until_released() {
        // The other half: suppression is not just a one-time hide. While a
        // voice session owns the screen, a client asking for the keyboard is
        // refused rather than fighting it.
        let mut s = KeyboardService::default();
        s.set_suppressed(true);
        assert!(s.show("demo").is_err(), "shown while suppressed");
        s.set_suppressed(false);
        assert!(s.show("demo").is_ok(), "released, so it may return");
        assert!(s.visible());
    }
}
