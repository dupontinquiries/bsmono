//! A minimal `clog!` so code lifted from voiceterm compiles unchanged.
//!
//! voiceterm's own `log.rs` is a full per-category file-routing system
//! (WEB2QML_LOG env var, timestamped files under
//! `~/.config/web2qml/logs/<category>.log`, its own test suite) — more
//! than a shared engine crate should carry, and specific to voiceterm's
//! own settings/config surface (nothing here should invent a
//! `WEB2QML_LOG`-shaped env var of its own).
//!
//! This crate's `clog!` keeps the one behavior that must never regress
//! when code moves here: **a log line is never silently dropped**. It
//! always echoes to stderr, exactly like voiceterm's macro does
//! unconditionally regardless of routing. It does not attempt the file
//! routing — a consumer that wants that can still see everything on
//! stderr, which is what voiceterm guarantees today ("ALWAYS echoes to
//! stderr... no log line is ever lost").
#[macro_export]
macro_rules! clog {
    ($cat:expr, $($arg:tt)*) => {
        eprintln!("[{}] {}", $cat, format!($($arg)*))
    };
}
