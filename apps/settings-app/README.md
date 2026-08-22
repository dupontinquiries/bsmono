# bigsilicon-settings (settings-app)

The central settings app for the egui-kit apps. A shared `SettingsEditor`
(egui UI on `uikit-egui`) plus two front-ends onto `settingsd`: the
`bsettings` GUI and the `bsettings-cli` command line — both read the
current `Prefs` from the daemon and publish changes to the `"settings"`
namespace so every app updates live. Uses `settings-kit` for the
schema/diff and `bigsilicon-client` for the daemon link.

## Verification (2026-08-19)

Had no build-status record anywhere. A session with a real rustc 1.96
(clears egui 0.34's MSRV) ran it for the first time: `cargo test` is
7/7 — diffing reports only changed keys (display changes, swipe-interaction
changes), an editor tracks pending edits until published, no change means
no publish, the palette list is available, and voice-page edits publish
correctly as a diff. `cargo check --features gui --bin bsettings` (the
eframe GUI binary, not exercised by the default `cargo test`) also compiles
clean. Only pre-existing `egui::SelectableLabel`/`Context::screen_rect`
API-rename deprecation warnings in `uikit-egui`, not touched.
