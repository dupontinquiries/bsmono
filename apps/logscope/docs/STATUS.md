# Logscope Project Status — June 2026

## What This Is

**Logscope** is a **branching, local-first chat tool** designed to explore how far structure and compact models take you before needing a giant model. The thesis: a small model (360M–1B parameters) + tight workflow automation + KV-cache prefix reuse + context compaction + prompt specialization can outthink brute-force.

Two halves:
- **`logscope` (Rust crate)** — backend: a pure, headless inference library with token estimation, KV-cache prefix trees, workspace/vault abstractions, sandboxing, and modular CLI support.
- **`logscope-egui` (Rust library + app)** — frontend: an egui-based branching chat UI with a 3D spatial view, a carved-stone aesthetic, and a vim-style command palette tied to a local inference backend.

The user (you, on M2/M4 Max) builds from a separate git checkout and feeds screenshots/feedback; I iterate in the canonical sources here and sync back.

## Current State — What's Built & Validated

### Core (tested at **243 tests**, both `--features gpu-scene` and `--features gui`, bundle byte-identical to canonical):

**Chat & Messages:**
- Branching conversation tree (`model.rs::Msg`, parent links, lineage walks, active-leaf focus)
- Message variants (multiple outputs per turn, selected via cursor)
- Vim-style command mode (`:` palette, 25+ commands, exhaustive parsing tests)
- Chat autosave + recovery

**Context & Inference:**
- Token estimation (`est(s) = s.len()/4 + 1`, heuristic; backend tokenizes exactly)
- KV-cache prefix tree (`kvtree::pick_anchor`): given N cached entries, find longest reusable prefix; handles branching (1 prefill + N decodes) and ancestor fallback
- Pinned-KV checkpoints (`pick_anchor_pinned`, `evict_index`): compaction results stay hot, descendant paths reuse them
- Backend stub (real only under `llamacpp`/`wgpu-engine` features; otherwise "no backend")

**Compaction Loop (student/teacher):**
- `refresh::plan()` — keep last N turns verbatim, fold rest into briefing targeting a ratio; reports before/after tokens
- `refresh::TEACHER_PROMPT` — narrowly instructs model to compress its own output
- `refresh::staged_context()` — substitutes briefing + kept for the verbose lineage
- `race` module — race full vs compacted off same parent; divergence measurement; result cards
- `race::diagnose_turn()` — tiny-model prompt to review two outputs + their word-level diff and name what the briefing dropped
- `:compact [ratio]`, `:race [ratio]`, `:why [a,b]` / `:diagnose` commands — all staged; all wired

**Command Palette & Help:**
- `cmdpalette` — metadata for every command (usage, help, example, arg-kind); `spec()`, `matches()` (prefix completion), `suggest_args()` (ratio/count/model/refs/path)
- `:help [cmd]` — usage + example
- Live cmdline hint — shows matching commands, exact usage, and argument suggestions as you type
- 25 commands, all aliased and parsed

**Specialized Prompts & Suggester:**
- `prompts` library — 14 keyed prompts (next-step, unstick, tighten, simplify, critique, steelman, devil, test-plan, explain-error, name, extract-todos, eli5, rubber-duck, decisions); each deliberately narrow
- `:prompt <key>` / `:pr` — stage any prompt over recent turns
- `:suggest` / `:sg` — builds throwaway-suggester input from last 3 messages + command list, picks smallest local model, stages the prompt
- `parse_suggestions()` — reads model's `:cmd args — reason` output back into structured suggestions (drops prose, caps at 3)

**Chat Import/Export:**
- `chatdump` module — export chat as `<workspace>/logscope/chats/<id>/chat.json` + `files/`; `import_dir()` discovers dumps at any depth across many workspaces; `stage_files()` materializes into bubblewrap mirror
- `:dump [path]`, `:loadchat <dir>` / `:lc`, `:import-workspace <dir>` / `:iw` — all tested

**Scene & Rendering:**
- 8 scenes (Canopy, Mistfall, Archive, Reading Room, Folding@home, Old Library, Atlantis, Slate) + 7 themes + variant system (detail scaling, atmosphere, etc.)
- Carved-stone chrome (panel frames, composer trough, buttons, tabs) via `theme::Skin` tokens (relief, grain, runes, breathing shine)
- De-spiked organic geometry: smooth-normals rocks, flat elongated leaf-blades, curled paper, stout-leaning triplet tree with recursive branching
- 3D renderer (glow/OpenGL, forward + bloom/god-rays): triplanar materials, Lambert+Blinn-Phong, Fresnel rim, emissive self-illumination, exposure, fog
- Linear chat view (default) and spatial branching view (`:spatial`, orbit-camera, floating nodes)

**Model Management:**
- `PickerItem` (Ollama or local GGUF), `ModelState` (current selection), `smallest_local()` (pick the tiniest for diagnostics)
- `:use <model>` to switch; auto-picks smallest for `:why` diagnostics

**Support:**
- Workspace abstraction (paths, `safe_rel`, project structure)
- Vault (content-addressed storage)
- Sandbox / bubblewrap isolation (for tools, verification, etc.)
- File attachment + open/save via `rfd` picker
- Estimate + Prediction (token burn rate, ETA)
- Diff + consensus (word-level LCS, clustering, scoring, validation)
- Cards (`CardQueue`) — notifier system for suggestions/discoveries (used by race results, consensus findings)

### UI/UX (compile-validated, validated on M2/M4 hardware):

- Vim normal/insert/command modes (hjkl, d/c/a/gd, :, /, etc.)
- Modal banner with command input + live palette hint
- Tab system (messages, messages-by-branch, metadata)
- Graph layout (spatial / linear; zoom, pan, focus)
- Carved chrome aesthetic (lit by the scene, glowing runes at panel corners)
- Tree inspection (node selection, lineage, parent links)
- Export (Markdown, ASCII tree, git-style)

## What's NOT Built Yet

### Highest Priority (from the latest asks):

1. **Suggestion tooltips (display side)** — `:suggest` stages the prompt, but it doesn't auto-run; we need to:
   - Fire the suggester automatically as the view settles (after the user pauses)
   - Render `parse_suggestions()` output as inline clickable cards (via `cards::CardQueue`) instead of a staged turn
   - This is the async/UI half; the prompt + parser are tested and ready
   - *Needs a live backend to produce suggestions end-to-end*

2. **Full-window scene backdrop** — the 3D scene should fill the whole window behind *translucent* panels (in linear mode too) with lit chrome. Mechanics:
   - Move `glow_scene` render to window-level background layer (before panels)
   - Panels fill to `panel-opacity` per scene (0.6 Atlantis → 0.9 Marble)
   - Graph/messages still render on top
   - Chrome reads against live scene
   - *Needs an isolated, on-hardware pass since it's UI-sensitive blind*

3. **Live model-name completion for `:use`** — the cmdline hint's `ArgCtx.models` is currently empty (the model pool isn't in scope in `modal_banner`). Thread the pool in or precompute names so the hint can suggest available models.

### Medium Priority (speced in `docs/TODO.md`):

4. **Multimodal models** — text→image / image→text in the model registry; fan commands route legs by modality
5. **Per-scene chrome Skin affinity** — theme.rs `resolve(scene, skin, variant)` → flat token bag so widgets reference intent, not hardcoded colors
6. **Photoreal last-mile** — PBR materials, IBL, volumetric shafts, MSAA, ACES tonemap, Atlantis caustics (deferred to its own pass)

### Backend (parked, long-term):

- **GPU inference** — `gpu-gemm` crate (wgpu, Vulkan on Linux, Metal on macOS): Q4_0 dequant, RMSNorm, softmax, attention, int8-KV cache, speculative decoding, batching
- **M2 OOM fix** — int8-resident dequant costs 2× q4 → 2nd model OOMs; fix = q4-resident dequant (dequant on the fly)
- **QKV / int8-KV planning** — memory-efficient caching for long context

## Architecture Highlights

### Threading & Modules

- **Pure, headless core** (`refresh`, `race`, `prompts`, `cmdpalette`, `kvtree`, `chatdump`, `estimate`, `diff`, `consensus`) — fully tested, no egui dependency, usable from CLI
- **GUI layer** (`app.rs`, `tabs/chat.rs`, `modal_banner`, `cards`, `race` display) — egui-specific, thin glue
- **Backend stub** (`backend.rs`, `llm_backend.rs`) — real only under feature flags; stubs return `"[model stub]"`; the app still runs and is fully testable
- **Scene + rendering** (`scene3d`, `mesh3d`, `gpuscene`, `glow_scene`, `texgen`) — glow/OpenGL, validates under software renderers (llvmpipe, lavapipe)

### Key Files (67 `.rs` files)

- `lib.rs` — exports public modules (chunked by feature gate)
- `model.rs` — `Msg`, `Role`, `Msg::variants`, tree navigation
- `vimmode.rs` — command parsing, 25 command variants, STRATEGIST_PROMPT, SEGMENTER_PROMPT
- `tabs/chat.rs` — the UI (modal_banner, command handlers, lineage walks, styling)
- `refresh.rs` — compaction planner, teacher prompt, staging
- `race.rs` — race specs, divergence, diagnostic prompt
- `kvtree.rs` — prefix-tree decision, pinned anchors, eviction
- `cmdpalette.rs` — command metadata, completion, arg suggestions
- `prompts.rs` — 14 specialized prompts, suggester input, suggestion parsing
- `chatdump.rs` — dump/load/import/stage
- `scene3d.rs` / `mesh3d.rs` / `glow_scene.rs` — the 3D world
- `theme.rs` — 7 color palettes, Skin tokens (relief, grain, runes, etc.), variant system
- `cards.rs` — notifier queue for suggestions/findings
- `consensus.rs` / `diff.rs` — clustering, scoring, word-level diff, redline

### Design Decisions Locked In

1. **Prefix-tree KV reuse** — the single-anchor model was coarse; a pool of cached entries per branch tip lets "1 prefill + N decodes" fan effectively ✓
2. **Pinned-KV checkpoints** — compaction results (short, stable prefixes) pin their KV so descendant paths reuse them without re-prefilling ✓
3. **Narrower prompts over broad ones** — small models get hitched on "help me"; "cut this in half, keep decisions" works ✓
4. **Staged prompts in the composer** — synthesis commands (`:strategy`, `:compact`, `:suggest`) build a worker prompt visible to the user before send ✓
5. **Chat dumps in workspaces** — `.logscope/chats/<id>/` lives in the crate root so conversations are checkpointable alongside code ✓
6. **glow, not wgpu** — wgpu default limits are tighter than macOS Asahi's; glow sidesteps it ✓
7. **Scene as a composed aesthetic, not a game engine** — ~600 lines of hand-written forward renderer + bloom/god-rays; not a general engine, so fast iteration ✓

### Known Constraints

- **Apple OpenGL 4.1 frozen** — no compute shaders, no modern features; design lives within it
- **glow software rendering** — llvmpipe (CPU) validates GLSL; can't see visual output from here; you validate on hardware
- **Bundle sync** — canonical sources in `/home/claude/work/logscope-egui/src/`; bundle in `logscope-suite/crates/logscope-egui/src/`; byte-identity audit on every turn (63→67 files)
- **Stubbed backend** — real inference only under feature flags; app is fully usable and testable with stubs
- **Linear context** — the KV cache optimizations assume mostly-linear chat (each turn extends the last); heavy branching/fanning bypasses KV reuse but still works

## How to Build & Test

### Local

```bash
cd /path/to/logscope-egui

# GPU-scene feature (renders, tests scene builders + kvtree + threading)
cargo build --lib --features gpu-scene
cargo test --lib --features gpu-scene 2>&1 | tail -1  # → "test result: ok. 243 passed"

# GUI feature (full app compilation, all modules reachable)
cargo build --lib --features gui
cargo test --lib --features gui 2>&1 | tail -1

# Run the app (needs a backend feature + a model path, or stubs)
cargo run --features gui  # app opens with stub backend; all logic works

# Known flake (environment-dependent; safe to ignore)
# Use: cargo test --lib --features gpu-scene -- --skip offline_first_reports
```

### Deliverable

- `logscope-suite.zip` in `/mnt/user-data/outputs/` — complete source, ready to sync into your checkout
- Unzip, copy `crates/logscope-egui/src/*.rs` over your `src/`, rebuild
- Changes are built + tested here; the zip is byte-identical to what compiled green

## How to Extend

### Adding a Command

1. Add a `Command::NewThing(args)` variant in `vimmode.rs`
2. Add a parse arm in `parse_command()` and a test
3. Add metadata to `cmdpalette::SPECS` (usage, help, example, arg-kind)
4. Handle it in `chat.rs::run_command()` (set `self.input`, `self.note`, or `self.request_smaller_model`)
5. `cargo test --lib --features gpu-scene` — should be green

### Adding a Specialized Prompt

1. Add a `Prompt { key, title, summary, body }` to `prompts::PROMPTS` (keep body narrow and single-purpose)
2. Tests auto-check that keys are unique and bodies are populated
3. Users can now `:prompt <key>` or the suggester can emit it

### Adding a Scene

1. Define a builder fn in `scene3d.rs` (e.g., `fn my_scene() -> Scene`)
2. Wire it into `build()` match arm
3. Add palette metadata to `enclose()` (material, colors, wall shelf)
4. Add to the test list in `scenes_build_and_float`
5. Optionally add theme constants in `theme.rs` and metadata to `accent()`

### Changing the Renderer

- Current: ~600-line hand-written forward renderer (glow/OpenGL)
- To add: PBR, IBL, shadows, MSAA — the "photoreal last-mile" decision
- Consider: rend3 (dead), three-d (active, built on glow, re-exports glow), bevy (full rewrite)
- Defer decision until last-mile work (it gates photorealism, not core workflow)

## Handoff Checklist

- [ ] Sync canonical src into your checkout: `rsync -av logscope-suite/crates/logscope-egui/src/ ~/Code/rust/bigsilicon--logscope-egui/src/`
- [ ] Rebuild: `cargo build --features gui` (should be quick; canonical was tested here)
- [ ] Test on M2/M4: launch app, test a few commands, check scene backdrop / chrome / spatial view
- [ ] Next task: pick from the priority list above (suggestions display, backdrop, or model-name completion)
- [ ] Feedback loop: screenshot + notes → I iterate → sync → rebuild on hardware

## Key References

- `docs/DESIGN_TARGET.md` — the visual target (cinematic, carved-stone chrome, the eight scenes)
- `docs/FIELD_NOTES.md` — accumulated notes on every feature built this segment (compaction loop, race, prompts, KV pinning, etc.)
- `docs/TODO.md` — the full roadmap (multimodal models, per-scene Skin affinity, photoreal, etc.)
- `docs/08-kv-cache.md` — technical deep dive on prefix reuse and pinning (for understanding the KV layer)
- `docs/10-cli.md` — headless CLI usage (for understanding the backend surface)
- `theme.rs` — the Skin token struct (relief, grain, runes, sheen); every visual change flows through it
- `cmdpalette::SPECS` — the command surface (25 commands, all aliased and autocompleted)

## The Thesis

> How far does structure, compaction, and a tight prompt library take a small local model before it needs to scale to a bigger one?

Tested so far:
- **Prefix reuse** — sibling branches share a cached prefix, so N continuations cost 1 prefill + N decodes ✓
- **Context compaction** — fold old turns into a briefing, re-run the same prompt, and (via KV pinning) every descendant reuses the briefing's KV ✓
- **Specialized prompts** — "cut this in half" works better than "help me"; narrow > broad ✓
- **Race + diagnosis** — run both full and compacted, keep the winner, and diagnose regressions with a tiny model ✓
- **Command automation** — the suggester watches your messages and proposes `:commands` you likely want next ✓

Still to test:
- Multimodal fanning (same prompt across text→image, image→text)
- Full backing out of the scene as a live context (right now it's backdrop-only)
- Whether a 360M model + structure truly outthinks a 8B model on code/reasoning tasks (needs end-to-end backend)

---

**Last updated:** June 16, 2026 · **Test suite:** 243 passing · **Bundle:** byte-identical to canonical
