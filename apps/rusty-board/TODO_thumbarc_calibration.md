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

## Still to do

- [ ] The OVERLAY. `thumbcal` has no drawing: something has to show the two corners to tap,
      trail the sweep as it is drawn, and paint the fitted polygon over the real keys at the
      confirm step. Painting it over the keys is the point — a shape shown in the abstract
      does not tell anyone whether the top row will be reachable.
- [ ] Persistence. A calibration that does not survive a restart is a demo. It belongs beside
      the other board settings, and `Arc`'s polygon is a short list of pairs, so the format is
      not the hard part; deciding what happens to a saved arc when the board's aspect changes
      is.
- [ ] Where it is reached from. Settings has a Layout group; this is a full-screen flow rather
      than a row, so it wants a button that opens it rather than living inline.
