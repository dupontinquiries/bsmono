# dispatch-r82 shader returns — STAGED, pending a render-validation merge pass
Source: results.zip (operator upload). NOT merged into live shaders yet — these are
render-side edits whose gate (glslang/spirv-val/descriptor_gate + a render build) the
coordinator sandbox can't fully run, so they merge in a dedicated render pass on-device.

Returned (6/7):
- r82-01 mob-pbr           ~ entity.frag (PBR materials), descriptor-STABLE
- r82-02 mob-eyes-glow     ~ entity.frag (+45/-2), HDR emissive eyes → bloom; descriptor-STABLE.
                             NOTE: needs the bake to colour eye boxes HDR (>1) to actually glow.
- r82-03 wing-membrane     ~ entity.frag + NEW membrane.glsl (translucent backlit wings)
- r82-05 bending-elem-vfx  + NEW bending_vfx.{vert,frag} (+ .spv) ; build.rs SHADERS[]+2 ; baseline+1
- r82-06 bending-aura      compiled entity.frag.spv present
- r82-07 physics-debris    + NEW particle.{vert,frag} ; build.rs SHADERS[]+2 ; baseline+2
- r82-04 water-wakes       STOP (correct): off-limits chunk.frag/froxel + can't validate in-sandbox.

MERGE PLAN (render pass): 01/02/03 all edit entity.frag → 3-way base-anchored
`git merge-file` against the r82 base (do NOT assume disjoint), then add the new
shader files (bending_vfx, particle) + their build.rs SHADERS[] + descriptor_baseline
entries, then `cargo build --release --features render` + harness descriptor_gate to
validate. Eyes-glow also needs a one-line bake change (HDR eye colour) to be visible.
new-docs in the upload (textures, blocks_manifest_v2, shader_synthesis_guide) → the
texture-pack pass, separate.
