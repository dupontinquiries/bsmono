//! help_manual — `HelpManualDialog.cpp`, ported.
//!
//! A scrollable manual that dismisses itself after a period of inactivity.
//!
//! ## The idle rule is subtler than "close after 30 seconds"
//!
//! The C++ resets the countdown on the scrollbar's `valueChanged`, and then
//! installs an event filter for wheel, mouse, touch and key events as well —
//! with a comment saying why: *"so a reader who pauses on a short section that
//! doesn't move the scrollbar isn't dismissed mid-read"*.
//!
//! That second half is the whole feature. A manual that closed on someone
//! reading a short page would be worse than one that never closed, because it
//! punishes exactly the careful reader. So the port keeps **two** distinct
//! activity sources rather than collapsing them into "did the scroll offset
//! change", which is the tidier-looking thing that would reintroduce the bug.
//!
//! ## The timer is fed, not owned
//!
//! `tick(dt_ms)` rather than a `QTimer`. An egui app already has a frame loop
//! with a delta, and a widget that owned a thread would need shutting down at
//! exactly the moment a dialog is being destroyed — the one place lifetimes are
//! already awkward.

/// How long the manual sits open with no interaction before dismissing itself.
///
/// Thirty seconds, from the original. Generous because reading takes time, and
/// any activity resets it — so an engaged reader never sees it fire.
pub const IDLE_DISMISS_MS: f32 = 30_000.0;

/// One section of the manual.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub anchor: String,
    pub title: String,
    pub body: String,
}

impl Section {
    pub fn new(anchor: &str, title: &str, body: &str) -> Section {
        Section { anchor: anchor.into(), title: title.into(), body: body.into() }
    }
}

/// Why the dialog closed, which the caller may want to distinguish.
///
/// A user who dismissed it deliberately has read enough; one who timed out may
/// not have. Collapsing them would make "don't show this again" impossible to
/// offer honestly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Closed {
    ByUser,
    Idle,
}

#[derive(Clone, Debug)]
pub struct HelpManual {
    sections: Vec<Section>,
    /// Milliseconds since the last activity of any kind.
    idle_ms: f32,
    open: bool,
    closed_by: Option<Closed>,
    filter: String,
}

impl Default for HelpManual {
    fn default() -> Self {
        HelpManual { sections: Vec::new(), idle_ms: 0.0, open: false,
                     closed_by: None, filter: String::new() }
    }
}

impl HelpManual {
    pub fn new(sections: Vec<Section>) -> HelpManual {
        HelpManual { sections, ..Default::default() }
    }

    pub fn open(&mut self) {
        self.open = true;
        self.closed_by = None;
        // Reset on open, not just on activity: a dialog reopened after being
        // idle-dismissed must get its full reading time again rather than
        // closing instantly.
        self.idle_ms = 0.0;
    }

    pub fn is_open(&self) -> bool { self.open }
    pub fn closed_by(&self) -> Option<Closed> { self.closed_by }
    pub fn idle_ms(&self) -> f32 { self.idle_ms }

    /// The user scrolled. Distinct from [`Self::interacted`] on purpose — see
    /// the module docs.
    pub fn scrolled(&mut self) { self.idle_ms = 0.0; }

    /// The user did something that is not a scroll: wheel, tap, drag, key.
    ///
    /// The reason this exists separately is a reader paused on a short section,
    /// whose scroll offset never changes.
    pub fn interacted(&mut self) { self.idle_ms = 0.0; }

    pub fn dismiss(&mut self) {
        if self.open { self.open = false; self.closed_by = Some(Closed::ByUser); }
    }

    /// Advance the idle countdown. Returns true when this call dismissed it.
    pub fn tick(&mut self, dt_ms: f32) -> bool {
        if !self.open { return false; }
        // Negative or absurd deltas are ignored rather than trusted: a
        // suspended app resumes with an enormous dt, and closing the manual
        // because the laptop lid was shut is not the behaviour anyone wants.
        if dt_ms <= 0.0 || dt_ms > IDLE_DISMISS_MS { return false; }
        self.idle_ms += dt_ms;
        if self.idle_ms >= IDLE_DISMISS_MS {
            self.open = false;
            self.closed_by = Some(Closed::Idle);
            return true;
        }
        false
    }

    /// How close to dismissal, 0..=1 — for a subtle progress hint.
    pub fn idle_progress(&self) -> f32 { (self.idle_ms / IDLE_DISMISS_MS).clamp(0.0, 1.0) }

    pub fn set_filter(&mut self, f: &str) {
        self.filter = f.to_string();
        // Typing is activity. Without this, searching the manual for thirty
        // seconds would dismiss it mid-search — the most frustrating possible
        // moment.
        self.interacted();
    }

    /// Sections matching the filter, in their authored order.
    ///
    /// Authored order rather than relevance-ranked: a manual has a deliberate
    /// sequence, and reordering it by match quality makes the same search
    /// return the same sections in a different order each time the text changes
    /// by one character.
    pub fn visible(&self) -> Vec<&Section> {
        let needle = self.filter.trim().to_lowercase();
        self.sections.iter()
            .filter(|s| needle.is_empty()
                || s.title.to_lowercase().contains(&needle)
                || s.body.to_lowercase().contains(&needle))
            .collect()
    }

    /// Index of a section by anchor, for deep links from elsewhere in the app.
    pub fn index_of(&self, anchor: &str) -> Option<usize> {
        self.sections.iter().position(|s| s.anchor == anchor)
    }

    pub fn is_empty(&self) -> bool { self.sections.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manual() -> HelpManual {
        let mut m = HelpManual::new(vec![
            Section::new("send", "Sending files", "Drag a file onto the window to send it."),
            Section::new("peers", "Finding peers", "Peers on your network appear automatically."),
            Section::new("privacy", "Privacy", "Everything is encrypted end to end."),
        ]);
        m.open();
        m
    }

    #[test]
    fn it_dismisses_itself_after_the_idle_period() {
        let mut m = manual();
        assert!(!m.tick(IDLE_DISMISS_MS - 1.0));
        assert!(m.is_open());
        assert!(m.tick(1.0), "the tick that crossed the line reports it");
        assert!(!m.is_open());
        assert_eq!(m.closed_by(), Some(Closed::Idle));
    }

    #[test]
    fn scrolling_resets_the_countdown() {
        let mut m = manual();
        m.tick(29_000.0);
        m.scrolled();
        assert_eq!(m.idle_ms(), 0.0);
        assert!(!m.tick(29_000.0), "a fresh thirty seconds");
        assert!(m.is_open());
    }

    #[test]
    fn a_reader_paused_on_a_short_section_is_not_dismissed_mid_read() {
        // The whole point of the second activity source. A manual that closed
        // on somebody reading a short page would be worse than one that never
        // closed, because it punishes exactly the careful reader.
        let mut m = manual();
        for _ in 0..10 {
            m.tick(29_000.0);
            m.interacted();          // a tap, a wheel nudge, a key — no scroll
        }
        assert!(m.is_open(), "still reading after five minutes of short sections");
    }

    #[test]
    fn typing_in_the_filter_counts_as_activity() {
        // Otherwise searching the manual for thirty seconds dismisses it
        // mid-search — the most frustrating possible moment.
        let mut m = manual();
        m.tick(29_000.0);
        m.set_filter("priv");
        assert_eq!(m.idle_ms(), 0.0);
    }

    #[test]
    fn a_deliberate_dismissal_is_distinguishable_from_a_timeout() {
        // A user who closed it has read enough; one who timed out may not have.
        // Collapsing them makes "don't show this again" impossible to offer
        // honestly.
        let mut m = manual();
        m.dismiss();
        assert_eq!(m.closed_by(), Some(Closed::ByUser));

        let mut n = manual();
        n.tick(IDLE_DISMISS_MS);
        assert_eq!(n.closed_by(), Some(Closed::Idle));
    }

    #[test]
    fn a_resumed_app_does_not_close_the_manual_because_the_lid_was_shut() {
        // A suspended process resumes with an enormous delta, and dismissing on
        // it is not what anyone means by "idle".
        let mut m = manual();
        assert!(!m.tick(4_000_000.0));
        assert!(m.is_open());
        assert_eq!(m.idle_ms(), 0.0, "the absurd delta was ignored entirely");
    }

    #[test]
    fn a_negative_delta_is_ignored() {
        // Clocks go backwards; the countdown must not go with them.
        let mut m = manual();
        m.tick(1_000.0);
        m.tick(-5_000.0);
        assert_eq!(m.idle_ms(), 1_000.0);
    }

    #[test]
    fn reopening_after_a_timeout_grants_the_full_reading_time_again() {
        // Otherwise the second open closes instantly and looks broken.
        let mut m = manual();
        m.tick(IDLE_DISMISS_MS);
        assert!(!m.is_open());
        m.open();
        assert_eq!(m.idle_ms(), 0.0);
        assert!(m.is_open());
        assert_eq!(m.closed_by(), None, "a fresh open has no closure reason yet");
    }

    #[test]
    fn a_closed_manual_does_not_count_down() {
        // Otherwise the idle state drifts while invisible and the next open
        // inherits it.
        let mut m = manual();
        m.dismiss();
        assert!(!m.tick(IDLE_DISMISS_MS * 2.0));
        assert_eq!(m.closed_by(), Some(Closed::ByUser), "still the user's doing");
    }

    #[test]
    fn the_filter_searches_bodies_as_well_as_titles() {
        // Somebody looking for "encrypted" does not know it lives under
        // "Privacy".
        let mut m = manual();
        m.set_filter("encrypted");
        assert_eq!(m.visible().len(), 1);
        assert_eq!(m.visible()[0].anchor, "privacy");
    }

    #[test]
    fn matches_stay_in_authored_order() {
        // A manual has a deliberate sequence, and relevance-ranking makes the
        // same search reorder itself as the text changes by one character.
        let mut m = manual();
        m.set_filter("e");
        let anchors: Vec<&str> = m.visible().iter().map(|s| s.anchor.as_str()).collect();
        assert_eq!(anchors, vec!["send", "peers", "privacy"]);
    }

    #[test]
    fn an_empty_or_whitespace_filter_shows_everything() {
        // A stray space from a phone keyboard must not empty the manual.
        let mut m = manual();
        m.set_filter("   ");
        assert_eq!(m.visible().len(), 3);
    }

    #[test]
    fn an_anchor_resolves_for_deep_links_from_elsewhere_in_the_app() {
        let m = manual();
        assert_eq!(m.index_of("peers"), Some(1));
        assert_eq!(m.index_of("nonexistent"), None);
    }

    #[test]
    fn idle_progress_is_clamped_for_a_hint_bar() {
        let mut m = manual();
        assert_eq!(m.idle_progress(), 0.0);
        m.tick(IDLE_DISMISS_MS / 2.0);
        assert!((m.idle_progress() - 0.5).abs() < 0.01);
    }
}
