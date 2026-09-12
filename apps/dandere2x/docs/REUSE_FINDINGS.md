# Block-reuse findings

Everything measured while trying to make dandere2x send fewer pixels through
the network, kept because most of it is negative and negative results are the
expensive kind to rediscover. [`GPU_FINDINGS.md`](GPU_FINDINGS.md) is the
companion record for kernel speed; this one is about the *planner* — which
blocks get reused, from where, and why the rest do not.

**How every number here was produced:** `dandere2x analyze` on one core (no
GPU, no upscaling), rustc 1.96, x86-64. `analyze` counts pixels routed to the
network and reports the block breakdown per frame; it says nothing about
quality. Anything below that claims a *quality* result was scored against a
higher-bitrate encode of the same frames with `tools/pair_dataset.py score`.

## The clips

Three, and the differences between them explain most of the results. Anyone
repeating this work should check which class their footage falls into before
trusting a number.

| clip | what it is | motion |
|---|---|---|
| `lone-ranger` | 1080p24 AV1 3.2 Mbps, blooper reel | whip pans, 30–46 px/frame, motion blur |
| `smite` | 1080p60 AV1 12.6 Mbps, game capture | 3D camera, sub-pixel on every edge, static HUD |
| `transformers` | 1080p24 AV1 2.5 Mbps + a 558 kbps encode of the same source | cuts every 1–2 s, letterboxed, handheld |

The Transformers pair is the only **ground truth** in the set: same source,
two bitrates, frame-aligned (37 dB to the matching frame, 25 dB to its
neighbours). It is what makes quality claims possible at all. Keep it.

## Summary — what pays

Measured on `analyze`, q90, 120 frames, as % of pixels through the main
network. Lower is better; the baseline is the planner as it stood before any
of this work.

| | lone-ranger | smite |
|---|---|---|
| baseline | 66.7 | 95.9 |
| `--region-redraw` (now default) | 53.6 | 95.9 |
| `+ --anchor-rmse 12` | 33.3 | 54.5 |
| `+ --anchor-max-age 5` | 36.9 | **31.4** (with half+quarter-pel) |
| `+ --block-cache 512M` (harvest on) | **31.9** | 31.6 |

2–3x less network work on both content classes. Three of the four levers are
planner bookkeeping, not new maths.

## Summary — what does not pay

Each of these is implemented, tested, off by default, and kept only so the
next person can see the number rather than re-derive it.

| idea | result |
|---|---|
| global background plate (`--plate-window`) | **net loss** on both clips |
| motion-seeded diamond (`--mv-seed`) | no change on either clip |
| 6-tap sub-pel filter (`--halfpel-filter 6tap`) | identical to bilinear |
| temporal median denoising of low-bitrate AV1 | +0.00 dB vs ground truth |
| "sharpest aligned frame" fusion | −1 dB on every frame |
| motion-layer reflection separation | no rigid second layer exists |
| gated (approximate) cache probes (`--cache-quant`) | +0.3 points for 2x the probes |

---

## 1. The redraw rule was the binding constraint, not the gate

Before anything else: on live-action film the planner was redrawing 187 of 240
frames at q90, *and at q60*, and reporting `stat 0` on a clip whose letterbox
bars are 11% of every frame and byte-identical throughout.

Cause: with `--bleed 8` a residual cell is 46 px against a 30 px block, 2.35x
the area, so `redraw_fraction 0.9` fires once ~38% of blocks miss — and then
throws away every block that *did* match, bars included.

Two fixes, both now default:

- **Region redraw** — upscale the bounding rectangle of the missing blocks as
  one image with its bleed ring and blend it back; keep the matched blocks
  outside it. Cheaper than packing whenever misses are dense (no per-block
  bleed), never dearer than the whole frame, and seamless inside.
- **`--redraw-fraction` exposed on the CLI.** It was a struct field before.

Lone Ranger 66.7% → 53.6%. SMITE unchanged, correctly: its misses are
everywhere, so the rectangle *is* the frame and the code declines to use it.

**Lesson.** Before evaluating any new reference, check what fraction of frames
are being redrawn wholesale. A world model cannot help a planner that is
discarding its matches.

## 2. Anchor propagation — the one idea from the "world model" family that worked

The measurement that motivated it: `d2x-core/examples/tdenoise --from F
--oracle hi.rgb` propagates one frame's blocks to its neighbours by motion
search, optionally taking the *pixels* from the high-bitrate version of that
frame — the upper bound of "restore one frame perfectly, then propagate it".

| distance from source | as-is | from the low frame | from a restored frame |
|---|---|---|---|
| ±1 | 38.2 dB / 0.979 | 37.6 / 0.978 | **39.0 / 0.987** |
| ±2 | 37.8 / 0.977 | 37.1 / 0.975 | **38.1 / 0.982** |
| ±3–4 | 37.3 / 0.975 | 36.7 / 0.973 | 37.4 / 0.978 (≈0) |
| ±6–12 | 37.5 / 0.974 | 36.6 / 0.970 | 37.0 / 0.971 (loss) |

Two things fall out. Propagating the *best low-bitrate* frame loses
everywhere — its high PSNR is lower quantisation error, not surviving detail
(HF energy 1.80, same as its neighbours). Propagating a *restored* frame gains
for ±2 and the copied detail is real (HF 1.91–1.95 against ground truth
1.90–1.98). Past ±2 integer alignment drift eats it.

Implemented as `BlockMatch::RecalledMoved`: the diamond search also runs
against the **anchor** — the last redrawn frame, whose output is a fresh
network pass — under a flat RMSE gate (`--anchor-rmse`) rather than the JPEG
gate, which is the gate the measurement used. `--anchor-max-age N` forces a
new anchor every N frames, bounding the propagation distance.

Non-obvious result: **bounding the age helped SMITE a lot** (54.5% → 31.4%).
Fresh anchors are closer, so more blocks pass the gate; 20 redraws in 120
frames is the floor and propagation fills the rest. On film the unbounded
version is slightly better (33.3% vs 36.9%) because its shots are short
anyway. Sweep `--anchor-max-age` in 3–5; the oracle says the gain dies past
±4 regardless of content.

`--anchor-rmse 8` costs almost nothing over 12 on film (33.6% vs 33.3%), so
tighten it first if quality is soft.

## 3. Sub-pixel — where the motion actually is

`--halfpel` refines a failed integer match to the eight half-pel neighbours;
at ×2 a half-pel input offset is an integer output offset, so the merge stays
an exact copy. `--quarterpel` refines further; exact only at ×4.

- SMITE q60: 83.3% → 67.9% (redraws 52 → 19). Large.
- SMITE q90: 95.4% → 94.6%. Almost nothing.
- Lone Ranger: nothing at any quality.

The obvious hypothesis was "bilinear is too soft for a strict gate", so the
H.264 6-tap filter went in (`--halfpel-filter 6tap`). It is **not** the
explanation: same 100 frames at q80, bilinear 90.2% and 6-tap 90.2%, 60 vs 58
half-pel blocks per frame. Filter sharpness is not the limiter.

The diff image (`smite_frame0_and_8x_diff_to_frame1.png`, in the run logs)
shows why: a 3D game camera puts every 30 px block at an *arbitrary*
sub-pixel phase with per-block perspective. Half-pel is a lattice; the content
is not on it. Quarter-pel at ×4 recovers 2.4 more points, which is the right
order for one more bit of phase.

**Lesson.** If a sub-pel refinement helps at a loose gate and not a strict one,
the answer is more phases, not a sharper filter.

## 4. The plate — a good idea for footage we do not have

fao-codec's v11 plate (per-pixel temporal median of frames placed under global
motion) was ported as a block source (`BlockMatch::Plated`, `--plate-window`),
including the phase-correlation pass, with two fixes: the sub-pixel fit now
wraps at the window edge (the original returns 0 at index 0, which is the
static-camera case), and the estimate is the median of five 512-px windows
rather than one, so an actor filling the centre does not read as the camera.

It is a **net loss** on all three clips. Lone Ranger 79.7% + 4.5% plate cost
versus 80.6% baseline; the plate serves ~24 of 2304 blocks per frame. Reasons,
in order of importance:

1. A single global translation is the wrong camera model for handheld. Debug
   (`D2X_PLATE_DEBUG=1`) shows 30–46 px/frame with a vertical component and
   cumulative drift of 558 × 228 px over one window.
2. On the game clip the camera is sub-pixel every frame, and the plate is
   placed at integer offsets by construction.
3. Even the aerial "locked-off" shot jitters 1–5 px/frame at confidence
   0.6–0.9, so windows collapse to 2–4 frames.

fao-codec's own log reached the same conclusion from the other side
(`docs/SESSION_LOG.md` v11.6: +1.21 dB handheld, +0.41 dB locked-off, with the
translation-only model named as the binding constraint).

Keep it off. Turn it on only for a genuinely locked-off or slow-pan shot —
check `D2X_PLATE_DEBUG=1` for confidence ≥ 0.8 and drift under ~2 px before
spending time on it. The sub-pixel version (`v11_plate::build_plate_sr`, S=2,
exact at ×2) is the next step if that class of footage matters.

`--mv-seed` (seed the diamond search with the frame's global motion) came from
the same port and measured no change on either clip: nothing passes the gate
at any integer offset on this footage, seeded or not. Harmless, off by default.

## 5. Low-bitrate artefacts are temporally coherent — temporal fusion cannot fix them

The hypothesis was the screen-door one: a frame's codec artefacts differ from
its neighbours', so a motion-aligned median keeps what frames agree on and
rejects what only one says.

`d2x-core/examples/tdenoise` implements it: per-block diamond search against
each of ±K neighbours, seeded with accumulated phase correlation, RMSE gate so
misregistered blocks stay out of the vote, per-pixel median. 97% of blocks
found at least one aligned neighbour, so the alignment side works.

Scored against the 2.5 Mbps encode of the same frames (frames 4–27):

| | PSNR | SSIM |
|---|---|---|
| 558 kbps as-is | 37.46 | 0.9740 |
| MC median ±4 | 37.45 | 0.9750 |
| MC median ±2 | 37.44 | 0.9746 |
| sharpest aligned candidate ±4 | 36.51 | — |

Zero. Modern AV1 at low bitrate fails as **coherent smoothing** — deblocking,
CDEF and loop restoration remove the same detail in every frame — not as
blocking or independent noise. The frames agree, so the median has nothing to
vote against. "Take the sharpest aligned block" loses 1 dB on every frame
because high-frequency energy cannot distinguish detail from misalignment.

The GOP quality spread that might be mined is mild: one frame at 40.7 dB, the
rest 36–38, and the 40.7 dB frame has the same HF energy as its neighbours.

**Lesson.** Cross-frame voting recovers information the encoder removed from
*some* frames. It cannot recover information removed from all of them. That
needs a prior — i.e. a network. `tools/pair_dataset.py` turns the low/high
pair into a training set and is the eval harness for exactly that.

The remaining cross-frame information is sub-pixel: frames sample the world at
different phases *before* the encoder low-passes them, so their aliasing
residue differs even when their smoothing matches. That is the SR-plate
mechanism and it needs sub-pel alignment.

## 6. Reflections do not separate by motion

Multi-peak phase correlation (top 3 peaks, 30 consecutive pairs of the water
shot): if a reflection moved as a second rigid layer, a second peak would sit
at a stable offset. It does not — second-peak offsets are scattered, median
height 0.13 of the first.

The planar-transparency model that motion-based layer separation needs assumes
a static scene behind a flat reflector. Waves are non-rigid; chrome on a moving
robot reflects a moving environment off a curved moving surface. Neither
qualifies. Base-texture ⊗ shading separation is *intrinsic decomposition*, a
per-frame learned problem, not a multi-frame motion one.

## 7. Subtile caching — and the ordering bug that hid it

The block cache was only ever fed from packed residual cells. A redraw draws
hundreds of cells and threw all of them away after writing one frame.
**Harvesting** (`--cache-harvest`, default on) puts every cell of a redraw or
region redraw into the cache, keyed like an ordinary cell. Those blocks are
*better* than residual-fed ones: they were drawn with real neighbours around
them rather than a bleed ring.

Round 1 measured **nothing**: inserts 14,567 → 54,390 (3.7x supply, free), hits
34 → 41 of 14,600 probes. Approximate probing (coarse quantised bucket +
verify under the gate, `--cache-quant`) was flat too: 36 hits at every
quantiser from 8 to 48, with a 4×4 signature and again with 2×2.

The cause was neither supply nor matcher. **The probe was in the wrong place.**
A frame about to be redrawn returned before the cache was consulted, and an
`--anchor-max-age` redraw short-circuited even earlier — so the two kinds of
frame most likely to repeat already-drawn content (a cut back to a known shot;
a periodic anchor) were the only frames that never probed.

Round 2, after moving the probe above the redraw decision and making a scene
cut and a forced anchor fall through instead of returning:

| lone-ranger, 120 f, `--anchor-rmse 12 --anchor-max-age 5`, 512 MB | hits / probes | pixels |
|---|---|---|
| no harvest | 38 / 72,240 (0.1%) | 36.9% |
| harvest | **15,710 / 72,280 (21.7%)** | **31.9%** |
| harvest + `--cache-quant 16` | 16,718 / 128,927 (13.0%) | 31.6% |

Transformers 639 → 1,129 hits (61.5% → 61.3%). SMITE 8 → 1,230 hits, pixels
unchanged — a 3D camera repeats nothing exactly.

Two changes that are each worthless alone. `--cache-quant` doubles the probe
count for a few percent more hits: a flag, not a default. A quantised hash is
still an exact match in a coarser space, which is why it adds so little; LSH or
a small ANN index over block descriptors is the honest version of that idea.

`analyze` now runs the cache **dry** — keys and byte costs, no pixels — so hit
rates can be measured without a GPU. A test pins that a dry cache evicts
identically to a real one of the same budget, and a merge handed a placeholder
fails its length check, so it cannot leak into output.

## What is still open

1. **Restoration net.** The Transformers pair is a training set;
   `tools/pair_dataset.py extract` writes verified-aligned crops and `score` is
   the judge. On the evidence in §5 this is the only thing that recovers detail
   from a low-bitrate source. SRVGG-class, exported with `tools/pth2st.py`, run
   as a cascade tier.
2. **Cache that outlives the shot.** 512 MB at 1080p holds ~4 seconds; film
   cuts back to the same set over far longer, and the FIFO discards shot A
   before shot A returns. An on-disk index keyed by a shot descriptor matches
   how film is actually cut.
3. **Probe with motion, not identity.** `RecalledMoved` serves 1238
   blocks/frame on SMITE because it searches a *position*. An ANN index over
   block descriptors would give the cache the same power.
4. **Quarter-pel everywhere at ×4**, and the S=2 SR plate for locked-off
   footage.

## Standing advice

- **`analyze` counts pixels, not quality.** Every % in this document is a cost
  number. The RMSE-12 anchor gate accepts things the JPEG gate refused, and a
  cached hit is a block drawn in another frame's context. Confirm with
  `dandere2x compare -r base.mkv -t test.mkv --per-frame` and read the
  **minimum** frame, not the mean — propagation errors cluster.
- **Check the footage class first.** Whip-pan film, sub-pixel 3D camera, and
  locked-off content give three different answers to the same question. Two of
  the three clips here are the hard cases, which is why so much of this record
  is bounds rather than wins.
- **Look at the diff image before theorising.** `ffmpeg` two frames, subtract,
  amplify 8x. It settled the reflection question and the sub-pixel question
  faster than any run did.
- Debug switches: `D2X_PLATE_DEBUG=1` prints per-pair global motion,
  confidence and window boundaries; `analyze` prints the per-frame block
  breakdown (`stat mov half q4 anc plt rec cache miss`) and the cache line.
