# TODO: menu + navigation parity with voiceterm-qt

Reported 2026-09-08, after using both builds: *"menus and navigation patterns
still not quite what they were usability-wise in voiceterm-qt"*.

NOT YET DIAGNOSED. This file exists so the report is not lost, and so the next
person starts from what was actually said rather than from a guess about it.

## Why this needs the two side by side

The report is comparative — "not what they were" — and I have no way to run the
Qt build here (its build script shells out to `qmake`, see `manpage_view`'s
header for the same wall). A list of things I *think* differ would be a list of
my own taste, and the useful version of this is the specific moment where the
hand goes to the wrong place.

## What to capture, on the device, with both installed

For each, note what the Qt build does and what the egui one does:

- [ ] Opening a menu: what opens it, and does the same gesture close it?
- [ ] Escape / back: does it close the menu, the surface under it, or both?
- [ ] Where focus lands after a menu closes — the control that opened it, or
      somewhere else?
- [ ] Tab and pane switching: the keystroke, the gesture, and the order the
      surfaces cycle in.
- [ ] Whether a menu can be dismissed by tapping outside it, and whether that
      tap also activates whatever is under it (the classic phone regression).
- [ ] The keycap row and the gesture menus: same actions, same positions?

## Known differences already in the code, for the same walk

These are deliberate divergences, listed so they are not re-reported as bugs:

- `gesture_menus` routes every tab move through `WsCall::Command`, so the
  palette, a swipe and the voice grammar reach tabs by one path. The Qt build
  had separate routes.
- The egui build's surfaces are a stack (`Surface`), so "close" always means the
  topmost one. The Qt build let some surfaces sit beside each other.

## Related, and possibly the actual cause

`uikit_egui::menu::menu_ui` gained an entrance animation (v0.1.7xx). If the
report predates that, it is not the cause; if it postdates it, check whether the
6pt fall makes the first tap land before the menu settles.
