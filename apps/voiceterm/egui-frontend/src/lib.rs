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

pub mod pty_input;
pub mod table_view;
pub mod row_render;
pub mod detect;
pub mod mail_view;
pub mod archived_view;
pub mod cards_view;
pub mod graph_view;
pub mod bars;
pub mod browser;
pub mod bookmarks_view;
pub mod build_view;
pub mod backlinks_view;
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

/// The table engine, on the same terms. `table_ops` holds TableOp and the apply
/// pipeline, `table_nlp` turns natural language into ops and meta-commands.
/// Both are plain Rust with no Qt in them, and `table_view` is the surface over
/// them — the view was what was missing, not the engine. Included rather than
/// re-implemented so the two frontends parse the same vocabulary; a second
/// parser would accept a slightly different English within a week.
/// The dirty-row tracker, on the same terms — pure, dependency-free, and fed by
/// the grid. `row_render` is the egui-side use of it; see that module for why
/// what it saves here is text LAYOUT rather than serialisation.
#[path = "../../src/row_cache.rs"]
pub mod row_cache;

/// The include chain terminates here: entity_extract needs clog!, log needs
/// util, and util is std-only with no crate:: references at all. Worth naming
/// as a cost — including a module drags its crate-internal dependencies with
/// it. It is still the right trade against a second copy of a 1,900-line
/// extractor that would accept slightly different output within a month.
#[path = "../../src/util.rs"]
pub mod util;

/// entity_extract logs through `crate::clog!`, so the logger comes with it.
/// log.rs is std-only as well, and including it rather than defining a stand-in
/// macro keeps the same categories and the same VOICETERM_LOG filtering — a
/// second logger would mean the two frontends disagreed about what is worth
/// recording.
#[macro_use]
#[path = "../../src/log.rs"]
pub mod log;

/// The detection engines, same terms again: `entity_extract` is
/// terminal-output-as-menu, `dumb_summary` spots ASCII progress bars. Both are
/// Qt-free and both had no surface at all — `detect` is the views they were
/// missing.
#[path = "../../src/entity_extract.rs"]
pub mod entity_extract;
#[path = "../../src/dumb_summary.rs"]
pub mod dumb_summary;

#[path = "../../src/table_ops.rs"]
pub mod table_ops;
#[path = "../../src/table_nlp.rs"]
pub mod table_nlp;
// The GestureHost+SlidePopup replacement (grid gesture menus with QML-parity variants) lives in
// the shared kit; re-exported under a voiceterm-familiar name.
pub use uikit_egui::slide_menu;
// `web` (HTML → egui RichDoc renderer) now lives in the shared uikit-web kit; re-export as `web`.
pub use uikit_web as web;

// The terminal (grid model + VT/ANSI parser + themed egui renderer + App) now lives in the
// shared `uikit-term` kit so other egui apps can embed it. Re-exported here so voiceterm's own
// code and callers keep using `voiceterm_egui::{Terminal, Grid, render_grid, App, …}` unchanged.
pub use uikit_term::*;

