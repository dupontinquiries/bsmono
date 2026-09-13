# Producer workflows — what the footage taught (a guide for the next Claude)

This is the synthesis of one analysis session: five hours of screen
recording of the maintainer producing in FL Studio (two projects, trap),
an OCR scan of that recording, and the maintainer's own commentary on
what hurts. It exists so the next session working on `apps/studio` does
not have to re-derive what the workflow IS before deciding what the app
is missing. Everything marked *observed* was seen in frames or OCR;
everything marked *said* is the maintainer's words; everything marked
*inferred* is my reading and should be treated as a hypothesis.

Companion reading: [`apps/studio/README.md`](../../apps/studio/README.md)
for what the app already does, `apps/studio/crates/studio-core/src/curves.rs`,
`batch.rs` and `room.rs` for the three model additions this analysis produced.

## 1. The shape of a session

*Observed*, 5 h 01 m, one machine (Windows, FL Studio 24, a NAS mounted
as "My Cloud"):

| span | what was happening |
|---|---|
| 0–15 m | Browser open, sample kits (Cymatics, a `Vox` chop folder), Omnisphere patch browser. Choosing sounds. |
| 15–60 m | iZotope Neutron on mixer inserts, Fruity Limiter, sidechain via Peak Controller, Ozone on master. Mixing. Bounced to `.wav` at 53 m and `.mp3` at 59 m. |
| 60–195 m | Piano roll with Omnisphere / FLEX, velocity lane, 808 slides, vocal chops loaded as instruments. A `Downloads – <studio name>` folder opened at 79 m (vocals delivered as files). "Open in Edison". Project saved as `<name>_3.flp`. Playlist automation clips from ~185 m. |
| 195–245 m | One long audio take on the playlist with Neutron on it. Arrangement passes. |
| 245–300 m | **A second project from scratch**: kit picked from a pack browser, 808 Kick / Clap / HiHat / Snare into the channel rack, three Omnisphere channels, arrangement dense with ramp automation, Ozone maximizer, ascending note runs drawn in the roll at the very end. |

Rough time split across the five hours, *inferred* from the OCR term
profile: sound selection and per-sound mixing ≈ 35 %, note entry
(roll/rack) ≈ 30 %, arrangement + automation ≈ 20 %, mastering/export ≈
10 %, file wrangling ≈ 5 %.

The maintainer's own description of the order, *said*:

> a lot of wrangling and mixing sounds to get the feel right from source
> is key to creating a unique vibe, with even more patterning and notes to
> create the song taking even more time. then comes the arrangement. these
> usually hit first, then a recording bit, then some adaptation around it.

So the phases are: **source → vibe** (mix each sound as you pick it, not
later) → **pattern/notes** → **arrangement** → **recording** → **adaptation
around the recording**. The footage matches this; note that mixing
happens *early*, per sound, not as a final stage. An app that treats "mix"
as a phase after "arrange" has the order wrong for this user.

## 2. The trap idiom, concretely

*Observed* unless marked.

- **Kit shape.** Every project starts with four channels named `808 Kick`,
  `808 Clap`, `808 HiHat`, `808 Snare`, then melodic channels (three
  Omnisphere instances layered), then extras from a pack: `808 – Vibe`,
  `Chant – Short`, `Clap – Soul`, `Snare – Dirty`, `Stick – Archangel`,
  `Open Hat`, `Kick – Party`. A pack browser's categories were: *Ad Libs,
  Drum Kit ×4, Bass Heavy Sounds, Percussion, Chants, Claps, Hi Hats,
  Risers, Sticks*. A rack whose default is a synth demo is wrong for this
  user; the default is that kit.
- **Tempo.** 140 BPM is the default (the FL tutorial the maintainer pasted
  says so; the rack in the app already defaults there).
- **Hats** are the rhythmic interest: rolls (ratchets), triplet fills, open
  hat on the off-beat. The roll in the app has ratchet mods; **triplet
  snap does not exist** (verified by grep — no `triplet` in `roll.rs` or
  `grid.rs`).
- **808** is the bass; it *slides* (pitch glide between notes) and has a
  long decay; it is often sidechained under the kick. The app's `Sub`
  synth has glide/drive/decay.
- **Melody** is pads and plucks from a big preset synth (Omnisphere, FLEX),
  three instances layered, long notes with a velocity lane, and near the
  end of a session an ascending staircase run (an arpeggiated riser). None
  of those instruments will ever run on a phone; the honest native
  substitute is an SFZ/SF2 multisample kind plus CLAP hosting on desktop.
- **Arrangement gestures**: patterns painted onto the playlist, then *ramp
  automation clips* everywhere — filter sweeps and risers into drops,
  volume dips for the beat drop. The automation is what makes an
  arrangement sound finished. The app's automation targets were track and
  bus gain only; effect-parameter automation was the biggest single gap
  after the curves work — closed in c17 (`AutoTarget::BusParam` /
  `ClipParam`, `Instance::set_param`, rides at control rate).
- **Mixing tools**: an assistant-style EQ/dynamics per insert (Neutron:
  EQ, compressor, sculptor, masking), a limiter, **delay and reverb on
  sends**, sidechain ducking, stereo width, and a maximizer on master
  (Ozone). The app has EQ/comp/limiter/multiband/imager/saturation and
  a mix assistant — and, as of c17, reverb and delay (`studio-fx::space`;
  before that there were none, verified against the registered kinds,
  and the sends had nothing worth sending to).
- **Vocals** arrive three ways: as chops loaded like drums (`vox –
  breath.wav`), as files delivered from a recording studio (the Downloads
  folder), and as a long single take on the playlist that then gets
  Neutron, cut up, and moved around. "Ad-libs" are a category, not an
  afterthought.
- **Export loop**: a `.wav` or `.mp3` bounce four times in five hours,
  mid-session, not at the end — to listen on a phone, to send to the
  singer. Projects are versioned by hand (`_3.flp`). The app's git-per-save
  and archive already beat this; what it lacks is the phone share sheet
  and an `mp3/m4a` target (it does wav/opus).

## 3. The gestures that actually cost time (and what we did about them)

*Said*, in the maintainer's words, with the app's answer:

1. **"clip groups would be huge for time savings"** and **"being able to
   move a clip group quickly and without messing up the chopped up
   innards"**. A vocal take is chopped into many clips; the maintainer
   calls the result a *conglomeration*; it must move, paste and cut as one
   thing. → Compositions already existed; `batch.rs` made a drag on any
   member move the whole group (one delta, collision-free order — moving a
   packed run of clips one at a time collided with its own unmoved
   neighbour), paste recreates the composition over the copies, and
   split-on-grid keeps the pieces in it.
2. **"make unique … how to label or identify them"**. When something is
   shared (a curve driving twelve clips), the person must know it is shared
   *before* editing it, and must be able to break one instance off.
   → `curve_share_count` → "(shared ×N)" in the label; `make_curve_unique_ops`.
   This generalizes: anything shared needs a visible count and a
   "unique" verb. Patterns and sources are not shared-editable here, so
   curves were the only case, but check again when adding shared objects.
3. **"duplicating clips and having to duplicate automations"**. FL's
   automation belongs to the clock, not the clip. → `curves.rs`: a curve is
   a project object, a binding aims it at a target through a gate, and
   `Gate::Clip` makes the ride follow the clip through every move and paste.
   Bindings multiply into the lane so a no-binding project renders
   bit-identically.
4. **"long clips that fight a user offscreen (especially on mobile) is a
   bad experience"**. A three-minute take two screens to the right refuses
   every drop in front of it. The maintainer sketched "an overlap line…
   stuff behind it doesn't play and doesn't hurt… maybe auto-shadowed
   channels that get created and grouped for effect busses". → `room.rs`:
   the model still refuses (right convergence rule); the shell resolves per
   track before sending — Trim (newest wins, default), Lane (shadow track
   beside the parent with the same route), Refuse. The "overlap line" as a
   drawn, draggable edge is not built.
5. **"cut on the beat … tackles the mixing with bluetooth issue"**. Over a
   Bluetooth link the ear is a few hundred milliseconds late, so cutting by
   ear is wrong; cutting on the grid is right. → split on beats/bars from
   the palette, on the rack grid.
6. **Right-click a parameter → automate it** ("the knob is the ingress
   point", from the FL analysis the maintainer endorsed). → context menu on
   every mixer fader.

## 4. Collaboration, as the maintainer wants it

*Said*: the singer/rapper is remote and it must work for "small scale
producers get started" without extra hardware.

- **The call lives in the app by default**, over the same carrier the ops
  use; anyone with a Dante rig mutes it and routes their own. The
  maintainer knows about group-call work in `snapclone` and chose in-app
  anyway for the routing simplicity. → `Packet::Voice` + `voice::Jitter`
  in `studio-collab`; the shell halves are built now (c17):
  `studio-collab::voice::Link`, `studio-media::voice` (Opus frames),
  `studio-app::call` (the pump), ☎ / mute / hold-to-talk in the toolbar.
- **Presence must say who is recording** so nobody moves the singer's
  playhead mid-take. → `Presence.recording`, overlay label `name ● rec`.
- **Whisper transcripts** matter for a specific reason: "when someone wants
  to try a certain line in the song over a beat drop" — find the phrase in
  the take by its words, treat it as a group, move it to the drop. The
  transcript index exists; the take-lane/comping UI does not (verified: no
  loop-record, take lanes or comp anywhere).
- Vocals today arrive as files from a studio. Chunked assets over the
  wire already beat that; comping is the missing half.

## 5. Music video, as the maintainer wants it

Nothing is built. The two cheap wins, in order: sync camera footage by
audio (the existing `align` cross-correlation with a different source
pair) and render an mp4 through the ffmpeg edit list `studio-video`
already shells to. Camera capture on the phone is PipeWire/libcamera
through the portal. Realtime video playback is the expensive thing and is
not needed for either win.

## 6. How to analyze footage like this (method)

What worked, so it can be repeated on the next recording:

1. **Inventory first.** `ffprobe` the video (this one was 5 h, AV1, low
   variable fps); `unzip -l` the scan; read the OCR tool's source to learn
   the row format before parsing. Do not extract the per-frame cache.
2. **Profile terms per 15-minute bin** from the OCR TSV with a vocabulary
   of window titles, plugin names and labels. That gives the session's
   shape in one table. Beware chrome: "Render" was the browser's
   *Rendered* folder in 148 frames and a real export dialog in four.
3. **Find the blind spots.** The OCR went quiet after ~195 min because the
   capture went letterboxed (smaller text). Check `t_s` coverage and
   sample frames visually where the OCR fails.
4. **Sample frames, then zoom.** One frame every 15 min tiled 4×5 shows
   the arc in one image; 2×2 tiles at 960 px, letterbox-cropped, are
   readable enough to identify plugins, track names and rack channels.
   Targeted full-resolution crops only where a label matters.
5. **Verify every "the app lacks X" against the code, not the README**,
   before listing it (CLAUDE.md §5). Four of the gaps above came from one
   grep each; the README had been read as saying the opposite for one of
   them.
6. **Rank by the maintainer's words, not by frame counts.** The frame
   counts said "Neutron"; the maintainer's message said "clip groups".
   Both were right, but the order of work follows the second.

## 7. Vocabulary the maintainer uses

So the next session reads them the same way: *conglomeration* — a chopped
take that must behave as one thing; *make unique* — break a shared
instance off; *shadowed channels* — auto-created sibling tracks routed to
the same bus; *overlap line* — a visible edge that placements respect;
*adaptation* — the edits made around a recording after it lands;
*wrangling* — per-sound mixing during selection; *the vibe* — what the
first phase is for.

## 8. Open, in the order agreed

1. ~~Voice link shell halves~~ — built (c17): `studio-collab::voice::Link`,
   `studio-media::voice`, `studio-app::call`; the README's "The call"
   section. Unverified against a real sound card or microphone.
2. ~~Reverb and delay in `studio-fx`~~ — built (c17): `studio-fx::space`,
   all-wet on a return bus, "sync delay to tempo" in the palette. The
   README's "The sends finally have something to feed".
3. ~~Effect-parameter automation on top of `curves.rs`~~ — built (c17):
   `AutoTarget::BusParam`/`ClipParam`, `Instance::set_param` +
   `is_live_param`, `studio-audio`'s `ParamRide` at 64-frame control
   rate, "automate" on the fx editor's live controls. The README's
   "Riding a knob".
4. Synth + piano control-panel sketch, SFZ/SF2 kind behind it.
5. ~~Take lanes, loop-record, comping~~ — built (c17): `studio_core`'s
   takes module (`take_lane_ops`, `comp_ops`, `take_lanes_mute_ops`),
   `looping::{Window, Passes}` + `Capture::split_take` in the shell,
   palette verbs for loop / comp / mute lanes. The README's "The second
   take". Still open inside it: the transcript-driven "best take" score
   (needs whisper on the machine).
6. Video: align-by-audio, ffmpeg edit-list render.
7. `Gate::Group` (a curve over a composition's hull), wire bump.
8. Triplet snap, arpeggiate/chord tools, folder-drop → kit, phone share
   sheet with an mp3/m4a target, the drawn overlap line.
