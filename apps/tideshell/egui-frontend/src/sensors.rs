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
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Pauses the polling sensor threads while the screen is asleep. A paused
/// thread blocks on a condvar (no wakeups at all) and, for iio-sensor-proxy
/// sensors, RELEASES its claim first, so the proxy can stop the sensor itself
/// instead of streaming readings nobody reads. Proximity is never gated: the
/// triple-tap wake needs it to reject pocket touches.
#[derive(Clone)]
pub struct SensorGate(Arc<(Mutex<bool>, Condvar)>);

impl Default for SensorGate {
    fn default() -> Self {
        SensorGate(Arc::new((Mutex::new(true), Condvar::new())))
    }
}

impl SensorGate {
    pub fn set_active(&self, active: bool) {
        let (m, cv) = &*self.0;
        let mut g = m.lock().unwrap();
        if *g != active {
            *g = active;
            cv.notify_all();
        }
    }
    pub fn is_active(&self) -> bool {
        *self.0 .0.lock().unwrap()
    }
    /// Block until active. Returns true if it had to wait (the caller re-claims).
    pub fn wait_active(&self) -> bool {
        let (m, cv) = &*self.0;
        let mut g = m.lock().unwrap();
        let waited = !*g;
        while !*g {
            g = cv.wait(g).unwrap();
        }
        waited
    }
}

// ---- local clock -----------------------------------------------------------

/// Minutes after local midnight (uses the system timezone via libc).
pub fn local_minute() -> u32 {
    let (h, m) = local_hm();
    h * 60 + m
}

/// Day of the year, 1..366 (libc tm_yday + 1).
pub fn local_yday() -> u32 {
    // SAFETY: as local_hm.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return 172;
        }
        (tm.tm_yday + 1).clamp(1, 366) as u32
    }
}

/// The BSSID and SSID of the connected WiFi from `nmcli`, if any
/// (`nmcli -t -f ACTIVE,SSID,BSSID dev wifi`; BSSID colons are escaped).
pub fn wifi_place() -> Option<(String, String)> {
    let out = std::process::Command::new("nmcli").args(["-t", "-f", "ACTIVE,SSID,BSSID", "dev", "wifi"]).stdin(std::process::Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_nmcli_wifi(&String::from_utf8_lossy(&out.stdout))
}

/// (bssid, ssid) of the active line. nmcli escapes ':' in the BSSID as '\:'.
pub fn parse_nmcli_wifi(text: &str) -> Option<(String, String)> {
    for line in text.lines() {
        let un = line.replace("\\:", "\u{0}");
        let mut parts = un.split(':');
        let active = parts.next()?;
        if active != "yes" {
            continue;
        }
        let ssid = parts.next()?.to_string();
        let bssid = parts.next()?.replace('\u{0}', ":").to_ascii_lowercase();
        if bssid.is_empty() {
            continue;
        }
        return Some((bssid, ssid));
    }
    None
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
    ((level.clamp(0.0, 1.0).powf(GAMMA) * max as f32).round() as u32).clamp(raw_floor(max), max.max(1))
}

/// The dimmest raw value ever sent: 1/256 of the panel's range (16 of the
/// FP5's 4095), never less than 1.
///
/// It was 1 -- chosen so an OLED is not switched OFF by a 0. But 1 of 4095 is
/// 0.02% of full brightness, indistinguishable from off: auto-brightness in a
/// dark room (or in a pocket, face down) drove the screen BLACK, and the
/// ambient core's MIN_LEVEL (0.02) maps to exactly that through the gamma.
/// 1/256 is dim but visibly lit. A judgement, not a measurement of this panel.
pub fn raw_floor(max: u32) -> u32 {
    max.div_ceil(256).max(1)
}

/// Whether a brightness value should be written NOW. Pure, so the rules are
/// tested without a panel.
///
/// * Never while the display is OFF. On this panel (`ae94000.dsi.0`) a
///   brightness write is a command over the DSI link; sent to a powered-down
///   panel it fails, and the DSI host logs the error -- the "dsi err worker"
///   lines in the journal. Auto-brightness kept writing through every screen-
///   off, including cosmic's own lock-screen and idle blanking.
/// * Never the raw value already written. A small level change often maps
///   to the same raw value, and each redundant write is still a DSI command.
/// * On power-ON, the latest level is written even if unchanged: the panel
///   was re-powered, and the value it holds is not to be trusted.
#[derive(Debug, Clone, Default)]
pub struct WriteGate {
    display_on: bool,
    last_written: Option<u32>,
    latest: Option<u32>,
}

impl WriteGate {
    pub fn new() -> Self {
        WriteGate { display_on: true, last_written: None, latest: None }
    }
    pub fn display_on(&self) -> bool {
        self.display_on
    }
    /// A new wanted value; `Some(raw)` if it should be written now.
    pub fn level(&mut self, raw: u32) -> Option<u32> {
        self.latest = Some(raw);
        (self.display_on && self.last_written != Some(raw)).then_some(raw)
    }
    /// The display changed power; `Some(raw)` to re-assert on power-on.
    pub fn display(&mut self, on: bool) -> Option<u32> {
        let was = self.display_on;
        self.display_on = on;
        if !on {
            return None;
        }
        // Newly on: re-assert. Already on but nothing written since waking (a
        // re-assert that failed because the panel was not ready yet): the
        // compositor's later confirmation re-asserts again. This also makes
        // the gate safe to feed from both tideshell and the compositor.
        if !was {
            self.last_written = None;
        }
        if self.last_written.is_none() { self.latest } else { None }
    }
    /// A write of `raw` succeeded.
    pub fn written(&mut self, raw: u32) {
        self.last_written = Some(raw);
    }
}

/// What the writer thread is told.
#[derive(Debug, Clone, Copy)]
pub enum BacklightMsg {
    Level(f32),
    Display(bool),
}

/// A panel just powered up needs a moment before it accepts commands.
pub const WAKE_SETTLE: Duration = Duration::from_millis(120);
/// Consecutive failures before a write method is abandoned for the next.
/// It was ONE: a single failure (a write racing a screen-off, say) moved on
/// for good -- logind, then brightnessctl, then sysfs, which a normal user
/// usually cannot write -- and brightness could never be set again.
pub const DEMOTE_AFTER: u32 = 3;

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

/// Battery temperature in °C from the first Battery power supply (sysfs
/// `temp` is in tenths of a degree), for cold mode.
pub fn battery_temp_c(sys: &Path) -> Option<f32> {
    let d = std::fs::read_dir(sys.join("class/power_supply")).ok()?.flatten().map(|e| e.path()).find(|p| std::fs::read_to_string(p.join("type")).map(|t| t.trim() == "Battery").unwrap_or(false))?;
    read_f32(&d.join("temp")).map(|t| t / 10.0)
}

// ---- orientation (iio-sensor-proxy accelerometer) ------------------------------

/// Orientation strings ("normal", "left-up", ...) as they change, ~5 Hz.
pub struct OrientationFeed {
    pub rx: Receiver<String>,
    status: Arc<Mutex<String>>,
}

impl OrientationFeed {
    pub fn spawn(bus: Option<Connection>) -> OrientationFeed {
        Self::spawn_gated(bus, SensorGate::default())
    }

    pub fn spawn_gated(bus: Option<Connection>, gate: SensorGate) -> OrientationFeed {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new("orientation: starting".to_string()));
        let st = status.clone();
        std::thread::Builder::new()
            .name("tideshell-orientation".into())
            .spawn(move || {
                let set = |s: String| *st.lock().unwrap() = s;
                let Some(bus) = bus else {
                    set("orientation: no system bus".into());
                    return;
                };
                let p = match sensor_proxy(&bus) {
                    Ok(p) => p,
                    Err(e) => {
                        set(format!("orientation: iio-sensor-proxy unavailable ({e})"));
                        return;
                    }
                };
                match p.get_property::<bool>("HasAccelerometer") {
                    Ok(true) => {}
                    Ok(false) => {
                        set("orientation: iio-sensor-proxy reports no accelerometer".into());
                        return;
                    }
                    Err(e) => {
                        set(format!("orientation: HasAccelerometer: {e}"));
                        return;
                    }
                }
                if let Err(e) = p.call_method("ClaimAccelerometer", &()) {
                    set(format!("orientation: ClaimAccelerometer: {e}"));
                    return;
                }
                set("orientation: iio-sensor-proxy accelerometer".into());
                let mut last = String::new();
                loop {
                    if !gate.is_active() {
                        let _ = p.call_method("ReleaseAccelerometer", &());
                        set("orientation: paused (screen asleep; claim released)".into());
                        gate.wait_active();
                        if let Err(e) = p.call_method("ClaimAccelerometer", &()) {
                            set(format!("orientation: re-claim: {e}"));
                            return;
                        }
                        set("orientation: iio-sensor-proxy accelerometer".into());
                        // Re-send whatever it reads now: the phone may have turned while asleep.
                        last.clear();
                    }
                    match p.get_property::<String>("AccelerometerOrientation") {
                        Ok(o) => {
                            if o != last {
                                last = o.clone();
                                if tx.send(o).is_err() {
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            set(format!("orientation: read failed ({e})"));
                            return;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            })
            .expect("spawn orientation thread");
        OrientationFeed { rx, status }
    }

    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }
}

// ---- proximity (iio-sensor-proxy) -------------------------------------------------

/// `true` = near, on change, ~5 Hz.
pub struct ProximityFeed {
    pub rx: Receiver<bool>,
    status: Arc<Mutex<String>>,
}

impl ProximityFeed {
    pub fn spawn(bus: Option<Connection>) -> ProximityFeed {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new("proximity: starting".to_string()));
        let st = status.clone();
        std::thread::Builder::new()
            .name("tideshell-proximity".into())
            .spawn(move || {
                let set = |s: String| *st.lock().unwrap() = s;
                let Some(bus) = bus else {
                    set("proximity: no system bus".into());
                    return;
                };
                let p = match sensor_proxy(&bus) {
                    Ok(p) => p,
                    Err(e) => {
                        set(format!("proximity: iio-sensor-proxy unavailable ({e})"));
                        return;
                    }
                };
                if !p.get_property::<bool>("HasProximitySensor").unwrap_or(false) {
                    set("proximity: no proximity sensor".into());
                    return;
                }
                if let Err(e) = p.call_method("ClaimProximity", &()) {
                    set(format!("proximity: ClaimProximity: {e}"));
                    return;
                }
                set("proximity: iio-sensor-proxy".into());
                let mut last: Option<bool> = None;
                loop {
                    match p.get_property::<bool>("ProximityNear") {
                        Ok(near) => {
                            if last != Some(near) {
                                last = Some(near);
                                if tx.send(near).is_err() {
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            set(format!("proximity: read failed ({e})"));
                            return;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            })
            .expect("spawn proximity thread");
        ProximityFeed { rx, status }
    }

    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }
}

// ---- IMU (IIO sysfs gyro + accelerometer) -----------------------------------------------

/// An IIO device with an angular-velocity or acceleration triplet.
#[derive(Debug, Clone, PartialEq)]
pub struct IioVec {
    pub dir: PathBuf,
    pub prefix: &'static str,
    pub scale: f32,
}

impl IioVec {
    pub fn read(&self) -> Option<[f32; 3]> {
        let axis = |a: &str| read_f32(&self.dir.join(format!("{}_{a}_raw", self.prefix)));
        Some([axis("x")? * self.scale, axis("y")? * self.scale, axis("z")? * self.scale])
    }
}

/// Find `in_anglvel` (gyro, rad/s after scale) and `in_accel` (m/s²)
/// devices under `<sys>/bus/iio/devices`.
pub fn find_iio_vec(sys: &Path, prefix: &'static str) -> Option<IioVec> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(sys.join("bus/iio/devices")).ok()?.flatten().map(|e| e.path()).collect();
    dirs.sort();
    dirs.into_iter().find(|d| d.join(format!("{prefix}_x_raw")).exists()).map(|dir| {
        let scale = read_f32(&dir.join(format!("{prefix}_scale"))).unwrap_or(1.0);
        IioVec { dir, prefix, scale }
    })
}

/// (gyro rad/s, accel) samples at ~50 Hz while the receiver listens.
pub struct ImuFeed {
    pub rx: Receiver<([f32; 3], [f32; 3])>,
    status: Arc<Mutex<String>>,
}

impl ImuFeed {
    pub fn spawn(sys: PathBuf) -> ImuFeed {
        Self::spawn_gated(sys, SensorGate::default())
    }

    pub fn spawn_gated(sys: PathBuf, gate: SensorGate) -> ImuFeed {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(String::new()));
        let st = status.clone();
        std::thread::Builder::new()
            .name("tideshell-imu".into())
            .spawn(move || {
                let (gyro, accel) = (find_iio_vec(&sys, "in_anglvel"), find_iio_vec(&sys, "in_accel"));
                let msg = match (&gyro, &accel) {
                    (Some(g), Some(a)) => format!("imu: gyro {} accel {}", g.dir.display(), a.dir.display()),
                    (None, Some(a)) => format!("imu: no gyro in IIO (accel {}); the camera world will follow gravity only", a.dir.display()),
                    (Some(g), None) => format!("imu: gyro {} but no accelerometer; no gravity correction", g.dir.display()),
                    (None, None) => "imu: no gyro or accelerometer in IIO sysfs".into(),
                };
                *st.lock().unwrap() = msg;
                if gyro.is_none() && accel.is_none() {
                    return;
                }
                loop {
                    // 50 Hz of sysfs reads is the most frequent timer in the shell; asleep it is all waste.
                    gate.wait_active();
                    let g = gyro.as_ref().and_then(|g| g.read()).unwrap_or([0.0; 3]);
                    let a = accel.as_ref().and_then(|a| a.read()).unwrap_or([0.0, 0.0, 0.0]);
                    if tx.send((g, a)).is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            })
            .expect("spawn imu thread");
        ImuFeed { rx, status }
    }

    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }
}

/// Compass heading (degrees) from iio-sensor-proxy, on change.
pub struct CompassFeed {
    pub rx: Receiver<f32>,
    status: Arc<Mutex<String>>,
}

impl CompassFeed {
    pub fn spawn(bus: Option<Connection>) -> CompassFeed {
        Self::spawn_gated(bus, SensorGate::default())
    }

    pub fn spawn_gated(bus: Option<Connection>, gate: SensorGate) -> CompassFeed {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new("compass: starting".to_string()));
        let st = status.clone();
        std::thread::Builder::new()
            .name("tideshell-compass".into())
            .spawn(move || {
                let set = |s: String| *st.lock().unwrap() = s;
                let Some(bus) = bus else {
                    set("compass: no system bus".into());
                    return;
                };
                let p = match sensor_proxy_compass(&bus) {
                    Ok(p) => p,
                    Err(e) => {
                        set(format!("compass: {e}"));
                        return;
                    }
                };
                if !p.get_property::<bool>("HasCompass").unwrap_or(false) {
                    set("compass: iio-sensor-proxy reports none".into());
                    return;
                }
                if let Err(e) = p.call_method("ClaimCompass", &()) {
                    set(format!("compass: ClaimCompass: {e}"));
                    return;
                }
                set("compass: iio-sensor-proxy".into());
                let mut last = f32::NAN;
                loop {
                    if !gate.is_active() {
                        let _ = p.call_method("ReleaseCompass", &());
                        set("compass: paused (screen asleep; claim released)".into());
                        gate.wait_active();
                        if let Err(e) = p.call_method("ClaimCompass", &()) {
                            set(format!("compass: re-claim: {e}"));
                            return;
                        }
                        set("compass: iio-sensor-proxy".into());
                        last = f32::NAN;
                    }
                    match p.get_property::<f64>("CompassHeading") {
                        Ok(h) => {
                            let h = h as f32;
                            if (h - last).abs() > 0.5 {
                                last = h;
                                if tx.send(h).is_err() {
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            set(format!("compass: read failed ({e})"));
                            return;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            })
            .expect("spawn compass thread");
        CompassFeed { rx, status }
    }

    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }
}

fn sensor_proxy_compass(bus: &Connection) -> zbus::Result<Proxy<'static>> {
    Proxy::new(bus, "net.hadess.SensorProxy", "/net/hadess/SensorProxy/Compass", "net.hadess.SensorProxy.Compass")
}

// ---- microphone level (pw-record) -----------------------------------------------------

/// dBFS per 100 ms window from the default source, through `pw-record`
/// writing raw f32 mono to stdout. Only while the receiver listens; the
/// child is killed on drop.
pub struct MicFeed {
    pub rx: Receiver<f32>,
    pub status: String,
    child: Option<std::process::Child>,
}

impl MicFeed {
    pub fn spawn() -> MicFeed {
        Self::spawn_with(&["pw-record".into(), "--format=f32".into(), "--rate=16000".into(), "--channels=1".into(), "-".into()], 16000)
    }

    /// `argv` writes raw f32 LE mono samples at `rate` to stdout.
    pub fn spawn_with(argv: &[String], rate: usize) -> MicFeed {
        let (tx, rx) = mpsc::channel();
        let mut cmd = std::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
        match cmd.spawn() {
            Ok(mut child) => {
                let mut out = child.stdout.take().expect("piped");
                let window = rate / 10; // 100 ms
                std::thread::Builder::new()
                    .name("tideshell-mic".into())
                    .spawn(move || {
                        use std::io::Read;
                        let mut buf = vec![0u8; window * 4];
                        loop {
                            let mut filled = 0;
                            while filled < buf.len() {
                                match out.read(&mut buf[filled..]) {
                                    Ok(0) | Err(_) => return,
                                    Ok(n) => filled += n,
                                }
                            }
                            let samples: Vec<f32> = buf.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
                            if tx.send(tideshell_core::ambient_sound::rms_dbfs(&samples)).is_err() {
                                return;
                            }
                        }
                    })
                    .expect("spawn mic thread");
                MicFeed { rx, status: format!("mic: {} at {rate} Hz, 100 ms windows", argv[0]), child: Some(child) }
            }
            Err(e) => MicFeed { rx, status: format!("mic: cannot start {}: {e} (PipeWire tools?)", argv[0]), child: None },
        }
    }
}

impl Drop for MicFeed {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
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
        Self::spawn_gated(sys, bus, SensorGate::default())
    }

    pub fn spawn_gated(sys: PathBuf, bus: Option<Connection>, gate: SensorGate) -> Ambient {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new("ambient: starting".to_string()));
        let st = status.clone();
        std::thread::Builder::new()
            .name("tideshell-ambient".into())
            .spawn(move || ambient_loop(sys, bus, tx, st, gate))
            .expect("spawn ambient thread");
        Ambient { rx, status }
    }

    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }
}

fn ambient_loop(sys: PathBuf, bus: Option<Connection>, tx: Sender<f32>, st: Arc<Mutex<String>>, gate: SensorGate) {
    let set = |s: String| *st.lock().unwrap() = s;
    if let Some(bus) = bus {
        match sensor_proxy_claim(&bus) {
            Ok(unit) => {
                set(format!("ambient: iio-sensor-proxy ({unit})"));
                loop {
                    if !gate.is_active() {
                        // Nobody reads brightness for a dark panel: let the proxy stop the sensor.
                        if let Ok(p) = sensor_proxy(&bus) {
                            let _ = p.call_method("ReleaseLight", &());
                        }
                        set("ambient: paused (screen asleep; claim released)".into());
                        gate.wait_active();
                        let _ = sensor_proxy_claim(&bus);
                        set(format!("ambient: iio-sensor-proxy ({unit})"));
                    }
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
                gate.wait_active();
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
    tx: Option<Sender<BacklightMsg>>,
    status: Arc<Mutex<String>>,
    /// Write interval in microseconds; `set_rate` changes it live.
    interval_us: Arc<std::sync::atomic::AtomicU64>,
    /// 0 smooth, 1 stepped, 2 safe; `set_mode` changes it live.
    mode: Arc<std::sync::atomic::AtomicU8>,
    pub backlight_name: String,
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
        let interval_us = Arc::new(std::sync::atomic::AtomicU64::new(1_000_000 / 10));
        let mode = Arc::new(std::sync::atomic::AtomicU8::new(0));
        let Some(bl) = find_backlight(sys) else {
            *status.lock().unwrap() = "backlight: none found under /sys/class/backlight".into();
            return BacklightOut { tx: None, status, interval_us, mode, initial: None, backlight_name: String::new() };
        };
        let initial = bl.read_level();
        let (tx, rx) = mpsc::channel::<BacklightMsg>();
        let st = status.clone();
        let iv = interval_us.clone();
        let md = mode.clone();
        let backlight_name = bl.name.clone();
        mode.store(match WriteMode::auto_for(&backlight_name) { WriteMode::Smooth => 0, WriteMode::Stepped => 1, WriteMode::Safe => 2 }, std::sync::atomic::Ordering::Relaxed);
        std::thread::Builder::new()
            .name("tideshell-backlight".into())
            .spawn(move || backlight_loop(bl, bus, tool, rx, st, iv, md))
            .expect("spawn backlight thread");
        BacklightOut { tx: Some(tx), status, interval_us, mode, initial, backlight_name }
    }

    pub fn set(&self, level: f32) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(BacklightMsg::Level(level));
        }
    }

    /// The display's power changed -- by tideshell or by the compositor.
    /// While off nothing is written; on power-on the level is re-asserted.
    pub fn set_display(&self, on: bool) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(BacklightMsg::Display(on));
        }
    }

    /// The write mode; "auto" picks by the backlight's name.
    pub fn set_mode(&self, pref: &str) -> WriteMode {
        let m = WriteMode::parse(pref).unwrap_or_else(|| WriteMode::auto_for(&self.backlight_name));
        self.mode.store(match m { WriteMode::Smooth => 0, WriteMode::Stepped => 1, WriteMode::Safe => 2 }, std::sync::atomic::Ordering::Relaxed);
        m
    }

    /// Maximum write rate in Hz (the loop never writes faster).
    pub fn set_rate(&self, hz: u32) {
        self.interval_us.store(1_000_000 / hz.clamp(1, 90) as u64, std::sync::atomic::Ordering::Relaxed);
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
// The write rate on the cheap paths is `BacklightOut::set_rate` (default
// 30 Hz). It was one frame of a 90 Hz panel: on the FP5 every write is a DSI
// command to the panel, and up to 90 a second competed with the display's own
// traffic.

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

/// After a failed write, the interval doubles (up to this) and halves back
/// on each success: a driver that objects to the rate is not hammered.
pub const BACKOFF_MAX: Duration = Duration::from_secs(1);

/// How values reach the panel. `Safe` exists for the FP5 (pmaports #4274):
/// its DPU driver corrupts the display, flickers, or switches the panel
/// off under a STREAM of brightness changes, while single writes are fine
/// ("tapping a spot in the slider works; dragging breaks it"). So Safe
/// holds a value until nothing new has arrived for SETTLE_MS and writes
/// once; Stepped writes only >= 5% changes at most twice a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    Smooth,
    Stepped,
    Safe,
}

pub const SETTLE_MS: u64 = 800;

impl WriteMode {
    pub fn parse(s: &str) -> Option<WriteMode> {
        match s {
            "smooth" => Some(WriteMode::Smooth),
            "stepped" => Some(WriteMode::Stepped),
            "safe" => Some(WriteMode::Safe),
            _ => None,
        }
    }

    /// `auto`: Safe on a DSI panel backlight, Smooth elsewhere.
    pub fn auto_for(backlight_name: &str) -> WriteMode {
        if backlight_name.contains("dsi") {
            WriteMode::Safe
        } else {
            WriteMode::Smooth
        }
    }
}

fn backlight_loop(bl: Backlight, bus: Option<Connection>, tool: Option<PathBuf>, rx: Receiver<BacklightMsg>,
                  st: Arc<Mutex<String>>, interval_us: Arc<std::sync::atomic::AtomicU64>,
                  mode: Arc<std::sync::atomic::AtomicU8>) {
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
    let mut gate = WriteGate::new();
    let mut failures = 0u32;
    let mut backoff: Option<Duration> = None;
    let mut last_raw: Option<u32> = None;
    while let Ok(first) = rx.recv() {
        // The configured rate on the cheap paths (logind is one D-Bus call,
        // sysfs one write); brightnessctl is a process per call, so <= 20 Hz.
        let configured = Duration::from_micros(interval_us.load(std::sync::atomic::Ordering::Relaxed));
        let mut interval = if methods[cur] == Method::Brightnessctl { configured.max(MIN_WRITE_INTERVAL) } else { configured };
        if let Some(b) = backoff {
            interval = interval.max(b);
        }
        if let Some(t) = last_write {
            let since = t.elapsed();
            if since < interval {
                std::thread::sleep(interval - since);
            }
        }
        // Coalesce: apply every waiting message in order; the gate decides
        // what (if anything) to write once they are all in.
        let mut want: Option<u32> = None;
        let mut woke = false;
        for msg in std::iter::once(first).chain(std::iter::from_fn(|| rx.try_recv().ok())) {
            match msg {
                BacklightMsg::Level(l) => {
                    if let Some(r) = gate.level(level_to_raw(l, bl.max)) { want = Some(r); }
                }
                BacklightMsg::Display(on) => {
                    want = gate.display(on);
                    woke |= on && want.is_some();
                    if !on { want = None; }
                }
            }
        }
        // The write mode (FP5, pmaports #4274): the DPU driver misbehaves under a STREAM of
        // brightness changes even when rate-limited, while single writes are fine. Safe keeps
        // taking messages until none has arrived for SETTLE_MS; Stepped until a >= 5% step
        // against the last written value has built up. Display messages keep their meaning
        // (through the same gate); a display-off ends the wait with nothing to write.
        let mode_now = match mode.load(std::sync::atomic::Ordering::Relaxed) {
            2 => WriteMode::Safe,
            1 => WriteMode::Stepped,
            _ => WriteMode::Smooth,
        };
        if mode_now != WriteMode::Smooth && want.is_some() && !woke {
            let wait = if mode_now == WriteMode::Safe { Duration::from_millis(SETTLE_MS) } else { Duration::from_millis(500) };
            loop {
                match rx.recv_timeout(wait) {
                    Ok(BacklightMsg::Level(l)) => {
                        if let Some(r) = gate.level(level_to_raw(l, bl.max)) { want = Some(r); }
                        if mode_now == WriteMode::Stepped {
                            if let (Some(w), Some(lw)) = (want, last_raw) {
                                if (w as i64 - lw as i64).unsigned_abs() as f32 >= bl.max as f32 * 0.05 { break; }
                            }
                        }
                    }
                    Ok(BacklightMsg::Display(on)) => {
                        want = gate.display(on);
                        woke |= on && want.is_some();
                        if !on { want = None; break; }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(_) => return,
                }
            }
        }
        let Some(raw) = want else { continue };
        if woke {
            std::thread::sleep(WAKE_SETTLE);
        }
        // Try from the current method; later ones only for THIS write until
        // the current one has failed DEMOTE_AFTER times in a row.
        let mut i = cur;
        let mut cur_err: Option<String> = None;
        loop {
            let r = match methods[i] {
                Method::Logind => match (&bus, &session) {
                    (Some(b), Some(Ok(p))) => logind_set_brightness(b, p, &bl.name, raw),
                    _ => Err("no logind session".into()),
                },
                Method::Brightnessctl => brightnessctl_set(tool.as_deref().unwrap(), &bl.name, raw),
                Method::Sysfs => bl.write_sysfs(raw).map_err(|e| format!("sysfs: {e}")),
            };
            match r {
                Ok(()) => {
                    gate.written(raw);
                    last_raw = Some(raw);
                    backoff = backoff.and_then(|b| (b > interval * 2).then_some(b / 2));
                    if i == cur {
                        if failures > 0 { set(note(cur, &skipped)); }
                        failures = 0;
                    } else if let Some(e) = &cur_err {
                        // Say what ACTUALLY applied this write, and why the
                        // preferred method did not -- not a status claiming
                        // a method works while writes go elsewhere.
                        let mut extra = skipped.clone();
                        extra.push(format!("{e} ({failures} of {DEMOTE_AFTER} before abandoning it)"));
                        set(note(i, &extra));
                    }
                    break;
                }
                Err(e) => {
                    if i == cur {
                        cur_err = Some(e.clone());
                        failures += 1;
                        if failures >= DEMOTE_AFTER && cur + 1 < methods.len() {
                            skipped.push(format!("{e} ({failures} in a row)"));
                            cur += 1;
                            failures = 0;
                            set(note(cur, &skipped));
                        }
                    }
                    if i + 1 < methods.len() {
                        i += 1;
                    } else {
                        // Every method failed for this write: back off, so a
                        // driver that objects is not hammered.
                        backoff = Some(backoff.map_or(interval * 2, |b| (b * 2).min(BACKOFF_MAX)));
                        set(format!("backlight: cannot set {} (all methods failed; last: {e}; backing off to {:?}) [{}]",
                                    bl.name, backoff.unwrap(), skipped.join("; ")));
                        break;
                    }
                }
            }
        }
        last_write = Some(std::time::Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE BLACK SCREEN: the ambient core's floor (MIN_LEVEL 0.02) went to
    /// raw 1 of 4095 through the gamma -- 0.02%, indistinguishable from off.
    #[test]
    fn the_dimmest_setting_is_visibly_lit_not_black() {
        assert_eq!(level_to_raw(0.02, 4095), 16, "was 1");
        assert_eq!(level_to_raw(0.0, 4095), 16);
        assert_eq!(raw_floor(255), 1, "a small range keeps its own floor");
        assert!(level_to_raw(0.5, 4095) > 16, "above the floor the curve is unchanged");
    }

    /// NO WRITES TO A DARK PANEL: while the display is off nothing is written
    /// (a DSI command to a powered-down panel is the journal's dsi error), and
    /// on power-on the newest level is re-asserted even if unchanged.
    #[test]
    fn nothing_is_written_while_the_display_is_off_and_power_on_reasserts() {
        let mut g = WriteGate::new();
        assert_eq!(g.level(500), Some(500));
        g.written(500);
        assert_eq!(g.display(false), None);
        assert_eq!(g.level(900), None, "off: not written");
        assert_eq!(g.level(40), None, "still off");
        assert_eq!(g.display(true), Some(40), "on: the NEWEST level, re-asserted");
        // That write FAILED (panel not ready): the compositor's confirmation
        // that the display is on re-asserts again.
        assert_eq!(g.display(true), Some(40), "confirmation retries a failed re-assert");
        g.written(40);
        assert_eq!(g.display(true), None, "written since waking: nothing to re-assert");
    }

    /// NO REDUNDANT COMMANDS: the same raw value twice is one write.
    #[test]
    fn an_unchanged_raw_value_is_not_written_again() {
        let mut g = WriteGate::new();
        assert_eq!(g.level(300), Some(300));
        g.written(300);
        assert_eq!(g.level(300), None);
        assert_eq!(g.level(301), Some(301));
        // A write that FAILED leaves nothing recorded, so the value is retried.
        assert_eq!(g.level(302), Some(302));
        assert_eq!(g.level(302), Some(302), "not written yet: still wanted");
    }

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
                if raw < 4 || raw <= raw_floor(max) {
                    continue; // below the scale's resolution, or clamped to the floor on purpose
                }
                let back = raw_to_level(raw, max);
                assert!((back - l).abs() < 0.03, "{l} -> {back} at max {max}");
            }
            assert_eq!(level_to_raw(0.0, max), raw_floor(max), "the floor, never 0 (an OLED can switch off)");
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
        // The write landed via sysfs, and the status says so, and why.
        assert!(wait_for(|| out.status().contains("via sysfs")), "{}", out.status());
        let st = out.status();
        assert!(st.contains("brightnessctl exited 1") && st.contains("1 of 3"), "{st}");
        // ONE failure no longer abandons a method for good; THREE in a row do.
        for (level, raw) in [(0.6f32, level_to_raw(0.6, 4095)), (0.3, level_to_raw(0.3, 4095))] {
            out.set(level);
            assert!(wait_for(|| std::fs::read_to_string(&path).unwrap_or_default() == raw.to_string()));
        }
        assert!(wait_for(|| out.status().contains("3 in a row")), "abandoned after three: {}", out.status());
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn every_method_failing_is_reported_with_a_backoff() {
        let sys = tmp("bl-allfail");
        put(&sys.join("class/backlight/panel/max_brightness"), "255");
        // A directory where the brightness file should be: every write fails.
        std::fs::create_dir_all(sys.join("class/backlight/panel/brightness")).unwrap();
        let out = BacklightOut::spawn_with(&sys, None, None);
        out.set(0.7);
        assert!(wait_for(|| out.status().contains("backing off")), "{}", out.status());
        assert!(out.status().contains("cannot set panel"));
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn iio_imu_discovery_and_scaling() {
        let sys = tmp("imu");
        put(&sys.join("bus/iio/devices/iio:device2/in_anglvel_x_raw"), "1000");
        put(&sys.join("bus/iio/devices/iio:device2/in_anglvel_y_raw"), "-500");
        put(&sys.join("bus/iio/devices/iio:device2/in_anglvel_z_raw"), "0");
        put(&sys.join("bus/iio/devices/iio:device2/in_anglvel_scale"), "0.001");
        put(&sys.join("bus/iio/devices/iio:device3/in_accel_x_raw"), "0");
        put(&sys.join("bus/iio/devices/iio:device3/in_accel_y_raw"), "9810");
        put(&sys.join("bus/iio/devices/iio:device3/in_accel_z_raw"), "0");
        put(&sys.join("bus/iio/devices/iio:device3/in_accel_scale"), "0.001");
        let g = find_iio_vec(&sys, "in_anglvel").unwrap();
        assert_eq!(g.read(), Some([1.0, -0.5, 0.0]));
        let a = find_iio_vec(&sys, "in_accel").unwrap();
        assert_eq!(a.read(), Some([0.0, 9.81, 0.0]));
        assert!(find_iio_vec(&sys, "in_magn").is_none());
        let feed = ImuFeed::spawn(sys.clone());
        let (gy, ac) = feed.rx.recv_timeout(Duration::from_secs(3)).expect("a sample");
        assert_eq!((gy, ac), ([1.0, -0.5, 0.0], [0.0, 9.81, 0.0]));
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn mic_levels_come_from_raw_f32_windows() {
        // 0.5 amplitude square wave for 0.3 s at 1000 Hz -> three -6 dBFS windows.
        let script = "import sys,struct
sys.stdout.buffer.write(b''.join(struct.pack('<f', 0.5 if (i//5)%2==0 else -0.5) for i in range(300)))
";
        let feed = MicFeed::spawn_with(&["python3".into(), "-c".into(), script.into()], 1000);
        assert!(feed.child.is_some(), "{}", feed.status);
        let mut got = vec![];
        for _ in 0..3 {
            if let Ok(v) = feed.rx.recv_timeout(Duration::from_secs(3)) {
                got.push(v);
            }
        }
        assert_eq!(got.len(), 3, "{got:?}");
        assert!(got.iter().all(|v| (v + 6.02).abs() < 0.1), "{got:?}");
    }

    #[test]
    fn battery_temperature_is_tenths_of_a_degree() {
        let sys = tmp("bt");
        put(&sys.join("class/power_supply/usb/type"), "USB");
        put(&sys.join("class/power_supply/battery/type"), "Battery");
        put(&sys.join("class/power_supply/battery/temp"), "63");
        assert_eq!(battery_temp_c(&sys), Some(6.3));
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn nmcli_active_wifi_is_parsed_with_escaped_bssid() {
        let t = "no:Neighbour:AA\\:BB\\:CC\\:DD\\:EE\\:01\nyes:Home Net:AA\\:BB\\:CC\\:DD\\:EE\\:FF\n";
        assert_eq!(parse_nmcli_wifi(t), Some(("aa:bb:cc:dd:ee:ff".to_string(), "Home Net".to_string())));
        assert_eq!(parse_nmcli_wifi("no:x:y\n"), None);
        assert!(local_yday() >= 1 && local_yday() <= 366);
    }

    #[test]
    fn safe_mode_writes_once_after_the_stream_settles() {
        assert_eq!(WriteMode::auto_for("ae94000.dsi.0"), WriteMode::Safe, "the FP5's panel");
        assert_eq!(WriteMode::auto_for("intel_backlight"), WriteMode::Smooth);
        let sys = fake_sys("safe");
        let tool = fake_tool(&sys, 0);
        let out = BacklightOut::spawn_with(&sys, None, Some(tool));
        assert_eq!(out.set_mode("safe"), WriteMode::Safe);
        // A 30-step ramp over 300 ms: with the panel unable to take a stream,
        // exactly one write must land, with the final value.
        for i in 0..=30 {
            out.set(0.3 + i as f32 * 0.02);
            std::thread::sleep(Duration::from_millis(10));
        }
        let calls = sys.join("calls");
        let want = format!("-q -d panel set {}", level_to_raw(0.9, 4095));
        assert!(wait_for(|| std::fs::read_to_string(&calls).unwrap_or_default().lines().last() == Some(want.as_str())), "{:?}", std::fs::read_to_string(&calls).ok());
        std::thread::sleep(Duration::from_millis(300));
        let n = std::fs::read_to_string(&calls).unwrap().lines().count();
        assert_eq!(n, 1, "one write for the whole ramp, not {n}");
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn nmcli_terse_wifi_lines_parse_with_escaped_colons() {
        let s = "no:Neighbours:AA\\:BB\\:CC\\:DD\\:EE\\:01\nyes:Home net:AA\\:BB\\:CC\\:DD\\:EE\\:FF\n";
        assert_eq!(parse_nmcli_wifi(s), Some(("aa:bb:cc:dd:ee:ff".into(), "Home net".into())));
        assert_eq!(parse_nmcli_wifi("no:x:AA\\:BB\n"), None);
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

#[cfg(test)]
mod backlight_merge_tests {
    //! The writer loop after the c20unified/hive merge: hive's machinery (coalescing through
    //! WriteGate, display gating, method fallback) with the FP5 write modes as a receiving
    //! stage. Run for real against a fake sysfs backlight: no bus, no brightnessctl, so the
    //! only method is the `brightness` file, and what reached the "panel" is its content.
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicU8};
    use std::sync::mpsc::channel;

    struct Rig {
        tx: std::sync::mpsc::Sender<BacklightMsg>,
        file: PathBuf,
        max: u32,
        dir: PathBuf,
    }

    fn rig(tag: &str, mode: WriteMode) -> Rig {
        let dir = std::env::temp_dir().join(format!("ts-bl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("brightness"), "0").unwrap();
        let max = 1000;
        let bl = Backlight { name: "test-backlight".into(), dir: dir.clone(), max };
        let (tx, rx) = channel();
        let st = Arc::new(Mutex::new(String::new()));
        let iv = Arc::new(AtomicU64::new(33_333));
        let md = Arc::new(AtomicU8::new(match mode { WriteMode::Smooth => 0, WriteMode::Stepped => 1, WriteMode::Safe => 2 }));
        std::thread::spawn(move || backlight_loop(bl, None, None, rx, st, iv, md));
        Rig { tx, file: dir.join("brightness"), max, dir }
    }
    fn on_panel(r: &Rig) -> u32 { std::fs::read_to_string(&r.file).unwrap().trim().parse().unwrap() }
    fn burst(r: &Rig) {
        for i in 1..=10 {
            r.tx.send(BacklightMsg::Level(i as f32 / 10.0)).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn smooth_writes_promptly() {
        let r = rig("smooth", WriteMode::Smooth);
        burst(&r);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(on_panel(&r), level_to_raw(1.0, r.max), "the burst's last value reached the panel");
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn safe_waits_for_quiet_then_writes_once() {
        let r = rig("safe", WriteMode::Safe);
        burst(&r);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(on_panel(&r), 0, "nothing written while values are still arriving or settling");
        std::thread::sleep(Duration::from_millis(SETTLE_MS + 400));
        assert_eq!(on_panel(&r), level_to_raw(1.0, r.max), "then exactly the last value");
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn safe_drops_a_pending_value_when_the_display_goes_off() {
        let r = rig("safe-off", WriteMode::Safe);
        r.tx.send(BacklightMsg::Level(0.6)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        r.tx.send(BacklightMsg::Display(false)).unwrap();
        std::thread::sleep(Duration::from_millis(SETTLE_MS + 400));
        assert_eq!(on_panel(&r), 0, "no DSI write to a dark panel");
        // Back on: the latest level is re-asserted (hive's gate), after the wake settle.
        r.tx.send(BacklightMsg::Display(true)).unwrap();
        std::thread::sleep(Duration::from_millis(SETTLE_MS + 600));
        assert_eq!(on_panel(&r), level_to_raw(0.6, r.max));
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn stepped_writes_a_real_step_without_waiting_for_quiet() {
        let r = rig("stepped", WriteMode::Stepped);
        r.tx.send(BacklightMsg::Level(0.5)).unwrap();
        std::thread::sleep(Duration::from_millis(700));
        let first = on_panel(&r);
        assert_eq!(first, level_to_raw(0.5, r.max), "a first value is written (after up to 500 ms)");
        // A change under 5 % is held while values keep arriving; a >= 5 % step goes out at once.
        r.tx.send(BacklightMsg::Level(0.505)).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(on_panel(&r), first, "a sub-5 % change is not written yet");
        r.tx.send(BacklightMsg::Level(0.9)).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(on_panel(&r), level_to_raw(0.9, r.max), "a large step is written before 500 ms of quiet");
        let _ = std::fs::remove_dir_all(&r.dir);
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    #[test]
    fn a_paused_gate_blocks_until_resumed_and_reports_that_it_waited() {
        let g = SensorGate::default();
        assert!(!g.wait_active(), "active: returns at once");
        g.set_active(false);
        let g2 = g.clone();
        let t = std::thread::spawn(move || g2.wait_active());
        std::thread::sleep(Duration::from_millis(50));
        assert!(!t.is_finished(), "still blocked while paused");
        g.set_active(true);
        assert!(t.join().unwrap(), "it waited");
    }
}

/// Battery charge (0..1) and whether it is charging, from one `power_supply` directory's
/// `capacity` and `status` text. `None` when the capacity is not a 0..100 number.
pub fn parse_battery(capacity: &str, status: &str) -> Option<(f32, bool)> {
    let pct: f32 = capacity.trim().parse().ok()?;
    if !(0.0..=100.0).contains(&pct) {
        return None;
    }
    Some((pct / 100.0, status.trim().eq_ignore_ascii_case("charging")))
}

/// The first `type = Battery` supply under `root` (`/sys/class/power_supply`).
pub fn read_battery(root: &std::path::Path) -> Option<(f32, bool)> {
    let mut dirs: Vec<_> = std::fs::read_dir(root).ok()?.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for d in dirs {
        if std::fs::read_to_string(d.join("type")).is_ok_and(|t| t.trim() == "Battery") {
            let cap = std::fs::read_to_string(d.join("capacity")).unwrap_or_default();
            let st = std::fs::read_to_string(d.join("status")).unwrap_or_default();
            if let Some(b) = parse_battery(&cap, &st) {
                return Some(b);
            }
        }
    }
    None
}

#[cfg(test)]
mod battery_tests {
    use super::*;

    #[test]
    fn capacity_and_status_parse_and_junk_does_not() {
        assert_eq!(parse_battery("63\n", "Discharging\n"), Some((0.63, false)));
        assert_eq!(parse_battery("100", "Charging"), Some((1.0, true)));
        assert_eq!(parse_battery("101", "Charging"), None);
        assert_eq!(parse_battery("", "Charging"), None);
        assert_eq!(parse_battery("-1", ""), None);
    }

    #[test]
    fn the_battery_supply_is_found_among_usb_and_wireless_ones() {
        let root = std::env::temp_dir().join(format!("ts-ps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (name, ty, cap, st) in [("a-usb", "USB", "", ""), ("qcom-battmgr-bat", "Battery", "63", "Charging"), ("z-wls", "Wireless", "", "")] {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("type"), ty).unwrap();
            if !cap.is_empty() { std::fs::write(d.join("capacity"), cap).unwrap(); }
            std::fs::write(d.join("status"), st).unwrap();
        }
        assert_eq!(read_battery(&root), Some((0.63, true)));
        assert_eq!(read_battery(&root.join("nonexistent")), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
