//! voxelkit — authored voxels in, posable box anatomy out.
//!
//! ## Why this is a bridge and not another extraction
//!
//! The obvious move after `creature-rig` was to hunt for duplicated voxel code
//! and pull it into a shared crate the way `shellkit` was built. I looked, and
//! there is none:
//!
//! * `clawbound-battleground/src/voxel.rs` — a dense `Grid` with
//!   `filled_box`/`ellipsoid`/`cone_y` and an AO-baking mesher. Nothing else
//!   in the fleet has one; `fn filled_box` and `fn ellipsoid` appear in
//!   exactly that one file.
//! * `blackstar-rs/src/voxel.rs` — voxel GAN critics. Shares the word and
//!   nothing else.
//! * `unendlich-engine` — streamed chunk meshing for a running world.
//! * `voxel-tools` — 52k lines of authoring, and **zero dependencies, pure
//!   std**.
//!
//! Copying any of it would have manufactured the duplication I was looking
//! for. The real finding is the last bullet: `voxel-tools` is a
//! general-purpose authoring library that happens to live inside one game's
//! workspace, so no other game can reach it. This crate reaches it.
//!
//! ## What it adds
//!
//! The step neither side has: **a voxel body becomes box anatomy**.
//!
//! `creature_rig::RigPart` is `(role, offset, half-extents)` in a body-local
//! frame. That is why the rig is reusable across games — and it is also why
//! clawbound could not use it: clawbound's creatures are single rigid
//! `VoxelMesh`es with no skeleton and nothing to pose. A voxel body has no
//! skeleton either, but it decomposes into boxes directly, which is the same
//! thing the rig wants.
//!
//! So: author a limb as a [`voxel_tools::solid::Solid`], label it with a
//! [`creature_rig::anim_rig::Role`], and [`BodyPlan::build`] gives back both
//! the rasterised voxels (to mesh and draw) and the `RigPart`s (to pose) —
//! from ONE description, so the drawn body and the posed body cannot
//! disagree. Two representations of one creature that drift is the failure
//! this exists to prevent.
//!
//! ## Layer
//!
//! No renderer, no game. Coordinates cross the boundary as `[f32; 3]`, the
//! same convention `creature-rig` holds to.

pub mod body;

pub use body::{BodyPart, BodyPlan, BuiltBody, PlanError};

/// Re-exported so a consumer needs one dependency, not three.
pub use creature_rig::anim_rig::{Role, RigPart};
pub use voxel_tools::solid::{Axis, Material, Solid};
