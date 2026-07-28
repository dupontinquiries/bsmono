# CADscope — the expanded build prompt

*A parametric, touch-first, keyboard-deep, AI-assisted CAD program for the monorepo. This document expands the original one-paragraph prompt into a grounded, buildable specification. Every referenced system was verified against the uploaded artifacts: the monorepo git bundle (`logscope-c4-288.bundle`, commit `e71cc7c3`), `skydribble-v1p26.zip`, `v2_tar.zst`, and the offline Rust 1.96 toolchain in `rustup.zip`.*

---

## 0. Mission

Build **`apps/cadscope`**: a new monorepo application that takes the `apps/logscope` engine as its base — its liaison chat, its graph-based fan-out orchestration, its egui shell conventions — and modernizes the rendering layer with the **skydribble** wgpu engine to become a fast, effective, *learnable* CAD program in the spirit of Shapr3D on iPad and Siemens NX. It runs on a Linux phone and feels genuinely good there: slick, touch-compatible UI built on the monorepo's `uikit-egui` + `touchkit` stack. It is equally deep on a keyboard, where modal vim/helix motions — already prototyped in `games/blackstar/tools/spine_grower.html` — let you *grow* strange and beautiful geometry without touching a mouse. When you're in a pinch, you summon the **liaison chat** and generate models with local AI, watching progress in real time and iterating conversationally; logscope's exploration-tree fanning keeps large projects lean and shippable. One subsystem is genuinely new and must be built from scratch: **a real texturing system** (the current state is a pile of Python export scripts, not a system). Finished models flow outward three ways: as **niftyplot** diagrams compatible with `apps/voiceterm` (including the spec'd-but-stubbed `hardware` kind for electrical engineering), as **game assets** injected into the monorepo's `games/` titles through their forge-pattern editors and the `v2` asset pipeline, and as **live collaborative sessions** over the `shared/snapcore` DHT.

---

## 1. Ground truth — what actually exists, and where

Before specifying anything new, here is the verified map of the parts the prompt references, including three naming corrections. Build agents: treat these paths as authoritative; do not re-derive them.

**The monorepo** (clone of `logscope-c4-288.bundle`, branch `main`). Taxonomy per `LAYOUT.md`: `apps/` (user-facing), `system/` (daemons: snapclone, photosync), `shared/` (the kit layer), `games/`, `tools/` (peek, drift, rustcheck), `vendor/` (opendht), with a build **orchestrator** at `build/build.sh` driven by `manifest/projects.toml` + `manifest/targets.toml` — there is deliberately *no* single cargo workspace. `UI_STRATEGY.md` declares egui the primary frontend tier for every app; Qt/GTK surfaces are archived-but-maintained kits. CADscope is born egui-primary and never needs a Qt twin.

**`apps/logscope`** — the base. A self-contained cargo workspace:

- `crates/logscope` — the headless library. Contains `liaison.rs` (*"the one component allowed to talk to the operator in prose"* — everything else emits structured `Event`s; the Liaison provably never invents: on an empty tree it answers "nothing observed") and `liaison_prompt.rs`, plus `exploration` (the branch/event tree) and `residency` (memory accounting).
- `crates/logscope-fan` — **the graph-based fanning approach, already extracted as an egui-free, std-only crate** precisely so new consumers like CADscope can take the logic without the 37k-line GUI. Modules: `dispatch`/`ollama`/`bridge`/`enlist` (fan work out to N local LLM workers), `explore` + `verdict` + `bestregion` (the exploration tree and how branches get judged), `consensus`, `decompose`, `pathfan` (`:paths` — fan out N *build-path archetypes* for one feature and compare them side by side), `stepwise`, `treeexport`, `coordinator`, and — critically for us — `vimmode` (modal control: Normal mode, `:` colon-commands with aliases, fzf-lite fuzzy ranking, `z` rotation prefix, `space` leader, joystick→Dir4 mapping) and `cmdpalette`. The layer map in `lib.rs` is explicit and dependency-clean.
- `crates/logscope-egui` — the egui 0.34 app (eframe, glow backend). Directly reusable ideas: `convograph.rs` (the durable conversation spine — *"the session is not the context window"*), `context_graph.rs`, `genviz.rs` (per-worker token-rate visualization with stable slots and EMA smoothing — the "see their progress in real time" substrate), `liaisonhud.rs`, `fabric.rs`/`fabric_net.rs` (multi-worker fabric), `mediagen.rs` (a *pure argv builder* for local `sd-cli` stable-diffusion.cpp image generation — unit-tested flags, spawn kept separate), `texgen` (procedural material colors), and a whole embryonic 3D stack: `mesh3d.rs` (software forward renderer into one `egui::Mesh`), `scene3d.rs` (baked themed scenes, per-vertex procedural "texture"), `gpuscene.rs` (GL-free GPU-ready core: true 4×4 view-projection reproducing `OrbitCam::project` pixel-for-pixel, `pack_sorted` interleaved triangle soup, matching GLSL) and `glow_scene.rs` (the actual GL pass behind feature `gpu-scene`). Its own doc comments call out the next rung: *"a depth-tested wgpu render pass via an egui paint callback with real shaders."* **CADscope is that next rung.**
- `crates/gpu-gemm` — the pure-Rust **wgpu→Vulkan LLM inference engine** + kernel validator (locked at wgpu 29.0.3 in `Cargo.lock`), excluded from default builds. This is the "power AI features" muscle when no ollama daemon is around.
- `DESIGN_generative_scenes.md` — the **sanitization boundary** design: external generators (LLM, image model) never produce code or touch the scene graph; they emit a constrained spec JSON (`RoomSpec`) that a clamp/match validator turns into safe primitives, with GBNF grammar-constrained decoding so the model *cannot* emit anything else. CADscope generalizes this pattern from rooms to parts.

**skydribble** (`skydribble-v1p26.zip`, standalone — *not currently in the monorepo tree*; folding it in is part of this work). Rust, **wgpu 22 + winit 0.29** ("same proven combo as clawbound/soldier"). What we take: `render.rs` (srgb surface, `Depth32Float`, MSAA 4, single dynamic-offset uniform buffer at 256-byte strides, 8192 objects, sky→scene→HUD in one pass), `meshgen.rs` (procedural unit meshes), `model.rs` (direct glTF `scene.gltf + scene.bin` loader that bakes node hierarchies, normalizes orientation/scale), `shader.wgsl` (validated offline by `naga` in dev-deps — keep this convention), and above all `world.rs`'s core idea: **"every shape is both a mesh instance and an analytic SDF, so the ball and car collide with exactly what you see."** That dual representation is the seed of CADscope's geometry kernel. Bonus: skydribble's sixteen map styles are adapted *from* logscope's `scene3d` ROOMS table, and `net.rs` already rides the monorepo's vendored `netcode` crate over the **snapcore DHT store** exactly per `games/NETCODE_DHT_ANALYSIS.md` — control plane (profile → `SessionRecord` announce/resolve) plus its own UDP data plane. The collaboration path is pre-blazed.

**`games/blackstar/tools/spine_grower.html`** — correction: the file is `spine_grower.html` (underscore), 26 KB, alongside `part_editor.html`. The prototype's motion grammar (from its own help panel): `h l` yaw, `j k` pitch, `H L` roll, `[ ]` **helix twist**, `0` straighten heading, `w b` thicken/narrow, `e` extrude ridge (spike), `p` taper pinch (waist), `f` flare (bell), `1–5` cross-section (○ □ △ ✦ ⬡), `~` wobble flow, `space` drop a node (bulge), `t` branch off tip, `y` return to parent, `m` mirror symmetry. This *is* the keyboard modeling language; CADscope promotes it from an HTML toy to a first-class modal mode.

**`v2_tar.zst`** — decompresses to `v2/`: **47 versioned Python scripts** that are the newest sculpted iteration of blackstar's asset pipeline (`texture_export_v1..v13`, `texture_audit_v1..v2`, `part_harvest_v1..v9`, `library_shrink_v1..v8`, `detail_apply_v1..v5`, `install_models_v1..v4`, `process_models_v1..v9`, `obj_diagnose_v1..v3`, `shape_pairs_v1`, `model_metadata_v1`, `extract_models`). Read them as *prior art and requirements documentation*, not as code to keep: e.g. `texture_export_v13` explains the real problem (merging materials collapses UVs; the fix is per-material parts, each IQM pointing at its real baseColor, with the engine's `<name>.png` / `norm_<name>.png` / `spec_<name>.png` naming convention), and `texture_audit_v2` defines the three levers that matter for pool hygiene (resolution caps, exact dupes by content hash, near-dupes by perceptual dHash, with projected-savings ranking).

**`shared/nifty-core` + `apps/voiceterm`** — niftyplot does not live inside voiceterm's app code; it lives in the shared kit `shared/nifty-core` (`niftyplot.rs` parser → `GraphDoc`, `graph_layout.rs` force-directed, `mermaid.rs`) and voiceterm consumes it via `src/niftyplot_view.rs`. The format spec is `apps/voiceterm/docs/niftyplot-spec.md`: five kinds dispatched by `%kind:` — `graph` (implemented), and `gantt` / **`hardware` (components, pins, traces, clearances)** / `sequence` / `state` all *"spec'd but stubbed."* The parser is deliberately permissive (unknown directives → warnings; conflicting constraints → largest-working-subset + debug footer). **CADscope's electrical-engineering export is therefore not "compatibility glue" — it is the first real implementation of niftyplot's `hardware` kind, contributed upstream into `nifty-core` where voiceterm gets it for free.**

**`shared/snapcore`** — the pure-Rust DHT. Layer 1 (routing) is hardened Kademlia (k-buckets, XOR metric, per-bucket replacement cache, good/questionable/bad liveness). The layered plan in `lib.rs` is exactly what collaboration needs: **cluster overlays** ("a cluster IS a shared mailbox you join" — dense peer tracking regardless of XOR distance, bridge nodes between mailboxes), radio/battery-aware **scheduling windows** (ephemeral data never wakes the radio — this matters on the phone target), and a transport story (STUN, UPnP/NAT-PMP, relay in pure Rust; hole-punch FSM pending; daemon-as-proxy for the integrated builds). Storage semantics (TTL, re-announce dedup, multi-value keys) proven in snapclone-microdht. Time is a caller-supplied logical `u64` everywhere, so it simulates deterministically — `sim.rs` exists.

**`shared/uikit/crates/uikit-egui` + `shared/touchkit`** — the touch UI stack, egui **0.34**. uikit-egui ships the five-layer *carved* widget surface (L0 egui → L1 helpers → L2 carved painters → L3 semantic widgets → L4 `Skin` tokens: restyling edits values, never call sites — ported *from logscope*), the `gesture_layer` (adapts `egui::InputState` to touchkit's `TwoTierGesture`, "snapclone's FP5-tuned swipe feel... with no per-app gesture code"), `theme_bridge`, plus `adaptive`, `dock`, `vertical_tabs`, `glass`, `livebg`, `nav`, `a11y`. touchkit itself is the backend-agnostic recognizer: per-frame pointer-set diffing → directional swipes, fling **momentum**, double-tap, long-press. The parity note in `manifest/projects.toml` for bo-suite-egui records a hard-won phone rule we must obey: on compact widths, prefer the vertical-tabs sidebar over whole-surface swipes *when a view contains sliders/drags that would race the page-swipe gesture* — a CAD viewport is exactly such a view.

**`games/` integration surface** — `games/EDITORS.md` names the two patterns every game editor follows. The one that matters here is the **forge pattern** (`soldier/src/buildscript.rs`, blackstar's `arena_forge.py`): *"a stable, documented EXTERNAL op format — names not IDs, ops as a sequence not a snapshot, per-op error tolerance... and an `export()` that round-trips so tooling can read → propose → hand back a full document. Authorable by a human, a GUI, or an LLM shown the schema."* CADscope's model history should *be* a forge-pattern document, which makes game injection, AI authorship, and collaboration all the same problem. `games/shared/netcode` + the `BSGAMES_HANDOFF_*` env protocol (bsgames games centre) are the launch/handoff seam.

**Toolchain** — `rustup.zip` carries `rust-1.96.0-x86_64-unknown-linux-gnu` plus `rustup.sh`/`pack-rust.sh` install scripts for offline provisioning. Pin the workspace to **Rust 1.96, edition 2021** (matching skydribble) unless the monorepo dictates otherwise.

**Version tensions to resolve deliberately (do not paper over):** skydribble is wgpu **22** / winit **0.29**; logscope's lockfile carries wgpu **29.0.3** (via gpu-gemm); the egui tier is **0.34** with logscope on the *glow* eframe backend. CADscope must land on **one** wgpu version: use eframe 0.34's **wgpu backend**, port skydribble's render/meshgen/model/shader code up to that wgpu (mechanical: descriptor and lifetime churn, `naga` tests catch shader drift), and prefer aligning gpu-gemm's wgpu so a single device/queue can be shared between rendering and on-GPU inference. If sharing proves premature, run gpu-gemm on its own device first and converge later — but record it as debt in the parity ledger, per repo custom.

---
## 2. Product pillars

**Fast.** Cold start to an editable viewport in under two seconds on a phone; 60 fps orbit on an 8k-triangle assembly on FP5-class hardware; every modeling op previews live under the finger/cursor before commit. The skydribble renderer already proves the budget class (one pass, dynamic-offset uniforms, MSAA 4).

**Effective.** Real parametric solids with an editable history, not a mesh sculptor. Booleans, fillets-by-rounding, shells, patterns, measured sketches. Export that actually lands: glTF, STL, IQM-with-textures, niftyplot.

**Learnable.** The Shapr3D lesson: one direct-manipulation gesture per concept, discoverable on screen; the NX lesson: everything the gestures do is also a named, searchable command with parameters. CADscope fuses them through `logscope-fan`'s `cmdpalette` + `vimmode`: every touch action has a colon-command; every colon-command is fuzzy-findable; the liaison can *explain any button you long-press* using its no-invention prose rules.

**Phone-first, not phone-tolerant.** Layouts come from `uikit-egui::adaptive`; gestures from `touchkit` (two-finger orbit, pinch dolly, one-finger tumble only inside the viewport, long-press = context menu, double-tap = frame selection, fling = momentum orbit). Obey the slider-vs-swipe rule from the bo-suite parity note. Battery honesty: repaint only on damage; snapcore's radio windows for sync.

**Keyboard-deep.** The spine-grower grammar becomes a first-class *Grow* mode; Normal mode gets helix-style selection-first editing (select faces/edges/bodies, then act). A `z` camera prefix and `space` leader arrive free from `vimmode`.

**AI when you're in a pinch — never in your way.** The liaison chat is a panel, not the product. It fans, it proposes, it renders progress honestly (genviz), and everything it emits crosses the sanitization boundary as ops/specs — never code, never direct scene-graph writes.

---

## 3. Placement and scaffold

```
apps/cadscope/                     # its own cargo workspace, like apps/logscope
  Cargo.toml                       # [workspace] members = crates/*
  README.md                        # quickstart + this doc distilled
  PARITY.md                        # debts vs. this spec (repo custom)
  crates/
    cad-kernel/                    # pure geometry: sdf-frep + mesh, ops, history graph. no egui, no wgpu
    cad-forge/                     # the op-script format (forge pattern): parse/apply/export/round-trip
    cad-view/                      # wgpu renderer (skydribble port) + egui_wgpu paint callback, picking, gizmos
    cad-motion/                    # modal input: Grow/Normal/Sketch modes over logscope-fan vimmode+cmdpalette
    cad-tex/                       # THE NEW TEXTURING SYSTEM (see §8): material graphs, bake, atlas, audit
    cad-liaison/                   # AI: PartSpec/OpScript schemas, sanitizers, fan adapters, progress model
    cad-net/                       # collab: op-log replication over netcode/snapcore, presence, locks
    cad-nifty/                     # niftyplot emit + the `hardware` kind implementation (upstreamed)
    cad-pool/                      # asset pools: manifests, audits, budgets, game injection adapters
    cadscope-egui/                 # the app: shell, panels, tabs — thin; logic lives in the crates above
  tools/                           # py shims that wrap v2-script functionality until cad-pool subsumes it
  examples/                        # golden models: bracket.cadf, helix_lamp.cadf, dominus_wing.cadf
```

Dependencies point *down* only: `cadscope-egui → {cad-view, cad-motion, cad-liaison, cad-net, cad-tex, cad-pool, cad-nifty} → cad-forge → cad-kernel`. `cad-kernel` and `cad-forge` are std-only and fully unit-tested, mirroring how `logscope-fan` was extracted so edits recompile in seconds. Shared deps: `uikit-egui`, `touchkit`, `nifty-core`, `snapcore`, `games/shared/netcode` by path; `logscope-fan` by path (it is already egui-free by design — consume, don't fork). Register the app in `manifest/projects.toml`:

```toml
[apps.cadscope]
path = "apps/cadscope"
kind = "cargo"
role = "app"
daemon = false
flatpak = false            # revisit at M6
runtime = "none"           # pure egui, wgpu backend — first non-glow egui app; note it in UI_STRATEGY.md
targets = ["desktop", "phone"]
notes = "parametric CAD; skydribble-derived wgpu renderer inside eframe-wgpu; logscope-fan liaison; forge-pattern history; snapcore collab. `cargo build --release -p cadscope-egui` produces the deployable bin."
```

Also: fold **skydribble itself** into the tree at `games/skydribble` on its own `project/skydribble` orphan branch per `LAYOUT.md`'s branching model, so CADscope's port has an in-tree upstream to diff against, and skydribble picks up the netcode/monorepo it already vendors. Add both to `build/build.sh`'s plan output.

---

## 4. Rendering and the engine port

The viewport is skydribble's renderer transplanted into egui's wgpu backend as a paint callback — exactly the "next rung" logscope's `gpuscene.rs` documents.

Port plan, in order: (1) lift `render.rs`'s pipeline creation into a `CadRenderer` that owns pipelines/buffers but borrows device+queue from `eframe`'s `RenderState`; (2) upgrade wgpu 22 → the 0.34-eframe wgpu, letting `naga` dev-tests pin `shader.wgsl` through the churn; (3) re-express the frame as an `egui_wgpu::CallbackTrait` — prepare uploads instance/uniform data, paint records sky→grid→bodies→overlays into egui's render pass (accepting egui's surface format and MSAA config; keep skydribble's `Depth32Float` via a callback-managed depth target); (4) keep the single dynamic-offset uniform discipline (256-byte strides, `MAX_OBJECTS` sized to assemblies not race worlds); (5) port `model.rs`'s glTF path for reference-model import (the Sketchfab normalization — nose-to-−Z, centered, uniform target scale — becomes "import & true-up"); (6) port `meshgen.rs` primitives as the tessellation vocabulary.

CAD additions the game never needed: **picking** (ID buffer pass — render body/face/edge IDs to an offscreen `R32Uint` target; on touch, sample a small neighborhood and pick the nearest-depth majority ID so fingers can hit edges), **transform gizmos** (screen-scaled, axis/plane handles, fat hit zones on touch), **section views** (a clip plane is trivial in the SDF world: `max(d, plane)` — expose it as a first-class inspection tool, a Shapr3D favorite), **edge/silhouette overlay** (derive feature edges at tessellation time; render as line list — NX-style display modes: shaded, shaded+edges, hidden-line), and an **orientation cube** widget from uikit's carved layer.

Camera: keep skydribble's feel but re-tune for inspection — orbit around picked pivot, `f` frames selection, inertial orbit fed by touchkit momentum ticks. The logscope `OrbitCam`-parity trick in `gpuscene` (2D overlays projecting pixel-identically with the 3D pass) is retained so measurement labels, sketch glyphs, and the collaboration cursors never swim against the geometry.

---

## 5. Geometry core: the SDF-first hybrid kernel

Honest scoping: the monorepo contains no B-rep kernel, and writing Parasolid is not a milestone — it's a decade. CADscope instead commits to what its ancestors already prove: **f-rep (analytic SDF) as the modeling truth, meshes as the viewing/export truth**, one history graph binding them.

- **Primitives & fields.** Every skydribble/`meshgen` shape plus CAD staples (box, cylinder, cone, sphere, capsule, torus, wedge, tube-along-curve) exists as an analytic SDF with exact or tightly-bounded distance. Sketch profiles (closed 2D polylines/arcs on a work plane) become fields by extrusion, revolution, sweep along a spine, and loft-between-sections.
- **The spine is a first-class solid.** `spine_grower`'s output — a piecewise curve with per-node cross-section (○ □ △ ✦ ⬡), radius, twist, ridge/pinch/flare modifiers, branches, mirror — is `cad-kernel::Spine`, evaluated as a swept SDF. This is what makes "draw cool strange shapes" a kernel feature, not a toy.
- **Ops.** Boolean union/intersect/subtract (exact on SDFs), rounded variants (the smooth-min family gives fillets/chamfer-like blends with a radius parameter — sell it as *Blend*, tune it to read as filleting), shell/offset (SDF offset is exact), linear/circular patterns (domain repetition with per-instance ID for picking), taper/twist/bend deformers (bounded-error with conservative distance, flagged in the doc as approximate).
- **Meshing.** Adaptive dual contouring (preserves sharp features from the SDF gradient) with per-body LOD; watertight guarantee for STL export; feature-edge extraction feeds §4's overlays and §9's schematic projection. Budget: interactive re-mesh of the edited body only, full assembly re-mesh in the background — fanned across cores, progress surfaced through the same genviz stream model as AI work (one progress system, two producers).
- **Measure & constrain (v1 scope).** Distance/angle/radius probes on the SDF are cheap and exact — ship measuring at M2. Sketch constraints ship as a *solver-lite*: horizontal/vertical/coincident/dimension via iterative relaxation, mirroring nifty-core's own precedent (`niftyplot.rs` records geometric constraints and falls back to a working-subset rather than blocking — same philosophy: never let a constraint conflict wedge the user).
- **History = forge document.** Every op is a named entry in a `cad-forge` op sequence: names not IDs, per-op error tolerance, full round-trip `export()`. Editing history = editing the document + incremental re-evaluation with memoized subtrees (the fan/graph discipline from logscope applied to geometry: the model *is* an exploration tree whose branches are design alternatives — `pathfan` for shapes). File format `.cadf` = the forge JSON + a sidecar cache.

This kernel deliberately trades exact NURBS surfacing for robustness, phone-speed, and AI-authorability. Record the trade in `PARITY.md`; leave a seam (`cad-kernel::brep` module stub + `truck`/`fornjot` evaluation notes) for a future exact-surface tier.

---
## 6. Input: two hands, one grammar

**Touch (the Shapr3D half).** Built entirely on `touchkit` + `uikit-egui::gesture_layer` — no per-app gesture code, per the kit's promise. Viewport: one finger tumbles, two fingers orbit/pan, pinch dollies, double-tap frames, long-press opens the radial context menu (carved-layer, thumb-reachable), fling glides with touchkit momentum. Direct modeling: drag a face along its normal to push/pull (live SDF preview), drag an edge to blend, tap-tap-drag to measure. Panels use `dock`/`vertical_tabs` on compact widths — never a whole-surface page swipe, because the viewport is one giant drag surface (the bo-suite lesson, now a hard rule). All chrome is `Skin`-tokened so the phone build can go dense-dark without touching call sites.

**Keyboard (the NX-meets-helix half).** `cad-motion` wraps `logscope-fan::vimmode` and extends its Mode enum:

- **Normal** — selection-first, helix style: `x` extend selection by entity class (vertex→edge→face→body), `;` collapse, `v` visual sub-select, `hjkl` walks topological neighbors of the selection, `gg`/`G` first/last feature in history, numbered registers hold selections. Verbs act on selection: `d` delete feature, `y` yank (as forge ops!), `p` paste ops, `.` repeat.
- **Grow** — `spine_grower.html` promoted verbatim: `h l` yaw, `j k` pitch, `H L` roll, `[ ]` helix twist, `0` straighten, `w b` thicken/narrow, `e` ridge, `p` pinch, `f` flare, `1–5` section, `~` wobble, `space` node, `t` branch, `y` to parent, `m` mirror. Exiting Grow commits the spine as one history feature. (Port the HTML prototype's tuning constants, then delete the toy or leave it as documentation.)
- **Sketch** — plane-locked 2D: `hjkl` nudge, `L`/`A`/`C` line/arc/circle chains, `=` dimension, `q` constrain, `Esc` closes profile.
- **Colon commands** — every op, view, export, and liaison verb: `:extrude 12mm`, `:blend 2`, `:sec xz`, `:export gltf`, `:pool push blackstar`, `:fan 4 alternatives for this bracket`. `cmdpalette` fuzzy-finds them; `space` leader mirrors the top ten onto touch buttons so the two vocabularies teach each other.
- `z` stays the camera prefix; joystick/gamepad arrives free via `vimmode::joystick_dir` (a phone with a controller is a workstation).

**Learnability glue.** First-run coach marks come from logscope's `coach.rs` pattern; long-press-anything → liaison explains it from facts (§7); `:tutor` fans a guided exercise.

---

## 7. AI: the liaison chat and the fan

The pinch-time flow: summon the liaison panel (persistent right dock on desktop, sheet on phone), type "a wall bracket for a 40mm pipe, two M5 holes", and watch models grow in the viewport as workers stream.

**Reuse, don't rebuild.** `cad-liaison` composes: `logscope-fan::{dispatch, ollama, enlist}` for fanning across local workers (ollama daemons, and `gpu-gemm` as the pure-wgpu fallback engine — the same GPU that renders can infer); `explore`/`verdict`/`bestregion` for the branch tree; `consensus` when multiple workers propose; `decompose` to split "a desk organizer" into part-tasks; `pathfan` to fan *N design archetypes* (cantilever vs. gusset vs. printed-hinge) that land as side-by-side ghost previews; `treeexport` to save the whole exploration. The chat transcript rides `convograph`'s spine model — the session is not the context window, so day-long modeling conversations stay lean (the "graph-based fanning approach [that] will help you stay lean" from the original prompt is precisely these two ideas together).

**The sanitization boundary, generalized.** Per `DESIGN_generative_scenes.md`: models never emit code or touch the scene graph. They emit either a **PartSpec** (constrained JSON: primitive/spine vocabulary, mm-clamped dimensions, enum materials — the RoomSpec pattern with a `sanitize()` that clamps, defaults, and drops unknowns) or a **forge OpScript** (the same op format humans and the GUI write — `EDITORS.md` literally designed it to be "authorable by ... an LLM shown the schema"). GBNF grammar-constrained decoding guarantees parseable output; the sanitizer guarantees safe output; per-op error tolerance guarantees one hallucinated parameter never voids the model. Bad spec → safe default, never a crash.

**Progress you can trust.** Worker token-rates render through the `genviz` model (stable slots, EMA); each fan branch previews as a ghost body that solidifies as its OpScript replays — geometry re-evaluation reports through the same stream type, so "AI thinking" and "kernel meshing" share one honest progress UI. The Liaison narrates only derivable facts (`liaison.rs`'s tested no-invention rule): "Branch 3 blocked: hole Ø exceeds wall thickness" comes from a real verdict event, and an idle system says "nothing observed," not something plausible.

**Texture AI.** `mediagen.rs`'s pure argv builder gets a sibling recipe set for material maps: prompt → `sd-cli` → tileable candidate → §8's ingest (seamless-ness check, downscale, PBR-ify). Same spawn-separation, same unit-tested flags.

---

## 8. The texturing system — the new build (`cad-tex`)

This is the subsystem the prompt flags as "sorely needed [and] underdeveloped," and the artifacts agree: what exists is per-vertex procedural color (logscope `scene3d`), four tileable 256² patterns in a Python script (`gen_texture.py`), and thirteen versions of an export workaround (`texture_export_v13`) whose docstring *is* the requirements list. Build the real thing:

1. **Material model.** A small, serializable **material graph**: nodes for procedural patterns (port `gen_texture.py`'s plating/hexmesh/circuit/rock as native generators, seamless by construction), image samplers, blend/mask/levels, and a PBR output (baseColor, normal, roughness/spec). Graphs are forge-documents too — diffable, LLM-authorable, collaborative.
2. **Projection without tears.** Default is **triplanar** projection off the SDF normal — zero UV work, ideal for grown/blended solids and for phones. Where real UVs matter (game export, decals), provide **auto-unwrap**: per-face-cluster charts from the feature edges §5 already extracts, packed into an atlas. Decal stamping (tap a face, drop a logo/label) works in both modes.
3. **Baking.** Bake the material graph (and SDF-derived AO + curvature — cheap and gorgeous from a distance field) to per-part texture sets at export, honoring the engine convention from `texture_export_v13`: per-material parts, `<name>.png` / `norm_<name>.png` / `spec_<name>.png`, real UVs preserved per part. That thirteen-version Python saga retires into one deliberate exporter.
4. **Hygiene as a feature.** Fold `texture_audit_v2` in natively: content-hash exact dupes, dHash near-dupes, resolution histograms, projected-savings ranking — surfaced in the pool UI (§10) with one-tap "cap to 1K / dedupe" fixes and `library_shrink`-style budgets.
5. **Phone budget.** BCn-compress on export where targets allow; viewport uses the graph evaluated at adaptive resolution; never ship an 8K map to an FP5.

Acceptance: texture a grown spine-lamp entirely by touch in under a minute; export the dominus wing example as per-part IQMs whose skins load in blackstar unmodified; audit a 50-model pool and reclaim ≥40% projected bytes with two taps.

---

## 9. niftyplot and the voiceterm bridge (`cad-nifty`)

Two deliverables. First, **emit `%kind: graph`** documents today: assembly structure, history DAGs, and pool dependency maps as niftyplot source that `nifty-core` parses and voiceterm's `niftyplot_view` renders — CADscope gains "explain this model as a diagram," voiceterm gains CAD content for free. Second, **implement `%kind: hardware` upstream in `shared/nifty-core`** — the spec already defines it (components, pins, traces, clearances; mm units are meaningful; `parallel(...)` has a per-kind reading) and the parser's permissive philosophy (warnings not errors, largest-working-subset layout, debug footer) tells you exactly how to fail soft. CADscope then round-trips EE work: sketch an enclosure around a board outline imported from a hardware doc; project a modeled assembly *to* a hardware doc (components from bodies-with-metadata, pins from tagged features, clearances measured on the SDF — real measurements, so the diagram is honest). Contribute parser + layout to `nifty-core` behind its existing kind dispatch; add goldens beside its current tests; voiceterm inherits the kind with zero app changes.

---

## 10. Asset pools and game injection (`cad-pool`)

Reframe the `v2/` scripts as the spec for a native subsystem. A **pool** is a directory-with-manifest (`pool.toml`: entries, tags, content hashes, texture stats, budgets) holding models in `.cadf` + baked exports. Operations, each traceable to its Python ancestor: **harvest** (`part_harvest_v9`: split imports into named parts), **diagnose** (`obj_diagnose_v3`), **audit** (§8.4), **shrink** (`library_shrink_v8`: enforce byte/tri budgets by re-bake/decimate), **metadata** (`model_metadata_v1`), **pair** (`shape_pairs_v1`: suggest families), **install** (`install_models_v4` → per-game adapters). Injection adapters per `EDITORS.md`: blackstar gets IQM + `.arena.json` sidecars via the arena-forge op path (`obj_to_iqm`/`gltf_to_iqm` logic ported); soldier gets buildscript ops; paperbound/gyroball get their JSON packs; skydribble (now in-tree) gets glTF straight into `assets/`. "Seamless" means: `:pool push soldier` results in the asset appearing in the game's editor list on next launch, and a running game launched through **bsgames handoff** (`BSGAMES_HANDOFF_*`, exactly as skydribble's `net.rs` documents) can hot-list new pool entries. The `tools/` Python shims wrap remaining v2 behavior until each verb goes native; delete a shim only when its native test goes green.

---

## 11. Realtime collaboration (`cad-net`)

The design writes itself because the model is an op log. **Control plane:** exactly skydribble's — `netcode::multiplayer::Multiplayer` publishes your profile to the snapcore DHT store, `host()` announces a `SessionRecord`, `join(friend)` resolves it; FileStore floor for same-machine/LAN dev, `opendht`/native-snapcore for global reach. A modeling session is a **snapcore cluster** ("a cluster IS a shared mailbox you join") — dense peer tracking, bridge nodes, and battery-aware windows so an idle phone collaborator costs nothing (durable ops earn a radio window; cursor twitches are ephemeral and never wake it). **Data plane:** replicate **forge ops**, not geometry — ops are small, ordered, and per-op tolerant by design. v1 concurrency: optimistic append with feature-level soft locks (you see a collaborator's ghost-grip on a body) and last-writer-wins per feature; the branch tree absorbs true conflicts as *forks you can `:pick` or `:merge`* — turning logscope's consensus machinery into design review. Presence: named cursors and view frusta via the pixel-parity overlay projection (§4). Determinism: `cad-kernel` evaluation is bit-stable given an op log (logical time, seeded noise), so replicas verify by hash — and the whole net layer tests inside `snapcore::sim`'s deterministic clock, per house style.

---

## 12. Milestones and acceptance gates

- **M0 — Scaffold & port.** Workspace up per §3; skydribble renderer inside eframe-wgpu on desktop *and* phone; orbit/pick a meshgen box at 60fps; `naga` + unit tests green in the monorepo orchestrator (`build/build.sh --run --only cadscope`). *Gate: the phone shows a lit, pickable box with touch orbit.*
- **M1 — Kernel v0 + forge.** Primitives, booleans, blend, shell, extrude-from-sketch; `.cadf` round-trip; history panel; undo = op removal + re-eval. *Gate: model the `bracket.cadf` golden by touch in ≤ 3 min.*
- **M2 — Grow mode + measure.** Full spine grammar; measuring; section view. *Gate: `helix_lamp.cadf` grown keyboard-only in ≤ 90 s by someone who read one help card.*
- **M3 — Liaison + fan.** Chat panel, PartSpec/OpScript sanitizers, 4-way `pathfan` ghosts, genviz progress, gpu-gemm fallback. *Gate: "bracket for a 40mm pipe" yields ≥ 3 sane candidates; the empty-tree liaison says "nothing observed."*
- **M4 — cad-tex.** Material graphs, triplanar, bake-to-convention, audit. *Gate: §8's acceptance list.*
- **M5 — Pools + injection.** Pool verbs native or shimmed; blackstar + soldier + skydribble adapters. *Gate: `:pool push` asset visible in-game.*
- **M6 — Collab + nifty.** DHT sessions on two devices incl. one phone; hardware-kind upstreamed. *Gate: two-editor session survives airplane-mode blips; voiceterm renders a CADscope hardware doc unmodified.*
- **M7 — Polish pass.** Coach marks, `:tutor`, skin variants, Flatpak decision, `PARITY.md` reconciled.

**House verification style (non-negotiable):** pure logic crates with dense unit tests (the `logscope-fan` extraction exists *because* of this); `--selftest` headless modes for anything interactive (the editor pattern); WGSL validated offline by `naga`; net logic under `snapcore::sim`; goldens in `examples/` rendered by CI where the orchestrator allows; every deviation from this spec logged in `PARITY.md` rather than silently dropped.

## 13. Non-goals (v1)

Exact NURBS/B-rep surfacing and STEP fidelity (seam reserved); FEM/simulation; 2D drafting sheets (niftyplot hardware covers the EE slice); Windows/macOS (Linux desktop + mainline-Linux phone first, per repo targets); cloud accounts (identity is snapcore's, sessions are peer-to-peer).

## 14. Open questions for the operator

1. wgpu unification: is aligning `gpu-gemm` to the render wgpu in scope now, or does it stay a second device until after M3?
2. Blend semantics: is smooth-min "filleting" acceptable to your eye at M1, or do we pull exact rounded-edge SDF ops forward?
3. Pool storage: do baked exports live in-repo (LFS-ish weight) or in an out-of-tree pool root referenced by manifest?
4. Should the spine-grower HTML stay as living documentation once Grow mode ships, or be deleted per the peek-dedup precedent?
5. Collab identity: reuse snapclone profiles wholesale, or a leaner CADscope-local identity on the same DHT?

## 15. Kickoff checklist

Install the offline toolchain (`rustup/rustup.sh` from `rustup.zip`, Rust 1.96); clone the bundle; `git switch main`; unzip skydribble to `games/skydribble` and commit on its orphan branch; extract `v2_tar.zst` to `apps/cadscope/tools/v2-reference/` (read-only reference); scaffold §3; add the manifest entry; make M0's box spin under a finger. Then work the gates in order — and when in a pinch, do exactly what the product does: open the liaison, fan the paths, pick the branch that ships.

---
### §15 addendum (M0, in-flight): fornjot-main snapshot received — M1 decision input
Stored: extracted at `/home/claude/fj-drop/fornjot-main`; original zip parked **untracked** at `vendor/incoming/fornjot-main.zip` (enters git only if the M1 analysis says adopt/borrow). License verified: **0BSD** (workspace `Cargo.toml`) — vendor-safe if we want it.

State of the project (from the snapshot itself): the v0.49-era b-rep kernel (`fj-core`, `fj-math`, 2024-03) is **deprecated in place** — husk crates pointing at consolidated `crates/fj` — and the live work is the `experiments/` series (2024-10 → 2025-12-03). The author's own framing: mainline "reached a local maximum… mired in complexity"; the surviving concepts are *"approximated geometry as a uniform intermediate representation, and a simplified topological objects graph"*, with 2025-11-07 showing that keeping exact local geometry definitions costs ~600 lines of core data-structure code to define a cube, and 2025-12-03 asking whether that information is needed at all.

Reading for CADscope §5: this **corroborates** the SDF-first course rather than challenging it — the strongest Rust b-rep attempt is independently migrating *toward* "approximate geometry as the uniform representation." It does **not** hand us a drop-in kernel. What it changes: **M1 must design persistent face/edge naming into forge ops** (the topological-naming problem is what b-rep is genuinely for), studying fj's object-graph/handle identity model — so the §5 seam stays open without rewriting documents later. Borrow candidates at M1: fj's validation-check architecture, fj-math predicates, the experiments' approx-geometry data model. Full mainline adoption: off the table (deprecated by its own author). Deep analysis scheduled at M1 kernel design, alongside the mesher.
