//! ssc-kit: raw accelerometer, gyroscope and magnetometer samples from a Fairphone 5.
//!
//! ## Where the sensors are (checked on the phone, 2026-10-04)
//!
//! Not on anything Linux can see. `/sys/bus/i2c`, `/sys/bus/iio` and the input devices hold the PMIC ADCs, the
//! cameras, the haptics and the amps, but no IMU. The firmware's sensor config
//! (`/usr/share/qcom/qcm6490/Fairphone/fp5/sensors/config/`) says:
//!
//! * `lahaila_lsm6dso_0.json`: an ST **LSM6DSO** (accelerometer + gyroscope + temperature + motion detect) on a
//!   **SPI** bus, instance 3 of the DSP's, data-ready interrupt handled by the DSP; allowed ODR 20-500 Hz.
//! * `lahaila_mmc56x3x_0.json`: a MEMSIC **MMC56x3x** magnetometer on **I2C**, DSP bus instance 1, address 0x30.
//! * `lahaila_stk3a5x_0.json` / `stk3a6x`: the light and proximity sensors.
//!
//! The ADSP runs Qualcomm's sensor core (SSC); `hexagonrpcd` serves it its config files over FastRPC, and `libssc`
//! speaks to it over QMI/qrtr. `iio-sensor-proxy` uses libssc for its orientation and light, but publishes only
//! coarse orientation, so continuous values come from here.
//!
//! ## The rate
//!
//! Stock libssc enables a sensor at the FIRST rate it lists, the slowest: ~27 Hz here, which is too slow for a
//! flick. `helper/libssc-sample-rate.patch` makes it take the listed rate nearest to `LIBSSC_SAMPLE_RATE`.
//! Measured on the phone: asking for 100, 200, 400 Hz gave about 104, 207, 414 samples a second on both the
//! accelerometer and the gyroscope. The magnetometer stays ~10 Hz.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// m/s^2, in the sensor's frame (the firmware's mount matrix is all zero, so libssc uses the identity).
    Accel,
    /// rad/s.
    Gyro,
    /// microtesla.
    Mag,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub kind: Kind,
    /// The phone's monotonic clock, milliseconds.
    pub t_ms: f64,
    pub v: [f32; 3],
}

/// One line of `sscstream`'s output: `A|G|M <ms> x y z`. Anything else (a warning, a torn line) is None; so is a
/// non-finite value, which would poison every filter downstream.
pub fn parse_line(line: &str) -> Option<Sample> {
    let mut it = line.split_whitespace();
    let kind = match it.next()? {
        "A" => Kind::Accel,
        "G" => Kind::Gyro,
        "M" => Kind::Mag,
        _ => return None,
    };
    let t_ms: f64 = it.next()?.parse().ok()?;
    let mut v = [0f32; 3];
    for slot in &mut v {
        *slot = it.next()?.parse().ok()?;
    }
    if it.next().is_some() || !t_ms.is_finite() || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    Some(Sample { kind, t_ms, v })
}

/// Where the helper lives on the phone: `~/.local/bin/sscstream` (a wrapper that points at the private libssc).
pub fn default_helper() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/root")).join(".local/bin/sscstream")
}

/// A running `sscstream`. Samples arrive on `rx`; dropping this kills the helper.
pub struct SscStream {
    child: Child,
    pub rx: Receiver<Sample>,
}

impl SscStream {
    /// Start `helper` for the sensors named in `which` ("a", "g", "m" in any combination) at about `rate_hz`.
    pub fn spawn(helper: &Path, which: &str, rate_hz: Option<u32>) -> std::io::Result<SscStream> {
        let mut cmd = Command::new(helper);
        cmd.arg(which).stdout(Stdio::piped()).stderr(Stdio::null());
        if let Some(r) = rate_hz { cmd.env("LIBSSC_SAMPLE_RATE", r.to_string()); }
        let mut child = cmd.spawn()?;
        let out = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
        let (tx, rx) = channel();
        std::thread::Builder::new().name("ssc-reader".into()).spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(s) = parse_line(&line) {
                    if tx.send(s).is_err() { break; }
                }
            }
        })?;
        Ok(SscStream { child, rx })
    }
}

impl Drop for SscStream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn it_parses_what_the_helper_prints_and_nothing_else() {
        let a = parse_line("A 51357123.8 3.58182 -0.29192 9.59761").unwrap();
        assert_eq!((a.kind, a.t_ms), (Kind::Accel, 51357123.8));
        assert_eq!(a.v, [3.58182, -0.29192, 9.59761]);
        assert_eq!(parse_line("G 1.0 -0.06720 -0.01955 0.17959").unwrap().kind, Kind::Gyro);
        assert_eq!(parse_line("M 1.0 -222.2 41.0 -152.6").unwrap().kind, Kind::Mag);
        for bad in ["", "X 1 2 3 4", "A 1 2 3", "A 1 2 3 4 5", "A one 2 3 4", "A 1 2 three 4", "A 1 nan 3 4", "A inf 2 3 4",
                    "** (sscstream:1): WARNING **: Mount matrix provided by firmware is all 0"] {
            assert!(parse_line(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn a_helper_that_prints_samples_is_read_and_stopped_on_drop() {
        // a stand-in helper: noise, two samples, then it keeps running (like the real one)
        let dir = std::env::temp_dir().join(format!("ssc-kit-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake-sscstream");
        std::fs::write(&script, "#!/bin/sh\necho \"rate=$LIBSSC_SAMPLE_RATE which=$1\" >&2\necho warning: x\necho 'A 1.0 0 0 9.8'\necho 'G 2.0 0.1 0 0'\nexec sleep 600\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let s = SscStream::spawn(&script, "ag", Some(200)).unwrap();
        let first = s.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let second = s.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((first.kind, second.kind), (Kind::Accel, Kind::Gyro));
        let pid = s.child.id();
        // dropped on its own thread: if the helper were not killed, the drop would wait out its 600 s sleep
        let (done_tx, done_rx) = channel();
        std::thread::spawn(move || { drop(s); let _ = done_tx.send(()); });
        done_rx.recv_timeout(Duration::from_secs(5)).expect("dropping the stream must stop the helper promptly");
        assert!(!Path::new(&format!("/proc/{pid}")).exists(), "the helper must not outlive the stream");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_helper_is_an_error_not_a_hang() {
        assert!(SscStream::spawn(Path::new("/nonexistent/sscstream"), "a", None).is_err());
    }
}
