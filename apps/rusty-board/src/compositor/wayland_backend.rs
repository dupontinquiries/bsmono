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
    /// Input-method-v2 object. Kept alive for backend lifetime.
    #[allow(dead_code)]
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

    // We need a wl_seat for input-method/virtual-keyboard creation.
    // Walk the registry on a throwaway queue, bind the first seat.
    let mut seat_queue: EventQueue<SeatProbe> = conn.new_event_queue();
    let qh = seat_queue.handle();
    let _registry = conn.display().get_registry(&qh, ());
    let mut probe = SeatProbe { seat: None };
    seat_queue
        .roundtrip(&mut probe)
        .map_err(|e| anyhow!("seat probe roundtrip failed: {}", e))?;

    let seat = probe.seat.ok_or_else(|| {
        anyhow!(
            "inherited Wayland connection didn't expose wl_seat — this \
             socket may not be a regular Wayland connection"
        )
    })?;
    Ok((conn, seat))
}

/// Strategy 2: share GDK's existing Wayland connection. Only viable when
/// the same socket exposes both rendering protocols (for GDK) AND the
/// input-method protocols (for us). True for wlroots-style compositors;
/// false for KWin (where input-method protocols live on a separate
/// privileged socket).
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

/// Minimal registry probe used during `open_inherited_connection` to find
/// a `wl_seat` on the inherited connection. Throwaway.
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
    let inherited_fd = crate::compositor::im_socket::take_inherited_fd();

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

    // 8. Hook the connection's fd into the GTK main loop so events get
    //    dispatched without a polling thread.
    install_glib_dispatcher(conn, inner.clone());

    Ok(Some(WaylandBackend { inner }))
}

impl WaylandBackend {
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
                let vkbd = inner
                    .vkbd
                    .as_ref()
                    .ok_or_else(|| anyhow!("no virtual keyboard (v2 mode)"))?;
                let entry = inner
                    .keymap
                    .lookup_keysym_name(base_name)
                    .ok_or_else(|| anyhow!("unknown keysym {:?}", base_name))?;
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
                let sym = xkb_keysym_from_name(base_name)
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
                if state.pending_active != state.active {
                    state.active = state.pending_active;
                    debug!("input-method state -> active={}", state.active);
                    (state.im_callback)(state.active);
                }
            }
            zwp_input_method_v2::Event::Unavailable => {
                warn!(
                    "input-method-v2 reported Unavailable — another OSK \
                     probably bound it first; show/hide won't work this \
                     session"
                );
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
                crate::ui::window::set_content_purpose(raw.min(255) as u8);
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
                crate::ui::window::set_content_purpose(purpose.min(255) as u8);
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

    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let keymap = xkb::Keymap::new_from_names(
        &context,
        "",     // rules: default
        "",     // model: default
        "us",   // layout
        "",     // variant
        None,   // options
        xkb::COMPILE_NO_FLAGS,
    )
    .ok_or_else(|| anyhow!("xkb: failed to compile 'us' keymap"))?;

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

    Ok(KeymapTable {
        keymap_text,
        by_char,
        by_name,
    })
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
        // Try to read any pending events from the socket. When the
        // backend already has events buffered, prepare_read() returns
        // None (we don't need to read yet — just dispatch). When the
        // socket may have new bytes for us, it returns Some(guard) and
        // we read.
        //
        // With our own connection (inherited fd), nothing else reads
        // the socket, so we must. With GDK's shared connection, GDK's
        // own poll loop is already doing the reading; prepare_read
        // will usually return None.
        let mut read_yielded = false;
        if let Some(guard) = conn.prepare_read() {
            use wayland_client::backend::WaylandError;
            match guard.read() {
                Ok(_) => {
                    read_yielded = true;
                }
                Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No new events — fall through to dispatch any
                    // already-buffered ones.
                }
                Err(e) => {
                    warn!("wl conn.read failed: {}", e);
                    return;
                }
            }
        } else {
            // None means events are already buffered in the queue —
            // count that as activity so we keep polling fast.
            read_yielded = true;
        }

        let dispatched = {
            let mut guard = inner.lock();
            let BackendInner {
                queue, app_data, ..
            } = &mut *guard;
            match queue.dispatch_pending(app_data) {
                Ok(n) => n,
                Err(e) => {
                    warn!("queue.dispatch_pending failed: {}", e);
                    return;
                }
            }
        };
        // Backstop flush — most outgoing paths already flush themselves,
        // so this is normally a no-op. Cheap to do unconditionally; not
        // worth tracking outgoing state to skip it.
        let _ = conn.flush();

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
