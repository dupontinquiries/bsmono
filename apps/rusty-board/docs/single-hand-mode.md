# Single-Hand Mode — Design Doc

This document captures the design of RustyBoard's single-hand thumb-typing
mode, which is the signature ergonomic feature of the project. It
composes several smaller mechanics into a coherent system. Read this
before working on the code so the pieces stay consistent.

## The user problem

One-handed thumb typing on phones >6" is uncomfortable because the
thumb's reachable region is roughly a fan-shaped arc, not a rectangle.
A full-width keyboard puts letters outside this arc — most painfully
the corner opposite the dominant hand. Users compensate by either:

- Tolerating awkward stretches (slow + thumb fatigue)
- Regripping the phone constantly (jarring + phone-drop risk)
- Switching to two-handed mode (defeats one-handed use case)

RustyBoard's single-hand mode reshapes the keyboard to fit the thumb's
actual reach, then adds a small constellation of gestures that take
advantage of the reshaped geometry.

## The full system

```
┌───────────────────────────────────────────────────┐
│            top edge: digit / minimize             │
│   ┌─────────────────────────────────────┐         │
│   │                                     │         │
│   │       letter region                 │         │
│   │       (compacted into arc)          │         │
│   │                                     │         │
│   │                                     │   right │
│ left edge ──────────────────────────────│   edge  │
│ (multi-                                 │  (multi-│
│  anchor                                 │  anchor │
│  space)                                 │  space) │
│   └─────────────────────────────────────┘         │
│              bottom edge: space                   │
└───────────────────────────────────────────────────┘
```

Five edges with semantic meaning, three keyboard states, and four
distinct gesture types. The user learns gradually — basic typing works
from day one, and the gestures self-teach via post-action flashes.

## States

The keyboard has three top-level states:

| State        | Description                                              | Transitions in                                                    | Transitions out                                                |
|--------------|----------------------------------------------------------|-------------------------------------------------------------------|----------------------------------------------------------------|
| `Full`       | Full letter layout, full features                        | (a) IM Activate when Dismissed; (b) Double-Activate when Tray     | (a) Long upward fling off top → Tray; (b) IM Deactivate        |
| `Tray`       | Minimized corner panel: expand / spacebar / close        | Long upward fling from Full                                       | (a) Tap close → Dismissed; (b) Double-Activate → Full          |
| `Dismissed`  | No keyboard rendered                                     | (a) Tap close in Tray; (b) IM Deactivate from Full or Tray        | IM Activate → Full                                             |

Where "Double-Activate" means: two `zwp_input_method_v1::activate`
events within 700ms with no intervening Deactivate. KWin fires this
when the user double-taps a text field (or taps two different fields
in quick succession; the second tap counts as a re-focus).

## Gestures

### From Full state

| Gesture                                                  | Action                                                     |
|----------------------------------------------------------|------------------------------------------------------------|
| Tap a letter key                                         | Emit letter (existing)                                     |
| Swipe within letter region                               | Word-recognition swipe-type (existing)                     |
| Swipe across the bottom or side edges                    | Springboard space — see below                              |
| Tap a multi-anchor space zone                            | Plain space                                                |
| Short upward arc starting on a top-row key, staying ≤1 row-height above the top row | Digit gesture: emit the digit for that column |
| Long upward fling, terminus > 1.5 row-heights above the top edge OR within 20px of the screen top | Minimize → Tray                  |
| Tap settings key (⚙)                                     | Open settings panel                                        |
| Cardinal swipe on a cardinal-grid key (when grid enabled) | Emit the gesture's mapped char                            |
| Cardinal swipe on the spacebar in any layout              | Arrow key (existing)                                       |

### From Tray state

| Gesture                                  | Action                                                          |
|------------------------------------------|-----------------------------------------------------------------|
| Tap tray spacebar                        | Plain space (also exits selection mode if active)               |
| Double-tap tray spacebar                 | Toggle selection mode on/off                                    |
| Cardinal swipe on tray spacebar (normal) | Arrow key                                                       |
| Cardinal swipe on tray spacebar (selection mode) | Shift+Arrow (extend selection)                          |
| Tap expand button                        | → Full                                                          |
| Tap close button                         | → Dismissed                                                     |

## Subsystems

### 1. Thumb-arc compaction

#### Data model

```rust
struct ThumbArc {
    /// Which corner the user anchors their thumb at.
    anchor: Corner,
    /// Polygon describing the reach envelope, in normalized
    /// coordinates [0.0, 1.0] across the layer surface. Each point is
    /// relative to the surface's full size. Polygon is closed (last
    /// point connects to first).
    polygon: Vec<(f64, f64)>,
}

enum Corner { BottomLeft, BottomRight }
```

Stored in `LayoutConfig.thumb_arc: Option<ThumbArc>`. `None` means
"single-hand mode off" — full-width layout, no compaction.

#### Defaults by phone size

If the user enables single-hand mode without calibration, we provide
a default polygon based on the surface width. Rough heuristic:

- Surface ≤ 360 logical px → no compaction needed (already small)
- 360 < surface ≤ 480 → mild compaction: top row 80% width, anchored right
- Surface > 480 → strong compaction: top row 65% width, anchored right

The default polygon for right-handed users is approximately:
- Bottom-right corner (0.0 below the keyboard, anchor)
- Bottom-left corner of the surface
- Up to (0.0, top_of_keyboard + 0.4)
- Arc curve from (0.0, top + 0.4) through (0.6, top) to (1.0, top)
- (1.0, bottom)
- (1.0, 0.0) back to anchor

Visually: a rounded right-triangle shape with the hypotenuse curving
from the top-right corner down to about 40% of the way up the left
edge.

For left-handed users, mirror horizontally.

#### Compaction algorithm

Given a polygon and the canonical row layout (rows with offset +
total key-width-units), we transform each row independently:

1. For each row index `i`, compute the y-coordinate (in normalized
   surface coordinates) where the row's vertical center falls.
2. Find the x-range of the polygon at that y-coordinate. This is
   `[x_min_at_y, x_max_at_y]`. For a right-handed compaction,
   `x_max_at_y` is always near 1.0 (the right edge); `x_min_at_y`
   varies by row — narrowest at the top, widest at the bottom.
3. Set the row's left margin and right margin so the row fits
   inside `[x_min_at_y, x_max_at_y]`.
4. Compute new unit width = row's available width / row's total
   key-width-units.

Keys within a row keep their relative positions and proportional
widths — only the row's outer bounds shift. Muscle memory is
preserved.

#### Calibration UX

Accessible from Settings > Single-hand mode > "Recalibrate thumb arc."

The calibration screen overlays the keyboard. Three steps:

1. **Anchor**: A pulsing target in the appropriate corner. User
   places thumb on it. Tap to confirm.
2. **Sweep**: A prompt "now sweep your thumb out and around as far
   as you can comfortably reach, then return to the anchor." A
   countdown/visual progress shows. We sample touch positions during
   the sweep.
3. **Confirm**: We render the captured polygon over a preview of the
   keyboard. User sees how the keyboard would look. Buttons: Accept,
   Redo, Cancel.

If the captured polygon is degenerate (too small, not a closed
loop, etc.), we fall back to the default for the user's phone size
and warn.

### 2. Letter-region boundary

The "letter region" is the bounding box (or polygon, when compacted)
of all letter keys. Its perimeter is the spacebar in springboard
mode.

For non-compacted layouts: a simple rectangle.

For compacted layouts: the same polygon used for the thumb arc, but
clipped to where the letter keys actually live (i.e., excluding the
trailing whitespace inside the arc that *isn't* on a key).

Edge identification: each segment of the boundary is tagged with
which "edge" it belongs to: `Top`, `Bottom`, `LeftSide`, `RightSide`.
The boundary detection logic uses this tag to decide *what* a
crossing means:

- `Top` crossing: minimize-to-tray (long enough fling) or top-row
  digit (short upward arc starting on a key) — the segmentation
  logic discriminates by gesture characteristics, not by edge alone
- `Bottom`, `LeftSide`, `RightSide`: springboard space

### 3. Springboard space (path segmentation)

#### Algorithm

When a swipe ends:

1. Walk the path. For each consecutive pair of points, compute
   whether the segment crosses the letter-region boundary.
2. Each crossing is annotated as:
   - **Out crossing**: path exits the letter region
   - **In crossing**: path re-enters the letter region
3. With hysteresis:
   - An out-crossing only "counts" if the path then travels at least
     `MIN_OUT_DWELL_PX` (e.g. 12px) inside the edge zone before
     re-entering. This rejects grazing.
   - A successful out-then-in pair counts as one space delimiter.
4. Path is segmented at each delimiter. Each segment is run through
   the existing word recognizer independently.
5. Special case: if the path ends in an edge zone (no closing
   in-crossing), treat that as "trailing space" — recognize the
   segment-so-far as a word, append a space.

#### Visual feedback

When a delimiter fires (during the swipe, or at swipe-end), flash a
small ripple at the crossing point. ~250ms fade-out, light accent
color. Teaches the user that the crossing counted.

#### Top edge is special

The top edge does NOT participate in springboard space. Crossing the
top edge is reserved for digit / minimize gestures. The
segmentation logic treats top crossings as either:

- Short up-arc from top row → emit digit, terminate swipe (don't
  also recognize anything else)
- Long up-fling → minimize, terminate swipe

### 4. Multi-anchor space rendering

Three space zones, rendered as faintly-colored strips outside the
letter region:

- **Bottom zone**: full width, ~36px tall (current spacebar size)
- **Right zone**: thin vertical strip, ~24px wide, spanning the
  height of the letter region's right edge. Only present in
  right-handed single-hand mode.
- **Left zone**: thin vertical strip, ~24px wide, spanning the
  height of the letter region's left edge. Only present in
  left-handed single-hand mode.

Each zone:

- Visually a translucent fill in the user's `special_fill` color
  (or theme default) with a small `⎵` icon at its geometric center
- Tappable (emits plain space)
- Cross-throughable (acts as springboard via path segmentation)
- Suppressed (faded out, non-tappable) during active swipe — except
  for the bottom zone, which stays tappable in case user wants to
  end mid-swipe

#### Post-word flash

When a word is committed (whether by tap-space-after-swipe, by
springboard cross-through, or by the legacy decoder), all visible
space zones briefly pulse — 150ms growing to full opacity + 1.1×
scale, 150ms fading back. Teaches the multi-anchor design.

The zone nearest to the user's last touch position pulses with
~30% extra intensity, hinting "this one is closest."

### 5. Top-row up-swipe → digit

Detected when:

- Swipe **starts** on a top-row letter key
- Swipe **stays within `THE_TOP` zone** (above top-row keys, but no
  higher than ~1 row-height above)
- Total path length < ~80px
- Duration < ~250ms

Action: emit the digit for that column (q→1, w→2, ..., p→0; for
Dvorak: '→1, ,→2, .→3, p→4, y→5, f→6, g→7, c→8, r→9, l→0).

Implementation: column lookup uses the *position* of the key in the
top row at swipe start, not the letter itself, so QWERTY and Dvorak
both work without per-layout digit tables.

### 6. Top-edge minimize → Tray

Detected when:

- Swipe terminates at y > 1.5 row-heights above the keyboard's top
  edge OR within 20px of the screen top
- Path's gross direction is predominantly upward (dy < -1.5 × |dx|)

Action: transition Full → Tray state.

If both digit and minimize conditions could match (short upward
arc that happens to also reach high), prefer **digit** (shorter
distance traveled, lower threshold). Minimize requires actual flight
off the keyboard.

### 7. Tray mode

#### Layout

Corner-anchored stack on the dominant side:

```
┌──────┐
│ ⬆ │   ← expand button (32×32)
│ ⎵ │   ← spacebar (200×48)
│ × │   ← close button (32×32)
└──────┘
```

Positioned in the bottom-right (right-handed) or bottom-left
(left-handed) corner. The wider spacebar means it can host arrow
swipes comfortably.

Total dimensions: ~200×140 logical px.

#### Spacebar gestures (in Tray)

- **Tap**: emit space, exit selection mode if active
- **Double-tap**: toggle `selection_mode` flag
- **Cardinal swipe (selection_mode off)**: emit Left / Right / Up
  / Down keysym
- **Cardinal swipe (selection_mode on)**: emit Shift+Left etc.

Double-tap detection: two tap events on the spacebar within 350ms.

#### Visual indicators

- When selection_mode is active: thin colored top-border on the tray
  spacebar (accent color)
- Tray spacebar has subtle "↑↓←→" arrows printed faintly so the
  user knows directional swipes are available
- Close button: red-tinted "×" so it's clearly destructive

### 8. Modifier-aware keysym

Backend extension. Current API:

```rust
fn press_keysym(&self, name: &str) -> Result<()>;
```

Extended to:

```rust
fn press_keysym(&self, name: &str) -> Result<()> {
    // Parse "Shift+", "Ctrl+", "Alt+" prefixes off the name.
    // Send appropriate modifier bitmask alongside the keysym.
}
```

Names like `"Left"` keep their plain-arrow semantics.
Names like `"Shift+Left"` parse the prefix into modifier bit 0 and
send the keysym with that modifier set.

For v1 path: include modifier mask in the `keysym(serial, time,
sym, state, modifiers)` request.

For v2 path: send `modifiers(mods_depressed, ...)` before and
`modifiers(0, ...)` after the `key()` event, so the modifier is
correctly scoped to just that keypress.

### 9. Composition with existing features

This system is gated by `layout.thumb_arc.is_some()`. When the user
hasn't enabled single-hand mode:

- Layout renders full-width as today
- Boundary detection still runs for the bottom edge only (so
  springboard-via-bottom-crossing still works for two-handed users)
- Top-edge minimize gesture still works (it's hand-agnostic)
- Top-row digit gesture still works
- Multi-anchor space NOT shown (only bottom space)
- Tray mode still available (minimize fling exits to tray
  regardless of single-hand mode)

So even users who never enable single-hand mode get: top-row digits,
swipe-up-to-minimize, tray mode with cursor arrows.

## Implementation order

The code build proceeds in this order, each phase building on the
previous:

1. **Modifier-aware press_keysym** — small backend change,
   foundation for shift-selection
2. **Boundary detection + springboard space** — works for
   non-compacted layouts first; full-width layout has a single
   bottom edge that acts as a springboard
3. **Top-row digit gesture** — small addition to existing swipe
   handler
4. **Top-edge minimize gesture** — also additions to swipe handler
5. **Tray mode widget + state machine** — significant new widget,
   double-activate detection
6. **Tray selection mode + shift-arrows** — small extension to tray
7. **Thumb-arc data model + default polygons** — config additions
   plus polygon math
8. **Compaction renderer** — row-by-row margin computation from
   polygon
9. **Calibration UX** — overlay widget that captures + previews
   polygon
10. **Multi-anchor space rendering** — only after compaction so the
    side strips have a defined position
11. **Post-word flash + crossing flash** — visual polish

Total ~1000 lines of new code spread across ~10 files.

## Tunable constants

For first build, use these. Expect to tune after testing:

```rust
const MIN_OUT_DWELL_PX: f64 = 12.0;          // hysteresis on boundary
const DIGIT_MAX_PATH_PX: f64 = 80.0;          // top-row up-swipe length cap
const DIGIT_MAX_DURATION_MS: i64 = 250;       // top-row up-swipe time cap
const MINIMIZE_DY_THRESHOLD: f64 = 80.0;      // upward fling distance
const MINIMIZE_DOMINANCE: f64 = 1.5;          // upward must dominate horiz
const DOUBLE_ACTIVATE_MS: i64 = 700;          // Activate-Activate window for Full restore
const DOUBLE_TAP_MS: i64 = 350;               // tray spacebar double-tap window
const FLASH_GROW_MS: i64 = 150;
const FLASH_FADE_MS: i64 = 150;
const SPRINGBOARD_RIPPLE_MS: i64 = 250;
```

## Things explicitly NOT in this round

- Recency-based fade of unused space zones (deferred)
- Word-prediction-aware preview of "which space is closest" (deferred)
- CW/CCW rotation gestures on the tray spacebar (none assigned)
- Custom calibration polygon editing (start with capture-or-default
  only, no manual edit handles)
- Multi-step shift composition (Ctrl+Shift+Left, etc.) — single
  modifier only this round
- Honeycomb layout (separately filed)
