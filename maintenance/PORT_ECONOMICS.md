# Port programme: measured, not estimated

Numbers taken from the tree at c4-522. Every figure here is reproducible with
the commands in `maintenance/tools/parity_audit.py` and the counts below; where
a number needs a caveat to be honest, the caveat is next to it rather than in a
footnote.

## 1. How much code the ports are

| app | port code | port tests | tests as % |
|---|---:|---:|---:|
| snapclone | 7,693 | 5,632 | 42% |
| bo-suite | 5,837 | 3,645 | 38% |
| voiceterm | 5,254 | 3,035 | 37% |
| rusty-board | 2,328 | 1,223 | 34% |
| bot-arena | 477 | 246 | 34% |
| fao-codec | 480 | 95 | 17% |
| krapper | 250 | 125 | 33% |
| **total** | **22,319** | **14,001** | **39%** |

Two-fifths of the port is tests. That is deliberate and it is not overhead being
hidden: the tests are where the ported *behaviour* is pinned — the rubber-band
curve, the DECCKM split, the queue's `queue_id`, the detent thresholds. Compare
the code column against the old UI, not the total.

## 2. Against the UI they replace

| app | old UI | port code | ratio |
|---|---:|---:|---:|
| snapclone | 56,193 | 7,691 | 0.14 |
| voiceterm | 34,708 | 5,246 | 0.15 |
| rusty-board | 19,886 | 2,326 | 0.12 |
| bot-arena | 2,523 | 476 | 0.19 |
| bo-suite | 17,270 | 5,836 | 0.34 |

**~6x smaller on average — and that number is flattered in three ways.**

1. **The ports are not at parity.** Named-surface coverage runs 80–100%, but
   code-referenced coverage is 18–73%. Several "covered" surfaces are named in a
   module doc and implemented under a Rust name; several others are genuinely
   thinner than the original.
2. **snapclone's 56,193 counts headers and implementations** — 44,008 of `.cpp`
   plus 12,063 of `.hpp`. A C++ UI declares everything twice. Against the `.cpp`
   alone the ratio is 0.17.
3. **QML boilerplate inflates bo-suite and voiceterm.** Anchors, `Behavior on`
   blocks and property declarations are lines that egui does not need and does
   not replace with anything.

bo-suite's 0.34 is the most honest of the five: it is the app whose port is
furthest along in *behaviour*, not just in names.

## 3. What the shared kits actually save

| kit | LOC | apps depending on it |
|---|---:|---:|
| uikit-egui | 5,807 | 8 |
| uikit-term | 2,449 | 1 |
| uikit-web | 1,231 | 1 |
| snapclone-theme | 759 | 6 |
| uikit-gesture | 538 | 3 |
| **total** | **10,784** | |

`uikit-egui` at eight consumers is the whole argument for the kit approach: 5,807
lines written once instead of eight times. Naively that is ~40,000 lines not
written, though the honest version is smaller — a per-app implementation would
have been leaner than a general one, so call it a real saving of the same order
but not eight times over.

`uikit-term` and `uikit-web` each have ONE consumer today. They are not yet
paying for themselves as shared code; they are paying for themselves as *tested*
code, which is a different and weaker claim.

## 4. The bigger saving is not the kit

**7,922 lines of engine were included rather than rewritten**, via `#[path]`:

```
entity_extract 1,902   terminal_workspace 1,302   table_ops 1,126
dumb_summary     964   table_nlp            739   nlp         650
row_cache        527   session_walk         336   log         206
util             170
```

That is a third as much again as the entire voiceterm port, and it is code that
would otherwise have been duplicated with a second parser, a second dirty-row
tracker, a second logger — each drifting from the original at its own rate. It
was available because those modules were Qt-free, which is the property worth
protecting in future work.

It also has a cost, recorded honestly: including a module drags its
crate-internal dependencies (`entity_extract` → `log` → `util`), and one module
could not come across at all — `manpages.rs` needs its Qt-free half split out
first, and that refactor cannot be verified here because the app crate's build
script needs `qmake`.

## 5. Is it getting easier?

Weak evidence, stated as such.

* **Module size trend.** First eight modules averaged 325 lines of code; last
  eight averaged 247 — about 24% smaller. But later surfaces were also simpler
  and more logic-shaped, so this measures two things at once.
* **Kit usage per module** rose from ~1.1 direct references to ~1.9 between the
  first and last eight. Real but small, and a low absolute number either way —
  most kit use is indirect, through `PaletteColors` and the gesture layer.
* **The strongest evidence is qualitative and worth more than either number:**
  the last six ports needed no new drawing primitives. They compose what is
  already there. `table_view`, `row_render`, `detect`, `explore`, `manpage_view`
  and `studio` are all *views over existing engines* — which is what the port
  becoming easier actually looks like from the inside.

The reason it got easier is therefore only partly eguikit. The larger part is
that the engines underneath were already Qt-free, and the port could spend its
time on the surface rather than on re-deriving behaviour.

## 6. Projected finish

At current ratios, and assuming the remaining named surfaces cost about what the
last thirty did, the ports land somewhere near **28,000–32,000 lines of code plus
~18,000 of tests**, against roughly **130,000 lines of old UI**. Call it 4–5x
compression rather than the 6x the current snapshot suggests, because the cheap
surfaces went first.

That projection excludes three things that are not going to be written by
finishing the list: the codec backends (`V2Encoder`, `V8GpuRasterizer`), the
Qt-side test harnesses, and `Dbus`. `PORT_STATUS.md`'s "what is actually left"
section has the full accounting.
