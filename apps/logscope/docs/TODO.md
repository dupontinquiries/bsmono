# Logscope — TODO / roadmap

Living backlog. Newest asks at top; design notes inline so each item is actionable.

## Models / testing

- [ ] **Text→image and image→text models** in the model pool, for better testing + fan-out.
  - *Why:* lets the suite exercise multimodal flows and fan a prompt across modalities (e.g. caption an
    image with one model, regenerate an image from the caption with another, compare) — richer eval than
    text-only.
  - *Shape:* add modality tags to the model registry (`text`, `text→image`, `image→text`); the model
    selector filters by what a command needs. `:use <name>` already switches the active model; extend the
    fan commands (`:fuzz`, `:fa`, `:mergectx`) so a fan can route legs to different-modality models.
  - *Backend:* image models ride the same provider seam as text (stub until a real backend/feature is on);
    image bytes flow through the existing attachment path + the new `chatdump` `files/` area.

## Student / teacher self-refresh (context compaction loop)

- [x] **`:refresh` / `:compact [ratio]`** — BUILT (planning + teacher prompt + staging + KV layer; the
  summary itself rides the staged turn and runs when a backend is loaded). The model reads its **own** output and writes a shorter,
  context-capturing summary, *staged* so later turns run against the compressed context. Re-running with a
  "refresh" focus lets the effective context keep growing without blowing the window.
  - *Concept:* same model as student **and** teacher. Teacher prompt: "compress the conversation so far into
    ≤N tokens that preserve every decision, constraint, and open thread; drop prose." The result becomes a
    **staging node** — a synthetic turn flagged `refresh` that downstream prompts use *instead of* the verbose
    turns it summarizes. Roll it forward: feed more → refresh again → rolling compaction.
  - *Reuse:* the synthesis commands (`:mergectx`, `:strategy`, `:segment`) already build a worker prompt that
    rides in the generated turn — `:refresh` is the same pattern with a compaction prompt + a target token
    budget (wire the existing token estimator to show before/after).
  - *Staging mechanics:* mark the summary turn so `build_context()` substitutes it for its source span; keep
    the originals in the tree (collapsed) so nothing is lost and a branch can re-expand. This is the
    in-app cousin of the transcript compaction the CLI already does.
  - [x] **Race + bad-jump diagnosis** (`:race`, `:why`/`:diagnose`) — BUILT (planning + divergence
    measurement + diagnostic prompt + commands; parallel exec + the card-on-finish notifier are the
    runner/UI step). Run the compacted context AND the full context off the same parent; surface each result
    as a notifier card (`cards`) to select; flag a regression and a smaller model diagnoses (via the
    `diff`-based redline) what the briefing dropped.
  - *Acceptance:* a long thread + `:compact 0.3` yields a turn ~30% the tokens that, used as context, lets the
    next generation continue coherently; token estimate drops; originals remain restorable.

## Command palette / prompts  (started)

- [x] **Palette help + argument suggestions** — `cmdpalette` (command metadata: usage, help, example,
  arg-kind; `matches()` completion; `suggest_args()` for ratio/count/model/refs/path). A live hint renders
  under the `:` cmdline (matching commands → usage → argument suggestions), and `:help [cmd]` prints usage.
- [x] **Specialized prompt library** — `prompts` (a keyed registry: next-step, unstick, tighten, simplify,
  critique, steelman, devil, test-plan, explain-error, name, extract-todos, eli5, rubber-duck, decisions).
  `:prompt <key>` / `:pr` stages one over recent turns. Grows without scattering string literals.
- [x] **Throwaway-model command suggester** — `:suggest` / `:sg`: builds `prompts::suggester_input` from the
  last 3 viewed messages + the command list, selects the smallest local model, and stages it;
  `prompts::parse_suggestions` turns the model's `:cmd args — reason` lines back into structured suggestions.
- [ ] **Suggestion tooltips (display side)** — run the suggester automatically as the view settles and render
  `parse_suggestions` output as inline, clickable tooltips/cards (`cards::CardQueue`) rather than a staged
  turn. This is the async/UI half (needs a live backend to produce suggestions); the prompt + parser are done.
- [ ] **Live model-name completion for `:use`** in the cmdline hint (the hint's `ArgCtx.models` is empty in
  `modal_banner`, which can't see the pool; thread the pool in or precompute names).

## UI

- [ ] **Scene as a full-window backdrop** (the big one). The 3D scene should fill the whole window behind
  *translucent* panels — in linear mode too — and the chrome should be lit by it.
  - *Mechanics:* move the scene render to a window-level background layer (paint the `glow_scene` callback
    into `ctx.screen_rect()` before the panels, both modes); lower panel fills to each scene's
    `panel-opacity` token from the theme bible (.6 Atlantis → .9 Marble); the spatial graph still renders on
    top in its area. Carved chrome then reads against the live scene.
  - *Risk:* egui layering + translucency + the glow callback at window scale — needs its own focused pass;
    can't be eyeballed here, so ship it isolated and iterate on hardware.
- [x] De-spiked stones / leaves (flat) / papers / stout-leaning triplet tree.
- [x] Carved chrome: panel frames, composer trough, buttons, tabs (relief + grain + runes via `Skin`).
- [ ] Per-scene chrome **Skin affinity** (bible): Carved→Foldout/Marble, Glass→Mistfall/Beach/Atlantis,
  Matte→Observatory, High-relief→Grove/Atlantis — and a `resolve(scene, skin, variant)` token bag so widgets
  read only `tok.*` (the anti-rot seam the bible specifies).
- [ ] Photoreal last-mile (deferred, needs renderer decision): PBR materials, IBL/reflections, volumetric
  shafts, MSAA, ACES tonemap; Atlantis caustics + glass transmission.

## Chats / workspaces  (started)

- [x] **`logscope/chats` dump** (`chatdump`): export a chat (turns + files) to `<ws>/logscope/chats/<id>/`,
  reload it, list dumps, stage files into a scratch mirror, import dumps from a directory of many workspaces
  (project structure respected, path-safe). Commands: `:dump [path]`, `:loadchat <dir>` / `:lc`,
  `:import-workspace <dir>` / `:iw`, `:summarize-files` / `:sf`.
- [ ] Capture **generated artifacts** into the dump's `files/` (right now `:dump` saves turns; wire the
  reply-artifact parser so derived files travel with the chat).
- [ ] **`:summarize-files`** end-to-end: stage the dump's `files/` into the **bubblewrap /tmp mirror**
  (`mount::isolate`), then fan a per-file summary across the model pool (multimodal-aware).
- [ ] **`:import-workspace`** → optionally mount the workspace under bubblewrap so the sandboxed tools
  (verify, summarize) can read the imported project directly.
