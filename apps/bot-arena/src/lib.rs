//! bot-arena core — the GTK-free half of the app, exposed as a lib.
//!
//! Everything here runs headless: the engine (population GA + real-eval
//! backends + checkpoints), the FSM playground, the shared registry paths,
//! model management, and the optional llama.cpp backend shim. The GTK4 GUI
//! (`src/main.rs`) and Corpus Studio (`src/bin/corpus_studio.rs`) are bins
//! behind the default `gtk-ui` feature; the egui front-end depends on this
//! crate with `default-features = false` and drives the same engine over the
//! same `EngineMsg` channel — one core, two shells, no view-model copies.

pub mod engine;
pub mod fsm;
pub mod llama_backend;
pub mod models;
pub mod registry;
