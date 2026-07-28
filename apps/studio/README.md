# Studio

A video NLE and an audio editor sharing one Rust core — informed by kdenlive and
Audacity, but not a transliteration of either.

## Why not a 1:1 port

kdenlive is ~350 C++ files over MLT; Audacity is ~1,500 C++ files plus 354 QML.
Transliterating either would take far longer than it is worth and would inherit
the one limitation both share: **a project is single-player**. Neither can be
edited by two people at once, and neither can be, because in both the edit is a
method call on a model — there is nothing to send.

Studio keeps what those codebases got right and makes one structural change.

## The change: an edit is a value

kdenlive's timeline holds clips that reference bin clips with an in-point, a
length and an effect stack; Audacity holds `WaveTrack`s of block `Sequence`s so
an edit rewrites a few blocks instead of a file. Studio keeps both ideas — clips
reference sources, sources own the samples — and represents **every edit as a
serializable, invertible `Op`**. From that one decision:

* **Undo** is the inverse of the last op. No command classes.
* **Collaboration** is sending the op to a peer.
* **Macros/scripting** are lists of ops.
* **Tests** are a fold of ops over an empty project.

Three model rules make concurrent editing converge without operational
transforms:

1. Clips are addressed by **stable id**, never by index, so a remote op cannot
   be aimed at the wrong clip by someone else's insert.
2. Positions and lengths are **absolute values on an integer timebase**, not
   deltas, so two peers moving one clip land on one of the two *intended*
   positions rather than summing into a third nobody chose.
3. Track membership is a field on the clip, so moving between tracks is the
   same op as moving in time.

The timebase is integer ticks at 705,600,000/sec — chosen so 44.1k, 48k, 96k,
24, 25, 30, 50, 60 and NTSC 30000/1001 are all exact. Editors that store seconds
as `f64` drift until a cut lands half a sample early; integer ticks also make
edits exactly reversible, which floats are not.

## Crates

| crate | what it is | tests |
|---|---|---|
| `studio-core` | timebase, tracks, clips, sources, effects, `Op`, `History` | 6 |
| `studio-collab` | convergent op log (Lamport + site), per-peer presence | 6 |
| `studio-egui` | timeline on uikit-egui: ruler, tracks, clips, waveforms, presence overlay | 7 |
| `studio-audio` | peak pyramids (Audacity's summary blocks) + mixdown | 8 |

## Realtime collaboration, concretely

`Doc` is one replica. A local edit is stamped `(lamport, site)`, applied
optimistically so the UI never waits for the network, and returned as a packet
to broadcast. `integrate` is idempotent (duplicates are dropped) and
order-independent (a late op triggers a replay from the opened project). Where
two peers genuinely race, a **per-field last-writer-wins** rule picks the same
winner on every replica.

Last-writer-wins is wrong for text — it loses characters — but right here: the
units of edit are whole objects with independent fields, so if two editors drag
the same clip to different places there is no meaningful merge, and both peers
must simply agree. Edits to *different* fields (one trims, one rides the gain)
commute and both survive. What the model must never do is average two positions
into a third; carrying absolute values instead of deltas is what prevents it.

**Presence is deliberately not history.** Playheads and selections are worthless
a second later, must never replay into a project, and would bloat the log undo
walks. They ride a separate channel with a per-site sequence number so a late
packet cannot resurrect a stale playhead. `peers_on(clip)` answers "is someone
else already working here?" — preventing conflicts is better than resolving
them.

## Honest status

This is a **core**, not a shipping application. What exists is tested and real:
the model, undo, and convergence. What does not exist yet, in rough order:

1. **Media I/O** — decode/encode. The intended shape is `symphonia` for audio
   decode, `ffmpeg` via CLI for video, keeping the core codec-free.
2. **Audio render graph** — mix tracks, apply effects, produce samples. The
   `Effect` type is already opaque-by-design so hosts can register kinds.
3. **Transport and playback** — mixdown produces samples; nothing yet sends
   them to a device or moves a playhead in real time.
4. **Transport** — the op packets are plain serde; snapclone's `snapclone-dht`
   already carries signed records between peers and is the obvious carrier.
5. **Block-backed sources** — Audacity's insight that samples live in chunks so
   an edit touches a few blocks, not a file.

A test found the one real bug in this pass: `rebuild()` originally replayed the
op log from an empty project and erased every track and source that pre-dated
the session. A `Doc` now remembers the project it opened.

## The timeline view

`studio-egui` keeps its arithmetic — time↔pixel, clip rectangles, hit testing,
snapping — in plain functions with no `Ui` in sight, for the same reason
snapclone's trace policy is Qt-free: that is the part which is easy to get
subtly wrong, and it is only testable if nothing needs a window. The drawing
code is then a thin pass over those results.

Three behaviours are pinned by tests because they are the ones every
reimplementation gets wrong:

* **Zoom holds the frame under the cursor.** Every editor does this; almost
  every rewrite forgets it and zooms about the viewport's left edge instead.
* **Hit testing is row-aware.** The same x on a different track is a different
  clip — a row-agnostic test produces exactly the confusion you would expect.
* **Snapping ignores the clip being dragged.** Otherwise a clip snaps to itself
  and will not move at all, which reads as the editor being broken.

The presence overlay is why the whole thing exists: peer playheads are drawn
above the clips in a per-site colour that is stable across machines, and any
clip a peer is working in gets a warning edge. `peers_on` is track-aware —
being inside a clip's time span is necessary but not sufficient, because a peer
scrubbing the video track is not editing the audio clip under the same instant.
A peer who declares no active track still matches on time alone, so the warning
degrades to "someone is around here" rather than vanishing. (A test caught that:
it originally flagged both clips under one playhead.)

## The audio half

The idea worth taking from Audacity is not any of its UI: it is that a
`Sequence` carries **summary blocks** — min/max at 256 samples per point and
again at 65,536 — so drawing a ten-hour recording zoomed out reads a few
kilobytes instead of gigabytes. `studio-audio`'s `Peaks` is that in a few dozen
lines: a pyramid where each level is built from the one below (O(n), not
O(n log n)), and `for_px` picks the level whose buckets are closest to one
screen pixel.

Min **and** max are kept, never an average, because a waveform drawn from
averages loses exactly the transients an editor is hunting for — a click, a
clipped peak, a drum attack. A test plants one spike in a quiet file and
requires it to still be visible at the coarsest level and in a 40-column
zoomed-out draw. Another checks every bucket of every level against a
brute-force scan, because a fast summary that lies is worse than no summary.

The timeline draws waveforms from that pyramid at one column per pixel, so the
cost of drawing a clip is its **width, not its duration** — a ten-minute clip
and a ten-second one draw at the same speed. Sources with no pyramid yet render
as plain blocks: analysis must never gate editing.

`mixdown` is a pure function of (project, sources, window): it walks the clips
intersecting the window, applies clip gain and fades, and sums to interleaved
stereo. Same inputs, same samples, every time — no device, no callback, no
clock, which is what makes a render testable at all. Tests pin the things that
are easy to get backwards: half gain is 6 dB down, a muted track is silent, and
**solo on any track silences every track that is not soloed** (the rule every
DAW shares and every reimplementation inverts once). Silence reports
`-inf` dBFS rather than a magic `-100.0`, so a caller cannot mistake "very
quiet" for "nothing".
