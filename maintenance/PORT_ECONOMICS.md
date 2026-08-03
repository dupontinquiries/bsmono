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
* **The qualitative claim I first made here was wrong**, and checking it is what
  produced this section's most useful finding. I wrote that "the last six ports
  needed no new drawing primitives". Two counts say otherwise:
  - The kits WERE extended during the programme, twice, and I had not counted
    it: `uikit-web/doc.rs` (a whole new document model) and `uikit-term/keys.rs`
    (DECCKM, bracketed paste, canonical line endings).
  - Worse, generic widget behaviour was written into APP code rather than the
    kit. `grep -l ripple` finds it in two apps; `breath|pulse` in eight modules;
    `ease::out_cubic` entrances in five. snapclone's `GlassButton` and
    bo-suite's `StepKey` came out **line-for-line identical** — same fields,
    same tick, byte-identical `(1.0 - press_scale) * dt / press_ms` ramp —
    written three sessions apart without either noticing.

  That is the "two lists of the same thing" failure this programme keeps finding
  in the Qt code, committed in the port itself. It is now fixed:
  `uikit_egui::motion::PressFeedback` holds the behaviour and both apps delegate
  to it, keeping their own constants — a B&O key is firmer than a snapclone one,
  and a kit that flattened that would be the opposite mistake.

So the honest answer is that the port got easier for two reasons and neither is
"the kit is finished". The engines underneath were already Qt-free, which is the
larger effect; and where the kit was genuinely short, I routed around it instead
of extending it. The remaining duplication above — breathing glows, entrance
easings — is the same smell and has not been promoted yet.

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

## 7. Promotions, and what stayed put

Three behaviours have now been lifted out of app code into
`uikit_egui::motion`, each because it had been written more than once:

| promoted | written independently in | what the copies had to learn separately |
|---|---|---|
| `PressFeedback` | snapclone `GlassButton`, bo-suite `StepKey` | the ripple must outlive the release |
| `Reveal` | snapclone accordion + bottom sheet, bo-suite device panel | it must stay drawn while it LEAVES |
| `Breath` | dial glow, anticipation cue, queue equalizer, peel hint, voice bar | phase must wrap; offset taps must not move in unison |
| `fmt::relative_time` | bo-suite library + sources, snapclone outbox + snap feed | a future timestamp must not read as a negative age |
| `fmt::plural` / `badge_count` | eight modules across three apps | zero is plural; a zero badge is absent |
| `gesture::rubber_band`, `VelocityTracker` | snapclone `swipe_physics` | resistance must never outrun the finger; a decelerating swipe was aimed at stopping |

The middle column is the argument. Each of those rules is a bug someone hits
once and fixes locally, and the third implementation hits it again.

**What deliberately did not move**, because "extract everything" is its own
failure mode:

* **Constants.** bo-suite's key dips 8% over 90ms, snapclone's 12% over 110ms;
  a card breathes over 1.8s at 0.35 peak, a securing handshake over 1s at full.
  Those are house feels. A kit that flattened them would make every app move the
  same way, which is worse than the duplication it removed.
* **`StepKey::flash`.** Tied to key-repeat — hold "−" and each step flashes
  though there was one press. snapclone's buttons have no equivalent, so sharing
  it would put a field in the kit that one consumer never sets.
* **The `Card` chevron, the sheet geometry, the peel edges.** Each is one app's
  reading of what the shared motion means, and that is where an app's character
  actually lives.

The test for whether a promotion is right is not "is this code identical" but
"did the second author have to rediscover a rule". All of the above pass it; the
constants fail it.

### The audit finds bugs, not just duplication

Two of these promotions turned up a real defect rather than mere repetition,
which is the strongest argument for doing them at all:

* **`relative_time`** — three of the four copies guarded against a negative
  interval; snapclone's outbox did not. A device reporting a future timestamp
  would have rendered "-4m ago". Nobody had seen it because you need a skewed
  clock to reproduce it. Delegating fixed it; a test now pins it.
* **`uikit-web` had two parsers that disagreed about what prose is.**
  `doc::parse_page` drops `<script>` and `<style>`; `parse_html` — the path the
  manual reader and the mail view use — had no notion of them and rendered
  `var x = 1; p{color:red}` into the document. The same page shown in two views
  would differ, which reads as a broken page rather than a bug and so goes
  unreported. Both now share one `is_noise_tag`, with a parity test that fails
  if they ever diverge again.

* **`uikit-gesture` already had the page-commit rule, and the port re-derived
  it.** `swipe_physics.rs` carried `COMMIT_FRACTION = 0.22`,
  `FLICK_PX_PER_MS = 0.30` and `FLICK_MIN_PX = 14.0` — character for character
  the values already in `GestureConfig`, whose own comment says it mirrors
  snapclone's `Theme.hpp`. The port worked forward from the Qt source to
  constants the kit had already been given. Now read from the kit, so a retune
  lands once.

  And the two disagreed about something real: the kit took flick direction from
  the **velocity**, the port from the **drag**. They differ only on a late
  reversal — 100px left then a 5px snap back right before lifting — where the
  velocity's sign turns the page the opposite way to how it visibly moved. The
  kit had no test covering that case, which is why it survived. The port's rule
  won and is now the kit's.

* **The easing guard was in a private copy.** bo-suite's scroll module had its
  own `out_cubic` that clamped `t` to 0..=1; the shared one did not, so every
  caller had to remember — and several were writing
  `out_cubic(x.clamp(0.0, 1.0))`, which is the tell. The clamp now lives in the
  easing itself and the redundant call-site clamps are gone.

  **No live bug was found here**, and that is worth stating plainly rather than
  dressing the change up: both unguarded call sites — `uikit-term`'s block slide
  and the confirm dialog's appear — bounded their input by other means. The
  change removes a class of future bug and a thing every caller had to know, not
  a defect on screen today. `out_back` still overshoots above 1 on the way,
  because that is the effect; what is clamped is the input, not the output.

* **A fix shipped into a kit that nothing could reach.** `keys::encode_mode`
  gained an application-cursor branch — the DECCKM split, ported specifically to
  fix arrow navigation inside fzf, vim and less. But `uikit-term`'s parser
  dropped the `?` in `CSI ? 1 h` as an "intermediate byte" and dispatched it as
  a plain `CSI 1 h`, so the mode never arrived. `app_cursor: true` existed
  **only in test constants**. The branch was unreachable in production from the
  day it was written, by me.

  Same class as `WayfinderPage` — a name that counts as covered while nothing
  runs — except here it was a kit feature rather than an app surface, and no
  audit would have caught it: the code was present, tested, and wired to
  nothing. The parser now tracks DEC private modes, `Grid::key_mode()` hands
  both flags over in one call, and a test walks the whole path from
  `CSI ? 1 h` to an arrow changing shape.

  It also fixed a quieter bug: `CSI ? 25 l` (hide cursor) had been arriving as
  `CSI 25 l` and being taken for an ANSI mode reset. Unhandled private modes are
  now ignored rather than mis-dispatched, because doing nothing is debuggable
  and doing the wrong thing is not.

### Kits still unaudited

`uikit-term`'s grid and parser have now been compared against voiceterm's
`pty_term.rs`. They are NOT duplicates: `pty_term` is a full emulator (alt
screen, scrollback, full SGR) and `uikit-term`'s is a renderer-side subset. The
gap that mattered was the private modes, above. Alt-screen (`CSI ? 1049 h`)
remains unhandled in the kit and is the next real difference. `uikit-term` and `uikit-web` also still
have one consumer each — they carry kit-shaped cost without kit-shaped payoff,
and that stays true until a second app embeds them.
