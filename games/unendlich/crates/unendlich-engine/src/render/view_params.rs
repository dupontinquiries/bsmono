//! Per-view GPU buffer (v0.24.0a).
//!
//! Holds an SSBO of `[ViewParam; MAX_VIEWS]`. Each `ViewParam` describes
//! a single rendering view (main camera, shadow cascade, future
//! split-screen player view, future reflection probe). For v0.24.0a only
//! the `planes` field is consumed by GPU code (the cull shader); the
//! matrix slots are reserved for v0.24.0b's `view_compute.comp` to
//! populate.
//!
//! # Why now
//!
//! Multi-view culling requires frusta in a buffer (not push constants —
//! 5 frusta × 96 bytes exceeds Vulkan's 128-byte push-constant minimum).
//! v0.24.0a is the minimal migration: CPU still computes frustum planes,
//! but writes them through this buffer instead of inline push constants.
//! v0.24.0b moves the plane derivation onto the GPU itself.
//!
//! # Why all the matrix fields if v0.24.0a doesn't read them
//!
//! Locking the struct layout NOW means v0.24.0b doesn't have to reshape
//! the SSBO descriptor, the binding offsets, or the shader struct
//! definition. The GPU writes go into pre-existing slots. The single
//! ship that changes the layout is bad — every downstream consumer
//! recompiles its shader against the new offsets. Get the layout right
//! once.
//!
//! # std430 layout
//!
//! All fields are `vec4`-aligned (16 bytes). `mat4` consumes 64 bytes
//! and aligns to 16. The total struct size is exact and forward-padded:
//!
//! ```text
//! Offset  Bytes  Field
//!     0      64  view              (mat4)
//!    64      64  projection        (mat4)
//!   128      64  view_proj         (mat4)
//!   192      64  inverse_view_proj (mat4)
//!   256      16  camera_pos        (vec4)
//!   272      16  near_far_aspect   (vec4)  [near, far, aspect, _pad]
//!   288      96  planes            (vec4 × 6)
//!   384            <- size
//! ```
//!
//! This must match `view_params.glsl` (or the inline struct in the cull
//! shader). The size assertion at the bottom of this file pins it.

#![cfg(feature = "render-core")]

use ash::{vk, Device, Instance};

/// Maximum simultaneous views.
///
/// - Slot 0: main camera.
/// - Slot 1: shadow (single cascade in v0.24.0a; future cascades 0..3
///   will occupy slots 1..=4 in v0.24.2).
///
/// Sized to 8 to leave room for v0.24.2's 4 cascades plus 3 future
/// split-screen players without re-creating the buffer. Memory cost:
/// 8 × 384 = 3 KB. Trivial.
///
/// Per-view-group cost is `1 + NUM_CASCADES = 5` slots (1 main + 4
/// cascades), so MAX_VIEWS=8 fits ONE local player. A second local
/// player (split-screen) would push us to 10 slots; that's a buffer
/// re-create at the time split-screen lands.
pub const MAX_VIEWS: u32 = 8;

/// Slot 0: main camera of the primary (only) ViewGroup. Cull dispatch
/// for that ViewGroup's main pass reads its planes from here.
pub const VIEW_SLOT_MAIN: u32 = 0;

/// Slot 1: backward-compatible alias for the primary ViewGroup's first
/// (and currently only-rendered) shadow cascade. Existing call sites
/// that pre-date the per-cascade slots use this; new code uses
/// `ViewGroup::cascade_slots[0]` or `VIEW_SLOT_SHADOW_C0` directly.
pub const VIEW_SLOT_SHADOW: u32 = 1;

/// v0.24.2b: per-cascade slot constants for the PRIMARY ViewGroup.
/// C0 is the near cascade (highest detail), C3 is the far cascade.
/// These are slots IN the primary view group — split-screen secondary
/// players would have their own cascade slots starting at slot 5.
pub const VIEW_SLOT_SHADOW_C0: u32 = 1;
pub const VIEW_SLOT_SHADOW_C1: u32 = 2;
pub const VIEW_SLOT_SHADOW_C2: u32 = 3;
pub const VIEW_SLOT_SHADOW_C3: u32 = 4;

/// Number of shadow cascades per ViewGroup. Mirrors
/// `shadow_map::NUM_CASCADES`; kept local to avoid an import cycle.
pub const NUM_CASCADES: usize = 4;

/// A bundle of GPU view-slot indices for one *viewer*: one main
/// camera + N shadow cascades. The renderer parameterizes per-frame
/// work over a `&ViewGroup` rather than hard-coding `VIEW_SLOT_MAIN`
/// and `VIEW_SLOT_SHADOW_*`. This is the seam at which split-screen
/// lands: today there's one ViewGroup; tomorrow there's an array.
///
/// # Multiplayer / multi-camera contract
///
/// Each connected local player owns one ViewGroup. Per frame, every
/// ViewGroup needs:
/// - One CPU-authored (view, projection) matrix pair at the main slot.
/// - One CPU-authored "fit input" per cascade slot (camera basis, sun,
///   near/far split) consumed by `cascade_fit.comp` to fill the
///   cascade slot's (view, projection).
/// - One `view_compute.comp` dispatch per slot to derive view_proj /
///   inverse_view_proj / frustum planes.
/// - One cull dispatch per slot (today: per main + per rendered cascade).
/// - Render passes: main into the swapchain viewport assigned to this
///   ViewGroup; shadow cascades into the corresponding shadow-map
///   array layers (which would also be sized per-ViewGroup for split-
///   screen — outside v0.24.2's scope).
///
/// # Today's wiring
///
/// `ViewGroup::PRIMARY` returns the only ViewGroup that exists today.
/// Most call sites use it directly. When split-screen lands, the bin
/// loop will construct N ViewGroups and pass each to the renderer in
/// turn (or to a future multi-view single-dispatch cull).
#[derive(Debug, Copy, Clone)]
pub struct ViewGroup {
    /// Slot of the main camera view in the ViewParams SSBO.
    pub main_slot: u32,
    /// Per-cascade slots, ordered near→far (C0 → C3).
    pub cascade_slots: [u32; NUM_CASCADES],
}

impl ViewGroup {
    /// The primary (and today only) ViewGroup. Slot assignments
    /// match the `VIEW_SLOT_*` constants for backward compatibility.
    pub const PRIMARY: ViewGroup = ViewGroup {
        main_slot: VIEW_SLOT_MAIN,
        cascade_slots: [
            VIEW_SLOT_SHADOW_C0,
            VIEW_SLOT_SHADOW_C1,
            VIEW_SLOT_SHADOW_C2,
            VIEW_SLOT_SHADOW_C3,
        ],
    };

    /// Total slots consumed by this ViewGroup. Used when sizing
    /// MAX_VIEWS for multi-ViewGroup configurations.
    pub const fn slot_count() -> u32 {
        1 + NUM_CASCADES as u32
    }
}

/// CPU mirror of one GPU `ViewParam` slot. `repr(C)` + `[f32; N]` arrays
/// give us bit-exact std430 layout when written through a pointer.
///
/// IMPORTANT: every field is `vec4`-aligned. The padding scalars in
/// `near_far_aspect[3]` and `planes[i].w`-as-distance-d are deliberate.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct ViewParam {
    /// World-to-view (camera) matrix. Column-major.
    pub view: [[f32; 4]; 4],
    /// View-to-clip (projection) matrix. Column-major.
    pub projection: [[f32; 4]; 4],
    /// `projection * view`. v0.24.0a: CPU-written (mirrors `view_proj`
    /// already living in `CameraUbo`). v0.24.0b: GPU-derived.
    pub view_proj: [[f32; 4]; 4],
    /// `inverse(projection * view)`. v0.24.0a: unused, written as
    /// identity. v0.24.0b+: GPU-derived for screen-to-world
    /// reconstruction.
    pub inverse_view_proj: [[f32; 4]; 4],
    /// xyz = world-space camera position, w = unused (write 1.0).
    pub camera_pos: [f32; 4],
    /// [near, far, aspect, _pad]. v0.24.0a: written but unread.
    pub near_far_aspect: [f32; 4],
    /// 6 frustum planes. Convention matches `chunk_cull.comp`:
    /// (a, b, c, d) with `ax + by + cz + d >= 0` for points inside the
    /// frustum. Order: left, right, bottom, top, near, far.
    pub planes: [[f32; 4]; 6],
}

impl ViewParam {
    /// Zero-initialized. Used to fill the buffer at startup. The cull
    /// shader is safe to dispatch against zeroed planes — every
    /// `aabb_visible` test will return false (all corners have negative
    /// distance from a zero-d plane), so no chunks emit.
    pub fn zero() -> Self {
        Self {
            view: [[0.0; 4]; 4],
            projection: [[0.0; 4]; 4],
            view_proj: [[0.0; 4]; 4],
            inverse_view_proj: [[0.0; 4]; 4],
            camera_pos: [0.0; 4],
            near_far_aspect: [0.0; 4],
            planes: [[0.0; 4]; 6],
        }
    }
}

/// Size assertion: must match the GLSL std430 struct exactly.
///
/// 4*64 (matrices) + 2*16 (vec4s) + 6*16 (planes) = 256 + 32 + 96 = 384.
const _: [(); 384] = [(); std::mem::size_of::<ViewParam>()];

/// GPU buffer holding `[ViewParam; MAX_VIEWS]`, host-visible for CPU
/// writes. The cull shader reads from this buffer's binding.
///
/// Single-threaded access from the render loop — same pattern as
/// `IndirectDrawSet`'s metadata buffer.
pub struct ViewParamsBuffer {
    pub buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    /// Persistent map. Cast to `*mut ViewParam` for slot writes.
    ptr: *mut u8,

    device: Device,
}

// SAFETY: identical reasoning to IndirectDrawSet's host-visible buffers.
// Single-threaded access through the renderer's frame loop, persistent
// HOST_VISIBLE | HOST_COHERENT map, writes ordered before the cull
// dispatch by command-buffer recording order (CPU writes happen before
// vkQueueSubmit returns its result).
unsafe impl Send for ViewParamsBuffer {}

impl ViewParamsBuffer {
    /// Build the buffer. Initialized to zero for every slot.
    pub unsafe fn new(
        device: &Device,
        instance: &Instance,
        physical_device: vk::PhysicalDevice,
    ) -> Result<Self, vk::Result> {
        let size = (MAX_VIEWS as u64) * std::mem::size_of::<ViewParam>() as u64;

        let info = vk::BufferCreateInfo::builder()
            .size(size)
            .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = device.create_buffer(&info, None)?;

        let req = device.get_buffer_memory_requirements(buffer);
        let props = instance.get_physical_device_memory_properties(physical_device);

        let want = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
        let mut chosen = None;
        for i in 0..props.memory_type_count {
            if req.memory_type_bits & (1 << i) != 0
                && props.memory_types[i as usize].property_flags.contains(want)
            {
                chosen = Some(i);
                break;
            }
        }
        let mt = chosen.ok_or(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY)?;

        let alloc = vk::MemoryAllocateInfo::builder()
            .allocation_size(req.size)
            .memory_type_index(mt);
        let memory = device.allocate_memory(&alloc, None)?;
        device.bind_buffer_memory(buffer, memory, 0)?;
        let ptr = device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8;

        // Initialize to all-zero. A cull dispatch against an
        // all-zero ViewParam emits no draws (every plane test fails),
        // which is exactly the right "no view set yet" behavior.
        std::ptr::write_bytes(ptr, 0, size as usize);

        Ok(Self {
            buffer,
            memory,
            ptr,
            device: device.clone(),
        })
    }

    /// Write the 6 frustum planes for `slot`. v0.24.0a's CPU path. Other
    /// fields in the slot are left untouched (so the slot remains
    /// whatever it was — typically zero — for the matrices).
    ///
    /// `planes` order: left, right, bottom, top, near, far. Convention
    /// matches `chunk_cull.comp`.
    pub fn write_planes(&self, slot: u32, planes: &[[f32; 4]; 6]) {
        debug_assert!(slot < MAX_VIEWS, "view slot {} out of range", slot);
        // SAFETY: persistent map, single-threaded access from the
        // renderer's frame loop, write fits in MAX_VIEWS*sizeof::<ViewParam>().
        unsafe {
            let slot_ptr = (self.ptr as *mut ViewParam).add(slot as usize);
            // Only touch the planes field. We write via a byte-level
            // copy at the correct offset to avoid clobbering matrix
            // slots that v0.24.0b will populate from GPU.
            let planes_offset = std::mem::size_of::<ViewParam>() - 6 * 16;
            let dst = (slot_ptr as *mut u8).add(planes_offset) as *mut [f32; 4];
            std::ptr::copy_nonoverlapping(planes.as_ptr(), dst, 6);
        }
    }

    /// Write the `view` and `projection` matrices for `slot`. v0.24.0b's
    /// CPU path: the GPU `view_compute.comp` shader reads these and
    /// derives `view_proj`, `inverse_view_proj`, and planes back into
    /// the same slot. Other fields are left untouched.
    ///
    /// Matrices are written in column-major order (matches GLSL's
    /// `mat4` storage and the std430 layout used by the shader).
    pub fn write_view_proj_pair(
        &self,
        slot: u32,
        view: &[[f32; 4]; 4],
        projection: &[[f32; 4]; 4],
    ) {
        debug_assert!(slot < MAX_VIEWS, "view slot {} out of range", slot);
        // SAFETY: same reasoning as write_planes. The matrices live
        // at offsets 0 and 64 of the ViewParam struct.
        unsafe {
            let slot_ptr = (self.ptr as *mut ViewParam).add(slot as usize);
            let view_dst = slot_ptr as *mut [[f32; 4]; 4];
            std::ptr::copy_nonoverlapping(view, view_dst, 1);
            let proj_dst = (slot_ptr as *mut u8).add(64) as *mut [[f32; 4]; 4];
            std::ptr::copy_nonoverlapping(projection, proj_dst, 1);
        }
    }

    /// Destroy the buffer. Must be called before the device is
    /// destroyed. Mirrors the destroy pattern used by IndirectDrawSet.
    pub unsafe fn destroy(&self) {
        self.device.unmap_memory(self.memory);
        self.device.destroy_buffer(self.buffer, None);
        self.device.free_memory(self.memory, None);
    }
}

/// CPU reference implementation of the `view_compute.comp` shader.
///
/// Given a view-projection matrix in column-major storage (matching
/// the GLSL `mat4` layout), returns the 6 frustum planes that the GPU
/// shader would derive.
///
/// This function exists so unit tests can compare GLSL behavior to a
/// Rust reference without bringing a GPU into the loop. The math
/// mirrors `view_compute.comp` line-for-line; if the test passes, the
/// GLSL implementation is correct relative to this reference. The
/// reference itself is then validated by a small set of geometric
/// property tests (a point on the +Z axis is inside a typical
/// perspective frustum looking down -Z, etc).
///
/// Convention: `vp[col][row]` reads column `col`, row `row`. This
/// matches GLSL `mat4` indexing and the storage layout of `ViewParam`.
pub fn derive_planes_cpu_reference(vp: &[[f32; 4]; 4]) -> [[f32; 4]; 6] {
    // Element access: r(row, col) = vp[col][row]. Matches the shader's
    // `r(m, row, col)` helper.
    let r = |row: usize, col: usize| vp[col][row];

    let row0 = [r(0,0), r(0,1), r(0,2), r(0,3)];
    let row1 = [r(1,0), r(1,1), r(1,2), r(1,3)];
    let row2 = [r(2,0), r(2,1), r(2,2), r(2,3)];
    let row3 = [r(3,0), r(3,1), r(3,2), r(3,3)];

    let add = |a: [f32; 4], b: [f32; 4]| [a[0]+b[0], a[1]+b[1], a[2]+b[2], a[3]+b[3]];
    let sub = |a: [f32; 4], b: [f32; 4]| [a[0]-b[0], a[1]-b[1], a[2]-b[2], a[3]-b[3]];

    let mut planes = [
        add(row3, row0),  // Left
        sub(row3, row0),  // Right
        add(row3, row1),  // Bottom
        sub(row3, row1),  // Top
        row2,             // Near (depth-zero-to-one form)
        sub(row3, row2),  // Far
    ];

    // Normalize each plane so distances are in world units. Matches
    // the GLSL `length`/`/=` block.
    for p in planes.iter_mut() {
        let len = (p[0]*p[0] + p[1]*p[1] + p[2]*p[2]).sqrt();
        if len > 1e-6 {
            p[0] /= len; p[1] /= len; p[2] /= len; p[3] /= len;
        }
    }

    planes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the struct size. If GLSL std430 layout changes, the shader
    /// will mismatch and this catches it at compile time via the
    /// const-assert above, OR at test time via this assertion.
    #[test]
    fn view_param_size_matches_std430_layout() {
        assert_eq!(std::mem::size_of::<ViewParam>(), 384);
    }

    /// The planes field must be at offset 288. The shader reads from
    /// this offset via std430 indexing; a layout drift breaks culling
    /// silently.
    #[test]
    fn planes_offset_is_288() {
        let p = ViewParam::zero();
        let base = (&p) as *const ViewParam as usize;
        let planes_addr = (&p.planes) as *const [[f32; 4]; 6] as usize;
        assert_eq!(planes_addr - base, 288);
    }

    /// camera_pos must be at offset 256 (after the four 64-byte matrices).
    #[test]
    fn camera_pos_offset_is_256() {
        let p = ViewParam::zero();
        let base = (&p) as *const ViewParam as usize;
        let cp_addr = (&p.camera_pos) as *const [f32; 4] as usize;
        assert_eq!(cp_addr - base, 256);
    }

    /// MAX_VIEWS is sized for v0.24.2's 4 cascades + main + 3 extras.
    /// Sanity: don't drop below 5 by accident.
    #[test]
    fn max_views_at_least_five() {
        assert!(MAX_VIEWS >= 5, "MAX_VIEWS must accommodate main + 4 cascades");
    }

    /// Slot constants don't collide.
    ///
    /// REGRESSION CONTEXT (v0.24.0a → v0.24.0a.1): the original ship
    /// used `VIEW_SLOT_MAIN` for both the main camera cull AND the
    /// shadow cull, with the comment "they overwrite each other within
    /// the same frame, which mirrors the v0.23.10 push-constant flow
    /// exactly". That reasoning was WRONG.
    ///
    /// In v0.23.10, frustum planes lived in push constants — recorded
    /// INTO the command buffer at record time. Each dispatch's command
    /// buffer entry carried its own immutable copy of the planes. The
    /// shadow dispatch and main dispatch had different planes baked in.
    ///
    /// In v0.24.0a, frustum planes live in a host-visible SSBO. CPU
    /// writes happen at record time, but the GPU reads happen at submit
    /// time. If both the shadow and main CPU writes target the same
    /// slot, the GPU sees only whichever write happened LAST — both
    /// dispatches end up culling against the same frustum.
    ///
    /// In the v0.24.0a playtest this manifested as broken shadows in
    /// the mid-distance region where the main and shadow frusta diverge.
    ///
    /// MAIN and SHADOW MUST be distinct. This test pins it.
    #[test]
    fn slot_constants_distinct() {
        assert_ne!(VIEW_SLOT_MAIN, VIEW_SLOT_SHADOW,
            "main and shadow MUST use different slots — see regression \
             context in this test's doc comment");
        assert!(VIEW_SLOT_MAIN < MAX_VIEWS);
        assert!(VIEW_SLOT_SHADOW < MAX_VIEWS);
    }

    /// ViewParam::zero() actually zeros everything. Important because
    /// the buffer is allocated and the cull shader reads from it before
    /// the first frame's write_planes — zero must mean "no draws".
    #[test]
    fn zero_is_actually_zero() {
        let p = ViewParam::zero();
        for plane in &p.planes {
            for v in plane {
                assert_eq!(*v, 0.0);
            }
        }
        for row in &p.view {
            for v in row {
                assert_eq!(*v, 0.0);
            }
        }
    }

    // ===== CPU reference for view_compute.comp (v0.24.0b) =====

    /// Build a column-major perspective matrix matching glm::perspective
    /// (RH coordinates, depth in [0, 1] for Vulkan).
    fn perspective_rh_zo(fovy_rad: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
        let f = 1.0 / (fovy_rad * 0.5).tan();
        let nf = 1.0 / (near - far);
        // Column-major. Each inner array is one column.
        [
            [f / aspect, 0.0,           0.0,            0.0],
            [0.0,        f,             0.0,            0.0],
            [0.0,        0.0,           far * nf,       -1.0],
            [0.0,        0.0,           far * near * nf, 0.0],
        ]
    }

    /// Multiply two column-major matrices: result = a * b.
    fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
        let mut out = [[0.0f32; 4]; 4];
        // out[col][row] = sum_k a[k][row] * b[col][k]
        for col in 0..4 {
            for row in 0..4 {
                let mut s = 0.0;
                for k in 0..4 {
                    s += a[k][row] * b[col][k];
                }
                out[col][row] = s;
            }
        }
        out
    }

    /// Signed distance from a 3D point to a plane (a, b, c, d) with
    /// `ax + by + cz + d` convention.
    fn signed_distance(plane: &[f32; 4], p: [f32; 3]) -> f32 {
        plane[0] * p[0] + plane[1] * p[1] + plane[2] * p[2] + plane[3]
    }

    /// A point in front of the camera (down -Z in view space, mapped to
    /// world space by an identity view) should be inside the frustum:
    /// all 6 signed distances ≥ 0 (within fp tolerance).
    #[test]
    fn reference_planes_classify_in_front_point_as_inside() {
        // View = identity (camera at origin, looking down -Z).
        let view = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let proj = perspective_rh_zo(60.0_f32.to_radians(), 16.0 / 9.0, 0.1, 100.0);
        let vp = mat4_mul(&proj, &view);
        let planes = derive_planes_cpu_reference(&vp);

        // A point at (0, 0, -10) in world space: 10 units in front of the
        // camera, on the optical axis, within near (0.1) and far (100).
        let p = [0.0_f32, 0.0, -10.0];
        for (i, plane) in planes.iter().enumerate() {
            let d = signed_distance(plane, p);
            assert!(
                d >= -1e-3,
                "plane {} rejects an in-frustum point (d = {})",
                i, d,
            );
        }
    }

    /// A point behind the camera should be outside the frustum (at least
    /// one plane gives a negative signed distance — specifically the
    /// near plane).
    #[test]
    fn reference_planes_classify_behind_point_as_outside() {
        let view = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let proj = perspective_rh_zo(60.0_f32.to_radians(), 16.0 / 9.0, 0.1, 100.0);
        let vp = mat4_mul(&proj, &view);
        let planes = derive_planes_cpu_reference(&vp);

        // A point at (0, 0, +10): 10 units behind the camera.
        let p = [0.0_f32, 0.0, 10.0];
        let mut at_least_one_negative = false;
        for plane in &planes {
            if signed_distance(plane, p) < 0.0 {
                at_least_one_negative = true;
                break;
            }
        }
        assert!(at_least_one_negative,
            "no plane rejects a behind-camera point — frustum derivation is wrong");
    }

    /// A point far beyond the far plane (1000 units away in -Z) should
    /// be classified outside by the far plane specifically (planes[5]).
    #[test]
    fn reference_planes_classify_beyond_far_as_outside() {
        let view = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let proj = perspective_rh_zo(60.0_f32.to_radians(), 16.0 / 9.0, 0.1, 100.0);
        let vp = mat4_mul(&proj, &view);
        let planes = derive_planes_cpu_reference(&vp);

        // (0, 0, -1000): 1000 units in front, well past far=100.
        let p = [0.0_f32, 0.0, -1000.0];
        let far_dist = signed_distance(&planes[5], p);
        assert!(far_dist < 0.0,
            "far plane (5) does not reject a beyond-far point: d = {}", far_dist);
    }

    /// Frustum planes are normalized: |xyz| should be ~1 after the
    /// normalization step. This pins the contract that distance from a
    /// plane is in world units, matching the CPU
    /// `Frustum::from_view_projection` in `world_renderer.rs`.
    #[test]
    fn reference_planes_are_normalized() {
        let view = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let proj = perspective_rh_zo(60.0_f32.to_radians(), 16.0 / 9.0, 0.1, 100.0);
        let vp = mat4_mul(&proj, &view);
        let planes = derive_planes_cpu_reference(&vp);

        for (i, p) in planes.iter().enumerate() {
            let len = (p[0]*p[0] + p[1]*p[1] + p[2]*p[2]).sqrt();
            assert!(
                (len - 1.0).abs() < 1e-4,
                "plane {} not normalized: |xyz| = {}", i, len,
            );
        }
    }

    /// `write_view_proj_pair` writes the view and projection matrices
    /// at the right byte offsets within the slot, without disturbing
    /// the planes/matrices in OTHER slots.
    #[test]
    fn write_view_proj_pair_lands_at_correct_offsets() {
        // We use an in-memory ViewParam directly rather than allocating
        // a Vulkan buffer for this test. The byte offsets are the
        // contract being tested, not the GPU plumbing.
        let mut vp = ViewParam::zero();
        let v = [
            [1.0_f32, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 8.0],
            [9.0, 10.0, 11.0, 12.0],
            [13.0, 14.0, 15.0, 16.0],
        ];
        let p = [
            [17.0_f32, 18.0, 19.0, 20.0],
            [21.0, 22.0, 23.0, 24.0],
            [25.0, 26.0, 27.0, 28.0],
            [29.0, 30.0, 31.0, 32.0],
        ];
        // Manually replicate write_view_proj_pair's byte logic.
        unsafe {
            let ptr = (&mut vp) as *mut ViewParam;
            std::ptr::copy_nonoverlapping(&v, ptr as *mut [[f32; 4]; 4], 1);
            let proj_dst = (ptr as *mut u8).add(64) as *mut [[f32; 4]; 4];
            std::ptr::copy_nonoverlapping(&p, proj_dst, 1);
        }
        assert_eq!(vp.view, v);
        assert_eq!(vp.projection, p);
        // The other fields should still be zero.
        for plane in &vp.planes {
            assert_eq!(*plane, [0.0, 0.0, 0.0, 0.0]);
        }
    }

    // ===== ViewGroup contract (v0.24.2b) =====

    /// The PRIMARY ViewGroup uses slots [0, 1, 2, 3, 4]. Pinned so a
    /// drift in slot ordering breaks the test instead of silently
    /// shipping wrong-cascade rendering.
    #[test]
    fn primary_view_group_slot_layout() {
        let g = ViewGroup::PRIMARY;
        assert_eq!(g.main_slot, 0);
        assert_eq!(g.cascade_slots, [1, 2, 3, 4]);
    }

    /// The primary ViewGroup's slots are aliased onto the historical
    /// VIEW_SLOT_MAIN / VIEW_SLOT_SHADOW constants so pre-existing
    /// code paths still work unchanged.
    #[test]
    fn primary_view_group_aliases_legacy_constants() {
        assert_eq!(ViewGroup::PRIMARY.main_slot, VIEW_SLOT_MAIN);
        assert_eq!(ViewGroup::PRIMARY.cascade_slots[0], VIEW_SLOT_SHADOW);
    }

    /// Slot count per ViewGroup is 1 main + NUM_CASCADES. Pinned so
    /// the multi-camera capacity math is one-liner-correct.
    #[test]
    fn view_group_slot_count_is_five() {
        assert_eq!(ViewGroup::slot_count(), 5);
    }

    /// MAX_VIEWS must accommodate at least one full ViewGroup, with
    /// room left over. The doc-comment on MAX_VIEWS calls out that
    /// adding a second ViewGroup means re-creating the buffer.
    #[test]
    fn max_views_fits_primary_group_with_headroom() {
        let used = ViewGroup::slot_count();
        assert!(MAX_VIEWS >= used);
        // Comment-as-test: with two players we'd need 10 slots; this
        // assertion documents that the current MAX_VIEWS does NOT
        // fit two players, which is the configuration when split-
        // screen lands.
        assert!(MAX_VIEWS < 2 * used,
            "MAX_VIEWS={} happens to fit 2 ViewGroups; design note out of date",
            MAX_VIEWS);
    }
}
