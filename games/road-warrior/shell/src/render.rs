//! egui_wgpu custom paint callback: renders scene.wgsl into the game viewport.
//!
//! Confirmed working: built and run against a real wgpu::Device (Vulkan via
//! lavapipe under Xvfb) — the pipeline, bind group, and buffer layout are no
//! longer guesses. Screenshots of a running session validated the sky/ground
//! split, car rotation on slopes, and (as of this revision) boss visibility.

use bytemuck::{Pod, Zeroable};
use eframe::{egui, egui_wgpu};
use eframe::wgpu;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct SceneUniforms {
    pub car_x: f32,
    pub car_y: f32,
    pub car_rotation: f32,
    pub time: f32,
    pub resolution: [f32; 2],
    /// Boss position relative to the car, in world units (boss.pos_x -
    /// vehicle.pos_x). Scaled to screen space in the shader so the boss
    /// comes into view as the car approaches, rather than needing the shader
    /// to know about world/camera scale conversions itself.
    pub boss_relative_x: f32,
    /// 1.0 if a boss exists for this level and isn't defeated, 0.0 otherwise
    /// — used instead of an Option in the shader, which has no such concept.
    pub boss_present: f32,
    /// 1.0 while the boss's attack is telegraphing, drives a flash effect
    /// so an incoming hit reads as reactable rather than a surprise.
    pub boss_telegraphing: f32,
    /// Which car is being driven (0=Behemoth, 1=SixWheeler, 2=RedSedan,
    /// 3=Pillager), so the shader can give each a distinct color/silhouette
    /// instead of hardcoding red for whatever's actually selected. Reuses a
    /// former padding slot rather than growing the struct — the total size
    /// (48 bytes) already matches what scene.wgsl expects; changing it risks
    /// reintroducing the exact alignment mismatch documented below.
    pub car_kind: f32,
    pub _pad1: f32,
    pub _pad2: f32,
}

/// How many world units of track the viewport shows, and where the car sits
/// across it.
///
/// Both are shared with the shader rather than duplicated: the shader places
/// the car at `CAR_SCREEN_X` and the CPU samples terrain so that the sample
/// under that column is the ground the car is standing on. Two independent
/// copies of either number puts the car above or below its own terrain.
pub const VIEW_SPAN_WORLD: f32 = 34.0;
pub const CAR_SCREEN_X: f32 = 0.32;
/// Terrain samples uploaded per frame, packed 4-per-vec4 for uniform-array
/// alignment (a `array<f32>` in uniform space would stride at 16 bytes and
/// waste three quarters of the buffer).
pub const TERRAIN_SAMPLES: usize = 64;
/// Sentinel for "there is no ground in this column" — a Gap.
///
/// A sentinel rather than an Option because this crosses into a shader,
/// which has no such concept. Far enough below any real terrain that the
/// shader's `< NO_GROUND_TEST` check cannot collide with a deep-but-real
/// valley.
pub const NO_GROUND: f32 = -1.0e4;

/// The ground profile across the viewport, left edge to right edge.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct TerrainUniforms {
    pub heights: [[f32; 4]; TERRAIN_SAMPLES / 4],
}

impl TerrainUniforms {
    /// Sample `height_at` across the visible span.
    ///
    /// A Gap samples as [`NO_GROUND`], not as the previous height. The first
    /// version held the last height across gaps, on the reasoning that
    /// dropping to 0.0 drew a cliff — and a screenshot showed why that was
    /// the wrong fix: the pit rendered as a solid plateau you could see
    /// yourself driving onto, with the drop appearing at the FAR edge. The
    /// car then fell through ground that looked walkable. Hiding the hazard
    /// a level is built around is worse than any cliff artifact.
    ///
    /// `height_at` also returns None off the ends of the track, but it
    /// clamps to the track bounds first, so those come back as the end
    /// segment's height and never reach the None arm.
    pub fn sample(track: &road_warrior_core::Track, car_pos_x: f32) -> Self {
        let mut flat = [0.0f32; TERRAIN_SAMPLES];
        for (i, slot) in flat.iter_mut().enumerate() {
            let t = i as f32 / (TERRAIN_SAMPLES - 1) as f32;
            let world_x = car_pos_x + (t - CAR_SCREEN_X) * VIEW_SPAN_WORLD;
            *slot = track.height_at(world_x).unwrap_or(NO_GROUND);
        }
        let mut heights = [[0.0f32; 4]; TERRAIN_SAMPLES / 4];
        for (i, chunk) in flat.chunks(4).enumerate() {
            heights[i].copy_from_slice(chunk);
        }
        Self { heights }
    }
}

pub struct SceneResources {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    terrain_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl SceneResources {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/scene.wgsl").into()),
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene-uniforms"),
            size: std::mem::size_of::<SceneUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let terrain_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene-terrain"),
            size: std::mem::size_of::<TerrainUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene-bgl"),
            entries: &[uniform_entry(0), uniform_entry(1)],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene-bg"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: terrain_buffer.as_entire_binding(),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        Self { pipeline, uniform_buffer, terrain_buffer, bind_group }
    }
}

pub struct SceneCallback {
    pub uniforms: SceneUniforms,
    pub terrain: TerrainUniforms,
}

impl egui_wgpu::CallbackTrait for SceneCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let resources: &SceneResources = callback_resources
            .get()
            .expect("SceneResources must be inserted into callback_resources at app init");
        queue.write_buffer(&resources.uniform_buffer, 0, bytemuck::bytes_of(&self.uniforms));
        queue.write_buffer(&resources.terrain_buffer, 0, bytemuck::bytes_of(&self.terrain));
        Vec::new()
    }

    fn paint<'a>(
        &'a self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'a>,
        callback_resources: &'a egui_wgpu::CallbackResources,
    ) {
        let resources: &SceneResources = callback_resources.get().expect("SceneResources missing");
        render_pass.set_pipeline(&resources.pipeline);
        render_pass.set_bind_group(0, &resources.bind_group, &[]);
        render_pass.draw(0..3, 0..1); // fullscreen triangle
    }
}

#[cfg(test)]
mod terrain_tests {
    //! The sampling half of terrain rendering, tested without a GPU.
    //!
    //! The shader half needs a device and a screenshot; this half is
    //! arithmetic and is where an off-by-a-span error would hide. Dust
    //! Bowl's real shape — Flat 30, Ramp 8 rising to 5, Gap 10, Flat 40 —
    //! is used deliberately rather than a synthetic track, so the numbers
    //! mean something when they fail.

    use super::*;
    use road_warrior_core::{Segment, Track};

    fn dust_bowl() -> Track {
        Track::new(
            "Dust Bowl",
            vec![
                Segment::Flat { length: 30.0, height: 0.0 },
                Segment::Ramp { length: 8.0, from_height: 0.0, to_height: 5.0 },
                Segment::Gap { length: 10.0 },
                Segment::Flat { length: 40.0, height: 0.0 },
            ],
        )
    }

    fn flat(t: &TerrainUniforms) -> Vec<f32> {
        t.heights.iter().flatten().copied().collect()
    }

    #[test]
    fn the_sample_under_the_car_is_the_ground_the_car_stands_on() {
        // This is the alignment that puts the wheels on the terrain rather
        // than above or below it: the sample at CAR_SCREEN_X must be
        // height_at(car_pos_x).
        let track = dust_bowl();
        for pos in [0.0f32, 12.0, 31.0, 33.0, 60.0] {
            let s = flat(&TerrainUniforms::sample(&track, pos));
            let idx = (CAR_SCREEN_X * (TERRAIN_SAMPLES - 1) as f32).round() as usize;
            let want = track.height_at(pos).unwrap_or(0.0);
            assert!(
                (s[idx] - want).abs() < 0.2,
                "at pos {pos}: sample {} vs height_at {want}",
                s[idx]
            );
        }
    }

    #[test]
    fn the_ramp_shows_up_before_the_car_reaches_it() {
        // The whole point of drawing terrain: you can see what is coming.
        // The ramp starts at x=30 and the view reaches
        // (1 - CAR_SCREEN_X) * VIEW_SPAN_WORLD ahead of the car.
        let track = dust_bowl();
        let lead = (1.0 - CAR_SCREEN_X) * VIEW_SPAN_WORLD;
        assert!(lead > 20.0, "the viewport should see well ahead; got {lead}");

        let early = flat(&TerrainUniforms::sample(&track, 5.0));
        assert!(
            early.iter().all(|h| h.abs() < 0.01),
            "at x=5 nothing but flat ground is in view yet"
        );

        let approaching = flat(&TerrainUniforms::sample(&track, 20.0));
        let peak = approaching.iter().cloned().fold(f32::MIN, f32::max);
        assert!(peak > 1.0, "at x=20 the ramp should be visible ahead; peak {peak}");
        // The rise must be AHEAD of the car, not behind. Comparing the last
        // sample to the first no longer works now that gaps sample as
        // NO_GROUND: at x=20 the right edge of the view is inside the pit,
        // so the last sample is the sentinel. Compare the peak's position
        // instead, which is what "ahead" actually means.
        let peak_idx = approaching
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        let car_idx = (CAR_SCREEN_X * (TERRAIN_SAMPLES - 1) as f32).round() as usize;
        assert!(peak_idx > car_idx, "the ramp should be ahead of the car, not behind it");
    }

    #[test]
    fn a_gap_samples_as_no_ground_so_the_pit_is_visible() {
        // The reverse of what this test asserted first. Holding the last
        // height across a gap drew the pit as a solid plateau: you watched
        // the car drive onto ground that was not there, with the drop
        // appearing at the far edge. A screenshot caught it.
        let track = dust_bowl();
        assert_eq!(track.height_at(42.0), None, "x=42 is inside the gap");
        let s = flat(&TerrainUniforms::sample(&track, 42.0));
        let idx = (CAR_SCREEN_X * (TERRAIN_SAMPLES - 1) as f32).round() as usize;
        assert!(s[idx] < -100.0, "the gap must sample as NO_GROUND, got {}", s[idx]);
    }

    #[test]
    fn the_pit_edges_line_up_with_the_gap_boundaries() {
        // The near edge is what the player has to judge the jump against,
        // so it has to be at x=38 and not somewhere else.
        let track = dust_bowl();
        let car = 38.0; // standing right at the lip
        let s = flat(&TerrainUniforms::sample(&track, car));
        for (i, h) in s.iter().enumerate() {
            let t = i as f32 / (TERRAIN_SAMPLES - 1) as f32;
            let world_x = car + (t - CAR_SCREEN_X) * VIEW_SPAN_WORLD;
            let in_gap = (38.0..48.0).contains(&world_x);
            assert_eq!(
                *h < -100.0,
                in_gap,
                "sample {i} at world x {world_x:.1}: gap={in_gap} but height={h}"
            );
        }
    }

    #[test]
    fn before_the_start_line_reads_as_the_first_segment() {
        // Sampling starts CAR_SCREEN_X * span BEHIND the car, which at x=0
        // is off the front of the track. `height_at` clamps to the track
        // bounds, so this comes back as the first segment's height rather
        // than as None — worth pinning, because an earlier version of this
        // test passed while believing it was exercising the None path.
        let track = dust_bowl();
        assert_eq!(track.height_at(-5.0), Some(0.0), "height_at clamps, it does not return None");
        let s = flat(&TerrainUniforms::sample(&track, 0.0));
        assert!(s[0].abs() < 0.01, "behind the start it matches the first flat segment");
    }

    #[test]
    fn the_packing_matches_what_the_shader_indexes() {
        // The shader reads heights[i / 4][i % 4]; if the CPU packed in a
        // different order the terrain would come out shuffled into blocks
        // of four.
        let track = Track::new(
            "Ascending",
            vec![Segment::Ramp { length: 100.0, from_height: 0.0, to_height: 50.0 }],
        );
        let s = flat(&TerrainUniforms::sample(&track, 50.0));
        for w in s.windows(2) {
            assert!(w[1] >= w[0] - 1e-3, "a monotonic ramp must sample monotonically: {s:?}");
        }
    }
}
