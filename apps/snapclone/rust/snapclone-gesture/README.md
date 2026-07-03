# snapclone-gesture

A reusable, platform-free **two-tier scroll/swipe gesture model**:

* **Small sustained scroll past a limit → a one-shot sub-page detent.** Keep
  nudging at the end of a scroll area; once accumulated motion crosses a
  threshold it advances ONE sub-page. A sustained scroll won't rip through
  several — a short post-fire lock gates re-fires.
* **Big fast swipe → a whole-page jump.** A flick (far enough AND fast enough),
  or a drag past a fraction of the page, commits a page move.

This is the gesture feel from SnapClone's navigator, extracted so other apps can
reuse it. It is **pure scalar/time logic** — no UI toolkit, no platform calls.

## What it does vs. what your app does

The module owns the part that's fiddly to get right:

* the detent accumulator with a **wall-clock rate budget**, so a 240Hz trackpad
  and a 60Hz mouse reach the threshold in the same real time (not 4× apart);
* an **arm window** that re-arms a fresh streak rather than dying;
* a **fire-once-per-streak** latch + **takeover lock** that stop one long scroll
  from advancing many sub-pages;
* the **distance + velocity** test that distinguishes a page-jump flick from a
  small drag, scaled to page size so it feels identical on phone and desktop.

Your app keeps the toolkit-specific bits: reading wheel/touch deltas, deciding
whether the inner view is at its scroll limit, rendering the progress "barrier",
and actually moving to the next page / sub-page.

## Rust

```rust
use snapclone_gesture::{TwoTierGesture, GestureConfig, Outcome, ReleaseOutcome};

let mut g = TwoTierGesture::new(GestureConfig::for_page_size(page_w));

// per at-the-limit scroll sample (signed delta, monotonic ms):
match g.on_scroll(delta, now_ms) {
    Outcome::DetentFired { dir }      => advance_subpage(dir.sign()),
    Outcome::DetentProgress { fraction, edge } => render_barrier(fraction, edge),
    Outcome::None => {}
}
// when the inner view is NOT at its limit (normal scrolling):
g.reset_streak();
// on release:
if let ReleaseOutcome::PageJump { dir } = g.on_release(vel_px_ms, drag_px) {
    jump_page(dir.sign());
}
```

Defaults mirror SnapClone's tuned constants (detent threshold 220px, arm window
1000ms, rate 0.75 px/ms, commit fraction 0.22, flick 0.30 px/ms & 14px). Override
any of them via `GestureConfig`.

## C / C++ (any app)

Link `libsnapclone_gesture.a` and include `snapclone_gesture_ffi.h`:

```c
ScGesture *g = sc_gesture_new(page_w);
double frac; int dir;
int o = sc_gesture_on_scroll(g, delta, now_ms, &frac, &dir);
if (o == SC_GESTURE_DETENT_FIRED)        advance_subpage(dir);
else if (o == SC_GESTURE_DETENT_PROGRESS) render_barrier(frac, dir);
/* ... */
int rdir; if (sc_gesture_on_release(g, vel, drag, &rdir) == SC_GESTURE_PAGE_JUMP)
    jump_page(rdir);
sc_gesture_free(g);
```

The FFI is panic-safe and null-tolerant. One `ScGesture` per gesture surface.

## Status / honesty

* **Logic is fully unit-tested** (15 tests: detent firing, runaway prevention,
  240Hz-vs-60Hz parity, arm-window re-arm, direction, page-jump by flick and by
  distance, page-size scaling, FFI lifecycle + null-safety). The release
  staticlib builds and the header compiles clean as C and C++.
* **The feel is validated by hand, not by the tests.** The tests prove the
  decisions are correct and stable; whether the *specific thresholds* feel as
  good in another app depends on that app's input scaling. Start with the
  defaults and tune via `GestureConfig` / `sc_gesture_new_config`.
* This crate has **no dependencies**, so it drops cleanly into another project:
  copy `rust/snapclone-gesture/` and (for C/C++) the header. Within the SnapClone
  workspace it must be built individually (`cargo build -p snapclone-gesture`)
  because of a pre-existing toolchain drift in unrelated sibling crates; on its
  own it builds with any recent stable Rust.
* **Not yet wired back into SnapClone itself.** The app's SwipeNavigator still
  has its own (now-tuned) inline copy of this logic; migrating it to call this
  crate is a separate, behavior-preserving refactor.
