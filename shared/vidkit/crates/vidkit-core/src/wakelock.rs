//! Keeping the screen awake during playback — and letting go.
//!
//! A phone blanks its display after 15 or 30 seconds of no input, and a video
//! player that does not say "I am using this" gets blanked mid-film. So an
//! inhibit is necessary.
//!
//! The interesting half is the release. An inhibit held after playback stops
//! keeps the screen and the GPU awake indefinitely, which is a worse battery
//! bug than the one it solves — the same shape as a repaint deadline nobody
//! clears, and just as invisible on a desk. Every path out of playback has to
//! drop it: pause, end of stream, an error, the window going hidden, and the
//! process exiting.
//!
//! # Audio-only is deliberately not a display inhibit
//!
//! Music and podcasts play with the screen off, and that is the whole point of
//! them. Holding the display awake for an audio-only stream would burn the
//! panel — the largest single draw on a phone — for something nobody is
//! looking at. [`Activity::AudioOnly`] therefore inhibits *suspend* and not the
//! display, which is what the compositor protocols distinguish and what most
//! players get wrong.
//!
//! The backend is a trait so the policy is testable with no compositor. A real
//! one is a thin wrapper over the wlr idle-inhibit protocol on wlroots
//! (postmarketOS/Phosh), or `org.freedesktop.ScreenSaver`'s `Inhibit`/
//! `UnInhibit` on a desktop session — neither of which is written here, and
//! neither of which should contain any of the logic below.

use std::time::{Duration, Instant};

/// What the player is doing, from the display's point of view.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Activity {
    /// Nothing playing.
    Idle,
    /// Video is playing and visible.
    Watching,
    /// Playing, but there is nothing to look at — audio-only, or the window is
    /// hidden or fully occluded.
    AudioOnly,
}

/// What the backend is being asked to hold.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Lock {
    /// Keep the display on.
    Display,
    /// Let the display blank, but do not suspend the machine.
    Suspend,
}

#[derive(Debug)]
pub enum InhibitError {
    /// No compositor interface. Not fatal: playback continues and the screen
    /// may blank, which is better than refusing to play.
    Unavailable(String),
    Backend(String),
}

impl std::fmt::Display for InhibitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InhibitError::Unavailable(s) => write!(f, "no inhibitor: {s}"),
            InhibitError::Backend(s) => write!(f, "inhibitor: {s}"),
        }
    }
}

/// The compositor side. A real implementation holds a protocol object; this
/// trait exists so none of the policy above depends on one.
pub trait Inhibitor {
    /// Take a lock, replacing any currently held.
    fn acquire(&mut self, lock: Lock, reason: &str) -> Result<(), InhibitError>;
    /// Drop whatever is held. Must be idempotent — it is called on every exit
    /// path including `Drop`, and a backend that errors on a double release
    /// turns a tidy shutdown into a log full of noise.
    fn release(&mut self) -> Result<(), InhibitError>;
}

/// Decides when to hold and when to let go.
///
/// Holds no compositor state itself, so a lost connection is the backend's
/// problem and the policy stays testable.
pub struct WakeLock<I: Inhibitor> {
    inhibitor: I,
    held: Option<Lock>,
    /// Refuse to re-acquire more often than this. Scrubbing toggles play state
    /// many times a second, and a compositor round trip per toggle is both slow
    /// and, on some of them, a flicker.
    pub min_interval: Duration,
    last_change: Option<Instant>,
    pending: Option<Lock>,
    pub changes: u32,
    pub last_error: Option<String>,
}

impl<I: Inhibitor> WakeLock<I> {
    pub fn new(inhibitor: I) -> Self {
        Self {
            inhibitor,
            held: None,
            min_interval: Duration::from_millis(500),
            last_change: None,
            pending: None,
            changes: 0,
            last_error: None,
        }
    }

    pub fn held(&self) -> Option<Lock> {
        self.held
    }

    pub fn inhibitor(&self) -> &I {
        &self.inhibitor
    }

    /// The lock an activity calls for.
    pub fn wanted(activity: Activity) -> Option<Lock> {
        match activity {
            Activity::Idle => None,
            Activity::Watching => Some(Lock::Display),
            // Not Display: see the module docs. Music with the screen off is
            // the point of music.
            Activity::AudioOnly => Some(Lock::Suspend),
        }
    }

    /// Bring the held lock in line with `activity`.
    ///
    /// `now` is passed in rather than read from the clock so the rate limit is
    /// testable without sleeping.
    pub fn update(&mut self, activity: Activity, now: Instant) {
        let want = Self::wanted(activity);
        if want == self.held {
            self.pending = None;
            return;
        }

        // Releasing is never rate limited. Holding the screen awake by accident
        // costs battery continuously; letting go a moment early costs nothing,
        // so the two directions are deliberately not symmetric.
        if want.is_none() {
            self.apply(None, now);
            return;
        }

        if let Some(last) = self.last_change {
            if now.saturating_duration_since(last) < self.min_interval {
                // Remembered, not dropped: the state that was asked for last
                // must eventually be reached, or a scrub that ends on "playing"
                // leaves the screen free to blank.
                self.pending = Some(want.expect("checked above"));
                return;
            }
        }
        self.apply(want, now);
    }

    /// Apply a rate-limited change that has come due. Call periodically.
    pub fn tick(&mut self, now: Instant) {
        let Some(want) = self.pending else { return };
        let due = match self.last_change {
            Some(last) => now.saturating_duration_since(last) >= self.min_interval,
            None => true,
        };
        if due {
            self.pending = None;
            self.apply(Some(want), now);
        }
    }

    fn apply(&mut self, want: Option<Lock>, now: Instant) {
        let result = match want {
            Some(lock) => self.inhibitor.acquire(lock, "video playback"),
            None => self.inhibitor.release(),
        };
        match result {
            Ok(()) => {
                self.held = want;
                self.last_error = None;
            }
            Err(e) => {
                // A missing inhibitor must not stop playback: the screen may
                // blank, which is worse than nothing and much better than
                // refusing to play. Recorded so a UI can say why.
                self.last_error = Some(e.to_string());
                // Treat a failed acquire as not held, so the next update
                // retries rather than believing a lock it never got.
                self.held = None;
            }
        }
        self.last_change = Some(now);
        self.changes += 1;
        self.pending = None;
    }

    /// Let go, unconditionally.
    pub fn release_now(&mut self) {
        if self.held.is_some() || self.pending.is_some() {
            let _ = self.inhibitor.release();
            self.held = None;
            self.pending = None;
        }
    }
}

impl<I: Inhibitor> Drop for WakeLock<I> {
    /// The backstop. A leaked inhibit outlives the player and keeps the screen
    /// awake until the session ends, which is the worst version of this bug
    /// because nothing on screen suggests a video player is responsible.
    fn drop(&mut self) {
        if self.held.is_some() {
            let _ = self.inhibitor.release();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default, Debug)]
    struct Log {
        acquired: Vec<Lock>,
        releases: u32,
        fail_acquire: bool,
    }

    #[derive(Clone)]
    struct MockInhibitor(Arc<Mutex<Log>>);

    impl Inhibitor for MockInhibitor {
        fn acquire(&mut self, lock: Lock, _reason: &str) -> Result<(), InhibitError> {
            let mut l = self.0.lock().unwrap();
            if l.fail_acquire {
                return Err(InhibitError::Unavailable("no compositor".into()));
            }
            l.acquired.push(lock);
            Ok(())
        }
        fn release(&mut self) -> Result<(), InhibitError> {
            self.0.lock().unwrap().releases += 1;
            Ok(())
        }
    }

    fn rig() -> (WakeLock<MockInhibitor>, Arc<Mutex<Log>>) {
        let log = Arc::new(Mutex::new(Log::default()));
        (WakeLock::new(MockInhibitor(log.clone())), log)
    }

    fn t(ms: u64) -> Instant {
        // A fixed base so the arithmetic is exact and nothing sleeps.
        static BASE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *BASE.get_or_init(Instant::now) + Duration::from_millis(ms)
    }

    #[test]
    fn watching_holds_the_display() {
        let (mut w, log) = rig();
        w.update(Activity::Watching, t(0));
        assert_eq!(w.held(), Some(Lock::Display));
        assert_eq!(log.lock().unwrap().acquired, vec![Lock::Display]);
    }

    /// The bug this module is mostly about. Every path out of playback must let
    /// go, or the screen stays awake indefinitely.
    #[test]
    fn every_exit_from_playback_releases() {
        // Going idle RELEASES; going audio-only REPLACES the display lock with
        // a weaker one. Both stop holding the screen awake, which is the claim,
        // but only the first reaches `release` — asserting a release for both
        // was asserting the mechanism rather than the effect.
        let (mut w, log) = rig();
        w.update(Activity::Watching, t(0));
        w.update(Activity::Idle, t(10_000));
        assert_eq!(w.held(), None, "still holding after going idle");
        assert_eq!(log.lock().unwrap().releases, 1);

        let (mut w, log) = rig();
        w.update(Activity::Watching, t(0));
        w.update(Activity::AudioOnly, t(10_000));
        assert_eq!(
            w.held(),
            Some(Lock::Suspend),
            "audio-only should drop to a suspend lock"
        );
        assert!(
            !log.lock().unwrap().acquired.contains(&Lock::Display)
                || log.lock().unwrap().acquired.last() == Some(&Lock::Suspend),
            "still holding the display for audio-only"
        );
    }

    /// Audio-only must not hold the DISPLAY. Music with the screen off is the
    /// point of music, and the panel is the biggest draw on a phone.
    #[test]
    fn audio_only_does_not_keep_the_screen_on() {
        let (mut w, log) = rig();
        w.update(Activity::AudioOnly, t(0));
        assert_eq!(w.held(), Some(Lock::Suspend));
        assert_eq!(
            log.lock().unwrap().acquired,
            vec![Lock::Suspend],
            "audio-only asked for a display lock"
        );
    }

    #[test]
    fn an_unchanged_activity_does_not_touch_the_compositor() {
        let (mut w, log) = rig();
        w.update(Activity::Watching, t(0));
        for i in 1..20 {
            w.update(Activity::Watching, t(i * 1000));
        }
        assert_eq!(
            log.lock().unwrap().acquired.len(),
            1,
            "re-acquired a lock it already held"
        );
    }

    /// Scrubbing toggles play state many times a second; a compositor round
    /// trip per toggle is slow and on some of them a visible flicker.
    #[test]
    fn rapid_toggling_is_rate_limited() {
        let (mut w, log) = rig();
        w.update(Activity::Watching, t(0));
        for i in 1..10 {
            // Toggle idle -> watching within the interval.
            w.update(Activity::Idle, t(i * 10));
            w.update(Activity::Watching, t(i * 10 + 5));
        }
        let acquired = log.lock().unwrap().acquired.len();
        assert!(acquired <= 3, "{acquired} acquires from one scrub");
    }

    /// ...but the state asked for last must still be reached, or a scrub that
    /// ends on "playing" leaves the screen free to blank.
    #[test]
    fn a_rate_limited_request_is_applied_later_not_dropped() {
        let (mut w, _log) = rig();
        // A prior change is needed for anything to be limited AGAINST: the
        // first version started from Idle, which is already the held state, so
        // `update` returned early, `last_change` stayed None and the next
        // request applied immediately. The test was rate-limiting nothing.
        w.update(Activity::Watching, t(0));
        w.update(Activity::Idle, t(5)); // releases, never limited
        w.update(Activity::Watching, t(10)); // inside the interval
        assert_eq!(w.held(), None, "should not have applied yet");

        w.tick(t(10_000));
        assert_eq!(
            w.held(),
            Some(Lock::Display),
            "the pending hold was dropped, not deferred"
        );
    }

    /// Releasing is never rate limited: holding the screen awake by mistake
    /// costs battery continuously, letting go early costs nothing.
    #[test]
    fn releasing_is_never_delayed() {
        let (mut w, log) = rig();
        w.update(Activity::Watching, t(0));
        w.update(Activity::Idle, t(1)); // well inside min_interval
        assert_eq!(w.held(), None, "a release was rate limited");
        assert_eq!(log.lock().unwrap().releases, 1);
    }

    /// No compositor is not a reason to stop playing.
    #[test]
    fn a_missing_inhibitor_does_not_stop_playback() {
        let (mut w, log) = rig();
        log.lock().unwrap().fail_acquire = true;
        w.update(Activity::Watching, t(0));
        assert_eq!(w.held(), None);
        assert!(w.last_error.is_some(), "the reason should be reportable");

        // And it retries once the interface appears, rather than believing a
        // lock it never got.
        log.lock().unwrap().fail_acquire = false;
        w.update(Activity::Watching, t(10_000));
        assert_eq!(w.held(), Some(Lock::Display));
    }

    /// The backstop: a leaked inhibit outlives the player and keeps the screen
    /// awake until the session ends, with nothing on screen to suggest why.
    #[test]
    fn dropping_the_wakelock_releases() {
        let log = Arc::new(Mutex::new(Log::default()));
        {
            let mut w = WakeLock::new(MockInhibitor(log.clone()));
            w.update(Activity::Watching, t(0));
            assert_eq!(log.lock().unwrap().releases, 0);
        }
        assert_eq!(
            log.lock().unwrap().releases,
            1,
            "the inhibit leaked past the player"
        );
    }

    #[test]
    fn release_now_is_idempotent() {
        let (mut w, log) = rig();
        w.update(Activity::Watching, t(0));
        w.release_now();
        w.release_now();
        w.release_now();
        assert_eq!(
            log.lock().unwrap().releases,
            1,
            "a tidy shutdown should not spam the compositor"
        );
    }

    #[test]
    fn the_wanted_lock_is_stated_once() {
        assert_eq!(WakeLock::<MockInhibitor>::wanted(Activity::Idle), None);
        assert_eq!(
            WakeLock::<MockInhibitor>::wanted(Activity::Watching),
            Some(Lock::Display)
        );
        assert_eq!(
            WakeLock::<MockInhibitor>::wanted(Activity::AudioOnly),
            Some(Lock::Suspend)
        );
    }
}
