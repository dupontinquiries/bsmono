// Clawbound — Battleground · Rust + wgpu Microvoxel Edition
// A 5v5 three-lane arena for the thirteen legendary ancestors of Clawbound,
// with dynamic weather and map events, everything rendered as instanced
// microvoxels.
//
// Layering (headless core → shell):
//   consts / gods / items / weather / map_events / wild / audio(dsp) / input / sim / systems / abilities / ai
//       — pure data + arithmetic, run under `cargo test` with no GPU
//   voxel / models / props / vfx        — mesh + particle generation (CPU)
//   render / scene / ui / app / headless — wgpu, egui, winit shells

pub mod consts;
pub mod gods;
pub mod items;
pub mod voxel;
pub mod models;
pub mod props;
pub mod weather;
pub mod map_events;
pub mod wild;
pub mod bosses;
pub mod disasters;
pub mod audio;
pub mod input;
pub mod gridpick;
pub mod gamepad;
mod touchkit;
pub mod touch;
pub mod sim;
pub mod systems;
pub mod abilities;
pub mod ai;
pub mod vfx;
pub mod render;
pub mod scene;
pub mod ui;
pub mod launch;
pub mod app;
pub mod headless;
