# fao-codec: where it stands, and where the remaining wins are

Written at the end of the 2026-08-31 session. `SESSION_LOG.md` is the
chronological record with every number; this is the summary a person
should read first, including the parts that argue against continuing.

## Reproducing the numbers

`cargo run --release -p fao-codec-lib --bin bench -- [--configs] [--csv out.csv] <clip.y4m> ...`

Emits bytes, MS-SSIM, and grid-discontinuity against the source's own
figure, plus a single figure of merit in bytes:

    bytes + (1 - MS-SSIM) x 25,000,000

That constant is the measured dead-zone ladder rate at dz32 (~27.0M on
film, ~24.1M on game content): the bytes it costs this codec to buy one
MS-SSIM point through its quantiser. Scoring in those units means a
change that trades bytes for quality is judged against the cheapest
alternative way of making the same trade, which is the comparison that
caught most of this session's errors.

The `egui-frontend` "benchmark a library" button should shell out to
this bin rather than reimplement it — headless keeps the numbers
obtainable on a build machine and in CI.

Do **not** score with PSNR. Every comparison in this project made on
PSNR turned out to flatter the codec, because PSNR does not charge for
the grid discontinuity that is this codec's characteristic artifact.

**And do not trust the luma-only figure either, yet (2026-09-01).**
MS-SSIM here is computed on Y. On SMITE the reconstruction's chroma
PSNR was 28 dB under a 29 dB luma while AV1 at a fifth of the bytes
sits at 46 dB chroma: the residual coder applies the luma dead zone to
chroma, so at dz32 chroma is essentially never corrected. A 12 dB
chroma deficit was invisible to every number in this file. Until the
bench reports chroma, read every comparison with that caveat.

## Standing, honestly

| content | v11 | AV1 anchor | verdict |
|---|---|---|---|
| film (bloopers, 19 shots) | 19.657 MB @ MS-SSIM 0.96200 | 0.989 MB @ 0.96538 | ~20x larger, slightly worse |
| game, flat-shaded (blackstar) | matched-quality ~43x | SVT-AV1 p2 | loses badly |
| game, textured (Minecraft 4K) | 29.8x at -0.14 dB | | loses |
| soldier, map canonical, actors as pixels | 22,468 B @ 44.2 dB / 0.99934 | 18,930 B @ 40.7 dB / 0.99783 | ~wash at matched quality |
| **soldier, map + entity canonical** | **1,997 B @ 57.1 dB / 0.99994** | 18,930 B @ 40.7 dB / 0.99783 | **v11 9.5x smaller, +16 dB** |
| **procedural surface + correct model** | **59,036 B @ 0.98050** | **~62 KB interpolated** | **v11 ~5% ahead** |

(soldier rows updated 2026-09-01; the 42,186 B @ 39.7 dB figure the
previous session recorded was 56% side info chasing a global exposure
drift — see SESSION_LOG for the decomposition and the three fixes.)

Two content classes win. The procedural one by ~5% with a correct
world model and the deblocker on (the 32% first reported was a PSNR
artifact). The engine one by ~10x, and only in a regime that must be
stated exactly: the decoder holds the game's renderer, the map mesh
and the actor mesh are library assets, and the wire carries camera +
actor poses (delta-coded, ~16 B/frame total here) plus a residual for
what neither side knows. With actors carried as pixels instead of
poses the same clip is a wash against AV1. That gap — 22,468 vs
1,997 B — is the price of not knowing what is moving, and it is the
whole argument for the entity split.

## What decides it

Not provenance. Two engine-rendered clips landed at opposite ends.
`bin/spectral_probe` computes HF fraction x temporal cost and ranked all
five clips in exactly their measured order:

    ground 2.77 (v11 wins) | soldier 1.63 | film 0.38 | SMITE 0.29 | blackstar 0.08

A model-based coder wins where **pixel statistics are hostile** — high
high-frequency content that motion compensation cannot reach. It loses
where they are friendly, and flat-shaded geometry is the friendliest
content there is for a conventional coder. n=5, crossover bracketed in
[1.63, 2.77], so use it for ordering, not as a threshold.

## Where size wins are still available

Ranked by evidence, strongest first.

1. **Nothing large, on film.** The bit budget decomposes as ~16%
   keyframe-equivalent and ~84% per-frame novelty. A world model
   replaces the keyframe function, so amortisation has a ~16% ceiling —
   and it is not differential, because AV1's keyframe amortises over
   long GOPs too. Stated plainly so nobody re-derives the marginal-cost
   thesis and expects it to pay here.
2. **Dictionary/long-range reference on textured engine content.**
   Measured 10.7% STRONG matches on Minecraft against 0.2–0.7% on film,
   and mode 4 gave -8.9% bytes with +0.31 dB there once the candidate
   selection was fixed. Unfinished: `DICT_MARGIN` and `DICT_MAX_CAND`
   were both set from sweeps whose winner sat at the grid EDGE, which
   this session proved is not an optimum. Re-sweep both before trusting
   the numbers.
3. **The 4x4 transform size.** `REGION=8` failed by tripling per-region
   flag cost (286 KB -> 618 KB) while residual barely fell, which
   implicates the transform rather than the region grid as the place
   finer adaptation could pay. Never swept; the only untested structural
   constant left.
4. **Per-region routing.** `spectral_probe` works at clip level; one
   clip mixes flat sky with textured ground, and routing is only
   interesting where content varies. Requires a conventional branch to
   route *to*, which does not exist.
5. **Plate routing on unreliable motion.** Shot 9 (motion confidence
   0.11) is STRICTLY better with no plate at all — 8.7% fewer bytes and
   higher quality. `--plate-route` now scores this correctly on MS-SSIM
   but costs a second full encode. Motion confidence (0.11 vs 0.81/0.89)
   separates the three shots tested perfectly and is the obvious cheap
   predictor — on three data points, which is exactly the evidence level
   that produced four wrong constants this session.

## Where 3D reconstruction is still valid

The decisive experiment is `bin/engine_lab`, which removes inverse
rendering from the path by handing the encoder the true mesh and true
poses. With footage carrying content the canonical cannot know (a
dynamic object, photometric drift):

    self-coded 2D plate    128,856 B @ 39.940 dB
    engine canonical        88,854 B @ 45.442 dB
    control, WRONG poses   156,252 B @ 37.368 dB

**-31% bytes AND +5.5 dB simultaneously**, with the wrong-pose control
21% *worse* than self-coding — so the win is geometry and pose being
correct, not an extra predictor. Reproduced on a real in-repo game
(`fao_engine_capture`, soldier): engine canonical 5.9x smaller than the
codec's own 2D plate branch and 7.9 dB better.

So the thesis is **correct given a correct world model**, and every
measured failure in this codec is an inverse-rendering failure, not a
coding one. Concretely:

* **Valid, and the only path with a measured payoff: engine-supplied
  geometry.** `engine_scene.rs` defines the `.faoscene` seam (mesh +
  per-frame pose); `plate_lab --use-canonical3d` encodes against it.
  Nothing in `games/blackstar-rs` writes one yet — `replay::Frame`
  carries state at 30 Hz and `cinema()` derives a camera, so the pieces
  exist; the arena mesh has no export and the gameplay camera is not
  logged. That is the highest-value remaining task in the project.
  Since 2026-09-01 the measured payoff is ~10x under AV1 on soldier,
  but only with actors as ENTITIES (mesh + pose stream), which the v1
  format cannot carry: it holds one mesh and one camera. v2 needs
  meshes[], per-frame instances (mesh id + pose), and a renderer
  identity — soldier's fog and lighting are not reproducible by the
  codec's generic rasteriser, and the cost of that mismatch is the
  next number to get.
* **Valid but blocked: the canonical must widen beyond a mesh.** The
  procedural-ground case cannot be a mesh — pixel-rate noise needs ~a
  vertex per pixel, discarding the saving. `engine_scene` carries
  geometry only and cannot express the one case v11 wins. Widening it to
  "mesh OR parameterised surface" (a shader identity plus parameters the
  decoder evaluates) is a deliberate wire-format decision with a
  measurement behind it.
* **Not valid on captured film, by measurement.** 88% of bloopers
  correspondences sit within 2px of a global affine — that IS the
  absence of parallax. Triangulation was chasing a depth signal below
  the noise floor. "Fix pan triangulation" is retired on that footage.
* **Blocked on the pose estimator, not the geometry, for game capture.**
  Three clips failed three different ways (no parallax; missing forward
  axis; no phase-correlation peak on gradients) and `--pose-from-tracks`
  fixed all three — now default, Minecraft 24 -> 101 points, blackstar
  0 -> 31. Remaining named gap: the pose model has no z/dolly term, and
  first-person motion is dominated by forward translation. Recover it
  from radial flow divergence.
* **2D citation is dead, structurally.** A plate is viewpoint-locked. A
  GOP 0.8s after its canonical cites ~3x better than one 28s later and
  still loses at matched quality. Cross-GOP reuse needs geometry that
  can be re-rendered from a new viewpoint; a plate cannot be.

## Method notes worth keeping

* **Seven constants were swept; five were wrong.** Each had been set
  from one clip with one metric, and neither fact was visible in the
  constant afterwards. Retuned values now carry their numbers in a
  comment at the site.
* **A sweep whose winner sits at the edge of the grid has not found an
  optimum.** Cost one retune to learn (`PLATE_DZ_DIV` 8 -> 4 -> 2).
* **Check whether the instrument is undecidable before concluding the
  question is.** Three rounds of deblock work looked unresolvable under
  PSNR/SSIM/blockiness and resolved in one measurement under MS-SSIM.
* **Adaptivity is not free.** Two mechanism-backed adaptive deblock
  rules both lost to a constant, and per-mover RD admission measured
  3.84% worse in J than admitting everything.
* **Edge handling is invisible in aggregate numbers.** 1080p had 8 rows
  per frame coded with no mode decision, at 5.5x the per-pixel cost,
  found only because the bounds were questioned directly.
