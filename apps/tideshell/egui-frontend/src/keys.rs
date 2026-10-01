//! The power and volume keys, read from evdev. On the phone they are their
//! own input devices (`pwrkey`, `gpio-keys`), found by their key capability
//! bitmaps under `/sys/class/input`, so nothing else is opened. Reading
//! `/dev/input/event*` needs the `input` group (or root).
//!
//! The power key is normally logind's: it powers the phone off. While
//! tideshell runs it takes logind's `handle-power-key` inhibitor lock
//! (released when the process exits, so the key returns to logind), and the
//! key becomes sleep/wake.

use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

pub const KEY_VOLUMEDOWN: u16 = 114;
pub const KEY_VOLUMEUP: u16 = 115;
pub const KEY_POWER: u16 = 116;
const EV_KEY: u16 = 1;

/// Whether a `capabilities/key` bitmap (space-separated hex words, most
/// significant first, 64 bits each on a 64-bit kernel) has bit `code`.
pub fn caps_has_key(caps: &str, code: u16) -> bool {
    let words: Vec<u64> = caps.split_whitespace().filter_map(|w| u64::from_str_radix(w, 16).ok()).collect();
    let (wi, bit) = ((code / 64) as usize, code % 64);
    words.iter().rev().nth(wi).is_some_and(|w| (w >> bit) & 1 == 1)
}

/// One `struct input_event` on 64-bit Linux: two i64 (time), u16 type, u16
/// code, i32 value = 24 bytes. Returns (type, code, value).
pub fn decode_event(b: &[u8]) -> Option<(u16, u16, i32)> {
    if b.len() < 24 {
        return None;
    }
    let ty = u16::from_ne_bytes([b[16], b[17]]);
    let code = u16::from_ne_bytes([b[18], b[19]]);
    let value = i32::from_ne_bytes([b[20], b[21], b[22], b[23]]);
    Some((ty, code, value))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyDevice {
    pub node: PathBuf,
    pub name: String,
    pub power: bool,
    pub volume: bool,
}

/// Devices with the power or volume keys, from `<sys>/class/input/event*`.
pub fn discover(sys: &Path, dev: &Path) -> Vec<KeyDevice> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(sys.join("class/input")) else { return out };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("event"))).collect();
    entries.sort();
    for e in entries {
        let caps = std::fs::read_to_string(e.join("device/capabilities/key")).unwrap_or_default();
        let power = caps_has_key(&caps, KEY_POWER);
        let volume = caps_has_key(&caps, KEY_VOLUMEUP) || caps_has_key(&caps, KEY_VOLUMEDOWN);
        if power || volume {
            let name = std::fs::read_to_string(e.join("device/name")).unwrap_or_default().trim().to_string();
            out.push(KeyDevice { node: dev.join(e.file_name().unwrap()), name, power, volume });
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPress {
    Power,
    VolumeUp,
    VolumeDown,
}

pub struct Keys {
    pub rx: Receiver<KeyEvent>,
    /// Written to by the reader thread whenever it sends an event, so the
    /// main loop can sleep on it. Without this a key press waits for the
    /// loop's timeout — a quarter of a second at idle — because an mpsc
    /// channel is not something poll can wait on.
    wake: Option<UnixDatagram>,
    pub status: String,
    _inhibit: Option<OwnedFd>,
}

impl Keys {
    /// Open the discovered devices and start readers. Only key PRESSES are
    /// reported (value 1); releases and repeats are dropped.
    /// `via_compositor`: cosmic-comp hands the POWER key over through the control
    /// socket (ctl.rs), so that node is not opened; the volume keys are still read
    /// from evdev (they need releases for holds), and the logind inhibitor is taken
    /// regardless of device access: logind reads the key itself and would power off.
    /// The descriptor to sleep on, if there is a reader thread.
    pub fn wake_fd(&self) -> Option<std::os::fd::BorrowedFd<'_>> {
        self.wake.as_ref().map(std::os::fd::AsFd::as_fd)
    }

    /// Drain the wake channel. The bytes carry nothing; the events are on
    /// the mpsc channel. This only stops poll firing again immediately.
    pub fn drain_wake(&self) {
        let Some(w) = &self.wake else { return };
        let mut scratch = [0u8; 64];
        while w.recv(&mut scratch).is_ok() {}
    }

    pub fn spawn(devices: Vec<KeyDevice>, bus: Option<&zbus::blocking::Connection>, via_compositor: bool) -> Keys {
        let (tx, rx) = mpsc::channel();
        let mut opened = Vec::new();
        let mut failed = Vec::new();
        // A socket pair the readers poke after every event, so the main loop
        // can WAIT for a key instead of noticing one on its next scheduled
        // pass. Non-blocking on both ends: a full buffer means the loop has
        // not drained yet, which already means it is about to run.
        let (wake_main, wake_reader) = match UnixDatagram::pair() {
            Ok((a, b)) => {
                let _ = a.set_nonblocking(true);
                let _ = b.set_nonblocking(true);
                (Some(a), Some(b))
            }
            Err(_) => (None, None),
        };
        // Through the compositor only the POWER key is routed there; the volume keys stay on
        // evdev either way (releases exist there, which holds need), so drop the power node.
        let devices: Vec<KeyDevice> = if via_compositor { devices.into_iter().filter(|d| d.volume && !d.power).collect() } else { devices };
        for d in &devices {
            match std::fs::File::open(&d.node) {
                Ok(f) => {
                    opened.push(format!("{} ({})", d.name, d.node.display()));
                    let tx: Sender<KeyEvent> = tx.clone();
                    let wake = wake_reader.as_ref().and_then(|w| w.try_clone().ok());
                    std::thread::Builder::new().name("tideshell-keys".into()).spawn(move || reader(f, tx, wake)).expect("spawn key reader");
                }
                Err(e) => failed.push(format!("{}: {e}", d.node.display())),
            }
        }
        // The inhibitor only when the power key can actually be READ: taking
        // it with the device unreadable left the key doing nothing at all on
        // the FP5 (logind no longer powered off, tideshell never saw the key).
        let power_readable = devices.iter().any(|d| d.power && opened.iter().any(|o| o.contains(&d.node.display().to_string())));
        let inhibit = if via_compositor { bus.and_then(|b| inhibit_power_key(b).ok()) } else if power_readable { bus.and_then(|b| inhibit_power_key(b).ok()) } else { None };
        let status = format!(
            "keys: {}{}{}{}",
            if via_compositor { "power from cosmic-comp via tideshell ctl; volume " } else { "" },
            if opened.is_empty() { "no readable power/volume device".to_string() } else { opened.join(", ") },
            if failed.is_empty() { String::new() } else { format!("; cannot open {} (add the user to the input group)", failed.join(", ")) },
            match (&inhibit, devices.iter().any(|d| d.power), power_readable) {
                (Some(_), _, _) => "; logind power-key inhibitor held",
                (None, true, false) => "; power key left to logind (device unreadable: it still powers off)",
                (None, true, true) => "; NO logind inhibitor (D-Bus refused): the power key still powers off",
                (None, false, _) => "",
            }
        );
        Keys { rx, wake: wake_main, status, _inhibit: inhibit }
    }
}

/// A key going down or coming up. Releases matter because a HOLD is only
/// knowable from one: a shortcut that fires on press can never distinguish a
/// tap from a hold, which is why the volume keys cannot be routed through
/// the compositor if they are to do more than step the volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: KeyPress,
    pub down: bool,
}

fn reader(mut f: std::fs::File, tx: Sender<KeyEvent>, wake: Option<UnixDatagram>) {
    let mut buf = [0u8; 24 * 16];
    loop {
        let n = match f.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        for chunk in buf[..n].chunks_exact(24) {
            // 2 is autorepeat, which is the kernel's own timer and not the
            // one this shell wants: holds are timed against the shell's
            // clock so they behave the same however the key repeat is set.
            if let Some((EV_KEY, code, v @ (0 | 1))) = decode_event(chunk) {
                let key = match code {
                    KEY_POWER => KeyPress::Power,
                    KEY_VOLUMEUP => KeyPress::VolumeUp,
                    KEY_VOLUMEDOWN => KeyPress::VolumeDown,
                    _ => continue,
                };
                if tx.send(KeyEvent { key, down: v == 1 }).is_err() {
                    return;
                }
                // The event is already on the channel; this only ends the
                // main loop's sleep. A failed send means the pipe is full,
                // which means it is waking anyway.
                if let Some(w) = &wake {
                    let _ = w.send(b"k");
                }
            }
        }
    }
}

/// logind's `Inhibit("handle-power-key", ..., "block")`: the returned fd IS
/// the lock; dropping it hands the key back to logind.
pub fn inhibit_power_key(bus: &zbus::blocking::Connection) -> Result<OwnedFd, String> {
    let p = zbus::blocking::Proxy::new(bus, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager").map_err(|e| e.to_string())?;
    let reply = p.call_method("Inhibit", &("handle-power-key", "tideshell", "tideshell turns the power key into sleep/wake", "block")).map_err(|e| format!("Inhibit: {e}"))?;
    let fd: zbus::zvariant::OwnedFd = reply.body().deserialize().map_err(|e| format!("Inhibit reply: {e}"))?;
    Ok(OwnedFd::from(fd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_bitmaps_are_read_from_the_right_word_and_bit() {
        // Bit 116 (KEY_POWER) lives in word 1 from the right, bit 52.
        let power = format!("{:x} {:x}", 1u64 << 52, 0u64);
        assert!(caps_has_key(&power, KEY_POWER));
        assert!(!caps_has_key(&power, KEY_VOLUMEUP));
        // Volume keys 114/115: word 1, bits 50/51.
        let vol = format!("{:x} {:x}", (1u64 << 50) | (1u64 << 51), 0u64);
        assert!(caps_has_key(&vol, KEY_VOLUMEUP) && caps_has_key(&vol, KEY_VOLUMEDOWN) && !caps_has_key(&vol, KEY_POWER));
        // A keyboard's long bitmap with many words: bit 30 (KEY_A) in the last word.
        let kbd = format!("{:x} {:x} {:x}", 0u64, 0u64, 1u64 << 30);
        assert!(!caps_has_key(&kbd, KEY_POWER));
        assert!(!caps_has_key("", KEY_POWER));
    }

    #[test]
    fn input_events_decode_and_only_presses_count() {
        let mut ev = [0u8; 24];
        ev[16..18].copy_from_slice(&EV_KEY.to_ne_bytes());
        ev[18..20].copy_from_slice(&KEY_POWER.to_ne_bytes());
        ev[20..24].copy_from_slice(&1i32.to_ne_bytes());
        assert_eq!(decode_event(&ev), Some((EV_KEY, KEY_POWER, 1)));
        assert_eq!(decode_event(&ev[..20]), None);
        let mut rel = ev;
        rel[20..24].copy_from_slice(&0i32.to_ne_bytes());
        assert_eq!(decode_event(&rel), Some((EV_KEY, KEY_POWER, 0)));
    }

    #[test]
    fn discovery_finds_only_power_and_volume_devices() {
        let root = std::env::temp_dir().join(format!("tideshell-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let put = |p: PathBuf, s: String| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, s).unwrap();
        };
        let sys = root.join("sys");
        put(sys.join("class/input/event0/device/capabilities/key"), format!("{:x} {:x}", 1u64 << 52, 0u64));
        put(sys.join("class/input/event0/device/name"), "pmic_pwrkey\n".into());
        put(sys.join("class/input/event1/device/capabilities/key"), format!("{:x} {:x}", (1u64 << 50) | (1u64 << 51), 0u64));
        put(sys.join("class/input/event1/device/name"), "gpio-keys\n".into());
        put(sys.join("class/input/event2/device/capabilities/key"), "0".into());
        put(sys.join("class/input/event2/device/name"), "Goodix Berlin Capacitive TouchScreen\n".into());
        let d = discover(&sys, &root.join("dev/input"));
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].name.as_str(), d[0].power, d[0].volume), ("pmic_pwrkey", true, false));
        assert_eq!((d[1].name.as_str(), d[1].power, d[1].volume), ("gpio-keys", false, true));
        assert!(d[0].node.ends_with("event0"));
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod wake_tests {
    use super::*;

    #[test]
    fn the_wake_channel_ends_a_sleep_and_drains() {
        // The events travel on the mpsc channel; this pair exists only so
        // the main loop can WAIT for one rather than noticing it on its next
        // scheduled pass, which at idle is a quarter of a second away.
        let (main, reader) = UnixDatagram::pair().unwrap();
        main.set_nonblocking(true).unwrap();
        reader.set_nonblocking(true).unwrap();
        let k = Keys { rx: mpsc::channel().1, wake: Some(main), status: String::new(), _inhibit: None };
        assert!(k.wake_fd().is_some());
        // Nothing yet.
        k.drain_wake();
        // A reader poke is readable, and draining clears it so poll does not
        // fire again on the same byte.
        reader.send(b"k").unwrap();
        reader.send(b"k").unwrap();
        k.drain_wake();
        let mut scratch = [0u8; 8];
        assert!(k.wake.as_ref().unwrap().recv(&mut scratch).is_err(), "drained");
        // Draining when there is nothing does not block or panic.
        k.drain_wake();
    }

    #[test]
    fn without_a_reader_there_is_nothing_to_sleep_on() {
        // The compositor route opens no devices, so there is no wake
        // descriptor and the loop simply does not add one.
        let k = Keys { rx: mpsc::channel().1, wake: None, status: String::new(), _inhibit: None };
        assert!(k.wake_fd().is_none());
        k.drain_wake();
    }
}
