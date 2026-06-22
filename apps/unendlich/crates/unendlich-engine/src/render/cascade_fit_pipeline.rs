//! Cascade-fit compute pipeline (v0.24.2b).
//!
//! Owns the Vulkan resources for `cascade_fit.comp`: descriptor set
//! layout (1 binding, the ViewParams SSBO), pipeline layout (push
//! constants describing camera + sun + cascade index + splits),
//! pipeline, descriptor set.
//!
//! Per frame, dispatched once per cascade (4 dispatches/frame in
//! v0.24.2b). Each dispatch reads the push constants, computes the
//! light-space view + ortho-projection for that cascade's near/far
//! slice, and writes the matrices into `view_params[cascade_index]`.
//!
//! Why a separate pipeline (not part of view_compute)
//!
//! cascade_fit computes the inputs (view, projection) that
//! view_compute later consumes to derive view_proj/inverse/planes.
//! Two passes with a barrier in between keeps the data-flow obvious.
//! Sharing a pipeline would mean a branch on push-constant content
//! inside the shader to decide which computation to run; not worth
//! the cleverness at this tiny scale.
//!
//! Cost: 4 one-invocation dispatches per frame plus one barrier.
//! Negligible.

#![cfg(feature = "render-core")]

use ash::{vk, Device};
use std::ffi::CStr;
use std::path::Path;

const ENTRY_NAME: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };

/// Push constants for `cascade_fit.comp`. Layout matches the GLSL
/// `PC` block exactly — 6 vec4s = 96 bytes.
///
/// All vec3 fields are followed by a single f32 (or f32-bit-cast u32)
/// to fill the 16-byte alignment GLSL requires for std430-style push
/// constants.
///
/// # Multi-camera notes
///
/// `splits_idx.w` carries the *destination slot* (not "cascade index"
/// in a global sense). For the PRIMARY ViewGroup's C0 the slot is 1;
/// for a hypothetical second ViewGroup's C0 it'd be 6 (or wherever
/// that group's cascade_slots[0] resolves to). The shader is agnostic
/// to ViewGroup identity — it just writes view+projection matrices
/// into whatever slot the caller specifies.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CascadeFitPushConstants {
    /// xyz = camera world-space position, w = fov_y in radians.
    pub cam_pos_fov: [f32; 4],
    /// xyz = camera forward (unit), w = aspect (w/h).
    pub cam_forward_asp: [f32; 4],
    /// xyz = camera up (unit), w = unused.
    pub cam_up_pad: [f32; 4],
    /// xyz = camera right (unit), w = unused.
    pub cam_right_pad: [f32; 4],
    /// xyz = sun direction (unit, ground-bound), w = unused.
    pub sun_dir_pad: [f32; 4],
    /// x = near_meters, y = far_meters, z = shadow_map_size (float),
    /// w = destination slot (bit-cast u32 → f32 via `f32::from_bits`).
    ///
    /// The slot is in PRIMARY ViewGroup's range for single-player
    /// (slots 1..=4 for the four cascades). Caller resolves the slot
    /// from a `ViewGroup` rather than hard-coding it.
    pub splits_idx: [f32; 4],
}

const _: [(); 96] = [(); std::mem::size_of::<CascadeFitPushConstants>()];

impl CascadeFitPushConstants {
    /// Build push constants for cascade `cascade_index` (0..NUM_CASCADES-1)
    /// of `view_group`. Resolves the destination slot from the
    /// ViewGroup so call sites don't reach for `VIEW_SLOT_SHADOW_C*`
    /// constants directly. Multi-camera-friendly.
    pub fn for_cascade(
        view_group: &crate::render::view_params::ViewGroup,
        cascade_index: usize,
        cam_pos: [f32; 3],
        cam_forward: [f32; 3],
        cam_up: [f32; 3],
        cam_right: [f32; 3],
        fov_y_radians: f32,
        aspect: f32,
        sun_dir: [f32; 3],
        near_meters: f32,
        far_meters: f32,
        shadow_map_size: f32,
    ) -> Self {
        debug_assert!(cascade_index < view_group.cascade_slots.len());
        let dest_slot = view_group.cascade_slots[cascade_index];
        Self {
            cam_pos_fov:    [cam_pos[0], cam_pos[1], cam_pos[2], fov_y_radians],
            cam_forward_asp:[cam_forward[0], cam_forward[1], cam_forward[2], aspect],
            cam_up_pad:     [cam_up[0], cam_up[1], cam_up[2], 0.0],
            cam_right_pad:  [cam_right[0], cam_right[1], cam_right[2], 0.0],
            sun_dir_pad:    [sun_dir[0], sun_dir[1], sun_dir[2], 0.0],
            splits_idx:     [near_meters, far_meters, shadow_map_size,
                             f32::from_bits(dest_slot)],
        }
    }
}

#[derive(Debug)]
pub enum CascadeFitError {
    Vulkan(vk::Result),
    ShaderMissing(String),
    Io(std::io::Error),
}
impl From<vk::Result> for CascadeFitError {
    fn from(r: vk::Result) -> Self { Self::Vulkan(r) }
}
impl From<std::io::Error> for CascadeFitError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}
impl std::fmt::Display for CascadeFitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vulkan(r) => write!(f, "Vulkan error: {:?}", r),
            Self::ShaderMissing(p) => write!(f, "Shader missing: {}", p),
            Self::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}
impl std::error::Error for CascadeFitError {}

pub struct CascadeFitPipeline {
    pub descriptor_set_layout: vk::DescriptorSetLayout,
    pub pipeline_layout: vk::PipelineLayout,
    pub pipeline: vk::Pipeline,
    pub descriptor_pool: vk::DescriptorPool,
    pub descriptor_set: vk::DescriptorSet,

    device: Device,
}

unsafe impl Send for CascadeFitPipeline {}

impl CascadeFitPipeline {
    pub unsafe fn new(
        device: &Device,
        view_params_buffer: vk::Buffer,
    ) -> Result<Self, CascadeFitError> {
        let descriptor_set_layout = create_descriptor_set_layout(device)?;
        let pipeline_layout = create_pipeline_layout(device, descriptor_set_layout)?;
        let pipeline = create_compute_pipeline(device, pipeline_layout)?;
        let (descriptor_pool, descriptor_set) =
            allocate_descriptor_set(device, descriptor_set_layout)?;
        update_descriptor_set(device, descriptor_set, view_params_buffer);

        Ok(Self {
            descriptor_set_layout,
            pipeline_layout,
            pipeline,
            descriptor_pool,
            descriptor_set,
            device: device.clone(),
        })
    }

    /// Record one cascade-fit dispatch. Caller is responsible for the
    /// shader-write → shader-read barrier on the ViewParams buffer
    /// between this dispatch and any subsequent reader.
    pub unsafe fn record_dispatch(
        &self,
        cmd: vk::CommandBuffer,
        push: &CascadeFitPushConstants,
    ) {
        self.device.cmd_bind_pipeline(
            cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline,
        );
        self.device.cmd_bind_descriptor_sets(
            cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline_layout,
            0, std::slice::from_ref(&self.descriptor_set), &[],
        );
        let bytes = std::slice::from_raw_parts(
            push as *const _ as *const u8,
            std::mem::size_of::<CascadeFitPushConstants>(),
        );
        self.device.cmd_push_constants(
            cmd, self.pipeline_layout,
            vk::ShaderStageFlags::COMPUTE, 0, bytes,
        );
        self.device.cmd_dispatch(cmd, 1, 1, 1);
    }

    pub unsafe fn destroy(&self) {
        let d = &self.device;
        d.destroy_pipeline(self.pipeline, None);
        d.destroy_pipeline_layout(self.pipeline_layout, None);
        d.destroy_descriptor_pool(self.descriptor_pool, None);
        d.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
    }
}

unsafe fn create_descriptor_set_layout(device: &Device) -> Result<vk::DescriptorSetLayout, CascadeFitError> {
    let bindings = [
        vk::DescriptorSetLayoutBinding::builder()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),
    ];
    let info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings);
    Ok(device.create_descriptor_set_layout(&info, None)?)
}

unsafe fn create_pipeline_layout(
    device: &Device, set_layout: vk::DescriptorSetLayout,
) -> Result<vk::PipelineLayout, CascadeFitError> {
    let pcr = vk::PushConstantRange::builder()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(std::mem::size_of::<CascadeFitPushConstants>() as u32);
    let info = vk::PipelineLayoutCreateInfo::builder()
        .set_layouts(std::slice::from_ref(&set_layout))
        .push_constant_ranges(std::slice::from_ref(&pcr));
    Ok(device.create_pipeline_layout(&info, None)?)
}

unsafe fn create_compute_pipeline(
    device: &Device, layout: vk::PipelineLayout,
) -> Result<vk::Pipeline, CascadeFitError> {
    let candidates = [
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/shaders/cascade_fit.comp.spv"),
        "assets/shaders/cascade_fit.comp.spv",
        "src/shaders/cascade_fit.comp.spv",
    ];
    let mut bytes: Option<Vec<u8>> = None;
    for p in &candidates {
        if Path::new(p).exists() {
            bytes = Some(std::fs::read(p)?);
            break;
        }
    }
    let bytes = bytes.ok_or_else(|| CascadeFitError::ShaderMissing(candidates[0].to_string()))?;
    let words: Vec<u32> = bytes.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    let module_info = vk::ShaderModuleCreateInfo::builder().code(&words);
    let module = device.create_shader_module(&module_info, None)?;

    let stage = vk::PipelineShaderStageCreateInfo::builder()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module)
        .name(ENTRY_NAME);
    let info = vk::ComputePipelineCreateInfo::builder()
        .stage(stage.build())
        .layout(layout);
    let pipelines = device.create_compute_pipelines(
        vk::PipelineCache::null(), &[info.build()], None,
    ).map_err(|(_, r)| CascadeFitError::Vulkan(r))?;

    device.destroy_shader_module(module, None);

    Ok(pipelines[0])
}

unsafe fn allocate_descriptor_set(
    device: &Device, layout: vk::DescriptorSetLayout,
) -> Result<(vk::DescriptorPool, vk::DescriptorSet), CascadeFitError> {
    let sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 1,
    }];
    let info = vk::DescriptorPoolCreateInfo::builder()
        .pool_sizes(&sizes).max_sets(1);
    let pool = device.create_descriptor_pool(&info, None)?;
    let alloc = vk::DescriptorSetAllocateInfo::builder()
        .descriptor_pool(pool)
        .set_layouts(std::slice::from_ref(&layout));
    let sets = device.allocate_descriptor_sets(&alloc)?;
    Ok((pool, sets[0]))
}

unsafe fn update_descriptor_set(
    device: &Device, set: vk::DescriptorSet, view_params: vk::Buffer,
) {
    let v_info = vk::DescriptorBufferInfo::builder()
        .buffer(view_params).range(vk::WHOLE_SIZE).build();
    let writes = [
        vk::WriteDescriptorSet::builder()
            .dst_set(set).dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&v_info)).build(),
    ];
    device.update_descriptor_sets(&writes, &[]);
}

/// CPU reference implementation of `cascade_fit.comp`.
///
/// Mirrors the GLSL math line-for-line so unit tests can compare GLSL
/// output (predicted by this function) to the canonical CPU
/// `single_cascade` in `src/render/light.rs`. If both produce the
/// same view+projection matrices, the GLSL implementation is correct.
///
/// Inputs map 1:1 to the GLSL push-constant struct. Output is
/// (`view`, `projection`) as column-major `[[f32; 4]; 4]`.
pub fn cascade_fit_cpu_reference(
    cam_pos:     [f32; 3],
    cam_forward: [f32; 3],
    cam_up:      [f32; 3],
    cam_right:   [f32; 3],
    fov_y:       f32,
    aspect:      f32,
    sun_dir:     [f32; 3],
    near_m:      f32,
    far_m:       f32,
    shadow_size: f32,
) -> ([[f32; 4]; 4], [[f32; 4]; 4]) {
    // Vec3 helpers — minimal so this file doesn't depend on glm.
    fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0]+b[0], a[1]+b[1], a[2]+b[2]] }
    fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
    fn scale(a: [f32; 3], s: f32) -> [f32; 3] { [a[0]*s, a[1]*s, a[2]*s] }
    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { a[0]*b[0] + a[1]*b[1] + a[2]*b[2] }
    fn length(a: [f32; 3]) -> f32 { dot(a, a).sqrt() }
    fn normalize(a: [f32; 3]) -> [f32; 3] {
        let l = length(a);
        if l > 1e-12 { [a[0]/l, a[1]/l, a[2]/l] } else { a }
    }
    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[1]*b[2] - a[2]*b[1], a[2]*b[0] - a[0]*b[2], a[0]*b[1] - a[1]*b[0]]
    }

    // Step 1: 8 frustum-slice corners in world space.
    let half_v_near = (fov_y * 0.5).tan() * near_m;
    let half_h_near = half_v_near * aspect;
    let half_v_far  = (fov_y * 0.5).tan() * far_m;
    let half_h_far  = half_v_far  * aspect;
    let near_center = add(cam_pos, scale(cam_forward, near_m));
    let far_center  = add(cam_pos, scale(cam_forward, far_m));
    let corners = [
        sub(sub(near_center, scale(cam_right, half_h_near)), scale(cam_up, half_v_near)),
        sub(add(near_center, scale(cam_right, half_h_near)), scale(cam_up, half_v_near)),
        add(sub(near_center, scale(cam_right, half_h_near)), scale(cam_up, half_v_near)),
        add(add(near_center, scale(cam_right, half_h_near)), scale(cam_up, half_v_near)),
        sub(sub(far_center,  scale(cam_right, half_h_far)),  scale(cam_up, half_v_far)),
        sub(add(far_center,  scale(cam_right, half_h_far)),  scale(cam_up, half_v_far)),
        add(sub(far_center,  scale(cam_right, half_h_far)),  scale(cam_up, half_v_far)),
        add(add(far_center,  scale(cam_right, half_h_far)),  scale(cam_up, half_v_far)),
    ];

    // Step 2: world_up branch.
    let sun_forward = normalize(sun_dir);
    let world_up = if sun_forward[1].abs() > 0.99 { [0.0, 0.0, 1.0] } else { [0.0, 1.0, 0.0] };

    // Step 3: frustum center & AABB diagonal.
    let mut center = [0.0f32; 3];
    for c in &corners {
        center[0] += c[0]; center[1] += c[1]; center[2] += c[2];
    }
    center[0] /= 8.0; center[1] /= 8.0; center[2] /= 8.0;
    let mut aabb_min = corners[0];
    let mut aabb_max = corners[0];
    for c in &corners {
        aabb_min[0] = aabb_min[0].min(c[0]); aabb_max[0] = aabb_max[0].max(c[0]);
        aabb_min[1] = aabb_min[1].min(c[1]); aabb_max[1] = aabb_max[1].max(c[1]);
        aabb_min[2] = aabb_min[2].min(c[2]); aabb_max[2] = aabb_max[2].max(c[2]);
    }
    let aabb_diag = length(sub(aabb_max, aabb_min));

    // Step 4: light view (look_at_rh).
    let light_eye = sub(center, scale(sun_forward, aabb_diag));
    let f = normalize(sub(center, light_eye));
    let s = normalize(cross(f, world_up));
    let u = cross(s, f);

    // Column-major mat4 matching GLSL.
    let mut light_view = [[0.0f32; 4]; 4];
    light_view[0][0] = s[0]; light_view[1][0] = s[1]; light_view[2][0] = s[2];
    light_view[0][1] = u[0]; light_view[1][1] = u[1]; light_view[2][1] = u[2];
    light_view[0][2] = -f[0]; light_view[1][2] = -f[1]; light_view[2][2] = -f[2];
    light_view[3][0] = -dot(s, light_eye);
    light_view[3][1] = -dot(u, light_eye);
    light_view[3][2] =  dot(f, light_eye);
    light_view[3][3] = 1.0;

    // Step 5: bounding-sphere radius, ceil to 1/16.
    let mut radius = 0.0f32;
    for c in &corners {
        let d = length(sub(*c, center));
        if d > radius { radius = d; }
    }
    radius = (radius * 16.0).ceil() / 16.0;

    // Step 6: sphere center in light space.
    // light_view * vec4(center, 1.0). Column-major: result[i] = sum_k light_view[k][i] * v[k].
    let v = [center[0], center[1], center[2], 1.0];
    let center_ls = [
        light_view[0][0]*v[0] + light_view[1][0]*v[1] + light_view[2][0]*v[2] + light_view[3][0]*v[3],
        light_view[0][1]*v[0] + light_view[1][1]*v[1] + light_view[2][1]*v[2] + light_view[3][1]*v[3],
        light_view[0][2]*v[0] + light_view[1][2]*v[1] + light_view[2][2]*v[2] + light_view[3][2]*v[3],
    ];

    // Step 7: sphere bounds in light space.
    let mut min_ls = [center_ls[0] - radius, center_ls[1] - radius, center_ls[2] - radius];
    let mut max_ls = [center_ls[0] + radius, center_ls[1] + radius, center_ls[2] + radius];

    // Step 8: ortho near/far with margin.
    let z_near = -max_ls[2] - 50.0;
    let z_far  = -min_ls[2] + 50.0;

    // Step 9: texel-snap X/Y.
    let texel = (2.0 * radius) / shadow_size;
    if texel > 0.0 {
        min_ls[0] = (min_ls[0] / texel).floor() * texel;
        min_ls[1] = (min_ls[1] / texel).floor() * texel;
        max_ls[0] = min_ls[0] + 2.0 * radius;
        max_ls[1] = min_ls[1] + 2.0 * radius;
    }

    // Step 10: ortho_rh_zo.
    let mut ortho = [[0.0f32; 4]; 4];
    ortho[0][0] =  2.0 / (max_ls[0] - min_ls[0]);
    ortho[1][1] =  2.0 / (max_ls[1] - min_ls[1]);
    ortho[2][2] = -1.0 / (z_far - z_near);
    ortho[3][0] = -(max_ls[0] + min_ls[0]) / (max_ls[0] - min_ls[0]);
    ortho[3][1] = -(max_ls[1] + min_ls[1]) / (max_ls[1] - min_ls[1]);
    ortho[3][2] = -z_near / (z_far - z_near);
    ortho[3][3] = 1.0;

    (light_view, ortho)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CascadeFitPushConstants must be exactly 96 bytes — six vec4s.
    /// The GLSL shader's push-constant block depends on this layout;
    /// any field added would require updating both sides together.
    #[test]
    fn push_constants_are_exactly_96_bytes() {
        assert_eq!(std::mem::size_of::<CascadeFitPushConstants>(), 96);
    }

    /// Field-by-field offset check, mirroring the GLSL push-constant
    /// block layout. If any field's offset shifts the GPU reads the
    /// wrong data — silently — and shadows go subtly wrong.
    #[test]
    fn push_constants_field_offsets() {
        let p = CascadeFitPushConstants {
            cam_pos_fov:    [0.0; 4],
            cam_forward_asp:[0.0; 4],
            cam_up_pad:     [0.0; 4],
            cam_right_pad:  [0.0; 4],
            sun_dir_pad:    [0.0; 4],
            splits_idx:     [0.0; 4],
        };
        let base = (&p) as *const _ as usize;
        let f = (&p.cam_pos_fov)    as *const _ as usize;
        let g = (&p.cam_forward_asp) as *const _ as usize;
        let h = (&p.cam_up_pad)      as *const _ as usize;
        let i = (&p.cam_right_pad)   as *const _ as usize;
        let j = (&p.sun_dir_pad)     as *const _ as usize;
        let k = (&p.splits_idx)      as *const _ as usize;
        assert_eq!(f - base,  0);
        assert_eq!(g - base, 16);
        assert_eq!(h - base, 32);
        assert_eq!(i - base, 48);
        assert_eq!(j - base, 64);
        assert_eq!(k - base, 80);
    }

    /// Compare `cascade_fit_cpu_reference` (the literal GLSL port) to
    /// the canonical CPU `single_cascade` from `light.rs`. If they
    /// produce the same view * projection within fp tolerance, the
    /// GLSL math is correct (since the reference is a line-for-line
    /// port of the shader).
    ///
    /// We compare the COMPOSED view_proj rather than view and
    /// projection separately: small differences in how the two factor
    /// the same transform (which `look_at` corner is used as the eye,
    /// where the margin is added in the ortho near/far, etc.) shouldn't
    /// fail this test — only the final transform matters.
    #[test]
    fn cpu_reference_matches_single_cascade_for_typical_camera() {
        use crate::render::camera::View;
        use nalgebra_glm as glm;

        // A typical mid-game camera.
        let mut view = View::new(glm::vec3(100.0, 80.0, -50.0));
        view.yaw = 0.6;
        view.pitch = -0.2;

        let aspect = 16.0 / 9.0;
        let sun = glm::vec3(0.4, -0.8, 0.45).normalize();
        let near_m = 0.5;
        let far_m  = 200.0;
        let shadow_size: f32 = 2048.0;

        // CPU canonical reference.
        let canonical = crate::render::light::single_cascade(
            &view, sun, aspect, near_m, far_m, shadow_size,
        );
        let canonical_vp = canonical.view_proj;

        // CPU GLSL-port reference. Need to extract camera basis the way
        // the shader will receive it (which mirrors what we'll send in
        // push constants).
        let forward = view.forward();
        let right = view.right();
        let up = glm::cross(&right, &forward).normalize();

        let (ref_view, ref_proj) = cascade_fit_cpu_reference(
            [view.position.x, view.position.y, view.position.z],
            [forward.x, forward.y, forward.z],
            [up.x, up.y, up.z],
            [right.x, right.y, right.z],
            view.fov_y_radians,
            aspect,
            [sun.x, sun.y, sun.z],
            near_m, far_m, shadow_size,
        );

        // Compose ref_view_proj = ref_proj * ref_view (column-major).
        let mut ref_vp = [[0.0f32; 4]; 4];
        for col in 0..4 {
            for row in 0..4 {
                let mut s = 0.0;
                for k in 0..4 {
                    s += ref_proj[k][row] * ref_view[col][k];
                }
                ref_vp[col][row] = s;
            }
        }

        // Compare element-by-element. Canonical is glm::Mat4 (column-major).
        for col in 0..4 {
            for row in 0..4 {
                let canonical_v = canonical_vp[(row, col)];
                let ref_v = ref_vp[col][row];
                let d = (canonical_v - ref_v).abs();
                // Tolerance: the math has many large intermediate
                // quantities (radius up to ~120m, snapped to 1/16). A
                // few ulps per element. 1e-3 is generous but not
                // generous enough to hide a real bug.
                assert!(
                    d < 1e-3,
                    "mismatch at [{},{}]: canonical={}, glsl_ref={}, diff={}",
                    row, col, canonical_v, ref_v, d,
                );
            }
        }
    }
}
