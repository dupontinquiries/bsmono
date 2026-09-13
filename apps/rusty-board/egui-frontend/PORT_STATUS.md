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
- **Stenotype** (`steno.rs`) — the GTK-free pairing engine (`src/steno/`) is mounted by
  `#[path]`, same decision as voice/config. `default_stenotype()` (already in `board-layouts`)
  renders through the existing `to_render`; the harder part was dispatch, not layout: steno
  phonemes collide across buttons/directions (`"n"` is both L1-north and R2-north), so a
  swipe's committed VALUE can't be the phoneme text the way the alphabet cardinal grid's can.
  `Key::steno_button` + a reserved `steno_token`/`decode_steno_token` pair carry the (button,
  gesture) identity through `paint_key` → `classify` → `Press::Steno` instead, while the
  on-screen label stays the phoneme (`SlideOption::new(id, label)` keeps those independent).
  Pairing/solo-flush has no glib timer to ride, so `steno.rs`'s `feed`/`poll_pending` track the
  pending stroke's deadline against a real `Instant` and get polled once per frame from
  `demo_app`'s `update()`. Candidate acceptance backspaces the raw phonemes already typed
  before inserting the accepted word (`accept_word`'s `backspaces` count) — the naive
  "just commit the tapped word" version this replaced would have produced `"katcat"`.
  16 new tests (token round-trip, layout projection, classify, pair/solo timing, microterm
  content mapping) plus the engine's own 24 mounted tests. **Not verified on a real running
  app** — this environment has no display; everything here is `cargo test`-verified, not
  clicked-through.

## Not done

- **The `Llm` settings section's own UI** — the field round-trips through the shared `Config`
  (`settings::ready(Section::Llm)` is `false` on purpose, asserted in a test), but the port
  doesn't host an assistant subsystem yet, so there's no port-side page for it. Not "the panel
  isn't built" — the panel exists; this one section is deliberately withheld rather than
  offering a control that does nothing.
- **Layer-shell placement** — done as a second SHELL, not a change to the screen: `shell::layer`
  (feature `layer`, on by default in the install scripts) owns a `zwlr_layer_shell_v1` surface
  through sctk, renders egui with glutin-EGL + egui_glow, translates sctk pointer/touch into
  egui events, and maps/unmaps on the screen's `ViewportCommand::Visible` (the protocol's own
  null-buffer / re-configure recipe). Bottom anchor, exclusive zone = 340, keyboard
  interactivity None, swap interval 0 with `wl_surface.frame` throttling, frames run only when
  due (idle = no paint). The eframe shell remains for desktops and as the runtime fallback when
  a compositor has no layer shell; there it is a **ruled toplevel** (`plasma-setup.sh` writes
  the KWin rule) and the status strip says which. `Feel::frame(&egui::Context)` +
  `Feel::pump_sink()` are the seam both shells use.
  UPDATE (desktop testing): the map/unmap sequence WAS wrong, and it was the first thing
  desktop testing hit — every hide-then-show cycle:
  `zwlr_layer_surface_v1#7: error 1: the layer surface has a width of 0 but its anchor
  doesn't include the left and the right screen edge` — connection killed, "layer-shell
  shell failed: read: Protocol error 1". Root cause: unmapping (the null-buffer commit
  the hide path makes) resets the layer surface "to the state it had right after
  layer_shell.get_layer_surface" per the protocol's own text — anchor, exclusive zone
  and keyboard interactivity all reset, not just size. The show path only re-sent size
  and exclusive zone, so the re-map committed width 0 with no anchor. A phone session
  that never hides the board never reaches this, which is why it shipped compile-clean.
  Fixed: the four requests (`set_anchor`, `set_size`, `set_exclusive_zone`,
  `set_keyboard_interactivity`) now live in one `apply_layer_state()`, called after
  `get_layer_surface` and again on every re-map, so the two sites cannot drift apart
  again. Still open from this line: integer-only scale (soft text at 2.5x) is
  unverified either way, and the fix itself has only been reproduced hide-then-show
  on KDE — sway is untested.

## Voice: connected, and faster (feature `voice`)

The egui build had the dictation state machine, the mic-state UI and the engine mounted from
the GTK app by `#[path]` — and nothing connecting them: "dictate" started a state machine no
audio ever reached. `voice_host.rs` is the connection, the GTK streaming worker re-shaped for a
frame loop (one inference in flight, bounded 12 s window, `Pass::Partial`), wired into
`toggle_dictation` and the per-frame poll. The 🎤 key on the cardinal boards and the toolbar
button both go through it.

The transcriber itself (`src/voice/transcriber.rs`, shared with the GTK build) is tuned for a
keyboard: the encoder is sized to the audio present (`audio_ctx`, floor 512 of 1500 — the
partial loop was paying for 30 s of silence padding on every 2 s window), partials skip the
temperature-fallback re-decodes and timestamp tokens, one `WhisperState` is reused instead of
allocated per call, `.en` models skip the language-detection pass, and every call records a
`Timing` the status row shows (`partial 2.1s ctx512 180ms · corpus 12`). The fingerprint
corpus (`audio_kit::fingerprint` 64-bit + length, voice-kit's cache, persisted at
`~/.local/share/rusty-board/voice-fingerprints.tsv`) answers a repeated COMMITTED utterance
before whisper runs; partials are never remembered. `RUSTY_BOARD_VOICE_FPCACHE=0` disables it.

Compile-verified only: this sandbox has no microphone and no model. The number that matters
is the `ms` in the status row on the phone; `MIN_AUDIO_CTX` and the model preference order in
voice-kit's `model.rs` are the two knobs if it is not enough.

## The state bug that hid every experimental keyboard (fixed)

`paint_key`'s glass branch (the "system" palette, which is `Board::new()`'s default) painted the
key and `return`ed on `resp.clicked()`. Everything after it — the compass widget, the
hairpin/rotation classifier, hold-variants, the steno token path — was unreachable under the
default palette, so every cardinal AND steno key was a plain click. The classifier, the widget
maths and the layout tables all passed their tests throughout; no test pressed a key and looked
at what came out. `paint_key` is now `paint_key_face` (looks) + `dispatch_key` (meaning) with no
early return, and `swipe_delivery_tests` in `lib.rs` drives synthetic pointer events through a
real `egui::Context` into `keyboard_ui`. The compass widget itself had a second bug — it
measured direction from the key's centre rather than the press point (fixed in uikit-egui, with
the committed direction now taken from the farthest reach, FlickBoard-style). Both are headless-
verified only; the steno board has still never been stroked on a device.

## Wayland without GTK (fixed)

The `wayland` feature depended on the GTK crate (`rusty-board/wayland` → `gdk-integration` +
`gtk4-layer-shell`), and `board-compositor`'s own `wayland` feature called GLib without
declaring it. Both are gone: the feature points at `board-compositor/wayland`, the backend
exposes `pump()` which `wayland.rs` calls per frame, and KWin's inherited `WAYLAND_SOCKET` is
captured by the binary before eframe connects (`platform::capture_inherited_socket`). Verified
by `cargo check --bins --features wayland` in a sandbox with no GTK.

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
