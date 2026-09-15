//! creature-rig — per-part procedural posing for box creatures.
//!
//! Extracted verbatim from `unendlich-engine`. See `Cargo.toml` for why, and
//! [`anim_rig`]'s own header for what the layers do.
//!
//! ## The shape of this crate is load-bearing
//!
//! `anim_rig.rs` refers to its dependencies as `crate::sim::deform`,
//! `crate::sim::flex_bend` and `crate::sim::liveliness`. [`sim`] reproduces
//! that path so the extracted files are **byte-identical** to the engine's,
//! which is the point: a diff against the origin is empty, so the two copies
//! can be compared mechanically until the engine switches over to depend on
//! this crate instead of keeping its own.
//!
//! Do not "tidy" the `sim` module away. Flattening it to
//! `creature_rig::deform` would edit five files whose whole value right now
//! is that they have not been edited.
//!
//! ## What the caller provides
//!
//! Anatomy as [`anim_rig::RigPart`]s — `(role, offset, half-extents)` in a
//! body-local frame, X forward / Y up / Z right — and an
//! [`anim_rig::MotionInput`]. Out come [`anim_rig::PosedBox`]es with a centre,
//! half-extents and their own rotation.
//!
//! Box anatomy is why this is reusable across the two games: unendlich's
//! creatures are boxes already, and a voxel creature decomposes into boxes
//! (torso, four legs, head, tail) without needing a skinned mesh format.
//!
//! ## What this crate does NOT decide
//!
//! Gait selection. [`anim_rig`] takes a `gait_phase` and a `speed` and applies
//! one diagonal-pair pattern. Which footfall pattern a species uses at a given
//! speed — walk, trot, pace, canter, bound — and whether a given animal may
//! use it at all is the caller's business, because it is species data rather
//! than geometry. clawbound-battleground's `anim::Build` is that layer; a
//! bison that never bounds and a mammoth that only ever walks are decisions
//! about animals, not about posing boxes.

pub mod anim_rig;
pub mod sim;

pub use anim_rig::{corners, pose, world_corners, MotionInput, PosedBox, RigPart, Role};
pub use sim::liveliness::MotionState;
