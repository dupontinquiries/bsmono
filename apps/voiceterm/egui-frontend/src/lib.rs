//! voiceterm-egui — the terminal, ported to egui.
//!
//! This is the tall part of the port, started the safe way: a **portable terminal core** —
//! a screen-grid model ([`Grid`]) plus a VT/ANSI [`Parser`] — that is pure and unit-tested,
//! with no egui or PTY in it. The egui cell renderer, the PTY process I/O, and the voice/mail
//! panes layer on top. Because voiceterm is where the fleet's staged commands land (via the
//! cross-app action protocol), [`Terminal::stage`] puts a command on the input line without
//! running it.
//!
//! The parser covers the everyday subset: printable text (UTF-8), the C0 controls
//! (`\n \r \t \x08`), and CSI sequences for cursor movement, erase, and SGR colour/attrs —
//! enough to run a shell, `ls --color`, vim's basic drawing, etc. Rarer sequences are ignored
//! rather than mis-rendered.

pub mod mail_view;
pub mod archived_view;
pub mod graph_view;
pub mod viz;
pub mod confirm_dialog;
pub mod chat_panel;
pub mod model_picker;
pub mod repl_voice_glow;
pub mod setup_wizard;
pub mod action_compass;
pub mod shell;
pub mod gesture_menus;

/// voiceterm's tab + pane model, shared with the QML frontend rather than
/// reimplemented here. It is pure `std` (no Qt, no `crate::`) and carries 28
/// tests in the app crate, so `#[path]` costs nothing and guarantees the two
/// frontends cannot drift. Lifting it into `uikit-term` alongside the block
/// tracking (8e93585) is the natural follow-up once a host that can build
/// egui 0.34 is available to validate the move.
#[path = "../../src/terminal_workspace.rs"]
pub mod terminal_workspace;
// The GestureHost+SlidePopup replacement (grid gesture menus with QML-parity variants) lives in
// the shared kit; re-exported under a voiceterm-familiar name.
pub use uikit_egui::slide_menu;
// `web` (HTML → egui RichDoc renderer) now lives in the shared uikit-web kit; re-export as `web`.
pub use uikit_web as web;

// The terminal (grid model + VT/ANSI parser + themed egui renderer + App) now lives in the
// shared `uikit-term` kit so other egui apps can embed it. Re-exported here so voiceterm's own
// code and callers keep using `voiceterm_egui::{Terminal, Grid, render_grid, App, …}` unchanged.
pub use uikit_term::*;

