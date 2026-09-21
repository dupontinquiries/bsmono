# tideshell

A touch shell for cosmic-comp on phones (Fairphone 5 first): a headless
engine and egui surfaces over it.

```
crates/tideshell-core/   std-only engine: edge gestures, drawer, right bar,
                         switcher, hold chooser, carousel, trackpad, prefs
egui-frontend/           surfaces + executors + tideshell-sim
```

## Gestures

| where | gesture | result |
|---|---|---|
| top edge | pull down | control center (first pull: one row; further: full + sliders) |
| bottom edge | short swipe up / flick | multitask switcher (MRU grid) |
| bottom edge | long swipe up (past 55%) | hatch app grid (`hatch-egui --toggle`) |
| bottom edge | swipe sideways | previous / more recent app (carousel: neighbour) |
| bottom edge | press and hold, slide, lift | toggle Carousel / Keyboard / Trackpad |
| right edge | swipe in (right-bar layout) | expand / collapse the bar |

`shell.layout = pull-down | right-bar` chooses between the drawer and a
right-hand bar of mixable widgets (`shell.bar.widgets`). Both are editable
in the settings sheet (drawer bottom, or the bar's `settings` widget).

## Simulator

```sh
cd egui-frontend
cargo run --release -- --trace                 # mouse = one finger, F1 = trace
cargo run --release -- --layout right-bar
cargo run --release -- --demo hold             # drawer|switcher|swipe|hold|bar|trackpad|settings
cargo run --release -- --live                  # also run real hatch / rusty-board effects
```

## Wired vs not

Wired and **verified against a real cosmic-comp epoch-1.8.0** (nested under
Xvfb, see `shared/systemcompkit/STATUS.md`): the live window list and
window actions. `tideshell-sim --wayland` shows the compositor's real
windows in the switcher; a click on a card raised that window
(activate), and close / maximize / unmaximize are each confirmed with
`systemcompkit`'s `toplevel_probe`. cosmic-comp advertises no fullscreen
capability, which is why carousel mode maximizes.

Wired, not yet exercised live: hatch-core launches (recents shared with
hatch), `hatch-egui --toggle`, rusty-board `Show`/`Hide` via `busctl`,
prefs file (`~/.config/tideshell/prefs`, the `shell.*` keys settingsd
would carry).

Not wired, and reported as `unwired:` in the trace rather than silently
ignored: the trackpad's EI pointer (needs cosmic-comp patch
`0002-dbus-ei-allow-tideshell` plus a reis client, as cosmic-osk does),
quick-toggle backends, brightness/volume, the settingsd transport, and the
layer-shell edge strips and surfaces that replace the simulator window on
a phone.

### Running against a nested cosmic-comp

```sh
# once: build cosmic-comp (pop-os/cosmic-epoch, epoch-1.8.0), ~6 min on one core
export DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 XDG_RUNTIME_DIR=/tmp/xdg
Xvfb :99 -screen 0 1000x1000x24 & cosmic-comp &   # falls back to winit/EGL
WAYLAND_DISPLAY=wayland-1 DISPLAY= gtk4-demo &    # some real windows
WAYLAND_DISPLAY=wayland-1 DISPLAY= tideshell-sim --wayland --trace
```
