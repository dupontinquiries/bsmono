//! orlosky-kit: Jason Orlosky's eye-tracking algorithms, in Rust. See `pupil` for the first of them, and the crate
//! description for the source and its license.

pub mod ellipse;
pub mod gray;
pub mod pupil;

pub use gray::{Blob, Gray, Mask};
pub use pupil::{detect, detect_any, Pupil};
