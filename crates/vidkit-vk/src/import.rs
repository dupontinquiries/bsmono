//! DMA-BUF -> VkImage -> wgpu::Texture.
//!
//! *** NOT COMPILED. *** No Vulkan loader or `ash` in the environment this was
//! written in. Treat every line as a reviewed proposal, not working code. The
//! extension names, struct chains and ordering constraints below are the parts
//! I am confident about; the exact ash/wgpu-hal call signatures are the parts
//! most likely to need adjusting against whatever versions you pin.
//!
//! # Required device extensions
//! - `VK_KHR_external_memory_fd`
//! - `VK_EXT_external_memory_dma_buf`
//! - `VK_EXT_image_drm_format_modifier`   <- the one people forget
//! - `VK_KHR_sampler_ycbcr_conversion` is **not** needed; see convert.wgsl.
//!
//! # Gotchas, in the order they will bite
//!
//! 1. **Never assume DRM_FORMAT_MOD_LINEAR.** V4L2 and VAAPI hand back tiled
//!    buffers with a real modifier. Query it (`VIDIOC_G_FMT` +
//!    `DRM_IOCTL_...`, or `vaExportSurfaceHandle`'s `objects[i].drm_format_modifier`)
//!    and pass it in `VkImageDrmFormatModifierExplicitCreateInfoEXT`. Guessing
//!    linear gives a black or smeared frame with no validation error on some
//!    drivers.
//!
//! 2. **Multi-plane imports need `VK_IMAGE_CREATE_DISJOINT_BIT`** and one
//!    `vkBindImageMemory2` call per plane with
//!    `VkBindImagePlaneMemoryInfo.planeAspect` set to
//!    `VK_IMAGE_ASPECT_MEMORY_PLANE_{0,1,2}_BIT_EXT` — the *memory* plane bits,
//!    not `VK_IMAGE_ASPECT_PLANE_n_BIT`. Mixing those up is the classic error.
//!    Alternatively import each plane as its own single-plane R8/R8G8/R16
//!    image, which is what this module does: simpler, portable, and it is what
//!    the convert shader wants anyway.
//!
//! 3. **`drmFormatModifierPlaneCount` must match** the number of fds/offsets
//!    you supply, and the offsets/strides go in
//!    `VkSubresourceLayout` entries in plane order.
//!
//! 4. **Ownership.** `vkImportMemoryFdInfoKHR` *takes* the fd — Vulkan closes
//!    it when the memory is freed. Do not also close it yourself, and do not
//!    reuse it. If you need it twice, `dup()`.
//!
//! 5. **wgpu import path.** `wgpu_hal::vulkan::Device::texture_from_raw` is the
//!    only way in. It does **not** take ownership: wgpu will not free the
//!    VkImage or VkDeviceMemory, so the drop callback / your own recycler must.
//!    Version churn here is real — `Instance::as_hal` changed from a returned
//!    `Option<&A::Instance>` to a closure form in wgpu 0.20, and the
//!    `hal::TextureDescriptor` gained/renamed fields again around 22/23. Pin
//!    wgpu and re-check this function on every bump; it will not fail to
//!    compile gracefully.
//!
//! 6. **Queue family ownership.** Frames produced by V4L2 or VAAPI are
//!    `VK_QUEUE_FAMILY_FOREIGN_EXT`. Acquire with an image memory barrier
//!    from `srcQueueFamilyIndex = VK_QUEUE_FAMILY_FOREIGN_EXT` and
//!    `oldLayout = VK_IMAGE_LAYOUT_UNDEFINED` before the compute pass, and
//!    release back after. Skipping this works on RADV and then corrupts on
//!    turnip.
//!
//! 7. **Synchronisation.** V4L2 gives you a fence fd per buffer
//!    (`V4L2_BUF_FLAG_IN_FENCE`/out-fence) or, more commonly on Venus, nothing
//!    at all — in which case the DQBUF completion *is* the sync point and a
//!    plain barrier suffices. VAAPI needs `vaSyncSurface` before export unless
//!    you use the explicit-sync extensions. Do not import a
//!    `VK_KHR_external_semaphore_fd` you did not actually get.

use std::os::unix::io::OwnedFd;

/// DRM fourccs we import. Everything else is a bug in format selection.
pub mod fourcc {
    pub const NV12: u32 = fourcc_code(b'N', b'V', b'1', b'2');
    pub const P010: u32 = fourcc_code(b'P', b'0', b'1', b'0');
    pub const YUV420: u32 = fourcc_code(b'Y', b'U', b'1', b'2');
    pub const P210: u32 = fourcc_code(b'P', b'2', b'1', b'0');

    pub const fn fourcc_code(a: u8, b: u8, c: u8, d: u8) -> u32 {
        (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
    }
}

/// One imported plane: a single-plane VkImage viewed as R8/R8G8/R16/R16G16.
pub struct ImportedPlane {
    // vk::Image, vk::DeviceMemory, vk::ImageView, plus the wgpu::Texture that
    // wraps it. Kept together so the recycler frees them as a unit.
    pub width: u32,
    pub height: u32,
}

pub struct ImportedFrame {
    pub planes: Vec<ImportedPlane>,
}

/// Map DRM fourcc + plane index to the Vulkan format we view that plane as.
///
/// The rule: luma planes become single-channel, interleaved chroma becomes
/// two-channel, and bit depth picks 8 vs 16. P010 stores 10 bits in the *high*
/// bits of each 16-bit sample, so R16_UNORM reads it back pre-scaled and the
/// shader needs no correction — which is exactly why P010 is worth preferring
/// over I420_10LE when the decoder offers a choice.
pub fn plane_format_name(fourcc_val: u32, plane: usize) -> Option<&'static str> {
    match (fourcc_val, plane) {
        (fourcc::NV12, 0) => Some("R8_UNORM"),
        (fourcc::NV12, 1) => Some("R8G8_UNORM"),
        (fourcc::P010, 0) => Some("R16_UNORM"),
        (fourcc::P010, 1) => Some("R16G16_UNORM"),
        (fourcc::P210, 0) => Some("R16_UNORM"),
        (fourcc::P210, 1) => Some("R16G16_UNORM"),
        (fourcc::YUV420, 0..=2) => Some("R8_UNORM"),
        _ => None,
    }
}

/// Chroma plane dimensions for a given fourcc.
pub fn chroma_dims(fourcc_val: u32, w: u32, h: u32) -> (u32, u32) {
    match fourcc_val {
        // 4:2:0 — half in both axes. Round up: odd dimensions are legal and
        // truncating loses the last row/column of chroma.
        fourcc::NV12 | fourcc::P010 | fourcc::YUV420 => ((w + 1) / 2, (h + 1) / 2),
        // 4:2:2 — half horizontally only.
        fourcc::P210 => ((w + 1) / 2, h),
        _ => (w, h),
    }
}

pub struct PlaneSource {
    pub fd: OwnedFd,
    pub offset: u64,
    pub stride: u64,
}

/// The call this module exists to provide.
///
/// Pseudocode of the body, which is the part worth reviewing:
///
/// ```text
/// for (i, plane) in planes.iter().enumerate():
///     fmt = plane_format_name(fourcc, i)
///     (pw, ph) = i == 0 ? (w, h) : chroma_dims(fourcc, w, h)
///
///     // 1. describe the external memory + modifier up front
///     ext_mem   = VkExternalMemoryImageCreateInfo {
///                     handleTypes: DMA_BUF_BIT_EXT }
///     layout    = VkSubresourceLayout { offset, rowPitch: stride, .. }
///     mod_info  = VkImageDrmFormatModifierExplicitCreateInfoEXT {
///                     drmFormatModifier: modifier,
///                     drmFormatModifierPlaneCount: 1,
///                     pPlaneLayouts: &layout }
///                 .push_next(ext_mem)
///
///     // 2. tiling MUST be DRM_FORMAT_MODIFIER_EXT, not OPTIMAL
///     image = vkCreateImage(VkImageCreateInfo {
///                 tiling: DRM_FORMAT_MODIFIER_EXT,
///                 usage:  SAMPLED,
///                 initialLayout: UNDEFINED,   // required for foreign import
///                 .. }.push_next(mod_info))
///
///     // 3. import the fd; Vulkan now owns it
///     import  = VkImportMemoryFdInfoKHR { handleType: DMA_BUF_BIT_EXT, fd }
///     ded     = VkMemoryDedicatedAllocateInfo { image }   // required in practice
///     req     = vkGetImageMemoryRequirements2(image)
///     type_ix = pick memory type from
///               vkGetMemoryFdPropertiesKHR(DMA_BUF, fd).memoryTypeBits
///               & req.memoryTypeBits          // <- intersect BOTH
///     mem     = vkAllocateMemory(.., import, ded)
///     vkBindImageMemory2(image, mem, 0)
///
///     // 4. hand to wgpu without transferring ownership
///     tex = wgpu_hal::vulkan::Device::texture_from_raw(
///               image, &hal_desc, /* drop_callback */ None)
/// ```
///
/// The `memoryTypeBits` intersection in step 3 is the one silent-failure
/// point: using only the image's requirements picks a type the dma-buf cannot
/// live in, and `vkAllocateMemory` returns `ERROR_INVALID_EXTERNAL_HANDLE`
/// with nothing else to go on.
pub fn import_dmabuf_frame(
    _planes: Vec<PlaneSource>,
    _fourcc_val: u32,
    _modifier: u64,
    _width: u32,
    _height: u32,
) -> Result<ImportedFrame, String> {
    Err("not implemented — see module docs".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_planes_map_to_r8_then_rg8() {
        assert_eq!(plane_format_name(fourcc::NV12, 0), Some("R8_UNORM"));
        assert_eq!(plane_format_name(fourcc::NV12, 1), Some("R8G8_UNORM"));
        assert_eq!(plane_format_name(fourcc::NV12, 2), None);
    }

    #[test]
    fn odd_dimensions_round_chroma_up() {
        assert_eq!(chroma_dims(fourcc::NV12, 1921, 1081), (961, 541));
    }

    #[test]
    fn fourcc_encoding_matches_drm() {
        // DRM_FORMAT_NV12 is 0x3231564e.
        assert_eq!(fourcc::NV12, 0x3231_564e);
    }
}
