//! gitmind-egui — the egui/kits frontend for gitmind (the main GUI; Qt/QML is the fallback).
//! The `App` + pages call the shared `cockpit` backend directly. The lib builds headless (egui
//! needs no display); the runnable window lives behind the `app` feature (eframe) in `main.rs`.
pub mod app;
pub mod remote_browse;
pub mod pages;
/// The changed-file list's model: porcelain parsing and stage decisions.
pub mod device_picker;
pub mod peek_viewer;
pub mod status_rows;
pub mod task;
pub mod fleet_actions;
