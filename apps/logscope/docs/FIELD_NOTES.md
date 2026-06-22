
---

## References absorbed → carved-frame chrome (step 1 of the design target)

Got the spatial brief + three synthesized UI mockups (Archive forest, Mistfall gorge ×2). The decisive insight:
these scenes are defined by **composition, not object lists** — each is built around one emotionally-significant
centerpiece (oculus / arch / basin / island-tree / grove), with the conversation graph nested in its focal void.
Full distillation in `docs/DESIGN_TARGET.md`; the raw brief is in `docs/refs/spatial-brief.txt`.

The mockups also pin down the chrome: **carved-material frames over translucent cores, with glowing rune
accents**, theme-adaptive. Started that this turn, fork-independent (no renderer decision needed):

- `theme::carved_frame` dresses a panel's margin band as carved stone — a double beveled relief lip (lit
  top-left, shadowed bottom-right), the active material's grain scattered along the border, and faint breathing
  **runes** at the corners (`rune_mark`, a tiny deterministic stroke-alphabet) in the theme accent. Driven by the
  `Skin` tokens, so it restyles with everything else.
- `theme::panel` now paints that frame after layout (in the empty inner-margin band, so it never overdraws
  content) and makes the panel core slightly translucent so the scene reads faintly through the stone.

Also fixed a pre-existing latent test bug surfaced while validating: `gpuscene::pack_is_sorted_and_shaped` packs
**12 floats/vertex** (pos3, normal3, color4, mat, **emissive**), and the asserts knew that, but the depth-sort
loop still strided by the old 11-float layout (`chunks(33)`, `t[11]/t[22]`) and read misaligned positions.
Corrected to `chunks(36)` / `t[12]/t[24]`.

Validated: builds under both feature sets; suite **220/220** (the lone failure is the known environment-only
sandbox network probe, unrelated). Carved frame is compile-validated; the look is hardware-pending as always.

Next per the build order: composition (rebuild each scene around its single centerpiece, graph in the void),
then per-theme graph-as-system, then the photoreal last-mile (reflections / volumetric shafts / PBR — the
deferred renderer decision).

---

## Fix: bundle/canonical source drift (the `m.emissive` build error)

The emissive channel (12th vertex float, for self-illuminating geometry — runes, lit text) had been added to
the canonical tree across `mesh3d.rs` (the `Mesh3.emissive` field + all constructors), `gpuscene.rs` (pack +
shader `a_emissive`/`v_emissive`), and `glow_scene.rs`/`chat.rs`, but three of those bundle copies
(`mesh3d.rs`, `glow_scene.rs`, `tabs/chat.rs`) never got synced — so the shipped bundle had `pack_sorted`
referencing `m.emissive` against a `Mesh3` that lacked the field. My builds only ever compiled the *canonical*
tree, so the broken bundle slipped through.

Fixed by resyncing every `.rs` file canonical→bundle (verified byte-identical) and, going forward, **compiling
the bundle source itself** — a clean from-scratch build of the synced tree passes under `--features gpu-scene`
and `--features gui`, suite **221/221**. Process change: every turn now re-diffs all files and clean-builds the
delivered source, not just the working copy.

---

## Chrome step 1 cont'd — carved trough for the composer

The composer (main chat input) now reads as a recess carved into the stone, matching how all three mockups
draw it. `theme::carved_trough` paints the *inverted* relief of a raised slab — shadow on the top/left lip you
look over, light on the bottom/right floor — plus a soft inner cast-shadow band under the top edge for depth,
and material grain along the edges. Like `carved_frame`, it paints only in the margin band after layout, so it
never touches the text. Theme-adaptive via the active material + `Skin` tokens; no runes (inputs stay calm).

Wired in `chat.rs`: the composer frame's rect is captured and the trough painted over it. Front-component
physicality now covers panels (frames), buttons (carved), and the composer (trough); tabs already use
`carved_tab`.

Validated: builds under `--features gpu-scene` + `--features gui`, suite **221/221**, only the 2 pre-existing
egui deprecation warnings. Per the new process rule, the entire delivered bundle tree was re-diffed
byte-identical to the built+tested canonical before packaging. Look is hardware-pending.

Still next, as its own focused pass: the composition rebuild (each scene around one dominant centerpiece, the
graph nested in its focal void) — held back deliberately because it's aesthetically sensitive and I can't see
the result, so it deserves a dedicated turn rather than being rushed in behind the chrome.

---

## De-spiked the organic geometry (stones, leaves, papers, tree)

Acted on hardware feedback ("stones still too spiky… leaves and papers too… tree should be shorter and more
angled"), matching the three.js prototype's recipe (now stashed in `docs/refs/`, alongside the 8-scene
`theme-bible.md`). The spikes were all from coarse meshes + incoherent noise + flat shading:

- **`rock`** — was a coarse 8×6 sphere displaced by *per-vertex random* noise (neighbours uncorrelated → spikes)
  and flat-shaded (faceted). Now a 16×12 sphere displaced by a *smooth low-frequency field* (neighbours move
  together), gently squashed into a half-buried river stone, with *smooth vertex normals* over shared topology
  → rounded and weathered. Feeds every cairn / rune-rock / stone node.
- **`leaf_cluster`** — was 7 pointy 4-vertex diamonds spiking outward. Now ~10 denser rounded **leaf cards**
  (8-point teardrop fans) lying mostly tangent to the crown, tipped slightly up → soft foliage.
- **`paper_fold`** — was a sharp tent ridge (`1−|u+w|`). Now a smooth cosine bow with edges curling up and a
  faint undulation → a gently curling vellum page, no crease.
- **Canopy tree** — was a single tall vertical trunk. Now **stout, leaning** (trunk dir `(.17,1,.07)`, len 4.2)
  with **recursive triplet branching** (trunk → 3 → 9, `tree_branch`) and a **rounded two-tier crown** so light
  dapples through — matching the prototype's `branch()` + `rotation.z` lean and the Grove brief.

All four scale with the detail variant (tessellation) and keep their seamless materials. Validated: builds
under both feature sets, suite **221/221**; bundle re-diffed byte-identical to the built+tested canonical.
Hardware-pending visually.

Absorbed the new theme bible: **8 scenes** (Foldout, Grove, Mistfall, Marble/Reading, Beach, Observatory,
Atlantis, + proposed Lost Program), each with palette/lighting/material/UI-mapping tokens and a `panel-opacity`
— confirming the next big piece: the **scene as a full-window backdrop** behind translucent panels, lit, in
linear mode too. That's the dedicated next turn.

---

## Flat leaves · Old Library centerpiece · Atlantis scene

Three pieces of hardware feedback / asks:

- **Leaves now flat.** The rounded octagon cards read as round discs; replaced with a flat elongated
  **leaf-blade** silhouette (pointed tip → rounded body → base, 6-point fan) lying flat against the crown,
  every vertex sharing the blade's single normal — a thin planar leaf, not a puff. Still scales with detail.
- **Old Library re-centered.** It was falling through to the stone `islands()` fallback (hence the stones).
  New `old_library()` centerpiece: a **lectern with a large open, faintly-glowing tome** (`emissive`) and
  loose pages drifting up around it (a page turning itself — presence), inside the existing bookshelf walls.
  No stones.
- **Atlantis added** (8th scene). New `ATLANTIS` palette (deep-teal abyss, blue-obsidian floor, cyan accent,
  glass chrome) + `atlantis()` builder: a **glass dome on a stone drum** over a wet blue-obsidian floor, an
  interior gallery ring, and rising glowing bubbles; smooth glassy drum walls via `enclose`. It's in the theme
  dropdown and the scene test.

Validated: builds under both feature sets, suite **221/221** (`scenes_build_and_float` now covers Atlantis +
Old Library), bundle re-diffed byte-identical to the built+tested canonical. Look hardware-pending.

(Caustics on the Atlantis floor and proper glass transmission are deferred to the photoreal/PBR pass; the
floor uses the wet-water material + the existing reflection for now.)

---

## Chat/workspace dump (`logscope/chats`) + command grammar; multimodal + roadmap TODOs

Of the four roadmap asks, built the most self-contained, backend-independent one fully — chat import/export as
a workspace dump — plus added the requested TODOs and planned the rest (`docs/TODO.md`).

- **New `chatdump` module** (non-gui, so the CLI can use it too; path-safe via an inlined `safe_rel`):
  - `ChatDump { id, title, created, turns, files }` → `export_to_workspace(root)` writes
    `<root>/logscope/chats/<id>/chat.json` + `files/<rel>`; `load(dir)` reloads; `list(root)` enumerates;
    `stage_files(scratch)` materializes the files into a mirror (for the bubblewrap /tmp area); `import_dir(dir)`
    walks a directory of *many* workspaces for `logscope/chats/<id>/chat.json` at any depth (skips `target/`),
    respecting project structure.
  - Four tests: round-trips through a workspace, rejects `..`/absolute path traversal, stages into a mirror,
    discovers dumps at depth.
- **Commands** added to the grammar (`vimmode`) + wired in `chat.rs`: `:dump [path]` (saves the active
  lineage), `:loadchat <dir>`/`:lc` (reload via the existing `load_imported`), `:import-workspace <dir>`/`:iw`
  (discover dumps), `:summarize-files`/`:sf` (staged; per-file summaries await a model). Parse tests added.
- **`docs/TODO.md`** created: the requested **text→image / image→text models** for testing + fan-out (with a
  registry/modality-tag plan); the **student/teacher `:refresh`/`:compact`** self-summarization loop design
  (same model as student+teacher, staged compaction node, rolling context growth); and the **full-window
  scene backdrop** plan (window-level background layer + per-scene `panel-opacity` translucency + lit chrome).

Validated: builds under `--features gpu-scene` + `--features gui`, suite **225/225** (4 new chatdump tests),
bundle re-diffed byte-identical to the built+tested canonical (file counts checked too).

Deferred deliberately: the student/teacher loop (needs a real backend to be meaningful) and the full-window
backdrop (aesthetically sensitive, needs its own focused pass) — both speced in `docs/TODO.md`.

---

## Student/teacher context compaction (`:compact` / `:refresh`) + pinned-KV checkpoints

Built the compaction loop the three aims asked for — more context, faster decode, KV reuse for certain paths.
The pure planning/staging/KV-bookkeeping is real + tested; the summary text itself rides a staged turn and is
produced by the model when a backend is loaded (same pattern as `:strategy`/`:mergectx`).

- **New `refresh` module** (non-gui, pure, tested): `plan(turns, keep_recent, ratio)` keeps the last N turns
  verbatim and folds the rest into a summary targeting `ratio` of the folded tokens, reporting before/after
  token counts (`saved_tokens`, `ratio_saved`). `TEACHER_PROMPT` is the "compact your own context, preserve
  decisions/constraints/open-threads, drop prose" instruction; `teacher_turn(folded, target)` builds the
  staged prompt; `staged_context(summary, kept)` substitutes the briefing + kept turns for the verbose
  lineage. 4 tests.
- **KV layer (`kvtree`)** — the connection to the engine's prefix-tree cache: `prefill_savings(prompt, reuse)`
  (tokens a hit avoids prefilling); `pick_anchor_pinned(..., pinned)` (longest prefix still wins; on an
  equal-length tie a **pinned** checkpoint beats an ordinary tip, so every branch descending from a
  compaction reuses the briefing's KV); `evict_index(len, pinned)` (drops the oldest UNPINNED entry, so a
  compaction checkpoint stays hot). The existing `pick_anchor` (called by `backend.rs`) is untouched. 3 tests.
- **Command** `:compact [ratio]` / `:refresh` (also `:cp`), wired in `chat.rs`: walks the active lineage,
  plans the compaction, stages the teacher prompt in the composer, and reports `context N→M tok (−X%)` plus
  "the briefing becomes a pinned KV checkpoint reused by every branch below it." Parse test added.

How the three aims land: the briefing is a fraction of the folded tokens → **more room**; fewer prompt tokens
→ **faster prefill/decode** (`prefill_savings` quantifies a hit); the briefing is a short, stable prefix that
gets **pinned** so descendant paths reuse its KV instead of re-prefilling history.

Validated: builds `--features gpu-scene` + `--features gui`; suite **232/232** (4 refresh + 3 kvtree new);
bundle re-diffed byte-identical to the built+tested canonical (64 .rs files).

Honest scope: planning, staging, token accounting, and the KV pin/evict decisions are real and tested. The
actual self-summarization executes only with a real inference backend (stubbed unless `llamacpp`/`wgpu-engine`
is on); and wiring the staged summary back in as the live context (the substitution `staged_context` models)
is the next step to make the loop close end-to-end in the app.

---

## Race the compacted vs full context + bad-jump diagnosis (`:race`, `:why`/`:diagnose`)

Closing the compaction loop the way it should close: don't commit to a compaction blind — run both and keep
the winner; if the compaction lost something, a tiny model says what.

- **New `race` module** (gui-gated; uses `diff` + `refresh`; pure + tested):
  - `dual_run(full, summary, kept) -> [RunSpec; 2]` builds the FULL-context run and the COMPACTED-context run
    (briefing + kept, via `refresh::staged_context`) with token costs — the compacted one is provably cheaper.
  - `divergence(full_out, compact_out)` = `diff::change_ratio` (0..1): how big a jump the compaction made.
  - `result_card(label, out, peer_div)` → `(key,title,body,action)` for the notifier (`cards::CardQueue`)
    surfaced as each run finishes — the "select from the runners" piece.
  - `DIAGNOSE_PROMPT` + `diagnose_turn(full_ctx, summary, full_out, compact_out)`: the smaller-model reviewer
    prompt; sees both answers + their word-level redline (empty context/briefing sections are omitted) and
    names what the briefing dropped. 4 tests.
- **Commands** (`vimmode` + `chat.rs`): `:race [ratio]` reports the dual-run token tradeoff (full vs compacted,
  −%) and the workflow; `:why`/`:diagnose [a,b]` flags a bad jump and stages the smaller-model diagnosis from
  two outputs — explicit refs, or by default the active leaf (compacted path) vs a sibling (full run) — noting
  the measured divergence and to `:use` a smaller model. Parse tests added.

Validated: builds `--features gpu-scene` + `--features gui`; suite **236/236** (4 new race tests); bundle
re-diffed byte-identical (65 .rs files).

Honest scope: the plan, the divergence measurement, the notifier-card content, and the diagnostic prompt are
real + tested. The actual parallel execution ("run both"), the async card-on-finish notifier, and the model
runs ride the existing fan queue + `cards` + model pool — those hooks (suggest a card when each branch
finishes; enqueue the two branches; auto-pick the smallest local model for `:why`) are the remaining
UI/runner integration, and need a live backend to exercise end-to-end.

---

## `:why` actually spawns the tiniest local model

Made the "spawn a tinier model" half of the race concrete. Before, `:why` told you to switch models by hand;
now it picks the smallest local model itself.

- `model::smallest_local(items)` — index of the local model with the fewest parameters (file size breaks
  ties; Ollama rows skipped, since the diagnosis is meant to be a cheap local file). Pure + tested.
- `LocalModel` already carries `params_billions` + `size_bytes`, so this is a real signal, not a name parse.
- Wiring: `:why` sets a `request_smaller_model` flag; `ui()` (where the model pool is in scope) consumes it
  right after the modal banner, selects the smallest local model, and updates the note to name it
  ("diagnosing the bad jump with smollm2-360m …") — or says to load a small GGUF if none is present.

Validated: builds `--features gpu-scene` + `--features gui`; suite **237/237** (new `model::pick_tests`);
bundle byte-identical (65 .rs files). The model swap + prompt staging are real; the actual diagnostic
generation still needs a live backend.

---

## Command palette help + argument suggestions + a specialized prompt library + a throwaway suggester

The new workflow has a lot of commands; this makes them discoverable and keeps small models on-task with
narrow prompts. Four pieces, all logic pure + tested, with the cmdline hint compile-validated.

- **`cmdpalette`** — command metadata (usage / help / example / arg-kind) for the workflow commands, with
  `spec()` (name or alias), `matches()` (prefix completion), and `suggest_args()` (ratio / count / model /
  refs / path). A live hint now renders under the `:` cmdline — matching commands, then the exact command's
  usage, then argument suggestions — and `:help [cmd]` prints usage + an example. 3 tests.
- **`prompts`** — a keyed library of 14 SPECIALIZED prompts (next-step, unstick, tighten, simplify, critique,
  steelman, devil, test-plan, explain-error, name, extract-todos, eli5, rubber-duck, decisions). Each body is
  deliberately narrow so a small model doesn't get hitched. `:prompt <key>` / `:pr` stages one over the recent
  turns; `:prompt` lists them. 1 test (keyed, unique, populated).
- **Throwaway suggester** — `:suggest` / `:sg` builds `suggester_input` from the last 3 viewed messages + the
  command names, picks the smallest local model, and stages it; `parse_suggestions` reads the model's
  `:cmd args — reason` lines back into structured suggestions (prose dropped, capped at 3). 2 tests.
- The `SUGGEST_COMMANDS` prompt is format-locked so a tiny model emits parseable lines.

Validated: builds `--features gpu-scene` + `--features gui`; suite **243** tests, all green except the
pre-existing environment-dependent `sandbox` network-probe flake (filtered → 0 failed). Bundle byte-identical
to the built+tested canonical (67 .rs files).

Remaining (in `docs/TODO.md`): rendering the parsed suggestions as inline clickable tooltips/cards and running
the suggester automatically as the view settles — that's the async/UI half and needs a live backend to
produce suggestions; the prompt + parser are done. And the full-window scene backdrop UI redesign still wants
its own focused on-hardware pass.

---

## Project Status Documentation

A comprehensive **STATUS.md** has been written for handoff to the next phase. It covers:
- What the project IS and its thesis
- What's BUILT + validated (243 tests, all passing except the pre-existing environment flake)
- What's NOT built yet (suggestion tooltips display, backdrop, live model-name completion, multimodal, etc.)
- Architecture highlights (pure headless core, feature-gated modules, scene builders)
- How to BUILD, TEST, and EXTEND
- The handoff checklist and references

This document is the entry point for someone picking up the project. Point them to `docs/STATUS.md`.
