# Scene design rules + scoring spec

These are the rules a good mind-palace scene render should satisfy. Each rule has a **measurable proxy**
computed from the rendered image (and, where noted, from scene stats) plus a **weight**. The scoring harness
(`tools/score_render.py` + `tools/tune_harness.sh`) renders candidate parameterizations, scores them against
these rules, and ranks them. The committed scene code is the hand-tuned baseline; candidates are env-var
overrides rendered into a scratch dir and NEVER overwrite the tree. I (the agent) am the hand-tuner: the harness
narrows the search, I pick + integrate the winner.

## Why this exists
The bottleneck is iteration count, not idea count. Rendering one shot, eyeballing it, and hand-editing is ~1
useful iteration per turn. A scorer lets one turn sweep ~8–16 candidates, rank them objectively, and surface the
best 3–4 for me to judge — so my judgement is spent on finalists, not on the whole search.

## Measurement notes
- llvmpipe OVER-BRIGHTENS and over-blooms. The scorer therefore treats brightness as a *band* (too dark AND too
  bright are both penalized) and adds an explicit blowout penalty. Trust the scorer for layout/coverage/contrast/
  hue/structure; treat absolute brightness as advisory (final brightness is judged on the user's real GPU).
- The UI chrome is excluded: the top ~3% (toolbar) and bottom ~8% (status bar) rows are cropped before metrics.
- "Background" = median colour of the two top corners (sky/void); "covered" pixels are those far from it.

## World-only matching, pixel-level similarity, and Markov-chain search
- WORLD-ONLY render: `LOGSCOPE_WORLD_ONLY=1` suppresses the conversation overlay (node dots/cords/runes/labels,
  3D node-bodies, composer caption) and renders ONLY the environment. This is the clean surface for matching the
  *world* against a reference without the data layer in the way.
- PIXEL matching: `score_render.py --pixel --ref <world.png>` uses global SSIM + (1-MSE) on a common-size grayscale
  — TRUE pixel-for-pixel, only meaningful when candidate and reference are the SAME view (a target world screenshot,
  or a golden render for regression). Validated: self-match SSIM 1.00; different scene SSIM 0.07. For UNALIGNED
  concept art, omit `--pixel` and the color/layout/depth `ref_similarity` is used instead.
- MARKOV-CHAIN search: `markov_search.py "<Scene>" --steps N [--ref ...]` walks a WIDE continuous parameter space
  (tune_scale/height/density, hour, yaw, pitch) by Metropolis / simulated-annealing: propose a Gaussian step,
  render WORLD-ONLY, score, accept if better (or probabilistically if worse, with a cooling temperature), track
  the best. Far more sample-efficient than a grid; checkpointed to mcmc_state.json so the chain RESUMES across
  invocations (rendering is slow — a few steps per call). Candidates stay in the scratch dir; the tree is untouched.

## Reference-image scoring (optional, supervised)
A scene can have a TARGET image in `tools/refs/<scene_key>.png` (e.g. the garden concept → `mansion_garden.png`).
When present, the harness passes it as `--ref` and the score blends a reference-similarity term with the rule
score: `final = (1 - ref_w) * rule_score + ref_w * ref_sim`, default `ref_w = 0.3` (`--ref-weight` to change).
`ref_sim` (framing-tolerant, via OpenCV) = `0.70*color + 0.20*layout + 0.10*depth`:
- COLOR: HSV Hue-Sat histogram correlation — the cue that transfers across render styles (concept art ↔ llvmpipe).
- LAYOUT: 64×48 grayscale normalized cross-correlation — coarse composition/value match.
- DEPTH: per-row mean-luminance profile correlation — vertical value layering / atmospheric-perspective proxy.
  (True monocular depth (MiDaS) is omitted: the weights aren't downloadable in this sandbox. The row-profile is the
  practical depth/composition cue for these horizon-layered scenes.)
ref_sim is most meaningful as a WITHIN-SCENE relative signal: rank a scene's candidates against that scene's
reference. Absolute values are compressed when the reference is stylized concept art (very different from the 3D
render), so color dominates and ref_w is kept moderate. A hard-failed render (black/void/washout) is gated down so
a good palette match can't rescue a broken frame. Drop a `<key>.png` into `tools/refs/` to supervise any scene.

## Rules (proxy → target → weight)
1. EXPOSURE-IN-BAND (w 0.16): mean luminance L in [0.10, 0.42]. Falloff outside. Catches black-outs + washouts.
2. NOT-BLOWN (w 0.12): fraction of near-white pixels (L>0.92) should be < 0.06; ramps to 0 by 0.22. (llvmpipe bloom.)
3. CONTRAST/DEPTH (w 0.14): std(L) rewarded up to ~0.20 (flat scenes read dead; depth needs tonal range).
4. COVERAGE/FILL (w 0.18): fraction of covered pixels in [0.40, 0.85]. Penalize empty voids AND wall-to-wall clutter.
5. HUE VARIETY (w 0.12): saturation-weighted hue-histogram entropy in a mid band — coherent but not monotone,
   not chaotic. Monotone grey/empty scenes score low; rainbow noise also penalized above the band.
6. STRUCTURE/EDGES (w 0.14): mean gradient magnitude (Sobel-ish) rewarded up to a cap — silhouettes + detail,
   not a flat wash. Above the cap (visual noise) it plateaus, not rewarded further.
7. VERTICAL SPREAD (w 0.14): std of the y-positions of covered pixels / frame height, rewarded up to ~0.30 —
   content using the full HEIGHT of the frame, not a flat band across the middle. (Directly encodes the user's
   "height variation" vibe.)

Total = weighted sum, in [0,1]. Weights sum to 1.0. Tunable here; the harness reads them from the JSON block below.

## Hard fails (score forced toward 0)
- BLACK FRAME: mean L < 0.03 (render failed / black-out) → score 0.
- VOID: coverage < 0.12 (essentially empty) → score *0.3.
- WASHOUT: blowout fraction > 0.5 → score *0.4.

## Candidate parameter space (env overrides — never edits code)
- `LOGSCOPE_SEED` — regenerates the procedural layout/scatter.
- `LOGSCOPE_HOUR` — time-of-day lighting (0–24).
- `LOGSCOPE_SEASON` — Forest only.
- `POS_ANGLE_INIT` — "yaw,pitch,dist" camera.
- `LOGSCOPE_EXP`, `LOGSCOPE_SAT` — grading (advisory; real grading judged on GPU).
- `LOGSCOPE_TUNE_SCALE` — multiplies map_radius (room/scene size).      [non-invasive global knob]
- `LOGSCOPE_TUNE_HEIGHT` — multiplies the flow vertical meander.        [non-invasive global knob]
- `LOGSCOPE_TUNE_DENSITY` — multiplies per-scene scatter counts.        [wired per-scene over time]

## Workflow per scene
1. Generate a candidate list (param sweeps) into `tools/cand_<scene>.txt` (lines: `label|ENV...`).
2. `sh tools/tune_harness.sh <Scene>` → renders each to `/home/claude/candidates/<scene>/`, scores, ranks,
   writes `scores.csv` + a top-K montage with scores.
3. I review the top 3–4 renders, pick, and HAND-MERGE the winning params into the scene builder (the only step
   that edits the committed tree). Re-score to confirm the baseline improved.

## Scoring weights + bands (the harness reads THIS json block verbatim)
```json
{
  "weights": {"exposure":0.16,"not_blown":0.12,"contrast":0.14,"coverage":0.18,"hue":0.12,"structure":0.14,"vspread":0.14},
  "bands": {
    "exposure":[0.10,0.42], "blow_lo":0.06,"blow_hi":0.22,
    "contrast_cap":0.20, "coverage":[0.40,0.85],
    "hue_entropy":[0.85,1.5], "structure_cap":0.030, "vspread_cap":0.30
  },
  "hard": {"black_meanL":0.03, "void_cov":0.12, "washout_blow":0.5}
}
```
