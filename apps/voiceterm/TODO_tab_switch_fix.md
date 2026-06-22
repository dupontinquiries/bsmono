# Tab switching (Browse ↔ Terminal SwipeView) — fix + next levers

## User symptoms (v0.1.616 dogfood)
- Swipe to terminal gets stuck halfway; finishing takes a couple tries, finnicky.
- Can't go back to web (one-way trip into the terminal).
- Terminal vertical scroll fighting for focus.
- "UI updates but then resets in the tab section, very quick / imperceptible."
- Log: `SwipeView.qml:16 Binding loop detected for property "currentIndex"`
  (this warning is COSMETIC — not the cause of the broken switching).

## Root cause (fixed v0.1.617)
`qml/main.qml` line ~2379 was:
    interactive: tabBar.currentIndex !== 1
This disabled the SwipeView swipe gesture whenever the terminal (index 1)
was showing:
  - On the terminal, interactive=false → swipe back to web impossible.
  - Mid-swipe toward terminal, as currentIndex crossed to 1, interactive
    flipped false MID-GESTURE → swipe aborted, snapped back to origin =
    "stuck halfway" + "UI updates then resets fast."
Fix: `interactive: true` (swipe both directions, never flips mid-gesture).
SwipeView + vertically-scrolling child is the standard Qt pattern.

## VERIFICATION STATUS: needs the running build
This is QML (can't compile/run in the sandbox) and a gesture-timing issue.
The change is minimal + well-reasoned, but CONFIRM on-device:
  1. From web, swipe left → lands on terminal cleanly (one try).
  2. From terminal, swipe right → returns to web (previously impossible).
  3. No accidental swipe-off when interacting with the terminal.

## If the scroll/focus fight PERSISTS after this (next levers — do ONE, then rebuild)
Only the `interactive` change shipped this turn (no stacking). If vertical
terminal scroll still fights the horizontal swipe:

A. Terminal focus-grab race. The terminal page (line ~3129) has
   `focus: tabBar.currentIndex === 1` and a `_termFocusTimer` (~50ms) that
   force-grabs focus when the terminal becomes active. If a swipe-back
   starts and the timer re-grabs focus to the terminal, it can fight the
   gesture. Lever: gate the force-focus so it doesn't fire while a swipe
   is in progress (SwipeView has `moving`/`dragging` — skip focus grab
   when `swipeView.moving`).

B. The `_syncingTab` guard race (lines 2402-2427). The guard is cleared
   SYNCHRONOUSLY, but SwipeView's internal currentIndex→ListView
   propagation can be deferred, so a late internal fire sees the guard
   already false. If "resets" still happens, try clearing the guard on a
   Qt.callLater instead of inline, so it spans the deferred propagation.

C. Binding-loop warning cleanup (cosmetic, do last). The warning persists
   because imperative assignments still re-enter SwipeView's internal
   ListView binding. Harmless; only worth touching once switching feels
   right, to de-spam the log.

## Why not all at once
Per standing directive: never blind-edit unverifiable QML, and don't stack
multiple untested changes in the same hot path before a user build. The
`interactive` line is the single dominant cause of the reported symptoms;
ship + confirm it before touching A/B/C.

### Lever A — exact anchors (confirmed v0.1.618 read-through)
The focus-grab lives at qml/main.qml:
- 3156: `focus: tabBar.currentIndex === 1` (FocusScope termFocusScope)
- 3157: `onActiveFocusChanged: { if (activeFocus) termKeyArea.forceActiveFocus() }`
- 3159-3166: Connections on tabBar → `_termFocusTimer.restart()` when index==1
- 3167-3174: Connections on swipeView → same
- 3175-3180: `_termFocusTimer` (50ms) → `termKeyArea.forceActiveFocus()`

If scroll-vs-swipe fight persists after v0.1.617, the precise one-line
guard to try (do ONLY this, then rebuild): in the _termFocusTimer
onTriggered (3178), skip the grab while a swipe is animating:
    onTriggered: {
        if (tabBar.currentIndex === 1 && !swipeView.moving)
            termKeyArea.forceActiveFocus()
    }
Rationale: a swipe-back that starts right as the terminal becomes active
can be fighting the 50ms timer re-grabbing focus to the terminal mid-
gesture. `swipeView.moving` is true during the animated swipe; gating on
it lets the gesture finish before focus is forced. Single, reversible,
low-blast-radius. STILL needs the running build to confirm — gesture
timing is not sandbox-observable.
