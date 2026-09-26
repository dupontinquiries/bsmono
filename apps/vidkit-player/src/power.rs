//! What the app may spend energy on, from the power supply.
//!
//! On mains everything is allowed. On battery the background work that costs
//! the most for the least is cut back, and more of it as the charge falls:
//!
//! | power               | keep-download | transcribe      | analysis | enhance |
//! |---------------------|---------------|-----------------|----------|---------|
//! | mains               | on            | 10 min ahead    | on       | on      |
//! | > 50%               | off           | 10 min ahead    | off      | on      |
//! | 20..50%             | off           | 5 min ahead     | off      | sharpen only |
//! | < 20%               | off           | paused          | off      | off     |
//!
//! Keep-download goes first: it keeps the radio busy fetching a second copy
//! of what is already streaming. Whisper next: it is the largest CPU load
//! the app can generate. Enhancement last, and in steps: sharpening is five
//! texture fetches a pixel, the temporal pass and bicubic add more.
//! `VIDKIT_POWER=mains` forces the mains policy (a desk with a flat laptop
//! battery that is always plugged in reads oddly).

use std::path::Path;
use std::time::Duration;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PowerState {
    pub on_battery: bool,
    /// 0..100, when a battery reports it.
    pub percent: Option<u8>,
}

impl PowerState {
    pub const MAINS: PowerState = PowerState { on_battery: false, percent: None };
}

/// Read /sys/class/power_supply. Pure over the directory it is given.
///
/// On battery when no `Mains`/`USB` supply is online and some `Battery` is
/// discharging. A machine with no batteries (a desktop) is on mains. The
/// percent is the lowest battery's `capacity`.
pub fn read(root: &Path) -> PowerState {
    let Ok(rd) = std::fs::read_dir(root) else { return PowerState::MAINS };
    let (mut external_online, mut discharging, mut percent): (bool, bool, Option<u8>) = (false, false, None);
    for e in rd.flatten() {
        let d = e.path();
        let get = |f: &str| std::fs::read_to_string(d.join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
        match get("type").as_str() {
            "Mains" | "USB" | "USB_C" | "USB_PD" => external_online |= get("online") == "1",
            "Battery" => {
                if get("status") == "Discharging" {
                    discharging = true;
                }
                if let Ok(c) = get("capacity").parse::<u8>() {
                    percent = Some(percent.map_or(c, |p: u8| p.min(c)));
                }
            }
            _ => {}
        }
    }
    PowerState { on_battery: discharging && !external_online, percent }
}

pub fn current() -> PowerState {
    if std::env::var("VIDKIT_POWER").as_deref() == Ok("mains") {
        return PowerState::MAINS;
    }
    read(Path::new("/sys/class/power_supply"))
}

/// What the app does in the background, per the table above.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct EnergyPolicy {
    pub keep_download: bool,
    /// `None` = transcribe everything; `Some(h)` = only `h` ahead;
    /// `Some(ZERO)` = paused.
    pub transcribe_ahead: Option<Duration>,
    pub analysis: bool,
    pub enhance: Enhancement,
    /// Software presenter: conversions a frame may spend, as a fraction of
    /// a whole frame (vidkit_core::damage). 1.0 never degrades.
    pub sw_budget: f32,
    /// Run software decode on the efficiency cores (big.LITTLE machines).
    pub efficiency_cores: bool,
    /// Show at most this many frames a second. Only when the battery is
    /// nearly flat: 60 fps content at 30 skips half the decode (AV1) and
    /// half of everything after it -- the last big lever before stopping.
    pub max_fps: Option<u32>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Enhancement {
    Full,
    SharpenOnly,
    Off,
}

pub fn policy(p: PowerState) -> EnergyPolicy {
    if !p.on_battery {
        // Ahead, even on mains: transcribing a whole video at once is what
        // heated a laptop (reported: base model, plugged in). "All" in the
        // Sections panel builds the full chapter list when it is wanted.
        return EnergyPolicy { keep_download: true, transcribe_ahead: Some(Duration::from_secs(600)), analysis: true, enhance: Enhancement::Full, sw_budget: 1.0, efficiency_cores: false, max_fps: None };
    }
    let pct = p.percent.unwrap_or(50);
    let (ahead, enhance, sw_budget) = match pct {
        51.. => (Duration::from_secs(600), Enhancement::Full, 1.0),
        20..=50 => (Duration::from_secs(300), Enhancement::SharpenOnly, 0.25),
        _ => (Duration::ZERO, Enhancement::Off, 1.0 / 16.0),
    };
    let max_fps = (pct < 20).then_some(30);
    EnergyPolicy { keep_download: false, transcribe_ahead: Some(ahead), analysis: false, enhance, sw_budget, efficiency_cores: true, max_fps }
}

/// Cut an enhancement down to what the policy allows.
pub fn limit(e: vidkit_egui::glow_present::Enhance, allowed: Enhancement) -> vidkit_egui::glow_present::Enhance {
    use vidkit_egui::glow_present::{Enhance, Scaler};
    match allowed {
        Enhancement::Full => e,
        Enhancement::SharpenOnly => Enhance { scaler: Scaler::Bilinear, sharpen: e.sharpen, temporal: false },
        Enhancement::Off => Enhance::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(root: &Path, name: &str, files: &[(&str, &str)]) {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        for (f, v) in files {
            std::fs::write(d.join(f), format!("{v}\n")).unwrap();
        }
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("vk-power-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_laptop_unplugged_is_on_battery_and_plugged_is_not() {
        let r = scratch("laptop");
        supply(&r, "AC", &[("type", "Mains"), ("online", "0")]);
        supply(&r, "BAT0", &[("type", "Battery"), ("status", "Discharging"), ("capacity", "37")]);
        assert_eq!(read(&r), PowerState { on_battery: true, percent: Some(37) });
        supply(&r, "AC", &[("type", "Mains"), ("online", "1")]);
        assert!(!read(&r).on_battery, "plugged in");
        let _ = std::fs::remove_dir_all(&r);
    }

    /// A phone: the charger is a USB supply; the battery reports status.
    #[test]
    fn a_phone_on_usb_power_is_not_on_battery() {
        let r = scratch("phone");
        supply(&r, "pmi632-charger", &[("type", "USB"), ("online", "1")]);
        supply(&r, "battery", &[("type", "Battery"), ("status", "Charging"), ("capacity", "12")]);
        assert!(!read(&r).on_battery);
        supply(&r, "pmi632-charger", &[("type", "USB"), ("online", "0")]);
        supply(&r, "battery", &[("type", "Battery"), ("status", "Discharging"), ("capacity", "12")]);
        assert_eq!(read(&r), PowerState { on_battery: true, percent: Some(12) });
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn a_desktop_with_no_supplies_is_on_mains() {
        assert_eq!(read(Path::new("/nonexistent/power_supply")), PowerState::MAINS);
        let r = scratch("empty");
        std::fs::create_dir_all(&r).unwrap();
        assert_eq!(read(&r), PowerState { on_battery: false, percent: None });
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn the_policy_cuts_back_in_steps_as_the_charge_falls() {
        let b = |p| policy(PowerState { on_battery: true, percent: Some(p) });
        let mains = policy(PowerState::MAINS);
        assert!(mains.keep_download && mains.analysis);
        assert_eq!(mains.transcribe_ahead, Some(Duration::from_secs(600)), "never the whole video unasked");
        assert_eq!(b(80).enhance, Enhancement::Full);
        assert!(!b(80).keep_download && !b(80).analysis, "background work stops first");
        assert_eq!(b(80).transcribe_ahead, Some(Duration::from_secs(600)));
        assert_eq!(b(35).enhance, Enhancement::SharpenOnly);
        assert_eq!(b(35).transcribe_ahead, Some(Duration::from_secs(300)));
        assert_eq!(b(10).enhance, Enhancement::Off);
        assert_eq!(b(10).transcribe_ahead, Some(Duration::ZERO), "whisper paused");
        assert!(b(80).efficiency_cores && !mains.efficiency_cores, "decode on the little cores on battery");
        assert_eq!((mains.max_fps, b(35).max_fps, b(10).max_fps), (None, None, Some(30)), "frame rate halves only near empty");
    }

    #[test]
    fn limiting_an_enhancement_keeps_only_what_is_allowed() {
        use vidkit_egui::glow_present::{Enhance, Scaler};
        let full = Enhance { scaler: Scaler::Bicubic, sharpen: Some(0.4), temporal: true };
        assert_eq!(limit(full, Enhancement::Full), full);
        assert_eq!(limit(full, Enhancement::SharpenOnly), Enhance { scaler: Scaler::Bilinear, sharpen: Some(0.4), temporal: false });
        assert_eq!(limit(full, Enhancement::Off), Enhance::default());
        assert!(limit(full, Enhancement::SharpenOnly).fetches_per_pixel() < full.fetches_per_pixel());
    }
}
