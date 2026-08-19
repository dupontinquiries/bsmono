# rusty-board → egui: port status

The GTK4 UI (`src/ui/*`, ~20k lines) is being reimplemented on egui + uikit-egui. This is
what is done, what is deliberately not, and where the two frontends share code instead of
duplicating it.

## Shared, not copied

The port's rule is that anything *behavioural* moves into a crate both frontends depend on,
rather than being written twice:

| crate | what it holds | who uses it |
|---|---|---|
| `crates/board-layouts` | the Key/KeyAction model, Layout/Row, JSON io, layout builders | GTK app + egui frontend |
| `crates/board-gestures` | **swipe typing**: `SwipePoint`, `KeyHitbox`, pivot extraction, `decode_swipe_path`, `GestureRecognizer`, candidate ranking | egui frontend (GTK adoption pending — see below) |

`board-gestures` is the GTK app's `src/gestures/{swipe,recognizer}.rs` moved **verbatim** —
same velocity-smoothing window, same 0.25-of-max pause threshold, same 12%-of-arc pivot
separation with its 50px floor, same tests. One change: `KeyHitbox` used to carry the GTK
crate's `keyboard::Key`, but the algorithm only ever reads `label` and `swipe_target`, so it
now carries those two fields and the crate is frontend-neutral. The 31 tests that came with
the code all pass in their new home, which is the evidence that nothing about recognition
moved.

## Done

- **Layouts** — the shipped JSON files (`en_us_qwerty`, `en_us_dvorak_ortho`, `symbols`) read
  through `board-layouts`, plus the demo/cardinal layouts. Shift cycle (off → one-shot →
  caps) and the symbols toggle behave as `keyboard_view` did.
- **Keys** — adaptive sizing (touch targets grow on phone class), palette theming, long-press
  variant popups for accents, MessagEase-style cardinal swipes on the cardinal layouts.
- **Swipe typing** — capture, trail, decode, candidates, commit. The trail is the Cairo
  overlay's behaviour in egui terms: quadratic Bézier through the midpoints, stroke tapering
  7px → 2px and alpha 0.85 → 0.05 head-to-tail. A drag under ~40px falls through as a tap, so
  the tap path keeps working while swipe is armed.
- **Accessibility** — every key registered in an `A11yScene` with stable ids, so voice focus
  survives a re-layout.
- **One screen** — `demo_app` is the single implementation of the keyboard screen; the deploy
  binary and `examples/feel` are both one call into it. (They used to be two hand-synced
  copies carrying a "kept in sync manually" comment; they had already drifted — the example
  had lost the real-layout tabs.)
- **Settings panel** (`settings.rs`) — NOT a 2469-line reimplementation of the GTK panel (this
  doc used to say that was still owed; it isn't the plan anymore, see the module's own doc
  comment for why). Instead the port mounts `src/config/mod.rs` by `#[path]`, the same GTK-free
  real `Config` type the engine uses, so there is one settings schema on disk instead of two
  that can drift. `settings::ready(Section)` is honest about what the port's own UI currently
  *presents* for each section — Appearance/Behavior/Gestures/Layout/Voice are reachable, Llm
  round-trips through the shared type but has no port-side UI yet (see "Not done" below).
- **Minimized tray** (`tray.rs`) — ported and wired into `demo_app`'s bottom panel: expand,
  spacebar (tap = space + exits selection mode, double-tap = toggles selection mode, cardinal
  swipe = arrow / Shift+arrow in selection mode), close. 11 tests.
- **Microterm** (`microterm.rs`) — the status/voice-partial/steno-candidates/LLM-prompt strip,
  modelled as a sum type (`Content`) rather than a struct of optionals on purpose — see the
  module doc — and drawn above the keyboard in `demo_app` when visible.
- **Voice input UI** (`voice.rs`, `dictation.rs`, `mic_source.rs`) — a real recording/
  transcribing surface exists, not just the dependency wiring this doc used to describe: rusty-
  board's own GTK-free voice stack (`src/voice/`) is mounted by `#[path]`, `DictationService`
  drives partial-replaces-not-appends commit semantics (see `dictation.rs`'s module doc for the
  double-transcription bug this designed out), and `demo_app` owns a live `DictationService` +
  mic broker. Gated behind the `voice` feature (whisper-rs + cpal native build) — off by
  default so a phone build that can't compile whisper-rs still ships a usable keyboard.

## Not done

- **The `Llm` settings section's own UI** — the field round-trips through the shared `Config`
  (`settings::ready(Section::Llm)` is `false` on purpose, asserted in a test), but the port
  doesn't host an assistant subsystem yet, so there's no port-side page for it. Not "the panel
  isn't built" — the panel exists; this one section is deliberately withheld rather than
  offering a control that does nothing.
- **Platform placement** — layer-shell anchoring, exclusive zone, focus-follows behaviour.
  This is eframe/winit territory rather than a UI port, and belongs with the on-device work.

## The one open decision: the GTK side of `board-gestures`

The GTK app has **not** been switched to the shared crate. It cannot be built or tested in
this environment (no GTK4/libadwaita), and swapping a hot path blind is how a regression
ships. The adoption is mechanical when someone has a GTK toolchain in front of them:

1. add `board-gestures = { path = "crates/board-gestures" }` to `Cargo.toml`;
2. delete `src/gestures/swipe.rs` and `src/gestures/recognizer.rs`;
3. in `src/gestures/mod.rs`, `pub use board_gestures::{recognizer, swipe};`
4. fix the four `KeyHitbox { key: … }` construction sites to pass `label` / `swipe_target`
   instead of a `Key`.

Until that happens the two copies exist, which is exactly the drift risk this crate was made
to prevent — so it is worth doing early rather than late. The shared crate's tests are the
ones that came from the GTK app, so a behavioural divergence would show up there first.
