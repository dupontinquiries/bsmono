//! Output backends and the input plumbing they share.
//!
//! [`Present`] is the seam that varies per device: who owns the panel. It is
//! output only. Input runs on threads (a blocking read per source) and is
//! handed to the runner through an [`InputHub`], whose wake pipe lets the
//! runner sleep in `poll()` with zero CPU until a finger lands or a timer
//! is due. That is what keeps an idle app from draining the battery.

pub mod mem;
// THE TABLET BACKENDS ARE LINUX-ONLY: qtfb (AppLoad's framebuffer socket),
// Blight and evdev use SOCK_CLOEXEC, pipe2, input_event and Linux ioctls. They
// were compiled everywhere, so any crate depending on eink-kit failed to build
// on macOS -- gitmind did, for a single PNG icon helper. Seen on a Mac running
// fleet_build.sh. `mem` and everything else here is portable.
#[cfg(target_os = "linux")]
pub mod qtfb;
#[cfg(target_os = "linux")]
pub mod blight;
#[cfg(target_os = "linux")]
pub mod evdev;

use crate::format::PixelFormat;
use crate::policy::Update;
use crate::touch::TouchEvent;
use std::io;
use std::os::fd::RawFd;
use std::sync::mpsc;

/// What a backend can do. Read once at startup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Caps {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: PixelFormat,
    /// Can each update carry its own waveform? (Blight: yes. qtfb: no.)
    pub per_update_waveform: bool,
    /// Does the panel show colour? Picks the default profile.
    pub color_panel: bool,
}

pub trait Present {
    fn name(&self) -> &'static str;
    fn caps(&self) -> Caps;
    /// The live, shared framebuffer. Also the "previous frame" for diffing.
    fn buffer(&mut self) -> &mut [u8];
    fn commit(&mut self, updates: &[Update]) -> io::Result<()>;
    /// True once after the backend replaced its buffer (e.g. reconnected to
    /// the display server): the next frame must redraw everything, since the
    /// new buffer holds none of what was drawn.
    fn take_reset(&mut self) -> bool {
        false
    }
    /// Called by the run loop every turn. `Ok(true)`: the backend replaced
    /// its buffer (redraw everything); `Err`: the display is gone.
    fn poll(&mut self) -> io::Result<bool> {
        Ok(false)
    }
}

/// Things input threads report.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Touch(TouchEvent),
    /// Raw digitizer coordinates alongside a touch, for calibration UIs.
    Raw { x: i32, y: i32 },
    /// The display server went away; the runner should exit.
    Closed(String),
    /// Nothing to deliver; wake the run loop so it polls the backend.
    Wake,
}

/// Receiving end, owned by the runner.
pub struct InputHub {
    rx: mpsc::Receiver<Input>,
    tx: mpsc::Sender<Input>,
    wake_r: RawFd,
    wake_w: RawFd,
}

/// What an [`InputSender`] does with touch and pen it is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Forward everything.
    Pass,
    /// Drop touch and pen (and their raw coordinates); `Closed` and `Wake` still go through. Used for the display
    /// server's own forwarded input when the digitizers are read directly (`EINK_INPUT=evdev`).
    DropTouch,
    /// Drop finger touch but let the pen (id `PEN_ID`) through: the finger is read directly while the pen still comes
    /// from the display server (`EINK_INPUT=evdev`, until the pen's direct mapping has been checked).
    DropFinger,
    /// Print each touch to the log and forward nothing (`EINK_INPUT=compare`): lets the direct reader's coordinates be
    /// checked against the display server's before it is trusted.
    LogTouch,
}

/// What `EINK_INPUT` means under qtfb: how AppLoad's forwarded input is treated, and which direct readers run and what
/// they do with what they read. Pure, so the table can be tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPlan {
    pub qtfb: Route,
    /// The direct touch reader, if it runs.
    pub touch: Option<Route>,
    /// The direct pen reader, if it runs.
    pub pen: Option<Route>,
}

impl InputPlan {
    pub fn parse(mode: &str) -> InputPlan {
        use Route::*;
        match mode {
            // the finger is read directly; the pen still comes from AppLoad
            "evdev" => InputPlan { qtfb: DropFinger, touch: Some(Pass), pen: None },
            // finger direct, the pen from AppLoad as usual, and the direct pen reader only LOGS (to check its mapping)
            "evdev-pen-compare" => InputPlan { qtfb: DropFinger, touch: Some(Pass), pen: Some(LogTouch) },
            // both read directly
            "evdev-all" => InputPlan { qtfb: DropTouch, touch: Some(Pass), pen: Some(Pass) },
            // AppLoad drives; both direct readers only log
            "compare" => InputPlan { qtfb: Pass, touch: Some(LogTouch), pen: Some(LogTouch) },
            // anything else (including the default "qtfb") is AppLoad's input alone
            _ => InputPlan { qtfb: Pass, touch: None, pen: None },
        }
    }
}

/// Sending end, cloned into each input thread.
#[derive(Clone)]
pub struct InputSender {
    tx: mpsc::Sender<Input>,
    wake_w: RawFd,
    route: Route,
}

impl InputSender {
    pub fn send(&self, ev: Input) {
        match (&ev, self.route) {
            (Input::Touch(_) | Input::Raw { .. }, Route::DropTouch) => return,
            (Input::Touch(t), Route::DropFinger) if t.id != crate::touch::PEN_ID => return,
            (Input::Raw { .. }, Route::DropFinger) => return,
            (Input::Touch(t), Route::LogTouch) => {
                eprintln!("eink: direct {:?} id={} ({:.0},{:.0})", t.phase, t.id, t.x, t.y);
                return;
            }
            (Input::Raw { .. }, Route::LogTouch) => return,
            _ => {}
        }
        if self.tx.send(ev).is_ok() {
            let b = [1u8];
            // Non-blocking: a full pipe already means "wake up".
            unsafe { libc::write(self.wake_w, b.as_ptr().cast(), 1) };
        }
    }
}

impl InputHub {
    pub fn new() -> io::Result<InputHub> {
        let mut fds = [0 as RawFd; 2];
        wake_pipe(&mut fds)?;
        let (tx, rx) = mpsc::channel();
        Ok(InputHub { rx, tx, wake_r: fds[0], wake_w: fds[1] })
    }

    pub fn sender(&self) -> InputSender {
        self.sender_with(Route::Pass)
    }

    pub fn sender_with(&self, route: Route) -> InputSender {
        InputSender { tx: self.tx.clone(), wake_w: self.wake_w, route }
    }

    /// Sleep until input arrives or `timeout_ms` passes (-1 = forever).
    pub fn wait(&self, timeout_ms: i32) {
        let mut p = libc::pollfd { fd: self.wake_r, events: libc::POLLIN, revents: 0 };
        unsafe { libc::poll(&mut p, 1, timeout_ms) };
        let mut sink = [0u8; 64];
        while unsafe { libc::read(self.wake_r, sink.as_mut_ptr().cast(), sink.len()) } > 0 {}
    }

    pub fn drain(&self) -> Vec<Input> {
        self.rx.try_iter().collect()
    }
}

/// A non-blocking, close-on-exec pipe for the hub's wake-ups.
///
/// Every presenter uses the hub, the portable `mem` one included, so this must
/// work everywhere. Linux keeps `pipe2`, which sets both flags ATOMICALLY -- a
/// concurrent fork cannot inherit the descriptors in between. macOS has no
/// `pipe2`, so elsewhere it is POSIX `pipe` then `fcntl`.
#[cfg(target_os = "linux")]
fn wake_pipe(fds: &mut [RawFd; 2]) -> io::Result<()> {
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn wake_pipe(fds: &mut [RawFd; 2]) -> io::Result<()> {
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    for &fd in fds.iter() {
        let ok = unsafe {
            let fl = libc::fcntl(fd, libc::F_GETFL);
            fl >= 0
                && libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK) == 0
                && libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) == 0
        };
        if !ok {
            let e = io::Error::last_os_error();
            unsafe { libc::close(fds[0]); libc::close(fds[1]); }
            return Err(e);
        }
    }
    Ok(())
}

impl Drop for InputHub {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.wake_r);
            libc::close(self.wake_w);
        }
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;
    use crate::touch::{Phase, TouchEvent};

    fn touch() -> Input { Input::Touch(TouchEvent { id: 1, phase: Phase::Start, x: 10.0, y: 20.0 }) }

    #[test]
    fn a_route_decides_whether_touch_is_forwarded_and_never_blocks_close_or_wake() {
        let hub = InputHub::new().unwrap();
        for (route, forwards_touch) in [(Route::Pass, true), (Route::DropTouch, false), (Route::DropFinger, false), (Route::LogTouch, false)] {
            let s = hub.sender_with(route);
            s.send(touch());
            s.send(Input::Raw { x: 1, y: 2 });
            s.send(Input::Closed("gone".into()));
            s.send(Input::Wake);
            let got = hub.drain();
            assert_eq!(got.iter().any(|i| matches!(i, Input::Touch(_))), forwards_touch, "{route:?}: touch");
            assert_eq!(got.iter().any(|i| matches!(i, Input::Raw { .. })), forwards_touch, "{route:?}: raw coordinates follow touch");
            assert!(got.iter().any(|i| matches!(i, Input::Closed(_))), "{route:?}: a closed display must always reach the runner");
            assert!(got.iter().any(|i| matches!(i, Input::Wake)), "{route:?}: wake");
        }
    }

    #[test]
    fn dropping_the_finger_keeps_the_pen() {
        let hub = InputHub::new().unwrap();
        let s = hub.sender_with(Route::DropFinger);
        s.send(Input::Touch(TouchEvent { id: 3, phase: Phase::Start, x: 1.0, y: 1.0 }));
        s.send(Input::Touch(TouchEvent { id: crate::touch::PEN_ID, phase: Phase::Start, x: 2.0, y: 2.0 }));
        let got = hub.drain();
        let ids: Vec<u32> = got.iter().filter_map(|i| if let Input::Touch(t) = i { Some(t.id) } else { None }).collect();
        assert_eq!(ids, vec![crate::touch::PEN_ID], "the finger is dropped, the pen is not");
    }

    #[test]
    fn each_input_mode_decides_who_may_drive_the_app() {
        use Route::*;
        // the default and any typo: AppLoad alone, no direct readers
        for m in ["qtfb", "", "evdv", "EVDEV"] { assert_eq!(InputPlan::parse(m), InputPlan { qtfb: Pass, touch: None, pen: None }, "{m:?}"); }
        let p = InputPlan::parse("evdev");
        assert_eq!((p.qtfb, p.touch, p.pen), (DropFinger, Some(Pass), None), "finger direct, pen still AppLoad's, no direct pen");
        let p = InputPlan::parse("evdev-pen-compare");
        assert_eq!((p.qtfb, p.touch, p.pen), (DropFinger, Some(Pass), Some(LogTouch)), "the direct pen only logs");
        let p = InputPlan::parse("evdev-all");
        assert_eq!((p.qtfb, p.touch, p.pen), (DropTouch, Some(Pass), Some(Pass)));
        let p = InputPlan::parse("compare");
        assert_eq!((p.qtfb, p.touch, p.pen), (Pass, Some(LogTouch), Some(LogTouch)), "AppLoad drives, nothing direct is forwarded");
        // no mode may let BOTH AppLoad and a direct reader drive the same device: every tap would arrive twice
        for m in ["evdev", "evdev-pen-compare", "evdev-all", "compare"] {
            let p = InputPlan::parse(m);
            let appload_drives_finger = p.qtfb == Pass;
            let direct_drives_finger = p.touch == Some(Pass);
            assert!(!(appload_drives_finger && direct_drives_finger), "{m}: a finger would be delivered twice");
            let appload_drives_pen = matches!(p.qtfb, Pass | DropFinger);
            let direct_drives_pen = p.pen == Some(Pass);
            assert!(!(appload_drives_pen && direct_drives_pen), "{m}: the pen would be delivered twice");
        }
    }
}
