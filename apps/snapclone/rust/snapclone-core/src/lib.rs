//! snapclone-core — pure business logic with no UI or network dependencies.
//!
//! # Boundary contract
//!
//! This crate defines WHAT the app does, not HOW it presents or transports.
//! Everything in here is:
//!   - No Qt, no egui, no Slint — zero UI framework dependency
//!   - No tokio::spawn, no async fn at the top level — callers provide runtime
//!   - Testable with `cargo test` on any machine with no display server
//!
//! # Layer position
//!
//! ```text
//!    snapclone-app (main.rs)
//!         │
//!    snapclone-qt-ui  ←── implements snapclone-ui-traits
//!         │
//!    snapclone-ui-traits  ←── THIS defines the contracts
//!         │
//!    snapclone-core   ←── YOU ARE HERE (business logic)
//!         │
//!    snapclone-storage, snapclone-identity (leaf crates)
//! ```
//!
//! # Extension pattern
//!
//! To build a different UI (e.g. egui mockup, Tauri web frontend):
//! 1. Depend on `snapclone-core` and `snapclone-ui-traits`
//! 2. Implement the traits from `snapclone-ui-traits` for your framework
//! 3. Wire up the `AppCore` event loop
//! 4. Ship — the storage, identity, crypto, and protocol logic are identical

pub mod app;
pub mod contact;
pub mod snap;
pub mod peer;
pub mod events;
pub mod commands;

pub use app::{AppCore, AppConfig};
pub use contact::{Contact, ContactState};
pub use snap::{Snap, SnapId, MediaType};
pub use peer::Peer;
pub use events::AppEvent;
pub use commands::AppCommand;
