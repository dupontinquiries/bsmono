# What voiceterm can learn from cosmic-term

Reference: System76's `cosmic-term` (6,270 lines) — iced/libcosmic front end,
`alacritty_terminal` back end, `cosmic-text` for shaping. Read for performance
technique, not for code to copy: voiceterm's terminal is already a different and
in places *better* design.

## What voiceterm already does as well or better

* **Packed cells.** `Cell` carries its attributes as bitflags inline. cosmic-term
  interns a `Metadata` struct into an `IndexSet` per frame and stores the index
  in the shaper's attrs — a good trick, but only because cosmic-text's attribute
  spans are heavy. voiceterm doesn't need it.
* **Run-length styled runs.** `render_row_runs` groups same-style cells before
  emitting, which cosmic-term gets implicitly from attribute spans.
* **Per-row dirty tracking** (`dirty_rows`, `mark_row_dirty`) with a per-row
  packed-string cache invalidated exactly where rows are marked dirty. This is
  *finer* than cosmic-term, which currently reads `term.damage()` and then throws
  the answer away — the code has `TermDamage::Partial(_damage_lines) => {}` with
  a `//TODO: use damage?` beside it.

## What was worth stealing

**A scroll is a shift, not a change.** This is the one real lesson, and it is
structural rather than clever. alacritty keeps the grid in a ring buffer, so
scrolling bumps an index and every row keeps its identity; caches keyed to rows
survive. voiceterm stores rows in a `Vec` and scrolled by `remove(top) +
insert(bot)`, then called `mark_all_dirty()`, which dropped the entire packed
cache. The code even flagged it:

> *"A future cache could special-case this as a blit; for now correctness over
> cleverness."*

Because a newline at the bottom of the screen is a scroll, **every line of
streaming output re-packed the whole screen** — O(lines × rows), the classic
`cat`-a-big-file cliff.

The fix keeps the `Vec` storage and shifts the cache in lockstep with the rows it
describes (`scroll_cache_up` / `scroll_cache_down`), so entry *i* still describes
whatever now sits at row *i*. The renderer is still told every row is dirty —
they all moved on screen — but being dirty now costs a memcpy of a cached string
instead of re-formatting it from cells.

Measured on the isolated grid (500 lines of streamed text, reading every visible
row each frame the way the renderer does):

| window | before | after |
|---|---|---|
| 24×80 | 11,494 repacks, 8.18 ms | **1,000 repacks, 1.32 ms** |
| 50×200 | 22,648 repacks, 19.60 ms | **1,000 repacks, 1.96 ms** |

6–10× faster, and the shape changed: the cost used to grow with window height
(23× ideal at 24 rows, 45× at 50) and is now flat at 2.0× regardless. Two tests
pin it — one that repacks must not scale with row count, one that a shifted cache
still equals a freshly packed row, because a cache that is fast and wrong is
worse than the slow one it replaced.

## Still worth taking, in rough value order

1. **Coalesce PTY wakeups into one rebuild per frame.** cosmic-term's PTY events
   only set `needs_update = true`; the rebuild happens once, lazily, at draw
   time. A burst of a thousand writes costs a thousand flag stores and one
   rebuild. Worth auditing whether voiceterm's read path can do the same.
2. **Damage-limited *drawing*, not just repacking.** We now avoid re-packing on
   scroll but still repaint every row. A GPU or blit path could scroll the
   rendered surface and repaint one line.
3. **Copy-on-write the grid snapshot** (`Arc::make_mut`) so the render side can
   read without holding the parser's lock.
4. **Attribute interning** *if* voiceterm ever moves to a shaping text engine
   where per-span attributes get expensive; not before.

## What not to copy

cosmic-term's per-frame `metadata_set.truncate(1)` and full `display_iter()` walk
rebuild every visible line's text on every update, leaning on cosmic-text's
`set_text` returning "did this change?" to skip re-shaping. That is a reasonable
trade when your text engine owns shaping caches; for voiceterm it would replace a
precise per-row invalidation with a full walk plus a string compare per row.
