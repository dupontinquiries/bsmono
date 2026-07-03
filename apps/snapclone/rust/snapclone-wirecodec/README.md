# snapclone-wirecodec

Content-aware wire compression for snap payloads — for conversations in
hard-to-service areas, where every byte that doesn't go on the wire is a byte
that can't be lost on a flaky link.

## What it does

Prefixes each payload with one codec tag byte and deflates it — but only when it
helps:

- `[0x00][verbatim]` — Stored (no compression)
- `[0x01][deflate]`  — Deflate-compressed

The policy (`should_compress` + the measure-and-keep check in `encode`):
- **Image / Video** (AV1, JPEG — already compressed) → Stored, no deflate
  attempt. Saves CPU on the biggest payloads; re-compressing wouldn't shrink them.
- **Text / Link / Location / Unknown** → try Deflate, keep it only if strictly
  smaller; otherwise Stored.
- **< 64 bytes** → Stored (deflate framing isn't worth it).

**Guarantees:** lossless (`decode(encode(x)) == x` for all inputs) and never
expands beyond +1 tag byte, even on incompressible data.

## Why content-aware

A naive "compress everything" layer would waste CPU re-deflating AV1/JPEG (which
don't shrink) and could even add overhead. Measured results from the linked C++
demo: a realistic 312-byte chat caption → **63 bytes on the wire (20%, ~5×
smaller)**; a 4096-byte media blob → stored verbatim (+1 byte), no wasted work.

## Integration (C++)

`SnapRouter` wraps the MEDIA bytes (the bulk of `serializeSnap`) through the FFI:
- Before serialization: `sc_wire_encode(mediaType, media, ...)`.
- After deserialization: `sc_wire_decode(wireMedia, ...)`.

The content class comes from `snap.mediaType` (Image=0…Location=4; -1=Unknown).
Header: `include/wirecodec/snapclone_wirecodec_ffi.h` (C + C++). Both use the
caller-buffer + out_len + BufferTooSmall-retry convention. The metadata header in
the envelope stays uncompressed (it's tiny); only the media body is wrapped, so
the wire-version handshake is unaffected and the change is backward-compatible if
gated (a v5 wire version, or a per-transport flag).

## Status

15 tests (11 lib + 4 FFI); in-workspace green; header C + C++ verified; staticlib
linked + run from C++ (5× on a caption, verbatim on media). Dependency:
miniz_oxide (pure-Rust deflate, no C deps). The C++ SnapRouter wiring is the
remaining integration step (kept separate so the on-wire change can be version-
gated and validated on a hardware test).
