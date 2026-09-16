# blackstar-demo

A small, testable slice: one car, one fold, one drive model, one walk.

    cargo test      # 18 tests, ~0.01s, no GPU
    cargo run       # watch a fold happen in the terminal

## Why it exists

`blackstar-rs/src/main.rs` is 9,937 lines across a 191-file crate. That is
navigable if you wrote it last week and hostile if you did not, and the
practical symptom is that a change cannot be checked without launching the
whole game — window, GPU, audio, SDF bake, 268 chassis. Every edit stays a
guess until it is on screen.

Everything here is plain data with **zero dependencies**. The whole crate
builds and tests in under a second.

| | |
|---|---|
| `Shape` | hull as control sections; geometry derived, never stored |
| `Fold` | vehicle ↔ robot as a state machine over `Ms` |
| `Drive` | bicycle model, speed-dependent steering authority |
| `Walk` | a gait where **the stance foot does not move** |

No `Instant::now()` anywhere. Every entry point takes a timestamp, so a
session is a fold over `Ms` and a test is a list of numbers.

**The rule that keeps it small: nothing in `lib.rs` may import a GPU type.**
If a feature cannot be expressed without one, it belongs in the shell.

## The tests are the specification

They assert design claims, not implementation:

* `the_stance_foot_does_not_move` — the one that matters. The exengine walk
  slid its parts: legs animated while feet travelled with the body, so nothing
  ever pushed against anything. Here the stance foot is pinned in world space
  and the body is moved by how far that foot is behind the hip. If this test
  ever fails, the walk has become an animation again.
* `steering_authority_falls_with_speed` — turning as sharply at 26 m/s as at 2
  is what makes arcade driving feel like sliding a sprite.
* `a_fold_cannot_be_interrupted` — every downstream system would need to cope
  with an arbitrary `t`, and none does.
* `fold_is_frame_rate_independent` — the same elapsed time gives the same `t`
  in one tick or forty.
* `the_waist_is_real` — MOIRE's hull pinches behind the front axle and flares
  over the rear. Smooth that out and it is a loaf; nothing else would notice.

## What it already caught

Writing the tests found a real disagreement immediately. `width()` measured
the hull sections (1.98 m) while the design note said 1.90, which came from a
render that included the wheels (2.11). Both honest measurements of different
things — and chasing it surfaced `TYRE_HALF_THICK`, a constant that lived
implicitly in two places and agreed by luck.

A 7% width error is exactly the size nobody notices by eye.

## What this is not

Not a replacement for `blackstar-rs`. It is the shape the important parts of
it could have, extracted small enough to iterate on. The viewer is a
`println!` loop on purpose — when this grows a window, the window goes in
`main.rs` and `lib.rs` stays as it is.
