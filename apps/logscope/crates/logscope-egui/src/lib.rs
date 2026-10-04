//! logscope-egui as a library. Two binaries build on it: the egui GUI (main.rs, needs the `gui` feature)
//! and the headless script CLI (bin/logscope-cli.rs, builds lean without `gui`). The CLI's whole closure
//! is egui-free — backend, fabric, genguard, vimmode, consensus, decompose, bestregion, kvtree,
//! treeexport, script, runner — so those stay ungated; everything that touches egui/eframe/rfd is behind
//! `gui`, which is what lets the CLI compile without pulling in eframe/vello.

pub mod gitmind_session;
// GUI-gated: the gesture MACHINE is std-only and lives in shared/touchkit (charge.rs); what is
// here is the egui drawing, so a headless build must not carry it. Landing the isolation work's
// ungating is what surfaced this — nothing had built this crate headlessly since these were added.
#[cfg(feature = "gui")] pub mod holdmenu;
// GUI-gated for the same reason: replicator's DECISIONS are in logscope::replicator (pure, tested
// headlessly); this file is the widgets.
#[cfg(feature = "gui")] pub mod replicatorui;
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
pub mod read_harness; // params-adaptive evidence harness + the injection-safe git/file sanitizer (c11)
pub mod line_calls;   // one-line tool grammar + refusal ladder for fence-incapable models (c11)
pub mod enginestrip;
#[allow(dead_code)] pub mod fabric;
pub mod genguard;
pub mod patchmode; // unified-diff patch mode: model edits via diffs instead of full-file rewrites
pub mod engprofile; // composable engineering autopilot profiles (hardware/native/gpu/optimizer/planner)
pub use logscope_fan::consensus;
pub use logscope_fan::vimmode;
pub use logscope_fan::treeexport;
pub use logscope_fan::bestregion;
#[allow(dead_code)] pub mod convoprov;
pub mod diagfeed;
/// The preflight overrides as described, clamped, UI-toggleable knobs.
pub mod engine_knobs;
/// Grouping the engine feed into foldable, copyable sections.
pub mod loggroup;
pub mod kvtree;
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
/// The hop from a spoken utterance to a `panelvoice::Ask`.
pub mod voiceask;
pub mod voice_intent;
pub mod imprint;
pub mod physui;
/// What the in-scene physical UI PANEL is made of, per scene — board, rim, suspension, lamp. Pure data.
pub mod physkin;
pub mod repetition; // pure repetition detection + graded recovery (decode + agent loops)
#[allow(dead_code)] pub mod tripwire; // pure deterministic scanners over in-progress code → review-point signal
#[allow(dead_code)] pub mod segment; // detect explain↔code / new-thought seams → pause, branch, checkpoint, predict
#[allow(dead_code)] pub mod taskclass; // classify one-shot/long-horizon × maintenance/greenfield → build posture
pub use logscope_fan::stepwise; // checkpoint protocol: prompt block, step-marker parsing, auto-continue policy, checklist
pub mod debatepanel; // live debate strip: last COMPLETE line per debater, blind members marked
pub mod voiceconfirm; // voice interjection review, on voiceterm ConfirmDialog shape
pub mod readability; // the measure: line length, the one readability factor the palette cannot fix
pub mod revealpace; // reveal at semantic boundaries when the model outruns reading speed
pub mod micresponse; // sound -> physical motion: leaf inertia, rune onsets, frame-rate-independent decay
// Migrated to scene-kit (see its Cargo.toml comment above): zero source
// changes since extraction, confirmed byte-identical before removing the
// local copy.
pub use scene_kit::textsurface; // which scene object carries words, and which shapes legibly can
pub mod chatgesture; // swipe -> variant/branch, with the guards that keep it from fighting the scroller
pub mod mobilelayout; // width rules: egui horizontal() does not wrap, so narrow rows overlap
pub mod replicatorlayout; // where the fan's columns go, and what Enter does when it is open (pure)
pub use logscope_fan::stepverify; // per-step verification: the feedback signal the coordinator lacked
pub mod docorg; // document-organization convention: front-matter, generated index, branch merge
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod plaque_scroll;
pub mod assets;
pub mod mediagen;
/// Image IN: pairing a vision projector with its model, and the command that follows.
pub mod visionpair;
/// The UX audit as a tool a worker bee can loop against.
pub mod uxloop;
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
#[allow(dead_code)] pub mod diag;
#[allow(dead_code)] pub mod prechecks;
#[allow(dead_code)] pub mod liaisonhud;
/// Which model answers a prompt, and what its reply may assert.
pub mod promptroute;
/// The generated personas a prompt can be routed to.
pub mod workerroster;
#[allow(dead_code)] pub mod metricspanel;
#[allow(dead_code)] pub mod bracecheck;
#[allow(dead_code)] pub mod contextsrc;
#[allow(dead_code)] pub mod ctxplan;
// Label de-collision for the chat graph — see the module docs. Not yet wired
// into the renderer; the geometry is the part worth getting right first.
#[allow(dead_code)] pub mod graph_labels;
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
#[cfg(feature = "gui")] pub mod roomtheme;
#[cfg(feature = "gui")] pub mod prefs;      // the few choices that must survive a restart (via config-kit)
#[cfg(feature = "gui")] pub mod sysstatus;  // the status light, derived from real signals — every one can veto
// UNGATED: pure data (no `use` at all) and `workspace_exec` — which is ungated — builds its
// `RunState`/`Doing` from it, so the gui gate made `--no-default-features` uncompilable from
// d218468d5 on. Nothing built the lean configuration in between; the fleet builds defaults.
pub mod runcard;    // the ACTIVE THREAD card: what the run is doing, as label/value rows
#[cfg(feature = "gui")] pub mod chords;     // keyboard chords for the quick-action chips, with the collision rules as tests
#[cfg(feature = "gui")] pub mod ctxcard;    // context-card facts: size line and a topic-match figure with its denominator
#[cfg(feature = "gui")] pub mod context_probe;
// Moved to the egui-free core (logscope-fan) so `deploy` can compose it with staffing and
// placement; re-exported here so the path every reader knows still resolves.
pub use logscope_fan::fan_policy;
#[cfg(feature = "gui")] pub mod probe_exec;
#[cfg(feature = "gui")] pub mod queen;
#[cfg(feature = "gui")] pub mod relief_policy;
#[cfg(feature = "gui")] pub mod noderole;   // what a node IS (root/fork/pruned/artifact…), for the label above its text
// UNGATED with runcard: `runcard::elide_task` reuses `nodelabel::elide_words` (deliberately, per
// its comment — a fourth `elide` was the mistake dupscan caught). Its only crate import is
// `spatial`, which is already ungated pure math.
pub mod nodelabel;  // node label text (word-safe elision) and which labels get drawn
#[cfg(feature = "gui")] pub mod navkeys;    // freecam movement keys by keyboard layout (QWERTY WASD / Dvorak .oeu)
#[cfg(feature = "gui")] pub mod nodetint;   // graph node colours derived from the space, audited for distinguishability  // chrome DERIVED from the room you are standing in, with a legibility audit over all 16
#[cfg(feature = "gui")] pub mod ui;
#[cfg(feature = "gui")] pub mod wizard;
#[cfg(all(feature = "gui", feature = "gpu-scene"))] pub mod scene_preview;
#[cfg(feature = "gui")] pub mod filepicker;
// UNGATED: diag::write_zip calls workspace::zip_store unconditionally, so `--no-default-features`
// did not compile. zip_store is a hand-rolled stored-ZIP writer over std and needs no egui — the
// gate was wrong, not the call. (Fix from the agent-isolation work, re-applied on this lineage.)
pub mod workspace;
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
// UNGATED for the same reason, one layer down: workspace calls merge::scan_items, which is string
// scanning.
#[allow(dead_code)] pub mod merge;
pub use logscope_fan::coordinator; // gui gate was vestigial: zero gui refs (fan compiles it standalone)
// UNGATED: std::process + std::fs only. Also makes the verifier testable headlessly instead of
// pulling eframe/glow into a test run.
#[allow(dead_code)] pub mod sandbox;
// ── agent isolation (std-only; from the agent/isolation work) ────────────────────────────────
// Ordered as that work argues they should be: capability first, channel integrity second,
// neutralisation third, detection last and never as a gate.
#[allow(dead_code)] pub mod isolate;
#[allow(dead_code)] pub mod taint;
#[allow(dead_code)] pub mod audit_log;
#[allow(dead_code)] pub mod review_gate;
// governor is declared now that AgentConfig carries gate + audit_log — the whole imported set is
// live. What remains from the sender's "known incomplete" list: Verdict::Refused needs a match arm
// in tabs/chat.rs, nothing outside Governor calls review_gate::apply, and the capability lease
// broker is unbuilt.
#[allow(dead_code)] pub mod governor;
#[allow(dead_code)] pub mod convograph; // pure (no egui) + imported by lean modules → not gui-gated
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod walkcam;
// Migrated to scene-kit (see its Cargo.toml comment above): zero source
// changes since extraction, confirmed byte-identical before removing the
// local copy. Still pure math (no egui) + imported by lean modules → not
// gui-gated.
pub use scene_kit::spatial;
// Picking + interaction states for the 3D scene: it had no hit testing at all,
// so nothing in it could be pointed at. Pure math, same gating as spatial.
#[allow(dead_code)] pub mod scenepick;

// Four modules that sat in src/ with no `mod` line — never compiled, 22 tests
// that had never run. Found by maintenance/tools/orphan_check.py; each checked
// against the iq3 case first (a superseded duplicate, where declaring builds a
// second copy of working code). None is that: no public item of any of them is
// defined elsewhere in this crate except `gen_profile::detect`, which shares
// only a name with `mount::detect`.
//
// Gated by what each actually imports, following the convention above.
#[allow(dead_code)] pub mod components;   // std only
#[allow(dead_code)] pub mod gen_profile;  // std only
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod hud_pack;   // eframe::egui geometry
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod touch_nav;  // touchkit + crate::spatial
// MIGRATED to scene-kit, same terms as glow_scene/spatial/textsurface. §75
// measured it: scene-kit's copy is a superset — 48 shared public functions,
// 5 extra there, 0 unique here — and the 253-line diff was almost entirely
// one added field (`Mesh3::uvs`) propagating through struct literals, NOT the
// two-way divergence §18 assumed from the diff count alone.
//
// No `theme` coupling to unpick, unlike glow_scene, so this is a straight
// re-export. `sun_clock` here delegated to `math_kit::daylight` and
// scene-kit's does too, so the curve is unaffected.
#[cfg(feature = "gui")] pub use scene_kit::mesh3d;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod pearl_input;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod panel_rows;
pub mod scene3d;
// MIGRATED to scene-kit — the last of the four, and the only one §18's
// "genuinely diverged" verdict was right about. It was two-way: scene-kit had
// the inscription materials (MAT_INSCRIPTION/2), the UV attribute plumbing and
// the u_page/u_page2 samplers; logscope had `ray_uv`, live at two call sites
// in the chat scene for touch picking.
//
// So neither direction was a copy. `ray_uv` was ported into scene-kit beside
// `dir_uv` (documented as its inverse, and only correct as a pair) WITH its
// two tests, then this became a re-export. The `theme::power_mode` coupling
// is handled by the same `perf::set_overrides` push that glow_scene needed —
// already wired in app.rs.
#[cfg(feature = "gui")] pub use scene_kit::gpuscene;
// MIGRATED to scene-kit, but the DIRECTION was the opposite of the others and
// getting it wrong would have cost ~330 lines. §75 measured it: 98% of
// scene-kit's texgen was already inside logscope's, and logscope's was 2.3x
// the size — logscope's grew after extraction (LAYERS went 5 -> 11, with a
// real `roughness` curve and more palettes), so scene-kit held the STALE copy.
//
// Migrating logscope onto scene-kit naively — the move that was right for
// glow_scene and mesh3d — would have deleted that work. scene-kit was updated
// FROM logscope first; only then is this re-export lossless.
#[cfg(feature = "gui")] pub use scene_kit::texgen;
// MIGRATED to scene-kit. 98% of this module's content was already there and
// scene-kit's copy additionally carries `PageTex`; the only lines unique to
// logscope's were `theme::power_mode`/`theme::detail` calls, which
// `scene_kit::perf::set_overrides` now receives from `app.rs` each frame — so
// the in-app power control still drives render resolution, which is the one
// capability this migration had to preserve.
//
// Re-exported rather than removed so `crate::glow_scene::` call sites keep
// working, the same shape as `spatial` and `textsurface` before it.
#[cfg(feature = "gpu-scene")] pub use scene_kit::glow_scene;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod estimate;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod multiparty;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod directives;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod longterm;
#[cfg(feature = "gui")] #[allow(dead_code)] pub mod interject;
#[allow(dead_code)] pub mod mount; // pure (git/workspace, no egui) + imported by lean modules → not gui-gated
#[allow(dead_code)] pub mod agent_render; // the agent transcript in the loaded model's chat template (pure)
#[allow(dead_code)] pub mod workspace_exec; // pure (std-only agent loop over an isolated tmp workspace) → not gui-gated
pub mod phase_guide; // engmode's Fsm wired to the agent loop: observation → signal, phase → prompt (StatefulGuide)
pub mod plancheck;   // is a plan step actually DONE? decided from what the run wrote, not from turns elapsing
pub mod planrun;     // plan steps → coordinator subtasks → runnable branches (the planner's missing caller)
pub mod branchshape; // per-branch conversation shape (depth/len/tokens/loop/pruned) — what the mind palace's rooms are built from
pub mod oracle;      // is a change CORRECT? test-status delta, vacuous-test detection, differential port checking
pub mod workergen; // corpus-recombined workers plus a hand-tuned legendary roster
pub mod matrix;  // rows a message falls through, columns it can take, a director at each branch
pub mod liaison; // operator-said vs organization-knows; the narrow protocol from the design note
#[allow(dead_code)] pub mod lanes; // concurrent generation lanes over one resident model (scheduling tested via the LaneGen seam)
#[cfg(feature = "gui")] pub mod tabs;

#[cfg(feature = "gui")]
pub mod addons;
#[cfg(feature = "wgpu-engine")]
pub mod toolfilter;   // the wgpu engine's grammar sampler: acceptor + tokenizer
pub mod llm_remote;   // an `Llm` on a llama-server: grammar-constrained tool fences + prompt cache
