# AI-agent docs — index

Single entry point to the agent-facing documentation scattered across the
monorepo. This is the **catalog**; the **synthesis** — a Claude-specific vs
general-LLM split with the logscope-tree mapping — now lives at
[[claude-vs-general]]. ZBracket nav (`[[name]]`) is the next wiring step.

## Operational directives — read first

[**/CLAUDE.md**](../../CLAUDE.md) at the repo root is the agent's operating
manual: how to arm session capture, how to build without filling the disk,
which crates need `--no-default-features` and why, how to measure a bundle
before merging it, and what to verify before committing. It is rules; this file
is a catalog.

That also settles plan item 4 below for one role: **CLAUDE.md is the directive
file**, `HANDOFF.md` stays the per-task handover, `TOOLBOX.md` the per-project
tool list.

## Running a GUI from the sandbox

[**HEADLESS_GUI_TESTING.md**](HEADLESS_GUI_TESTING.md) — how to build, launch,
type into and screenshot an egui app here: Xvfb **plus a window manager**
(without one, synthetic keys land nowhere), `xdotool` with a slow enough delay
that a software-GL app keeps up, and `import` + the image viewer to actually
look at the result. Also the two ways the measuring instrument produced a
false alarm, and the rule that follows: probe before patching.

## The GPU on the author's machine

[**APPLE_M2_GPU_STACK.md**](APPLE_M2_GPU_STACK.md) — the M2 Max under Asahi:
Honeykrisp / Mesa 26.1.8 / Vulkan 1.4, the `ash` and `wgpu` crates that sit on it,
what has been MEASURED there (and what is still only an older doc's claim), the
driver faults that bite (`vec4<u32>` byte reads, absent submit callbacks, the
subgroup-matvec hang), and the three harness tiers for GPU work including the
real-hardware gate. Read it before writing or timing anything GPU-side.

## Catalog (≈40 docs)

### Domain guides — what the user actually does

- [`producer-workflows.md`](producer-workflows.md) — what five hours of
  the maintainer's FL Studio footage taught about trap/producer
  workflows: session shape, the idiom (kit, tempo, hats, 808, ramps,
  sends), the gestures that cost time and what `apps/studio` did about
  them, collaboration and music-video targets, the analysis method, and
  the maintainer's vocabulary. Read before deciding what the studio app
  is missing.

**games/unendlich** — by far the largest cluster. (Moved from `apps/`; the old path was purged, so references to `apps/unendlich` in older notes are historical.)
- `HANDOFF_I1_viewmodel.md`, `HAIKU_WORKER_BRIEF.md`, `docs/KNOWN-TEST-FAILURES.md`
- `handoff/` — ~25 task handoffs: `HANDOFF_{fog,weather,water_physics,plants,
  textures,textures_foliage,world_surface,worldmgmt,mesh_budget,mesh_partition,
  menus,menus_bin_wiring,items_tooltip,gamemode_button,audio_occlusion,
  bending_compose,dragon_size,frame_correlation,gpu_foliage_sway,rare_structures,
  record_cost,roamer_cap_wiring,simvectors,swords_creative}.md`, plus
  `BIN_PATCHES_air_behaviors.md`, `BIN_PATCHES_bending_ai.md`

**Other projects**
- `apps/depot/MANUAL.md` (operator manual), `apps/depot/DECISIONS.md` (the
  Linux/ecosystem defaults behind each choice), `apps/depot/TODO.md`; overview in
  [`../architecture/OFFLINE_SUPPLY_CHAIN.md`](../architecture/OFFLINE_SUPPLY_CHAIN.md)
- `apps/logscope/HANDOFF.md`, `apps/logscope/reference-docs/HANDOFF-imagegen.md`
- `apps/bo-suite/docs/HANDOFF.md`
- `apps/gitmind/TOOLBOX.md`, `apps/gitmind/DEPLOY_TO_A_PHONE.md` (transports, first contact
  from a browser, enrolment, depot credentials)
- `apps/tideshell/deploy/DEPLOY-FP5.md` (the numbered on-phone checks),
  `apps/tideshell/deploy/GITMIND-DEPLOY.md` (the deploy plan gitmind should run)
- `apps/fao-codec/egui-frontend/PORT_STATUS.md` (the egui port against fao_app);
  `apps/fao-codec/docs/` (the codec: CODEC_FORMATS, ARCHITECTURE, …)
- `apps/voiceterm/docs/power-user-brainstorm.md`
- `apps/ambient-learner/docs/` (MAIN_CHANGES, NEURAL_NETWORK_TRAINING,
  REMAINING_FEATURES, SWIPE_DEBOUNCE_AND_DBMAINT)
- `system/snapclone/docs/group-mailbox.md`
- `tools/drift/CLAUDE.md`

## Plan (next, on greenlight)

1. **This INDEX = the hub.** Each project keeps its docs in place; this file is
   the navigable catalog. (Done.)
2. **[[claude-vs-general]]** — separates Claude-specific guidance from what
   generalizes to any LLM, and maps the workflow onto **logscope's tree-based
   development**. (Done.)
3. **ZBracket navigation** — expose this catalog so web2qml/voiceterm users can
   walk it via ZBrackets (link scheme TBD against the w2q nav model).
4. **Convention** — settle on one filename (`HANDOFF.md` vs `CLAUDE.md` vs
   `TOOLBOX.md`) per role so new docs are discoverable.

Backlog item **A** in `maintenance/SCRATCHPAD.md`.
