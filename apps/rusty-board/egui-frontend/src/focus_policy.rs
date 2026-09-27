//! focus_policy — should the keyboard be on screen right now?
//!
//! Pure decision logic: no egui, no wayland-client, no clock of its own.
//! Callers pass what they observed and a monotonic millisecond stamp; this
//! returns a verdict and the reason for it. That is what makes the failure
//! modes below testable at all — none of them can be reproduced on a
//! developer machine, and two of them cannot be reproduced on a *working*
//! phone either.
//!
//! ## The bug this exists to end
//!
//! `demo_app` used to drive `ViewportCommand::Visible` straight off
//! `Sink::is_visible()`, which for the Wayland sink is a `Cell<bool>`
//! starting at `false` and flipped only by an input-method Activate. So
//! "no Activate has arrived" and "the field just lost focus" were the same
//! state, and both meant hidden. Three ways to get stuck in the first one:
//!
//! 1. **Another OSK owns the seat.** `zwp_input_method_v2` is one-per-seat;
//!    a second client gets `unavailable` and never an Activate. squeekboard
//!    ships enabled on most postmarketOS/Phosh images, so this is the
//!    default state there, not an edge case. `board_compositor::im_health`
//!    is the protocol-side half of this fix.
//! 2. **A compositor whose Activate never comes** for some other reason —
//!    an image with a partial input-method implementation, a seat we bound
//!    the wrong side of. Unenumerable by construction, which is why rule 3
//!    below is a timeout rather than a list.
//! 3. **The user turned auto-hide off** and the front-end never read the
//!    setting. `behavior.auto_hide_on_no_focus` has existed in the shared
//!    `Config` the whole time (the egui build mounts the GTK app's real
//!    config module — see `settings.rs`); only the GTK front-end honoured
//!    it. It is the documented escape hatch, and here it did nothing.
//!
//! In all three the symptom is identical and maximally unhelpful: the
//! process is running, the log says the backend came up, and the screen
//! stays empty.
//!
//! ## The asymmetry the default leans on
//!
//! Hiding when we should show is unrecoverable from the user's side — they
//! cannot tap a keyboard that is not drawn. Showing when we should hide
//! costs one unwanted appearance which the next real focus event corrects.
//! So every uncertain state here resolves to VISIBLE, and the grace timer
//! (rule 3) exists so that "uncertain" eventually includes "this compositor
//! has said nothing at all for a long time".
//!
//! The GTK front-end's own comment (`ui/window.rs`, initial visibility)
//! reaches the same conclusion from the other direction — it starts hidden
//! because a keyboard covering the screen at login is worse than a beat of
//! delay — and then names the exact hole this module fills: *"If a backend
//! genuinely never fires Activate, the user can disable auto-hide in
//! settings."* That is a fallback only if something reads the setting.
//!
//! ## What this deliberately does not do
//!
//! It does not port the GTK build's v1 speculative-Activate deferral
//! (`v2_trusted` + the 80 ms / 270 ms SurroundingText reconciler). That
//! solves the opposite complaint — Plasma Mobile's KWin fires Activate on
//! window-level focus, so the keyboard appears over fields the user never
//! selected — and it cannot be ported as-is regardless, because this
//! front-end's `surrounding_text_callback` is an empty sink, so the signal
//! it gates on does not arrive here. Tracked, not fixed.

/// What the front-end knows about the focus signal this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusSource {
    /// No compositor focus signal exists: the `Preview` sink, an X11
    /// session, or a compositor exposing neither input-method protocol.
    /// Nothing will ever ask us to hide, so nothing should.
    None,
    /// A live input-method connection.
    InputMethod {
        /// The latest Activate/Deactivate state.
        active: bool,
        /// `board_compositor::im_health::input_method_unavailable()` —
        /// the compositor told us another client owns the seat's input
        /// method, so `active` is frozen at whatever it was and means
        /// nothing.
        unavailable: bool,
    },
}

/// The user's `behavior.auto_hide_on_no_focus`, resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoHide {
    /// Default. Follow the compositor, with the fallbacks above.
    Follow,
    /// `auto_hide_on_no_focus = false`: stay on screen regardless. The
    /// escape hatch, and the one setting guaranteed to work on a
    /// compositor we have never seen.
    Never,
}

impl AutoHide {
    /// From the shared config's `Option<bool>`. `None` is documented in
    /// `config/mod.rs` as meaning `Some(true)`.
    pub fn from_config(auto_hide_on_no_focus: Option<bool>) -> AutoHide {
        match auto_hide_on_no_focus {
            Some(false) => AutoHide::Never,
            _ => AutoHide::Follow,
        }
    }
}

/// Why the policy decided what it decided. Carried so the status line and
/// the log can say it: a keyboard that is visible for a *reason* the user
/// can read is the difference between "it works" and "it works today".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// `auto_hide_on_no_focus = false`.
    AutoHideDisabled,
    /// No focus signal exists to obey (desktop preview, X11, no protocol).
    NoFocusSignal,
    /// A field is focused.
    Focused,
    /// A field lost focus, and we trust that.
    Unfocused,
    /// Another client owns the seat's input method; `active` is meaningless.
    InputMethodTakenByAnotherClient,
    /// Nothing has ever activated within the grace window. Assume the
    /// signal is broken rather than assume the user never typed.
    NoActivateWithinGrace,
    /// Nothing has activated yet, but the grace window is still open.
    AwaitingFirstActivate,
}

impl Reason {
    /// One short line for the status strip.
    pub fn describe(self) -> &'static str {
        match self {
            Reason::AutoHideDisabled => "auto-hide off (always visible)",
            Reason::NoFocusSignal => "no compositor focus signal",
            Reason::Focused => "field focused",
            Reason::Unfocused => "no field focused",
            Reason::InputMethodTakenByAnotherClient => {
                "another on-screen keyboard owns the input method — \
                 showing anyway (disable the other OSK)"
            }
            Reason::NoActivateWithinGrace => {
                "compositor never signalled focus — showing anyway"
            }
            Reason::AwaitingFirstActivate => "waiting for first focus",
        }
    }

    /// Whether this reason means the focus signal is not to be trusted.
    /// The status line renders these differently: they are the states a
    /// person can act on.
    pub fn is_degraded(self) -> bool {
        matches!(
            self,
            Reason::InputMethodTakenByAnotherClient | Reason::NoActivateWithinGrace
        )
    }
}

/// A decision, with the reason attached so callers do not re-derive it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub visible: bool,
    pub reason: Reason,
}

/// How long to wait for a first Activate before concluding none is coming.
///
/// Chosen for the cost of being wrong in each direction, not for any
/// property of the protocol. Too short and a user who logs in and reads
/// something for a minute gets a keyboard over their screen — the
/// regression the GTK build's initial-visibility comment warns about. Too
/// long and a genuinely broken image is unusable for that long. 45 s is
/// past any plausible "focused a field and started typing" latency, and a
/// false positive self-corrects on the next real Activate, so the error is
/// one unwanted appearance rather than a stuck state.
pub const DEFAULT_GRACE_MS: u64 = 45_000;

/// Environment override, for diagnosing a device without editing config:
/// `RUSTY_BOARD_FOCUS_GRACE_MS=0` shows the keyboard immediately when no
/// Activate has arrived, `=disabled` never applies the timeout at all.
pub const GRACE_ENV: &str = "RUSTY_BOARD_FOCUS_GRACE_MS";

#[derive(Debug, Clone, Copy)]
pub struct FocusPolicy {
    auto_hide: AutoHide,
    /// `None` disables the grace fallback entirely.
    grace_ms: Option<u64>,
    /// When we first saw a live input-method source, for the grace window.
    first_seen_ms: Option<u64>,
    /// Whether a real Activate has ever arrived. Once it has, silence
    /// means "no field focused" and is to be believed.
    ever_activated: bool,
}

impl Default for FocusPolicy {
    fn default() -> Self {
        FocusPolicy {
            auto_hide: AutoHide::Follow,
            grace_ms: Some(DEFAULT_GRACE_MS),
            first_seen_ms: None,
            ever_activated: false,
        }
    }
}

impl FocusPolicy {
    pub fn new(auto_hide: AutoHide, grace_ms: Option<u64>) -> FocusPolicy {
        FocusPolicy { auto_hide, grace_ms, first_seen_ms: None, ever_activated: false }
    }

    /// Build from the shared config plus the environment override.
    pub fn from_config(auto_hide_on_no_focus: Option<bool>) -> FocusPolicy {
        FocusPolicy::new(
            AutoHide::from_config(auto_hide_on_no_focus),
            grace_from_env(std::env::var(GRACE_ENV).ok().as_deref()),
        )
    }

    /// Whether a real Activate has been seen this session. Exposed for the
    /// status line, and because a test that cannot see this cannot tell
    /// "believed the compositor" from "gave up on it".
    pub fn ever_activated(&self) -> bool {
        self.ever_activated
    }

    /// When the decision could next change WITHOUT any new input: the end of
    /// the grace window, while it is still open. `None` otherwise — every
    /// other transition is driven by a compositor event, which wakes the
    /// host on its own. Lets an idle host sleep instead of polling.
    pub fn next_timed_change_ms(&self, now_ms: u64) -> Option<u64> {
        if self.auto_hide == AutoHide::Never || self.ever_activated {
            return None;
        }
        let (grace, first) = (self.grace_ms?, self.first_seen_ms?);
        let waited = now_ms.saturating_sub(first);
        (waited < grace).then(|| grace - waited)
    }

    /// One frame. `now_ms` must be monotonic; its origin does not matter.
    pub fn decide(&mut self, now_ms: u64, source: FocusSource) -> Decision {
        // The escape hatch outranks everything, including a working
        // signal: a person who set this is telling us they do not want
        // the keyboard to disappear, and no amount of protocol evidence
        // should overrule that.
        if self.auto_hide == AutoHide::Never {
            return Decision { visible: true, reason: Reason::AutoHideDisabled };
        }

        let (active, unavailable) = match source {
            FocusSource::None => {
                return Decision { visible: true, reason: Reason::NoFocusSignal };
            }
            FocusSource::InputMethod { active, unavailable } => (active, unavailable),
        };

        if self.first_seen_ms.is_none() {
            self.first_seen_ms = Some(now_ms);
        }

        // `unavailable` is checked before `active` on purpose: a dead
        // input-method object can still be carrying a stale `active` from
        // before it died, and the whole point is that we stop reading it.
        if unavailable {
            return Decision {
                visible: true,
                reason: Reason::InputMethodTakenByAnotherClient,
            };
        }

        if active {
            self.ever_activated = true;
            return Decision { visible: true, reason: Reason::Focused };
        }

        if self.ever_activated {
            // The signal has proved itself. Silence now means what it says.
            return Decision { visible: false, reason: Reason::Unfocused };
        }

        match self.grace_ms {
            // Fallback disabled: behave as before, and stay hidden. Kept
            // because a device where the timeout misfires needs a way to
            // turn it off that is not "turn off auto-hide entirely".
            None => Decision { visible: false, reason: Reason::AwaitingFirstActivate },
            Some(grace) => {
                let waited = now_ms.saturating_sub(self.first_seen_ms.unwrap_or(now_ms));
                if waited >= grace {
                    Decision { visible: true, reason: Reason::NoActivateWithinGrace }
                } else {
                    Decision { visible: false, reason: Reason::AwaitingFirstActivate }
                }
            }
        }
    }
}

/// Parse the grace override. Anything unparseable falls back to the
/// default rather than to "disabled" — a typo in an env var must not
/// silently remove a fallback.
pub fn grace_from_env(raw: Option<&str>) -> Option<u64> {
    match raw.map(str::trim) {
        None | Some("") => Some(DEFAULT_GRACE_MS),
        Some("disabled") | Some("off") | Some("never") => None,
        Some(v) => match v.parse::<u64>() {
            Ok(ms) => Some(ms),
            Err(_) => {
                eprintln!(
                    "[rusty-board] {GRACE_ENV}={v:?} is not a number of \
                     milliseconds (or 'disabled'); using {DEFAULT_GRACE_MS}"
                );
                Some(DEFAULT_GRACE_MS)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_only_timed_change_is_the_grace_window_closing() {
        let im = |a| FocusSource::InputMethod { active: a, unavailable: false };
        let mut p = FocusPolicy::new(AutoHide::Follow, Some(1_000));
        assert_eq!(p.next_timed_change_ms(0), None, "nothing seen yet");
        p.decide(100, im(false));
        assert_eq!(p.next_timed_change_ms(400), Some(700));
        assert_eq!(p.next_timed_change_ms(1_200), None, "window already closed");
        p.decide(500, im(true));
        assert_eq!(p.next_timed_change_ms(600), None, "activated: only events matter now");
        let never = FocusPolicy::new(AutoHide::Never, Some(1_000));
        assert_eq!(never.next_timed_change_ms(0), None);
    }

    use super::*;

    fn im(active: bool) -> FocusSource {
        FocusSource::InputMethod { active, unavailable: false }
    }
    fn dead() -> FocusSource {
        FocusSource::InputMethod { active: false, unavailable: true }
    }

    #[test]
    fn a_working_compositor_still_gets_plain_auto_hide() {
        // The regression guard for everything below: none of the
        // fallbacks may change what a healthy device does.
        let mut p = FocusPolicy::default();
        assert!(!p.decide(0, im(false)).visible, "hidden before first focus");
        let d = p.decide(1_000, im(true));
        assert!(d.visible && d.reason == Reason::Focused);
        let d = p.decide(2_000, im(false));
        assert!(!d.visible && d.reason == Reason::Unfocused);
        // And it stays believable afterwards — the grace window must not
        // re-arm and start forcing the keyboard up between fields.
        let d = p.decide(2_000 + DEFAULT_GRACE_MS * 10, im(false));
        assert!(!d.visible, "a proven signal is trusted forever, not for 45s");
    }

    #[test]
    fn another_osk_owning_the_input_method_shows_rather_than_hides() {
        // The postmarketOS/Phosh-with-squeekboard case. `active` is false
        // and will stay false forever; hiding on it is the reported bug.
        let mut p = FocusPolicy::default();
        let d = p.decide(0, dead());
        assert!(d.visible);
        assert_eq!(d.reason, Reason::InputMethodTakenByAnotherClient);
        assert!(d.reason.is_degraded(), "the user needs to be told, not just shown");
        // Immediately, not after the grace window — this one is
        // deterministic evidence, so waiting would be 45s of nothing for
        // no gain.
        assert!(p.decide(1, dead()).visible);
    }

    #[test]
    fn a_stale_active_on_a_dead_object_is_not_believed_either_way() {
        // Ordering test: `unavailable` is checked first, so a dead object
        // still carrying active=true reports the actionable reason rather
        // than the reassuring one. Without the ordering this passes
        // `visible` anyway and says "field focused", which sends whoever
        // reads the status line looking in the wrong place.
        let mut p = FocusPolicy::default();
        let d = p.decide(0, FocusSource::InputMethod { active: true, unavailable: true });
        assert_eq!(d.reason, Reason::InputMethodTakenByAnotherClient);
        assert!(!p.ever_activated(), "a dead object's activate is not an activate");
    }

    #[test]
    fn silence_past_the_grace_window_shows_the_keyboard() {
        let mut p = FocusPolicy::default();
        assert!(!p.decide(0, im(false)).visible);
        assert!(!p.decide(DEFAULT_GRACE_MS - 1, im(false)).visible, "still waiting");
        let d = p.decide(DEFAULT_GRACE_MS, im(false));
        assert!(d.visible);
        assert_eq!(d.reason, Reason::NoActivateWithinGrace);
    }

    #[test]
    fn the_grace_clock_starts_at_first_sight_not_at_epoch() {
        // `now_ms` is monotonic but its origin is the caller's. If the
        // policy compared against 0 it would fire on the first frame of
        // any app whose clock did not start at zero — which is every app
        // using egui's `input.time` after a warm start.
        let mut p = FocusPolicy::default();
        assert!(!p.decide(9_000_000, im(false)).visible, "first frame is not a timeout");
        assert!(!p.decide(9_000_000 + DEFAULT_GRACE_MS - 1, im(false)).visible);
        assert!(p.decide(9_000_000 + DEFAULT_GRACE_MS, im(false)).visible);
    }

    #[test]
    fn a_late_first_activate_takes_the_signal_back() {
        // The grace fallback must not be a one-way door: a compositor
        // that was merely slow gets its authority back, and normal
        // auto-hide resumes. Otherwise one slow boot means the keyboard
        // never hides again for the rest of the session.
        let mut p = FocusPolicy::default();
        // The first call is what STARTS the grace clock, so the timeout
        // is measured from here, not from t=0. Written without this
        // frame the test failed, and it was the test that was wrong —
        // `the_grace_clock_starts_at_first_sight_not_at_epoch` pins the
        // behaviour this one had misremembered.
        assert!(!p.decide(0, im(false)).visible);
        assert!(p.decide(DEFAULT_GRACE_MS, im(false)).visible);
        assert!(p.decide(DEFAULT_GRACE_MS + 1, im(true)).visible);
        let d = p.decide(DEFAULT_GRACE_MS + 2, im(false));
        assert!(!d.visible, "signal proved itself; auto-hide resumes");
        assert_eq!(d.reason, Reason::Unfocused);
    }

    #[test]
    fn auto_hide_off_beats_every_other_rule() {
        let mut p = FocusPolicy::new(AutoHide::Never, Some(DEFAULT_GRACE_MS));
        for src in [im(false), im(true), dead(), FocusSource::None] {
            let d = p.decide(0, src);
            assert!(d.visible, "{src:?}");
            assert_eq!(d.reason, Reason::AutoHideDisabled, "{src:?}");
        }
    }

    #[test]
    fn no_signal_at_all_means_visible() {
        // The desktop/Preview default, which must not change: this is
        // what makes the front-end runnable without a compositor.
        let mut p = FocusPolicy::default();
        let d = p.decide(0, FocusSource::None);
        assert!(d.visible);
        assert_eq!(d.reason, Reason::NoFocusSignal);
    }

    #[test]
    fn the_grace_fallback_can_be_switched_off_without_disabling_auto_hide() {
        let mut p = FocusPolicy::new(AutoHide::Follow, None);
        let d = p.decide(DEFAULT_GRACE_MS * 100, im(false));
        assert!(!d.visible);
        assert_eq!(d.reason, Reason::AwaitingFirstActivate);
    }

    #[test]
    fn config_none_means_auto_hide_on() {
        // `config/mod.rs` documents None as Some(true). Asserted here
        // because reading it as "unset, so leave it visible" would invert
        // the default for every user who has never touched the setting.
        assert_eq!(AutoHide::from_config(None), AutoHide::Follow);
        assert_eq!(AutoHide::from_config(Some(true)), AutoHide::Follow);
        assert_eq!(AutoHide::from_config(Some(false)), AutoHide::Never);
    }

    #[test]
    fn a_typo_in_the_grace_env_var_keeps_the_fallback() {
        // The failure this guards is silent: `=disabled` and `=disable`
        // differ by one character and by whether the keyboard can
        // recover on a broken compositor.
        assert_eq!(grace_from_env(None), Some(DEFAULT_GRACE_MS));
        assert_eq!(grace_from_env(Some("")), Some(DEFAULT_GRACE_MS));
        assert_eq!(grace_from_env(Some("disable")), Some(DEFAULT_GRACE_MS));
        assert_eq!(grace_from_env(Some("banana")), Some(DEFAULT_GRACE_MS));
        assert_eq!(grace_from_env(Some("disabled")), None);
        assert_eq!(grace_from_env(Some("off")), None);
        assert_eq!(grace_from_env(Some(" 2500 ")), Some(2_500));
        assert_eq!(grace_from_env(Some("0")), Some(0), "0 means show immediately");
    }

    #[test]
    fn grace_zero_shows_on_the_very_first_silent_frame() {
        let mut p = FocusPolicy::new(AutoHide::Follow, Some(0));
        let d = p.decide(12_345, im(false));
        assert!(d.visible);
        assert_eq!(d.reason, Reason::NoActivateWithinGrace);
    }
}
