//! clawbound-rs — a native (Rust + wgpu) papercraft cat-vs-predator game.
//!
//! This library holds the **platform-agnostic** core: math, the papercraft mesh
//! builder, world generation (domain-warp terrain, Whittaker biomes, genetic
//! foliage, talus erosion, water), the posable cat rig, the hunter archetypes +
//! micro-FSM, branch physics, map scoring, and objectives. It has no window or
//! GPU-surface dependency, so the same crate is driven by:
//!   * the native headless renderer (`src/main.rs`) — offscreen → PNG, and
//!   * a windowed/WASM entry (built locally; see `BUILD_WEB.md`) — wgpu surface.
//!
//! Keeping the core here (not in the binary) is what makes the web test client a
//! recompile of the *same* game rather than a separate port.

pub mod math;
pub mod mesh;
pub mod physics;
pub mod cat;
pub mod clan;
pub mod combat;
pub mod lore;
pub mod class;
pub mod capture;
pub mod ecology;
pub mod hunter;
pub mod iqm;
pub mod mapeval;
pub mod menu;
pub mod net;
pub mod objectives;
pub mod openworld;
pub mod powerup;
pub mod prowl;
pub mod ring;
pub mod sim;
pub mod weather;
pub mod world;
pub mod worldscript;
