//! aperture-scene — the physical-UI file viewer (`aperture-view`).
//! `viewer` is the headless brain (scroll, raster window, touch, orbit —
//! tested), `scene` the world geometry (tested through `pack_sorted`), `app`
//! the one egui/GL-touching layer, and the bin a thin eframe boot.

pub mod rooms;
pub mod scene;
pub mod viewer;
#[cfg(feature = "app")]
pub mod app;
