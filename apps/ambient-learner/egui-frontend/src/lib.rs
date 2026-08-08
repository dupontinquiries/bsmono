//! ambient-learner-egui — the runnable frontend.
//!
//! The view code is NOT copied. One `#[path]` include pulls in the parent's
//! `src/egui_lib.rs`, and because a path-included module resolves ITS nested
//! modules relative to its own directory, `pub mod ui { pub mod egui; }` inside
//! it finds `src/ui/egui/mod.rs` without further help. One include, one copy of
//! the pages, and this crate is only the host that opens a window around them.
//!
//! Why a separate crate at all: the parent has iced as an optional dependency,
//! and cargo resolves optional dependencies whether or not their feature is
//! enabled. iced 0.12 caps web-sys below what eframe 0.34 needs, so an eframe
//! binary in the parent fails to RESOLVE even with iced switched off. That is
//! not something a feature flag can fix, and it is why the parent's own comment
//! — "they cannot be enabled together" — understated the problem.

#[path = "../../src/egui_lib.rs"]
mod applib;

pub use applib::*;
