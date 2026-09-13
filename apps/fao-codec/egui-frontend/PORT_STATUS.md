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

## Not started

- **World tab** — 3D scene viewer (freecam, orbit, colour modes). The largest remaining gap;
  the mesh/rasterizer plumbing the v8 Player work exposed (`SceneMesh`, `V8Decoder`,
  `v8_rasterizer`) is the natural foundation for it.
- **Real inter-frame transcode** — the transcode work below is intra-only by design; wiring
  `V2Encoder`'s T-frames into it (once its compressed-bytes API exists) is the next step, and
  `seq_lab`'s K/T cost split is already the right report for when it lands.
- **`--v9` options**, AV1 export comparison.

## Done, added since the above was written

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
