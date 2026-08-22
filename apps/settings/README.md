# settings (BigSilicon)

The central BigSilicon settings app: an egui UI on `uikit-egui` that edits
the shared `Prefs` (gesture feel, touch, palette, accessibility) and
publishes changes over the `sharemd` bus so every egui-kit app updates live.
The "daemon that talks to a larger settings app" other apps reference is
just `sharemd` + `bigsilicon-client::Settings` — this crate is that app.

## Verification (2026-08-19)

`egui-frontend/` had no build-status record anywhere. A session with a real
rustc 1.96 (clears egui 0.34's MSRV) ran it for the first time: `cargo test`
is 3/3 — changed prefs actually produce a publish over the bus, the palette
list is non-empty, and the settings app correctly forwards changes to the
publish path. Compiles clean; only pre-existing `egui::Panel`/
`CentralPanel::show` API-rename deprecation warnings, not touched.
