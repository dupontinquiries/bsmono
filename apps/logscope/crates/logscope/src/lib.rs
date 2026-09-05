// lib.rs — exports for logscope-filter binary and any future external crates.
// The GUI binary (main.rs) declares its own mod tree independently.
// This lib only needs to expose the headless-safe modules.

pub mod vision_encoder;
pub mod vision_preprocess;
pub mod guardrails;
pub mod synthesis;
pub mod persona;
pub mod gpumetrics;
pub mod tuneprofile;
pub mod modelorg;
pub mod replicator;
pub mod residency;
pub mod liaison;
pub mod liaison_prompt;
pub mod liaison_relay;
pub mod stepview;
pub mod filesep;
pub mod msgview;
pub mod repoquery;
pub mod workspace_access;
pub mod thinkerwatch;
pub mod promptframe;
pub mod codecompare;
pub mod buildpolicy;
pub mod fanassemble;
pub mod fanplan;
pub mod reflex;
pub mod traceval;
pub mod exploration;
/// Which capabilities need the network, and which must never require it.
///
/// Declared late, and that is the point: this file existed in `src/` for a long
/// time with no `mod autonomy` anywhere, so it was never compiled and its two
/// tests never ran — while `cratecheck.rs`, `buildpolicy.rs` and
/// `buildsignal.rs` all cite `autonomy::Reach` in prose as the invariant they
/// uphold, and `buildpolicy`'s own test comments "The Reach invariant: no core
/// capability REQUIRES the network."
///
/// Three modules were enforcing a rule whose definition the compiler had never
/// seen. Found by `maintenance/tools/orphan_check.py`.
pub mod autonomy;
pub mod blackboard;
pub mod budget;
pub mod toolreg;
pub mod lang;
pub mod analyzer;
pub mod charts;
pub mod pipeline;
pub mod probe;
pub mod rotation;
pub mod silence;
pub mod shorten;
#[cfg(feature = "convo")]
pub mod conversation_db;
pub mod model_selector;
pub mod modelroles;
pub mod cpuplacement;
pub mod coreclass;
pub mod constrain;
pub mod smallrole;
pub mod reasontemplate;
pub mod refute;
pub mod rolebench;
pub mod branchtree;
pub mod sketchexpand;
pub mod hybridsplit;
pub mod tensorplace;
pub mod coresidency;
pub mod loadadvice;
pub mod rolequant_hint;
pub mod deliberate;
pub mod panelmode;
pub mod debaterun;
pub mod codefmt;
pub mod docroute;
pub mod manifest;
pub mod interleave;
pub mod dwell;
pub mod benchscenario;
pub mod convbench;
pub mod continuation;
pub mod codesmell;
pub mod benchartifact;
pub mod machinery;
pub mod longrun;
pub mod opsched;
pub mod deviceplan;
pub mod fleet;
pub mod delegateledger;
pub mod dispatchpick;
pub mod reachability;
pub mod peerrecord;
pub mod microdht;
pub mod pairflow;
pub mod phonesession;
pub mod mobilelayout;
pub mod remoteworkspace;
pub mod blastradius;
pub mod confinement;
pub mod externalwatch;
pub mod buildsignal;
pub mod quantcompare;
pub mod filewindow;
pub mod symbolfind;
pub mod dupscan;
pub mod unwired;
pub mod patchcheck;
pub mod anchor;
pub mod contextwidth;
pub mod modelfit;
pub mod capacity_tree;
pub mod linearize;
pub mod mdtable;
pub mod verifyplan;
pub mod consult;
pub mod panelvoice;
pub mod injecttime;
pub mod injectfork;
pub mod activemodel;
pub mod cratecheck;
pub mod diagreport;
pub mod correlate;
pub mod providers;
pub mod llm_backend;
#[cfg(feature = "bloc")]
pub mod bloc_engine;
#[allow(dead_code)] pub mod specdecode;
#[allow(dead_code)] pub mod multimodal;
pub mod markdown_core;
#[allow(dead_code)] pub mod batching;
#[allow(dead_code)] pub mod kvcache;
#[allow(dead_code)] pub mod deploy;
#[allow(dead_code)] pub mod memplan;
#[allow(dead_code)] pub mod arch;
#[allow(dead_code)] pub mod gemmplan;
#[allow(dead_code)] pub mod dedup;
#[allow(dead_code)] pub mod moe;
#[allow(dead_code)] pub mod mla;
#[allow(dead_code)] pub mod rope;
#[allow(dead_code)] pub mod quantfmt;
