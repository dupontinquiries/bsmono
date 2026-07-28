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
| `studio-collab` | convergent op log, presence, wire, **carrier adapter** | 19 |
| `studio-egui` | timeline on uikit-egui + touch gestures, theater/split modes, panels | 14 |
| `studio-audio` | peak pyramids (Audacity's summary blocks) + mixdown | 8 |
| `studio-live` | pattern rack, synths (incl. beatspace), recording, **transport** | 18 |

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
3. **Device I/O** — the transport clock and mixdown are done; nothing yet
   hands samples to a sound card or pulls them from a microphone.
4. **Publishing to a live DHT** — session keys, chunking and reassembly are
   done and tested against the awkward carrier shape; what remains is calling
   `snapclone-dht`'s put/get from a running node.
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

## The rack (the FL half)

Audacity's model is a tape recorder; FL Studio's is a step rack. An editor with
only one is annoying in a predictable way — you bounce a loop to audio just to
place it, or draw a hundred identical clips because there is no pattern to
repeat. Studio keeps the timeline as the arrangement and adds the rack as a
**generator**: a pattern placed on the timeline is a clip whose source is
synthesised rather than decoded, so collaboration, undo and mixdown do not care
where the samples came from.

Patterns are bitmasks per channel, not `Vec<bool>` — a rack is edited by
toggling single steps thousands of times a session, and a pattern that fits in
a word is trivially cheap to send to a collaborator.

### beatspace as a synth type

`apps/beatspace` is a mel-spectrogram GAN that walks a latent space and
crossfades generated beats; its player already exposes `next_sample()`, which
*is* the `Synth` interface. So it becomes a synth **kind** whose "note" is a
position in latent space: `SynthKind::Beatspace { z, crossfade }`. A pattern
step can move `z`, which turns latent drift into sequenced automation instead of
a live-only thumb gesture.

The model is **not linked here**. `SynthKind` is a description; a host with
beatspace loaded registers a renderer, and a host without one still opens, edits
and plays the project — the beatspace channel is simply silent. A test pins
that, because a project that fails to open because a plugin is missing is the
worst failure mode a DAW has.

## Recording, and why takes are ops

`Recorder` captures interleaved input while armed (audio before you hit record
can never leak into a take), tracks a peak for the meter with reset-on-read so
a transient is never missed between frames, and commits via **`commit_ops`** —
returning `AddSource` + `AddClip` rather than mutating.

That is the whole trick: a take reaches a collaborator's timeline by exactly the
mechanism a drag does. Recording is not special-cased in the sync layer, which
is why sharing it works at all.

## Phones, and the two ways to watch a session

Both source editors assume a mouse and a big screen. The useful touch set is
small and must be unambiguous, because a misread gesture in an editor destroys
work: one finger drags, two fingers pinch to zoom about their midpoint, two
fingers slide to pan, long-press is the context action (there is no right
button), double-tap zooms to fit.

Telling pinch from pan is the whole problem, since fingers never move purely.
The recognizer compares change-in-spread against change-in-midpoint, requires
one to dominate by 1.6× *and* the gesture to have moved more than 12px before
committing, then **holds that decision for the rest of the gesture** — which is
what stops a timeline lurching between zooming and panning mid-drag. Tests pin
all three: a clear pinch, a clear slide, jitter deciding nothing, and a
committed pinch surviving pure translation.

**Theater vs Split** is not cosmetic. In theater there is one program view and
one playhead — the session's, not yours — for a director and an editor looking
at the same cut. In split everyone keeps their own playhead and peers show as
markers, which is divide-and-conquer and where a long-form edit spends most of
its life. The mode decides whether your scrub is *broadcast*; getting that
backwards is the most annoying thing a collaborative editor can do, so it is a
mode and not a buried preference. Theater falls back to your own playhead if the
host goes quiet, because a frozen picture is worse than a slightly wrong one.

On a narrow screen the editor shows **one solid panel at a time** (Timeline,
Rack, Mixer, Program). Translucent floating panels over a timeline are
unreadable at six inches and impossible to hit accurately.

## The transport, and three bugs it is designed around

A playhead is easy to get approximately right and hard to get exactly right, and
the errors read as "it feels wrong" without users being able to say why.

**The clock counts samples, not seconds.** Advancing a float by `dt` each UI
frame accumulates error and ties musical time to frame rate — a dropped frame
moves the playhead. `advance(frames)` takes what the device actually consumed,
so the playhead is wherever the samples say it is. Video follows audio, never
the reverse. A test runs 10,000 awkward-sized buffers and demands the position
be exact.

**Recorded audio arrives late.** Input passes through the device buffer before
your callback sees it, so a take stamped at the current playhead lands late by
exactly the round-trip latency. The symptom is that everything you record drifts
behind the click by a consistent few milliseconds — invisible until you layer
two takes, and then maddening. `record_position()` subtracts it (clamped at zero
so a large configured latency near the project start cannot go negative). This
is the single most common bug in home-grown recorders.

**Loops must not drop samples at the seam.** When the playhead crosses the loop
end mid-buffer, the remainder belongs to the *start* of the loop. `Advance`
reports the split and `spans()` hands the renderer both halves, because a
renderer that assumes one contiguous span produces the click every naive looper
has.

A **count-in** rolls the clock back so the count happens before the punch point,
keeps the recorder disarmed while it runs, and reports the exact buffer where it
arms. The punch frame is stored rather than recomputed, so changing the tempo
mid-count cannot move the target.

### Following a shared transport

`Sync::Follow` carries the host's position *and* the round-trip time, because
the packet describes where the host **was** a hop ago — a naive follower is
permanently behind. Adding half the RTT puts you where it is now. And a follower
only re-seeks past a threshold: correcting every packet stutters the audio,
never correcting lets drift accumulate, so small offsets are left to the local
clock and only a real scrub causes a jump. This is the transport-level half of
theater mode, kept out of the UI because a clock is not a UI concern.

## The wire

`Doc` converges given the ops; `wire` is how a peer gets them. The transport
underneath is deliberately unspecified — a DHT, a socket, a file and a websocket
are all "somewhere bytes go".

The codec is hand-rolled rather than a general format, because an op is small,
fixed-shape and enormously frequent (every nudge of a clip is one) and a general
encoding would spend more bytes on field names than on the edit. What that buys,
beyond size:

* **A version byte**, so a peer on an older build refuses the packet instead of
  silently misreading a struct that gained a field. Unknown op kinds are refused
  the same way.
* **Truncation is loud.** Every read is checked, so a short packet returns
  `None` rather than a half-decoded op. A test feeds every prefix of a valid
  packet and requires all of them to fail — an edit applied from a corrupt
  packet would diverge one replica silently, which is the one failure this
  design exists to prevent.
* **A stream stays aligned.** `drain` pulls whole packets and leaves the partial
  tail for the next read, and a packet that fails to decode is *skipped* rather
  than fatal: the frame length is still trustworthy, so one bad packet must not
  cost the session every edit after it.

Joining needs no coordination. A newcomer sends `Hello` with the highest Lamport
clock it has (0 when cold); the peer answers with every op stamped after that.
Because `integrate` is idempotent and order-independent, the joiner can be
receiving live edits *while* the catch-up is still arriving — a test does
exactly that, then replays the entire history again and requires nothing to
change.

## Surviving a real carrier

`wire` says what a packet looks like; `carrier` says how it survives something
that actually moves bytes. `snapclone-dht` is the intended one, and its shape is
the awkward case worth designing against: records have a **size cap**, delivery
is **unordered and lossy**, and storage is **keyed rather than streamed** — there
is no connection to carry framing.

A single clip nudge fits a record easily. The catch-up that brings a joiner up
to date does not: a few hundred ops is tens of kilobytes against a DHT value
measured in low kilobytes. So packets are **chunked**, with enough header to
rebuild them, and a packet that already fits still becomes one chunk so the
receiving path has one shape to handle instead of two.

The `Reassembler` tolerates duplicates and reordering because the carrier
produces both as normal behaviour, not as edge cases. The subtle part is that a
**duplicate must not be counted twice** — otherwise a message "completes" while
still missing a piece, hands up a truncated packet, `Packet::decode` rejects it,
and the edit is lost for good. A test feeds the same first chunk three times and
requires that only the genuinely last piece completes the message.

What this deliberately does **not** do is retransmission or ordering. `Doc` is
already idempotent and order-independent, and a lost chunk is recovered by the
joiner asking again — building a reliability layer on top of a CRDT is solving a
problem the CRDT already solved.

Session keys are derived from the session name by the same FNV-1a fold
snapclone uses for sc-IDs, so every participant computes the same key without
being told it, and a short name still fills all 20 bytes instead of clustering
near zero.
