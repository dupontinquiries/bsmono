# GPU_BRIDGE.md — wiring the moisture sim into fog approach 4

Goal: get the 2.5D moisture sim (`moisture_field.rs`, CPU, tested) onto screen as
fog approach 4. Done as ONE unit so it's debuggable together. The texture core
(`moisture_texture.rs`) is already drafted but **unregistered** so it can't break
the approach-3 build. The interlocking pieces below stay unwired until we do them
in one pass.

## The chain
CPU `MoistureField` → `fog_density` grid (row-major f32, w*h)
  → `MoistureTexture::record_upload` (binding 4, R32_SFLOAT 2D)
  → `chunk.frag` samples it at world XZ, blends fog.

## Steps (in order)

1. **Register the module.** `crates/unendlich-engine/src/lib.rs`: add
   `pub mod moisture_texture;` near `pub mod moisture_field;` (line ~21).

2. **Add binding 4 to the chunk descriptor set.**
   `render/chunk_pipeline.rs`, the `dsl_bindings` array (~line 214, after binding
   3 the storage buffer):
   ```rust
   vk::DescriptorSetLayoutBinding::builder()
       .binding(4)
       .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
       .descriptor_count(1)
       .stage_flags(vk::ShaderStageFlags::FRAGMENT)
       .build(),
   ```
   Then bump the descriptor **pool** sizes: the COMBINED_IMAGE_SAMPLER pool count
   needs +1 per set (bindings 1 + 2 + 4 = 3 samplers/set now). Find the
   `vk::DescriptorPoolSize` for COMBINED_IMAGE_SAMPLER and raise its count.

3. **Own + write the texture in the renderer.** `render/world_renderer.rs`:
   - field: `moisture_tex: MoistureTexture` (construct in renderer init with
     `MoistureTexture::new(instance, device, physical_device, W, H)` — start
     W=H=256).
   - in the descriptor-set write block (mirror the froxel write ~line 898), add a
     write for binding 4 using `self.moisture_tex.descriptor_image_info()`:
     ```rust
     let m_info = [self.moisture_tex.descriptor_image_info()];
     // WriteDescriptorSet { dst_binding: 4, COMBINED_IMAGE_SAMPLER, image_info: &m_info, .. }
     ```
   - per tick, before the render pass: `unsafe { self.moisture_tex.record_upload(device, cmd, &grid) }`.
   - on teardown: `self.moisture_tex.destroy(device)`.

4. **Drive the sim in the bin.** `bin/unendlich_world.rs`:
   - own a `MoistureField::new(W, H, cell, origin)` (cell ~4.0m → 256*4 = 1 km
     window; recenter origin on the camera occasionally, snapped to `cell`).
   - build the wind sampler from atmosphere wind + a chunk-heightmap closure
     (`wind_sampler(base, gust, gust_scale, deflect, origin, cell, &|x,z| height)`).
   - build the climate closure (`|x,z| ClimateSample { temperature, is_water }`)
     from temperature + water presence at that world cell.
   - tick: `field.begin_tick(); field.advect(dt, DEBUG.advect_clamp(), wind); field.step_climate(&params, climate);`
   - grid for upload: `let grid: Vec<f32> = (0..H).flat_map(|z| (0..W).map(move |x| field.fog_density(&params, x, z, T(x,z)))).collect();`
     pass it to the renderer's upload.

5. **Pass the field's world rect to the shader via spare UBO slots** (no layout
   change — all documented-free, register in RULESET slot table):
   - `camera_pos.w  = origin_x`
   - `sun_color.a   = origin_z`
   - `ambient_color.a = 1.0 / (W as f32 * cell)`   // inv world-size (square)

6. **Sample in `chunk.frag` approach 4.** Add at top:
   `layout(set = 0, binding = 4) uniform sampler2D moisture_tex;`
   Replace the approach-4 fall-through with:
   ```glsl
   } else if (fog_approach == 4) {
       vec2 origin = vec2(ubo.camera_pos.w, ubo.sun_color.a);
       float inv   = ubo.ambient_color.a;
       vec2 uv = (v_world_pos.xz - origin) * inv;       // world -> [0,1]
       float m = texture(moisture_tex, uv).r;           // visible condensate
       float dens = m * 0.15;                           // tune
       fog = clamp(1.0 - exp(-dens * dist), 0.0, 1.0);
   }
   ```
   Then `bash handoff/force_recompile_shader.sh chunk.frag`.

7. **Validate**: `python3 handoff/ubo_parity.py` (camera_pos.w / sun_color.a /
   ambient_color.a now meaningful — keep Rust/GLSL field order identical),
   `bash harness/run.sh`, then in-game flip the panel to approach 4.

## Gotchas
- Binding 4 must exist in the pool AND the layout AND the shader, or validation
  errors / a blank sample. That's why this is one unit, not incremental.
- `record_upload` transitions to SHADER_READ_ONLY_OPTIMAL; the froxel path leaves
  its image in GENERAL — don't copy that layout for the moisture sampler.
- Recenter the field origin snapped to `cell` (never sub-cell) or the world-
  anchored guarantee breaks and the fog will swim (R10).
- Spare-slot reuse: `camera_pos.w` was nominally 1.0; nothing in chunk.frag reads
  it, but confirm no CPU path multiplies `camera_pos` as a homogeneous vec4.
