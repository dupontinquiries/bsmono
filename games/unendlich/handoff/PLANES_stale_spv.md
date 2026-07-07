# PLANES — ROOT CAUSE: stale precompiled shader SPIR-V

## What's happening
Your build ships committed SPIR-V (`assets/shaders/*.spv`) with `.spv.hash`
sidecars. `crates/unendlich-engine/build.rs` recompiles a shader ONLY when the
GLSL's SHA-256 differs from its `.hash` sidecar, AND a GLSL compiler is
installed. Its own header documents the exact bug you're seeing:

> "the v0.24.20 vertex-pack ship hit exactly this: pre-pack `.spv` survived
>  against post-pack `.glsl`, the pipeline unpacked vertices, **every vertex
>  decoded into `[0,1]³` near origin**"

Vertices collapsing to `[0,1]³` near the chunk origin = flat textured triangles
fanning out from a point — your screenshot. It survives GPU_MESHER=0/2, water
reverts, sway toggles, and my shader edits, because the GPU is running an **old
`chunk.vert.spv`** that unpacks positions differently than the current mesher
packs them. My `.vert` edits never took effect: with no compiler installed,
build.rs falls back to the stale committed `.spv`.

## Confirm (one command)
```
which glslc glslangValidator
```
If BOTH are missing, that's the smoking gun — the build has been using stale
SPIR-V the whole time.

## Fix
1. **Install a GLSL→SPIR-V compiler** (either works):
   - Arch/KDE:  `sudo pacman -S shaderc`   (gives `glslc`)
     or         `sudo pacman -S glslang`   (gives `glslangValidator`)
   - Debian/Ubuntu: `sudo apt install glslc`  or  `glslang-tools`
   - Or the LunarG Vulkan SDK (ships both).
2. **Force a clean shader recompile** (this zip already deleted the `.hash`
   sidecars, so build.rs can't skip — but delete the stale `.spv` too to be
   certain it can't fall back to them):
   ```
   rm -f assets/shaders/*.spv assets/shaders/*.spv.hash
   ```
   Then run `run-latest.sh` as usual (it builds with `hot-shaders`, which reads
   the freshly compiled `.spv` from disk).
3. Watch the build output: build.rs prints a warning if it can't find a
   compiler. If you see that warning, the recompile did NOT happen — install the
   compiler and retry. With `UNENDLICH_STRICT_SHADERS=1` set, a missing compiler
   becomes a hard error instead of a silent fallback (recommended while
   debugging this).

## Why this zip helps
- `chunk.vert` restored to pristine (my r608 sway-off test removed — it was never
  the cause).
- All 35 `.spv.hash` sidecars deleted → build.rs will recompile every shader from
  the current GLSL on the next build (given a compiler), guaranteeing the SPIR-V
  matches the source and the vertex unpack matches the packing.

## After the fix
The fans should be gone. If they persist even after a confirmed recompile
(`glslc`/`glslangValidator` present, `.spv` regenerated with fresh timestamps),
then it's not the shader — reply and we bisect the other draw paths
(GPU_DECORATE, LOD, entity) by env, but this is almost certainly it.
