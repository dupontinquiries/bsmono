//! triggers — shooting without touching the screen: the volume keys, the
//! power key, a shake.
//!
//! None of these reach an egui window. A phone's volume and power keys are
//! `gpio-keys` / PMIC input devices that the compositor handles; winit maps
//! the volume keys to keys egui drops. So they are read where they
//! originate — `/dev/input/event*`, found through `/proc/bus/input/devices`
//! by the keys each device declares — and a shake is read from the IIO
//! accelerometer in sysfs, the same place the ambient-learner reads its
//! sensors.
//!
//! What each costs, said plainly because they are not free:
//!
//! * **volume keys** — reading needs access to the input device (on
//!   postmarketOS the `input` group; otherwise the panel says "permission
//!   denied"). A device that has ONLY a few keys (a phone's volume
//!   rocker) is grabbed while the camera is open, so a press takes a photo
//!   instead of also changing the volume. A keyboard with volume keys on it
//!   is never grabbed — that would take the whole keyboard.
//! * **power key** — logind acts on it (suspend, blank). Opt-in, and while
//!   on, a `systemd-inhibit --what=handle-power-key` child is held so the
//!   press is the shutter's alone. Without logind, the screen may still
//!   blank; the option says so.
//! * **shake** — the IIO accelerometer polled at 50 Hz on a thread.
//!
//! NOT RUN: on a phone. Device discovery, the key bitmaps, the event
//! decoding and the shake detector are tested against captured text and
//! fake files; the grab ioctl and logind are not reachable here.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

pub const KEY_VOLUMEDOWN: u16 = 114;
pub const KEY_VOLUMEUP: u16 = 115;
pub const KEY_POWER: u16 = 116;
const EV_KEY: u16 = 1;
/// A device declaring at most this many keys is a button cluster, safe to grab.
pub const GRAB_MAX_KEYS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger { VolumeUp, VolumeDown, Power, Shake }

impl Trigger {
    pub fn label(self) -> &'static str {
        match self { Trigger::VolumeUp => "volume +", Trigger::VolumeDown => "volume −", Trigger::Power => "power", Trigger::Shake => "shake" }
    }
}

/// An input device as `/proc/bus/input/devices` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDev {
    pub name: String,
    /// `event3` etc.
    pub event: String,
    /// Key codes the device can produce.
    pub keys: Vec<u16>,
}

impl InputDev {
    pub fn has(&self, k: u16) -> bool { self.keys.contains(&k) }
    pub fn grabbable(&self) -> bool { !self.keys.is_empty() && self.keys.len() <= GRAB_MAX_KEYS }
    pub fn path(&self) -> PathBuf { PathBuf::from("/dev/input").join(&self.event) }
}

/// Parse `/proc/bus/input/devices`. `long_bits`: the kernel's word size for
/// the bitmaps (64 on arm64/x86_64).
pub fn parse_devices(text: &str, long_bits: u32) -> Vec<InputDev> {
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let mut name = String::new();
        let mut event = None;
        let mut keys = Vec::new();
        for line in block.lines() {
            if let Some(n) = line.strip_prefix("N: Name=") { name = n.trim_matches('"').to_string(); }
            if let Some(h) = line.strip_prefix("H: Handlers=") {
                event = h.split_whitespace().find(|t| t.starts_with("event")).map(String::from);
            }
            if let Some(b) = line.strip_prefix("B: KEY=") {
                let words: Vec<&str> = b.split_whitespace().collect();
                for (i, w) in words.iter().rev().enumerate() {
                    let v = u64::from_str_radix(w, 16).unwrap_or(0);
                    for bit in 0..long_bits.min(64) {
                        if v >> bit & 1 == 1 { keys.push((i as u32 * long_bits + bit) as u16); }
                    }
                }
            }
        }
        if let Some(event) = event { out.push(InputDev { name, event, keys }); }
    }
    out
}

/// One `struct input_event` on a 64-bit kernel: 16 bytes of time, then
/// type, code (u16) and value (i32).
pub const EVENT_SIZE: usize = 24;

/// Key presses in a buffer of events (value 1 is press; 0 release, 2 repeat).
pub fn decode_presses(buf: &[u8]) -> Vec<u16> {
    buf.chunks_exact(EVENT_SIZE).filter_map(|e| {
        let ty = u16::from_ne_bytes([e[16], e[17]]);
        let code = u16::from_ne_bytes([e[18], e[19]]);
        let value = i32::from_ne_bytes([e[20], e[21], e[22], e[23]]);
        (ty == EV_KEY && value == 1).then_some(code)
    }).collect()
}

/// Which keys to listen for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Keys { pub volume: bool, pub power: bool }

/// The devices worth opening for `want`.
pub fn pick(devs: &[InputDev], want: Keys) -> Vec<InputDev> {
    devs.iter().filter(|d| (want.volume && (d.has(KEY_VOLUMEUP) || d.has(KEY_VOLUMEDOWN))) || (want.power && d.has(KEY_POWER))).cloned().collect()
}

/// Reader threads over input devices, and the logind inhibitor.
pub struct KeyWatch {
    pub rx: Receiver<Trigger>,
    stop: Arc<AtomicBool>,
    /// Per device: what happened when it was opened.
    pub report: Vec<String>,
    inhibitor: Option<std::process::Child>,
}

impl KeyWatch {
    /// Open the devices from `proc_devices` text (normally
    /// `/proc/bus/input/devices`) whose keys are wanted. `dev_dir` is where
    /// their nodes are (normally `/dev/input`).
    pub fn start(proc_devices: &str, dev_dir: &Path, want: Keys) -> KeyWatch {
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mut report = Vec::new();
        for d in pick(&parse_devices(proc_devices, 64), want) {
            let path = dev_dir.join(&d.event);
            match std::fs::File::open(&path) {
                Ok(f) => {
                    let grabbed = d.grabbable() && grab(&f);
                    report.push(format!("{} ({}){}", d.name, path.display(), if grabbed { ", grabbed" } else { "" }));
                    spawn_reader(f, tx.clone(), stop.clone(), want);
                }
                Err(e) => report.push(format!("{}: {} ({e})", d.name, path.display())),
            }
        }
        if report.is_empty() { report.push("no input device with those keys".into()); }
        let inhibitor = if want.power { inhibit_power_key() } else { None };
        if want.power {
            report.push(match inhibitor { Some(_) => "power key: held from logind".into(),
                                          None => "power key: could not inhibit logind — the system may also act on it".into() });
        }
        KeyWatch { rx, stop, report, inhibitor }
    }
}

impl Drop for KeyWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(mut c) = self.inhibitor.take() { let _ = c.kill(); let _ = c.wait(); }
    }
}

fn spawn_reader(mut f: std::fs::File, tx: Sender<Trigger>, stop: Arc<AtomicBool>, want: Keys) {
    std::thread::Builder::new().name("keys".into()).spawn(move || {
        let mut buf = [0u8; EVENT_SIZE * 16];
        loop {
            // A blocking read: the thread ends with the device (EOF/error)
            // or on the first event after `stop`.
            let n = match f.read(&mut buf) { Ok(0) | Err(_) => return, Ok(n) => n };
            if stop.load(Ordering::Relaxed) { return; }
            for code in decode_presses(&buf[..n - n % EVENT_SIZE]) {
                let t = match code {
                    KEY_VOLUMEUP if want.volume => Trigger::VolumeUp,
                    KEY_VOLUMEDOWN if want.volume => Trigger::VolumeDown,
                    KEY_POWER if want.power => Trigger::Power,
                    _ => continue,
                };
                if tx.send(t).is_err() { return; }
            }
        }
    }).ok();
}

/// EVIOCGRAB: exclusive access while held; released when the fd closes.
#[cfg(target_os = "linux")]
fn grab(f: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    extern "C" { fn ioctl(fd: i32, req: std::ffi::c_ulong, ...) -> i32; }
    const EVIOCGRAB: std::ffi::c_ulong = 0x4004_4590;
    // SAFETY: a valid fd for the life of `f`; EVIOCGRAB takes an int by value.
    unsafe { ioctl(f.as_raw_fd(), EVIOCGRAB, 1i32) == 0 }
}
#[cfg(not(target_os = "linux"))]
fn grab(_: &std::fs::File) -> bool { false }

/// The logind inhibitor command — held as a child for as long as the
/// power key is the shutter.
pub fn inhibit_args() -> Vec<&'static str> {
    vec!["--what=handle-power-key", "--who=snapshot-cam", "--why=the power key is the shutter", "--mode=block", "sleep", "infinity"]
}

fn inhibit_power_key() -> Option<std::process::Child> {
    let mut c = std::process::Command::new("systemd-inhibit").args(inhibit_args())
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().ok()?;
    // It exits at once if logind refused.
    std::thread::sleep(std::time::Duration::from_millis(150));
    match c.try_wait() { Ok(None) => Some(c), _ => None }
}

// ── shake ────────────────────────────────────────────────────────────────

/// Shake detection from acceleration samples (m/s²).
///
/// A shake is several sharp jolts in quick succession: the acceleration
/// with the slow part (gravity, the arm's drift) taken out exceeds
/// `JOLT` at least `JOLTS` times, alternating direction, within `WINDOW_MS`.
/// Walking, a bump on a table and turning the phone over do not do that.
#[derive(Debug, Clone)]
pub struct Shake {
    slow: Option<[f32; 3]>,
    jolts: Vec<(f64, [f32; 3])>,
    cooldown_until: f64,
}

pub const JOLT: f32 = 12.0;
pub const JOLTS: usize = 3;
pub const WINDOW_MS: f64 = 800.0;
pub const COOLDOWN_MS: f64 = 1500.0;

impl Default for Shake { fn default() -> Self { Shake { slow: None, jolts: vec![], cooldown_until: 0.0 } } }

impl Shake {
    /// Feed a sample; true when it completes a shake.
    pub fn feed(&mut self, t_ms: f64, a: [f32; 3]) -> bool {
        let slow = *self.slow.get_or_insert(a);
        let fast = [a[0] - slow[0], a[1] - slow[1], a[2] - slow[2]];
        self.slow = Some([0, 1, 2].map(|i| slow[i] + (a[i] - slow[i]) * 0.1));
        if t_ms < self.cooldown_until { return false; }
        let mag = (fast[0] * fast[0] + fast[1] * fast[1] + fast[2] * fast[2]).sqrt();
        self.jolts.retain(|(t, _)| t_ms - t <= WINDOW_MS);
        if mag >= JOLT {
            // A new jolt only if it points away from the last one — one
            // long push is one jolt, not ten samples of it.
            let reversed = self.jolts.last().is_none_or(|(_, d)| d[0] * fast[0] + d[1] * fast[1] + d[2] * fast[2] < 0.0);
            if reversed { self.jolts.push((t_ms, fast)); }
        }
        if self.jolts.len() >= JOLTS {
            self.jolts.clear();
            self.cooldown_until = t_ms + COOLDOWN_MS;
            return true;
        }
        false
    }
}

/// An IIO accelerometer in sysfs.
#[derive(Debug, Clone, PartialEq)]
pub struct Accel { pub dir: PathBuf, pub scale: f32 }

/// Find the accelerometer under `root` (normally `/`).
pub fn find_accel(root: &Path) -> Option<Accel> {
    let base = root.join("sys/bus/iio/devices");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&base).ok()?.flatten().map(|e| e.path()).collect();
    dirs.sort();
    dirs.into_iter().find(|d| d.join("in_accel_x_raw").exists()).map(|dir| {
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).ok().and_then(|s| s.trim().parse::<f32>().ok());
        let scale = read("in_accel_scale").or_else(|| read("in_accel_x_scale")).unwrap_or(1.0);
        Accel { dir, scale }
    })
}

impl Accel {
    pub fn read(&self) -> Option<[f32; 3]> {
        let r = |a: &str| std::fs::read_to_string(self.dir.join(format!("in_accel_{a}_raw"))).ok()?.trim().parse::<f32>().ok();
        Some([r("x")? * self.scale, r("y")? * self.scale, r("z")? * self.scale])
    }
}

/// A 50 Hz poller that sends `Trigger::Shake`.
pub struct ShakeWatch { pub rx: Receiver<Trigger>, stop: Arc<AtomicBool>, pub accel: Accel }

impl ShakeWatch {
    pub fn start(accel: Accel) -> ShakeWatch {
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let (a, s) = (accel.clone(), stop.clone());
        std::thread::Builder::new().name("shake".into()).spawn(move || {
            let t0 = std::time::Instant::now();
            let mut det = Shake::default();
            while !s.load(Ordering::Relaxed) {
                if let Some(v) = a.read() {
                    if det.feed(t0.elapsed().as_secs_f64() * 1000.0, v) && tx.send(Trigger::Shake).is_err() { return; }
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }).ok();
        ShakeWatch { rx, stop, accel }
    }
}

impl Drop for ShakeWatch { fn drop(&mut self) { self.stop.store(true, Ordering::Relaxed); } }

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a PinePhone (volume on the LRADC, power on the PMIC)
    /// and a laptop keyboard, trimmed.
    const DEVICES: &str = "I: Bus=0019 Vendor=0001 Product=0001 Version=0100
N: Name=\"1c21800.lradc\"
P: Phys=sun4i_lradc/input0
H: Handlers=kbd event0
B: PROP=0
B: EV=3
B: KEY=c000000000000 0

I: Bus=0019 Vendor=0000 Product=0000 Version=0000
N: Name=\"axp20x-pek\"
P: Phys=m1kbd/input2
H: Handlers=kbd event1
B: PROP=0
B: EV=3
B: KEY=10000000000000 0

I: Bus=0011 Vendor=0001 Product=0001 Version=ab83
N: Name=\"AT Translated Set 2 keyboard\"
P: Phys=isa0060/serio0/input0
H: Handlers=sysrq kbd leds event2
B: PROP=0
B: EV=120013
B: KEY=402000000 3803078f800d001 feffffdfffefffff fffffffffffffffe
";

    #[test]
    fn devices_are_found_by_the_keys_they_declare() {
        let d = parse_devices(DEVICES, 64);
        assert_eq!(d.len(), 3);
        let lradc = &d[0];
        assert_eq!(lradc.event, "event0");
        assert_eq!(lradc.keys, vec![KEY_VOLUMEDOWN, KEY_VOLUMEUP]);
        assert!(lradc.grabbable());
        assert_eq!(d[1].keys, vec![KEY_POWER]);
        let kb = &d[2];
        assert!(kb.has(KEY_VOLUMEUP) && kb.has(KEY_POWER) && kb.has(30), "a keyboard with media keys");
        assert!(!kb.grabbable(), "a keyboard is never grabbed");
        let vol = pick(&d, Keys { volume: true, power: false });
        assert_eq!(vol.iter().map(|x| x.event.as_str()).collect::<Vec<_>>(), ["event0", "event2"]);
        assert_eq!(pick(&d, Keys { volume: false, power: true }).len(), 2);
        assert!(pick(&d, Keys::default()).is_empty());
    }

    fn ev(ty: u16, code: u16, value: i32) -> Vec<u8> {
        let mut v = vec![0u8; 16];
        v.extend_from_slice(&ty.to_ne_bytes());
        v.extend_from_slice(&code.to_ne_bytes());
        v.extend_from_slice(&value.to_ne_bytes());
        v
    }

    #[test]
    fn only_presses_are_triggers_not_releases_repeats_or_syncs() {
        let mut buf = ev(EV_KEY, KEY_VOLUMEUP, 1);
        buf.extend(ev(0, 0, 0));
        buf.extend(ev(EV_KEY, KEY_VOLUMEUP, 2));
        buf.extend(ev(EV_KEY, KEY_VOLUMEUP, 0));
        buf.extend(ev(EV_KEY, KEY_POWER, 1));
        assert_eq!(decode_presses(&buf), vec![KEY_VOLUMEUP, KEY_POWER]);
    }

    #[test]
    fn a_device_file_becomes_triggers_through_the_reader_thread() {
        let d = tempfile::tempdir().unwrap();
        let mut buf = ev(EV_KEY, KEY_VOLUMEDOWN, 1);
        buf.extend(ev(EV_KEY, KEY_VOLUMEDOWN, 0));
        buf.extend(ev(EV_KEY, KEY_POWER, 1));
        std::fs::write(d.path().join("event0"), &buf).unwrap();
        std::fs::write(d.path().join("event1"), &buf).unwrap();
        let w = KeyWatch::start(DEVICES, d.path(), Keys { volume: true, power: false });
        let got = w.rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(got, Trigger::VolumeDown);
        assert!(w.rx.recv_timeout(std::time::Duration::from_millis(200)).is_err(), "power was not asked for");
        assert!(w.report.iter().any(|r| r.contains("event2") && r.contains("No such file")), "{:?}", w.report);
    }

    #[test]
    fn a_shake_is_several_reversing_jolts_and_a_bump_or_a_walk_is_not() {
        let g = [0.0, 0.0, 9.81];
        let mut s = Shake::default();
        let mut t = 0.0;
        for _ in 0..50 { assert!(!s.feed(t, g)); t += 20.0; }
        // A single bump.
        assert!(!s.feed(t, [0.0, 25.0, 9.81])); t += 20.0;
        for _ in 0..60 { assert!(!s.feed(t, g)); t += 20.0; }
        // Walking: gentle bounce.
        for i in 0..100 { assert!(!s.feed(t, [0.0, 0.0, 9.81 + 3.0 * ((i as f32) * 0.6).sin()]), "walking shook"); t += 20.0; }
        // A shake: hard back-and-forth.
        let mut fired = false;
        for i in 0..20 {
            let a = if i % 4 < 2 { 30.0 } else { -30.0 };
            fired |= s.feed(t, [a, 0.0, 9.81]);
            t += 20.0;
        }
        assert!(fired, "a shake was missed");
        // And not again straight away.
        let mut again = false;
        for i in 0..20 { again |= s.feed(t, [if i % 4 < 2 { 30.0 } else { -30.0 }, 0.0, 9.81]); t += 20.0; }
        assert!(!again, "fired again inside the cooldown");
    }

    #[test]
    fn the_accelerometer_is_found_and_scaled() {
        let d = tempfile::tempdir().unwrap();
        let dev = d.path().join("sys/bus/iio/devices/iio:device1");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::create_dir_all(d.path().join("sys/bus/iio/devices/iio:device0")).unwrap();
        for (f, v) in [("in_accel_x_raw", "100"), ("in_accel_y_raw", "-50"), ("in_accel_z_raw", "1000"), ("in_accel_scale", "0.009806")] {
            std::fs::write(dev.join(f), v).unwrap();
        }
        let a = find_accel(d.path()).expect("found");
        let v = a.read().unwrap();
        assert!((v[2] - 9.806).abs() < 0.01 && (v[0] - 0.9806).abs() < 0.01, "{v:?}");
        assert!(find_accel(&d.path().join("nothing")).is_none());
        assert_eq!(inhibit_args()[0], "--what=handle-power-key");
    }
}
