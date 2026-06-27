//! Compositor / windowing-system integration.
//!
//! This module is the bridge between RustyBoard and the desktop. It does
//! two jobs:
//!
//! 1. **Window placement.** On Wayland with `wlr-layer-shell` support, we
//!    make the keyboard window a bottom-anchored *layer surface* — the
//!    same trick panels and notifications use. Other windows resize
//!    around us, we don't steal focus, and the compositor knows we're a
//!    persistent on-screen keyboard rather than a regular app window.
//!    On X11 (or compositors without layer-shell), we fall back to a
//!    keep-above borderless toplevel.
//!
//! 2. **Key injection.** When the user taps a key or finishes a swipe,
//!    we need to deliver the resulting characters/keystrokes to the
//!    *focused* application. We do this through `zwp_virtual_keyboard_v1`
//!    on Wayland (the same protocol Maliit uses for Phosh and Sway).
//!    On X11, we shell out to `xdotool` as a fallback.
//!
//! ## Why not just `enigo` or `xdotool` everywhere?
//!
//! On Wayland those tools don't work — the compositor enforces that
//! arbitrary clients can't inject input. Phosh, Sway, river, and Wayfire
//! all support the `virtual_keyboard` and `input_method` protocols
//! specifically so on-screen keyboards can do this. Falling back through
//! `xdotool` on X11 keeps RustyBoard usable on legacy desktops.
//!
//! ## Mapping to Maliit's mechanism
//!
//! - Maliit uses the *Qt platform plugin* `maliit-input-method` which
//!   talks to the Wayland compositor on the keyboard's behalf. The QML
//!   side just calls `MaliitGeometry.shown = true/false` and the platform
//!   layer translates that to compositor IPC.
//! - RustyBoard uses `gtk4-layer-shell` (analog: layer-surface placement)
//!   plus the same `virtual_keyboard_v1` protocol Maliit uses for key
//!   delivery. The compositor still does the show/hide based on
//!   `text-input-v3` focus events; we just react to those.

pub mod im_socket;
pub mod placement;
pub mod virtual_keyboard;

#[cfg(feature = "wayland")]
pub mod wayland_backend;

pub use placement::{configure_window_placement, Placement};
pub use virtual_keyboard::{VirtualKeyboard, VirtualKeyboardBackend};
