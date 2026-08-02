//! logscope-fan — the egui-free orchestration core, extracted from `logscope-egui` so that
//! editing fan-out logic recompiles THIS zero-dependency crate in seconds instead of the whole
//! 37k-line GUI crate (and so the CLI / future headless consumers can take just the logic).
//!
//! Everything here is std-only and pure-ish (the compiler spawn in `verdict` and the socket
//! seams documented in `dispatch`/`enlist` are the only I/O); the 100-test suite moved with the
//! files. `logscope-egui` re-exports every module at its old `crate::` path, so no consumer
//! changed.
//!
//! The layer map (arrows = internal deps; nothing depends on anything outside this crate):
//!
//!   enlist ──────────────► dispatch ─────► ollama, bridge, localexec*, explore
//!   bestregion ──────────► verdict ──────► explore
//!   vimmode ─────────────► cmdpalette
//!   decompose ───────────► bridge
//!   consensus, quest, treeexport, stepwise, coordinator — leaves
//!
//!   (*localexec stays in logscope-egui: it drives `backend`, which is model/GUI territory.)

#[allow(dead_code)] pub mod bestregion;
#[allow(dead_code)] pub mod bridge;
#[allow(dead_code)] pub mod cmdpalette;
#[allow(dead_code)] pub mod consensus;
#[allow(dead_code)] pub mod coordinator;
#[allow(dead_code)] pub mod decompose;
#[allow(dead_code)] pub mod dispatch;
#[allow(dead_code)] pub mod enlist;
#[allow(dead_code)] pub mod explore;
#[allow(dead_code)] pub mod ollama;
#[allow(dead_code)] pub mod quest;
#[allow(dead_code)] pub mod stepwise;
#[allow(dead_code)] pub mod announce;
#[allow(dead_code)] pub mod machineid;
#[allow(dead_code)] pub mod reflexive;
#[allow(dead_code)] pub mod peerauth;
#[allow(dead_code)] pub mod natpath;
#[allow(dead_code)] pub mod rendezvous;
#[allow(dead_code)] pub mod trustkit;
#[allow(dead_code)] pub mod workerkit;
#[allow(dead_code)] pub mod buildfarm;
#[allow(dead_code)] pub mod stepverify;
#[allow(dead_code)] pub mod treeexport;
#[allow(dead_code)] pub mod verdict;
#[allow(dead_code)] pub mod vimmode;
