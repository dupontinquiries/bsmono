//! hwctl — the camera's own manual controls, where it has them.
//!
//! The DSLR half that cannot be done in software: real exposure time, real
//! gain, the sensor's own white balance and focus. A V4L2 camera (a webcam,
//! the PinePhone's sensors through the media pipeline, a capture stick)
//! exposes these as controls, and `v4l2-ctl` sets them while the stream
//! runs — the same subprocess choice capture-kit made for GStreamer: the
//! command is a line you can read in the log and paste into a terminal.
//!
//! Driver names drift (`exposure_absolute` became `exposure_time_absolute`,
//! `white_balance_temperature_auto` became `white_balance_automatic`), so a
//! control is found by [`Role`] through its aliases rather than by one name.
//!
//! NOT reached: a camera behind PipeWire/libcamera (most phones on current
//! postmarketOS) exposes its controls through libcamera, not V4L2 control
//! ids; this module reports "no hardware controls" there rather than
//! pretending. The parse and the command are tested against captured
//! `v4l2-ctl` output and a stand-in script; no real sensor was attached.

use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { Int, Bool, Menu, Other }

/// One control, as `v4l2-ctl --list-ctrls` printed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ctrl {
    pub name: String,
    pub kind: Kind,
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub default: i64,
    pub value: i64,
    /// The driver says it does nothing right now (manual exposure while
    /// auto-exposure is on).
    pub inactive: bool,
}

/// What a control is FOR, whatever the driver calls it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role { AutoExposure, Exposure, Gain, AutoWb, WbTemp, AutoFocus, Focus }

impl Role {
    pub const ALL: [Role; 7] = [Role::AutoExposure, Role::Exposure, Role::Gain, Role::AutoWb, Role::WbTemp, Role::AutoFocus, Role::Focus];
    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Role::AutoExposure => &["auto_exposure", "exposure_auto"],
            Role::Exposure => &["exposure_time_absolute", "exposure_absolute", "exposure"],
            Role::Gain => &["gain", "analogue_gain", "iso_sensitivity"],
            Role::AutoWb => &["white_balance_automatic", "white_balance_temperature_auto"],
            Role::WbTemp => &["white_balance_temperature"],
            Role::AutoFocus => &["focus_automatic_continuous", "focus_auto"],
            Role::Focus => &["focus_absolute"],
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Role::AutoExposure => "auto exposure", Role::Exposure => "shutter", Role::Gain => "gain",
            Role::AutoWb => "auto WB", Role::WbTemp => "WB", Role::AutoFocus => "autofocus", Role::Focus => "focus",
        }
    }
}

/// Parse `v4l2-ctl --list-ctrls`. Section headings and anything else that
/// is not a control line are skipped.
pub fn parse(text: &str) -> Vec<Ctrl> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((head, tail)) = line.split_once(':') else { continue };
        let mut hw = head.split_whitespace();
        let (Some(name), Some(id), Some(kind)) = (hw.next(), hw.next(), hw.next()) else { continue };
        if !id.starts_with("0x") { continue; }
        let kind = match kind.trim_matches(|c| c == '(' || c == ')') {
            "int" | "int64" => Kind::Int, "bool" => Kind::Bool, "menu" | "intmenu" => Kind::Menu, _ => Kind::Other,
        };
        let field = |k: &str| tail.split_whitespace().find_map(|t| t.strip_prefix(k)?.strip_prefix('=')?.parse::<i64>().ok());
        let Some(value) = field("value") else { continue };
        let flags = tail.split_whitespace().find_map(|t| t.strip_prefix("flags=")).unwrap_or("");
        out.push(Ctrl {
            name: name.to_string(), kind,
            min: field("min").unwrap_or(if kind == Kind::Bool { 0 } else { value }),
            max: field("max").unwrap_or(if kind == Kind::Bool { 1 } else { value }),
            step: field("step").unwrap_or(1).max(1),
            default: field("default").unwrap_or(value),
            value,
            inactive: flags.split(',').any(|f| f == "inactive"),
        });
    }
    out
}

/// A device's controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controls {
    pub device: String,
    pub ctrls: Vec<Ctrl>,
}

impl Controls {
    pub fn get(&self, role: Role) -> Option<&Ctrl> {
        role.aliases().iter().find_map(|a| self.ctrls.iter().find(|c| c.name == *a))
    }
    pub fn has_any(&self) -> bool { Role::ALL.iter().any(|r| self.get(*r).is_some()) }

    /// Is auto-exposure off? UVC's menu: 1 is manual, 3 aperture priority.
    pub fn manual_exposure(&self) -> Option<bool> {
        self.get(Role::AutoExposure).map(|c| c.value == AE_MANUAL)
    }

    /// The `(name, value)` pairs to switch a role — with the paired automatic
    /// control turned off first when a manual value is set, because a driver
    /// ignores an exposure time while auto-exposure owns it.
    pub fn plan_set(&self, role: Role, value: i64) -> Vec<(String, i64)> {
        let mut out = Vec::new();
        let auto_off = |r: Role, off: i64, out: &mut Vec<(String, i64)>| {
            if let Some(a) = self.get(r) { if a.value != off { out.push((a.name.clone(), off)); } }
        };
        match role {
            Role::Exposure | Role::Gain => auto_off(Role::AutoExposure, AE_MANUAL, &mut out),
            Role::WbTemp => auto_off(Role::AutoWb, 0, &mut out),
            Role::Focus => auto_off(Role::AutoFocus, 0, &mut out),
            _ => {}
        }
        if let Some(c) = self.get(role) {
            let v = (value.clamp(c.min, c.max) - c.min) / c.step * c.step + c.min;
            out.push((c.name.clone(), v));
        }
        out
    }

    /// Record values that were applied, so the panel shows them before the
    /// next re-list.
    pub fn note_applied(&mut self, pairs: &[(String, i64)]) {
        for (n, v) in pairs { if let Some(c) = self.ctrls.iter_mut().find(|c| &c.name == n) { c.value = *v; } }
    }
}

/// UVC's auto-exposure menu value for "manual".
pub const AE_MANUAL: i64 = 1;
/// UVC's "aperture priority" — auto exposure time.
pub const AE_AUTO: i64 = 3;

/// A V4L2 exposure time (units of 100 µs) as a shutter speed.
pub fn shutter_label(units: i64) -> String {
    let s = units.max(1) as f64 / 10_000.0;
    if s >= 0.5 { format!("{s:.1} s") } else { format!("1/{:.0} s", 1.0 / s) }
}

/// The `v4l2-ctl` arguments for a set.
pub fn set_args(device: &str, pairs: &[(String, i64)]) -> Vec<String> {
    let mut a = vec!["-d".to_string(), device.to_string()];
    for (n, v) in pairs { a.push("-c".into()); a.push(format!("{n}={v}")); }
    a
}

/// The program: `SNAPSHOT_CAM_V4L2_CTL` or `v4l2-ctl` on PATH.
pub fn program() -> String {
    std::env::var("SNAPSHOT_CAM_V4L2_CTL").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "v4l2-ctl".into())
}

pub fn list_with(prog: &str, device: &str) -> Result<Controls, String> {
    let out = Command::new(prog).args(["-d", device, "--list-ctrls"]).output()
        .map_err(|e| format!("{prog}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{prog} -d {device} --list-ctrls: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(Controls { device: device.into(), ctrls: parse(&String::from_utf8_lossy(&out.stdout)) })
}

pub fn apply_with(prog: &str, device: &str, pairs: &[(String, i64)]) -> Result<(), String> {
    if pairs.is_empty() { return Ok(()); }
    let args = set_args(device, pairs);
    let out = Command::new(prog).args(&args).output().map_err(|e| format!("{prog}: {e}"))?;
    if out.status.success() { Ok(()) } else {
        Err(format!("{prog} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a UVC webcam (Logitech C920 class) on kernel 6.x.
    const C920: &str = "
User Controls

                     brightness 0x00980900 (int)    : min=0 max=255 step=1 default=128 value=128
                           gain 0x00980913 (int)    : min=0 max=255 step=1 default=0 value=31
        white_balance_automatic 0x0098090c (bool)   : default=1 value=1
      white_balance_temperature 0x0098091a (int)    : min=2000 max=6500 step=1 default=4000 value=4000 flags=inactive
           power_line_frequency 0x00980918 (menu)   : min=0 max=2 default=2 value=2 (60 Hz)

Camera Controls

                  auto_exposure 0x009a0901 (menu)   : min=0 max=3 default=3 value=3 (Aperture Priority Mode)
         exposure_time_absolute 0x009a0902 (int)    : min=3 max=2047 step=1 default=250 value=333 flags=inactive
     exposure_dynamic_framerate 0x009a0903 (bool)   : default=0 value=1
                 focus_absolute 0x009a090a (int)    : min=0 max=250 step=5 default=0 value=0 flags=inactive
     focus_automatic_continuous 0x009a090c (bool)   : default=1 value=1
";

    /// An older kernel's names for the same things.
    const OLD: &str = "
                  exposure_auto 0x009a0901 (menu)   : min=0 max=3 default=3 value=1
              exposure_absolute 0x009a0902 (int)    : min=1 max=5000 step=1 default=157 value=200
 white_balance_temperature_auto 0x0098090c (bool)   : default=1 value=0
";

    fn c920() -> Controls { Controls { device: "/dev/video0".into(), ctrls: parse(C920) } }

    #[test]
    fn a_real_listing_parses_and_every_role_is_found() {
        let c = c920();
        assert_eq!(c.ctrls.len(), 10);
        let e = c.get(Role::Exposure).unwrap();
        assert_eq!((e.min, e.max, e.value, e.inactive), (3, 2047, 333, true));
        assert_eq!(c.get(Role::AutoWb).unwrap().kind, Kind::Bool);
        assert_eq!(c.get(Role::Focus).unwrap().step, 5);
        for r in Role::ALL { assert!(c.get(r).is_some(), "{r:?}"); }
        assert_eq!(c.manual_exposure(), Some(false));
    }

    #[test]
    fn older_driver_names_are_the_same_roles() {
        let c = Controls { device: "/dev/video2".into(), ctrls: parse(OLD) };
        assert_eq!(c.get(Role::Exposure).unwrap().name, "exposure_absolute");
        assert_eq!(c.manual_exposure(), Some(true));
        assert_eq!(c.get(Role::AutoWb).unwrap().value, 0);
        assert!(c.get(Role::Focus).is_none());
    }

    #[test]
    fn setting_a_manual_value_turns_its_automatic_off_first_and_snaps_to_the_step() {
        let c = c920();
        assert_eq!(c.plan_set(Role::Exposure, 100),
                   vec![("auto_exposure".into(), AE_MANUAL), ("exposure_time_absolute".into(), 100)]);
        assert_eq!(c.plan_set(Role::Focus, 37), vec![("focus_automatic_continuous".into(), 0), ("focus_absolute".into(), 35)]);
        assert_eq!(c.plan_set(Role::WbTemp, 99_999).last().unwrap().1, 6500, "clamped to the driver's range");
        assert_eq!(c.plan_set(Role::AutoExposure, AE_AUTO), vec![("auto_exposure".into(), AE_AUTO)]);
        assert_eq!(set_args("/dev/video0", &c.plan_set(Role::Exposure, 100)),
                   ["-d", "/dev/video0", "-c", "auto_exposure=1", "-c", "exposure_time_absolute=100"]);
    }

    #[test]
    fn shutter_speeds_read_like_a_camera() {
        assert_eq!(shutter_label(333), "1/30 s");
        assert_eq!(shutter_label(10), "1/1000 s");
        assert_eq!(shutter_label(5000), "0.5 s");
    }

    #[test]
    fn list_and_apply_go_through_the_program_and_its_failure_is_reported() {
        // A stand-in v4l2-ctl: lists the captured controls, records sets.
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("calls");
        let listing = d.path().join("listing");
        std::fs::write(&listing, C920).unwrap();
        let prog = d.path().join("v4l2-ctl");
        std::fs::write(&prog, format!(
            "#!/bin/sh\necho \"$@\" >> {log}\ncase \"$*\" in *--list-ctrls*) cat {listing};; *bogus*) echo 'VIDIOC_S_EXT_CTRLS: failed: Invalid argument' >&2; exit 1;; esac\n",
            log = log.display(), listing = listing.display())).unwrap();
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap(); }
        let p = prog.to_str().unwrap();
        let c = list_with(p, "/dev/video0").unwrap();
        assert!(c.has_any());
        apply_with(p, "/dev/video0", &c.plan_set(Role::Gain, 64)).unwrap();
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.contains("-d /dev/video0 -c auto_exposure=1 -c gain=64"), "{calls}");
        let e = apply_with(p, "/dev/video0", &[("bogus".into(), 1)]).unwrap_err();
        assert!(e.contains("Invalid argument"), "{e}");
        assert!(list_with("/nonexistent/v4l2-ctl", "/dev/video0").is_err());
    }
}
