//! cockpit — the shared, Qt-free library of fleet-cockpit operations. Every frontend
//! (GUI, CLI, future TUI) builds on this one implementation so their capabilities stay
//! identical: building apps locally, deploying/pushing over ssh, importing a zip
//! (extract → detect → build → run / save / merge), reconciling against existing repos,
//! exploring merge strategies, reclaiming disk, managing access keys, and more.

pub mod appcatalog;
pub mod browse;
pub mod bttransfer;
pub mod buildflags;
pub mod deploy;
pub mod fleet;
pub mod flow;
pub mod identity;
pub mod importer;
pub mod installer;
pub mod merge;
pub mod netshare;
pub mod ops;
pub mod places;
pub mod preview;
pub mod reclaim;
pub mod suite;

pub use ops::*;
