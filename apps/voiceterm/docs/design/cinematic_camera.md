# Cinematic follow-camera: principles, not position-averaging

Status: **DESIGN + robust rewrite (v0.1.506).** Supersedes the
v0.1.505 faithful port. The user's one-phrase insight — "it's more
cinematic" — is the key that turns impakt-cam's accidental good feel
into a principled, robust algorithm.

## Why impakt-cam felt better than competitors (the real principle)

Competing camera-follow plugins do POSITION tracking: target = cursor,
ease the output to it. impakt-cam accidentally did something closer to
SIGNAL tracking: it averaged the cursor's target over a sliding TIME
WINDOW and continuously re-targeted the tween. The effect: the camera
responds to the recent TRAJECTORY of motion, not just the latest
point — so holding a movement key produces a fluid, slightly
anticipatory glide instead of a stuttering chase. That trajectory-
awareness is what reads as "cinematic."

## Why it collapsed (impakt's edge cases, read from the source)

1. **Reference-frame mixing (the spectacular collapse).** It stored
   `getBoundingClientRect().top` — SCREEN coords — in the time window.
   As the camera animates, those screen positions shift under it, so
   averaging points captured at different scroll states mixes
   incompatible frames. Fast scrolling → meaningless average → jump.
2. **Equal-weight average:** a 290ms-stale point counts as much as the
   current cursor → lag + overshoot on direction changes.
3. **Kill-and-restart tween every event** (`this.anim.tween.kill()`):
   the new animation starts from REST, discarding velocity continuity
   → micro-stutter on rapid input.
4. **setTimeout-per-point eviction:** unbounded timer churn, fuzzy
   wall-clock window membership.
5. DOM dependence forced the screen-coord mistake in the first place.

## The cinematic principles (what a real camera operator does)

A basic camera is a SECURITY camera: points at where the subject IS.
A cinematic camera:
  - **Lead room / look-ahead** — leads a moving subject using its
    VELOCITY, not just position. (impakt had position only.)
  - **Deadband / static framing** — doesn't re-frame for tiny subject
    movement; only moves when the subject drifts past a threshold.
    Kills the jitter-chase a naive camera suffers.
  - **Critically-damped motion** — a dolly accelerates/decelerates
    smoothly and NEVER overshoots; eases toward a MOVING target while
    preserving velocity continuity (fixes impakt's restart-from-rest).
  - **The cut** — when the subject leaves frame entirely (huge jump),
    you CUT, you don't dolly. (impakt's snap cutoff, but principled:
    a cut resets the smoothing state.)

## The robust model (v0.1.506)

A continuous control system in STABLE CONTENT COORDINATES (line-based,
not screen px) — so there is NO reference-frame-mixing collapse mode.

State: camera scroll position `cam`, camera velocity `vel`, and a
short history of (time, cursor_line) to estimate cursor VELOCITY.

Per tick (the caller drives it, like a native QML frame update — no
per-frame JS):
  1. Estimate cursor velocity from recent samples (lines/sec).
  2. Compute the LOOK-AHEAD target: cursor_center + lead_factor *
     velocity (lead room — frame ahead of motion), clamped.
  3. DEADBAND: if the target is within `deadband` of the current
     camera framing, hold (don't chase jitter).
  4. CRITICALLY-DAMPED step toward the target (a spring-damper with
     damping ratio 1.0 → no overshoot), advancing `cam` and `vel` by
     dt. Velocity continuity preserved across re-targets (the fix for
     impakt's stutter).
  5. CUT: if the cursor jumped beyond `cut_threshold` lines, snap
     `cam` to centered, zero `vel`, clear history (a cut, not a dolly).

Robustness falls out of the principles: a velocity-led, critically-
damped, deadbanded controller tracking a moving target in content
coords cannot collapse — there's no averaged-snapshot step that can
mix frames, no kill-restart, no unbounded timers.

## Dependencies (the user's other concern)

impakt pulled ALL of GSAP (gsap/TweenMax/TweenLite/Tween/Elastic/
Power1) + a debounce lib to use ONE tween + ONE easing + a trivial
debounce. Our version: ZERO dependencies. The damped spring is ~15
lines of math; the QML side eases nothing extra — it just binds
contentY to the camera position the Rust controller advances (or,
simpler, QML's native Behavior eases to the per-tick target). No GSAP,
no DOM, no JS per frame.

## Context-awareness for THIS codebase

  - Works in LINE/content coordinates the grid already exposes
    (cursor_row, line height, rows-per-page) — not DOM screen rects.
  - The "cut" pairs with the existing big-content-change paths
    (scrollback jumps, alt-screen enter) which are already
    discontinuities — a cut there is correct, not a glitch.
  - Pairs with the v0.1.504 helix-edit→re-render: the camera leads
    toward the edited/active section.
  - Caller-driven tick = maps onto a native QML frame
    (FrameAnimation / a Timer at display rate) advancing one f32 — the
    no-JS-overhead path.
