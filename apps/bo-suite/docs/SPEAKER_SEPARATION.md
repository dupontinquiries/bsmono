# Speaker Separation: Why EQ isn't enough, and what to do

## What's working today

The "track separator mixer" applies per-band EQ across stereo. That
gets you frequency-domain control: boost bass, cut presence, etc.
applied to L and R together (or independently with per-channel gain).

This is genuine source shaping but it's not *source separation*. EQ
can't reach inside a mixed track and extract the vocal from the
instrumental, or send drums to one speaker and bass to another. The
frequency bands overlap too much: a male vocal lives at 80-800 Hz
which is also where the kick drum, bass guitar, and most of the
keyboards sit. Cutting 100-200 Hz to "isolate the vocal" removes
half the bass instrument too.

## Why "speaker separation" needs more than EQ

The user wants per-speaker assignment of musical sources — e.g.
"vocals to the Stage, drums to the Emerge, bass to Smokey." To do
that you need to *separate* the sources first, then route them.
Two routes available:

### Route A: M/S processing (cheap, partial)

Stereo recordings encode information in the L+R difference (the
"sides") and the L+R sum (the "mid"). The mid is mostly things
panned center: lead vocal, kick, bass. The sides are mostly things
panned wide: reverb tails, panned guitars, backing vocals.

By outputting `mid` to one speaker and `sides` to another, you get
real perceptual separation for ~50% of mainstream pop content — at
the cost of zero CPU. Just three FFTs aren't needed; this is a
1-line matrix op per sample:

```
mid  = (L + R) * 0.5
side = (L - R) * 0.5
```

This works WELL for content recorded with traditional stereo
miking. It fails for "modern" tracks where everything is
synthetic-panned and the "sides" channel is mostly silence.

### Route B: spectral mask separation (expensive, full)

Real source separation needs a neural network model — Demucs,
Spleeter, or Open-Unmix. These produce per-source stems (vocals,
drums, bass, other) by running the spectrogram through a learned
mask predictor.

CPU cost: ~5x realtime on a decent x86, ~1x realtime on aarch64
(Fairphone-class hardware). Memory: ~500 MB peak. Latency: would
need to chunk + cross-fade, so the controller would either
pre-separate full tracks at queue-add time (free at playback time
but lots of disk space) or do live separation (audible artifacts
at chunk boundaries unless cross-fade is generous).

The honest call: for a controller this is too expensive at
runtime. Pre-separating tracks on first add is the right shape.

## Recommended path

1. **Ship M/S processing now.** It's a 1-day implementation: add a
   `mid_side_split` boolean to the active scene/route, compute mid
   and side from the stereo input per frame, route them to the two
   speakers configured for the layout (Stage = mid, Smokey = sides
   would be a typical pair). This won't fully isolate the vocal
   but it WILL produce noticeably wider spatial spread than what
   per-band EQ achieves.

2. **Add a "separation depth" slider.** Internally, blend between
   "pure stereo" (no separation) and "pure M/S" by mixing:
   ```
   left_out  = (1 - depth) * L + depth * mid
   right_out = (1 - depth) * R + depth * side
   ```
   At depth=0 the listener hears regular stereo; at depth=1 they
   hear true M/S separation. Most content sounds best at
   depth=0.3-0.6. Slider gives the user control over the
   isolation level they're complaining about.

3. **Phase 2 (future): Demucs offline.** When the user adds a
   track they want better separation on, run Demucs on it once,
   cache the four stems under `~/.local/share/bo-controller/stems/`.
   At playback time, mix the stems live with per-speaker assignment.
   This gets you "vocals to speaker A, drums to speaker B" with no
   runtime CPU cost beyond what mixing already is.

   The Demucs setup is a shell-out pattern identical to the
   lyrics_transcriber's whisper-cli approach — same UX, same
   error surface, same model-path-not-found semantics.

## Why pure EQ "doesn't isolate"

To be explicit: a 4-band EQ can boost the 2-4 kHz band where vocals
sit, but the instruments in that band don't go away — they're just
quieter relative to the vocal. The vocal becomes more PROMINENT but
isn't ISOLATED. True isolation needs either:

- Source-of-truth separation upstream (different mic feeds, never
  the case for finished music), or
- Spectral mask separation downstream (neural net work, expensive)

The user's intuition that "the EQ works but separation doesn't" is
correct — it's separation theater, not separation. The fix is to
add a mechanism that ACTUALLY separates (M/S now, neural stems
later) rather than push the EQ harder.

## Quick test the user can run

Before any code change, the user can prove out whether M/S would
help on their typical content with a one-liner:

```bash
# Play the mid (centered content) through one BT speaker
ffmpeg -i song.opus -af "pan=mono|c0=0.5*FL+0.5*FR" -f wav - | \
    paplay --device=bluez_sink.AA_BB_CC_DD_EE_FF.a2dp_sink

# Play the side (panned-wide content) through another
ffmpeg -i song.opus -af "pan=mono|c0=0.5*FL-0.5*FR" -f wav - | \
    paplay --device=bluez_sink.00_11_22_33_44_55.a2dp_sink
```

If on tracks they care about, the mid speaker carries the lead
vocal and rhythm section while the side speaker carries texture
and reverb — that's the route to take. If the side speaker is
nearly silent, the content is heavily mono-summed and M/S won't
help — go straight to Demucs.
