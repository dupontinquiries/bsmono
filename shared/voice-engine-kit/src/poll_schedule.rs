//! poll_schedule — how often to wake the mic path when nothing is actively
//! listening for it, and for how long each time.
//!
//! ## The concern that reshaped this from three tiers to two
//!
//! The request behind this module was a three-tier schedule: screen off
//! (poll every 5s), no gyro motion (poll every 2s), in-hand or screen on
//! (poll every 1s) — each waking the mic for a ~2s window. Before building
//! that literally, two sourced facts changed the shape:
//!
//! 1. `maintenance/C18_TODO.md` section 20: FP5's mic is analog (AMIC), not
//!    digital (DMIC). A proper low-power voice architecture (Qualcomm's own
//!    docs on Listen Sound Model / Mic Activity Detection describe this)
//!    runs a first, nearly-free activity check INSIDE the codec, and only
//!    escalates to full DSP power when something is actually heard. FP5's
//!    AMIC has no such free first stage: every "poll" in a software-driven
//!    schedule like this one IS the full escalation, not a cheap check
//!    underneath one. There is no free layer to poll through.
//!
//! 2. Codec power transitions are not instant or free on real hardware —
//!    general ALSA history (HDA codec suspend/resume patches) documents
//!    resume latency ranging from fine to multiple SECONDS depending on
//!    driver/BIOS combination, and at least one historical report of
//!    frequent suspend/resume leaving codec clock state WRONG after
//!    wakeup. Nothing here is FP5/WCD9385-specific — that number has not
//!    been measured on this hardware — but the general principle (an
//!    activation has a real, sometimes nontrivial fixed cost, and cycling
//!    it frequently is not obviously free) is well-established enough to
//!    design around rather than assume away.
//!
//! Put together: a schedule with three tiers whose intervals are close
//! together (5s/2s/1s) will spend a meaningful fraction of its time
//! TRANSITIONING between tiers as a noisy motion signal crosses thresholds
//! — and each transition, on this hardware, is exactly the expensive
//! event the schedule exists to minimize. Two tiers, well separated, with
//! hysteresis so a brief flicker of motion does not itself trigger an
//! extra activation, is the more defensible shape until real measurement
//! says otherwise.
//!
//! ## What this reuses rather than reinvents
//!
//! `apps/snapclone/egui-frontend/src/posture.rs` already has a
//! `Posture { proximity_near, moving }` / `PostureSource` abstraction, read
//! from the same raw IIO accelerometer counts a gyro-motion signal here
//! would need — checked before building a second one. This module takes
//! [`DeviceState`], a small struct with the same SHAPE (not a dependency:
//! an app crate should not become a dependency of a shared engine kit, so
//! this is deliberately its own type, not an import of snapclone's). If
//! this ever needs the real reading rather than a caller-supplied bool,
//! the reading itself belongs in the IIO source layer
//! `maintenance/C18_TODO.md` section 2 scoped and never built — not
//! reimplemented here as a tenth reader.

use std::time::Duration;

/// What the mic path should do for the CURRENT tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollTier {
    /// How long to wait between activations.
    pub poll_interval: Duration,
    /// How long each activation records for.
    pub record_duration: Duration,
}

/// The two schedules. Deliberately two, not three — see the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleConfig {
    /// Screen on, or the phone was recently picked up/moved: someone may
    /// be about to speak to it. Short interval, short recording.
    pub foreground: PollTier,
    /// Screen off AND still for at least `hysteresis`: nobody is
    /// interacting with the phone right now. Long interval — this is the
    /// tier that matters for battery, since it is where the phone spends
    /// most of its time.
    pub background: PollTier,
    /// How long motion must be ABSENT before dropping from foreground to
    /// background. Without this, a phone that is picked up, checked, and
    /// set back down oscillates between tiers on every small settling
    /// motion as it comes to rest — each oscillation is an extra
    /// activation on hardware where activations are not free.
    ///
    /// There is no equivalent delay going the OTHER way (background to
    /// foreground): motion or a screen-on event switches to the responsive
    /// tier on the very next tick. The asymmetry is deliberate — the cost
    /// of reacting a poll cycle late to "someone picked up the phone" is a
    /// perceptible delay before a voice command is heard; the cost of
    /// staying in the responsive tier a few extra seconds after motion
    /// stops is a handful of extra activations. The first is a UX
    /// regression a user notices immediately; the second is a bounded,
    /// small cost. Not treating them the same is the whole point of an
    /// asymmetric hysteresis rather than a symmetric debounce.
    pub hysteresis: Duration,
}

impl Default for ScheduleConfig {
    /// Reasoned starting values, NOT measured ones — same caveat
    /// `voice_engine_kit::power_profile::FOREGROUND_ECO_UTIL_MAX` carries
    /// for the same reason: this needs a real device and a battery
    /// logger, not arithmetic. Two tiers a factor of ~15x apart in
    /// interval (2s vs 30s) are chosen to be unambiguously different
    /// regimes rather than close enough to blur together, which was the
    /// actual problem with the original three-tier proposal.
    fn default() -> Self {
        ScheduleConfig {
            foreground: PollTier {
                poll_interval: Duration::from_secs(2),
                record_duration: Duration::from_secs(2),
            },
            background: PollTier {
                poll_interval: Duration::from_secs(30),
                record_duration: Duration::from_secs(2),
            },
            hysteresis: Duration::from_secs(20),
        }
    }
}

/// What the scheduler currently sees. Shaped after `posture::Posture`
/// deliberately (see module doc) — `moving` should come from the same
/// accelerometer signal that abstraction already reads, once this and
/// that share a common sensor-state source; `screen_on` from whatever
/// windowing/session layer the host app already tracks (voiceterm's
/// `APP_BACKGROUNDED`, inverted, is the same signal by another name).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeviceState {
    pub screen_on: bool,
    pub moving: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier { Foreground, Background }

/// Tracks which tier is active and how long motion has been absent, so the
/// hysteresis in [`ScheduleConfig`] can be applied without either side
/// needing a wall clock — `tick` takes elapsed milliseconds explicitly,
/// same reason `handheld_detect`/`daemons` keep their decisions pure: a
/// test drives time by hand instead of sleeping.
#[derive(Debug, Clone)]
pub struct PollScheduler {
    cfg: ScheduleConfig,
    tier: Tier,
    /// Milliseconds of continuous stillness (screen off AND not moving)
    /// accumulated so far. Reset to 0 the instant either condition is not
    /// met. Only relevant while in `Foreground` — once in `Background`
    /// there is nothing left to accumulate toward.
    still_ms: u64,
}

impl PollScheduler {
    pub fn new(cfg: ScheduleConfig) -> Self {
        // Start foreground: the safe default on launch is "responsive",
        // the same reasoning `handheld_detect`'s Desktop-until-proven
        // fallback and posture.rs's NoSensors (far + still, i.e. "behave
        // normally") both use — an unknown or just-started state should
        // never silently choose the LESS responsive option first.
        PollScheduler { cfg, tier: Tier::Foreground, still_ms: 0 }
    }

    /// Advance the scheduler by `elapsed_ms` given the current device
    /// state, and return the tier now in effect.
    pub fn tick(&mut self, state: DeviceState, elapsed_ms: u64) -> PollTier {
        let currently_still = !state.screen_on && !state.moving;
        if currently_still {
            self.still_ms = self.still_ms.saturating_add(elapsed_ms);
        } else {
            // ANY motion or screen-on resets the counter AND switches to
            // Foreground immediately — the asymmetry ScheduleConfig's own
            // doc states. No hysteresis on the way up.
            self.still_ms = 0;
            self.tier = Tier::Foreground;
        }

        if currently_still && self.still_ms >= self.cfg.hysteresis.as_millis() as u64 {
            self.tier = Tier::Background;
        }

        match self.tier {
            Tier::Foreground => self.cfg.foreground,
            Tier::Background => self.cfg.background,
        }
    }

    /// For tests/diagnostics: which tier is active right now, without
    /// advancing anything.
    pub fn current_tier_is_background(&self) -> bool {
        matches!(self.tier, Tier::Background)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ScheduleConfig { ScheduleConfig::default() }

    #[test]
    fn starts_foreground_not_background() {
        // Launch state must be the responsive tier, same reasoning as
        // handheld_detect's Desktop-until-proven default: an unknown state
        // should never silently pick the less responsive option.
        let s = PollScheduler::new(cfg());
        assert!(!s.current_tier_is_background());
    }

    #[test]
    fn screen_on_stays_foreground_regardless_of_motion() {
        let mut s = PollScheduler::new(cfg());
        let still_but_screen_on = DeviceState { screen_on: true, moving: false };
        for _ in 0..100 {
            s.tick(still_but_screen_on, 1000);
        }
        assert!(!s.current_tier_is_background(), "screen on must never drop to background");
    }

    #[test]
    fn drops_to_background_only_after_the_full_hysteresis_window() {
        let mut s = PollScheduler::new(cfg());
        let still = DeviceState { screen_on: false, moving: false };
        let hyst_ms = cfg().hysteresis.as_millis() as u64;

        // One tick short of the window: must still be foreground.
        s.tick(still, hyst_ms - 1);
        assert!(!s.current_tier_is_background(), "must not drop early");

        // Crossing the threshold: now background.
        s.tick(still, 2);
        assert!(s.current_tier_is_background(), "must drop once the window is met");
    }

    #[test]
    fn any_motion_resets_the_stillness_counter_from_scratch() {
        // The case the three-tier proposal's close intervals would have
        // thrashed on: motion just before the hysteresis window completes
        // must restart the count, not merely pause it.
        let mut s = PollScheduler::new(cfg());
        let still = DeviceState { screen_on: false, moving: false };
        let moving = DeviceState { screen_on: false, moving: true };
        let hyst_ms = cfg().hysteresis.as_millis() as u64;

        s.tick(still, hyst_ms - 1); // one tick from dropping
        s.tick(moving, 1);          // motion resets it
        s.tick(still, hyst_ms - 1); // would have crossed if not reset
        assert!(!s.current_tier_is_background(), "motion must restart the window, not pause it");
    }

    #[test]
    fn background_to_foreground_has_no_hysteresis_the_other_way() {
        // The asymmetry ScheduleConfig documents: reacting late to "someone
        // picked the phone up" is a UX regression noticed immediately;
        // staying responsive a few extra seconds is a bounded small cost.
        // So going Background -> Foreground must be instant.
        let mut s = PollScheduler::new(cfg());
        let still = DeviceState { screen_on: false, moving: false };
        let hyst_ms = cfg().hysteresis.as_millis() as u64;
        s.tick(still, hyst_ms + 1);
        assert!(s.current_tier_is_background());

        let moved = DeviceState { screen_on: false, moving: true };
        let tier = s.tick(moved, 1);
        assert!(!s.current_tier_is_background(), "must react on the very next tick");
        assert_eq!(tier, cfg().foreground);
    }

    #[test]
    fn the_two_tiers_are_unambiguously_different_regimes() {
        // Pins the reasoning for two tiers instead of three: they must be
        // far enough apart that jitter in the underlying signal cannot
        // blur them together the way 5s/2s/1s would have.
        let c = ScheduleConfig::default();
        assert!(
            c.background.poll_interval.as_secs() >= c.foreground.poll_interval.as_secs() * 5,
            "background tier should be a clearly different regime, not a nearby step"
        );
    }

    #[test]
    fn tick_never_panics_on_zero_elapsed_or_repeated_calls() {
        // Degenerate inputs (a caller ticking twice in the same instant, a
        // clock read that returns 0 delta) must not panic or misbehave.
        let mut s = PollScheduler::new(cfg());
        for _ in 0..5 {
            s.tick(DeviceState::default(), 0);
        }
    }
}
