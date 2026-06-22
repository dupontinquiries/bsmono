# Voice performance: measured findings (v0.1.646)

Measured with a synthetic-PCM benchmark of the PURE matching path (no
whisper, no voice files needed). x86_64 sandbox CPU — absolute ms differ
from the M2, but scaling shape + relative cost transfer.

## top_matches (the phonetic "instant preview" path) — MEASURED
Linear in DB size, with a flat (prefilter-defeated) synthetic DB:
    DB    10 fps : 0.13 ms/call
    DB    50 fps : 0.25 ms/call
    DB   100 fps : 0.36 ms/call
    DB   300 fps : 1.17 ms/call
    DB   600 fps : 2.22 ms/call
    DB  1000 fps : 3.58 ms/call
Real DB is ~85 fps → ~0.3 ms/call. The phonetic layer is ALREADY
effectively instant. It is NOT the latency you feel.
Real-world is even faster: candidate_bins (v612) pitch-prefilter skips
~80% of cross-speaker fingerprints; my synthetic seeds cluster pitches and
partly defeat it, so production numbers are lower than the table.

## Where the ~1s actually is: WHISPER
The logs show "transcribed in ~1.0–1.8s". That's whisper FFI inference,
NOT the matching code. Can't be meaningfully benchmarked in-sandbox (no
Metal GPU; x86_64 CPU ≠ M2). The "make it instant" win is therefore NOT in
the matching loop — it's one of:
  1. Don't WAIT for whisper when the phonetic preview is confident enough
     to act (the two-tier system already previews at 250ms; the lever is a
     confidence gate that fires the command on a strong phonetic match
     without waiting for the 1500ms whisper pass). PURE + testable: a
     decision fn over the top_matches score margin.
  2. Speed whisper: the logs show "force-tiny requested but Tiny not
     loaded — falling through" — the tiny model isn't loaded, so even
     wake checks pay base-model cost. Loading tiny for wake/preview would
     cut that. (Config/FFI — user side.)
  3. Metal: whisper-rs has a metal feature (voice-metal) — the user's M2
     would benefit; check it's enabled in their build.

## Honest limits of in-sandbox voice work
- CAN: benchmark + fuzz all PURE matching (top_matches, dtw, cosine,
  similarity, candidate_bins, VAD, wake_gate, confusion table) with
  synthetic PCM/feature vectors. No voice files needed for PERFORMANCE or
  ROBUSTNESS (panic/edge fuzzing).
- CAN'T (meaningfully): time whisper inference (hardware-bound), or
  validate speaker-ID ACCURACY without real labeled voice samples
  (synthetic seeds prove the code runs + scales, not that it picks the
  right human). Real voice files WOULD help accuracy validation.

## The "instant" fast-path EXISTS but is gated on accuracy (key finding)
voice.rs ~3354: `tier1_skip_whisper = promote && hit.distance < threshold
&& hit.count >= 2` → returns the cached transcript and SKIPS whisper
entirely. THE INSTANT PATH IS ALREADY BUILT. It's OFF because:
  - the log shows "[tier1] promote set to false", and
  - shadow mode (compute-but-don't-act) shows frequent DIVERGE:
      distance=0.001 cached="yes"          whisper="open a file"
      distance=0.004 cached="go to apple.com" whisper="command run cargo test"
      distance=0.021 cached="go to the end of the video" whisper="yeah"
A distance of 0.001 (near-identical acoustics) mapping to the WRONG
transcript means the acoustic fingerprint → transcript LABEL is stale: the
same voice signature is cached against an old/unrelated transcript. So
flipping promote=on today would fire WRONG commands.

### The real "make it instant" work (accuracy, not speed)
Reduce shadow divergence until promote is safe to enable. Investigable
in-sandbox with synthetic + fuzz data:
  - why does a 0.001-distance hit carry a wrong transcript? (cache keying /
    eviction / label staleness in acoustic_index)
  - should the gate also require the cached transcript to be recent / not
    contradicted? (add a freshness or agreement check to the skip
    condition — pure, testable)
  - tune thresh_same/thresh_cross from the shadow histogram.
This is the highest-value voice work and most of it is now sandbox-doable
(compile + fuzz the acoustic_index cache logic). Needs care: the failure
mode is executing the WRONG command, so changes go behind the existing
promote opt-in + shadow validation, never blind-on.
