# Speaker representation: vector embeddings, not a scalar similarity

Status: **DESIGN + pure core (v0.1.498).** Captures a convergence the
user and I reached independently, and confirms existing precedent in
the codebase. Pure embedding+distance core built and tested; rewiring
identify() and any disk-format change deliberately deferred.

## The realization (user, paraphrased)

Representing a speaker through ANY single metric is a weakness no
matter how good that metric is. A hand-tuned scalar similarity
(pitch + timbre + voicing with intuited weights) is a low-dimensional
collapse of a high-dimensional thing, and every weight choice is a
guess that fails somewhere. The right model is to **vectorize**
speakers (and temporary speakers) and reason about distance /
membership in a high-dimensional space — closer to an embedding /
neural-network approach.

## Why this is correct (and the layering it unlocks)

A speaker is a REGION in feature space, not a point on a dial.
"Same person?" is a distance/membership question, not a threshold on
one blended number. Production speaker-ID uses learned embeddings
(d-vectors / x-vectors), not weighted feature sums, for exactly this
reason.

This also makes the v0.1.497 layered-corpus operations GEOMETRIC
instead of threshold-hacky:
  - a **temporary speaker** = a cluster of nearby vectors not yet
    promoted;
  - **merge** = cluster join;
  - **split a wrongly-merged speaker** (the v0.1.496 bug's cleanup) =
    re-cluster;
  - **general vocabulary** = vectors that recur across many speakers'
    clusters.
The vector view and the layered-corpus view are the same idea from
two directions.

## Precedent ALREADY in the codebase (important)

`acoustic_index.rs` (v0.1.284) ALREADY uses a vector approach — for
phrase caching, not identity: each utterance → a 13-dim MFCC-MEAN
vector, cosine-distance nearest neighbour. Its own comments record an
empirical fact that matters here:
> same-utterance same-speaker ~0.05, same-word same-speaker ~0.10,
> cross-speaker ~0.30+
i.e. the MFCC-mean vector ALREADY separates speakers better than the
pitch-based TonalProfile::similarity does. The vector machinery and
the discriminating signal both already exist — they just aren't used
for speaker IDENTITY.

## Proposed representation

### SpeakerEmbedding — a fixed-length voice vector
Richer than the phrase-mean (which conflates WHAT was said with WHO
said it). To characterise the VOICE across phonemes, pool MFCC
STATISTICS over the utterance:
  - mean of each of the 13 MFCC coefs (13 dims) — spectral envelope
  - std-dev of each coef (13 dims) — how much the voice MOVES (this
    is the seed of the singer / speed-talker / rhythm dynamics the
    user envisions: a singer holding pitch has low variance in some
    coefs; a speed-talker compresses timing → different variance
    signature)
→ a 26-dim embedding. Cheap, no FFT beyond the MFCC already computed,
no model. L2-normalized so cosine distance is well-behaved.

(Future: replace the hand-pooled 26-dim vector with a LEARNED
embedding from a small on-device model via the ONNX/tract route —
this is where the "neural network approach" and the eventual
singing/rhythm experiments live. The pooled vector is the bridge:
same INTERFACE — `embed(audio) -> Vec<f32>` + distance — so swapping
the producer later doesn't disturb the identity/clustering layer.)

### Identity as distance, not threshold-on-scalar
identify(audio) → embed → nearest confirmed-speaker centroid by
cosine distance, with:
  - a distance gate (reject if nearest is too far → unknown);
  - a MARGIN gate (reject if 1st and 2nd nearest are too close →
    ambiguous, same principle as v0.1.496's identify guard, now in
    vector space);
  - speakers stored as CENTROID + spread (so membership is
    Mahalanobis-ish, not just centroid distance — a tight-voiced
    speaker and a variable one get different acceptance radii).

## What v0.1.498 actually ships (pure core only)

- `speaker_embed.rs`: `embed_from_mfcc(mfcc, n_frames) -> Vec<f32>`
  (26-dim mean+std, L2-normalized) + `cosine_distance(a, b)` +
  `embedding_margin(query, candidates) -> (best_idx, dist, margin)`
  (nearest + ambiguity margin). Pure, no IO, fully tested.
- NOT yet: rewiring identify(), changing Speaker storage, or any
  disk format. Those calcify representation/format and need the
  v0.1.497 open questions answered + a real build. Sequenced.

## GPU acceleration (Vulkan/Mesa) — where it helps, where it doesn't

User asked where Vulkan/Mesa GPU accel could cut multi-speaker
processing cost. Honest analysis:

- **DTW matcher (current path): NOT a GPU fit.** DTW is a sequential
  dynamic-programming recurrence (each cell depends on left/up/diag)
  with a data-dependent early-termination cutoff — the worst case for
  a GPU (no vectorization across the recurrence, heavy branch
  divergence, dispatch overhead per query). It's already threaded on
  CPU with an atomic cutoff. Moving it to Vulkan would likely be
  SLOWER. Leave it on CPU.
- **Embedding distance (this direction): GOOD GPU fit.** Once
  speakers are vectors, multi-speaker identity = one query vs N
  centroids by cosine distance = a batched GEMV: flat data, one
  independent output per centroid, no branches. Scales as a single
  dispatch as the corpus grows.

**What v0.1.499 ships (no new dependency):** `batch_cosine_distance`
written AS a GPU compute kernel would be — flat contiguous buffer,
one branchless reduction per row, single guarded division at the end.
This is fast + auto-vectorizable on CPU NOW, and maps 1:1 onto a
Vulkan compute shader LATER (upload query+centroids, dispatch one
work-item per centroid, read back distances). Tested to match the
per-pair reference exactly (the oracle a future Vulkan port
validates against).

**Why no Vulkan code yet:** there is no general GPU-compute crate in
the project (the only Vulkan is internal to llama-cpp-sys, not
callable for arbitrary compute). A real shader path needs a heavy new
dep (wgpu/ash) + SPIR-V + the Mesa driver path — premature while the
embedding isn't even wired into identify(), and unverifiable in the
sandbox. The kernel-shaped CPU function is the right seam: prove the
operation, defer the dependency until corpus scale justifies it.

## Sequencing (when built)

1. (this) pure embed + distance + margin core, tested.
2. Speaker stores a centroid embedding (+ spread). Additive; pick a
   format that versions cleanly (learn from FP01…FP05).
3. identify() uses embedding distance + margin + spread, with the
   provisional buffer (v0.1.497) holding uncertain queries.
4. Clustering for temp-speaker promote/merge/split.
5. (far future) learned embedding model swaps in behind embed()'s
   interface; opens the singer / speed-talker / rhythm / sung-command
   experiments the user envisions.
