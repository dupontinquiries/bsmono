//! arrange — the arrangement timeline, in four layers that each stand alone.
//!
//! * `model`  — tracks and clips on a beat grid; what an arrangement IS.
//! * `cmd`    — every edit as a `Command`, applied with undo.
//! * `tools`  — the pointer, in beats and tracks: gestures in, commands out.
//! * `render` — the library of sources and the block mixer with a looping
//!              transport.
//! * `palette` — every action by name, ranked for a command palette.
//! * `stretch` — WSOLA time-stretch for tempo-locked clips.
//! * `project` — save and load: a JSON of the timeline beside the library's
//!              audio as WAV files.
//!
//! None of them know egui or an audio device. `arrangepage` (behind the
//! `app` feature) is the view over them; the generator thread drives the
//! transport. The layering is the seam for lifting this into a shared
//! crate later — nothing here is about beats specifically except the
//! `Beat` source kind.

// The view behind the `app` feature is the only caller of much of this,
// but the core and its tests belong in every build (and the test binary
// with `app` on would have to link ALSA, which not every machine has).
#![cfg_attr(not(feature = "app"), allow(dead_code))]

pub mod cmd;
pub mod model;
pub mod palette;
pub mod project;
pub mod render;
pub mod stretch;
pub mod tools;
