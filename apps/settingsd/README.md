# settingsd

The settings daemon: serves the cross-app settings namespace over the
`"settings"` socket (`datashare-core`'s Hub for KV + pub-sub), persists
every change to `~/.config/gitmind` via `config-kit` so settings survive
restarts, and seeds sane defaults on first run. The central settings app
(`settings-app`/`bigsilicon-settings`) and every egui-kit app talk to it
through `bigsilicon-client`.

## Verification (2026-08-19)

Had no build-status record anywhere. Ran it for the first time: `cargo
test` is 2/2 — a live change fans out to subscribers correctly, and
settings persist and reload correctly across a simulated restart.
Compiles clean, no display or heavy dependency tree needed (no egui here —
this is the headless daemon half of the settings story).
