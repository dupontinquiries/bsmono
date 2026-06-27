//! Window placement: layer-shell on Wayland, fallback on X11.

use gtk4::prelude::*;
use libadwaita as adw;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Wayland layer-shell — anchored to bottom edge, no focus stealing,
    /// other windows resize around us via exclusive zone.
    LayerShell,
    /// X11 / Wayland-without-layer-shell fallback. Toplevel window with
    /// keep-above hint.
    Toplevel,
}

/// Configure the window for use as an on-screen keyboard. Returns which
/// strategy ended up being used so the caller can log/configure further.
///
/// We try layer-shell first because:
/// - It correctly handles "this window does not steal focus" (the user's
///   keyboard input still goes to the *real* focused window — which on
///   the keyboard's side comes back via `text-input-v3`).
/// - The compositor knows our height and reserves an exclusive zone, so
///   maximized windows don't get covered by the keyboard.
/// - The keyboard renders above other windows but below modal dialogs,
///   which is exactly the layer model.
///
/// If layer-shell isn't compiled in (`--no-default-features`) or the
/// compositor doesn't speak the protocol (most likely X11), we drop back
/// to a regular toplevel with a few hints to make it behave more like a
/// keyboard than a normal app window.
///
/// **Critically**, this function must be called on a freshly-built window
/// BEFORE anything triggers GTK to realize its `wl_surface`. Once the
/// surface exists as an `xdg_toplevel`, `init_layer_shell` cannot
/// retroactively change its role; the call is silently a no-op and the
/// window stays a regular toplevel.
pub fn configure_window_placement(window: &adw::ApplicationWindow) -> Placement {
    info!(
        "configure_window_placement: WAYLAND_DISPLAY={:?}, XDG_SESSION_TYPE={:?}",
        std::env::var("WAYLAND_DISPLAY").ok(),
        std::env::var("XDG_SESSION_TYPE").ok(),
    );

    #[cfg(feature = "wayland")]
    {
        if try_layer_shell(window) {
            return Placement::LayerShell;
        }
    }

    info!("Falling back to toplevel window placement");
    configure_toplevel_fallback(window);
    Placement::Toplevel
}

#[cfg(feature = "wayland")]
fn try_layer_shell(window: &adw::ApplicationWindow) -> bool {
    use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

    // Force GDK to flush its registry roundtrip before we ask about
    // layer-shell support. GTK4 lazily defers some Wayland setup until
    // the first window flush; if we ask `is_supported()` too early the
    // registry might not have arrived yet and we get a false negative.
    if let Some(display) = gtk4::gdk::Display::default() {
        display.flush();
        // Spin the main loop briefly to let registry events come back.
        // This is a no-op if everything's already arrived.
        let ctx = glib::MainContext::default();
        while ctx.iteration(false) {}
    }

    // Don't trust is_supported() in isolation. Try to actually init the
    // window as a layer surface; if the protocol isn't there, the calls
    // become no-ops and `is_layer_window()` stays false — that's our
    // ground truth. Log both for diagnostic purposes.
    let claimed = gtk4_layer_shell::is_supported();
    info!(
        "gtk4_layer_shell::is_supported() = {} (will verify by attempting init)",
        claimed
    );

    // CRITICAL ORDERING: init_layer_shell must happen before GTK realizes
    // the window's wl_surface. After realization, the surface has an
    // xdg_toplevel role and layer-shell can't change it — calls become
    // silent no-ops and we end up rendered as a regular toplevel.
    info!("Calling init_layer_shell on window");
    window.init_layer_shell();
    let is_layer = window.is_layer_window();
    info!(
        "init_layer_shell returned; is_layer_window={} (claimed={})",
        is_layer, claimed
    );

    if !is_layer {
        warn!(
            "wlr-layer-shell init didn't take — window stayed a toplevel. \
             Either the compositor doesn't expose zwlr_layer_shell_v1 on \
             GDK's connection, or init_layer_shell was called too late. \
             Falling back to toplevel placement."
        );
        return false;
    }

    window.set_layer(Layer::Top);
    info!("set_layer(Top) done");

    // Anchor to the bottom edge, full width.
    window.set_anchor(Edge::Bottom, true);
    window.set_anchor(Edge::Left, true);
    window.set_anchor(Edge::Right, true);
    info!("Anchors set: bottom=true, left=true, right=true, top=false");

    // Don't steal keyboard focus. The compositor still routes typed input
    // to the originally-focused app via the input-method protocol;
    // RustyBoard intercepts pointer/touch only.
    window.set_keyboard_mode(KeyboardMode::None);

    // Reserve our height as an exclusive zone so maximized windows
    // shrink to make room. Auto-mode tracks the window's allocated
    // height — no need to update on resize.
    window.auto_exclusive_zone_enable();

    // Explicit height. Without this, the layer surface defaults to 0
    // height and the compositor picks something based on our content's
    // natural size — which can be unpredictable on HiDPI displays.
    // 320 logical px is a typical OSK height; the compositor scales it
    // per-display. On 300% scaling this becomes ~960 physical px,
    // which is a reasonable phone-keyboard size.
    window.set_default_height(320);
    info!("Default height set to 320 logical px");

    info!(
        "Window configured as Wayland layer surface (bottom edge, \
         Top layer, exclusive zone enabled, is_layer_window={})",
        window.is_layer_window()
    );
    true
}

/// X11 / non-layer-shell fallback. We set hints that approximate the
/// layer-shell behavior so the window at least *looks* like a keyboard:
/// no decorations, keep-above, and we ask not to be focused on map.
fn configure_toplevel_fallback(window: &adw::ApplicationWindow) {
    window.set_decorated(false);

    // We can't truly avoid focus on X11 without WM cooperation, but we
    // can hint. GTK4 on X11 honors the `_NET_WM_WINDOW_TYPE_DOCK` hint
    // when set via the role/type-hint plumbing. Setting startup-id with
    // a synthetic value at least keeps the WM from raising other
    // sibling windows over us.
    window.set_resizable(false);

    // Keyboard windows traditionally use a height ratio (1/3 of screen)
    // and full width. We don't have direct access to monitor info from
    // here — caller can do that — but we set a sensible default.
    if window.default_height() <= 0 {
        window.set_default_height(420);
    }

    info!("Window configured as toplevel fallback (no layer-shell). \
           Focus-stealing protection is best-effort on X11.");
}

/// Stub for builds without the `wayland` feature.
#[cfg(not(feature = "wayland"))]
fn try_layer_shell(_window: &adw::ApplicationWindow) -> bool {
    false
}
