# TODO: "click X" → search-then-act (replace blind click_link)

## Why
Voice "click configure now" currently does executeIntent kind=click_link
param="configure now" — a blind immediate click on whatever matches that
text. The demo log showed it firing on a fuzzy/ambiguous target. Instead:
SEARCH the page for matching elements, then present per ambiguity.

## Done (pure, tested — v0.1.626)
ui_landmarks::plan_click(phrase, elements, few_max) -> ClickPlan:
- NoMatch — nothing above floor.
- ConfirmOne{id, label_score} — one strong match (score >= 0.6 AND >= 0.3
  ahead of #2). Show a confirm affordance, NOT a silent auto-click.
- ChooseFew{ids} — 2..=few_max matches → numbered list, pick by voice or
  tap.
- ManyBadges{ids} — > few_max → number badges on elements + rotating word
  suggestions, live voice picks by number.
Built on the existing resolve() scorer (label/color/shape/position). 5
tests incl. the "configure now"→ConfirmOne case and near-tie→not-confirm.
few_max default 5 (the "few vs many" cutover from your spec).

## Wiring (build-gated — needs the running app)
1. **Element registry**: the page/renderer must expose clickable elements
   as ui_landmarks::Element { id, label, zone, color, shape }. For web:
   harvest links/buttons/(text elements — TENTATIVE, "to clip maybe")
   from the DOM/render tree with stable ids + visible labels + their
   on-screen zone. This is the main new plumbing.
2. **Intercept the intent**: where executeIntent handles click_link
   (qml ~ the [ui] executeIntent kind=click_link path), instead call a
   backend method that runs plan_click(phrase, registry, 5) and returns
   the ClickPlan (serialized).
3. **Render each bucket**:
   - ConfirmOne → small confirm chip "Click 'Configure now'? [yes/no]"
     (voice "yes"/tap). On yes → the existing click-by-id path.
   - ChooseFew → numbered overlay list (1..n) with labels; voice says the
     number/label OR tap. (phonetic number words → index.)
   - ManyBadges → draw numbered badges ON the elements + a rotating
     animated set of word suggestions; live voice number picks.
4. **Selection**: reuse voice_intent positional/number parsing for "two"/
   "number 3"/label; on pick → click-by-id.

## Notes
- "search ... text elements to clip (maybe)": include plain text elements
  only tentatively — they bloat the candidate set and rarely are the
  click target. Suggest links+buttons first; add text elements behind a
  flag if the demo wants it.
- ConfirmOne threshold (0.6 / 0.3 gap) is tunable in plan_click if the
  demo shows it confirms too eagerly or too rarely.

## Verified / boundary
plan_click fully tested in-sandbox (927 pure tests green). The element
registry + QML presentation need the build. The pure decision means the
QML just renders 4 cases + reports the pick — no matching logic in QML.

---

# SEPARATE: graph view north-detent
Moved to its own file → TODO_graph_north_detent.md (it was reported twice
and kept getting lost buried here under an unrelated feature).

## WIRED v0.1.724 — search-then-act replaces blind click
- click_search(phrase) qt_method: harvests page links + buttons/inputs/[role=
  button] as ui_landmarks::Element, runs plan_click(phrase, 5), serializes the
  ClickPlan: "none" / "confirm\t<id>\t<label>\t<score>" / "few\t…" / "many\t…".
- executeIntent click_link now routes through click_search instead of the blind
  find_link_fuzzy nav: ConfirmOne → confirm chip ("Click 'X'?"), ChooseFew/
  ManyBadges → numbered chooser Popup (clickChooser), NoMatch → find-on-page.
  control:<label> ids (buttons) fall back to find-on-page on pick.
- Verified buckets end-to-end: unique match → confirm, ambiguous "configure"
  (two options) → few (the blind-click bug, fixed), no match → none.
RUNTIME-UNTESTED: the chooser Popup + voice number-pick routing need a live GUI;
the plan logic + wire are unit-tested. NOTE: page links/buttons have no geometry
from the HTML parse, so zone is neutral (Center) — a render-tree harvest could
fill zone/color/shape later for "the red button on the right" style picks.
