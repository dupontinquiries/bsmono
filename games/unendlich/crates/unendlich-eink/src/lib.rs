//! unendlich's world on an e-ink tablet.
//!
//! The Vulkan engine cannot run on a Paper Pro, but its world generator is
//! pure Rust. This crate draws the world's SURFACE with a 2.5D heightfield
//! renderer over a cache of `surface_y_at` columns filled in the background,
//! and hands the pixels to eink-kit. Layers, each testable alone:
//!
//! * `heights`: the cache (levels of detail, near-first fill, never blocks);
//! * `view`: the renderer (camera + lookup → RGBA);
//! * `app`: the eink-kit app (touch camera, progressive resolution).
pub mod app;
pub mod heights;
pub mod view;
