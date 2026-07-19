//! RustyBoard library — exposes internal modules so `rusty-board-config`
//! and tests can use them.

pub mod compositor;
pub mod config;
pub mod gestures;
pub mod ipc;
pub mod keyboard;
// The layout layer moved to crates/board-layouts (gtk-free, shared with egui-frontend); the
// crate re-export keeps every `crate::layouts::` path — builder, io, Layout, Row — working.
pub use board_layouts as layouts;
pub mod llm;
pub mod steno;
pub mod ui;
pub mod util;
pub mod voice;
