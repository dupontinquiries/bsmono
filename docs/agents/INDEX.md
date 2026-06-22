# AI-agent docs — index

Single entry point to the agent-facing documentation scattered across the
monorepo. This is the **catalog** (done); the **synthesis** (a Claude-specific
vs general-LLM split, and ZBracket navigation for web2qml/voiceterm users) is
the next step — see "Plan" below.

## Catalog (≈40 docs)

**apps/unendlich** — by far the largest cluster.
- `HANDOFF_I1_viewmodel.md`, `HAIKU_WORKER_BRIEF.md`, `docs/KNOWN-TEST-FAILURES.md`
- `handoff/` — ~25 task handoffs: `HANDOFF_{fog,weather,water_physics,plants,
  textures,textures_foliage,world_surface,worldmgmt,mesh_budget,mesh_partition,
  menus,menus_bin_wiring,items_tooltip,gamemode_button,audio_occlusion,
  bending_compose,dragon_size,frame_correlation,gpu_foliage_sway,rare_structures,
  record_cost,roamer_cap_wiring,simvectors,swords_creative}.md`, plus
  `BIN_PATCHES_air_behaviors.md`, `BIN_PATCHES_bending_ai.md`

**Other projects**
- `apps/logscope/HANDOFF.md`, `apps/logscope/reference-docs/HANDOFF-imagegen.md`
- `apps/bo-suite/docs/HANDOFF.md`
- `apps/gitmind/TOOLBOX.md`
- `apps/voiceterm/docs/power-user-brainstorm.md`
- `apps/ambient-learner/docs/` (MAIN_CHANGES, NEURAL_NETWORK_TRAINING,
  REMAINING_FEATURES, SWIPE_DEBOUNCE_AND_DBMAINT)
- `system/snapclone/docs/group-mailbox.md`
- `tools/drift/CLAUDE.md`

## Plan (next, on greenlight)

1. **This INDEX = the hub.** Each project keeps its docs in place; this file is
   the navigable catalog. (Done.)
2. **`docs/agents/claude-vs-general.md`** — separate what is Claude-specific
   (tool names, sandbox quirks, our suspender workflow) from what generalizes to
   any LLM, and what maps onto **logscope's tree-based development** approach.
3. **ZBracket navigation** — expose this catalog so web2qml/voiceterm users can
   walk it via ZBrackets (link scheme TBD against the w2q nav model).
4. **Convention** — settle on one filename (`HANDOFF.md` vs `CLAUDE.md` vs
   `TOOLBOX.md`) per role so new docs are discoverable.

Backlog item **A** in `maintenance/SCRATCHPAD.md`.
