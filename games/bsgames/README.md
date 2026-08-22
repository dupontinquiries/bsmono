# bsgames

BigSilicon Games centre — launch the fleet's games, see friends' presence
over the netcode DHT, and chat via snapclone. Backend + egui GUI, in the
`bsettings` mould.

## Verification (2026-08-19)

Had no build-status record anywhere. Ran it for the first time with rustc
1.96: `cargo test` (default, no `app`/GUI feature) is 22/22 — friend
add/dedup, presence sorting, the host/join handoff env encoding, ID
generation and round-trip parsing, chat routing through `share-kit`'s
mailbox on a `snapcore` store, DM-id order-independence, and the first-run
wizard's phase transitions (including skip-jumps-to-done and unknown
combinations being a no-op). `cargo check --features app` (the eframe GUI)
also compiles clean. Only pre-existing `egui::SelectableLabel`/
`Context::screen_rect` API-rename deprecation warnings in `uikit-egui`, not
touched.
