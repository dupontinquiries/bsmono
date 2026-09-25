# vacc, vendored

Upstream: https://github.com/andreytkachenko/vacc
Commit:   108dc16c2ca6fa7c6e623471273425b0d5c5c64e (2026-09-22)
Licence:  MIT OR Apache-2.0 (workspace `license`); the H.265 CPU decoder is a
          port of hevc.js, MIT (crates/vacc-software-decode/HEVC_LICENSE).
          The H.264 CPU decoder is described upstream as a bit-exact port of
          edge264; edge264's own licence is NOT included in the upstream tree.
          VERIFY it before distributing binaries that contain it.

## Why it is here

Vulkan Video leaves bitstream parsing, reference management and reordering
to the application. vacc does those (shared parser + DPB) with backends for
Vulkan Video (ash), NVDEC, VAAPI stateless and CPU. Upstream reports all 40
decodable cells byte-exact against FFmpeg (300 frames each; RTX 3060 for
Vulkan/NVDEC, Meteor Lake iHD for VAAPI), on 640x360 Big Buck Bunny only.

Checked here (x86_64, no GPU): the CPU H.264 backend decodes an H.264 High
clip with 3 B-frames 120/120 frames byte-identical to ffmpeg; the Vulkan
backend builds and fails cleanly with no driver ("Unable to find a Vulkan
driver").

## What was taken, what was changed

Taken: vacc-core, vacc-common, vacc-parser, vacc-software-decode,
vacc-vulkan, vacc-vulkan-common, vacc-vulkan-decode.

Left out: the root `vacc` package (a unified API with automatic backend
fallback, depending on the crates below; the root Cargo.toml keeps only the
workspace sections), vacc-examples (a CLI), vacc-vaapi-decode (needs libva headers and
its own libva binding crate -- the next one to add: it streams, and it is the
in-process route to DRM-PRIME frames on Intel/AMD and on Asahi's VA layer),
vacc-nvdec-decode (libnvcuvid at runtime; add for NVIDIA), assets/ (134 MB of
test media).

Changed, one patch: crates/vacc-vulkan/Cargo.toml no longer depends on
`vk-video` 0.3.1 and `gpu-video` 0.4.0. Nothing in the source uses them (one
comment names a header path); each pulls a full wgpu stack. Without them the
Vulkan backend builds in ~14 s.

## Known gap: the Vulkan backend does not stream

vacc-vulkan-decode's `Decoder::submit`/`decode` return "not supported - use
decode_all instead": the Vulkan path extracts every access unit from the
whole bitstream, decodes, then sorts each GOP by POC. Playback needs
incremental submit with a bounded reorder buffer. The per-access-unit logic
exists inside decode_all_h26x / _vp9 / _av1; turning those loops into state
machines is the patch to write (ideally upstream). Until then vidkit-vacc
refuses Vulkan for streaming rather than pretending.
