//! Real Wayland backend: shares GTK's `wl_display` to bind the
//! `zwp_virtual_keyboard_v1` (key delivery) and `zwp_input_method_v2`
//! (focus-driven show/hide) protocols.
//!
//! ## Why share GDK's connection
//!
//! The compositor routes `text-input-v3` focus events to *one*
//! input-method client per seat. If we open a second Wayland connection,
//! we'd be a different client from GTK and might never receive the
//! activate/deactivate events the user's app generates.
//!
//! `gdk4-wayland` with the `wayland_crate` feature exposes the
//! `WaylandDisplay`'s underlying `WlDisplay` and `WaylandSeat`'s
//! underlying `WlSeat` as `wayland-client` types. We piggyback on
//! GDK's connection: bind protocol globals on it, create our objects,
//! then process events on the shared fd via a GLib `unix_fd_add_local`
//! source so the GTK main loop drives both GDK and our protocol queue.
//!
//! ## Lifecycle
//!
//! 1. `WaylandBackend::try_new()` — find GDK display, downcast to
//!    WaylandDisplay, get the `WlDisplay`/`WlSeat`, build a queue,
//!    bind manager globals, create `zwp_virtual_keyboard_v1` and
//!    `zwp_input_method_v2`, send the keymap to the virtual keyboard.
//! 2. The dispatcher closure registered with GLib drains
//!    `EventQueue::dispatch_pending()` whenever the wl fd is readable.
//! 3. `commit_text` / `press_keysym` translate to `key(time, code, st)`
//!    pairs on the virtual keyboard.
//! 4. The input-method dispatch handler fires user-supplied callbacks
//!    on Activate/Deactivate so the keyboard window can show/hide.
//!
//! Tested compositors (per protocol availability): Sway, river,
//! Wayfire, Phosh, KWin (kwin_wayland with virtual-keyboard enabled).

#![cfg(feature = "wayland")]

use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use std::os::unix::io::AsFd;
use std::sync::Arc;
use tracing::{debug, info, warn};

use wayland_client::{
    protocol::{wl_registry, wl_seat::WlSeat},
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};

use wayland_protocols_misc::{
    zwp_input_method_v2::client::{
        zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
        zwp_input_method_v2::{self, ZwpInputMethodV2},
    },
    zwp_virtual_keyboard_v1::client::{
        zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
        zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
    },
};

// v1 input-method is what Plasma Mobile's KWin exposes (only). It's a
// different shape than v2: a single global (no manager), event-driven
// activate/deactivate, and a per-focus context object on which we send
// commit_string and keysym requests directly. No separate
// virtual-keyboard protocol needed; keysym requests on the context go
// through the same path KWin uses internally.
use wayland_protocols::wp::input_method::zv1::client::{
    zwp_input_method_context_v1::{self, ZwpInputMethodContextV1},
    zwp_input_method_v1::{self, ZwpInputMethodV1},
};

// ---- Public API surface --------------------------------------------------

/// Callback invoked when the input-method-v2 state flips. Wired to the
/// keyboard window's show/hide.
///
/// Note: this is a single-threaded `Rc<dyn Fn>`, not `Arc<dyn Fn + Send +
/// Sync>`. The callback is only ever invoked from the GLib main-loop
/// timer that drains our wayland queue, which runs on the GTK main
/// thread. Requiring `Send + Sync` would force us to wrap the GTK widgets
/// inside the callback in something thread-safe — but GTK widgets *can't*
/// be `Send` (they contain `*mut c_void`), so the call site would fail to
/// compile. Using `Rc<dyn Fn>` lets us capture the window directly.
pub type InputMethodActiveCallback = std::rc::Rc<dyn Fn(bool)>;

/// Callback for surrounding-text updates from the host's input
/// method. Fired whenever the focused app reports its current
/// editor contents and cursor position. Wired to the engine so
/// features like Synonym / Rewrite have real context to work
/// with (instead of relying solely on our local undo stack).
///
/// Same single-threaded constraint as InputMethodActiveCallback.
pub type SurroundingTextCallback =
    std::rc::Rc<dyn Fn(String, u32, u32)>;

/// Live Wayland backend. Owned by the bridge in `ui::window`; dropped on
/// app exit. Cloning this is cheap (everything inside is Arc-shared).
#[derive(Clone)]
pub struct WaylandBackend {
    inner: Arc<Mutex<BackendInner>>,
    /// The connection the queue belongs to, kept OUTSIDE the mutex so
    /// `pump` can read the socket without holding the backend lock
    /// across a syscall (the dispatch handlers take that lock).
    conn: Connection,
}

struct BackendInner {
    /// The shared connection (same Wayland connection GDK uses).
    _conn: Connection,
    /// Queue we own — separate dispatch for our globals so we don't
    /// step on GDK's queue.
    queue: EventQueue<AppData>,
    /// Application-side state delivered to dispatch handlers.
    app_data: AppData,
    /// Which protocol family is active.
    mode: BackendMode,
    /// Virtual keyboard object — present in v2 mode for sending keys
    /// to the focused app. Unused in v1 mode (keysyms go on context).
    vkbd: Option<ZwpVirtualKeyboardV1>,
    /// Input-method-v2 object: `commit_string` for text while a field is focused.
    im: Option<ZwpInputMethodV2>,
    /// Resolved keysym→keycode map (from the keymap we sent the compositor).
    /// Also used in v1 mode for symbolic-keysym name lookup.
    keymap: KeymapTable,
}

/// Which input-method protocol family the backend is driving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackendMode {
    /// `zwp_input_method_v2` + `zwp_virtual_keyboard_v1`. Phosh, wlroots,
    /// Sway, mobile-Mutter. Double-buffered, explicit Done events.
    V2,
    /// `zwp_input_method_v1` (with context). Plasma Mobile's KWin.
    /// Direct request/event flow; commit_string and keysym go on the
    /// per-focus context object.
    V1,
}

/// Strategy 1: open a fresh Wayland connection on an inherited fd
/// (the `WAYLAND_SOCKET` KWin passed us). We still need a `wl_seat`
/// matching this connection — bind one off the registry directly.
///
/// Returns (connection, wl_seat). Errs if the fd is unusable or if the
/// new connection doesn't expose a `wl_seat`.
fn open_inherited_connection(fd: std::os::unix::io::RawFd) -> Result<(Connection, WlSeat)> {
    use std::os::unix::io::{FromRawFd, OwnedFd};
    use std::os::unix::net::UnixStream;

    // Wrap the raw fd into an OwnedFd, then into a UnixStream — that's
    // what `Connection::from_socket` accepts. The fd is already a
    // connected unix socket to the compositor (KWin passed it that way).
    //
    // SAFETY: main.rs got the fd from KWin via WAYLAND_SOCKET and removed
    // it from the env so nothing else uses it; we ensure it's only opened
    // once (im_socket::take_inherited_fd consumes the slot).
    let owned: OwnedFd = unsafe { OwnedFd::from_raw_fd(fd) };
    let stream = UnixStream::from(owned);

    let conn = Connection::from_socket(stream)
        .map_err(|e| anyhow!("failed to open Wayland connection from inherited fd {}: {}", fd, e))?;

    let seat = probe_seat(&conn).map_err(|e| {
        anyhow!(
            "inherited Wayland connection didn't expose wl_seat — this \
             socket may not be a regular Wayland connection ({e})"
        )
    })?;
    Ok((conn, seat))
}

/// Strategy 2: share GDK's existing Wayland connection. Only viable when
/// the same socket exposes both rendering protocols (for GDK) AND the
/// input-method protocols (for us). True for wlroots-style compositors;
/// false for KWin (where input-method protocols live on a separate
/// privileged socket).
///
/// Only available under `gdk-integration` — this is the GTK app's own
/// connection-sharing convenience, not something a GTK-free consumer
/// (egui-frontend) has a GDK display to share in the first place.
#[cfg(feature = "gdk-integration")]
fn share_gdk_connection(
    gdk_display: &gtk4::gdk::Display,
) -> Result<(Connection, WlSeat)> {
    use gdk4_wayland::prelude::*;

    let gdk_seat = gdk_display
        .default_seat()
        .ok_or_else(|| anyhow!("no default GDK seat"))?;
    let wl_seat_obj = gdk_seat.downcast::<gdk4_wayland::WaylandSeat>().map_err(|_| {
        anyhow!("default seat is not a WaylandSeat (this shouldn't happen on Wayland)")
    })?;
    let wl_seat = wl_seat_obj
        .wl_seat()
        .ok_or_else(|| anyhow!("WaylandSeat::wl_seat() returned None"))?;

    // Reach the shared `Connection` *through the seat*. Every Proxy has a
    // `WeakBackend`, which upgrades to a `Backend`, which
    // `Connection::from_backend` accepts.
    let weak = wl_seat.backend();
    let backend = weak
        .upgrade()
        .ok_or_else(|| anyhow!("seat's WeakBackend has dropped"))?;
    let conn = Connection::from_backend(backend);

    Ok((conn, wl_seat))
}

/// Strategy 3: open a brand-new connection to whatever `$WAYLAND_DISPLAY`
/// points at, independent of any toolkit. For a process with no GDK/GTK
/// display at all (an eframe/winit app, say) — this is the only strategy
/// available, and it's actually the architecturally *normal* one: a
/// separate input-method client, in its own connection, is how Maliit
/// itself does it. The per-seat-singleton concern in this module's doc
/// comment is about NOT fragmenting one toolkit's rendering connection
/// from its own input-method connection; it isn't a reason a wholly
/// separate process/binary can't be its own single input-method client.
fn open_standalone_connection() -> Result<(Connection, WlSeat)> {
    // KWin (Plasma Mobile / Plasma desktop) launches the configured
    // virtual keyboard with a PRIVILEGED socket in `WAYLAND_SOCKET`; the
    // input-method globals live on that connection, not on the public
    // `$WAYLAND_DISPLAY` one. The GTK build captures it in main.rs and
    // `try_new` consumes it — the standalone path used to ignore it, so an
    // eframe build started by KWin connected to the public socket, found no
    // input-method global, and fell back to preview. Same precedence as
    // `try_new`: inherited fd first, `$WAYLAND_DISPLAY` otherwise.
    if let Some(fd) = crate::im_socket::take_inherited_fd() {
        info!("Wayland backend (standalone): using inherited input-method fd {}", fd);
        return open_inherited_connection(fd);
    }
    let conn = Connection::connect_to_env()
        .map_err(|e| anyhow!("failed to connect to $WAYLAND_DISPLAY: {}", e))?;
    let seat = probe_seat(&conn)?;
    Ok((conn, seat))
}

/// Walk the registry on a throwaway queue to find a `wl_seat`. Shared by
/// the inherited-fd and standalone strategies — both need to bind their
/// own seat since neither goes through GDK.
fn probe_seat(conn: &Connection) -> Result<WlSeat> {
    let mut seat_queue: EventQueue<SeatProbe> = conn.new_event_queue();
    let qh = seat_queue.handle();
    let _registry = conn.display().get_registry(&qh, ());
    let mut probe = SeatProbe { seat: None };
    seat_queue
        .roundtrip(&mut probe)
        .map_err(|e| anyhow!("seat probe roundtrip failed: {}", e))?;
    probe.seat.ok_or_else(|| {
        anyhow!("Wayland connection didn't expose wl_seat — is this a regular compositor socket?")
    })
}

/// Entry point for callers with no GDK/GTK display in the process at
/// all — an eframe/winit build, for instance. Skips the GDK-display
/// gate `try_new` starts with and always uses
/// [`open_standalone_connection`]; everything past connection setup
/// (registry walk, v1/v2 detection, virtual-keyboard + input-method
/// object creation, the GLib dispatcher) is identical, via
/// [`finish_setup`].
///
/// Still requires a running GLib main context to actually pump events —
/// see [`install_glib_dispatcher`]'s doc. A winit/eframe app has none by
/// default; the caller needs to drive one itself (e.g.
/// `glib::MainContext::default().iteration(false)` once per frame) or
/// this backend will silently never see an Activate/Deactivate event.
pub fn try_new_standalone(
    im_callback: InputMethodActiveCallback,
    surrounding_text_callback: SurroundingTextCallback,
) -> Result<Option<WaylandBackend>> {
    let (conn, wl_seat) = open_standalone_connection()?;
    finish_setup(conn, wl_seat, im_callback, surrounding_text_callback)
}

/// Steps shared by every connection strategy once we have a `Connection`
/// + `WlSeat` in hand: build a queue, walk the registry for the
/// input-method/virtual-keyboard globals, detect v1 vs v2, create the
/// protocol objects, and install the GLib-driven dispatcher.
fn finish_setup(
    conn: Connection,
    wl_seat: WlSeat,
    im_callback: InputMethodActiveCallback,
    surrounding_text_callback: SurroundingTextCallback,
) -> Result<Option<WaylandBackend>> {
    // 2. Build a fresh event queue on whichever connection we got. This
    //    queue is independent of GDK's; events for our protocol objects
    //    get routed here, GDK's events stay with GDK.
    let mut queue: EventQueue<AppData> = conn.new_event_queue();
    let qh = queue.handle();

    // 3. Walk the registry to bind the manager globals we need.
    let display = conn.display();
    let _registry = display.get_registry(&qh, ());

    let mut app_data = AppData {
        registry_done: false,
        vk_manager: None,
        im_manager: None,
        im_v1: None,
        im_v1_context: None,
        im_v1_serial: 0,
        seat: wl_seat.clone(),
        active: false,
        pending_active: false,
        im_v2_serial: 0,
        im_callback: im_callback.clone(),
        surrounding_text_callback: surrounding_text_callback.clone(),
    };

    // Roundtrip to populate globals.
    queue
        .roundtrip(&mut app_data)
        .map_err(|e| anyhow!("registry roundtrip failed: {}", e))?;

    // Detect which input-method protocol family is available.
    //
    // Plasma Mobile's KWin exposes only the legacy v1 protocol. Phosh
    // and wlroots-based compositors expose v2 (plus virtual-keyboard-v1).
    // We prefer v2 when present because it's the modern protocol with
    // double-buffered state and explicit "Done" events, but fall back
    // to v1 if that's all the compositor offers.
    //
    // For the v1 path: keysym requests go directly on the context object
    // (no separate virtual-keyboard channel needed), so the vk_manager
    // is optional. For v2: vk_manager is required for keystroke delivery.
    let has_v2 = app_data.vk_manager.is_some() && app_data.im_manager.is_some();
    let has_v1 = app_data.im_v1.is_some();

    if !has_v2 && !has_v1 {
        warn!(
            "Compositor exposes neither input-method-v2 (with \
             virtual-keyboard-v1) nor input-method-v1. Cannot send \
             keystrokes; falling back to no-op key delivery."
        );
        return Ok(None);
    }

    let mode = if has_v2 {
        info!("Using input-method-v2 + virtual-keyboard-v1 (modern path)");
        BackendMode::V2
    } else {
        info!(
            "Using input-method-v1 (Plasma Mobile / legacy path). \
             commit_string and keysym requests will go directly on the \
             v1 context object."
        );
        BackendMode::V1
    };

    // ---- v2 setup: build the virtual keyboard and input-method objects ----
    let (vkbd, im, keymap) = if matches!(mode, BackendMode::V2) {
        // unwraps safe because has_v2 == true above
        let vk_manager = app_data.vk_manager.clone().unwrap();
        let im_manager = app_data.im_manager.clone().unwrap();

        let vkbd = vk_manager.create_virtual_keyboard(&wl_seat, &qh, ());
        let keymap = build_us_keymap()?;
        send_keymap(&vkbd, &keymap)?;

        let im = im_manager.get_input_method(&wl_seat, &qh, ());

        info!("Wayland backend up: vkbd=yes, input_method=yes (v2)");
        (Some(vkbd), Some(im), keymap)
    } else {
        // v1 path: no virtual-keyboard needed. We don't pre-create an
        // input-method either — we already bound the global, and KWin
        // will activate it when a text input gets focus.
        info!("Wayland backend up: vkbd=no, input_method=yes (v1)");
        // Build a keymap anyway so press_keysym can look up keysym names
        // even on the v1 path (we use it for symbol→keysym translation).
        let keymap = build_us_keymap()?;
        (None, None, keymap)
    };

    let inner = Arc::new(Mutex::new(BackendInner {
        _conn: conn.clone(),
        queue,
        app_data,
        mode,
        vkbd,
        im,
        keymap,
    }));

    // 8. Dispatching is the HOST's job, through `WaylandBackend::pump`.
    //    The GTK app has a GLib main loop and installs the self-adaptive
    //    timer below on top of it; a host with its own frame loop (the
    //    egui build) calls `pump()` once per frame instead. The timer
    //    used to be installed unconditionally, which made the plain
    //    `wayland` feature depend on `glib` without declaring it — the
    //    GTK-free path could not compile on its own.
    #[cfg(feature = "gdk-integration")]
    install_glib_dispatcher(conn.clone(), inner.clone());

    Ok(Some(WaylandBackend { inner, conn }))
}

/// Registry probe used by both `open_inherited_connection` and
/// `open_standalone_connection` to find a `wl_seat` — throwaway queue,
/// dropped once the roundtrip completes.
struct SeatProbe {
    seat: Option<WlSeat>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for SeatProbe {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == "wl_seat" && state.seat.is_none() {
                let v = version.min(7);
                let s: WlSeat = registry.bind(name, v, qh, ());
                state.seat = Some(s);
            }
        }
    }
}

impl Dispatch<WlSeat, ()> for SeatProbe {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Try to construct a real Wayland backend. Returns `Ok(None)` (rather
/// than `Err`) when the prerequisites simply aren't met (X11 session,
/// missing protocols, etc) — the caller falls back to xdotool/noop.
/// Returns `Err` for genuine setup failures.
///
/// ## Connection strategy
///
/// We try two paths depending on what the host compositor does:
///
/// 1. **KWin/Weston input-method**: a privileged Wayland fd is inherited
///    via `WAYLAND_SOCKET=<fd>` (which `main.rs` captured into
///    `compositor::im_socket`). We open a SECOND wayland-client
///    connection on that fd. The privileged socket exposes
///    `zwp_input_method_v2` and `zwp_virtual_keyboard_v1` (which the
///    public socket doesn't). GDK keeps its own connection on the public
///    socket for rendering / layer-shell — they don't conflict.
///
/// 2. **wlroots-style (Sway/Phosh/river)**: there's no separate
///    privileged socket; input-method and virtual-keyboard are exposed
///    on the same public socket GDK is using. We share GDK's connection
///    by reaching it through `WaylandSeat::backend()`.
///
/// In both cases the resulting `EventQueue<AppData>` is what we use to
/// dispatch protocol events; the only difference is whether it lives on
/// our own connection (case 1) or GDK's (case 2).
///
/// GTK-app entry point — needs a live GDK display, hence
/// `gdk-integration`. GTK-free consumers want [`try_new_standalone`].
#[cfg(feature = "gdk-integration")]
pub fn try_new(
    im_callback: InputMethodActiveCallback,
    surrounding_text_callback: SurroundingTextCallback,
) -> Result<Option<WaylandBackend>> {
    use gdk4_wayland::prelude::*;

    // 1. Find a GDK Wayland display. On X11 this returns None.
    let gdk_display = match gtk4::gdk::Display::default() {
        Some(d) => d,
        None => return Ok(None),
    };
    if gdk_display
        .clone()
        .downcast::<gdk4_wayland::WaylandDisplay>()
        .is_err()
    {
        debug!("GDK display is not a WaylandDisplay (probably X11)");
        return Ok(None);
    }

    // Decide which connection strategy to use based on whether main.rs
    // captured an inherited input-method fd.
    let inherited_fd = crate::im_socket::take_inherited_fd();

    let (conn, wl_seat) = if let Some(fd) = inherited_fd {
        info!(
            "Wayland backend: using inherited input-method fd {} as a \
             separate Wayland connection (the privileged socket KWin \
             passed us). GDK keeps its public-socket connection for \
             rendering & layer-shell.",
            fd
        );
        open_inherited_connection(fd)?
    } else {
        info!(
            "Wayland backend: no WAYLAND_SOCKET inherited; sharing GDK's \
             connection (wlroots-style — input-method protocols on the \
             public socket)."
        );
        share_gdk_connection(&gdk_display)?
    };

    finish_setup(conn, wl_seat, im_callback, surrounding_text_callback)
}

impl WaylandBackend {
    /// Read whatever the compositor has sent, dispatch it to the protocol
    /// handlers (Activate/Deactivate reach the callbacks from here), and
    /// flush anything queued to go out. Non-blocking: returns immediately
    /// with the number of events dispatched, `0` when nothing arrived.
    ///
    /// This is the one entry point a host loop needs. The GTK app never
    /// calls it — its GLib timer (`install_glib_dispatcher`, under
    /// `gdk-integration`) does the same three steps on a timer. Any other
    /// host (eframe per frame, a layer-shell loop after each poll) calls
    /// this itself; there is no thread and no toolkit inside.
    pub fn pump(&self) -> Result<usize> {
        dispatch_once(&self.conn, &self.inner)
    }

    /// The connection's socket, for a host loop to `poll()` alongside its
    /// own: readable means `pump()` has something to do. With it a host can
    /// sleep until the compositor speaks instead of waking on a timer just
    /// to ask. Valid for as long as this backend lives.
    pub fn poll_fd(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd;
        self.conn.backend().poll_fd().as_raw_fd()
    }

    /// Whether this backend uses the modern input-method-v2 path
    /// (Phosh/wlroots) versus the legacy v1 path (Plasma Mobile).
    /// Useful for deciding whether to trust Activate events for
    /// auto-hide behavior.
    pub fn uses_v2(&self) -> bool {
        matches!(self.inner.lock().mode, BackendMode::V2)
    }

    /// Send a string of literal text to the focused application.
    ///
    /// **v2 path**: pressing/releasing each character's keysym via
    /// `zwp_virtual_keyboard_v1`. This works whether or not the IM is
    /// active because virtual-keyboard isn't tied to focus.
    ///
    /// **v1 path**: `zwp_input_method_context_v1::commit_string` with the
    /// latest serial. Only works while a text input is focused (i.e.
    /// while we have an active context). If no context, the call is a
    /// no-op (logged at debug level).
    pub fn commit_text(&self, text: &str) -> Result<()> {
        let inner = self.inner.lock();

        match inner.mode {
            BackendMode::V2 => {
                // TEXT GOES THROUGH THE INPUT METHOD when a field is focused. The key path
                // below sends QWERTY evdev keycodes, which cosmic-comp maps with the SEAT's
                // layout, not the keymap we upload: on a Dvorak seat `i` came out `c`.
                // commit_string is the characters themselves, whatever the layout.
                debug!("commit_text({} chars): {}", text.chars().count(), if inner.app_data.active { "through the input method (a text field is active)" } else { "as virtual-keyboard key events (no active text field: the focused surface gets keys)" });
                if inner.app_data.active {
                    if let Some(im) = inner.im.as_ref() {
                        im.commit_string(text.to_string());
                        im.commit(inner.app_data.im_v2_serial);
                        let conn = inner._conn.clone();
                        drop(inner);
                        let _ = conn.flush();
                        return Ok(());
                    }
                }
                let vkbd = inner
                    .vkbd
                    .as_ref()
                    .ok_or_else(|| anyhow!("no virtual keyboard (v2 mode)"))?;

                for ch in text.chars() {
                    let Some(entry) = inner.keymap.lookup_char(ch) else {
                        debug!("char {:?} not in keymap; skipping", ch);
                        continue;
                    };

                    let mods_depressed = if entry.shifted { 1 } else { 0 };
                    vkbd.modifiers(mods_depressed, 0, 0, 0);

                    let time = monotonic_ms();
                    let evdev = entry.evdev_keycode;
                    vkbd.key(time, evdev, 1);
                    vkbd.key(time + 1, evdev, 0);

                    if entry.shifted {
                        vkbd.modifiers(0, 0, 0, 0);
                    }
                }
            }
            BackendMode::V1 => {
                let Some(ctx) = inner.app_data.im_v1_context.as_ref() else {
                    // SILENT FAILURE PATH — this is the most likely
                    // cause of "swipe-typed words don't appear" on
                    // Plasma Mobile / KWin. If no IM context is
                    // active, the v1 path has nothing to commit
                    // through. Logged at warn! (not debug!) so an
                    // on-device journal pull surfaces this without
                    // RUST_LOG tweaks.
                    tracing::warn!(
                        "v1 commit_text dropped — no active im_v1_context. \
                         text={:?} chars={}. Likely cause: compositor sent \
                         Deactivate for the target text-input between \
                         drag_begin and drag_end (or never sent Activate).",
                        text, text.chars().count()
                    );
                    let conn = inner._conn.clone();
                    drop(inner);
                    let _ = conn.flush();
                    return Ok(());
                };
                let serial = inner.app_data.im_v1_serial;
                // SUCCESS PATH — also info! so the journal proves the
                // commit happened. Without this log, an absent
                // commit-text line means "no commit fired"; with this
                // log, we can disambiguate "commit fired but didn't
                // reach the app" from "commit was dropped here."
                tracing::info!(
                    "v1 commit_string sent: serial={} chars={} text={:?}",
                    serial, text.chars().count(), text
                );
                ctx.commit_string(serial, text.to_string());
            }
        }

        let conn = inner._conn.clone();
        drop(inner);
        let _ = conn.flush();
        Ok(())
    }

    /// Press+release a single named keysym (e.g. "BackSpace", "Return",
    /// "Tab", "Left", "Up", "Down", "Right").
    ///
    /// **Modifier prefixes**: the name may be prefixed by one or more
    /// modifier names joined with `+`. Supported prefixes:
    ///   - `Shift+` → bit 0
    ///   - `Ctrl+`  → bit 2
    ///   - `Alt+`   → bit 3
    /// They may compose: `Ctrl+Shift+Left`. Order doesn't matter; we
    /// parse greedily. The bare keysym name comes last.
    ///
    /// **v2 path**: sets modifiers with `vkbd.modifiers(...)` before
    /// the key event and clears them after, so the modifier is scoped
    /// to just that keypress.
    ///
    /// **v1 path**: passes the modifier bitmask directly to the v1
    /// context's `keysym` request, which takes a modifiers argument.
    /// KWin translates to a key event for the focused app with the
    /// modifier applied.
    pub fn press_keysym(&self, name: &str) -> Result<()> {
        let (mods, base_name) = parse_keysym_modifiers(name);
        let inner = self.inner.lock();

        match inner.mode {
            BackendMode::V2 => {
                // A plain printable character with no modifier: commit it as text (see
                // `commit_text` -- keycodes are re-mapped by the seat's layout).
                if mods == 0 && inner.app_data.active {
                    let mut cs = base_name.chars();
                    if let (Some(c), None) = (cs.next(), cs.next()) {
                        if !c.is_control() {
                            drop(inner);
                            return self.commit_text(base_name);
                        }
                    }
                }
                debug!("press_keysym({name:?}): virtual-keyboard key (input method active={})", inner.app_data.active);
                let vkbd = inner
                    .vkbd
                    .as_ref()
                    .ok_or_else(|| anyhow!("no virtual keyboard (v2 mode)"))?;
                // A single character ("Ctrl+c" from a latched modifier) is
                // found by character: the named table only has BackSpace,
                // arrows and the like. A shifted character adds Shift.
                let mut mods = mods;
                let entry = match inner.keymap.lookup_keysym_name(base_name) {
                    Some(e) => e,
                    None => {
                        let mut cs = base_name.chars();
                        let e = match (cs.next(), cs.next()) {
                            (Some(c), None) => inner.keymap.lookup_char(c),
                            _ => None,
                        }
                        .ok_or_else(|| anyhow!("unknown keysym {:?}", base_name))?;
                        if e.shifted {
                            mods |= 1;
                        }
                        e
                    }
                };
                let evdev = entry.evdev_keycode;
                let time = monotonic_ms();
                if mods != 0 {
                    vkbd.modifiers(mods, 0, 0, 0);
                }
                vkbd.key(time, evdev, 1);
                vkbd.key(time + 1, evdev, 0);
                if mods != 0 {
                    vkbd.modifiers(0, 0, 0, 0);
                }
            }
            BackendMode::V1 => {
                let Some(ctx) = inner.app_data.im_v1_context.as_ref() else {
                    debug!(
                        "press_keysym({:?}) on v1 path but no active context; \
                         dropping.",
                        name
                    );
                    let conn = inner._conn.clone();
                    drop(inner);
                    let _ = conn.flush();
                    return Ok(());
                };
                // Printable ASCII: the keysym IS the code point (a chord like
                // "Ctrl+c" from a latched modifier).
                let sym = xkb_keysym_from_name(base_name)
                    .or_else(|| {
                        let mut cs = base_name.chars();
                        match (cs.next(), cs.next()) {
                            (Some(c), None) if (' '..='~').contains(&c) => Some(c as u32),
                            _ => None,
                        }
                    })
                    .ok_or_else(|| anyhow!("unknown keysym name {:?}", base_name))?;
                let serial = inner.app_data.im_v1_serial;
                let time = monotonic_ms();
                debug!(
                    "v1 keysym(serial={}, time={}, sym=0x{:x}, mods=0x{:x}, name={})",
                    serial, time, sym, mods, name
                );
                ctx.keysym(serial, time, sym, 1, mods); // pressed
                ctx.keysym(serial, time + 1, sym, 0, mods); // released
            }
        }

        let conn = inner._conn.clone();
        drop(inner);
        let _ = conn.flush();
        Ok(())
    }
}

/// Map a symbolic keysym name (e.g. "BackSpace", "Return") to its XKB
/// keysym numeric value. Hardcoded for the names rusty-board's engine
/// emits — extending later is just adding to the table.
/// Parse "ModName+Other+...+KeysymName" into a Wayland modifiers
/// bitmask plus the bare keysym name. Modifier names recognized:
/// "Shift" (bit 0), "Ctrl" (bit 2), "Alt" (bit 3). Bit 1 is caps-lock,
/// which we don't synthesize. Unknown prefixes are treated as part of
/// the keysym name (so e.g. a hypothetical "Foo+Bar" keysym still
/// works if `xkb_keysym_from_name` recognizes the whole string).
///
/// Examples:
///   parse_keysym_modifiers("Left")            => (0, "Left")
///   parse_keysym_modifiers("Shift+Left")      => (1, "Left")
///   parse_keysym_modifiers("Ctrl+Left")       => (4, "Left")
///   parse_keysym_modifiers("Ctrl+Shift+Left") => (5, "Left")
///
/// We split on `+`, peel off prefixes whose normalized name matches
/// a known modifier, and rejoin the rest. The rest is what gets
/// looked up against the keysym table.
fn parse_keysym_modifiers(name: &str) -> (u32, &str) {
    let mut mods: u32 = 0;
    let mut remaining = name;

    loop {
        let Some(plus_idx) = remaining.find('+') else {
            break;
        };
        let (prefix, rest) = remaining.split_at(plus_idx);
        let rest = &rest[1..]; // skip the '+'

        let prefix_lower = prefix.trim().to_ascii_lowercase();
        let bit = match prefix_lower.as_str() {
            "shift" => 1,        // bit 0
            "ctrl" | "control" => 4, // bit 2
            "alt" => 8,          // bit 3
            _ => break,          // not a recognized modifier; stop here
        };
        mods |= bit;
        remaining = rest;
    }

    (mods, remaining)
}

fn xkb_keysym_from_name(name: &str) -> Option<u32> {
    // XKB keysym values from /usr/include/xkbcommon/xkbcommon-keysyms.h
    // (also at https://wayland.app/protocols/wayland — keysym values are
    // universally agreed across X11/Wayland/XKB).
    Some(match name {
        "BackSpace" => 0xff08,
        "Tab" => 0xff09,
        "Return" => 0xff0d,
        "Escape" => 0xff1b,
        "Delete" => 0xffff,
        "Home" => 0xff50,
        "End" => 0xff57,
        "Left" => 0xff51,
        "Up" => 0xff52,
        "Right" => 0xff53,
        "Down" => 0xff54,
        "Page_Up" => 0xff55,
        "Page_Down" => 0xff56,
        "space" => 0x0020,
        _ => return None,
    })
}

// ---- Internal: registry + dispatcher state -------------------------------

struct AppData {
    /// Set after the registry roundtrip completes; not read again, but
    /// useful for diagnostics in future work.
    #[allow(dead_code)]
    registry_done: bool,
    vk_manager: Option<ZwpVirtualKeyboardManagerV1>,
    im_manager: Option<ZwpInputMethodManagerV2>,
    /// v1 input-method global (Plasma Mobile path). On compositors that
    /// expose v2, this stays None and v2 is preferred.
    im_v1: Option<ZwpInputMethodV1>,
    /// Active v1 context — non-None while a text input is focused.
    /// Replaced/cleared by activate/deactivate events.
    im_v1_context: Option<ZwpInputMethodContextV1>,
    /// Most recent serial received from the text-input side via
    /// commit_state. We echo this back on every commit_string/keysym
    /// request so the compositor can correlate.
    im_v1_serial: u32,
    /// Kept alive so the seat proxy doesn't drop while we have objects
    /// associated with it.
    #[allow(dead_code)]
    seat: WlSeat,
    /// Tracks the current input-method state for double-buffered events.
    active: bool,
    /// Pending state, applied on the next `Done` event.
    pending_active: bool,
    /// Number of `Done` events seen: the serial a v2 `commit` must carry.
    im_v2_serial: u32,
    /// Where to call when state actually flips.
    im_callback: InputMethodActiveCallback,
    /// Where to call when the host reports surrounding text. The
    /// callback receives (text, cursor_byte_offset, anchor_byte_offset).
    surrounding_text_callback: SurroundingTextCallback,
}

impl Dispatch<wl_registry::WlRegistry, ()> for AppData {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            // Log EVERY global so we can diagnose what protocols this
            // socket exposes. Critical for Plasma Mobile debugging where
            // the privileged IM socket may not expose layer-shell.
            info!(
                "wl_registry::global: name={} interface={} version={}",
                name, interface, version
            );
            match interface.as_str() {
                "zwp_virtual_keyboard_manager_v1" => {
                    let v = version.min(1);
                    let m: ZwpVirtualKeyboardManagerV1 = registry.bind(name, v, qh, ());
                    state.vk_manager = Some(m);
                }
                "zwp_input_method_manager_v2" => {
                    let v = version.min(1);
                    let m: ZwpInputMethodManagerV2 = registry.bind(name, v, qh, ());
                    state.im_manager = Some(m);
                }
                "zwp_input_method_v1" => {
                    // v1 is a singleton global (no manager). Bind it
                    // directly. KWin will start firing activate/deactivate
                    // events on it as soon as we're bound and a text
                    // input gets focus.
                    let v = version.min(1);
                    let im: ZwpInputMethodV1 = registry.bind(name, v, qh, ());
                    state.im_v1 = Some(im);
                }
                _ => {}
            }
        }
    }
}

// Manager objects have no events; required by Dispatch though.
macro_rules! noop_dispatch {
    ($t:ty) => {
        impl Dispatch<$t, ()> for AppData {
            fn event(
                _: &mut Self,
                _: &$t,
                _: <$t as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

noop_dispatch!(ZwpVirtualKeyboardManagerV1);
noop_dispatch!(ZwpVirtualKeyboardV1);
noop_dispatch!(ZwpInputMethodManagerV2);

// Input-method-v2 fires the events we actually care about.
impl Dispatch<ZwpInputMethodV2, ()> for AppData {
    fn event(
        state: &mut Self,
        _: &ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v2::Event::Activate => {
                state.pending_active = true;
            }
            zwp_input_method_v2::Event::Deactivate => {
                state.pending_active = false;
            }
            zwp_input_method_v2::Event::Done => {
                state.im_v2_serial = state.im_v2_serial.wrapping_add(1);
                if state.pending_active != state.active {
                    state.active = state.pending_active;
                    debug!("input-method state -> active={}", state.active);
                    (state.im_callback)(state.active);
                }
            }
            zwp_input_method_v2::Event::Unavailable => {
                warn!(
                    "input-method-v2 reported Unavailable — another OSK \
                     owns the seat's input method (squeekboard is enabled \
                     by default on most postmarketOS/Phosh images). This \
                     object is dead: no Activate will ever arrive on it, \
                     so focus-driven show/hide has no signal for the rest \
                     of this session."
                );
                // Recorded, not just logged. A front-end that hides
                // itself when nothing is focused has to be able to tell
                // "no field is focused" from "there is no focus signal";
                // only the second means it should stop hiding. See
                // `im_health`.
                crate::im_health::set_input_method_unavailable();
            }
            zwp_input_method_v2::Event::SurroundingText {
                text,
                cursor,
                anchor,
            } => {
                debug!(
                    "input-method-v2 surrounding_text len={} cursor={} anchor={}",
                    text.len(),
                    cursor,
                    anchor
                );
                (state.surrounding_text_callback)(text, cursor, anchor);
            }
            zwp_input_method_v2::Event::ContentType { hint: _, purpose } => {
                // Same content-purpose plumbing as the v1 path — see
                // the v1 handler for full notes. WEnum<ContentPurpose>
                // → u8 for the process-wide atomic. Pattern-match the
                // WEnum variants directly: Value carries the known
                // enum (cast to u32 since the generated enum is
                // repr(u32)); Unknown already carries the raw u32 for
                // values the binding doesn't recognize.
                let raw: u32 = match purpose {
                    WEnum::Value(p) => p as u32,
                    WEnum::Unknown(raw) => raw,
                };
                debug!("v2 ContentType: purpose={}", raw);
                // v2 carries text-input-v3 numbering, which differs from
                // v1's after `password`; see set_content_purpose_v3.
                crate::content_purpose::set_content_purpose_v3(raw);
            }
            // TextChangeCause: not yet wired.
            _ => {}
        }
    }
}

// ---- v1 input-method Dispatch impls --------------------------------------
//
// v1 is much simpler than v2:
//   - The global itself fires Activate(new_id<context>) when a text input
//     focuses, and Deactivate(context) when it blurs.
//   - The new context object then fires commit_state(serial),
//     surrounding_text, content_type, etc. We read serials and store
//     them so we can echo them back on commit_string / keysym requests.
//   - There's no Done/double-buffering. State changes are immediate.

impl Dispatch<ZwpInputMethodV1, ()> for AppData {
    fn event(
        state: &mut Self,
        _: &ZwpInputMethodV1,
        event: zwp_input_method_v1::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v1::Event::Activate { id } => {
                // info! (not debug!) — context lifecycle is exactly
                // what we need visible in the journal to diagnose
                // "swipe doesn't write" bugs. Activate creates the
                // context object that commit_string later uses; if
                // Deactivate fires between drag_begin and drag_end,
                // the commit drops silently. Both events should be
                // visible at default log verbosity.
                tracing::info!("input-method-v1: Activate (new context)");
                // Defensive: if we somehow have a stale context (e.g.
                // compositor activated twice without an intervening
                // deactivate), destroy it before replacing.
                if let Some(old) = state.im_v1_context.take() {
                    tracing::info!("Destroying stale v1 context before replacing");
                    old.destroy();
                }
                state.im_v1_context = Some(id);
                state.im_v1_serial = 0;
                if !state.active {
                    state.active = true;
                    (state.im_callback)(true);
                }
            }
            zwp_input_method_v1::Event::Deactivate { context } => {
                tracing::info!("input-method-v1: Deactivate — context destroyed");
                // Per spec we must destroy the context object after a
                // deactivate. Forget our reference too.
                context.destroy();
                state.im_v1_context = None;
                state.im_v1_serial = 0;
                if state.active {
                    state.active = false;
                    (state.im_callback)(false);
                }
            }
            _ => {}
        }
    }

    // The Activate event has a `new_id<zwp_input_method_context_v1>`
    // argument. wayland-rs auto-binds new_id arguments to a Dispatch
    // user-data, but it needs us to specify which user-data type
    // (here: `()`) and what interface. Without this override, wayland-rs
    // panics with "Missing event_created_child specialization for event
    // opcode 0 of zwp_input_method_v1" — which is exactly the crash we
    // observed before adding this.
    //
    // The opcode for `activate` is 0 (it's the first event in the
    // protocol XML). The macro generates an event_created_child fn that
    // matches on the opcode and returns ObjectData configured for our
    // Dispatch<ZwpInputMethodContextV1, ()> impl.
    wayland_client::event_created_child!(AppData, ZwpInputMethodV1, [
        zwp_input_method_v1::EVT_ACTIVATE_OPCODE => (ZwpInputMethodContextV1, ()),
    ]);
}

impl Dispatch<ZwpInputMethodContextV1, ()> for AppData {
    fn event(
        state: &mut Self,
        _: &ZwpInputMethodContextV1,
        event: zwp_input_method_context_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_context_v1::Event::CommitState { serial } => {
                // The text-input side acks state via commit_state. We
                // must echo this serial back in every commit_string /
                // keysym request so the compositor can correlate.
                state.im_v1_serial = serial;
            }
            zwp_input_method_context_v1::Event::SurroundingText {
                text,
                cursor,
                anchor,
            } => {
                debug!(
                    "input-method-v1 surrounding_text len={} cursor={} anchor={}",
                    text.len(),
                    cursor,
                    anchor
                );
                (state.surrounding_text_callback)(text, cursor, anchor);
            }
            zwp_input_method_context_v1::Event::ContentType { hint: _, purpose } => {
                // Store the purpose so the focus-show logic can pick
                // the right keyboard (e.g., terminal → full keyboard
                // even when focus-tray mode is on). The hint field is
                // a bitmask of fine-grained behaviors (auto-cap,
                // password obscure, etc.); we don't consume it yet.
                //
                // v1 protocol bindings deliver `purpose` as a raw u32
                // (older protocol, simpler types). v2 uses
                // WEnum<ContentPurpose>; the type asymmetry between
                // the two paths is intentional in wayland-protocols.
                debug!("ContentType: purpose={}", purpose);
                crate::content_purpose::set_content_purpose(purpose.min(255) as u8);
            }
            zwp_input_method_context_v1::Event::Reset
            | zwp_input_method_context_v1::Event::InvokeAction { .. }
            | zwp_input_method_context_v1::Event::PreferredLanguage { .. } => {
                // We don't track text-change-cause, action-invoke, or
                // language preferences yet. Future work.
            }
            _ => {}
        }
    }
}

// ---- Keymap construction (US ASCII) --------------------------------------

/// One keymap entry — the evdev keycode and whether shift is needed.
#[derive(Clone, Copy, Debug)]
struct KeymapEntry {
    evdev_keycode: u32,
    shifted: bool,
}

#[derive(Default)]
struct KeymapTable {
    /// Raw xkb-format keymap text (sent verbatim to the compositor in
    /// `send_keymap`).
    keymap_text: String,
    /// Lookup: ASCII char → entry.
    by_char: std::collections::HashMap<char, KeymapEntry>,
    /// Lookup: keysym name (e.g. "F1", "Home") → entry.
    by_name: std::collections::HashMap<String, KeymapEntry>,
}

impl KeymapTable {
    fn lookup_char(&self, ch: char) -> Option<KeymapEntry> {
        self.by_char.get(&ch).copied()
    }
    fn lookup_keysym_name(&self, name: &str) -> Option<KeymapEntry> {
        self.by_name.get(name).copied()
    }
}

/// Build a US-ASCII xkb keymap as a string, plus a hardcoded lookup
/// table for the most common ASCII chars.
///
/// Why hardcode? Walking the xkb keymap programmatically requires either
/// the `key_for_each` callback API (whose closure-based iteration is
/// awkward to translate to a HashMap) or unsafe iteration over Keycode
/// ranges. The US ASCII layer is invariant across xkbcommon versions,
/// so a static table is both simpler and more reliable. The keymap
/// *text* still goes through xkbcommon so it's exactly what the
/// compositor expects to consume.
fn build_us_keymap() -> Result<KeymapTable> {
    use xkbcommon::xkb;

    // THE SEAT'S layout, not US: cosmic-comp maps the keycodes we send with its own
    // layout (Dvorak on the owner's phone), so the character -> keycode table has to
    // be that layout's, or `c` types `j` and Ctrl+C is Ctrl+J.
    let (layout, variant) = seat_layout();
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let keymap = xkb::Keymap::new_from_names(&context, "", "", &layout, &variant, None, xkb::COMPILE_NO_FLAGS)
        .or_else(|| xkb::Keymap::new_from_names(&context, "", "", "us", "", None, xkb::COMPILE_NO_FLAGS))
        .ok_or_else(|| anyhow!("xkb: failed to compile '{layout}/{variant}' or 'us'"))?;

    let keymap_text = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);

    // Hardcoded US-ASCII map. Evdev keycodes are from
    // <linux/input-event-codes.h> — these match the keymap xkbcommon
    // generates for "us" layout.
    let mut by_char = std::collections::HashMap::new();
    let mut by_name = std::collections::HashMap::new();

    // Letters: a-z. KEY_A = 30, KEY_B = 48, ..., KEY_Z = 44 (out of order
    // because the rows are a-s-d-f-g-h-j-k-l on the home row, but we
    // enumerate by the ASCII letter).
    let letters = [
        ('a', 30), ('b', 48), ('c', 46), ('d', 32), ('e', 18),
        ('f', 33), ('g', 34), ('h', 35), ('i', 23), ('j', 36),
        ('k', 37), ('l', 38), ('m', 50), ('n', 49), ('o', 24),
        ('p', 25), ('q', 16), ('r', 19), ('s', 31), ('t', 20),
        ('u', 22), ('v', 47), ('w', 17), ('x', 45), ('y', 21),
        ('z', 44),
    ];
    for (ch, kc) in letters {
        by_char.insert(ch, KeymapEntry { evdev_keycode: kc, shifted: false });
        // Capital letter — same keycode but with shift.
        let upper = ch.to_ascii_uppercase();
        by_char.insert(upper, KeymapEntry { evdev_keycode: kc, shifted: true });
    }

    // Top row digits + symbols. 1=2, 2=3, ..., 0=11.
    let digits = [
        ('1', 2),  ('2', 3),  ('3', 4),  ('4', 5),  ('5', 6),
        ('6', 7),  ('7', 8),  ('8', 9),  ('9', 10), ('0', 11),
    ];
    for (ch, kc) in digits {
        by_char.insert(ch, KeymapEntry { evdev_keycode: kc, shifted: false });
    }
    // Shifted digit row: ! @ # $ % ^ & * ( )
    let shifted_digits = [
        ('!', 2), ('@', 3), ('#', 4), ('$', 5), ('%', 6),
        ('^', 7), ('&', 8), ('*', 9), ('(', 10), (')', 11),
    ];
    for (ch, kc) in shifted_digits {
        by_char.insert(ch, KeymapEntry { evdev_keycode: kc, shifted: true });
    }

    // Punctuation / symbol keys (unshifted then shifted). Evdev codes:
    // - ` (12 grave),  - (12 minus), = (13 equal),
    // [ (26), ] (27), \ (43), ; (39), ' (40), , (51), . (52), / (53)
    let punct_unshifted = [
        ('-', 12), ('=', 13), ('[', 26), (']', 27), ('\\', 43),
        (';', 39), ('\'', 40), (',', 51), ('.', 52), ('/', 53),
        ('`', 41),
    ];
    for (ch, kc) in punct_unshifted {
        by_char.insert(ch, KeymapEntry { evdev_keycode: kc, shifted: false });
    }
    let punct_shifted = [
        ('_', 12), ('+', 13), ('{', 26), ('}', 27), ('|', 43),
        (':', 39), ('"', 40), ('<', 51), ('>', 52), ('?', 53),
        ('~', 41),
    ];
    for (ch, kc) in punct_shifted {
        by_char.insert(ch, KeymapEntry { evdev_keycode: kc, shifted: true });
    }

    // Whitespace and common control.
    by_char.insert(' ', KeymapEntry { evdev_keycode: 57, shifted: false });
    by_char.insert('\t', KeymapEntry { evdev_keycode: 15, shifted: false });
    by_char.insert('\n', KeymapEntry { evdev_keycode: 28, shifted: false });

    // Named keysym lookups (for press_keysym).
    let named = [
        ("BackSpace", 14),
        ("Return", 28),
        ("Enter", 28),
        ("Tab", 15),
        ("Escape", 1),
        ("Esc", 1),
        ("space", 57),
        ("Space", 57),
        ("Left", 105),
        ("Right", 106),
        ("Up", 103),
        ("Down", 108),
        ("Home", 102),
        ("End", 107),
        ("Page_Up", 104),
        ("Page_Down", 109),
        ("Delete", 111),
        ("F1", 59), ("F2", 60), ("F3", 61), ("F4", 62),
        ("F5", 63), ("F6", 64), ("F7", 65), ("F8", 66),
        ("F9", 67), ("F10", 68), ("F11", 87), ("F12", 88),
    ];
    for (name, kc) in named {
        by_name.insert(name.to_string(), KeymapEntry {
            evdev_keycode: kc,
            shifted: false,
        });
    }

    overlay_layout_chars(&keymap, &mut by_char);

    Ok(KeymapTable {
        keymap_text,
        by_char,
        by_name,
    })
}

/// (layout, variant) the compositor is using: COSMIC's `xkb_config`, else the
/// `XKB_DEFAULT_*` environment, else plain `us`.
fn seat_layout() -> (String, String) {
    let cfg = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config")))
        .map(|d| d.join("cosmic/com.system76.CosmicComp/v1/xkb_config"))
        .and_then(|p| std::fs::read_to_string(p).ok());
    if let Some((l, v)) = cfg.as_deref().and_then(layout_from_cosmic_config) {
        return (l, v);
    }
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    match env("XKB_DEFAULT_LAYOUT") {
        Some(l) => (l, env("XKB_DEFAULT_VARIANT").unwrap_or_default()),
        None => ("us".into(), String::new()),
    }
}

/// `layout: "us", variant: "dvorak",` out of COSMIC's RON-ish `xkb_config`. Only the
/// first layout of a comma list is used (the one a fresh session starts in).
fn layout_from_cosmic_config(text: &str) -> Option<(String, String)> {
    let field = |name: &str| -> Option<String> {
        let line = text.lines().find(|l| l.trim_start().starts_with(&format!("{name}:")))?;
        let v = line.split_once(':')?.1.trim().trim_end_matches(',').trim();
        let v = v.strip_prefix("Some(").and_then(|x| x.strip_suffix(')')).unwrap_or(v);
        Some(v.trim_matches('"').split(',').next().unwrap_or("").trim().to_string())
    };
    let layout = field("layout").filter(|l| !l.is_empty())?;
    Some((layout, field("variant").unwrap_or_default()))
}

/// Replace the US-guess entries with what `keymap` really produces: for every evdev
/// key, the character it types plain and with Shift. Plain wins over shifted, the
/// lower keycode wins a tie (so `a` is never found on a numpad key).
fn overlay_layout_chars(keymap: &xkbcommon::xkb::Keymap, by_char: &mut std::collections::HashMap<char, KeymapEntry>) {
    use xkbcommon::xkb;
    let shift_bit = 1u32 << keymap.mod_get_index("Shift");
    let mut found: std::collections::HashMap<char, KeymapEntry> = std::collections::HashMap::new();
    for evdev in 1u32..=88 {
        for shifted in [false, true] {
            let mut st = xkb::State::new(keymap);
            if shifted {
                st.update_mask(shift_bit, 0, 0, 0, 0, 0);
            }
            let Some(c) = char::from_u32(st.key_get_utf32(xkb::Keycode::new(evdev + 8))) else { continue };
            if c.is_control() || c == '\0' {
                continue;
            }
            let better = match found.get(&c) {
                None => true,
                Some(old) => (old.shifted && !shifted) || (old.shifted == shifted && evdev < old.evdev_keycode),
            };
            if better {
                found.insert(c, KeymapEntry { evdev_keycode: evdev, shifted });
            }
        }
    }
    // The control characters keep their US entries (Tab, Enter): same keys everywhere.
    by_char.extend(found);
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn table(layout: &str, variant: &str) -> std::collections::HashMap<char, KeymapEntry> {
        use xkbcommon::xkb;
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let km = xkb::Keymap::new_from_names(&ctx, "", "", layout, variant, None, xkb::COMPILE_NO_FLAGS).unwrap();
        let mut m = std::collections::HashMap::new();
        overlay_layout_chars(&km, &mut m);
        m
    }

    #[test]
    fn dvorak_finds_c_where_qwerty_has_i_and_us_is_unchanged() {
        let dv = table("us", "dvorak");
        assert_eq!(dv[&'c'].evdev_keycode, 23, "dvorak c sits on the QWERTY i key");
        assert_eq!(dv[&'i'].evdev_keycode, 34, "and i on the QWERTY g key");
        assert_eq!(dv[&'s'].evdev_keycode, 39, "s on the QWERTY ; key");
        assert!(dv[&'C'].shifted && !dv[&'c'].shifted);
        let us = table("us", "");
        assert_eq!((us[&'c'].evdev_keycode, us[&'a'].evdev_keycode, us[&'i'].evdev_keycode), (46, 30, 23));
        assert!(us.len() > 90, "the scan found a whole layout: {}", us.len());
    }

    #[test]
    fn cosmic_config_text_yields_layout_and_variant() {
        let t = "(\n    rules: \"\",\n    layout: \"us\",\n    variant: \"dvorak\",\n    options: None,\n)";
        assert_eq!(layout_from_cosmic_config(t), Some(("us".into(), "dvorak".into())));
        assert_eq!(layout_from_cosmic_config("(layout: \"\")"), None);
        let t2 = "(\n layout: \"us,de\",\n variant: \",nodeadkeys\",\n)";
        assert_eq!(layout_from_cosmic_config(t2), Some(("us".into(), String::new())));
    }
}

/// Send the keymap to the compositor via the virtual_keyboard's
/// `keymap()` request. The protocol takes a memfd containing the keymap
/// text, plus its size.
fn send_keymap(vkbd: &ZwpVirtualKeyboardV1, keymap: &KeymapTable) -> Result<()> {
    use std::io::Write;
    let bytes = keymap.keymap_text.as_bytes();
    let mut mfd = memfd_create("rusty-board-keymap")?;
    mfd.write_all(bytes)?;
    mfd.write_all(b"\0")?; // protocol expects NUL-terminated

    // KEYMAP_FORMAT_XKB_V1 = 1 in the xkb keymap format enum.
    const KEYMAP_FORMAT_XKB_V1: u32 = 1;
    vkbd.keymap(KEYMAP_FORMAT_XKB_V1, mfd.as_fd(), (bytes.len() + 1) as u32);
    Ok(())
}

/// Create an anonymous file via memfd_create syscall. We can't use a
/// regular tempfile because the compositor may keep the fd around past
/// our process exit, and we don't want the file lingering on disk.
#[cfg(target_os = "linux")]
fn memfd_create(name: &str) -> Result<std::fs::File> {
    use std::ffi::CString;
    use std::os::unix::io::FromRawFd;

    let cname = CString::new(name)?;
    // Linux memfd_create syscall — libc provides it.
    let fd = unsafe { libc::memfd_create(cname.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        bail!("memfd_create failed: {}", std::io::Error::last_os_error());
    }
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

/// Outside Linux there is no memfd_create (libc has neither it nor MFD_CLOEXEC:
/// a macOS build of rusty-board-egui with `wayland` stopped here). The same
/// anonymous, fd-only file: a temporary file UNLINKED as soon as it is open,
/// so nothing lingers on disk however long the compositor keeps the fd.
#[cfg(not(target_os = "linux"))]
fn memfd_create(name: &str) -> Result<std::fs::File> {
    unlinked_tempfile(name)
}

/// Compiled everywhere (used off Linux) so it is tested here.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn unlinked_tempfile(name: &str) -> Result<std::fs::File> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let path = std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()));
    let f = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path)?;
    std::fs::remove_file(&path)?;
    Ok(f)
}

// ---- GLib integration ----------------------------------------------------
//
// Important note about how event dispatch works when sharing GDK's
// connection: GDK's main-loop integration is the thing that actually
// `read()`s from the Wayland fd. Events arriving for objects on *our*
// `EventQueue` are sorted into our queue's internal buffer by the
// wayland-client backend; we just need to call `dispatch_pending` to
// consume them. So we don't need to register the fd with GLib ourselves
// — that would create a second reader and risk fighting GDK over the
// `prepare_read` lock. Instead we drain on a low-frequency timeout,
// which is plenty for input-method events (they fire on focus changes,
// not at refresh rate).

/// One dispatcher step: read the socket if it can be read now, dispatch
/// the buffered events for our queue, flush the backstop. Shared by the
/// GLib timer and `WaylandBackend::pump`, so the two hosts cannot drift in
/// what "processing events" means.
///
/// Returns `Err` only for a dead connection (`read` or `dispatch_pending`
/// failing for a reason other than WouldBlock); a caller on a timer should
/// stop re-arming on `Err`.
fn dispatch_once(conn: &Connection, inner: &Arc<Mutex<BackendInner>>) -> Result<usize> {
    // When the backend already has events buffered, prepare_read()
    // returns None (nothing to read yet — just dispatch). When the socket
    // may have new bytes for us, it returns Some(guard) and we read. With
    // our own connection nothing else reads the socket, so we must; with
    // GDK's shared connection its poll loop is already reading and
    // prepare_read usually returns None.
    if let Some(guard) = conn.prepare_read() {
        use wayland_client::backend::WaylandError;
        match guard.read() {
            Ok(_) => {}
            Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => bail!("wl conn.read failed: {}", e),
        }
    }
    let dispatched = {
        let mut guard = inner.lock();
        let BackendInner { queue, app_data, .. } = &mut *guard;
        queue
            .dispatch_pending(app_data)
            .map_err(|e| anyhow!("queue.dispatch_pending failed: {}", e))?
    };
    // Backstop flush — most outgoing paths already flush themselves, so
    // this is normally a no-op. Cheap to do unconditionally.
    let _ = conn.flush();
    Ok(dispatched)
}

#[cfg(feature = "gdk-integration")]
fn install_glib_dispatcher(conn: Connection, inner: Arc<Mutex<BackendInner>>) {
    // Self-adaptive cadence. The dispatcher serves two jobs:
    //
    //   1. Read incoming wayland events (focus changes, surrounding-text
    //      updates) and dispatch them to handlers.
    //   2. Flush our outgoing requests as a backstop.
    //
    // Job 2 is mostly redundant: every outgoing path that matters
    // (commit_text, press_keysym, modifier toggles, im commits)
    // already calls conn.flush() itself after queuing — see the four
    // explicit flush calls in commit_text / press_keysym. So we only
    // need the periodic flush as a backstop for protocol-level acks.
    //
    // That means we can back off aggressively when no events have
    // arrived. dispatch_pending returns the count of events
    // dispatched on this tick; > 0 = something happened, snap to
    // FAST_MS. Otherwise double the interval, capped at IDLE_MS, so
    // worst-case latency from idle to picking up a focus event is
    // one IDLE_MS — 320ms here, well under the threshold of feeling
    // sluggish.
    //
    // On the FP5 the previous fixed 80ms timer was ~12 wakeups/sec
    // forever; this pattern drops idle to ~3 wakeups/sec while
    // keeping responsiveness during active focus traffic.
    const FAST_MS: u64 = 40;
    const IDLE_MS: u64 = 320;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    let interval_slot: Rc<Cell<u64>> = Rc::new(Cell::new(IDLE_MS));
    type Rearm = Rc<dyn Fn()>;
    let rearm_slot: Rc<RefCell<Option<Rearm>>> = Rc::new(RefCell::new(None));
    let rearm_inner = rearm_slot.clone();

    let tick: Rearm = Rc::new(move || {
        // "Active" = something was read or dispatched this tick; the
        // adaptive cadence below snaps to FAST_MS on activity. A `None`
        // from prepare_read (events already buffered) counts as activity,
        // which dispatch_once folds into its dispatched count.
        let dispatched = match dispatch_once(&conn, &inner) {
            Ok(n) => n,
            Err(e) => {
                warn!("{}", e);
                return;
            }
        };
        let read_yielded = dispatched > 0;
        let active = read_yielded || dispatched > 0;
        let next = if active {
            FAST_MS
        } else {
            (interval_slot.get().saturating_mul(2)).min(IDLE_MS)
        };
        interval_slot.set(next);
        if let Some(rearm) = rearm_inner.borrow().as_ref() {
            let rearm = rearm.clone();
            glib::timeout_add_local_once(
                std::time::Duration::from_millis(next),
                move || rearm(),
            );
        }
    });
    *rearm_slot.borrow_mut() = Some(tick.clone());
    tick();
}

// ---- Misc ---------------------------------------------------------------

fn monotonic_ms() -> u32 {
    use std::time::Instant;
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_millis() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_builds_with_basic_chars() {
        // We can't test in a sandbox without xkb data files, but we can
        // verify the helper at least returns SOMETHING when xkbcommon is
        // working. In CI environments without /usr/share/X11/xkb this
        // will fail, which is expected — gate on env.
        if std::env::var("RUSTYBOARD_TEST_XKB").is_err() {
            return;
        }
        let table = build_us_keymap().expect("keymap build");
        assert!(table.lookup_char('a').is_some());
        assert!(table.lookup_char('A').map(|e| e.shifted).unwrap_or(false));
        assert!(table.lookup_char(' ').is_some());
    }
}

#[cfg(test)]
mod unlinked_tempfile_tests {
    use super::unlinked_tempfile;
    use std::io::{Read, Seek, SeekFrom, Write};

    /// The off-Linux keymap buffer: readable and writable through the fd, and
    /// nothing left on disk.
    #[test]
    fn the_keymap_buffer_lives_only_in_its_fd() {
        let name = "rusty-board-keymap-test";
        let mut f = unlinked_tempfile(name).unwrap();
        f.write_all(b"xkb_keymap { };").unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();
        let mut back = String::new();
        f.read_to_string(&mut back).unwrap();
        assert_eq!(back, "xkb_keymap { };");
        let left: Vec<_> = std::fs::read_dir(std::env::temp_dir()).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(&format!("{name}-{}", std::process::id())))
            .collect();
        assert!(left.is_empty(), "unlinked at once: {left:?}");
    }
}

