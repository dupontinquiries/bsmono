# CADscope — parity and debt ledger

House convention (`PARITY.md`, per-project): record what was *not* carried over, what was
simplified, and what has not actually been observed working — so the next session inherits facts
instead of assumptions. Nothing here is a bug report; these are known, chosen, or unverified.

## Closed

- ~~**The viewport draws primitives only.**~~ Closed at M1. `cad_kernel::mesh::dual_contour` meshes
  arbitrary SDF trees, `cad-view` carries them as `MeshKind::Generated`, and the demo document now
  contains three CSG parts (a drilled bracket, a blended lamp, a patterned stud row) that draw.

## Grow mode (M2)

- **`ridge` is tracked but not yet geometry.** The `e` key toggles it, the turtle carries it and
  the status bar shows it, but nothing in the swept solid responds. In the original grower, ridge
  spikes the profile points whose local x is within 0.35 of the axis — an f-rep equivalent needs
  the section SDF to take a ridge parameter. Until then `e` is an honest no-op on the shape, and
  that is the one place where a key does less than the original.
- **Preview is coarse and throttled.** While growing, the form is re-meshed at resolution 28 at
  most every 120 ms; the committed part is meshed at 56. Expect the live shape to look chunkier
  than what lands in the document. Both numbers are constants at the top of `main.rs`.
- **The preview is not pickable.** It is drawn, not a `Part`, so tapping it does nothing until it
  is committed with Esc.
- **No undo inside a session.** Growth is append-only until you commit; there is no way to walk
  stations back. Committing writes ops, so undo at the *document* level is the natural home for
  this and it does not exist yet either.
- **Growth does not see the rest of the model.** No snapping to an existing face, no collision, no
  starting a spine from a picked point — a session always begins at the origin. Growing *onto*
  existing geometry is the obvious next affordance.
- **Polygon sections are underestimates near their vertices.** They are evaluated as the max over
  edge half-planes, which is exact on a face and conservative at a corner. Safe for tracing and
  meshing; it does mean a hex profile's corners mesh very slightly soft.

## The reference corpus (§17, v0)

- **Indexed, but nothing is cached.** A scan of the monorepo takes ~33 s for 4,894 entries and is
  redone from scratch every time. §17.4 calls for a content-hashed derived cache; until it exists,
  `--ref-index` is a diagnostic rather than something the app can afford at startup.
- **glTF/GLB models index by name only.** IQM gives headers and OBJ gives measured geometry;
  3,150 of 3,155 model entries now carry real facts, and the remainder are glTF. A reader for it is
  the obvious next parser.
- **Units are unknown, and that limits what `volume of` means.** Nothing in the corpus records
  whether a file is in millimetres, metres or engine units — the measured volumes span twelve
  orders of magnitude, and some of the largest are almost certainly a unit convention rather than a
  large object. §17.1 lists unit scale and up-axis as derived facts; they are not implemented.
  Until they are, "same volume as X" is only meaningful between files that share a convention, and
  `ref.toml`'s per-root unit hint is the intended fix.
- **A measured volume assumes a closed mesh.** The divergence-theorem sum is only a volume for a
  closed, consistently wound surface. An open or non-manifold OBJ still produces a number, and that
  number is meaningless. `obj_diagnose_v3` from the v2 lineage already knows how to detect this and
  should gate the fact.
- **IQM bounds are nearly always absent.** IQM stores them per animation frame, so a static export
  has none: exactly 1 IQM in the tree reports bounds, against 1,000 OBJ files that now compute
  theirs from vertices. This is what settled the design — reference volume comes from vertex data,
  measured with the mesher's own sum.
- **Names are ambiguous and that is deliberate.** `obsidian` resolves to three entries (its base,
  normal and specular maps, correctly grouped under one name); `puma` resolves to a top hit plus
  two weaker ones. `resolve` returns a ranked list and never collapses to one answer — a silent
  pick is how you scale a part to the volume of the wrong model.
- **No verbs yet.** The index answers "what is called this"; `volume of`, `texture from`, `fit` and
  `like` are M4–M7. The model→texture join is proven (2,295 resolve against the real tree) but is
  not yet wired to anything.
- **Niftyplot entries parse to nothing.** No `.nifty` files exist in the tree yet, and the parser
  arrives at M6 with the hardware-kind implementation.

## Not verified in-sandbox

- **The app has never been run.** The build environment had one core, 3 GB RAM, no GPU and no
  display. What *has* been observed: `cargo test` (47 tests), offline naga WGSL validation, and
  `cadscope --selftest`, which evaluates the document, meshes every CSG part and ray-probes the
  result headlessly — it passes, and its printed volumes agree with hand calculation to within
  ~1.5%, and which drives a scripted Grow session through branch, knuckle, section change and
  mirror to a closed solid. What remains unobserved is everything downstream of the GPU: that the pass draws, that
  depth sorts, that a tap selects what is under the finger, that the selection tint breathes.
  Run `cargo run --release -p cadscope-egui` on hardware to close this.
- **`depth_buffer: 32` runtime path.** The pipeline declares `Depth32Float` because
  `egui_wgpu::depth_format_from_bits(32, 0)` returns it and eframe passes `NativeOptions.depth_buffer`
  through. Read from the 0.34.3 source, not executed. If the pass and pipeline formats ever
  disagree, wgpu will say so loudly at startup.

## The mesher (M1)

- **Uniform grid, not adaptive.** One cell size over the whole part, so cost is O(resolution³) and
  a large part with one small fillet pays for that fillet everywhere. Octree refinement driven by
  QEF residual or curvature is the follow-on, and it is what eventually makes meshing cheap enough
  to run on every keystroke instead of on document change.
- **Meshing is synchronous on the UI thread.** `App::remesh` runs at startup and on any future
  document edit. At M1's demo scale (~20k triangles total) that is imperceptible, but a heavy part
  will stall the frame. The fix is the genviz-style worker + progress model already planned for the
  liaison (M3) — meshing and AI generation should share one progress surface.
- **No vertex clustering.** Dual contouring places exactly one vertex per cell, so a cell
  containing two separate surface sheets (a wall thinner than the cell size, two parts nearly
  touching) will weld them and produce a non-manifold junction. Manifold-preserving DC needs
  per-cell surface-component clustering. Practical guard today: keep `resolution` fine enough that
  a cell is smaller than the thinnest wall — for `Shell`, thinner than `t`.
- **Sharp features are clamped to their cell.** The QEF solution is clamped into the owning cell
  to guarantee vertices cannot cross into a neighbour and self-intersect. This is the standard
  robustness trade and it very slightly rounds a corner whose ideal vertex lies outside its cell.
- **Generated meshes have no UVs.** `from_trimesh` zeroes them. The shader only reads UVs for the
  ground grid, so nothing is wrong today, but texturing CSG geometry needs real parameterization —
  that is `cad-tex`'s auto-unwrap work (M4).
- **No mesh caching across edits.** A document change re-meshes every CSG part from scratch and
  bumps the renderer epoch, dropping all generated GPU buffers. Correct, and wasteful once
  documents are large: incremental re-meshing of only the parts whose subtree changed wants a
  content hash on the SDF tree.

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

- **Picking is SDF ray-marching, not an ID buffer.** O(parts) sphere traces per click. Correct and
  dependency-free, but it will not scale to large assemblies and it cannot pick a *feature* (face,
  edge, vertex) — only a part. The ID-buffer pass planned in §4 supersedes it when feature
  selection lands (M2).
- **`Model::pick` has no BVH.** Every part is traced on every click. Fine at this scale; add a
  bounding-sphere BVH when assemblies grow.
- **Not a B-rep kernel.** Exact analytic surfaces, no persistent topological naming. STEP export,
  exact fillet/chamfer semantics, and stable face references across edits are all consequences of
  this choice, not oversights. See the fornjot analysis note in the plan (§15 addendum) — the
  decision to be revisited is *persistent face/edge naming in forge ops*, which must be designed in
  before documents accumulate, not retrofitted.
- **`Blend` is smooth-min, not a true fillet.** It reads as a fillet and is bounded
  (`min(a,b) - k/4 ≤ blend ≤ min(a,b)`, tested), but it is not a constant-radius rolling-ball
  fillet and will not measure as one. Whether that is acceptable for real parts is open question
  #2 in the plan.
- **`Sdf::aabb` is conservative, not tight.** `Rotate` re-bounds eight rotated corners, and
  `Blend` pads by the full `k`. A loose box costs mesher resolution (cells spent on empty space),
  never correctness.

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
- **The four remaining stub crates are stubs.** `cad-tex`, `cad-liaison`, `cad-net`, `cad-nifty`
  and `cad-pool` contain a milestone constant and a test asserting it. They exist to fix the
  dependency direction early, and they are counted in the 47 tests — do not read that number as
  coverage of unwritten features.
