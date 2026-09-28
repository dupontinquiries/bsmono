//! aperture-lsp — a client for the Language Server Protocol: spawn a
//! server, do the `initialize` handshake, tell it about an open file, and
//! ask `textDocument/completion` at a cursor.
//!
//! This is the piece `aperture-core::workspace_symbols`'s own module doc
//! names as future work ("an LSP (already named, unbuilt, in TODO.md) or a
//! name-resolution pass") and `aperture-core::route`'s AI-tier design
//! deliberately mirrors the shape of. It does NOT replace either of those —
//! `workspace_symbols`'s local tree-sitter outline and `route`'s
//! keyword-scored candidates are both fast, deterministic, and work with no
//! external process at all. A real language server is slower to warm up,
//! heavier, and occasionally absent (no `rust-analyzer` on PATH, or a
//! language aperture has no server config for yet), so the honest design is
//! LSP as an ADDITIONAL, optional tier — same "instant local tier first,
//! richer tier when available" pattern `route`'s two-tier split already
//! uses, not a replacement for either existing seam.
//!
//! ## What's here
//! * [`rpc`] — the wire format (`Content-Length` framed JSON), tested
//!   against adversarial byte streams, not just its own round-trip.
//! * [`client`] — spawns the server, does the handshake, exposes
//!   `did_open`/`did_change`/`completion`. Deliberately blocking (see that
//!   module's doc for why) — the concurrency story an editor's UI thread
//!   needs is a worker wrapped AROUND this client, the same shape
//!   `aperture_complete::engine::Engine` already is for AI completions,
//!   not something duplicated inside this crate before a real caller shows
//!   up needing it.
//!
//! ## What's not here (yet)
//! * Diagnostics (`textDocument/publishDiagnostics`) — a server-initiated
//!   notification, which the reader thread currently drops on the floor.
//!   Real and useful; deferred because completions were the smaller,
//!   independently-testable slice to get right first.
//! * Go-to-definition, hover, incremental sync (`did_change` always sends
//!   the whole document) — same reasoning: real gaps, not implemented
//!   because nothing in aperture calls this crate yet to prove which of
//!   them actually matters first.
//! * Wiring into `aperture-egui` at all. This crate is usable and tested
//!   standalone (integration-tested against a real `rust-analyzer` when
//!   one is on PATH); nothing in the shell calls it yet.

pub mod client;
pub mod rpc;

pub use client::{CompletionItem, Diagnostic, DiagnosticSeverity, Hover, IncomingCall, CallersAnswer, Location, LspClient};
