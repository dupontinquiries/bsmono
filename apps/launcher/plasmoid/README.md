# launcher/plasmoid — the Qt/QML surface (archived-maintained tier)

Fullscreen touch tile launcher for Plasma 6 on the FP5: a kickerdash fork with
animated tile geometry (slide-in + restack), a maliit-aware bottom search field,
and the touch-input fixes below. Per `UI_STRATEGY.md` this is the Qt tier;
`../egui-frontend` is the primary port.

## What changed for touch (the "taps close the window" class)

- `main.qml` — the three stacked full-screen MultiPointTouchAreas are removed.
  They sat over the plasmoid root, stole presses from everything, and one
  referenced a nonexistent `gridView` (ReferenceError on first swipe). Dead
  close/fade machinery went with them.
- `DashboardRepresentation.qml` — tap-to-close and swipe-down-to-close are
  pointer *handlers* on the background (TapHandler / DragHandler), which only
  receive events children didn't claim and which distinguish tap from slipped
  drag. Keyboard height is a de-duped root property (the old inner-Column copy
  was unreachable from the grid scope). Search field focus/visuals have one
  owner each.
- `ItemGridView.qml` (from the improved artifact) — release on empty space no
  longer closes the launcher; collaborators (`dragHelper`, `kickerInstance`,
  `rootItem`) are injected properties, not globals.
- `ItemGridDelegate.qml` — added the missing `org.kde.plasma.plasmoid` import
  (it reads `Plasmoid.configuration`).

## Files that carry over unchanged

These are unchanged from the working package and are intentionally not
duplicated here; copy them in from the existing install (or upstream
kickerdash) when assembling the package:

```
contents/ui/ActionMenu.qml          contents/ui/ItemListDialog.qml
contents/ui/CompactRepresentation.qml  contents/ui/ItemListView.qml
contents/ui/ConfigGeneral.qml       contents/ui/ItemMultiGridView.qml
contents/ui/ItemListDelegate.qml    contents/ui/SideBarSection.qml
contents/code/tools.js              (upstream kicker, GPL — do not retype)
```

## Install / iterate on device

```
kpackagetool6 --type Plasma/Applet --install .      # or --upgrade
plasmoidviewer --applet .                            # windowed dev loop
qmllint contents/ui/*.qml                            # static check
```

## Verification status

**Not compiled or run in the authoring sandbox** (no Qt6/Plasma host — same
policy as qt-ui-kit). The QML here has been reviewed, not executed; `qmllint`
on device is the first gate. Known-untested spots: DragHandler-vs-GridView
grab arbitration for swipe-down-to-close, and maliit's keyboardRectangle
reporting under scale factors ≠ 1.
