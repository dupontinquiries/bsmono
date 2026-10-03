//! Screen sleep: power key -> fade to black -> panel off (system stays awake,
//! so touches still arrive) -> a deliberate triple tap, or the power key,
//! wakes it with a fade back in.
//!
//! While fading out or off, every touch belongs to the wake detector and
//! never reaches an app: the caller maps a fullscreen, fully opaque,
//! input-taking surface for that. Waking optionally lands on the lock screen
//! (`lock_on_sleep`), the phone convention.
//!
//! With a proximity sensor, "near" means the screen is covered (a pocket, a
//! face, a table): the wake detector is off and touches are ignored while
//! it lasts, plus POCKET_HOLD_MS after it clears so the sensor's own lag
//! cannot let a pocket tap through.
//!
//! Two other paths lead to the same Off, so there is ONE screen-off and one
//! way back:
//! * **Idle** (`idle`): the shell asks the compositor for an idle notification
//!   (ext-idle-notify, which honours inhibitors like a playing video) and
//!   sleeps exactly as the power key does.
//! * **The compositor blanking on its own** (`display_reported(false)`, e.g.
//!   cosmic-idle's screen-off timer): adopted as Off, so touches are captured
//!   and the triple tap is what wakes it. And the panel coming back on while
//!   Off WITHOUT a triple tap or the power key — cosmic-idle turns outputs on
//!   at the first input after it blanked them, i.e. a single pocket touch —
//!   is refused: the answer is `DisplayPower(false)` again.
//!
//! The triple-tap detector is strict because a pocket produces touches: a tap
//! is short (TAP_MAX_MS) and still (TAP_SLOP); three must land within
//! TRIPLE_WINDOW_MS of the first touch and within CLUSTER px of the first
//! tap. A long press, a swipe, taps spread across the screen or a slow
//! sequence never wake it.

use crate::Millis;

/// Screen fades, by day. In the evening and at night each gets EVENING_EXTRA_MS more: a slow
/// ramp is kinder to dark-adapted eyes (see `Sleep::evening`).
pub const FADE_OUT_MS: f32 = 1000.0;
pub const FADE_IN_MS: f32 = 1000.0;
pub const EVENING_EXTRA_MS: f32 = 1000.0;
pub const TAP_MAX_MS: Millis = 200;
pub const TAP_SLOP: f32 = 24.0;
pub const TRIPLE_WINDOW_MS: Millis = 700;
pub const CLUSTER: f32 = 90.0;
pub const POCKET_HOLD_MS: Millis = 800;
/// A panel lit by someone else while we are Off stays lit (black, capturing taps) this long after
/// its last touch, then is turned off ONCE. Refusing at once made every touch event of a swipe
/// light and blank the panel again, ~5 ms apart: 132 times in one boot, with a DSI command
/// timeout in the kernel at the same instant, and the triple tap never got to complete.
pub const RELIGHT_GRACE_MS: Millis = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Awake,
    FadingOut,
    Off,
    FadingIn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepAction {
    /// zwlr_output_power_v1.set_mode(on/off).
    DisplayPower(bool),
    /// Engage the lock screen (on sleep, when lock_on_sleep).
    Lock,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sleep {
    pub phase: Phase,
    /// 0 = fully visible, 1 = black.
    pub black: f32,
    pub lock_on_sleep: bool,
    /// Completed taps in the current sequence: (time of touch-down, x, y).
    taps: Vec<(Millis, f32, f32)>,
    /// The touch currently down: (slot, down time, x, y, moved too far).
    down: Option<(i32, Millis, f32, f32, bool)>,
    /// Proximity "near" (covered) and when it last cleared.
    near: bool,
    near_cleared_at: Option<Millis>,
    /// Times the compositor blanked the panel on its own (adopted as Off).
    pub external_blanks: u32,
    /// Times the panel was lit while Off without a triple tap (turned off again).
    pub refused_wakes: u32,
    /// The panel was lit by someone else while Off, and the time of the last touch since. While
    /// `Some`, the panel is on but black (OLED: no light, almost no power) and the wake detector
    /// keeps running; `quiet` turns it off again after RELIGHT_GRACE_MS.
    lit_at: Option<Millis>,
    /// The bedside clock is lighting the panel on purpose (nightclock.rs):
    /// a lit report now is ours, not an intruder's.
    pub glance: bool,
    /// Evening or night (set by the shell from the clock): the fades are longer.
    pub evening: bool,
}

impl Default for Sleep {
    fn default() -> Self {
        Sleep { phase: Phase::Awake, black: 0.0, lock_on_sleep: true, taps: Vec::new(), down: None, near: false, near_cleared_at: None, external_blanks: 0, refused_wakes: 0, lit_at: None, glance: false, evening: false }
    }
}

impl Sleep {
    pub fn fade_out_ms(&self) -> f32 { FADE_OUT_MS + if self.evening { EVENING_EXTRA_MS } else { 0.0 } }
    pub fn fade_in_ms(&self) -> f32 { FADE_IN_MS + if self.evening { EVENING_EXTRA_MS } else { 0.0 } }

    /// Whether touches must be swallowed (opaque input surface mapped).
    pub fn captures_input(&self) -> bool {
        matches!(self.phase, Phase::FadingOut | Phase::Off)
    }

    /// The proximity sensor: near = covered.
    pub fn set_near(&mut self, near: bool, now: Millis) {
        if self.near && !near {
            self.near_cleared_at = Some(now);
        }
        self.near = near;
        if near {
            self.reset_taps();
        }
    }

    /// Covered right now, or cleared less than POCKET_HOLD_MS ago.
    pub fn is_covered(&self, now: Millis) -> bool {
        self.near || self.near_cleared_at.is_some_and(|t| now.saturating_sub(t) < POCKET_HOLD_MS)
    }

    pub fn is_animating(&self) -> bool {
        matches!(self.phase, Phase::FadingOut | Phase::FadingIn)
    }

    /// The user has been idle for the configured time: the same as the power
    /// key from Awake, and nothing otherwise (idle while asleep means nothing).
    pub fn idle(&mut self) -> Vec<SleepAction> {
        match self.phase {
            Phase::Awake => self.power_key(),
            _ => vec![],
        }
    }

    /// The compositor's report of the panel, whoever changed it. See the module
    /// doc: blanking we did not ask for is adopted as Off; the panel lit while
    /// we are Off (not via `wake`, which moves to FadingIn first) is turned off
    /// again.
    pub fn display_reported(&mut self, on: bool, now: Millis) -> Vec<SleepAction> {
        match (on, self.phase) {
            (false, Phase::Awake | Phase::FadingIn) => {
                self.phase = Phase::Off;
                self.black = 1.0;
                self.reset_taps();
                self.external_blanks += 1;
                if self.lock_on_sleep {
                    vec![SleepAction::Lock]
                } else {
                    vec![]
                }
            }
            (true, Phase::Off) if !self.glance => {
                // Stay black and keep listening; `quiet` blanks the panel once things settle.
                if self.lit_at.is_none() {
                    self.refused_wakes += 1;
                }
                self.lit_at = Some(now);
                vec![]
            }
            (false, Phase::Off) => {
                self.lit_at = None;
                vec![]
            }
            _ => vec![],
        }
    }

    pub fn power_key(&mut self) -> Vec<SleepAction> {
        match self.phase {
            Phase::Awake | Phase::FadingIn => {
                self.phase = Phase::FadingOut;
                self.reset_taps();
                if self.lock_on_sleep {
                    vec![SleepAction::Lock]
                } else {
                    vec![]
                }
            }
            Phase::FadingOut | Phase::Off => self.wake(),
        }
    }

    /// How long until `quiet` would blank the panel (None when nothing is pending), so a loop that
    /// sleeps while the screen is off can wake for it instead of firing up to its whole idle
    /// timeout late.
    pub fn quiet_in(&self, now: Millis) -> Option<Millis> {
        match (self.phase, self.lit_at) {
            (Phase::Off, Some(t)) if self.down.is_none() => Some((t + RELIGHT_GRACE_MS).saturating_sub(now)),
            _ => None,
        }
    }

    /// Called every tick with the clock: turn off a panel somebody else lit while we are Off,
    /// once, after RELIGHT_GRACE_MS without a touch (and never mid-touch).
    pub fn quiet(&mut self, now: Millis) -> Vec<SleepAction> {
        match (self.phase, self.lit_at) {
            (Phase::Off, Some(t)) if self.down.is_none() && now.saturating_sub(t) >= RELIGHT_GRACE_MS => {
                self.lit_at = None;
                vec![SleepAction::DisplayPower(false)]
            }
            _ => vec![],
        }
    }

    fn wake(&mut self) -> Vec<SleepAction> {
        self.lit_at = None;
        let was_off = self.phase == Phase::Off;
        self.phase = Phase::FadingIn;
        self.reset_taps();
        if was_off {
            vec![SleepAction::DisplayPower(true)]
        } else {
            vec![] // still fading out: the panel never went off
        }
    }

    fn reset_taps(&mut self) {
        self.taps.clear();
        self.down = None;
    }

    pub fn tick(&mut self, dt_ms: f32) -> Vec<SleepAction> {
        match self.phase {
            Phase::FadingOut => {
                self.black = (self.black + dt_ms / self.fade_out_ms()).min(1.0);
                if self.black >= 1.0 {
                    self.phase = Phase::Off;
                    return vec![SleepAction::DisplayPower(false)];
                }
            }
            Phase::FadingIn => {
                self.black = (self.black - dt_ms / self.fade_in_ms()).max(0.0);
                if self.black <= 0.0 {
                    self.phase = Phase::Awake;
                }
            }
            _ => {}
        }
        vec![]
    }

    /// Touch input while captured (fading out or off). Returns wake actions.
    pub fn touch_down(&mut self, slot: i32, x: f32, y: f32, now: Millis) -> Vec<SleepAction> {
        if !self.captures_input() || self.is_covered(now) {
            return vec![];
        }
        if self.lit_at.is_some() {
            self.lit_at = Some(now); // a touch keeps the lit black panel from being blanked under it
        }
        // A second finger means "not a deliberate tap sequence".
        if self.down.is_some() {
            self.reset_taps();
            return vec![];
        }
        if self.taps.first().is_some_and(|f| now.saturating_sub(f.0) > TRIPLE_WINDOW_MS) {
            self.taps.clear();
        }
        self.down = Some((slot, now, x, y, false));
        vec![]
    }

    pub fn touch_move(&mut self, slot: i32, x: f32, y: f32) {
        if let Some((s, _, x0, y0, moved)) = &mut self.down {
            if *s == slot && (x - *x0).hypot(y - *y0) > TAP_SLOP {
                *moved = true;
            }
        }
    }

    pub fn touch_up(&mut self, slot: i32, now: Millis) -> Vec<SleepAction> {
        let Some((s, t0, x, y, moved)) = self.down else { return vec![] };
        if s != slot {
            return vec![];
        }
        self.down = None;
        if self.lit_at.is_some() {
            self.lit_at = Some(now);
        }
        let is_tap = !moved && now.saturating_sub(t0) <= TAP_MAX_MS;
        let clustered = self.taps.first().is_none_or(|f| (x - f.1).hypot(y - f.2) <= CLUSTER);
        let in_window = self.taps.first().is_none_or(|f| now.saturating_sub(f.0) <= TRIPLE_WINDOW_MS);
        if !(is_tap && clustered && in_window) {
            // Anything else restarts the sequence; a valid tap can start a new one.
            self.taps.clear();
            if is_tap {
                self.taps.push((t0, x, y));
            }
            return vec![];
        }
        self.taps.push((t0, x, y));
        if self.taps.len() >= 3 {
            return self.wake();
        }
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asleep() -> Sleep {
        let mut s = Sleep::default();
        s.power_key();
        for _ in 0..100 {
            s.tick(11.1);
        }
        assert_eq!(s.phase, Phase::Off);
        s
    }

    fn tap(s: &mut Sleep, x: f32, y: f32, t: Millis, dur: Millis) -> Vec<SleepAction> {
        s.touch_down(0, x, y, t);
        s.touch_up(0, t + dur)
    }

    #[test]
    fn power_key_locks_fades_then_turns_the_panel_off() {
        let mut s = Sleep::default();
        assert_eq!(s.power_key(), vec![SleepAction::Lock]);
        assert!(s.captures_input(), "touches swallowed from the first frame of the fade");
        let mut off = vec![];
        let mut t = 0.0;
        while s.phase == Phase::FadingOut {
            off.extend(s.tick(11.1));
            t += 11.1;
            assert!(t < 1200.0);
        }
        assert_eq!(off, vec![SleepAction::DisplayPower(false)]);
        assert!((t - FADE_OUT_MS).abs() < 12.0, "fade took {t} ms");
        assert_eq!(s.black, 1.0);
    }

    #[test]
    fn evening_fades_take_a_second_longer_both_ways() {
        let take = |evening: bool| {
            let mut s = Sleep { evening, ..Sleep::default() };
            s.power_key();
            let mut out = 0.0f32;
            while s.phase == Phase::FadingOut { s.tick(10.0); out += 10.0; }
            s.power_key(); // wake
            let mut inn = 0.0f32;
            while s.phase == Phase::FadingIn { s.tick(10.0); inn += 10.0; }
            (out, inn)
        };
        let (d_out, d_in) = take(false);
        let (n_out, n_in) = take(true);
        assert!((d_out - 1000.0).abs() < 20.0 && (d_in - 1000.0).abs() < 20.0, "day {d_out}/{d_in}");
        assert!((n_out - 2000.0).abs() < 20.0 && (n_in - 2000.0).abs() < 20.0, "night {n_out}/{n_in}");
    }

    #[test]
    fn triple_tap_wakes_and_fades_in() {
        let mut s = asleep();
        assert!(tap(&mut s, 200.0, 500.0, 1000, 80).is_empty());
        assert!(tap(&mut s, 210.0, 505.0, 1200, 80).is_empty());
        assert_eq!(tap(&mut s, 195.0, 498.0, 1400, 80), vec![SleepAction::DisplayPower(true)]);
        assert_eq!(s.phase, Phase::FadingIn);
        for _ in 0..100 {
            s.tick(11.1);
        }
        assert_eq!((s.phase, s.black), (Phase::Awake, 0.0));
        assert!(!s.captures_input());
    }

    #[test]
    fn pocket_touches_do_not_wake() {
        // Double tap: no.
        let mut s = asleep();
        tap(&mut s, 200.0, 500.0, 0, 80);
        tap(&mut s, 200.0, 500.0, 200, 80);
        assert_eq!(s.phase, Phase::Off);
        // Too slow: three taps over 1.5 s.
        let mut s = asleep();
        for i in 0..3 {
            assert!(tap(&mut s, 200.0, 500.0, i * 500, 80).is_empty());
        }
        assert_eq!(s.phase, Phase::Off, "slow sequence");
        // Spread across the screen.
        let mut s = asleep();
        tap(&mut s, 50.0, 100.0, 0, 80);
        tap(&mut s, 350.0, 600.0, 150, 80);
        tap(&mut s, 200.0, 850.0, 300, 80);
        assert_eq!(s.phase, Phase::Off, "spread out");
        // Long presses.
        let mut s = asleep();
        for i in 0..3 {
            tap(&mut s, 200.0, 500.0, i * 200, 400);
        }
        assert_eq!(s.phase, Phase::Off, "presses, not taps");
        // A swipe in the middle breaks the sequence.
        let mut s = asleep();
        tap(&mut s, 200.0, 500.0, 0, 80);
        s.touch_down(0, 200.0, 500.0, 150);
        s.touch_move(0, 200.0, 580.0);
        s.touch_up(0, 220);
        tap(&mut s, 200.0, 500.0, 300, 80);
        assert_eq!(s.phase, Phase::Off, "swipe broke it");
    }

    #[test]
    fn a_covered_screen_never_wakes_on_taps_until_the_hold_after_it_clears() {
        let mut s = asleep();
        s.set_near(true, 1000);
        for i in 0..3 {
            tap(&mut s, 200.0, 500.0, 1100 + i * 200, 80);
        }
        assert_eq!(s.phase, Phase::Off, "in a pocket: no");
        s.set_near(false, 2000);
        for i in 0..3 {
            tap(&mut s, 200.0, 500.0, 2100 + i * 200, 80); // within the hold
        }
        assert_eq!(s.phase, Phase::Off, "just out of the pocket: still no");
        let t0 = 2000 + POCKET_HOLD_MS + 100;
        for i in 0..2 {
            tap(&mut s, 200.0, 500.0, t0 + i * 200, 80);
        }
        assert_eq!(tap(&mut s, 200.0, 500.0, t0 + 400, 80), vec![SleepAction::DisplayPower(true)], "after the hold: wakes");
        // The power key still works covered (a deliberate press).
        let mut s = asleep();
        s.set_near(true, 0);
        assert_eq!(s.power_key(), vec![SleepAction::DisplayPower(true)]);
    }

    #[test]
    fn a_failed_sequence_can_restart_cleanly() {
        let mut s = asleep();
        tap(&mut s, 200.0, 500.0, 0, 80);
        tap(&mut s, 200.0, 500.0, 2000, 80); // too late: starts a new sequence
        tap(&mut s, 200.0, 500.0, 2200, 80);
        assert_eq!(tap(&mut s, 200.0, 500.0, 2400, 80), vec![SleepAction::DisplayPower(true)]);
    }

    #[test]
    fn power_key_wakes_too_and_mid_fade_cancels_without_a_power_cycle() {
        let mut s = asleep();
        assert_eq!(s.power_key(), vec![SleepAction::DisplayPower(true)]);
        let mut s = Sleep::default();
        s.power_key();
        s.tick(100.0); // part-way through the fade
        assert!(s.power_key().is_empty(), "panel never went off: nothing to turn on");
        assert_eq!(s.phase, Phase::FadingIn);
    }

    #[test]
    fn idle_sleeps_like_the_power_key_and_only_from_awake() {
        let mut s = Sleep::default();
        assert_eq!(s.idle(), vec![SleepAction::Lock]);
        assert_eq!(s.phase, Phase::FadingOut);
        let mut s = asleep();
        assert!(s.idle().is_empty(), "idle while asleep: nothing");
        assert_eq!(s.phase, Phase::Off);
    }

    #[test]
    fn compositor_blanking_is_adopted_and_still_needs_a_triple_tap() {
        let mut s = Sleep::default();
        assert_eq!(s.display_reported(false, 0), vec![SleepAction::Lock], "cosmic-idle blanked us: lock, like a sleep");
        assert_eq!((s.phase, s.black), (Phase::Off, 1.0));
        assert!(s.captures_input(), "touches go to the wake detector, not the apps under a dark panel");
        assert!(tap(&mut s, 200.0, 500.0, 0, 80).is_empty());
        assert_eq!(s.phase, Phase::Off, "one tap does not wake");
        tap(&mut s, 200.0, 500.0, 150, 80);
        assert_eq!(tap(&mut s, 200.0, 500.0, 300, 80), vec![SleepAction::DisplayPower(true)]);
        assert_eq!(s.external_blanks, 1);
    }

    #[test]
    fn a_panel_lit_behind_our_back_while_off_stays_black_and_is_blanked_once_when_quiet() {
        let mut s = asleep();
        // The storm: the compositor reports "on" on every touch event, ~5 ms apart. None of them
        // may produce a power command.
        for k in 0..40 {
            assert!(s.display_reported(true, 1000 + k * 5).is_empty(), "report {k} must not fight the compositor");
        }
        assert_eq!(s.phase, Phase::Off);
        assert!(s.captures_input(), "still black and still listening for the triple tap");
        assert_eq!(s.refused_wakes, 1, "one intrusion, not forty");
        let last = 1000 + 39 * 5; // the last report
        // Nothing happens before the grace has run from the LAST report.
        assert!(s.quiet(last + RELIGHT_GRACE_MS - 1).is_empty());
        // ... then exactly one DisplayPower(false), and not again.
        assert_eq!(s.quiet(last + RELIGHT_GRACE_MS), vec![SleepAction::DisplayPower(false)]);
        assert!(s.quiet(last + RELIGHT_GRACE_MS + 10).is_empty());
        // The panel going off is a report that agrees with us.
        assert!(s.display_reported(false, 20_000).is_empty());
    }

    #[test]
    fn someone_tapping_the_black_screen_to_wake_it_is_never_blanked_under_their_finger() {
        // The wedge: taps every few seconds each lit the panel, a short re-blank timer fired between
        // two of them, and the compositor lit it again within a millisecond with brightness
        // commands in flight; the DSI link died for ~11 s. A power cycle must not land between taps
        // however spaced they are, so the panel stays lit (black, free on OLED) long after a touch.
        let mut s = asleep();
        let mut cmds = 0;
        let mut t = 1000u64;
        for _ in 0..8 {
            s.display_reported(true, t); // the compositor lights the panel on this touch
            s.touch_down(0, 100.0, 100.0, t);
            s.touch_up(0, t + 60);
            // every half second until the next tap, 12 s later, the loop asks the timer
            for k in 0..24 {
                cmds += s.quiet(t + 60 + k * 500).len();
            }
            t += 12_000;
        }
        assert_eq!(cmds, 0, "no power command between taps 12 s apart");
        // and once they stop, it is blanked exactly once
        let last = t - 12_000 + 60;
        assert!(s.quiet(last + RELIGHT_GRACE_MS - 1).is_empty());
        assert_eq!(s.quiet(last + RELIGHT_GRACE_MS), vec![SleepAction::DisplayPower(false)]);
    }

    #[test]
    fn the_quiet_timer_can_be_asked_when_it_is_due() {
        let mut s = asleep();
        assert_eq!(s.quiet_in(0), None, "nothing lit, nothing pending");
        s.display_reported(true, 1000);
        assert_eq!(s.quiet_in(1000), Some(RELIGHT_GRACE_MS));
        assert_eq!(s.quiet_in(1000 + 4000), Some(RELIGHT_GRACE_MS - 4000));
        assert_eq!(s.quiet_in(1000 + RELIGHT_GRACE_MS + 777), Some(0), "overdue means now");
        s.touch_down(0, 1.0, 1.0, 3000);
        assert_eq!(s.quiet_in(3000), None, "never while a finger is down");
        s.touch_up(0, 3050);
        assert_eq!(s.quiet_in(3050), Some(RELIGHT_GRACE_MS), "restarted by the touch");
        s.quiet(3050 + RELIGHT_GRACE_MS);
        assert_eq!(s.quiet_in(99_999), None, "done once blanked");
    }

    #[test]
    fn a_touch_keeps_the_lit_panel_from_being_blanked_and_a_triple_tap_still_wakes_it() {
        let mut s = asleep();
        s.display_reported(true, 0);
        // a finger is down at 4 s: the grace is measured from the touch, and never mid-touch
        s.touch_down(0, 200.0, 500.0, 4000);
        assert!(s.quiet(4000 + RELIGHT_GRACE_MS + 100).is_empty(), "never blank under a finger");
        s.touch_up(0, 4060);
        assert!(s.quiet(4060 + RELIGHT_GRACE_MS - 1).is_empty(), "the touch restarted the grace");
        // the triple tap completes while the panel is lit
        tap(&mut s, 200.0, 500.0, 5000, 80);
        tap(&mut s, 200.0, 500.0, 5150, 80);
        assert_eq!(tap(&mut s, 200.0, 500.0, 5300, 80), vec![SleepAction::DisplayPower(true)]);
        assert_eq!(s.phase, Phase::FadingIn);
        assert!(s.quiet(60_000).is_empty(), "a woken panel is never blanked by the quiet timer");
        // Reports that agree with us are no-ops.
        let mut s = asleep();
        assert!(s.display_reported(false, 0).is_empty());
        let mut s = Sleep::default();
        assert!(s.display_reported(true, 0).is_empty());
    }

    #[test]
    fn awake_touches_are_not_captured() {
        let mut s = Sleep::default();
        assert!(s.touch_down(0, 1.0, 1.0, 0).is_empty());
        assert!(!s.captures_input());
        s.lock_on_sleep = false;
        assert!(s.power_key().is_empty(), "no Lock when lock_on_sleep is off");
    }
}
