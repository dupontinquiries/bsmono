//! ffi.rs — the C surface for the Qt/C++ side. `lib.rs` declared this module
//! behind `feature = "ffi"` from the start; the file did not exist, so
//! enabling the feature failed to compile and nothing in the app could
//! reach this crate. This is that bridge.
//!
//! # Shape
//!
//! One opaque `ScNat` handle owns a current-thread tokio runtime and a
//! [`NatTraversal`]. Calls block on the runtime (Qt's event loop is the
//! caller's thread; a blocking STUN call of a few seconds is how the C++
//! `start()`+timer pair behaved from the caller's point of view, just
//! without the timer). Events queue on the handle and are drained with
//! [`sc_nat_poll_event`] — the C side polls from a `QTimer`, the way every
//! other `sc_*` FFI in this tree hands data back without callbacks into C++.
//!
//! Strings out are `sc_nat_str` (pointer + len, not NUL-terminated, owned by
//! the event until the next poll). Nothing here allocates on the C side.
//!
//! # Not cbindgen-generated yet
//!
//! The header for these is written by hand into `include/network/` when the
//! Qt side adopts it; the signatures are kept C-simple (ints, u16, byte
//! pointers) so that is mechanical.

use crate::traversal::{NatConfig, NatEvent, NatTraversal, PeerAddress};
use std::collections::VecDeque;
use std::ffi::CStr;
use std::net::{IpAddr, SocketAddr};
use std::os::raw::{c_char, c_int};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::mpsc;

/// Opaque handle.
pub struct ScNat {
    rt: Runtime,
    nat: NatTraversal,
    events: mpsc::Receiver<NatEvent>,
    /// Backing storage for the most recently polled event's strings, so the
    /// pointers handed to C stay valid until the next poll.
    last_id: String,
    last_ip: String,
    last_data: Vec<u8>,
    pending: VecDeque<NatEvent>,
}

/// Event kinds, matching [`NatEvent`] variants.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScNatEventKind {
    None = 0,
    ReflexiveAddress = 1,
    CgnatDetected = 2,
    StunFailed = 3,
    PunchSeen = 4,
    HolePunchReady = 5,
    HolePunchFailed = 6,
    ScudFrame = 7,
}

/// A borrowed, non-NUL-terminated string/bytes view. Valid until the next
/// `sc_nat_poll_event` on the same handle.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ScNatStr {
    pub ptr: *const u8,
    pub len: usize,
}

impl ScNatStr {
    fn empty() -> Self {
        ScNatStr { ptr: std::ptr::null(), len: 0 }
    }
    fn of(b: &[u8]) -> Self {
        ScNatStr { ptr: b.as_ptr(), len: b.len() }
    }
}

/// One polled event.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ScNatEvent {
    pub kind: ScNatEventKind,
    /// Peer sc-ID for punch events; the reason text for `StunFailed`.
    pub id: ScNatStr,
    /// Dotted IP for address-bearing events.
    pub ip: ScNatStr,
    pub port: u16,
    /// SCUD frame bytes for `ScudFrame`.
    pub data: ScNatStr,
}

fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller passes a NUL-terminated C string that outlives the call.
    unsafe { CStr::from_ptr(p) }.to_str().ok()
}

/// Create a handle: binds the punch socket (9876 → 9877 → OS pick) and
/// starts the receive loop. `self_id` is this device's sc-ID. Returns NULL
/// on failure.
#[no_mangle]
pub extern "C" fn sc_nat_new(self_id: *const c_char) -> *mut ScNat {
    let Some(id) = cstr(self_id) else { return std::ptr::null_mut() };
    let Ok(rt) = Builder::new_current_thread().enable_all().build() else { return std::ptr::null_mut() };
    match rt.block_on(NatTraversal::bind(NatConfig::default(), id)) {
        Ok((nat, events)) => Box::into_raw(Box::new(ScNat {
            rt,
            nat,
            events,
            last_id: String::new(),
            last_ip: String::new(),
            last_data: Vec::new(),
            pending: VecDeque::new(),
        })),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Destroy a handle.
#[no_mangle]
pub extern "C" fn sc_nat_free(h: *mut ScNat) {
    if !h.is_null() {
        // SAFETY: created by `sc_nat_new`, freed exactly once by the caller.
        drop(unsafe { Box::from_raw(h) });
    }
}

/// The local punch port actually bound.
#[no_mangle]
pub extern "C" fn sc_nat_local_port(h: *const ScNat) -> u16 {
    // SAFETY: valid handle from `sc_nat_new`.
    unsafe { h.as_ref() }.map(|n| n.nat.local_port()).unwrap_or(0)
}

/// Blocking STUN discovery (`start()`). Returns 1 on success (and queues a
/// `ReflexiveAddress` event), 0 on failure (and queues `StunFailed`).
#[no_mangle]
pub extern "C" fn sc_nat_discover(h: *mut ScNat) -> c_int {
    // SAFETY: valid handle.
    let Some(n) = (unsafe { h.as_mut() }) else { return 0 };
    let ok = n.rt.block_on(n.nat.discover()).is_ok();
    n.drain();
    c_int::from(ok)
}

/// `restartStunOnly()`: redo STUN without touching the socket or punches.
#[no_mangle]
pub extern "C" fn sc_nat_restart_stun_only(h: *mut ScNat) -> c_int {
    // SAFETY: valid handle.
    let Some(n) = (unsafe { h.as_mut() }) else { return 0 };
    let ok = n.rt.block_on(n.nat.restart_stun_only()).is_ok();
    n.drain();
    c_int::from(ok)
}

/// `initiateHolePunch()`. `ip` is a dotted/colon-form IP string. Returns 0
/// if the arguments could not be parsed; refusals (self-target etc.) are
/// reported as a `HolePunchFailed` event, not a return code, as in the C++.
#[no_mangle]
pub extern "C" fn sc_nat_punch(h: *mut ScNat, peer_id: *const c_char, ip: *const c_char, port: u16) -> c_int {
    // SAFETY: valid handle.
    let Some(n) = (unsafe { h.as_mut() }) else { return 0 };
    let (Some(peer), Some(ip_s)) = (cstr(peer_id), cstr(ip)) else { return 0 };
    let Ok(ip) = ip_s.parse::<IpAddr>() else { return 0 };
    n.rt.block_on(n.nat.initiate_hole_punch(peer, PeerAddress { ip, port }));
    n.drain();
    1
}

/// `stopHolePunch()`.
#[no_mangle]
pub extern "C" fn sc_nat_stop_punch(h: *mut ScNat, peer_id: *const c_char) {
    // SAFETY: valid handle.
    if let (Some(n), Some(peer)) = (unsafe { h.as_mut() }, cstr(peer_id)) {
        n.nat.stop_hole_punch(peer);
    }
}

/// `sendUdpData()`: raw bytes to a punched peer over the shared socket.
#[no_mangle]
pub extern "C" fn sc_nat_send_udp(h: *mut ScNat, ip: *const c_char, port: u16, data: *const u8, len: usize) -> c_int {
    // SAFETY: valid handle; `data` points to `len` readable bytes.
    let Some(n) = (unsafe { h.as_mut() }) else { return 0 };
    let Some(ip_s) = cstr(ip) else { return 0 };
    let Ok(ip) = ip_s.parse::<IpAddr>() else { return 0 };
    if data.is_null() && len != 0 {
        return 0;
    }
    let bytes: &[u8] = if len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(data, len) } };
    c_int::from(n.rt.block_on(n.nat.send_udp(SocketAddr::new(ip, port), bytes)).is_ok())
}

/// Pump the runtime briefly so background tasks (receive loop, punch
/// bursts, keepalives) make progress, then dequeue one event. Call from a
/// timer. Returns an event with `kind == None` when the queue is empty.
#[no_mangle]
pub extern "C" fn sc_nat_poll_event(h: *mut ScNat) -> ScNatEvent {
    let none = ScNatEvent { kind: ScNatEventKind::None, id: ScNatStr::empty(), ip: ScNatStr::empty(), port: 0, data: ScNatStr::empty() };
    // SAFETY: valid handle.
    let Some(n) = (unsafe { h.as_mut() }) else { return none };
    // Give spawned tasks a slice of time: a current-thread runtime only runs
    // them while something is blocking on it. The sleep is created INSIDE
    // the async block — `tokio::time::sleep` needs the runtime's timer at
    // construction, not just at poll, so building it as `block_on`'s
    // argument panics with "no reactor running".
    n.rt.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(1)).await });
    n.drain();
    let Some(ev) = n.pending.pop_front() else { return none };
    n.last_id.clear();
    n.last_ip.clear();
    n.last_data.clear();
    let (kind, port) = match &ev {
        NatEvent::ReflexiveAddress { ip, port } => { n.last_ip = ip.to_string(); (ScNatEventKind::ReflexiveAddress, *port) }
        NatEvent::CgnatDetected { ip } => { n.last_ip = ip.to_string(); (ScNatEventKind::CgnatDetected, 0) }
        NatEvent::StunFailed { reason } => { n.last_id = reason.clone(); (ScNatEventKind::StunFailed, 0) }
        NatEvent::PunchSeen { peer_id, addr } => { n.last_id = peer_id.clone(); n.last_ip = addr.ip.to_string(); (ScNatEventKind::PunchSeen, addr.port) }
        NatEvent::HolePunchReady { peer_id, addr } => { n.last_id = peer_id.clone(); n.last_ip = addr.ip.to_string(); (ScNatEventKind::HolePunchReady, addr.port) }
        NatEvent::HolePunchFailed { peer_id } => { n.last_id = peer_id.clone(); (ScNatEventKind::HolePunchFailed, 0) }
        NatEvent::ScudFrame { data, from } => { n.last_ip = from.ip().to_string(); n.last_data = data.clone(); (ScNatEventKind::ScudFrame, from.port()) }
    };
    ScNatEvent {
        kind,
        id: ScNatStr::of(n.last_id.as_bytes()),
        ip: ScNatStr::of(n.last_ip.as_bytes()),
        port,
        data: ScNatStr::of(&n.last_data),
    }
}

impl ScNat {
    /// Move everything currently in the event channel into `pending`.
    fn drain(&mut self) {
        while let Ok(ev) = self.events.try_recv() {
            self.pending.push_back(ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn handle_round_trip_and_self_punch_refusal_through_the_c_surface() {
        let id = CString::new("sc-alice").unwrap();
        let h = sc_nat_new(id.as_ptr());
        assert!(!h.is_null(), "bind failed");
        assert_ne!(sc_nat_local_port(h), 0);

        let peer = CString::new("sc-alice").unwrap(); // our own id
        let ip = CString::new("10.0.0.2").unwrap();
        assert_eq!(sc_nat_punch(h, peer.as_ptr(), ip.as_ptr(), 9876), 1);
        let ev = sc_nat_poll_event(h);
        assert_eq!(ev.kind, ScNatEventKind::HolePunchFailed);
        let got = unsafe { std::slice::from_raw_parts(ev.id.ptr, ev.id.len) };
        assert_eq!(got, b"sc-alice");

        assert_eq!(sc_nat_poll_event(h).kind, ScNatEventKind::None);
        sc_nat_free(h);
    }

    #[test]
    fn null_and_garbage_inputs_are_rejected_not_dereferenced() {
        assert!(sc_nat_new(std::ptr::null()).is_null());
        assert_eq!(sc_nat_local_port(std::ptr::null()), 0);
        assert_eq!(sc_nat_discover(std::ptr::null_mut()), 0);
        assert_eq!(sc_nat_poll_event(std::ptr::null_mut()).kind, ScNatEventKind::None);
        sc_nat_free(std::ptr::null_mut());
    }
}
