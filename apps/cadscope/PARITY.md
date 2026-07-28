# CADscope — parity and debt ledger

House convention (`PARITY.md`, per-project): record what was *not* carried over, what was
simplified, and what has not actually been observed working — so the next session inherits facts
instead of assumptions. Nothing here is a bug report; these are known, chosen, or unverified.

## Not verified in-sandbox

- **The app has never been run.** The build environment had one core, 3 GB RAM, no GPU and no
  display. M0's gate was met by compilation + 36 passing tests + offline naga WGSL validation. The
  first person with hardware should run `cargo run --release -p cadscope-egui` and confirm: the
  scene draws, depth sorts correctly, tap-picking selects the part under the finger, and the
  selection tint breathes. Until then, treat "the viewport works" as *plausible*, not *observed*.
- **`depth_buffer: 32` runtime path.** The pipeline declares `Depth32Float` because
  `egui_wgpu::depth_format_from_bits(32, 0)` returns it and eframe passes `NativeOptions.depth_buffer`
  through. Read from the 0.34.3 source, not executed. If the pass and pipeline formats ever
  disagree, wgpu will say so loudly at startup.

## Simplified from skydribble

- **MSAA 1, not 4.** Skydribble owns its surface and asks for 4x. Here egui owns the pass, so the
  pipeline's sample count must match `NativeOptions.multisampling` (default: 1). Raising it means
  setting both together. CAD edges will look harsher than the game until then.
- **Only the object pass was ported.** Left upstream: the ray-traced shape path (`RtShapeRaw`, the
  baked candidate grid), sky/fog post, bloom, cinematic exposure/vignette/grain, and the `tube` /
  `pad_disc` / `terrain` mesh generators. The `Globals` struct here keeps the first six fields of
  skydribble's and drops the rest; add them back field-wise if a CAD feature needs them.
- **glTF model loading is not wired.** `model.rs` stayed upstream. Import arrives with the asset
  pool work (M5).

## Kernel and viewport gaps

- **The viewport draws primitives only.** `display_of` maps a primitive leaf plus its
  translate/rotate wrappers to a cached mesh. A part whose SDF is a CSG tree (`Union`, `Subtract`,
  `Blend`, `Shell`) is skipped by the renderer — *it is still fully modeled, picked, and measured
  correctly by the kernel*, it just has no triangles yet. The M1 adaptive dual-contouring mesher
  closes this. Consequence today: a `Combine` op makes a part vanish from view while remaining in
  the parts list and remaining pickable. This is expected, not a bug.
- **Picking is SDF ray-marching, not an ID buffer.** O(parts) sphere traces per click. Correct and
  dependency-free, but it will not scale to large assemblies and it cannot pick a *feature* (face,
  edge, vertex) — only a part. The ID-buffer pass planned in §4 supersedes it when feature
  selection lands (M2).
- **`Model::pick` has no BVH.** Every part is traced on every click. Fine at M0 scale; add a
  bounding-sphere BVH when assemblies grow.
- **Not a B-rep kernel.** Exact analytic surfaces, no persistent topological naming. STEP export,
  exact fillet/chamfer semantics, and stable face references across edits are all consequences of
  this choice, not oversights. See the fornjot analysis note in the plan (§15 addendum) — the
  decision to be revisited at M1 is *persistent face/edge naming in forge ops*, which must be
  designed in before documents accumulate, not retrofitted.
- **`Blend` is smooth-min, not a true fillet.** It reads as a fillet and is bounded
  (`min(a,b) - k/4 ≤ blend ≤ min(a,b)`, tested), but it is not a constant-radius rolling-ball
  fillet and will not measure as one. Whether that is acceptable for real parts is open question
  #2 in the plan.

## UI stack

- **Raw egui input; `uikit-egui` and `touchkit` are not wired.** Orbit/pan/pinch are read directly
  from egui's input state. That means no fling momentum, no long-press, no double-tap, and none of
  the FP5-tuned two-tier gesture recognition. The `shared/uikit` skin, `gesture_layer`, adaptive
  layout and vertical-tabs sidebar all arrive at M2. Note the bo-suite rule when they do: at
  compact widths use the vertical-tabs sidebar, never whole-surface swipe, because these views
  contain sliders and drags.
- **No phone layout yet.** `targets` claims `phone` because the architecture is built for it
  (touch-first input, single dynamic-offset uniform buffer, idle-unless-selected repainting), but
  no narrow-width layout exists and none has been tested on a device.

## Integration not yet attempted

- **gpu-gemm device sharing.** The whole point of unifying on wgpu 29 is that `apps/logscope`'s
  pure-Rust inference engine and this renderer can eventually share a device and queue. Nothing has
  been shared yet; they merely agree on a version. First real test is M3.
- **The five stub crates are stubs.** `cad-tex`, `cad-liaison`, `cad-net`, `cad-nifty`, `cad-pool`
  contain a milestone constant and a test asserting it. They exist to fix the dependency direction
  early, and they are counted in the 36 tests — do not read that number as coverage of unwritten
  features.
