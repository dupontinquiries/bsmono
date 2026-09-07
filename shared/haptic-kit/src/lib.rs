pub mod device;
pub mod pattern;

pub use device::{Motor, MotorError};
pub use pattern::{MotorState, Pattern, PatternPlayer, Pulse};
