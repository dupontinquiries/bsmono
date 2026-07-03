# Position freshness — wayfinder near-realtime usefulness

## The problem

The wayfinder points an arrow at where a peer is. It used to draw bearing +
distance whenever a location existed AT ALL — a fix from 3 hours ago produced the
same confident arrow as one from 30 seconds ago. A confident arrow aimed at a
stale position is worse than no arrow: it misleads.

The data to fix this already existed — `peer_locations.recorded_at` (indexed)
and `accuracy_m` — it just wasn't consumed at render time. (Storage hygiene was
fine: fixes are pruned after 7 days via purgeStaleLocations. But 7-day pruning is
orthogonal to presentation: a 6-day-old fix shouldn't drive a confident arrow.)

## The model (`freshness.rs`)

`confidence(policy, age_secs, accuracy_m)` → `Confidence`:
- **Live** — fresh (≤2 min) AND precise (≤50m): trust the arrow.
- **Recent** — ≤10 min, or fresh-but-imprecise: show it, mark approximate.
- **Stale** — ≤1 h, or fresh-but-coarse (>1km, e.g. IP-geo): direction is a hint.
- **LastKnown** — older: not a nav target, show "last seen N ago".

Two rules make it trustworthy:
- **Accuracy never improves the age tier** — a perfectly precise 2h-old fix is
  still LastKnown.
- **Coarse caps freshness** — a 5km IP-geo fix, even seconds old, never reads as
  Live (it's capped at Stale).

`presentation(conf)` → SolidArrow / FadedArrow / LastSeenLabel.
`opacity(conf)` → 1.0 / 0.85 / 0.55 / 0.35 (monotonic; the renderer fades the
arrow + avatar as confidence drops).
`last_seen_label(age)` → "15s ago" / "5m ago" / "3h ago" / "2d ago".

Thresholds live in `FreshnessPolicy` (tunable; the FFI exposes an `_ex` variant
that takes them explicitly so they can come from config).

## Integration (C++ wayfinder)

Per node in `refreshLayout`, after fetching the peer's `LocationFix`:
```cpp
const int64_t age = fix.recordedAt.secsTo(QDateTime::currentDateTimeUtc());
const int conf = sc_loc_confidence(age, fix.accuracy_m);
const int tier = sc_loc_presentation(conf);
node.opacity   = sc_loc_opacity(conf);
if (tier == SC_LOC_TIER_LAST_SEEN_LABEL) {
    char buf[16]; sc_loc_last_seen_label(age, buf, sizeof buf);
    node.distanceLabel = QString::fromUtf8(buf);   // "3h ago" instead of "1.2km NW"
    node.showArrow = false;
} else {
    node.showArrow = true;                          // faded if tier == FADED_ARROW
}
```
Header: `include/location/snapclone_freshness_ffi.h` (C + C++).

## Status

14 tests (10 model + 4 FFI); in-workspace green; header verified C + C++;
staticlib linked + run from C++ across realistic fix ages (Live/Recent/Stale/
LastKnown + the fresh-but-coarse demotion). The C++ wayfinder wiring (reading the
tier into paint) is the remaining integration step — pure presentation, no data
migration, validates on a device where the arrows are actually visible.
