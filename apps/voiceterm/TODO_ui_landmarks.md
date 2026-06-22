# TODO: UI landmarks — voice references elements by position/color/shape

User idea: richer voice targeting — reference UI elements by WHERE they
are ("top right", "the bottom"), COLOR ("the red button"), or SHAPE ("the
round one"). User's sequencing call (good): build the NON-DESTRUCTIVE
highlight FIRST (glow the resolved element so resolution quality can be
seen + dogfooded harmlessly), then bolt on actions once trusted.

## Shipped (pure, tested — v0.1.601)
`src/ui_landmarks.rs` (9 tests, in the cargo harness):
- `Zone` (3x3 grid) + `Zone::from_norm(cx,cy)` — element position →
  TopLeft..BottomRight.
- `parse_spatial(phrase) -> SpatialTerm` — "top right"/"the bottom"/"on
  the left"/"middle" → row/col axes.
- `Element { id, label, zone, color, shape }` — the registered-element
  model.
- `resolve(phrase, &[Element], top_n) -> Vec<Match{id,score,reasons}>` —
  ranks by label + color + shape + spatial signals SUMMED; an element
  that CONTRADICTS a stated descriptor (says "red", element is blue;
  says "left", element is right) is penalized so it can't win on an
  unrelated signal. Color/shape synonyms (grey=gray, circular=round).

## Step 1 — the highlight (build-gated, NON-destructive) ← DO FIRST
Goal: when speech references an element, GLOW it. No action taken.

1. **Element registry (QML → Backend).** Each voice-targetable QML
   element registers itself: id, label, its mapped-to-root bounding box
   (x,y,w,h), a semantic color name (needs themes — see below), a shape
   tag. On layout change, update the box. A Backend qt_method like
   `register_ui_element(id, label, x, y, w, h, color, shape)` storing
   into a Vec; convert (x,y,w,h) + viewport size → normalized center →
   `Zone::from_norm`.
2. **On transcript:** call `ui_landmarks::resolve(phrase, &elements, 1)`.
   If top score is above a confidence floor, emit a signal
   `highlight_element(id)`.
3. **QML glow:** on `highlight_element(id)`, the element with that id
   runs a brief pulse (reuse the existing `Behavior on border.color` /
   scale pulse infra — main.qml has ColorAnimation pulses already). Auto-
   clear after ~1.5s. NON-destructive: nothing is clicked.
4. **Dogfood:** say "the red button", "top right", "the mic" — watch
   which element glows. This reveals resolution quality safely.

### Themes (prerequisite for COLOR references)
For "the red button" to work, elements must KNOW their semantic color.
Today colors are inline literals. Minimum viable: a small theme table
mapping role→color-name, and each registered element passes its role's
color NAME ("red"/"green"/...) to register_ui_element. Full theming
(swappable palettes) is a later, larger piece — not needed for step 1,
which just needs the color NAME known at registration.

## Step 2+ — bolt on actions (only after step 1 dogfoods well)
- "click the red button" → resolve → if confident, click; else show the
  top-N as a picker (scores + reasons already returned).
- Region scroll/focus: "focus the left panel", "scroll the bottom".
- Disambiguation: when top-2 scores are close, glow BOTH and ask "which
  one?" (reasons explain why each matched).

## Guardrails
- Resolution stays PURE in ui_landmarks (tested); QML only registers
  elements + renders the glow + (later) acts. Keep the resolver free of
  QML types.
- Confidence floor before highlighting, so an unmatched phrase doesn't
  flash a random element.
- Contradiction penalty (already in resolve) prevents "red on the left"
  from glowing a red-on-the-right element.
