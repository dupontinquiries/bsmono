# Why Billie Jean sounded huge: Stage spatial upmix vs plain stereo

You played a few tracks to the Beosound Stage over Bluetooth and one of
them (Michael Jackson — *Billie Jean*) opened up into a wide, surround-y,
"phenomenal" sound. This doc explains what that mode is, why it happened,
and how it differs from plain stereo — grounded in exactly what the logs
show bo-controller did.

## TL;DR

bo-controller did **not** create the surround effect. It sent the Stage a
clean, full-bandwidth **2.0 stereo** stream and the **Stage's own onboard
DSP upmixed it** into its spatial/surround profile. Our job in that chain
is narrow but important: hand the speaker the best possible stereo signal
so its processor has the most to work with. The track that "popped" did so
because *Billie Jean* is a famously wide, well-separated stereo mix — it
gives a spatial upmixer a lot of raw material.

## What the logs actually show

For every track, including Billie Jean, the playback line was identical in
shape:

```
bluetooth: play .../Billie Jean.opus → sink=bluez_output.50_1E_2D_5B_E6_EF.1 (needs_decode=true)
bluetooth: ffmpeg -ss 0 -i .../Billie Jean.opus -f wav - | paplay --device=bluez_output.50_1E_2D_5B_E6_EF.1
```

Three things to read out of that:

1. **`sink=bluez_output.50_1E_2D_5B_E6_EF.1`** — audio left the laptop over
   the standard A2DP Bluetooth sink for the Stage. A2DP carries **two
   channels**. Nothing in that link is "surround"; whatever multichannel
   you heard was created *inside the Stage*, after the Bluetooth hop.

2. **`-f wav`** — ffmpeg decoded the Opus file to plain **16-bit PCM
   stereo** and piped it to `paplay`. There is **no** `-ac` (channel
   count), **no** `-af`/`pan` (channel mixing), **no** upmix filter
   anywhere in the command. bo-controller does not touch the channel
   layout of music. It is a literal stereo passthrough.

3. **`needs_decode=true`** — because paplay can't play Opus directly, we
   decode to PCM first. The decode is full quality (Opus → 16-bit PCM),
   so the Stage receives the complete stereo waveform, not a re-compressed
   or downmixed version.

So the pipeline bo-controller ran was:

```
Opus file ──ffmpeg decode──▶ 2.0 PCM ──A2DP/Bluetooth──▶ Beosound Stage ──Stage DSP──▶ what your ears heard
                                                          └─────────── the magic happens here ───────────┘
```

The surround mode is the **last** box, and it's entirely the speaker's.

## What that "mode" is on the Stage

The Beosound Stage is a soundbar with multiple drivers and Bang &
Olufsen's spatial processing (marketed under names like *Beosonic* and the
soundbar's wide/surround listening modes). When it receives a 2.0 source,
its DSP can **upmix** — synthesize a wider, more enveloping field from the
two channels by:

- **Mid/side decomposition.** Anything panned to the center (lead vocal,
  kick, snare, bass) is the "mid"; anything that differs between L and R
  (reverb tails, hats, wide synths, doubled guitars) is the "side." The
  Stage can steer the mid to its center drivers and push the side content
  out to its edge/side-firing drivers, widening the image.
- **Decorrelation / delay** of the side content so it arrives slightly
  apart in time across drivers, which the ear reads as spaciousness.
- **Driver array geometry.** The Stage has physically separated drivers,
  so once the side energy is split out it actually radiates from different
  places, not a virtualized fake.

None of those steps require a multichannel input — they *derive* the
spatial field from stereo. That's why a 2.0 Bluetooth feed can come out
sounding like surround.

## Why *Billie Jean* specifically

A spatial upmixer is only as good as the stereo it's fed. *Billie Jean* is
a textbook "wide" mix: the iconic bassline and the four-on-the-floor kick
sit dead-center (strong mid), while the percussion, the doubled vocal
ad-libs, and the synth/reverb wash are spread hard across L/R (rich side
content). When the Stage pulls the side channel out to its edge drivers,
there's a lot there — so the effect is dramatic.

A mono-ish or narrowly-mixed track has little side content, so the same
upmix has less to expand and sounds closer to plain stereo. Same speaker,
same mode, same bo-controller pipeline — different *source width*. That's
the whole difference. Nothing changed in our code between tracks (the log
confirms identical play commands), so the variation you heard was the
music, not the app.

## Plain stereo vs the Stage's spatial mode — side by side

| | **Plain stereo (2.0)** | **Stage spatial upmix** |
|---|---|---|
| Channels in the signal | 2 (L, R) | 2 (L, R) — *same input* |
| Where the field is made | Mixing engineer, baked into the file | Speaker DSP, in real time |
| Drivers used | L/R pair | Center + side/edge drivers |
| Center content (vocal, bass, kick) | Phantom center between two speakers | Anchored on dedicated center drivers |
| Side content (reverb, wides, doubles) | Stays in the L/R pair | Steered out to side drivers, widened |
| Sounds best with | Any mix | Wide, well-separated mixes (e.g. Billie Jean) |
| bo-controller's involvement | none beyond clean stereo delivery | none beyond clean stereo delivery |

## How to get it again (and keep it)

The mode lives on the speaker, so reproducing it is about (a) keeping our
delivery clean and (b) selecting it on the Stage.

- **Delivery is already optimal.** Full-quality Opus → PCM stereo with no
  downmix or resample tricks is exactly what you want to feed a spatial
  processor. There's nothing to change in bo-controller to "enable" it.
- **The Stage remembers its listening mode per input.** The wide/surround
  profile is usually selected in the B&O app or by a preset on the Stage
  for the Bluetooth input. If a future session sounds flat, check that the
  Stage's Bluetooth input is still set to the spatial/wide mode rather
  than a "direct"/stereo mode — that's the most likely thing that changed,
  not anything on our side.
- **Reach for wide mixes** when you want to show it off. Records known for
  big stereo (a lot of '80s pop, modern pop with hard-panned production)
  will trigger the most obvious expansion.

## What bo-controller could add later (optional, not needed)

Today the app is hands-off on channel layout for music, which is the right
default — you never want to double-process audio that the Stage is about
to upmix. If we ever wanted an app-side option it would be a *labeled,
off-by-default* toggle, because it can fight the speaker's own DSP:

- A "**show the Stage what it's working with**" note in the UI that simply
  reports the source is 2.0 and the Stage may be upmixing — informational,
  no processing.
- For *non-spatial* speakers (plain BT headphones), an optional gentle
  stereo-widening or our existing `upmix.rs` (currently a soundscape-only
  module) could be offered — but that should never run when the sink is a
  Stage/soundbar that already spatializes, or you'd get a muddy
  double-widened result.

The safe rule, and the current behavior: **decode cleanly, send stereo,
let the speaker spatialize.** That's what made Billie Jean sound the way
it did.

---

*Reference: this writeup is reconstructed from a real playback session log.
The defining evidence is that every `bluetooth: play` line used `-f wav`
(stereo PCM) to the A2DP sink with no channel flags, so any surround was
necessarily produced downstream by the Beosound Stage, not by bo-controller.*
