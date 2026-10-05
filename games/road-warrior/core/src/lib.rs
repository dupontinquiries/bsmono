pub mod boss;
pub mod campaign;
/// What the car just did, named for the shell to play.
pub mod cues;
pub mod encounter;
pub mod level;
/// Rays traced once per level into a cache the fixed camera can reuse.
pub mod lighting;
/// Vehicle armour to surface response, consumed by the shader.
pub mod material;
pub mod progress;
pub mod sim;
pub mod track;
pub mod vehicle;

pub use boss::{Attack, AttackKind, Boss, BossPhase};
pub use campaign::{is_level_complete, Campaign};
pub use cues::{Cue, Engine, Listener};
pub use lighting::{BakedLighting, Sun};
pub use material::Material;
pub use encounter::EncounterOutcome;
pub use level::{BossData, LevelData};
pub use progress::SaveState;
pub use sim::advance;
pub use track::{Segment, Track};
pub use vehicle::{CarKind, GroundContact, Vehicle, VehicleStats};
