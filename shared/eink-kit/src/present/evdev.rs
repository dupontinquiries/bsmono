//! Kernel input devices: find the touchscreen, read its axis ranges, and
//! (for backends without their own input channel) read it directly.
//!
//! Reading is NON-exclusive: no EVIOCGRAB. If xochitl is also reading the
//! device it keeps working; that is the "play nice" rule. It also means
//! this direct reader must not be used under qtfb, where xochitl already
//! forwards the same touches, or every tap would arrive twice.

use super::{Input, InputSender};
use crate::touch::{AbsRange, MtDecoder, Transform, ABS_MT_POSITION_X, ABS_MT_POSITION_Y, EV_ABS};
use std::io;
use std::os::fd::RawFd;

// asm-generic ioctl encoding (arm and aarch64 both use it), computed as
// u32 like the kernel's _IOC macro and cast to `libc::Ioctl`: that is
// `c_ulong` on glibc but `c_int` on MUSL, and the tablet's AppLoad build is
// static musl. Returning c_ulong compiled on this glibc host and would not
// have type-checked on the build that matters. On musl the cast wraps the
// read bit (1 << 31) negative, which is exactly what C passes.
const fn ioc_read(nr: u32, size: u32) -> libc::Ioctl {
    ((2u32 << 30) | (size << 16) | ((b'E' as u32) << 8) | nr) as libc::Ioctl
}
const fn eviocgbit(ev: u32, len: u32) -> libc::Ioctl {
    ioc_read(0x20 + ev, len)
}
const fn eviocgabs(abs: u32) -> libc::Ioctl {
    ioc_read(0x40 + abs, 24)
}
const fn eviocgname(len: u32) -> libc::Ioctl {
    ioc_read(0x06, len)
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Values from <linux/input.h> as C computes them (asm-generic).
    #[test]
    fn ioctl_numbers_match_the_kernel_headers() {
        assert_eq!(eviocgabs(0x35) as u32, 0x8018_4575); // EVIOCGABS(ABS_MT_POSITION_X)
        assert_eq!(eviocgbit(0x03, 8) as u32, 0x8008_4523); // EVIOCGBIT(EV_ABS, 8)
        assert_eq!(eviocgname(128) as u32, 0x8080_4506); // EVIOCGNAME(128)
    }
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

/// A touchscreen found on this system.
#[derive(Debug, Clone)]
pub struct TouchDevice {
    pub number: u16,
    pub path: String,
    pub name: String,
    pub x: AbsRange,
    pub y: AbsRange,
}

fn open_ro(path: &str) -> io::Result<RawFd> {
    let c = std::ffi::CString::new(path).unwrap();
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(fd) }
}

fn probe(number: u16) -> Option<TouchDevice> {
    let path = format!("/dev/input/event{number}");
    let fd = open_ro(&path).ok()?;
    let mut bits = [0u8; 8]; // ABS_MAX is 0x3f → 64 bits
    let ok = unsafe { libc::ioctl(fd, eviocgbit(EV_ABS as u32, bits.len() as u32), bits.as_mut_ptr()) } >= 0;
    let has = |code: u16| bits[(code / 8) as usize] & (1 << (code % 8)) != 0;
    let result = if ok && has(ABS_MT_POSITION_X) && has(ABS_MT_POSITION_Y) {
        let mut ax = AbsInfo::default();
        let mut ay = AbsInfo::default();
        unsafe {
            libc::ioctl(fd, eviocgabs(ABS_MT_POSITION_X as u32), &mut ax);
            libc::ioctl(fd, eviocgabs(ABS_MT_POSITION_Y as u32), &mut ay);
        }
        let mut name = [0u8; 128];
        unsafe { libc::ioctl(fd, eviocgname(name.len() as u32), name.as_mut_ptr()) };
        let name = String::from_utf8_lossy(&name).trim_end_matches('\0').to_string();
        Some(TouchDevice {
            number,
            path: path.clone(),
            name,
            x: AbsRange { min: ax.minimum, max: ax.maximum },
            y: AbsRange { min: ay.minimum, max: ay.maximum },
        })
    } else {
        None
    };
    unsafe { libc::close(fd) };
    result
}

/// The pen digitizer: `EINK_PEN_DEV=/dev/input/eventN` if set, else the
/// first device with absolute X/Y and `BTN_TOOL_PEN`.
pub fn find_pen() -> Option<TouchDevice> {
    use crate::touch::{ABS_X, ABS_Y, BTN_TOOL_PEN, EV_KEY};
    let probe_pen = |number: u16| -> Option<TouchDevice> {
        let path = format!("/dev/input/event{number}");
        let fd = open_ro(&path).ok()?;
        let mut abs = [0u8; 8];
        let mut keys = [0u8; 96]; // KEY_MAX 0x2ff → 768 bits
        let ok = unsafe {
            libc::ioctl(fd, eviocgbit(EV_ABS as u32, abs.len() as u32), abs.as_mut_ptr()) >= 0
                && libc::ioctl(fd, eviocgbit(EV_KEY as u32, keys.len() as u32), keys.as_mut_ptr()) >= 0
        };
        let has = |bits: &[u8], code: u16| bits.get((code / 8) as usize).is_some_and(|b| b & (1 << (code % 8)) != 0);
        let r = if ok && has(&abs, ABS_X) && has(&abs, ABS_Y) && has(&keys, BTN_TOOL_PEN) {
            let (mut ax, mut ay) = (AbsInfo::default(), AbsInfo::default());
            unsafe {
                libc::ioctl(fd, eviocgabs(ABS_X as u32), &mut ax);
                libc::ioctl(fd, eviocgabs(ABS_Y as u32), &mut ay);
            }
            let mut name = [0u8; 128];
            unsafe { libc::ioctl(fd, eviocgname(name.len() as u32), name.as_mut_ptr()) };
            Some(TouchDevice {
                number,
                path: path.clone(),
                name: String::from_utf8_lossy(&name).trim_end_matches('\0').to_string(),
                x: AbsRange { min: ax.minimum, max: ax.maximum },
                y: AbsRange { min: ay.minimum, max: ay.maximum },
            })
        } else {
            None
        };
        unsafe { libc::close(fd) };
        r
    };
    if let Ok(p) = std::env::var("EINK_PEN_DEV") {
        return probe_pen(p.trim_start_matches("/dev/input/event").parse().ok()?);
    }
    (0u16..32).find_map(probe_pen)
}

/// The multitouch device: `EINK_TOUCH_DEV=/dev/input/eventN` if set, else
/// the first device reporting MT position axes. (The pen digitizer reports
/// single-touch ABS_X/ABS_Y and is skipped by that rule.)
pub fn find_touchscreen() -> Option<TouchDevice> {
    if let Ok(p) = std::env::var("EINK_TOUCH_DEV") {
        let n = p.trim_start_matches("/dev/input/event").parse().ok()?;
        return probe(n);
    }
    (0u16..32).find_map(probe)
}

/// Blocking reader thread over a device file.
pub fn spawn_reader(dev: &TouchDevice, width: u32, height: u32, t: Transform, input: InputSender) -> io::Result<()> {
    let fd = open_ro(&dev.path)?;
    let mut dec = MtDecoder::new(dev.x, dev.y, width, height, t);
    std::thread::Builder::new().name("evdev-touch".into()).spawn(move || {
        let sz = std::mem::size_of::<libc::input_event>();
        let mut evs: [libc::input_event; 64] = unsafe { std::mem::zeroed() };
        let mut out = Vec::new();
        loop {
            let n = unsafe { libc::read(fd, evs.as_mut_ptr().cast(), sz * evs.len()) };
            if n <= 0 {
                input.send(Input::Closed(format!("touch device read failed: {}", io::Error::last_os_error())));
                return;
            }
            for e in &evs[..n as usize / sz] {
                dec.feed(e.type_, e.code, e.value, &mut out);
            }
            forward(&dec, &mut out, &input);
        }
    })?;
    Ok(())
}

/// Blocking reader thread over the pen digitizer, the pen counterpart of [`spawn_reader`].
pub fn spawn_pen_reader(dev: &TouchDevice, width: u32, height: u32, t: Transform, input: InputSender) -> io::Result<()> {
    let fd = open_ro(&dev.path)?;
    let mut dec = crate::touch::PenDecoder::new(dev.x, dev.y, width, height, t);
    std::thread::Builder::new().name("evdev-pen".into()).spawn(move || {
        let sz = std::mem::size_of::<libc::input_event>();
        let mut evs: [libc::input_event; 64] = unsafe { std::mem::zeroed() };
        let mut out = Vec::new();
        loop {
            let n = unsafe { libc::read(fd, evs.as_mut_ptr().cast(), sz * evs.len()) };
            if n <= 0 { return; } // touch reports Closed; the pen just stops
            for e in &evs[..n as usize / sz] {
                dec.feed(e.type_, e.code, e.value, &mut out);
            }
            forward_pen(&dec, &mut out, &input);
        }
    })?;
    Ok(())
}

/// Start the direct touch and pen readers for a `width` x `height` surface; both send through `input`.
pub fn start_direct(touch: Transform, pen: Transform, width: u32, height: u32, input: InputSender) {
    match find_touchscreen() {
        Some(dev) => {
            eprintln!("eink: direct touch = event{} {:?} x{:?} y{:?} -> {width}x{height}", dev.number, dev.name, dev.x, dev.y);
            if let Err(e) = spawn_reader(&dev, width, height, touch, input.clone()) { eprintln!("eink: direct touch failed: {e}"); }
        }
        None => eprintln!("eink: no multitouch device found for direct input"),
    }
    match find_pen() {
        Some(dev) => {
            eprintln!("eink: direct pen = event{} {:?} x{:?} y{:?} -> {width}x{height}", dev.number, dev.name, dev.x, dev.y);
            if let Err(e) = spawn_pen_reader(&dev, width, height, pen, input) { eprintln!("eink: direct pen failed: {e}"); }
        }
        None => eprintln!("eink: no pen digitizer found for direct input"),
    }
}

pub(crate) fn forward_pen(dec: &crate::touch::PenDecoder, out: &mut Vec<crate::touch::TouchEvent>, input: &InputSender) {
    if let Some((x, y)) = dec.last_raw {
        input.send(Input::Raw { x, y });
    }
    for t in out.drain(..) {
        input.send(Input::Touch(t));
    }
}

pub(crate) fn forward(dec: &MtDecoder, out: &mut Vec<crate::touch::TouchEvent>, input: &InputSender) {
    if out.is_empty() {
        return;
    }
    if let Some((x, y)) = dec.last_raw {
        input.send(Input::Raw { x, y });
    }
    for t in out.drain(..) {
        input.send(Input::Touch(t));
    }
}
