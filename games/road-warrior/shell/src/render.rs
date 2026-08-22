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

pub struct SceneResources {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
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

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene-bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
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

        Self { pipeline, uniform_buffer, bind_group }
    }
}

pub struct SceneCallback {
    pub uniforms: SceneUniforms,
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
