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
pub mod blockers;
pub mod anim;
/// Cut a voxel body into posable parts; posed rig -> per-part matrices.
pub mod rigged;
/// What a cat's body does about roots, ferns, rivers, leaps and fights.
pub mod puppet;
/// The aimed leap: onto rocks and logs, chained from perch to perch.
pub mod leap;
pub mod camps;
/// Clan border markers: placed, refreshed on patrol, and lasting days rather
/// than minutes. The half of the scent system wildkit does not cover, because
/// a trail answers "who walked here" and a marker answers "whose ground is
/// this".
pub mod markers;
/// The clock, and what it does to a cat's senses. `wildkit::sim::climate` says
/// what the weather is like at a place; this says what time it is, which is
/// the other half of "LOS vs ears varies by time of day".
pub mod daylight;
/// The fresh-kill economy: caught, carried, piled, eaten before it turns.
/// wildkit's forage modules are a creature feeding itself; nothing there
/// carries prey home, and carrying it home is the meta.
pub mod freshkill;
/// The Dark Forest bleeding into the living one: sight fails, scent persists,
/// hearing lies. A `PerceptParams` modifier, the pattern `daylight` set.
pub mod shadowrealm;
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
