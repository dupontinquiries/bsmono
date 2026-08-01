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
- **`like` projects onto one axis; it is not a medial axis.** Proportions come from the dominant
  axis by power iteration on the vertex covariance, with a silhouette radius per station. A limb
  reads well; a branching form does not — a hand with five fingers summarises as one thick stub.
  Real skeletonisation would follow branches, and this deliberately does not pretend to.
- **`like` needs vertices, so it needs an OBJ.** An IQM is indexed from its header and declines
  with "no vertices to measure" rather than approximating proportions from a bounding box. That
  rules out most of the corpus — 2,150 IQM against 1,000 OBJ.
- **Symmetry is measured about both principal planes, and both numbers matter.** A form symmetric
  about one plane and lopsided about the other — most limbs, most brackets — shows a high
  `symmetry` and a low `symmetry_worst`. Reporting only the first would call such a form symmetric,
  which is how the first version of this test passed while being wrong.
- **All five verbs go through one funnel.** `verbs::parse_request` and `verbs::execute` are the
  only path from a sentence to an outcome; the panel, the CLI and (later) `cad-liaison` all call
  them rather than growing their own parsers. `fits <name>` resolves to a board or a document by
  what the name *is* in the corpus, so nobody has to remember which word goes with which kind —
  a lookup, so still deterministic and still one implementation.
- **All five verbs live.** `volume of` (via `Op::MatchVolume`, measured and re-checked)
  `texture from` (`Index::textures_of_model`, a join that reports unresolved materials rather than
  dropping them), `fit <doc>`, `fits <niftyplot>` and `like <model>` all work against the real
  tree. The three live verbs are now reachable from the UI: a reference
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

## The sanitization boundary (`cad-liaison`)

- **No model has ever talked to it.** `cad-liaison` implements the boundary — a constrained JSON
  `Proposal` in, clamped `cad-forge` ops out — and `--propose` exercises it with hand-written JSON.
  What has *not* happened is any connection to an actual LLM: no `logscope-fan` dispatch, no
  prompting, no GBNF grammar. The boundary is tested against what a model might say, not against
  what one did say.
- **A measurement cannot be asserted, only requested.** `match_volume_of` names a part and a
  reference and carries no number; the corpus measures. A proposal containing `volume_mm3` has
  nowhere to put it and serde drops it. This is the mechanism that makes "same volume as puma"
  trustworthy — the number never travels through the model — and it is tested directly.
- **Names are not paths.** Anything containing a slash, a backslash, or `..` is refused as a name,
  so a proposal cannot reach a file the corpus never indexed.
- **An unknown boolean mode defaults to union and complains.** It deliberately does not guess:
  `subtrct` becoming `subtract` would silently remove material on a misspelling, so it unions and
  says what it did.
- **`SCHEMA` is a string, kept honest only by a test.** One test asserts every op and shape name
  appears in it. That catches a vocabulary that grows past its documentation, but nothing enforces
  that the prose around it is accurate.
- **The measurement loop closes, and is verified end to end.** `resolve_measurements` turns each
  request into a real `Op::MatchVolume` carrying a number no model ever saw, and `--propose`
  re-measures the built part afterwards: a proposal naming `ref_cube_20mm` produces a target of
  8000.0 mm³ and a part measuring 8005.7 mm³, 0.07% out. Still absent from the *egui panel*, which
  handles `volume of` typed by a person but not a proposal's pending list.
- **`--propose` and `--ref-index` build their scan options separately.** Both must load `ref.toml`,
  and `--propose` originally did not — every measurement declined for want of units, silently and
  plausibly. Two call sites configuring one subsystem is the shape of that bug; a shared
  constructor would be better than the comment now warning about it.
- **The corpus contains one measurable reference**, `refs/ref_cube_20mm.obj`, authored so the loop
  has something honest to hit: closed, manifold, exactly 8000 mm³, under the one `ref.toml` root
  that declares units. Every other measurable model in the tree still declines for undeclared
  units, which remains correct behaviour and an unwritten declaration rather than a defect.

## The texturing system (`cad-tex`, M4)

- **The noise is not an FFT.** The Python gets tileability by band-limiting white noise with an
  FFT; this synthesises the same object directly, summing random-phase sinusoids at the integer
  frequencies the filter would have kept. Equivalent in kind, not bit-identical to upstream — the
  textures will not match `gen_texture.py` pixel for pixel.
- **A per-panel feature cannot tile, and that is correct.** With 64px panels in a 256px texture the
  panel at row 0 and the panel at row 255 are different panels, so their tints differ across the
  wrap exactly as they differ everywhere else. The first tiling test asserted otherwise and was
  wrong; it now compares the wrap step against the pattern's own worst *interior* step, which is
  the property that actually matters.
- **The port diverges from `gen_texture.py`'s plating tint deliberately.** Upstream indexes panels
  by absolute position, so the tint sequence restarts at the wrap and the texture shows a grid when
  repeated. The port hashes wrapped panel coordinates instead. Worth reporting upstream.
- **Normal maps are inferred from albedo luminance**, by Sobel gradient, as upstream does. A height
  field derived from colour is not physically principled; it is what makes these read as the games'
  own textures.
- **PNG output uses stored zlib blocks, not deflate.** Files are roughly 3x larger than they need
  to be. Every reader accepts them, and a bake runs occasionally, so the trade is deliberate.
- **Triplanar only; no unwrap.** An SDF has no parameterisation and any unwrap is invalidated by
  the next op, so §8's auto-unwrap from feature edges is not implemented. Triplanar has a known
  cost — three samples per texel — and a known weakness: on a surface whose normal is exactly axis
  aligned, two opposite faces sample the same texel, which is visible on a cube's ±X faces.
- **All three maps are bound.** Bind group 2 carries albedo, normal and specular as parallel array
  textures — layer *i* of each is one material, so a single index selects all three. Albedo is
  sRGB; the other two are linear, because a normal is a vector and a specular map is a mask and
  decoding either as colour would bend the numbers.
- **Normal mapping is whiteout blending, not a tangent basis.** An f-rep surface has no UVs and
  therefore no consistent tangent direction, so the usual TBN construction has nothing to build
  from. Each projection's sample is swizzled into its own plane and summed. This is the standard
  answer and it is cheap, but it is an approximation: a normal map with strong directional detail
  will read slightly differently across the three projections.
- **Normal-map validity is checked on the CPU**, which is what can be checked without a GPU: every
  texel decodes to unit length within 0.006 and none points into the surface, for all five
  patterns, asserted in tests and reported by `--selftest`. That the *shader* then uses them
  correctly is unverified.
- **Never seen rendered.** The atlas upload, the sampler, and the shader path all compile and pass
  naga validation, and the palette is verified headlessly by `--selftest` (each material's wrap step
  measured against its own interior step). Nothing has drawn a textured pixel, because this
  environment has no GPU.
- **One texture scale for the whole scene**, on a slider. Triplanar has no per-part UV scale to
  inherit, and a shared scale is what makes neighbouring parts read as one object; a per-part
  override is a reasonable future addition, not an oversight.
- **The palette is fixed at startup**, five patterns at seed 7. Nothing regenerates a material with
  a different seed or colours from the UI, though `Material::with_colours` supports it.

## Asset pools (`cad-pool`, M5)

- **Planning and applying are separate calls, and the default is a plan.** This is
  `library_shrink_v8.py`'s rule in its own capitals — dry run by default — and `apply` is the only
  function in the crate that writes. It also never deletes an original: reclaiming space by
  discarding a source would break the v2 pipeline's re-processability guarantee, which is why those
  scripts shrink rather than prune.
- **Dedupe is a fingerprint, not a hash.** Size plus the first and last kilobyte. Two distinct
  assets colliding would have to share a length *and* both ends, which is unlikely enough for
  selection, and it is cheap over thousands of files. A full content hash is the upgrade if a
  collision is ever observed. Against the real tree this identified 371 duplicates reclaiming
  36.7 MB — most of them small emitter definitions that genuinely are byte-identical.
- **The triangle budget is advisory.** It is reported and never enforced, because a budget that
  silently drops a model the game needs is worse than one that warns. Only the byte budget stops
  inclusion.
- **Selection is name-ordered, not importance-ordered.** When a byte budget binds, what gets in is
  whatever sorts first — there is no notion of which assets a game needs most. Against the demo
  manifest that means 227 included and 3,018 over budget, which is a real limitation rather than a
  tuning issue: a pool needs a priority signal (referenced-by-scene, hand-ranked, recently-used)
  before its budget behaviour is trustworthy.
- **No injection into `games/` yet.** `apply` copies into a destination directory. The per-game
  adapters of §10 — writing into a game's own layout, honouring the bsgames handoff — are not
  written.

## Collaboration (`cad-net`, M6)

- **Ops replicate; geometry never does.** An edit serialises to under 200 bytes and a test asserts
  it. That is only sound because `Doc::apply` is deterministic, which the kernel's own tests pin —
  if replay ever became order-dependent, convergence would break silently rather than loudly.
- **The log is the truth and the document is derived.** Edits are Lamport-stamped and totally
  ordered by (stamp, peer). Convergence is tested directly: ten peers, each hearing the same edits
  in a different rotation, must produce identical digests.
- **Locks are advisory and nothing enforces them.** A hard lock needs consensus, consensus needs a
  quorum, and a quorum makes a two-person session on a flaky link stop working. Since the forge
  tolerates conflicting ops per-op, the worst case of ignoring a lock is a warning, which is a
  better failure than a stall. `advisable()` reports; it cannot refuse.
- **`since(stamp)` is not a version vector.** Reconnect sends everything after a single stamp,
  which over-sends when peers are at different points. Correct, and wasteful; a version vector is
  the upgrade.
- **No transport, and no snapcore integration.** This crate is sans-io by design and has never
  touched a socket. `shared/snapcore`'s cluster API (`join`/`leave`/`members`, logical `now: u64`)
  is what it is meant to sit on, and that wiring — announce, resolve, gossip — does not exist.
  Nothing here has been tested under `snapcore::sim`.
- **No UI.** Sessions run in tests and in `--selftest`; the app cannot join one.

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
