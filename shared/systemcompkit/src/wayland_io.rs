//! wayland_io — the one part of `systemcompkit` that touches a real Wayland
//! socket, and, matching `bt-kit`'s own `rfcomm_io.rs` and `bt_send.rs`'s
//! reasoning for the same choice, kept as thin as possible on purpose:
//! no protocol-interpretation logic lives here that isn't already tested in
//! `idle`/`toplevel` — this file's only job is turning real wayland-client
//! events into calls on those pure trackers.
//!
//! Currently: `ext-idle-notify-v1` only, as the first proof the
//! `wayland-client`/`wayland-protocols` dependency pair actually works for
//! this crate's shape before building out layer-shell and the toplevel-list
//! wiring on top of the same pattern. `toplevel`'s own header names exactly
//! which protocol that wiring targets when it's built (`ext-foreign-
//! toplevel-list-v1` + `zcosmic_toplevel_manager_v1`, not `wlr-foreign-
//! toplevel-management`) — not done in this file yet.
//!
//! ## Verification ceiling
//!
//! Same honest limit as everything else touching real hardware/a real
//! compositor this session: there is no Wayland compositor in a build
//! sandbox to connect to. This type-checks against the real
//! `wayland-client` 0.31 / `wayland-protocols` 0.32 API (confirmed via that
//! crate's own doc examples and generated-protocol XML, not guessed) — that
//! is the extent of what's confirmed here. The actual `idled`/`resumed`
//! round trip against `cosmic-comp` is not verified.

use crate::idle::{IdleEvent, IdleTracker};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};

/// App state for the idle-notify connection. The threshold name is carried
/// as each `ExtIdleNotificationV1`'s own user-data (see `register_threshold`
/// below) — the simplest way to know WHICH threshold an `idled`/`resumed`
/// event is for for, with no side table to keep in sync.
struct IdleIoState {
    tracker: IdleTracker,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for IdleIoState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Dynamic add/remove of globals after startup isn't handled — this
        // connection is short-lived per session, not expected to outlive a
        // compositor restart. A longer-lived shell process would need this
        // filled in; recorded rather than silently assumed away.
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for IdleIoState {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // The notifier global is a factory only (`get_idle_notification`) —
        // its generated `Event` type exists per wayland-scanner's usual
        // shape but the protocol XML defines no actual events for it.
    }
}

impl Dispatch<WlSeat, ()> for IdleIoState {
    fn event(_: &mut Self, _: &WlSeat, _: wayland_client::protocol::wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // Seat capability/name events aren't needed for this — the seat
        // object exists here only to be handed to get_idle_notification.
    }
}

impl Dispatch<ExtIdleNotificationV1, String> for IdleIoState {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        threshold_name: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mapped = match event {
            ext_idle_notification_v1::Event::Idled => Some(IdleEvent::Idled),
            ext_idle_notification_v1::Event::Resumed => Some(IdleEvent::Resumed),
            _ => None, // the protocol is version 2; an unknown future event is ignored, not a panic
        };
        if let Some(ev) = mapped {
            state.tracker.apply(threshold_name, ev);
        }
    }
}

pub struct IdleSession {
    conn: Connection,
    queue: EventQueue<IdleIoState>,
    state: IdleIoState,
    notifier: ExtIdleNotifierV1,
    seat: WlSeat,
}

#[derive(Debug)]
pub enum ConnectError {
    NoDisplay(String),
    MissingGlobal(&'static str),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::NoDisplay(e) => write!(f, "no wayland display: {e}"),
            ConnectError::MissingGlobal(name) => write!(f, "compositor does not advertise {name}"),
        }
    }
}

impl IdleSession {
    /// Connects and binds `ext_idle_notifier_v1` + a `wl_seat`. Fails
    /// plainly (`MissingGlobal`) rather than partially working if the
    /// compositor doesn't advertise idle-notify at all — a shell falling
    /// back to "no idle tracking" should decide that explicitly at the call
    /// site, not discover it later as notifications that silently never
    /// fire.
    pub fn connect() -> Result<Self, ConnectError> {
        let conn = Connection::connect_to_env().map_err(|e| ConnectError::NoDisplay(e.to_string()))?;
        let (globals, queue) = registry_queue_init::<IdleIoState>(&conn)
            .map_err(|e| ConnectError::NoDisplay(e.to_string()))?;
        let qh = queue.handle();

        let notifier: ExtIdleNotifierV1 = globals
            .bind(&qh, 1..=2, ())
            .map_err(|_| ConnectError::MissingGlobal("ext_idle_notifier_v1"))?;
        let seat: WlSeat =
            globals.bind(&qh, 1..=1, ()).map_err(|_| ConnectError::MissingGlobal("wl_seat"))?;

        Ok(IdleSession { conn, queue, state: IdleIoState { tracker: IdleTracker::new() }, notifier, seat })
    }

    /// Registers a new named threshold with the compositor. `timeout_ms` is
    /// the idle duration per the protocol's own semantics: no user activity
    /// for at least this long fires `Idled`. Multiple thresholds (dim, then
    /// lock, then suspend, the same layered shape `cosmic-idle` itself
    /// uses) are independent `ExtIdleNotificationV1` objects, one per call.
    pub fn register_threshold(&mut self, name: &str, timeout_ms: u32) {
        let qh = self.queue.handle();
        self.notifier.get_idle_notification(timeout_ms, &self.seat, &qh, name.to_string());
        self.state.tracker.register(name);
    }

    /// Non-blocking: processes whatever events are already buffered, then
    /// returns. A caller on an egui-style redraw loop calls this once per
    /// frame rather than blocking the render thread on the next idle event,
    /// which might be seconds or hours away.
    pub fn poll(&mut self) -> &IdleTracker {
        let _ = self.queue.dispatch_pending(&mut self.state);
        &self.state.tracker
    }

    /// Flushes any pending requests (e.g. `register_threshold` calls made
    /// since the last flush) to the compositor. `wayland-client` buffers
    /// requests locally until flushed; forgetting this is a common way for
    /// a wayland-client app to silently do nothing.
    pub fn flush(&self) -> Result<(), String> {
        self.conn.flush().map_err(|e| e.to_string())
    }
}
