# Testing checklist — v0.1.505 → v0.1.517

Everything shipped this session, in the order worth testing it. Each
item lists: what to do, what PASS looks like, and what I could NOT
verify in the sandbox (so you know where the real risk is).

Most of the *logic* is already proven by standalone tests (counts
noted). What needs YOU is: (1) the full `cargo build` against system
Qt, (2) anything touching the live QML surface, and (3) the power /
scheduler behaviour that needs heterogeneous silicon + a meter.

Legend for what each test needs:
  [BUILD]  just a successful build + run
  [LIVE]   interact with the running UI
  [HW]     a phone/laptop with P+E (big.LITTLE) cores
  [METER]  a power/battery measurement to confirm a saving

---

## 0. BUILD GATE (do this first — everything depends on it)

[BUILD] Full compile against system Qt:
    RUSTFLAGS="-C link-arg=-Wl,--allow-multiple-definition" \
      cargo build --features "voice claude llamacpp"
  PASS: links and runs. This is the ONE check the sandbox can't do —
  all modules compile standalone here, but the full crate (main.rs +
  QML + pty + Qt) only builds on your machine.
  WATCH: 5 NEW source files this session — follow_camera.rs,
  bottom_bar.rs, dead_air.rs, cpu_efficiency.rs (+ the existing ones
  edited: corpus_recorder.rs, connections.rs, acoustic_index.rs,
  main.rs, qml/main.qml). If anything fails to compile it'll be a
  cross-module type/signature mismatch the standalone tests couldn't
  catch.
  NOTE: cpu_efficiency.rs uses libc::syscall + SYS_sched_setattr —
  confirm libc resolves it on your aarch64 target (it's declared in
  libc 0.2 for aarch64 = syscall 274).

---

## 1. SIDEBAR SCROLL PERF (v0.1.508–509)  [LIVE]

The original complaint: sitemap/browser-palette sidebar scrolls slow
vs other sections. Two changes shipped: reuseItems+cacheBuffer on the
ListView, and a 90ms debounce on the filter field.

Test A — scroll feel:
  - Open the browser-palette / sitemap drawer on a page with many
    entries (sitemap + headings + links + archive).
  - Flick-scroll up and down repeatedly.
  PASS: noticeably smoother than before; no hitching as new rows
  appear at the edges.
  IF STILL SLOW: the likely remaining cause is the variable-height
  word-wrap delegate (documented in docs/design/sidebar_scroll_perf.md
  with a ranked next-step plan). I did NOT change row height/wrap
  because it alters appearance — that's the next lever, on you to
  judge.

Test B — filter typing:
  - Type quickly in the sidebar's "Filter / search..." field.
  PASS: filtering feels responsive; no lag per keystroke. (Filter now
  rebuilds once ~90ms after you stop typing, not per character.)
  WATCH: results should be IDENTICAL to before — the debounce is
  behaviour-neutral. If filtering ever feels like it "misses" the
  last character, that's a debounce bug — flag it.

COULD NOT VERIFY: scroll smoothness is a frame-rate question; I only
read the code. Needs your eyes on the running build.

---

## 2. GRAMMAR-CONTEXT LABELING of recordings (v0.1.510–511)  [LIVE]

Recordings now get stamped with the active voice grammar
(editor:helix / repl:gdb / shell / web) and the corpus listing shows
a Grammar column. Includes a REGRESSION FIX (header was bleeding into
displayed paragraph text).

Test A — new recordings get labeled:
  - With a terminal tab focused on a normal shell, record a voice
    paragraph (the usual read-aloud / corpus capture flow).
  - Open helix (hx) in the terminal, record again.
  - Open gdb, record again.
  - Check ~/.config/web2qml/corpus/<speaker>/<ts>.txt for each.
  PASS: each .txt has a `GRAMMAR: <label>\n` line in its header
  (shell / editor:helix / repl:gdb respectively), then `---`, then the
  paragraph text.
  NOTE: persistence is gated by WEB2QML_PERSIST_LEARNING for the
  phonetic DB but the corpus .txt is written regardless — confirm the
  sidecar appears.

Test B — the corpus listing shows grammar + the regression is fixed:
  - Open the corpus listing (list_recordings_markdown surface).
  PASS: a "Grammar" column appears; pre-this-session recordings show
  "—"; new ones show their label. CRITICAL REGRESSION CHECK: the
  "Paragraph" column shows the SPOKEN TEXT, not header lines like
  "URL: ... TITLE: ...". If you see header text in the paragraph
  cell, the strip failed — flag it.

Test C — backward-compat (old recordings still work):
  - Confirm recordings made BEFORE this session still appear in the
    listing with their paragraph text intact (grammar shows "—").
  PASS: no old recording is broken or mis-parsed.

ALREADY VERIFIED (standalone): the parse round-trip across 9 header
cases + the sidecar strip across 7 cases. The on-disk format change
was proven backward-compatible before the writer emits the new field.

---

## 3. CINEMATIC FOLLOW-CAMERA (v0.1.505–506)  [BUILD then LIVE]

NOTE: the camera CORE (follow_camera.rs, 10/10 tests) is built but NOT
yet wired to a live contentY/cursor hook — that wiring was deferred as
live-surface work. So:

Test (only if/when you wire it):
  - Bind a scroll view's contentY to the camera and feed it cursor
    line + timestamps on cursor move (see docs/design/
    cinematic_camera.md for the tick() contract).
  PASS criteria when wired: cursor moves → view glides smoothly to
  re-center (lead room ahead of motion); tiny cursor jitters do NOT
  move the view (deadband); a huge jump SNAPS instead of a long slow
  scroll (cut); no overshoot/bounce.
  TUNING: stiffness 12 / lead 0.15s / deadband 0.75 lines /
  cut 150 lines are starting points in CameraConfig — adjust by feel
  on-device.

COULD NOT VERIFY: the actual feel. The math (no overshoot, converges,
tracks motion) is proven; "cinematic" is a judgment only you can make
on hardware.

---

## 4. UNIFIED BOTTOM BAR (v0.1.507–508)  [LIVE]

NOTE: the bottom-bar MODEL (bottom_bar.rs, 7/7) + a placeholder data
feed (bottom_bar_model() backend method) are built; the visible QML
Row is NOT (deferred — needs the running surface to lay out). So
there's nothing to SEE yet unless you wire the Row.

Test (only when you build the Row):
  - Add a Row of GestureHost buttons bound to bottom_bar_model(),
    each driving the shared SlidePopup with its options; route
    onSelected → a backend action.
  PASS: tap fires the primary action; tap-and-hold-drag fans out the
  menu exactly like the terminal pipe/brace buttons; the bar reflects
  the model (Tabs button badged with count, etc).
  No regression risk today: the placeholder method is unused by QML,
  so it can't break anything currently visible.

---

## 5. DEAD-AIR CLEAVING (v0.1.512–513)  [BUILD]

NOTE: dead_air.rs (13/13) is a pure planning core + a samples→frames
bridge. It is NOT wired to the capture path or to any cut/speed action
yet. So there's no end-user behaviour to test until wired — but the
core can be exercised directly if you want confidence:

Test (optional, direct):
  - Feed frames_from_samples() a known WAV's samples, then
    plan_cleave(), and print the CleavePlan.
  PASS: voice spans classified Voice, silent gaps > ~300ms classified
  DeadAir, dead_air_fraction() reasonable for the clip, compressed_ms
  < total_ms. Micro-gaps between words NOT cleaved.
  WATCH: the adaptive threshold needs REAL audio (it derives from the
  clip's own loudness distribution). The v0.1.513 fix (spread can't
  rescue silence) was found exactly because clean synthetic silence
  behaved differently from real audio — so test with a REAL recording,
  not a generated tone.

COULD NOT VERIFY: behaviour on real microphone audio with real room
noise. The algorithm is the kss-lineage adaptive design; tune
CleaveConfig (min_silence_frames, ECORE thresholds) against your
actual recordings.

---

## 6. uclamp E-CORE BIAS (v0.1.515–517)  [HW] [METER] — the big one to measure

The reason you asked for this doc. Background corpus/index work is
hinted onto efficiency cores when the app is backgrounded (screen
off), full-speed when you're present. Two threads carry it:
the corpus WRITER and the acoustic-index REBUILD.

Test A — support probe [HW/BUILD]:
  - On the device, confirm the kernel supports uclamp:
      cat /boot/config-$(uname -r) | grep CONFIG_UCLAMP_TASK
    (or zcat /proc/config.gz | grep CONFIG_UCLAMP). Need =y.
  - Confirm kernel >= 5.3: uname -r.
  - Confirm the cpufreq governor for the FREQUENCY effect:
      cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
    schedutil = full effect; performance/ondemand = placement bias may
    still apply via EAS but no clock cap.
  PASS: CONFIG_UCLAMP_TASK=y, kernel >= 5.3. If not, the feature
  silently no-ops (by design) — nothing breaks, just no saving.

Test B — placement actually moves to E-cores [HW]:
  - Background the app (screen off / app to background; wait out the
    1500ms debounce).
  - Trigger an acoustic-index rebuild (restart the app backgrounded,
    or whatever path calls rebuild_from_corpus_background) and/or
    finish a recording so the corpus writer runs.
  - Watch per-core load: `htop` (sort by CPU, watch which cores light
    up) or `taskset -p <tid>` / inspect /proc/<pid>/task/<tid>/stat
    for the "acoustic-index-build" and "corpus-writer" threads.
  PASS: those worker threads run predominantly on the E/LITTLE cores
  while backgrounded.
  CONTROL: do the same while the app is FOREGROUNDED — they should run
  on whatever the scheduler picks (no clamp). This is the conditional
  behaviour; verify it actually differs by foreground/background.

Test C — the saving is real [METER]:
  - With a battery/power meter (or powertop, or
    /sys/class/power_supply draw), measure system power during a
    backgrounded index rebuild WITH the build vs a build where you've
    forced the clamp off (or an older version).
  PASS: measurable reduction in package power / cores stay at lower
  freq during the backgrounded rebuild.
  HONEST: the size of the win is governor- and SoC-dependent; could be
  small. The point is it should never be NEGATIVE and never slow a
  foreground op.

Test D — no UX regression [LIVE] — IMPORTANT:
  - Foreground the app. Press the voice button; confirm wake/first-
    press latency is UNCHANGED (those threads are deliberately NOT
    clamped).
  - Confirm a recording made while foregrounded still saves promptly.
  PASS: zero perceptible slowdown of anything you interact with. If
  voice feels laggier, something got clamped that shouldn't have —
  flag it immediately (this is the one real risk of the feature).

COULD NOT VERIFY: all of B/C — needs heterogeneous silicon, the right
kernel config, and a meter. The struct ABI (56 bytes), flags, syscall
number, and graceful no-op-on-failure are verified; that it MOVES
work and SAVES power is yours to measure.

---

## 7. REGRESSION SWEEP (things I touched that could have side effects)

[LIVE] Quick passes to confirm nothing adjacent broke:
  - Voice recording end-to-end still works (start → speak → stop →
    .wav + .txt written). (corpus_recorder.rs changed a lot:
    grammar field, header build, EcoGuard.)
  - The connections graph / VoiceFromPage still works if you use it
    (connections.rs parse_voice_txt was rewritten to be key-order-
    tolerant — old behaviour must be preserved).
  - App background/foreground transition still throttles the wake
    poll as before (voice.rs is_app_backgrounded path — unchanged,
    but now also read by EcoGuard).
  - The browser palette still navigates correctly when you tap a
    sitemap/heading/link entry (the ListView delegate was touched for
    perf but not its onClicked logic).

---

## PRIORITY ORDER if short on time

1. BUILD GATE (§0) — nothing else matters until it compiles.
2. Regression sweep §7 — make sure I didn't break recording/nav.
3. §2 grammar labeling Test B (the regression check) — concrete,
   high-value, easy to eyeball.
4. §6 Test D (voice latency unchanged) — the one real risk.
5. §1 sidebar scroll — the original user complaint.
6. §6 B/C power measurement — the headline efficiency claim, when you
   have the meter.
Everything else (camera, bottom bar, dead_air) is unwired and only
testable once you build its live surface.

---

## What is PROVEN already (so you can deprioritize)

Standalone-tested logic that needs no device check (only re-verified
by the build gate compiling):
  follow_camera 10/10, bottom_bar 7/7, dead_air 13/13, speaker_embed
  12/12, cpu_efficiency pure 8/8, note_cards 18/18, editor_voice
  15/15, repl_voice 17/17, grammar parse round-trip 9 cases, sidecar
  strip 7 cases. Plus qt_struct (429 fields), qml_lint (5 files),
  qml_backend_check (every backend.X resolves) all clean.
