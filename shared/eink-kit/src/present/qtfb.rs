//! qtfb backend: a window inside xochitl, hosted by AppLoad.
//!
//! Protocol from asivery/rm-appload `src/qtfb/common.h` (read at 95d2de5):
//! a SOCK_SEQPACKET socket at /tmp/qtfb.sock, raw C structs, a POSIX shm
//! buffer `/dev/shm/qtfb_<key>` the client draws into, and UPDATE messages
//! naming the changed rect. The window's key comes from AppLoad in
//! `QTFB_KEY`; a process not launched by AppLoad has no window to attach to.
//!
//! Limits that shape this backend (all from reading the server source):
//! * Refresh mode is per-window, and setting it sleeps the server's
//!   connection thread for one second. So the mode is set ONCE at startup
//!   and `per_update_waveform` is false; the policy adapts.
//! * No completion signal: updates are fire-and-forget.
//! * Touch arrives already mapped to framebuffer pixels (rotation included),
//!   so no calibration is needed here. The pen arrives as Qt mouse events
//!   and is forwarded as PEN_*; it is treated as one more finger so it can
//!   press buttons too.

use super::{Caps, Input, InputSender, Present};
use crate::device::Model;
use crate::format::PixelFormat;
use crate::policy::{Update, Waveform};
use crate::touch::{Phase, TouchEvent};
use std::io;
use std::os::fd::RawFd;

const SOCKET_PATH: &[u8] = b"/tmp/qtfb.sock\0";
const MESSAGE_INITIALIZE: u8 = 0;
const MESSAGE_UPDATE: u8 = 1;
const MESSAGE_TERMINATE: u8 = 3;
const MESSAGE_USERINPUT: u8 = 4;
const MESSAGE_SET_REFRESH_MODE: u8 = 5;
const MESSAGE_REQUEST_FULL_REFRESH: u8 = 6;
const UPDATE_ALL: i32 = 0;
const UPDATE_PARTIAL: i32 = 1;

const INPUT_TOUCH_PRESS: i32 = 0x10;
const INPUT_TOUCH_RELEASE: i32 = 0x11;
const INPUT_TOUCH_UPDATE: i32 = 0x12;
const INPUT_PEN_PRESS: i32 = 0x20;
const INPUT_PEN_RELEASE: i32 = 0x21;
const INPUT_PEN_UPDATE: i32 = 0x22;
use crate::touch::PEN_ID;

/// qtfb refresh modes (REFRESH_MODE_*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Mode {
    UltraFast = 0,
    Fast = 1,
    Animate = 2,
    Content = 3,
    Ui = 4,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim() {
            "ufast" | "ultrafast" => Some(Mode::UltraFast),
            "fast" => Some(Mode::Fast),
            "animate" => Some(Mode::Animate),
            "content" => Some(Mode::Content),
            "ui" => Some(Mode::Ui),
            _ => None,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InitContents {
    key: i32,
    fb_type: u8,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct UpdateContents {
    kind: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}
#[repr(C)]
union ClientBody {
    init: InitContents,
    update: UpdateContents,
    refresh_mode: i32,
}
#[repr(C)]
struct ClientMessage {
    ty: u8,
    body: ClientBody,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct InitResponse {
    shm_key: i32,
    shm_size: usize,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct UserInput {
    input_type: i32,
    dev_id: i32,
    x: i32,
    y: i32,
    d: i32,
}
#[repr(C)]
union ServerBody {
    init: InitResponse,
    input: UserInput,
    _state: [i32; 2],
}
#[repr(C)]
struct ServerMessage {
    ty: u8,
    body: ServerBody,
}

fn send_msg(fd: RawFd, m: &ClientMessage) -> io::Result<()> {
    let n = unsafe { libc::send(fd, (m as *const ClientMessage).cast(), std::mem::size_of::<ClientMessage>(), libc::MSG_NOSIGNAL) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub struct QtfbPresent {
    fd: RawFd,
    shm: *mut u8,
    shm_len: usize,
    caps: Caps,
    /// Send SET_REFRESH_MODE / REQUEST_FULL_REFRESH (EINK_QTFB_MODES=1).
    modes: bool,
    trace: bool,
    sent: u32,
    last: &'static str,
}

fn type_name(t: u8) -> &'static str {
    match t {
        MESSAGE_INITIALIZE => "INITIALIZE",
        MESSAGE_UPDATE => "UPDATE",
        MESSAGE_TERMINATE => "TERMINATE",
        MESSAGE_SET_REFRESH_MODE => "SET_REFRESH_MODE",
        MESSAGE_REQUEST_FULL_REFRESH => "REQUEST_FULL_REFRESH",
        _ => "?",
    }
}

// The shm pointer is only touched from the runner thread.
unsafe impl Send for QtfbPresent {}

impl QtfbPresent {
    /// Connect to window `key`, map its buffer, set the refresh mode once,
    /// and start a thread forwarding touch/pen input to `input`.
    ///
    /// Refresh-mode and full-refresh messages are OFF by default. AppLoad's
    /// own shim (the path KOReader uses, working on a Paper Pro) never sends
    /// them; on a real Paper Pro, a SET_REFRESH_MODE sent right after init was
    /// followed by the server's recv failing and the connection closing
    /// within milliseconds (every later UPDATE then failed with EPIPE). So
    /// this follows the proven path: UPDATEs only, and a full refresh is an
    /// UPDATE_ALL. `EINK_QTFB_MODES=1` restores the mode messages to test.
    pub fn connect(key: i32, model: Model, mode: Mode, input: InputSender) -> io::Result<QtfbPresent> {
        let modes = std::env::var("EINK_QTFB_MODES").as_deref() == Ok("1");
        let trace = std::env::var("EINK_TRACE").as_deref() == Ok("1");
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (d, s) in addr.sun_path.iter_mut().zip(SOCKET_PATH) {
            *d = *s as libc::c_char;
        }
        let r = unsafe {
            libc::connect(fd, (&addr as *const libc::sockaddr_un).cast(), std::mem::size_of::<libc::sockaddr_un>() as u32)
        };
        if r != 0 {
            let e = io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(io::Error::new(e.kind(), format!("connect /tmp/qtfb.sock: {e} (is AppLoad running?)")));
        }
        let fb_type = model.qtfb_rgb565_type();
        send_msg(fd, &ClientMessage { ty: MESSAGE_INITIALIZE, body: ClientBody { init: InitContents { key, fb_type } } })?;
        let mut resp: ServerMessage = unsafe { std::mem::zeroed() };
        let n = unsafe { libc::recv(fd, (&mut resp as *mut ServerMessage).cast(), std::mem::size_of::<ServerMessage>(), 0) };
        if n < 1 {
            return Err(io::Error::new(io::ErrorKind::ConnectionRefused, format!("qtfb refused window key {key}")));
        }
        let init = unsafe { resp.body.init };
        let path = format!("/dev/shm/qtfb_{}\0", init.shm_key);
        let shm_fd = unsafe { libc::open(path.as_ptr().cast(), libc::O_RDWR | libc::O_CLOEXEC) };
        if shm_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let p = unsafe { libc::mmap(std::ptr::null_mut(), init.shm_size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, shm_fd, 0) };
        unsafe { libc::close(shm_fd) };
        if p == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let (w, h) = model.size();
        let stride = w as usize * 2;
        if init.shm_size < stride * h as usize {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                format!("qtfb shm is {} bytes, expected {} for {w}x{h} RGB565 (wrong EINK_MODEL?)", init.shm_size, stride * h as usize)));
        }
        if modes && mode != Mode::Ui {
            // Costs one second of server-side sleep; done once, here.
            send_msg(fd, &ClientMessage { ty: MESSAGE_SET_REFRESH_MODE, body: ClientBody { refresh_mode: mode as i32 } })?;
        }
        eprintln!("eink: qtfb refresh mode: {}", if modes { format!("{mode:?} (EINK_QTFB_MODES=1)") } else { "AppLoad's default (mode messages off; EINK_QTFB_MODES=1 to send them)".into() });
        let rfd = unsafe { libc::dup(fd) };
        std::thread::Builder::new().name("qtfb-input".into()).spawn(move || reader(rfd, input))?;
        Ok(QtfbPresent {
            fd,
            shm: p.cast(),
            shm_len: init.shm_size,
            caps: Caps { width: w, height: h, stride, format: PixelFormat::Rgb565, per_update_waveform: false, color_panel: model.color() },
            modes,
            trace,
            sent: 0,
            last: "INITIALIZE",
        })
    }

    /// Send one message, counting and (EINK_TRACE=1) logging it. A broken
    /// connection is reported with how far it got, so a server that hangs up
    /// on a particular message shows which one.
    fn send(&mut self, m: &ClientMessage, what: &str) -> io::Result<()> {
        let name = type_name(m.ty);
        if self.trace {
            eprintln!("eink: qtfb send #{} {name} {what}", self.sent + 1);
        }
        match send_msg(self.fd, m) {
            Ok(()) => {
                self.sent += 1;
                self.last = name;
                Ok(())
            }
            Err(e) if e.raw_os_error() == Some(libc::EPIPE) => Err(io::Error::new(io::ErrorKind::BrokenPipe, format!(
                "AppLoad closed the qtfb connection (after {} message(s) since init, last {}; this one {name})",
                self.sent, self.last))),
            Err(e) => Err(e),
        }
    }
}

fn reader(fd: RawFd, input: InputSender) {
    loop {
        let mut m: ServerMessage = unsafe { std::mem::zeroed() };
        let n = unsafe { libc::recv(fd, (&mut m as *mut ServerMessage).cast(), std::mem::size_of::<ServerMessage>(), 0) };
        if n < 1 {
            input.send(Input::Closed("qtfb socket closed".into()));
            unsafe { libc::close(fd) };
            return;
        }
        if m.ty != MESSAGE_USERINPUT {
            continue; // rotation/state changes: not handled yet
        }
        let u = unsafe { m.body.input };
        let (phase, id) = match u.input_type {
            INPUT_TOUCH_PRESS => (Phase::Start, u.dev_id as u32),
            INPUT_TOUCH_UPDATE => (Phase::Move, u.dev_id as u32),
            INPUT_TOUCH_RELEASE => (Phase::End, u.dev_id as u32),
            INPUT_PEN_PRESS => (Phase::Start, PEN_ID),
            INPUT_PEN_UPDATE => (Phase::Move, PEN_ID),
            INPUT_PEN_RELEASE => (Phase::End, PEN_ID),
            _ => continue,
        };
        input.send(Input::Touch(TouchEvent { id, phase, x: u.x as f32, y: u.y as f32 }));
    }
}

impl Present for QtfbPresent {
    fn name(&self) -> &'static str {
        "qtfb"
    }
    fn caps(&self) -> Caps {
        self.caps
    }
    fn buffer(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.shm, self.shm_len) }
    }
    fn commit(&mut self, updates: &[Update]) -> io::Result<()> {
        for u in updates {
            if u.waveform == Waveform::Full {
                if self.modes {
                    self.send(&ClientMessage { ty: MESSAGE_REQUEST_FULL_REFRESH, body: ClientBody { refresh_mode: 0 } }, "")?;
                } else {
                    // The shim's path: a whole-screen update.
                    self.send(&ClientMessage {
                        ty: MESSAGE_UPDATE,
                        body: ClientBody { update: UpdateContents { kind: UPDATE_ALL, x: 0, y: 0, w: 0, h: 0 } },
                    }, "all (full refresh)")?;
                }
                continue;
            }
            let r = u.rect;
            self.send(&ClientMessage {
                ty: MESSAGE_UPDATE,
                body: ClientBody { update: UpdateContents { kind: UPDATE_PARTIAL, x: r.x0, y: r.y0, w: r.width(), h: r.height() } },
            }, &format!("{}x{}+{}+{}", r.width(), r.height(), r.x0, r.y0))?;
        }
        Ok(())
    }
}

impl Drop for QtfbPresent {
    fn drop(&mut self) {
        let _ = send_msg(self.fd, &ClientMessage { ty: MESSAGE_TERMINATE, body: ClientBody { refresh_mode: 0 } });
        unsafe {
            libc::munmap(self.shm.cast(), self.shm_len);
            libc::close(self.fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The server `recv`s exactly sizeof(ClientMessage). These are the C
    /// sizes for the header as read; a mismatch would silently desync.
    #[test]
    fn wire_structs_match_the_c_layout() {
        assert_eq!(std::mem::size_of::<ClientMessage>(), 24);
        assert_eq!(std::mem::align_of::<ClientBody>(), 4);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(std::mem::size_of::<ServerMessage>(), 32);
        #[cfg(target_pointer_width = "32")]
        assert_eq!(std::mem::size_of::<ServerMessage>(), 24);
    }
}
