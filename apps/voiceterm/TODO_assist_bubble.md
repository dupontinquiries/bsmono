# TODO: assist-bubble surfacing primitive (build BEFORE the active assists)

## Why this is first
User critique (round 2): the sidebar + missing-packages affordance feel
"thrown in because there was space." That's a design-principle failure:
surfaces placed by available real-estate, not by workflow intent. The
active-assist ideas (R3 slowness bubbles, debug→gdb picker, stuck-cadence
nudge) will each repeat that mistake if built as separate UI. So build ONE
shared surfacing channel first.

## The principle
A surface must earn its place by a workflow MOMENT ("you're about to need
X" / "you appear stuck" / "this is slow"), not by filling empty UI.
Corollary: active assists are TRANSIENT, TRIGGERED by detected state, and
DISMISSABLE — never permanent furniture.

## What to build
One "assist bubble" primitive:
- a single transient overlay channel (not the sidebar), bottom/corner,
  auto-dismiss + manual dismiss, max 1-2 at once, rate-limited so it never
  nags (reuse the throttle discipline from the pitch-hint log fix).
- a typed trigger model: each assist registers (trigger predicate, content
  builder, action). Triggers fire on DETECTED STATE only.
- a "snooze / don't show this kind" affordance so it can't become noise.
- consistent with the existing toast, but richer (can carry an action +
  voice-pickable options), and explicitly NOT the cafe-quote ambient style
  unless idle.

## Audit task (do alongside)
Re-justify the sidebar + missing-packages affordance against "what task is
the user in when this should appear?" If no answer → make it non-ambient
(on-demand) or remove. This is the concrete fix for the round-2 critique.

## Status: design-only. Prereq for R3/debug-picker/stuck-nudge.
EOF

## DONE v0.1.662 — scheduler core (the "never nags" guarantee), pure + tested
src/assist_bubble.rs (7 tests): BubbleScheduler.offer(kind, now) → Show /
Suppress{Snoozed|PerKindCooldown|GlobalCooldown|MaxConcurrent}, + dismiss /
snooze / unsnooze. Conservative defaults: 1 concurrent, 5-min per-kind
cooldown, 20-s global floor.
- Proven on the real scenario: Hector's 6 rapid reboots collapse to 3
  non-nagging reentry bubbles (closely-spaced ones suppressed by per-kind
  cooldown). This is the pure guarantee that the channel can't become the
  "thrown in because there was space" furniture.

## STILL TO WIRE (build-gated, QML)
1. The transient overlay: bottom/corner, auto-dismiss timer + manual close,
   carries a title + action + (optional) voice-pickable options. Consistent
   with toast but richer. NOT ambient/cafe-style unless idle.
2. Hold a BubbleScheduler in Backend; route candidate assists through
   offer() before showing; dismiss() on close; snooze() on "don't show this
   kind".
3. First consumers: session_auto proposals (NewTabForFile / NewSession /
   ReentryAfterReboot) + recall re-entry. Each is a `kind`.
4. AUDIT (still open): re-justify sidebar + missing-packages against "what
   task is the user in when this should appear?" — make non-ambient or
   remove if there's no answer.

## WIRED v0.1.666 — assist-bubble channel live + debug_detect first consumer
End-to-end (Rust compile-verified; QML written to the workflow-toast pattern,
qml_lint clean):
- Backend holds a BubbleScheduler (with_defaults). Signal assist_bubble_ready
  (kind, title, options-wire) + qt_methods assist_pick / assist_dismiss /
  assist_snooze.
- debug_detect wired into pty_read_screen's output drain: new output is
  stashed (_debug_scan_pending) inside the pty borrow and scanned AFTER it
  ends (scan_for_debug_signals, &mut self). First recognized crash/panic/
  oops/debugger signal routes its bubble_kind through scheduler.offer(); on
  Show, builds gdb/monitor options from the last command and fires
  assist_bubble_ready.
- QML assistBubble overlay (bottom-right, z=502, 12s auto-dismiss): title +
  per-option buttons (assist_pick → types command, no auto-run) + close
  (assist_dismiss) + "don't show this again" (assist_snooze).
- Compile gate caught the borrow error (scan needs &mut self inside the &pty
  borrow) → fixed via deferred-scan-after-borrow.

### Verified vs build-gated
Rust + qt_struct + qml_lint + harness all green. RUNTIME (does the bubble
render/position right, does pick type the command, does the 12s dwell feel
right) needs the user's aarch64 build.

### Remaining consumers to wire (now that the channel exists)
- slowness_probe: needs a /proc sampling tick on a slow op → offer slow:*
  bubble. (sampling loop = new plumbing)
- session_auto: needs the real Activity stream (cwd/editor/reboot events) →
  offer NewTab/NewSession/Reentry. (event plumbing)
- AUDIT still open: sidebar + missing-packages re-justification.
