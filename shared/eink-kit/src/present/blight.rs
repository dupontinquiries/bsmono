//! Blight backend: Oxide's display server (Eeems-Org/oxide, `libblight_protocol`).
//!
//! The library is loaded with `dlopen` at runtime instead of linked. That
//! keeps this crate buildable and testable on a host that has no Oxide, and
//! turns "Oxide not installed" into a clear runtime error instead of a
//! loader failure. It does NOT lift the libc constraint: the library is
//! glibc, so the binary must be a gnu (dynamic) build to load it.
//!
//! Prototypes transcribed from `shared/libblight_protocol/libblight_protocol.h`
//! (oxide 6a84e6b). Untested against a live server.
//!
//! What Blight gives that qtfb does not: a waveform AND a colour/mono content
//! hint per repaint, so `per_update_waveform` is true and the policy runs its
//! fast-then-settle scheme; Grayscale8 surfaces for mono panels; and raw
//! `input_event`s per device through a shared-memory ring.

use super::{Caps, Input, InputSender, Present};
use crate::device::Model;
use crate::format::PixelFormat;
use crate::policy::{Update, Waveform};
use crate::touch::{MtDecoder, Transform};
use std::ffi::{c_char, c_int, c_uint, c_ushort, c_void, CString};
use std::io;

type Bus = c_void;

#[repr(C)]
struct BlightBuf {
    fd: c_int,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
    stride: c_uint,
    format: c_int,
    scale: f64,
    data: *mut u8,
}

#[repr(C)]
struct InputBuffer {
    device: c_ushort,
    fd: c_int,
    ring: *mut c_void,
}

// QImage::Format values, which BlightImageFormat mirrors.
const FORMAT_RGB16: c_int = 7;
const FORMAT_GRAYSCALE8: c_int = 24;
// BlightWaveformMode
const WF_ULTRAFAST: c_int = 0;
const WF_FAST: c_int = 1;
const WF_ANIMATE: c_int = 2;
const WF_UI: c_int = 4;
const WF_FULL: c_int = 5;
// BlightContentType
const CT_MONO: c_int = 0;
const CT_COLOR: c_int = 1;
// BlightUpdateMode
const UM_PARTIAL: c_int = 0;
const UM_FULL: c_int = 1;

struct Api {
    bus_connect_system: unsafe extern "C" fn(*mut *mut Bus) -> c_int,
    service_available: unsafe extern "C" fn(*mut Bus) -> bool,
    service_open: unsafe extern "C" fn(*mut Bus) -> c_int,
    create_buffer: unsafe extern "C" fn(c_int, c_int, c_uint, c_uint, c_uint, c_int, f64) -> *mut BlightBuf,
    add_surface: unsafe extern "C" fn(*mut Bus, *mut BlightBuf) -> c_ushort,
    surface_repaint: unsafe extern "C" fn(c_int, c_ushort, c_int, c_int, c_uint, c_uint, c_int, c_int, c_int) -> c_uint,
    remove_surface: unsafe extern "C" fn(c_int, c_ushort) -> c_int,
    service_input_open: unsafe extern "C" fn(*mut Bus, c_ushort) -> *mut InputBuffer,
    event_from_buffer: unsafe extern "C" fn(*mut InputBuffer, *mut *mut libc::input_event, bool) -> c_int,
    event_free: unsafe extern "C" fn(*mut libc::input_event),
}

fn load() -> io::Result<Api> {
    let candidates: Vec<String> = match std::env::var("EINK_BLIGHT_LIB") {
        Ok(p) => vec![p],
        Err(_) => ["libblight_protocol.so", "libblight_protocol.so.1", "/opt/lib/libblight_protocol.so", "/usr/lib/libblight_protocol.so"]
            .iter().map(|s| s.to_string()).collect(),
    };
    let mut handle = std::ptr::null_mut();
    for c in &candidates {
        let cs = CString::new(c.as_str()).unwrap();
        handle = unsafe { libc::dlopen(cs.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if !handle.is_null() {
            break;
        }
    }
    if handle.is_null() {
        return Err(io::Error::new(io::ErrorKind::NotFound,
            format!("libblight_protocol not found (tried {candidates:?}; is Oxide installed? set EINK_BLIGHT_LIB)")));
    }
    unsafe fn sym<T>(h: *mut c_void, name: &str) -> io::Result<T> {
        let cs = CString::new(name).unwrap();
        let p = libc::dlsym(h, cs.as_ptr() as *const c_char);
        if p.is_null() {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("libblight_protocol lacks {name}")));
        }
        Ok(std::mem::transmute_copy(&p))
    }
    unsafe {
        Ok(Api {
            bus_connect_system: sym(handle, "blight_bus_connect_system")?,
            service_available: sym(handle, "blight_service_available")?,
            service_open: sym(handle, "blight_service_open")?,
            create_buffer: sym(handle, "blight_create_buffer")?,
            add_surface: sym(handle, "blight_add_surface")?,
            surface_repaint: sym(handle, "blight_surface_repaint")?,
            remove_surface: sym(handle, "blight_remove_surface")?,
            service_input_open: sym(handle, "blight_service_input_open")?,
            event_from_buffer: sym(handle, "blight_event_from_buffer")?,
            event_free: sym(handle, "blight_event_free")?,
        })
    }
}

/// Which Blight waveform the policy's "fast" maps to. Unmeasured on the
/// colour panel, hence the knob (`EINK_FAST_WAVEFORM=ufast|fast|animate`).
pub fn fast_waveform(name: &str) -> Option<c_int> {
    match name {
        "ufast" | "ultrafast" => Some(WF_ULTRAFAST),
        "fast" => Some(WF_FAST),
        "animate" => Some(WF_ANIMATE),
        _ => None,
    }
}

pub struct BlightPresent {
    api: std::sync::Arc<Api>,
    fd: c_int,
    surface: c_ushort,
    buf: *mut BlightBuf,
    caps: Caps,
    fast: c_int,
}

unsafe impl Send for BlightPresent {}
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

struct SendPtr<T>(*mut T);
unsafe impl<T> Send for SendPtr<T> {}

impl BlightPresent {
    pub fn connect(model: Model, fast: c_int, touch: Transform, input: InputSender) -> io::Result<BlightPresent> {
        let api = std::sync::Arc::new(load()?);
        let mut bus = std::ptr::null_mut();
        if unsafe { (api.bus_connect_system)(&mut bus) } < 0 || bus.is_null() {
            return Err(io::Error::new(io::ErrorKind::ConnectionRefused, "blight: cannot connect to the system bus"));
        }
        if !unsafe { (api.service_available)(bus) } {
            return Err(io::Error::new(io::ErrorKind::NotFound, "blight: display server not running (start Oxide / its display server)"));
        }
        let fd = unsafe { (api.service_open)(bus) };
        if fd < 0 {
            return Err(io::Error::new(io::ErrorKind::ConnectionRefused, "blight: service_open failed"));
        }
        let (w, h) = model.size();
        // Grey for mono panels: a third of the memory of RGB888 and exactly
        // what the panel shows. RGB565 for colour: half of RGB888.
        let (fmt, qfmt) = if model.color() { (PixelFormat::Rgb565, FORMAT_RGB16) } else { (PixelFormat::Gray8, FORMAT_GRAYSCALE8) };
        let stride = w as usize * fmt.bytes_per_pixel();
        let buf = unsafe { (api.create_buffer)(0, 0, w, h, stride as c_uint, qfmt, 1.0) };
        if buf.is_null() || unsafe { (*buf).data.is_null() } {
            return Err(io::Error::new(io::ErrorKind::Other, "blight: create_buffer failed"));
        }
        let surface = unsafe { (api.add_surface)(bus, buf) };
        if surface == 0 {
            return Err(io::Error::new(io::ErrorKind::Other, "blight: add_surface failed"));
        }
        // Input: find the touchscreen ourselves (for its axis ranges), then
        // read it through Blight's ring rather than the device file, since
        // the display server owns input while it runs.
        match super::evdev::find_touchscreen() {
            Some(dev) => {
                let ib = unsafe { (api.service_input_open)(bus, dev.number) };
                if ib.is_null() {
                    eprintln!("eink: blight refused input for event{}; touch disabled", dev.number);
                } else {
                    eprintln!("eink: touch = event{} {:?} x{:?} y{:?}", dev.number, dev.name, dev.x, dev.y);
                    let api2 = api.clone();
                    let ib = SendPtr(ib);
                    let mut dec = MtDecoder::new(dev.x, dev.y, w, h, touch);
                    std::thread::Builder::new().name("blight-touch".into()).spawn(move || {
                        let ib = ib;
                        let mut out = Vec::new();
                        loop {
                            let mut ev: *mut libc::input_event = std::ptr::null_mut();
                            let r = unsafe { (api2.event_from_buffer)(ib.0, &mut ev, true) };
                            if r < 0 || ev.is_null() {
                                input.send(Input::Closed("blight input ring closed".into()));
                                return;
                            }
                            let e = unsafe { *ev };
                            unsafe { (api2.event_free)(ev) };
                            dec.feed(e.type_, e.code, e.value, &mut out);
                            super::evdev::forward(&dec, &mut out, &input);
                        }
                    })?;
                }
            }
            None => eprintln!("eink: no multitouch device found; touch disabled"),
        }
        Ok(BlightPresent {
            api,
            fd,
            surface,
            buf,
            caps: Caps { width: w, height: h, stride, format: fmt, per_update_waveform: true, color_panel: model.color() },
            fast,
        })
    }
}

impl Present for BlightPresent {
    fn name(&self) -> &'static str {
        "blight"
    }
    fn caps(&self) -> Caps {
        self.caps
    }
    fn buffer(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut((*self.buf).data, self.caps.stride * self.caps.height as usize) }
    }
    fn commit(&mut self, updates: &[Update]) -> io::Result<()> {
        for u in updates {
            let (wf, mode) = match u.waveform {
                Waveform::Fast => (self.fast, UM_PARTIAL),
                Waveform::Quality => (WF_UI, UM_PARTIAL),
                Waveform::Full => (WF_FULL, UM_FULL),
            };
            let ct = if u.color { CT_COLOR } else { CT_MONO };
            let r = u.rect;
            let marker = unsafe {
                (self.api.surface_repaint)(self.fd, self.surface, r.x0, r.y0, r.width() as c_uint, r.height() as c_uint, wf, ct, mode)
            };
            if marker == 0 {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "blight: repaint failed"));
            }
        }
        Ok(())
    }
}

impl Drop for BlightPresent {
    fn drop(&mut self) {
        unsafe { (self.api.remove_surface)(self.fd, self.surface) };
    }
}
