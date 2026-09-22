//! The trackpad's output: relative pointer, buttons and scroll sent to
//! cosmic-comp over libei (`reis`), the same path cosmic-osk uses.
//!
//! Connection: `LIBEI_SOCKET` if set, else cosmic-comp's
//! `com.system76.CosmicComp.Ei.GetSenderSocket` on the session bus. cosmic-comp
//! only answers callers that own an allowlisted name, so this owns
//! `org.tideshell.Shell` first — which needs cosmic-comp patch
//! `0002-dbus-ei-allow-tideshell`. Without it the call fails with
//! AccessDenied, and that text is what [`Status::Failed`] shows.
//!
//! A thread owns the EI connection; the UI sends [`PointerOut`]s over a
//! channel and reads [`Status`]. Output sent before the device is resumed is
//! dropped (a stale motion replayed later would jump the pointer).

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reis::ei;
use reis::event::{DeviceCapability, EiEvent, EiEventConverter};
use tideshell_core::trackpad::{Button, PointerOut};

pub const BUS_NAME: &str = "org.tideshell.Shell";
const DEVICE_TYPE_KEYBOARD: u32 = 1;
const DEVICE_TYPE_POINTER: u32 = 2;
/// evdev KEY_LEFTCTRL: held during a pinch (EI has no gesture interface, so
/// pinch zoom goes out as Ctrl + wheel, the zoom convention of apps).
pub const KEY_LEFTCTRL: u32 = 29;
/// One wheel notch in EI's discrete scroll units.
pub const NOTCH: i32 = 120;
/// linux/input-event-codes.h
pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Connecting,
    /// Connected, waiting for the compositor to resume the device.
    Waiting,
    Ready,
    Failed(String),
}

/// One EI operation, before it touches a socket.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EiOp {
    Motion(f32, f32),
    Button(u32, bool),
    Scroll(f32, f32),
    /// Absolute position in the compositor's logical space (fat cursor).
    Absolute(f32, f32),
    Key(u32, bool),
    /// Discrete wheel scroll in 1/120 notches (EI scroll_discrete).
    ScrollDiscrete(i32, i32),
}

/// Pure mapping. Scroll is inverted: the trackpad model reports finger
/// motion ("natural": content follows the fingers), while EI scroll deltas
/// are in scroll direction (positive y = scroll down = content moves up).
pub fn to_op(p: PointerOut) -> EiOp {
    match p {
        PointerOut::Motion { dx, dy } => EiOp::Motion(dx, dy),
        PointerOut::Button { button, pressed } => EiOp::Button(
            match button {
                Button::Left => BTN_LEFT,
                Button::Right => BTN_RIGHT,
            },
            pressed,
        ),
        PointerOut::Scroll { dx, dy } => EiOp::Scroll(-dx, -dy),
        PointerOut::Absolute { x, y } => EiOp::Absolute(x, y),
        PointerOut::ZoomBegin => EiOp::Key(KEY_LEFTCTRL, true),
        // Zoom in (fingers apart) = Ctrl + wheel up (negative y).
        PointerOut::Zoom { steps } => EiOp::ScrollDiscrete(0, -NOTCH * steps),
        PointerOut::ZoomEnd => EiOp::Key(KEY_LEFTCTRL, false),
    }
}

pub struct EiPointer {
    tx: Sender<PointerOut>,
    status: Arc<Mutex<Status>>,
}

impl EiPointer {
    pub fn spawn() -> EiPointer {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(Status::Connecting));
        let st = status.clone();
        std::thread::Builder::new()
            .name("tideshell-ei".into())
            .spawn(move || {
                if let Err(e) = run(rx, &st) {
                    *st.lock().unwrap() = Status::Failed(e);
                }
            })
            .expect("spawn ei thread");
        EiPointer { tx, status }
    }

    pub fn send(&self, p: PointerOut) {
        let _ = self.tx.send(p);
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
}

fn socket_from_cosmic_comp() -> Result<UnixStream, String> {
    let bus = zbus::blocking::Connection::session().map_err(|e| format!("session bus: {e}"))?;
    bus.request_name(BUS_NAME).map_err(|e| format!("own {BUS_NAME}: {e}"))?;
    let reply = bus
        .call_method(
            Some("com.system76.CosmicComp"),
            "/com/system76/CosmicComp/Ei",
            Some("com.system76.CosmicComp.Ei"),
            "GetSenderSocket",
            &(DEVICE_TYPE_POINTER | DEVICE_TYPE_KEYBOARD,),
        )
        .map_err(|e| format!("GetSenderSocket: {e}"))?;
    let fd: zbus::zvariant::OwnedFd = reply.body().deserialize().map_err(|e| format!("reply: {e}"))?;
    // Keep the bus connection alive for the process: dropping it releases
    // the name, and cosmic-comp may re-check ownership later.
    std::mem::forget(bus);
    Ok(UnixStream::from(OwnedFd::from(fd)))
}

struct Dev {
    device: ei::Device,
    pointer: Option<ei::Pointer>,
    button: Option<ei::Button>,
    scroll: Option<ei::Scroll>,
    absolute: Option<ei::PointerAbsolute>,
    keyboard: Option<ei::Keyboard>,
    resumed: bool,
}

fn run(rx: Receiver<PointerOut>, status: &Mutex<Status>) -> Result<(), String> {
    let ctx = match ei::Context::connect_to_env().map_err(|e| e.to_string())? {
        Some(c) => c,
        None => ei::Context::new(socket_from_cosmic_comp()?).map_err(|e| e.to_string())?,
    };
    let resp = reis::handshake::ei_handshake_blocking(&ctx, "tideshell", ei::handshake::ContextType::Sender)
        .map_err(|e| format!("handshake: {e:?}"))?;
    let mut conv = EiEventConverter::new(&ctx, resp);
    *status.lock().unwrap() = Status::Waiting;
    let mut devs: Vec<Dev> = Vec::new();
    let mut sequence = 1u32;
    let t0 = Instant::now();

    loop {
        // 1. Protocol traffic.
        // reis folds WouldBlock into Ok(0) ("nothing to read") and reports
        // EOF as an UnexpectedEof error (wire/backend.rs). Treating Ok(0) as
        // EOF quit on the first idle poll; seen live against cosmic-comp.
        match ctx.read() {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err("compositor closed the EI connection".into()),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(format!("read: {e}")),
        }
        while let Some(r) = ctx.pending_event() {
            match r {
                reis::PendingRequestResult::Request(ev) => conv.handle_event(ev).map_err(|e| format!("{e:?}"))?,
                reis::PendingRequestResult::ParseError(e) => return Err(format!("parse: {e}")),
                reis::PendingRequestResult::InvalidObject(_) => {}
            }
        }
        while let Some(ev) = conv.next_event() {
            match ev {
                EiEvent::SeatAdded(s) => {
                    s.seat.bind_capabilities(DeviceCapability::Pointer | DeviceCapability::PointerAbsolute | DeviceCapability::Button | DeviceCapability::Scroll | DeviceCapability::Keyboard);
                }
                EiEvent::DeviceAdded(d) => {
                    let dev = &d.device;
                    if [DeviceCapability::Pointer, DeviceCapability::PointerAbsolute, DeviceCapability::Button, DeviceCapability::Scroll, DeviceCapability::Keyboard].iter().any(|c| dev.has_capability(*c)) {
                        devs.push(Dev {
                            device: dev.device().clone(),
                            pointer: dev.interface::<ei::Pointer>(),
                            button: dev.interface::<ei::Button>(),
                            scroll: dev.interface::<ei::Scroll>(),
                            absolute: dev.interface::<ei::PointerAbsolute>(),
                            keyboard: dev.interface::<ei::Keyboard>(),
                            resumed: false,
                        });
                    }
                }
                EiEvent::DeviceResumed(r) => {
                    if let Some(d) = devs.iter_mut().find(|d| &d.device == r.device.device()) {
                        // Protocol order: (last_serial, sequence).
                        d.device.start_emulating(r.serial, sequence);
                        sequence += 1;
                        d.resumed = true;
                    }
                }
                EiEvent::DevicePaused(p) => {
                    if let Some(d) = devs.iter_mut().find(|d| &d.device == p.device.device()) {
                        d.resumed = false;
                    }
                }
                EiEvent::DeviceRemoved(r) => devs.retain(|d| &d.device != r.device.device()),
                EiEvent::Disconnected(d) => return Err(format!("disconnected: {:?}", d.reason)),
                _ => {}
            }
        }
        let ready = devs.iter().any(|d| d.resumed);
        {
            let mut st = status.lock().unwrap();
            let want = if ready { Status::Ready } else { Status::Waiting };
            if *st != want {
                *st = want;
            }
        }

        // 2. Output from the UI. Block briefly so the loop is not a spin.
        let first = match rx.recv_timeout(Duration::from_millis(8)) {
            Ok(p) => Some(p),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        let outs: Vec<PointerOut> = first.into_iter().chain(rx.try_iter()).collect();
        if outs.is_empty() || !ready {
            let _ = conv.connection().flush();
            continue;
        }
        let serial = conv.connection().serial();
        let now_us = t0.elapsed().as_micros() as u64;
        // One frame per operation, so a tap's press and release carry
        // distinct timestamps instead of forming one zero-length frame.
        // Measured against cosmic-comp epoch-1.8.0: batching all of them in
        // one frame ALSO works there (clicks land and focus the window), so
        // this is robustness toward other EIS servers and toolkits, not a
        // cosmic-comp requirement. (An earlier comment here claimed batching
        // broke clicks; that was a race in the self-test, see tideshell-sim.)
        for (k, p) in outs.into_iter().enumerate() {
            let (i, done) = match to_op(p) {
                EiOp::Motion(x, y) => pick(&devs, |d| d.pointer.as_ref().map(|i| i.motion_relative(x, y))),
                EiOp::Button(b, down) => pick(&devs, |d| {
                    d.button.as_ref().map(|i| {
                        i.button(b, if down { ei::button::ButtonState::Press } else { ei::button::ButtonState::Released })
                    })
                }),
                EiOp::Scroll(x, y) => pick(&devs, |d| d.scroll.as_ref().map(|i| i.scroll(x, y))),
                EiOp::Absolute(x, y) => pick(&devs, |d| d.absolute.as_ref().map(|i| i.motion_absolute(x, y))),
                EiOp::Key(code, down) => pick(&devs, |d| {
                    d.keyboard.as_ref().map(|i| i.key(code, if down { ei::keyboard::KeyState::Press } else { ei::keyboard::KeyState::Released }))
                }),
                EiOp::ScrollDiscrete(x, y) => pick(&devs, |d| d.scroll.as_ref().map(|i| i.scroll_discrete(x, y))),
            };
            if done {
                devs[i].device.frame(serial, now_us + k as u64);
            }
        }
        let _ = conv.connection().flush();
    }
}

/// Run `f` on the first resumed device that has the needed interface.
fn pick(devs: &[Dev], f: impl Fn(&Dev) -> Option<()>) -> (usize, bool) {
    for (i, d) in devs.iter().enumerate().filter(|(_, d)| d.resumed) {
        if f(d).is_some() {
            return (i, true);
        }
    }
    (0, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_map_to_evdev_codes() {
        assert_eq!(to_op(PointerOut::Button { button: Button::Left, pressed: true }), EiOp::Button(BTN_LEFT, true));
        assert_eq!(to_op(PointerOut::Button { button: Button::Right, pressed: false }), EiOp::Button(BTN_RIGHT, false));
    }

    #[test]
    fn motion_passes_through_and_scroll_is_natural() {
        assert_eq!(to_op(PointerOut::Motion { dx: 3.0, dy: -2.0 }), EiOp::Motion(3.0, -2.0));
        // Fingers move down (dy > 0): content follows, i.e. scroll UP (EI y < 0).
        assert_eq!(to_op(PointerOut::Scroll { dx: 0.0, dy: 10.0 }), EiOp::Scroll(-0.0, -10.0));
        assert_eq!(to_op(PointerOut::Absolute { x: 12.5, y: 40.0 }), EiOp::Absolute(12.5, 40.0), "absolute is not inverted or scaled");
    }

    #[test]
    fn pinch_maps_to_ctrl_and_wheel_notches() {
        assert_eq!(to_op(PointerOut::ZoomBegin), EiOp::Key(KEY_LEFTCTRL, true));
        assert_eq!(to_op(PointerOut::Zoom { steps: 2 }), EiOp::ScrollDiscrete(0, -240), "zoom in = wheel up");
        assert_eq!(to_op(PointerOut::Zoom { steps: -1 }), EiOp::ScrollDiscrete(0, 120));
        assert_eq!(to_op(PointerOut::ZoomEnd), EiOp::Key(KEY_LEFTCTRL, false));
    }
}
