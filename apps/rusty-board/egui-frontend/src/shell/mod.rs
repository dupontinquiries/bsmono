//! shell — what owns the window/surface and runs `Feel::frame` in it.
//!
//! The screen (`demo_app::Feel`) is written against a bare `egui::Context`: it lays itself out
//! in whatever `screen_rect` it is given, reads pointer/touch events out of `RawInput`, and asks
//! for exactly one thing back, `ViewportCommand::Visible`. Everything that differs between hosts
//! — how a surface is created, what role it has, how it is hidden, where its input comes from —
//! is a shell, and a shell is a function that owns a loop and calls `feel.frame(&ctx)`.
//!
//! | shell | surface role | placement it can offer | input |
//! |---|---|---|---|
//! | eframe (`demo_app::run`) | xdg_toplevel via winit | `RuledToplevel` / `Floating` | winit |
//! | [`layer`] (`shell::layer::run`) | `zwlr_layer_surface_v1` via sctk | `LayerShell` | sctk pointer + touch |
//!
//! A future X11 shell (a toplevel with a `_NET_WM_STRUT_PARTIAL` and override-redirect) or a
//! COSMIC/other-compositor shell is another row, not another `if cfg!` in the screen.
//!
//! Selection is in the binary: on a Wayland session with the `layer` feature, try the layer
//! shell first and fall back to eframe if the compositor has no layer-shell global.
//! `RUSTY_BOARD_PLACEMENT=ruled|floating` skips the attempt.

#[cfg(feature = "layer")]
pub mod layer;
