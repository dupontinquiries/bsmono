# logscope-egui (Step 1 skeleton)

egui/eframe port skeleton of logscope's UI — the shell that replaces the
GTK4/Adwaita `src/ui/window.rs` + the ~500-line `components.rs` dropdown.

**Status:** compiles clean against the real pins (`egui`/`eframe` 0.34, rustc
1.96) — verified with `cargo check` (0 warnings, 0 errors).

## Run
```bash
cargo run --release        # needs a display (X11/Wayland); glow/OpenGL backend
```

## What's here
- `src/theme.rs` — `logscope.css` ported to an egui theme: the dark + amber
  (`#f6a800`) palette, JetBrains-Mono-style ramp, amber hover/active/selection.
  Adwaita named colors made concrete; tweak the consts to match the CSS exactly.
- `src/app.rs` — the shell: header (title + model picker + Rescan), the
  `AdwViewSwitcherBar` as a tab row (all 10 tabs: Sources/Peek/Tokens/Blocks/
  Rotate/Cargo/Chat/Code/Modify/Bloc), and placeholder panels.
- `src/main.rs` — eframe entry (glow backend; add the dash's wgpu→glow fallback
  by enabling eframe's `wgpu` feature).

## The point (the dropdown)
The model picker is the thing that was buggy in GTK. Here a background "scan"
thread streams `ModelEntry` batches over a channel; `ui()` drains them into a
`Vec`; the next frame paints. **No `popup_open` tracking, no `pending` defer
buffer, no splice-don't-rebuild, no signal races, no `UiScheduler`** — the whole
`components.rs` bug class is structurally gone. Hit **Rescan** to watch models
stream in without flicker or lost selection.

## Next
Port the tabs one at a time (cheapest first: Sources → Rotate → Peek → …).
The non-UI crates (the streaming engine, `bloc_engine`, `conversation_db`,
`model_selector`, `providers`) are reused unchanged — only `src/ui/**` is GTK.
Wire the real model list by swapping the demo scan for `providers::ModelEntry`
+ the existing `fetch_models_into_store` thread.

## Tabs (this build)
All 10 tabs are ported as real interactive panels (faithful to `src/ui/*`):
Sources (presets/command/filter/output/AI), Peek (syntax monospace + toolbar),
Tokens & Blocks (native egui bar charts, replacing the plotters PNGs), Rotate
(options → generated logrotate config), Cargo (diagnostics list + Ollama
explain), Chat (messages + input, uses the active model), Code (file list +
block + AI actions), Modify (clipboard history + Replace/Append/Patch), Bloc
(AST-scored blocs + editor). Each lives in `src/tabs/<name>.rs`. Deep logic
(real log streaming, tree-sitter highlighting, LLM streaming, conversation_db)
wires to the existing crates — the panels carry representative state so the
shell runs and looks right today.

## Backend note (glow, not wgpu)
This crate pins eframe to the **glow (OpenGL)** backend on purpose. eframe
0.34's *default* features select **wgpu**, whose default required limits include
`max_color_attachments: 8`; the Asahi GPU driver exposes only 5, so wgpu device
creation fails with `RequestDeviceError(LimitsExceeded …)`. glow doesn't request
those limits, so it just works (egui only ever uses one color attachment).
`main.rs` also sets `renderer: eframe::Renderer::Glow` explicitly.

The `xkbcommon: couldn't find a Compose file for locale "en_DK.utf8"` line is a
harmless winit/xkbcommon notice (no Compose file for that locale). Silence it if
you like with `LC_CTYPE=en_US.UTF-8` or by setting `XCOMPOSEFILE`.

ash/Vulkan: not needed here. eframe has only glow + wgpu backends; an ash path
would mean hand-rolling a winit+ash+egui renderer (right for unendlich's Vulkan
engine, overkill for a 2D tool). glow is the standard, lightest fix for this.

## Real data flow (this build)
Tabs now share an in-app log buffer (`src/analysis.rs::Doc`) instead of
per-tab demo data:
- **Sources** has a log-input editor + "Load into view →" (sets the Doc), plus
  a live filter (substring/regex/invert) that previews matching lines.
- **Peek** renders the Doc with line numbers + syntax coloring.
- **Tokens** computes real top-N token frequencies from the Doc.
- **Blocks** computes a real repeat histogram: lines are digit-normalized into
  templates (so `GET /health 200 0.4ms` variants collapse) and grouped by
  XxHash64 — with a legend of the top repeated patterns.
`analysis.rs` is pure and unit-tested (`cargo test analysis`, 3 passing), so the
computation is verified independent of the GUI.
