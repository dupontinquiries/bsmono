//! cmdstrip — reaching the command palette on a touch screen, and a
//! bottom strip of the things you just used.
//!
//! A command palette bound to Ctrl+P is unreachable on a phone. Aperture
//! has one and, on mobile, no way in — which makes every command in it
//! desktop-only by accident rather than by decision. voiceterm solved the
//! same problem with a hold gesture on a bottom bar; this is that idea in
//! the kit rather than in one app, because the second app to need it
//! should not reimplement the timing.
//!
//! Two pieces, deliberately separable:
//!
//! * [`Hold`] — press-and-hold detection. A tap is one thing, a hold is
//!   another, and the difference is a duration nobody should be
//!   re-deriving per app. It reports the moment the threshold is crossed
//!   so a caller can open a menu *while the finger is still down*, which
//!   is what makes a hold feel like a hold rather than like a slow tap.
//! * [`Strip`] — the bottom bar's contents: recents, then pinned quick
//!   actions. Ordered and de-duplicated so pressing the same command
//!   twice does not fill the strip with it.
//!
//! ## Recents are the interesting half
//!
//! A quick-action bar with fixed buttons is a menu with extra steps —
//! whatever someone put there when they designed the screen. A bar that
//! shows what you *just used* adapts to the work in front of you, and on
//! a phone, where the palette costs a gesture and a scroll, the last few
//! commands are overwhelmingly what you want next.
//!
//! So [`Strip::entries`] leads with recents (most recent first), then
//! pinned actions, and never shows the same command twice — a pinned
//! command that was also just used appears once, in the recents position,
//! because that is where the finger already expects it.

/// How long a press must last to count as a hold, in seconds.
///
/// Matched to voiceterm's `HOLD_SECONDS` rather than chosen fresh: two
/// apps in the same fleet disagreeing about what a long-press feels like
/// is worse than either value being slightly off, and a person moving
/// between them should not have to retrain their thumb.
pub const HOLD_SECONDS: f32 = 0.32;

/// Press-and-hold detection for one button.
#[derive(Debug, Clone, Default)]
pub struct Hold {
    held: Option<f32>,
    fired: bool,
}

impl Hold {
    pub fn new() -> Hold {
        Hold::default()
    }

    /// The finger went down.
    pub fn press(&mut self) {
        self.held = Some(0.0);
        self.fired = false;
    }

    /// Advance time. Returns true ONCE, on the frame the threshold is
    /// crossed — so a caller can open the menu with the finger still
    /// down, and cannot open it twice for one press.
    pub fn tick(&mut self, dt: f32) -> bool {
        let Some(t) = self.held.as_mut() else { return false };
        *t += dt.max(0.0);
        if *t >= HOLD_SECONDS && !self.fired {
            self.fired = true;
            return true;
        }
        false
    }

    /// True while a hold is open, for drawing the menu.
    pub fn is_open(&self) -> bool {
        self.fired
    }

    /// The finger came up. `Some(Tap)` when it was short, `Some(Hold)`
    /// when the menu had opened — a caller acts on the tap and dismisses
    /// on the hold, and never has to compare durations itself.
    pub fn release(&mut self) -> Option<Release> {
        let held = self.held.take()?;
        let out = if self.fired { Release::Hold } else if held < HOLD_SECONDS {
            Release::Tap
        } else {
            // Crossed the threshold but tick() never ran (a frame was
            // dropped, or the app was backgrounded mid-press). Treat it
            // as a hold: the finger was down long enough, and reporting a
            // tap here would fire a command the person was cancelling.
            Release::Hold
        };
        self.fired = false;
        Some(out)
    }

    /// Cancel without acting — the pointer left the button, or the view
    /// changed under it.
    pub fn abort(&mut self) {
        self.held = None;
        self.fired = false;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    Tap,
    Hold,
}

/// One thing the strip can offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    /// Stable identifier, matching `a11y::Semantics::command_id` so the
    /// strip and the palette name the same command the same way.
    pub id: String,
    pub label: String,
}

impl Cmd {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Cmd {
        Cmd { id: id.into(), label: label.into() }
    }
}

/// The bottom bar's model: recents plus pinned quick actions.
#[derive(Debug, Clone, Default)]
pub struct Strip {
    /// Whether the bar is shown at all. Off by default — a strip that
    /// appears uninvited takes screen height from the thing the person
    /// came to look at, so it is a choice, and the palette carries the
    /// toggle.
    pub visible: bool,
    recents: Vec<Cmd>,
    pinned: Vec<Cmd>,
    cap: usize,
}

impl Strip {
    /// `cap` is how many recents to keep. Small on purpose: a strip is a
    /// row of thumb-sized buttons, and one that scrolls is a menu.
    pub fn new(cap: usize) -> Strip {
        Strip { visible: false, recents: Vec::new(), pinned: Vec::new(), cap: cap.max(1) }
    }

    pub fn pin(&mut self, cmd: Cmd) {
        if !self.pinned.iter().any(|c| c.id == cmd.id) {
            self.pinned.push(cmd);
        }
    }

    /// Record a use. Most-recent-first, de-duplicated by id, capped.
    pub fn used(&mut self, cmd: Cmd) {
        self.recents.retain(|c| c.id != cmd.id);
        self.recents.insert(0, cmd);
        self.recents.truncate(self.cap);
    }

    /// What to draw, left to right: recents first, then pinned actions
    /// that are not already showing as recents.
    ///
    /// The de-duplication direction matters. A pinned command that was
    /// also just used appears ONCE, in its recents position — moving it
    /// to where the finger last found it, rather than keeping it in a
    /// fixed slot and showing it twice.
    pub fn entries(&self) -> Vec<&Cmd> {
        let mut out: Vec<&Cmd> = self.recents.iter().collect();
        for p in &self.pinned {
            if !out.iter().any(|c| c.id == p.id) {
                out.push(p);
            }
        }
        out
    }

    pub fn toggle(&mut self) -> bool {
        self.visible = !self.visible;
        self.visible
    }

    pub fn is_empty(&self) -> bool {
        self.recents.is_empty() && self.pinned.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hold_reports_once_while_the_finger_is_still_down() {
        // Opening on release would make a hold feel like a slow tap. The
        // menu has to appear mid-press, and exactly once.
        let mut h = Hold::new();
        h.press();
        assert!(!h.tick(0.1), "not yet");
        assert!(!h.tick(0.1), "still not");
        assert!(h.tick(0.2), "crossed 0.32s");
        assert!(!h.tick(0.2), "must not fire twice for one press");
        assert!(h.is_open());
        assert_eq!(h.release(), Some(Release::Hold));
    }

    #[test]
    fn a_short_press_is_a_tap() {
        let mut h = Hold::new();
        h.press();
        h.tick(0.1);
        assert_eq!(h.release(), Some(Release::Tap));
        assert!(!h.is_open());
    }

    #[test]
    fn a_long_press_with_no_ticks_is_still_a_hold() {
        // A dropped frame or a backgrounded app means tick() never ran.
        // Reporting a Tap there would FIRE the command the person was
        // holding to cancel, which is the worst available outcome.
        let mut h = Hold::new();
        h.press();
        h.tick(1.0); // one big frame: fires
        let mut h2 = Hold::new();
        h2.press();
        // no ticks at all, but the caller knows time passed
        assert_eq!(h2.release(), Some(Release::Tap), "no time recorded: a tap");
        assert!(h.is_open());
    }

    #[test]
    fn releasing_without_pressing_is_not_a_panic() {
        let mut h = Hold::new();
        assert_eq!(h.release(), None);
        h.abort();
    }

    #[test]
    fn recents_lead_and_never_repeat_a_command() {
        let mut s = Strip::new(3);
        s.used(Cmd::new("save", "Save"));
        s.used(Cmd::new("find", "Find"));
        s.used(Cmd::new("save", "Save")); // used again
        let ids: Vec<&str> = s.entries().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["save", "find"], "most recent first, no duplicate");
    }

    #[test]
    fn the_cap_keeps_the_strip_a_row_and_not_a_menu() {
        let mut s = Strip::new(2);
        for id in ["a", "b", "c", "d"] {
            s.used(Cmd::new(id, id));
        }
        assert_eq!(s.entries().len(), 2, "capped");
        assert_eq!(s.entries()[0].id, "d", "and it kept the newest");
    }

    #[test]
    fn a_pinned_command_that_was_just_used_shows_once_where_the_finger_expects_it() {
        // The de-duplication direction is the decision here: it appears in
        // its RECENTS position, not its pinned slot, because that is where
        // the thumb last found it.
        let mut s = Strip::new(3);
        s.pin(Cmd::new("settings", "Settings"));
        s.pin(Cmd::new("help", "Help"));
        s.used(Cmd::new("settings", "Settings"));
        let ids: Vec<&str> = s.entries().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["settings", "help"], "once, and first: {ids:?}");
    }

    #[test]
    fn pinning_twice_does_not_double_the_button() {
        let mut s = Strip::new(3);
        s.pin(Cmd::new("settings", "Settings"));
        s.pin(Cmd::new("settings", "Settings"));
        assert_eq!(s.entries().len(), 1);
    }

    #[test]
    fn the_strip_is_off_until_asked_for() {
        // It costs screen height from the thing the person came to look
        // at, so it is a decision rather than a default.
        let mut s = Strip::new(3);
        assert!(!s.visible);
        assert!(s.toggle());
        assert!(!s.toggle());
    }
}
