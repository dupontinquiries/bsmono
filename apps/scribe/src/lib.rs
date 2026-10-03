//! scribe -- type scenes and lines; every line is truly centred; export to HTML for printing.
//!
//! The pipeline is three pure steps, all tested without a window:
//!
//! 1. [`script::parse`]: plain text in, a [`script::Script`] out. A line starting with `#` (or `INT.` /
//!    `EXT.`) is a SCENE; every other line is a LINE; `NAME: words` puts a speaker over the line;
//!    `(words)` is a direction; `===` is a page break; `Title:` / `Author:` lines at the top make a
//!    title page. Nothing to learn beyond that.
//! 2. [`style::Style`]: a look (font stack, size, spacing, how scenes and speakers are set), chosen
//!    from [`style::STYLES`] with an optional font override.
//! 3. [`html::render`]: a self-contained page, `@page` and print rules included, with the text
//!    centred in its column (`text-align: center`, never an indent).

pub mod cli;
pub mod html;
pub mod script;
pub mod store;
pub mod style;

/// The editor window's contents (the `app` feature: egui and the shared UI kit).
#[cfg(feature = "app")]
pub mod editor;
