//! gitmind-egui — the egui/kits frontend for gitmind (the main GUI; Qt/QML is the fallback).
//! The `App` + pages call the shared `cockpit` backend directly. The lib builds headless (egui
//! needs no display); the runnable window lives behind the `app` feature (eframe) in `main.rs`.
pub mod app;
pub mod pages;
pub mod task;
