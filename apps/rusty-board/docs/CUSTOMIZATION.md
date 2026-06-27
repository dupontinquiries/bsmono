# Customization

Everything user-tweakable lives in `~/.config/rusty-board/config.toml`. This doc walks through the practical knobs.

## Theme

```toml
[appearance]
theme = "auto"           # "auto" | "light" | "dark"
accent_color = "3584e4"  # any 6-char hex, no leading "#"
opacity = 0.95           # 0.0 (invisible) to 1.0 (opaque)
key_radius = 8.0         # pixels
font_family = "Cantarell"
font_size_ratio = 0.42   # of key height
```

Picking an accent: it's used for the active-shift highlight, the action-key fill (return, etc.), and the suggestion press state. Pick something readable against both light and dark backgrounds — saturated mids work well, pastels don't.

## Animations

```toml
[appearance]
animation_ms = 150
```

This is the default for timed animations (key press flash). The spring animations (shift toggle, etc.) use a fixed damping/stiffness that *feels* right and isn't currently exposed. If you want to tune them, edit `src/ui/animations.rs` and recompile.

## Behavior

```toml
[behavior]
auto_capitalize = true      # caps after . ! ? and at sentence start
auto_correct = false        # not yet implemented
word_prediction = true      # suggestions bar
haptic_feedback = true      # vibrate on press (Wayland only, where supported)
sound_feedback = false      # plays a click — most users hate this on
long_press_ms = 350         # how long until the popover appears
double_tap_ms = 300         # caps lock if you double-tap shift within this
repeat_delay_ms = 400       # initial backspace repeat delay
repeat_interval_ms = 50     # backspace repeat rate after delay
auto_hide_on_no_focus = true  # hide when no editable widget has focus (Wayland with input-method-v2 only)
```

`long_press_ms` is the most-tweaked value: 350ms is comfortable for most people; gamers and fast typists often want 250–300; people with motor difficulties go up to 500–600.

`auto_hide_on_no_focus` only does anything on Wayland compositors that support `zwp_input_method_v2` (Sway, river, Wayfire, Phosh, KWin 6+). On X11 the keyboard stays visible whenever the process is running. Set to `false` if you want the keyboard always-visible even on a supporting Wayland compositor.

## Voice

```toml
[voice]
enabled = true
model_path = "/home/you/.local/share/rusty-board/models/ggml-base.en.bin"
language = "auto"          # "auto" | "en" | "fr" | "de" | ...
sample_rate = 16000
streaming = true           # show partial transcripts as you speak
min_audio_ms = 300         # ignore taps shorter than this
threads = 4                # whisper inference threads
```

**Model trade-offs:**

| Model     | Size    | RAM   | Speed | Accuracy |
|-----------|---------|-------|-------|----------|
| tiny.en   | 75 MB   | ~390M | fast  | ok       |
| base.en   | 150 MB  | ~500M | fast  | good     |
| small.en  | 480 MB  | ~1G   | mid   | great    |
| medium.en | 1.5 GB  | ~2.5G | slow  | excellent |

For a phone-sized device, `tiny.en` or `base.en` is what you want. `small.en` is the sweet spot on a laptop.

The `threads` setting matters: too many threads on a small CPU thrashes; too few wastes capacity. On a 4-core phone, set `threads = 2` and leave one core for the keyboard UI.

## Gestures

```toml
[gestures]
swipe_typing = true
spacebar_cursor = true       # drag on space to move the cursor
shift_swipe = true           # drag on shift to select words
swipe_threshold = 20.0       # pixels — below this counts as a tap
gesture_sample_rate = 60     # Hz; usually matches refresh rate
smoothing = 0.3              # 0.0 = none, 1.0 = max — finger tremor reduction
```

If swipes are picking up extra letters near the trajectory, increase `smoothing`. If swipes feel laggy or imprecise, decrease it.

`swipe_threshold` separates "I tapped a key" from "I started a swipe". On a small screen, 15px is fine; on a tablet, 25–30px.

## Layout

```toml
[layout]
active = "en_us_qwerty"
available = ["en_us_qwerty", "en_us_dvorak", "en_us_colemak", "fr_azerty", "de_qwertz"]
number_row = false           # show a top row of digits
emoji_button = true
voice_button = true
```

If you turn off `voice_button`, you can still trigger voice via D-Bus or a keyboard shortcut from your compositor.

## Adding a new layout

Layouts are JSON files in `~/.local/share/rusty-board/layouts/` (or wherever you installed the share dir). Drop a new file in there and reference it from `[layout].active`:

```json
{
  "name": "my_layout",
  "display_name": "My Custom Layout",
  "language": "en",
  "direction": "ltr",
  "rows": [
    {
      "keys": [
        { "label": "a", "shifted": "A" },
        { "label": "b", "shifted": "B" }
      ]
    }
  ]
}
```

`width` defaults to 1.0 (a normal letter key). Modifier keys are usually 1.5; the spacebar is 5.0.

`extended` is a list of strings shown in the long-press popover. `extended_shifted` is the same list when shift is active.

`swipe_target: false` makes the key invisible to swipe decoding. Set this on shift, backspace, return, etc. — you don't want a swipe across "shift" to insert a letter.

## CSS overrides

The full CSS is at `~/.local/share/rusty-board/styles/keyboard.css`. Edit it to change anything that's not exposed in the config. To do per-user overrides without losing them on reinstall, drop a file at `~/.config/rusty-board/custom.css`:

```css
.key-button {
    font-weight: 700;
}

.keyboard-view {
    background: linear-gradient(to bottom, #2a2a2a, #1a1a1a);
}
```

(Loading user CSS isn't wired up yet; PRs welcome.)
