# gtk-ui-kit — the archived, maintained GTK4/libadwaita UI kit

Home for the reusable GTK4 + libadwaita surface of the fleet. **Archived, not retired:** egui is the
go-forward UI (see `../../UI_STRATEGY.md`), but the GTK surface stays here as a first-class UI library kept
for feature parity and reference.

## What belongs here
- Shared GTK4/libadwaita widgets, styles (CSS), and composite templates used across the GTK front-ends
  (ambient-learner, bot-arena, logscope, rusty-board).

## What does NOT belong here
- App-specific application/window logic — stays in the app; the app pulls widgets/resources from the kit.

## Building
Requires GTK4 + libadwaita dev libraries (gdk-pixbuf, glib) — verified on a GTK host, **not** in the
headless CI sandbox.

## Migration status
Empty foundation. Per-app lift proceeds on a GTK host, one app at a time, keeping each app building.
