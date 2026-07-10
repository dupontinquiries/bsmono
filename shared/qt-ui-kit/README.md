# qt-ui-kit — the archived, maintained Qt/QML UI kit

Home for the reusable Qt/QML surface of the fleet. **Archived, not retired:** the egui front-ends are the
go-forward UI (see `../../UI_STRATEGY.md`), but the Qt/QML surface stays here because it's a powerful UI
library in its own right and still leads egui on some surfaces (feature parity).

## What belongs here
- Shared QML components and styles used across the Qt front-ends (gitmind, voiceterm, bo-suite/bo-controller,
  ambient-learner) — buttons, panels, gesture hosts, the theme constants, transition/animation idioms.
- The qmetaobject glue that's genuinely shared (not app-specific state).

## What does NOT belong here
- App-specific view models / backend wiring — those stay in the app; the app re-points its QML imports here.

## Building
Requires Qt (qmetaobject/qttypes) — verified on a Qt-capable host, **not** in the headless CI sandbox.

## Migration status
Empty foundation. Per-app lift (gitmind/crates/gui → here, voiceterm/qml → here, bo-controller/qml → here,
ambient-learner qml → here) proceeds on a Qt host, one app at a time, keeping each app building.
