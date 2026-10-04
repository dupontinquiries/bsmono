//! gaze-kit: from eye measurements to a cursor that reads well. See `gate` (refuse a blink or a lid), `filter` (One Euro), `calib` (affine calibration).

pub mod calib;
pub mod filter;
pub mod gate;

pub use calib::Affine;
pub use filter::{OneEuro2D, OneEuroParams};
pub use gate::{PupilGate, Verdict};
