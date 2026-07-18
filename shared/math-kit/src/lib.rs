//! math-kit — shared non-crypto math for the fleet.
//!
//! The survey that motivated this kit (2026-07-18) found at least seven
//! independent `Vec3` definitions (clawbound, soldier, logscope spatial,
//! fao-codec geometry, bo-controller room_geometry, unendlich voxel_raycast,
//! unendlich sim/fixed), four `Mat4`s, and `lerp`/`smoothstep`/`clamp` copies
//! in ~20 files including every soldier example. This crate is the superset
//! home for those, extracted with **bit-identical semantics** where a consumer
//! has pinned tests (clawbound's `Vec3`/`Mat4`/`perspective`/`look_at` are
//! adopted verbatim — its 206-test suite is the parity proof).
//!
//! Scope rules:
//! - non-crypto only — keys/tokens/digests live in `crypto-kit`
//! - RNG lives in `core-kit` (`SplitMix64`) — not duplicated here
//! - audio/DSP lives in `audio-kit`; lattice/value noise in `noise-kit`
//! - zero dependencies, `std` + `f32` math, deterministic on every platform
//!
//! Adoption map (call sites found duplicating this kit, for future lifts):
//! - `vec`/`mat`: games/soldier/src/math.rs, apps/logscope/.../spatial.rs,
//!   apps/fao-codec/fao_codec_lib/src/geometry.rs,
//!   apps/bo-suite/bo-controller/src/room_geometry.rs
//! - `scalar`: games/soldier/examples/* (each carries a private `lerp`),
//!   games/soldier/src/particles.rs (`lerpf`), clawbound/main.rs easing
//! - `stats`: apps/voiceterm (layered_layout, token_log), apps/bo-suite
//!   (speaker_calibration, stream_separator, rtt_tracker, stem_split),
//!   apps/ambient-learner/src/helpers.rs, snapclone-audio prosody.rs
//! - `hash`: the ~10 hand-rolled FNV-1a copies (gitmind identity/encode,
//!   fao-codec block_dict/gpu_pipeline, bo-suite device_driver/soundscape,
//!   paperbound-core's lockstep state hash, …). NOTE: several of those pin
//!   hashes in protocols or tests — migrate by *replacing the constants with
//!   a call*, never by changing the algorithm. `fnv1a_64` here is the
//!   standard offset-basis/prime pair those copies already use.

pub mod aabb;
pub mod geom;
pub mod hash;
pub mod mat;
pub mod scalar;
pub mod stats;
pub mod vec;

pub use aabb::{Aabb2, Aabb3};
pub use mat::{look_at, perspective, Mat4};
pub use vec::{v2, v3, Vec2, Vec3};
