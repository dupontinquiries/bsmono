// power_state.rs — battery / AC awareness for the soundscape daemon.
//
// bo-muxd often runs on a portable controller (a laptop, a handheld, an
// SBC on a battery hat). A soundscape engine that decodes + mixes audio
// continuously is not free: it keeps the CPU out of deep idle, holds the
// audio device open, and (when spreading across channels) does real DSP
// work. None of that is welcome when the device is unplugged and low.
//
// This module reads the battery state from Linux's power-supply sysfs
// (no dependencies, no daemons — just files), and exposes a small policy
// the activity watcher uses to:
//   * go PASSIVE early when the battery is critically low, and
//   * stretch the idle timeout when on battery so we drop to passive
//     sooner after the user stops interacting.
//
// Everything degrades safely: a desktop with no battery reads as
// "AC / unknown capacity" and the battery triggers simply never fire,
// so behavior is identical to before on mains-powered installs.
//
// The sysfs layout (read fresh each poll; cheap, a few small files):
//   /sys/class/power_supply/<BAT*>/capacity   -> 0..100 (percent)
//   /sys/class/power_supply/<BAT*>/status     -> Charging|Discharging|Full|...
//   /sys/class/power_supply/<AC*|ADP*>/online -> 0|1
// Battery directory names vary by platform (BAT0, BAT1, macsmc-battery,
// axp20x-battery, …) so we glob for anything whose `type` is "Battery".

use std::path::{Path, PathBuf};

/// Where the power-supply class lives. A constant so tests can point at
/// a fixture directory instead.
pub const POWER_SUPPLY_DIR: &str = "/sys/class/power_supply";

/// A snapshot of the system's power state.
#[derive(Debug, Clone, PartialEq)]
pub struct PowerState {
    /// Battery charge 0..100, or None if no battery is present/readable.
    pub capacity: Option<u8>,
    /// True if we're running on battery (discharging, no AC online).
    /// Defaults to false (assume mains) when it can't be determined —
    /// the safe choice, so we never throttle a desktop by mistake.
    pub on_battery: bool,
    /// True if a battery exists at all (vs a desktop/mains-only box).
    pub has_battery: bool,
}

impl PowerState {
    /// A mains-powered default for systems with no battery.
    pub fn mains() -> Self {
        Self { capacity: None, on_battery: false, has_battery: false }
    }

    /// Read the current power state from the real system sysfs.
    pub fn read() -> Self {
        Self::read_from(Path::new(POWER_SUPPLY_DIR))
    }

    /// Read from an arbitrary power-supply root (for tests).
    pub fn read_from(root: &Path) -> Self {
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => return Self::mains(), // no sysfs → assume mains
        };

        let mut capacity: Option<u8> = None;
        let mut battery_status: Option<String> = None;
        let mut has_battery = false;
        let mut ac_online: Option<bool> = None;

        for entry in entries.flatten() {
            let dir = entry.path();
            let kind = read_trimmed(&dir.join("type")).unwrap_or_default();
            match kind.as_str() {
                "Battery" => {
                    has_battery = true;
                    if let Some(c) = read_trimmed(&dir.join("capacity"))
                        .and_then(|s| s.parse::<u8>().ok())
                    {
                        // If multiple batteries, keep the lowest (the one
                        // that'll die first dictates caution).
                        capacity = Some(match capacity {
                            Some(prev) => prev.min(c),
                            None => c,
                        });
                    }
                    if battery_status.is_none() {
                        battery_status = read_trimmed(&dir.join("status"));
                    }
                }
                "Mains" | "USB" | "ADP" => {
                    if let Some(v) = read_trimmed(&dir.join("online")) {
                        let online = v.trim() == "1";
                        ac_online = Some(ac_online.unwrap_or(false) || online);
                    }
                }
                _ => {}
            }
        }

        if !has_battery {
            return Self::mains();
        }

        // Decide on_battery. Prefer the explicit AC "online" flag; fall
        // back to the battery's own status string ("Discharging").
        let on_battery = match ac_online {
            Some(online) => !online,
            None => battery_status
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case("Discharging"))
                .unwrap_or(false),
        };

        Self { capacity, on_battery, has_battery }
    }
}

/// Tunable policy for how the battery state changes daemon behavior.
#[derive(Debug, Clone)]
pub struct PowerPolicy {
    /// At or below this charge (while on battery), the daemon goes
    /// passive to stop the continuous audio/DSP drain. 0 disables.
    pub critical_capacity: u8,
    /// At or below this charge (while on battery), the idle timeout is
    /// shortened by `low_idle_factor` so we drop to passive sooner.
    pub low_capacity: u8,
    /// Multiplier (<1.0) applied to the idle timeout when low + on
    /// battery. e.g. 0.5 = go passive after half the usual idle wait.
    pub low_idle_factor: f32,
}

impl Default for PowerPolicy {
    fn default() -> Self {
        Self {
            critical_capacity: 15,
            low_capacity: 30,
            low_idle_factor: 0.5,
        }
    }
}

impl PowerPolicy {
    /// Should we go passive *right now* purely on battery grounds?
    /// True only when on battery AND at/below the critical threshold.
    pub fn should_force_passive(&self, p: &PowerState) -> bool {
        if self.critical_capacity == 0 || !p.on_battery {
            return false;
        }
        matches!(p.capacity, Some(c) if c <= self.critical_capacity)
    }

    /// Effective idle timeout given the power state: the base timeout,
    /// scaled down when we're low on battery so the daemon naps sooner.
    pub fn effective_idle_secs(&self, base_secs: u64, p: &PowerState) -> u64 {
        if p.on_battery {
            if let Some(c) = p.capacity {
                if c <= self.low_capacity {
                    let scaled = (base_secs as f32 * self.low_idle_factor) as u64;
                    return scaled.max(30); // never below 30 s
                }
            }
        }
        base_secs
    }

    /// Duration form of `effective_idle_secs`, preserving sub-second
    /// precision (the real daemon uses minute-scale timeouts, but tests
    /// and any future fast configs use millisecond Durations). The 30 s
    /// floor is only applied when the BASE timeout is already at least
    /// 30 s, so a deliberately tiny configured timeout is still honored.
    pub fn effective_idle(&self, base: std::time::Duration, p: &PowerState)
        -> std::time::Duration
    {
        if p.on_battery {
            if let Some(c) = p.capacity {
                if c <= self.low_capacity {
                    let scaled = base.mul_f32(self.low_idle_factor);
                    let floor = std::time::Duration::from_secs(30);
                    // Only enforce the floor when the base wasn't already
                    // sub-floor (don't lengthen a deliberately tiny timeout).
                    return if base >= floor { scaled.max(floor) } else { scaled };
                }
            }
        }
        base
    }

    /// A short human-readable reason for a battery-forced passive, for
    /// the status line the controller shows.
    pub fn critical_reason(&self, p: &PowerState) -> String {
        match p.capacity {
            Some(c) => format!(
                "battery low ({}%) and unplugged — soundscape paused to save power",
                c),
            None => "on battery — soundscape paused to save power".to_string(),
        }
    }
}

/// Read a sysfs file and trim trailing whitespace/newline. None on any
/// error (missing file, permission, non-utf8).
fn read_trimmed(path: &PathBuf) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, name: &str, val: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), val).unwrap();
    }

    fn fixture(tmp: &Path, bat_cap: Option<&str>, bat_status: Option<&str>,
               ac_online: Option<&str>) {
        if let Some(c) = bat_cap {
            let b = tmp.join("BAT0");
            write(&b, "type", "Battery");
            write(&b, "capacity", c);
            if let Some(s) = bat_status { write(&b, "status", s); }
        }
        if let Some(o) = ac_online {
            let a = tmp.join("AC0");
            write(&a, "type", "Mains");
            write(&a, "online", o);
        }
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pstest_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn no_sysfs_is_mains() {
        let p = PowerState::read_from(Path::new("/nonexistent/path/xyz"));
        assert_eq!(p, PowerState::mains());
        assert!(!p.on_battery);
        assert!(!p.has_battery);
    }

    #[test]
    fn desktop_no_battery_is_mains() {
        let d = tmpdir("desktop");
        // Only a Mains supply, no battery.
        let a = d.join("AC0");
        write(&a, "type", "Mains");
        write(&a, "online", "1");
        let p = PowerState::read_from(&d);
        assert!(!p.has_battery);
        assert!(!p.on_battery);
    }

    #[test]
    fn unplugged_battery_reads_on_battery() {
        let d = tmpdir("unplugged");
        fixture(&d, Some("42"), Some("Discharging"), Some("0"));
        let p = PowerState::read_from(&d);
        assert_eq!(p.capacity, Some(42));
        assert!(p.on_battery);
        assert!(p.has_battery);
    }

    #[test]
    fn plugged_battery_not_on_battery() {
        let d = tmpdir("plugged");
        fixture(&d, Some("80"), Some("Charging"), Some("1"));
        let p = PowerState::read_from(&d);
        assert_eq!(p.capacity, Some(80));
        assert!(!p.on_battery);
    }

    #[test]
    fn status_fallback_when_no_ac_online() {
        // Some platforms expose no Mains supply; fall back to status.
        let d = tmpdir("statusfb");
        let b = d.join("BAT0");
        write(&b, "type", "Battery");
        write(&b, "capacity", "55");
        write(&b, "status", "Discharging");
        let p = PowerState::read_from(&d);
        assert!(p.on_battery, "should infer on_battery from Discharging status");
    }

    #[test]
    fn lowest_of_multiple_batteries_wins() {
        let d = tmpdir("multibat");
        let b0 = d.join("BAT0");
        write(&b0, "type", "Battery"); write(&b0, "capacity", "70");
        write(&b0, "status", "Discharging");
        let b1 = d.join("BAT1");
        write(&b1, "type", "Battery"); write(&b1, "capacity", "20");
        write(&b1, "status", "Discharging");
        let p = PowerState::read_from(&d);
        assert_eq!(p.capacity, Some(20), "should keep the lowest battery");
    }

    #[test]
    fn force_passive_only_when_critical_and_unplugged() {
        let pol = PowerPolicy::default(); // critical 15
        let crit_unplugged = PowerState { capacity: Some(10), on_battery: true, has_battery: true };
        let crit_plugged = PowerState { capacity: Some(10), on_battery: false, has_battery: true };
        let ok_unplugged = PowerState { capacity: Some(50), on_battery: true, has_battery: true };
        assert!(pol.should_force_passive(&crit_unplugged));
        assert!(!pol.should_force_passive(&crit_plugged), "plugged in: never force");
        assert!(!pol.should_force_passive(&ok_unplugged), "plenty of charge: don't force");
        assert!(!pol.should_force_passive(&PowerState::mains()));
    }

    #[test]
    fn idle_timeout_shortens_when_low_on_battery() {
        let pol = PowerPolicy::default(); // low 30, factor 0.5
        let base = 600u64;
        let low_unplugged = PowerState { capacity: Some(25), on_battery: true, has_battery: true };
        let high_unplugged = PowerState { capacity: Some(90), on_battery: true, has_battery: true };
        let low_plugged = PowerState { capacity: Some(25), on_battery: false, has_battery: true };
        assert_eq!(pol.effective_idle_secs(base, &low_unplugged), 300);
        assert_eq!(pol.effective_idle_secs(base, &high_unplugged), 600, "high charge: unchanged");
        assert_eq!(pol.effective_idle_secs(base, &low_plugged), 600, "plugged: unchanged");
        assert_eq!(pol.effective_idle_secs(base, &PowerState::mains()), 600);
    }

    #[test]
    fn idle_timeout_never_below_floor() {
        let pol = PowerPolicy { critical_capacity: 5, low_capacity: 30, low_idle_factor: 0.01 };
        let low = PowerState { capacity: Some(20), on_battery: true, has_battery: true };
        assert_eq!(pol.effective_idle_secs(600, &low), 30, "clamped to 30 s floor");
    }

    #[test]
    fn effective_idle_duration_preserves_subsecond() {
        use std::time::Duration;
        let pol = PowerPolicy::default(); // low 30, factor 0.5
        let low = PowerState { capacity: Some(20), on_battery: true, has_battery: true };
        let mains = PowerState::mains();
        // Minute-scale base on battery+low → halved, floored at 30 s.
        assert_eq!(pol.effective_idle(Duration::from_secs(600), &low),
                   Duration::from_secs(300));
        // A deliberately tiny base (tests use ms) is NOT lengthened to the
        // 30 s floor — the floor only applies when base >= 30 s.
        let tiny = Duration::from_millis(40);
        assert_eq!(pol.effective_idle(tiny, &low), Duration::from_millis(20));
        // On mains, unchanged regardless.
        assert_eq!(pol.effective_idle(tiny, &mains), tiny);
    }
}
