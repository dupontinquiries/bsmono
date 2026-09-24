//! focus-core — deciding what a voice-coil lens actuator is allowed to do.
//!
//! One actuator, several would-be users: autofocus, a manual focus slider,
//! and (because it makes an audible tick) a notification cue. They cannot all
//! write to it, so this crate owns the decision and the V4L2 shell above it
//! owns only the `VIDIOC_S_CTRL`.
//!
//! Pure: no file descriptors, no clock of its own. Every entry point takes
//! `now_ms` and returns what to write, so the whole policy is testable
//! against a fake clock.
//!
//! ## What the hardware forces
//!
//! **A spring-return VCM holds position with continuous current.** Resting at
//! the minimum costs nothing; holding at full travel dissipates the most,
//! permanently, against the lens stack. So idleness must return to rest —
//! [`Config::park_after_ms`] — and that is a thermal rule, not tidiness.
//!
//! **Moving during an exposure smears the focal plane.** Not motion blur that
//! stabilisation can undo: the focus distance itself changes mid-integration.
//! A capture lease therefore blocks everything, including autofocus, from the
//! moment the shutter is armed.
//!
//! **The actuator is rated for autofocus duty**, on the order of 10^5–10^6
//! moves, mostly short. Using it as a buzzer is far outside that, so cues get
//! a budget ([`Config::cue_duty`]) rather than good intentions.
//!
//! ## Measured on a Fairphone 5, 2026-09-23
//!
//! `dw9800k` at i2c 17-000e binds the mainline `dw9719` driver and exposes
//! `focus_absolute` 0..1023 on a Lens subdev beside `imx858` — the
//! ULTRA-WIDE, the only camera on that phone with an actuator at all. The
//! lens is audible across much of its travel rather than at one resonance,
//! which is why the cue limits here are about duty and amplitude rather than
//! about picking a frequency.

/// Who wants the lens. Higher ordinal wins; ties go to the incumbent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum User {
    /// An audible notification. Yields to everything.
    Cue,
    /// A person dragging a focus slider.
    Manual,
    /// The autofocus algorithm.
    Autofocus,
    /// A capture in progress. Nothing may move the lens.
    Capture,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    pub min: u16,
    pub max: u16,
    /// Return to `min` after this long with no request.
    pub park_after_ms: u64,
    /// Moves smaller than this are dropped: below the audible/visible
    /// threshold, and each one is an I2C transaction and a tick.
    pub dead_band: u16,
    /// Minimum gap between writes.
    pub min_interval_ms: u64,
    /// A cue burst may last at most this long,
    pub cue_max_burst_ms: u64,
    /// may use at most this fraction of any [`Config::cue_window_ms`],
    pub cue_duty: f32,
    pub cue_window_ms: u64,
    /// and may swing at most this far from rest.
    pub cue_amplitude: u16,
    /// Above this temperature, cues stop entirely.
    pub cue_max_temp_c: i32,
}

impl Default for Config {
    /// Deliberately timid. These are not tuned numbers — they are the
    /// numbers that keep the actuator inside its design duty while we learn
    /// what it can do. Loosen them with measurements, not with optimism.
    fn default() -> Config {
        Config {
            min: 0,
            max: 1023,
            park_after_ms: 3_000,
            dead_band: 8,
            min_interval_ms: 2,
            cue_max_burst_ms: 150,
            cue_duty: 0.02,
            cue_window_ms: 60_000,
            cue_amplitude: 256,
            cue_max_temp_c: 45,
        }
    }
}

/// Why a request did not move the lens. Every refusal names itself; a silent
/// no-op in a subsystem nobody can see is how this becomes unexplainable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// A capture holds the lens.
    Capture,
    /// Another user outranks this one.
    Outranked(User),
    /// Within the dead band of where it already is.
    TooSmall,
    /// Another write happened too recently.
    TooSoon,
    /// This cue would exceed the duty budget.
    CueBudget,
    /// Too hot for cues.
    TooHot(i32),
    /// Cue bursts are capped.
    CueTooLong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Write this value.
    Move(u16),
    /// Do nothing, for this reason.
    No(Refused),
}

#[derive(Debug, Clone)]
pub struct Focus {
    cfg: Config,
    pos: u16,
    holder: Option<User>,
    /// None until the first write: a zero here would make `now_ms == 0` look
    /// like "a write just happened" and refuse the very first move.
    last_write_ms: Option<u64>,
    last_request_ms: u64,
    /// Cue activity as (start, end) pairs inside the current window.
    cue_spans: Vec<(u64, u64)>,
    cue_started_ms: Option<u64>,
    temp_c: Option<i32>,
    parked: bool,
}

impl Focus {
    pub fn new(cfg: Config) -> Focus {
        Focus {
            pos: cfg.min,
            cfg,
            holder: None,
            last_write_ms: None,
            last_request_ms: 0,
            cue_spans: Vec::new(),
            cue_started_ms: None,
            temp_c: None,
            parked: true,
        }
    }

    pub fn position(&self) -> u16 {
        self.pos
    }
    pub fn holder(&self) -> Option<User> {
        self.holder
    }
    pub fn config(&self) -> &Config {
        &self.cfg
    }
    /// Latest reading from a camera-adjacent thermal zone.
    pub fn set_temperature(&mut self, c: i32) {
        self.temp_c = Some(c);
    }

    /// Take the lens for `user`. A capture always wins; otherwise the
    /// incumbent keeps it against an equal or lower rank.
    pub fn claim(&mut self, user: User, now_ms: u64) -> Result<(), Refused> {
        match self.holder {
            Some(h) if h == User::Capture && user != User::Capture => Err(Refused::Capture),
            Some(h) if h > user => Err(Refused::Outranked(h)),
            _ => {
                if user == User::Cue && self.cue_started_ms.is_none() {
                    self.cue_started_ms = Some(now_ms);
                }
                self.holder = Some(user);
                self.last_request_ms = now_ms;
                Ok(())
            }
        }
    }

    pub fn release(&mut self, user: User, now_ms: u64) {
        if self.holder == Some(user) {
            if user == User::Cue {
                if let Some(start) = self.cue_started_ms.take() {
                    self.cue_spans.push((start, now_ms));
                }
            }
            self.holder = None;
            self.last_request_ms = now_ms;
        }
    }

    /// Ask to move. The caller must hold the lens.
    pub fn request(&mut self, user: User, target: u16, now_ms: u64) -> Decision {
        if self.holder == Some(User::Capture) && user != User::Capture {
            return Decision::No(Refused::Capture);
        }
        if let Some(h) = self.holder {
            if h > user {
                return Decision::No(Refused::Outranked(h));
            }
        }
        if user == User::Cue {
            if let Some(t) = self.temp_c {
                if t >= self.cfg.cue_max_temp_c {
                    return Decision::No(Refused::TooHot(t));
                }
            }
            if let Some(start) = self.cue_started_ms {
                if now_ms.saturating_sub(start) > self.cfg.cue_max_burst_ms {
                    return Decision::No(Refused::CueTooLong);
                }
            }
            // >=, not >: a budget spent exactly to the limit is spent.
            if self.cue_used_ms(now_ms) >= (self.cfg.cue_window_ms as f32 * self.cfg.cue_duty) as u64 {
                return Decision::No(Refused::CueBudget);
            }
        }

        let target = self.clamp_for(user, target);
        if let Some(last) = self.last_write_ms {
            if now_ms.saturating_sub(last) < self.cfg.min_interval_ms {
                return Decision::No(Refused::TooSoon);
            }
        }
        if target.abs_diff(self.pos) < self.cfg.dead_band && !(self.parked && target != self.pos) {
            self.last_request_ms = now_ms;
            return Decision::No(Refused::TooSmall);
        }
        self.pos = target;
        self.parked = target == self.cfg.min;
        self.last_write_ms = Some(now_ms);
        self.last_request_ms = now_ms;
        Decision::Move(target)
    }

    /// A cue may only swing `cue_amplitude` from rest; everyone else gets the
    /// full range.
    fn clamp_for(&self, user: User, target: u16) -> u16 {
        let hi = if user == User::Cue {
            self.cfg.min.saturating_add(self.cfg.cue_amplitude).min(self.cfg.max)
        } else {
            self.cfg.max
        };
        target.clamp(self.cfg.min, hi)
    }

    fn cue_used_ms(&self, now_ms: u64) -> u64 {
        let floor = now_ms.saturating_sub(self.cfg.cue_window_ms);
        let closed: u64 = self.cue_spans.iter().filter(|(_, e)| *e >= floor).map(|(s, e)| e.saturating_sub((*s).max(floor))).sum();
        let open = self.cue_started_ms.map(|s| now_ms.saturating_sub(s.max(floor))).unwrap_or(0);
        closed + open
    }

    /// Call periodically. Returns a write when the lens has been idle long
    /// enough that holding current is no longer worth paying for.
    pub fn tick(&mut self, now_ms: u64) -> Option<u16> {
        self.cue_spans.retain(|(_, e)| *e + self.cfg.cue_window_ms >= now_ms);
        if self.holder.is_some() || self.parked {
            return None;
        }
        if now_ms.saturating_sub(self.last_request_ms) >= self.cfg.park_after_ms {
            self.pos = self.cfg.min;
            self.parked = true;
            self.last_write_ms = Some(now_ms);
            return Some(self.cfg.min);
        }
        None
    }

    /// What the shell must do before the process exits, or on a signal: put
    /// the lens back so nothing is left holding current.
    pub fn park_value(&self) -> u16 {
        self.cfg.min
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f() -> Focus {
        Focus::new(Config::default())
    }

    #[test]
    fn a_capture_freezes_the_lens_for_everyone() {
        let mut x = f();
        x.claim(User::Autofocus, 0).unwrap();
        assert_eq!(x.request(User::Autofocus, 500, 10), Decision::Move(500));
        // The shutter arms mid-sweep; autofocus must stop moving at once.
        x.claim(User::Capture, 20).unwrap();
        assert_eq!(x.request(User::Autofocus, 600, 30), Decision::No(Refused::Capture));
        assert_eq!(x.request(User::Cue, 100, 30), Decision::No(Refused::Capture));
        assert_eq!(x.position(), 500, "held where it was");
        // A capture may still refocus itself.
        assert_eq!(x.request(User::Capture, 520, 40), Decision::Move(520));
        x.release(User::Capture, 50);
        assert_eq!(x.request(User::Autofocus, 600, 60), Decision::Move(600));
    }

    #[test]
    fn rank_decides_and_the_incumbent_keeps_a_tie() {
        let mut x = f();
        x.claim(User::Cue, 0).unwrap();
        assert_eq!(x.claim(User::Autofocus, 1), Ok(()), "autofocus takes it from a cue");
        assert_eq!(x.claim(User::Cue, 2), Err(Refused::Outranked(User::Autofocus)));
        assert_eq!(x.request(User::Cue, 50, 3), Decision::No(Refused::Outranked(User::Autofocus)));
        assert_eq!(x.claim(User::Autofocus, 4), Ok(()), "same user re-claims");
        x.release(User::Autofocus, 5);
        assert_eq!(x.claim(User::Cue, 6), Ok(()));
    }

    #[test]
    fn idleness_returns_the_lens_to_rest() {
        // Holding position costs continuous current, so a lens left at 800
        // heats the module for as long as nobody notices.
        let mut x = f();
        x.claim(User::Manual, 0).unwrap();
        assert_eq!(x.request(User::Manual, 800, 0), Decision::Move(800));
        x.release(User::Manual, 10);
        assert_eq!(x.tick(1_000), None, "not yet");
        assert_eq!(x.tick(3_010), Some(0), "parked");
        assert_eq!(x.position(), 0);
        assert_eq!(x.tick(9_000), None, "and stays parked without rewriting");
    }

    #[test]
    fn a_held_lens_is_never_parked_underneath_its_holder() {
        let mut x = f();
        x.claim(User::Manual, 0).unwrap();
        x.request(User::Manual, 700, 0);
        assert_eq!(x.tick(600_000), None, "still held, so still where it was put");
        assert_eq!(x.position(), 700);
    }

    #[test]
    fn tiny_moves_and_fast_moves_are_dropped() {
        let mut x = f();
        x.claim(User::Manual, 0).unwrap();
        assert_eq!(x.request(User::Manual, 400, 0), Decision::Move(400));
        assert_eq!(x.request(User::Manual, 404, 100), Decision::No(Refused::TooSmall), "inside the dead band");
        assert_eq!(x.request(User::Manual, 420, 100), Decision::Move(420));
        assert_eq!(x.request(User::Manual, 500, 101), Decision::No(Refused::TooSoon), "1 ms after the last write");
        assert_eq!(x.request(User::Manual, 500, 103), Decision::Move(500));
    }

    #[test]
    fn cues_are_quiet_brief_and_rationed() {
        let mut x = f();
        x.claim(User::Cue, 0).unwrap();
        // Amplitude is clamped to a fraction of travel, whatever was asked.
        assert_eq!(x.request(User::Cue, 1023, 0), Decision::Move(256));
        // A burst may not run long.
        assert_eq!(x.request(User::Cue, 0, 200), Decision::No(Refused::CueTooLong));
        x.release(User::Cue, 200);
        // 200 ms spent of a 1200 ms budget (2 % of 60 s); spend the rest.
        x.claim(User::Cue, 1_000).unwrap();
        assert_eq!(x.request(User::Cue, 200, 1_000), Decision::Move(200));
        x.release(User::Cue, 2_000);
        x.claim(User::Cue, 3_000).unwrap();
        assert_eq!(x.request(User::Cue, 100, 3_000), Decision::No(Refused::CueBudget), "1200 ms used");
        // The window slides: long afterwards, the budget is free again.
        x.release(User::Cue, 3_000);
        x.claim(User::Cue, 90_000).unwrap();
        assert_eq!(x.request(User::Cue, 100, 90_000), Decision::Move(100));
    }

    #[test]
    fn heat_stops_cues_but_not_the_camera() {
        let mut x = f();
        x.set_temperature(48);
        x.claim(User::Cue, 0).unwrap();
        assert_eq!(x.request(User::Cue, 200, 0), Decision::No(Refused::TooHot(48)));
        x.release(User::Cue, 0);
        x.claim(User::Autofocus, 1).unwrap();
        assert_eq!(x.request(User::Autofocus, 900, 1), Decision::Move(900), "focusing is not optional");
        x.set_temperature(30);
        x.release(User::Autofocus, 2);
        x.claim(User::Cue, 3).unwrap();
        assert_eq!(x.request(User::Cue, 200, 3), Decision::Move(200));
    }

    #[test]
    fn targets_are_clamped_to_the_drivers_range() {
        let cfg = Config { min: 100, max: 400, ..Config::default() };
        let mut x = Focus::new(cfg);
        assert_eq!(x.position(), 100, "rest is the driver minimum, not zero");
        x.claim(User::Manual, 0).unwrap();
        assert_eq!(x.request(User::Manual, 9_999, 0), Decision::Move(400));
        assert_eq!(x.request(User::Manual, 0, 10), Decision::Move(100));
        assert_eq!(x.park_value(), 100);
    }
}
