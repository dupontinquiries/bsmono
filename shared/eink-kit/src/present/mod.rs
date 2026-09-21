//! Output backends and the input plumbing they share.
//!
//! [`Present`] is the seam that varies per device: who owns the panel. It is
//! output only. Input runs on threads (a blocking read per source) and is
//! handed to the runner through an [`InputHub`], whose wake pipe lets the
//! runner sleep in `poll()` with zero CPU until a finger lands or a timer
//! is due. That is what keeps an idle app from draining the battery.

pub mod mem;
pub mod qtfb;
pub mod blight;
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

/// Sending end, cloned into each input thread.
#[derive(Clone)]
pub struct InputSender {
    tx: mpsc::Sender<Input>,
    wake_w: RawFd,
}

impl InputSender {
    pub fn send(&self, ev: Input) {
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
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let (tx, rx) = mpsc::channel();
        Ok(InputHub { rx, tx, wake_r: fds[0], wake_w: fds[1] })
    }

    pub fn sender(&self) -> InputSender {
        InputSender { tx: self.tx.clone(), wake_w: self.wake_w }
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

impl Drop for InputHub {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.wake_r);
            libc::close(self.wake_w);
        }
    }
}
