//! gitmind-egui — the egui/kits frontend for gitmind (the main GUI; Qt/QML is the fallback).
//! The `App` + pages call the shared `cockpit` backend directly. The lib builds headless (egui
//! needs no display); the runnable window lives behind the `app` feature (eframe) in `main.rs`.
pub mod app;
pub mod remote_browse;
/// The ssh/cargo adapter that lets a fleet reroll execute.
pub mod ssh_runner;
pub mod pages;
/// The layout rules a 360-point screen needs (see the phone photo and `tests/phone_layout.rs`).
pub mod narrow;
/// What gitmind listens for, over `voiced-core` (registration, grammar, and the action).
pub mod voice_intents;
/// The changed-file list's model: porcelain parsing and stage decisions.
pub mod device_picker;
pub mod peek_viewer;
pub mod status_rows;
pub mod show;
/// The app catalog as a searchable, finger-sized list (Fleet, Push).
pub mod app_list;
pub mod task;
pub mod fleet_actions;
