# aperture — design

The prompt was: *"a atom/vscode/zed/pulsar/kate successor in rust and egui
with AI autocomplete and smooth camera and animations that is lightning fast,
using the monorepo for code reuse and suite integration."* This document is
that prompt expanded into the decisions actually taken, so the next session
(or the next person) argues with choices, not with a vibe.

## Vision, sharpened

An editor whose three signature behaviors are each **one legible mechanism**:

1. **Fast** = O(viewport) frames. A rope for edits, a per-line lexer cache for
   color, and a renderer that lays out only visible rows + 2 overscan. File
   size never appears in the frame cost.
2. **Smooth** = one motion primitive. Scroll, caret, and font-zoom all move
   through the same critically-damped `Smooth` (exponential decay,
   framerate-independent, overshoot-free — unit-tested for both properties).
   "Camera feel" is two rate constants, not a zoo of tweens.
3. **AI you can see** = the loop pane (F2). Every completion request, its
   latency, and its outcome (or error) is a row; the provider, host, model,
   and token budget are editable fields applied live. The ghost is a
   three-state machine (`Idle → Pending{generation} → Shown`) whose every
   transition is a tested method. No black box: the pane *is* the loop.

Non-goals for v0, chosen consciously: LSP, tree-sitter, multi-cursor, tabs /
splits, plugins, IME, CJK-correct caret x, cloud TLS providers. Each has a
seam waiting for it (see TODO.md §aperture) — the point of v0 is that adding
them later touches one module, not the design.

## Architecture: three crates, dependency-poor on purpose

```
aperture-core      rope Buffer, invertible Op, grouped History, Editor,
  (dep: ropey)     LineLexer + SyntaxCache            — no I/O, no UI, no time
aperture-complete  CompletionRequest/Outcome, Debounce, Engine (worker
  (deps: none)     thread, newest-request-wins), providers: ollama FIM /
                   subprocess / static demo           — no UI, time as a param
aperture-egui      state.rs (the brain: bindings, ghost interplay — egui-free,
  (egui, eframe,   fully headless-tested), view.rs (the only module that
   uikit-egui, …)  draws), camera, ghost, palette, loop pane, eframe boot
```

Rules that keep the seams honest: **time is always a parameter** (`now_ms`),
so history coalescing, debounce, and the ghost fade are tested with fake
clocks; **the view may not contain editing logic** — if a behavior can't be
reached from `state.rs` tests, it doesn't belong in `view.rs`. That inversion
is why this editor has a real headless test story without `egui_kittest`.

**Edit as a value** (pattern from `apps/studio`): every mutation is an
invertible `Op`; undo/redo is folding inverses; typing coalesces into
gestures by a 900 ms window + char-class boundary. Accepting a ghost is just
`insert_text`, so *undo of an AI acceptance is ordinary undo* — one Ctrl+Z
removes the suggestion, the next removes your word. Collab/macros later reuse
the same value.

**Completion correctness is one integer.** `Editor.version` bumps on
mutation only (not caret moves); a request carries it as `generation`; an
outcome is shown only if its generation equals the *current* version. Stale
answers can't ghost a buffer that moved on — they still land in the loop
pane, labeled, because seeing the misses is the point. The engine's worker
drains its inbox and serves only the newest request (`engine.rs` test pins
this with a gated provider), so a slow model never queues up a backlog.

**Ghost feel**: typing the suggestion's own next characters *consumes* them
(no flicker, no re-request); divergence dismisses and re-arms the debounce;
Tab accepts all, Ctrl+→ accepts a word (newline+indent counts as one word so
repeated taps walk multi-line suggestions), Esc dismisses. Multi-line ghosts
render first line + a `⏎ +N lines` chip — honest about size without
reflowing text below.

## Reuse map (what the monorepo bought)

- `shared/uikit/uikit-egui`: `theme_bridge::apply_palette` (all 14 snapclone
  palettes; syntax colors derive from `PaletteColors`, so theming the editor
  = theming the suite), `fuzzy::score` for the Ctrl+P palette (same ranking
  feel as gitmind's pickers), `motion::ease` for the ghost fade,
  `scroll::area` in the loop pane, `desktop::viewport` for boot.
- `apps/logscope/logscope-fan/ollama.rs`: the dependency-free raw-TcpStream
  ollama client — adapted, credited, extended with FIM `suffix`, generation
  options, and chunked-transfer de-framing (an editor can't rely on the
  lucky Content-Length case). Second call site → future `shared/infer-kit`.
- `apps/studio`: the invertible-Op pattern (pattern reuse, not a dep).
- App conventions from `bo-suite-egui`: `[lib]` + `[[bin]]` with
  `app` feature default-on, eframe `glow`+`wayland`+`x11`, nested workspace,
  manifest entry (`runtime = "none"`).

## Providers

`Provider` is a blocking trait on a worker thread (simplest thing that can't
jank the UI). Three impls: **ollama** (default; env `APERTURE_OLLAMA`,
`APERTURE_MODEL`, default model `qwen2.5-coder:1.5b-base` — a base FIM model,
*instruct models produce chat, not code continuations*), **command**
(`APERTURE_CMD`; stdin gets `prefix NUL suffix`, stdout is the completion —
any script is a provider, which is the cloud escape hatch without TLS in the
binary), and **demo** (offline static table, so first run shows the whole
loop working with no server). Swapping is live via `Engine::swap_provider`.

## Verification status (honest ledger)

- **Compiled + tested, 47 green**: aperture-core (22), aperture-complete
  (10), aperture-egui state/camera/ghost/palette headless (14), plus one
  integration test driving the *real* Engine worker thread + demo provider
  end to end (type → debounce → request → outcome → ghost → Tab → undo).
  The tests caught three real defects during the session: an EPIPE masking a
  provider subprocess's stderr, a race in the coalescing test's own
  synchronization, and an engine-presence check sitting at the debounce
  instead of the send site.
- **Compiled only**: view.rs painting, eframe boot — no display in the build
  sandbox, so "the window opens and looks right" is unverified here.
- **Unverified end-to-end**: a live ollama roundtrip (no server in sandbox);
  the wire format is pinned by tests against the documented API instead.

## Gotchas hit (so the next session doesn't)

- **egui 0.34.x declares MSRV rustc 1.92** (whole family: ecolor, emath,
  epaint, epaint_default_fonts, egui-winit, egui_glow, eframe — and pinning
  0.34.2 doesn't help, the floor is per-minor). Ubuntu apt tops out at
  rustc 1.91. It compiles fine on 1.91 with `--ignore-rust-version` — the
  floor is policy, not syntax — but the flag must ride every cargo
  invocation (build *and* test). On a ≥1.92 toolchain, drop it.
- epaint 0.34 pulls `vello_cpu` — a heavy compile; first builds are slow,
  `profile.dev opt-level=1` is set in the workspace.
- eframe with `default-features = false` still *locks* `egui-wgpu`/wgpu in
  Cargo.lock (optional deps lock even when off) — ignore the lockfile bulk,
  it doesn't build.
- glow + `wayland`,`x11` features (not wgpu) is the deliberate mobile
  choice: PinePhone/A64-class GPUs are far happier on GLES than on Vulkan
  via wgpu.
- `caret x = col × char_w` assumes true monospace: correct for ASCII,
  wrong for CJK double-width (egui does no full shaping). Galley-measured
  x is the fix when it matters (TODO).
- tree-sitter deferred specifically because grammar crates version-pair
  with the runtime and churn their `Language`/`LANGUAGE` symbol — pin both
  sides together when adding.
- **eframe 0.34 App trait**: `ui(&mut self, ui: &mut egui::Ui, frame)` is
  now the *required* driver; `update(ctx)` is deprecated but still called
  (runner calls update then ui each frame). bo-suite-egui's "real update +
  empty ui" pattern therefore works but rides the deprecation — migrate it
  when touching that app next. New `logic()` hook runs even when the window
  is not visible.
- **egui 0.34 panel API**: `SidePanel`/`TopBottomPanel` are deprecated
  aliases of a unified `Panel::left/right/top/bottom`; `default_width` →
  `default_size`; ctx-attached `.show(ctx, …)` is deprecated for
  `.show_inside(root_ui, …)` — panels now nest in the root `Ui` that
  `App::ui` hands you. `CentralPanel` survives, also via `show_inside`.
- **egui 0.34 fonts access**: `ctx.fonts(|f| …)` yields an immutable
  `FontsView`; `row_height`, `glyph_width`, and `layout_job` all need
  `ctx.fonts_mut(|f| …)` now (glyph caching mutates).
- `snapclone-theme::PaletteId::all()` has 14 entries; anything that hardcodes
  9 (older docs) is stale.

## aperture-view: the physical viewer (second shell, same core)

The second deliverable of the session: *view files with logscope's physical
UI and scenes.* It exists to prove the layering — `aperture-core` didn't
change by one line to gain a 3D shell.

**The extraction decision.** logscope's physical UI (spatial → mesh3d →
gpuscene → glow_scene, + texgen and the textsurface decision layer, ~3.9k
lines, self-contained) became `shared/scene-kit`, because aperture is its
second consumer and the repo lifts kits at two call sites. logscope keeps
its copy untouched this session and migrates at the next scene co-change
(TODO.md §scene-kit) — the same adapt-then-merge route the ollama client
took. The rejected alternative was depending on `logscope-egui` wholesale:
123 modules of an LLM app compiled into an editor, for four files.

**What the kit gained at extraction** — the two gaps logscope's own
`docs/physical-ui-findings.md` names as blocking the whole direction:

1. **Per-vertex UVs** (12 → 14 floats, `location=5 in vec2 a_uv`).
   `Mesh3.uvs: Option<Vec<[f32;2]>>` — `None` on every existing builder
   packs (0,0) and changes nothing. `MAT_INSCRIPTION` (= 5.0, past the last
   texgen layer) samples a new `u_page` texture *in page space* — so words
   sit ON a face instead of floating. Relief for inscription faces falls
   back to paper (layer 0): parchment tooth under the ink, not the clamped
   moss layer. This was the findings doc's "enabling change", verbatim.
2. **`u_touch` is live.** The proximity helper existed, was uploaded every
   frame, and was never once set — dead code with a silent fallback. The
   kit's fragment shader now drives emissive from it (every surface warms
   faintly, inscribed ink more), and aperture-view sets it: click the page,
   the page answers, age advanced by frame delta so the fade paces the same
   at 60 and 120 Hz — the findings' own prescription.

**The scroll trick.** Rasterizing 42 rows for a 40-row window and sliding
the quad's UV window (`set_page_window` rewrites four UV pairs) means the
`Smooth`-animated scroll costs *zero* rasterization — the page re-inks only
when the integer window moves (`ensure_page`, version-gated; the GL side
re-uploads on version bump alone). Same motion primitive as the editor.

**The scene.** A stone floor, a wooden lectern carrying the inscribed
parchment (overlay-biased so props can never occlude the reading face —
mesh3d's physical-UI depth pull, used as intended), and four plinths behind:
paper, wood, stone, moss — RUN_ON_DEVICE's context-panel materials,
standing where the reader can see what the world is made of.

**Viewer verification ledger.** Headless-tested: viewer brain (7 — raster
window versioning, clamps, touch aging, UV offset), scene packing (2 —
14-float stride, UV ride-along, window slide), inscribe (3 — glyphs, span
colors, scale), plus the kit's 50 inherited tests updated to the new vertex
(53 total). Compiled + boot-smoked only: the GL path (shader edits, page
upload, callback) — no display in the sandbox, so the scene has *never been
seen*. The shader changes follow the existing material-branch patterns and
the attribute/stride math is mechanical, but visual verification is the
first thing to do on a real machine: `cargo run --bin aperture-view -- 
some_file.rs` (add `--ignore-rust-version` on rustc <1.92).

## Milestones next (each lands independently, seams already cut)

1. tree-sitter behind `LineLexer` (syntax.rs untouched elsewhere).
2. Streaming ghosts (`stream:true`, Provider grows a cancel).
3. File-fuzzy Ctrl+O over a workspace walk + `uikit_egui::fuzzy`.
4. Multi-cursor (`Selection` → `Vec<Selection>`, edits fan out).
5. LSP crate on the same worker/inbox pattern as the completion Engine.
6. Physical viewer: logscope migrates to scene-kit (re-export shims +
   `uvs: None` literal sweep); editing on the lectern (the ghost machine is
   shell-agnostic already); a Runestone theme via `textsurface::for_scene`.
