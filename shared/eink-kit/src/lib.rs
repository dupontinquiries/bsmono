//! eink-kit — egui apps on e-ink tablets, no GPU, no eframe.
//!
//! ```text
//!   app (EinkApp::ui)                      ── same code as a desktop egui app
//!     │
//!   engine::Engine                         ── headless core, fake-clock testable
//!     ├─ raster     egui meshes → pixels, tiled, diffed into the live buffer
//!     ├─ damage     dirty tiles → few rects (+ colour flag)
//!     ├─ motion     per-tile change rate + app hints → which regions are "moving"
//!     ├─ policy     rects → waveform per update, capped motion lane, deferred clean-up
//!     └─ touch      evdev MT-B → touch events → egui (pointer emulation)
//!     │
//!   present::Present                       ── who owns the panel (varies per device)
//!     ├─ qtfb     AppLoad window inside xochitl (fixed mode, touch pre-mapped)
//!     ├─ blight   Oxide display server (per-update waveform, raw input ring)
//!     └─ mem      tests / headless simulator with PPM + update-log dumps
//! ```
//!
//! Everything tunable is an environment variable (see `config`), printed at
//! startup, and `EINK_TRACE=1` prints each refresh decision with its reason.

pub mod config;
pub mod damage;
pub mod device;
pub mod engine;
pub mod format;
pub mod geom;
pub mod motion;
pub mod policy;
pub mod present;
pub mod raster;
pub mod touch;

pub use egui;
pub use motion::{hint, motion_interval, Hint};
pub use engine::{diagnostics, request_full_refresh, run, EinkApp, Engine, FromCtx};
