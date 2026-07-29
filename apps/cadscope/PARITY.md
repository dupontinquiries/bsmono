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

- **The corpus is scanned lazily, on first reference query, and synchronously.** A warm scan is
  0.06 s and imperceptible; a cold one on a large tree will visibly stall the window. It belongs on
  the worker/progress model planned for the liaison (M3), which is also where meshing should go.
- **A reference never edits the document on its own.** `verbs::execute` answers questions; turning
  a measured volume into `Op::MatchVolume` requires an explicit button press. A verb that quietly
  edited the model would be one nobody could audit.

- **Indexed, but nothing is cached.** A scan of 4,894 entries takes ~50 s cold and ~0.7 s with the
  files in the page cache, which locates the cost precisely: this is I/O bound, not parser bound.
  A content-hashed derived cache (§17.4) therefore buys the cold-start case and nothing else — but
  the cold-start case is the one a user actually experiences. Until it exists, `--ref-index` is a
  diagnostic rather than something app startup can afford.
- **glTF/GLB models index by name only.** IQM gives headers and OBJ gives measured geometry;
  3,150 of 3,155 model entries now carry real facts, and the remainder are glTF. A reader for it is
  the obvious next parser.
- **Units are declared, never inferred — and nothing is declared yet.** `ref.toml` exists and is
  parsed (longest path prefix wins, so a subtree overrides the root it sits in), but this
  repository's file ships with every declaration commented out, deliberately: nobody has
  established what unit these game assets were exported in, and guessing wrong is a factor of a
  thousand in length and a billion in volume. The consequence is that `Index::volume_mm3` currently
  refuses for **every** model in the tree with `UnknownUnits`. That is the system working, not
  failing — but it does mean the verb is unusable here until someone fills the file in.
- **`up_axis` is still not derived.** §17.1 lists it beside unit scale; only units landed.
- **Most of the corpus cannot be measured, and now says so.** The closed-manifold gate is
  implemented: vertices are welded by quantized position (exporters duplicate coincident vertices
  constantly), edges are censused, and `volume` is withheld unless every edge is shared by exactly
  two triangles. Against the real tree that is **165 closed models out of 1,000 with geometry —
  835 are open shells**. The previous, ungated run reported 998 volumes, so 833 of them were
  confident numbers for surfaces that enclose nothing. Anything relying on `volume of` must expect
  "not measurable" to be the common answer, and that is the honest state of the corpus rather than
  a defect in the reader.
- **Open shells still report area and bounds**, which are meaningful without closure, so an open
  model is not useless as a reference — it just cannot answer a volume question.
- **IQM bounds are nearly always absent.** IQM stores them per animation frame, so a static export
  has none: exactly 1 IQM in the tree reports bounds, against 1,000 OBJ files that now compute
  theirs from vertices. This is what settled the design — reference volume comes from vertex data,
  measured with the mesher's own sum.
- **Ambiguity is refused only when it matters.** The greeble catalogue ships an `.iqm` and an
  `.obj` for the same part, and only the OBJ carries vertices — that is one model described twice,
  not two models in conflict, so the measurable candidate answers. Two candidates with genuinely
  *different* volumes still refuse and list their paths.
- **Names are ambiguous and that is deliberate.** `obsidian` resolves to three entries (its base,
  normal and specular maps, correctly grouped under one name); `puma` resolves to a top hit plus
  two weaker ones. `resolve` returns a ranked list and never collapses to one answer — a silent
  pick is how you scale a part to the volume of the wrong model.
- **`fit <doc>` is a bounding-box test, and reads every dimension as a limit.** A spec line like
  "wall >= 2.5 mm" is a *minimum*, and the verb does not parse comparator words — such a line is
  shown with its measured neighbour so a person can judge, never silently marked satisfied. The
  checklist is an aid to reading a spec, not a certificate that one has been met.
- **Dimension extraction deliberately misses some real dimensions.** Two false-positive classes
  turned up when `fit` was first run over the real tree, and both were fixed by *narrowing*: the
  inch mark `"` was removed entirely (prose like `"===1" meaning` parsed as one inch, and in a tree
  of developer documentation quotation marks outnumber inch marks enormously), and the abbreviation
  `in` now requires tight binding to its number (`5in`, not `5 in`) because "in" is also an English
  preposition — `level 1 in ~0.5` and `git 2.43 in CI` both parsed as inches. A spec writing `5 in`
  is therefore missed. That is the intended direction of error: a missing requirement is visible in
  a checklist, an invented one silently becomes a constraint nobody wrote.
- **No units are declared for the monorepo root.** `ref.toml` is read from the scan root, and the
  one in `apps/cadscope` applies only when scanning there — so a whole-tree scan reports 0 models
  with declared units and `volume of` correctly declines every one of them. That is the mechanism
  working, not failing, but it means the verb is unusable tree-wide until someone who knows the
  conventions writes the declarations.
- **Four verbs live, one to go.** `volume of` (via `Op::MatchVolume`, measured and re-checked)
  `texture from` (`Index::textures_of_model`, a join that reports unresolved materials rather than
  dropping them), `fit <doc>` and `fits <niftyplot>` all work against the real tree. Only
  `like <model>` remains (M7). The three live verbs are now reachable from the UI: a reference
  panel takes a typed sentence, runs it through `cad_ref::verbs::execute`, and shows the answer or
  the refusal. **That panel has never been seen** — like the rest of the UI it is compile-verified
  only, with no display here. What *is* verified is the funnel underneath it, which the CLI
  exercises on the real corpus through the identical code path.
- **The board extent is not the board outline.** `fits` returns the bounding box of placed
  components, zones and vias, because niftyplot's hardware kind carries no outline statement. A
  real enclosure wants clearance added, and `BoardFit::with_clearance` makes that the caller's
  explicit decision rather than a silent constant. Do not read the extent as the edge of the PCB.
- **Only the hardware kind is interpreted.** `cad-nifty` parses its header for every kind and its
  body only for `hardware`; graph bodies belong to `shared/nifty-core`, which already parses them,
  and a second spelling of one format would be worse than none. Gantt, sequence and state remain
  unparsed by anybody.
- **The corpus contains exactly one board**, `apps/cadscope/refs/sensor_hub.nifty`, written as a
  worked example because the tree had no `.nifty` files at all. The parser is tested against the
  spec's own I2C example, but it has never met a board somebody else wrote.

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
