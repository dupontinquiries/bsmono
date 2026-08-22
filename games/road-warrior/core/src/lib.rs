pub mod boss;
pub mod campaign;
pub mod encounter;
pub mod level;
pub mod progress;
pub mod sim;
pub mod track;
pub mod vehicle;

pub use boss::{Attack, AttackKind, Boss, BossPhase};
pub use campaign::{is_level_complete, Campaign};
pub use encounter::EncounterOutcome;
pub use level::{BossData, LevelData};
pub use progress::SaveState;
pub use sim::advance;
pub use track::{Segment, Track};
pub use vehicle::{CarKind, GroundContact, Vehicle, VehicleStats};
