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
| `studio-core` | timebase, tracks, clips, sources, effects, **automation lanes**, `Op`, `History` | 39 |
| `studio-collab` | convergent op log, presence, wire, carrier, project files, assets, sessions, **DHT publish/discover** | 63 |
| `studio-egui` | timeline, touch, theater/split, Helix keys, **spectrum canvas** | 93 |
| `studio-audio` | peak pyramids (Audacity's summary blocks) + mixdown (tracks, clip/bus effects, sends) + **streaming render** + **block-backed edit sequences** + **destructive editing** | 83 |
| `studio-live` | pattern rack, **piano roll**, synths (incl. beatspace), recording, transport | 69 |
| `studio-voice` | whisper transcript index, spoken repertoire, command palette | 12 |
| `studio-fx` | EQ, dynamics, transform, colour, retiming, analysis, saturation, multiband, imager, chain, **assistant** | 105 |
| `studio-media` | audio decode (Symphonia: WAV/FLAC/Vorbis/MP3/AAC) into `SourceAudio` | 9 |
| `studio-video` | video probing, thumbnails, audio-track extraction (real `ffmpeg`/`ffprobe`) | 11 |

**Toolchain note:** egui/eframe 0.34 declare `rust-version = 1.92`, so the
GUI crates need rustc ≥ 1.92 (the offline `rustup.zip` toolchain pack, 1.96,
works). On an older rustc, `cargo build --ignore-rust-version` compiles them
anyway — the declared MSRV is policy, not a used language feature — but a
real 1.92+ is the supported path. With that in place every row above,
including `studio-egui` and `studio-app`, builds and its tests run directly.

`studio-egui` DID get re-verified this pass, for the first time — a real
rustc 1.96 toolchain cleared egui 0.34's MSRV, which no prior session here
could do. Its count above (93, up from 28) is a real `cargo test` result,
not carried over; every row in this table now reflects an actual run.
`studio-app` (the eframe binary crate wiring everything together) also
compiles clean — `cargo check -p studio-app` — confirmed for the first
time too. Both had zero errors; the only warnings are pre-existing
`egui::Panel`/`TopBottomPanel` API-rename deprecations (0.34 renamed
`.show()` to `.show_inside()` and split `TopBottomPanel` into
`Panel::top`/`Panel::bottom`), not bugs, not touched here.

**The rack is now an application, not just a model.** `studio-app` renders
the Rack panel: the step grid (paint on/off, per the grid module's rules), a
per-step modifier lane (velocity, hat-roll ratchet, retune, 808 slide,
probability — `studio-egui::lanes`), channel strip, BPM and swing, and a
bounce button that lands the pattern on the timeline as an ordinary
undoable/shareable `AddSource`+`AddClip`. Space on the Rack panel drives the
loop; on the Timeline it stays the tape transport. Audition playback loops
the rendered pattern through cpal (`studio-app::audition`, `audio` feature,
default on) and degrades to a silent wall-clock playhead when no device
exists — the status line says which sink you got. `studio-live` grew the
trap half the rack existed for: synthesized `Drum` voices (kick, snare,
clap, closed/open hats — deterministic, zero assets), `Tempo::swing`
(placement-time, off-16ths only), `Rack::trap_kit()` as the demo groove, and
rate-correct `Sampler` playback (linear resample when source rate ≠ render
rate). `.opus` decodes natively via `studio-media`'s `opus` feature (ogg +
libopus, default on) — stereo preserved, pre-skip and final-granule
trimmed so loops seam cleanly — and files dropped on the window import as
rack sampler channels; the project stores the file's own path (and, through
collab assets, its compressed bytes), never inflated PCM. **The timeline plays.** The transport now renders the arrangement through
the tested `mixdown` (whole-project, offline, re-rendered on any edit or
undo) and drives it out the same sink audition uses — generalized to
interleaved one-shot/loop buffers with seek. One sink, one owner: starting
the rack loop stops the tape and vice versa; a finished arrangement stops
the transport and says so. This is a deliberate trade against
`studio-engine`'s streaming path: at beat-project sizes a full re-render per
edit is cheap and cannot glitch; `DeviceEngine` remains the named path for
when projects outgrow it. Channels also gained **choke groups** (`Channel::
choke_group`, wire v6): a group hit cuts the group's ringing voice with a
~2ms fade; same-sample hits layer; the kit's hats ship in group 1. Drops are
panel-routed: a file dropped on the Rack becomes a sampler channel, dropped
on the Timeline it becomes a clip at the playhead on the first audio track
*with room* — a melody loop is arrangement material, not a truncated
one-shot. **Beats leave the building.** `studio-media` gained the encode half of its
Opus support — Ogg Opus out (RFC 7845), pre-skip declared from the
encoder's real lookahead, final granule marking the true end, and the
encoder over-fed by one lookahead of silence so a frame-aligned input
doesn't leave its last 312 samples inside libopus (a bug the unaligned test
case couldn't see and the aligned one caught; both alignments are pinned
now). Exports roundtrip sample-exact through this crate's own decoder and
read correctly in ffprobe. The shell exports the arrangement as 16-bit
dithered WAV or 128k Opus to `./exports/<project>-<stamp>.*` — no dialog,
the status line prints the path — and a bounce now writes its WAV to the
path its `AddSource` op records, so the project's references are real
files. **The Mixer is a panel now, and the model paid its tax first.** `Track`
gained `gain` (`Op::SetTrackGain`, wire v7, refused on a locked track,
serde-defaulted to UNITY so a pre-fader project cannot load silent) and
`mixdown` multiplies it with each clip's own gain — pinned at the usual
half-gain-is-6dB. The panel: fader per track committing ONE op on drag
release (a value per frame would be sixty undo entries and sixty packets
per gesture — `Paint`'s rationale, one layer up; the drag preview is visual
only, audio follows on release, and a live fader ride is honestly the
streaming engine's job), mute as the existing `SetTrackFlags` op, and solo
as deliberately-local monitoring state that shapes renders and exports but
never the project. **The shell is a peer now.** `Studio::edit` goes through the Doc's op log
(`Doc::local` — stamped, replay-safe) instead of mutating the project
directly; the old `History` wiring bypassed the log, which would have LOST
every local edit on the first remote-conflict replay. Undo/redo are inverse
ops routed back through the same door, so an undo travels to peers as the
edit it is — and a refusal after remote conflict is said out loud. Local
activity (ops and whole-rack LWW syncs, published from one per-frame choke
point) lands in `Studio::drain_outbox()`; `Studio::integrate_packet()`
accepts everything a peer sends, answers `Hello` with a log catch-up, and
adopts the LWW winner's rack. Six two-peer tests exercise the seam headless:
groove convergence both directions, concurrent project edits without a
referee, undo-as-op, flaky-link catch-up, snapshot join (the documented
broadcast()+rack_broadcast() flow — NOT snapshot+log, whose double-apply bug
broadcast()'s own docs record), and simultaneous-edit LWW agreement. **Media follows its op.** The wire gained the asset quartet (v8):
`AssetRequest`/`AssetManifest`/`ChunkRequest`/`Chunk`, built on the assets
module's content-defined chunking — an `AddSource` travels light and the
peer pulls exactly the chunks its content-addressed store lacks, so a
shared sample or a re-bounce that changed one bar transfers one bar (or
nothing — pinned by test). Chunks are verified by their id on arrival; a
corrupt chunk is dropped, never assembled into a take that is subtly wrong
at the mix. Bounces register their WAV bytes (one encode feeds both the
file the op's path promises and the chunk store); imports register the
COMPRESSED original — a .opus stays .opus on the wire, which is the whole
size story. The flagship test is the point end to end: A bounces, and B's
own `mixdown` renders A's kick. **The wire is real.** `studio-collab::stream` is a TCP carrier: two
threads per connection over `Packet::frame`/`drain`, mpsc facing the shell
so the `Studio` stays single-threaded around the Doc, and dumb on purpose —
it never reads a packet. Every thread exit path shuts the socket down (an
abandoned socket leaves the far side blocked in `read()` forever, which is
exactly how the dropped-peer test first failed). The shell opens it from
`STUDIO_LISTEN=addr:port` / `STUDIO_CONNECT=addr:port` and pumps it per
frame; on connect it sends `Hello { since: 0 }` EXPLICITLY rather than
`Doc::hello()` — that helper reports our own max lamport, which under-asks
on first contact between peers who both edited offline and would silently
lose their concurrent ops; the seen-set makes over-fetching free. The
flagship now runs over 127.0.0.1: A bounces, and B — connected only by
TCP — renders A's kick, holds A's groove, and the projects converge. One
peer, no reconnect, no discovery, no encryption: stated in the module docs;
the dht network half stays the named future. **Beats survive restarts.** The collab file format grew a fourth,
MANDATORY rack section (length 0 = no rack; a file cut at the boundary
refuses like any other prefix — the tempting "tolerate a missing tail"
reading was a truncation hole the damaged-file test's every-prefix sweep
caught, since a truncated file loaded rack-less would overwrite a complete
original on the next save). `Studio::save_to`/`load_from` write and open
whole sessions: project, history, rack; media stays as path references
resolved RELATIVE TO THE FILE, so a project folder moves as a unit, and
reloaded sources arrive through `incoming_audio` — the same door peer
media uses. A missing sample is counted out loud and its clip stays
silent; garbage refuses to open rather than becoming an empty project.
The shell opens `STUDIO_PROJECT` (default `./project.studio`) at startup
— a load FAILURE starts a demo but warns that Ctrl+S will overwrite —
and saves on Ctrl+S or the save button. **Clips answer the mouse.** The timeline's `hit_test`/`snap` helpers were
built and never wired: clicks were thrown away by the shell and no drag
path existed. `timeline` now returns a `TimelineResponse` — click (the
shell selects; shift toggles) and drop (`MoveClip`, an op like any other:
undoable, on the wire, and refusable out loud when `check_room` says no).
The drag itself lives on the `View` as transient state, draws a ghost, and
never touches the project; `move_target` is a pure, tested function that
snaps, clamps at zero, and refuses to land audio on a video row (a
cross-kind vertical move is a re-route the mixer cannot honour — refused
at the gesture, not the op). The rack grew pattern LENGTH (1–64 steps, the
rows' u64; shortening hides bits without eating them, so
shorten-then-lengthen round-trips) and DUPLICATE (variations start from
the groove, not silence). **The timeline scrolls, zooms, and trims.** The wheel pans (both axes —
a timeline is a horizontal thing), ctrl-wheel/pinch zooms about the
pointer, both hover-gated; panning clamps at zero. Edge grabs trim (7px
grips), body grabs move; `grab` classifies purely, `trim_target` clamps to
a 10ms sliver, the source's material, and — for a LEFT trim — moves start
and in-point together so audio never smears in time, revealing earlier
material only as far as the source actually has it. An ambiguous grab on a
tiny clip goes to the preferred side, and `alt-;` (`FlipTrimSide` — a key
the map minted before the drag existed) finally flips it. A left trim
lands as its two true facts, TrimClip then MoveClip: two ops, two undos,
the precedent a bounce set. The Rack panel scrolls on both axes (a 64-step pattern is ~1.5k px of
grid, and the stated targets include phone-sized viewports) and the Mixer
scrolls vertically. Also still named: streaming pattern playback inside
`DeviceEngine`'s realtime render, group-undo for multi-op gestures (a
left-trim is TrimClip+MoveClip — two undo presses for one gesture; the
core has `History::apply_group`, the shell's op-log undo has no grouping
yet), and the Program panel, which says so on screen instead of showing
nothing.

**Toward the full studio shape.** mixdown had the sampler's old chipmunk
bug hiding one layer up — sources were indexed at the RENDER rate, so a
44.1k file on the timeline played ~9% sharp; fixed with the same
fractional-position interpolation, pinned by a 24k-ramp test, which is
what makes render rates a real parameter: WAV exports at 44.1/48/96/192
(a 96k export is the same music at twice the frames, tested), Opus stays
48k by codec contract. The Mixer shows the BUS half the model always had:
create busses, route tracks (SetRoute), per-bus gain/mute, and effect
chains built from the studio-fx registry (saturation/eq/compressor/
limiter/color) — the track→saturated-drum-bus→master shape is one test,
including its four-op undo. A library sidebar (STUDIO_LIBRARY, default
./library; two-level scan, audio-only, sorted) offers every sample the
two doors drops already had: rack channel or timeline clip. Tape stop and halftime are native effects now
(`studio-fx::playrate`) and addable from any bus chain. **The keyboard plays.** The sink grew additive one-shot VOICES (an SPSC
channel the audio callback drains, so triggering a note never locks and
never disturbs the loop it mixes over, with an oldest-drops cap so a stuck
key cannot grow unbounded work per buffer) — which is what both playing
mode and the library's ▶ preview needed. `shared/analog-kit` is the
Wooting Analog SDK behind a plain value: dlopen by documented name, HID
usage → depth, digital fallback with no cfg at the call site, and a
MUSICAL velocity curve rather than skydribble's steering curve (a high
floor is right for turning a car and wrong for playing quietly). It is
factored from `games/skydribble/src/analog.rs`, which proved the approach;
that game is deliberately NOT refactored onto it in the same breath.
studio-app supplies it as the real backend for `studio_egui::playkeys`'s
`AnalogSource` seam. **Controllers play it.** `studio-app::midi` splits at the line that
matters: a PURE parser (running status, note-on-velocity-0-is-note-off,
14-bit bend, truncated packets dropped — all tested with nothing plugged
in) under a feature-gated `midir` connection, so a build without ALSA
still compiles and reports no ports. A controller note becomes the same
`playkeys::PlayNote` the computer keyboard makes, so there is ONE note
path: voiced through the sink now, and landed in the pattern when the
transport is recording — quantised to the NEAREST step, both because a
note struck a hair early belongs on the beat and because Transport stores
position in frames, so an exact boundary reads back a fraction below it
(flooring recorded a whole step early; a test caught it). CC and pitch
bend are parsed and deliberately unrouted until there is a learn-and-bind
UI: a mod wheel that silently moved an unrelated fader is worse than one
that does nothing. MIDI clock is not parsed at all — syncing to an
external clock means resampling against drifting wall time, which belongs
with the streaming engine, not a 0xF8 match arm. Named, not built, from
the same wishlist: batch multi-clip editing,
clip-level chain editing,  
 a playing
mode for keys — now built, see below — Wooting analog input, and VST/CLAP
hosting — studio-fx::retune is the native autotune seed; for external
FOSS instruments (Surge XT, Dexed, sfizz + Salamander for piano) a plugin
host is its own project.

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

1. **Media I/O** — both halves exist now. `studio-media` wraps Symphonia
   (WAV/FLAC/Vorbis/MP3/AAC, pure Rust, no system codec libs) behind
   `decode_file`/`decode_bytes`, returning the same `SourceAudio` a
   recorded take or a bounced pattern already produces. 9 tests, including
   de-interleaving, a 200,000-sample file for the packet-loop/EOF handling,
   and `decode_bytes` (audio already in memory, no file — the seam
   `studio-video` needs) matching `decode_file` exactly. `studio-video`
   shells to real `ffmpeg`/`ffprobe` (confirmed present in this sandbox
   before writing a line of it — an assumption from earlier in this same
   session, that video decode needed tooling this sandbox doesn't have,
   turned out wrong the same way the egui assumption a few commits back
   did): `probe` for duration/resolution/fps/audio-track metadata,
   `thumbnail_png` for a single frame's raw PNG bytes, `extract_audio`
   piping a video's audio track straight into `decode_bytes` with no temp
   file. 8 tests, all against real subprocesses and ffmpeg-generated
   fixtures, not fakes. *Since this was written* the picture side landed
   too (the film merge and after): `frame_rgb` decodes frames for the
   viewport, which composites every video layer with its grade, style,
   kaleidoscope, transform, blend and fade; and `studio_video::edit`
   RENDERS the timeline to a file -- cuts, black gaps, the same picture
   effects, fades to and from black, the mixdown underneath -- with the
   best H.264 encoder the machine has (libx264, else libopenh264 as on
   Fedora, else mpeg4). Overlapping video tracks export as the preview
   composites them -- blend modes, opacity, and a reframed layer showing
   the track under it -- with each mode's pixels checked against the
   compositor. What the export still cannot do is named per cut by
   `edit::unsupported` rather than dropped: retiming. Rotation renders,
   about the transform's anchor, checked pixel-for-pixel against the
   compositor (which turned the wrong way until c20). A
   clip's fade is an opacity ramp, so a fade over a clip on a lower track
   is a cross-dissolve into it (and over nothing, a fade to black) -- in
   the preview and the export alike.
2. **Audio render graph** — `studio_audio::mixdown` mixes tracks, applies
   clip and bus effect chains, handles sends/mute/solo, and is real and
   tested (72 tests) — `studio-live`'s own
   `a_bounced_pattern_is_audible_through_a_real_mixdown` proves a pattern
   bounce survives the whole path. The gap this used to describe (a fresh
   effect-instance set built on every call, correct offline, wrong once per
   device buffer) is closed too: `StreamingRenderer` wraps the same mixing
   logic — `mixdown` itself didn't change, it's now a two-line wrapper
   around the shared `mixdown_with_cache` — around an `EffectCache` that
   persists instances across contiguous calls and resets automatically on a
   seek. Checked directly: consecutive buffer-sized calls match a one-shot
   offline render sample-for-sample; a chain edited mid-playback (this is a
   collaborative editor, so that's a real case, not a hypothetical) takes
   effect on the very next contiguous call.
3. **Device I/O** — `studio-io`'s `device` feature now builds a full-duplex
   `CpalIo`: a dedicated thread owns the (`!Send`) cpal streams, an `rtrb`
   ring buffer bridges input to output since the two have no shared clock,
   and config negotiation picks a real device config instead of ignoring
   the request. Compiled and unit-tested (17 tests, including the
   negotiation fallback logic against hand-built device configs) — **not**
   yet run against real hardware, since there is still no sound card in the
   authoring sandbox. That verification is the next step, not a redesign.
4. **Publishing to a live DHT** — `studio-collab::dht` now calls
   `snapcore::store::Store`'s put/get directly: `publish` chunks a `Packet`
   under the session key, `discover` reassembles whatever's complete.
   Genuinely tested end-to-end (9 tests, including a full `Session::broadcast`
   → publish → discover → joiner round trip) — what's still missing is the
   network half, iteratively storing to and reading from the nodes actually
   closest to a key, which is `share-kit`'s own still-pending "networked
   Backend." This module is the local half of that seam, done; not a
   placeholder for it.
5. **Block-backed sources** — `studio_audio::block::BlockSequence` now
   exists: one channel as a sequence of variable-length blocks behind
   `Arc<[f32]>`, so `insert`/`delete` rebuild only the block(s) straddling
   the edit and leave every other block the same allocation, not just the
   same bytes — checked directly via pointer identity, not value equality,
   across 18 tests. Kept separate from `SourceAudio` (mixdown/`Peaks`'s own
   type) rather than rewriting it: block-backing pays for itself on the
   *edit* path, not playback, so `to_flat_channels`/`from_flat_channels`
   bridge the two rather than forking the representation everywhere else
   that already works with flat audio. `BlockSequence` now has a real
   caller too: `studio_audio::destructive` (`delete_range_ops`/
   `insert_at_ops`) cuts a range out of a source or punches material into
   one, and hands back the same `Op::AddSource`/`Op::AddClip` shape
   `repitch`/`bounce_ops`/`Recorder::commit_ops` already use — a
   destructive edit never mutates the source it started from, so it's
   non-destructive at the model level and needs no new convergence rule.
   Checked directly, not just argued: two concurrent destructive edits of
   the same recording, applied in either order, converge to byte-identical
   projects using nothing but the pre-existing op semantics.

This closes every item that was on this list when this section was last
written honestly. What's real and load-bearing now, named precisely rather
than left as a vague "not built yet": real hardware verification for
`studio-io` (item 3), the network half of DHT publishing (item 4), and
for video (item 1) retiming in the export and realtime playback in the
viewport, rather than a seek per frame. None of those are guesses — each is
a concrete next step pinned by a test or a doc comment that says exactly
what it's waiting on. Item 2's own remaining piece — a streaming render
path — is no longer on this list; it's built.

One more thing this list never named because items 2 and 3 were written
before either existed: `studio-engine` now has `device::DeviceEngine`,
wiring its own `EngineState`/`Command` ring to `StreamingRenderer` and
`studio_io::AudioIo` into an actual callback — proven end-to-end against a
real `AudioIo` implementor (`TestIo`, standing in for `CpalIo` since there's
still no hardware in this sandbox), not just called directly. This crate
needed no UI toolchain to build that: it has never depended on egui.
The render path is allocation-free on its warm path too now:
`StreamingRenderer::render_into` writes into a caller-provided buffer and
reuses a `RenderScratch` (the master accumulator and every bus's buffer,
resized in place rather than rebuilt) across calls, and `DeviceEngine::
tick` calls it instead of the `Vec`-returning `render`. Checked directly —
a bus scratch buffer's `.capacity()` is unchanged across two same-size
contiguous calls, not just assumed stable. What's still real and named
rather than glossed over: `EffectCache`/`RenderScratch` growing on a
structural edit (a new bus, a clip's first effect) still allocates, which
is the right trade for something that happens on an edit rather than every
buffer, and `Recorder::feed`'s buffer (built before this session) still
reallocates past its two-second initial capacity on a longer take.
`Transport::advance` now also reports exactly where within a buffer a
count-in's punch point lands (`Advance::punch_offset`), and `DeviceEngine`
uses it to capture only the post-punch samples of a straddling buffer
instead of the whole thing — the count-in's own silence no longer leaks
into the start of a take.

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

**A picture above the tracks.** The frame-under-the-playhead preview
(`studio-video::frame_rgb`, topmost video track, half-second cache) existed
only as the workbench's side-slot viewport — reachable from a different tab,
not from the Timeline itself, so editing had no monitor. A `preview` toggle on
the bar (default on) now opens the same viewport in a resizable panel above
the tracks, so scrubbing the timeline shows what plays without leaving the
editing surface.

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

### Placing a pattern, and staying live

A pattern reaches the timeline three ways now — the rack's own bounce button,
⇥ in the library, and dragging a pattern row onto a track — and all three go
through one path, `Studio::place_pattern(index, at, track, …)`: render at the
rack tempo, land as the same `AddSource`+`AddClip` a bounce or a recorded take
produces (undoable, and shareable by the collaboration mechanism those already
use), refusing a non-audio track or an overlapping drop *before* the source
lands rather than leaving an orphan.

The drop itself is `studio-egui`'s job, not the host's: `pattern_drop_target`
snaps to the same edges a moved clip snaps to, and routes a drop over a video
row to the nearest audio track — a pattern is sound, the same refusal a
cross-kind clip move already makes.

What makes this more than a bounce: `Studio::pattern_links` keeps every placed
source linked to its pattern **by name** (indices move when patterns are
deleted), and an edit to the rack re-renders every linked source and re-mixes
the arrangement through the same tested mixdown the export uses — so the
timeline playhead passing through a placed pattern hears it as it is *now*,
through the mixer, not as a frozen bounce from when it was dropped. Deleting
the source pattern unlinks and keeps the last render rather than going silent
or stale without saying so. Links are not persisted — a reopened project
plays the frozen take until the pattern is placed again, which is a
deliberate simplification, not an oversight (linking on reload needs the
pattern's *content*, not just its name, to be a safe match).

## Recording, and why takes are ops

`Recorder` captures interleaved input while armed (audio before you hit record
can never leak into a take), tracks a peak for the meter with reset-on-read so
a transient is never missed between frames, and commits via **`commit_ops`** —
returning `AddSource` + `AddClip` rather than mutating.

That is the whole trick: a take reaches a collaborator's timeline by exactly the
mechanism a drag does. Recording is not special-cased in the sync layer, which
is why sharing it works at all.

## The second take

One take lands where the playhead was; that has worked since takes were
ops. The second take of the same phrase landed on the first, and the
overlap policy either refused it, trimmed the take you might have
wanted, or moved it to the one shadow lane where the third collided
again. A vocal session is eight takes of the same two bars.

`studio_core`'s takes module: a take goes to the first lane in the
track's CHAIN with room — the track, then `vox ↓`, then `vox ↓ ↓`, each
a shadow lane of the one before (room.rs's own convention applied to
itself), created only when every existing lane over that span is
taken. Eight takes of two bars are eight lanes; a take of different
bars goes back on the parent. A lane is a track, a take is a clip, the
chain is a fact about names; nothing new travels on the wire. The lane
and its clip land as one undo step. With a lane's clip selected, the
take target is the chain's ROOT, so re-taking the phrase you are
looking at stacks under the same track, never under the lane.

**Loop-record.** "loop the selected clips' span" gives the transport a
range; the playback block renders the range ALONE and sets it looped,
so the wrap happens in the callback, sample exact — the singer hears
the bar come round on time, so what she sings lands on time — and the
sink's counter maps back into the timeline by `start + (frame mod
len)`. Recording inside it: each time the playhead comes round (moves
backwards by more than a jitter — `looping::Passes`, tested with
numbers), the microphone's audio so far is cut (`Capture::split_take`,
the buffer swapped out without stopping the stream) and placed at the
loop's start on the next free lane, trimmed to the loop's length. The
pass boundary is where the UI saw the wrap, so a frame's worth of the
next pass sits past the loop's end on this one and is trimmed off;
not corrected further, because a sample-exact split needs the capture
and the sink on one clock, and they are two devices.

**Comping.** The root track is the comp; each lane is an alternative.
"comp: use the selected takes on the track" copies each selected lane
clip's span onto the root — same source, the offset carried in
`src_in`, whatever the root held over that span giving way by the trim
rule — so the comp is ordinary clips that render, export and travel,
and the lanes keep every take intact underneath. A swipe that takes
half of one take is `s` on the lane and a comp of the half. "mute the
take lanes" is a track flag on each, as one step. Two adjacent choices
from one lane do not trim each other: room is made against a probe
that carries the comp's own placements.

What this does not decide is which take is best. That is a score, and
the score the maintainer wants is the transcript's — the take whose
words match the lyric — which needs whisper on the machine. The choice
arrives as a lane per span; where it came from is the shell's business.

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

## The call

The singer is remote, and a small producer should not need an intercom
box or a second program to hear them. So the voice call rides the same
wire the ops do, in the app by default; whoever has a Dante rig mutes it
and routes their own. `Packet::Voice` is one 20 ms Opus frame with a
site and a sequence number, and it never touches the document: the
shell's `integrate_packet` puts it in `Studio::voice_inbox` and the
call takes it from there — the same seam `outbox` is, pointing the
other way, so the document's integration stays a pure function with no
device in it.

The layers, headless first (each tested without a sound card):

* `studio-collab::voice` — the jitter buffer (frames held to a depth,
  played in order, a missing slot concealed rather than waited for, a
  late frame dropped rather than played late), a `Codec` seam, a
  streaming `Resampler`, and `Link`: one buffer and one decoder per
  peer, one encoder for us, the additive mix, and a meter. A muted link
  keeps SENDING encoded silence, so the far side's loss counter reads
  packet loss and only packet loss. Every frame carries a one-byte codec
  tag; a receiver conceals a tag it does not speak and counts it —
  a mismatched build is a silent peer with a rising number, not a
  speaker full of static.
* `studio-media::voice` — Opus at its narrowest: a frame in, a packet
  out. VOIP mode, 24 kbit/s, DTX on (measured: a silent frame cost 41
  bytes without it and one byte with it, plus a 25-byte refresh every
  400 ms), libopus's own packet-loss concealment.
* `studio-app::call` — the pump. The sink grew a SEQUENTIAL stream
  (`stream_push`: chunks played end to end over the loop and the
  voices — a one-shot per frame would land wherever the callback was
  and gap or overlap) and the microphone grew a live tap that shares
  the device with a take. The UI thread is the playout clock, by proxy:
  the stream is kept topped up to 100 ms every frame and each chunk
  pushed is one slot popped from every peer's buffer. The stream is
  filled with silence from the moment the call starts, not from the
  first frame heard — otherwise the first pump after a peer speaks pops
  the whole depth at once and the jitter buffer's sixty milliseconds
  are spent on the sink's queue instead of the network. The TCP test
  found that race; the fake-only tests could not.

The controls: ☎ joins and leaves, mute, push-to-talk with a button you
HOLD (the one push-to-talk that works on a phone), and the status line
reads the meter — peers, buffering, lost, late, a foreign codec, UI
stalls, dropped mic blocks. No microphone makes the call listen-only,
not refused. A peer who speaks before you join is named as ringing.
Echo cancellation is not built; headphones are the honest answer and
the join message says so. Verified end to end over 127.0.0.1 with only
the two devices faked; NOT yet against a sound card or a microphone.

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

## Riding a knob

The FL footage has automation clips on everything — filter sweeps and
risers into drops, a reverb's mix opening on the last bar — and the
app's lanes reached track and bus gain only. `AutoTarget::BusParam` and
`ClipParam` (an effect by its index in its chain, a parameter by a
16-byte key so the target stays `Copy` and the op fixed-shape) are the
rest of it. A parameter lane follows the fader rule: the effect runs
with `stored × lane × Π(bindings)`, so a curve is a ride on what the
knob says, a filter sweep is a cutoff at 1 kHz and a curve from 0.1 to
8, and everything the curves module promised — gated to a clip, moving
with it, duplicated with it, one curve on twelve ad-libs — holds on a
knob as it does on a fader.

The part that is not bookkeeping: an effect must take a new value
WITHOUT losing its state. Rebuilding the instance at every point
empties the delay line and resets the filter — a click per point. So
`Instance::set_param` moves what can be moved mid-stream (gains,
thresholds, mixes, feedback, damping, a band's design with its memory
carried over), refuses what cannot (a delay's time, a lookahead, a
band's type — buffer sizes, not values), and `is_live_param` states the
same set for the editor, which offers "automate" only on those. The
render applies rides at control rate, 64 frames, the classic bargain.
Pinned: a low-pass swept over a tone by redesign-in-place keeps its
largest step at the tone's own slope; the same sweep by reinstantiate
steps an order larger. And a ride on a disabled effect's neighbour
still reaches the right one, because the cache keeps the chain index
with each instance.

Wire target kinds 2 and 3 without a `WIRE_VERSION` bump: an older peer
already refuses an unknown target kind outright, and a bump would
refuse every older save file for a target most projects never draw.

## Three small tools the footage kept showing

**Chords on the roll.** `Key::scale_step` walks the scale (a third is
two degrees, a fifth four, the octave seven; an out-of-key note steps
from the nearest scale note below it). `Pattern::harmonize` copies
every note on the channel that far away, diatonic when the project has
a key and the major interval in semitones when not, never stacking a
(start, key) that exists; `Pattern::arpeggiate` breaks every chord into
a run — up, down, or up-and-down without repeating the ends — filling
the chord's span. Channel-wide on purpose: the roll has no note
selection, and "add fifths to this bass line" is what is meant. A tool
row above the roll, and palette verbs.

**A folder is a kit.** Drop a directory on the app and every audio file
in it, in name order, is a sampler channel — the way a drum pack
unzips. Non-audio and undecodable files are skipped and counted; more
than 32 takes the first 32 and says so.

**The phone's formats.** mp3 and m4a exports, through ffmpeg
(`studio_video::transcode_audio`): the dithered WAV written beside the
target, transcoded, removed. Stereo only, like Opus. Read back by the
app's own decoder in the tests, the arrangement's length within a
codec frame. Still open on this line: the share sheet itself, which is
a portal call, and the triplet grid, which is a sub-step note position
and therefore a rack-wire change, not a snap setting.

## The picture, rendered

The arrangement rendered to audio since the first mixdown; the other
half of a music video is `studio_video::edit`. The cut list is timeline
arithmetic: every clip on a video track, the FIRST video track on top
where two overlap (every editor's rule, and how the arrangement paints
them), black where none — a gap is a segment, not a skip, because the
audio's timing must hold. The ffmpeg command is a string built from it
(one input per distinct file, `trim` + `setpts` per cut, `color` per
gap, everything scaled and padded to one frame size, `concat`, the
mixdown WAV mapped under it, `-shortest`), tested as a string; and
tested for real where ffmpeg has libx264, by rendering two coloured
fixtures cut against each other and reading the centre pixel back at
four moments. "export the music video" runs it on the export worker
and lands the mp4 next to the project file; a video source whose file
is not where the project says is a gap, and a timeline with no video
is refused rather than rendered black.

Sync by audio was already there (`align_offset_ms`, and the slate's own
aligner with a confidence) — verified before writing a third, which the
agents' guide asks for and this session nearly failed to do.

## A curve over a composition

`Gate::Group(id)`: a binding active over the group's HULL — from its
first member's start to its last member's end, the gap between members
included — with the curve's origin at the hull's start. A riser across
the whole drop, a filter opening over a chorus made of twelve clips;
the ride follows the group as its members move (the origin moves with
the first, the span stretches with the last) and grows as members are
added. A group with no members, or none at all, is never active, and
its binding stays harmless. The palette's "automation curve over the
selected clip's group" mints one, curve and binding as one undo step.
On the wire it is gate kind 3, without a version bump, for the reason
the parameter targets gave.

## The sends finally have something to feed

The mixer had busses and sends since the console panel landed, and the
registry had nothing worth sending to: `voice::Delay` is a tuned comb (a
timbre, by design not a room) and there was no reverb at all. The
footage in `docs/agents/producer-workflows.md` has delay and reverb on
sends in every project. `studio-fx::space` is them:

* **Reverb** — Schroeder/Moorer as Freeverb arranged it: eight parallel
  low-pass-feedback combs into four series allpasses per channel, the
  right channel's lines 23 samples longer so the two decays decorrelate
  into width. Forty lines, allocates nothing after construction, and
  the tail is a known exponential — which is what makes `size` a number
  the tests check (a 0.9 room holds ten times the energy of a 0.2 room
  two seconds in) rather than a feel. The tunings scale with the sample
  rate so a 48 kHz project gets the same room as a 44.1 kHz one, not a
  smaller one. Every room is fed the mono sum, per Freeverb, and the
  frame's rooms are computed together when its last channel arrives:
  a half-stale width matrix measured as a high-pass on the tail — 74 %
  of the energy differed between the ears at width 0 — so every channel
  is one sample late, uniformly, which for a room is nothing.
* **Delay** — time in ms, feedback, a one-pole low-pass INSIDE the loop
  (each repeat duller than the last), mix, and ping-pong, where the
  frame's mono sum enters the last channel's line and each channel's
  repeat feeds the other's, so a hit bounces across the field instead
  of stacking. `instantiate` has a rate and no tempo, so sync is the
  shell's: "sync delay to tempo" rewrites every delay's time on every
  bus and selected clip to a dotted eighth / eighth / quarter /
  sixteenth at the active song's tempo (or the rack's) — as ops, so it
  undoes and travels.

Both carry per-channel state behind the per-sample interface, the
bargain `playrate` made. On a bus that other busses send into, the
mixer adds them all-wet (`mix` 1.0); as an insert, 30 %. The rule is a
pure function with a test, not a checkbox.

## The chain strip

The third piece of the Ozone grammar, after the shared canvas and the meters: a
strip showing what the signal actually passes through, in order, each stage
bypassable and draggable. It exists because **order changes the sound** and most
interfaces hide that — an EQ before a compressor makes the compressor react to
the boosted band, while the same EQ after it does not, and those are audibly
different masters. A test asserts the difference rather than the theory.

Three things the strip has to get right beyond drawing:

* **Bypass must be true bypass** — identical to the stage not being there, not
  "nearly", because the button's whole job is to answer "is this helping?"
  honestly.
* **Latency must be reported.** A chain containing a lookahead limiter delays
  the signal; a host that does not know by how much drifts this track against
  every other one, and the error is a constant few milliseconds that sounds like
  sloppy playing rather than a bug. Bypassing the limiter removes its delay too,
  which is the subtle half — a host told the *wrong* latency is worse than one
  told none.
* **A drag that ends where it started must cost nothing**, since it is the most
  common gesture on a strip.

The strip also warns about the orders that undo themselves: a limiter that is
not last (stages after it push the signal back over the ceiling), two limiters
(the second cannot improve on the first, only squash it), and widening after
limiting (which lifts peaks back above the ceiling). Advice, not enforcement —
an unusual chain is sometimes exactly what a record needs.

The end-to-end test is the one that matters: EQ → multiband → saturation →
imager → +6 dB → limiter, everything built so far in the order it would actually
be used, must still not let a sample past the ceiling.

## The assistant: diagnosis first, and much less of it

The iZotope assistants got the **critique** right — fed a mix they identified
real, specific problems, reliably, across wildly different material. They got the
**prescription** wrong: heavy-handed, especially on EQ and compression, to the
point where the common workflow was to dial the result back to somewhere between
15% and 50% and prefer that.

So the two are separated. `analyze` produces **findings** — what was observed,
where, how confident, and a plain sentence saying why it matters — and that half
is worth having on its own; you can read it and fix things yourself. `propose`
turns findings into moves scaled by a **strength** defaulting to 0.3, in the
middle of the band people actually used.

Four rules follow, all tested:

* **Strength 0 proposes nothing**, so "apply" is fully undoable by turning the
  dial down rather than being a one-way door.
* **Even at full strength nothing is drastic** — 4 dB of EQ and 2.5:1 are the
  caps. The failure being corrected is a tool that, given a big number, proposes
  a big move.
* **Some findings deliberately propose nothing.** An over-compressed master
  cannot be fixed by more processing and a phase problem is a mix decision, not
  a mastering one. The finding still shows, because knowing is the point.
* **A balanced mix produces no findings at all.** This is the actual root of
  heavy-handedness: a tool that always finds something will always do something,
  and a mix that is already fine gets damaged by an assistant justifying itself.

### Two bugs the tests caught, both mine

The first cut compared a narrow band against a broadband average, which is
biased on any sparse spectrum — and it duly diagnosed a low-mid build-up in a
deliberately balanced mix. Exactly the heavy-handedness this module exists to
avoid, caught by its own test.

Fixing that surfaced a deeper one in `analysis`: `band_db` averages **per bin**,
so it measures spectral *density*. Measuring a flat signal's octave levels
showed them declining ~20 dB from 40 Hz to 5 kHz purely because higher octaves
hold more bins — meaning every mix would read as bright-thin and real harshness
would be missed. Comparing bands of different widths must **sum**, so
`band_power_db` now does, and the findings compare octave totals against the
median octave. The median rather than the mean, because it ignores one loud band
instead of being dragged by it — which is what a tonal-balance judgement wants.

## Streaming the media

Ops are tiny; media is not. A session where everyone sees the same timeline but
only one person has the takes is not collaboration, it is screen-sharing with
extra steps.

Assets are **content-addressed and chunked**, for one reason above the others:
peers usually already have most of it. Two editors working from the same shoot
have the same rushes; a re-render changes a few seconds of a ten-minute file;
someone rejoining has everything from yesterday. Addressing chunks by content
rather than position makes all of that free to discover and free to transfer.

Boundaries are **content-defined** — a chunk ends where a rolling hash of the
last 48 bytes hits a pattern — not at fixed offsets. The difference shows the
moment anything is inserted: with fixed offsets, adding a frame at the head of a
file shifts every subsequent boundary and *nothing* dedupes, so a ten-minute
take re-transfers in full because a second was added at the front. A test
inserts 5 KB at the head of an 800 KB file and requires most chunks to survive.

My first cut got this wrong in the way that still passes a casual look: it
accumulated the hash over the whole chunk and reset at each boundary, so
boundaries depended on where the *previous* boundary fell rather than on the
content. Two tests caught it — a file containing the same block twice shared no
chunks, and a run of zeros did not either. A true rolling window (buzhash, with
the hash deliberately *not* reset at a boundary) fixes it.

Assembly re-hashes every chunk instead of trusting it, because a take that is
subtly wrong in one place gets discovered at the mix, which is far too late.
`need` returns missing chunks in first-appearance order so playback can start
before the transfer finishes. What is deliberately absent: transfer scheduling,
rarest-first, peer selection — that belongs with whoever is moving the bytes,
and guessing at it here would be inventing policy for a network this crate
cannot see.

## Teams, join and leave

A **team** is a name plus a list of snapclone sc-IDs — the contacts already on
the device. Nothing new to configure and no separate account: if you can send
someone a snap, you can invite them to an edit. The team id is derived from the
*sorted* member list, so two people building "the same team" independently land
on the same session, and the session key comes from that id the same way
snapclone derives a key from an sc-ID — every member computes it without being
told where the session lives.

The session key is **not a secret** (anyone who can guess the team id can find
it), so the roster is what decides who counts. A join from a contact outside the
team is refused.

### The distinction that matters on leave

Leaving removes your **presence**, not your **edits**. A collaborator who goes
home has still done the work, and their ops are part of the project's history —
deleting a departed peer's contributions is exactly what a naive "clean up on
disconnect" does. `leave` drops presence and touches nothing else, and a test
asserts the project is byte-identical afterwards with their clip still in it.

### Crashes do not say goodbye

A peer whose laptop shuts mid-edit never sends a leave, so liveness is a timeout
on the last thing heard from them — a session that only removes polite
departures fills with ghosts. Presence packets count as signs of life, because
separating "merge presence" from "note we heard from them" is how someone
actively scrubbing gets swept for silence. Members go **stale** before they are
swept, so the UI can say "connection may be poor" rather than letting a
collaborator silently vanish, and returning is a `Rejoined` rather than a new
arrival so the UI can say "ada is back".

`is_idle()` answers the question membership exists for: can this project be
archived, or is someone still working on it?

### One bug worth recording

`broadcast` first sent only the op log — but the log describes edits made *this
session*, and a project opened from a file has a base state predating it. A
newcomer therefore received a clip op with no track to put it on. It now sends a
full snapshot via `file::snapshot`, so the ops that rebuild a project for a
collaborator are literally the ones that rebuild it from disk. Same class of bug
as `Doc::rebuild` starting from an empty project, caught the same way.


**The keyboard is an instrument (and still a keyboard).** `studio-egui::playkeys`
adds a Playing mode beside the Helix motions keymap: the tracker layout
(`asdfghjk` white, `we tyu` black, `zxcvbnm` an octave down) keyed on egui's
PHYSICAL key rather than the logical one — on AZERTY a character-keyed layout
scatters C major across the board, and position is what "locale adaptive"
actually means. Playing mode makes the editing keys inert (in it, `d` is a
note, not a delete) with Escape as the guaranteed way out — tested, because a
mode you cannot leave is a trap. Notes land in the pattern as tuned steps on
the selected channel at the playhead's column: recording into the grid rather
than sounding a live voice is a deliberate line — step entry works today and
its result is what the grid, the bounce, and a peer all already understand,
while a live voice needs a polyphonic one-shot mixer over the audition sink
(named, with audition-on-click, as the next input milestone). Velocity comes
through an `AnalogSource` seam: `NullAnalog` gives a held key full velocity,
and an analog keyboard's travel becomes the step's velocity — a soft press
stays soft in the pattern. The Wooting binding itself is NOT written here:
`games/skydribble/src/analog.rs` already dlopens the Analog SDK behind a cargo
feature with a no-op fallback, and lifting that into a shared crate (rather
than copying 200 lines of FFI no test in this repo can exercise without the
hardware) is the honest next step.


**Sends: the parallel taps.** The Mixer's bus rows gained a sends line —
add a send to any other bus, set its level (commit-on-release, like every
fader here), flip pre/post-fader, remove it. Self-sends and duplicates are
filtered out of the menu: a bus sending to itself is a feedback path the
mixdown would have to defend against, and two sends down one path are two
faders fighting. This closes the gap that made "a reverb bus" unbuildable
in the UI even though `mixdown` implemented sends all along: ROUTING a drum
bus into a reverb replaces the drums with reverb, while SENDING gives it a
copy — two tests pin exactly that distinction (a send raises the mix's
energy over the dry path; muting the return brings the dry mix back
unchanged; level scales the copy).


**Autotune, print-side — where its own docs say it belongs.** `studio-fx::
autotune` (retune's brain + psola's hands) has been complete and unwired;
the tempting move was a chain effect kind, and it would have been wrong:
the module is explicitly a whole-buffer pass — epoch continuity and the
ratio track both need more context than a realtime callback has — so a
chain effect that silently required the whole take would be a lie about
what chains are. Instead the shell exposes it as a take-bounce: select a
clip, pick a key and mode, "tune take", and the corrected version lands as
a NEW source and clip beside the dry one (a new recording, not an edit in
place: the dry take stays on disk, stays undoable, stays audible to a peer,
and A/B is a mute away). It goes to the first audio track with room, the
same rule import follows. Tests pin a 452 Hz take pulled home toward A440
in A minor, identical length so it still lines up with the beat, both ops
undoable, and refusals said out loud for no-selection and for a clip whose
media has not arrived yet — tuning an absent source would land a silent
'take' you would only discover later. Live-monitor autotune remains what
the module says it is: separate work needing a streaming wrapper that
carries epoch and ratio state across calls.


**The clip inspector — the per-clip half of the signal path, finally
reachable.** `Clip` has carried `gain` and `effects` from the start and
`mixdown` has always rendered both (per-clip instances, cached by clip id),
but nothing in the shell could edit them: that half was reachable only by
writing ops in a test. The inspector sits above the arrangement — it is
about the thing you just clicked, so it belongs where your eyes already
are — and collapses to nothing when nothing is selected. Per-clip gain
(commit-on-release, like every fader here) and a chain built from the same
`ADDABLE_FX` catalog the Mixer uses, deliberately shared rather than
re-listed so two catalogs cannot drift. Disabling an effect keeps it in the
chain at its position, because the A/B a person wants is "with and without
this one", not "delete it and add it back at the end". Why per-clip effects
exist beside bus chains at all: a bus treats everything reaching it the
same way — that is what a bus is — while "this one ad-lib needs a filter
sweep and the other twelve do not" is a per-clip fact, and routing that
ad-lib to its own bus to express it would be inventing structure to hold a
single decision. Tests pin that a clip's chain reaches the render, that its
neighbour never hears it, that bypass is audible while the effect keeps its
place, and that clip gain is one undoable gesture. Multi-selection edits the
first clip and says so rather than pretending — a batch shape this panel
does not have yet.


**Live autotune — the other algorithm, not the same one rewrapped.**
`studio-fx::realtune` is the streaming counterpart to the offline pass:
detection on a rolling ring (YIN every ~5 ms), the pitch snapped to a
`Target` — either a key (`Scale`) or, the point of the exercise, an
explicit set of MIDI notes so a hook that only lands on chord tones snaps
to exactly those and a passing note goes to the nearer MEMBER rather than
to some other in-key degree — and resynthesis by a two-tap crossfading
delay line rather than PSOLA. A delay ramping at rate `1 - ratio` IS a
pitch shift; the ramp's wrap hides under Hann windows that sum to unity at
50% overlap. That is cheaper than PSOLA, needs no lookahead, and has fixed
reportable latency (~22 ms), which is what a monitoring path must be able
to state about itself. It IS registered as a chain kind (`"autotune"`),
because unlike the whole-buffer pass it is honestly an insert. Measured,
not asserted: the shifter is exact to under a cent, while the LOOP is
open — the ratio comes from YIN's estimate of the input and is never
checked against the output — so the detector's own bias (~6 cents at 452
Hz, ~16 at 554) passes through. Tests measure by zero crossings on
purpose: verifying with the same YIN that produced the bias would have
hidden it, which is exactly what the first draft of those tests did.


**The tuner, smoothed and made affordable.** Three changes, each with a
measurement behind it. (1) `analyse()` allocated a Vec every hop — a malloc
inside the audio path, invisible in tests and audible as a dropout the
first time the allocator takes a lock; the scratch buffers are now
preallocated. (2) Detection was full-rate YIN at a 70 Hz floor, ~2M
multiply-adds per 5 ms hop, most of a core for ONE voice; it is now a
coarse search on 4× decimated audio followed by a local full-rate
difference search with parabolic interpolation. The suite time for these
tests fell from 5.1 s to 0.5 s, and accuracy IMPROVED — the refinement
closed most of the open-loop bias the previous pass documented, so the
assertions went from 15–25 cents to under 5. (3) The applied ratio now
ramps across the hop instead of jumping at each analysis boundary: a step
in the delay line's read rate is a tick every 5 ms on a sustained note, and
a test pins that no single sample moves the ratio by more than a thousandth
even with smoothing set to hard-snap. A cost test asserts 1 s of stereo
processes in under 0.25 s so a regression shows up as a failure rather
than as a crackle.

**Adaptive saturation — character from the material.** `studio-fx::
adaptive_sat` (chain kind `adaptivesat`) drives from two independent
detectors: REGISTER (the fundamental, via the same coarse+refine detector
the tuner uses, mapped log-wise between 60 Hz and 600 Hz) and SIBILANCE
(high-band energy over total, from one-poles — cheap, pitch-free, responds
in milliseconds like an "s" demands). Low notes get more drive and high
notes less, because an 808 wants tape and a lead is already bright;
sibilance always pulls drive DOWN, because a saturator's job is adding
harmonics and sibilance is the last thing that needs more. Either amount
set to zero disables that detector alone — plain register-following drive,
or plain de-essing — and both at zero is bit-identical to the fixed
`Saturator` it wraps, which is what makes it A/B-able against what it
replaces. `Link::Tied` scales the sibilance response by register: careful
up top, relaxed down low, one gesture instead of two knobs kept consistent
— a mode rather than the default, because material whose sibilance lives
in the low register (a distorted bass with fret noise) wants them apart.
Profiles deliberately do NOT crossfade per-sample — different harmonic
signatures and DC behaviour would smear the very character the stage
picks; the amount and asymmetry move instead. Unvoiced stretches HOLD the
register so a drum hit between two sung notes cannot swing the character,
and `detectors()` exposes both numbers, because an adaptive effect whose
decisions you cannot see is indistinguishable from a broken one.


**The Program panel is the piano roll.** FL's grammar splits a channel's
content in two — the step grid for hits, the roll for held notes, chords
and slides — and the Rack tab only ever had the first. `Pattern` gains
`rolls`: per-channel `Note` lists (key offset from the channel's own pitch,
start, length, velocity, slide), serde-defaulted so a pre-roll rack loads
with the empty roll it effectively had. A roll note renders through the
SAME placement pass as a hit: swing moves it by the off-step rule, choke
groups cut it, the bounce and the audition loop render it, and the
whole-rack LWW sync carries it — a note is more pattern, not a second
engine. Its length is its gate (cut with the choke fade; a short-decay 808
under a long note simply ends earlier), and slide is a note property
gliding from the previous roll note, which is where FL puts it. Samplers
pitch now too: with no pitch field for `apply_step_mod` to offset, the
renderer repitches the RENDERED voice — which un-no-ops the grid's Tune
lane on sample channels in the same breath and makes a one-shot chop a
melodic instrument. Wire v9 carries the roll. The panel itself follows
`grid`'s contract: pure geometry (row/key mapping across negative offsets,
body-vs-edge hit testing) returning edits as values; drag draws a note out
in one gesture, click removes, the right edge resizes, right-click toggles
slide. Velocity got its lane: bars under the
roll sharing its columns the way `lanes` sits under the grid, one dynamic
per start column because a chord is one gesture. The automation strip
landed too — `studio_egui::auto` under the timeline, mapping time through
the SAME `View` the arrangement uses so alignment is by construction:
click places a point, right-click removes, a vertical drag is one op and
a time move is two (the left-trim precedent), all through the op-log door
so a ride is undoable, on the wire, and refusable out loud.

**Automation — the ride, as ops.** `Project` gains lanes: points per
`AutoTarget` (track gain and bus gain first; effect parameters landed
in c17 — "Riding a knob" below), edited by
`SetAutoPoint`/`RemoveAutoPoint` in the house op shape — absolute
(time, value) so peers converge per-point, a refusal when a removal has
nothing to restore, locked tracks refusing their lane the way they refuse
their fader. A lane RIDES its fader (multiplied, never substituted —
pinned at 0.5 × 0.5 = 0.125), and a project with no lanes renders
bit-identically to a pre-automation one. `mixdown` evaluates per sample
at the tick its clip loop already computes; the bus fold rides per
interleaved frame so a ramp cannot tilt the stereo image; the streaming
engine inherits both through the shared path. A file saves lanes because
a file is the ops that rebuild the project — `snapshot()` emits each
lane's points before the relock, tested with a locked track's lane on
purpose. Wire v10. Named, not built: the timeline view that draws and
drags these points.


**The export flow grew its questions.** The one-click exporter stays (a
synchronous whole-arrangement render to `./exports`, no dialog); "export…"
now opens the flow around the questions: opus or wav; full arrangement,
selection (the hull of the selected clips, refused out loud when nothing
is), or the rack loop; one file or stems — a stem is a soloed track for the
arrangement and a channel for the loop, and a track earns a stem only if it
carries material in the span (for a selection export, only SELECTED
material: soloing a track whose in-range clip wasn't selected would smuggle
it in). WAV keeps rate and layout; opus gets a bitrate and pins 48 kHz
stereo — the codec's contract, enforced in the run rather than trusted to
the dialog. Then the native picker (`rfd`, XDG portal, behind the `picker`
feature — a headless build falls back to `./exports` and the status line
says so; with a real picker, cancelling it cancels), then the tracker: a
worker thread renders chunk by chunk through `mixdown_with_cache` — one
cache per job, so the tracked export is bit-identical to the untracked one
by the streaming renderer's own pinned property — reporting progress over
mpsc drained once per frame, with a cancel checked between chunks that
writes NO partial file and keeps the files already finished. The decision
layer (`export::plan_jobs`/`plan_rack_jobs`) is pure and tested headless;
the end-to-end tests export real files, decode the opus back through this
repo's own decoder, and pin that a cancelled run leaves nothing behind.


**Saves have history now.** The `.studio` file already carried everything a
session is — snapshot ops (mixer busses, chains, sends, automation), the
rack (channels, patterns, rolls), optional undo history — and its folder
already moved as a unit. `project_dir` makes that folder a git repository:
every Ctrl+S commits (identity supplied inline, so a machine that never ran
`git config` still saves), `meta.toml` lands beside the file (name, bpm,
counts — the greppable card catalogue), and "the mix from Tuesday" is
`file_at(commit, "project.studio")`, read without touching the working
tree. An unchanged save is `Ok(None)` — a fact, not an error — and a
missing git binary demotes to a status note, never a lost save.
`.gitattributes` marks compressed media `-delta`: the deltas do not exist,
and the real compression rule stays the asset store's (never store PCM).
For media that outgrows git — video — `MediaMode::External` gitignores
`media/` and commits a manifest instead, so every version still says what
it needs while the bytes live in a content-addressed store: `apps/gitmind`'s
(AV1 transcode, near-dup chunk linking — its `MEDIA.md`) is the intended
tenant of that seam, by path rather than by a cross-app crate dependency.

**The workbench layout.** Four full-screen tabs remain the phone's layout;
the desktop gets "workbench": the timeline (inspector, arrangement,
automation strip) stays on screen while Rack / Program / Mixer live in a
resizable strip below it, sharing the SAME panel state as the tabs so
switching layouts never loses your place.

**The routing map.** The Mixer's rows are the right place to CHANGE routing
and a hopeless place to READ it — a graph question answered one dropdown at
a time. The map draws the whole graph at once: tracks left, busses in
columns by depth (deepest first, so left→right follows the signal), master
right; main routes solid, sends dashed (amber = pre-fader); an unrouted
track's implicit master feed drawn explicitly, because a default nobody can
see is most of why routing reads as confusing. Read-only on purpose: the
map explains, the rows edit. Layout is pure and pinned, including that a
foreign file's cycle terminates instead of hanging the frame.


**The archive is the professional artifact.** The folder-of-relative-paths
save loses files eventually — a moved sample, a half-synced directory, a
session with holes. `.studioarch` is the session as ONE file: all state
(the SaveFile — mixer, automation, rack with patterns and rolls) and every
reference recording embedded, opening whole or refusing whole (every-prefix
truncation sweep, the collab format's discipline; trailing garbage refuses
too). The save popup's FIDELITY SLIDER decides what the audio costs:
sketch/demo/standard re-encode each source to Opus proxies at 64/96/160k —
the musician's proxy workflow, an archive that plays everywhere — while
master embeds originals byte-for-byte (a `.opus` stays `.opus`) with
dithered 16-bit WAV for PCM-only takes, never a hole. The popup also holds
the credits / authorized collaborators and the SEARCH PATHS where each
device may hold full-quality artifacts: on open, `find_upgrade` checks them
— exact basename first, then lossless extensions before lossy, because a
worse file than the proxy is an upgrade in name only — and decodes the
device's file over the embedded proxy. Pinned end to end: a bounced session
packed, moved to a bare directory, reopened from the archive ALONE, and the
beat still renders. `project_dir`'s git history stays as working-folder
convenience; the archive is what leaves the building.

**The viewport, and the music-video loop.** The workbench grew a
configurable right slot — viewport (default), mixer, rack, program, or off
— so the frame under the playhead sits beside the music panels instead of
an export → import → watch iteration through the filesystem. Clip choice is
pure and pinned (topmost video track wins, trims honoured, a gap lets the
lower track show through); frames come from `studio_video::frame_rgb` (raw
RGB at a fixed size, aspect padded — no image decoder in the loop, tested
against a real ffmpeg fixture) cached on a half-second key: a scrub
preview and edit reference at ~2 fps, said on screen, with realtime
playback named as the streaming decoder's job. Tabs stay the phone layout;
every strip is resizable or off.


**Archive v2: dedup is exact; similarity is a report.** Media is stored as
content-defined chunks (the assets module's own chunker): a re-bounce that
changed one bar, a comped vocal reusing a phrase — the shared bytes land
once, every reassembly verifies against its whole-asset hash, and one
flipped bit refuses the whole archive, because a subtly-wrong take is worse
than a refused file. The tempting next lever — batching MERELY SIMILAR
audio under one entry — is deliberately not a lever: two takes 97% alike
may differ precisely in the distortion someone chose, and a format that
collapses that has made a musical decision on its own authority. Instead
the pack report names the near-duplicate pairs with their chunk-overlap
score (gitmind's yardstick) so a person can consolidate if the takes really
are one take; the status line surfaces dedup savings and the pairs. (One
test lesson recorded in the suite: periodic fixture data never trips the
rolling hash, making every file one MAX_CHUNK — noise-like data or the
fixture tests nothing.)

**The roster is enforced.** An archive that names artists derives the
session TEAM from them — the same sorted-roster id math hand-built teams
use, so every holder of the archive lands on one session and a stranger's
join is refused at the door. An archive with no roster keeps the opener's
own team: metadata that was never written must not lock anyone out.

**Layout, saved simply, per device.** `<name>.workspace` beside the
project: workbench on/off, panel, side slot, zoom, scroll — written on
save, read on open, lenient by construction (unknown keys are a newer
build's business, garbage means defaults, hostile numbers clamp), and
never load-bearing: view state must not be the reason a project fails to
open. This is the pattern for several frontends sharing one project — each
keeps its own sidecar over the shared file, and their states save simply
because they never entangle.


**Vocal alignment, and the tooltip that teaches it.** Mapping a double to
its lead by eye is the slow part of vocal work, so the timeline grew the
tools: select the lead, shift-click the take, **A** aligns it — envelope
cross-correlation (one value per millisecond, so it is rate-agnostic by
construction and phase-blind, which two mics of one performance need),
±500 ms search, landing as an ordinary `MoveClip`: undoable, on the wire,
refusable out loud (wrong selection count and absent media both SAY so).
**Q** quantizes selected clip starts to the rack's step grid; **, / .**
nudge by a step and **< / >** by a millisecond, clamped at zero. Two sign
disciplines are pinned because both first drafts got them wrong: silence
correlates to an honest zero rather than the leftmost shift that beat the
sentinel, and the take lands at `reference − lag` — the `+` version would
have doubled every error instead of fixing it. And the part that makes any
of this exist for a user: hovering a clip shows its name and the tool list
— `CLIP_HINTS`, a public const the shell's key handler pins BY TEST: every
intercepted key must appear in the tooltip and stay unbound in the Helix
keymap, so the shortcuts, the keymap, and what the clip teaches cannot
drift apart. A shortcut nobody can see is a shortcut nobody uses.


**The mixer grew its input side, the timeline grew compositions, the
library grew eyes.** Routing could only ever be edited from the SOURCE end
— a track's combo, a bus's parent fixed at creation. `SetBusParent` (wire
v11) makes reparenting an op with the same cycle refusal `AddBus` had, and
the Mixer's "bus inputs" section reads the graph the way a console does:
each bus lists what feeds it, ✕ returns a feeder to master, "+ input" adds
a track or bus from the receiving side. Compositions are kdenlive's
pre-batched groups as the smallest honest model: `SetGroup`/`RemoveGroup`
(absolute membership, refused empty, dissolving never touches a clip),
folded on the timeline as one chevroned block — `g` groups a selection
(born closed, because the fold is the point), the ▸ opens it, clicking the
block selects its members so nudge/quantize/align/delete act on the
composition as one thing, and open-or-closed is per-device sidecar state,
never an op. The viewfinder collapses when no video clip exists — a music
project gets its width back. And the library finally fits a real one
(23,000 files deep): a LAZY tree (one readdir per opened folder, cached),
a bounded deep search, batch-converter variant detection (the `.wav`
beside its `FFaudioConverter/*.opus` — lossless offered first), and a
filename classifier feeding the ⋯ menu's smart offers: "replace channel
'Snare'" when the rack has one, "clip on a new track → bus 'Drums'" for
drum material with the bus found by name or made on the spot. Every
classifier case in the tests is a verbatim filename from the library this
was asked against; a wrong guess costs one wrong suggestion, never a
wrong action. (The voiceterm hover-drag selector is the named upgrade for
that menu once the widget lives in shared/.)


**The morph pad.** logscope has its matrix, beatspace its latent pad; the
sample library now has the same gesture: load one to four SIMILAR sounds
onto the field's corners from a sample's ⋯ menu and the puck blends them
(renormalizing bilinear weights — two anchors degrade to a clean one-axis
crossfade, pinned), or with one sound loaded the puck reshapes it: X is
spectral tilt (a crossfade toward a one-pole low/high split whose halves
sum back to the original by construction), Y is decay (the roll's gate
rule, driven by a knob). Neutral is pinned to be the IDENTITY — a pad that
colours the sound at rest cannot be trusted anywhere else — and blending
is rate-agnostic (a 44.1k anchor beside a 48k one, pinned by
zero-crossing). The pad's output enters the workflow through the
library's own door: right-click bounces the current sound to
`<library>/morphs/` as a WAV the tree, classifier and rack/clip buttons
treat like any other sample (unique names, the exporter's rule); double
right-click resets the puck AND un-bounces the file the double's first
click just wrote — a reset that leaves junk behind teaches people to fear
the reset. The hint line lives on the widget, the tooltip lesson applied
at birth. Similar-sounds blending stays a hands-on act: the pad judges
nothing and merges nothing on its own authority — the archive's
distortion rule, one surface over.


**Lossless without the WAV bill.** studio-media grew a real FLAC encoder —
deliberately the honest subset: fixed predictors (best of orders 0–4 per
frame) with Rice coding, 16-bit mono/stereo, dithered from f32 by the WAV
path's own TPDF rule, escape-to-verbatim for pathological frames. Small
enough to read whole, and validated the only way that proves anything:
sample-exact round trips through symphonia's independent decoder, with
multi-byte frame numbers exercised on purpose (the clever coded-number
writer broke at exactly frame 128; the boring explicit-thresholds version
replaced it and the test that caught it stays). Library-bound renders now
ride a per-device fidelity rung — opus 128/192/256, flac, wav — chosen on
a settings slider and stored in the workspace sidecar, because storage
constraints ARE per device: opus-192 is the default (transparent on most
systems at a tenth the size), flac is the lossless rung, wav exists for
whoever insists and is nobody's default. A build without the opus feature
falls to flac, bigger but never a hole. The archive's Master-level PCM
fallback went the same way (v3): a bounce embeds as FLAC, byte-exact
originals still byte-exact.


**The breakout loop.** The pad's place in the workflow, made structural:
start with a basic pattern, break the sound OUT to the pad, tune it,
return the authored sound to the pattern. The classifier now proposes pad
loadouts — "pad: load with 3 similar" in a sample's ⋯ menu, neighbours
found in the sample's OWN directory by the same classifier (a format
variant of the same sound is not a neighbour) — proposed, loaded only by
the click. "⇐ channel" breaks the selected rack channel's sample onto the
pad. And the return path closes the loop twice over: "⇒ replace" bounces
at the device's fidelity rung and swaps the channel to the bounced FILE
(the source of truth is the file, not the in-memory blend — what you hear
later is what returned), while "⇒ layer" adds it as a new channel that
INHERITS the source channel's steps and step-mods in every pattern, a
copy not a link, so the layer plays in lockstep and then diverges freely
— which is what developing the authored layers means. Every returned
sound is also a library file: authorship accretes where the samples live.


**The synthesizer is in the piano roll now.** The built-in synths — `Sub`,
the 808 voice with glide, tube drive and decay, and `Sine` — were half
wired to the roll already: a note's key reached them through the tune mod
and slide became glide, but the envelope was a fixed decay from t=0, so a
held note DIED at `decay_ms` no matter how long it was drawn. The synth
was reachable and could not be held. `Synth::render_gated`
(default-implemented on the trait, so a host synth inherits held notes for
free) gives the tonal kinds a gated envelope: ~2ms attack, sustain at
unity while the note lasts, then a fixed 30ms note-off release — NOT a
second `decay_ms`, because the roll's oldest pin refused that draft: a
2-step note on a 2-second 808 must stop at 2 steps, and the long ring-out
is the hit path's sound. Placement sizes each note's window as gate plus
release tail; the sampler and drums are untouched (the sampler's window
already IS its gate, a drum's decay is its identity). Pinned: held means
held (step-5 RMS ≈ step-0 RMS where the old envelope was silent),
note-off still gates half a step later, and an octave in the roll is an
octave out of the synth by zero-crossing count. Draw notes on the 808
channel and it is a bass synth; the glide the hits already had chains
through the roll the same way.


**The mix & master pass, callable from the mixer.** `studio_fx::assistant`
(findings-first analysis, strength-capped proposals, and the two rules its
own tests enforce — strength 0 changes nothing, a balanced mix yields NO
findings) and `studio_copilot::mix_pass` (findings → real ops) both
existed and neither was reachable: the analysis had no caller, the
proposal had no button. Now the Mixer — the panel mixing and mastering is
FOR — has "mix & master…", which measures the actual mixdown (loudest
power-of-two window, crest, correlation, low-end width from a low-passed
pair) and opens a window in the assistant's own order: measurements, then
findings, then a strength dial, then one apply. Two things the caller had
to get right. `mix_pass` emits each move as a standalone one-effect chain
because it is pure and cannot see the bus — applied verbatim those would
delete the user's own effects and then each other, so `mix_assistant::
compose` folds them onto the existing chain as one `SetBusChain`. And the
whole pass applies as ONE undo step (`Op::Group`, wire v12, atomic with
rollback): a mastering pass that takes six presses of `u` to reverse is a
pass nobody dares try. Pinned: the user's reverb keeps its place at the
head of the chain, one `u` restores both chain and gain, and a mid-group
failure leaves nothing behind. One bug found by its own test and worth
recording: judging silence on the mono SUM skipped analysis on a fully
out-of-phase mix — precisely the mix most worth warning about — so
silence is judged per channel and a collapsed sum reads one channel
instead, letting the correlation finding carry the warning.


**The command palette (`C-p`), built as paths rather than a list.** Type a
few letters, pick a verb, and if the verb needs a target the palette asks
for that too: "route" → which track → which bus, *or* "new bus", which
creates and routes in ONE group so changing your mind never leaves an
orphan bus. That step structure is the design, not decoration — because a
command is a path of choices resolved by matching text,
`Palette::resolve(project, &["route", "vocals", "drums"])` returns the
finished act with no UI at all. That is the seam voice mode plugs into
later: no second implementation of the wizard, no second registry of
verbs. Three properties are pinned. Fuzzy ranking is a subsequence match
with word-start and run bonuses, so "rtb" finds "route track to bus" and
"undo" beats longer matches. Every entry that has a keybinding CARRIES
that key (`Act::Key`) rather than reimplementing the verb, shows it
right-aligned — a palette that teaches its way out of being needed — and
a test asserts every claimed key is one the keymap or the shell's
interception actually handles. And a completed path applies through
`edit_group`, so a two-op command is one press of `u`. While the palette
is open it owns the keyboard: typing `d` is a search, not a delete.


**The library sidebar, narrowed.** Search results used to repeat the folder
on every row, which is what forced the sidebar as wide as the longest path
in the pack; they group under one folder header now and the rows carry
only the filename (the full path survives underneath, so every action
still works). Every button moved to the RIGHT in one column the eye can
track, the identicon to the LEFT: extension tag, tinted by format, plus an
ANVIL when the file is heavy for a phone. Heaviness is judged from
extension and size — the only inputs available without probing 23,000
files — and pinned against the two cases it was specified against: a
96 kHz float WAV at 6144 kb/s wears the anvil (heavy by KIND, whatever its
size), a 219 kb/s Opus never does at any plausible one-shot size, FLAC is
light until it is simply big, and mp3 is marked by kind. The mark has to
be rare to mean anything. Below ~260 px the row goes compact: the name
elides to first-four…last-four (`Vox_Dry_Waduwap_high_A#.opus` →
`Vox_…h_A#`), short names shown whole, and the two word-buttons fold into
⋯ — the controls survive, which is the right thing to keep. Finally,
triage against rack overload: SWIPE right to pin a sample to the top of
its listing, left to demote it to the bottom, both toggling, both also in
the ⋯ menu because a gesture nobody discovers is a feature nobody has. A
swipe needs real travel (36 px) so a wandering scroll never re-ranks the
library. Pins are per project, stored in the workspace sidecar rather than
the op log: a pin is how one person keeps their head straight in one
session, not a fact about the music, and a collaborator's triage must not
rearrange yours mid-take.


**Two touch bugs from real use.** The list would not scroll over text on a
phone: the row claimed the gesture with `Sense::drag`, so the ScrollArea
never saw it. Rows are hover-sensed now and the press is tracked by hand,
which leaves every drag with the scroll area — and the swipe additionally
requires the travel to be decisively horizontal (at least 2:1 over
vertical), because a finger flicking the list up through a row always
wanders sideways. Pinned: a 60×200 px flick is a scroll, an 80×39 px drag
is a pin. Second, the row mixed `small_button` (tight, text-sized) with
`menu_button` (full-height), so each icon sat at a different offset inside
a differently sized hit rect — every control is now the same 34 px square
via one `touch_button` helper, the row is one touch tall so identicon,
name and buttons share a centre line, and the compact threshold is derived
from the actual control budget (`wants_compact`) rather than a guessed
pixel count. Folding to compact removes buttons from the row but never
verbs: both fold into ⋯.


**The playhead can be moved now.** `g g` and `g e` always reset it to the
start and end — but nothing on screen said so and the ruler did nothing,
which meant a new user had no way back to the start at all. Three fixes,
each at its own layer: the ruler band takes clicks AND drags and reports
`sought` every frame of the drag, so scrubbing follows the finger (clamped
at zero, since a negative playhead is not a position, and strictly above
the tracks, since selecting a clip must never scrub); ⏮ and ⏭ sit in the
top bar with their key bindings in the tooltip; and the palette carries
"playhead to start / to end" with the same hints. The seek is a transport
move, not a model edit — moving the playhead is not something a
collaborator should undo. Pinned headlessly: the band seeks through the
same transform the ruler labels use, the strip below it does not, and the
gutter column is not time.


**Render efficiency: 2.2× on a real workload.** Measured, not guessed —
the sample loop was doing a `BTreeMap` lookup PER SAMPLE to fetch the
track's automation lane (48,000 tree walks per second of audio per track,
for a value that cannot change within a track), then binary-searching that
lane per sample, then re-checking the destination bound per sample. The
lookup is hoisted, the lane is swept with a cursor (`eval_lane_cursor` —
O(1) amortized forward, falling back to the search on any backwards jump
so it is exactly `eval_lane` for every input), and the bound is resolved
once by arithmetic. Same machine, same 8-track/80-clip/dense-automation
workload: 110 ms → 50 ms for ten seconds of audio, 91× → 202× realtime.
Guarded by an equivalence test that recomputes the expected mix from the
model with the naive per-sample lookup, so the fast path cannot drift from
the meaning.

**Vocal takes.** The editor could import, bounce and synthesize audio and
could not RECORD any. `capture.rs` is the input half of `audition`'s
output half: a cpal input stream behind the `audio` feature, a null
capture that SAYS it has no device rather than faking one, and an audio
callback that never blocks — it `try_lock`s and drops-and-counts rather
than waiting, because a dropped block is a click in one take while a
blocked callback is a glitch in everything. Dropped blocks are reported on
the take, not swallowed. ⏺ in the top bar turns red and counts up while
live; `C-r` and the palette do the same. A finished take is trimmed of the
walk to and from the microphone (a silent take is returned WHOLE — a
mic-gain mistake must not delete the performance), imported as an ordinary
source, and placed as a clip at the playhead where recording started, so a
take is one undoable, shareable, archivable op like any other edit. Its
asset is stored FLAC-encoded, so a phone session's takes cost what they
should. `C-r`, not `r`: the palette's drift test refused `r` the moment
this tried to take it, because the keymap already owns it — the guard
doing exactly its job.


**Microphone: negotiate, don't assume.** The first capture build failed on
a real Linux stack with `snd_pcm_hw_params` → ENOENT, which is ALSA's way
of saying "not that combination, and I will not tell you which" — because
cpal's `default_input_config()` reports a configuration the device then
refuses. Opening now walks what the device actually enumerates: `attempts`
builds the try-order (48 kHz first, the project rate with no resample,
then 44.1, then the range's own bounds; mono before stereo, since a vocal
take is mono and mono opens where stereo sometimes will not; backend
buffer before a pinned 1024-frame period, because ALSA plug devices reject
one or the other and which varies by machine), rates outside the device's
enumerated range are never proposed, and nothing is tried twice. Both
stages must pass — `build_input_stream` succeeding is not enough, since
hw_params can still refuse at `play()`. If the default device refuses
every attempt the open falls through to every other input device on the
host, and total failure reports how many configurations were tried plus
the last error, rather than repeating one opaque backend string. The
negotiated device and rate land in the status line, because "recording" at
a rate nobody chose is how a take ends up the wrong length.


**The save structure, rebuilt around containment.** A real bug, reported
from real use: the first `project_dir` ran `git init` and `git add -A` in
whatever directory the project file sat in — launched from a source
checkout, that is the USER'S OWN REPOSITORY, and a save committed 2,734
files of their code with the message "save". The fix is structural. A
project now lives in its own directory under a projects root
(`$STUDIO_PROJECTS`, else wherever the last archive was written, else
`$XDG_DATA_HOME/studio/projects`) and never in the working directory. That
directory is marked with `.studio-project` at creation. `guard` refuses
any directory that is not marked ours, and refuses outright when the
directory OR ANY ANCESTOR is a git work tree we did not create — the
ancestor walk being the case that actually did the damage. Commits name
explicit pathspecs; there is no `-A` left in the file, so even a
mis-scoped directory could stage only the handful of files a project
consists of. A folder holding nothing but project files is still adoptable
(an older build's save), one stray `src/` is not. All four properties are
pinned, including that a refused save leaves the foreign tree byte-for-byte
untouched.

Shaped by what the neighbours do: Zrythm makes each project a DIRECTORY
under `XDG_DATA_HOME` with timestamped snapshots in `backups/` — the same
layout, with git commits playing the part of numbered backups; Reaper
defaults to a projects ROOT with a per-project subdirectory, which is the
"never save loose in the cwd" half; and Audacity abandoned `.aup` plus a
`_data` folder for the single-file `.aup3` after years of users moving the
folder and breaking sessions — the same argument that makes `.studioarch`
the thing you send while this directory stays the working copy.


**Note names, a project key, and the guide that reads it.** Every piano
roll row now names itself — `A1`, `C#4`, `A5` — instead of only the octave
lines carrying a signed offset. The labels come from `studio_core::
note_label`, which reads the SAME reference pitch the synth does (key 0 is
A1, 55 Hz, `sub_freq`'s own reference), so a label and a sound cannot
disagree; that is pinned. The project gained a `key` (root + scale: major,
minor, dorian, mixolydian, chromatic) as an absolute, undoable op, and the
roll dims out-of-key rows when the guide is on — dimmed, never disabled: a
scale is advice, and a tool that silently refused the blue note would be
wrong about music. `None` key means no opinion and nothing is dimmed. The
palette carries "set project key" (root, then scale) and "toggle
out-of-key dimming".

**Markers, and the wishes behind them.** Reading the FL Studio and
kdenlive communities, the same request appears in both vocabularies:
named markers, jump-to, and "Delete All Timeline Markers" (kdenlive
26.04 literally shipped that action; FL's Alt+T marker naming is a
top-of-list wish; Premiere users ask for marker-based selection). So:
`SetMarker` is absolute BY NAME (setting an existing name moves it — "no,
the chorus is here"), `ClearMarkers` inverts to a single `Group` so one
`u` restores every one, and the palette gains "drop a marker here", "jump
to marker" (a wizard over the project's own markers), "delete all
markers", "hide (mute) a track" (the other repeated ask), and "split
every clip at the playhead".

**One voice framework, no second command system.** `voice.rs` is the
dialog handler and the port of voiceterm-qt's feedback popup, built on the
seam the palette already had: a command is a path of choices resolved by
matching text, so speech needs no verbs of its own. Three pure pieces.
`normalize` closes the gap a recognizer leaves — "eight oh eight" → `808`,
"see sharp" → `C#`, "b flat" → `A#` (flats spoken, sharps stored),
connectives and filler dropped so they cannot dilute a score. `policy`
decides what one utterance may do: destructive or wide acts are said back
and wait for a leading, unambiguous "yes" — the first draft searched the
whole reply for an affirmative and took "no, go to the mixer" as consent,
which is worse than no confirmation and is now pinned against. `Dialog`
walks the palette consuming the longest matching prefix at each step, so a
whole command fits in one breath ("route vocals drum bus") or arrives word
by word, with the popup listing what can be said next — the reason
voiceterm showed a popup at all. `EXTRA_SPOKEN` collects the phrasings
nobody would put in a menu because nothing was listening: asking the room
a question, acting on everything except one thing, starting a take on a
count. The recognizer itself is not in this repo; `App::voice_utterance`
is the one call a backend needs.


**Two libraries, one search, ordered by where you are.** `STUDIO_VIDEO_LIB`
names a footage root beside `STUDIO_LIBRARY`'s sample root — two roots
rather than one tree because they live on different disks and arrive on
different schedules (a pack is installed once, footage arrives per shoot).
Search now covers both, and the ACTIVE TAB decides the order: on the
timeline footage is a first-class hit; in the rack and the program tab
samples come first and footage stays reachable underneath, because a
sampler can absolutely take its material from a video's audio and refusing
that would be the tool deciding what a sound is allowed to come from.
Footage rows carry a ▣ so a video turning up in a sample search is never a
surprise, and switching tabs re-ranks the results rather than serving
stale ones.

**The video sampler's headless half.** `studio_video::audio_segment` pulls
the audio between two points out of a video as WAV bytes any decoder here
reads — trimming inside ffmpeg (`-ss` before `-i`, `-t` for the span) so
taking two seconds out of a forty-minute rush costs two seconds of work.
A backwards or empty span is refused, and a video with no audio is
`NoAudioStream` — a named fact rather than an empty buffer that would look
like silence — in both shapes ffmpeg reports it. Tested end to end against
a generated fixture: half a second asked for, half a second of real tone
back. The UI is deliberately NOT wired: the plugin's shape is the open
question, and this is the part that will be true whatever shape it takes.


**Genre vocabularies for the voice fastpath.** A recognizer emits what a
person SAYS, and nobody names a file "braams" or "hi hat" — so the
fastpath gained starter dictionaries: common, trap, hip hop, classical,
film score, country, and instrument names. Each is a small table mapping a
spoken phrase to the word the library actually uses ("sub bass" → 808,
"pedal steel" → steel, "pizzicato" → pizz, "whoosh" → riser), applied
longest-phrase-first so a two-word term never splits into two useless
ones. Deliberately STARTER-sized: the terms someone says in the first
minute, not an attempt at a genre's whole language — they are data, so
growing one is a line.

The design rule that matters: **an alias may only widen what can be
understood, never narrow it.** Aliasing "vocals" to "vox" stops a track
literally named *vocals* from matching — so the aliased words are a second
READING, tried first, with the literal utterance tried after. That bug
appeared immediately in the tests and is now pinned both ways. A drift
guard checks every entry in every table: targets lowercase, single-token,
non-empty, and spoken sides that survive normalization — an alias pointing
at a word no file uses would leave the spoken term worse off than saying
nothing. Unknown words always pass through, `common` cannot be switched
off, and the chosen domains ride in the workspace sidecar because the ears
follow the room, not the song. Pick them from the palette: "voice
vocabulary (genre words)".


**Find in the timeline — voiceterm's lever, not a second implementation.**
Searching a session of takes for a spoken word is the case where a real
recognizer is correct and far too slow to type into. voiceterm already
solved it and its own notes name the mechanism: the fingerprint DB
short-circuits whisper on most utterances. So the pure half of that —
`phonetic.rs`'s MFCC pipeline and `acoustic_index.rs`'s 39-dimensional
mean/std/delta fingerprint with its cosine metric — was lifted VERBATIM
into `shared/voiceprint-kit`, where a second app can use it without a
second implementation drifting from the first. voiceterm's exemplar
database, promotion policy and whisper pool stay in voiceterm, which is
where that policy belongs.

`find.rs` layers the search: a NAME match first because it is free and
certain (and never invents a distance it did not measure); then the
fingerprint prescreen over every candidate clip — 39 floats and a cosine,
microseconds each — which orders the field without deciding it; then
verification on the survivors only, through a `Verifier` trait whose
intended implementation is voiceterm's whisper pool. With no verifier
present, results are returned as a RANKING and labelled as such: the panel
shows "name", "sound ~0.14", or "heard: …" so a cheap guess can never be
mistaken for something a recognizer read. A verifier that runs and
disagrees demotes the hit — heard-but-wrong is weaker evidence than
never-heard, because something did listen. And the gate answers the other
question: search everything, only tracks carrying audio, or ONLY the
silent ones ("why can I not hear track twelve" — a track with no clips at
all and a clip below the floor are both reported, while media that never
arrived is neither, because that is a different problem). One fixture
lesson recorded in the kit: steady synthetic tones are degenerate for a
mean/std/delta statistic — two thirds of its dimensions are variance and
frame-to-frame delta, which for a constant tone hold only rounding noise,
so the first test ranked a 5 Hz shift as farther than a whole different
vowel. Moving formants are what speech is.
