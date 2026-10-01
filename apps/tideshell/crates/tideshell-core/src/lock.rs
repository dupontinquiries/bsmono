//! The lock screen as a state machine. Transport-free: it asks for
//! authentication (`LockAction::Authenticate`, done off-thread through PAM by
//! the caller) and says when the session may be released
//! (`LockAction::Unlock`, which the caller turns into
//! `ext_session_lock_v1.unlock_and_destroy`).
//!
//! Security properties, each pinned by a test:
//! * `Unlock` is emitted only after `on_auth_result(true)` for the attempt in
//!   flight; no input sequence reaches it otherwise, and stale results are
//!   ignored.
//! * The PIN leaves this struct at submit (moved into the action), so it is
//!   not held while authentication runs or after.
//! * Input is ignored while authenticating and during backoff; backoff
//!   escalates with consecutive failures.
//!
//! **Cover mode** (`require_auth == false`, the current default): no PIN, no
//! PAM. The swipe itself dismisses: a committed drag slides the cover away and
//! `Unlock` follows once it has settled. It is a visual cover, not security;
//! the PIN/PAM path is kept for when authentication arrives.
//!
//! Interaction: swipe up to reveal the PIN pad (a `DragTransition` that
//! follows the finger), type, submit; on success the lock slides away
//! (another transition) and only then is the session released.

use crate::anim::DragTransition;
use crate::Millis;

pub const MAX_PIN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockPhase {
    Unlocked,
    Locked,
    Authenticating,
    /// Auth succeeded; the exit animation is running.
    Unlocking,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockAction {
    /// Take the session lock (ext_session_lock_manager_v1.lock).
    Engage,
    /// Check this secret with PAM, then call `on_auth_result`.
    Authenticate(String),
    /// Release the session lock.
    Unlock,
}

/// Seconds to wait after the n-th consecutive failure.
pub fn backoff_secs(failures: u32) -> u64 {
    match failures {
        0..=4 => 0,
        5 => 30,
        6 => 60,
        _ => 300,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LockScreen {
    pub phase: LockPhase,
    /// 0 = clock/background, 1 = PIN pad shown.
    pub reveal: DragTransition,
    /// Unlock animation, 0 -> 1.
    pub exit: DragTransition,
    pin: String,
    pub failures: u32,
    backoff_until: Option<Millis>,
    pub message: Option<String>,
    /// false = cover mode: the swipe dismisses, no PIN (see module docs).
    pub require_auth: bool,
}

impl Default for LockScreen {
    fn default() -> Self {
        LockScreen {
            phase: LockPhase::Unlocked,
            reveal: DragTransition::default(),
            exit: DragTransition::default(),
            pin: String::new(),
            failures: 0,
            backoff_until: None,
            message: None,
            require_auth: false,
        }
    }
}

impl LockScreen {
    pub fn is_locked(&self) -> bool {
        self.phase != LockPhase::Unlocked
    }

    /// Digits typed so far (for drawing dots; never the digits themselves).
    pub fn pin_len(&self) -> usize {
        self.pin.len()
    }

    pub fn in_backoff(&self, now: Millis) -> bool {
        self.backoff_until.is_some_and(|t| now < t)
    }

    pub fn backoff_left_secs(&self, now: Millis) -> u64 {
        self.backoff_until.map(|t| t.saturating_sub(now).div_ceil(1000)).unwrap_or(0)
    }

    pub fn lock(&mut self) -> Vec<LockAction> {
        if self.is_locked() {
            return vec![];
        }
        self.phase = LockPhase::Locked;
        self.reveal = DragTransition::default();
        self.exit = DragTransition::default();
        self.pin.clear();
        self.message = None;
        vec![LockAction::Engage]
    }

    fn accepting_input(&self, now: Millis) -> bool {
        self.require_auth && self.phase == LockPhase::Locked && self.reveal.target() >= 1.0 && !self.in_backoff(now)
    }

    pub fn drag_reveal(&mut self, progress: f32, now: Millis) {
        if self.phase == LockPhase::Locked {
            self.reveal.drag(progress, now);
        }
    }

    /// Dismiss the cover without a swipe (the morse unlock). Cover mode only: with a PIN the cover
    /// opens through authentication, never through this.
    pub fn open_cover(&mut self) -> bool {
        if self.phase == LockPhase::Locked && !self.require_auth {
            self.reveal.animate_to(1.0);
            return true;
        }
        false
    }

    pub fn release_reveal(&mut self) {
        if self.phase == LockPhase::Locked {
            if self.reveal.release() < 1.0 {
                self.pin.clear(); // pad hidden again: drop partial input
            }
        }
    }

    pub fn digit(&mut self, d: char, now: Millis) {
        if d.is_ascii_digit() && self.accepting_input(now) && self.pin.len() < MAX_PIN {
            self.pin.push(d);
            self.message = None;
        }
    }

    pub fn backspace(&mut self, now: Millis) {
        if self.accepting_input(now) {
            self.pin.pop();
        }
    }

    pub fn submit(&mut self, now: Millis) -> Vec<LockAction> {
        if !self.accepting_input(now) || self.pin.is_empty() {
            return vec![];
        }
        self.phase = LockPhase::Authenticating;
        self.message = None;
        vec![LockAction::Authenticate(std::mem::take(&mut self.pin))]
    }

    /// The result of the attempt started by `submit`. Ignored unless one is
    /// in flight.
    pub fn on_auth_result(&mut self, ok: bool, now: Millis) {
        if self.phase != LockPhase::Authenticating {
            return;
        }
        if ok {
            self.failures = 0;
            self.backoff_until = None;
            self.phase = LockPhase::Unlocking;
            self.exit.animate_to(1.0);
        } else {
            self.failures += 1;
            let wait = backoff_secs(self.failures);
            self.backoff_until = (wait > 0).then(|| now + wait * 1000);
            self.message = Some(if wait > 0 { format!("Too many attempts. Try again in {wait} s") } else { "Wrong PIN".into() });
            self.phase = LockPhase::Locked;
        }
    }

    pub fn tick(&mut self, dt_ms: f32, now: Millis) -> Vec<LockAction> {
        self.reveal.tick(dt_ms);
        self.exit.tick(dt_ms);
        if self.backoff_until.is_some_and(|t| now >= t) {
            self.backoff_until = None;
            self.message = None;
        }
        // Cover mode: the committed swipe IS the unlock, once it has settled.
        if !self.require_auth && self.phase == LockPhase::Locked && self.reveal.is_settled() && self.reveal.progress >= 1.0 {
            self.phase = LockPhase::Unlocked;
            return vec![LockAction::Unlock];
        }
        if self.phase == LockPhase::Unlocking && self.exit.is_settled() && self.exit.progress >= 1.0 {
            self.phase = LockPhase::Unlocked;
            return vec![LockAction::Unlock];
        }
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_mode_swipe_dismisses_short_swipe_springs_back_no_auth_ever() {
        let mut l = LockScreen::default();
        assert!(!l.require_auth, "cover mode is the default for now");
        l.lock();
        for i in 0..=5 {
            l.drag_reveal(i as f32 * 0.03, i * 60); // short, slow
        }
        l.release_reveal();
        let mut acts = vec![];
        for i in 0..80 {
            acts.extend(l.tick(11.1, 1000 + i * 11));
        }
        assert!(acts.is_empty() && l.is_locked() && l.reveal.progress == 0.0, "sprang back: {acts:?}");
        for i in 0..=10 {
            l.drag_reveal(i as f32 * 0.06, 3000 + i * 11); // flick
        }
        l.release_reveal();
        type_pin(&mut l, "1234", 3200);
        assert!(l.submit(3200).is_empty(), "no PIN path in cover mode");
        for i in 0..80 {
            acts.extend(l.tick(11.1, 3200 + i * 11));
        }
        assert_eq!(acts, vec![LockAction::Unlock]);
        assert!(!l.is_locked());
    }

    fn revealed() -> LockScreen {
        let mut l = LockScreen { require_auth: true, ..LockScreen::default() };
        assert_eq!(l.lock(), vec![LockAction::Engage]);
        for i in 0..=10 {
            l.drag_reveal(i as f32 / 10.0, i * 20);
        }
        l.release_reveal();
        for _ in 0..60 {
            l.tick(11.1, 1000);
        }
        l
    }

    fn type_pin(l: &mut LockScreen, pin: &str, now: Millis) {
        for c in pin.chars() {
            l.digit(c, now);
        }
    }

    #[test]
    fn correct_pin_unlocks_only_after_auth_and_the_exit_animation() {
        let mut l = revealed();
        type_pin(&mut l, "2468", 2000);
        assert_eq!(l.pin_len(), 4);
        assert_eq!(l.submit(2000), vec![LockAction::Authenticate("2468".into())]);
        assert_eq!(l.pin_len(), 0, "the PIN left the model at submit");
        assert_eq!(l.phase, LockPhase::Authenticating);
        assert!(l.tick(11.1, 2011).is_empty(), "no unlock without an auth result");
        l.on_auth_result(true, 2100);
        let mut unlocked = vec![];
        for i in 0..100 {
            unlocked.extend(l.tick(11.1, 2100 + i * 11));
        }
        assert_eq!(unlocked, vec![LockAction::Unlock], "exactly once, after the slide-away");
        assert!(!l.is_locked());
    }

    #[test]
    fn no_input_sequence_unlocks_without_auth() {
        let mut l = revealed();
        let mut acts = vec![];
        for i in 0..500u64 {
            let now = 2000 + i * 7;
            match i % 7 {
                0..=3 => l.digit(char::from(b'0' + (i % 10) as u8), now),
                4 => l.backspace(now),
                5 => acts.extend(l.submit(now)),
                _ => l.on_auth_result(false, now),
            }
            acts.extend(l.tick(11.1, now));
        }
        assert!(!acts.contains(&LockAction::Unlock));
        assert!(l.is_locked());
    }

    #[test]
    fn wrong_pin_backs_off_after_five_and_input_is_ignored_meanwhile() {
        let mut l = revealed();
        let mut now = 2000;
        for n in 1..=4 {
            type_pin(&mut l, "0000", now);
            l.submit(now);
            l.on_auth_result(false, now);
            assert_eq!(l.message.as_deref(), Some("Wrong PIN"), "failure {n}");
            now += 1000;
        }
        type_pin(&mut l, "0000", now);
        l.submit(now);
        l.on_auth_result(false, now);
        assert!(l.in_backoff(now + 1000) && l.backoff_left_secs(now) == 30);
        type_pin(&mut l, "1234", now + 1000);
        assert_eq!(l.pin_len(), 0, "digits ignored during backoff");
        assert!(l.submit(now + 1000).is_empty());
        l.tick(11.1, now + 30_000);
        assert!(!l.in_backoff(now + 30_000) && l.message.is_none());
        type_pin(&mut l, "1", now + 30_000);
        assert_eq!(l.pin_len(), 1, "accepted again after the backoff");
    }

    #[test]
    fn stale_and_duplicate_auth_results_are_ignored() {
        let mut l = revealed();
        l.on_auth_result(true, 2000);
        assert_eq!(l.phase, LockPhase::Locked, "no attempt in flight: ignored");
        type_pin(&mut l, "1", 2000);
        l.submit(2000);
        l.on_auth_result(false, 2001);
        l.on_auth_result(true, 2002);
        assert_eq!(l.phase, LockPhase::Locked, "second result for the same attempt ignored");
    }

    #[test]
    fn input_needs_the_pad_revealed_and_hiding_it_drops_partial_input() {
        let mut l = LockScreen { require_auth: true, ..LockScreen::default() };
        l.lock();
        type_pin(&mut l, "12", 0);
        assert_eq!(l.pin_len(), 0, "pad not revealed");
        let mut l = revealed();
        type_pin(&mut l, "12", 2000);
        for i in 0..=10 {
            l.drag_reveal(1.0 - i as f32 / 10.0 * 0.9, 3000 + i * 20);
        }
        l.release_reveal();
        assert_eq!(l.pin_len(), 0);
    }

    #[test]
    fn reveal_follows_the_finger() {
        let mut l = LockScreen::default();
        l.lock();
        l.drag_reveal(0.37, 0);
        assert_eq!(l.reveal.progress, 0.37);
        assert!(l.lock().is_empty(), "locking twice is a no-op");
    }

    #[test]
    fn backoff_schedule() {
        assert_eq!((backoff_secs(4), backoff_secs(5), backoff_secs(6), backoff_secs(9)), (0, 30, 60, 300));
    }
}
