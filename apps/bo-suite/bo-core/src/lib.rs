//! bo-core — modules shared by bo-controller and bo-mux. These were byte-identical copies in both
//! crates; hoisted here so there's one source of truth (each crate re-exports them).
pub mod audio_features;
pub mod backup_beeper;
pub mod music_masker;
pub mod pattern_learner;
