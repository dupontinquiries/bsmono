//! cockpit — the shared, Qt-free library of fleet-cockpit operations. Every frontend
//! (GUI, CLI, future TUI) builds on this one implementation so their capabilities stay
//! identical: building apps locally, deploying/pushing over ssh, importing a zip
//! (extract → detect → build → run / save / merge), reconciling against existing repos,
//! exploring merge strategies, reclaiming disk, managing access keys, and more.

pub mod repomap;
pub mod appcatalog;
pub mod browse;
pub mod bttransfer;
pub mod buildflags;
pub mod build_report;
pub mod buildsystem;
pub mod daemons;
pub mod deploy;
pub mod device_firewall;
pub mod fleet;
/// A LAN apk mirror: what may be cached forever, and what must not be.
pub mod apkcache;
/// The fastboot wire protocol and flash-script model, without a subprocess.
pub mod fastboot;
pub mod fbdiag;
/// First contact with a freshly-imaged phone: pairing code and join script.
pub mod join;
/// Trying a kernel on the spare slot, and getting evidence out when it dies.
pub mod kerneltry;
/// Kernel output parsed, normalised, and reduced to a crash signature.
pub mod klog;
/// One-button reroll across the test fleet: plan, dedupe, report.
pub mod reroll;
/// The fleet, remembered between runs: stable ids, IP churn, wipe detection.
pub mod roster;
/// A run in progress, folded from the event stream into what a UI draws.
pub mod runview;
/// Driving a fastboot exchange, with the wire behind a trait.
pub mod session;
/// A/B slots: flash a kernel to the spare half and keep a way back.
pub mod slots;
pub mod sparse;
/// Preflight: does this device reach a udev rule that grants access?
pub mod udevcheck;
pub mod flow;
pub mod identity;
pub mod importer;
pub mod incoming;
pub mod installer;
pub mod merge;
pub mod netshare;
pub mod ops;
pub mod packaging;
pub mod places;
pub mod preview;
pub mod rendezvous;
pub mod pydeps;
pub mod artifacts;
pub mod suite;
pub mod sysdeps;

pub use ops::*;
