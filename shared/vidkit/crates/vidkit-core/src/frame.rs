//! The one frame type every decoder produces and the presenter consumes.
//!
//! The seam that matters: a decoded frame is either a set of DMA-BUF fds the
//! GPU can import with zero copies, or CPU planes that need a staging upload.
//! Everything downstream — the convert pass, the frame queue, the UI — is
//! written against `Frame` and does not care which.

use crate::codec::Transfer;
use std::os::unix::io::OwnedFd;
use std::time::Duration;

/// Colour matrix. Separate from `Transfer` because streams mix them freely.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Matrix {
    Bt601,
    Bt709,
    Bt2020Ncl,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Range {
    /// 16..235 luma. The default for basically all broadcast-lineage video.
    Limited,
    Full,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct ColorInfo {
    pub matrix: Matrix,
    pub transfer: Transfer,
    pub range: Range,
}

impl Default for ColorInfo {
    fn default() -> Self {
        Self {
            matrix: Matrix::Bt709,
            transfer: Transfer::Sdr,
            range: Range::Limited,
        }
    }
}

/// Layout of the plane set. Only the ones we actually import.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum PlaneLayout {
    /// Y plane + interleaved CbCr. 8 bit.
    Nv12,
    /// Y + interleaved CbCr, 10 bit in the high bits of 16.
    P010,
    /// Three separate 8-bit planes.
    I420,
    /// Three separate 16-bit planes, 10 significant bits.
    I420_10le,
}

impl PlaneLayout {
    pub fn plane_count(self) -> usize {
        match self {
            PlaneLayout::Nv12 | PlaneLayout::P010 => 2,
            PlaneLayout::I420 | PlaneLayout::I420_10le => 3,
        }
    }

    pub fn bytes_per_sample(self) -> usize {
        match self {
            PlaneLayout::Nv12 | PlaneLayout::I420 => 1,
            PlaneLayout::P010 | PlaneLayout::I420_10le => 2,
        }
    }
}

#[derive(Debug)]
pub struct DmaBufPlane {
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

#[derive(Debug)]
pub struct DmaBufImage {
    pub planes: Vec<DmaBufPlane>,
    /// DRM format modifier. **Must** be queried and passed through to
    /// `VkImageDrmFormatModifierExplicitCreateInfoEXT`; assuming LINEAR is the
    /// classic way to get a garbled or black frame on tiled hardware.
    pub modifier: u64,
    /// DRM fourcc (`DRM_FORMAT_NV12` etc). Kept raw so the Vulkan layer owns
    /// the fourcc -> VkFormat mapping.
    pub fourcc: u32,
}

#[derive(Debug)]
pub struct CpuPlanes {
    pub data: Vec<Vec<u8>>,
    pub strides: Vec<u32>,
}

#[derive(Debug)]
pub enum FrameData {
    DmaBuf(DmaBufImage),
    Cpu(CpuPlanes),
}

#[derive(Debug)]
pub struct Frame {
    pub data: FrameData,
    pub width: u32,
    pub height: u32,
    pub layout: PlaneLayout,
    pub color: ColorInfo,
    /// Presentation timestamp on the stream timeline.
    pub pts: Duration,
    /// Nominal duration; used to decide whether to hold or drop under pressure.
    pub duration: Option<Duration>,
    /// Opaque handle back to whatever owns the underlying buffer.
    ///
    /// This exists because hardware decoders hand out a *fixed, small pool* of
    /// buffers: V4L2 gives you N capture buffers and the decoder stalls until
    /// you requeue them. So a frame is a borrow from the decoder, not a gift,
    /// and whoever finishes with it must call `Decoder::recycle` with this
    /// token. `None` means the frame owns its memory outright (software decode
    /// into a fresh allocation) and nothing needs returning.
    pub recycle: Option<RecycleToken>,
    /// AV1 film grain the decoder did NOT apply, for the presenter to
    /// synthesise (on the GPU) at source resolution. `None`: the picture is
    /// final -- no grain, or the decoder already applied it.
    pub grain: Option<std::sync::Arc<crate::av1_grain::GrainParams>>,
}

/// Index into a decoder's buffer pool. Deliberately not `Copy`: it should be
/// moved back to the decoder exactly once, and a `Copy` token invites
/// double-requeue, which V4L2 answers with `EINVAL` at best and a corrupt
/// frame at worst.
#[derive(PartialEq, Eq, Debug)]
pub struct RecycleToken(pub u32);

impl Frame {
    pub fn is_zero_copy(&self) -> bool {
        matches!(self.data, FrameData::DmaBuf(_))
    }

    /// Bytes we would have to push over the bus if this frame needs a staging
    /// upload. Used by the scheduler to decide whether a CPU path can keep up.
    pub fn upload_cost_bytes(&self) -> usize {
        match &self.data {
            FrameData::DmaBuf(_) => 0,
            FrameData::Cpu(p) => p.data.iter().map(|v| v.len()).sum(),
        }
    }
}
