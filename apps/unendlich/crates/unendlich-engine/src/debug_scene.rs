//! `debug_scene` (r469) — shared debug-overlay population + workload-summary formatting.
//!
//! Extracted from unendlich_fullgame.rs / unendlich_world.rs, which held byte-identical copies
//! (kitfinder exact-clone class: populate_debug_scene ~1476 tokens + format_workload_summary). One
//! home so a fix lands once and can't diverge between the two binaries.

#![allow(clippy::too_many_arguments)]

use crate::physics::PhysicsBody;
use crate::constants::runtime_render_distance;
use crate::world::chunk::ChunkCoord;

pub fn populate_debug_scene(
    scene: &mut crate::debug_overlay::DebugScene,
    chunk_manager: &crate::world::chunk_manager::ChunkManager,
    body: Option<&PhysicsBody>,
    camera_position: &nalgebra_glm::Vec3,
    camera_forward: &nalgebra_glm::Vec3,
    heat_sources: &crate::heat_source::HeatSourceMap,
    day_phase: f32,
) {
    use crate::world::ticks::TickId;
    let _ = TickId::ZERO; // silence unused if branches drop the use

    // ---- Wind arrow at the player ----
    let player_pos = match body {
        Some(b) => b.position,
        None => [camera_position.x, camera_position.y, camera_position.z],
    };
    let sim_t = chunk_manager.current_tick().to_sim_seconds(chunk_manager.tick_period());
    let (wind_x, wind_z, wind_mag) = crate::wind::wind_at(
        player_pos[0] as f64,
        player_pos[1] as f64,
        player_pos[2] as f64,
        sim_t,
    );
    // Vertical convection component — tilts the wind arrow up/down
    // based on local temperature gradient. Updrafts → arrow points
    // up; downdrafts → arrow points down. Player can SEE the
    // convection field by walking around (over hot biomes the arrow
    // tilts up; over cold biomes / above sea the arrow may dip).
    //
    // Heat-aware AND solar-aware: factors in fires + heat sources +
    // diurnal cycle. Pressing F9 to light a fire creates a strong
    // updraft. Standing in sunlight at noon adds an updraft over
    // open ground; midnight removes it. This is the visual
    // confirmation that the climate × heat × diurnal couplings
    // are alive.
    let _ = sim_t; // sim_t was used by old non-heat-aware path
    let vertical_wind = crate::temperature::convection_at_full(
        player_pos[0].round() as i32,
        player_pos[1].round() as i32,
        player_pos[2].round() as i32,
        heat_sources,
        day_phase,
    );
    if wind_mag > 0.01 || vertical_wind.abs() > 0.01 {
        // Place arrow 2 blocks above the player so it's clearly visible.
        let origin = [player_pos[0], player_pos[1] + 2.0, player_pos[2]];
        // Length scales with TOTAL wind magnitude (horizontal + vertical).
        let total_mag = (wind_mag * wind_mag + vertical_wind * vertical_wind).sqrt();
        let length = (total_mag * 2.0).clamp(0.2, 2.0);
        // Color shifts with vertical: cyan when horizontal, warm yellow
        // when strongly upward (visible heat plume), cool blue when
        // strongly downward.
        let color = if vertical_wind > 0.05 {
            [1.0, 0.9, 0.4] // updraft — warm yellow
        } else if vertical_wind < -0.05 {
            [0.3, 0.5, 1.0] // downdraft — cool blue
        } else {
            [0.4, 0.85, 1.0] // neutral horizontal — sky cyan
        };
        scene.add_arrow(
            origin,
            [wind_x, vertical_wind, wind_z],
            length,
            color,
            0.12,
        );
    }

    // ---- Player velocity arrow ----
    if let Some(b) = body {
        let vmag = (b.velocity[0]*b.velocity[0]
            + b.velocity[1]*b.velocity[1]
            + b.velocity[2]*b.velocity[2]).sqrt();
        if vmag > 0.05 {
            // Origin at body center.
            let origin = b.position;
            // Scale length by velocity magnitude (capped).
            let length = (vmag * 0.2).clamp(0.1, 1.0);
            let color = if b.on_ground {
                [0.2, 1.0, 0.3] // green = grounded motion
            } else {
                [1.0, 0.6, 0.2] // orange = airborne motion
            };
            scene.add_arrow(origin, b.velocity, length, color, 0.10); // was 0.05
        }

        // Player AABB outline. Bumped to 0.10 thickness so the wireframe
        // is actually readable (was 0.03 = 3cm, basically invisible).
        scene.add_aabb(b.aabb_min(), b.aabb_max(), [1.0, 1.0, 0.2], 0.10);
    }

    // ---- Forward-direction compass ----
    //
    // A bright magenta marker placed RIGHT IN FRONT of the camera
    // along its actual forward vector. Guaranteed visible at toggle
    // time so the user sees something immediately and knows F4
    // worked. The previous version placed it at world-+X which only
    // happened to be visible if the camera was looking that way —
    // most camera orientations missed it entirely.
    //
    // 2.5 blocks ahead is close enough to be obvious but far enough
    // to not be inside the player's body.
    let fwd_dist = 2.5_f32;
    let marker_pos = [
        camera_position.x + camera_forward.x * fwd_dist,
        camera_position.y + camera_forward.y * fwd_dist,
        camera_position.z + camera_forward.z * fwd_dist,
    ];
    scene.add_marker(
        marker_pos,
        [1.0, 0.2, 1.0],   // magenta — never confuses with real geometry
        0.4,                // 0.4-block cube — large and obvious
    );

    // Also drop a SECOND magenta marker slightly above the player, as
    // a "you are here" indicator when looking down. Belt-and-suspenders
    // visibility — between forward-projection and overhead, at least
    // one will be in view.
    scene.add_marker(
        [player_pos[0], player_pos[1] + 3.5, player_pos[2]],
        [1.0, 0.5, 1.0],
        0.3,
    );

    // ---- Heat source markers ----
    //
    // Show an orange marker at every active heat source. Lets the
    // player see WHERE they planted fires (F9), as well as any
    // worldgen-seeded sources (lava in caves, etc.) once those
    // exist. Color saturation tracks intensity; size tracks radius.
    //
    // Cold sources (negative intensity) appear blue. Both cold and
    // hot share the same marker so the player can debug downdrafts
    // by checking what's nearby.
    for src in heat_sources.snapshot() {
        let intensity = src.intensity_c;
        // Color: hot = orange-red (1, 0.5, 0.1), cold = blue (0.2, 0.5, 1).
        // Saturation scales with |intensity| / 1000, clamped.
        let mag = (intensity.abs() / 1000.0).clamp(0.1, 1.0);
        let color = if intensity > 0.0 {
            [1.0, 0.5 - 0.3 * mag, 0.1 * (1.0 - mag)]
        } else {
            [0.2 * (1.0 - mag), 0.5 - 0.3 * mag, 1.0]
        };
        // Marker size proportional to source radius, but capped.
        let size = (src.radius * 0.15).clamp(0.15, 0.6);
        scene.add_marker(src.position, color, size);
    }

    // ---- Weathering marker at player's standing block ----
    //
    // Show a small color-coded cube right under the player's feet
    // representing the weathering value of the block they're standing
    // on. Makes the per-block weathering system visible: a uniform
    // sand desert will show a checkerboard of slightly-different
    // weathering colors (per-block hash variation), confirming the
    // bumpy-block character has actual data backing it.
    //
    // Color scheme:
    //   weathering = 0    → magenta-violet (fresh, e.g. player-placed)
    //   weathering = 0.5  → mid-yellow
    //   weathering = 1.0  → bright orange-red (fully weathered)
    {
        let foot_y = (player_pos[1] - 1.0).floor() as i32;
        let foot_x = player_pos[0].floor() as i32;
        let foot_z = player_pos[2].floor() as i32;
        // Look up the block at the player's feet and its weathering.
        let foot_chunk_coord = ChunkCoord::new(
            foot_x.div_euclid(CHUNK_SIZE),
            foot_y.div_euclid(CHUNK_SIZE),
            foot_z.div_euclid(CHUNK_SIZE),
        );
        let weathering_chunks = chunk_manager.chunks_arc();
        if let Some(entry) = weathering_chunks.get(&foot_chunk_coord) {
            let chunk = entry.value();
            let lx = foot_x.rem_euclid(CHUNK_SIZE);
            let ly = foot_y.rem_euclid(CHUNK_SIZE);
            let lz = foot_z.rem_euclid(CHUNK_SIZE);
            let block = chunk.get(lx, ly, lz);
            // Only show marker for blocks that actually weather.
            if !matches!(block, crate::world::blocks::BlockId::Air) {
                let idx = crate::world::chunk::Chunk::block_index(lx, ly, lz) as u16;
                let state = chunk.instance_state.get(&idx).copied()
                    .unwrap_or_else(|| {
                        // Use the worldgen default for this block type at this position.
                        let mut s = crate::world::chunk::BlockInstanceState::default();
                        s.age_ticks = crate::world::weathering::worldgen_default_weathering_at_depth(block, foot_y);
                        s
                    });
                let w = crate::world::weathering::weathering_value(state);
                // Color ramp: 0=violet, 0.5=yellow, 1=red.
                let color = if w < 0.5 {
                    let t = w * 2.0;
                    [
                        0.7 + 0.3 * t,        // 0.7→1.0
                        0.2 + 0.7 * t,        // 0.2→0.9
                        0.9 - 0.9 * t,        // 0.9→0.0
                    ]
                } else {
                    let t = (w - 0.5) * 2.0;
                    [
                        1.0,
                        0.9 - 0.5 * t,        // 0.9→0.4
                        0.0,
                    ]
                };
                let marker_pos = [
                    foot_x as f32 + 0.5,
                    foot_y as f32 + 0.5,
                    foot_z as f32 + 0.5,
                ];
                scene.add_marker(marker_pos, color, 0.25);
            }
        }
    }

    // ---- Lifecycle stage markers ----
    //
    // For plants in the player's current chunk that are not pristine
    // Mature, place a small color-coded marker just above. Limited to
    // the player's chunk to bound work — a forest of Drying plants
    // would otherwise dump tens of thousands of markers per frame.
    use crate::world::plant_lifecycle::Stage;
    let player_chunk = chunk_manager.player_chunk();
    let chunks = chunk_manager.chunks_arc();
    if let Some(entry) = chunks.get(&player_chunk) {
        let chunk = entry.value();
        let (origin_x, origin_y, origin_z) = player_chunk.block_origin();
        let mut count = 0;
        for &plant_idx in &chunk.plant_indices {
            if count > 200 { break; } // perf cap
            let block = chunk.blocks[plant_idx as usize];
            // Skip leaves — they have a different state machine.
            if matches!(block,
                crate::world::blocks::BlockId::OakLeaves
                | crate::world::blocks::BlockId::BirchLeaves)
            {
                continue;
            }
            let inst = chunk.instance_state.get(&plant_idx).copied().unwrap_or_default();
            let stage = Stage::from_flags(inst.flags);
            let color = match stage {
                Stage::Seedling => [0.6, 1.0, 0.4],   // pale green
                Stage::Growing  => [0.2, 1.0, 0.3],   // bright green
                Stage::Mature   => continue,           // skip Mature (default state)
                Stage::Drying   => [1.0, 0.85, 0.2],  // straw yellow
                Stage::Withered => [0.7, 0.3, 0.1],   // brown
            };
            let (lx, ly, lz) = crate::world::chunk::Chunk::index_to_xyz(plant_idx);
            let wx = (origin_x + lx) as f32 + 0.5;
            let wy = (origin_y + ly) as f32 + 1.5;  // float above the plant
            let wz = (origin_z + lz) as f32 + 0.5;
            scene.add_marker([wx, wy, wz], color, 0.30);
            count += 1;
        }
    }

    // ---- Tick-cull boundary AABB ----
    //
    // Outline the volume within which tick handlers run. Outside this
    // box, plant lifecycle / leaf decay are paused. Useful for
    // verifying the cull radius is correctly tracking the player.
    use crate::constants::CHUNK_SIZE;
    let radius = runtime_render_distance() + 1;
    let (px, py, pz) = player_chunk.block_origin();
    let lo = [
        (px - radius * CHUNK_SIZE) as f32,
        (py - radius * CHUNK_SIZE) as f32,
        (pz - radius * CHUNK_SIZE) as f32,
    ];
    let hi = [
        (px + (radius + 1) * CHUNK_SIZE) as f32,
        (py + (radius + 1) * CHUNK_SIZE) as f32,
        (pz + (radius + 1) * CHUNK_SIZE) as f32,
    ];
    scene.add_aabb(lo, hi, [0.5, 0.3, 0.8], 0.5); // purple, thick
}

pub fn format_workload_summary(loaded: &[u64], rendering: &[u64]) -> String {
    fn percentile(sorted: &[u64], pct: u32) -> u64 {
        if sorted.is_empty() { return 0; }
        let idx = ((pct as usize * (sorted.len() - 1)) / 100).min(sorted.len() - 1);
        sorted[idx]
    }
    fn stats(samples: &[u64]) -> (u64, u64, u64, u64, u64) {
        if samples.is_empty() { return (0, 0, 0, 0, 0); }
        let mut s = samples.to_vec();
        s.sort_unstable();
        let n = s.len() as u64;
        let min = s[0];
        let p50 = percentile(&s, 50);
        let p90 = percentile(&s, 90);
        let max = *s.last().unwrap();
        (n, min, p50, p90, max)
    }
    let (n_l, min_l, p50_l, p90_l, max_l) = stats(loaded);
    let (n_r, min_r, p50_r, p90_r, max_r) = stats(rendering);
    format!(
        "Workload-shape summary (samples taken once per FPS-log window):\n  \
         loaded     n={:>4}  min={:>5}  p50={:>5}  p90={:>5}  max={:>5}\n  \
         rendering  n={:>4}  min={:>5}  p50={:>5}  p90={:>5}  max={:>5}",
        n_l, min_l, p50_l, p90_l, max_l,
        n_r, min_r, p50_r, p90_r, max_r,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Empty samples produce a well-formed summary with all zeros.
    #[test]
    fn workload_summary_handles_empty() {
        let s = format_workload_summary(&[], &[]);
        assert!(s.contains("loaded"));
        assert!(s.contains("rendering"));
        assert!(s.contains("n=   0"),
            "expected n=0 for empty samples; got:\n{}", s);
    }

    /// Single-sample run: min == p50 == p90 == max.
    #[test]
    fn workload_summary_single_sample() {
        let s = format_workload_summary(&[100], &[42]);
        let loaded_line = s.lines().find(|l| l.contains("loaded")).unwrap();
        assert!(loaded_line.contains("min=  100"));
        assert!(loaded_line.contains("p50=  100"));
        assert!(loaded_line.contains("max=  100"));
        let rendering_line = s.lines().find(|l| l.contains("rendering")).unwrap();
        assert!(rendering_line.contains("min=   42"));
        assert!(rendering_line.contains("max=   42"));
    }

    /// Multi-sample run: percentiles roughly match expectations.
    /// Samples = 0..=100, so min=0, p50≈50, p90≈90, max=100.
    #[test]
    fn workload_summary_percentile_shape() {
        let samples: Vec<u64> = (0..=100).collect();
        let s = format_workload_summary(&samples, &samples);
        let line = s.lines().find(|l| l.contains("loaded")).unwrap();
        assert!(line.contains("min=    0"),
            "expected min=0; got: {}", line);
        assert!(line.contains("max=  100"),
            "expected max=100; got: {}", line);
        // p50 should be 50 or 51 depending on indexing — accept either.
        assert!(line.contains("p50=   50") || line.contains("p50=   51"),
            "expected p50≈50; got: {}", line);
        // p90 should be 90 or 91.
        assert!(line.contains("p90=   90") || line.contains("p90=   91"),
            "expected p90≈90; got: {}", line);
    }

    /// loaded vs rendering can have different shapes — they're tracked
    /// independently and reported on separate lines.
    #[test]
    fn workload_summary_independent_tracks() {
        let loaded = vec![1000u64, 1000, 1000, 1000];
        let rendering = vec![50u64, 100, 150, 200];
        let s = format_workload_summary(&loaded, &rendering);
        let loaded_line = s.lines().find(|l| l.contains("loaded")).unwrap();
        assert!(loaded_line.contains("min= 1000"));
        assert!(loaded_line.contains("max= 1000"));
        let rendering_line = s.lines().find(|l| l.contains("rendering")).unwrap();
        assert!(rendering_line.contains("min=   50"));
        assert!(rendering_line.contains("max=  200"));
    }
}
