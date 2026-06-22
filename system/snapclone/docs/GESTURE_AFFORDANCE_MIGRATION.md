# GESTURE_AFFORDANCE_MIGRATION.md

**Status:** design / not yet implemented
**Filed:** V32.33 session, 2026-05-16
**Scope:** app-wide UI architecture change, multi-phase
**Estimated effort:** 10+ turns across multiple sessions, widget-by-widget rollout

---

## Why this exists

This doc captures the gesture-affordance architecture direction so the
desired behaviors aren't lost before implementation begins. It is NOT a
spec — it's a migration plan. The first phase (infrastructure only) is
small. The full rollout (every interactive widget showing
consider-state) is genuinely a multi-month project.

The trigger: in V32.33 the SwipeNavigator was rewritten from a hybrid
gesture handler (TouchBegin makes partial decision, TouchUpdate fills
in the rest with reclassification) to a top-down dispatcher (TouchBegin
classifies once, TouchUpdate dispatches without reclassifying). This
fixed the architectural problem of mid-stroke flips but introduced a
UX wart: **a horizontal flick inside a vertical-scrolling area no
longer triggers a page swipe — it gets swallowed silently because the
gesture committed to ChildScrollV at TouchBegin.**

The right fix isn't to add an escape hatch (that's the hybrid pattern
again). The right fix is to introduce a THIRD gesture state —
"Ambiguous" — that sits between "touch arrived" and "gesture committed
to an intent." During Ambiguous, the UI hints at what's possible
without committing any behavior. The gesture resolves based on motion
crossing thresholds.

This requires more than just a gesture-state-machine change. It
requires every interactive widget in the app to understand a
"consider" visual state distinct from "active" — currently widgets
only have idle and active, so on mobile they flash briefly when
touched but never show "we see you, here's what would happen if you
move." That latter state is the affordance hint.

This doc plans the migration.

---

## The new gesture state model

```
                ┌──────────────────┐
                │      Idle        │
                └────────┬─────────┘
                         │ TouchBegin
                         ▼
                ┌──────────────────┐
                │    Ambiguous     │ ◄── UI shows consider hints here
                │ (no commits yet) │     scrollbars visible, button
                └────────┬─────────┘     glow, page-peek edges, etc.
                         │
       ┌─────────────────┼─────────────────┬─────────────────┐
       │ vert motion     │ horiz motion    │ release w/ no    │ timeout
       │ > threshold     │ > threshold     │ motion           │ + no motion
       ▼                 ▼                 ▼                  ▼
┌─────────────┐  ┌──────────────┐  ┌─────────────┐  ┌─────────────────┐
│ChildScrollV │  │  PageSwipe   │  │  TapCommit  │  │ HoldState (TBD) │
│ scrolling   │  │ swiping      │  │ click fires │  │ context menu?   │
└─────────────┘  └──────────────┘  └─────────────┘  └─────────────────┘
       │                 │                 │                  │
       └─────────────────┴─────────────────┴──────────────────┘
                         │ TouchEnd / TouchCancel
                         ▼
                ┌──────────────────┐
                │      Idle        │
                └──────────────────┘
```

### Resolution rules

From Ambiguous, the gesture resolves to a concrete intent when:

- **Vertical motion > kLockThresholdPx (currently 14px)** → ChildScrollV
  (if a scrollable ancestor exists) or stay Ambiguous (if no scrollable
  ancestor — vertical motion in non-scrollable areas is currently
  meaningless; could be reserved for future use like swipe-to-dismiss)
- **Horizontal motion > kLockThresholdPx** → PageSwipe (always —
  regardless of whether we're inside a scroll area, horizontal motion
  past threshold is always interpreted as page-swipe intent)
- **TouchEnd with total motion < kTapMaxMovementPx (~8-12px)** →
  TapCommit (fire click on the widget under the original touch)
- **Timeout** (Ambiguous lasted longer than kAmbiguousTimeoutMs without
  motion, currently TBD — maybe 400ms) → HoldState (long-press; mostly
  unused today but available for context-menu type features)

### What "no commits" means in Ambiguous

During Ambiguous, the gesture system:

- **Does NOT** route touch events to children
- **Does NOT** start scrolling
- **Does NOT** start page-swipe drag
- **Does NOT** fire button clicks (those fire on TapCommit only)
- **Does NOT** show "pressed" state on buttons (that's TapCommit territory)

The gesture system:

- **DOES** track the widget under the original touch
- **DOES** emit an affordance signal so widgets can show consider state
- **DOES** prepare scrollbars to be visible
- **DOES** allow page-edge peek animations to start
- **DOES** track motion to decide which resolution branch to take

### Key design choice: horizontal motion wins regardless of scroll context

This is the explicit design decision that makes the new architecture
better than the rewrite that shipped in V32.33:

> If horizontal motion crosses the threshold, the gesture resolves to
> PageSwipe — even if the touch started inside a vertical-scrolling
> area like the Settings page.

This is the inverse of the V32.33 rewrite's swallow-horizontal-motion
behavior. The rationale: a user who flicks horizontally INSIDE a
scrollable area is clearly trying to navigate pages, not scroll the
content. Page-swipe is the correct interpretation.

The Ambiguous state makes this safe to do because BEFORE the motion
crosses threshold, nothing has been committed — no scroll has started,
no swipe has started. The user sees affordance hints but no behavior.
When motion finally crosses threshold, the system commits cleanly to
whichever direction won.

This is the architectural win that the hybrid pattern couldn't deliver
(because it had to commit too early to make the system functional).

---

## Phase 1: Gesture infrastructure (deliverable)

**Goal:** add the Ambiguous state to SwipeNavigator without breaking
existing behavior. No widget changes yet.

### Changes required

**SwipeNavigator.hpp:**

- Add `GestureIntent::Ambiguous` to the existing enum
- Add `QTimer *m_ambiguousTimer` member for the resolution timeout
- Add affordance signal:
  ```cpp
  signals:
      /// Fired when a gesture enters Ambiguous state. focusWidget is
      /// the widget under the original touch (typically a leaf widget).
      /// Widgets that want to show consider-state animations subscribe
      /// to this. Currently no widgets are subscribed — Phase 2 wires
      /// the first one up.
      void gestureAffordanceShown(QWidget *focusWidget, const QPoint &pos);

      /// Fired when a previously-Ambiguous gesture commits to a
      /// concrete intent. focusWidget receives this so it can transition
      /// from consider-state to either idle (intent != tap) or active
      /// (intent == tap).
      void gestureAffordanceResolved(QWidget *focusWidget,
                                     GestureIntent finalIntent);

      /// Fired when an Ambiguous gesture ends without crossing motion
      /// thresholds AND without being a tap — e.g. user lifted finger
      /// after holding still for too long. focusWidget can return to
      /// idle without animating an active state.
      void gestureAffordanceCancelled(QWidget *focusWidget);
  ```
- Add constants:
  ```cpp
  static constexpr int kAmbiguousTimeoutMs   = 400;  // Hold → HoldState
  static constexpr int kTapMaxMovementPx     = 10;   // Sub-threshold = tap
  ```

**SwipeNavigator.cpp event() rewrite (Phase 1 version):**

- TouchBegin: classify intent → if PassToChild, immediate ignore (no
  change). If ChildScrollV/H or PageSwipe, transition to Ambiguous
  state INSTEAD of immediately starting the scroll/swipe machinery.
  Emit `gestureAffordanceShown`. Start ambiguousTimer.
- TouchUpdate: if in Ambiguous, check motion against thresholds:
  - `qAbs(dx) >= kLockThresholdPx` → resolve to PageSwipe. Emit
    `gestureAffordanceResolved(focus, PageSwipe)`. Call
    `gestureBegin(touchStart)` then `gestureUpdate(cur)` to retroactively
    apply the motion so far. Stop ambiguousTimer.
  - `qAbs(dy) >= kLockThresholdPx` AND scrollable parent exists →
    resolve to ChildScrollV. Emit
    `gestureAffordanceResolved(focus, ChildScrollV)`. Set up the
    scroll-target state and start delivering scroll updates.
  - Below thresholds: stay Ambiguous, accept event, do nothing visible.
- TouchEnd:
  - If still Ambiguous AND motion < kTapMaxMovementPx → TapCommit.
    Forward synthesized click to focusWidget. Emit
    `gestureAffordanceResolved(focus, TapCommit)` so widget shows brief
    pressed-state animation before returning to idle.
  - If still Ambiguous AND motion ≥ kTapMaxMovementPx but neither axis
    crossed threshold → cancel. Emit `gestureAffordanceCancelled`.
  - If resolved (ChildScrollV / PageSwipe) → existing TouchEnd path
    handles it.
- AmbiguousTimer fires → HoldState (Phase 1: just emit
  `gestureAffordanceCancelled` and reset; long-press handling is Phase
  4+ territory).

**Crucially:** no widgets are subscribed to the signals in Phase 1.
This phase is wire-up only. App behavior should be IDENTICAL to the
V32.33 rewrite, just with a small (≤ kLockThresholdPx) motion grace
period before scrolling/swiping engages.

### Test plan for Phase 1

- Scrolling on Settings page: should work identically (user moves
  finger 14+px vertically, scroll engages).
- Tapping buttons: should work identically (user lifts finger after
  <10px motion, click fires).
- Page-swipe between Inbox/Camera/Friends: should work identically.
  ALSO: horizontal flick INSIDE Settings should now trigger page-swipe
  (this is the new behavior — confirms Ambiguous → PageSwipe path).
- No visible affordance changes (no widgets subscribed yet).

### What Phase 1 does NOT do

- No "consider" visual states on any widget
- No scrollbar pre-show
- No page-edge peek
- No long-press handling
- No haptic feedback

These are deferred to later phases.

---

## Phase 2: First widget responds (validation)

**Goal:** wire up ONE widget type — buttons — to show consider-state
during Ambiguous. Validate the design before app-wide rollout.

### Why buttons first

Buttons are the simplest interactive widget. They have a small set of
visual states (idle, hover, pressed, disabled), they're used
everywhere so the impact is visible, and their consider-state design
is uncontroversial — most platforms use a subtle background tint or
shadow to indicate "you're touching me, release to activate."

### Changes required

**Create `SnapButton` widget** (or extend existing button styling):

- Subscribes to SwipeNavigator's `gestureAffordanceShown` signal
- When `focusWidget == this`: transitions to "consider" visual state
  (subtle background tint, perhaps a slight scale-up or glow)
- When `gestureAffordanceResolved(this, TapCommit)`: shows brief
  pressed-state animation, then idle
- When `gestureAffordanceResolved(this, ChildScrollV)` or
  `gestureAffordanceResolved(this, PageSwipe)`: returns to idle
  without animation (user is doing something else, not pressing us)
- When `gestureAffordanceCancelled(this)`: returns to idle

**Apply selectively:** start by replacing buttons on the Settings page
only. This is a contained area for validation without app-wide
disruption.

### Test plan for Phase 2

- Touch a SnapButton without moving — should see consider tint, then
  pressed flash on release, then idle.
- Touch a SnapButton and slide finger vertically into the page — should
  see consider tint briefly, then it should disappear as the gesture
  resolves to ChildScrollV.
- Touch a SnapButton and slide horizontally — should see consider tint
  briefly, then it should disappear as the gesture resolves to
  PageSwipe.
- Non-SnapButton widgets should behave as before (no consider state).

### Open design questions for Phase 2

- How fast does consider-state appear? Immediate? After a small
  delay (50ms)? Too fast feels jumpy; too slow misses the point.
- How prominent is the consider tint? Subtle (5% opacity) or visible
  (15-20%)? Subtle is more elegant but easier to miss; prominent is
  clearer but can feel heavy.
- Does the consider tint use color, or shape (border, shadow)? Or
  both? Color shows on dark themes; shape/shadow shows regardless.

These questions get answered by trying things on the phone and seeing
what feels right. Don't pre-decide them.

---

## Phase 3: List rows and scrollable content

**Goal:** scrollable lists show that they CAN scroll during Ambiguous.

### Changes required

- Lists (inbox, friends, contacts) become aware of Ambiguous state
- When a touch lands on a list, during Ambiguous: scrollbars become
  visible (currently auto-hidden until scroll starts); the first row
  under the touch optionally shows a subtle highlight
- When Ambiguous resolves to ChildScrollV: scrollbars stay visible,
  highlight clears, normal scroll proceeds
- When Ambiguous resolves to TapCommit: highlight goes to "pressed"
  state briefly, then click fires on the row, then idle

### Why list rows are tricky

List items are usually clickable (open snap, open contact, etc.). The
naive thing is to treat them like buttons. But list items are also
scroll handles — touching one and dragging vertically is how the user
scrolls. The Ambiguous state has to balance these two intents.

The right model: list items show consider state DURING Ambiguous. If
the gesture resolves to TapCommit (no motion past threshold), the row
gets pressed-state + click fires. If gesture resolves to ChildScrollV,
the row's consider-state clears and scrolling proceeds.

The user experience this targets: "I touch a row, see it light up
slightly so I know I've selected it. If I let go, it opens. If I
start dragging, the row stops being highlighted and the list scrolls
instead."

### Open design questions for Phase 3

- What about lists where rows are tall? If a row takes 100% of the
  visible area, the user might not realize they could be scrolling.
  Maybe make the scrollbar always-visible during Ambiguous regardless?
- How do nested scrollables work? A scroll area inside a scroll area
  (rare but possible). The inner one should claim the Ambiguous focus,
  the outer one shouldn't react.

---

## Phase 4: Page-swipe affordances

**Goal:** users see that page-swipe is possible during Ambiguous.

### Changes required

- During Ambiguous, edges of the current page show a faint peek of the
  next/previous page
- Direction-dependent: only show left-edge peek if there's a page to
  the left, only show right-edge peek if there's a page to the right
- The peek is static during Ambiguous — it doesn't grow with horizontal
  motion until the gesture resolves to PageSwipe
- When resolved to PageSwipe: existing physics takes over, peek
  becomes drag-following
- When resolved to ChildScrollV or TapCommit: peek fades back to 0
  smoothly

### Open design questions for Phase 4

- How much peek? 4px (subtle, easy to miss), 8px (visible), 16px
  (prominent, looks like the next page is half-loaded)? Probably 4-8px.
- Does the peek fade in (timed animation) or appear instantly when
  Ambiguous starts? Instant is responsive; faded is calmer.
- What if the user holds their finger still for the full
  kAmbiguousTimeoutMs? Does the peek expand, hint at long-press, or
  stay static? Probably static — long-press is its own gesture.

---

## Phase 5: Other interactive widgets

**Goal:** roll out the consider model to remaining widgets.

### Candidates

- Text inputs (QLineEdit): consider-state could show the input is
  about to receive focus, before actually claiming focus and showing
  the keyboard
- Sliders: consider-state shows the slider thumb is about to be
  grabbed
- Checkboxes / radio buttons: consider-state previews the toggled
  state without committing
- Custom widgets (QR code label, signature pad, etc.): need
  per-widget design

### Approach

Each widget type gets its own design pass. Some will be trivial copies
of the button pattern. Some (signature pad) will be more complex
because their normal touch handling is incompatible with the consider
model — those may need to opt out via the existing
`swipenav-passthrough` property.

---

## Things that MUST be preserved through the migration

A short list of behaviors the existing UI has that the new system
needs to preserve. Capturing here so they don't get lost.

### Single-finger lock (from V32.32)

Currently SwipeNavigator locks to the first finger that lands; second
finger is ignored until first lifts. This prevents multi-touch chaos.
The new Ambiguous state must preserve this: a second touch landing
during Ambiguous should be ignored (not extend Ambiguous, not
re-classify, not anything).

### Stuck-gesture watchdog (from V32.32)

The 2.5-second watchdog that force-resets state if TouchEnd never
arrives. Must continue to work during Ambiguous state — if Ambiguous
gets stuck (e.g. window backgrounded mid-gesture), the watchdog
clears it.

### Horizontal-only scroll areas (theme carousel)

Currently SwipeNavigator detects these and defers entirely to their
own inner filter. The new system needs to preserve this: when the
focus widget is inside a horizontal-only scroll area, Ambiguous should
NOT engage — the inner filter handles it. This is the ChildScrollH
intent path in the current code.

### Scroll-area axis-lock within itself

Inside a vertical scroll area, the user might want horizontal scroll
(rare but valid — some lists scroll both ways). The current code has
an axis-lock decision inside the scroll area. The new system needs to
keep this — Ambiguous resolves to ChildScrollV broadly, then within
that intent the axis-lock decides H vs V scroll. NOT a re-classification
(intent stays ChildScrollV), just a sub-decision within that intent.

### Scroll-pacing (60Hz cap on scrollbar updates)

The existing `commitScrollPaced` rate-limiting to prevent the 120Hz
touchscreen from outrunning the 60Hz display. Must be preserved in
the new ChildScrollV resolution path.

### Tap forwarding without scroll

Currently, if user taps in a scroll area without scrolling (motion <
12px), the tap is forwarded to whatever widget was under the touch.
The new TapCommit resolution path needs to do the same — forward
synthesized mouse click to the focus widget on Ambiguous resolution
to TapCommit.

### Property-based opt-outs

`swipenav-tappable` and `swipenav-passthrough` properties. The new
classifier already handles these. Must continue to.

### QLineEdit focus protection

Currently `gestureUpdate` refuses to engage if the touch is over a
QLineEdit (which would have been a focus-steal). The new classifier
handles this via PassToChild. The behavior must persist.

### Wheel/trackpad gestures

Separate code path. Not affected by the Ambiguous redesign (touch and
wheel are independent). Make sure the refactor doesn't break wheel
event handling.

---

## What gets renamed / restructured

When Phase 1 lands, some existing concepts get clearer names. Filing
the rename plan so the diff is reviewable.

- `m_passingToScrollArea` → `m_gestureResolvedTo == ChildScrollV`
  (no separate flag needed; intent already carries this info)
- `m_intent` (from V32.33 rewrite) gets the new value `Ambiguous`
  added. Default of m_intent stays None.
- `classifyIntent` keeps its signature but starts returning Ambiguous
  in cases where it currently returns ChildScrollV/H/PageSwipe.
- A new method `resolveAmbiguous(QPointF cur)` does the
  threshold-based transition from Ambiguous to a concrete intent.

---

## Open questions to revisit before Phase 1 implementation

These need answers before code lands:

1. **What's the right value for kAmbiguousTimeoutMs?** 400ms is a
   guess. Should be tested in practice.
2. **Should Ambiguous have its own QTimer or piggyback on the existing
   gesture watchdog?** Probably new timer — different semantics.
3. **What's the visual feel of the Ambiguous-to-resolved transition?
   Crisp cut, or animated fade?** Probably crisp — animation would
   make the system feel laggy.
4. **Does the affordance signal carry the focus widget by raw pointer
   or by QPointer?** Has to be QPointer-safe since widgets can be
   destroyed during a gesture (rare but possible).
5. **Should there be a way to disable the Ambiguous behavior for
   testing?** Probably yes — a developer flag that reverts to
   immediate-resolution mode for performance comparison.

---

## Risk register

Things that could go wrong during the migration:

- **Inconsistent adoption.** Phase 3+ widgets adopt at different
  paces. Users see consider-state on some widgets, not others.
  Mitigation: prioritize high-visibility widgets (buttons, list rows,
  page edges) and accept that custom widgets adopt later.
- **Perceived laggy taps.** If Ambiguous duration is too long, simple
  taps feel slow because they have to wait through the timeout.
  Mitigation: tune kAmbiguousTimeoutMs to be short enough that taps
  don't feel slow, but long enough that the consider-state is visible.
  Probably 100-200ms is the sweet spot, not 400ms.
- **Affordance signal storm.** If many widgets subscribe to the signal,
  every TouchBegin causes N signal emissions. Mitigation: ensure
  signals are only emitted when there's actually a focus change, and
  consider an opt-in subscription mechanism for widgets that care.
- **Animation jank during Ambiguous.** If many widgets animate
  consider-state simultaneously, the 60Hz paint budget can blow up.
  Mitigation: same as ambient-feature migration — limit
  simultaneous animations, prefer cheap visual changes (opacity
  toggle, color tint) over expensive ones (geometry change, shadow).
- **Accessibility regression.** Screen readers expect button-press
  semantics. The Ambiguous state might confuse assistive tech if not
  designed carefully. Mitigation: ensure click events still fire on
  TapCommit, and that the consider-state doesn't generate spurious
  accessibility events.

---

## Effort estimate (honest)

- Phase 1 (infrastructure): 2-3 focused turns
- Phase 2 (button validation): 2-3 turns, mostly tuning visual feel
- Phase 3 (list rows): 3-5 turns, tricky interactions
- Phase 4 (page-swipe peek): 2-3 turns
- Phase 5 (other widgets): 5-10 turns, depends on how many widget
  types need attention

Total: ~15-25 turns across multiple sessions. This is a real project.

Compare to the alternative (revert the V32.33 rewrite to the V32.32
hybrid pattern): 1 turn. The hybrid worked, just with the jitter you
specifically called out as architecturally wrong. Choosing to do this
migration is choosing to do the architecturally correct thing at real
cost.

---

## Decision log

- **V32.33 (this filing):** identified the need for Ambiguous state
  after V32.33's top-down rewrite created the "horizontal flick in
  scrollable area gets swallowed" UX wart. Filed this migration doc.
  Phase 1 not yet implemented.

- **V32.33+ Phase 1 (implemented):**
  - Added Ambiguous to the GestureIntent enum, plus TapCommit
  - Added affordance signals (gestureAffordanceShown/Resolved/Cancelled)
  - Added resolveAmbiguous(dx, dy) and endAmbiguous(endPos, wasTap) helpers
  - TouchBegin now enters Ambiguous instead of committing directly
  - TouchUpdate resolves Ambiguous on motion past threshold
  - TouchEnd resolves Ambiguous as TapCommit (small motion) or Cancel
  - No widgets subscribed yet — signal emission is wire-up only

  **Build issue discovered post-ship:** moc-generated code couldn't
  see GestureIntent because the enum was in `private:` section but
  used as signal argument type. Signal argument types must be visible
  from at least the same access scope as the signal. Fixed by hoisting
  the enum to `public:` near the top of the class with a Q_ENUM
  registration. This is a general Qt rule worth remembering for future
  signal additions.

  **Threshold tuning (Phase 1):**
  - Split kLockThresholdPx (legacy, 14px) into per-axis:
    kPageSwipeThresholdPx (6px) for horizontal page-swipe resolution,
    kScrollThresholdPx (14px) for vertical scroll engagement.
  - Updated gestureUpdate() inside the PageSwipe path to also use
    the 6px threshold, so the page actually starts following the
    finger as soon as Ambiguous resolves to PageSwipe (previously
    the 14px threshold inside gestureUpdate created an 8px dead
    zone after the Ambiguous resolution).
  - The horizontal-axis-bias (1.5) protects diagonal motion from
    accidentally resolving to PageSwipe — a 6px dx only wins if
    dy < 9px, which excludes most accidental diagonal scrolling.

  **Behavior change to watch for in testing:**
  - Page-swipe now starts following the finger at 6px of horizontal
    motion (was 14px). Should feel noticeably more responsive.
  - Vertical scroll still requires 14px of motion to engage.
  - Horizontal flick from inside a scrollable area now triggers
    page-swipe (the architectural fix from Phase 1).
  - Taps with <10px movement forward correctly to the focus widget.
  - Movement of 10-13px that doesn't satisfy axis-bias for PageSwipe
    AND doesn't reach 14px for scroll will Cancel (no tap, no scroll).
    This might feel like "I tapped and nothing happened" — watch for
    it on the phone.

(Future updates: each phase implementation should add an entry here
with what was done and what surprises came up.)

- **V32.33+ Phase 1.5 (implemented):** Risk reduction on Phase 1.
  - **Widened tap tolerance** kTapMaxMovementPx from 10 to 16. The
    10-15px Cancel gap (motion didn't reach axis-lock thresholds but
    moved enough to fail tap detection) was producing dropped taps
    when users jittered during deliberate touches. 16 is at the upper
    end of mobile platform tap-jitter tolerances. Watch on phones for
    the opposite failure mode: deliberate small swipes (~14px) now
    register as taps. If this becomes a problem, the right answer is
    probably to use axis-bias for the tap-vs-cancel distinction: if
    the motion satisfied PageSwipe bias OR ChildScrollV's vertical
    threshold, it's a swipe attempt → Cancel; otherwise it's a tap.

  - **Aligned eventFilter path thresholds** with the new per-axis
    constants. Discovered while auditing: eventFilter is actually the
    PRIMARY touch entry point in practice. SwipeNavigator's own
    event() only fires when a touch lands on a part of SwipeNavigator
    not covered by a child — which is rare in a real app because
    pages cover the full area. So virtually all touches go through
    eventFilter. Without this alignment, Phase 1's threshold tuning
    would have only applied to a rarely-used code path.

  - **NOT done:** full migration of eventFilter to the Ambiguous-state
    model. The eventFilter path has a fundamentally different
    structure (must let TouchBegin propagate to child for mouse
    synthesis, may claim later) that doesn't map cleanly onto the
    event() path's Ambiguous approach. Doing this properly requires
    extracting shared helpers (`processTouchBegin/Update/End` in
    SwipeNavigator coords) and routing both paths through them.
    Filed as Phase 1.6 work.

- **Phase 1.6 (NOT YET DONE):** Shared helpers between event() and
  eventFilter. Goal: eventFilter benefits from the full Ambiguous-
  state design, including affordance signals. Tricky bit: eventFilter
  needs to NOT consume TouchBegin (so Qt mouse-synth works for
  buttons) while still tracking the gesture. The pattern: signal
  emission can happen on TouchBegin regardless of whether SwipeNavigator
  claims the event. Only the "claim it" decision differs between
  paths. Extract helpers in this shape:

  ```cpp
  void beginAmbiguous(QPointF posInSwipeNav, const QWidget *leaf);
  bool updateAmbiguous(QPointF posInSwipeNav);  // returns true if resolved
  void finalizeOnRelease(QPointF posInSwipeNav, bool wasTap);
  ```

  Then event() and eventFilter both call into these with their own
  coordinate-mapping logic. The "claim vs let-through" decision stays
  in the calling site since it differs between paths.

  Effort: 3-5 turns, similar scope to the original Phase 1 work.
  Risk: high — eventFilter has subtle interactions (single-finger lock
  across paths, wheel cancellation, modal-dialog handoff) that need
  preserving.

- **V32.33+ Phase 1.7 (audit pass, implemented):** Self-review after
  several Phase 1.x ships discovered two real bugs that hadn't fired
  yet because no widgets are subscribed to affordance signals. Filing
  in case they become observable in Phase 2+:

  - **Watchdog forced-reset during Ambiguous didn't emit cancellation
    signal.** If a touch enters Ambiguous and the system never delivers
    TouchEnd (window backgrounded mid-touch, app focus change, hardware
    glitch), the 2.5s watchdog force-resets state. The forced reset
    cleared m_affordanceFocus without emitting gestureAffordanceCancelled,
    so any subscribed widget would be stuck in consider-state.
    Fixed: watchdog now emits cancellation signal before reset if intent
    is Ambiguous.

  - **TouchCancel during Ambiguous was treated as potential tap.** The
    TouchEnd/TouchCancel handler made no distinction between user-
    initiated release (TouchEnd) and system-revoked gesture (TouchCancel).
    If a touch was cancelled by palm rejection or system intervention,
    a small-motion gesture would still fire a synthetic click. That's
    wrong — TouchCancel must never fire a tap. Fixed: TouchCancel
    always cancels regardless of motion.

  - **eventFilter "lazy gesture begin" returns true even when
    gestureUpdate rejects.** Pre-existing behavior — not introduced by
    Phase 1.x. If gestureUpdate rejects the motion (e.g. direction
    ratio fails: it's clearly vertical, not horizontal), the event
    handler still returns true, claiming the gesture. Should return
    false to let child see the touch. Risky to change without phone
    testing, so deferred. Filed in case it surfaces as a symptom later:
    might explain occasional "I started scrolling and the page started
    swiping instead" reports if anyone makes them.

  - **Page-swipe path in eventFilter doesn't reset m_touchStart on
    horizontal-lock when canScrollH is false.** Looked at this during
    audit. Sets m_passingToScrollArea = false then calls gestureBegin.
    gestureBegin sets m_touchStart itself, so this is correct. Not
    a bug; documented for future-me.

- **V32.33+ Phase 1.8 (deep audit, implemented):** Second-pass review
  after Phase 1.7 found two more concrete bugs. Found four more:

  - **Velocity history wiped at Ambiguous→PageSwipe resolution.**
    resolveAmbiguous called gestureBegin which calls velocityReset.
    Any velocity samples collected during Ambiguous were lost. A fast
    flick that crossed threshold in one TouchUpdate would have only
    2 velocity samples (start, current) which gives degraded
    commit-vs-bounce decisions. Fix: inline only the animation-handoff
    bits of gestureBegin in resolveAmbiguous, skip the velocity reset.
    State init (m_touchStart, m_dragDistance, m_pageWidth) was already
    done at TouchBegin so it doesn't need to repeat.

  - **No velocity sample at TouchBegin in event() path.** velocityReset
    was called but velocityRecord(start) was not. First sample only
    happened on TouchUpdate, so very-fast taps would have empty
    velocity history. Fix: velocityRecord(touchPos.x()) immediately
    after velocityReset.

  - **No velocity samples during Ambiguous TouchUpdate.** When Ambiguous
    swallowed events without resolving, no velocity tracking happened.
    Combined with above bug, a fast-then-stop gesture would mis-classify
    on release. Fix: velocityRecord(cur.x()) at the top of Ambiguous
    TouchUpdate handling, before resolveAmbiguous.

  - **Wrong point ID in TouchEnd tap detection.** ChildScrollV and
    PageSwipe TouchEnd handlers used `tp.first()` to get end position
    instead of finding the locked point. Multi-touch end events can
    deliver points in any order — if user has finger A locked but
    finger B is also released in the same event, .first() could be
    finger B. Fix: search for m_lockedPointId in the points list,
    fallback to .first() only if no locked point. Applied to both
    Ambiguous, ChildScrollV, and PageSwipe TouchEnd paths.
    Pre-existing in ChildScrollV/PageSwipe (not introduced by Phase 1).

  - **eventFilter shallow widget removal.** childEvent only removed
    the direct child from m_filteredWidgets, but installFilterRecursive
    added all descendants. Asymmetric: addition recursive, removal
    shallow. Stale descendant pointers persisted in the QSet. If Qt
    later reused those memory addresses for unrelated widgets, the
    new widgets would be incorrectly treated as filtered. Fix:
    new `uninstallFilterRecursive` helper that matches the addition
    pattern. Pre-existing bug, not introduced by Phase 1.

  - **ChildScrollV TouchEnd didn't respect TouchCancel.** Same bug
    as the Ambiguous case fixed in Phase 1.7 — small-motion synthetic
    tap could fire on system-cancelled gesture. Fix: pipe `isSystemCancel`
    through, never fire tap on TouchCancel.

  The pattern through Phase 1.7 and 1.8: I shipped Phase 1 too fast
  and the audit work is what should have happened before each ship.
  Three of the four bugs in Phase 1.8 were directly caused by
  Phase 1's redesign (velocity tracking) and could have been caught
  with a careful read at ship time. The other two are pre-existing
  bugs the audit happened to surface. Worth filing: **next time a
  multi-turn refactor is under way, audit each turn against the
  previous turn before the new code goes out.**

- **V32.33+ Phase 1.9 (re-audit, implemented):** Third audit pass
  specifically looking for the bug classes Phase 1.8 surfaced:
  parallel-path drift between event() and eventFilter, multi-touch
  point ID handling, state lifecycle gaps. Found three more bugs:

  - **eventFilter TouchEnd used pts.first() instead of locked point.**
    Same fix pattern as Phase 1.8 applied to the event() path's three
    TouchEnd handlers — extracted findLockedPos lambda to share the
    locked-point search logic. Also piped isSystemCancel through so
    TouchCancel doesn't fire synthetic taps.

  - **Stale Phase 1 state when eventFilter takes over a gesture.**
    Phase 1 added m_intent, m_candidateIntent, and m_affordanceFocus
    members. event() resets these in resetGestureState. But eventFilter
    has its own legacy state machine — it doesn't touch m_intent.
    If an event()-path gesture ended properly, fine; but if it ended
    abnormally (test setup, hard crash before reset), m_intent could
    be stale and the watchdog might act on stale m_affordanceFocus.
    Fix: eventFilter TouchBegin defensively clears m_intent,
    m_candidateIntent, and m_affordanceFocus before its own setup.

  - **Tap fires at release position, not start position.** This is
    pre-existing iOS/Android-inconsistency. With kTapMaxMovementPx
    raised to 16, the finger can drift 16px between press and release
    and `forwardTap(endPos)` would call `widgetAt(endPos)` — potentially
    firing on an adjacent widget the user didn't press. iOS and Android
    both fire taps on the widget the touch STARTED on (within tolerance),
    not where it released. Fix: all three forwardTap callsites
    (endAmbiguous, ChildScrollV TouchEnd, eventFilter passingToScrollArea
    TouchEnd) now pass m_touchStart (the press position) instead of
    end/release position. The PageSwipe path (which uses release
    position for velocity in gestureEnd) is unchanged.

  Things I checked and found OK:

  - classifyIntent handles null leaf correctly (skips loop, returns
    PageSwipe).
  - m_scrollTarget is QPointer-safe (auto-nulls on scroll area
    destruction).
  - m_animConn lambda check via QPointer prevents post-destroy invocation.
  - Wheel event path doesn't interact with Ambiguous state (touch
    and wheel are independent streams; wheel cancels itself via
    its own idle timer).

  Things flagged for Phase 1.6 (NOT fixed this audit):

  - eventFilter velocity tracking during pre-lock wait period is
    incomplete — velocity samples don't accumulate until gestureBegin
    is called at lock time. Same fix needed as event() Ambiguous,
    but eventFilter's gestureBegin call would wipe them. Proper fix
    requires unifying both paths' state machines.
  - eventFilter "lazy gesture begin" returns true even when gestureUpdate
    rejects motion (direction-ratio fail). Should let touch propagate
    in that case. Pre-existing, not introduced by Phase 1.
  - kFlickMinPx (14) doesn't match kPageSwipeThresholdPx (6) by design
    — a 6-14px page-swipe attempt won't flick-commit and must drag past
    22% of page width or bounce back. Verified intentional, not a bug.

  After three audit passes, the SwipeNavigator code is more correct
  than at the start of the session. The bugs found in Phase 1.8 and
  1.9 included pre-existing bugs that have been in the codebase since
  V32.32. Phase 1's restructuring helped surface them, even though
  Phase 1 itself also introduced bugs that needed fixing.

- **V32.33+ Phase 1.10 (deep audit, implemented):** Fourth pass
  targeting different bug classes — math correctness, animation
  conflicts, multi-system state coordination. Found six more:

  - **Dead code in velocityCompute.** The function computed sumWx,
    sumWt, sumW, meanX, meanT but only used endpointVel. The
    weighted-mean slope was never blended in despite a comment
    saying it should be. Implemented the actual blend: 60%
    weighted-slope (robust to outlier endpoints) + 40% endpoint
    velocity (responsive to lift-off speed). Required a second pass
    through the ring buffer to compute the slope numerator/denominator
    using weighted least-squares regression.

  - **TouchBegin didn't stop running animations.** Phase 1's TouchBegin
    skipped gestureBegin (and its animation-stop logic) by going
    straight into Ambiguous state. If a previous swipe's commit
    animation was still running when a new touch began, both the user
    motion AND the animation drove m_slideOffset, producing visible
    conflict. Added the animation-handoff logic at TouchBegin to match
    the original gestureBegin behavior. The duplicate in
    resolveAmbiguous is left as defensive code for future paths.

  - **m_swipeLocked and m_dragging not reset at TouchBegin.** Wheel
    events leave these set to true when active. Wheel-cancelled-by-
    touch path didn't clear them. The next touch gesture inherited
    locked state, meaning gestureUpdate's below-threshold-wait
    branch was skipped. Defensive clearing added.

  - **m_slideOffset not reset on wheel-cancel.** Same family as above
    — wheel left slide offset non-zero, new touch started with
    leftover visual state. Now wheel-cancel path explicitly resets
    slide offset to 0 and calls layoutPages.

  - **Fade animations conflict on rapid re-swipe.** startCommitAnimation
    creates QGraphicsOpacityEffect fade animations. On rapid re-swipe,
    the same fx had MULTIPLE concurrent fade animations both calling
    fx->setOpacity, producing flicker. Fix: search outgoing's
    QPropertyAnimation children for any already targeting this fx's
    opacity property, stop them before starting new one.

  - **Flick direction can conflict with drag direction.** The commit
    direction selection was `isFlick ? (velocity < 0.0) : (drag < 0.0)`.
    If the user dragged right (positive drag) and then flicked
    slightly left at release (negative velocity), isFlick would
    pick velocity-direction → commit to NEXT (right) page even though
    the user had been visually viewing the PREVIOUS (left) page during
    the drag. That's wrong UX. Fix: require velocity and drag direction
    to agree for the flick path to apply. Disagreement falls through
    to drag commit (which uses drag direction).

  Pattern noted in this pass: bugs in math-heavy or animation-heavy
  code don't show up in casual review — they need specific scenarios
  to trigger. The dead-code velocity bug had been there since
  whenever the blend was first written; the flick-direction bug
  could have been there since the V32.32 physics rewrite. Both would
  show up as "swipes feel weird sometimes" rather than crashes.

- **V32.33+ Phase 1.11 (fifth pass, implemented):** Targeting Qt
  internal interaction patterns — visibility, focus, animation
  lifecycle, signal emission semantics. Found four more:

  - **layoutPages calls setVisible on already-correct widgets,
    risking focus loss.** Qt's setVisible(false) clears focus on
    the widget and descendants; setVisible(true) doesn't restore.
    During rapid bounce-back animations, layoutPages visits each
    page dozens of times — the redundant calls do nothing functional
    but trigger Qt internal bookkeeping that can drop focus from
    a QLineEdit on the bouncing page. Fix: only call setVisible
    when the state actually changes.

  - **goToPage uses exact equality on slideOffset.** `if (... &&
    m_slideOffset == 0.0)` fails when slideOffset is ~1e-15 from
    a previous animation's floating-point residue. Switched to
    qFuzzyIsNull. Same family of bug as anything comparing doubles
    with ==.

  - **Spurious pageChanged emissions on animation-interrupted
    bounce-back.** Three places (gestureBegin, event() TouchBegin
    animation-stop, resolveAmbiguous animation-stop) emit pageChanged
    unconditionally when stopping an in-flight animation. If the
    animation was a bounce-back (no real page change), this is a
    spurious emission. Subscribers that count transitions (analytics,
    lifecycle hooks, navigation history) get confused. Fix: capture
    prevIndex before the snap-decide logic, only emit if it
    actually changed. Applied to all three places PLUS goToPage's
    animate callback.

  - **Watchdog doesn't disconnect stale anim lambdas before restart.**
    Watchdog fires only when something already went wrong. Defensive
    pattern: if a lambda from a previous animation is still connected,
    the new bounce-back from watchdog would fire that lambda on
    completion, operating on stale state. Added disconnect before
    setting up new animation.

  Things I checked and found OK in this pass:

  - resizeEvent doesn't cancel gestures, but the coordinate-space
    arguments I worried about don't actually apply — widget-local
    coords stay consistent across resizes.
  - layoutPages handles zero pages without crashing.
  - startBounceAnimation has Q_UNUSED on velocity parameter (not
    a bug — comment explains it's intentional, velocity is just a
    proxy for duration here, not used directly).
  - QPropertyAnimation::DeleteWhenStopped properly cleans up the
    fade animations across the lifecycle.
  - The connectAnimFinished lambda self-disconnect pattern is
    correct when interleaved with manual stop() calls.

  Things flagged but not changed:

  - m_pages uses raw QWidget* not QPointer. If a page is destroyed
    independently of the navigator (uncommon but possible), m_pages
    holds a dangling pointer. Filed as future hardening — Qt's
    parent-child relationship makes this rare in practice.
  - TouchUpdate where the locked point appears in Released state
    isn't explicitly handled. Qt normally delivers Released states
    in TouchEnd events; this would only matter if Qt's delivery
    diverges from spec. Speculative — not changed.

- **V32.33+ Phase 1.12 (cross-module audit, implemented):** Same bug-class
  lens applied to AppController, StorageManager, and AmbientInboxDriver.
  Found a critical wire-format mismatch and a major architectural finding:

  - **S-21 (filed in CRYPTO_REVIEW.md): stable ID is random, not
    cryptographically bound to keypair.** Identity::loadOrGenerate
    creates `sc-<8-random-hex>` from QRandomGenerator, then persists.
    The sc-ID is NOT derived from the user's pubkey. This means:
    (a) anyone can claim any sc-ID by editing identity.txt; (b) 32-bit
    collision space → birthday paradox at ~65K users; (c) S-2 + S-20
    fixes (signed SHAK + TOFU) don't fully close persistent-MITM
    because the sc-ID itself is attacker-chooseable. Real fix is to
    derive sc-ID from SHA-256(pubkey). Filed as architectural — needs
    migration plan, not a single-turn fix.

  - **Bug 33 (functional, broken in current build): CONNREQ wire format
    mismatch between sender and receiver.** AppController_commands.cpp
    line 503 published `CONNREQ\n%1\n%2\n%3\n` (4 fields). Receiver at
    AppController.cpp line 379 requires `parts.size() < 5`. Every
    CONNREQ published via this path was being SILENTLY REJECTED by
    recipients. The mailbox-backed-reconnection-on-relookup path
    never worked. This is one of those bugs where two halves of the
    same protocol drifted in parallel — sender added a field at one
    point (in connect.cpp), receiver added the validation, but the
    other sender path wasn't updated. Fixed by aligning the format.

  - **Bug 31 (security): mailbox nonce dedupe inserted BEFORE decrypt
    attempt.** This made replay protection vulnerable to a "spam to
    flush" attack: attacker sends 500 bogus blobs, our cache evicts
    legitimate older nonces (FIFO), attacker replays previously-seen
    blob, dedupe miss, we re-process it. Fix: defer the dedupe insert
    until AFTER successful decryption. Failed-decrypt nonces don't
    consume cache slots. Also added the dedupe insert to the retry
    success path (was missing entirely), and added a pre-check in
    retry to skip if duplicate was processed in the meantime.

  - **Bugs 34, 35, 36 (data consistency): transaction() return values
    not checked in StorageManager.** Three places (applyMigration,
    acceptPendingKeyChange, purgeAllData) called `m_db.transaction()`
    without checking the return value. If transaction() fails (db
    locked, busy, in another tx), subsequent statements run individually
    and rollback() does nothing. applyMigration partial-commit would
    leave the schema inconsistent. acceptPendingKeyChange could end
    up with the user's accept applied but the pending row not deleted
    (or vice versa). Fixed all three.

  - **Bug 39 (UX): AmbientInboxDriver re-derived after user reverted
    to static palette.** Comment in the code explicitly said "skip
    silently rather than racing the cleanup" but the code actually
    fell through to derive anyway, re-establishing the synthetic
    palette and overriding the user's static-palette choice. The
    comment described intent that the code didn't implement. Fixed
    by actually implementing the skip.

  Cross-module pattern observed: the bug classes that show up in
  gesture code (state lifecycle, parallel-path drift, fire-once-when-it-
  should-not-fire) appear in coordination-heavy code generally. The
  CONNREQ format mismatch is literally the same "parallel paths drift
  out of sync" bug as the SwipeNavigator event() vs eventFilter drift
  from Phase 1.9 — just at a different scale (network protocol vs
  function paths in one widget).

  Things checked and OK:

  - activeLookupHandles: belt-and-suspenders generation check + handle
    disconnect prevents stale lambda races.
  - peerDiscovered lambda lifecycle: 'this' context auto-disconnects
    on AppController destruction.
  - stabilityTimers: properly parented, parent destruction cleans up.
  - SQLite ON CONFLICT clause semantics in recordPendingKeyChange:
    unqualified column names refer to existing row, not post-update —
    so the SET clause logic is correct despite appearing fragile.

  Things flagged but not changed:

  - peerPunchAddr in AppController grows unbounded — no removal logic.
    Slow memory leak over a long session with many distinct peers.
    Needs clear semantics for when to expire entries.
  - Several QString-to-numeric conversions silently return 0 on parse
    failure (parts[3].toLongLong, etc.). Defensive but the resulting
    behavior is usually safe (timestamp 0 = "very old" = ignored).
    Not changed.
