# Plan: speaker-detection upgrade + honest take on threaded/capped lookups

## Part 1 — the threaded-lookup + 5ms-cap idea: NOT the bottleneck (measured)
The instinct (cap runtime, parallelize, stream partial results) is sound
ENGINEERING — but the data says it's aimed at the wrong place right now:
- Measured: top_matches = 0.3 ms at the real DB (~85 fp), 3.6 ms at 1000.
- A 5 ms budget isn't crossed until ~1400 fingerprints.
- Thread spawn/join (~10-50 µs) is comparable to the ENTIRE scan at current
  size — threading 85 entries would be SLOWER (overhead > work).
- The ~1 s latency you feel is WHISPER (FFI), not the lookup. Parallelizing
  a 0.3 ms step against a 1000 ms problem saves nothing felt.

### The version of the idea that IS worth building (deferred until it earns it)
- A pure, testable "anytime" scan: a time/》-budget that returns best-so-far
  and flags "partial" if a cap is crossed. Build it as a NO-OP at current
  scale (cap so high it never fires < ~1000 fp), purely as future-proofing
  for when cross-session persistence grows the DB. Cheap insurance, not a
  speedup today.
- A rayon-parallel scan GATED on N > a measured crossover (~500+ fp), so
  small DBs stay single-threaded. Add rayon only when the DB can actually
  get that big (i.e. after cross-session persistence lands).
Neither is urgent. Documented so the instinct isn't lost; both wait until
the DB size justifies them.

### Where parallelism ACTUALLY pays now: ACCURACY, not lookup speed
Whisper already runs ~1 s. In that window the CPU is mostly idle on the
matching side. Spend it running RICHER / MULTIPLE speaker matchers in
parallel and combining them → better speaker ID → lower shadow divergence
→ the whisper-skip fast-path (voice.rs ~3354) can finally turn on. THAT is
the real "make it instant" path, and it routes straight into Part 2.

## Part 2 — increase speaker detection (the real prize; now unblocked)

### Why it matters
Current speaker "embedding" = a 13-dim MEAN of MFCC frames
(acoustic_index.rs). The code's own comments admit it: noisy cross-speaker
matches, no temporal info. This coarseness is WHY the shadow path diverges
(a 0.001-distance acoustic hit carried the wrong transcript) — the feature
can't reliably separate who-said-what. Better speaker features → less
divergence → fast-path unlock + correct attribution for the sudo-shim
tiers.

### What's newly possible (verified this session)
candle-core (pure-Rust tensors, NO C++/ONNX/Python) BUILDS in-sandbox in
~79 s. That's almost certainly the old blocker — a real neural speaker
embedding (ECAPA-TDNN / x-vector / wespeaker) needs a tensor runtime, and
now one compiles. So the advanced path is open. But it carries a
model-weights dependency + real size, so it's the SECOND step, not first.

### Staged plan (each step verifiable here; lowest risk first)

**Step 1 — richer CLASSICAL embedding (no new deps, pure, do first).**
compute_mfcc already returns the full n_frames×13 matrix; we throw it away
by averaging. Replace the 13-dim mean with a 39-dim vector:
  [ mean(13) | std-dev(13) | delta-mean(13) ]
- std-dev captures within-utterance variance (voice texture).
- delta-mean captures spectral DYNAMICS (how the voice moves) — a strong
  speaker cue the static mean misses.
Pure + testable + fuzzable in-sandbox NOW. Bump the acoustic_index file
FORMAT version (AI01→AI02; the format already reserves this — unknown
versions fall back to whisper cleanly), so old DBs degrade gracefully.
Measure separation: synthesize N "voices" (distinct formant seeds),
fuzz cross-vs-same-speaker cosine margins, confirm 39-dim separates better
than 13-dim before shipping. Expected: meaningfully lower cross-speaker
false matches at ~zero added latency (39 floats vs 13 is still µs).

**Step 2 — score fusion (uses the "match multiple times in parallel" idea).**
Run the classical matcher AND a second view (e.g. pitch/F0 contour
similarity, already computed in TonalProfile) in parallel during the
whisper window; fuse the scores (weighted). Pure fusion fn + tests. This
is where "match multiple times" earns its place — for ACCURACY, free under
the whisper latency. Lower divergence further.

**Step 3 — neural embedding (candle), only if 1+2 don't get divergence low
enough to flip promote on.** Load a small pretrained speaker-embedding
model (ECAPA-TDNN-style) via candle; produce a 192/256-dim x-vector;
cosine over those. Biggest accuracy jump, biggest effort + a model-weights
ship. Gate behind a feature flag; keep classical as fallback. Verify the
embedding extraction in-sandbox (candle runs on CPU here); validate
ACCURACY needs real labeled voice samples from the user.

**Step 4 — flip the fast-path.** With divergence measured low (shadow
histogram), enable promote with tuned thresholds. THIS delivers the
"instant" experience — whisper skipped on confident hits. Stays behind the
opt-in + shadow validation; never blind-on (failure mode = wrong command).

### What I need from the user (only for Step 3 accuracy validation)
A handful of labeled voice samples per speaker (e.g. 5 utterances each from
2-3 people) would let me validate that the embedding actually separates
real humans — synthetic seeds prove the code runs + scales, not that it
picks the right person. Steps 1-2 need NO voice files.

## Recommended order
Step 1 (richer classical embedding) now — pure, no deps, measurable, and it
alone may cut divergence a lot. Then re-measure shadow divergence before
deciding whether 2/3 are needed. Don't build the threaded/capped lookup or
the neural model until the data says they're warranted.

## TODO: collect family voice samples (for Step 3/4 accuracy validation)
Get labeled voice samples from family WHEN THEY VISIT — a handful of
utterances each from 2-3 different people (e.g. 5 short commands per
person), saved with who-said-what labels. This is the one thing synthetic
data can't substitute for: it validates that the speaker embedding
separates REAL humans (not just synthetic formant seeds) and lets us tune
the promote thresholds so the whisper-skip fast path can be safely enabled
(Step 4). Steps 1-2 need no samples; this unblocks 3-4.
