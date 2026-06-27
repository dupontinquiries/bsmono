# Cardinal QWERTY — Layout Design Notes

## Status

**Undecided.** Candidate analysis below; user has yet to weigh in.

## Context

The cardinal-Dvorak layout (`default_cardinal_grid_dvorak`) was
redesigned with "Candidate B" principles: bigram-clustered hot cell
(`e t l r h` in top-middle), vowels-left/consonants-right balance,
counter-swipe satisfaction at word boundaries.

The cardinal-QWERTY layout (`default_cardinal_grid`) still uses the
older hand-position placement. Decision needed: should it adopt
Candidate B's bigram-cluster placement, or remain distinct?

## The core question

The cardinal grid is **its own paradigm**, not a remapping of
QWERTY's 3×10 structure into 3×3. So is there any sense in which
"cardinal QWERTY" should differ from "cardinal Dvorak"?

Two views:

### View 1 — There is no "cardinal QWERTY"

Bigram-cluster placement is layout-independent. `the` is `the`
regardless of what the user's full keyboard is. QWERTY muscle memory
doesn't transfer to one-thumb cardinal typing because:

- Cardinal uses one thumb on 3×3 cells; QWERTY uses 10 fingers on
  3×10 keys
- The cardinal grid's compass directions (N/E/S/W) have no analog
  in QWERTY's physical layout
- Frequency-optimized centers and bigram-paired swipes give the
  best ergonomics, derived from English statistics, not from QWERTY

**Implication:** ship the Dvorak Candidate B layout for both letter
systems. The "letters" config option becomes irrelevant when cardinal
mode is on. Maybe we hide that toggle when cardinal is enabled.

### View 2 — Cardinal QWERTY exists as a transition mode

Users who already touch-type QWERTY but are new to cardinal might
benefit from a layout where letters appear in QWERTY-adjacent
positions. E.g., `q` near the top-left, `p` near the top-right,
home-row letters in the middle row. This is a learning bridge:
users wean themselves off QWERTY by typing on a cardinal layout
that "looks like" QWERTY.

**Implication:** maintain two cardinal layouts. QWERTY-cardinal uses
QWERTY-zone placement; Dvorak-cardinal uses Candidate B's bigram
cluster.

## Three candidate layouts considered

### Candidate A — Hand-position (preserve QWERTY's L/R split)

```
   ┌─────────────┬────────────┬─────────────┐
   │      w      │      y     │      o      │
   │   q  e  r   │   t  u  i  │   .  p      │
   │      t      │      .     │      .      │
   ├─────────────┼────────────┼─────────────┤
   │      s      │     ⎵      │      j      │
   │   a  d  f   │  +arrows   │   h  k  l   │
   │      g      │            │      .      │
   ├─────────────┼────────────┼─────────────┤
   │      x      │   ⌫ ⏎ ⚙   │      m      │
   │   z  c  v   │            │   n  ,  .   │
   │      b      │            │      /      │
   └─────────────┴────────────┴─────────────┘
```

Tries to put left-hand QWERTY letters on the left, right-hand on
the right. Problems: `t`/`y` are on the QWERTY hand-boundary and
fight for the top-middle slot; `i`/`o` get squished into top-right;
some swipes repeat letters across cells.

### Candidate B — Same as Dvorak (bigram-cluster)

```
   ┌─────────────┬────────────┬─────────────┐
   │      '      │      t     │      y      │
   │   q  a  o   │   l  e  r  │   x  i  u   │
   │      ,      │      h     │      .      │
   ├─────────────┼────────────┼─────────────┤
   │      f      │     ⎵      │      c      │
   │   z  s  d   │  +arrows   │   w  n  g   │
   │      b      │            │      m      │
   ├─────────────┼────────────┼─────────────┤
   │      j      │   ⌫ ⏎ ⚙   │             │
   │      k  p   │            │   (blank)   │
   │      ;      │            │             │
   └─────────────┴────────────┴─────────────┘
```

Identical to Dvorak Candidate B. Frequency-optimal centers, hot
cell holds `e t l r h`. Argues that cardinal mode is layout-
independent.

### Candidate C — QWERTY muscle-memory transition

```
   ┌─────────────┬────────────┬─────────────┐
   │      q      │      r     │      p      │
   │   z  a  w   │   t  e  y  │   o  i  u   │
   │      x      │      f     │      j      │
   ├─────────────┼────────────┼─────────────┤
   │      d      │     ⎵      │      h      │
   │   c  s  v   │  +arrows   │   g  n  k   │
   │      b      │            │      l      │
   ├─────────────┼────────────┼─────────────┤
   │             │   ⌫ ⏎ ⚙   │             │
   │      m  ,   │            │   (blank)   │
   │      .  /   │            │             │
   └─────────────┴────────────┴─────────────┘
```

Centers are top-5 English letters (`a e i s n`); swipes pull from
QWERTY-adjacent positions where possible (`w` is N of `a`, etc.).
Inherently leaky because QWERTY's 30 keys don't fit 3×3 cleanly.

## My recommendation

**View 1 + Candidate B.** Use the Dvorak Candidate B layout for both
letter-system choices. The "letters" config toggle is meaningful for
the full keyboard (where it picks QWERTY vs Dvorak rows/columns) but
becomes a no-op for cardinal mode. We could hide the letters toggle
when cardinal is enabled, with a tooltip like "Cardinal mode uses a
fixed letter placement optimized for English bigrams."

But this is the user's call — there's a real argument for keeping
QWERTY-cardinal as a learning aid.

## To commit View 1

1. In `Layout::default_cardinal_grid()` (the QWERTY-cardinal
   factory), replace the body with the same content as
   `default_cardinal_grid_dvorak()`. Or:
2. Have `resolve_letter_layout` ignore `letters` when cardinal is
   enabled and always return `default_cardinal_grid_dvorak()`.
3. Update settings UI to grey out the Letters toggle when cardinal
   is enabled, with explanatory tooltip.

## To commit View 2

1. Pick which candidate (A or C). C is probably better since A has
   structural problems.
2. Write `Layout::default_cardinal_grid()` body to implement it.
3. Update settings UI to keep the Letters toggle active.
