//! Echelon deterministic simulation core.
//!
//! Invariants every contributor must uphold (see ARCHITECTURE notes):
//! - No floating point anywhere in this crate. Positions/HP/etc. are Q32.32.
//! - No HashMap iteration; entity storage is a Vec with stable indices.
//! - All randomness flows through the seeded PCG in `World`.
//! - Any observable state must be folded into `World::state_hash`.

pub mod cmd;
pub mod fx;
pub mod map;
pub mod rng;
pub mod world;

pub use cmd::{Cmd, PlayerCmd};
pub use fx::{Fx, V2};
pub use world::World;
