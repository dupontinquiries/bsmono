//! tomescope library crate root. Both the `tomescope` GUI binary and the
//! `shelfctl` CLI depend on this crate for the format parsers, content
//! model, library index, layout engine, and everything else that isn't
//! specifically GUI event-loop glue (which stays in `main.rs`).

pub mod content;
pub mod formats;
pub mod formats_cbz;
pub mod formats_epub;
pub mod formats_pdf;
pub mod layout;
pub mod library;
pub mod lookup;
pub mod opds;
pub mod panels;
pub mod persist;
pub mod reader;
pub mod settingsui;
pub mod stats;
pub mod study;
pub mod theme;
pub mod tts;
pub mod util;
