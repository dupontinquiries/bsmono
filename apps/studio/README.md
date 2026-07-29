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
| `studio-collab` | convergent op log, presence, wire, carrier, **project files** | 25 |
| `studio-egui` | timeline, touch, theater/split, Helix keys, **spectrum canvas** | 28 |
| `studio-audio` | peak pyramids (Audacity's summary blocks) + mixdown | 8 |
| `studio-live` | pattern rack, synths (incl. beatspace), recording, transport | 18 |
| `studio-voice` | whisper transcript index, spoken repertoire, command palette | 9 |
| `studio-fx` | EQ, dynamics, transform, colour, retiming, analysis, saturation, multiband, **imager** | 40 |

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

## Saving: the file is the ops

kdenlive saves an MLT XML tree; Audacity saves an AUP3 SQLite database. Both
describe the project's *final state*, which makes the file format a second model
that has to be kept in step with the first — and drift between them is where
"this project opens wrong in the new version" comes from.

Studio does not need a second model. Every edit is already an `Op`, so a project
file is **the ops that build it**, and loading is replaying them through the same
`apply` the editor uses. A file therefore cannot describe something the editor
is unable to construct.

Three things fall out: the wire codec *is* the file codec (one encoding to keep
correct, already tested against truncation and version skew); a file is
replayable straight into a live session, so opening a project while
collaborating is the same path a joiner takes; and history is optional — keep it
and undo survives a restart, omit it and nothing is lost.

Two traps the tests pin. **Order matters**: sources before tracks before clips,
because a clip validates against its source and needs somewhere to live —
getting it wrong silently drops clips, the classic "half my project is missing".
And **locked tracks refuse clips**, so a snapshot that adds a track already
locked reloads empty; the snapshot adds tracks unlocked, places the clips, then
restores the flags, and the loader never has to know. A damaged or foreign file
is refused outright rather than half-read, because quietly loading an empty
project over someone's work is the failure that actually loses it.

## Helix keys, where they earn their place

Helix is **selection-first**: select a thing, then act on it (`w` then `d`),
where vim is verb-first (`dw`). In a text buffer that is a preference; on a
timeline it is simply how editing already works — you click a clip and then do
something to it. So the grammar maps on with almost no distortion, and brings
two things a timeline badly wants:

* **Multiple selections as a first-class idea.** `C` adds a cursor, so "trim the
  head of these nine clips" is one operation rather than nine drags. Both source
  editors make that awkward.
* **Motions that mean something.** In text `w` is a word; on a timeline the
  equivalent unit is the **edit point**, so `w`/`b` step cut to cut — exactly how
  you navigate a rough assembly — and `3w` skips three.

| key | timeline meaning |
|---|---|
| `w` `b` | next / previous cut (`3w` counts) |
| `e` | end of this clip |
| `g g` `g e` `g c` | timeline start / end / back to the playhead |
| `x` | select the clip under the playhead |
| `%` `;` | select everything / collapse to one |
| `C` `alt-C` | add the clip below / above — multi-clip editing |
| `d` `y` `p` | delete, yank, paste |
| `s` | **split** at the playhead |
| `u` `U` | undo / redo (already what `History` does) |
| `v` `i` `Esc` | select mode / text entry / back out |
| `space` `:` | play-pause / command palette |

**What is deliberately not copied:** regex selection, tree-sitter objects, and
`%`-as-whole-file have no timeline meaning, and inventing one produces keys
nobody can guess. `s` is the single departure — split, because every video
editor already binds it there. A key that surprises both audiences is worse than
one that surprises half.

Three traps the tests pin, all of them the ones modal editors actually hit: a
count must **not stick** to the next motion; a half-typed sequence must clear on
an unbound key rather than lingering (a pending prefix that survives a stray
press is how an editor feels haunted); and **insert mode must be literal**, so
naming a track "wd" cannot step a cut and delete a clip. Motions stop at the
ends instead of wrapping, because an editor that jumps from the last cut back to
the head on one extra press has lost you.

## Finding things by what was said

Transcript search, voice commands and the command palette look like three
features and are one: *type or say a phrase, get the thing you meant*. Built
separately they become three half-good search boxes.

**The insight that makes transcript search correct.** whisper.cpp transcribes a
**source file**; an editor searches a **timeline**. Those are different
coordinate spaces, and the difference is not cosmetic — a clip has an in-point
and a length, so most of a take is usually *not on the timeline at all*. A
search returning raw transcript offsets will happily send you to a line that was
trimmed out, and the same line appears twice if the source is used in two clips.
`TranscriptIndex::find` maps every hit through the clips that reference the
source: trimmed-away dialogue does not appear, and a line inside two clips
yields two results — one per place you could actually go. That is the whole
difference between a search that helps and one you stop trusting after the third
wrong jump.

**Speakers** are a first-class filter rather than a post-filter, because on a
two-hour multi-mic session that is the difference between a useful list and 400
hits. "Where does the second singer come in", "only the interviewer".

**Fast path**: the index is inverted (token → segments), built once, and a query
is a posting intersection rather than a scan — the palette searches on every
keystroke, and a palette that stutters is one nobody opens.

**The repertoire** is voiceterm's idea applied to editing: the useful unit is not
a command but the *sequence you keep repeating* — "split here, delete the left,
snap the rest up". Recording it once turns a nine-key ritual into three words.
Steps are ops, so a routine is undoable, shareable and replayable by the same
machinery as everything else; nothing about voice is special-cased downstream.
Phrase matching scores token overlap with a bonus for verbatim order and a
**floor beneath which nothing fires**, because running the *wrong* workflow on
someone's timeline is far worse than running none. Usage counts break ties, so
the palette gets better the more you edit.

## Designed, not yet built

The rest of the request, specified rather than hand-waved so it can be picked up
cleanly:

**Touch feedback in `uikit-egui`.** The pieces worth having are a spring-damper
`Anim<T>` (critically damped by default, so nothing overshoots into looking
cheap), a `Ripple` bound to press position, `Squish` on press with the release
carrying momentum, and a rubber-band overscroll — all frame-rate independent
(integrate by `dt`, never per-frame constants) and all honouring
`prefers-reduced-motion`. The reason to put them in the kit rather than in
Studio: every fleet app then inherits them, and touch feedback that differs per
app reads as breakage. Default-on for Studio via a `TouchFeel` in the shared
theme.

**Keyboard hinting for rusty-board.** The board already knows its layout and
cardinal variants; hinting means surfacing the *next likely keys* — from the
engine's own prediction — as a transient overlay on the keycaps, plus a
"what can I swipe from here" ring on long-press. The honest hard part is
choosing when NOT to hint, since a keyboard that redraws constantly is worse
than one that never helps.

**whisper.cpp integration.** `Segment` is already whisper's output shape
(start/end/text/speaker/confidence), so the binding is a decode-and-feed job:
run the model over a source once, cache the transcript beside the media, and
`TranscriptIndex::add` it. Deliberately left at the boundary because it needs a
model file and a machine, not because it is unclear.

## The plugins

Nothing in `studio-fx` is novel, and that is exactly why it is worth having: EQ,
dynamics, transform, colour and retiming are **settled**, with textbook-correct
behaviour that can be asserted instead of eyeballed. Each is a plain struct with
a `process` and no allocation in the hot path, so the same code runs in a render
and in a realtime callback. `instantiate` turns a project's stored
`Effect { kind, params }` into a running instance, or declines — so a project
made with a plugin you lack still opens.

**EQ** is RBJ-cookbook biquads in Direct Form I, which costs one extra state
variable and behaves markedly better when coefficients change under a moving
knob. Frequencies are clamped *before* the trig, because a user can drag a
slider past Nyquist and NaN coefficients emit a burst of noise. The curve drawn
in the UI is computed from the coefficients rather than measured, so the display
cannot disagree with the filter.

**Compressor**: dB-domain detection, separate attack and release, and a
quadratic soft knee that is continuous in *slope* as well as value — a slope
discontinuity is precisely what makes a compressor sound like it is grabbing. A
test walks the curve and refuses a slope jump.

**Limiter**: a compressor at infinite ratio with real lookahead. The first cut
had the classic bug — gain computed from the incoming sample but applied to the
delayed one, so a transient dropped the gain while an old quiet sample was
passing and it had released again by the time the loud one emerged. The test
caught it leaking 1.03 against a 0.91 ceiling. The gain now accounts for every
sample still inside the window, which is O(window) per sample; a monotonic deque
would make it O(1) and is the right optimisation later, but being audibly
correct first is worth more than being fast at letting samples through.

**Transform** stores normalised fractions rather than pixels, so a project
survives a resolution change, and exposes `invert` because what a renderer
actually needs is "for this output pixel, where do I sample?" — a test round-trips
both directions, since `apply` and `invert` disagreeing shifts the picture.

**Colour** applies lift → gamma → gain in that order (changing it silently
changes the look of every existing project) and desaturates on Rec.709 luma, not
a flat average — the classic error that turns skin tones grey-green.

**Retiming** is where naive implementations break, because the useful question
is not "how fast" but "given a timeline instant, which source instant do I
read". For a ramp that is the **integral** of the speed curve, not the speed:
ramping 1× → 3× over ten seconds consumes twenty seconds of source and the
halfway point sits at 7.5s, not 10s. Getting it wrong drifts picture against
sound across the ramp while both look individually correct.

## Learning from Ozone and Neutron

iZotope's Ozone 8 (mastering) and Neutron 3 (mixing) are the best-designed audio
interfaces of their generation and have no Linux build, which makes them a
reasonable thing to learn from. What is worth taking is **interaction design**,
and the DSP underneath is textbook — none of the numbers here are anyone's
secret. Their artwork, presets and branding are theirs and are not copied.

The structural idea worth stealing: **the spectrum is the canvas**. Every Ozone
module draws on one shared frequency axis, so an EQ curve, a multiband
crossover, a compressor threshold and the live signal are legible together
instead of requiring you to correlate four separate boxes. Everything else
follows from that — direct manipulation of nodes and crossovers *on* the
spectrum rather than knobs beside it, and a visible module chain you can
reorder.

`analysis` is the layer all of that is drawn from: an FFT (hand-rolled — thirty
lines, no dependency in a realtime meter's hot path, and the most testable
function in existence), calibrated spectra where a full-scale sine reads 0 dBFS
so the display can actually be trusted, third-octave band energies averaged in
the **power** domain (averaging dB under-reads a band with one strong partial,
which is exactly the case you care about), true peak by oversampling (a sample
peak of 0 dBFS can still reconstruct above full scale — the reason a "safe"
master clips on a consumer DAC), stereo correlation and width, and
**gain-matched A/B**, which is the single most important honesty feature in a
mastering tool because louder always sounds better and an unmatched comparison
lies every time.

**Masking** is Neutron's standout idea and the one that maps best onto Studio:
instances on different tracks compare spectra and show *where* two sources fight
— not "your vocal is buried" but "your vocal is buried between 300 and 800 Hz",
which is the difference between a meter you act on and one you ignore. Studio
already has the multi-track model and a channel between collaborators, so this
is the whole feature minus the drawing.

A test caught the definition being wrong on the first pass: a 9 kHz source was
reported as "masked" by a bass at 100 Hz, where the target has no content at
all. Masking requires something *of yours* to bury, so a band where the target
is absent now scores zero — the correct definition rather than a threshold tweak.

## Saturation: why these curves

Every soft-clipper sounds *fine*; the classics sound **good**, and the whole
difference is which harmonics they make — which is arithmetic, not mystique:

* A **symmetric** curve (`f(-x) = -f(x)`) can only make ODD harmonics — 3rd,
  5th. That is the console/transistor sound: present, a bit aggressive.
* An **asymmetric** curve also makes EVEN harmonics. The 2nd is an octave up,
  which the ear reads as warmth rather than distortion — the tube sound, and why
  a tube stage flatters a vocal where a clipper just makes it loud.

So the profiles differ in symmetry and knee shape, and the tests **measure the
harmonic signature with an FFT** instead of trusting the comment. That caught
two of my own claims being wrong: I had Tape in the even-harmonic group, but
real tape is 3rd-harmonic dominant with only a trace of 2nd — it is warm because
of its soft knee and compression, not because it is asymmetric. And Chebyshev
blends T2/T4 (even) with T3 (odd), so it belongs in both.

The classics — **Tube**, **Tape**, **Console**, **Diode** — are kept because
they sound good. The funky four never existed in hardware: **Wavefolder**
reflects instead of clipping so harmonics keep climbing with drive rather than
settling; **Crush** makes drive select bit depth, so it gets grainier instead of
louder; **Chebyshev** synthesises harmonics directly, so the knob literally
picks *which* harmonic to add; **Rectify** rings an octave up on transients.

### The performance decision

Distortion makes harmonics above the signal, and any past Nyquist fold back as
inharmonic tones — the metallic edge that makes cheap plugins sound cheap. The
fix is oversampling, which costs 4× the work. So it runs **only when it is doing
something**: below a drive floor the curve is near-linear, generates almost
nothing to fold, and the stage collapses to a multiply. A mastering chain with
eight modules where six sit near unity should not pay for eight oversamplers.

A DC blocker follows every curve, because asymmetric shaping adds offset that is
inaudible on its own and quietly eats headroom in everything downstream — the
classic reason a "warm" chain runs out of room. Crush and Chebyshev take drive
as a *parameter* rather than input gain; multiplying their input by eight first
buries the effect entirely, which is what the harmonics test caught.

## Visual direction: unendlich, not iZotope

The interaction grammar comes from Ozone and Neutron — spectrum as the shared
canvas (now built: `studio_egui::spectrum`), direct manipulation of nodes and crossovers on it, a visible reorderable
module chain, gain-matched A/B, masking between tracks. The *look* comes from
unendlich instead: atmospheric depth rather than flat panels — fog gradients
behind the spectrum, sun/ambient colour grading the whole surface, modules
sitting in a lit volume rather than boxed in chrome. Borrowing the grammar and
not the skin is also what keeps this a spiritual successor rather than a clone.

## The shared canvas, built

`spectrum` is the one structural idea from Ozone made real: an EQ curve, a
multiband crossover, a compressor threshold and the live signal all share **one
frequency axis**, drawn into one lit volume. Modules paint overlays into the
returned rect rather than owning boxes of their own — which is the whole point,
since you drag nodes and crossovers *on* the spectrum because they are already
there.

The look is unendlich's: a vertical fog gradient with the warmth low, so the
spectrum sits *in* something rather than on top of a flat panel, and the signal
is filled to the floor so it reads as a mass in that volume rather than a line
floating in it.

Four pieces of arithmetic are pure and tested, because an axis that is subtly
wrong is the bug you never quite see — you just stop trusting the display, which
defeats the entire point of putting everything on it:

* **Log frequency.** Linear is the classic mistake: it hands half the display to
  10–20 kHz, where almost nothing musical happens, and squashes the two octaves
  that carry a mix into a few pixels. A test asserts equal octaves occupy equal
  width.
* **Node grabbing in screen space, not frequency space.** Two nodes an octave
  apart are close on screen at 40 Hz and far at 10 kHz; picking by frequency
  distance grabs the wrong one exactly where nodes are most crowded.
* **Crossovers clamped in octaves.** They cannot pass each other — that produces
  bands of negative width and a multiband which silently drops audio — and the
  minimum gap is octaves rather than Hz, because 200 Hz is generous at 400 Hz
  and invisible at 12 kHz.
* **Column averaging in the power domain.** A pixel column spanning many bins up
  high would otherwise under-read whenever one bin is hot, which is precisely
  the partial you are looking at.

## Multiband: the property that decides whether it is any good

Split a signal into bands, do nothing to them, add them back — you must get what
you started with. It sounds obvious and most naive implementations fail it, and
the failure is a dip or bump at every crossover that no amount of knob-twiddling
fixes, because it is in the splitter rather than the settings.

**Linkwitz–Riley 4th order** is the standard answer, and building it turned up
two errors worth recording:

1. **I inverted the high band.** That is the rule for LR*2*, where the halves sit
   180° apart at the crossover — but LR4 is two cascaded Butterworth sections
   and brings them back *in phase*, so flipping produces a deep null exactly at
   the crossover. The reconstruction test found it instantly: −24.7 dB in,
   −42.9 dB out at 1 kHz.
2. **Every lower band shared one allpass filter's state.** With three or more
   bands, each band below a crossover needs the phase that crossover imposed, or
   the sum has a hole — so each needs its *own* allpass instance. Two- and
   three-band still passed; four-band came out 1.7 dB off, which is exactly the
   kind of small, plausible error that ships.

A third finding was in the **test**, not the code: measuring reconstruction with
a fast log sweep compares a window of output against the same window of input,
but the filters have group delay, so those windows hold different parts of the
sweep. A four-band chain read 1.8 dB "wrong" from measurement artefact alone.
Steady tones that have settled have no such ambiguity, and reconstruction now
holds within 0.5 dB across two, three and four bands.

The rest is assembly: per-band compressor, gain, mute and solo (solo beats mute
and silences everything not soloed — the rule every mixer shares and every
reimplementation inverts once), and `reductions_db()` for the per-band gain
meters an Ozone-style display draws under the spectrum. Bypassed bands still
pass through the splitter, so bypassing one band cannot change the crossover
behaviour of the others.

## The imager, and the invariant that makes it safe

Width is done by scaling the **side** signal (L−R) and leaving the **mid** (L+R)
alone. That one choice buys the property that makes a widener safe on a master:

> **The mono fold-down does not change, whatever you do to the width.**

Mono *is* the mid, and width never touches it — so you can widen the air without
the phone-speaker version of the mix quietly losing material, which is exactly
what delay- or phase-based widening does. A test asserts it sample by sample,
because it is the entire reason to build it this way rather than the easy way.

Per band, because the two moves every mastering engineer makes are opposite and
both standard: **narrow the bass** (below ~120 Hz, where a wide low end smears
the mix and wrecks a vinyl cut) and **widen the air** (above ~8 kHz, where it
costs nothing and buys space). One width knob cannot do both. It rides the same
Linkwitz–Riley splitter as the multiband, which is why unity width everywhere is
genuinely transparent rather than nearly.

The goniometer is oriented so mono draws a **vertical** line and out-of-phase a
horizontal one — what every hardware vectorscope does, and why an engineer can
read it at a glance. Per-band correlation completes the picture: not "this mix
has phase problems" but "your 200 Hz is out of phase", which is the difference
between a meter you act on and one you learn to ignore.
