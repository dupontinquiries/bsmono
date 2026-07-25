//! logscope-egui as a library. Two binaries build on it: the egui GUI (main.rs, needs the `gui` feature)
//! and the headless script CLI (bin/logscope-cli.rs, builds lean without `gui`). The CLI's whole closure
//! is egui-free — backend, fabric, genguard, vimmode, consensus, decompose, bestregion, kvtree,
//! treeexport, script, runner — so those stay ungated; everything that touches egui/eframe/rfd is behind
//! `gui`, which is what lets the CLI compile without pulling in eframe/vello.

pub mod gitmind_session;
pub mod repo;
// ── CLI closure (egui-free; always built) ────────────────────────────────────
#[allow(dead_code)] pub mod filescan;
pub mod filesummary;
#[allow(dead_code)] pub mod swarmview;
#[allow(dead_code)] pub mod vision_cli;
#[allow(dead_code)] pub mod tts_cli;
#[allow(dead_code)] pub mod media;
#[allow(dead_code)] pub mod ws_registry;
pub mod spatial_audio;
pub mod workbench;
pub mod tab_layout;
pub mod log_gestures;
pub mod chat_layout;
pub mod backend;
#[allow(dead_code)] pub mod fabric;
pub mod genguard;
pub mod patchmode; // unified-diff patch mode: model edits via diffs instead of full-file rewrites
pub use logscope_fan::consensus;
pub use logscope_fan::vimmode;
pub use logscope_fan::treeexport;
pub use logscope_fan::bestregion;
#[allow(dead_code)] pub mod kvtree;
#[cfg(feature = "wgpu-engine")] #[allow(dead_code)] pub mod anchorpool; // tiered warm-resume KV pool (Hot f32 / Warm int8 / Cold host-RAM)
pub use logscope_fan::decompose;
#[allow(dead_code)] pub mod devloop;
pub use logscope_fan::enlist;
pub use logscope_fan::dispatch;
pub use logscope_fan::explore;
pub use logscope_fan::ollama;
pub use logscope_fan::bridge;
#[allow(dead_code)] pub mod localexec;
#[allow(dead_code)] pub mod speculate;
#[allow(dead_code)] pub mod voicefan;
#[allow(dead_code)] pub mod voice_ingest;
#[allow(dead_code)] pub mod reaction;
#[allow(dead_code)] pub mod whisper_pool;
#[allow(dead_code)] pub mod voice;
pub mod voice_session;
pub mod voice_intent;
pub mod imprint;
pub mod picking;
pub mod physui;
pub mod repetition; // pure repetition detection + graded recovery (decode + agent loops)
#[allow(dead_code)] pub mod tripwire; // pure deterministic scanners over in-progress code → review-point signal
#[allow(dead_code)] pub mod segment; // detect explain↔code / new-thought seams → pause, branch, checkpoint, predict
#[allow(dead_code)] pub mod taskclass; // classify one-shot/long-horizon × maintenance/greenfield → build posture
pub use logscope_fan::stepwise; // checkpoint protocol: prompt block, step-marker parsing, auto-continue policy, checklist
pub mod docorg; // document-organization convention: front-matter, generated index, branch merge
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod plaque_scroll;
pub mod assets;
pub mod mediagen;
pub mod decode;
pub mod voice_dict;
#[macro_use] pub mod log;
pub mod fastpath;
pub mod voice_confusion;
pub mod voice_intent_es;
pub mod voice_intent_fr;
pub mod voice_intent_de;
pub mod voice_lang;
#[allow(dead_code)] pub mod explain;
#[allow(dead_code)] pub mod contextsrc;
#[allow(dead_code)] pub mod ctxplan;
pub use logscope_fan::quest;
pub use logscope_fan::verdict;
#[allow(dead_code)] pub mod script;
#[allow(dead_code)] pub mod runner;
#[allow(dead_code)] pub mod chatdump;
#[allow(dead_code)] pub mod refresh;
#[allow(dead_code)] pub mod prompts;
pub use logscope_fan::cmdpalette;

// ── GUI + its supporting modules (need egui/eframe/rfd/tree-sitter) ───────────
#[cfg(feature = "gui")] pub mod analysis;
#[cfg(feature = "gui")] pub mod app;
#[cfg(feature = "gui")] pub mod model;
#[cfg(feature = "gui")] pub mod theme;
#[cfg(feature = "gui")] pub mod ui;
#[cfg(feature = "gui")] pub mod wizard;
#[cfg(all(feature = "gui", feature = "gpu-scene"))] pub mod scene_preview;
#[cfg(feature = "gui")] pub mod filepicker;
#[cfg(feature = "gui")] pub mod workspace;
#[cfg(feature = "gui")] pub mod vault;
#[cfg(feature = "gui")] pub mod ast;
#[cfg(feature = "gui")] pub mod vertices;
#[allow(dead_code)] pub mod fabric_net; // pure std (TCP fabric) — available to the lean CLI too, not gui-gated
#[cfg(feature = "gui")] pub mod context_graph;
#[cfg(feature = "gui")] pub mod retrieval;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod rag; // BM25 retrieval over the LogScope corpus (RAG)
#[cfg(feature = "gui")] pub mod pipeline;
#[cfg(feature = "gui")] pub mod diff;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod scroll;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod cards;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod coach;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod race;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod genviz;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod palette;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod corpus;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod merge;
pub use logscope_fan::coordinator; // gui gate was vestigial: zero gui refs (fan compiles it standalone)
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod sandbox;
#[allow(dead_code)] pub mod convograph; // pure (no egui) + imported by lean modules → not gui-gated
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod walkcam;
#[allow(dead_code)] pub mod spatial; // pure math (no egui) + imported by lean modules → not gui-gated
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod mesh3d;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod pearl_input;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod scene3d;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod gpuscene;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod texgen;
#[cfg(feature = "gpu-scene")] pub mod glow_scene;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod estimate;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod multiparty;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod directives;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod longterm;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod interject;
#[allow(dead_code)] pub mod mount; // pure (git/workspace, no egui) + imported by lean modules → not gui-gated
#[allow(dead_code)] pub mod workspace_exec; // pure (std-only agent loop over an isolated tmp workspace) → not gui-gated
#[allow(dead_code)] pub mod lanes; // concurrent generation lanes over one resident model (scheduling tested via the LaneGen seam)
#[cfg(feature = "gui")] pub mod tabs;
