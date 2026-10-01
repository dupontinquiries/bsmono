//! speaker-dsp — a small-speaker audio chain in pure Rust.
//!
//! Replaces the LV2/LADSPA/PipeWire-filter-chain design (bankstown + swh-plugins) with Rust
//! built on the monorepo's own kits: `studio-fx` for the filters, saturator, compressor and
//! lookahead limiter; `audio-kit` for measurement. The idea kept from bankstown is the
//! psychoacoustics; what is added is a speaker model that sizes every stage.
//!
//! See [`vbass`] for the psychoacoustics, [`chain`] for the power-efficiency argument, and
//! [`model`] for why the speaker numbers are tagged as assumed until measured.

pub mod chain;
pub mod loudness;
pub mod model;
pub mod vbass;

pub use chain::SpeakerChain;
pub use loudness::{Correction, LoudnessComp};
pub use model::{Provenance, SpeakerModel};
pub use vbass::{VirtualBass, VirtualBassParams};
