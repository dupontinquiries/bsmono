# Voice DB matching speed

## The problem (from hardware logs)
Logs flood every utterance with:
  [phonetic] pitch index miss for bin N, falling back to full scan
  [phonetic] rejecting stale pitch hint ... ratio 1.3x-9x > 1.3

The phonetic DB lookup (top_matches → DTW per candidate) prunes
candidates by pitch bin (50Hz wide) using a q_bin±1 window. DTW is the
dominant cost (~2-10ms/candidate), so pruning matters. BUT pitch
estimation is noisy — the SAME speaker logs as 477Hz then 578Hz (a 2-bin
swing). When the real match sat in bin q_bin±2, the ±1 window found
nothing → FULL SCAN (DTW vs every fingerprint), firing on most
utterances. So the index's pruning benefit was being thrown away exactly
when it mattered.

## Fix (v0.1.587)
Progressive widening in candidate selection: try q_bin±1, and if empty
widen to q_bin±3 (±150Hz, past the observed noise) BEFORE the full-scan
fallback. Keeps pruning on the common noisy-pitch case and ALSO finds
matches the narrow window missed (a correctness win too). __wake_word__
fingerprints are still always included (v0.1.576) regardless of bin.

## Latent bug this work exposed
Getting cargo + real deps (libc, aho-corasick) let phonetic's tests RUN
for the first time (they were never in the rustc-staging harness). That
immediately surfaced from_audio_survives_pathological_input failing:
1e30 audio samples overflowed band energies to +inf, total=inf,
band/total = inf/inf = NaN → NaN spectral centroid → poisons all
downstream speaker-match math. The `> 1e-9` guard passed for inf. Fixed
by also requiring total_band_energy.is_finite(). (v0.1.587)

## Further speed ideas (need the running build to validate)
- Cache the q_bin→candidate-set per session keyed on DB-revision, so
  repeated lookups in the same bin skip index assembly.
- The "rejecting stale pitch hint" spam suggests the pitch HINT
  mechanism is frequently wrong (hint 477Hz vs query 4608Hz = 9.6x).
  Worth investigating whether the hint helps or hurts net — it may be
  cheaper to drop a hint that's >2x off immediately (the code does
  reject >1.3x, but logs the rejection per-frame; rate-limit or
  short-circuit). Needs profiling on-device (WEB2QML_PROFILE).
- DTW early-termination cutoff tuning: dtw_bounded already early-exits;
  the cutoff could be seeded from the current best to prune harder.
