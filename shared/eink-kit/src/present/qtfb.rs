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
    /// Mode messages (SET_REFRESH_MODE / REQUEST_FULL_REFRESH) allowed now.
    modes: bool,
    trace: bool,
    sent: u32,
    last: &'static str,
    mode: Mode,
    plan: ModePlan,
    reset: bool,
    /// Set by the reader when AppLoad hung up on the current connection.
    peer_closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// A mode request is inside its blame window: the reader must not
    /// report a hang-up as the display closing (the run loop would exit
    /// before the fallback could reconnect; caught by the fake-server test).
    in_flight: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// When to ask AppLoad for the fast waveform.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ModePlan {
    /// Never (default): AppLoad's own mode, the path its shim uses.
    Off,
    /// Once drawing is under way: after the first update plus a delay.
    Deferred { after: Option<std::time::Instant> },
    /// Immediately after init: what failed on a Paper Pro. For testing.
    Immediate,
    /// Sent at this time; for a short window a hang-up is blamed on it.
    Sent { at: std::time::Instant },
    /// Sent and survived the window: the fast waveform is in use.
    Accepted,
    /// AppLoad dropped the connection after a mode message: never again.
    Failed,
}

const DEFER: std::time::Duration = std::time::Duration::from_secs(2);
/// How long after a mode message a hang-up is blamed on it. After this, a
/// hang-up is AppLoad closing the window: the app exits as before (no
/// orphaned process reconnecting to a window that is gone).
const BLAME_WINDOW: std::time::Duration = std::time::Duration::from_secs(4);

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
        Self::connect_at(std::ffi::CString::new(SOCKET_PATH.strip_suffix(b"\0").unwrap()).unwrap(), key, model, mode, input)
    }

    /// [`connect`](Self::connect) to a given socket path (tests run a fake
    /// server built from AppLoad's source).
    pub fn connect_at(socket_path: std::ffi::CString, key: i32, model: Model, mode: Mode, input: InputSender) -> io::Result<QtfbPresent> {
        let plan = match std::env::var("EINK_QTFB_MODES").as_deref() {
            Ok("1") | Ok("deferred") => ModePlan::Deferred { after: None },
            Ok("immediate") => ModePlan::Immediate,
            _ => ModePlan::Off,
        };
        Self::connect_with(socket_path, key, model, mode, input, plan)
    }

    fn connect_with(socket_path: std::ffi::CString, key: i32, model: Model, mode: Mode, input: InputSender, plan: ModePlan) -> io::Result<QtfbPresent> {
        let trace = std::env::var("EINK_TRACE").as_deref() == Ok("1");
        let generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let peer_closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let in_flight = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flags = Flags { generation: generation.clone(), peer_closed: peer_closed.clone(), in_flight: in_flight.clone() };
        let (fd, shm, shm_len) = open_window(&socket_path, key, model, &input, &flags)?;
        let (w, h) = model.size();
        let mut me = QtfbPresent {
            fd,
            shm,
            shm_len,
            caps: Caps { width: w, height: h, stride: w as usize * 2, format: PixelFormat::Rgb565, per_update_waveform: false, color_panel: model.color() },
            modes: false,
            trace,
            sent: 0,
            last: "INITIALIZE",
            mode,
            plan,
            reset: false,
            peer_closed,
            in_flight,
        };
        eprintln!("eink: qtfb refresh mode: {}", match plan {
            ModePlan::Off => "AppLoad's default (EINK_QTFB_MODES=deferred to request the fast waveform)".to_string(),
            ModePlan::Deferred { .. } => format!("{mode:?}, requested {}s after drawing starts (EXPERIMENT: the AppLoad on a Paper Pro closes the window in reply)", DEFER.as_secs()),
            _ => format!("{mode:?} immediately (EINK_QTFB_MODES=immediate)"),
        });
        if plan == ModePlan::Immediate {
            me.request_mode()?;
        }
        Ok(me)
    }

    fn request_mode(&mut self) -> io::Result<()> {
        if self.mode == Mode::Ui {
            self.plan = ModePlan::Failed; // nothing to ask for
            return Ok(());
        }
        let m = ClientMessage { ty: MESSAGE_SET_REFRESH_MODE, body: ClientBody { refresh_mode: self.mode as i32 } };
        self.plan = ModePlan::Sent { at: std::time::Instant::now() };
        self.in_flight.store(true, std::sync::atomic::Ordering::SeqCst);
        self.modes = true;
        self.send(&m, &format!("{:?}", self.mode))
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
            Err(e) if e.raw_os_error() == Some(libc::EPIPE)
                && matches!(self.plan, ModePlan::Sent { at } if at.elapsed() < BLAME_WINDOW) => {
                // Seen on a Paper Pro: AppLoad destroys the WINDOW along with
                // the connection, so a reconnect lands on nothing and the app
                // lingers invisible. Say why and let the app exit.
                self.plan = ModePlan::Failed;
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "AppLoad closed the window after SET_REFRESH_MODE: this AppLoad does not support refresh-mode messages (it tears the window down with the connection, so there is nothing to reconnect to). Run without EINK_QTFB_MODES."))
            }
            Err(e) if e.raw_os_error() == Some(libc::EPIPE) => Err(io::Error::new(io::ErrorKind::BrokenPipe, format!(
                "AppLoad closed the qtfb connection (after {} message(s) since init, last {}; this one {name})",
                self.sent, self.last))),
            Err(e) => Err(e),
        }
    }
}

/// Connect to AppLoad's socket, initialise window `key`, map its buffer and
/// start the input reader. Shared by first connect and reconnect.
fn open_window(
    socket_path: &std::ffi::CString,
    key: i32,
    model: Model,
    input: &InputSender,
    flags: &Flags,
) -> io::Result<(RawFd, *mut u8, usize)> {
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (d, s) in addr.sun_path.iter_mut().zip(socket_path.as_bytes_with_nul()) {
        *d = *s as libc::c_char;
    }
    let r = unsafe { libc::connect(fd, (&addr as *const libc::sockaddr_un).cast(), std::mem::size_of::<libc::sockaddr_un>() as u32) };
    if r != 0 {
        let e = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(io::Error::new(e.kind(), format!("connect {}: {e} (is AppLoad running?)", socket_path.to_string_lossy())));
    }
    let fb_type = model.qtfb_rgb565_type();
    send_msg(fd, &ClientMessage { ty: MESSAGE_INITIALIZE, body: ClientBody { init: InitContents { key, fb_type } } })?;
    let mut resp: ServerMessage = unsafe { std::mem::zeroed() };
    let n = unsafe { libc::recv(fd, (&mut resp as *mut ServerMessage).cast(), std::mem::size_of::<ServerMessage>(), 0) };
    if n < 1 {
        unsafe { libc::close(fd) };
        return Err(io::Error::new(io::ErrorKind::ConnectionRefused, format!("qtfb refused window key {key}")));
    }
    let init = unsafe { resp.body.init };
    let path = format!("/dev/shm/qtfb_{}\0", init.shm_key);
    let shm_fd = unsafe { libc::open(path.as_ptr().cast(), libc::O_RDWR | libc::O_CLOEXEC) };
    if shm_fd < 0 {
        let e = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(e);
    }
    let p = unsafe { libc::mmap(std::ptr::null_mut(), init.shm_size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, shm_fd, 0) };
    unsafe { libc::close(shm_fd) };
    if p == libc::MAP_FAILED {
        let e = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(e);
    }
    let (w, h) = model.size();
    if init.shm_size < w as usize * 2 * h as usize {
        unsafe { libc::munmap(p, init.shm_size); libc::close(fd) };
        return Err(io::Error::new(io::ErrorKind::InvalidData,
            format!("qtfb shm is {} bytes, expected {} for {w}x{h} RGB565 (wrong EINK_MODEL?)", init.shm_size, w as usize * 2 * h as usize)));
    }
    let rfd = unsafe { libc::dup(fd) };
    let (inp, fl, mine) = (input.clone(), flags.clone(), flags.generation.load(std::sync::atomic::Ordering::SeqCst));
    std::thread::Builder::new().name("qtfb-input".into()).spawn(move || reader(rfd, inp, fl, mine))?;
    Ok((fd, p.cast(), init.shm_size))
}

/// Connection state shared with the reader thread.
#[derive(Clone)]
struct Flags {
    generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    peer_closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    in_flight: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

fn reader(fd: RawFd, input: InputSender, flags: Flags, mine: u64) {
    use std::sync::atomic::Ordering::SeqCst;
    loop {
        let mut m: ServerMessage = unsafe { std::mem::zeroed() };
        let n = unsafe { libc::recv(fd, (&mut m as *mut ServerMessage).cast(), std::mem::size_of::<ServerMessage>(), 0) };
        if n < 1 {
            unsafe { libc::close(fd) };
            // A reconnect closes the old socket on purpose: not the end.
            if flags.generation.load(SeqCst) != mine {
                return;
            }
            flags.peer_closed.store(true, SeqCst);
            if flags.in_flight.load(SeqCst) {
                input.send(Input::Wake); // the run loop polls; qtfb decides
            } else {
                input.send(Input::Closed("qtfb socket closed".into()));
            }
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
    fn take_reset(&mut self) -> bool {
        std::mem::take(&mut self.reset)
    }

    fn poll(&mut self) -> io::Result<bool> {
        if !self.peer_closed.load(std::sync::atomic::Ordering::SeqCst) {
            if let ModePlan::Sent { at } = self.plan {
                if at.elapsed() >= BLAME_WINDOW {
                    self.plan = ModePlan::Accepted;
                    self.in_flight.store(false, std::sync::atomic::Ordering::SeqCst);
                    eprintln!("eink: AppLoad accepted {:?}: fast waveform in use", self.mode);
                }
            }
            return Ok(false);
        }
        match self.plan {
            ModePlan::Sent { at } if at.elapsed() < BLAME_WINDOW => {
                self.plan = ModePlan::Failed;
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "AppLoad closed the window after SET_REFRESH_MODE: this AppLoad does not support refresh-mode messages (it tears the window down with the connection, so there is nothing to reconnect to). Run without EINK_QTFB_MODES."))
            }
            _ => Err(io::Error::new(io::ErrorKind::BrokenPipe, "AppLoad closed the qtfb connection")),
        }
    }

    fn commit(&mut self, updates: &[Update]) -> io::Result<()> {
        if let ModePlan::Sent { at } = self.plan {
            if at.elapsed() >= BLAME_WINDOW {
                self.plan = ModePlan::Accepted;
                self.in_flight.store(false, std::sync::atomic::Ordering::SeqCst);
                eprintln!("eink: AppLoad accepted {:?}: fast waveform in use", self.mode);
            }
        }
        if let ModePlan::Deferred { after } = self.plan {
            match after {
                None if !updates.is_empty() => self.plan = ModePlan::Deferred { after: Some(std::time::Instant::now() + DEFER) },
                Some(t) if std::time::Instant::now() >= t => self.request_mode()?,
                _ => {}
            }
        }
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

    /// A fake qtfb server following AppLoad's source (fbmanagement.cpp):
    /// SEQPACKET, INITIALIZE -> shm + response, UPDATE counted. With
    /// `drop_on_mode`, it hangs up after SET_REFRESH_MODE the way AppLoad did
    /// on a Paper Pro.
    struct Fake {
        path: std::path::PathBuf,
        connections: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        updates: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    fn fake_server(drop_on_mode: bool) -> Fake {
        use std::sync::atomic::{AtomicUsize, Ordering};
        // unique per call: tests run in parallel inside one process, and two that both asked for `drop_on_mode = true`
        // used to delete and rebind each other's socket (an intermittent "connect: No such file or directory")
        static SERIAL: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!("qtfb-fake-{}-{}-{}.sock", std::process::id(), drop_on_mode, SERIAL.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_file(&path);
        let c = std::ffi::CString::new(path.to_string_lossy().as_bytes()).unwrap();
        let srv = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0) };
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (d, s) in addr.sun_path.iter_mut().zip(c.as_bytes_with_nul()) { *d = *s as libc::c_char; }
        assert_eq!(unsafe { libc::bind(srv, (&addr as *const libc::sockaddr_un).cast(), std::mem::size_of::<libc::sockaddr_un>() as u32) }, 0);
        assert_eq!(unsafe { libc::listen(srv, 4) }, 0);
        let (connections, updates) = (std::sync::Arc::new(AtomicUsize::new(0)), std::sync::Arc::new(AtomicUsize::new(0)));
        let (cn, up) = (connections.clone(), updates.clone());
        std::thread::spawn(move || loop {
            let fd = unsafe { libc::accept(srv, std::ptr::null_mut(), std::ptr::null_mut()) };
            if fd < 0 { return; }
            let n = cn.fetch_add(1, Ordering::SeqCst) as i32;
            let up = up.clone();
            std::thread::spawn(move || {
                loop {
                    let mut m: ClientMessage = unsafe { std::mem::zeroed() };
                    let r = unsafe { libc::recv(fd, (&mut m as *mut ClientMessage).cast(), std::mem::size_of::<ClientMessage>(), 0) };
                    if r < 1 { break; }
                    match m.ty {
                        MESSAGE_INITIALIZE => {
                            let shm_key = 900_000 + std::process::id() as i32 % 1000 * 10 + n;
                            let (w, h) = Model::PaperPro.size();
                            let size = (w * h * 2) as usize;
                            let name = std::ffi::CString::new(format!("/dev/shm/qtfb_{shm_key}")).unwrap();
                            let sfd = unsafe { libc::open(name.as_ptr(), libc::O_RDWR | libc::O_CREAT, 0o600) };
                            unsafe { libc::ftruncate(sfd, size as i64); libc::close(sfd) };
                            let resp = ServerMessage { ty: MESSAGE_INITIALIZE, body: ServerBody { init: InitResponse { shm_key, shm_size: size } } };
                            unsafe { libc::send(fd, (&resp as *const ServerMessage).cast(), std::mem::size_of::<ServerMessage>(), 0) };
                        }
                        MESSAGE_UPDATE => { up.fetch_add(1, Ordering::SeqCst); }
                        MESSAGE_SET_REFRESH_MODE if drop_on_mode => break, // what AppLoad did
                        MESSAGE_TERMINATE => break,
                        _ => {}
                    }
                }
                unsafe { libc::close(fd) };
            });
        });
        Fake { path, connections, updates }
    }

    fn update(x: i32) -> Update {
        Update { rect: crate::geom::IRect::new(x, 0, x + 10, 10), waveform: Waveform::Fast, color: false, reason: "test" }
    }

    fn pump(p: &mut QtfbPresent, secs: f32) {
        let t = std::time::Instant::now();
        let mut i = 0;
        while t.elapsed().as_secs_f32() < secs {
            p.commit(&[update(i % 100)]).expect("commit must not fail: fallback reconnects");
            i += 1;
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    fn a_dropped_connection_after_the_mode_request_exits_with_the_reason() {
        // AppLoad on a Paper Pro destroys the window with the connection: no
        // reconnect (it would land on nothing), a clear error instead.
        if !std::path::Path::new("/dev/shm").is_dir() { return; }
        let f = fake_server(true);
        let hub = crate::present::InputHub::new().unwrap();
        let path = std::ffi::CString::new(f.path.to_string_lossy().as_bytes()).unwrap();
        let mut p = QtfbPresent::connect_with(path, 42, Model::PaperPro, Mode::Animate, hub.sender(), ModePlan::Deferred { after: None }).unwrap();
        let t = std::time::Instant::now();
        let mut err = None;
        while t.elapsed() < std::time::Duration::from_secs(5) && err.is_none() {
            if let Err(e) = p.commit(&[update(1)]).and_then(|_| p.poll().map(|_| ())) { err = Some(e); }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let e = err.expect("the drop is reported");
        assert!(e.to_string().contains("does not support refresh-mode messages"), "{e}");
        assert_eq!(f.connections.load(std::sync::atomic::Ordering::SeqCst), 1, "no reconnect");
        assert!(hub.drain().iter().all(|i| !matches!(i, Input::Closed(_))), "the reason comes from the backend, not a bare 'closed'");
    }

    #[test]
    fn an_idle_app_also_exits_with_the_reason() {
        if !std::path::Path::new("/dev/shm").is_dir() { return; }
        let f = fake_server(true);
        let hub = crate::present::InputHub::new().unwrap();
        let path = std::ffi::CString::new(f.path.to_string_lossy().as_bytes()).unwrap();
        let mut p = QtfbPresent::connect_with(path, 44, Model::PaperPro, Mode::Animate, hub.sender(), ModePlan::Immediate).unwrap();
        let t = std::time::Instant::now();
        let mut res = Ok(false);
        while t.elapsed() < std::time::Duration::from_secs(2) {
            res = p.poll();
            if res.is_err() { break; }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(res.unwrap_err().to_string().contains("does not support refresh-mode messages"));
    }

    #[test]
    fn an_accepted_mode_stays_and_a_later_hangup_is_a_normal_exit() {
        if !std::path::Path::new("/dev/shm").is_dir() { return; }
        let f = fake_server(false);
        let hub = crate::present::InputHub::new().unwrap();
        let path = std::ffi::CString::new(f.path.to_string_lossy().as_bytes()).unwrap();
        let mut p = QtfbPresent::connect_with(path, 43, Model::PaperPro, Mode::Animate, hub.sender(), ModePlan::Deferred { after: None }).unwrap();
        pump(&mut p, 2.5);
        assert!(matches!(p.plan, ModePlan::Sent { .. }));
        std::thread::sleep(BLAME_WINDOW);
        pump(&mut p, 0.2);
        assert_eq!(p.plan, ModePlan::Accepted, "survived the window: fast waveform in use");
        assert_eq!(f.connections.load(std::sync::atomic::Ordering::SeqCst), 1, "no reconnect when the mode is accepted");
    }

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
