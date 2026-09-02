//! platform — what the host window system is, and what it can do for an on-screen keyboard.
//!
//! ## The seam
//!
//! An on-screen keyboard has three platform-shaped obligations that an ordinary app does not:
//!
//! 1. **Do not take focus.** The text goes to whoever HAD focus; a keyboard that gets focus on
//!    click types into itself.
//! 2. **Sit at the bottom edge and make room.** Other windows should resize around it
//!    (Wayland: a layer surface with an exclusive zone; X11: a strut).
//! 3. **Appear when a field is focused, vanish when it is not** — Maliit's behaviour, driven by
//!    the compositor's Activate/Deactivate (see `wayland.rs`), never by a timer.
//!
//! Which of those the host can grant, and how, differs by compositor and by shell toolkit —
//! and it is going to keep differing: Phosh and Sway speak wlr-layer-shell, KWin speaks it too
//! but also has window rules, a future X11 or COSMIC host is different again, and eframe/winit
//! can do (1) and (3) but not (2). This module is the one place that decision is made, as data
//! ([`Placement`]) that the shell reads, so that adding a host means adding an arm here rather
//! than another `if cfg!` in the screen code.
//!
//! ## What this module deliberately does NOT do
//!
//! It does not animate the WINDOW. A keyboard that slides its window in from the bottom
//! edge makes the compositor re-layout every other window on every frame of the slide, and
//! on a tiling compositor that is a visible stutter across the whole screen, for a cosmetic
//! effect. The show/hide is one map/unmap; the motion lives INSIDE the board
//! ([`reveal_progress`] — keys fade and settle within a rectangle whose size never changes),
//! which costs the compositor one relayout per show, not one per frame.
//!
//! Nothing here touches a display: every function is pure over the environment or a struct,
//! and the tests run headless.

/// Which display protocol the process was started under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    /// `$WAYLAND_DISPLAY` set, or a `WAYLAND_SOCKET` inherited from the compositor.
    Wayland,
    /// `$DISPLAY` set and no Wayland variables.
    X11,
    /// Neither. Headless test, or a misconfigured launcher.
    Unknown,
}

/// Read the session from the environment. `WAYLAND_SOCKET` counts: KWin launches its
/// configured virtual keyboard with that alone on some setups, and a keyboard that sees no
/// `WAYLAND_DISPLAY` and concludes "X11" would then reach for xdotool on a Wayland session.
pub fn session() -> Session {
    session_from(
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        std::env::var_os("WAYLAND_SOCKET").is_some() || inherited_socket_captured(),
        std::env::var_os("DISPLAY").is_some(),
    )
}

/// Pure core of [`session`], so the precedence is testable.
pub fn session_from(wayland_display: bool, wayland_socket: bool, x11_display: bool) -> Session {
    if wayland_display || wayland_socket {
        Session::Wayland
    } else if x11_display {
        Session::X11
    } else {
        Session::Unknown
    }
}

#[cfg(feature = "wayland")]
fn inherited_socket_captured() -> bool {
    board_compositor::im_socket::has_inherited_fd()
}
#[cfg(not(feature = "wayland"))]
fn inherited_socket_captured() -> bool {
    false
}

/// Capture KWin's privileged input-method socket BEFORE any toolkit connects to Wayland.
///
/// KWin (Plasma Mobile and Plasma desktop) starts the configured virtual keyboard with
/// `WAYLAND_SOCKET=<fd>`: a second, privileged connection on which `zwp_input_method_v1/v2`
/// and `zwp_virtual_keyboard_v1` are exposed. Two things go wrong if this is left in the
/// environment when winit starts:
///
/// - `wayland-client`'s `connect_to_env` PREFERS `WAYLAND_SOCKET` over `WAYLAND_DISPLAY`, so
///   the RENDERING connection would be made on the privileged socket, and the public
///   `$WAYLAND_DISPLAY` socket (where `wl_compositor` and the shell protocols live) would
///   never be opened;
/// - libwayland-style connect unsets `WAYLAND_SOCKET` after looking at it, so by the time the
///   input-method backend runs, the fd number is gone.
///
/// So: parse it, stash it for `board_compositor::wayland_backend` (which opens its own
/// connection from the fd), and remove it from the environment so the toolkit connects to
/// the public socket. This is what the GTK binary's `main.rs` does; the egui binary did not,
/// which is one reason an eframe build launched by KWin fell back to preview.
///
/// Returns whether a socket was captured. Call it first thing in `main`, before
/// `eframe::run_native`.
pub fn capture_inherited_socket() -> bool {
    let Some(sock) = std::env::var_os("WAYLAND_SOCKET") else { return false };
    let parsed = sock.to_str().and_then(|s| s.trim().parse::<i32>().ok());
    // SAFETY: called from `main` before any other thread exists (the toolkit has not been
    // started yet), which is the precondition `remove_var` needs on Unix.
    unsafe { std::env::remove_var("WAYLAND_SOCKET") };
    match parsed {
        Some(fd) => {
            stash_socket(fd);
            true
        }
        None => {
            eprintln!("[rusty-board] WAYLAND_SOCKET={sock:?} is not an fd number; ignoring");
            false
        }
    }
}

#[cfg(feature = "wayland")]
fn stash_socket(fd: i32) {
    board_compositor::im_socket::set_inherited_fd(fd);
}
#[cfg(not(feature = "wayland"))]
fn stash_socket(fd: i32) {
    // No backend to hand it to in this build. Say so, because a keyboard started by KWin with
    // a build that cannot use the socket will silently type into its own preview.
    eprintln!("[rusty-board] KWin passed input-method fd {fd}, but this build has no `wayland` feature — preview only");
}

/// How the keyboard window is placed and hidden on this host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// A `zwlr_layer_shell_v1` surface: bottom anchor, exclusive zone = keyboard height,
    /// keyboard-interactivity none. The compositor does all three obligations. This is what
    /// the GTK build has via gtk4-layer-shell and what Phosh/Sway/KWin all honour.
    ///
    /// Not reachable from the eframe shell (winit gives its surface the xdg_toplevel role and
    /// a surface has exactly one role); `shell::layer` (feature `layer`) is the shell that
    /// speaks the protocol directly and constructs the screen with this placement.
    LayerShell,
    /// A normal toplevel that a window-manager RULE keeps unfocusable, above other windows and
    /// at the bottom edge. On KWin `scripts/plasma-setup.sh` writes that rule for the app id.
    /// Show/hide is a map/unmap of a window whose size never changes.
    RuledToplevel,
    /// A plain window, for development on a desktop with no rule installed. Clicking it will
    /// focus it on most window managers; the preview sink is the honest mode here.
    Floating,
}

impl Placement {
    /// One line for the status strip, so a person can tell which of the three obligations the
    /// host is actually meeting rather than guessing from behaviour.
    pub fn describe(self) -> &'static str {
        match self {
            Placement::LayerShell => "layer-shell (docked, exclusive zone, no focus)",
            Placement::RuledToplevel => "toplevel + window rule (on top, no focus, bottom edge)",
            Placement::Floating => "floating window (may take focus — dev mode)",
        }
    }
}

/// Decide the placement for the EFRAME shell (the layer shell does not ask; it knows). eframe
/// cannot do layer-shell itself, so on Wayland the best it can offer is a ruled toplevel — and
/// whether the rule exists is the window manager's knowledge, not ours, so the env override is
/// how a person tells us.
///
/// `RUSTY_BOARD_PLACEMENT=floating|ruled` overrides for testing.
pub fn placement() -> Placement {
    placement_from(session(), std::env::var("RUSTY_BOARD_PLACEMENT").ok().as_deref())
}

/// Pure core of [`placement`].
pub fn placement_from(session: Session, override_: Option<&str>) -> Placement {
    match override_.map(str::trim) {
        Some("floating") => return Placement::Floating,
        Some("ruled") => return Placement::RuledToplevel,
        Some("layer") | Some("layer-shell") => return Placement::LayerShell,
        _ => {}
    }
    match session {
        Session::Wayland => Placement::RuledToplevel,
        Session::X11 | Session::Unknown => Placement::Floating,
    }
}

/// Reveal animation length. Short: the keyboard should be usable before the eye has finished
/// noticing it appeared, and the whole point of animating INSIDE the board is that nothing
/// outside it waits for this.
pub const REVEAL_MS: f32 = 180.0;

/// Ease-out for the in-board reveal: `t` in 0..=1 → 0..=1, arriving fast and settling.
pub fn reveal_progress(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Vertical settle for the board content during the reveal: keys start `distance` px low and
/// rise to rest. Applied as spacing INSIDE the panel, so the window rect is untouched.
pub fn reveal_offset(t: f32, distance: f32) -> f32 {
    distance * (1.0 - reveal_progress(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_wins_over_x11_when_both_are_set() {
        // XWayland sessions export DISPLAY too; a keyboard must not conclude X11 from it.
        assert_eq!(session_from(true, false, true), Session::Wayland);
        assert_eq!(session_from(false, true, true), Session::Wayland, "an inherited socket alone is a Wayland session");
        assert_eq!(session_from(false, false, true), Session::X11);
        assert_eq!(session_from(false, false, false), Session::Unknown);
    }

    #[test]
    fn placement_defaults_by_session_and_the_override_wins() {
        assert_eq!(placement_from(Session::Wayland, None), Placement::RuledToplevel);
        assert_eq!(placement_from(Session::X11, None), Placement::Floating);
        assert_eq!(placement_from(Session::Unknown, None), Placement::Floating);
        assert_eq!(placement_from(Session::Wayland, Some("floating")), Placement::Floating);
        assert_eq!(placement_from(Session::X11, Some(" ruled ")), Placement::RuledToplevel);
        assert_eq!(placement_from(Session::X11, Some("layer-shell")), Placement::LayerShell);
        // an unknown override is ignored, not a crash and not Floating-by-accident
        assert_eq!(placement_from(Session::Wayland, Some("bogus")), Placement::RuledToplevel);
    }

    #[test]
    fn every_placement_describes_itself_distinctly() {
        let all = [Placement::LayerShell, Placement::RuledToplevel, Placement::Floating];
        for a in all {
            for b in all {
                if a != b { assert_ne!(a.describe(), b.describe()); }
            }
        }
    }

    #[test]
    fn the_reveal_starts_low_and_settles_at_rest() {
        assert_eq!(reveal_progress(0.0), 0.0);
        assert!((reveal_progress(1.0) - 1.0).abs() < 1e-6);
        // ease OUT: past halfway in time it is well past halfway in distance
        assert!(reveal_progress(0.5) > 0.8, "{}", reveal_progress(0.5));
        assert_eq!(reveal_offset(0.0, 12.0), 12.0);
        assert!(reveal_offset(1.0, 12.0).abs() < 1e-5);
        // monotone: a settle that bounces would read as a glitch on a key you are about to press
        let mut last = reveal_offset(0.0, 12.0);
        for i in 1..=20 {
            let o = reveal_offset(i as f32 / 20.0, 12.0);
            assert!(o <= last + 1e-6, "offset rose at step {i}");
            last = o;
        }
    }

    #[test]
    fn capture_parses_and_clears_the_socket_variable() {
        // SAFETY: no other test in this crate touches WAYLAND_SOCKET, and it is restored.
        let prev = std::env::var_os("WAYLAND_SOCKET");
        unsafe { std::env::set_var("WAYLAND_SOCKET", "not-a-number") };
        assert!(!capture_inherited_socket(), "garbage must not count as captured");
        assert!(std::env::var_os("WAYLAND_SOCKET").is_none(), "cleared either way, so the toolkit never sees it");
        unsafe { std::env::remove_var("WAYLAND_SOCKET") };
        assert!(!capture_inherited_socket(), "absent is not captured");
        if let Some(v) = prev { unsafe { std::env::set_var("WAYLAND_SOCKET", v) }; }
    }
}
