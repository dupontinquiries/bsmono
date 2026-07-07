# BIN PATCHES — forge animation + ambient-environment chain + dragon LOD
# (unendlich_world.rs / bin frame loop — can't compile in the lib sandbox; all the
#  logic below lives in tested lib cores, so these are thin call sites.)

## 1. Animate the forge dragon (rig already lib-side)
The forge dragon now rigs to a skeleton and animates in `forge_to_vertices`:
- LEGS swing from `creature.gait_phase` (already advanced by the existing locomotion — no change needed).
- WINGS / JAW / idle head-sway run on `creature.anim_time` (NEW field, seconds).
- The body leans with `creature.pitch / roll / torso_bend` (already set by `settle_posture`).

ONE bin change: advance the idle clock each frame for every creature you tick:
    creature.anim_time += dt;            // wherever you advance gait_phase / physics
(Default 0 ⇒ no idle motion, so this is optional; legs already animate without it.)
Rest pose (gait_phase==0 && anim_time==0) is identical to the old static cloud, so
nothing else regresses.

## 2. Ambient-environment chain — `sim::env_frame`
Per FRAME, once:
    use unendlich_engine::sim::env_frame::{frame_env, TimeOfDay, Weather};
    let env = frame_env(
        TimeOfDay { t01: day_fraction, latitude_rad: world_lat, season01: season },
        Weather { overcast01, humidity01, turbidity01, wind_speed, rain01 },
    );
    // upload to the existing lighting/fog shader uniforms (shaders themselves untouched):
    //   env.shading.sun_color / sun_intensity / ambient_color / ambient_intensity
    //   env.shading.sky_zenith / sky_horizon / exposure
    //   env.sun_dir                              (light direction)
    //   env.fog (density_at / extinction) + env.fog_color
    let ambient = env.ambient_sound_floor;       // dB-ish masking floor

AI hearing (replace any raw radius check):
    use unendlich_engine::sim::sound_masking::{can_hear, Ears, Sound};
    if can_hear(Ears { base_range, sensitivity }, Sound { loudness, freq01 }, dist2, occlusion, ambient) { ... }

Per CHUNK tick (each chunk owns parallel height/water/moisture tiles, row-major w*h):
    use unendlich_engine::sim::env_frame::tile_step;
    tile_step(&mut heights, &mut water, &mut moisture, w, h, rain, talus); // flow -> erode -> moisture
    // apply eroded `heights` back to the chunk; keep `water`/`moisture` for next tick.

Plant growth from that moisture (per plant):
    use unendlich_engine::sim::plant_growth_env::{effective_rate, Conditions, Tolerance};
    use unendlich_engine::sim::plant_growth; // advance stages with the env-scaled rate
    let rate = effective_rate(base_rate, Conditions { moisture01, light01, temp_c, season01 }, tol);
    // feed `rate` into plant_growth's advance; light01 from env (sun_irradiance / shading).

## 3. Dragon LOD by distance — `sim::forge_lod`
Keep each dragon's current forge resolution (start 256 = DRAGON_FORGE_RES). As the
camera moves:
    use unendlich_engine::sim::forge_lod::{next_res, should_rebuild};
    let want = next_res(cur_res, camera_dist, 256);   // hysteretic: no boundary thrash
    if should_rebuild(cur_res, want) {
        let m = unendlich_engine::spawn::spawn_dragon_with_genome_res(pos, &genome, want);
        creature.forge = m.forge.clone();             // swap the Arc<ForgeModel>
        cur_res = want;
    }
Only rebuild on a tier change (not per frame). Far dragons drop to 16³ (≈ a few
hundred boxes) instead of 256³ (~12k), cutting overspawn vertex load hard while the
nearest dragon stays full detail. Genome/anatomy/hitboxes are unchanged by a rebuild
(only the visual voxel model swaps).

## Notes
- `env_frame`, `forge_lod`, the rig (`dragon_forge::skin`), and `creature.anim_time`
  are all merged + unit-tested in the lib. These call sites are the only bin work.
- TimeOfDay/Weather are bin-owned; you can start with fixed values (e.g. noon, calm)
  and refine later — the cores already handle the full range.
