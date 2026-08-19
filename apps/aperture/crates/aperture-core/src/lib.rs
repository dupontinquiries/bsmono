//! aperture-core — the headless editor.
//!
//! The layering rule is studio's (see apps/studio/README.md), applied to text:
//! **an edit is a value**. Every mutation of the buffer is an [`Op`] that
//! carries enough to invert itself, so undo is "apply the inverse", a macro is
//! a `Vec<Op>`, a test is a fold of ops over an empty buffer, and a future
//! collab session is "send the op" — one mechanism, no command classes.
//!
//! What lives here:
//! * [`buffer`] — a `ropey` rope with (line, col) ↔ char-index conversion and
//!   load/save. O(log n) edits are the substance behind the "fast" claim.
//! * [`op`] — the invertible edit value.
//! * [`history`] — grouped undo/redo with typing coalescing.
//! * [`editor`] — cursor + selection + the command surface the shell calls
//!   (`insert_text`, `move_*`, `undo`, …). Sticky columns, word motion,
//!   auto-indent. Selection is a value (anchor/head as char indices), designed
//!   so multi-cursor is `Vec<Selection>` later, not a rewrite.
//! * [`syntax`] — the [`syntax::LineLexer`] seam + an incremental per-line
//!   state cache. Ships a small hand-rolled Rust lexer; the cache interface is
//!   the socket tree-sitter plugs into later (see DESIGN.md §syntax).
//! * [`project_search`] — "find in project": grep an already-walked file
//!   list for a substring, capped per-file and in total, returning
//!   line/column hits a shell can render as a jump list.
//! * [`workspace_symbols`] (feature `tree-sitter`) — "go to symbol in
//!   workspace": the same per-file outline the symbol popup uses, run over
//!   every walked file so a name search jumps across the tree, not just the
//!   open file. A name index, not semantic go-to-definition.
//! * [`route`] (feature `tree-sitter`) — "go to via route": the instant,
//!   deterministic tier of a natural-language "go to" search — score
//!   [`workspace_symbols`] + file content against a query's keywords,
//!   return a short ranked list with a one-line description each. Modeled
//!   on logscope's `filesummary.rs` two-tier design; the AI-assisted
//!   second tier is `aperture_complete::route_ai`.
//!
//! No egui, no sockets, no threads. `cargo test -p aperture-core` runs the
//! whole editing model headless in well under a second — that is the point.
//!
//! One additional I/O boundary, on the same terms as load/save: `gitspace`
//! shells out to `git` (`std::process::Command`, no new dependency) to read
//! history, diffs, and blame. It's a boundary the SAME shape as reading a
//! file off disk — the editor still has zero opinions about threads or
//! sockets — so it lives here rather than forcing every consumer (the flat
//! shell AND the physical viewer) to duplicate the parsers or take on a
//! heavier crate just to see what changed.

pub mod buffer;
pub mod editor;
pub mod gitspace;
pub mod history;
pub mod op;
pub mod project_search;
pub mod syntax;
pub mod width;
#[cfg(feature = "tree-sitter")]
pub mod ts;
#[cfg(feature = "tree-sitter")]
pub mod workspace_symbols;
#[cfg(feature = "tree-sitter")]
pub mod route;

pub use buffer::{Buffer, Pos};
pub use editor::{Editor, Selection};
pub use op::Op;
