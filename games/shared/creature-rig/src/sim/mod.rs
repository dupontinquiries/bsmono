//! The engine's `sim::` namespace, reproduced.
//!
//! `anim_rig.rs` says `use crate::sim::deform;`. Keeping that path valid here
//! is what lets the extracted files stay byte-identical to the ones still in
//! `unendlich-engine` — see the crate root.
//!
//! Only the four modules `anim_rig` actually needs came across. The engine's
//! `sim` has many more (`ragdoll_bridge`, `ragdoll_service`, and the rest of
//! the simulation); those stay in the engine because they are about a running
//! world, not about posing a body. `ragdoll.rs` is here because
//! [`anim_rig::RagdollSample`] is the input format for its output, so a caller
//! that wants the ragdoll blend needs both — and it too has no internal edges.
//!
//! [`anim_rig::RagdollSample`]: crate::anim_rig::RagdollSample

pub mod deform;
pub mod flex_bend;
pub mod liveliness;
pub mod ragdoll;
