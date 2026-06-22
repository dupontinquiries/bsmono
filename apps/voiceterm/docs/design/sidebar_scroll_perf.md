# Sidebar scroll perf — investigation & on-device plan

Filed: "scroll in some sidebars (sitemap) is slow vs other sections."
v0.1.508 shipped the safe first fix (reuseItems + cacheBuffer). This
catalogues the remaining suspects with EVIDENCE from code-reading and
ranks them, so on-device profiling is targeted, not exploratory. None
of the below were changed blind — each is an appearance or behaviour
tradeoff that needs the running app to judge.

## The list in question

The browser-palette / sitemap drawer ListView (qml/main.qml ~23808),
bound to `browserModel`. Holds Actions, Site Map, Archive, Page Info,
Headings, Links, Interactive — can be 100+ rows.

## Findings, ranked by likely impact

### 1. Variable-height word-wrap delegate (LIKELY primary cost)
Delegate height: `model.isHeader?30:Math.max(_bt.implicitHeight+8,28)`
— bound to the Text's MEASURED height, with `wrapMode: WordWrap` +
`maximumLineCount: 2`. EVIDENCE: every other ListView in the app uses
FIXED height (`height: 40` at ~9364, ~10153) and scrolls fine. So the
sitemap is the outlier: QML must lay out each row's wrapped text to
know its height during a flick — a per-row cost the fixed-height lists
don't pay.
  - Fix options (ALL change appearance → on-device decision):
    a. Fixed single-line height + elide (lose 2-line wrap for long
       titles — they truncate). Cheapest scroll.
    b. Fixed 2-line height always (short rows get extra padding —
       changes spacing/density).
    c. Keep as-is if recycling (v0.1.508) made it acceptable.
  - Profile FIRST: does scroll stutter persist after v0.1.508? If not,
    leave it (a is a real visual change).

### 2. Per-keystroke full model rebuild in fuzzy filter (SECONDARY)
`applyFuzzy()` (~23845) does `browserModel.clear()` + re-append on
EVERY keystroke — destroying/recreating every delegate per character.
Combined with finding #1's variable heights, typing in the filter is
where it'd feel worst. Also `_fullPaletteData` is snapshotted via
`JSON.parse(JSON.stringify(row))` (deep-clone-by-serialization, a QML
anti-pattern) — but that's ONCE (guarded), so minor.
  - Fix options: debounce applyFuzzy (e.g. 80ms) so a fast typist
    triggers fewer rebuilds; or build the filtered list and diff
    against the model instead of clear+append. Behaviour-neutral
    (debounce) → safer than #1, but verify the feel.

### 3. cacheBuffer tuning (already partially addressed)
v0.1.508 set cacheBuffer: 800 (~2 screens). If scroll into fresh rows
still hitches, raise it; if memory matters on big lists, lower. A dial,
not a bug.

## On-device profiling plan (do in order)

1. Profile the sitemap drawer scroll AFTER v0.1.508. Is it fixed?
   - If yes → done; close the item.
   - If no → continue.
2. Toggle finding #1 option (a) (fixed-height elide) behind a quick
   local edit; compare scroll feel. If the visual truncation is
   acceptable and it fixes the stutter → ship it.
3. If filter-typing is the slow part (not raw scroll), debounce
   applyFuzzy (#2) — behaviour-neutral, low risk.
4. Re-profile; tune cacheBuffer (#3) only if fresh-row hitch remains.

## Why nothing else changed now

Findings #1 and #2 both trade appearance or behaviour for perf and
their benefit is unverifiable without the running app + a frame
profiler. v0.1.508's recycling fix was the one change that's pure
win (can't worsen scroll, no appearance change). The rest is a ranked
plan for the device, not blind edits.
