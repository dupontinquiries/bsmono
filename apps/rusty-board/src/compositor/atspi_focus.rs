//! AT-SPI-based focus tracking, for desktop sessions where we fell back
//! to the `X11Xdotool` (or `Noop`) virtual-keyboard backend.
//!
//! ## Why this exists
//!
//! `zwp_input_method_v1`/`v2` give us Maliit-style "show the keyboard
//! when a text field gets focus" behavior for free on Phosh/Sway/Plasma
//! Mobile — see `wayland_backend.rs`. Plain X11 desktops (GNOME on
//! Xorg, most KDE Plasma *desktop* sessions, older/NVIDIA setups that
//! stay on Xorg) have no equivalent compositor protocol, and previously
//! `build_vkbd_with_im_callback` just returned the bare `xdotool`
//! backend with none of the focus-driven show/hide, tray-mode, or
//! per-focus housekeeping (steno buffer clear, undo-history clear,
//! surrounding-text tracking) that the Wayland path gets.
//!
//! AT-SPI (the accessibility bus every GTK/Qt app already publishes
//! to, for screen readers) is the one desktop-wide signal that fires
//! when a text field is focused or the caret moves inside one,
//! regardless of window manager. It's the same mechanism screen
//! readers and older on-screen keyboards (onboard, florence) used for
//! auto-show on GNOME/KDE X11.
//!
//! ## What we actually watch
//!
//! Two AT-SPI object events, straight off the bus, no explicit
//! `org.a11y.atspi.Registry.RegisterEvent` call:
//!
//! - `TextCaretMoved` — fires only on objects that implement the
//!   `EditableText`/`Text` interfaces, i.e. an actual text field. This
//!   is our "show" signal; it's a much better one than plain
//!   `StateChanged:focused`, which also fires for buttons, list items,
//!   etc. and would pop the keyboard up constantly.
//! - `StateChanged` with `state == "focused"` — used only for the
//!   *hide* half, mirroring the debounce the Wayland v1 path already
//!   needs (see `wayland_backend.rs`'s SurroundingText-confirmation
//!   comment): a raw focus-loss doesn't hide immediately, because
//!   focus commonly bounces briefly between two editable widgets (tab
//!   order, a popup taking focus mid-click) and a naive immediate hide
//!   flickers the keyboard off and back on for every such hop.
//!
//! ## Known gap
//!
//! Some AT-SPI providers only forward events to clients that have
//! called `org.a11y.atspi.Registry.RegisterEvent` first rather than
//! broadcasting unconditionally to bus subscribers. If auto-show turns
//! out not to fire on a given desktop, that registration call is the
//! first thing to add — this file deliberately skips it to keep the
//! first version small, on the assumption (unverified across desktops)
//! that a plain match rule is enough on GNOME/KDE's current AT-SPI
//! bus.
//!
//! ## NOT compiled or run against a live AT-SPI bus
//!
//! This is written against my best recollection of zbus 4.x's async
//! API and the AT-SPI D-Bus wire format (`StateChanged`'s body is
//! `(String, i32, i32, Value)` — state name, detail1, detail2, an
//! unused reserved variant). I have not built this against the repo's
//! actual pinned zbus version or exercised it against a real
//! `org.a11y.Bus`. Before relying on it: `cargo build --features
//! dbus`, then check with `busctl --user monitor` (or `dbus-monitor
//! --session`, filtering the a11y bus) that `TextCaretMoved` /
//! `StateChanged` are actually arriving while this thread runs, and
//! adjust the zbus call names (`add_match_rule`, `MessageStream`,
//! `body().deserialize()`) if they've moved between zbus releases.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use tracing::{debug, warn};

use crate::compositor::wayland_backend::InputMethodActiveCallback;

/// How long to wait after an apparent focus-loss before actually
/// hiding, in case focus lands on another editable field immediately
/// after (tab order, a popup grabbing focus mid-click, etc).
const HIDE_DEBOUNCE: Duration = Duration::from_millis(300);

/// Spawn the AT-SPI watcher thread and wire its events into `cb`
/// (the same `InputMethodActiveCallback` the Wayland input-method
/// path uses), so X11-desktop sessions get the same show/hide +
/// per-focus housekeeping behavior.
///
/// `cb` is `Rc<dyn Fn(bool)>` — not `Send` — so it can only run on the
/// GTK main thread. The AT-SPI listener runs on its own thread (it
/// needs to block on the D-Bus connection) and hands events across
/// via a plain `std::sync::mpsc` channel; a `glib` timeout source on
/// the main thread drains that channel and calls `cb` directly.
pub fn spawn_atspi_focus_watcher(cb: InputMethodActiveCallback) {
    let (tx, rx) = std_mpsc::channel::<bool>();

    if let Err(e) = std::thread::Builder::new()
        .name("atspi-focus".into())
        .spawn(move || {
            if let Err(e) = run_watcher(&tx) {
                warn!("AT-SPI focus watcher exited: {e:#}");
            }
        })
    {
        warn!("could not spawn AT-SPI focus watcher thread: {e}");
        return;
    }

    // GTK-main-thread side: last-writer-wins for shows, debounced for
    // hides, same shape as the Wayland v1 defer-and-check logic.
    let pending_hide_since: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    let currently_shown = Rc::new(Cell::new(false));

    glib::source::timeout_add_local(Duration::from_millis(50), move || {
        let mut saw_show = false;
        let mut saw_hide = false;
        while let Ok(active) = rx.try_recv() {
            if active {
                saw_show = true;
            } else {
                saw_hide = true;
            }
        }

        if saw_show {
            pending_hide_since.set(None);
            if !currently_shown.get() {
                currently_shown.set(true);
                cb(true);
            }
        } else if saw_hide && currently_shown.get() && pending_hide_since.get().is_none() {
            pending_hide_since.set(Some(Instant::now()));
        }

        if let Some(since) = pending_hide_since.get() {
            if since.elapsed() >= HIDE_DEBOUNCE {
                pending_hide_since.set(None);
                if currently_shown.get() {
                    currently_shown.set(false);
                    cb(false);
                }
            }
        }

        glib::ControlFlow::Continue
    });
}

fn run_watcher(tx: &std_mpsc::Sender<bool>) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_watcher_async(tx))
}

async fn run_watcher_async(tx: &std_mpsc::Sender<bool>) -> anyhow::Result<()> {
    use futures::stream::StreamExt;

    // The accessibility bus is a separate D-Bus connection whose
    // address the session bus hands out on request — it isn't the
    // session bus itself.
    let session = zbus::Connection::session().await?;
    let reply = session
        .call_method(
            Some("org.a11y.Bus"),
            "/org/a11y/bus",
            Some("org.a11y.Bus"),
            "GetAddress",
            &(),
        )
        .await?;
    let address: String = reply.body().deserialize()?;

    let a11y = zbus::connection::Builder::address(address.as_str())?
        .build()
        .await?;

    let dbus_proxy = zbus::fdo::DBusProxy::new(&a11y).await?;
    dbus_proxy
        .add_match_rule(
            zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface("org.a11y.atspi.Event.Object")?
                .build(),
        )
        .await?;

    let mut stream = zbus::MessageStream::from(&a11y);
    while let Some(msg) = stream.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                debug!("AT-SPI message stream error: {e}");
                continue;
            }
        };
        let header = msg.header();
        let Some(member) = header.member().map(|m| m.as_str()) else {
            continue;
        };
        match member {
            // Only fires on Text/EditableText-interface objects, i.e.
            // an actual editable field got a cursor placed in it —
            // the show signal.
            "TextCaretMoved" => {
                let _ = tx.send(true);
            }
            // detail1: 1 == gained focus, 0 == lost focus. Losing
            // focus is only a *candidate* hide; the main-thread poller
            // debounces it in case focus lands on another field.
            "StateChanged" => {
                if let Ok((state, detail1, _detail2, _reserved)) = msg
                    .body()
                    .deserialize::<(String, i32, i32, zbus::zvariant::Value)>()
                {
                    if state == "focused" {
                        let _ = tx.send(detail1 == 1);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}
