//! The V4L2 M2M ioctl surface, behind a trait.
//!
//! Every ioctl the stateful decoder needs, and nothing else. The point of the
//! trait is that the *state machine* — which is where all the bugs live — can
//! be driven by a mock in unit tests, on a machine with no `/dev/video*` at
//! all. The real implementation is a thin `nix::ioctl_*` wrapper with no logic
//! in it.

use std::time::Duration;
use vidkit_core::frame::FrameData;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Queue {
    /// `V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE` — compressed bitstream goes in.
    Output,
    /// `V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE` — decoded frames come out.
    Capture,
}

/// V4L2 pixel format fourccs for the codecs we feed in.
pub mod pixfmt {
    pub const fn code(a: u8, b: u8, c: u8, d: u8) -> u32 {
        (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
    }
    pub const H264: u32 = code(b'H', b'2', b'6', b'4');
    pub const HEVC: u32 = code(b'H', b'E', b'V', b'C');
    pub const VP8: u32 = code(b'V', b'P', b'8', b'0');
    pub const VP9: u32 = code(b'V', b'P', b'9', b'0');
    /// `V4L2_PIX_FMT_AV1_FRAME` — note the `F`. This is the **stateless**
    /// (Request API) AV1 format; there is no stateful AV1 fourcc, because no
    /// shipping stateful decoder does AV1. A Venus-class device will therefore
    /// never enumerate this, which is the correct outcome rather than a gap:
    /// AV1 on the Fairphone goes to dav1d.
    pub const AV1_FRAME: u32 = code(b'A', b'V', b'1', b'F');
    // Raw output formats.
    pub const NV12: u32 = code(b'N', b'V', b'1', b'2');
    pub const P010: u32 = code(b'P', b'0', b'1', b'0');
    pub const YUV420M: u32 = code(b'Y', b'M', b'1', b'2');
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct CaptureFormat {
    pub width: u32,
    pub height: u32,
    pub fourcc: u32,
    pub modifier: u64,
}

#[derive(Debug)]
pub struct DequeuedCapture {
    pub index: u32,
    pub data: FrameData,
    pub pts: Duration,
    /// `V4L2_BUF_FLAG_LAST`. Terminates a drain or a resolution change; missing
    /// it means hanging forever waiting for a frame that will never come, so
    /// every drain loop needs a timeout as a backstop.
    pub last: bool,
    /// `V4L2_BUF_FLAG_ERROR`. The buffer is garbage but must still be requeued.
    pub error: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Event {
    /// `V4L2_EVENT_SOURCE_CHANGE` with `V4L2_EVENT_SRC_CH_RESOLUTION`.
    SourceChange,
    /// `V4L2_EVENT_EOS`. Advisory — the `LAST` buffer flag is authoritative.
    Eos,
}

#[derive(Debug)]
pub enum ApiError {
    /// `EAGAIN`. Not an error: nothing ready right now.
    WouldBlock,
    /// `EPIPE` on `dequeue_output` after a drain. Also not really an error.
    Drained,
    Ioctl(&'static str, i32),
    Unsupported(&'static str),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::WouldBlock => write!(f, "would block"),
            ApiError::Drained => write!(f, "drained"),
            ApiError::Ioctl(n, e) => write!(f, "{n} failed: errno {e}"),
            ApiError::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// One decoder device.
///
/// Ordering constraints the implementation may assume the caller respects, and
/// which [`crate::stateful`] enforces:
///
/// - `set_output_format` before any `request_buffers`.
/// - `stream_on(Output)` before the first `queue_output`.
/// - **No `request_buffers(Capture, _)` or `stream_on(Capture)` until after the
///   first [`Event::SourceChange`].** The driver does not know the frame size
///   before it has parsed headers, and asking early gets you either `EINVAL`
///   or, worse, a correctly-sized-by-accident allocation that breaks on the
///   next stream.
pub trait V4l2Api {
    fn set_output_format(&mut self, fourcc: u32, width: u32, height: u32) -> ApiResult<()>;

    /// Returns the count actually allocated, which may exceed the request.
    fn request_buffers(&mut self, queue: Queue, count: u32) -> ApiResult<u32>;

    fn stream_on(&mut self, queue: Queue) -> ApiResult<()>;
    fn stream_off(&mut self, queue: Queue) -> ApiResult<()>;

    fn capture_format(&mut self) -> ApiResult<CaptureFormat>;

    /// `V4L2_CID_MIN_BUFFERS_FOR_CAPTURE`. Allocating exactly this many is a
    /// trap: it is the decoder's *reference frame* requirement and leaves zero
    /// spare for frames the presenter is holding, so playback stalls one frame
    /// in. Always add headroom.
    fn min_capture_buffers(&mut self) -> ApiResult<u32>;

    /// `Ok(false)` means no free OUTPUT buffer — back-pressure, not an error.
    fn queue_output(&mut self, data: &[u8], pts: Option<Duration>) -> ApiResult<bool>;

    /// Reclaim a consumed OUTPUT buffer. `Ok(false)` when none is ready.
    fn dequeue_output(&mut self) -> ApiResult<bool>;

    fn queue_capture(&mut self, index: u32) -> ApiResult<()>;
    fn dequeue_capture(&mut self) -> ApiResult<Option<DequeuedCapture>>;

    fn dequeue_event(&mut self) -> ApiResult<Option<Event>>;

    /// `V4L2_DEC_CMD_STOP`. Begins a drain.
    fn decoder_cmd_stop(&mut self) -> ApiResult<()>;
    /// `V4L2_DEC_CMD_START`. Resumes after a completed drain.
    fn decoder_cmd_start(&mut self) -> ApiResult<()>;
}
