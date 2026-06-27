# rusty-board QML test harness

A Plasma Mobile QML app that exercises the keyboard's **external** behavior — focus events, text commits, ContentType hints, dismiss gesture, latency. Use this alongside the Rust tests in `src/ui/keyboard_behaviors_tests.rs` (120 cases) for full coverage.

## What the QML harness tests vs the Rust tests

The two test layers are **complementary, not overlapping**:

| Layer | Tests | Coverage |
|-------|-------|----------|
| Rust (`keyboard_behaviors_tests.rs`) | 30 × 4 = 120 cases | Engine state machine, dispatcher routing, layout invariants, gesture classification |
| QML harness (`RustyBoardTestHarness.qml`) | 30 manual cases | Wayland input-method protocol, focus surfacing, ContentType reading, on-device latency feel, multi-field workflow |

Rust tests run **without GTK** — they're fast (<1s for all 120), repeatable, and CI-friendly. They prove the keyboard's logic. The QML harness needs a running keyboard against a running Plasma Mobile, and verifies the actual on-device behavior — things like "does the keyboard appear when I focus this field" or "does CapsLock leak across focus changes" — that require the full integration.

## Running on the phone

1. Build and install rusty-board as your input method:
   ```
   cargo build --release
   # copy + install per your Plasma Mobile setup
   ```

2. Copy the harness to the device:
   ```
   scp test_harness_qml/RustyBoardTestHarness.qml phone:/tmp/
   ```

3. Run it:
   ```
   ssh phone qmlscene /tmp/RustyBoardTestHarness.qml
   ```

4. Tap each TextField in turn. The rusty-board keyboard should appear; verify the listed expectation; mark Pass / Fail / Skip.

## What the 30 QML cases cover

| # | Category | Specifically tests |
|---|----------|---|
| 1-3 | Focus + auto-cap | Field-focus shows keyboard; auto-cap fires on sentence start and empty field |
| 4-7 | ContentType hints | URL, email, number, password fields |
| 8-9 | Cross-field state | Undo + CapsLock isolation across fields |
| 10-12 | Symbols transitions | CapsLock survives `?123`/`ABC`; one-shot Shifted is consumed; auto-cap survives a symbols visit |
| 13-16 | Swipe gestures | Dismiss flick; swipe-typing auto-space; manual key breaks chain; top-row digit no auto-space |
| 17-19 | Backspace + undo | Single backspace; hold to repeat; long-press undo |
| 20-21 | Special keys | Tab fires as keysym (not `\t`); Return inserts newline |
| 22-25 | Cardinal / Stenotype | Cardinal verbatim commits; steno phoneme chain; strict-mode dismiss; renamed `?123` label |
| 26-27 | Voice / LLM | Voice transcript; LLM synonym without backspace flood |
| 28-29 | Latency | First keystroke after idle is instant; burst typing keeps up |
| 30 | Edge case | Focus loss mid-swipe doesn't strand state |

## Adding more QML cases

The `TestCase` component takes a `title`, an `expectation` (what the user should see), and arbitrary child content (usually a TextField). Add a new `TestCase { ... }` block in the ColumnLayout and it appears in the scrollable list.

## Note on QML testing limitations

QML's `TestCase` framework (from `QtTest 1.15`) can *automate* QML-side UI interactions, but it can't poke the keyboard's internal state — the keyboard runs out-of-process and communicates over wayland. So this harness is fundamentally a **manual verification** tool. The automated coverage lives in the Rust tests.

If you want fully automated end-to-end testing, the path forward is:
1. Run a headless wayland compositor (Cage, weston, or sway in headless mode).
2. Launch rusty-board against it.
3. Launch a QML test app that uses `Qt.inputMethodAccepted` and similar APIs to inject focus changes.
4. Inject simulated touch events via `libinput` or `wayland-debug` virtual devices.

That's a substantial test-harness build — well beyond what this manual harness provides.
