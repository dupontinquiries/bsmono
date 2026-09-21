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

// asm-generic ioctl encoding (arm and aarch64 both use it).
const fn ioc_read(nr: u32, size: u32) -> libc::c_ulong {
    ((2u32 << 30) | (size << 16) | ((b'E' as u32) << 8) | nr) as libc::c_ulong
}
const fn eviocgbit(ev: u32, len: u32) -> libc::c_ulong {
    ioc_read(0x20 + ev, len)
}
const fn eviocgabs(abs: u32) -> libc::c_ulong {
    ioc_read(0x40 + abs, 24)
}
const fn eviocgname(len: u32) -> libc::c_ulong {
    ioc_read(0x06, len)
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
