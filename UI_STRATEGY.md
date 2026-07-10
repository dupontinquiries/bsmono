# UI strategy — egui primary, Qt/GTK archived as kits

The egui front-ends are the go-forward UI for every app. The Qt/QML and GTK front-ends are **archived,
not retired**: they stay in the tree, keep building, and are factored into their own kits, because
(a) they're powerful UI libraries in their own right and (b) egui hasn't reached feature parity with them
on every surface yet. Nothing here deletes a working front-end.

## The three tiers

- **egui (primary).** Every app iterates here first. Shared widgets live in `shared/uikit/crates/uikit-egui`
  (motion, palette, dialogs, pickers, log/tile views). Colours/animation/behaviour track the Qt apps as the
  north star (see the `Night` palette matching gitmind's Qt surface).
- **qt-ui-kit (archived, maintained).** The reusable Qt/QML surface, kept for parity and reference.
- **gtk-ui-kit (archived, maintained).** The reusable GTK4/libadwaita surface, same footing.

## Frontend inventory (as of this pass)

| app             | egui (primary)                    | Qt/QML (→ qt-ui-kit)        | GTK (→ gtk-ui-kit)      |
|-----------------|-----------------------------------|-----------------------------|-------------------------|
| gitmind         | crates/gui-egui                   | crates/gui (15 qml)         | —                       |
| voiceterm       | egui-frontend                     | qml (18 qml)                | —                       |
| bo-suite        | egui-frontend                     | bo-controller/qml (46 qml)  | —                       |
| ambient-learner | src/ui/egui (feature `egui`)      | 14 qml                      | gtk4 + libadwaita       |
| bot-arena       | egui-frontend                     | —                           | gtk4                    |
| rusty-board     | egui-frontend                     | —                           | gtk4                    |
| logscope        | crates/logscope-egui              | —                           | crates/logscope (gtk)   |
| snapclone       | egui-frontend                     | —                           | —                       |
| av1tuner(-m1)   | av1tuner_app                      | —                           | —                       |
| settings-app    | SettingsEditor (`bsettings`)      | —                           | —                       |

## Why kits, not deletion

A Qt/QML surface and a GTK surface are each a coherent, reusable UI library: theming, component set, and
interaction model. Collapsing each into a kit (rather than leaving per-app copies) means one place to
maintain the reference implementation, and a clean home to pull from when an egui surface needs to reach
parity with something the mature front-end already does well.

## Migration steps (per app, on a Qt/GTK-capable build host)

1. **egui is default.** Ensure the app's default/primary build path is the egui front-end. (Config only —
   e.g. gitmind already keeps both GUIs out of `default-members`, so the CLI stack builds without a
   toolchain; the GUI is opt-in and should default to `gui-egui`.)
2. **Lift the reusable surface.** Move the shared Qt/QML components into `shared/qt-ui-kit` (and GTK into
   `shared/gtk-ui-kit`); leave app-specific glue in the app, re-pointing its QML import / GTK resource
   paths at the kit.
3. **Keep it building.** The kit and the app must still compile and run on a Qt/GTK host — these steps are
   verified there, not in the headless CI sandbox (which has neither Qt nor GTK).
4. **Parity ledger.** Record, per surface, what the Qt/GTK front-end still does that the egui one doesn't,
   so egui iterations have a concrete target.

## Build/orchestration notes

- App→path resolution is discovered from the filesystem (apps/ vs games/), not hard-coded — a new
  app/game needs no store change.
- Apps needing a system toolchain belong in the hardware stage: **unendlich** (Vulkan + `glslc` shaders),
  **ambient-learner** (GTK4/libadwaita), the snapclone/photosync C++ stack, and opendht. The pure-cargo
  pass should either skip these or build them with a headless feature.
