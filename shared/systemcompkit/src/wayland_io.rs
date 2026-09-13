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

use crate::capture::{CaptureEvent, CaptureSession};
use crate::idle::{IdleEvent, IdleTracker};
use crate::layer::{KeyboardInteractivity, Layer, LayerEvent, LayerSurfaceIntent, LayerSurfaceState};
use crate::toplevel::{ToplevelEvent, ToplevelTracker};
use cosmic_protocols::toplevel_info::v1::client::{
    zcosmic_toplevel_handle_v1::{self, ZcosmicToplevelHandleV1},
    zcosmic_toplevel_info_v1::{self, ZcosmicToplevelInfoV1},
};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_compositor::WlCompositor, wl_output::WlOutput, wl_registry, wl_seat::WlSeat, wl_surface::WlSurface,
};
use wayland_client::{event_created_child, Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_manager_v1::{self, ExtImageCopyCaptureManagerV1},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
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

    /// Requests a new size and exclusive zone on this already-mapped
    /// surface — the mechanism `Placement`'s collapsed/expanded transition
    /// needs, closing the gap `layer.rs`'s `top_panel`/`side_panel` docs
    /// call out. Protocol-legal at any time on `zwlr_layer_surface_v1`:
    /// `set_size`/`set_exclusive_zone` are plain requests, no
    /// destroy/recreate round trip. The compositor answers with a fresh
    /// `configure` event through the SAME `Dispatch` impl that handles the
    /// first one — nothing new to wire there — so `poll()`'s
    /// `configured_size()` reflects the new size once that event arrives.
    /// Anchor is deliberately not a parameter here: changing which edge a
    /// mapped surface is anchored to mid-session is a different, odder
    /// operation (effectively "become a different bar") that no caller in
    /// this monorepo needs yet — `Placement` picks anchor once, at
    /// `connect()` time, via `layer_intent()`.
    ///
    /// Not exercised against a real compositor — see this crate's own
    /// `STATUS.md` for why nothing here is. It type-checks against the
    /// real `zwlr_layer_surface_v1` request signatures and follows the
    /// same set-then-commit-then-flush shape `connect()` already uses for
    /// the initial configure; that is the honest ceiling until something
    /// runs it on hardware.
    pub fn request_resize(&mut self, size: (u32, u32), exclusive_zone: i32) -> Result<(), String> {
        self.layer_surface.set_size(size.0, size.1);
        self.layer_surface.set_exclusive_zone(exclusive_zone);
        self.surface.commit();
        self.flush()
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

struct TopIoState {
    tracker: ToplevelTracker,
    info: Option<ZcosmicToplevelInfoV1>,
    next_id: u64,
    // Retained specifically for capture: `zcosmic_toplevel_handle_v1`'s own
    // events (title/app_id/state/closed) are enough for ToplevelTracker,
    // but requesting a capture source needs the ORIGINAL
    // ext_foreign_toplevel_handle_v1 proxy itself, per
    // ext_foreign_toplevel_image_capture_source_manager_v1's own
    // create_source signature. Without this map, capture_toplevel below
    // would have nothing to hand it.
    handles: std::collections::HashMap<u64, ExtForeignToplevelHandleV1>,
    capture_source_mgr: Option<ExtForeignToplevelImageCaptureSourceManagerV1>,
    copy_capture_mgr: Option<ExtImageCopyCaptureManagerV1>,
    captures: std::collections::HashMap<u64, CaptureSession>,
    // Retained specifically to destroy explicitly on Closed, not for any
    // event dispatch of their own (both are already inert Dispatch impls
    // below) — dropping a wayland-client proxy does NOT send a wire
    // `destroy` request (confirmed: no Drop impl on generated proxy types
    // in wayland-backend), so without this a capture source/session for
    // every toplevel ever captured would leak server-side for the whole
    // life of a long-running shell process, not just until the toplevel closes.
    capture_sources: std::collections::HashMap<u64, ExtImageCaptureSourceV1>,
    copy_sessions: std::collections::HashMap<u64, ExtImageCopyCaptureSessionV1>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for TopIoState {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtForeignToplevelListV1, ()> for TopIoState {
    fn event(
        state: &mut Self,
        _: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            // The base handle is enough to KNOW a toplevel exists, but
            // title/app_id/state all come from the COSMIC extension object
            // (confirmed from the protocol XML: zcosmic_toplevel_handle_v1
            // carries its own title/app_id/state/closed events directly,
            // not delegated back to the base handle) -- so every new
            // toplevel immediately gets one, right here, rather than
            // waiting for something else to request it.
            let id = state.next_id;
            state.next_id += 1;
            if let Some(info) = &state.info {
                info.get_cosmic_toplevel(&toplevel, qh, id);
            }
            state.handles.insert(id, toplevel);
            state.tracker.apply(ToplevelEvent::New(id));
        }
    }

    event_created_child!(TopIoState, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for TopIoState {
    fn event(_: &mut Self, _: &ExtForeignToplevelHandleV1, _: ext_foreign_toplevel_handle_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // Deliberately a no-op: this tracker's whole model is sourced from
        // the zcosmic extension handle instead (see the Toplevel event
        // handler above) since that's the one authoritative source for
        // title/app_id/state/closed once obtained. Listening to the base
        // handle's OWN copies of the same events would mean two paths
        // that could, in principle, disagree about a single toplevel's
        // state -- avoided by picking one source of truth rather than
        // reconciling two.
    }
}

impl Dispatch<ZcosmicToplevelInfoV1, ()> for TopIoState {
    fn event(_: &mut Self, _: &ZcosmicToplevelInfoV1, _: zcosmic_toplevel_info_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // `done` (batch-commit signal) and the deprecated-since-v2
        // `toplevel` event both land here; neither is acted on -- this
        // tracker applies each property change as it arrives rather than
        // buffering until `done`, a real simplification from the
        // protocol's own double-buffering intent, recorded rather than
        // silently assumed correct. `ToplevelTracker`'s own field-by-field
        // `apply` already tolerates events arriving one at a time, so this
        // doesn't produce wrong data -- callers just may see a toplevel
        // "flicker" through a partially-updated state for one dispatch
        // cycle during a multi-field change, rather than seeing every
        // field change atomically together.
    }
}

impl Dispatch<ZcosmicToplevelHandleV1, u64> for TopIoState {
    fn event(
        state: &mut Self,
        _: &ZcosmicToplevelHandleV1,
        event: zcosmic_toplevel_handle_v1::Event,
        id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = *id;
        match event {
            zcosmic_toplevel_handle_v1::Event::Title { title } => state.tracker.apply(ToplevelEvent::Title(id, title)),
            zcosmic_toplevel_handle_v1::Event::AppId { app_id } => state.tracker.apply(ToplevelEvent::AppId(id, app_id)),
            zcosmic_toplevel_handle_v1::Event::State { state: raw } => {
                // Wire form per the protocol: a packed array of u32 (native
                // endian), each a State enum discriminant, present-if-set --
                // NOT a bitmask. Mirrors cosmic-protocols' own shipped
                // example's decode exactly, since getting this wrong (e.g.
                // treating it as a bitmask) would silently misread every
                // toplevel's state without any type error to catch it.
                let mut s = crate::toplevel::ToplevelState::default();
                for chunk in raw.chunks_exact(4) {
                    match u32::from_ne_bytes(chunk.try_into().unwrap()) {
                        0 => s.maximized = true,
                        1 => s.minimized = true,
                        2 => s.activated = true,
                        3 => s.fullscreen = true,
                        _ => {} // a future state value this crate doesn't model yet -- ignored, not a panic
                    }
                }
                state.tracker.apply(ToplevelEvent::StateChanged(id, s));
            }
            zcosmic_toplevel_handle_v1::Event::Closed => {
                // Both the tracker AND everything this session retained
                // for `id` — a closed toplevel's handle is no longer valid
                // to request a NEW capture source from, and any capture
                // negotiation already in flight for it has nothing left to
                // capture. Explicit destroy on the retained proxies, not
                // just dropping them from the maps: see this struct's own
                // field comments for why a drop alone leaks them server-side.
                state.handles.remove(&id);
                state.captures.remove(&id);
                if let Some(session) = state.copy_sessions.remove(&id) {
                    session.destroy();
                }
                if let Some(source) = state.capture_sources.remove(&id) {
                    source.destroy();
                }
                state.tracker.apply(ToplevelEvent::Closed(id));
            }
            _ => {} // geometry/output/workspace events: not modeled by ToplevelTracker's current fields
        }
    }
}

impl Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for TopIoState {
    fn event(
        _: &mut Self,
        _: &ExtForeignToplevelImageCaptureSourceManagerV1,
        _: wayland_protocols::ext::image_capture_source::v1::client::ext_foreign_toplevel_image_capture_source_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Factory only (create_source) — no events.
    }
}

impl Dispatch<ExtImageCaptureSourceV1, ()> for TopIoState {
    fn event(
        _: &mut Self,
        _: &ExtImageCaptureSourceV1,
        _: wayland_protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // No events of its own — an opaque handle passed to create_session.
    }
}

impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for TopIoState {
    fn event(_: &mut Self, _: &ExtImageCopyCaptureManagerV1, _: ext_image_copy_capture_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // Factory only — no events.
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, u64> for TopIoState {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mapped = match event {
            ext_image_copy_capture_session_v1::Event::BufferSize { width, height } => {
                Some(CaptureEvent::BufferSize { width, height })
            }
            ext_image_copy_capture_session_v1::Event::ShmFormat { format } => Some(CaptureEvent::ShmFormat(format.into())),
            ext_image_copy_capture_session_v1::Event::Done => Some(CaptureEvent::Done),
            ext_image_copy_capture_session_v1::Event::Stopped => Some(CaptureEvent::Stopped),
            _ => None, // dmabuf negotiation: out of scope, same as OutputCaptureSession
        };
        if let (Some(ev), Some(session)) = (mapped, state.captures.get_mut(id)) {
            session.apply(ev);
        }
    }
}

/// A live connection tracking every open toplevel window — the model an
/// app-switcher/task-view renders from. See `toplevel.rs`'s own header for
/// why this targets `ext-foreign-toplevel-list-v1` + COSMIC's
/// `zcosmic_toplevel_info_v1` extension specifically, confirmed from the
/// protocol XML's own text ("It thus extends ext_foreign_toplevel_v1") —
/// not from cosmic-protocols' shipped example, which demonstrates a path
/// the XML itself marks `deprecated-since="2"`.
pub struct ToplevelSession {
    conn: Connection,
    queue: EventQueue<TopIoState>,
    state: TopIoState,
}

impl ToplevelSession {
    pub fn connect() -> Result<Self, ConnectError> {
        let conn = Connection::connect_to_env().map_err(|e| ConnectError::NoDisplay(e.to_string()))?;
        let (globals, queue) = registry_queue_init::<TopIoState>(&conn)
            .map_err(|e| ConnectError::NoDisplay(e.to_string()))?;
        let qh = queue.handle();

        let list: ExtForeignToplevelListV1 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|_| ConnectError::MissingGlobal("ext_foreign_toplevel_list_v1"))?;
        // The COSMIC extension is genuinely optional per the base
        // protocol's own design ("intentionally minimalistic... additional
        // functionality... implemented in extension protocols") -- a
        // compositor could advertise the base list without it. Absent
        // here means every toplevel still gets tracked by id (the `New`
        // event fires regardless), just without title/app_id/state ever
        // populating, which callers can already tell from empty strings.
        let info: Option<ZcosmicToplevelInfoV1> = globals.bind(&qh, 1..=3, ()).ok();
        // Both genuinely optional, same reasoning as `info` above — a
        // compositor could support toplevel listing without supporting
        // capturing any of them. Absent means `capture_toplevel` below
        // always returns `None`, which callers can already distinguish
        // from "capture negotiation still in progress" (constraints not
        // yet received) since no CaptureSession ever gets created at all.
        let capture_source_mgr: Option<ExtForeignToplevelImageCaptureSourceManagerV1> =
            globals.bind(&qh, 1..=1, ()).ok();
        let copy_capture_mgr: Option<ExtImageCopyCaptureManagerV1> = globals.bind(&qh, 1..=1, ()).ok();

        let state = TopIoState {
            tracker: ToplevelTracker::new(),
            info,
            next_id: 1,
            handles: std::collections::HashMap::new(),
            capture_source_mgr,
            copy_capture_mgr,
            captures: std::collections::HashMap::new(),
            capture_sources: std::collections::HashMap::new(),
            copy_sessions: std::collections::HashMap::new(),
        };
        // `list` is only needed long enough to have been bound above —
        // toplevel events arrive via the registry-bound object automatically,
        // this function has no further use for the variable itself.
        drop(list);

        Ok(ToplevelSession { conn, queue, state })
    }

    pub fn poll(&mut self) -> &ToplevelTracker {
        let _ = self.queue.dispatch_pending(&mut self.state);
        &self.state.tracker
    }

    /// Starts capture negotiation for a specific open toplevel — the live-
    /// thumbnail case `CaptureSource::Toplevel` names in `capture.rs`.
    /// `None` if the id isn't currently tracked (already closed, or never
    /// existed), or if the compositor doesn't support toplevel capture at
    /// all (checked once at `connect()`, not re-checked per call).
    /// Idempotent: calling this again for an id already negotiating
    /// returns the SAME session rather than starting a second, redundant
    /// negotiation — a caller polling every frame to check on progress
    /// shouldn't have to remember whether it already called this once.
    pub fn capture_toplevel(&mut self, id: u64, paint_cursors: bool) -> Option<()> {
        if self.state.captures.contains_key(&id) {
            return Some(());
        }
        let handle = self.state.handles.get(&id)?;
        let source_mgr = self.state.capture_source_mgr.as_ref()?;
        let copy_mgr = self.state.copy_capture_mgr.as_ref()?;
        let qh = self.queue.handle();

        let source = source_mgr.create_source(handle, &qh, ());
        let options = if paint_cursors {
            ext_image_copy_capture_manager_v1::Options::PaintCursors
        } else {
            ext_image_copy_capture_manager_v1::Options::empty()
        };
        let session = copy_mgr.create_session(&source, options, &qh, id);
        self.state.capture_sources.insert(id, source);
        self.state.copy_sessions.insert(id, session);
        self.state.captures.insert(id, CaptureSession::new());
        Some(())
    }

    pub fn capture_state(&self, id: u64) -> Option<&CaptureSession> {
        self.state.captures.get(&id)
    }

    pub fn flush(&self) -> Result<(), String> {
        self.conn.flush().map_err(|e| e.to_string())
    }
}

struct CaptureIoState {
    session: CaptureSession,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for CaptureIoState {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlOutput, ()> for CaptureIoState {
    fn event(_: &mut Self, _: &WlOutput, _: wayland_client::protocol::wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // Geometry/mode/name events aren't needed here — this output
        // handle exists only to be handed to create_source.
    }
}

impl Dispatch<ExtOutputImageCaptureSourceManagerV1, ()> for CaptureIoState {
    fn event(
        _: &mut Self,
        _: &ExtOutputImageCaptureSourceManagerV1,
        _: wayland_protocols::ext::image_capture_source::v1::client::ext_output_image_capture_source_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Factory only (create_source) — no events.
    }
}

impl Dispatch<ExtImageCaptureSourceV1, ()> for CaptureIoState {
    fn event(
        _: &mut Self,
        _: &ExtImageCaptureSourceV1,
        _: wayland_protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // No events of its own — an opaque handle passed to create_session.
    }
}

impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for CaptureIoState {
    fn event(_: &mut Self, _: &ExtImageCopyCaptureManagerV1, _: ext_image_copy_capture_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // Factory only (create_session, create_pointer_cursor_session) — no events.
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for CaptureIoState {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mapped = match event {
            ext_image_copy_capture_session_v1::Event::BufferSize { width, height } => {
                Some(CaptureEvent::BufferSize { width, height })
            }
            ext_image_copy_capture_session_v1::Event::ShmFormat { format } => Some(CaptureEvent::ShmFormat(format.into())),
            ext_image_copy_capture_session_v1::Event::Done => Some(CaptureEvent::Done),
            ext_image_copy_capture_session_v1::Event::Stopped => Some(CaptureEvent::Stopped),
            // dmabuf_device/dmabuf_format: real GPU-buffer negotiation,
            // deliberately out of scope here per this module's own header
            // (buffer allocation is a caller concern, not this crate's).
            _ => None,
        };
        if let Some(ev) = mapped {
            state.session.apply(ev);
        }
    }
}

/// Negotiates capture parameters for the whole output — see `capture.rs`'s
/// own header for exactly where this crate's scope ends (buffer-constraint
/// discovery) and a caller's begins (actually allocating a buffer and
/// requesting a frame). `CaptureSource::Toplevel` capture is not wired here
/// yet: it needs `ToplevelSession` extended to retain the real
/// `ExtForeignToplevelHandleV1` proxies (currently discarded once a
/// `ToplevelInfo`'s fields are populated), a real, separate piece of work.
pub struct OutputCaptureSession {
    conn: Connection,
    queue: EventQueue<CaptureIoState>,
    state: CaptureIoState,
    _source: ExtImageCaptureSourceV1,
    _capture_session: ExtImageCopyCaptureSessionV1,
}

impl OutputCaptureSession {
    /// `paint_cursors`: whether the cursor should be composited into
    /// captured frames — the protocol's own distinction, passed straight
    /// through rather than this crate picking a default that hides the
    /// choice.
    pub fn connect(paint_cursors: bool) -> Result<Self, ConnectError> {
        let conn = Connection::connect_to_env().map_err(|e| ConnectError::NoDisplay(e.to_string()))?;
        let (globals, queue) = registry_queue_init::<CaptureIoState>(&conn)
            .map_err(|e| ConnectError::NoDisplay(e.to_string()))?;
        let qh = queue.handle();

        let output: WlOutput =
            globals.bind(&qh, 1..=4, ()).map_err(|_| ConnectError::MissingGlobal("wl_output"))?;
        let source_mgr: ExtOutputImageCaptureSourceManagerV1 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|_| ConnectError::MissingGlobal("ext_output_image_capture_source_manager_v1"))?;
        let capture_mgr: ExtImageCopyCaptureManagerV1 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|_| ConnectError::MissingGlobal("ext_image_copy_capture_manager_v1"))?;

        let source = source_mgr.create_source(&output, &qh, ());
        let options = if paint_cursors {
            ext_image_copy_capture_manager_v1::Options::PaintCursors
        } else {
            ext_image_copy_capture_manager_v1::Options::empty()
        };
        let capture_session = capture_mgr.create_session(&source, options, &qh, ());
        conn.flush().map_err(|e| ConnectError::NoDisplay(e.to_string()))?;

        Ok(OutputCaptureSession {
            conn,
            queue,
            state: CaptureIoState { session: CaptureSession::new() },
            _source: source,
            _capture_session: capture_session,
        })
    }

    pub fn poll(&mut self) -> &CaptureSession {
        let _ = self.queue.dispatch_pending(&mut self.state);
        &self.state.session
    }

    pub fn flush(&self) -> Result<(), String> {
        self.conn.flush().map_err(|e| e.to_string())
    }
}
