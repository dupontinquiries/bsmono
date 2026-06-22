# Next-session handoff (written end of v0.1.247)

This document exists so a fresh conversation can start without
re-asking the user for context. Read this first, then the most
recent changelog entries (`docs/changelog/0.1.1x.md`, v0.1.241 →
v0.1.247), and `docs/todo.md` for what's filed.

## Current state

- **Shipped version:** v0.1.247
- **Last hardware-validated version:** v0.1.244 ("basic app
  opening is good"). Seven ships now stacked unvalidated:
  v0.1.241-247. Priority 1 sanity check pending.
- **Fallback zips available** at `/mnt/user-data/outputs/`:
  - `web2qml-stage-m-v0.1.240-fallback.zip` (pre-v0.1.241 cleanup)
  - `web2qml-stage-m-v0.1.242-fallback.zip` (post-PA1, pre-polish)

## What landed in the recent ship cascade

**v0.1.241** — Reading overlay moved to top of screen (was
bottom); teleprompter-style scroll for the word display;
cleanup pass on qml_gen helpers (maybe_wrap_extended,
hexRgba/hexLuminance).

**v0.1.242 (PA1)** — Per-content-type WPM multipliers landed.
First ship of the Phoneme alignment + tiered reading model arc.
ContentType enum (Prose/Code/List/Table) threaded through 10
call sites. New backend qt_property `current_paragraph_content_type`.
ReadingOverlay applies `_baseMultiplierFor()` when reading
starts. Visible pill "<type> ×<mult>" near rate button.

**v0.1.243** — Selection box: new `selection_accent` color
(`#ffaa44` on dark, `#c04020` on light); breathing opacity
animation (0.7 → 1.0 → 0.7 over 3s); parallax-lite 2px slide
on hover. Markdown table overflow fixed via
`Layout.preferredWidth: 1` Qt idiom.

**v0.1.244** — Code blocks selectable end-to-end (PA1 finish).
Both `<pre>` blocks and manpage fenced code wrap with
ContentType::Code. Inline `<code>` stays unselectable.
Profiling protocol documented in `docs/troubleshooting.md`.
TabBar emits `tab:term`/`tab:browse` events. ptyPoll emits
`altscreen:enter`/`altscreen:exit` events.

## The hardware test plan

When the user returns with hardware time, this is the priority
order for validation:

### Priority 1: Sanity-check the recent ships didn't break anything

Walk through a normal usage session:
1. App launches cleanly. No QML warnings in console.
2. Look for `[phoneme-db] loaded v1 (eSpeak NG text-to-speech):
   3 manpages, 27 paragraphs, ipa=true` in stderr (v0.1.246).
3. Browse to a normal web page. Page renders. Tap a paragraph.
   Selection box left edge shows with the new orange/red-brown
   accent (depending on theme). Breathing animation runs while
   selected. Hovering shifts the strip 2px right (desktop only).
4. Confirm read-aloud still works. Pill shows
   "prose ×0.95" at the start. Words advance correctly.
5. Find a markdown table on a manpage. Confirm it fits
   viewport on a normal desktop window (no horizontal scroll).
6. Find a code block in welcome manpage. Tap it. Pill shows
   "code ×0.70". Read-aloud uses slower pace.
7. Tap a paragraph in welcome manpage — stderr should show
   `phonemes: N words from welcome` (v0.1.246). Tap a paragraph
   on a wikipedia article — should show `phonemes: not bundled`.
8. Switch to terminal tab. Run helix or another TUI. Confirm
   no obvious regression.

**Audit-derived spot checks (v0.1.247 audit found these as
potential issues; verify on hardware whether they actually
manifest):**

- Try the **Amber theme**. Selection-accent color `#ffaa44`
  is very close to Amber's text color `#ffb000`. Selection
  strip might be invisible against text. Filed as M-sized
  todo if confirmed.
- Try the **Sepia and ReadLt themes** with a paragraph
  containing inline links. Selection accent `#c04020` is in
  the same hue family as Sepia/ReadLt's link colors. Could
  read as "is this a link?" Filed if confirmed.
- On any theme, **tap a 2-3 word manpage paragraph** if you
  find one. Confirm it's selectable (manpage path wraps
  unconditionally, unlike webpages' 10-50 word filter).
  Then check whether the phoneme-db log says "not bundled"
  for it — that'd be the inconsistency I filed (generator
  filters by 3-200 but runtime doesn't).

If any of these surface a regression, the v0.1.242 fallback zip
is the rollback point (PA1 baseline, before the v0.1.243-247
polish).

### Priority 2: Konsole-gap profiling (TP4 from terminal-perf arc)

If sanity check passes, do the perf comparison while you have
the hardware. The protocol is now documented at
`docs/troubleshooting.md`. Run:

```
WEB2QML_PROFILE=1 cargo run --release --features voice
```

Then exercise both shell and TUI workloads — TUI is the primary
focus per the user's v0.1.241 hypothesis:

- Shell: `cat /var/log/journal-export.log`, `seq 1 50000`,
  scroll-through a paged file, continuous typing 30s
- TUI (primary): helix on a 1000-line file with rapid
  scrolling, htop refreshing 30s with sort toggles, gitui
  navigating a large repo

Run the same workloads in Konsole. Capture wall time, peak
CPU%, memory delta, and subjective feel. Share
`~/.config/web2qml/profiling.log`.

The outcome categories drive different next steps (see
`docs/todo.md` "Terminal performance investigation" arc):
- <2x slower → localized fixes, no architectural change
- 2-5x slower → specific hot spots, profile to find
- >5x slower architectural → Option G (2x2 tile grouping)
  is the cheapest first move; Option F (foveated regime-
  aware) layers on if G insufficient

### Priority 3: Hardware-blocked items

Two items in the immediate queue need hardware diagnosis
before they can be fixed:

- **Terminal scroll cuts commands at window edges** — needs
  visual reproduction. What does the cut look like? Does it
  happen on first-page-load or after scrolling? Is it
  predictable by content position?

- **Entered commands appended with newline** — needs
  diagnosis. Try the same workload in plain bash WITHOUT
  vim-mode. If newline still appears, our key handling is
  inserting it. If not, it's shell-side.

## Next code-ready ship (after hardware testing)

### PA2 (highest priority within the phoneme arc)

Silence-detection multiplier with decay + persistence per
content type. Concrete plan:

- Wire silence-detection signal from voice runtime (VAD from
  v0.1.225)
- Add stack/decay logic in QML:
  - 750ms near-silence → bump activeMultiplier down 0.05
  - Each bump decays 0.01 per 500ms after 5s active period
  - Floor at 0.5x (don't bump further past that)
- Visible: pill still shows current value, now with live
  changes during reading
- Persistence: new file `~/.config/web2qml/reading_calibration.toml`
  with one row per content type. Updated on stopReading();
  loaded at cold-start as activeMultiplier baseline.

**Gating concern:** PA2 depends on the voice runtime's
silence-detection signal being reliable on hardware. That part
hasn't been validated since the v0.1.225 VAD fix. If the
silence signal is too noisy or never fires, PA2 won't work
even when "implemented." Worth testing the voice runtime
explicitly during hardware Priority 2 before starting PA2.

### Other code-ready items if PA2 is blocked

- **Default WPM 5% drop**: already subsumed by PA1's
  prose=0.95 multiplier. No work needed.
- **Selection box skew/bulge animations** (extension of
  v0.1.243): only if user finds breathing+parallax
  insufficient.
- **TP2 deeper timing instrumentation**: only after TP4
  data shows specific paths matter. Premature instrumentation
  in cheap paths is noise.
- **Terminal animation Phase 1** (cell-update fade): adds
  visual work to a path already flagged as slow. Wait until
  perf investigation establishes baseline OR option G tile
  grouping lands.

## Architectural arcs filed for future months

Don't start these without the user's explicit go-ahead:

- **Phoneme alignment + tiered reading model** (16 sub-ships,
  PA1 done, PA2-7 + PB1-4 + PC1-5 ahead). 4-6 month arc.
- **TTS-bootstrap arc** (2.7.1-2.7.5). Complementary to the
  phoneme arc; produces synthetic fingerprints for cold-start
  speaker matching.
- **Terminal performance investigation** (TP1-3 done in 244;
  TP4 pending hardware; TP5+ derived from data).
- **Folder picker for network mounts** (FP1-4). Not urgent
  — no FileDialog use is on the critical path yet.
- **Terminal animation** (4 phases). Independent of perf
  arc but should follow it sequence-wise (perf fixes first,
  delight second).

## A behavioral note for the next conversation

The recent conversation pattern was many turns of architecture
filing followed by a cascade of four code ships. Both phases
have value, but watch the ratio. The recent cascade became
possible because the prior architecture work mapped the surface
area concretely (PA1 had a files-and-functions plan ready in
todo.md). Without that mapping, code ships are slower.

When the user says "continue" or "best assessment" without
specifying, my honest read is:
- If recent ships are unvalidated on hardware: stop and ask
  for hardware time, do an audit pass while waiting
- If recent ships are validated: proceed with next filed
  ready-to-go item
- If filed plans are missing: do architecture filing first

This document was written at the end of v0.1.244, before
hardware validation. Hardware testing is the next priority.

## Active backlog snapshot

**Immediate queue (deferred for hardware):**
- Terminal scroll cuts commands (M)
- Newline-on-input shell-vs-ours diagnosis (M)

**Hardware-test-ready (waiting on profile data):**
- TP4 Konsole-baseline comparison

**Architectural arcs (filed, awaiting authorization to start):**
- PA2 (silence-detection, ~half a session, but voice-runtime-dependent)
- PA3 (build-time phoneme database, ~one session)
- Option G 2x2 tile grouping (2-3 weeks, dependent on TP4 data)

**Polish (low priority):**
- Selection box skew/bulge if breathing+parallax insufficient

**Multi-month arcs (filed):**
- Phoneme alignment full (PA2-7 + PB1-4 + PC1-5)
- TTS-bootstrap (2.7.1-2.7.5)
- Terminal animation phases
- Folder picker network-mount filtering

## Validation state

- 255 qt_* fields, 38 NOTIFY → 30 signals (unchanged since v0.1.242)
- qml_lint clean, brace check clean, qt_struct_check clean
- All four recent ships pass build (assumed; not run on hardware)
