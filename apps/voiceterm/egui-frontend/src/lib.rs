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

pub mod viz;
// `web` (HTML → egui RichDoc renderer) now lives in the shared uikit-web kit; re-export as `web`.
pub use uikit_web as web;

// The terminal (grid model + VT/ANSI parser + themed egui renderer + App) now lives in the
// shared `uikit-term` kit so other egui apps can embed it. Re-exported here so voiceterm's own
// code and callers keep using `voiceterm_egui::{Terminal, Grid, render_grid, App, …}` unchanged.
pub use uikit_term::*;

