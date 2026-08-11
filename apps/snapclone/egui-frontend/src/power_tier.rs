//! power_tier — battery and power state, from `PowerManager`.
//!
//! Three sources for one fact, tried in order, because **no single one is
//! present everywhere this app runs**:
//!
//! ```text
//!   Tier 1  KDE PowerDevil   instant signals, Plasma only
//!   Tier 2  upower           battery detail, most desktops
//!   Tier 3  /sys polling     always there, coarse
//! ```
//!
//! ## The Plasma check exists to avoid a timeout, not to be tidy
//!
//! *"Detect Plasma session once — avoids wasting time on D-Bus timeouts when
//! PowerDevil is not running (postmarketOS without KDE)."*
//!
//! That is the target platform naming itself. Asking a bus for a service nobody
//! is offering does not fail fast; it blocks for the D-Bus default timeout, and
//! doing that during startup on a phone is a visible stall before the first
//! frame.
//!
//! ## The watchdog runs regardless of which tier is live
//!
//! *"30-second coarse timer regardless of which D-Bus tier is live. D-Bus
//! signals handle instant updates; this catches anything they miss."*
//!
//! Belt and braces on purpose: a signal subscription that silently dies leaves
//! the app believing the battery is wherever it was last told. A coarse poll
//! cannot fix the latency but it bounds the staleness, and thirty seconds of
//! stale battery is a different problem from indefinitely stale.
//!
//! ## The initial read happens before any signal
//!
//! *"Initial read — state is known before first signal fires."* Otherwise the
//! first thing the app knows about power is whatever changes first, and a phone
//! that is plugged in and idle changes nothing for a long time.

/// Which source told us.
///
/// Carried rather than discarded, because "battery at 40%" and "battery at 40%
/// according to a 30-second poll" support different decisions — the second is
/// not fresh enough to trigger something abrupt.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Tier {
    /// Instant D-Bus signals. Plasma only.
    PowerDevil,
    /// upower: detail and remaining time, most desktops.
    UPower,
    /// Coarse polling. Always available.
    SysPoll,
}

impl Tier {
    /// Is this source fresh enough to act on immediately?
    pub fn is_instant(self) -> bool { self != Tier::SysPoll }

    pub fn label(self) -> &'static str {
        match self {
            Tier::PowerDevil => "PowerDevil", Tier::UPower => "upower", Tier::SysPoll => "/sys",
        }
    }
}

/// The watchdog interval. Coarse on purpose — a precise timer would wake the
/// CPU on schedule, which is the opposite of what a power module should do.
pub const WATCHDOG_MS: u64 = 30_000;

/// Where the always-available tier reads from.
pub const SYS_POWER_SUPPLY: &str = "/sys/class/power_supply";

/// The desktop's power profile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PowerProfile { Performance, Balanced, PowerSaver }

impl PowerProfile {
    /// Parse the string `org.kde.Solid.PowerManagement` reports.
    ///
    /// Unknown values become Balanced rather than PowerSaver: a profile we do
    /// not recognise should not silently throttle the app, and guessing the
    /// most restrictive option makes a new profile name look like a bug in
    /// this app.
    pub fn parse(s: &str) -> PowerProfile {
        match s.trim().to_ascii_lowercase().as_str() {
            "performance" => PowerProfile::Performance,
            "power-saver" | "powersaver" => PowerProfile::PowerSaver,
            _ => PowerProfile::Balanced,
        }
    }
}

/// Should we look for PowerDevil at all?
///
/// Checked from the environment rather than by trying the bus, because trying
/// costs a full D-Bus timeout when nobody answers.
pub fn is_plasma_session(plasma_platform: Option<&str>, xdg_session_desktop: Option<&str>) -> bool {
    if plasma_platform.map(|p| p.starts_with("phone")).unwrap_or(false) { return true; }
    match xdg_session_desktop {
        Some(x) => {
            let l = x.to_ascii_lowercase();
            l.contains("kde") || l.contains("plasma")
        }
        None => false,
    }
}

/// Which tiers to subscribe to, in order.
///
/// upower and /sys are always included: PowerDevil can be present and still go
/// away when the session restarts, and losing every source at once is worse
/// than carrying a redundant one.
pub fn tiers_for(is_plasma: bool) -> Vec<Tier> {
    let mut v = Vec::new();
    if is_plasma { v.push(Tier::PowerDevil); }
    v.push(Tier::UPower);
    v.push(Tier::SysPoll);
    v
}

/// What the app believes about power.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PowerState {
    /// 0..=100, or None before the first read.
    pub percent: Option<u8>,
    pub on_ac: bool,
    pub profile: PowerProfile,
    pub screen_off: bool,
    /// Which tier last supplied the percentage.
    pub source: Option<Tier>,
    /// When it last changed, for staleness.
    pub updated_ms: u64,
}

impl Default for PowerState {
    fn default() -> Self {
        PowerState { percent: None, on_ac: false, profile: PowerProfile::Balanced,
                     screen_off: false, source: None, updated_ms: 0 }
    }
}

impl PowerState {
    /// Accept a reading from a tier.
    ///
    /// A better tier always wins; a worse one only fills a gap or refreshes its
    /// own reading. Without this rule the 30-second poll would keep overwriting
    /// PowerDevil's instant value with its own staler one, and the app would
    /// behave as if the good source were not there.
    pub fn apply(&mut self, tier: Tier, percent: u8, now_ms: u64) -> bool {
        let better_or_same = match self.source {
            None => true,
            Some(cur) => tier <= cur,
        };
        if !better_or_same { return false; }
        self.percent = Some(percent.min(100));
        self.source = Some(tier);
        self.updated_ms = now_ms;
        true
    }

    /// Is what we believe too old to act on?
    ///
    /// Bounded by the watchdog: if the poll has run and we still have nothing
    /// newer, something is wrong with every source.
    pub fn is_stale(&self, now_ms: u64) -> bool {
        self.percent.is_none() || now_ms.saturating_sub(self.updated_ms) > WATCHDOG_MS * 2
    }

    /// Should background work be held back?
    ///
    /// Three conditions, and the AC one is what keeps it from being annoying: a
    /// plugged-in phone at 10% is charging, and throttling it means it never
    /// catches up on the work it deferred.
    pub fn should_defer_background(&self) -> bool {
        if self.on_ac { return false; }
        self.profile == PowerProfile::PowerSaver
            || self.percent.map(|p| p <= LOW_BATTERY_PERCENT).unwrap_or(false)
    }
}

/// Below this, on battery, background work waits.
pub const LOW_BATTERY_PERCENT: u8 = 15;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_without_kde_never_asks_for_powerdevil() {
        // "Avoids wasting time on D-Bus timeouts when PowerDevil is not running
        // (postmarketOS without KDE)." Asking a bus for a service nobody offers
        // does not fail fast — it blocks for the default timeout, and doing that
        // at startup on a phone is a visible stall before the first frame.
        assert!(!is_plasma_session(None, Some("GNOME")));
        assert!(!is_plasma_session(None, None));
        let tiers = tiers_for(false);
        assert!(!tiers.contains(&Tier::PowerDevil));
    }

    #[test]
    fn a_plasma_mobile_session_is_detected_from_either_variable() {
        assert!(is_plasma_session(Some("phone"), None));
        assert!(is_plasma_session(None, Some("KDE")));
        assert!(is_plasma_session(None, Some("plasma")));
        assert!(is_plasma_session(None, Some("plasmamobile")));
    }

    #[test]
    fn the_lower_tiers_are_kept_even_when_powerdevil_is_present() {
        // PowerDevil can go away when the session restarts, and losing every
        // source at once is worse than carrying a redundant one.
        let t = tiers_for(true);
        assert_eq!(t, vec![Tier::PowerDevil, Tier::UPower, Tier::SysPoll]);
    }

    #[test]
    fn the_sys_tier_is_always_present() {
        // It is the one that needs no daemon at all.
        for plasma in [true, false] {
            assert!(tiers_for(plasma).contains(&Tier::SysPoll));
        }
    }

    #[test]
    fn a_coarse_poll_does_not_overwrite_an_instant_reading() {
        // Without the rule the 30-second poll keeps replacing PowerDevil's
        // value with a staler one, and the app behaves as if the good source
        // were not there.
        let mut s = PowerState::default();
        assert!(s.apply(Tier::PowerDevil, 80, 1_000));
        assert!(!s.apply(Tier::SysPoll, 60, 2_000), "poll overwrote an instant reading");
        assert_eq!(s.percent, Some(80));
        assert_eq!(s.source, Some(Tier::PowerDevil));
    }

    #[test]
    fn an_instant_reading_replaces_a_polled_one() {
        // The other direction must work, or a startup poll would lock out the
        // better source for the whole session.
        let mut s = PowerState::default();
        s.apply(Tier::SysPoll, 60, 1_000);
        assert!(s.apply(Tier::PowerDevil, 80, 2_000));
        assert_eq!(s.source, Some(Tier::PowerDevil));
    }

    #[test]
    fn a_tier_may_always_refresh_its_own_reading() {
        // Otherwise the battery would freeze at whatever the best tier first
        // reported.
        let mut s = PowerState::default();
        s.apply(Tier::SysPoll, 60, 1_000);
        assert!(s.apply(Tier::SysPoll, 55, 31_000));
        assert_eq!(s.percent, Some(55));
    }

    #[test]
    fn the_first_read_happens_before_any_signal() {
        // "Initial read — state is known before first signal fires." Otherwise
        // the first thing the app knows about power is whatever CHANGES first,
        // and a phone plugged in and idle changes nothing for a long time.
        let s = PowerState::default();
        assert!(s.is_stale(0), "an unread state must report itself unknown");
        assert_eq!(s.percent, None);
    }

    #[test]
    fn staleness_is_bounded_by_the_watchdog() {
        // If the poll has run twice and there is still nothing newer, every
        // source is failing.
        let mut s = PowerState::default();
        s.apply(Tier::SysPoll, 50, 1_000);
        assert!(!s.is_stale(1_000 + WATCHDOG_MS));
        assert!(s.is_stale(1_000 + WATCHDOG_MS * 2 + 1));
    }

    #[test]
    fn a_percentage_above_a_hundred_is_clamped() {
        // Some drivers report over 100 while charging, and a progress bar past
        // full looks like a bug in the app rather than the driver.
        let mut s = PowerState::default();
        s.apply(Tier::UPower, 137, 0);
        assert_eq!(s.percent, Some(100));
    }

    #[test]
    fn an_unknown_profile_name_does_not_throttle_the_app() {
        // A profile we do not recognise should not silently restrict things,
        // and guessing the most restrictive option makes a NEW profile name
        // look like a bug here.
        assert_eq!(PowerProfile::parse("turbo-mega"), PowerProfile::Balanced);
        assert_eq!(PowerProfile::parse(""), PowerProfile::Balanced);
        assert_eq!(PowerProfile::parse("power-saver"), PowerProfile::PowerSaver);
        assert_eq!(PowerProfile::parse("PERFORMANCE"), PowerProfile::Performance);
    }

    #[test]
    fn a_charging_phone_is_not_throttled_at_low_battery() {
        // A plugged-in phone at 10% is CHARGING, and throttling it means it
        // never catches up on the work it deferred.
        let mut s = PowerState::default();
        s.apply(Tier::UPower, 10, 0);
        s.on_ac = true;
        assert!(!s.should_defer_background());
        s.on_ac = false;
        assert!(s.should_defer_background());
    }

    #[test]
    fn power_saver_defers_even_at_full_charge() {
        // The user asked for it; battery level is not the only reason to save.
        let mut s = PowerState::default();
        s.apply(Tier::UPower, 100, 0);
        s.profile = PowerProfile::PowerSaver;
        assert!(s.should_defer_background());
    }

    #[test]
    fn an_unknown_battery_level_does_not_defer_by_itself() {
        // Before the first read, deferring everything would stall startup on a
        // desktop with no battery at all.
        let s = PowerState::default();
        assert!(!s.should_defer_background());
    }

    #[test]
    fn the_watchdog_interval_matches_the_original() {
        assert_eq!(WATCHDOG_MS, 30_000);
        assert!(Tier::PowerDevil.is_instant() && Tier::UPower.is_instant());
        assert!(!Tier::SysPoll.is_instant(), "a poll is not an instant source");
    }
}
