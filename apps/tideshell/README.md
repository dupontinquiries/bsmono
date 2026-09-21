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

`shell.cursor = normal | fat`: with **fat**, while the trackpad drives the
pointer the shell draws an iPadOS-style translucent disc that eases after
the finger (exponential ease-out), shrinks on press and fades when idle;
the disc's position is sent to the compositor as an EI *absolute* pointer
position, so clicks land on it. cosmic-comp's own cursor is hidden
meanwhile through its live `cursor_hide_timeout` config (no patch); your
own value is saved in `~/.config/tideshell/cursor_hide_timeout.saved` and
restored on exit, or at the next start if tideshell died. Limits: only in
trackpad mode (a hardware mouse moves a pointer the shell cannot see);
cosmic-comp shows its cursor during a held-button drag; no magnetic
snapping onto other apps' buttons (no protocol exposes them).

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

The trackpad's output is also verified live: with cosmic-comp patch
`0002-dbus-ei-allow-tideshell`, the shell owns `org.tideshell.Shell`, gets an
EI socket (a caller without an allowlisted name is refused: "Access
denied"), and `--ei-selftest -450 0` moved cosmic-comp's pointer 450 px and
clicked: gtk4-widget-factory's `WAYLAND_DEBUG` log shows the motion and a
BTN_LEFT press/release, and the click activated that window. Scroll goes
the same path but was not exercised live.

Wired, not yet exercised live: hatch-core launches (recents shared with
hatch), `hatch-egui --toggle`, rusty-board `Show`/`Hide` via `busctl`,
prefs file (`~/.config/tideshell/prefs`, the `shell.*` keys settingsd
would carry).

Not wired, and reported as `unwired:` in the trace rather than silently
ignored: quick-toggle backends, brightness/volume, the settingsd transport, and the
layer-shell edge strips and surfaces that replace the simulator window on
a phone.

### Running against a nested cosmic-comp

```sh
# once: build cosmic-comp (pop-os/cosmic-epoch, epoch-1.8.0), ~6 min on one core
export DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 XDG_RUNTIME_DIR=/tmp/xdg
Xvfb :99 -screen 0 1000x1000x24 & cosmic-comp &   # falls back to winit/EGL
WAYLAND_DISPLAY=wayland-1 DISPLAY= gtk4-demo &    # some real windows
WAYLAND_DISPLAY=wayland-1 DISPLAY= tideshell-sim --wayland --trace
# trackpad/EI also needs a session bus shared by both:
#   dbus-daemon --session --address=unix:path=$XDG_RUNTIME_DIR/bus --fork
#   export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus   (before cosmic-comp)
```
