# TODO: graph view — stronger detent on perfect north alignment

## User report (flagged TWICE — recurring irritation, "STILL not enough detent")
When rotating/panning the graph (card) view near "north" (upright /
origin), there isn't enough magnetic pull to snap and HOLD at exact
alignment. You can drift slightly off and it doesn't catch. The existing
behavior feels under-damped at the top.

## Diagnosis (done — code-read v0.1.62x session)
`_snapToNorth()` (qml/main.qml ~19430) is an explicit ONE-SHOT action:
it sets scale→1.0, flicks to origin, tilt→0 in a single animated jump.
There is NO CONTINUOUS detent — nothing that catches you as you pan/zoom
THROUGH or NEAR north during a manual gesture. So the snap only happens
when the explicit action is invoked, not magnetically during free
movement. That's the gap.

## Fix direction (build-gated — gesture FEEL, unverifiable in sandbox)
Add a snap-when-near zone to the manual handlers:
- PinchHandler (~20365) + the pan Flickable (~20297).
- On release (onActiveChanged → false / onMovementEnded): if scale is
  within ~5% of 1.0 AND the flick/content offset is within ~N px of the
  north origin AND tilt within a few degrees, animate to exact north via
  the existing _north*Anim (reuse, don't add new animations).
- Tune the catch-zone width so it feels "grabby" near north but does NOT
  fight an intentional small offset (if the user clearly wants to sit
  slightly off-north, a too-wide zone is annoying). Start ~5% scale /
  ~20px / ~3° and adjust by feel.

## Why not done yet
Pure gesture-feel: the harness can't see "does it snap satisfyingly."
Needs on-device tuning, and should land ALONE (not stacked on other
render/gesture changes) so the feel is easy to judge and revert. The
diagnosis above means the wiring is well-scoped — it's a focused
build-and-feel turn, not an investigation.

## Cross-ref
Was previously buried in TODO_click_search.md (unrelated); promoted here
because it was reported twice and kept getting lost.

## v0.1.637 — first detent attempt (scale, needs feel-tuning)
Added a continuous detent to the PinchHandler (qml ~20365): onActiveChanged
→ when the pinch ENDS and graphRoot.scale is within ±8% of 1.0, animate it
to exactly 1.0 via the existing _northScaleAnim (target graphRoot.scale —
verified). Reuses the proven animator, no new animation surface. Catch
zone ±8% — wide enough to feel grabby, leaves intentional zoom alone.
UNVERIFIABLE in sandbox (gesture feel). TUNE on device: if it doesn't
catch enough, widen 0.08 → 0.12; if it fights intentional zoom, narrow to
0.05. NOT YET: pan-offset detent (snap contentX/Y near origin) and tilt
detent — add those once the scale detent's feel is confirmed, same
pattern. lint clean.

## Open-button regression (reported same turn) — DIAGNOSTIC NEEDED
"open button in graph view on each card no longer works." Traced: NONE of
the recent render/voice edits (v628-636) touched the card/preview/open
code. card_open_source (main.rs ~8758) + the Open buttons (qml ~21725+)
are structurally intact; kind-prefix match (p/c/w/f → load_page) looks
correct. Can't repro from reading. NEED a log: tap Open on a card that
should work, grep "cards" + "card_open". Two failure signals:
- "[cards] card_open_source: kind \"X\" has no source action" → find_node
  returns an unhandled kind, OR node lookup changed.
- toast "No source for this card" → card_open_source returned false: for
  p/f that means empty subtitle (URL/path), or find_node(id)=None (the
  card id the QML holds no longer matches an enumerated node).
Report which + the card kind → pinpoints node-lookup vs empty-subtitle vs
load_page regression. Do NOT blind-edit the open path.

## v0.1.644 — detent bug FOUND + fixed (this is why "still no detent")
The v0.1.642 runtime log showed: qml/main.qml:20406 "TypeError: Cannot
read property 'stop' of undefined" — fired on pinch-end. Cause: my v637
detent called `cardsCanvas._northScaleAnim.stop()` with the cardsCanvas.
qualifier, but _northScaleAnim is a bare `id` (a child element), NOT a
member property of cardsCanvas — so cardsCanvas._northScaleAnim was
undefined and .stop() threw on every pinch-end. The detent never ran; it
errored. The other call sites (19483/19516) correctly use the unqualified
`_northScaleAnim`. Fix: dropped the qualifier on all 3 lines to match.
Now the detent should actually fire. (±8% catch zone still a feel guess —
tune on device per the earlier note.) lint clean.
