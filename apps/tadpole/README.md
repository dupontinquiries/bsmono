# tadpole

A Frog-shaped OCR app, all Rust, on egui with touch support: capture or drop
an image, get selectable text, copy a region or the lot. Handles diarised
transcripts (turn/speaker labels), table and diagram detection in OCR'd
pages, and reading-order reconstruction for skewed or multi-column scans.

## Verification (2026-08-19)

`egui-frontend/` had no build-status record anywhere. A session with a real
rustc 1.96 (clears egui 0.34's MSRV) ran it for the first time: `cargo test`
is 94/94, spanning speaker/turn diarisation and filtering, table vs. diagram
vs. prose detection (including deliberately adversarial cases — a giant
heading that shouldn't merge every row, a wobbly baseline that should still
group into rows, a paragraph that should never parse as a table), region
selection and reading-order reconstruction (skewed corners, left-to-right
grouping despite uneven line tops), and the app-level flows (drop-to-scan,
missing-model handling, rescanning after new models arrive, copy semantics
per selection state).

Compiles clean. Not run on a real display (headless sandbox) or against a
real camera/OCR model: the pure logic is confirmed, the on-device capture
and recognition pipeline is not.
