//! gyro — motion controls, ported from the monorepo's gyro-ball prototype
//! (games/gyroball: `sensor_daemon/tilt_sensor_sysfs.c` + the Python reader in
//! `v15.py`) and shaped for DRIVING instead of a tilt maze.
//!
//! Three sources, auto-detected in order, all feeding the same normalized
//! tilt vector:
//!
//! 1. **IIO sysfs, read natively** — the port of `tilt_sensor_sysfs.c`. Walks
//!    `/sys/bus/iio/devices/iio:device*` for `in_accel_{x,y,z}_raw` (and
//!    `in_anglvel_*` when the device has a real gyroscope), applies the
//!    device's own `_scale`, and samples at 50 Hz. No daemon, no pipe.
//! 2. **The daemon protocol on stdin** — `TILT_SENSOR_V1` header then
//!    `ACCEL x y z` lines, so the existing C daemon still works:
//!    `./tilt_sensor | skydribble`.
//! 3. **`monitor-sensor --accel`** — iio-sensor-proxy's coarse change events
//!    ("Tilt changed: tilted-up"), spawned on demand. Discrete, so these
//!    become impulses that decay, exactly like the prototype's fallback.
//!
//! The reader lives on its own thread and publishes through a channel: a
//! blocked or missing sensor can never stall a frame.

use std::io::BufRead;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

/// What the reader thread publishes: a device-frame tilt/rate sample.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    /// Left/right tilt (or roll rate), device units already scaled.
    pub x: f32,
    /// Fore/aft tilt (or pitch rate).
    pub y: f32,
    /// True for continuous sources; false for coarse impulse events.
    pub analog: bool,
}

/// Where the samples came from (shown in the menu so you can see it working).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Source {
    None,
    Sysfs,
    Daemon,
    Monitor,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::None => "NONE",
            Source::Sysfs => "IIO SYSFS",
            Source::Daemon => "TILT DAEMON",
            Source::Monitor => "MONITOR-SENSOR",
        }
    }
}

// ---------------------------------------------------------------- parsing --

/// Parse one line of the daemon / monitor-sensor protocol. Returns a sample
/// (analog) or an impulse (discrete). Ported from `v15.py::sensor_reader`.
pub fn parse_line(line: &str) -> Option<Sample> {
    let l = line.trim();
    if l.is_empty() {
        return None;
    }
    if let Some(rest) = l.strip_prefix("ACCEL ") {
        let mut it = rest.split_whitespace();
        let x: f32 = it.next()?.parse().ok()?;
        let y: f32 = it.next()?.parse().ok()?;
        return Some(Sample { x, y, analog: true });
    }
    if let Some(i) = l.find("Tilt changed:") {
        // discrete kick, magnitude 1: the caller decays it
        return Some(match l[i + "Tilt changed:".len()..].trim() {
            "tilted-up" => Sample { x: 0.0, y: -1.0, analog: false },
            "tilted-down" => Sample { x: 0.0, y: 1.0, analog: false },
            _ => Sample { x: 0.0, y: 0.0, analog: false },
        });
    }
    if let Some(i) = l.find("orientation changed:") {
        return Some(match l[i + "orientation changed:".len()..].trim() {
            "left-up" => Sample { x: -1.0, y: 0.0, analog: false },
            "right-up" => Sample { x: 1.0, y: 0.0, analog: false },
            _ => Sample { x: 0.0, y: 0.0, analog: false },
        });
    }
    None
}

/// Read one number out of a sysfs file (the C daemon's `read_sysfs_double`).
fn read_num(path: &std::path::Path) -> Option<f64> {
    let t = std::fs::read_to_string(path).ok()?;
    t.trim().parse::<f64>().ok()
}

/// Find an IIO device exposing the wanted channel prefix, returning
/// (device dir, scale). Port of the C daemon's glob-based discovery.
fn find_iio(prefix: &str) -> Option<(std::path::PathBuf, f64)> {
    let root = std::path::Path::new("/sys/bus/iio/devices");
    for e in std::fs::read_dir(root).ok()? {
        let dir = e.ok()?.path();
        let name = dir.file_name()?.to_string_lossy().to_string();
        if !name.starts_with("iio:device") {
            continue;
        }
        if !dir.join(format!("{prefix}_x_raw")).exists() {
            continue;
        }
        let scale = read_num(&dir.join(format!("{prefix}_scale"))).unwrap_or(1.0);
        return Some((dir, if scale == 0.0 { 1.0 } else { scale }));
    }
    None
}

// ----------------------------------------------------------------- reader --

fn spawn_reader() -> (Receiver<Sample>, Arc<std::sync::atomic::AtomicU8>) {
    let (tx, rx) = std::sync::mpsc::channel::<Sample>();
    let src = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let src_w = src.clone();
    std::thread::spawn(move || {
        use std::sync::atomic::Ordering;
        // 1. native IIO sysfs — gyroscope preferred, accelerometer otherwise
        let gyro = find_iio("in_anglvel");
        let accel = find_iio("in_accel");
        if let Some((dir, scale)) = gyro.or(accel) {
            let prefix = if dir.join("in_anglvel_x_raw").exists() { "in_anglvel" } else { "in_accel" };
            src_w.store(1, Ordering::Relaxed);
            let (px, py) = (dir.join(format!("{prefix}_x_raw")), dir.join(format!("{prefix}_y_raw")));
            loop {
                let x = read_num(&px).unwrap_or(0.0) * scale;
                let y = read_num(&py).unwrap_or(0.0) * scale;
                if tx.send(Sample { x: x as f32, y: y as f32, analog: true }).is_err() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20)); // 50 Hz
            }
        }
        // 2. the daemon protocol on stdin (only when it is actually piped)
        if !platform_stdin_is_tty() {
            let stdin = std::io::stdin();
            let mut saw = false;
            for line in stdin.lock().lines().map_while(Result::ok) {
                if line.trim() == "TILT_SENSOR_V1" {
                    saw = true;
                    src_w.store(2, Ordering::Relaxed);
                    continue;
                }
                if let Some(s) = parse_line(&line) {
                    if !saw {
                        src_w.store(3, Ordering::Relaxed);
                    }
                    if tx.send(s).is_err() {
                        return;
                    }
                }
            }
        }
        // 3. spawn monitor-sensor as the last resort
        if let Ok(mut ch) = std::process::Command::new("monitor-sensor")
            .arg("--accel")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(out) = ch.stdout.take() {
                src_w.store(3, Ordering::Relaxed);
                for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(s) = parse_line(&line) {
                        if tx.send(s).is_err() {
                            let _ = ch.kill();
                            return;
                        }
                    }
                }
            }
        }
    });
    (rx, src)
}

fn platform_stdin_is_tty() -> bool {
    // No libc dependency: a piped stdin has a readable metadata size on Linux
    // and is not a character device. Best-effort — worst case we read a tty
    // that never produces protocol lines, on a thread, harmlessly.
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if let Ok(md) = std::fs::metadata("/dev/stdin") {
            return md.file_type().is_char_device();
        }
    }
    true
}

// ------------------------------------------------------------- the driver --

/// Gyro drive: turns device motion into steering, pitch and roll.
pub struct Gyro {
    rx: Option<Receiver<Sample>>,
    src_flag: Option<Arc<std::sync::atomic::AtomicU8>>,
    pub source: Source,
    /// smoothed, calibrated, deadzoned control vector (-1..1)
    pub x: f32,
    pub y: f32,
    raw: (f32, f32),
    impulse: (f32, f32),
    bias: (f32, f32),
    calib_t: f32,
    calib_acc: (f32, f32),
    calib_n: u32,
    pub calibrating: bool,
    pub live: bool,
    idle: f32,
}

impl Gyro {
    pub fn new(enabled: bool) -> Gyro {
        let (rx, src_flag) = if enabled { let (a, b) = spawn_reader(); (Some(a), Some(b)) } else { (None, None) };
        Gyro {
            rx,
            src_flag,
            source: Source::None,
            x: 0.0,
            y: 0.0,
            raw: (0.0, 0.0),
            impulse: (0.0, 0.0),
            bias: (0.0, 0.0),
            calib_t: 0.0,
            calib_acc: (0.0, 0.0),
            calib_n: 0,
            calibrating: false,
            live: false,
            idle: 0.0,
        }
    }

    /// Start (or restart) the reader — used when the setting is toggled on.
    pub fn enable(&mut self) {
        if self.rx.is_none() {
            let (a, b) = spawn_reader();
            self.rx = Some(a);
            self.src_flag = Some(b);
        }
    }

    /// Hold the device still and call this: two seconds of averaging becomes
    /// the neutral bias, which is what kills gyro drift.
    pub fn calibrate(&mut self) {
        self.calibrating = true;
        self.calib_t = 2.0;
        self.calib_acc = (0.0, 0.0);
        self.calib_n = 0;
    }

    /// Drain the channel and fold new samples into the control vector.
    pub fn update(&mut self, dt: f32, cfg: &crate::config::GyroCfg) {
        use std::sync::atomic::Ordering;
        if let Some(f) = &self.src_flag {
            self.source = match f.load(Ordering::Relaxed) {
                1 => Source::Sysfs,
                2 => Source::Daemon,
                3 => Source::Monitor,
                _ => Source::None,
            };
        }
        let mut got = false;
        if let Some(rx) = &self.rx {
            loop {
                match rx.try_recv() {
                    Ok(s) => {
                        got = true;
                        if s.analog {
                            self.raw = (s.x, s.y);
                        } else {
                            // discrete: kick an impulse that decays (the
                            // prototype's model for coarse event sources)
                            self.impulse.0 = (self.impulse.0 + s.x).clamp(-1.5, 1.5);
                            self.impulse.1 = (self.impulse.1 + s.y).clamp(-1.5, 1.5);
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.rx = None;
                        break;
                    }
                }
            }
        }
        if got {
            self.idle = 0.0;
        } else {
            self.idle = (self.idle + dt).min(9.0);
        }
        self.live = self.idle < 1.5 && (self.source != Source::None || self.rx.is_none());

        // calibration: average the raw stream, then subtract it forever
        if self.calibrating {
            self.calib_acc.0 += self.raw.0;
            self.calib_acc.1 += self.raw.1;
            self.calib_n += 1;
            self.calib_t -= dt;
            if self.calib_t <= 0.0 {
                if self.calib_n > 0 {
                    self.bias = (
                        self.calib_acc.0 / self.calib_n as f32,
                        self.calib_acc.1 / self.calib_n as f32,
                    );
                }
                self.calibrating = false;
            }
        }

        // impulses decay whether or not new events arrive
        let decay = (-6.0 * dt).exp();
        self.impulse.0 *= decay;
        self.impulse.1 *= decay;

        // calibrated -> scaled -> deadzoned -> clamped, then smoothed
        let sens = cfg.sens.max(0.01);
        let mut tx = (self.raw.0 - self.bias.0) * sens + self.impulse.0;
        let mut ty = (self.raw.1 - self.bias.1) * sens + self.impulse.1;
        if cfg.invert_x {
            tx = -tx;
        }
        if cfg.invert_y {
            ty = -ty;
        }
        let dz = cfg.deadzone.clamp(0.0, 0.5);
        let shape = |v: f32| {
            let a = v.abs();
            if a <= dz {
                0.0
            } else {
                ((a - dz) / (1.0 - dz)).min(1.0) * v.signum()
            }
        };
        let (tx, ty) = (shape(tx), shape(ty));
        // smoothing: a settle time in seconds, so the number reads like a feel
        let k = 1.0 - (-dt / cfg.smooth.max(0.008)).exp();
        self.x += (tx - self.x) * k;
        self.y += (ty - self.y) * k;
    }

    /// Test hooks: inject a raw analog sample / a discrete impulse / force
    /// the live flag, so the whole stack is exercisable without hardware.
    pub fn feed_test(&mut self, x: f32, y: f32) {
        self.raw = (x, y);
        self.idle = 0.0;
    }
    pub fn impulse_test(&mut self, x: f32, y: f32) {
        self.impulse.0 += x;
        self.impulse.1 += y;
        self.idle = 0.0;
    }
    pub fn set_live_for_test(&mut self, v: bool) {
        self.live = v;
        self.source = if v { Source::Sysfs } else { Source::None };
    }

    /// The gyro's contribution to (steer, pitch, roll) for this frame, given
    /// whether the car is airborne and whether the activation gate is open.
    pub fn drive(&self, cfg: &crate::config::GyroCfg, airborne: bool, gate: bool) -> (f32, f32, f32) {
        if !self.live || !gate {
            return (0.0, 0.0, 0.0);
        }
        let (x, y) = (self.x, self.y);
        if airborne {
            match cfg.air_mode {
                // 0: tilt rolls, fore/aft pitches — the flick-aim scheme
                0 => (0.0, y, x),
                // 1: tilt yaws instead (turn the phone, turn the nose)
                _ => (x, y, 0.0),
            }
        } else {
            (x, 0.0, 0.0)
        }
    }
}
