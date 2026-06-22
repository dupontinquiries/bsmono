# Dead-air / filler cleaving — the reVos/kss algorithm, ported

Status: **DESIGN + pure core (v0.1.512).** Roadmap item 4, the
signature reVos-lineage feature. Read the actual uploaded source
(reVos-main IDEA.md + kss6.py) and ported the ALGORITHM — not a naive
fixed-threshold gate — as pure native Rust. The audio-pipeline
integration (feeding it real per-chunk levels from the capture path,
acting on the cleave plan) is the live surface, deferred.

## What reVos/kss actually did (read from source)

reVos IDEA.md describes three relevant protocols:
  - **SS Protocol**: speed up quiet/silent parts on a user-malleable
    SPEED CURVE (e.g. logarithmic) — not just cut, variable-speed by
    how quiet/long the gap is.
  - **Lecture Protocol**: voice-centric cleaving that skips silent
    parts (the kSS predecessor — hard cut).
  - **Uh-ware**: remove filler sounds via a preset database / user
    selection.

kss6.py's silence detection (the robust part worth porting):
  1. **ADAPTIVE threshold** (kss6 line 738): the silence cutoff is NOT
     a fixed dB constant. It's derived from THIS recording's volume
     distribution:
        vt = THRESHOLD_FRAC * (max_volume + mean_volume) / 2
     so it auto-adapts to a quiet vs loud speaker, silent vs noisy
     room. This is why kss didn't collapse across different recordings
     the way a fixed -40dB gate would.
  2. **DUAL-signal keep** (line 739): a chunk is voice (kept) if
     `loudness > vt` OR `spread > vrt` (a second "reach/spread" signal
     with its own adaptive threshold). The OR catches consonants/
     transients a pure-loudness gate misses.
  3. Per-preset params (voice vs music treatment, period, max chunk).

## The port (v0.1.512) — pure core

`src/dead_air.rs` works on the abstraction the dev-loop capture path
can supply: a sequence of fixed-duration audio FRAMES, each with a
loudness level (dBFS-like) + a spread/variation measure + a time span.
It does NOT do audio decoding (that's the pipeline's job) — it takes
levels and returns a cleave plan. Pure + testable.

Ported faithfully:
  - **Adaptive threshold** from the frame loudness distribution
    (max + mean midpoint × fraction) — no magic dB constant.
  - **Dual-signal keep** (loudness OR spread over their thresholds).
  - **Hysteresis / min-silence-run**: a silent run must exceed a
    minimum duration before it's cleaved (kss's period/keep-silence —
    avoids chopping the natural micro-gaps between words, which would
    make speech sound robotic). Short gaps are kept.
  - **Speed-curve generalization** (SS Protocol): instead of only
    hard-cutting, each cleaved gap carries a suggested compression
    (keep a tiny lead/​tail, or a speed factor) by its duration — the
    caller chooses cut vs speed.

Output: a Vec<Segment { start_ms, end_ms, kind: Voice | DeadAir }>
plus summary (total dead air, % compressible). The caller (audio
pipeline / transcript timeline) acts on it: hard-cut DeadAir, or
speed it per the curve, or drop filler spans.

## Why adaptive + dual-signal + hysteresis (the robustness)

A naive fixed-threshold gate (the "very basic algorithm") fails three
ways kss's design avoids:
  - fixed dB → wrong for any recording that isn't the calibration
    loudness (adaptive fixes this);
  - loudness-only → clips soft consonants / trailing voice (dual
    signal fixes this);
  - no min-run → chops inter-word micro-gaps into robotic staccato
    (hysteresis fixes this).
Same pattern as the cinematic-camera port: extract WHY it felt good,
rebuild it more robustly.

## Filler words (Uh-ware) — ALREADY EXISTS, wire don't rebuild

kss/reVos's uh-ware (remove "um"/"uh"/pen-taps). UPDATE (v0.1.514
wiring audit): a corpus-LEARNED text-level filler filter ALREADY
EXISTS in the backend — filler_words.rs + token_stats::strip_fillers,
exposed as voice_is_filler / voice_strip_fillers / voice_mark_filler /
voice_unmark_filler (currently unwired to QML; see
docs/wiring_audit_v2.md finding B). So dead_air should NOT grow its
own filler stripper. Division of labour: dead_air handles ACOUSTIC
dead air (this module); the existing filler backend handles TEXT
fillers; they compose on a transcript-with-timings. The acoustic
per-sound classification (pen-tap detection) remains the deep arc.

## Deferred (live surface / can't sandbox-verify)

Feeding real per-frame levels from the capture path; whisper word-
timing alignment for filler spans; acting on the plan (cut/speed).
The pure cleave-planning + adaptive thresholding is what ships now.

## v0.1.513 refinement — spread cannot rescue silence (a fix the port needed)

Adding `frames_from_samples` (the samples→Frame bridge) + an
end-to-end test surfaced a real flaw in the faithfully-ported dual-
signal OR: a digitally-silent frame has spread ≈ 0 (peak≈rms≈0), and
the ADAPTIVE spread threshold (vrt) can sit at/below 0 when most
frames are near-silent — so `spread > vrt` was TRUE for silence,
keeping it and defeating cleaving entirely. kss never hit this because
pydub fed it real audio where silence still has nonzero spread; clean
synthetic silence exposed it. Fix: spread may only RESCUE a frame
whose loudness is above a silence floor (vt - 20dB) — so the second
signal catches soft consonants (which have loudness) but never lifts
true silence. More robust than kss's raw OR.
