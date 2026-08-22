# snapshot-cam

A local camera: take a photo, keep it on this device, and hand it to
photosync or a snapclone contact when you choose to. Tracks disclosure
state explicitly — a capture starts local, and sharing/sending is a
distinct, trackable transition, not implied by the photo simply existing.

## Verification (2026-08-19)

Had no build-status record anywhere. A session with a real rustc 1.96
(clears egui 0.34's MSRV, and this crate's default `app` feature pulls the
full `wgpu` eframe backend, not just `glow`) ran it for the first time:
`cargo test` is 14/14 — capture-is-born-local, disclosure tracking (sending
counts as disclosure, backing up does not), re-targeting a failed send,
refusing a second concurrent send, filename generation defends against a
device name breaking the path, and the roll answers "what have I shared"
newest-first. Compiles clean; one pre-existing `egui::CentralPanel::show`
deprecation warning (0.34 renamed it `show_inside()`), not touched.
