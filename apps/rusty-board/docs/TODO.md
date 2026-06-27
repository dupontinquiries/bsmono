# RustyBoard — Filed Work Items

This file tracks pending features and known issues, with enough
context that future sessions can pick them up without re-deriving the
design.

## Recently fixed (keep here a couple rounds for reference)

- **Key repeat wired to engine** — `add_press_release_callbacks` on
  KeyWidget uses the button's `state-flags-changed` signal
  (GTK_STATE_FLAG_ACTIVE) for press/release, not GestureClick (whose
  cancel didn't fire reliably when a parent swipe gesture claimed the
  sequence, leaving the repeat timer running forever = "delete always
  on" bug). Repeats gated to Backspace/Tab/arrow Keysyms via
  `is_key_repeatable`. Letters/space deliberately don't repeat.

- **Experimental mode separated from base layout** — new
  `cfg.layout.experimental_mode` ("none"/"cardinal"/"stenotype").
  `letters` is now strictly qwerty/dvorak. Settings: Letters picker
  (2-way) + Keyboard-mode picker (3-way) in Experimental. Config
  load migrates old polluted `letters` values and the deprecated
  `cardinal_enabled` bool. Fixes the dvorak+cardinal→qwerty switch
  and the settings/voice-tap reverting to default layout.

- **Steno keyboard session 1** — `LayoutStyle::Stenotype`,
  `KeyAction::Steno`, `src/steno/mod.rs` (pair-stroke engine with
  generation-counter timeouts, phoneme table, 10 briefs),
  `default_stenotype()` 4-button layout, dispatch intercepts in
  keyboard_view, CSS approximating the \\// parallelogram lean.
  Word-expansion is the filed follow-up (see High priority).

- **Glass effect** — `cfg.appearance.glass_effect`, settings toggle,
  `.glass-keyboard` CSS (uniform tint + backdrop blur + single top
  highlight; the earlier multi-gradient version was tacky). Live-
  toggle via `set_glass_effect`. Fluidity expansion filed (High pri).

- **Voice readiness FSM** — `src/voice/readiness.rs` three-state
  (Asleep/Warm/Active) machine drives polling cadence and pre-warm/
  release. 1Hz coordinator pre-warms transcriber on Warm, releases
  on Asleep. Cut idle timer wakeups from ~90/sec to ~15/sec (level
  pump self-reschedules at FSM cadence; event bridge 33→100ms;
  engine bridge 16→33ms).

- **Cardinal letter centering** — empty cardinal cell slots get an
  invisible `hint_spacer` so the 3×3 grid's homogeneous sizing
  centers the main label even when some directions are unmapped.

- **Canvas-open made deliberate** — short up-swipe on space → Up
  arrow; only a long (≥70px) up-swipe opens the drawing canvas, and
  only when no experimental mode is active. Stops the canvas popping
  open and shifting the whole UI during normal typing.

- **Disappearance bug** (regression where `install.sh` writes a
  user-level `.desktop` file lacking `X-KDE-Wayland-VirtualKeyboard=true`
  that shadows the correct system-level file from `plasma-setup.sh`).
  Fixed: install.sh template now includes the magic line; plasma-setup.sh
  detects shadowing files and offers to remove. Troubleshooting entry
  in `scripts/README.md`.

- **Install/redeploy path divergence** — install.sh's user-install
  default puts the binary at `~/.local/bin/rusty-board` but
  redeploy.sh previously always wrote to `/usr/local/bin`. Fixed:
  redeploy.sh now auto-detects the install path by reading the
  existing .desktop file's `Exec=` line; install.sh now warns when
  the user-install path will diverge from redeploy.sh's default.

- **Config parse-error recovery** — a broken `config.toml` (e.g.
  user edits a typo or a struct schema change post-load) used to
  fail the whole startup. Now: rename the broken file to
  `.toml.broken` and start with defaults. User keeps a working
  keyboard and can recover their settings from the backup file.

- **Removed dead `wayland_backend_active`** — vestigial fn and
  atomic in window.rs; one dead-code warning down.

- **`scripts/sanity-check.sh`** — diagnostic script that walks the
  registration chain (binary, .desktop, kwinrc, KWin process,
  log file) and reports OK/WARN/FAIL with hints. Doesn't modify.

- **`--debug-layout` CLI flag** — dumps a per-combo per-row
  geometry table to stdout, useful for diagnosing overflow on
  device.

- **Candidate B Dvorak cardinal** — committed in
  `Layout::default_cardinal_grid_dvorak()`. Hot cell `e t l r h` in
  top-middle, vowels-left/consonants-right balance.

- **Thumb-arc compaction sub-round 1** — settings toggle, default
  arc by screen width, scanline row-fitting math, OUTER_MARGIN
  accounting in `compute_unit_width`.

- **Anchoring overflow** — fixed by re-budgeting `compute_unit_width`
  for `effective_cols = cols + max_offset` plus margins/borders.

- **Color picker UX** — single shared CSS provider, "Default" as
  first swatch, border on selected, no separate "Use default" button,
  fixes `colors reset on reopen` via reload-from-disk.

- **Layout-overflow test harness** — `#[cfg(test)] mod
  layout_overflow_tests` in `keyboard_view.rs` validates 13 combos × 4
  screen widths. Pure helper `compute_unit_width_from_screen_w`
  extracted for testability.

- **Utility row dominant-side flip** — `default_cardinal_grid_dvorak_for_anchor(anchor)`
  swaps ⚙/⌫ based on the user's `cfg.layout.thumb_arc.anchor`.

- **Springboard space (boundary-crossing path segmentation)** —
  Already wired in the drag-end handler at `keyboard_view.rs:471`.
  Per-segment recognition + trailing-space insertion + spacebar
  pulse animation on word boundaries.

## High priority (do next)

### Swipe code audit refinement

**Filed during the perf/state-bug session.** The swipe dispatch path
in `keyboard_view.rs` has accreted special cases — minimize-fling,
spacebar-arrow, canvas-open (now gated to long-up-swipe + non-
experimental), top-row-digit, cardinal dispatch, steno intercept,
and the recognizer pipeline — all sharing one `dispatch_*` entry and
gated by an `is_cardinal` boolean that now means "cardinal OR steno."

**What the audit should cover:**
- Replace the overloaded `is_cardinal` bool with an explicit
  `enum DispatchMode { Standard, Cardinal, Stenotype }` derived once
  from the layout, so the three gates (lines ~734, ~3120, ~3190)
  can't drift out of sync the way they did this session.
- Document the precedence order of the swipe interceptors as a single
  comment block at the top of the swipe-source branch — right now you
  have to read all of them to know what wins.
- Verify each early-return actually consumes the gesture (no path
  falls through to the recognizer after a partial match). The
  canvas-open bug was exactly this: a swipe matched the spacebar-up
  detector, opened a canvas that didn't render, and the gesture
  partially leaked.
- Unit-test the classifier boundaries (the 28px/70px/dominance
  thresholds) with synthetic paths; right now they're only validated
  by on-phone feel.
- Consider extracting the interceptor chain into a
  `Vec<Box<dyn SwipeInterceptor>>` with a clear "claimed/passed"
  return, so adding a new gesture doesn't mean editing a 200-line fn.

### Fluidity: transitions, glass, and live effects

**Filed during the glass-effect work.** The glass effect is static
(uniform tint + backdrop blur). Expanding fluidity:
- **Layout transitions** — animate the keyboard rebuild when
  switching pages (letters ↔ symbols ↔ numbers) and modes. Currently
  `rebuild_view` clears + repopulates instantly, which reads as a
  jarring pop. A crossfade or slide would help. GTK4 `gtk::Stack`
  with a transition type is the natural fit, but the keyboard_box
  isn't a Stack today — would need restructuring.
- **Key press ripples** — extend the existing `Animator::key_press_pulse`
  with a material-style ripple originating at the touch point.
- **Live glass** — make the glass tint react to what's behind it
  (requires the KDE blur protocol / `org_kde_kwin_blur_manager`, see
  the screen-bleed note) or at minimum animate the top highlight on
  scroll/typing so it feels alive rather than a flat sheet.
- **Microterm reveal** — the microterm revealer slide is functional
  but abrupt; tune the easing curve and duration to match the glass
  aesthetic.
- **Cardinal/steno gesture trails** — fade the trail with a subtle
  glow rather than a hard line; reuse `swipe_trail.rs` infrastructure.
- Gate all of this behind a perf budget: the FP5 is the constraint.
  Effects should degrade gracefully (a "reduce animations" setting,
  or auto-disable when the glass effect is off).

### Word expansion with the steno keyboard

**Filed during steno session 1.** The steno engine currently commits
raw phoneme pairs or one of 10 hardcoded briefs. The big-bandwidth
payoff (the whole point of steno — fewer taps, more words) needs a
real phoneme→word expansion layer.

**Implementation plan:**
- Build/import a phoneme→word dictionary. CMU Pronouncing Dictionary
  (ARPAbet, ~134k words, public domain) is the obvious source. Map
  its ARPAbet to our internal phoneme alphabet (the
  `build_phoneme_table` keys in `steno/mod.rs`).
- A trie or FST over phoneme sequences. As pair-strokes accumulate
  into a phoneme buffer, walk the trie and surface candidate words.
- Wire the existing suggestion-bar placeholder (the
  `show_suggestions` appearance flag) to display top-N candidates;
  tap to commit, or keep stroking to disambiguate.
- Brief table expansion: mine the top ~200-1000 English words for
  intuitive 1-2 stroke briefs (see the steno design discussion —
  Plover's dictionary is a pattern source, not a direct import,
  since our chord space is 100 pair-strokes not 22-key chords).
- Schwa-disambiguation: the design collapses unstressed vowels to
  schwa, so the expander must handle "consonant skeleton + approx
  vowels → ranked real words" like a pinyin IME. This is the hard
  part and where most of the value is.
- Spanish phoneme additions (/ɲ x ɾ/) deferred to a later round.

### Wall-rebound / bounding-box abstractions for gesture handling

**Filed during cardinal-gate work.** When a user swipes off the
screen edge during a gesture, the path just terminates with no
visual feedback or recovery. A "wall rebound" model would treat
screen edges as elastic and reflect the perceived intent back into
the keyboard region.

**Concrete use-cases:**
- Cardinal swipe on a right-edge cell going east: user's thumb
  exits the screen before the swipe is "long enough" to classify.
- Springboard swipe that exits the bottom edge of the keyboard
  during a long word.
- Single-hand mode's compacted layout having keys near screen edges
  where overshoot is the norm.

**Implementation sketch:**
- Introduce a `Bounds` struct in `src/gestures/bounds.rs`:
  `{ left, right, top, bottom: f64 }`. Comparison helpers
  (`contains(p)`, `nearest_inside(p) -> SwipePoint`, etc.).
- During `connect_drag_update`, when a sample exceeds the bounds,
  push two samples: the literal one (so timing math stays right)
  AND a clamped-to-edge version. Classification reads the clamped
  one; analytics could read the literal one.
- Settings: per-edge elasticity (in / out / both). Off by default;
  enable for power users.

**Required for the cardinal-grid right column** when single-hand
mode shrinks the keyboard such that natural thumb arcs overshoot.
Without this, single-hand-mode-right users will hit the right edge
constantly mid-gesture.

### Cardinal QWERTY layout decision

See `docs/cardinal-qwerty-design.md` for the three candidates and
my recommendation. Needs user input on whether to use Candidate B
(same as Dvorak, bigram-cluster) or maintain a separate
QWERTY-flavored cardinal as a learning bridge.

### Thumb-arc sub-round 2

Compaction sub-round 1 is shipped (settings toggle, default arc by
screen width, scanline row-fitting). Sub-round 2 covers:

1. **Springboard space (boundary-crossing path segmentation)** —
   **DONE.** Wired in the drag-end handler at `keyboard_view.rs:471`
   via `segment_path` + per-segment recognition + trailing-space
   insertion. Tested via on-phone typing.

2. **Calibration UI** — user-traced thumb arc. A separate panel
   accessed from settings that puts the screen in "draw your arc"
   mode; user drags a finger; the resulting polygon is normalized
   and saved to `cfg.layout.thumb_arc.polygon`.

3. **Multi-anchor space** — vertical strips of space-targets on the
   dominant side, so reaching space doesn't require crossing to the
   center cell. The data structure: extend `Layout` with an optional
   list of additional space-cell positions. Render them as
   visually-grouped strips with a single shared spacebar feel.

4. **Post-word highlight pulse** — when a word ends at a space cell,
   briefly flash the cell with the accent color. Teaches the
   springboard mechanic and gives positive feedback.

### Test binary CLI flag

**Done as a `#[test]` module.** Wrapping it as a CLI flag
(`--debug-layout`) is still filed. Would dump per-combo per-row
geometry to stdout, useful for on-device diagnosis without rerunning
the test suite. Small fn that calls the same pure helpers, prints a
table.

## Medium priority

### Telemetry / A/B layout testing

**Filed during Candidate B discussion.** The eureka was that
typing involves bigram traversal patterns (`e → r → e → l → ...`)
that can be measured for ergonomic quality.

**Metrics:**
- Tap-to-swipe ratio
- Flip frequency (left side → right side transitions)
- Counter-swipe-at-end-of-word frequency (release after consistent
  direction)
- Per-thumb attribution (heuristic: source-cell column +
  middle-column-direction → left/right thumb)
- Thumb balance %

**A/B mode:** an opt-in flag that occasionally swaps one letter's
position per N words and measures the metric delta. Run for a week,
keep the placements that improved the metrics.

**Output:** `~/.local/state/rusty-board/typing-stats.toml` updated
periodically.

### Deprecation warnings cleanup

`style_context()` and the per-widget `add_provider` calls in
`settings_panel.rs` are deprecated as of GTK4 4.10. The replacement is
`widget.add_css_class(class_name)` for classes set via global CSS, which
we already use for the color swatch palette. Migration is mostly
done; there might be one or two stragglers.

## Low priority / blocked

### Whisper voice model bundling

Voice input requires a ~30MB ggml-tiny.en-q5_1 Whisper model. Current
search path includes `/usr/share/rusty-board/models` and
`~/.config/web2qml/models`. Migration to a shared `~/.local/share/
ai-models` location is filed for a future round; for now users have
to drop the model manually.

### Honeycomb layout mode

Hex-grid layout as an alternative to the cardinal grid. Filed but no
design pass yet.

### Slide-down-to-dismiss gesture

Currently dismiss is via the tray close button. Filed as an
alternative for "I want to hide the keyboard now without going
through tray."

### Shift+arrow selection mode end-to-end verification

The `parse_keysym_modifiers` function in wayland_backend.rs already
handles `"Shift+Left"` → modifier=1 + sym="Left", and both v1 and v2
paths pass modifiers to the compositor. The tray double-tap toggle
sets a selection-mode flag that prefixes `"Shift+"` to arrow swipes.
But end-to-end verification against a real text field (does the
focused app actually see selection-extending arrows?) hasn't been
done on phone yet.

---

Last updated: see git log for this file.

