# fao-codec-egui: port status (against fao_app, the GTK4/libadwaita original)

`fao_app` (~13K lines) stays the parity reference and is not being deleted. This port
reaches for the real backend crates (`fao_codec_lib`, `fao_codec_v2`) directly rather than
wrapping GTK handlers, which is also why it's the one crate in this tree verified with the
offline Rust 1.96 toolchain — it builds headless, GTK does not.

## Done

- **Shell** — tab bar (Lab / Sequence Lab / Player / Library), uikit-egui theme + a11y.
- **Encode Lab** — the real `encode_iframe_recon` on a synthetic torture pattern OR a loaded
  PNG/JPEG (`image` crate; odd dimensions cropped to even for the half-size chroma planes).
  Reports actual bitstream bytes/bpp/PSNR (an early version reported the reconstruction's
  byte length instead — 24.000 bpp on every quality setting; fixed, and the regression test
  now asks for strictly-more, not `>=`). Orig / Decoded / Error (`|src−dec|×4`) views.
- **Sequence Lab** (`seq_lab.rs`, `seq_page.rs`) — drives the actual `V2Encoder` over a
  synthetic pan, reports the K-frame/T-frame cost split per frame (an average would hide a
  motion-estimation failure the split reveals) plus skip percentage. Also hosts `reproject`
  (per-pixel motion from depth + camera pose, correcting an earlier doc's claim that FAO
  lacks a depth buffer) and `part_fit` (structure fitting, showing the accept AND the reject
  — the module's own doc says a confident wrong fit is worse than the miss it would prevent).
- **Library page** — the FAO Web Library (persistent cross-encode canonical-mesh catalog):
  init/open, stats, prune, rollback (rotating `.bak.0..4`, itself reversible), bench. Same
  backend calls as the GTK panel; native folder choosers become a path field (recorded
  divergence, not an oversight).
- **Player** — opens a `.fao`, decodes and displays frames:
  - `decode.rs` is fao_app's `decode_fao_frame_to_rgb` transcribed branch for branch: D/P/L
    I-frames, K via the `FAOBKD01` block-dict trailer, T by decoding its anchor first
    (recursively, through a cache) then `decode_with_anchor`, v10 chained on the previous
    frame, the v8-era `'R'` repeat-marker walk-back (600 frames), raw/zlib YUV-or-RGB sized
    by the header.
  - **v8/v8.x rendering** — renders through the SAME `V8Decoder::render_frame_rgb` fao_app
    calls. Reading that function first (rather than assuming the full trailer stack was
    needed) found it only ever consults `self.webs` (the `V8MX` multi-web trailer) and
    `self.library_refs`; `scene_mesh` (`V8MS`) and `pose_track` (`V8PT`, three on-disk
    formats in fao_app) are never read by rendering itself — `pose_track` only feeds the
    optional jitter/EMA stabilizer, and the per-frame pose comes straight out of the frame
    bytes. So only `V8MX` needed porting, and it already had a public one-call parser
    (`V8Decoder::parse_v8mx`) — nothing to transcribe there either, just the needle-search
    that locates the trailer's byte span, copied from fao_app. Verified with a real render:
    a byte-exact synthetic V8MX trailer (one triangle, correctly zstd-compressed) through
    the actual CPU rasterizer, checked for non-black output at the right size — not a mock.
  - Transport: ⏮ ◀ ▶ ⏭ (anchor-to-anchor uses the existing anchor map), frame slider with
    marker + anchor readout, wall-clocked playback at the file's fps.
  - **A/B split** — two decks (Load A / Load B / B = A), draggable-divider comparison drawn
    from both textures.
  - File info line: `+dict` / `+v8` when those trailers are present.
  - What decode.rs still reports as a named `Gap` rather than rendering, and why: `'V8'`/`'Z'`
    frames whose webs are ALL library refs (`V8LR`, needs an external file via `FAO_LIBRARY`
    — not ported); `'V4'`..`'V7'` (fao_app renders nothing for these either).

## Encode page (c20unified)

The tester's report ("I can only do a test frame that doesn't even have feedback") was about
this page not existing. It does now, as the default tab (`encode_page.rs`):
- a video path, probed with ffprobe as you type (size, fps, duration, codec, frame estimate),
  quality, a first-N-frames cap, compact or fast mode, and an output path derived from the
  input unless edited;
- Encode runs on a worker thread and the page shows progress with fps and ETA; Cancel stops
  between frames; the output is written to `<name>.fao.part` and renamed only on success, so a
  crash mid-encode never leaves a half file under the real name;
- done: frames, size against the input file, bpp, mean PSNR, patch and repeat counts, **Open
  in Player**, and -- with an archive open -- the file added to it.

The encoder underneath is the hive line's streaming `transcode_cancellable_with` (see below).
A live per-frame cost graph and a decoded-output preview existed here before the merge and
were dropped with it: that transcoder's progress callback reports frame counts only. A
per-frame hook in its loop would bring them back.

The GTK dialog's codec versions (v2.0–v8, the v9 trailer) are still not ported; their
encoders live in fao_app's main.rs. The page says so rather than offering choices that would
all do the same thing.

## From the hive line (merged c20unified, e74b33b9c)

A sibling session did most of the codec-side work, documented in `../docs/` (CODEC_FORMATS,
ARCHITECTURE). What it changed about THIS port, from its commits:
- `transcode.rs` streams frames in and writes the container out as it goes, so memory is one
  frame whatever the length; cancellable; parallel in fast mode;
- **compact mode is the default** (`MOTION_DEFAULT`): patch frames predicted from the
  previous decoded frame, plus key frames and one-byte repeat frames; measured at 1080p,
  a camera pan 40% smaller at +2 dB against fast mode, at about half the speed;
- a **Media** tab: local folders and dufs servers, glob patterns, encode selected matches;
- the Player opens files of any length (lazy anchors, the dictionary and V8 trailers read
  without the whole file) and plays a `.fao` straight off an HTTP server with range requests;
- a codec catalogue picker and an exporter.

## Inter frames: the v2.1 T-frame path (c20unified, superseded)

Before the merge this port had its own inter-frame encoder: v2.1 T-frames (`TranslatedFrame`,
motion vectors + luma residual against the last I-frame), decided per frame against a PSNR
floor, with a test that the Player decodes every frame to exactly the quality the encoder
measured. It was replaced by the hive line's compact mode on evidence: v2.1 predicts every
T-frame from the anchor, so on a camera pan the uncovered edge grows each frame and most
frames were refused, while compact predicts from the previous frame. The code is in history
(12329fb78).

What outlived it is a **library fix** (`fao_codec_lib/src/v21_predictive.rs`, 3e2a9fa33): T-frame
motion compensation moved only luma, and the decoder copied the anchor's chroma unmoved, so in
any T-frame with motion the colour stayed where the anchor had it. Chroma now moves with its
block (MV/2); a 2 px shift decoded at 21.6 dB before, 24.7 dB after, at the same bytes. Every
`.fao` with T-frames decodes with it, including those fao_app writes.

## Archive (c20unified)

fao_app's video library -- "+ New Archive" (a folder, `archive.fao_lib` written there),
"Open Archive" (the manifest, or its folder), "+ Add Video" (a .fao; title, size, rate, frames,
length from its header), rows that open in the Player -- over the same `fao_codec_lib::Archive`
calls. Every change is saved at once, as in GTK. An encode finished on the Encode page is added
to the open archive, as fao_app's Export does, and the page says so.

Divergences, recorded in `archive.rs`: path fields for the native choosers; metadata read from
the .fao's first 4 KB plus its size from the filesystem (fao_app read the whole file); missing
files marked in the list; "Remove" drops the entry, never the file.

**Library fix found on the way**: `Archive::load_manifest` sliced unchecked past a 28-byte
length guard, so a truncated or damaged manifest -- a file the user picks -- panicked instead of
returning an error (first panic at a 28-byte truncation). Rewritten over a bounds-checked cursor,
same format; counts bounded by the bytes left. Test: every truncation of a real manifest plus
four corrupted length fields load without panicking, and the full file round-trips.

## Not started

- **World tab** — 3D scene viewer (freecam, orbit, colour modes). The largest remaining gap;
  the mesh/rasterizer plumbing the v8 Player work exposed (`SceneMesh`, `V8Decoder`,
  `v8_rasterizer`) is the natural foundation for it.
- ~~Real inter-frame transcode~~ — done: compact mode, from the hive line (above). (`V2Encoder`
  turned out to be `VideoEncoder` plus scene-graph statistics, with no bytes to wire.)
- **`--v9` options**, AV1 export comparison.

## Done, added since the above was written

(Historical: the intra-only slice below has since been replaced by the streaming transcoder
described above. Kept for the two library bugs it found.)

- **Real sequence transcode** (`transcode.rs`) — a real video file in, a real `.fao` out,
  opened straight in the Player. Deliberately the simplest pipeline that is real: every frame
  intra-coded with `compress_frame_dct` (the exact function Encode Lab already exercises), no
  T-frames/K-dict/`VideoEncoder` GOP logic — `VideoEncoder::encode_frame` doesn't expose its
  compressed bytes, only the reconstruction, so wiring it up is the item above, not a detail
  of this slice. `ffprobe` for stream info, one `ffmpeg` process piping raw RGB24 for the
  whole decode (not fao_app's one-process-per-frame). Tests generate their own input with
  `ffmpeg`'s `testsrc` filter — no fixture files needed — including one that runs the exact
  loop the Player uses (`FaoFile::parse` → `decode_rgb` per frame) against the transcoded
  output. This is also where **both `decompress_frame_dct` bugs below were found**: nothing
  in this port had put a real, ffmpeg-decoded frame through the codec before, and both bugs
  were invisible to every prior test because they all decoded against that same encode's own
  expectations.

## A bug this port's tests found in the shared lib, not itself

`fao_codec_lib::decompress_frame_dct` had two independent RLE bugs, both found via
`transcode.rs`'s ffmpeg-driven tests — the first real content this codec had been decoded
against rather than its own encode's expectations:
- an escape/filler symbol (a long zero-run marker) misread as a real coefficient, shifting
  every coefficient after it by one position for the rest of the block;
- a full block (last coefficient at zigzag position 63) never reading its trailing EOB
  marker, desyncing every plane read after it — found as a quality-vs-PSNR cliff (fine
  through q=77, ~15dB collapse at q=78 on the same content) with the escape counter at zero
  either side, then localized by splitting PSNR per plane.

Both fixed in the lib, with two permanent regression tests there — see that commit for the
full bisection.

`fao_codec_lib::write_fao_container` wrote a 14-byte on-disk frame header (it emitted a
`gmy` byte the reader never expected) while indexing every frame at the 13-byte size
`read_fao_frame` and the transcode worker actually use. Every file it produced read back one
byte late — frame 0's marker was `0`, frame 1's header didn't parse. No app called this
writer, so it survived; the Player's round-trip test was its first caller and failed on the
first run. Fixed in the lib, not worked around here.

## Damaged files (c20unified)

`decode.rs`'s `damaged_files_are_refused_not_panicked_on` takes a real `.fao` from the
transcoder (key, patch and repeat frames) and feeds every truncation and every single byte set
to 0x00 or 0xFF -- 46,330 files -- through `FaoFile::parse` and every frame's decode. It found
two library bugs, both in paths the Player and fao_app share:
- `read_fao_index` reserved as many entries as the header's frame count claimed; one damaged
  byte asked for 68 GB and the allocator ABORTED the process -- not a panic, so the app
  vanished instead of reporting a bad file. It now reserves only what the file can hold.
- `read_fao_frame` indexed one byte past a frame header when the file ended right after it
  (an off-by-one guard), and its bounds sums could wrap on 32-bit targets (armv7: reMarkable
  1/2). Fixed with `get` and `checked_add`.
Now every one of the 46,330 is refused cleanly. Since the Player also streams partial downloads
over HTTP, damaged input is not hypothetical.

