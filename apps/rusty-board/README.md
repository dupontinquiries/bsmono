# RustyBoard

A modern Rust-based on-screen keyboard with **GTK4 + libadwaita**, **swipe typing**, and **whisper-rs voice input**. Designed as a contemporary successor to Maliit with a focus on smooth animations, modern UX, and efficient touch input.

## Features

- 🎨 **GTK4 + libadwaita** — adaptive light/dark themes, smooth spring animations, modern aesthetics
- 👆 **Swipe typing** — Gboard-style continuous gesture input with dictionary-backed word recognition. **Speed-aware**: slow, deliberate swipes preserve your raw input; fast, sloppy ones lean on the dictionary for correction.
- 🎤 **Voice input** — local, offline transcription via whisper-rs (no cloud)
- ⌨️ **Multiple layouts** — QWERTY (staggered), Dvorak (strict 10-column ortholinear grid), Colemak, AZERTY, QWERTZ; symbols page; **dedicated 4-column number pad** for PINs and forms
- 🪟 **Wayland-native placement** — anchors to the bottom edge as a layer surface (via `gtk4-layer-shell`), reserves exclusive zone so other windows resize around it, doesn't steal focus. Falls back to a borderless toplevel on X11.
- ⚡ **Virtual keyboard injection** — `zwp_virtual_keyboard_v1` on Wayland (Phosh, Sway, river, Wayfire, KWin), `xdotool` fallback on X11
- 👁️ **Auto show/hide** — `zwp_input_method_v2` integration: the keyboard appears when the focused app has an editable widget, hides when focus moves elsewhere. Disable via `behavior.auto_hide_on_no_focus = false` in the config.
- 🔡 **Long-press popovers** — accented characters and alternate symbols
- 🖼️ **Customizable** — TOML config for colors, animations, gestures, behavior
- 🔌 **D-Bus interface** — `io.rustyboard.Keyboard1` for external show/hide/voice control

## Build dependencies

| Distro          | Command |
|-----------------|---------|
| Debian / Ubuntu | `sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libgtk4-layer-shell-dev libxkbcommon-dev libasound2-dev xdotool` |
| Fedora          | `sudo dnf install gcc pkgconf-pkg-config gtk4-devel libadwaita-devel gtk4-layer-shell-devel libxkbcommon-devel alsa-lib-devel xdotool` |
| Arch            | `sudo pacman -S base-devel pkgconf gtk4 libadwaita gtk4-layer-shell libxkbcommon alsa-lib xdotool` |
| Alpine          | `sudo apk add build-base pkgconf gtk4.0-dev libadwaita-dev gtk4-layer-shell-dev libxkbcommon-dev alsa-lib-dev xdotool` |

`gtk4-layer-shell` (the C library, version ≥ 1.0) is required for layer-shell placement on Wayland. `libxkbcommon` is required for synthesizing the keymap RustyBoard sends the compositor as part of `zwp_virtual_keyboard_v1`. `xdotool` is only needed if you want X11 fallback for keystroke injection.

Plus a recent stable Rust toolchain (≥ 1.75): https://rustup.rs/

## Quick start

```bash
git clone <repo>
cd rusty-board

# Build & install to ~/.local
./scripts/install.sh --with-model --model ggml-base.en.bin

# Or skip the model (voice input will need a model later)
./scripts/install.sh

# Run
rusty-board
```

## Build profiles

RustyBoard builds in four named configurations, layered from smallest
to fullest. Each is a Cargo feature bundle — build one with
`--no-default-features --features <profile>`:

```bash
# basic — smallest useful keyboard: QWERTY/Dvorak, tap + swipe-type,
#         Wayland injection + D-Bus control. No voice, LLM, or
#         experimental input surfaces.
cargo build --release --no-default-features --features basic

# full-keyboard — basic + every input SURFACE (cardinal grid,
#         stenotype, character-draw canvas, command/palette).
#         Still no voice or LLM.
cargo build --release --no-default-features --features full-keyboard

# build-voice — full-keyboard + Whisper voice transcription.
cargo build --release --no-default-features --features build-voice

# build-functioncalling — the works: full-keyboard + voice + LLM /
#         function-calling. This is what `cargo build` ships by
#         default.
cargo build --release   # == --features build-functioncalling
```

### Primitives (compose your own)

The profiles are bundles of these fine-grained toggles, which you can
also mix directly:

| Primitive     | Adds                                                        |
|---------------|-------------------------------------------------------------|
| `wayland`     | layer-shell + input-method/virtual-keyboard protocols, xkb  |
| `dbus`        | D-Bus IPC (show/hide, external control)                     |
| `input-modes` | cardinal grid, stenotype, character-draw, palette surface   |
| `voice`       | Whisper transcription (mic capture, model in RAM)           |
| `llm`         | LLM text transforms, `/continue`, function-calling actions  |

Config written by a fuller build degrades gracefully on a leaner one:
e.g. an `experimental_mode = "stenotype"` setting is simply ignored by
a `basic` build, which falls back to the base letter layout.



```bash
./scripts/uninstall.sh
# Want to keep your config / models?
./scripts/uninstall.sh --keep-config --keep-models
```

## Configuration

```bash
rusty-board-config show     # print current config
rusty-board-config path     # where it lives
rusty-board-config reset    # restore defaults
```

The config lives at `$XDG_CONFIG_HOME/rusty-board/config.toml` (typically `~/.config/rusty-board/config.toml`). It's plain TOML — edit it however you like.

Highlights:

```toml
[appearance]
theme = "auto"            # "auto", "light", "dark"
accent_color = "3584e4"   # GNOME blue
key_radius = 8.0
animation_ms = 150
opacity = 0.95
font_family = "Cantarell"

[behavior]
auto_capitalize = true
word_prediction = true
haptic_feedback = true
long_press_ms = 350

[voice]
enabled = true
# model_path = "/path/to/ggml-base.en.bin"  # uncomment to override
language = "auto"
streaming = true

[gestures]
swipe_typing = true
spacebar_cursor = true
swipe_threshold = 20.0
smoothing = 0.3
```

## Project layout

```
rusty-board/
├── Cargo.toml
├── README.md
├── src/
│   ├── main.rs                  Application entry
│   ├── lib.rs                   Library exports
│   ├── bin/config.rs            CLI config tool
│   ├── config/                  TOML config schema + load/save
│   ├── keyboard/                State machine, key model, engine
│   ├── gestures/                Swipe tracker, gesture recognizer
│   ├── layouts/                 Layout schema + builtins
│   ├── ui/                      GTK widgets, animations, theme
│   ├── voice/                   cpal recorder + whisper-rs transcriber
│   └── ipc/                     D-Bus service
├── resources/
│   ├── layouts/*.json           QWERTY, Dvorak, symbols
│   ├── styles/keyboard.css      GTK CSS for slick visuals
│   └── dictionary_en.txt        Word list for swipe recognition
├── scripts/
│   ├── install.sh
│   ├── uninstall.sh
│   └── download-model.sh
├── systemd/
│   └── rusty-board.service      User-mode autostart
└── docs/
    ├── ARCHITECTURE.md
    └── CUSTOMIZATION.md
```

## Architecture overview

The keyboard separates concerns cleanly so each part is testable in isolation:

1. **`keyboard::engine::KeyboardEngine`** is the heart. The UI sends it `EngineInput` (taps, swipes, focus changes); it emits `EngineEvent`s (text committed, state changed) which both the UI and the IPC layer subscribe to.
2. **`gestures::SwipeTracker`** accumulates pointer samples while a drag is in progress. On release, **`gestures::GestureRecognizer`** snaps each sample to the nearest letter key, dedupes consecutive duplicates, and ranks dictionary words by a subsequence similarity score.
3. **`voice::VoiceService`** owns a `cpal` input stream (in `recorder`) and a `whisper-rs` transcriber. Audio is written into a ring buffer, drained on stop, resampled to 16 kHz, and run through Whisper.
4. **`ui::KeyboardView`** is a GTK4 `Box` that builds key widgets from the active `Layout`, attaches a `GestureDrag` for swipe detection, and reacts to engine events on a 30 Hz GLib timeout.

## Voice setup

Voice input needs a Whisper model. Download one with:

```bash
./scripts/download-model.sh base.en        # ~150MB, good quality
./scripts/download-model.sh tiny.en        # ~75MB, faster, ok quality
./scripts/download-model.sh small.en       # ~480MB, better quality
```

Models go to `~/.local/share/rusty-board/models/`. `RustyBoard` finds them automatically; you can override the path in `config.toml`.

## D-Bus interface

```bash
# Show / hide the keyboard
busctl --user call io.rustyboard.Keyboard /io/rustyboard/Keyboard io.rustyboard.Keyboard1 Show
busctl --user call io.rustyboard.Keyboard /io/rustyboard/Keyboard io.rustyboard.Keyboard1 Hide

# Trigger voice
busctl --user call io.rustyboard.Keyboard /io/rustyboard/Keyboard io.rustyboard.Keyboard1 StartVoice

# Listen for committed text
busctl --user monitor io.rustyboard.Keyboard
```

## Testing

```bash
cargo test                       # unit tests (no GTK required)
cargo test --no-default-features # without voice / D-Bus
```

Unit tests cover the state machine, swipe path decoding, dictionary recognition, audio resampling, and config (de)serialization.

## License

GPL-3.0-or-later. See `LICENSE`.

## Status

Early development. Working: state machine, gesture decoding, layouts, GTK UI scaffolding, voice service, D-Bus skeleton. In progress: Wayland virtual keyboard backend, popover for long-press, full word-prediction integration, on-screen swipe trail rendering.

PRs welcome.
