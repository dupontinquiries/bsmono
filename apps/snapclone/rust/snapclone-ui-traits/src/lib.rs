//! snapclone-ui-traits — the UI boundary layer.
//!
//! # Purpose
//!
//! This crate defines WHAT the UI must do, not HOW it does it.
//! Any UI backend (Qt6, egui, Slint, web/WASM, terminal) implements
//! these traits and gets the full SnapClone feature set for free.
//!
//! # How to build a new UI backend
//!
//! 1. Depend on this crate and `snapclone-core`
//! 2. Implement `SnapApp` for your root application struct
//! 3. Implement `TabBar`, `GestureHost`, `ContactList`, `SnapViewer`
//! 4. Call `snapclone_core::AppCore::new()` and wire events → trait methods
//!
//! # Layer contract
//!
//! ```text
//! ┌────────────────────────────────────────────────────────┐
//! │  your_ui_backend (egui / Qt / Slint / web)             │
//! │    implements: TabBar, GestureHost, ContactList, ...    │
//! ├────────────────────────────────────────────────────────┤
//! │  snapclone-ui-traits  ← THIS CRATE (stable boundary)  │
//! ├────────────────────────────────────────────────────────┤
//! │  snapclone-core (AppCore, AppEvent, AppCommand)        │
//! └────────────────────────────────────────────────────────┘
//! ```
//!
//! The traits in this crate are the ONLY things a UI backend needs to look at.
//! Core logic changes never break UI backends; UI changes never break the core.

pub mod tabs;
pub mod gesture;
pub mod contact;
pub mod theme;
pub mod app;

pub use tabs::{TabDef, TabBar, BadgeCount};
pub use gesture::{GestureEvent, GestureHost, SwipeDirection};
pub use contact::{ContactListItem, ContactList, ContactAction};
pub use theme::Theme;
pub use app::SnapApp;
