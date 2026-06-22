//! peek library. The bins are thin CLIs over this; the gitmind GUI uses
//! `view` to render file content (mmap + newline index, bucket/symbol
//! selection) inside the file-viewer pane.
pub mod cli;
pub mod formatter;
pub mod glob;
pub mod parser;
pub mod query;
pub mod reader;
pub mod view;
