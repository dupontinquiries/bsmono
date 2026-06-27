# RustyBoard → Qt Rewrite Handoff

**Audience.** A developer or LLM session starting fresh on a Rust + Qt rewrite of RustyBoard. You should read this top-to-bottom before writing any code. It encodes ~6 months of bug-hunting and architectural decisions that aren't obvious from reading the source.

**TL;DR.** Keep the Rust core (`keyboard::engine`, `keyboard::state`, `gestures::*`, `keyboard::code_mode`, layout definitions, voice/llm modules). Replace the GTK4/libadwaita layer (`ui::*`, the layer-shell integration, the trail widget, the settings panel) with QtQuick + KWayland. The split is clean because the engine was designed engine-first from day one — the UI only sends `EngineInput` and reacts to `EngineEvent`.

---

## 1. Project context

### What RustyBoard is

An on-screen keyboard for **Plasma Mobile on Alpine postmarketOS, running on Fairphone 5**. Targets Wayland exclusively (no X11 fallback worth keeping). Built as a Maliit successor because Maliit's QML keyboard had unfixable issues with shift state, swipe typing, and word ribbon rendering.

### What users want from it

- **Swipe typing** that actually works (the Maliit one drops half the inputs)
- **Cardinal-grid mode** — an experimental gesture system where each large cell has up/down/left/right/diagonal gestures emitting different characters. Fewer keys, bigger touch targets, better thumb-typing
- **Stenotype mode** — chord-based steno for advanced users
- **Voice input** via whisper.cpp (`whisper-rs` binding)
- **LLM-assisted swipe disambiguation** (optional feature flag) — local llama.cpp model reranks ambiguous swipe candidates
- **Single-hand thumb-arc mode** — compacts the keyboard into a thumb-reachable wedge
- **Custom layouts** — `default_cardinal_grid_words` with prose/C/Rust/Python clusters in the rightmost columns

### Target platform constraints

- **Fairphone 5**: ARMv8, ~6 GB RAM, 1080×2340 screen. Phone runs Alpine pmOS, kernel ~6.6, KWin 6.x.
- **Layer-shell anchoring**: keyboard docks at the bottom with an exclusive zone so other windows resize around it
- **KWin only exposes `zwp_input_method_v1` + `zwp_input_panel_v1`** (NOT the v2 protocols used by Phosh/wlroots). This was a major surprise; see §7.
- **Plasma Mobile's `kwinrc`** is the registration mechanism: `[Wayland] InputMethod=/path/to/.desktop`. KWin launches RustyBoard via `--inputmethod`.

### Repo & related transcripts

Source lives at `/home/kesslerd/Code/rust/bigsilicon--rusty-board/rusty-board` (laptop) and `/home/user/Code/rusty-board/` (phone). The current dev sandbox is `/home/claude/rusty-board/`. Project summary + transcript catalog: `/mnt/transcripts/journal.txt`. The most recent bug-hunt transcripts are dated 2026-06-21; earlier sessions cover Wayland protocol work (May 2026), Plasma Mobile integration (mid-May), and the original Maliit codebase the project replaces (March 2026).

Current state: **542 Rust unit tests + 13 Python sim scenarios passing**. Two user-reported swipe bugs are open at handoff (see §10).

---

## 2. Why Qt is being considered

This isn't documented in the codebase yet — capture the *actual* reasons during the rewrite so future-you knows what to optimize for. Likely candidates:

- **GTK4 on KWin is a second-class citizen.** Plasma Mobile is a KDE/Qt environment. GTK4 input-method clients work, but the layer-shell-on-KWin support has been fragile (we hit `compute_bounds → None` issues that may underlie the swipe bugs — see §6).
- **`gtk4-layer-shell` is a hack.** Requires LD_PRELOAD because the wayland protocol calls have to be intercepted *before* libwayland-client resolves them. The pre-exec dance in `main.rs` re-execs the binary with `LD_PRELOAD=libgtk4-layer-shell.so` set. Qt's `LayerShellQt` is a first-party KDE module — no hacks.
- **KWayland integration is native in Qt.** `KWayland::Client::PlasmaWindowManagement`, `KWayland::Client::TextInputManager`, etc. exist as proper Qt types. Doing the equivalent in GTK requires raw `wayland-client-sys` calls (see `src/compositor/wayland_im_v1.rs`).
- **Touch input on KWin via GTK4 has consistency issues.** We've seen first-tap loss, focus glitches, and coordinate-frame mismatches that may be GTK4↔KWin protocol negotiation rather than RustyBoard bugs.
- **The user base on Plasma Mobile expects Qt UI.** Visual consistency matters for adoption.

What you DON'T get from Qt:
- Better swipe gesture recognition (the recognizer is in Rust, unchanged)
- Better whisper integration (the voice service is in Rust, unchanged)
- A faster engine (the engine is already lock-optimized; see §11)
- A magical fix to the cardinal/alphabet swipe bugs (those are coordinate-frame, possibly fixable in either toolkit — see §6)

---

## 3. Architecture: what to preserve

The codebase is engine-first. The UI is a thin reactive shell over the engine. **Keep the engine and gesture core intact; replace only the UI layer.**

### Keep verbatim

```
src/keyboard/
├── engine.rs         ← Single source of truth for keyboard state
├── state.rs          ← KeypadState + StateMachine (one-shot shift / capslock cycle)
├── key.rs            ← Key struct, KeyAction enum, KeyGestures struct
├── code_mode.rs      ← Cardinal-grid-words language toggle (Prose/C/Rust/Python)
├── steno/            ← Phoneme buffer + dictionary lookup
└── character_draw.rs ← Handwriting input canvas state
```

The engine API surface is `EngineInput → handle_input() → Vec<EngineEvent>`. Everything is synchronous; events that need to fire asynchronously go through `set_direct_dispatch(Rc<dyn Fn(&EngineEvent)>)` callbacks. The Qt UI registers ONE direct-dispatch callback that translates engine events into UI updates.

### Keep verbatim

```
src/gestures/
├── swipe.rs          ← SwipeTracker (exponential smoothing), extract_pivots, decode_swipe_path, KeyHitbox
├── cardinal.rs       ← Cardinal classifier (Tap/Cardinal/Hairpin/Rotation/FLICK)
├── boundary.rs       ← Springboard segmentation (multi-word swipes)
├── recognizer.rs     ← GestureRecognizer (dictionary scoring with spatial distance)
├── ai_swipe.rs       ← Adaptive dictionary + n-gram reranking
├── adaptive_dictionary.rs
├── ngram.rs
├── hitbox_nudger.rs  ← Per-user hitbox adaptation (used minimally)
└── llm_swipe.rs      ← Optional LLM-FFI rerank (gated behind `llama-ffi` feature)
```

The gesture pipeline has been bug-hunted extensively. See §5 for the hard-won knowledge — DO NOT rewrite the thresholds, classifier order, smoothing constants, or recognizer scoring without understanding why each is what it is.

### Keep verbatim

```
src/layouts/mod.rs    ← All layout definitions (qwerty, dvorak, cardinal_grid, cardinal_grid_words, steno, symbols, numbers, arrows)
src/voice/            ← Voice service (whisper-rs)
src/config/mod.rs     ← Config (TOML, serde)
src/keyboard/dispatch_mode.rs  ← Standard/Cardinal/Stenotype routing classifier
```

### Adapt or rewrite

```
src/ui/                       ← All GTK4 widget code; rewrite as QtQuick + QML
src/compositor/placement.rs   ← gtk4-layer-shell integration; replace with LayerShellQt
src/compositor/wayland_im_v1.rs   ← Direct Wayland-client calls; replace with KWayland::Client
src/main.rs                   ← LD_PRELOAD re-exec dance (drop), GTK init (drop), gtk4::Application::run (replace with QGuiApplication::exec)
```

### Optional / rewrite if you want

```
src/ipc/dbus.rs   ← D-Bus event broadcast (used by external tools to monitor commits). Tokio-based. If keeping, easy. If dropping, simplify.
```

---

## 4. The engine pattern (don't break this)

`KeyboardEngine` is wrapped in `Rc<RefCell<>>` (single-threaded; we don't need Send) and holds state in `parking_lot::RwLock`. **All state mutations are write-locked; reads use read-locks**.

### Event flow

```
User taps key 'a'
  → drag handler captures the tap
  → engine.handle_input(EngineInput::KeyTap(Key::character("a")))
  → engine acquires write lock, runs state machine, releases
  → returns Vec<EngineEvent>  (e.g. [TextCommitted("a"), StateChanged(Normal)])
  → handle_input ALSO fires direct_dispatch callback for each event (UI bridge)
  → UI receives TextCommitted, forwards to vkbd.commit_text(text) → Wayland
  → UI receives StateChanged, updates key visuals via KeyWidget::update_for_state
```

The engine is the only thing that knows about shift state, auto-cap, swipe-auto-space, last_commit_was_swipe, etc. The UI doesn't peek at this — it just listens for `StateChanged` events.

### Lock discipline

Multiple recent perf rounds **collapsed dual lock acquisitions into single write-locks** in the engine arms (NamedKey, SurroundingTextChanged, Character|Literal|Space, SwipeWord, StenoCommit). Don't reintroduce sequential `state.read()` then `state.write()` patterns — they cause ~3× the lock overhead on the hot path.

If you port the engine to Qt, the lock pattern stays. Rust `RwLock` is fine across an FFI boundary as long as the UI side doesn't try to mutate engine state from a different thread. Qt event loop runs on the main thread; FFI calls into Rust will be single-threaded. ✓

### Direct dispatch vs event channel

The engine supports two delivery modes:
1. `handle_input` returns `Vec<EngineEvent>` synchronously
2. `set_direct_dispatch(Rc<dyn Fn(&EngineEvent)>)` lets the engine fire events into a callback before returning

The UI uses (2) for low-latency commits (text appears in the focused app before `handle_input` returns). D-Bus subscribers use (1) — they pull from the returned Vec via a polling layer.

Qt port: use a QML `Connections` object on a singleton `EngineBridge` that exposes `engineEvent(QString, QVariantMap)` signals.

---

## 5. The gesture pipeline (read this carefully)

### Hit-test → drag_begin → drag_update → drag_end → dispatch

This is the chain. Every step has subtleties:

#### 5.1 Hit-test (where the touch lands)

`KeyHitbox::contains(x, y)` uses **inclusive** bounds on both sides (`x >= b.x && x <= b.x + b.width`). This is used in `drag_begin` and most dispatch paths. There's ONE inconsistency in `dispatch_cardinal_swipe` (`src/ui/keyboard_view.rs:~4604`) that uses **strict** `<` instead — flagged but not yet fixed. Use `contains()` everywhere in the rewrite.

#### 5.2 Drag begin → behavior classification

Based on the hit, classify the drag's `ButtonBehavior`:
- `Normal` — a regular key tap-or-cancel
- `SwipeSource` — letter key in standard mode, swipes feed the word recognizer
- `Cardinal` — cell in cardinal-grid or steno mode, swipes feed `dispatch_cardinal_swipe`

The classification logic has THREE branches:
1. **Hit + standard mode + Character action + swipe-typing enabled** → upgrade Normal to SwipeSource
2. **Hit + cardinal mode + SwipeSource behavior** → downgrade SwipeSource to Normal (cardinal cells shouldn't freeform-swipe)
3. **Hit miss + fallback**:
   - Cardinal mode + suppress_freeform → Some(Cardinal) (off-cell starts still dispatch as cardinal)
   - Standard mode + swipe_typing_enabled → Some(SwipeSource) (off-key starts can still swipe-type)

Both fallbacks were added recently because the rewrite-swipes commit broke them. Keep them.

#### 5.3 Swipe tracker (with smoothing)

`SwipeTracker::update(x, y)` applies exponential smoothing: `blended = α * prev + (1-α) * new` where α=0.3 (production). Returns `bool` indicating whether a point was actually pushed (i.e., distance from previous >= min_sample_distance=4 px).

**Smoothing factor 0.3 reduces stored max_excursion by ~10-20% on short paths (5 samples).** Earlier we thought it was 30% — empirically tested and it's less. Real 18-20 px raw flicks store ~16-17 px excursion. Calibrate thresholds against THIS, not against raw values.

#### 5.4 Cardinal classifier (`src/gestures/cardinal.rs`)

This is the most-bug-hunted piece. Classification order matters:

```
1. Tap check        (excursion < 13 px AND duration < tap_max_ms) → Tap
2. Rotation check   (path traces 75%+ around a region cycle)        → Rotation(CW/CCW)
3. Hairpin check    (excursion >= hairpin_min AND end near start)   → Hairpin(direction)
4. Cardinal check   (end_dist_from_start >= tap_radius_px=20)        → Cardinal(direction)
5. FLICK fallback   (excursion >= 13 px AND duration < hairpin_max) → Cardinal(direction)
6. Otherwise        → Unrecognized
```

**Recently fixed**: `FLICK_MIN_EXCURSION_PX` was 15, leaving a gap (13, 15) where gestures classified as Unrecognized. The fix is to set it to 13 (matching `TAP_MAX_EXCURSION_PX`), closing the gap. The lenient threshold profile (used when single-hand mode is on) has `tap_radius_px=26`, which makes the gap WORSE there — every short cardinal-mode swipe goes through FLICK in lenient mode. Keep `FLICK_MIN = TAP_MAX = 13` in the rewrite.

**Direction calculation** for Cardinal/Hairpin/FLICK uses `direction_from(start.x, start.y, mx, my)` where `(mx, my)` is the max-excursion point. `direction_from` returns an 8-way direction; if `cardinal_diagonals()` is false (default), it folds to 4-way via `fold_to_four_way`.

**Local-frame translation**: `dispatch_cardinal_swipe` translates the path so the source cell's center is at origin BEFORE calling classify. This makes max_excursion magnitudes consistent across cells of different sizes. The cardinal classifier itself doesn't know about this translation — it just sees the relative path.

**Lenient mode thresholds**: `CardinalThresholds::lenient()` is used when single-hand mode is on. Wider tap radius, longer max times, more tolerant. The default `CardinalThresholds::default()` is the strict profile.

#### 5.5 Word recognizer (`src/gestures/recognizer.rs`)

For SwipeSource dispatches:
```
seg.points → decode_swipe_path → raw string of letters
            (via extract_pivots → pivots_to_word)
recognize_with_speed(path, keys, avg_v) → Vec<WordCandidate>
            (returns [] if raw is empty)
candidates  → apply_to_candidates (AI rerank) → take .first() → chosen
chosen ?? raw   → push to accumulated
```

If the gesture spans the spacebar springboard, `segment_path` splits the path into multiple segments and runs the recognizer per-segment, joining with spaces. This is the multi-word-swipe path.

**Position normalization**: `set_key_positions` stores key centers normalized by the median key width (`unit`). Different layouts can use this without rescoring; same dictionary scores correctly across qwerty/dvorak.

**Recognizer cache** (`LAST_RECOGNIZER_REFRESH_GEN` atomic): the recognizer's positions are only rebuilt when `HITBOX_GENERATION` changes (i.e., on `rebuild_view` calls). Don't reintroduce per-swipe position rebuilds — they were 30+ unnecessary hashmap inserts per gesture.

#### 5.6 Trail rendering

`SwipeTrail` widget renders the user's drag path as a colored line. Currently a GTK4 custom widget using `DrawingArea` + cairo. Key parameters:
- `TRAIL_CAP = 256` points (amortized trim by 32)
- `3 stroke bands` per layer (cumulative gradient look)
- Color from `cfg.appearance.colors.trail` (or accent fallback)

For Qt: use `QQuickPaintedItem` or a custom QML `Canvas`. The point-collection logic and band-rendering stays in Rust; the Qt side just gets a path-to-render. **Trail draws in overlay frame, not KB frame** (see §6).

---

## 6. ⚠️ THE COORDINATE SYSTEM (open issue at handoff)

This is the current open bug. **Read carefully before designing the Qt UI's coordinate handling.**

### The current pipeline

```
Touch event at OVERLAY frame coords (x, y)
  ↓
drag_begin: offset = kb_box.compute_bounds(overlay).map(...).unwrap_or((0, 0))
  ↓
kx = x - offset.x, ky = y - offset.y   (KB frame)
  ↓
swipe.lock().start(kx, ky)   (tracker stores KB-frame points)
  ↓
drag_update: same offset (cached in offset_for_update Cell)
  swipe.lock().update(kx, ky)
  trail.push(overlay_x, overlay_y)   ← trail uses OVERLAY frame
  ↓
drag_end: path = tracker.finish()
  refresh_hitboxes (hitboxes computed via widget.compute_bounds(keyboard_box) — KB frame)
  ↓
dispatch_swipe_source / dispatch_cardinal_swipe receives path (KB) + boxes (KB)
```

### The hypothesized failure mode

`gtk4::Widget::compute_bounds` can transiently return `None` during Plasma Mobile's layer-surface "mapped-but-not-fully-sized" window. When that happens at drag_begin, `offset` falls back to `(0, 0)` and:
- `kx, ky` end up in OVERLAY frame (not KB frame)
- Hitboxes are still computed in KB frame (different timing or different widget tree branch)
- **Frame mismatch silently breaks recognition** — the path lands on the wrong row of letters, OR the cardinal hit-test misses the cell entirely

We shipped a defensive fix (`recover_frame` helper in `src/ui/keyboard_view.rs`) that recomputes the offset at drag_end and translates the path by the delta if it drifted. The fix is gated on a 1-px tolerance — small noise doesn't trigger correction.

**Whether this is actually the bug**: unconfirmed at handoff. Unit tests pass because they don't go through GTK. The user reported the bugs persist after defensive fix; on-device journal logs were not yet pulled.

### What the Qt rewrite should do

**Pick one frame and stick to it.** Suggested approach:

1. **Run the gesture controller on the keyboard surface directly** (`MouseArea` or `MultiPointTouchArea` inside the keyboard QML root). Coordinates come in keyboard-local frame natively. No translation needed.
2. **Compute hitboxes in the same frame**. Each Key QML item's `mapToItem(root, 0, 0)` gives its position in keyboard-frame.
3. **Trail renders in keyboard-frame too**. No overlay-vs-content frame split.
4. **If you must support overshoot** (drags starting just outside the keyboard), use a slightly larger MouseArea on the parent and translate the start point to keyboard-frame ONCE at gesture begin, using `mapToItem` which DOES NOT fail the way `compute_bounds` does.

**Avoid `mapFromGlobal` for in-app coords.** It involves more transformations and has scaling pitfalls on high-DPI Wayland.

### A separate concern: HiDPI / Wayland scaling

The Fairphone 5 runs at logical 1.5× scale (or similar). GTK4 handles this transparently — events and `compute_bounds` are both in logical pixels. Qt also handles this, but with different defaults. Make sure:
- `QGuiApplication::setHighDpiScaleFactorRoundingPolicy(Qt::HighDpiScaleFactorRoundingPolicy::PassThrough)` for fractional scaling
- Touch coords from `QTouchEvent::points()[0].position()` are in QQuickItem-local logical coords
- Don't mix `QPointF` from `QTouchEvent::globalPosition()` (screen frame) with item-local coords

---

## 7. ⚠️ Wayland integration (Plasma Mobile specifics)

This is the OTHER big rewrite area. The current implementation uses raw wayland-client-sys; the Qt port should use KWayland::Client (much less code, but you need to know the specifics).

### Protocols needed

| Protocol | Used for | Plasma Mobile KWin support |
|---|---|---|
| `zwlr_layer_shell_v1` | Anchor keyboard at bottom + exclusive zone | ✅ Yes |
| `zwp_input_method_v1` | Activate/Deactivate from focused-app context | ✅ Yes (only v1, NOT v2) |
| `zwp_input_panel_v1` | Mark keyboard surface as an input-panel | ✅ Yes |
| `zwp_virtual_keyboard_v1` | Send key events back to focused app | ✅ Yes |
| `zwp_input_method_v2` | Modern variant (Phosh, wlroots) | ❌ NOT exposed by Plasma Mobile's KWin |

**This is the most important compositor detail in the project.** Phosh / wlroots-based compositors use v2; Plasma Mobile uses v1. The v1 protocol has different state machine semantics (no commit_string text replacement, no rich content-purpose handling). All the input-method handling code in `src/compositor/wayland_im_v1.rs` is the v1 version. Use `LayerShellQt` + `KWayland::Client::TextInputManager` (which transparently picks v1 vs v2 depending on compositor advertisement) and let it pick v1 on Plasma Mobile.

### KWin registration ceremony

KWin doesn't accept arbitrary clients as input-methods. The flow:

1. Install `/usr/share/applications/rusty-board.desktop` with `X-KDE-Wayland-VirtualKeyboard=true`
2. Edit `~/.config/kwinrc`:
   ```ini
   [Wayland]
   InputMethod=/usr/share/applications/rusty-board.desktop
   ```
3. Restart the session. KWin launches RustyBoard via `kwin_wayland --inputmethod <command>`
4. The launched binary inherits a privileged Wayland socket via `WAYLAND_SOCKET` fd that exposes the input-method protocols (NOT exposed on the public socket)

The current binary handles the fd inheritance in `src/main.rs` via env-var detection. **The Qt rewrite must replicate this**: detect `WAYLAND_SOCKET` env var, use that fd for connection if set, fall back to `WAYLAND_DISPLAY` otherwise. KWayland's default behavior should handle this — but verify, because there have been edge cases.

### `KWIN_IM_SHOW_ALWAYS=1`

For testing: set this env var in the session to force KWin to always show the keyboard even when not focused. Useful for development on a laptop without a touchscreen. The current code also supports `behavior.auto_hide_on_no_focus = false` in `config.toml` to achieve the same effect from inside RustyBoard.

### `zwp_input_method_v1` state machine

v1 uses these events from compositor → IM:
- `activate(text_input)` — focus arrived; show keyboard
- `deactivate(text_input)` — focus left; hide keyboard
- `surrounding_text(text, cursor, anchor)` — current text context
- `content_type(hint, purpose)` — what kind of field (URL, email, password, etc.)
- `reset` — clear any pending compose state

v1 IM → compositor:
- `commit_string(text)` — text to insert (different from v2's `commit_string`!)
- `preedit_string(text, commit)` — preedit composition
- `keysym(time, keysym, state, modifiers)` — virtual keypress

The current code (`src/compositor/wayland_im_v1.rs`) implements these. On the v1 protocol, `commit_string` is fire-and-forget — the compositor inserts the text and the IM doesn't get an ACK. The v2 version is different (more like a request/response). When porting, treat them as different protocols, not "v1 is a subset of v2."

---

## 8. Layouts

Layouts are statically defined in `src/layouts/mod.rs`. They use a `Layout` struct with:
- `name`, `style` (Standard/CardinalGrid/Stenotype), `letters` (Vec<Vec<Key>>)
- `grid_columns: f32` — the row's width unit invariant (sums of `key.width` in each row must equal this)
- `experimental` flag

### Current layouts

| Name | Style | Notes |
|---|---|---|
| `qwerty` | Standard | Stock qwerty ortholinear |
| `dvorak` | Standard | Dvorak ortholinear, has 0.33-wide arcade-buffer keys flanking spacebar on row 4 (recent addition) |
| `symbols` | Standard | Punct + math + emoji |
| `numbers` | Standard | 10-digit pad |
| `arrows` | Standard | Small 2-row arrow tray (when only nav is needed) |
| `cardinal_grid` | CardinalGrid | 3×3 letter grid + cardinal swipes for vowels |
| `cardinal_grid_words` | CardinalGrid | 5×5 with rightmost 2 columns = code-mode-aware word/template cells |
| `stenotype` | Stenotype | 2-row chord-based steno |

### The `cardinal_grid_words` innovation

The rightmost 2 columns × 3 letter rows = 6 "word cluster" cells. Cell contents change based on `code_mode::current()`:
- **Prose**: articles, "I", to-be verbs, connectors, prepositions, common verbs (all with trailing space)
- **C**: includes, types, control flow, printf/IO, ptr/mem, struct/decls
- **Rust**: `fn`/`pub`, `let`/`mut`, control flow, `?`/`.unwrap`/`Some`/`Ok`, `->`/`<T>`/`&`/`Vec`, `use`/`derive`/`cfg`
- **Python**: `def`/`class`/`lambda`, `self.`/`super()`, `if`/`elif`/`else`/`try`, `return`/`yield`/`raise`, `import`/`from`, `print`/`format`

The LANG button on row 5 cycles via `KeyAction::CodeLangCycle` → `code_mode::cycle()` → triggers a UI rebuild with new cluster contents.

This is innovative and well-loved — keep it identical in the Qt port. The cluster definitions are in `default_cardinal_grid_words()` + 4 helper fns (`prose_clusters`, `c_clusters`, `rust_clusters`, `python_clusters`).

### Arcade buffer keys

Dvorak's row 4 has thin (0.33-wide) `⌫` (left flank) and `⏎` (right flank) keys around the spacebar. Spacebar shrunk from 3.0 → 2.34 to fit. They use the `arcade-buffer` CSS class. In Qt, port the styling but keep the layout structure identical.

### Layout swap mechanics

When the user switches layouts (e.g., via `?123` toggle for symbols, or settings change), `rebuild_view` is called:
1. `HITBOX_GENERATION` atomic increments
2. All child widgets are removed from `keyboard_box`
3. New widgets are built from the new layout's keys
4. The window's `default_height` is updated based on the new row count + `HEIGHT_SCALE`

In Qt: a Component or QML loader pattern works well. Each layout is a `Component`, `Loader.sourceComponent = currentLayout`.

---

## 9. Config + settings

`src/config/mod.rs` defines the TOML schema. Loaded from `~/.config/rusty-board/config.toml`. Hot-reloadable via the settings panel (only some fields take effect immediately; layout changes need a rebuild).

### Critical knobs that affect testing

| Field | Default | What it does |
|---|---|---|
| `behavior.auto_hide_on_no_focus` | `Some(true)` | If false, keyboard stays visible regardless of focus. **Set to false for laptop testing.** |
| `gestures.swipe_typing` | true | Master toggle for freeform swipe-type recognition |
| `gestures.swipe_threshold` | 20.0 | Min path length for `is_significant_swipe` |
| `gestures.smoothing` | 0.3 | Exponential smoothing factor |
| `layout.letters` | "qwerty" | Active letter layout (qwerty/dvorak/cardinal_grid/cardinal_words/stenotype) |
| `layout.cardinal_enabled` | false | Use cardinal_grid mode |
| `layout.compact` | false | Compact rows (38 px instead of 46) |
| `layout.height_scale` | 1.0 | Multiplier on keyboard height [0.5, 1.5] |
| `layout.width_scale` | 1.0 | Multiplier on keyboard width [0.5, 1.5] |
| `layout.bottom_pad_px` | 0 | Extra padding below the keyboard [0, 240] |

### The settings panel

`src/ui/settings_panel.rs` renders sliders, dropdowns, color pickers. Live-updates via an `on_change` callback that:
- Updates `cur_layout` cache, `HEIGHT_SCALE`, `WIDTH_SCALE`, `BOTTOM_PAD`, `COMPACT_MODE` atomics
- Reinstalls CSS provider (for color/compact changes)
- Does **NOT** rebuild the keyboard widget tree (that happens on Done)

The `on_done` callback rebuilds the keyboard. Done is mandatory — without it, the new sizes don't apply to widgets.

In Qt port: use a `Settings` QML object with property bindings to live-update visuals. Layout-mode changes can use a `Loader` with `sourceComponent` swap on commit.

---

## 10. Open bugs at handoff

### 10.1 ⚠️ Cardinal swipes don't work on cardinal/steno layouts (user-reported)

**Symptom**: user makes what they perceive as a directional swipe on a cardinal cell; nothing happens. The threshold-gap part (Tap [< 13] vs FLICK [>= 15]) was fixed by setting FLICK_MIN to 13. After that fix, user reports still not working.

**Hypothesis**: coordinate-frame issue (see §6). Defensive `recover_frame` shipped but unconfirmed on-device.

**Diagnostic added**: `Drag begin: SUSPICIOUS offset` warn-level log fires when `compute_bounds` returns None at drag_begin. On-device journal pull would confirm the hypothesis.

### 10.2 ⚠️ Swipe-typing line shows but does not write on alphabet layout (user-reported)

**Symptom**: user swipes through letter keys on qwerty/dvorak; the trail draws; nothing is committed.

**Pipeline diagnostics added** at `src/ui/keyboard_view.rs:~4515` and `src/keyboard/engine.rs:~360`:
- `swipe pipeline inputs: seg=0 points=N boxes=N swipe_target_keys=N avg_v=X`
- `swipe pipeline output: candidates=N raw="..."`
- `SwipeWord commit: input="..." shifted="..." prepend_space=...`
- `vkbd commit_text ok: N chars` (or `vkbd commit_text failed: ...`)

Whichever line is missing or empty pinpoints the failing stage. Most likely: empty recognizer candidates due to frame-mismatched path (same root cause as 10.1).

### 10.3 Inconsistent hit-test in `dispatch_cardinal_swipe` (known, unshipped)

Line ~4604 uses strict `<` while `KeyHitbox::contains` uses `<=`. Edge-of-cell touches can dispatch to a different cell than drag_begin classified. Fix is trivial: use `contains()`. Just hasn't been prioritized.

### 10.4 `enter_tray_inline` doesn't clear `SWIPE_GESTURE_ACTIVE` (known, unshipped)

After a minimize-fling, the swipe-active flag stays set. First tap after re-show may be suppressed. Symptom hard to reproduce reliably.

### 10.5 `is_significant_swipe` requires `points.len() >= 3` (documented as design, but maybe wrong)

Ultra-fast 2-point flicks fall through this gate. Documented as "by design" in `sim/scenarios.py::two_point_swipe_falls_through_significance`. The Cardinal dead-zone recovery catches some of these; the Standard swipe-source path doesn't.

### 10.6 Row weight is dead code

The `Row.weight: f32` field exists but no code reads it. To make a row taller (e.g., bottom row with arcade buffers), the weight pattern would need wiring up in `build_row_static*` and `desired_height_for_named`. ~4 production call sites + test updates. Not done.

### 10.7 DESIRED_KEYBOARD_HEIGHT is a write-only atomic

`src/ui/keyboard_view.rs:36` declares it; 5 places write to it; nothing reads it. Originally was supposed to be consumed by `placement.rs` but placement uses a hardcoded `set_default_height(320)`. Dead code; document or remove.

### 10.8 `check_auto_cap` fires Shifted on empty surrounding text

Search bars and other fields with no initial content start in Shifted state. Probably intentional ("first character of a new field is capitalized"), but user-reported as annoying for some fields.

---

## 11. Performance lessons (carry to Qt)

These are the hard-won perf optimizations from many bug-hunt cycles. Don't reintroduce the patterns they replaced.

### Single write-lock per engine arm

Old pattern: `state.read()` for some derived value, then `state.write()` for mutation. New pattern: one `state.write()` that does both. ~3× lock overhead reduction on hot paths (`NamedKey`, `SurroundingTextChanged`, `Character|Literal|Space`, `SwipeWord`, `StenoCommit`).

### `apply_shift_inline(word, keypad)` pure function

Replaced `apply_shift_to_swipe(&self, word)` which acquired its own lock. The new free function takes `KeypadState` by value (Copy), no locking. Called inside an already-held write-lock.

### Recognizer position cache

`LAST_RECOGNIZER_REFRESH_GEN` atomic + `HITBOX_GENERATION` invalidation. Skip rebuilding ~30 hashmap entries per swipe when nothing changed.

### Hitbox cache

`LAST_HITBOX_REFRESH_GEN` atomic. Same pattern — only recompute on widget tree change.

### Trail rendering optimizations

- Bands per layer: 4 → 3
- Cap-trim amortized 32× (`TRAIL_CAP=256`, `TRAIL_TRIM_SLACK=32`)
- Denom hoisted outside band loop
- Distance-filter (4 px min between samples) before push

### Dimension-notify gating

GTK fires `notify::default_width/height` on every layout pass. Gate with `Rc<Cell<i32>>` so only actual changes trigger `invalidate_hitbox_cache`.

### `KeyWidget::update_for_state` uses direct add/remove

Replaced `set_css_classes(Vec<&str>)` which allocates a Vec on every shift state change. Direct `add_css_class` / `remove_css_class` doesn't allocate.

### `apply_press_pulse` via CSS keyframe

Replaced `adw::SpringAnimation` (Rust callback per frame) with a CSS class swap that triggers a 220 ms keyframe. ~13 callbacks eliminated per key press.

### Squared-distance comparisons

Throughout the gesture code: when comparing distances for ordering, square both sides (`dx*dx + dy*dy < threshold * threshold`) to avoid sqrt. Used in `tap_eviction`, `pivots_to_word`, `swipe.update`'s min-distance check.

### Cardinal classify dedup

`classify` previously computed `max_excursion_from_start` (for Tap check) AND `max_excursion` (for FLICK/Hairpin checks) — identical iterations. Now computed once at the top.

### Cardinal hairpin single-call

`max_excursion_point(path, start)` was called twice (once for `.0`, once for `.1`). Now `(mx, my) = max_excursion_point(...)` destructure.

### What to NOT optimize prematurely in the Qt port

- The dictionary lookup is already fast. Don't index it differently unless profiling says so.
- The smoothing operation is O(N) over the path — N is < 100 per swipe. Don't vectorize it.
- The CSS reinstall on theme change is genuinely slow on a phone (~50 ms) but only fires on settings change. Qt's QSS reload is faster but not a hot path.

---

## 12. Test strategy

### Rust unit tests (542 passing at handoff)

`cargo test --no-default-features` runs the entire suite minus voice/llm features. Tests are co-located with source (`#[cfg(test)] mod tests { ... }`). Heavy use of synthetic gesture paths and KeyHitbox fixtures.

Key test modules:
- `src/gestures/cardinal.rs::tests` — classify tests for all branches + smoothing diagnostics
- `src/gestures/swipe.rs::tests` — extract_pivots, decode_swipe_path, SwipeTracker
- `src/gestures/recognizer.rs::tests` — end-to-end decode (Dvorak "the" swipe)
- `src/keyboard/engine.rs::tests` — state machine, event emission, auto-cap, swipe-auto-space
- `src/ui/keyboard_behaviors_tests.rs` — integration tests (drag begin / drag end branches, off-key fallbacks, cardinal nearest-cell)
- `src/layouts/mod.rs::tests` — layout invariants (grid_columns row sums, key counts)

### Python sim (13 scenarios passing)

`python3 -m sim.run` in `sim/` directory. Pure-Python port of the drag_end decision tree. Scenarios cover cardinal normal/steno/dead-zone/minimize/horizontal/abort cases. NOT a port of the recognizer (Rust-only). Useful for testing dispatch logic without GTK.

### What to add in the Qt port

- **Coordinate-frame integration test** — set up a fake KB hierarchy with known offsets, drive a synthetic swipe, verify decode returns expected letters. This is what the current Rust suite is MISSING (current tests bypass GTK; that's why the coordinate bug isn't caught).
- **Qt Quick Test scenarios** — visual regression for layout rendering (key positions, sizes, CSS class application).
- **Wayland mock for input-method-v1** — fake compositor that delivers activate/deactivate/surrounding_text events; verify RustyBoard's response. KWayland has fixtures for this.

---

## 13. Build & deployment

### Current build

```
cargo build --release [--features voice llm dbus llama-ffi]
```

Default features include voice + dbus. Optional: llm (uses llama.cpp via FFI), llama-ffi (direct integration, gated behind feature).

Binary lands at `target/release/rusty-board`. Strip + install:
```
install -m755 target/release/rusty-board /usr/local/bin/
```

### Cross-compile for Fairphone 5 (ARMv8)

```
rustup target add aarch64-unknown-linux-musl    # Alpine uses musl
cargo build --release --target aarch64-unknown-linux-musl
```

Needs cross-compile-friendly C deps. Currently using vendored whisper.cpp / llama.cpp; both build cleanly cross-target.

### Plasma Mobile install

Use `scripts/install.sh --system` to install the binary + `.desktop` file + (optionally) Whisper model. Then edit `~/.config/kwinrc`:
```ini
[Wayland]
InputMethod=/usr/share/applications/rusty-board.desktop
```
Logout/login.

### Qt port build

Probably `cmake + ninja` (KDE convention) rather than Cargo. Mixed-language build is well-supported via the `corrosion-rs` cmake integration:
- CMake builds the Qt frontend + QML
- Corrosion compiles the Rust workspace as a static library, exposes C ABI to the Qt frontend
- Final binary links both

Alternative: keep `cargo build` as the driver, use `cxx-qt` or `qmetaobject-rs` for the Rust↔Qt binding. This is cleaner for a Rust-majority codebase but the Qt UI parts will be in Rust-with-macros rather than raw QML — tradeoff.

Recommendation: **cxx-qt** for the binding layer. It's the most active Rust-Qt integration in 2026 and was designed by KDAB specifically for cases like this (Rust core, Qt UI). Use Cargo as the build driver, with QML files copied into the build via a build.rs step.

---

## 14. Voice + LLM (optional features)

### Voice input

`src/voice/` runs whisper.cpp via `whisper-rs`. Model search order (see `src/voice/model_path.rs`):
1. `$WHISPER_MODELS_DIR` env var
2. `~/models/` (unified directory for the user)
3. `~/.local/share/rusty-board/models/`
4. `~/.config/web2qml/models/` (legacy Maliit dir)
5. `~/.local/share/whisper/models/`
6. `/usr/share/rusty-board/models/`
7. `/usr/share/whisper/models/`

Filename preference per directory: quantized `.gguf` (e.g., `ggml-base.en-q5_1.gguf`) → quantized `.bin` → full-precision `.gguf` → full-precision `.bin`.

The voice service runs in its own thread, writes audio chunks to a buffer, transcribes on a separate thread, sends `EngineInput::TextCommitted` to the engine.

For Qt: voice service stays in Rust unchanged. The 🎤 button in the UI calls a Rust function `voice::start_recording()` → service does its thing → engine emits TextCommitted → Qt UI sees the resulting text update.

### LLM swipe disambiguation

`src/gestures/llm_swipe.rs` (current state: skeleton, gated behind `llm` feature). Will pass the top N swipe candidates to a local llama.cpp model with prompt "given this context, which word is most likely?" The model reranks. Useful for ambiguous swipes (e.g., "the" vs "tho" vs "tge").

Not yet shipped to production. Documented in `docs/llm_swipe_design.md`. The architecture is ready for it; just needs the `llama-ffi` feature integration and a model in `~/models/`.

---

## 15. File-by-file porting plan

### Keep verbatim (Rust)

```
src/keyboard/                                       — all engine + state + key + steno + code_mode + character_draw
src/gestures/                                       — all gesture pipeline modules
src/layouts/mod.rs                                  — all layout definitions
src/voice/                                          — voice service (if keeping voice)
src/config/mod.rs                                   — TOML config
src/keyboard/dispatch_mode.rs                       — DispatchMode classification
src/ipc/dbus.rs                                     — D-Bus (if keeping)
resources/dictionary_en.txt                          — bundled dictionary
resources/styles/keyboard.css → port to QSS or QML  — visual styling (NOT structural)
```

### Replace (UI layer)

```
src/ui/keyboard_view.rs    (5400 lines)  → QML root + Rust bridge via cxx-qt
src/ui/key_widget.rs                     → QML Key.qml component
src/ui/animations.rs                     → QML Behavior + PropertyAnimation
src/ui/theme.rs                          → Qt style sheet or QML Material theme
src/ui/swipe_trail.rs                    → QML Canvas or QQuickPaintedItem
src/ui/settings_panel.rs                 → QML SettingsView
src/ui/microterm.rs                      → QML MicroTerm view
src/ui/window.rs                         → cxx-qt registered QObject for window lifecycle
```

### Replace (compositor)

```
src/compositor/placement.rs              → LayerShellQt::Window
src/compositor/wayland_im_v1.rs          → KWayland::Client::TextInputManager
src/main.rs                              → QGuiApplication, simpler init (no LD_PRELOAD dance)
```

### Recommend rewrite-from-scratch (not direct port)

- Settings panel layout — Qt has much better dropdown / slider / color-picker primitives. Port the schema, redesign the UI.
- The trail widget — Qt's `QQuickPaintedItem` or shader approach is cleaner than the GTK cairo path.

### Order of operations (suggested)

1. Set up cxx-qt + Cargo workspace + minimal QML app that shows a single keyboard layout (qwerty, no gestures)
2. Wire `KeyTap` from QML to engine; verify TextCommitted reaches focused app via virtual_keyboard_v1
3. Add layer-shell anchoring + KWin input-method-v1 registration
4. Add layout switching (symbols/numbers/dvorak)
5. Add gestures — swipe-typing first (most-used), then cardinal, then steno
6. **Spend time on the coordinate-frame design.** Get this right at the architecture level, not as defensive fixes later.
7. Add settings panel
8. Add voice
9. Add the trail rendering
10. Polish: animations, theming, single-hand mode

---

## 16. What to ask the user during the rewrite

These are the things only the user can decide:

- **Keep D-Bus event broadcast?** Currently fires every `TextCommitted` on a bus. Used by external monitoring tools. Cheap to keep; may not be needed.
- **Keep LLM swipe?** Currently a feature flag, not user-visible. Either ship it or delete the skeleton.
- **Keep character-drawing?** `src/keyboard/character_draw.rs` is the long-press-spacebar handwriting canvas. Used? Not used? User feedback unclear.
- **Cardinal_grid_words language toggle: keep, expand, or simplify?** Currently cycles 4 modes (Prose/C/Rust/Python). Could add more (JS, Go, shell, etc.) but UI cost grows.
- **Stenotype mode: keep?** Niche but loved by stenographers. ~1000 lines of code. If user doesn't use it, can drop.

---

## 17. Conversation summary (for context)

The last 8 hours of work (across multiple sessions) focused on:

1. **Cardinal swipe threshold gap** — discovered via diagnostic test that a 16 px raw N-flick smooths down to ~13.8 px stored excursion, which fell in the gap between `TAP_MAX_EXCURSION_PX=13` and `FLICK_MIN_EXCURSION_PX=15`, returning `Unrecognized` → user perceived "cardinal swipes don't work." Fix shipped: `FLICK_MIN_EXCURSION_PX = 13` (matching TAP_MAX). Confirmed by regression-guard tests.

2. **Alphabet swipe "line shows but no write"** — investigation. Recognizer pipeline unit-tested end-to-end with realistic Dvorak hitboxes; works correctly in isolation. Bug must be in the integration between Rust pipeline and on-device behavior. Added diagnostic logging at every stage; not yet pulled from device.

3. **Coordinate-frame hypothesis** — user suggested "could the coordinate system be throwing off the swipes too?" — yes, plausible. `gtk4::Widget::compute_bounds` can transiently return None on Plasma Mobile during layer-surface "mapped-but-not-fully-sized" windows, causing offset = (0,0) and frame mismatch with hitboxes. Defensive fix shipped: `recover_frame` helper at drag_end recomputes offset and corrects path drift. Confirmation pending on-device test.

4. **Arcade buffer keys on Dvorak** — added thin (0.33-wide) `⌫` (left flank) and `⏎` (right flank) keys around the spacebar. Spacebar narrowed from 3.0 → 2.34. New `extra_css_class` field on Key for layout-author-supplied classes. `arcade-buffer` CSS class with distinctive accent styling. User likes the placement.

5. **Bug-hunt findings (open, unshipped)**:
   - Inconsistent hit-test in `dispatch_cardinal_swipe` (strict vs contains)
   - `enter_tray_inline` doesn't clear `SWIPE_GESTURE_ACTIVE`
   - drag-drawing-end doesn't bump `TAP_GENERATION`
   - `is_significant_swipe` requires `points.len() >= 3` (drops 2-point flicks)

6. **Perf optimizations** — consolidated `max_excursion` computation in `classify` (one iteration instead of two), single `max_excursion_point` call in hairpin branch (was being called twice), etc. Cumulative perf state documented in §11.

The two open user-reported bugs (cardinal swipes, alphabet swipe-typing) are very likely the same root cause: coordinate-frame mismatch on Plasma Mobile under load. If a Qt rewrite gets the coordinate handling right at the architecture level (gesture controller on the keyboard surface itself, not on a parent overlay), both bugs likely disappear.

---

## 18. Quick reference

### Build
```bash
cargo build --release --no-default-features
```

### Test
```bash
cargo test --no-default-features --lib       # 542 tests
python3 -m sim.run                            # 13 scenarios
```

### Run with verbose logging
```bash
RUST_LOG=info,rusty_board=debug ./target/release/rusty-board
```

### Force keyboard visible (laptop testing)
```toml
# ~/.config/rusty-board/config.toml
[behavior]
auto_hide_on_no_focus = false
```

### View on-device logs
```bash
tail -f $XDG_RUNTIME_DIR/rusty-board.log
# or
tail -f /tmp/rusty-board.log
```

### Critical constants (don't change without testing)
- `TAP_MAX_EXCURSION_PX = 13.0`
- `FLICK_MIN_EXCURSION_PX = 13.0` (matched to TAP_MAX intentionally)
- `TAP_EVICTION_PX = 12.0`
- `Smoothing factor = 0.3`
- `NEAREST_CARDINAL_RECT_PX = 10.0`
- `swipe_threshold = 20.0`
- `Trail bands = 3`, `TRAIL_CAP = 256`, `TRAIL_TRIM_SLACK = 32`
- `min_sample_distance = 4 px` (in SwipeTracker)
- `grid_columns = 10.0` (layout invariant for row sums)

---

## Closing note

The Rust core is solid. The bugs we've fought have all been at the GTK4/KWin boundary — not in our state machine, gesture math, or layout logic. A Qt rewrite that gets the Wayland integration and coordinate handling right at the architecture level should let the existing core shine through.

Read §6 (coordinate system) and §7 (Wayland) twice. Most of the time will be saved or lost there.

— Handoff prepared 2026-06-22, current commit state: 542 Rust tests passing, 13 sim scenarios passing, latest shipped zip at `/mnt/user-data/outputs/rusty-board.zip`.
