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
3. **egui frontend** on `uikit-egui`: timeline view, waveform, transport, and
   peer playheads drawn from `Doc::peers` (the collaboration UI is the point).
4. **Transport** — the op packets are plain serde; snapclone's `snapclone-dht`
   already carries signed records between peers and is the obvious carrier.
5. **Block-backed sources** — Audacity's insight that samples live in chunks so
   an edit touches a few blocks, not a file.

A test found the one real bug in this pass: `rebuild()` originally replayed the
op log from an empty project and erased every track and source that pre-dated
the session. A `Doc` now remembers the project it opened.
