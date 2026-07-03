# Path A — the beatbox / vibe-capture pipeline

Path B (Scratchpad-first) is what's shipping now. Path A is the audio half of
the workflow that feeds Scratchpad with a real recorded vibe instead of a hand-
typed one. This document is the durable record of decisions so they don't
get lost between sessions.

## The four stages (per the workflow diagram)

1. **Beatbox** — capture the bed. Either vocally beatbox 1–4 layers (kick,
   snare, hat, melody hum) OR load an existing `.opus` track and set in/out
   points to define the loop region.
2. **Calibrate** — one-shot probe before playback to decide whether echo
   cancellation should run on the Vibe-capture mic stream.
3. **Vibe capture** — loop the bed, mic armed, user freestyles or hums over.
   Recording saved; analysis extracts a `VibeSummary` (BPM, syllables/bar,
   stress pattern, dominant pitch range).
4. **Scratchpad** — handed off to Path B with a real `VibeSummary` instead
   of the manual one. Mic stays hot, captured fragments persist.

## Bed sources

### Vocal beatbox

Same `cpal` capture path we already use. New: a 1–4 layer stacking model.
Each layer is a fixed-length loop (quantised to bar count); user records into
each slot separately. Mute/solo per slot. Mix is summed at playback.

Tempo detection: autocorrelation on the onset envelope of the first recorded
layer (typically kick). Failure mode is the half-time / double-time ambiguity
common in trap (~70 BPM beats often read as 140). Mitigation: tap-to-tempo
fallback before recording, and a "double/half" toggle on the detected value.

### `.opus` file / folder

Open Folder → scan recursively for `*.opus` → for each:

- Decode via `symphonia` (handles opus in ogg container).
- Probe duration.
- Run BPM detection (same autocorrelation pipeline used for vocal beatbox).
- Cache: `(path, bpm, duration, ms_per_bar)` so re-opening the same folder is
  fast.

UI: a list of beats with BPM and duration columns, filterable by BPM range
(e.g. "70–90 trap"). Click → loads, shows full waveform.

**Bounds (in/out points).** Two draggable handles over the waveform define
the loop region. Snap to detected bar boundaries by default; hold modifier
to drag freely. The selection becomes the bed; everything outside is
discarded for the session. Looping is sample-accurate over the [in, out)
range.

Crates needed:

```toml
symphonia = { version = "0.5", features = ["opus", "ogg"] }
```

## Echo cancellation: `webrtc-audio-processing`

We commit to real AEC, not detect-and-warn. The crate is `webrtc-audio-processing`
on crates.io (wraps the WebRTC project's AudioProcessing module — battle-tested,
used by browsers).

System deps on Linux: `libwebrtc-audio-processing-dev` (Debian/Ubuntu) or the
equivalent. Adds a build-time burden but the runtime behaviour is correct.

Wiring shape:

```
playback stream (the bed) ──► AEC reference channel
mic stream  ─────────────────► AEC primary channel
                                       │
                                       ▼
                            ec-processed mic samples
                                       │
                                       ▼
                            onset / pitch / syllable analysis
```

Sync constraint: AEC needs the reference to lead the primary by a few ms (the
expected speaker→mic latency). cpal exposes per-stream timestamps; we feed
both into a small ring buffer that aligns them before handing off to the
processor.

Gate behind a Cargo feature so Path B builds don't drag in the system dep:

```toml
[features]
aec = ["dep:webrtc-audio-processing"]
```

Path A wires up the feature; Path B users never hit it.

## Calibration probe

Before the first Vibe-capture pass:

1. Play a 200 ms swept sine (or a clean chirp) at moderate volume on the
   default output.
2. Record the mic during and just after.
3. Cross-correlate the played signal against the captured one.
4. If correlation peak > threshold AND lagged by a plausible speaker→mic
   distance (1–80 ms): there's bleed. Enable AEC for the session.
5. If correlation is in the noise floor: probably headphones. AEC off.

This is a 300 ms one-shot — not a constantly running filter — done once per
session change.

## Vibe analysis → `VibeSummary`

The recorded hum-over track is analysed to populate the same `VibeSummary`
struct that Scratchpad already consumes (defined in `src/vibe.rs`). The work
breaks down as:

| Field | How it's computed |
|---|---|
| `bpm` | Inherited from the bed loop (already known). |
| `syllables_per_bar` | Count onsets in the hum track, divide by bars. |
| `stress_pattern` | Onset peak amplitudes thresholded into stressed/unstressed. |
| `rhyme_seeds` | None — the user types these in Scratchpad. |
| `vocab_name` | Inherited from the active session. |
| `source` | `VibeSource::Recording { audio_path }` |

The audio path lets Scratchpad replay the original take alongside the lyrics
the user writes against it.

## Arranger export (future, not in scope yet)

Vibe sessions serialise to a structured JSON document containing the bed
metadata, the vibe summary, the lyrics, and references to the recorded audio
fragments. The arranger binary consumes one or more session files and stitches
them into a full track. Out of scope until A is shipped.

Tentative shape, for context:

```json
{
  "version": "1",
  "bed": { "source": "opus", "path": "...", "in_ms": 4200, "out_ms": 16400, "bpm": 76 },
  "vibe": { "bpm": 76, "syllables_per_bar": 8, "stress_pattern": "x.x.x.x.", "vocab": "Juice WRLD" },
  "lyrics": "running from her love\ndigging up a grave\n...",
  "fragments": [ { "ms_start": 8200, "ms_end": 11400, "audio_path": "..." } ]
}
```
