# web2qml — Verification & Validation Provenance

_Scope: how work shipped in this development arc (≈ v0.1.743 → v0.1.758) was
verified, split between (1) what is **machine-verified in the sandbox** before
shipping and (2) what **only the developer's real hardware can validate**. This
document is the single reference for "what proves this works, and who proved
it."_

---

## Executive summary

web2qml is a voice-first native terminal + browser (Rust + Qt6/QML). It is built
and dogfooded on real hardware (an Asahi-Linux MacBook M2 and a postmarketOS
phone). Development happens in a **headless Linux sandbox** that can compile the
full Rust crate, run pure-logic tests, statically analyse the QML, and measure
layout/performance — but **cannot** render QML, capture audio, spawn a PTY, or
process touch/gesture. That hard boundary defines a two-party verification model:

- **Sandbox (automated, every change):** compile gate, ~1,484 pure tests, the
  real Qt `qmllint` plus a custom linter, struct/signal integrity, golden-output
  diffing, and measurement harnesses for layout quality and idle/in-use power.
  Anything expressible as pure logic, a static property of the source, or a
  measurable metric is proven here before a build is handed over.
- **Developer hardware (human-in-the-loop):** everything that only exists at
  runtime — the actual on-screen render, touch/pan/zoom gestures, microphone
  capture and wake-word detection, live PTY I/O, and "does it feel right."
  Validation is supplied as **dogfood run logs, screenshots, and explicit
  on-device confirmations**, which repeatedly caught real bugs the static gates
  could not see.

The guiding rule throughout: **never assert a capability that wasn't tested.**
Runtime-untested code ships labelled as such, with rollback notes, and the
developer's logs/screenshots are treated as the authoritative signal for
anything past the sandbox boundary.

---

## Table A — Verification tooling (sandbox side: "what was installed/built")

| Tool / asset | Type | What it verifies | Run cadence |
|---|---|---|---|
| Rust toolchain (`cargo`, `rustc`) | environment | Full crate compiles with `--features "voice claude"`; the compile gate is mandatory after any `main.rs`/FFI/`qt_method`/signal change | Every Rust change |
| `scripts/cargo_test_pure.sh` | test harness | **1,456 tier-1 + 28 integration** pure-logic tests (parsers, layout math, workspace model, signaling) | Every change |
| `scripts/cargo_check_pure.sh` | compile check | Fast type-check of the pure module set without the full Qt link | As needed |
| `/usr/lib/qt6/bin/qmllint` | real Qt parser | QML syntax, duplicate ids, undefined references, type errors (the authoritative QML check) | Every QML change |
| `scripts/qml_lint.py` | custom linter | id↔property collisions, signal/property collisions, chord-registry coverage (catches footguns qmllint allows) | Every QML change |
| `scripts/qt_struct_check.py` | integrity check | Every `qt_*` field is initialised; `NOTIFY` clauses map to declared signals (551 fields / 64 signals at v758) | Every struct change |
| `scripts/qml_golden.sh` | golden diff | HTML→QML compiler output matches 9 frozen fixtures (regression guard) | Compiler changes |
| `scripts/qml_backend_check.py` | cross-check | QML-referenced backend methods exist on the Rust side | As needed |
| `scripts/layout_metrics.sh` + `layout_analyze.py` | measurement harness | Graph-layout quality across a 6-graph corpus: 11 metrics (aspect, crossings, backward edges, layer imbalance, overlaps, label-fit, disconnected, edge-angle, edge-length CV…) → prioritized lessons | Layout changes |
| `tools/layout_tool` (**new this arc**) | standalone CLI | Runs the real parser + layout engine on **any** `.mmd`/`.md` file, reports positions + metrics, `--json` for the analyzer | On demand |
| `scripts/perf_audit.py` | static perf scan | Catalogs all ~85 QML timers (interval/gate/idle-drain vs transient) + hot-signal fan-out → performance matrix | Perf changes |
| `scripts/perf_regression.py` | threshold lock | Fails if idle/in-use cost regresses: `always_on_timers ≤ 2`, `idle_wakeups/s ≤ 6`, `fast_idle_timers ≤ 1`, `max_signal_fanout ≤ 9` | Perf changes |
| `scripts/sim-corpus.sh`, `voice_ci.sh`, `corpus_wer_harness.sh`, `voice_tts_*.sh` | voice harnesses | Speech-corpus routing through `voice_intent`; WER scoring; TTS comparison | Voice changes |
| `scripts/rust_brace_check.py`, `validate.sh` | structural | Brace balance; aggregate validation runner | As needed |
| Standalone `rustc --test` / staged `/tmp` crate | ad-hoc proof | Isolated proof of a routing decision or a single module before wiring (e.g. "split this pane horizontal" → route vs reject) | Per risky change |

## Table B — Sandbox environment: capabilities vs. hard limits

| Capability | Available in sandbox? | Consequence |
|---|---|---|
| Compile Rust (full crate) | ✅ | Type-safety, FFI signatures, `qt_method`/signal wiring proven pre-ship |
| Pure-logic unit tests | ✅ | Parsers, layout math, workspace model, signaling fully testable |
| Real `qmllint` (Qt6) | ✅ | QML syntax / ids / refs / types checked |
| `node`, `espeak-ng`, `ffmpeg` | ✅ | Limited audio/phoneme tooling for harnesses |
| GUI / display / Vulkan | ❌ | **No QML render** — visual output untestable |
| Audio capture / microphone | ❌ | **No voice capture / wake-word / VAD** at runtime |
| PTY / shell spawn | ❌ | **No live terminal I/O** |
| Touch / gesture / pan-zoom | ❌ | **No gesture behaviour** (pinch, flick, drag) |
| `whisper-cli` / `piper` / `sox` | ❌ | Speech transcription path not runnable |

## Table C — Developer-supplied validation (human-in-the-loop: "what you had to provide")

| Category | What only the developer can validate | How it was supplied |
|---|---|---|
| Visual render | Graph cards, arrows, padding, subgraph boxes actually drawing correctly | **Screenshots** |
| Gestures | Graph pan/zoom, getting stuck/bouncing, reaching edges | Screenshots + description |
| Voice path | Wake-word firing, transcription accuracy, intent routing on real speech | **Run logs** (`[wake-listen]`, `[voice]`, `[parse]`, `[ui] executeIntent`) |
| Terminal I/O | Commands running, splits, shell behaviour | Run logs + terminal screenshots |
| Keyboard | Shortcuts firing vs. being swallowed by focus (e.g. `^T` to PTY) | Description + logs |
| Runtime errors | QML `ReferenceError` / `Unable to assign to QColor` only thrown at load | **Run logs** (stderr) |
| Feel / UX | Reflashing, annoyance, "looks nice and readable" | Description + screenshots |
| Hardware reality | Behaviour on Asahi M2 laptop **and** pmOS phone | Direct dogfooding |

## Table D — Dogfood validation log (this arc)

| Version | Developer supplied | What it caught / confirmed |
|---|---|---|
| v0.1.747-era | Screenshot of graph | Confirmed arrows + layout "nice and readable"; requested padding, arrow shortening, asked about "extra components" (the pill-card ears) |
| v0.1.751 | Full run log + terminal output | **"split this pane horizontal" ran as the shell `split` binary** — voice misrouted to shell; also surfaced 28× QColor errors + `_activeEdit` ReferenceError |
| v0.1.754 | Run log + graph screenshot | Confirmed adaptive-gap layout; surfaced `_anchorCacheReady` assign error; asked for standalone layout tool; flagged no voice/shortcut for terminal tabs; **graph view stuck off-screen** |
| v0.1.757 (regression report) | Run log + description | **Graph bounced infinitely** + couldn't scroll to edges (my v755 recovery bug); `Ctrl+Shift+T` inserted `^T`; ESC didn't close F1 help; sessions page reflashed constantly |

> Note: several fixes are **runtime-untested at ship** and await the next dogfood
> pass — they are labelled in the changelog with rollback notes. The
> developer's inability to test during one stretch is why the work pivoted to
> fully sandbox-verifiable items (adaptive gap, the layout tool, group boxes).

---

## Detailed sections

### 1. The verification workflow (the loop)

Every code change follows the same gate sequence before a build is handed over:

1. **Read the source** (never blind-edit). 2. **Compile gate**
   (`cargo check --features "voice claude"`) for any Rust/FFI/signal change.
3. **Pure tests** (`cargo_test_pure.sh`) — must stay green (1,456 + 28).
4. **QML checks** — real `qmllint` + `qml_lint.py` for any `.qml` change.
5. **Struct integrity** (`qt_struct_check.py`) for struct/signal changes.
6. **Domain harness** where applicable — `layout_metrics.sh` for layout,
   `perf_audit.py`/`perf_regression.py` for timers/signals.
7. **Ad-hoc proof** for risky logic (a staged `/tmp` crate or `rustc --test`
   to prove e.g. a voice-routing decision in isolation).
8. **Ship** the full-project zip; **verify the zip contents** (version string +
   key markers). 9. **Label runtime-untested surfaces** honestly + give rollback
   notes.

This is the "measure → fix → re-measure" discipline: hypotheses are validated
against a harness **before** live code changes (the layout aspect-ratio work and
the perf/idle-drain work are the canonical examples).

### 2. What the sandbox proves (with examples)

- **Pure cores** — `terminal_workspace` (21 tests: split tree, geometric focus,
  resize, command parsing), `event_signal` (7 tests: lock-free bitmask),
  `group_layout` (6 tests: subgraph box geometry), `layered_layout`,
  `card_density`, `node_id`, `mermaid` (42 tests). All run with no device.
- **Routing decisions** — e.g. "split this pane horizontal" was proven to route
  to a pane-split (and "split the bill with me" proven to *not*) via a
  standalone harness, separate from the full app.
- **Layout quality** — measured, not eyeballed: adaptive layer-gap dropped
  cpu_pipeline aspect 15.96 → 9.54, optical_link 10.43 → 6.54 (harness numbers),
  with balanced graphs held steady.
- **Power cost** — `perf_audit.py` distinguished true idle-drain timers from
  transient polls and found the always-on wake/ask-AI polls;
  `perf_regression.py` locked the result so a future feature mix can't silently
  reintroduce drain.
- **Group boxes** — verified end-to-end in a staged crate: a "Data Memory"
  subgraph parsed → laid out → produced a correct 234×108 container box.

### 3. What only the developer can validate (and why)

The sandbox has no display, audio, PTY, or input device. Therefore the following
are **structurally unverifiable** in CI and depend entirely on dogfooding:

- **Render correctness** — a card/box/arrow can be geometrically correct in the
  data and still look wrong on screen (the pill-card "ears" were a render-only
  artifact invisible to every static check).
- **Gesture behaviour** — the graph pan/zoom bounce and edge-scroll problems
  were pure runtime-interaction bugs; the fix is reasoned + qmllint-clean but its
  *behaviour* awaits the next on-device pass.
- **Voice/audio** — wake-word firing, VAD calibration, transcription accuracy,
  and the full intent → action chain only exist with a live microphone.
- **Focus/keyboard routing** — `Ctrl+Shift+T` being swallowed as `^T` by the
  focused terminal is a focus-precedence reality the sandbox can't reproduce.
- **Load-time QML errors** — `ReferenceError` / `Unable to assign to QColor` are
  thrown when the QML engine instantiates the tree; the developer's stderr logs
  are the only place they surface.

### 4. Honesty & rollback policy

- Runtime-untested code ships **explicitly labelled** in the changelog, with a
  one-line rollback note (usually "additive — revert the block").
- When a prior fix is found not to have shipped (the v752 split fix regressed
  out of the tree), that is **reported plainly** and re-applied, not quietly
  patched.
- Credit is not accepted for unfinished work (the coalesced-timer exchange:
  clarified it was the *primitive*, not the timer, before building the timer).
- Claims are scoped to what was actually verified; "compiles + lints + pure
  tests pass" is never overstated as "works on device."

### 5. Reproducing the gates locally

```sh
# compile gate
WEB2QML_PROFILE=1 RUSTFLAGS="-C link-arg=-Wl,--allow-multiple-definition" \
  cargo check --features "voice claude"

# pure tests (1,456 + 28)
bash scripts/cargo_test_pure.sh

# QML: real Qt parser + custom linter
/usr/lib/qt6/bin/qmllint qml/main.qml
python3 scripts/qml_lint.py

# struct/signal integrity
python3 scripts/qt_struct_check.py src/main.rs

# layout quality (6-graph corpus)
bash scripts/layout_metrics.sh

# any diagram, standalone
tools/layout_tool/target/release/layout_tool path/to/diagram.mmd

# perf: idle/in-use cost matrix + regression lock
python3 scripts/perf_audit.py qml/main.qml
python3 scripts/perf_regression.py
```

### 6. Coverage matrix — feature → strongest available proof

| Feature area | Sandbox proof | Needs dogfood? |
|---|---|---|
| Mermaid parsing, graph layout math | pure tests + layout harness | No |
| Subgraph box geometry | pure tests + end-to-end staged run | Box **draw** only |
| Voice intent routing | pure tests + standalone routing proof | Real-speech accuracy |
| Terminal workspace model (tabs/panes/splits) | 21 pure tests | Live PTY + render |
| Event signaling / coalesced poll | 7 pure tests + perf harness | On-device timing |
| Idle/in-use power | perf audit + regression thresholds | On-device thermals |
| QML render / gestures / focus | qmllint + custom lint (structure only) | **Yes — fully** |
| Audio / wake-word / VAD | n/a | **Yes — fully** |

---

_Generated for the web2qml project. Tooling counts reflect v0.1.758
(1,456 tier-1 + 28 integration pure tests; 551 qt_* fields / 64 signals;
6-graph layout corpus; 4 locked perf thresholds)._
