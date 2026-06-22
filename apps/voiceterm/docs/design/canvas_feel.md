# Canvas feel: flick physics, looser bounds, north detent (v0.1.717)

Three dogfood complaints about the graph/card canvas views, all addressed.

## 1. Touch-drag friction / flick momentum
- cardsFlick: boundsBehavior DragOverBounds → **DragAndOvershootBounds** so a
  FLICK overshoots the edge and rubber-bands back (before, fast flicks stopped
  dead at the boundary). Added a `rebound` Transition (OutBack spring-back).
  flickDeceleration 1800 → **1200** (longer coast), maxFlickVelocity 5000 →
  8000.
- The custom background-pan glide (bgPanLayer/_glideTimer): friction per tick
  0.92 → **0.96** (a 0.92^60≈0.7%/s decay died in ~0.3s; 0.96 coasts like a
  real touch canvas). The glide now integrates INTO an overscroll zone and
  springs back instead of killing velocity dead at the edge.

## 2. Cramped fit + restrictive bounds
- Reachable margins 80px → **160px (right/bottom) + 60px (top/left)**.
- Rubber-band overscroll zone (`_overscroll` 220px): both drag (`_rubber`,
  asymptotic resistance) and glide may travel past an edge, then spring back —
  so the canvas feels elastic, not walled.
- Fit-to-view: was capped at 1.0 (a tiny graph stranded small in a big
  viewport). Now uses backend `canvas_fit_scale` — 4% padding (was 8%) and a
  **1.5× zoom-in cap**, floored at 0.15 for huge graphs.

## 3. Detent to north (the long-deferred one)
The PinchHandler already had a scale detent (snap to 1.0 within 8%) with a TODO
"pan/tilt detents can follow." Now wired: on pinch release, the graph's rotation
snaps to the nearest cardinal (north emphasized) when within 12°, via the tested
backend `canvas_detent_snap`, animated by `_northRotAnim` with OutBack overshoot
(the magnetic "notch" feel). `_snapToNorth` also flattens rotation to 0.

## Pure core (tested) — src/canvas_physics.rs (7 tests)
- `detent_snap(angle, detents, threshold)` — magnetic snap, north always a
  detent, wrap-around aware.
- `fit_scale(gw,gh,vw,vh,pad,min,max)` — looser fit + zoom-in cap.
- `flick_target/flick_duration` — kinematic flick projection (available for
  future native-flick replacement; QML currently uses Flickable + the glide).
- `overscroll_clamp` — rubber-band bounds.
qt_methods canvas_detent_snap + canvas_fit_scale call these ONCE per gesture
(release / fit), never per frame — keeps IPC off the hot path.

## Status
canvas_physics 7 tests; harness 1364 + 28; qt_struct 527; qml_lint clean;
compile green. The QML gesture wiring is RUNTIME-UNTESTED (no GUI/touch in
sandbox) — dogfood confirms the feel; the math is unit-tested.

## v0.1.718 dogfood tuning
- TOP/RIGHT fit: leading margins (top/left) were 60px while trailing were 160 —
  the graph's top/right edges couldn't clear the screen because _panMin lacked
  headroom. Now 160px on ALL four sides (symmetric), so every edge clears.
- "Small swipe → huge leap back": the release velocity was raw delta/dt, which
  spiked when two move events fired ~1ms apart (tiny delta / tiny dt = enormous
  velocity). Fixed the impact-velocity ramping: ignore sub-8ms samples (dt
  floor), exponential-smooth the estimate (0.5 EMA so one spiky frame can't
  dominate), hard-cap at 2.2 px/ms, and zero stale velocity if the pointer
  paused >60ms before release (hold-then-lift no longer flings).
