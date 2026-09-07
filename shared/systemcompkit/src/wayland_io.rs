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
use crate::layer::{KeyboardInteractivity, Layer, LayerEvent, LayerSurfaceIntent, LayerSurfaceState};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_compositor::WlCompositor, wl_registry, wl_seat::WlSeat, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1},
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

struct LayerIoState {
    state: LayerSurfaceState,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for LayerIoState {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlCompositor, ()> for LayerIoState {
    fn event(_: &mut Self, _: &WlCompositor, _: wayland_client::protocol::wl_compositor::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // wl_compositor has no events — a factory for surfaces/regions only.
    }
}

impl Dispatch<WlSurface, ()> for LayerIoState {
    fn event(_: &mut Self, _: &WlSurface, _: wayland_client::protocol::wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // `enter`/`leave` (which output the surface is on) aren't tracked
        // here — out of scope for "get a surface configured and on
        // screen", which is as far as this crate's job extends. A caller
        // that needs per-output scale/placement awareness handles that
        // itself, same boundary `hatch`'s adaptive.rs work drew between
        // "the kit provides the primitive" and "the app decides policy".
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for LayerIoState {
    fn event(_: &mut Self, _: &ZwlrLayerShellV1, _: zwlr_layer_shell_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // The shell global is a factory only (`get_layer_surface`) — no events.
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for LayerIoState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mapped = match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                Some(LayerEvent::Configure { serial, width, height })
            }
            zwlr_layer_surface_v1::Event::Closed => Some(LayerEvent::Closed),
            _ => None,
        };
        if let Some(ev) = mapped {
            // Acked HERE, immediately, rather than handed back to the
            // caller to remember — `layer.rs`'s own header explains why
            // this is the one thing this protocol punishes silently
            // (surface never quite appears, no error) rather than loudly.
            // A caller that wants to know a configure happened (to resize
            // its own render target, say) still can — `state.state` is
            // readable via `LayerSurface::configured_size()` below.
            if let Some(serial) = state.state.apply(ev) {
                proxy.ack_configure(serial);
            }
        }
    }
}

/// A layer-shell surface, connected and live. Rendering/buffer-attach is
/// deliberately NOT this crate's job — `wl_surface`, `wl_buffer`, and
/// actually drawing something belong to whatever egui/uikit backend sits
/// on top; this hands back the raw `WlSurface` so that backend can attach
/// to it, and otherwise only handles the layer-shell protocol's own
/// configure/ack/closed bookkeeping.
pub struct LayerSurface {
    conn: Connection,
    queue: EventQueue<LayerIoState>,
    io_state: LayerIoState,
    surface: WlSurface,
    layer_surface: ZwlrLayerSurfaceV1,
}

impl LayerSurface {
    /// Connects, binds `wl_compositor` + `zwlr_layer_shell_v1`, creates a
    /// surface, and requests it per `intent`. The surface is not yet
    /// configured when this returns — call `poll()` at least once (and
    /// generally every frame) before assuming `configured_size()` is
    /// `Some`; a compositor sends the first `configure` asynchronously,
    /// not as part of surface creation itself.
    pub fn connect(intent: LayerSurfaceIntent, namespace: &str) -> Result<Self, ConnectError> {
        let conn = Connection::connect_to_env().map_err(|e| ConnectError::NoDisplay(e.to_string()))?;
        let (globals, queue) = registry_queue_init::<LayerIoState>(&conn)
            .map_err(|e| ConnectError::NoDisplay(e.to_string()))?;
        let qh = queue.handle();

        let compositor: WlCompositor =
            globals.bind(&qh, 1..=6, ()).map_err(|_| ConnectError::MissingGlobal("wl_compositor"))?;
        let shell: ZwlrLayerShellV1 =
            globals.bind(&qh, 1..=4, ()).map_err(|_| ConnectError::MissingGlobal("zwlr_layer_shell_v1"))?;

        let surface = compositor.create_surface(&qh, ());
        let wire_layer = match intent.layer {
            Layer::Background => zwlr_layer_shell_v1::Layer::Background,
            Layer::Bottom => zwlr_layer_shell_v1::Layer::Bottom,
            Layer::Top => zwlr_layer_shell_v1::Layer::Top,
            Layer::Overlay => zwlr_layer_shell_v1::Layer::Overlay,
        };
        let layer_surface = shell.get_layer_surface(&surface, None, wire_layer, namespace.to_string(), &qh, ());

        let mut wire_anchor = zwlr_layer_surface_v1::Anchor::empty();
        if intent.anchor.top {
            wire_anchor |= zwlr_layer_surface_v1::Anchor::Top;
        }
        if intent.anchor.bottom {
            wire_anchor |= zwlr_layer_surface_v1::Anchor::Bottom;
        }
        if intent.anchor.left {
            wire_anchor |= zwlr_layer_surface_v1::Anchor::Left;
        }
        if intent.anchor.right {
            wire_anchor |= zwlr_layer_surface_v1::Anchor::Right;
        }
        layer_surface.set_anchor(wire_anchor);
        layer_surface.set_size(intent.size.0, intent.size.1);
        layer_surface.set_exclusive_zone(intent.exclusive_zone);
        layer_surface.set_keyboard_interactivity(match intent.keyboard_interactivity {
            KeyboardInteractivity::None => zwlr_layer_surface_v1::KeyboardInteractivity::None,
            KeyboardInteractivity::Exclusive => zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
            KeyboardInteractivity::OnDemand => zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand,
        });
        // The initial commit with no buffer attached is what actually asks
        // the compositor to configure this surface — set_* requests alone
        // are buffered client-side until a commit sends them.
        surface.commit();
        conn.flush().map_err(|e| ConnectError::NoDisplay(e.to_string()))?;

        Ok(LayerSurface {
            conn,
            queue,
            io_state: LayerIoState { state: LayerSurfaceState::new() },
            surface,
            layer_surface,
        })
    }

    /// Non-blocking, same contract as `IdleSession::poll` — call once per
    /// frame rather than blocking on the compositor's next event.
    pub fn poll(&mut self) -> LayerSurfaceState {
        let _ = self.queue.dispatch_pending(&mut self.io_state);
        self.io_state.state
    }

    pub fn configured_size(&self) -> Option<(u32, u32)> {
        self.io_state.state.configured_size
    }

    pub fn is_closed(&self) -> bool {
        self.io_state.state.closed
    }

    /// The raw surface, for whatever rendering backend attaches buffers to
    /// it. Deliberately not wrapped further — see this struct's own header.
    pub fn wl_surface(&self) -> &WlSurface {
        &self.surface
    }

    pub fn flush(&self) -> Result<(), String> {
        self.conn.flush().map_err(|e| e.to_string())
    }
}

impl Drop for LayerSurface {
    fn drop(&mut self) {
        // Explicit destroy rather than relying on the connection closing:
        // per the protocol's own description, a well-behaved client
        // destroys the layer surface before the underlying wl_surface, and
        // relying on socket teardown to do this implicitly risks a
        // compositor briefly seeing a surface with no role before it
        // notices the connection is gone.
        self.layer_surface.destroy();
        self.surface.destroy();
        let _ = self.conn.flush();
    }
}
