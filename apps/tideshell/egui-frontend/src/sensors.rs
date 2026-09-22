//! Ambient light in, backlight out, and the local clock: the transports under
//! `tideshell_core::ambient`. Each has a D-Bus path and a sysfs fallback, and
//! each reports what it found as a status line for the trace, because which
//! of them works depends on the device image.
//!
//! * **Light:** iio-sensor-proxy (`net.hadess.SensorProxy`, system bus): the
//!   standard service, which on Qualcomm phones fronts the sensor hub. Falls
//!   back to IIO sysfs (`in_illuminance_input`, or `raw * scale + offset`).
//! * **Backlight:** found in `/sys/class/backlight`; written through logind
//!   `Session.SetBrightness` on the user's graphical session, which works
//!   without root. The session is looked up via `user/self`'s `Display`
//!   property, not `session/auto`: a systemd user service (tideshell's
//!   autostart unit) belongs to no session, so "auto" would not resolve.
//!   Falls back to writing sysfs directly when that is writable.
//!
//! D-Bus functions take the connection as a parameter so the tests can run
//! them against mock services on a private bus.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ---- local clock -----------------------------------------------------------

/// Minutes after local midnight (uses the system timezone via libc).
pub fn local_minute() -> u32 {
    let (h, m) = local_hm();
    h * 60 + m
}

/// Local hour, minute and weekday (0 = Sunday).
pub fn local_time() -> tideshell_core::clock::LocalTime {
    // SAFETY: as local_hm.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return tideshell_core::clock::LocalTime { hour: 12, minute: 0, weekday: 0 };
        }
        tideshell_core::clock::LocalTime { hour: tm.tm_hour as u32, minute: tm.tm_min as u32, weekday: tm.tm_wday as u32 }
    }
}

/// Local (hour, minute). Falls back to noon if the time cannot be read.
pub fn local_hm() -> (u32, u32) {
    // SAFETY: time(NULL) and localtime_r with valid pointers to owned values.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return (12, 0);
        }
        (tm.tm_hour as u32, tm.tm_min as u32)
    }
}

// ---- IIO light sensor (sysfs) ------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum IioKind {
    Input,
    Raw { scale: f32, offset: f32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct IioLight {
    pub dir: PathBuf,
    kind: IioKind,
}

fn read_f32(p: &Path) -> Option<f32> {
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

/// First IIO device under `<sys>/bus/iio/devices` with an illuminance channel.
pub fn find_iio_light(sys: &Path) -> Option<IioLight> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(sys.join("bus/iio/devices")).ok()?.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        if dir.join("in_illuminance_input").exists() {
            return Some(IioLight { dir, kind: IioKind::Input });
        }
        if dir.join("in_illuminance_raw").exists() {
            let scale = read_f32(&dir.join("in_illuminance_scale")).unwrap_or(1.0);
            let offset = read_f32(&dir.join("in_illuminance_offset")).unwrap_or(0.0);
            return Some(IioLight { dir, kind: IioKind::Raw { scale, offset } });
        }
    }
    None
}

impl IioLight {
    /// Lux, per the IIO ABI: `input` is already processed; `raw` needs
    /// `(raw + offset) * scale`.
    pub fn read(&self) -> Option<f32> {
        match self.kind {
            IioKind::Input => read_f32(&self.dir.join("in_illuminance_input")),
            IioKind::Raw { scale, offset } => read_f32(&self.dir.join("in_illuminance_raw")).map(|r| (r + offset) * scale),
        }
    }
}

// ---- backlight ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Backlight {
    pub name: String,
    pub dir: PathBuf,
    pub max: u32,
}

/// Perceived level <-> raw value. Panel drivers are roughly linear in
/// luminance, perception is not, so the UI level is gamma-mapped.
pub const GAMMA: f32 = 2.2;

pub fn level_to_raw(level: f32, max: u32) -> u32 {
    // Never 0: on some panels (OLED, the FP5's) 0 can switch the panel off.
    ((level.clamp(0.0, 1.0).powf(GAMMA) * max as f32).round() as u32).clamp(1, max.max(1))
}

pub fn raw_to_level(raw: u32, max: u32) -> f32 {
    (raw.min(max) as f32 / max.max(1) as f32).powf(1.0 / GAMMA)
}

/// The backlight device, preferring `firmware` > `platform` > `raw` per the
/// kernel's sysfs-class-backlight guidance.
pub fn find_backlight(sys: &Path) -> Option<Backlight> {
    let mut found: Vec<(u8, Backlight)> = Vec::new();
    for e in std::fs::read_dir(sys.join("class/backlight")).ok()?.flatten() {
        let dir = e.path();
        let Some(max) = read_f32(&dir.join("max_brightness")).map(|m| m as u32).filter(|m| *m > 0) else { continue };
        let rank = match std::fs::read_to_string(dir.join("type")).unwrap_or_default().trim() {
            "firmware" => 0,
            "platform" => 1,
            "raw" => 2,
            _ => 3,
        };
        let name = e.file_name().to_string_lossy().into_owned();
        found.push((rank, Backlight { name, dir, max }));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.cmp(&b.1.name)));
    found.into_iter().next().map(|(_, b)| b)
}

impl Backlight {
    pub fn read_level(&self) -> Option<f32> {
        read_f32(&self.dir.join("brightness")).map(|r| raw_to_level(r as u32, self.max))
    }

    fn write_sysfs(&self, raw: u32) -> std::io::Result<()> {
        std::fs::write(self.dir.join("brightness"), raw.to_string())
    }
}

// ---- D-Bus: iio-sensor-proxy and logind ------------------------------------------

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

fn sensor_proxy(bus: &Connection) -> zbus::Result<Proxy<'static>> {
    Proxy::new(bus, "net.hadess.SensorProxy", "/net/hadess/SensorProxy", "net.hadess.SensorProxy")
}

/// Claim the light sensor. The claim lasts as long as `bus` stays open.
pub fn sensor_proxy_claim(bus: &Connection) -> Result<String, String> {
    let p = sensor_proxy(bus).map_err(|e| e.to_string())?;
    let has: bool = p.get_property("HasAmbientLight").map_err(|e| format!("HasAmbientLight: {e}"))?;
    if !has {
        return Err("iio-sensor-proxy reports no ambient light sensor".into());
    }
    p.call_method("ClaimLight", &()).map_err(|e| format!("ClaimLight: {e}"))?;
    p.get_property::<String>("LightLevelUnit").map_err(|e| format!("LightLevelUnit: {e}"))
}

pub fn sensor_proxy_read(bus: &Connection) -> Result<f32, String> {
    let p = sensor_proxy(bus).map_err(|e| e.to_string())?;
    p.get_property::<f64>("LightLevel").map(|v| v as f32).map_err(|e| format!("LightLevel: {e}"))
}

/// The user's graphical session, from logind's `user/self` `Display`.
pub fn logind_graphical_session(bus: &Connection) -> Result<OwnedObjectPath, String> {
    let u = Proxy::new(bus, "org.freedesktop.login1", "/org/freedesktop/login1/user/self", "org.freedesktop.login1.User").map_err(|e| e.to_string())?;
    let (id, path): (String, OwnedObjectPath) = u.get_property("Display").map_err(|e| format!("user Display: {e}"))?;
    if id.is_empty() || path.as_str() == "/" {
        return Err("logind: no graphical session for this user".into());
    }
    Ok(path)
}

pub fn logind_set_brightness(bus: &Connection, session: &OwnedObjectPath, name: &str, raw: u32) -> Result<(), String> {
    let s = Proxy::new(bus, "org.freedesktop.login1", session.as_str().to_owned(), "org.freedesktop.login1.Session").map_err(|e| e.to_string())?;
    s.call_method("SetBrightness", &("backlight", name, raw)).map(|_| ()).map_err(|e| format!("SetBrightness: {e}"))
}

// ---- threads -------------------------------------------------------------------

/// Background reader: lux values arrive on `rx` (~4 Hz), `status()` says
/// which source is in use.
pub struct Ambient {
    pub rx: Receiver<f32>,
    status: Arc<Mutex<String>>,
}

impl Ambient {
    pub fn spawn(sys: PathBuf, bus: Option<Connection>) -> Ambient {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new("ambient: starting".to_string()));
        let st = status.clone();
        std::thread::Builder::new()
            .name("tideshell-ambient".into())
            .spawn(move || ambient_loop(sys, bus, tx, st))
            .expect("spawn ambient thread");
        Ambient { rx, status }
    }

    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }
}

fn ambient_loop(sys: PathBuf, bus: Option<Connection>, tx: Sender<f32>, st: Arc<Mutex<String>>) {
    let set = |s: String| *st.lock().unwrap() = s;
    if let Some(bus) = bus {
        match sensor_proxy_claim(&bus) {
            Ok(unit) => {
                set(format!("ambient: iio-sensor-proxy ({unit})"));
                loop {
                    match sensor_proxy_read(&bus) {
                        Ok(v) => {
                            if tx.send(v).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            set(format!("ambient: iio-sensor-proxy failed ({e}); trying sysfs"));
                            break;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
            Err(e) => set(format!("ambient: iio-sensor-proxy unavailable ({e}); trying sysfs")),
        }
    }
    match find_iio_light(&sys) {
        Some(light) => {
            set(format!("ambient: sysfs {}", light.dir.display()));
            loop {
                if let Some(v) = light.read() {
                    if tx.send(v).is_err() {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        None => set("ambient: no light sensor found; brightness follows time of day only".into()),
    }
}

/// Background writer: `set(level)` coalesces to the newest value; `status()`
/// says how brightness is being applied (or why it cannot be).
pub struct BacklightOut {
    tx: Option<Sender<f32>>,
    status: Arc<Mutex<String>>,
    /// The level found at startup (None: no readable backlight).
    pub initial: Option<f32>,
}

impl BacklightOut {
    pub fn spawn(sys: &Path, bus: Option<Connection>) -> BacklightOut {
        Self::spawn_with(sys, bus, find_brightnessctl())
    }

    /// `spawn` with an explicit brightnessctl path (tests pass a fake one).
    pub fn spawn_with(sys: &Path, bus: Option<Connection>, tool: Option<PathBuf>) -> BacklightOut {
        let status = Arc::new(Mutex::new(String::new()));
        let Some(bl) = find_backlight(sys) else {
            *status.lock().unwrap() = "backlight: none found under /sys/class/backlight".into();
            return BacklightOut { tx: None, status, initial: None };
        };
        let initial = bl.read_level();
        let (tx, rx) = mpsc::channel::<f32>();
        let st = status.clone();
        std::thread::Builder::new()
            .name("tideshell-backlight".into())
            .spawn(move || backlight_loop(bl, bus, tool, rx, st))
            .expect("spawn backlight thread");
        BacklightOut { tx: Some(tx), status, initial }
    }

    pub fn set(&self, level: f32) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(level);
        }
    }

    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }
}

/// How a brightness value gets applied, in the order tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// logind Session.SetBrightness on the graphical session (no root).
    Logind,
    /// The brightnessctl tool (known to work on the FP5 image; it itself
    /// uses logind or a udev-granted sysfs permission).
    Brightnessctl,
    /// Writing /sys/class/backlight/<name>/brightness directly.
    Sysfs,
}

/// Minimum spacing between writes: brightnessctl is a process per call and
/// a ramp produces many steps; coalescing keeps only the newest value.
pub const MIN_WRITE_INTERVAL: Duration = Duration::from_millis(50);
/// logind / sysfs: one frame of a 90 Hz panel.
pub const FAST_WRITE_INTERVAL: Duration = Duration::from_micros(11_111);

/// `brightnessctl` on PATH, if installed.
pub fn find_brightnessctl() -> Option<PathBuf> {
    std::env::var_os("PATH")?.to_str()?.split(':').map(|d| Path::new(d).join("brightnessctl")).find(|p| p.is_file())
}

fn brightnessctl_set(tool: &Path, name: &str, raw: u32) -> Result<(), String> {
    let out = std::process::Command::new(tool)
        .args(["-q", "-d", name, "set", &raw.to_string()])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("brightnessctl: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("brightnessctl exited {}: {}", out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn backlight_loop(bl: Backlight, bus: Option<Connection>, tool: Option<PathBuf>, rx: Receiver<f32>, st: Arc<Mutex<String>>) {
    let set = |s: String| *st.lock().unwrap() = s;
    let session = bus.as_ref().map(logind_graphical_session);
    // Candidate methods in order; the first that works stays in use, and a
    // failure moves on to the next (with the reason kept for the status).
    let mut methods: Vec<Method> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    match &session {
        Some(Ok(_)) => methods.push(Method::Logind),
        Some(Err(e)) => skipped.push(format!("logind: {e}")),
        None => skipped.push("logind: no system bus".into()),
    }
    match &tool {
        Some(_) => methods.push(Method::Brightnessctl),
        None => skipped.push("brightnessctl: not installed".into()),
    }
    methods.push(Method::Sysfs);
    let describe = |m: Method| match (m, &session, &tool) {
        (Method::Logind, Some(Ok(p)), _) => format!("logind {}", p.as_str()),
        (Method::Brightnessctl, _, Some(t)) => format!("brightnessctl ({})", t.display()),
        _ => "sysfs".to_string(),
    };
    let mut cur = 0usize;
    let note = |cur: usize, extra: &[String]| {
        let mut s = format!("backlight: {} (max {}) via {}", bl.name, bl.max, describe(methods[cur]));
        if !extra.is_empty() {
            s.push_str(&format!(" [{}]", extra.join("; ")));
        }
        s
    };
    set(note(cur, &skipped));
    let mut last_write: Option<std::time::Instant> = None;
    while let Ok(mut level) = rx.recv() {
        // 90 Hz on the cheap paths (logind is one D-Bus call, sysfs one
        // write); brightnessctl is a process per call, so 20 Hz.
        let interval = if methods[cur] == Method::Brightnessctl { MIN_WRITE_INTERVAL } else { FAST_WRITE_INTERVAL };
        if let Some(t) = last_write {
            let since = t.elapsed();
            if since < interval {
                std::thread::sleep(interval - since);
            }
        }
        // Coalesce: a ramp sends many values; only the newest matters.
        while let Ok(newer) = rx.try_recv() {
            level = newer;
        }
        let raw = level_to_raw(level, bl.max);
        loop {
            let r = match methods[cur] {
                Method::Logind => match (&bus, &session) {
                    (Some(b), Some(Ok(p))) => logind_set_brightness(b, p, &bl.name, raw),
                    _ => Err("no logind session".into()),
                },
                Method::Brightnessctl => brightnessctl_set(tool.as_deref().unwrap(), &bl.name, raw),
                Method::Sysfs => bl.write_sysfs(raw).map_err(|e| format!("sysfs: {e}")),
            };
            match r {
                Ok(()) => break,
                Err(e) if cur + 1 < methods.len() => {
                    skipped.push(e);
                    cur += 1;
                    set(note(cur, &skipped));
                }
                Err(e) => {
                    set(format!("backlight: cannot set {} (all methods failed; last: {e}) [{}]", bl.name, skipped.join("; ")));
                    break;
                }
            }
        }
        last_write = Some(std::time::Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tideshell-sensors-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn iio_light_input_and_raw_variants() {
        let sys = tmp("iio");
        put(&sys.join("bus/iio/devices/iio:device0/in_accel_x_raw"), "12\n");
        assert_eq!(find_iio_light(&sys), None, "accelerometer is not a light sensor");
        put(&sys.join("bus/iio/devices/iio:device1/in_illuminance_raw"), "200\n");
        put(&sys.join("bus/iio/devices/iio:device1/in_illuminance_scale"), "0.5\n");
        put(&sys.join("bus/iio/devices/iio:device1/in_illuminance_offset"), "10\n");
        let l = find_iio_light(&sys).unwrap();
        assert_eq!(l.read(), Some(105.0), "(raw + offset) * scale");
        put(&sys.join("bus/iio/devices/iio:device0/in_illuminance_input"), "321.5\n");
        let l = find_iio_light(&sys).unwrap();
        assert_eq!(l.read(), Some(321.5), "processed input preferred, first device wins");
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn backlight_prefers_firmware_and_reads_levels() {
        let sys = tmp("bl");
        assert_eq!(find_backlight(&sys), None);
        put(&sys.join("class/backlight/panel-raw/max_brightness"), "4095\n");
        put(&sys.join("class/backlight/panel-raw/type"), "raw\n");
        put(&sys.join("class/backlight/panel-raw/brightness"), "4095\n");
        put(&sys.join("class/backlight/acpi_video0/max_brightness"), "100\n");
        put(&sys.join("class/backlight/acpi_video0/type"), "firmware\n");
        put(&sys.join("class/backlight/broken/max_brightness"), "0\n");
        let b = find_backlight(&sys).unwrap();
        assert_eq!((b.name.as_str(), b.max), ("acpi_video0", 100));
        std::fs::remove_dir_all(sys.join("class/backlight/acpi_video0")).unwrap();
        let b = find_backlight(&sys).unwrap();
        assert_eq!(b.name, "panel-raw");
        assert_eq!(b.read_level(), Some(1.0));
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn gamma_mapping_round_trips_and_never_hits_zero() {
        // What is guaranteed: monotonic, and a tight round trip wherever the
        // backlight has resolution. At the bottom of an 8-bit scale the
        // gamma pushes levels below one raw step (0.05 -> raw 0.35 -> 1 ->
        // reads back 0.08): inherent quantization, not a mapping bug.
        for max in [255u32, 4095] {
            let mut prev = 0;
            for i in 0..=200 {
                let raw = level_to_raw(i as f32 / 200.0, max);
                assert!(raw >= prev, "monotonic at {i}");
                prev = raw;
            }
            for i in 1..=20 {
                let l = i as f32 / 20.0;
                let raw = level_to_raw(l, max);
                if raw < 4 {
                    continue; // below the scale's resolution
                }
                let back = raw_to_level(raw, max);
                assert!((back - l).abs() < 0.03, "{l} -> {back} at max {max}");
            }
            assert_eq!(level_to_raw(0.0, max), 1, "never 0: an OLED can switch off");
            assert_eq!(level_to_raw(1.0, max), max);
        }
        assert!(level_to_raw(0.5, 4095) < 4095 / 2, "gamma: half perceived is well under half raw");
    }

    fn fake_sys(tag: &str) -> PathBuf {
        let sys = tmp(tag);
        put(&sys.join("class/backlight/panel/max_brightness"), "4095");
        put(&sys.join("class/backlight/panel/brightness"), "1000");
        sys
    }

    fn fake_tool(dir: &Path, exit: i32) -> PathBuf {
        let tool = dir.join("brightnessctl");
        let log = dir.join("calls");
        put(&tool, &format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit {exit}\n", log.display()));
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        tool
    }

    fn wait_for(mut f: impl FnMut() -> bool) -> bool {
        (0..150).any(|_| {
            let ok = f();
            if !ok {
                std::thread::sleep(Duration::from_millis(20));
            }
            ok
        })
    }

    #[test]
    fn without_logind_brightnessctl_is_used_and_sysfs_left_alone() {
        let sys = fake_sys("bctl");
        let tool = fake_tool(&sys, 0);
        let out = BacklightOut::spawn_with(&sys, None, Some(tool));
        for i in 0..=30 {
            out.set(0.4 + i as f32 * 0.02);
        }
        let want = format!("-q -d panel set {}", level_to_raw(1.0, 4095));
        let calls = sys.join("calls");
        assert!(wait_for(|| std::fs::read_to_string(&calls).unwrap_or_default().lines().last() == Some(want.as_str())), "last call: {:?}", std::fs::read_to_string(&calls).ok());
        let n = std::fs::read_to_string(&calls).unwrap().lines().count();
        assert!(n < 31, "coalesced and throttled: {n} calls for 31 values");
        assert!(out.status().contains("via brightnessctl"), "{}", out.status());
        assert!(out.status().contains("logind: no system bus"), "the reason logind was skipped is shown: {}", out.status());
        assert_eq!(std::fs::read_to_string(sys.join("class/backlight/panel/brightness")).unwrap(), "1000");
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn a_failing_brightnessctl_falls_through_to_sysfs_and_says_why() {
        let sys = fake_sys("bctl-fail");
        let tool = fake_tool(&sys, 1);
        let out = BacklightOut::spawn_with(&sys, None, Some(tool));
        out.set(1.0);
        let path = sys.join("class/backlight/panel/brightness");
        assert!(wait_for(|| std::fs::read_to_string(&path).unwrap_or_default() == "4095"));
        let st = out.status();
        assert!(st.contains("via sysfs") && st.contains("brightnessctl exited 1"), "{st}");
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn local_clock_is_in_range() {
        let m = local_minute();
        assert!(m < 1440);
    }
}

/// Real D-Bus plumbing against mock iio-sensor-proxy and logind services on
/// a private dbus-daemon. Skips (passes vacuously, with a note) when
/// dbus-daemon is not installed.
#[cfg(test)]
mod dbus_tests {
    use super::*;
    use std::process::{Child, Command, Stdio};

    struct MockSensors {
        level: f64,
        claimed: Arc<Mutex<bool>>,
    }
    #[zbus::interface(name = "net.hadess.SensorProxy")]
    impl MockSensors {
        fn claim_light(&self) {
            *self.claimed.lock().unwrap() = true;
        }
        #[zbus(property)]
        fn has_ambient_light(&self) -> bool {
            true
        }
        #[zbus(property)]
        fn light_level(&self) -> f64 {
            self.level
        }
        #[zbus(property)]
        fn light_level_unit(&self) -> String {
            "lux".into()
        }
    }

    struct MockUser;
    #[zbus::interface(name = "org.freedesktop.login1.User")]
    impl MockUser {
        #[zbus(property)]
        fn display(&self) -> (String, OwnedObjectPath) {
            ("c1".into(), OwnedObjectPath::try_from("/org/freedesktop/login1/session/c1").unwrap())
        }
    }

    struct MockSession {
        calls: Arc<Mutex<Vec<(String, String, u32)>>>,
    }
    #[zbus::interface(name = "org.freedesktop.login1.Session")]
    impl MockSession {
        fn set_brightness(&self, subsystem: String, name: String, value: u32) {
            self.calls.lock().unwrap().push((subsystem, name, value));
        }
    }

    struct Bus(Child, String);
    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.0.kill();
        }
    }

    fn private_bus() -> Option<Bus> {
        let sock = std::env::temp_dir().join(format!("tideshell-dbus-{}", std::process::id()));
        let _ = std::fs::remove_file(&sock);
        let addr = format!("unix:path={}", sock.display());
        let child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", &format!("--address={addr}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        for _ in 0..50 {
            if sock.exists() {
                return Some(Bus(child, addr));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    #[test]
    fn sensor_proxy_and_logind_end_to_end() {
        let Some(bus) = private_bus() else {
            eprintln!("dbus-daemon not available: skipping D-Bus mock test");
            return;
        };
        let claimed = Arc::new(Mutex::new(false));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let _server = zbus::blocking::connection::Builder::address(bus.1.as_str())
            .unwrap()
            .name("net.hadess.SensorProxy")
            .unwrap()
            .name("org.freedesktop.login1")
            .unwrap()
            .serve_at("/net/hadess/SensorProxy", MockSensors { level: 432.0, claimed: claimed.clone() })
            .unwrap()
            .serve_at("/org/freedesktop/login1/user/self", MockUser)
            .unwrap()
            .serve_at("/org/freedesktop/login1/session/c1", MockSession { calls: calls.clone() })
            .unwrap()
            .build()
            .unwrap();
        let client = || zbus::blocking::connection::Builder::address(bus.1.as_str()).unwrap().build().unwrap();

        // Direct calls.
        let c = client();
        assert_eq!(sensor_proxy_claim(&c).as_deref(), Ok("lux"));
        assert!(*claimed.lock().unwrap(), "ClaimLight was called");
        assert_eq!(sensor_proxy_read(&c), Ok(432.0));
        let session = logind_graphical_session(&c).unwrap();
        assert_eq!(session.as_str(), "/org/freedesktop/login1/session/c1");
        logind_set_brightness(&c, &session, "panel", 777).unwrap();
        assert_eq!(calls.lock().unwrap().last().cloned(), Some(("backlight".into(), "panel".into(), 777)));

        // The threads, as the runtime uses them.
        let amb = Ambient::spawn(PathBuf::from("/nonexistent"), Some(client()));
        let lux = amb.rx.recv_timeout(Duration::from_secs(3)).expect("a reading");
        assert_eq!(lux, 432.0);
        assert_eq!(amb.status(), "ambient: iio-sensor-proxy (lux)");

        let sys = std::env::temp_dir().join(format!("tideshell-dbus-sys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&sys);
        std::fs::create_dir_all(sys.join("class/backlight/panel")).unwrap();
        std::fs::write(sys.join("class/backlight/panel/max_brightness"), "1000").unwrap();
        std::fs::write(sys.join("class/backlight/panel/brightness"), "250").unwrap();
        calls.lock().unwrap().clear();
        let out = BacklightOut::spawn_with(&sys, Some(client()), None);
        assert!((out.initial.unwrap() - raw_to_level(250, 1000)).abs() < 1e-6);
        // A ramp: many values in a burst. Coalescing may drop middle ones,
        // but the LAST value must arrive, via logind, not sysfs.
        for i in 0..=50 {
            out.set(0.5 + i as f32 * 0.01);
        }
        let want = level_to_raw(1.0, 1000);
        let mut ok = false;
        for _ in 0..100 {
            if calls.lock().unwrap().last().map(|c| c.2) == Some(want) {
                ok = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let n = calls.lock().unwrap().len();
        assert!(ok, "final level reached logind: {:?}", calls.lock().unwrap().last());
        assert!(n <= 51, "{n} calls");
        assert!(out.status().contains("via logind"), "{}", out.status());
        assert_eq!(std::fs::read_to_string(sys.join("class/backlight/panel/brightness")).unwrap(), "250", "sysfs untouched when logind works");
        let _ = std::fs::remove_dir_all(&sys);
    }
}
