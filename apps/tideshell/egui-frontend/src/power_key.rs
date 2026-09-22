//! power_key — the hardware power button, read directly and kept away from logind.
//!
//! ## Why this exists
//!
//! `Shell::power_key` (tideshell-core) already decides what a press should do --
//! lock, fade, wake -- with tests. Nothing fed it a real press: no reader, and
//! nothing stopped systemd-logind acting on the key FIRST (its default is to
//! suspend or power off, before tideshell ever sees the event). So today the
//! button does whatever logind's `HandlePowerKey` says, not what this shell says.
//!
//! ## Two pieces, both required
//!
//! 1. An INHIBITOR: `login1.Manager.Inhibit("handle-power-key", ..., "block")`
//!    over the system bus. Holding the returned fd is what makes logind stand
//!    aside -- the same mechanism GNOME and KDE's session use. Dropping the fd
//!    (or exiting) hands the key back to logind's default instantly; this is
//!    deliberately a lock we hold, not a setting we change.
//! 2. A READER on the device itself. gpio-keys power buttons appear as an evdev
//!    node reporting `EV_KEY` with `KEY_POWER` in its capability bitmap -- the
//!    same discovery method `eink-kit::present::evdev::find_touchscreen` uses
//!    for touchscreens, `EVIOCGBIT` rather than a name match, because names vary
//!    by device tree (a Fairphone 5 and a desktop's ACPI button are unrelated
//!    kernel drivers reporting the same key). Opened non-exclusively, like that
//!    reader: another listener (there should not be one, now that logind is
//!    inhibited) still sees the key.
//!
//! ## What is genuinely tested here, and what is not
//!
//! The decode (`decode`) and the bitmap test (`reports_key_power`) are pure and
//! tested against synthetic bytes / fabricated bitmaps -- the same kind of proof
//! `eink-kit`'s evdev tests use, and for the same reason: there is no gpio-keys
//! device in this container. Device discovery, the inhibitor call, and the
//! reader thread are real code with no test double for a Linux input device or a
//! system bus, so they are UNVERIFIED here -- exactly the caveat this codebase
//! already carries for every other direct-hardware reader (eink-kit's own
//! touchscreen probe, tideshell's ambient-light and backlight paths).

use std::io;
use std::os::fd::{OwnedFd, RawFd};
use std::sync::mpsc::{self, Receiver, Sender};

/// `KEY_POWER` from `linux/input-event-codes.h`. Not in `libc` -- that crate
/// stops at syscalls and structs, and key codes are a userspace header; every
/// other reader in this tree (eink-kit's `touch.rs`) defines the ones it needs
/// the same way.
const KEY_POWER: u16 = 116;
const EV_KEY: u16 = 0x01;

/// One press, debounced to a single edge by the kernel's own `value` field
/// (0 = up, 1 = down, 2 = autorepeat) -- only `value == 1` is a fresh press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerPress;

/// Decode one raw evdev event. `None` for anything that is not a fresh
/// KEY_POWER down -- autorepeat, the release, and every other key or axis.
pub fn decode(type_: u16, code: u16, value: i32) -> Option<PowerPress> {
    (type_ == EV_KEY && code == KEY_POWER && value == 1).then_some(PowerPress)
}

/// Does an `EVIOCGBIT(EV_KEY, ...)` bitmap report `KEY_POWER`?
///
/// The bitmap is bits packed into bytes, kernel byte order (bit N of the
/// bitmap is byte N/8, bit N%8 of that byte) -- the layout `ioctl(EVIOCGBIT)`
/// fills in and the one `eink-kit::touch::find_touchscreen` already reads the
/// same way for `KEY_POWER`'s neighbours (`BTN_TOOL_PEN`, etc).
pub fn reports_key_power(key_bits: &[u8]) -> bool {
    let (byte, bit) = (KEY_POWER as usize / 8, KEY_POWER as usize % 8);
    key_bits.get(byte).is_some_and(|b| b & (1 << bit) != 0)
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    const fn ioc_read(nr: u32, size: u32) -> libc::Ioctl {
        ((2u32 << 30) | (size << 16) | ((b'E' as u32) << 8) | nr) as libc::Ioctl
    }
    const fn eviocgbit(ev: u32, len: u32) -> libc::Ioctl {
        ioc_read(0x20 + ev as u32, len)
    }

    fn open_ro(path: &str) -> io::Result<RawFd> {
        let c = std::ffi::CString::new(path).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
        if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(fd) }
    }

    /// Scan `/dev/input/event0..31` (or `TIDESHELL_POWER_DEV`) for the one
    /// reporting `KEY_POWER`, by capability, not by name.
    pub fn find_power_button() -> Option<String> {
        if let Ok(p) = std::env::var("TIDESHELL_POWER_DEV") {
            return probe(&p).then_some(p);
        }
        (0u16..32).map(|n| format!("/dev/input/event{n}")).find(|p| probe(p))
    }

    fn probe(path: &str) -> bool {
        let Ok(fd) = open_ro(path) else { return false };
        let mut bits = [0u8; (KEY_POWER as usize / 8) + 1];
        let ok = unsafe { libc::ioctl(fd, eviocgbit(EV_KEY as u32, bits.len() as u32), bits.as_mut_ptr()) } >= 0;
        unsafe { libc::close(fd) };
        ok && reports_key_power(&bits)
    }

    /// Ask logind to leave `handle-power-key` to us. The returned fd must be
    /// KEPT ALIVE for the inhibitor to hold -- dropping it (including at
    /// process exit) hands the key straight back to logind's default.
    pub fn inhibit_power_key(bus: &zbus::blocking::Connection) -> Result<OwnedFd, String> {
        let m = zbus::blocking::Proxy::new(bus, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager")
            .map_err(|e| e.to_string())?;
        let reply = m.call_method(
            "Inhibit",
            &("handle-power-key", "tideshell", "tideshell handles the power key", "block"),
        ).map_err(|e| format!("Inhibit: {e}"))?;
        reply.body().deserialize::<zbus::zvariant::OwnedFd>().map(OwnedFd::from).map_err(|e| format!("Inhibit reply: {e}"))
    }

    /// Reader thread: opens the device (non-exclusive; see the module doc) and
    /// sends a `PowerPress` for every fresh KEY_POWER down.
    pub fn spawn_reader(path: String, tx: Sender<PowerPress>) -> io::Result<()> {
        let fd = open_ro(&path)?;
        // Reads must BLOCK: this thread has nothing else to do, and O_NONBLOCK
        // (needed above only so the capability probe never hangs on a device
        // with no queued input) would spin it at full CPU between key presses.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) };
        std::thread::Builder::new().name("tideshell-power-key".into()).spawn(move || {
            let sz = std::mem::size_of::<libc::input_event>();
            let mut evs: [libc::input_event; 8] = unsafe { std::mem::zeroed() };
            loop {
                let n = unsafe { libc::read(fd, evs.as_mut_ptr().cast(), sz * evs.len()) };
                if n <= 0 {
                    return; // device gone; the shell keeps running without it
                }
                for e in &evs[..n as usize / sz] {
                    if let Some(p) = decode(e.type_, e.code, e.value) {
                        if tx.send(p).is_err() {
                            return; // receiver dropped: shutting down
                        }
                    }
                }
            }
        })?;
        Ok(())
    }
}

/// The power key, wired up: an inhibitor held for the process lifetime and a
/// channel of presses. Constructing this is best-effort -- a phone with no
/// button to find, or a session with no logind, still runs; it just does not
/// see the key.
pub struct PowerKey {
    rx: Receiver<PowerPress>,
    /// Held only for its `Drop`: releasing it returns the key to logind. Never
    /// read.
    _inhibitor: Option<OwnedFd>,
}

impl PowerKey {
    #[cfg(target_os = "linux")]
    pub fn spawn(bus: Option<&zbus::blocking::Connection>) -> PowerKey {
        let inhibitor = bus.and_then(|b| match linux::inhibit_power_key(b) {
            Ok(fd) => Some(fd),
            Err(e) => {
                eprintln!("power key: could not inhibit handle-power-key ({e}); logind may act on it first");
                None
            }
        });
        let (tx, rx) = mpsc::channel();
        match linux::find_power_button() {
            Some(path) => {
                eprintln!("power key: reading {path}");
                if let Err(e) = linux::spawn_reader(path, tx) {
                    eprintln!("power key: could not open device: {e}");
                }
            }
            None => eprintln!("power key: no evdev device reports KEY_POWER"),
        }
        PowerKey { rx, _inhibitor: inhibitor }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn spawn(_bus: Option<&()>) -> PowerKey {
        let (_tx, rx) = mpsc::channel();
        PowerKey { rx, _inhibitor: None }
    }

    /// Drain presses since the last call, as `Ambient::rx`'s caller does for lux.
    pub fn drain(&self) -> impl Iterator<Item = PowerPress> + '_ {
        self.rx.try_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_press_decodes_and_nothing_else_does() {
        assert_eq!(decode(EV_KEY, KEY_POWER, 1), Some(PowerPress));
        assert_eq!(decode(EV_KEY, KEY_POWER, 0), None, "release");
        assert_eq!(decode(EV_KEY, KEY_POWER, 2), None, "autorepeat, not a new press");
        assert_eq!(decode(EV_KEY, 30 /* KEY_A */, 1), None, "a different key");
        assert_eq!(decode(0x03 /* EV_ABS */, KEY_POWER, 1), None, "power's code on the wrong event type");
    }

    #[test]
    fn the_bitmap_check_finds_key_power_at_its_real_bit_position() {
        // KEY_POWER = 116 -> byte 14, bit 4. A bitmap that is all zero except
        // that one bit is a device that reports exactly KEY_POWER.
        let mut bits = [0u8; 32];
        bits[14] = 1 << 4;
        assert!(reports_key_power(&bits));
        assert!(!reports_key_power(&[0u8; 32]), "an all-zero bitmap reports nothing");
    }

    /// A neighbouring bit must not be mistaken for KEY_POWER -- the mistake a
    /// hand-rolled bit index (rather than /8, %8) would make silently.
    #[test]
    fn a_neighbouring_bit_is_not_key_power() {
        for shift in [3u32, 5] {
            let mut bits = [0u8; 32];
            bits[14] = 1 << shift;
            assert!(!reports_key_power(&bits), "bit {shift} of byte 14 is not KEY_POWER");
        }
    }

    /// A bitmap too short to contain KEY_POWER's byte must not panic or, worse,
    /// read past the buffer -- a short reply from a minimal/virtual device.
    #[test]
    fn a_short_bitmap_is_handled_without_panicking() {
        assert!(!reports_key_power(&[]));
        assert!(!reports_key_power(&[0xFF; 10]), "10 bytes does not reach byte 14");
    }
}
