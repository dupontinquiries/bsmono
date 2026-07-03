# FAO Codec — Documentation

FAO (Frame-Aligned Output) is an experimental video codec that 
represents footage as a streaming 3D scene reconstruction rather than 
a sequence of compressed frames. The mesh-based output enables 
cross-video mesh sharing (a per-machine library), depth-aware 
playback, and incremental encoding of recurring scenes.

It is not yet competitive with AV1 on natural video at typical bitrates 
(currently ~8x AV1 size on horror/cinematic footage). See 
`V8_PLUS_ROADMAP.md` for the path to closing that gap.

## Documentation map

Read in this order if you're new:

1. **GETTING_STARTED.md** — build, encode, play, set up a library. Run 
   through this first to confirm everything works.
2. **VISION.md** — what FAO is for, who it's for, why it exists
3. **ARCHITECTURE.md** — system map of crates, modules, data flow
4. **CODEC_FORMATS.md** — bitstream specs for v1/v2/v3/v7/v8 and 
   sub-variants (V81, V8p, V8MX, V8LR)
5. **LIBRARY_MODE.md** — cross-encode mesh sharing design and current 
   implementation status
6. **V8_PLUS_ROADMAP.md** — proposed v8.1/v8.2/v8.3+ enhancements to 
   close the gap with AV1
7. **TODO.md** — current task list, grouped by area
8. **SESSION_LOG.md** — chronological notes by work theme (useful 
   context for picking up after a break)
9. **GLOSSARY.md** — terms used throughout the codebase
10. **CONTRIBUTING.md** — conventions for editing this code

## At a glance

- 6 crates: `fao_codec_core` (primitives), `fao_codec_lib` (library 
  + v3-v8 codecs), `fao_codec_v1` (v2.0/v2.1 reference), `fao_codec_v2` 
  (currently empty placeholder), `fao_app` (GTK4 desktop app), 
  `fao_cli` (command-line tool)
- ~19k lines of Rust + ~340 lines of WGSL
- Tested on macOS (Apple M4 Max) and Linux
- Optional GPU acceleration via wgpu (Metal/Vulkan)

## What just shipped (recent highlights)

- **Real vertex colors** (huge quality fix; was painting teal-tinted 
  confidence visualization)
- **Library Phase 3** (cross-encode mesh sharing via V8LR trailer + 
  attached library handle)
- **Incremental rasterizer** (~10x decode speedup on mesh-heavy content)
- **LK structure-tensor hoisting** (~2.7x encode speedup)
- **Many small UI/codec bugfixes** from macOS testing

See SESSION_LOG.md for full chronology.
