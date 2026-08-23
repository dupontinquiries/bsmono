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
#[allow(dead_code)] pub mod pairing;
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
#[allow(dead_code)] pub mod clusternode;
#[allow(dead_code)] pub mod discovery;
#[allow(dead_code)] pub mod lanwire;
#[allow(dead_code)] pub mod announce;
#[allow(dead_code)] pub mod machineid;
// The coordinator's own four, declared late — every one of them sat in src/
// with no `mod` line, so none was compiled and their eleven tests had never
// run. Found by maintenance/tools/orphan_check.py, verified individually with
// `rustc --edition 2021 --test` on the file verbatim (each has no imports at
// all outside its own test module), and none of their exported names collides
// with anything else in this crate.
//
// `reflex` is worth a word: the crate already declares `reflexive`, which is a
// completely different thing — NAT reflexive-address learning, not the
// coordinator's pattern-to-reaction spinal cord. The near-collision is a
// plausible reason nobody noticed one of the two was missing.
// The per-worker capability contract HiveMind_Strategy_Logscope.md makes
// mandatory: Duty says where a worker may run and engmode::prompt_for says what
// phase it is in, but nothing told a worker what it IS.
// One correct fence scan, shared. Seven files look for triple backticks with
// three different strategies and three of them had bugs; this is the primitive
// they should have been policies over. Honours the CommonMark run-length rule
// (a 4-tick block contains 3-tick blocks), which all four sites get wrong.
// The clock the conversation never had: an operator asked to "keep working for
// the next hour" and no turn carried a timestamp, so the phrase was unreadable.
#[allow(dead_code)] pub mod sessiontime;
#[allow(dead_code)] pub mod reply;
#[allow(dead_code)] pub mod capability;
#[allow(dead_code)] pub mod liaison_fan;
#[allow(dead_code)] pub mod reflex;
#[allow(dead_code)] pub mod pathfan;
#[allow(dead_code)] pub mod styles;
#[allow(dead_code)] pub mod reflexive;
#[allow(dead_code)] pub mod peerauth;
#[allow(dead_code)] pub mod natpath;
#[allow(dead_code)] pub mod rendezvous;
#[allow(dead_code)] pub mod trustkit;
#[allow(dead_code)] pub mod workerkit;
#[allow(dead_code)] pub mod buildfarm;
pub mod agentscore;
#[allow(dead_code)] pub mod ircsrc;
pub mod engmode;
#[allow(dead_code)] pub mod stepverify;
#[allow(dead_code)] pub mod treeexport;
#[allow(dead_code)] pub mod verdict;
#[allow(dead_code)] pub mod vimmode;
