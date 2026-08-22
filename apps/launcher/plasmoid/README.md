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

**Not run in the authoring sandbox** (no Plasma host, no display — same
policy as qt-ui-kit) — nothing here has actually opened as a running
dashboard.

What *has* been checked, mechanically rather than by eye: Qt 6.4.2's
`qmllint` (installed via `apt` from Ubuntu 24.04's own repos —
`qt6-declarative-dev-tools`, since the sandbox has network access to
`archive.ubuntu.com`) parsed all six `.qml` files here with **zero syntax
errors**. Every warning it does report is import-resolution noise: KDE
Frameworks' QML modules (`org.kde.plasma.*`, `org.kde.kirigami`,
`org.kde.plasma.private.kicker`) aren't type-checkable from a generic Ubuntu
box, because Plasma 6's actual runtime (`libplasma`) isn't packaged there —
Ubuntu 24.04 only ships the older KF5/Plasma 5 `plasma-framework`
(confirmed via `apt-cache policy`), which targets a different, incompatible
QML API surface (versioned imports like `org.kde.plasma.core 2.0`, not the
unversioned `org.kde.plasma.core` this file uses) and would give
misleading signal either way if force-installed alongside Plasma 6 code.
So: structurally and syntactically sound, confirmed by a real parser: yes.
Semantically correct against actual Plasma 6 types (SideBarItem gets the
right props, ItemGridView's model shape matches what the delegate expects,
etc.): still unverified, and can only be closed out with `qmllint` run on
an actual Plasma 6 host, or `plasmoidviewer` for a real render.

Known-untested spots beyond that: DragHandler-vs-GridView grab arbitration
for swipe-down-to-close, and maliit's keyboardRectangle reporting under
scale factors ≠ 1.
