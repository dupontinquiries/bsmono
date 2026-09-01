//! Virtual keyboard delivery + focus tracking — the "does it actually
//! type into the world, and does it know when to show itself" half of an
//! on-screen keyboard, factored out of `rusty-board` so it has no GTK
//! dependency by default.
//!
//! ## Why this crate exists (and didn't from the start)
//!
//! This code used to live in `rusty-board::compositor`. That was fine
//! for the GTK app, but `rusty-board` also has `gtk4` as a plain,
//! non-optional dependency of the whole crate (its `ui` module is GTK
//! widgets throughout) — so anything depending on `rusty-board` at all,
//! even just for this protocol code, pulled in the entire GTK4/libadwaita
//! dependency graph whether it used any of it or not. The egui port
//! wants exactly this code (key delivery + focus-driven show/hide) and
//! none of that.
//!
//! ## What stayed behind in `rusty-board`
//!
//! `compositor::placement` — layer-shell anchoring — did NOT move here.
//! Its whole job is calling `gtk4_layer_shell::LayerShell` methods on a
//! `gtk4::Window`; there is no GTK-free version of "anchor this GTK
//! window as a layer surface" to extract, and an eframe/winit consumer
//! doesn't have a `gtk4::Window` to anchor in the first place. See
//! `PORT_STATUS.md`'s "Platform placement" entry in the egui crate for
//! that gap's status.
//!
//! ## The one GTK-shaped seam that DOES live here
//!
//! `wayland_backend::try_new` (GDK-connection-sharing) and
//! `share_gdk_connection` need a live `gtk4::gdk::Display` — gated
//! behind the `gdk-integration` feature, which only the GTK app enables.
//! `wayland_backend::try_new_standalone` is the GTK-free entry point:
//! it opens its own connection via `$WAYLAND_DISPLAY` instead of sharing
//! GDK's, which is also architecturally the normal way a real input
//! method does this (Maliit is a separate connection from the app being
//! typed into) — see that function's doc comment.

pub mod virtual_keyboard;

#[cfg(feature = "wayland")]
pub mod wayland_backend;

pub mod im_socket;

#[cfg(feature = "atspi")]
pub mod atspi_focus;

pub mod content_purpose;

pub use virtual_keyboard::{VirtualKeyboard, VirtualKeyboardBackend};
