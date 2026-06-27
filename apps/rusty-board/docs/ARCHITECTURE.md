# Architecture

This document describes how RustyBoard's modules fit together. It's intended as a guide for contributors and as a record of design decisions.

## Module map

```
main.rs                     Application bootstrap
└── ui::window              Builds the adw::ApplicationWindow
    └── ui::keyboard_view   The keyboard widget itself
        ├── layouts         Static layout definitions (JSON-loadable)
        ├── ui::key_widget  One Button per key
        ├── ui::animations  Spring & timed animations
        ├── ui::theme       CSS provider
        └── gestures        SwipeTracker + GestureRecognizer
            └── keyboard::engine   Central state owner
                └── keyboard::state, keyboard::key
```

Side modules:

- `voice::` runs in its own thread; talks to the engine through a channel.
- `ipc::dbus` runs on a tokio runtime; broadcasts engine events.
- `config::` is a plain TOML loader, no async, no threading.

## The engine pattern

`KeyboardEngine` is the single source of truth for keyboard state. The UI never mutates state directly — it sends `EngineInput` and reacts to `EngineEvent`. This means:

1. **Tests can drive the engine without GTK.** All state-machine and gesture-recognition tests run headless.
2. **Multiple front-ends are possible.** A future Wayland-only mode that bypasses GTK's window can drive the same engine.
3. **D-Bus is just another consumer.** It subscribes to `EngineEvent::TextCommitted` and forwards it to the bus.

Channels are crossbeam unbounded — backpressure is unlikely on a keyboard.

## Why one-shot shift, separate caps lock

Mobile users tap shift, type one capital, expect to return to lowercase. Desktop users sometimes want capslock. The state machine encodes both:

- `Normal -> Shifted -> CapsLock -> Normal` cycles on each shift tap.
- `after_keypress()` resets `Shifted -> Normal` but leaves `CapsLock` alone.

This is in `keyboard::state::KeyboardState`. Tests in the same file pin the behavior.

## Swipe decoding

The path from a finger drag to a word has four stages now (was three before — we added pivot extraction and a much richer scoring layer):

### 1. Sample collection (`gestures::swipe::SwipeTracker`)

While dragging:
- Reject samples within `min_sample_distance` (4px) of the previous accepted one — prevents jitter accumulation.
- Apply exponential smoothing (`α = 1 - smoothing`) to suppress finger tremor.
- Each sample is timestamped (`SwipePoint::with_timestamp` for tests; `SwipePoint::new` for live capture).

### 2. Pivot extraction (`gestures::swipe::extract_pivots`)

Naive subsampling ("every 3rd point") loses information. Real swipe-typing systems detect *pivots* — points where the user lingers on a key. We detect pivots two ways:

- **Velocity minima.** Compute per-segment velocity `dist/dt`. A point is a pivot if both its incoming and outgoing velocity are below 30% of the path's max velocity (an adaptive threshold — works at any swipe speed).
- **Direction changes.** If the angle between (prev→p) and (p→next) is sharper than ~140° interior (turn > 40°), the point is a pivot.

Pivots within `min_pivot_separation` (25px) of the previous pivot are dropped — wobble around a single key shouldn't generate three pivots.

The first and last sample are always pivots — users land deliberately at the start and end.

### 3. Pivots → raw word (`gestures::swipe::decode_swipe_path`)

For each pivot, find the nearest hitbox flagged `swipe_target`. Emit the key's label only if it differs from the last emitted label (collapses duplicates). Result: a noisy raw string like `helo`, `hippo`, or `keybard`.

### 4. Raw → ranked candidates (`gestures::recognizer::GestureRecognizer`)

The previous version used `is_subsequence` + length penalty, which conflates good and bad matches. The new version scores each candidate by **summing four signals**:

```
score = edit_distance × 1.0           # Damerau-Levenshtein (incl. transpositions), capped at 4
      + spatial_distance × 0.4        # avg key-to-key distance for aligned letters
      + endpoint_bonus                # -1.5 if both endpoints match, -0.5 if one
      + frequency_penalty             # ln(rank+1)/10, capped at 1.0
```

Lower is better. The four signals interact in useful ways:

- **DL distance with transposition** correctly handles `hlelo` → `hello` as a single edit (not two).
- **Spatial scoring** disambiguates substitutions: `helli` → `hello` (i↔o adjacent) beats `helli` → `helps` (i↔p further).
- **Endpoint bonus** isn't a hard filter (the old version's was, which dropped legit matches when the user mis-released); it's just a strong nudge.
- **Frequency penalty** breaks ties: at score parity, `the` beats `tho`.

The recognizer's spatial map is refreshed from actual on-screen widget positions on every swipe completion, so it stays in sync with whatever layout (and screen size) the user is on. Unit-key normalization makes the scale window-size-independent.

## Live swipe trail

The keyboard is wrapped in a `gtk4::Overlay`. The `KeyboardView` lives at the bottom; a `SwipeTrail` widget (a `DrawingArea` with `can_target=false`) floats above it.

While the user drags, every motion update is pushed to the trail's point buffer and `queue_draw()` is called. Cairo renders the path in two passes:

1. **Outer halo** — wide, faint, blurry. Width tapers from 4px (tail) to 18px (head); alpha 0.04 → 0.22.
2. **Inner stroke** — narrow, opaque core. Width 1.5px → 7px; alpha 0.05 → 0.85.

Plus a bright dot + white inner highlight at the head for that "wet ink" look.

When the gesture ends, the trail fades over ~250ms via a 60Hz timeout that decrements alpha by 0.06 per frame, then clears the points. On cancel (insignificant swipe), the trail is wiped immediately.

Pointer events fall through to the keyboard underneath because the DrawingArea sets `can-target=false`.

## Ortholinear layout support

Layouts now have an `ortholinear: bool` flag and a `grid_columns` width. When ortholinear:

- The `KeyboardView` ignores per-row `offset` values (which create the traditional QWERTY-style stagger).
- Rows must sum to exactly `grid_columns` units; the `default_dvorak_ortholinear()` constructor enforces this and tests verify it row-by-row.
- All modifier/special keys default to width 1.0 (instead of 1.5) so the grid stays perfectly rectangular.
- Trailing-edge padding fills any short rows so the keyboard always renders as a clean rectangle.

The Dvorak ortho layout uses 10 columns × 4 rows. Rows 1 and 2 are canonical Dvorak (`' , . p y f g c r l` and `a o e u i d h t n s`); rows 3 and 4 redistribute the remaining 8 letters and modifier keys to fit exactly 10 cells per row. All 26 letters of the alphabet are present (verified by test).

## Voice pipeline

```
[mic] → cpal stream → callback → mono mixdown → HeapRb<f32>
                                                      │
                                                      ▼ (on stop)
                                                drain → resample → whisper
```

- The cpal stream runs on the audio thread, not the GTK thread.
- The `HeapRb` from `ringbuf` is lock-free for the producer/consumer roles, but we wrap it in a `Mutex` for simplicity since contention is essentially zero.
- Resampling is naive linear interpolation. Speech has narrow bandwidth, so this is fine; if you need fidelity, swap in a windowed-sinc.
- Whisper runs synchronously (it's CPU-bound; threading happens inside whisper.cpp itself via `set_n_threads`).

## Speed-aware scoring

Real-world swipes range from ~0.1 px/ms (deliberate, "tracing" the letters) to ~3 px/ms (slap-it-out fast). At those extremes the user is signaling different intent, so the recognizer adapts:

| Signal | Slow swipe (~0.1 px/ms) | Fast swipe (~3.0 px/ms) |
|---|---|---|
| Edit-distance weight | 0.3 (lenient) | 0.7 (strict) |
| Spatial weight | 1.2 (high) | 0.8 (lower) |
| Frequency weight | 0.7 | 1.3 |
| Endpoint bonus | small | larger |
| Length penalty | low | higher |
| Raw-fallback score | 1.5 (raw can win) | 5.5 (dictionary almost always wins) |

The intuition: when you swipe slowly you're being precise — your raw input deserves to compete with the dictionary's suggestions. When you swipe fast, you trust the keyboard to clean up after you.

The interpolation is linear in `speed_factor = clamp(velocity / 2.0, 0, 1)`. `path_avg_velocity()` in `gestures::swipe` computes the value, and `recognizer::candidates_for_raw_with_speed` consumes it.

## Compositor integration

This is the part that mirrors what Maliit's QML+platform-plugin combo does:

### Window placement (layer-shell)

The keyboard window is configured as a Wayland *layer surface* via `gtk4-layer-shell`, before `present()`:

```text
window.init_layer_shell()
      .set_layer(Top)            // above normal apps, below modals
      .set_anchor(Bottom)        // dock at bottom edge
      .set_anchor(Left + Right)  // span full width
      .set_keyboard_mode(None)   // never receive keyboard focus
      .auto_exclusive_zone()     // reserve our height; other windows resize around us
```

The `KeyboardMode::None` is the part that maps to "this is an OSK, not a normal app." The user's typed input goes to whatever app was focused when the keyboard appeared; we deliver characters to that app via `virtual_keyboard_v1` (below). On X11 / compositors without layer-shell, we fall back to a borderless toplevel — works but doesn't reserve space and can be raised over.

Maliit does the same thing through the Qt platform plugin `maliit-input-method`; QML side just sets `MaliitGeometry.shown = true/false` and the platform layer translates that to compositor IPC. RustyBoard inverts the layering: the layer-shell library handles geometry directly, and a thin `compositor::placement` module is the only platform-aware part.

### Key delivery (virtual keyboard)

The `compositor::VirtualKeyboard` trait abstracts how characters reach the focused app:

- **`WaylandVirtualKeyboard`** (real implementation in `compositor::wayland_backend`) — uses `zwp_virtual_keyboard_v1`. Compositor support: Phosh, Sway, river, Wayfire, KWin. The backend shares GTK's Wayland connection via `gdk4-wayland` with the `wayland_crate` feature enabled, so we don't open a second connection (which would compete with GTK for input-method routing). Sends a hardcoded US-ASCII xkb keymap to the compositor at startup, then translates `commit_text(&str)` calls to per-character `key()` events with `modifiers()` for shifted characters.
- **`X11VirtualKeyboard`** — shells out to `xdotool type --clearmodifiers --` for text and `xdotool key` for keysyms. Fully working.
- **`NoopVirtualKeyboard`** — logs only; useful for development and screenshot mode.

Selection happens at startup in `VirtualKeyboardBackend::detect()`: try Wayland first if `$WAYLAND_DISPLAY` is set, then X11 if `xdotool` is in `$PATH`, otherwise no-op.

### Show/hide on focus (input-method)

The `compositor::wayland_backend` module also binds `zwp_input_method_v2` on the same shared connection. The compositor sends `Activate` when an editable widget gets focus and `Deactivate` when focus leaves; we wire these to `window.set_visible(true/false)` so the keyboard auto-shows when the user starts typing and auto-hides otherwise. Per-protocol semantics: events are double-buffered, applied on `Done`. We track pending state and fire the user callback only when the new state differs from the current one.

Compositors that support `zwp_input_method_v2` include Sway, river, Wayfire, Phosh. KWin's input-method support is partial (KWin 6.x is generally compatible). Compositors without it: GNOME's Mutter (uses its own ibus integration), Hyprland (limited).

If the compositor reports `Unavailable` (typically because another OSK like fcitx5 has already bound the seat's input-method object), we log a warning and the keyboard stays always-visible. There can only be one input-method client per seat.

Configuration: `behavior.auto_hide_on_no_focus = true | false` in `~/.config/rusty-board/config.toml`. Defaults to `true`.

### Connection sharing

The trickiest part of the implementation is sharing GTK's `wl_display` rather than opening a second one. The reason: input-method routing is per-seat, and the compositor only forwards focus events to *one* input-method client per seat. If we opened our own connection we'd be a different client from GTK and might race for the binding. By piggybacking on GDK's connection (via `gdk4_wayland::WaylandDisplay::connection()` and `gdk4_wayland::WaylandSeat::wl_seat()`), we use the same client identity GTK uses.

We do create our own `EventQueue` separate from GDK's, so dispatch of our protocol events doesn't interfere with GDK's own GTK widget events. GDK reads the wl fd; events for our protocol objects get sorted into our queue's internal buffer; we drain that via a 50ms GLib timeout calling `dispatch_pending()`.

### Caveats and known limitations

- **No `text-input-v3` direct binding** — we get focus events via input-method's Activate/Deactivate rather than text-input directly. This is the standard OSK pattern; text-input-v3 is what apps use to *talk to* the input-method.
- **US ASCII keymap only** — chars outside the US-ASCII range fall through to no-op (logged at debug level). A full impl would dynamically generate the keymap from a user-configurable layout. For now the recommendation is: type accented characters via long-press popovers, which produce the actual character (the swipe/tap pipeline produces UTF-8) — but `commit_text` will silently skip them on the Wayland backend until we expand the keymap.
- **No InputMethodKeyboardGrab** — the protocol allows the input method to grab keyboard input from the compositor (for CJK composing). We don't use this; we're a tap/swipe keyboard, not a composing one.

## CSS strategy

We use libadwaita's *semantic* CSS variables (`@view_bg_color`, `@card_bg_color`, `@borders`) so a single stylesheet adapts to light, dark, and high-contrast themes. User overrides land at the bottom of the loaded CSS as `:root` variables (`--rb-key-radius`, etc.) so they cascade over the defaults without us having to template the whole sheet.

## Animations

GTK4 alone has no friendly animation API; libadwaita does. We use:

- **`adw::TimedAnimation`** for finite eased transitions (key press flash, slide-in).
- **`adw::SpringAnimation`** for state changes (shift toggle visual feedback). Springs feel more "physical" than ease curves.
- **CSS `@keyframes`** for indefinite loops (voice recording pulse) — doesn't need driving from Rust.

## Threading

- **Main / GTK thread**: window, widgets, engine input dispatch, animation play, virtual-keyboard bridge.
- **Audio thread** (cpal-managed): pushes samples into the ring buffer.
- **Tokio thread** (D-Bus): listens for method calls, emits signals.
- **Whisper thread**: spawned per transcription via `std::thread::spawn` so we don't block the audio drain. (Currently inline; see `voice/service.rs` TODO.)

The engine is `Send + Sync` (everything inside is `Arc<RwLock<>>` or channels). The UI never blocks on the engine.

## What's not done yet

- **Non-ASCII key delivery on Wayland.** The Wayland backend's keymap is hardcoded US ASCII. Characters outside that range (accented letters, CJK, emoji) fall through to debug-level skips. A complete impl would dynamically generate the xkb keymap from the active layout configuration.
- **Long-press popover.** The data is in `Key::extended`; the popover widget isn't built yet.
- **Word prediction.** The suggestions bar is hardcoded; no N-gram model or context-aware lookup.
- **Alignment-aware spatial scoring.** Spatial distance currently does naive position-by-position matching, which loses fidelity when raw and candidate differ in length. The proper fix is to walk the edit-distance DP path and score along it.
- **InputMethodKeyboardGrab path.** We don't currently grab the compositor's hardware keyboard input through the input-method protocol. CJK composing would need this; tap/swipe input doesn't.

