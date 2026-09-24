# Proposed shaders -- written, never compiled

A shader here has been written (and reviewed by hand) but never fed to a GLSL compiler, let
alone run on a GPU. It lives outside `src/shaders/` on purpose: everything in `src/shaders/`
is a LIVE shader to the build -- `shader_artifacts.rs` requires it to be in `build.rs`'s
`SHADERS[]` and to ship a precompiled `.spv`, and adding an uncompiled file to `SHADERS[]`
would make every machine that has `glslc` compile it at build time, where one error in it
breaks everyone's build.

| file | from | what it is |
|---|---|---|
| `chunk_cull_subgroup.comp` | `a2e8568a9` | a barrier-free variant of `chunk_cull.comp`: `subgroupBallot` replaces the per-workgroup shared-memory tally, 32-wide for Apple GPUs. The commit message has the audit and the reasoning. Nothing in the engine loads it. |

## To promote one

1. `glslc shaders_proposed/<name> -o /tmp/<name>.spv` -- it must compile;
2. `git mv shaders_proposed/<name> src/shaders/` and add `src/shaders/<name>` to `build.rs`'s
   `SHADERS[]`;
3. commit the `.spv` and its `.spv.hash` sidecar the same way as the other core shaders (the
   `shader_artifacts` tests check both, and that the hash matches the source);
4. load it from Rust behind a capability check (subgroup ops are an optional feature), and
   measure it against `chunk_cull.comp` on the target GPU.
