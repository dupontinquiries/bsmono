//! conversation — the detail view and the diagnostics dialog.
//!
//! `ConversationDetailDialog` is where a conversation actually happens: snaps to
//! tap open, voice payloads, a call roster of `CallParticipant`, and an "add
//! people" path. `DiagnosticsDialog` runs network checks and reports them as
//! they complete.
//!
//! ## Snaps are tapped once
//!
//! `snapTapped(snapId)` opens a snap, and an opened snap with a timer is gone.
//! So a tap must be **idempotent within the view**: a double-tap, or a re-render
//! that replays the click, must not consume the snap twice — the second consume
//! is against a snap that no longer exists, and the user sees a photo vanish
//! they never saw.
//!
//! ## Diagnostics report as they finish
//!
//! `onCheckCompleted` fires per check, `onDiagnosisReady` when all are in. A
//! diagnostics dialog that shows nothing until everything completes is useless
//! precisely when it matters — the run is slow *because* something is wrong.

/// Someone in the call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallParticipant {
    pub id: String,
    pub name: String,
}

/// A message in the thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Text { id: String, from: String, body: String, at: i64 },
    /// Tapping opens it; an opened snap with a timer does not come back.
    Snap { id: String, from: String, at: i64, opened: bool },
    Voice { id: String, from: String, at: i64, secs: u32 },
}

impl Item {
    pub fn id(&self) -> &str {
        match self { Item::Text { id, .. } | Item::Snap { id, .. } | Item::Voice { id, .. } => id }
    }
    pub fn at(&self) -> i64 {
        match self { Item::Text { at, .. } | Item::Snap { at, .. } | Item::Voice { at, .. } => *at }
    }
}

/// The conversation.
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    pub items: Vec<Item>,
    pub call: Vec<CallParticipant>,
    pub draft: String,
    /// Snaps already consumed this session, so a second tap cannot consume
    /// again. Held here rather than trusted from the item list, because the
    /// list is rebuilt from the network and can arrive stale.
    consumed: Vec<String>,
}

impl Conversation {
    /// Oldest first, ties broken on id so the thread cannot reorder between
    /// frames — a message that jumps under the thumb is one you mis-tap, and
    /// mis-tapping here opens a snap.
    pub fn ordered(&self) -> Vec<&Item> {
        let mut v: Vec<&Item> = self.items.iter().collect();
        v.sort_by(|a, b| a.at().cmp(&b.at()).then(a.id().cmp(b.id())));
        v
    }

    /// `snapTapped`. Returns the id to send, or `None` if this snap is already
    /// open or already consumed.
    ///
    /// Idempotent on purpose: a double-tap or a replayed click must not consume
    /// twice, because the second consume is against a snap that no longer
    /// exists and the user watches a photo vanish they never saw.
    pub fn tap_snap(&mut self, id: &str) -> Option<String> {
        if self.consumed.iter().any(|c| c == id) { return None; }
        let open = self.items.iter().any(|i| matches!(i,
            Item::Snap { id: sid, opened: false, .. } if sid == id));
        if !open { return None; }
        self.consumed.push(id.to_string());
        for i in self.items.iter_mut() {
            if let Item::Snap { id: sid, opened, .. } = i {
                if sid == id { *opened = true; }
            }
        }
        Some(id.to_string())
    }

    pub fn is_consumed(&self, id: &str) -> bool { self.consumed.iter().any(|c| c == id) }

    /// Unopened snaps waiting — the badge a conversation row shows.
    pub fn unopened(&self) -> usize {
        self.items.iter().filter(|i| matches!(i, Item::Snap { opened: false, .. })).count()
    }

    /// Can the draft be sent? Whitespace is not a message.
    pub fn can_send(&self) -> bool { !self.draft.trim().is_empty() }

    /// Send, returning the body and clearing the draft.
    pub fn take_draft(&mut self) -> Option<String> {
        if !self.can_send() { return None; }
        Some(std::mem::take(&mut self.draft).trim().to_string())
    }

    /// Is a call up? An empty roster is not a call — showing call chrome with
    /// nobody in it reads as a call you cannot hear.
    pub fn in_call(&self) -> bool { !self.call.is_empty() }
}

/// One network check.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub name: String,
    pub outcome: Outcome,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Still running. A distinct state, not an absence: "not yet" and "failed"
    /// look identical if a pending check is simply missing from the list.
    Pending,
    Pass,
    Warn,
    Fail,
}

impl Outcome {
    /// Sort rank: failures first, because they are what the dialog was opened
    /// for. Pending sits above passes so a slow check does not look finished.
    pub fn rank(self) -> u8 {
        match self { Outcome::Fail => 0, Outcome::Warn => 1, Outcome::Pending => 2, Outcome::Pass => 3 }
    }
}

/// The diagnostics run.
#[derive(Debug, Clone, Default)]
pub struct Diagnostics {
    pub checks: Vec<Diagnostic>,
}

impl Diagnostics {
    /// `onCheckCompleted` — replaces by name, so a re-run updates rather than
    /// appending a second row for the same check.
    pub fn complete(&mut self, d: Diagnostic) {
        match self.checks.iter_mut().find(|c| c.name == d.name) {
            Some(slot) => *slot = d,
            None => self.checks.push(d),
        }
    }

    /// Failures first, then warnings, then still-running, then passes; ties
    /// alphabetical so the list does not reshuffle as results land.
    pub fn ordered(&self) -> Vec<&Diagnostic> {
        let mut v: Vec<&Diagnostic> = self.checks.iter().collect();
        v.sort_by(|a, b| a.outcome.rank().cmp(&b.outcome.rank()).then(a.name.cmp(&b.name)));
        v
    }

    /// Everything in? `onDiagnosisReady` fires on this.
    pub fn finished(&self) -> bool {
        !self.checks.is_empty() && !self.checks.iter().any(|c| c.outcome == Outcome::Pending)
    }

    /// The headline. Reported even while running, because a diagnostics dialog
    /// that shows nothing until the end is useless precisely when it matters —
    /// the run is slow BECAUSE something is wrong.
    pub fn summary(&self) -> String {
        let f = self.checks.iter().filter(|c| c.outcome == Outcome::Fail).count();
        let w = self.checks.iter().filter(|c| c.outcome == Outcome::Warn).count();
        let p = self.checks.iter().filter(|c| c.outcome == Outcome::Pending).count();
        if f > 0 { format!("{f} failed") }
        else if w > 0 { format!("{w} warning{}", if w == 1 { "" } else { "s" }) }
        else if p > 0 { format!("checking… ({p} left)") }
        else if self.checks.is_empty() { "no checks run".into() }
        else { "all clear".into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(id: &str, at: i64, opened: bool) -> Item {
        Item::Snap { id: id.into(), from: "ada".into(), at, opened }
    }

    fn convo() -> Conversation {
        Conversation {
            items: vec![
                Item::Text { id: "t1".into(), from: "ada".into(), body: "hi".into(), at: 100 },
                snap("s1", 200, false),
                snap("s2", 300, true),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn a_snap_can_only_be_consumed_once() {
        // The second consume is against a snap that no longer exists, and the
        // user watches a photo vanish they never saw.
        let mut c = convo();
        assert_eq!(c.tap_snap("s1").as_deref(), Some("s1"));
        assert_eq!(c.tap_snap("s1"), None, "a double tap consumes nothing");
        assert!(c.is_consumed("s1"));
    }

    #[test]
    fn an_already_opened_snap_is_not_re_consumed() {
        let mut c = convo();
        assert_eq!(c.tap_snap("s2"), None, "s2 arrived already opened");
        assert_eq!(c.tap_snap("nope"), None, "and an unknown id does nothing");
    }

    #[test]
    fn consumption_survives_the_list_being_rebuilt_from_the_network() {
        // The item list is rebuilt from the network and can arrive stale, which
        // is exactly when a re-tap would double-consume.
        let mut c = convo();
        c.tap_snap("s1");
        c.items = vec![snap("s1", 200, false)];   // stale: server has not caught up
        assert_eq!(c.tap_snap("s1"), None, "we know we already opened it");
    }

    #[test]
    fn the_unopened_badge_counts_what_is_waiting() {
        let mut c = convo();
        assert_eq!(c.unopened(), 1);
        c.tap_snap("s1");
        assert_eq!(c.unopened(), 0);
    }

    #[test]
    fn the_thread_cannot_reorder_between_frames() {
        // A message that jumps under the thumb is one you mis-tap, and
        // mis-tapping here opens a snap.
        let c = Conversation {
            items: vec![snap("b", 100, false), snap("a", 100, false)],
            ..Default::default()
        };
        let first: Vec<&str> = c.ordered().iter().map(|i| i.id()).collect();
        let again: Vec<&str> = c.ordered().iter().map(|i| i.id()).collect();
        assert_eq!(first, again);
        assert_eq!(first, vec!["a", "b"], "equal times break on id");
    }

    #[test]
    fn whitespace_is_not_a_message() {
        let mut c = convo();
        c.draft = "   ".into();
        assert!(!c.can_send());
        assert!(c.take_draft().is_none());
        c.draft = "  hello  ".into();
        assert_eq!(c.take_draft().as_deref(), Some("hello"), "trimmed");
        assert!(c.draft.is_empty(), "and cleared");
    }

    #[test]
    fn an_empty_roster_is_not_a_call() {
        // Call chrome with nobody in it reads as a call you cannot hear.
        let mut c = convo();
        assert!(!c.in_call());
        c.call.push(CallParticipant { id: "1".into(), name: "Ada".into() });
        assert!(c.in_call());
    }

    #[test]
    fn diagnostics_show_failures_first_and_pending_above_passes() {
        // Failures are what the dialog was opened for, and a slow check must not
        // look finished.
        let mut d = Diagnostics::default();
        for (n, o) in [("dns", Outcome::Pass), ("relay", Outcome::Fail),
                       ("stun", Outcome::Pending), ("mtu", Outcome::Warn)] {
            d.complete(Diagnostic { name: n.into(), outcome: o, detail: String::new() });
        }
        let order: Vec<&str> = d.ordered().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(order, vec!["relay", "mtu", "stun", "dns"]);
    }

    #[test]
    fn a_rerun_updates_a_check_rather_than_adding_a_second_row() {
        let mut d = Diagnostics::default();
        d.complete(Diagnostic { name: "dns".into(), outcome: Outcome::Fail, detail: "x".into() });
        d.complete(Diagnostic { name: "dns".into(), outcome: Outcome::Pass, detail: "ok".into() });
        assert_eq!(d.checks.len(), 1);
        assert_eq!(d.checks[0].outcome, Outcome::Pass);
    }

    #[test]
    fn the_summary_is_useful_before_the_run_finishes() {
        // A dialog that shows nothing until the end is useless precisely when it
        // matters — the run is slow BECAUSE something is wrong.
        let mut d = Diagnostics::default();
        assert_eq!(d.summary(), "no checks run");
        d.complete(Diagnostic { name: "a".into(), outcome: Outcome::Pending, detail: String::new() });
        assert!(d.summary().contains("checking"));
        assert!(!d.finished());
        d.complete(Diagnostic { name: "a".into(), outcome: Outcome::Fail, detail: String::new() });
        assert_eq!(d.summary(), "1 failed");
        assert!(d.finished());
    }
}
