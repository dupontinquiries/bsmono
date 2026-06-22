# snapclone-composer

The decision model for the group chat **live composer**: a 3-up media feed
(prev / active / next) you scrub through over the last 24h, with voice-gated
playback so only one tile plays sound at a time.

## What it decides

`Composer::new(tiles, now_ms)` filters to the trailing 24h and sorts oldest→
newest. Then:
- `live_view(voice)` — newest tile active (the default "live" position).
- `scrub(base, steps, voice)` — move the active tile by `steps` detents
  (the gesture layer produces `steps`); clamps at the ends.
- `activate_id(id, voice)` — tap-to-replay a visible sub-tile; falls back to the
  live view if the id has aged out.

It returns a `ComposerView`: up to three `VisibleTile`s, each with a
`PlaybackState`:
- **active** → `PlayingWithAudio`
- **neighbors, no voice** → `PlayingMuted` (loop silently)
- **neighbors, active has voice** → `FrozenForVoice` (hold a still, muted) —
  this is the "one at a time" rule
- **any still image** → `Still` (never plays audio; a still can't gate voice)

## Why platform-free + tested

All the interacting rules — the rolling window, the 24h cutoff, voice gating,
scrub clamping, replay, stills-can't-have-voice — are pure state math. They live
here, unit-tested (17 tests), so the Qt group view is a thin renderer: feed it
tiles + scrub + voice, apply the returned playback states to QMediaPlayers.

## C / C++

Link `libsnapclone_composer.a`, include `snapclone_composer_ffi.h`. Fill a
caller-owned `ScVisibleTile[3]` via `sc_composer_view(...)` /
`sc_composer_activate_id(...)`. A linked C++ consumer has been run end-to-end.

## Layout (the "two small, one big" retiling)

`TileLayout` is a 2D tiling window manager (NOT a 3D solid). One BIG focused
pane + the other present slots as SMALL peripherals. `layout(slots, big_slot)`
returns pixel rects; `next_big(...)` is the re-tile step the rotate gesture
drives (promote a peripheral into focus). `auto_edge` picks a right-column
layout on wide viewports and a bottom-row layout on tall (phone) ones. Purpose:
keep context open while optimizing the viewport, and buffer the eyes when three
messages land at once. It is independent of the feed/voice logic.

## Status

28 tests; in-workspace `cargo test -p snapclone-composer` green; header verified
C and C++; staticlib linked + run from C++. Zero dependencies. NOT yet wired
into Qt — the group view widget, the gesture→steps mapping, and the
QMediaPlayer wiring are the next (device-checkable) step.

## Design (per the designer)

- Scroll = hybrid detent+momentum (snapclone-gesture).
- The three = active +/- 1 in time.
- Voice -> neighbors **dim + time-warp** (PlaybackState::Dimmed, 0.35x), not freeze.
- The "rotatable T stack" is a **2D retiling window manager**: two small + one
  big, rotate = promote a different tile to big. (Earlier prism model removed.)
