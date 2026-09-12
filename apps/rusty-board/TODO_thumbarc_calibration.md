# TODO: thumb-arc calibration

`thumbarc::Arc::default_for` picks a compaction from the board's WIDTH — mild past 360pt,
strong past 480. That is a guess about a hand from a number about a screen, and it is wrong in
both directions: a small hand on a mild board still cannot reach the far top corner, and a
large hand on a strong one has its keys pulled in for nothing.

## Done: the flow (`thumbcal`)

Anchor, sweep, confirm, as a state machine with no UI in it.

* **Three steps, not one.** A bare "sweep your thumb" produces a stroke with no known origin,
  and the same arc means different things depending on where the hand is braced. The anchor is
  the one thing the user knows precisely and the app cannot infer, so it is asked for first, by
  tap.
* **Confirm exists because a sweep can be bad** — a hand that slipped, a stroke that stopped
  short. Committing that silently gives someone a keyboard they cannot use and no idea what
  changed.
* **Two refusals with reasons**: fewer than 8 samples is a tap, and less than a quarter of the
  diagonal of travel measured the corner rather than the reach. Both would fit an arc that
  compacts the board to almost nothing — and undoing that means finding this screen again on a
  keyboard that is now unusable.
* **Repeats do not pad the count.** A finger held still emits the same point every frame.
* **Retry keeps the anchor.** A bad sweep should not make someone re-tap a corner they got
  right.
* **Normalised 0..1**, so a calibration survives a rotation, a resize and a different device.
  In points it would be re-run every time the keyboard changed size, which is exactly when
  nobody wants to.

## Done: the overlay and the way in

`calibration_ui` draws all four steps over the BOARD's rect — not the window's, which differ
by the status bar on a phone, and a calibration offset by it would measure a reach nobody has.

The scrim is translucent on purpose: the keys are the thing being measured against, and a
reach shape over a black rectangle answers a different question than the one asked. A pixel
test asserts the keys still show through, and another that every step draws something — a
calibration that shows nothing while the finger moves is one people repeat because they assume
it missed.

Both corners are offered at the anchor step: which thumb is in use is exactly what this cannot
infer, and a right-handed default would make left-handed users tap twice. Cancel is present on
the rejected step too, so someone who opened this by accident can leave without sweeping.

Opened by `RUSTY_BOARD_CALIBRATE=1`, the same env convention as `RUSTY_BOARD_PLACEMENT`. Not a
settings row yet, and honest about that — but reachable, which a flow needs to be before it is
a feature.

While it is open the overlay TAKES the board: a sweep that also typed would fill the screen
with letters at the moment the operator is concentrating on their thumb.

The accepted value is the REACH, not the polygon. `Style` derives the arc per board rect, so a
shape fitted at one size would be wrong the moment the keyboard rotated. `Style::calibrated_reach`
is `Option<f32>` and `None` keeps the width-based guess — a hand that has never been measured
is better served by an estimate than by nothing.

## Still to do
- [ ] Persistence. A calibration that does not survive a restart is a demo. It belongs beside
      the other board settings, and `Arc`'s polygon is a short list of pairs, so the format is
      not the hard part; deciding what happens to a saved arc when the board's aspect changes
      is.
- [ ] Where it is reached from. Settings has a Layout group; this is a full-screen flow rather
      than a row, so it wants a button that opens it rather than living inline.
