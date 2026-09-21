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

Wired: hatch-core launches (recents shared with hatch), `hatch-egui
--toggle`, rusty-board `Show`/`Hide` via `busctl`, prefs file
(`~/.config/tideshell/prefs`, same `shell.*` keys settingsd would carry).

Not wired, and reported as `unwired:` in the trace rather than silently
ignored: window activate/close/maximize (needs `zcosmic_toplevel_manager_v1`
requests in systemcompkit), the trackpad's EI pointer (needs cosmic-comp
patch `0002-dbus-ei-allow-tideshell` plus a reis client, as cosmic-osk
does), quick-toggle backends, brightness/volume, settingsd transport, and
the layer-shell edge strips that feed real touches on a phone. Nothing here
has run on a compositor yet; the simulator is the verification ceiling.
